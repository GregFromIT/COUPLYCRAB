//! The assistant's tools. All read-only, all built on the same store functions the `GET`
//! endpoints use (no new data paths). The model chooses a tool and phrases the answer; every
//! number, date, load and volume comes from deterministic code in here and in `fitness-core`:
//! relative dates ("thursday", "next monday") are resolved here, counts are returned in the
//! result, and arguments that name something real (an exercise, a list) are offered to the model
//! as a closed set in the tool's schema.
//!
//! Scope: training data (workouts, history, volume, PRs, tests) is the *current user's*;
//! todos and shopping lists are shared by the household.

use crate::llm::ToolSpec;
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use fitness_core::{Exercise, FitnessStore, TestKind};
use serde_json::{json, Map, Value};
use shopping_core::ShoppingStore;
use std::sync::{Arc, Mutex};
use todo_core::TodoStore;

/// A tool result bigger than this is cut down (and marked `"truncated": true`) so one big list
/// can't fill the model's context window.
pub const MAX_RESULT_BYTES: usize = 8 * 1024;
/// ... and all the tool results of one question together may not exceed this (the agent shrinks
/// later results to what is left, down to `MIN_RESULT_BYTES`), so a five-call loop cannot
/// overflow the context window either.
pub const MAX_TOTAL_RESULT_BYTES: usize = 16 * 1024;
pub const MIN_RESULT_BYTES: usize = 1024;

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

/// Text models write when they mean "not given" (seen in the live eval: `"since": "null"`, `"list": "default"`).
fn means_nothing(s: &str) -> bool {
    matches!(s.to_lowercase().as_str(), "" | "null" | "none" | "nil" | "undefined" | "n/a" | "na" | "never" | "default")
}

/// An optional string argument: missing, null, empty or "null"-like text is `None`; anything but a string is an error.
fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if means_nothing(s.trim()) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim())),
        Some(other) => Err(format!("argument {key:?} must be a string, got {other}")),
    }
}

fn days_in(unit: &str) -> Option<i64> {
    match unit.trim_end_matches('s') {
        "day" => Some(1),
        "week" => Some(7),
        _ => None,
    }
}

/// A date for a tool argument: `2026-10-12`, or words that code resolves relative to `today`, so
/// the model never does calendar arithmetic (small models get it wrong): `today`, `tomorrow`,
/// `yesterday`; a weekday (`thursday`, `this thursday`: the first one from today on); `next
/// thursday` (the first one after today); `last thursday` (the most recent before today);
/// `this week`, `next week`, `last week` (a day in that week); `3 days ago`, `in 2 weeks`.
pub fn resolve_date(text: &str, today: NaiveDate) -> Result<NaiveDate, String> {
    let t = text.trim().to_lowercase();
    if let Ok(d) = NaiveDate::parse_from_str(&t, "%Y-%m-%d") {
        return Ok(d);
    }
    let words: Vec<&str> = t.split_whitespace().filter(|w| !matches!(*w, "on" | "the")).collect();
    let weekday = |w: &str| w.parse::<Weekday>().ok();
    let on_or_after = |w: Weekday, after: bool| {
        let mut d = today + Duration::days(i64::from(after));
        while d.weekday() != w {
            d += Duration::days(1);
        }
        d
    };
    let before = |w: Weekday| {
        let mut d = today - Duration::days(1);
        while d.weekday() != w {
            d -= Duration::days(1);
        }
        d
    };
    let found = match words.as_slice() {
        ["today"] | ["now"] | ["this", "week"] => Some(today),
        ["tomorrow"] => Some(today + Duration::days(1)),
        ["yesterday"] => Some(today - Duration::days(1)),
        ["next", "week"] => Some(today + Duration::days(7)),
        ["last", "week"] | ["previous", "week"] => Some(today - Duration::days(7)),
        [w] | ["this", w] => weekday(w).map(|w| on_or_after(w, false)),
        ["next", w] => weekday(w).map(|w| on_or_after(w, true)),
        ["last", w] | ["previous", w] => weekday(w).map(before),
        [n, unit, "ago"] => n.parse::<i64>().ok().zip(days_in(unit)).map(|(n, u)| today - Duration::days(n * u)),
        ["in", n, unit] => n.parse::<i64>().ok().zip(days_in(unit)).map(|(n, u)| today + Duration::days(n * u)),
        _ => None,
    };
    found.ok_or_else(|| {
        format!(
            "{text:?} is not a date. Use YYYY-MM-DD (like {today}) or words such as today, tomorrow, yesterday, thursday, next monday, last friday, this week, last week, \"3 days ago\""
        )
    })
}

