//! Live eval: ask a real Ollama model the questions in `evals/questions.toml` against the fixed
//! seed dataset, and grade tool choice and facts. NOT part of `cargo test` (needs a model host).
//!
//! ```text
//! cargo run -p assistant-core --example eval -- --config <path to config.toml> [options]
//!   --config <path>     a config.toml with an [assistant] section (url, model)
//!   --url <url> --model <name>   instead of --config
//!   --questions <path>  default: crates/assistant-core/evals/questions.toml
//!   --only <id>         run one question (repeatable)
//!   --repeat <n>        ask each question n times (models are not deterministic); default 1
//!   --out <path>        write the results as JSON
//! ```
//! Exit code: 0 every run passed, 1 some failed, 2 the model host could not be used.
//! Setup steps for the host: docs/BLOCKERS.md ("v2 owner setup").

use assistant_core::{
    eval::{self, Question},
    seed, Agent, AgentError, ChatRequest, LlmClient, Message, OllamaClient,
};
use serde_json::{json, Value};
use std::{path::PathBuf, process::ExitCode, sync::Arc, time::{Duration, Instant}};

struct Args {
    url: String,
    model: String,
    questions: PathBuf,
    only: Vec<String>,
    repeat: usize,
    out: Option<PathBuf>,
}

const SETUP: &str = "\
Could not use the model host. Owner steps (docs/BLOCKERS.md, \"v2 owner setup\"):
  1. Install Ollama: https://ollama.com/download
  2. Start it (the desktop app, or `ollama serve`) and pull a model that supports tools, e.g. `ollama pull qwen3:8b`
  3. Put its address and model in config.toml:  [assistant]  url = \"http://127.0.0.1:11434\"  model = \"qwen3:8b\"
  4. Run this again.";

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let (mut config, mut url, mut model, mut only, mut repeat, mut out) = (None, None, None, vec![], 1usize, None);
    let mut questions = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/evals/questions.toml"));
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--config" => config = Some(PathBuf::from(val("--config")?)),
            "--url" => url = Some(val("--url")?),
            "--model" => model = Some(val("--model")?),
            "--questions" => questions = PathBuf::from(val("--questions")?),
            "--only" => only.push(val("--only")?),
            "--repeat" => repeat = val("--repeat")?.parse().map_err(|_| "--repeat needs a number".to_string())?,
            "--out" => out = Some(PathBuf::from(val("--out")?)),
            other => return Err(format!("unknown argument {other:?}; see the top of crates/assistant-core/examples/eval.rs")),
        }
    }
    if let Some(path) = config {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let v: toml::Value = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let a = v.get("assistant").ok_or("no [assistant] section in that config.toml")?;
        url = url.or(a.get("url").and_then(|x| x.as_str()).map(String::from));
        model = model.or(a.get("model").and_then(|x| x.as_str()).map(String::from));
    }
    let (url, model) = (url.ok_or("no url: pass --config or --url")?, model.ok_or("no model: pass --config or --model")?);
    Ok(Args { url, model, questions, only, repeat: repeat.max(1), out })
}

