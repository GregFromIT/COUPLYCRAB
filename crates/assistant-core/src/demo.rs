//! `DemoLlm`: a stand-in for a real model, for `peer serve --fake-llm`. It is NOT intelligent: it
//! picks a tool from keywords in the question, then reads the tool's result back in one plain
//! sentence per tool. That is enough to click through the chat screen (and see which tools ran)
//! without Ollama. Real answers need a real model (`[assistant]` in `config.toml`).

use crate::llm::{ChatRequest, ChatResponse, LlmClient, LlmFuture, Message};
use serde_json::{json, Value};

pub struct DemoLlm;

impl LlmClient for DemoLlm {
    fn chat<'a>(&'a self, req: &'a ChatRequest) -> LlmFuture<'a> {
        let reply = demo_reply(req);
        Box::pin(async move { Ok(reply) })
    }
}

const HELP: &str = "I'm the demo assistant: no language model is connected, so I only match keywords. \
Try asking about today's workout, your weekly volume or sets, PRs, the beep test, overdue todos, or the shopping list.";

fn demo_reply(req: &ChatRequest) -> ChatResponse {
    let last_user = req.messages.iter().rposition(|m| m.role == "user").unwrap_or(0);
    let question = req.messages.get(last_user).map(|m| m.content.to_lowercase()).unwrap_or_default();
    let results: Vec<&Message> = req.messages[last_user..].iter().filter(|m| m.role == "tool").collect();
    if !results.is_empty() {
        let text: Vec<String> = results.iter().map(|m| summarize(m.tool_name.as_deref().unwrap_or(""), &serde_json::from_str(&m.content).unwrap_or(Value::Null))).collect();
        return ChatResponse::text(&text.join(" "));
    }
    let has = |words: &[&str]| words.iter().any(|w| question.contains(w));
    let mut calls: Vec<(&str, Value)> = vec![];
    if has(&["workout", "train", "exercise today", "recommend", "bench", "squat", "load"]) {
        calls.push(("get_next_workout", json!({})));
    }
    if has(&["volume", "sets", "muscle"]) {
        calls.push(("get_weekly_volume", json!({})));
    }
    if has(&[" pr", "pr ", "prs", "record", "personal best", "1rm", "one rep"]) {
        calls.push(("get_prs", json!({})));
    }
    if has(&["beep", "bleep"]) {
        calls.push(("get_test_history", json!({ "kind": "BeepTest" })));
    }
    if has(&["overdue", "late"]) {
        calls.push(("get_todos", json!({ "filter": "overdue" })));
    } else if has(&["todo", "to-do", "task"]) {
        calls.push(("get_todos", json!({ "filter": "open" })));
    }
    if has(&["shopping", "grocer", "buy", "bunnings"]) {
        calls.push(("get_shopping_list", json!({})));
    }
    if calls.is_empty() {
        ChatResponse::text(HELP)
    } else {
        ChatResponse::calls(calls)
    }
}

fn kg(v: &Value) -> String {
    v.as_f64().map_or("?".into(), |x| format!("{}", (x * 100.0).round() / 100.0))
}

fn summarize(tool: &str, v: &Value) -> String {
    if let Some(e) = v["error"].as_str() {
        return format!("The {tool} tool reported: {e}.");
    }
    match tool {
        "get_next_workout" => match v["workout"].as_object() {
            None => format!("{}", v["reason"].as_str().unwrap_or("No workout.")),
            Some(w) => {
                let items: Vec<String> = w["items"]
                    .as_array()
                    .map(|a| a.iter().map(|i| match i["recommendation"].as_object() {
                        Some(r) => format!("{}: {} kg × {} ({})", i["exercise_name"].as_str().unwrap_or("?"), kg(&r["load_kg"]), r["reps"], r["reason"].as_str().unwrap_or("")),
                        None => format!("{}: no history yet, pick a load", i["exercise_name"].as_str().unwrap_or("?")),
                    }).collect())
                    .unwrap_or_default();
                format!("{} (week {}). {}", w["name"].as_str().unwrap_or("Workout"), v["week"], items.join(" "))
            }
        },
        "get_weekly_volume" => {
            let mut sets: Vec<(String, f64)> = v["sets_per_muscle"].as_object().map(|o| o.iter().filter_map(|(k, x)| x.as_f64().filter(|n| *n > 0.0).map(|n| (k.clone(), n))).collect()).unwrap_or_default();
            sets.sort_by(|a, b| b.1.total_cmp(&a.1));
            let list: Vec<String> = sets.iter().map(|(m, n)| format!("{m} {n}")).collect();
            format!("Sets in the week of {}: {}.", v["week_start"].as_str().unwrap_or("?"), if list.is_empty() { "none logged".into() } else { list.join(", ") })
        }
        "get_prs" => {
            let best: Vec<String> = v["prs"].as_array().map(|a| a.iter().filter(|p| p["kind"] == "e1rm").map(|p| format!("{} {} kg est. 1RM", p["exercise"].as_str().unwrap_or("?"), kg(&p["e1rm_kg"]))).collect()).unwrap_or_default();
            if best.is_empty() { "No PRs yet.".into() } else { format!("Best estimated one-rep maxes: {}.", best.join("; ")) }
        }
        "get_test_history" => match v["results_best_first"].as_array().and_then(|a| a.first()) {
            Some(b) => format!("Best {} result: {} on {}.", v["kind"].as_str().unwrap_or("test"), b["score"], b["date"].as_str().unwrap_or("?")),
            None => "No results for that test yet.".into(),
        },
        "get_todos" => {
            let todos: Vec<String> = v["todos"].as_array().map(|a| a.iter().map(|t| format!("{} (due {})", t["title"].as_str().unwrap_or("?"), t["due"].as_str().unwrap_or("no date"))).collect()).unwrap_or_default();
            if todos.is_empty() { "No matching todos.".into() } else { format!("{} todo(s): {}.", todos.len(), todos.join("; ")) }
        }
        "get_shopping_list" => {
            let items: Vec<String> = v["items"].as_array().map(|a| a.iter().map(|i| format!("{}{}", i["name"].as_str().unwrap_or("?"), if i["ticked"] == true { " ✓" } else { "" })).collect()).unwrap_or_default();
            format!("{}: {}.", v["list"].as_str().unwrap_or("List"), if items.is_empty() { "empty".into() } else { items.join(", ") })
        }
        other => format!("Got a result from {other}."),
    }
}