fn opt_date(args: &Value, key: &str, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
    opt_str(args, key)?.map(|s| resolve_date(s, today).map_err(|e| format!("argument {key:?}: {e}"))).transpose()
}

/// `since`: like a date, but not after today (a model that passes today's date for "last time"
/// would otherwise hide every earlier session, and gets told why instead of an empty answer).
///
/// Text that is not a date at all ("last time", "recently") is ignored rather than an error, and the
/// result says so: the live eval showed small models give up on the error instead of retrying.
fn since_arg(args: &Value, ctx: &Context) -> Result<(Option<NaiveDate>, Option<String>), String> {
    let text = opt_str(args, "since")?;
    let since = match text.map(|t| resolve_date(t, ctx.today)) {
        None => None,
        Some(Ok(d)) => Some(d),
        Some(Err(_)) => {
            return Ok((None, text.map(|t| format!("`since` {t:?} is not a date, so it was ignored: these are all of them."))));
        }
    };
    if since.is_some_and(|d| d > ctx.today) {
        return Err(format!("argument \"since\" is after today ({}); leave it out unless the question gives a start date", ctx.today));
    }
    Ok((since, None))
}

/// Add a note to a result object (joined with any note already there).
fn add_note(out: &mut Value, note: Option<String>) {
    if let Some(n) = note {
        let joined = out["note"].as_str().map_or(n.clone(), |old| format!("{old} {n}"));
        out["note"] = json!(joined);
    }
}

