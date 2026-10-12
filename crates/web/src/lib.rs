//! HTTP layer: JSON API under `/api/v1` (+ the embedded frontend, see `assets`).
//!
//! Every write: validate → mutate store → `AppState::saved` (atomic save + sync
//! trigger, provided by the caller) → respond. See docs/API.md.

mod api;
mod assets;
mod assistant;
mod events;
mod identity;
mod shopping;
mod training;

pub use events::Events;
pub use identity::decode_rfc2047;

use axum::{
    extract::{DefaultBodyLimit, FromRequest, Request, State},
    http::{header, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use chrono::{DateTime, Utc};
use fitness_core::FitnessStore;
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use shopping_core::ShoppingStore;
use todo_core::TodoStore;

pub const BODY_LIMIT: usize = 64 * 1024;

/// `[web]` section of `config.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Must be loopback: identity headers are only trustworthy if nothing but
    /// `tailscale serve` can reach this port.
    pub listen: SocketAddr,
    /// IANA name, e.g. `Australia/Sydney`; "today" for `planned-day` etc.
    pub timezone: String,
    /// Tailscale login → user ID, or → list of user IDs for a shared account
    /// (then the user picker chooses among them).
    pub users: HashMap<String, OneOrMany>,
    /// Allowed `Origin`s for writes. Default: `http://127.0.0.1:<port>` and
    /// `http://localhost:<port>`. Add `https://<host>.<tailnet>.ts.net`.
    pub origins: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn ids(&self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        WebConfig {
            listen: SocketAddr::from(([127, 0, 0, 1], 7430)),
            timezone: "UTC".into(),
            users: HashMap::new(),
            origins: vec![],
        }
    }
}

impl WebConfig {
    pub fn allowed_origins(&self) -> Vec<String> {
        if !self.origins.is_empty() {
            return self.origins.clone();
        }
        let port = self.listen.port();
        vec![format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Todo,
    Fitness,
    Shopping,
}

impl DocKind {
    pub const ALL: [DocKind; 3] = [DocKind::Todo, DocKind::Fitness, DocKind::Shopping];

    /// The name used in `/events` messages.
    pub fn name(self) -> &'static str {
        match self {
            DocKind::Todo => "todo",
            DocKind::Fitness => "fitness",
            DocKind::Shopping => "shopping",
        }
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

pub type SavedHook = Arc<dyn Fn(DocKind) -> Result<(), String> + Send + Sync>;

/// Shared by all handlers. Stores use `std::sync::Mutex`: the same lock is used
/// by the v0 sync loop and is never held across an `.await`.
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub todo: Arc<Mutex<dyn TodoStore + Send>>,
    pub fit: Arc<Mutex<dyn FitnessStore + Send>>,
    pub shopping: Arc<Mutex<dyn ShoppingStore + Send>>,
    /// Tell it when a doc changed by other means too (a peer merge): see `Events`.
    pub events: Events,
    /// The assistant, if configured (`AppState::with_assistant`).
    pub(crate) assistant: Option<assistant::Handle>,
    /// Called after each successful write: persist the doc and trigger sync.
    pub saved: SavedHook,
    pub web: WebConfig,
    pub tz: chrono_tz::Tz,
    /// Every user ID that may be picked (config `user_id` + all `[web.users]` values).
    pub known_users: Vec<String>,
    /// `--dev-user`: skip identity, everyone is this user.
    pub dev_user: Option<String>,
    pub now: fn() -> DateTime<Utc>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        todo: Arc<Mutex<dyn TodoStore + Send>>,
        fit: Arc<Mutex<dyn FitnessStore + Send>>,
        shopping: Arc<Mutex<dyn ShoppingStore + Send>>,
        events: Events,
        saved: SavedHook,
        web: WebConfig,
        default_user: &str,
        dev_user: Option<String>,
    ) -> Result<Self, String> {
        let tz: chrono_tz::Tz = web.timezone.parse().map_err(|_| format!("unknown timezone {:?}", web.timezone))?;
        if !web.listen.ip().is_loopback() {
            return Err(format!("[web] listen = {}: must be a loopback address (127.0.0.1)", web.listen));
        }
        let mut users: BTreeSet<String> = web.users.values().flat_map(|u| u.ids()).collect();
        users.insert(default_user.to_string());
        users.extend(dev_user.clone());
        Ok(AppState(Arc::new(Inner { todo, fit, shopping, events, assistant: None, saved, web, tz, known_users: users.into_iter().collect(), dev_user, now: Utc::now })))
    }

    /// Turn on `/assistant/chat`, answering with `llm` (`model` is its name; `fake` marks the
    /// built-in keyword demo). Call before cloning the state, like `with_clock`.
    pub fn with_assistant(self, llm: Arc<dyn assistant_core::LlmClient>, model: &str, fake: bool) -> Self {
        let mut inner = Arc::try_unwrap(self.0).unwrap_or_else(|_| panic!("with_assistant before cloning the state"));
        inner.assistant = Some(assistant::Handle::new(&inner, llm, model, fake));
        AppState(Arc::new(inner))
    }

    /// Replace the clock (tests).
    pub fn with_clock(self, now: fn() -> DateTime<Utc>) -> Self {
        let inner = Arc::try_unwrap(self.0).unwrap_or_else(|_| panic!("with_clock before cloning the state"));
        AppState(Arc::new(Inner { now, ..inner }))
    }

    fn programs_exist(&self) -> Result<bool, ApiError> {
        let fit = self.0.fit.lock().map_err(|_| ApiError::internal("store mutex poisoned"))?;
        Ok(!fit.list_programs()?.is_empty())
    }

    fn today(&self) -> chrono::NaiveDate {
        (self.0.now)().with_timezone(&self.0.tz).date_naive()
    }
}

// ---- errors ----

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    /// For 410 Gone: the path that replaces the endpoint.
    replacement: Option<&'static str>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError { status, code, message: message.into(), replacement: None }
    }
    fn gone(message: impl Into<String>, replacement: &'static str) -> Self {
        ApiError { replacement: Some(replacement), ..Self::new(StatusCode::GONE, "gone", message) }
    }
    fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid", message)
    }
    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }
    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "error": { "code": self.code, "message": self.message } });
        if let Some(r) = self.replacement {
            body["replacement"] = r.into();
        }
        (self.status, axum::Json(body)).into_response()
    }
}

