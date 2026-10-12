//! Support for the live eval (`cargo run -p assistant-core --example eval`): the question file
//! format, grading an answer, and an **oracle** that answers each question from the data with
//! the tools a good model would call. The oracle runs in `cargo test` (no model needed) and
//! proves every question is answerable from the seed and every expected fact is really in it,
//! so a failing live run points at the model, not at the question.

use crate::tools::{Context, ToolBackend};
use serde::Deserialize;
use serde_json::Value;

/// One question and what counts as a good answer.
#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub id: String,
    pub ask: String,
    /// Tools that must have been called (extra calls are fine).
    #[serde(default)]
    pub tools: Vec<String>,
    /// Every one must appear in the answer (case-insensitive).
    #[serde(default)]
    pub contains: Vec<String>,
    /// At least one must appear (if the list is not empty).
    #[serde(default)]
    pub contains_any: Vec<String>,
    /// None may appear.
    #[serde(default)]
    pub not_contains: Vec<String>,
    /// Facts in `contains` that the model has to work out (a count, say) rather than copy from a
    /// tool result; the oracle skips them (the oracle test checks them by hand).
    #[serde(default)]
    pub derived: Vec<String>,
    /// How the data answers it: the calls a good model would make, with arguments.
    #[serde(default)]
    pub oracle: Vec<OracleCall>,
    /// The oracle's calls are expected to fail (the question is about something that isn't there).
    #[serde(default)]
    pub oracle_error: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OracleCall {
    pub tool: String,
    #[serde(default = "empty_args")]
    pub args: Value,
}

fn empty_args() -> Value {
    Value::Object(Default::default())
}

#[derive(Deserialize)]
struct File {
    question: Vec<Question>,
}

pub fn load(text: &str) -> Result<Vec<Question>, String> {
    toml::from_str::<File>(text).map(|f| f.question).map_err(|e| e.to_string())
}

/// Why `answer` (given after calling `tools_called`) fails `q`; empty means it passes.
pub fn grade(q: &Question, answer: &str, tools_called: &[String]) -> Vec<String> {
    let a = answer.to_lowercase();
    let mut why = vec![];
    for t in &q.tools {
        if !tools_called.contains(t) {
            why.push(format!("did not call {t}"));
        }
    }
    for c in &q.contains {
        if !a.contains(&c.to_lowercase()) {
            why.push(format!("answer lacks {c:?}"));
        }
    }
    if !q.contains_any.is_empty() && !q.contains_any.iter().any(|c| a.contains(&c.to_lowercase())) {
        why.push(format!("answer has none of {:?}", q.contains_any));
    }
    for c in &q.not_contains {
        if a.contains(&c.to_lowercase()) {
            why.push(format!("answer contains {c:?}"));
        }
    }
    why
}

/// Run the question's oracle calls against `tools`; return what is wrong with the question
/// (a call that errors when it should not, or a fact missing from every result).
pub fn check_oracle(q: &Question, tools: &dyn ToolBackend, ctx: &Context) -> Vec<String> {
    let mut problems = vec![];
    let mut seen = String::new();
    let specs: Vec<String> = tools.specs().into_iter().map(|s| s.function.name).collect();
    for t in &q.tools {
        if !specs.contains(t) {
            problems.push(format!("expects unknown tool {t}"));
        }
    }
    let mut errors = 0;
    for c in &q.oracle {
        match tools.call(ctx, &c.tool, &c.args) {
            Ok(v) => seen.push_str(&v.to_string().to_lowercase()),
            Err(e) => {
                errors += 1;
                seen.push_str(&e.to_lowercase());
            }
        }
    }
    if q.oracle.is_empty() && !q.tools.is_empty() {
        problems.push("has expected tools but no oracle call".into());
    }
    if q.oracle_error != (errors > 0) && !q.oracle.is_empty() {
        problems.push(format!("oracle_error is {} but {errors} call(s) failed", q.oracle_error));
    }
    for t in &q.tools {
        if !q.oracle.iter().any(|c| &c.tool == t) {
            problems.push(format!("expected tool {t} is not in the oracle"));
        }
    }
    if !q.oracle_error {
        for f in q.contains.iter().filter(|f| !q.derived.contains(f)) {
            if !seen.contains(&f.to_lowercase()) {
                problems.push(format!("fact {f:?} is not in the oracle's tool results"));
            }
        }
    }
    problems
}
