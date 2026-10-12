//! A read-only assistant over the household's data, driven by a local model (Ollama).
//!
//! * [`llm`]: the `LlmClient` trait, `OllamaClient` (plain `reqwest`), `FakeLlmClient` (scripted).
//! * [`tools`]: seven read-only tools built on the same store functions as the `GET` endpoints.
//! * [`agent`]: the loop (at most 5 model calls, 60 s), the system prompt, and errors.
//!
//! Design and rationale: PLAN-v2 Track C, docs/DECISIONS.md "Assistant core (v2-M6)".

pub mod agent;
pub mod demo;
pub mod llm;
pub mod tools;

pub use demo::DemoLlm;
pub use agent::{Agent, AgentError, Answer, ChatTurn, ToolRun};
pub use llm::{ChatRequest, ChatResponse, FakeLlmClient, LlmClient, LlmError, Message, OllamaClient, ToolSpec};
pub use tools::{Context, StoreTools, ToolBackend};

#[cfg(test)]
mod tests;
