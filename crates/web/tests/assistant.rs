//! `/assistant/status` and `/assistant/chat`, with scripted models (no Ollama needed).

mod common;

use assistant_core::{ChatRequest, ChatResponse, DemoLlm, FakeLlmClient, LlmClient, LlmError};
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn with_llm(llm: Arc<dyn LlmClient>) -> App {
    let web = web::WebConfig { timezone: "Australia/Sydney".into(), ..users_config() };
    app_with(web, None, |s| s.with_assistant(llm, "qwen3:test", false))
}

async fn chat(a: &App, login: &str, body: Value) -> (StatusCode, Value) {
    a.as_user(login, Method::POST, "/api/v1/assistant/chat", Some(body)).await
}

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";

#[tokio::test]
async fn not_configured_says_how_to_set_it_up() {
    let a = app(users_config(), None);
    let (s, v) = a.as_user(ALICE, Method::GET, "/api/v1/assistant/status", None).await;
    assert_eq!((s, v), (StatusCode::OK, json!({ "configured": false, "model": null, "fake": false })));
    let (s, v) = chat(&a, ALICE, json!({ "message": "hi" })).await;
    assert_eq!((s, v["error"]["code"].clone()), (StatusCode::SERVICE_UNAVAILABLE, json!("assistant_not_configured")));
    assert!(v["error"]["message"].as_str().unwrap().contains("[assistant]"));
}