fn rows_empty(v: &Value) -> bool {
    v["sessions_newest_first"].as_array().is_some_and(|a| a.is_empty())
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
        let date = opt_date(args, "date", ctx.today)?.unwrap_or(ctx.today);
        let tz = ctx.tz;
        let n = lock(&*self.fit)?.next_workout(&ctx.user_id, date, &|ms| local_date(tz, ms)).map_err(|e| e.to_string())?;
        let mut v = serde_json::to_value(&n).map_err(|e| e.to_string())?;
        // The program has a name too ("Upper/Lower"); say plainly which workout and which weekday.
        v["weekday"] = json!(date.format("%A").to_string());
        v["workout_name"] = n.workout.as_ref().map_or(Value::Null, |w| json!(w.name));
        Ok(v)
    }

    fn exercise_history(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let name = opt_str(args, "exercise")?.ok_or("argument \"exercise\" is required")?;
        let (since, since_note) = since_arg(args, ctx)?;
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
                let d = local_date(ctx.tz, s.started_at);
                (!sets.is_empty()).then(|| json!({ "date": d.to_string(), "weekday": d.format("%A").to_string(), "status": s.status.as_str(), "sets": sets }))
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
        let mut out = json!({ "exercise": { "id": ex.id, "name": ex.name }, "session_count": rows.len(), "sessions_newest_first": rows, "e1rm_newest_first": e1rm });
        if let (true, Some(d)) = (rows_empty(&out), since) {
            // Say what the date filter hid, so a model that passed `since` by mistake can fix it.
            let earlier: Vec<NaiveDate> = sessions
                .iter()
                .filter(|s| s.user_id == ctx.user_id && s.sets.values().any(|x| x.exercise_id == ex.id))
                .map(|s| local_date(ctx.tz, s.started_at))
                .collect();
            if let Some(last) = earlier.iter().max() {
                out["note"] = json!(format!("No sessions since {d}, but there are {} earlier ones; the most recent was on {last}. Call again without `since` if the question is not about a start date.", earlier.len()));
            }
        }
        add_note(&mut out, since_note);
        Ok(out)
    }

    fn weekly_volume(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let date = opt_date(args, "week", ctx.today)?.unwrap_or(ctx.today);
        let fit = lock(&*self.fit)?;
        let sessions = fit.list_sessions().map_err(|e| e.to_string())?;
        let exercises: std::collections::HashMap<String, Exercise> = fit.list_exercises().map_err(|e| e.to_string())?.into_iter().map(|e| (e.id.clone(), e)).collect();
        let tz = ctx.tz;
        let sets = fitness_core::weekly_volume(&sessions, &exercises, &ctx.user_id, date, &|ms| local_date(tz, ms));
        let start = fitness_core::week_start(date);
        let total: f64 = sets.values().sum();
        // Every muscle at the maximum: a tie is reported as a tie, not broken arbitrarily.
        let top = sets.values().copied().fold(0.0, f64::max);
        let most: Vec<&String> = sets.iter().filter(|(_, n)| top > 0.0 && **n == top).map(|(m, _)| m).collect();
        Ok(json!({
            "week_start": start.to_string(),
            "week_start_weekday": start.format("%A").to_string(),
            "sets_per_muscle": sets,
            "total_sets": total,
            "most_trained_muscles": most,
            "note": "primary muscle = 1 set, secondary = 0.5; warm-ups excluded",
        }))
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
        let (since, since_note) = since_arg(args, ctx)?;
        let since = since.map(|d| d.to_string());
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
        let mut out = json!({ "kind": kind.as_str(), "score_unit": unit, "count": rows.len(), "results_best_first": rows });
        if let (0, Some(d)) = (rows.len(), &since) {
            let all = fit.list_tests(&kind).map_err(|e| e.to_string())?.into_iter().filter(|t| t.user_id == ctx.user_id).count();
            if all > 0 {
                out["note"] = json!(format!("No results since {d}, but there are {all} earlier ones. Call again without `since` if the question is not about a start date."));
            }
        }
        add_note(&mut out, since_note);
        Ok(out)
    }

    fn todos(&self, ctx: &Context, args: &Value) -> Result<Value, String> {
        let (from, to) = (opt_date(args, "from", ctx.today)?.map(|d| d.to_string()), opt_date(args, "to", ctx.today)?.map(|d| d.to_string()));
        let all = lock(&*self.todo)?.list().map_err(|e| e.to_string())?;
        let found = todo_core::filter_todos(all, opt_str(args, "filter")?, &ctx.today.to_string(), from.as_deref(), to.as_deref()).map_err(|e| e.to_string())?;
        let todos: Vec<Value> = found.into_iter().map(|t| json!({ "title": t.title, "notes": t.notes, "due": t.due, "completed": t.completed })).collect();
        Ok(json!({ "today": ctx.today.to_string(), "count": todos.len(), "todos": todos }))
    }

    fn shopping_list(&self, _ctx: &Context, args: &Value) -> Result<Value, String> {
        let asked = opt_str(args, "list")?;
        let want = asked.unwrap_or("Groceries");
        let s = lock(&*self.shopping)?;
        let lists = s.lists().map_err(|e| e.to_string())?;
        let list = lists
            .iter()
            .find(|l| l.id == want || l.name.eq_ignore_ascii_case(want))
            .or_else(|| (asked.is_none() && lists.len() == 1).then(|| &lists[0]))
            .ok_or_else(|| format!("no shopping list called {want:?}; the lists are: {}", lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")))?;
        let items: Vec<Value> = s
            .items(&list.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|i| json!({ "name": i.name, "quantity": i.quantity, "note": i.note, "category": i.category, "ticked": i.checked() }))
            .collect();
        let ticked = items.iter().filter(|i| i["ticked"] == true).count();
        Ok(json!({ "list": list.name, "count": items.len(), "ticked_count": ticked, "unticked_count": items.len() - ticked, "items": items }))
    }

    fn exercise_names(&self) -> Vec<String> {
        self.fit.lock().ok().and_then(|f| f.list_exercises().ok()).map(|v| v.into_iter().map(|e| e.name).collect()).unwrap_or_default()
    }

    fn list_names(&self) -> Vec<String> {
        self.shopping.lock().ok().and_then(|s| s.lists().ok()).map(|v| v.into_iter().map(|l| l.name).collect()).unwrap_or_default()
    }
}

/// A string parameter; `values`, if any, makes it a closed set (JSON Schema `enum`), which tool-calling
/// models follow far better than free text.
fn string_prop(description: &str, values: Vec<String>) -> Value {
    let mut p = json!({ "type": "string", "description": description });
    if !values.is_empty() {
        p["enum"] = json!(values);
    }
    p
}

fn props(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Map<_, _>>())
}

const DATE_WORDS: &str = "YYYY-MM-DD, or words such as 'thursday', 'next monday', 'tomorrow', 'last week'. Pass the words as the user said them; do not work out the date yourself.";

