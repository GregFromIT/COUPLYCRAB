//! The model side: message types, the `LlmClient` trait, the Ollama client and a scripted fake.
//! Format: https://docs.ollama.com/capabilities/tool-calling (`POST /api/chat`, `stream: false`;
//! tool calls arrive as `message.tool_calls[].function{name, arguments}` with `arguments` an
//! object; results go back as `{"role": "tool", "tool_name": ..., "content": ...}`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For `role: "tool"`: the function the result belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl Message {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Message { role: role.into(), content: content.into(), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub function: FunctionCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// An object per Ollama's docs; some models emit a JSON string instead (the agent accepts both).
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema: `{"type": "object", "properties": {...}, "required": [...]}`.
    pub parameters: Value,
}

impl ToolSpec {
    pub fn new(name: &str, description: &str, properties: Value, required: &[&str]) -> Self {
        ToolSpec {
            kind: "function",
            function: FunctionSpec {
                name: name.into(),
                description: description.into(),
                parameters: json!({ "type": "object", "properties": properties, "required": required }),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// `options.num_ctx`: Ollama's default context is 4096 tokens; https://docs.ollama.com/faq
    pub num_ctx: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub message: Message,
}

impl ChatResponse {
    pub fn text(s: &str) -> Self {
        ChatResponse { message: Message::new("assistant", s) }
    }

    /// An assistant message asking for one tool call.
    pub fn call(name: &str, arguments: Value) -> Self {
        Self::calls(vec![(name, arguments)])
    }

    pub fn calls(calls: Vec<(&str, Value)>) -> Self {
        let tool_calls = calls.into_iter().map(|(n, a)| ToolCall { function: FunctionCall { name: n.into(), arguments: a } }).collect();
        ChatResponse { message: Message { role: "assistant".into(), tool_calls, ..Default::default() } }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmError {
    /// Could not reach the model host (not running, wrong URL, no route).
    Offline(String),
    Timeout,
    /// The host answered but not usefully (HTTP error, model missing, malformed body).
    Bad(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Offline(e) => write!(f, "model host unreachable: {e}"),
            LlmError::Timeout => write!(f, "model host timed out"),
            LlmError::Bad(e) => write!(f, "model host error: {e}"),
        }
    }
}
impl std::error::Error for LlmError {}

pub type LlmFuture<'a> = Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>>;

/// One chat round trip. Boxed futures (not `async fn`) so it can be a `dyn` object, chosen at run time.
pub trait LlmClient: Send + Sync {
    fn chat<'a>(&'a self, req: &'a ChatRequest) -> LlmFuture<'a>;
}

// ---- Ollama ----

pub struct OllamaClient {
    http: reqwest::Client,
    base: String,
}

impl OllamaClient {
    /// `url` like `http://127.0.0.1:11434`. `timeout` bounds each request.
    pub fn new(url: &str, timeout: Duration) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder().timeout(timeout).build().map_err(|e| LlmError::Bad(e.to_string()))?;
        Ok(OllamaClient { http, base: url.trim_end_matches('/').to_string() })
    }

    fn body(req: &ChatRequest) -> Value {
        let mut body = json!({
            "model": req.model,
            "messages": req.messages,
            "stream": false,
            "options": { "num_ctx": req.num_ctx },
        });
        if !req.tools.is_empty() {
            body["tools"] = json!(req.tools);
        }
        body
    }
}

impl LlmClient for OllamaClient {
    fn chat<'a>(&'a self, req: &'a ChatRequest) -> LlmFuture<'a> {
        Box::pin(async move {
            let res = self.http.post(format!("{}/api/chat", self.base)).json(&Self::body(req)).send().await.map_err(|e| {
                if e.is_timeout() { LlmError::Timeout } else if e.is_connect() { LlmError::Offline(e.to_string()) } else { LlmError::Bad(e.to_string()) }
            })?;
            let status = res.status();
            let text = res.text().await.map_err(|e| if e.is_timeout() { LlmError::Timeout } else { LlmError::Bad(e.to_string()) })?;
            if !status.is_success() {
                // Ollama's error body is `{"error": "model 'x' not found"}`.
                let msg = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or(text);
                return Err(LlmError::Bad(format!("HTTP {status}: {msg}")));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| LlmError::Bad(format!("not JSON: {e}")))?;
            let message: Message = serde_json::from_value(v["message"].clone()).map_err(|e| LlmError::Bad(format!("no message in the response: {e}")))?;
            Ok(ChatResponse { message })
        })
    }
}

// ---- scripted fake, for tests and `--fake-llm` ----

type Script = dyn Fn(&ChatRequest, usize) -> Result<ChatResponse, LlmError> + Send + Sync;

/// Answers from a script and remembers every request it was sent.
pub struct FakeLlmClient {
    script: Box<Script>,
    pub requests: Mutex<Vec<ChatRequest>>,
}

impl FakeLlmClient {
    /// `f(request, n)` where `n` counts calls from 0.
    pub fn with_fn(f: impl Fn(&ChatRequest, usize) -> Result<ChatResponse, LlmError> + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(FakeLlmClient { script: Box::new(f), requests: Mutex::new(vec![]) })
    }

    /// The listed responses, in order; running out is an error.
    pub fn scripted(responses: Vec<Result<ChatResponse, LlmError>>) -> Arc<Self> {
        let responses = Mutex::new(responses.into_iter().collect::<std::collections::VecDeque<_>>());
        Self::with_fn(move |_, _| responses.lock().unwrap().pop_front().unwrap_or(Err(LlmError::Bad("fake script exhausted".into()))))
    }

    pub fn sent(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl LlmClient for FakeLlmClient {
    fn chat<'a>(&'a self, req: &'a ChatRequest) -> LlmFuture<'a> {
        let n = {
            let mut r = self.requests.lock().unwrap();
            r.push(req.clone());
            r.len() - 1
        };
        let out = (self.script)(req, n);
        Box::pin(async move { out })
    }
}