#[tokio::test]
async fn a_question_runs_tools_over_the_shared_data_and_returns_what_ran() {
    let a = app_with(users_config(), None, |s| s.with_assistant(Arc::new(DemoLlm), "demo", true));
    let (s, v) = a.as_user(ALICE, Method::GET, "/api/v1/assistant/status", None).await;
    assert_eq!((s, v), (StatusCode::OK, json!({ "configured": true, "model": "demo", "fake": true })));
    // Data made through the ordinary API...
    a.as_user(BOB, Method::PUT, "/api/v1/lists/groceries", Some(json!({ "name": "Groceries" }))).await;
    a.as_user(BOB, Method::PUT, "/api/v1/lists/groceries/items/m", Some(json!({ "name": "Milk", "quantity": "2 L" }))).await;
    a.as_user(BOB, Method::PUT, "/api/v1/todos/t1", Some(json!({ "title": "Call the plumber", "due": "2026-10-01" }))).await;
    // ... is what the assistant reads, for either user (todos and shopping are shared).
    for who in [ALICE, BOB] {
        let (s, v) = chat(&a, who, json!({ "message": "What's on the shopping list and what's overdue?" })).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let tools: Vec<_> = v["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(tools, ["get_todos", "get_shopping_list"]);
        assert!(v["answer"].as_str().unwrap().contains("Groceries: Milk") && v["answer"].as_str().unwrap().contains("Call the plumber"), "{v}");
        assert_eq!(v["tools"][0]["arguments"], json!({ "filter": "overdue" }));
        assert_eq!(v["tools"][1]["result"]["items"][0]["quantity"], "2 L", "the raw tool results come back for the UI to show");
        assert_eq!((v["iterations"].clone(), v["hit_iteration_cap"].clone()), (json!(2), json!(false)));
    }
    // History from the browser is accepted.
    let body = json!({ "message": "and the beep test?", "history": [{ "role": "user", "content": "hi" }, { "role": "assistant", "content": "hello" }] });
    assert_eq!(chat(&a, ALICE, body).await.0, StatusCode::OK);
}

#[tokio::test]
async fn the_model_is_told_who_is_asking_and_the_date_and_gets_only_read_tools() {
    let llm = FakeLlmClient::scripted(vec![Ok(ChatResponse::text("fine")), Ok(ChatResponse::text("fine"))]);
    let a = with_llm(llm.clone());
    chat(&a, ALICE, json!({ "message": "hello" })).await;
    chat(&a, BOB, json!({ "message": "hello" })).await;
    let sent = llm.sent();
    // The mocked clock: 2026-10-11 14:30 UTC = Monday 2026-10-12 in Sydney.
    assert!(sent[0].messages[0].content.contains("household assistant for alice. Today is Monday 2026-10-12 (timezone Australia/Sydney)"), "{}", sent[0].messages[0].content);
    assert!(sent[1].messages[0].content.contains("household assistant for bob."));
    assert_eq!((sent[0].model.as_str(), sent[0].num_ctx), ("qwen3:test", 8192));
    let names: Vec<&str> = sent[0].tools.iter().map(|t| t.function.name.as_str()).collect();
    assert_eq!(names.len(), 7);
    assert!(names.iter().all(|n| n.starts_with("get_")), "no tool can write: {names:?}");
}

#[tokio::test]
async fn needs_an_identity_and_the_usual_write_protections() {
    let a = with_llm(FakeLlmClient::scripted(vec![Ok(ChatResponse::text("x"))]));
    let (s, v, _) = a.req(Method::POST, "/api/v1/assistant/chat", Some(json!({ "message": "hi" })), &[]).await;
    assert_eq!((s, v["error"]["code"].clone()), (StatusCode::UNAUTHORIZED, json!("no_user")));
    assert_eq!(a.get("/api/v1/assistant/status").await.0, StatusCode::UNAUTHORIZED);
    let (s, _, _) = a.req(Method::POST, "/api/v1/assistant/chat", Some(json!({ "message": "hi" })), &[("Tailscale-User-Login", ALICE), ("Content-Type", "text/plain")]).await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (s, v, _) = a.req(Method::POST, "/api/v1/assistant/chat", Some(json!({ "message": "hi" })), &[("Tailscale-User-Login", ALICE), ("Origin", "https://evil.example")]).await;
    assert_eq!((s, v["error"]["code"].clone()), (StatusCode::FORBIDDEN, json!("bad_origin")));
}

#[tokio::test]
async fn bad_requests_are_rejected_before_the_model_is_asked() {
    let llm = FakeLlmClient::scripted(vec![]);
    let a = with_llm(llm.clone());
    let many: Vec<Value> = (0..41).map(|i| json!({ "role": "user", "content": format!("{i}") })).collect();
    for body in [json!({ "message": "" }), json!({ "message": "   " }), json!({ "message": "x".repeat(2001) }), json!({ "message": "hi", "history": many }), json!({})] {
        let (s, _) = chat(&a, ALICE, body.clone()).await;
        assert!(s.is_client_error(), "{body}");
    }
    assert!(llm.sent().is_empty(), "nothing reached the model");
}

#[tokio::test]
async fn host_problems_get_their_own_status_and_code() {
    for (err, status, code) in [
        (LlmError::Offline("connection refused".into()), StatusCode::SERVICE_UNAVAILABLE, "assistant_offline"),
        (LlmError::Timeout, StatusCode::GATEWAY_TIMEOUT, "assistant_timeout"),
        (LlmError::Bad("model not found".into()), StatusCode::BAD_GATEWAY, "assistant_error"),
    ] {
        let a = with_llm(FakeLlmClient::scripted(vec![Err(err)]));
        let (s, v) = chat(&a, ALICE, json!({ "message": "hi" })).await;
        assert_eq!((s, v["error"]["code"].clone()), (status, json!(code)));
    }
    let a = with_llm(FakeLlmClient::scripted(vec![Err(LlmError::Offline("refused".into()))]));
    let (_, v) = chat(&a, ALICE, json!({ "message": "hi" })).await;
    assert!(v["error"]["message"].as_str().unwrap().contains("Is Ollama running?"));
    // The rest of the app is unaffected.
    assert_eq!(a.get_as(ALICE, "/api/v1/health").await, StatusCode::OK);
}

struct Slow;
impl LlmClient for Slow {
    fn chat<'a>(&'a self, _: &'a ChatRequest) -> assistant_core::llm::LlmFuture<'a> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(ChatResponse::text("slow"))
        })
    }
}

#[tokio::test]
async fn only_a_couple_of_chats_run_at_once() {
    let a = Arc::new(with_llm(Arc::new(Slow)));
    let spawn = |a: Arc<App>| tokio::spawn(async move { chat(&a, ALICE, json!({ "message": "slow one" })).await });
    let (first, second) = (spawn(a.clone()), spawn(a.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await; // both are inside the model call now
    let (s, v) = chat(&a, BOB, json!({ "message": "third" })).await;
    assert_eq!((s, v["error"]["code"].clone()), (StatusCode::TOO_MANY_REQUESTS, json!("assistant_busy")));
    first.abort();
    second.abort();
}

impl App {
    async fn get_as(&self, login: &str, uri: &str) -> StatusCode {
        self.as_user(login, Method::GET, uri, None).await.0
    }
}