impl ToolBackend for StoreTools {
    fn specs(&self) -> Vec<ToolSpec> {
        let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (exercises, lists) = (self.exercise_names(), self.list_names());
        vec![
            ToolSpec::new(
                "get_next_workout",
                "What is planned to train on a day (default today): the workout's name, its exercises with sets, rep range, target RIR, the recommended load and reps for each and why (`reason`), plus warm-ups. Use it for any question about a day's workout, including 'what weight should I squat on Thursday'.",
                props(vec![("date", string_prop(&format!("The day. {DATE_WORDS} Leave out for today."), vec![]))]),
                &[],
            ),
            ToolSpec::new(
                "get_exercise_history",
                "The sets logged for one exercise, newest sessions first, and its estimated one-rep max over time. Use it for 'what did I lift last time' and 'when did I last do X'.",
                props(vec![
                    ("exercise", string_prop("Which exercise.", exercises.clone())),
                    ("since", string_prop(&format!("Only if the question gives a start date, e.g. 'since September'. {DATE_WORDS} Leave it out otherwise: it hides older sessions."), vec![])),
                ]),
                &["exercise"],
            ),
            ToolSpec::new(
                "get_weekly_volume",
                "How many sets of each muscle group (chest, back, shoulders, biceps, triceps, quads, hamstrings, glutes, calves, abs, forearms) were done in a week, and which muscle had the most. Use it for any question about sets, volume or what was trained most.",
                props(vec![("week", string_prop(&format!("Any day in the week. {DATE_WORDS} Leave out for this week."), vec![]))]),
                &[],
            ),
            ToolSpec::new(
                "get_prs",
                "Personal records: the best estimated one-rep max and the most reps at a load, for one exercise or all. Use it for 'heaviest', 'best' or 'PR' questions about lifting.",
                props(vec![("exercise", string_prop("Which exercise. Leave out for all.", exercises))]),
                &[],
            ),
            ToolSpec::new(
                "get_test_history",
                "Results of benchmark tests, best first: beep test, one rep max, max reps, timed run.",
                props(vec![
                    ("kind", string_prop("Which test.", strs(&["BeepTest", "OneRepMax", "MaxReps", "TimedRun"]))),
                    ("since", string_prop(&format!("Only if the question gives a start date. {DATE_WORDS} Leave it out otherwise."), vec![])),
                ]),
                &["kind"],
            ),
            ToolSpec::new(
                "get_todos",
                "The household's todos, with how many there are. Use filter 'overdue' for late ones, 'open' for everything not done, 'undated' for ones with no due date, 'due_between' with from and/or to for a date range.",
                props(vec![
                    ("filter", string_prop("Which todos. Leave out for all.", strs(&["all", "open", "overdue", "undated", "due_between"]))),
                    ("from", string_prop(&format!("For due_between. {DATE_WORDS}"), vec![])),
                    ("to", string_prop(&format!("For due_between. {DATE_WORDS}"), vec![])),
                ]),
                &[],
            ),
            ToolSpec::new(
                "get_shopping_list",
                "The items on a shopping list, with quantity, category and whether each is ticked, and how many. Use it for 'what's on the shopping list' and for a named list such as Bunnings.",
                props(vec![("list", string_prop("Which list. Leave it out for the default list (Groceries).", lists))]),
                &[],
            ),
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

/// At most [`MAX_RESULT_BYTES`] of JSON; see [`cap_to`].
pub fn cap(v: Value) -> Value {
    cap_to(v, MAX_RESULT_BYTES)
}

/// At most `max` bytes of JSON: halve the longest list until it fits and mark
/// `"truncated": true`. Still valid JSON, so the model can read what is left.
pub fn cap_to(mut v: Value, max: usize) -> Value {
    if size(&v) <= max {
        return v;
    }
    let mut v = loop {
        let n = longest_array(&v);
        if n <= 1 || !halve(&mut v, n) || size(&v) + 20 <= max {
            break v;
        }
    };
    let mut out = match v.take() {
        Value::Object(o) => o,
        other => Map::from_iter([("result".to_string(), other)]),
    };
    out.insert("truncated".into(), Value::Bool(true));
    let v = Value::Object(out);
    if size(&v) <= max {
        return v;
    }
    // Still too big (a huge string, say): keep a prefix of the text.
    let text: String = v.to_string().chars().take(max / 2).collect();
    json!({ "truncated": true, "preview": text })
}
