//! Shared harness for the router tests: `tower::ServiceExt::oneshot`, no sockets.
//! Pattern: https://github.com/tokio-rs/axum/tree/main/examples/testing
#![allow(dead_code)]

use axum::{
    body::Body,
    http::{header, HeaderMap, Method, Request, StatusCode},
    Router,
};
use fitness_core::AmFitnessStore;
use http_body_util::BodyExt;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use shopping_core::AmShoppingStore;
use todo_core::AmTodoStore;
use tower::ServiceExt;
use web::{AppState, Events, OneOrMany, WebConfig};

pub struct App {
    pub router: Router,
    pub saves: Arc<AtomicUsize>,
    pub todo: Arc<Mutex<AmTodoStore>>,
    pub fit: Arc<Mutex<AmFitnessStore>>,
    pub shopping: Arc<Mutex<AmShoppingStore>>,
    pub events: Events,
}

pub fn app(web: WebConfig, dev_user: Option<&str>) -> App {
    app_with(web, dev_user, |s| s)
}

/// Like `app`, with a chance to change the state (e.g. `with_assistant`) before the router is built.
pub fn app_with(web: WebConfig, dev_user: Option<&str>, f: impl FnOnce(AppState) -> AppState) -> App {
    let todo = Arc::new(Mutex::new(AmTodoStore::new()));
    let fit = Arc::new(Mutex::new(AmFitnessStore::new()));
    let shopping = Arc::new(Mutex::new(AmShoppingStore::new()));
    let events = Events::new();
    let saves = Arc::new(AtomicUsize::new(0));
    let s = saves.clone();
    let state = AppState::new(
        todo.clone(),
        fit.clone(),
        shopping.clone(),
        events.clone(),
        Arc::new(move |_| {
            s.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
        web,
        "alice",
        dev_user.map(Into::into),
    )
    .unwrap()
    // 2026-10-11 14:30 UTC = 2026-10-12 01:30 in Sydney (AEDT, +11).
    .with_clock(|| "2026-10-11T14:30:00Z".parse().unwrap());
    let state = f(state);
    App { router: web::router(state), saves, todo, fit, shopping, events }
}

pub fn dev() -> App {
    app(WebConfig { timezone: "Australia/Sydney".into(), ..Default::default() }, Some("alice"))
}

pub fn users_config() -> WebConfig {
    WebConfig {
        users: HashMap::from([
            ("alice@example.com".to_string(), OneOrMany::One("alice".into())),
            ("bob@example.com".to_string(), OneOrMany::One("bob".into())),
            ("Zoë@example.com".to_string(), OneOrMany::One("zoe".into())),
            ("shared@example.com".to_string(), OneOrMany::Many(vec!["alice".into(), "bob".into()])),
        ]),
        ..Default::default()
    }
}

impl App {
    pub async fn req(&self, method: Method, uri: &str, body: Option<Value>, headers: &[(&str, &str)]) -> (StatusCode, Value, HeaderMap) {
        let mut b = Request::builder().method(method.clone()).uri(uri);
        if method != Method::GET && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
            b = b.header(header::CONTENT_TYPE, "application/json");
        }
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let body = body.map_or(Body::empty(), |v| Body::from(v.to_string()));
        let res = self.router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into())) };
        (status, v, headers)
    }
    pub async fn get(&self, uri: &str) -> (StatusCode, Value) {
        let (s, v, _) = self.req(Method::GET, uri, None, &[]).await;
        (s, v)
    }
    pub async fn put(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        let (s, v, _) = self.req(Method::PUT, uri, Some(body), &[]).await;
        (s, v)
    }
    pub async fn send(&self, m: Method, uri: &str, body: Value) -> (StatusCode, Value) {
        let (s, v, _) = self.req(m, uri, Some(body), &[]).await;
        (s, v)
    }
    pub async fn as_user(&self, login: &str, m: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (s, v, _) = self.req(m, uri, body, &[("Tailscale-User-Login", login)]).await;
        (s, v)
    }
}

pub const P: Method = Method::PATCH;
pub const POST: Method = Method::POST;
