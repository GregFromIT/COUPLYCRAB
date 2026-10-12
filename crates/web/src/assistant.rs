//! `/assistant/*`: chat with the read-only assistant. It runs inside this process, so it
//! inherits the identity and CSRF protection of every other endpoint, and its tools read
//! the same stores the other endpoints do (`assistant-core`). See docs/API.md.

use crate::{
    identity::Me,
    ApiError, ApiResult, AppState, Body,
};
use assistant_core::{Agent, AgentError, Answer, ChatTurn, Context, LlmClient, StoreTools};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;

const MAX_MESSAGE: usize = 2000;
const MAX_HISTORY: usize = 40;
/// Chats that may run at once: a model host answers one thing at a time, so more would only queue.
const CONCURRENT: usize = 2;

pub(crate) struct Handle {
    agent: Agent,
    model: String,
    fake: bool,
    busy: Semaphore,
}

impl Handle {
    pub(crate) fn new(state_stores: &crate::Inner, llm: Arc<dyn LlmClient>, model: &str, fake: bool) -> Handle {
        let tools = StoreTools { fit: state_stores.fit.clone(), todo: state_stores.todo.clone(), shopping: state_stores.shopping.clone() };
        Handle { agent: Agent::new(llm, Arc::new(tools), model), model: model.into(), fake, busy: Semaphore::new(CONCURRENT) }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/assistant/status", get(status)).route("/assistant/chat", post(chat))
}

#[derive(Serialize)]
struct Status {
    configured: bool,
    model: Option<String>,
    /// The built-in keyword demo, not a language model (`peer serve --fake-llm`).
    fake: bool,
}

async fn status(State(st): State<AppState>, _me: Me) -> Json<Status> {
    let h = st.0.assistant.as_ref();
    Json(Status { configured: h.is_some(), model: h.map(|h| h.model.clone()), fake: h.is_some_and(|h| h.fake) })
}

#[derive(Deserialize)]
struct ChatIn {
    message: String,
    /// Earlier turns, kept by the browser (nothing is stored here).
    #[serde(default)]
    history: Vec<ChatTurn>,
}

fn from_agent(e: AgentError) -> ApiError {
    match e {
        AgentError::Offline(_) => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "assistant_offline", format!("{e}. Is Ollama running? See docs/BLOCKERS.md.")),
        AgentError::Timeout => ApiError::new(StatusCode::GATEWAY_TIMEOUT, "assistant_timeout", e.to_string()),
        AgentError::Bad(_) => ApiError::new(StatusCode::BAD_GATEWAY, "assistant_error", e.to_string()),
    }
}

async fn chat(State(st): State<AppState>, Me(me): Me, Body(b): Body<ChatIn>) -> ApiResult<Json<Answer>> {
    let Some(h) = st.0.assistant.as_ref() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "assistant_not_configured",
            "The assistant is not set up: add an [assistant] section (url, model) to config.toml, or start with --fake-llm to try the screen.",
        ));
    };
    let message = b.message.trim();
    if message.is_empty() {
        return Err(ApiError::bad("message is required"));
    }
    if message.chars().count() > MAX_MESSAGE {
        return Err(ApiError::bad(format!("message is longer than {MAX_MESSAGE} characters")));
    }
    if b.history.len() > MAX_HISTORY {
        return Err(ApiError::bad(format!("history has more than {MAX_HISTORY} turns")));
    }
    let _permit = h.busy.try_acquire().map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "assistant_busy", "The assistant is already answering other questions. Try again in a moment."))?;
    let ctx = Context { user_id: me, today: st.today(), tz: st.0.tz };
    Ok(Json(h.agent.ask(&ctx, &b.history, message).await.map_err(from_agent)?))
}
