//! The assistant's tools. All read-only, all built on the same store functions the `GET`
//! endpoints use (no new data paths). The model chooses a tool and phrases the answer; every
//! number, date, load and volume comes from deterministic code in here and in `fitness-core`.
//!
//! Scope: training data (workouts, history, volume, PRs, tests) is the *current user's*;
//! todos and shopping lists are shared by the household.

use crate::llm::ToolSpec;
use chrono::NaiveDate;
use fitness_core::{Exercise, FitnessStore, TestKind};
use serde_json::{json, Map, Value};
use shopping_core::ShoppingStore;
use std::sync::{Arc, Mutex};
use todo_core::TodoStore;

/// A tool result bigger than this is cut down (and marked `"truncated": true`) so one big list
/// can't fill the model's context window.
pub const MAX_RESULT_BYTES: usize = 8 * 1024;

/// Who is asking and what "today" is, in the household's timezone.
#[derive(Debug, Clone)]
pub struct Context {
    pub user_id: String,
    pub today: NaiveDate,
    pub tz: chrono_tz::Tz,
}

pub trait ToolBackend: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    /// `Err` text goes back to the model as `{"error": ...}`.
    fn call(&self, ctx: &Context, name: &str, args: &Value) -> Result<Value, String>;
}

pub struct StoreTools {
    pub fit: Arc<Mutex<dyn FitnessStore + Send>>,
    pub todo: Arc<Mutex<dyn TodoStore + Send>>,
    pub shopping: Arc<Mutex<dyn ShoppingStore + Send>>,
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, String> {
    m.lock().map_err(|_| "the data store is unavailable".to_string())
}

fn local_date(tz: chrono_tz::Tz, ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&tz).date_naive()).unwrap_or_default()
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// An optional string argument: missing or null is `None`; anything but a string is an error.
fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim())),
        Some(other) => Err(format!("argument {key:?} must be a string, got {other}")),
    }
}

fn opt_date(args: &Value, key: &str) -> Result<Option<NaiveDate>, String> {
    opt_str(args, key)?
        .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| format!("argument {key:?} must be a date like 2026-10-12, got {s:?}")))
        .transpose()
}

fn find_exercise(exercises: &[Exercise], query: &str) -> Result<Exercise, String> {
    let q = query.trim().to_lowercase();
    if let Some(e) = exercises.iter().find(|e| e.id == query.trim() || e.name.to_lowercase() == q) {
        return Ok(e.clone());
    }
    let hits: Vec<&Exercise> = exercises.iter().filter(|e| e.name.to_lowercase().contains(&q)).collect();
    let names = |v: &[&Exercise]| v.iter().map(|e| e.name.clone()).collect::<Vec<_>>().join(", ");
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no exercise matches {query:?}; the exercises are: {}", names(&exercises.iter().take(30).collect::<Vec<_>>()))),
        many => Err(format!("{query:?} matches several exercises ({}); use the full name", names(many))),
    }
}

fn parse_test_kind(s: &str) -> Result<TestKind, String> {
    let k: String = s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    Ok(match k.as_str() {
        "beeptest" | "beep" | "bleep" | "bleeptest" => TestKind::BeepTest,
        "onerepmax" | "1rm" | "max" => TestKind::OneRepMax,
        "maxreps" | "reps" => TestKind::MaxReps,
        "timedrun" | "run" | "timedruntest" => TestKind::TimedRun,
        _ => return Err(format!("kind {s:?} is not one of BeepTest, OneRepMax, MaxReps, TimedRun")),
    })
}

impl StoreTools {
    fn next_workout(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let date = opt_date(args, "date")?.unwrap_or(ctx.today);
        let tz = ctx.tz;
        let n = lock(&*self.fit)?.next_workout(&ctx.user_id, date, &|ms| local_date(tz, ms)).map_err(|e| e.to_string())?;
        serde_json::to_value(n).map_err(|e| e.to_string())
    }

    fn exercise_history(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let name = opt_str(args, "exercise")?.ok_or("argument \"exercise\" is required")?;
        let since = opt_date(args, "since")?;
        let fit = lock(&*self.fit)?;
        let ex = find_exercise(&fit.list_exercises().map_err(|e| e.to_string())?, name)?;
        let sessions = fit.list_sessions().map_err(|e| e.to_string())?;
        let mine = |s: &&fitness_core::Session| s.user_id == ctx.user_id && since.is_none_or(|d| local_date(ctx.tz, s.started_at) >= d);
        let rows: Vec<Value> = sessions
            .iter()
            .filter(mine)
            .filter_map(|s| {
                let sets: Vec<Value> = s
                    .ordered_sets()
                    .into_iter()
                    .filter(|x| x.exercise_id == ex.id)
                    .map(|x| json!({ "kind": x.kind.as_ref().map_or("Working", |k| k.as_str()), "load_kg": x.load, "reps": x.reps, "rir": x.rir, "duration_s": x.duration_s, "distance_m": x.distance_m }))
                    .collect();
                (!sets.is_empty()).then(|| json!({ "date": local_date(ctx.tz, s.started_at).to_string(), "status": s.status.as_str(), "sets": sets }))
            })
            .take(15)
            .collect();
        let e1rm: Vec<Value> = fitness_core::e1rm_series(&sessions, &ctx.user_id, &ex.id)
            .into_iter()
            .filter(|p| since.is_none_or(|d| local_date(ctx.tz, p.started_at) >= d))
            .rev()
            .take(15)
            .map(|p| json!({ "date": local_date(ctx.tz, p.started_at).to_string(), "e1rm_kg": round1(p.e1rm), "best_set": { "load_kg": p.load_kg, "reps": p.reps, "rir": p.rir } }))
            .collect();
        Ok(json!({ "exercise": { "id": ex.id, "name": ex.name }, "sessions_newest_first": rows, "e1rm_newest_first": e1rm }))
    }