/// Is the host up and the model pulled? Then load the model with a first request, which can be slow.
async fn probe(args: &Args) -> Result<(), String> {
    let http = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().map_err(|e| e.to_string())?;
    let tags: Value = http
        .get(format!("{}/api/tags", args.url.trim_end_matches('/')))
        .send()
        .await
        .map_err(|e| format!("{} is not reachable: {e}", args.url))?
        .json()
        .await
        .map_err(|e| format!("{} did not answer like Ollama: {e}", args.url))?;
    let names: Vec<String> = tags["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()).unwrap_or_default();
    if !names.iter().any(|n| *n == args.model || n.strip_suffix(":latest") == Some(args.model.as_str())) {
        return Err(format!("model {:?} is not pulled on that host (run `ollama pull {}`). Pulled: {}", args.model, args.model, names.join(", ")));
    }
    println!("loading {} ...", args.model);
    let slow = OllamaClient::new(&args.url, Duration::from_secs(300)).map_err(|e| e.to_string())?;
    let warm = ChatRequest { model: args.model.clone(), messages: vec![Message::new("user", "Reply with the single word OK.")], tools: vec![], num_ctx: 8192 };
    slow.chat(&warm).await.map_err(|e| format!("the model did not answer a first request: {e}"))?;
    Ok(())
}

struct Run {
    id: String,
    pass: bool,
    ms: u128,
    tools: Vec<String>,
    /// `name({"arg": ...})` for each call, to see what the model actually passed.
    calls: Vec<String>,
    answer: String,
    failures: Vec<String>,
}

async fn run_one(agent: &Agent, q: &Question) -> Run {
    let started = Instant::now();
    let ctx = seed::context();
    let (answer, tools, calls, failures) = match agent.ask(&ctx, &[], &q.ask).await {
        Ok(a) => {
            let tools: Vec<String> = a.tools.iter().map(|t| t.name.clone()).collect();
            let calls: Vec<String> = a.tools.iter().map(|t| format!("{}({}){}", t.name, t.arguments, if t.error { " -> error" } else { "" })).collect();
            let mut why = eval::grade(q, &a.answer, &tools);
            if a.hit_iteration_cap {
                why.push("hit the iteration cap".into());
            }
            (a.answer, tools, calls, why)
        }
        Err(e @ (AgentError::Timeout | AgentError::Offline(_) | AgentError::Bad(_))) => (String::new(), vec![], vec![], vec![format!("error: {e}")]),
    };
    Run { id: q.id.clone(), pass: failures.is_empty(), ms: started.elapsed().as_millis(), tools, calls, answer, failures }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let text = match std::fs::read_to_string(&args.questions) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {e}", args.questions.display());
            return ExitCode::from(2);
        }
    };
    let questions: Vec<Question> = match eval::load(&text) {
        Ok(q) => q.into_iter().filter(|q| args.only.is_empty() || args.only.contains(&q.id)).collect(),
        Err(e) => {
            eprintln!("{}: {e}", args.questions.display());
            return ExitCode::from(2);
        }
    };
    if let Err(e) = probe(&args).await {
        eprintln!("{e}\n\n{SETUP}");
        return ExitCode::from(2);
    }
    let client = match OllamaClient::new(&args.url, assistant_core::agent::TIMEOUT) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let agent = Agent::new(client, Arc::new(seed::seed().tools()), &args.model);
    println!("{} questions x {} on {} ({}), seed: today {} {}\n", questions.len(), args.repeat, args.model, args.url, seed::TODAY, seed::TIMEZONE);

    let mut all: Vec<Run> = vec![];
    for q in &questions {
        let mut passes = 0;
        for n in 0..args.repeat {
            let r = run_one(&agent, q).await;
            passes += usize::from(r.pass);
            let tag = if args.repeat > 1 { format!("{}#{}", q.id, n + 1) } else { q.id.clone() };
            println!("[{}] {:<28} {:>5.1}s  tools: {}", if r.pass { "PASS" } else { "FAIL" }, tag, r.ms as f64 / 1000.0, if r.tools.is_empty() { "-".into() } else { r.tools.join(", ") });
            if !r.pass {
                for f in &r.failures {
                    println!("        - {f}");
                }
                for c in &r.calls {
                    println!("        call: {c}");
                }
                println!("        answer: {}", r.answer.replace('\n', " ").chars().take(220).collect::<String>());
            }
            all.push(r);
        }
        let _ = passes;
    }

    let total = all.len();
    let passed = all.iter().filter(|r| r.pass).count();
    let by_id = |id: &str| questions.iter().find(|q| q.id == id).unwrap();
    let with_tools: Vec<&Run> = all.iter().filter(|r| !by_id(&r.id).tools.is_empty()).collect();
    let tools_ok = with_tools.iter().filter(|r| by_id(&r.id).tools.iter().all(|t| r.tools.contains(t))).count();
    let mean = all.iter().map(|r| r.ms).sum::<u128>() as f64 / total.max(1) as f64 / 1000.0;
    println!("\npassed {passed}/{total}; chose the expected tools in {tools_ok}/{} of the runs that needed one; mean {mean:.1}s per answer", with_tools.len());
    let weak: Vec<String> = questions.iter().filter(|q| all.iter().filter(|r| r.id == q.id).any(|r| !r.pass)).map(|q| q.id.clone()).collect();
    if !weak.is_empty() {
        println!("missed at least once: {}", weak.join(", "));
    }

    if let Some(path) = &args.out {
        let doc = json!({
            "model": args.model, "url": args.url, "today": seed::TODAY, "repeat": args.repeat, "passed": passed, "total": total, "tools_ok": tools_ok, "tool_runs": with_tools.len(),
            "mean_seconds": (mean * 10.0).round() / 10.0,
            "runs": all.iter().map(|r| json!({ "id": r.id, "pass": r.pass, "seconds": (r.ms as f64 / 100.0).round() / 10.0, "tools": r.tools, "calls": r.calls, "failures": r.failures, "answer": r.answer })).collect::<Vec<_>>(),
        });
        if let Err(e) = std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap()) {
            eprintln!("could not write {}: {e}", path.display());
        } else {
            println!("results written to {}", path.display());
        }
    }
    if passed == total { ExitCode::SUCCESS } else { ExitCode::from(1) }
}