impl From<todo_core::Error> for ApiError {
    fn from(e: todo_core::Error) -> Self {
        use todo_core::Error::*;
        match &e {
            NotFound(_) => Self::new(StatusCode::NOT_FOUND, "not_found", e.to_string()),
            Invalid(_) => Self::bad(e.to_string()),
            Conflict(_) => Self::new(StatusCode::CONFLICT, "conflict", e.to_string()),
            Automerge(_) => Self::internal(e.to_string()),
        }
    }
}

impl From<shopping_core::Error> for ApiError {
    fn from(e: shopping_core::Error) -> Self {
        use shopping_core::Error::*;
        match &e {
            NotFound(_) => Self::new(StatusCode::NOT_FOUND, "not_found", e.to_string()),
            Invalid(_) => Self::bad(e.to_string()),
            Conflict(_) => Self::new(StatusCode::CONFLICT, "conflict", e.to_string()),
            Automerge(_) => Self::internal(e.to_string()),
        }
    }
}

impl From<fitness_core::Error> for ApiError {
    fn from(e: fitness_core::Error) -> Self {
        use fitness_core::Error::*;
        match &e {
            NotFound(_) => Self::new(StatusCode::NOT_FOUND, "not_found", e.to_string()),
            NotOwner => Self::forbidden(e.to_string()),
            Invalid(_) => Self::bad(e.to_string()),
            Conflict(_) => Self::new(StatusCode::CONFLICT, "conflict", e.to_string()),
            Automerge(_) => Self::internal(e.to_string()),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// `axum::Json` with rejections in our error format (400 bad JSON, 413, 415).
pub struct Body<T>(pub T);

impl<T: serde::de::DeserializeOwned, S: Send + Sync> FromRequest<S> for Body<T> {
    type Rejection = ApiError;
    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(Body(v)),
            Err(r) => {
                let code = match r.status() {
                    StatusCode::PAYLOAD_TOO_LARGE => "too_large",
                    StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
                    _ => "bad_json",
                };
                Err(ApiError::new(r.status(), code, r.body_text()))
            }
        }
    }
}

// ---- request protection (CSRF via network identity) ----

/// Writes must be `Content-Type: application/json` (a foreign page can't send
/// that without a CORS preflight, which we never answer) and, if an `Origin`
/// header is present, it must be ours. We send no CORS headers at all.
async fn protect(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        let json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json")));
        if !json {
            return ApiError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type", "writes require Content-Type: application/json")
                .into_response();
        }
        if let Some(origin) = req.headers().get(header::ORIGIN) {
            let ok = origin.to_str().is_ok_and(|o| state.0.web.allowed_origins().iter().any(|a| a == o));
            if !ok {
                return ApiError::new(StatusCode::FORBIDDEN, "bad_origin", "cross-origin write rejected").into_response();
            }
        }
    }
    next.run(req).await
}

/// v1 templates, plans and blocks were replaced by programs: once any program exists their
/// endpoints answer 410 Gone and point to `/programs`. A middleware, so it answers before a
/// request body is parsed (an old client with a stale body still gets the 410).
async fn gone_v1(State(state): State<AppState>, req: Request, next: Next) -> Response {
    const GONE: [&str; 5] = ["/templates", "/plan", "/blocks", "/planned-day", "/program-status"];
    let path = req.uri().path().trim_start_matches("/api/v1");
    if GONE.iter().any(|p| path == *p || path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))) {
        match state.programs_exist() {
            Ok(true) => {
                return ApiError::gone("templates, weekly plans and blocks were replaced by programs", "/api/v1/programs").into_response();
            }
            Ok(false) => {}
            Err(e) => return e.into_response(),
        }
    }
    next.run(req).await
}

pub fn router(state: AppState) -> Router {
    let api = api::routes()
        .layer(middleware::from_fn_with_state(state.clone(), gone_v1))
        .layer(middleware::from_fn_with_state(state.clone(), protect))
        .layer(DefaultBodyLimit::max(BODY_LIMIT));
    Router::new()
        .nest("/api/v1", api)
        .fallback(assets::serve)
        .with_state(state)
}

/// Today's date in the IANA zone `tz` (e.g. `Australia/Sydney`), for the CLI.
pub fn today_in(tz: &str) -> Result<chrono::NaiveDate, String> {
    let tz: chrono_tz::Tz = tz.parse().map_err(|_| format!("unknown timezone {tz:?}"))?;
    Ok(Utc::now().with_timezone(&tz).date_naive())
}