    fn weekly_volume(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let date = opt_date(args, "week")?.unwrap_or(ctx.today);
        let fit = lock(&*self.fit)?;
        let sessions = fit.list_sessions().map_err(|e| e.to_string())?;
        let exercises: std::collections::HashMap<String, Exercise> = fit.list_exercises().map_err(|e| e.to_string())?.into_iter().map(|e| (e.id.clone(), e)).collect();
        let tz = ctx.tz;
        let sets = fitness_core::weekly_volume(&sessions, &exercises, &ctx.user_id, date, &|ms| local_date(tz, ms));
        let start = fitness_core::week_start(date);
        Ok(json!({ "week_start": start.to_string(), "week_start_weekday": start.format("%A").to_string(), "sets_per_muscle": sets, "note": "primary muscle = 1 set, secondary = 0.5; warm-ups excluded" }))
    }

    fn prs(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let fit = lock(&*self.fit)?;
        let exercises = fit.list_exercises().map_err(|e| e.to_string())?;
        let only = opt_str(args, "exercise")?.map(|q| find_exercise(&exercises, q)).transpose()?;
        let sessions = fit.list_sessions().map_err(|e| e.to_string())?;
        let name = |id: &str| exercises.iter().find(|e| e.id == id).map_or(id.to_string(), |e| e.name.clone());
        let prs: Vec<Value> = fitness_core::prs(&sessions, &ctx.user_id, only.as_ref().map(|e| e.id.as_str()))
            .into_iter()
            .map(|p| json!({ "exercise": name(&p.exercise_id), "kind": p.kind, "load_kg": p.load_kg, "reps": p.reps, "e1rm_kg": round1(p.e1rm), "date": local_date(ctx.tz, p.started_at).to_string() }))
            .collect();
        Ok(json!({ "prs": prs, "note": "kind e1rm = best estimated one-rep max; kind reps = most reps at that load or heavier" }))
    }

    fn test_history(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let kind = parse_test_kind(opt_str(args, "kind")?.ok_or("argument \"kind\" is required")?)?;
        let since = opt_date(args, "since")?.map(|d| d.to_string());
        let fit = lock(&*self.fit)?;
        let exercises = fit.list_exercises().map_err(|e| e.to_string())?;
        let rows: Vec<Value> = fit
            .list_tests(&kind)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|t| t.user_id == ctx.user_id && since.as_ref().is_none_or(|s| t.date >= *s))
            .map(|t| {
                let score = t.beep.map_or_else(|| t.value.map(|v| json!(v)).unwrap_or(Value::Null), |b| json!(b.to_string()));
                json!({ "date": t.date, "score": score, "exercise": t.exercise_id.and_then(|id| exercises.iter().find(|e| e.id == id).map(|e| e.name.clone())) })
            })
            .collect();
        let unit = match kind {
            TestKind::BeepTest => "level.shuttle (8.10 is better than 8.5)",
            TestKind::OneRepMax => "kg",
            TestKind::MaxReps => "reps",
            _ => "seconds (lower is better)",
        };
        Ok(json!({ "kind": kind.as_str(), "score_unit": unit, "results_best_first": rows }))
    }

    fn todos(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let (from, to) = (opt_date(args, "from")?.map(|d| d.to_string()), opt_date(args, "to")?.map(|d| d.to_string()));
        let all = lock(&*self.todo)?.list().map_err(|e| e.to_string())?;
        let found = todo_core::filter_todos(all, opt_str(args, "filter")?, &ctx.today.to_string(), from.as_deref(), to.as_deref()).map_err(|e| e.to_string())?;
        let todos: Vec<Value> = found.into_iter().map(|t| json!({ "title": t.title, "notes": t.notes, "due": t.due, "completed": t.completed })).collect();
        Ok(json!({ "today": ctx.today.to_string(), "todos": todos }))
    }

    fn shopping_list(&self, _ctx: &Context, args: &Value) -> Result<Value, String> {
        let want = opt_str(args, "list")?.unwrap_or("Groceries");
        let s = lock(&*self.shopping)?;
        let lists = s.lists().map_err(|e| e.to_string())?;
        let list = lists
            .iter()
            .find(|l| l.id == want || l.name.eq_ignore_ascii_case(want))
            .or_else(|| (opt_str(args, "list").ok().flatten().is_none() && lists.len() == 1).then(|| &lists[0]))
            .ok_or_else(|| format!("no shopping list called {want:?}; the lists are: {}", lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")))?;
        let items: Vec<Value> = s
            .items(&list.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|i| json!({ "name": i.name, "quantity": i.quantity, "note": i.note, "category": i.category, "ticked": i.checked() }))
            .collect();
        Ok(json!({ "list": list.name, "items": items }))
    }
}

fn props(pairs: &[(&str, &str)]) -> Value {
    let mut m = Map::new();
    for (k, d) in pairs {
        m.insert((*k).into(), json!({ "type": "string", "description": d }));
    }
    Value::Object(m)
}

impl ToolBackend for StoreTools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![
            ToolSpec::new("get_next_workout", "The workout planned for a day (default today): its exercises with sets, rep range, target RIR, the recommended load and reps for each, and why (the `reason`), plus warm-ups.", props(&[("date", "YYYY-MM-DD, default today")]), &[]),
            ToolSpec::new("get_exercise_history", "Logged sets and estimated one-rep max (e1RM) over time for one exercise, newest first.", props(&[("exercise", "Exercise name, e.g. Bench Press"), ("since", "YYYY-MM-DD, optional")]), &["exercise"]),
            ToolSpec::new("get_weekly_volume", "Working sets per muscle group for a week (Monday start); default the current week.", props(&[("week", "Any date in the week, YYYY-MM-DD, default today")]), &[]),
            ToolSpec::new("get_prs", "Personal records: best estimated one-rep max and most reps at a load, for one exercise or all.", props(&[("exercise", "Exercise name, optional")]), &[]),
            ToolSpec::new("get_test_history", "Results of benchmark tests, best first: beep test, one rep max, max reps, timed run.", props(&[("kind", "BeepTest, OneRepMax, MaxReps or TimedRun"), ("since", "YYYY-MM-DD, optional")]), &["kind"]),
            ToolSpec::new("get_todos", "The household's todos. filter: all, open, overdue, undated or due_between (then give from and/or to).", props(&[("filter", "all | open | overdue | undated | due_between"), ("from", "YYYY-MM-DD, for due_between"), ("to", "YYYY-MM-DD, for due_between")]), &[]),
            ToolSpec::new("get_shopping_list", "The items on a shopping list (default Groceries), with quantity, category and whether ticked.", props(&[("list", "List name, default Groceries")]), &[]),
        ]
    }

    fn call(&self, ctx: &Context, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "get_next_workout" => self.next_workout(ctx, args),
            "get_exercise_history" => self.exercise_history(ctx, args),
            "get_weekly_volume" => self.weekly_volume(ctx, args),
            "get_prs" => self.prs(ctx, args),
            "get_test_history" => self.test_history(ctx, args),
            "get_todos" => self.todos(ctx, args),
            "get_shopping_list" => self.shopping_list(ctx, args),
            other => Err(format!("unknown tool {other:?}; the tools are: {}", self.specs().iter().map(|s| s.function.name.as_str()).collect::<Vec<_>>().join(", "))),
        }
    }
}

// ---- size cap ----

fn size(v: &Value) -> usize {
    v.to_string().len()
}

fn longest_array(v: &Value) -> usize {
    match v {
        Value::Array(a) => a.len().max(a.iter().map(longest_array).max().unwrap_or(0)),
        Value::Object(o) => o.values().map(longest_array).max().unwrap_or(0),
        _ => 0,
    }
}

/// Halve the first array of length `len`; true if one was found.
fn halve(v: &mut Value, len: usize) -> bool {
    match v {
        Value::Array(a) if a.len() == len => {
            a.truncate(len / 2);
            true
        }
        Value::Array(a) => a.iter_mut().any(|x| halve(x, len)),
        Value::Object(o) => o.values_mut().any(|x| halve(x, len)),
        _ => false,
    }
}

/// At most [`MAX_RESULT_BYTES`] of JSON: halve the longest list until it fits and mark
/// `"truncated": true`. Still valid JSON, so the model can read what is left.
pub fn cap(mut v: Value) -> Value {
    if size(&v) <= MAX_RESULT_BYTES {
        return v;
    }
    let mut v = loop {
        let n = longest_array(&v);
        if n <= 1 || !halve(&mut v, n) || size(&v) + 20 <= MAX_RESULT_BYTES {
            break v;
        }
    };
    let mut out = match v.take() {
        Value::Object(o) => o,
        other => Map::from_iter([("result".to_string(), other)]),
    };
    out.insert("truncated".into(), Value::Bool(true));
    let v = Value::Object(out);
    if size(&v) <= MAX_RESULT_BYTES {
        return v;
    }
    // Still too big (a huge string, say): keep a prefix of the text.
    let text: String = v.to_string().chars().take(MAX_RESULT_BYTES / 2).collect();
    json!({ "truncated": true, "preview": text })
}
