//! `/api/v1` handlers. Each one maps to a store trait method; see docs/API.md.

use crate::{
    identity::{get_me, put_me, Me},
    ApiError, ApiResult, AppState, Body, DocKind,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{NaiveDate, TimeZone};
use fitness_core::{
    BeepScore, Block, BlockContext, Equipment, Exercise, ExternalActivity, FitnessStore, MeasurementType, NewTemplateItem, PlanSlot, Session,
    SessionStatus, SetKind, SetLog, Template, TestKind, TestResult, WeeklyPlan, Write,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use todo_core::{Todo, TodoStore};

pub(crate) type Q = Query<HashMap<String, String>>;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(|| async { Json(serde_json::json!({ "status": "ok" })) }))
        .route("/me", get(get_me).put(put_me))
        .route("/todos", get(list_todos))
        .route("/todos/{id}", put(put_todo).patch(patch_todo).delete(delete_todo))
        .route("/todos/{id}/complete", post(complete_todo))
        .route("/todos/{id}/uncomplete", post(uncomplete_todo))
        .merge(crate::training::routes())
        .merge(crate::shopping::routes())
        .route("/exercises", get(list_exercises))
        .route("/exercises/{id}", put(put_exercise).patch(patch_exercise))
        .route("/templates", get(list_templates))
        .route("/templates/{id}", get(get_template).put(put_template).patch(patch_template))
        .route("/plan", get(get_plan).put(put_plan))
        .route("/blocks", get(list_blocks))
        .route("/blocks/{id}", put(put_block).patch(patch_block))
        .route("/planned-day", get(planned_day))
        .route("/program-status", get(program_status))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", get(get_session).put(put_session))
        .route("/sessions/{id}/sets/{set_id}", put(put_set))
        .route("/sessions/{id}/finish", post(finish_session))
        .route("/tests", get(list_tests))
        .route("/tests/{id}", put(put_test))
        .route("/external-activities", put(put_external))
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such endpoint") })
}

// ---- helpers ----

pub(crate) fn lock<T: ?Sized>(m: &std::sync::Mutex<T>) -> ApiResult<std::sync::MutexGuard<'_, T>> {
    m.lock().map_err(|_| ApiError::internal("store mutex poisoned"))
}

impl AppState {
    pub(crate) fn read_todo<T>(&self, f: impl FnOnce(&dyn TodoStore) -> todo_core::Result<T>) -> ApiResult<T> {
        Ok(f(&*lock(&self.0.todo)?)?)
    }
    pub(crate) fn write_todo<T>(&self, f: impl FnOnce(&mut dyn TodoStore) -> todo_core::Result<T>) -> ApiResult<T> {
        let out = f(&mut *lock(&self.0.todo)?)?;
        self.saved(DocKind::Todo)?;
        Ok(out)
    }
    /// Persist (and trigger sync), then tell open browsers the doc moved.
    fn saved(&self, kind: DocKind) -> ApiResult<()> {
        (self.0.saved)(kind).map_err(ApiError::internal)?;
        self.0.events.notify(kind);
        Ok(())
    }
    pub(crate) fn read_shopping<T>(&self, f: impl FnOnce(&dyn shopping_core::ShoppingStore) -> shopping_core::Result<T>) -> ApiResult<T> {
        Ok(f(&*lock(&self.0.shopping)?)?)
    }
    pub(crate) fn write_shopping<T>(&self, f: impl FnOnce(&mut dyn shopping_core::ShoppingStore) -> shopping_core::Result<T>) -> ApiResult<T> {
        let out = f(&mut *lock(&self.0.shopping)?)?;
        self.saved(DocKind::Shopping)?;
        Ok(out)
    }
    pub(crate) fn read_fit<T>(&self, f: impl FnOnce(&dyn FitnessStore) -> fitness_core::Result<T>) -> ApiResult<T> {
        Ok(f(&*lock(&self.0.fit)?)?)
    }
    pub(crate) fn write_fit<T>(&self, f: impl FnOnce(&mut dyn FitnessStore) -> fitness_core::Result<T>) -> ApiResult<T> {
        let out = f(&mut *lock(&self.0.fit)?)?;
        self.saved(DocKind::Fitness)?;
        Ok(out)
    }
}

/// 201 for a new resource, 200 otherwise.
pub(crate) fn created<T: Serialize>(new: bool, body: T) -> Response {
    (if new { StatusCode::CREATED } else { StatusCode::OK }, Json(body)).into_response()
}

pub(crate) fn check_id(id: &str) -> ApiResult<()> {
    let ok = (1..=64).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    if ok { Ok(()) } else { Err(ApiError::bad(format!("id {id:?}: 1-64 chars of A-Z a-z 0-9 - _"))) }
}

pub(crate) fn check_text(field: &str, s: &str, max: usize, required: bool) -> ApiResult<()> {
    if required && s.trim().is_empty() {
        return Err(ApiError::bad(format!("{field} is required")));
    }
    if s.chars().count() > max {
        return Err(ApiError::bad(format!("{field} is longer than {max} characters")));
    }
    Ok(())
}

pub(crate) fn check_opt(field: &str, s: &Option<String>, max: usize) -> ApiResult<()> {
    s.as_deref().map_or(Ok(()), |s| check_text(field, s, max, false))
}

pub(crate) fn check_num(field: &str, v: Option<f64>) -> ApiResult<()> {
    match v {
        Some(x) if !x.is_finite() || x < 0.0 || x > 1e7 => Err(ApiError::bad(format!("{field} must be a number from 0 to 10000000"))),
        _ => Ok(()),
    }
}

pub(crate) fn parse_date(field: &str, s: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| ApiError::bad(format!("{field} {s:?}: expected YYYY-MM-DD")))
}

pub(crate) const TITLE: usize = 200;
pub(crate) const NOTES: usize = 2000;

/// `?user=` (read the other user's data) or the current user.
pub(crate) fn target_user(q: &HashMap<String, String>, me: &str) -> String {
    q.get("user").cloned().unwrap_or_else(|| me.to_string())
}

/// For PATCH: distinguishes "field absent" (`None`) from "set to null" (`Some(None)`).
pub(crate) fn some<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

// ---- todos (shared by both users) ----

async fn list_todos(State(st): State<AppState>, _me: Me, Query(q): Q) -> ApiResult<Json<Vec<Todo>>> {
    let today = st.today().to_string();
    let todos = st.read_todo(|s| s.list())?;
    let range = |k: &str| q.get(k).map(|v| parse_date(k, v).map(|d| d.to_string())).transpose();
    let (from, to) = (range("from")?, range("to")?);
    Ok(Json(todo_core::filter_todos(todos, q.get("filter").map(String::as_str), &today, from.as_deref(), to.as_deref())?))
}

#[derive(Deserialize)]
struct TodoIn {
    title: String,
    #[serde(default)]
    notes: String,
    due: Option<String>,
}

async fn put_todo(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<TodoIn>) -> ApiResult<Response> {
    check_id(&id)?;
    check_text("title", &b.title, TITLE, true)?;
    check_text("notes", &b.notes, NOTES, false)?;
    let new = st.write_todo(|s| s.put(&id, &b.title, &b.notes, b.due.as_deref()))?;
    Ok(created(new, st.read_todo(|s| s.get(&id))?))
}

#[derive(Deserialize)]
struct TodoPatch {
    title: Option<String>,
    notes: Option<String>,
    #[serde(default, deserialize_with = "some")]
    due: Option<Option<String>>,
}

async fn patch_todo(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<TodoPatch>) -> ApiResult<Json<Todo>> {
    let t = st.read_todo(|s| s.get(&id))?;
    let title = b.title.unwrap_or(t.title);
    let notes = b.notes.unwrap_or(t.notes);
    let due = b.due.unwrap_or(t.due);
    check_text("title", &title, TITLE, true)?;
    check_text("notes", &notes, NOTES, false)?;
    st.write_todo(|s| s.edit(&id, &title, &notes, due.as_deref()))?;
    Ok(Json(st.read_todo(|s| s.get(&id))?))
}

fn set_completed(st: &AppState, id: &str, completed: bool) -> ApiResult<Json<Todo>> {
    st.write_todo(|s| s.set_completed(id, completed))?;
    Ok(Json(st.read_todo(|s| s.get(id))?))
}

async fn complete_todo(State(st): State<AppState>, _me: Me, Path(id): Path<String>) -> ApiResult<Json<Todo>> {
    set_completed(&st, &id, true)
}

async fn uncomplete_todo(State(st): State<AppState>, _me: Me, Path(id): Path<String>) -> ApiResult<Json<Todo>> {
    set_completed(&st, &id, false)
}

async fn delete_todo(State(st): State<AppState>, _me: Me, Path(id): Path<String>) -> ApiResult<StatusCode> {
    st.write_todo(|s| s.delete(&id))?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- exercises (shared) ----

async fn list_exercises(State(st): State<AppState>, _me: Me) -> ApiResult<Json<Vec<Exercise>>> {
    Ok(Json(st.read_fit(|s| s.list_exercises())?))
}

#[derive(Deserialize)]
struct ExerciseIn {
    name: String,
    measurement: MeasurementType,
    tempo: Option<String>,
    #[serde(default)]
    primary_muscles: Vec<String>,
    #[serde(default)]
    secondary_muscles: Vec<String>,
    equipment: Option<Equipment>,
    default_rest_s: Option<u32>,
}

fn check_exercise(e: &Exercise) -> ApiResult<()> {
    check_text("name", &e.name, TITLE, true)?;
    check_opt("tempo", &e.tempo, 20)?;
    if let Some(Equipment::Unknown(q)) = &e.equipment {
        return Err(ApiError::bad(format!("equipment {q:?}: expected Barbell, Dumbbell, Machine, Cable, Bodyweight or Other")));
    }
    if e.default_rest_s.is_some_and(|r| r > 3600) {
        return Err(ApiError::bad("default_rest_s must be 0-3600"));
    }
    if let MeasurementType::Unknown(m) = &e.measurement {
        return Err(ApiError::bad(format!("measurement {m:?}: expected WeightReps, Reps, Timed or Distance")));
    }
    Ok(())
}

async fn put_exercise(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<ExerciseIn>) -> ApiResult<Response> {
    check_id(&id)?;
    let e = Exercise {
        tempo: b.tempo,
        primary_muscles: b.primary_muscles,
        secondary_muscles: b.secondary_muscles,
        equipment: b.equipment,
        default_rest_s: b.default_rest_s,
        ..Exercise::new(id, &b.name, b.measurement)
    };
    check_exercise(&e)?;
    let w = st.write_fit(|s| s.put_exercise(e.clone()))?;
    Ok(created(w == Write::Created, e))
}

#[derive(Deserialize)]
struct ExercisePatch {
    name: Option<String>,
    measurement: Option<MeasurementType>,
    #[serde(default, deserialize_with = "some")]
    tempo: Option<Option<String>>,
    primary_muscles: Option<Vec<String>>,
    secondary_muscles: Option<Vec<String>>,
    #[serde(default, deserialize_with = "some")]
    equipment: Option<Option<Equipment>>,
    #[serde(default, deserialize_with = "some")]
    default_rest_s: Option<Option<u32>>,
}

async fn patch_exercise(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<ExercisePatch>) -> ApiResult<Json<Exercise>> {
    let mut e = st.read_fit(|s| s.get_exercise(&id))?;
    e.name = b.name.unwrap_or(e.name);
    e.measurement = b.measurement.unwrap_or(e.measurement);
    e.tempo = b.tempo.unwrap_or(e.tempo);
    e.primary_muscles = b.primary_muscles.unwrap_or(e.primary_muscles);
    e.secondary_muscles = b.secondary_muscles.unwrap_or(e.secondary_muscles);
    e.equipment = b.equipment.unwrap_or(e.equipment);
    e.default_rest_s = b.default_rest_s.unwrap_or(e.default_rest_s);
    check_exercise(&e)?;
    st.write_fit(|s| s.update_exercise(e.clone()))?;
    Ok(Json(e))
}

// ---- templates (shared) ----

async fn list_templates(State(st): State<AppState>, _me: Me) -> ApiResult<Json<Vec<Template>>> {
    Ok(Json(st.read_fit(|s| s.list_templates())?))
}

async fn get_template(State(st): State<AppState>, _me: Me, Path(id): Path<String>) -> ApiResult<Json<Template>> {
    Ok(Json(st.read_fit(|s| s.get_template(&id))?))
}

#[derive(Deserialize)]
struct TemplateIn {
    name: String,
    target_minutes: u32,
    items: Vec<NewTemplateItem>,
}

fn check_template_items(st: &AppState, items: &[NewTemplateItem]) -> ApiResult<()> {
    if items.len() > 50 {
        return Err(ApiError::bad("a template has at most 50 items"));
    }
    for it in items {
        st.read_fit(|s| s.get_exercise(&it.exercise_id)).map_err(|_| ApiError::bad(format!("unknown exercise_id {:?}", it.exercise_id)))?;
        check_opt("tempo", &it.tempo, 20)?;
        check_num("target_load", it.target_load)?;
        if it.target_sets == 0 || it.target_sets > 100 {
            return Err(ApiError::bad("target_sets must be 1-100"));
        }
    }
    Ok(())
}

async fn put_template(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<TemplateIn>) -> ApiResult<Response> {
    check_id(&id)?;
    check_text("name", &b.name, TITLE, true)?;
    check_template_items(&st, &b.items)?;
    let t = Template { id: id.clone(), name: b.name, target_minutes: b.target_minutes, items: Template::items_from(b.items), deleted: false };
    let w = st.write_fit(|s| s.put_template(t))?;
    Ok(created(w == Write::Created, st.read_fit(|s| s.get_template(&id))?))
}

#[derive(Deserialize)]
struct TemplatePatch {
    name: Option<String>,
    target_minutes: Option<u32>,
    /// Replaces all items (new item IDs).
    items: Option<Vec<NewTemplateItem>>,
}

async fn patch_template(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<TemplatePatch>) -> ApiResult<Json<Template>> {
    let mut t = st.read_fit(|s| s.get_template(&id))?;
    if let Some(n) = b.name {
        check_text("name", &n, TITLE, true)?;
        t.name = n;
    }
    t.target_minutes = b.target_minutes.unwrap_or(t.target_minutes);
    if let Some(items) = b.items {
        check_template_items(&st, &items)?;
        t.items = Template::items_from(items);
    }
    st.write_fit(|s| s.update_template(t.clone()))?;
    Ok(Json(t))
}

// ---- weekly plan (per user) ----

async fn get_plan(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<WeeklyPlan>> {
    let user = target_user(&q, &me);
    let plan = st.read_fit(|s| s.get_plan(&user))?;
    Ok(Json(plan.unwrap_or(WeeklyPlan { user_id: user, slots: HashMap::new() })))
}

#[derive(Deserialize)]
struct PlanIn {
    slots: HashMap<String, PlanSlot>,
}

async fn put_plan(State(st): State<AppState>, Me(me): Me, Body(b): Body<PlanIn>) -> ApiResult<Json<WeeklyPlan>> {
    for s in b.slots.values() {
        check_opt("external_label", &s.external_label, TITLE)?;
    }
    let plan = WeeklyPlan { user_id: me, slots: b.slots };
    st.write_fit(|s| s.set_plan(plan.clone()))?;
    Ok(Json(plan))
}

// ---- blocks (per user) ----

async fn list_blocks(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<Vec<Block>>> {
    let user = target_user(&q, &me);
    Ok(Json(st.read_fit(|s| s.list_blocks())?.into_iter().filter(|b| b.user_id == user).collect()))
}

#[derive(Deserialize)]
struct BlockIn {
    name: String,
    start_date: String,
    weeks: u32,
    deload_week: Option<u32>,
    deload_pct: Option<f64>,
}

/// The block with this ID, if any, must belong to `me`.
fn own_block(st: &AppState, id: &str, me: &str) -> ApiResult<Option<Block>> {
    match st.read_fit(|s| s.get_block(id)) {
        Ok(b) if b.user_id != me => Err(ApiError::forbidden("that block belongs to the other user")),
        Ok(b) => Ok(Some(b)),
        Err(e) if e.status == StatusCode::NOT_FOUND => Ok(None),
        Err(e) => Err(e),
    }
}

async fn put_block(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(b): Body<BlockIn>) -> ApiResult<Response> {
    check_id(&id)?;
    check_text("name", &b.name, TITLE, true)?;
    own_block(&st, &id, &me)?;
    let block = Block {
        id, user_id: me, name: b.name, start_date: b.start_date, weeks: b.weeks,
        deload_week: b.deload_week, deload_pct: b.deload_pct, deleted: false,
    };
    let w = st.write_fit(|s| s.put_block(block.clone()))?;
    Ok(created(w == Write::Created, block))
}

#[derive(Deserialize)]
struct BlockPatch {
    name: Option<String>,
    start_date: Option<String>,
    weeks: Option<u32>,
    #[serde(default, deserialize_with = "some")]
    deload_week: Option<Option<u32>>,
    #[serde(default, deserialize_with = "some")]
    deload_pct: Option<Option<f64>>,
    deleted: Option<bool>,
}

async fn patch_block(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(p): Body<BlockPatch>) -> ApiResult<Json<Block>> {
    let mut b = own_block(&st, &id, &me)?.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found", format!("{id} not found")))?;
    if let Some(n) = p.name {
        check_text("name", &n, TITLE, true)?;
        b.name = n;
    }
    b.start_date = p.start_date.unwrap_or(b.start_date);
    b.weeks = p.weeks.unwrap_or(b.weeks);
    b.deload_week = p.deload_week.unwrap_or(b.deload_week);
    b.deload_pct = p.deload_pct.unwrap_or(b.deload_pct);
    b.deleted = p.deleted.unwrap_or(b.deleted);
    st.write_fit(|s| s.update_block(b.clone()))?;
    Ok(Json(b))
}

// ---- today / status ----

async fn planned_day(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<fitness_core::PlannedDay>> {
    let date = match q.get("date") {
        Some(d) => parse_date("date", d)?,
        None => st.today(),
    };
    let user = target_user(&q, &me);
    Ok(Json(st.read_fit(|s| s.planned_for(&user, date))?))
}

#[derive(Serialize)]
struct NextBlock {
    block_id: String,
    name: String,
    start_date: String,
}

#[derive(Serialize)]
struct ProgramStatus {
    date: String,
    block: Option<BlockContext>,
    /// Sessions logged against the current block week.
    sessions_this_week: usize,
    next_block: Option<NextBlock>,
}

async fn program_status(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<ProgramStatus>> {
    let user = target_user(&q, &me);
    let today = st.today();
    let (day, blocks, sessions) = st.read_fit(|s| Ok((s.planned_for(&user, today)?, s.list_blocks()?, s.list_sessions()?)))?;
    let sessions_this_week = day.block.as_ref().map_or(0, |b| {
        sessions.iter().filter(|s| s.user_id == user && s.block_id.as_deref() == Some(&b.block_id) && s.block_week == Some(b.week)).count()
    });
    let today_s = today.to_string();
    let next_block = blocks
        .into_iter()
        .filter(|b| b.user_id == user && b.start_date > today_s)
        .min_by(|a, b| a.start_date.cmp(&b.start_date))
        .map(|b| NextBlock { block_id: b.id, name: b.name, start_date: b.start_date });
    Ok(Json(ProgramStatus { date: today_s, block: day.block, sessions_this_week, next_block }))
}

// ---- sessions (per user) ----

async fn list_sessions(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<Vec<Session>>> {
    let user = target_user(&q, &me);
    let since_ms = match q.get("since") {
        Some(d) => {
            let midnight = parse_date("since", d)?.and_hms_opt(0, 0, 0).expect("midnight exists");
            st.0.tz.from_local_datetime(&midnight).earliest().map_or(i64::MIN, |t| t.timestamp_millis())
        }
        None => i64::MIN,
    };
    let limit = match q.get("limit") {
        Some(l) => l.parse::<usize>().ok().filter(|l| (1..=500).contains(l)).ok_or_else(|| ApiError::bad("limit must be 1-500"))?,
        None => 50,
    };
    let v = st.read_fit(|s| s.list_sessions())?;
    Ok(Json(v.into_iter().filter(|s| s.user_id == user && s.started_at >= since_ms).take(limit).collect()))
}

async fn get_session(State(st): State<AppState>, _me: Me, Path(id): Path<String>) -> ApiResult<Json<Session>> {
    Ok(Json(st.read_fit(|s| s.get_session(&id))?))
}

#[derive(Deserialize)]
struct SessionIn {
    program_id: Option<String>,
    workout_id: Option<String>,
    template_id: Option<String>,
    block_id: Option<String>,
    block_week: Option<u32>,
    slot: Option<String>,
    label: Option<String>,
}

async fn put_session(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(b): Body<SessionIn>) -> ApiResult<Response> {
    check_id(&id)?;
    check_opt("label", &b.label, TITLE)?;
    if b.slot.as_deref().is_some_and(|s| s != "AM" && s != "PM") {
        return Err(ApiError::bad("slot must be AM or PM"));
    }
    if let Ok(existing) = st.read_fit(|s| s.get_session(&id)) {
        if existing.user_id != me {
            return Err(ApiError::forbidden("that session belongs to the other user"));
        }
    }
    let mut s = Session::start(id.clone(), &me);
    (s.template_id, s.block_id, s.block_week, s.slot, s.label) = (b.template_id, b.block_id, b.block_week, b.slot, b.label);
    (s.program_id, s.workout_id) = (b.program_id, b.workout_id);
    let w = st.write_fit(|f| f.put_session(s))?;
    Ok(created(w == Write::Created, st.read_fit(|f| f.get_session(&id))?))
}

async fn put_set(
    State(st): State<AppState>,
    Me(me): Me,
    Path((id, set_id)): Path<(String, String)>,
    Body(set): Body<SetLog>,
) -> ApiResult<Response> {
    check_id(&set_id)?;
    for (f, v) in [("load", set.load), ("distance_m", set.distance_m)] {
        check_num(f, v)?;
    }
    if set.exercise_id.is_empty() {
        return Err(ApiError::bad("exercise_id is required"));
    }
    if let Some(SetKind::Unknown(k)) = &set.kind {
        return Err(ApiError::bad(format!("kind {k:?}: expected Warmup, Working, Drop or Failure")));
    }
    if set.rir.is_some_and(|r| r > 10) || set.partials.is_some_and(|p| p > 50) || set.rest_taken_s.is_some_and(|r| r > 7200) {
        return Err(ApiError::bad("rir must be 0-10, partials 0-50, rest_taken_s 0-7200"));
    }
    check_opt("recommendation_id", &set.recommendation_id, 200)?;
    let w = st.write_fit(|s| s.put_set(&me, &id, &set_id, set))?;
    let session = st.read_fit(|s| s.get_session(&id))?;
    Ok(created(w == Write::Created, session))
}

#[derive(Deserialize)]
struct FinishIn {
    status: SessionStatus,
    note: Option<String>,
}

async fn finish_session(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(b): Body<FinishIn>) -> ApiResult<Json<Session>> {
    check_opt("note", &b.note, NOTES)?;
    st.write_fit(|s| s.finish_session(&me, &id, b.status, b.note.as_deref()))?;
    Ok(Json(st.read_fit(|s| s.get_session(&id))?))
}

// ---- benchmark tests (per user) ----

#[derive(Serialize)]
struct TestOut {
    #[serde(flatten)]
    result: TestResult,
    /// `8.10` for beep tests, else the value.
    score: String,
    /// y value for charts: `level + shuttle / shuttles_in_level` for beep tests.
    plot_value: f64,
}

impl From<TestResult> for TestOut {
    fn from(r: TestResult) -> Self {
        let (score, plot_value) = match r.beep {
            Some(b) => (b.to_string(), b.plot_value()),
            None => (r.value.map(|v| v.to_string()).unwrap_or_default(), r.value.unwrap_or(0.0)),
        };
        TestOut { result: r, score, plot_value }
    }
}

const KINDS: [TestKind; 4] = [TestKind::BeepTest, TestKind::OneRepMax, TestKind::MaxReps, TestKind::TimedRun];

async fn list_tests(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<Vec<TestOut>>> {
    let user = target_user(&q, &me);
    let kinds: Vec<TestKind> = match q.get("kind") {
        Some(k) => vec![k.parse().expect("infallible")],
        None => KINDS.to_vec(),
    };
    let since = q.get("since").map(|d| parse_date("since", d).map(|d| d.to_string())).transpose()?;
    let mut out = vec![];
    for k in kinds {
        let v = st.read_fit(|s| s.list_tests(&k))?;
        out.extend(v.into_iter().filter(|t| t.user_id == user && since.as_ref().is_none_or(|s| t.date >= *s)).map(TestOut::from));
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct TestIn {
    kind: TestKind,
    date: String,
    /// `level.shuttle`, for `BeepTest`.
    beep: Option<String>,
    value: Option<f64>,
    exercise_id: Option<String>,
}

async fn put_test(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(b): Body<TestIn>) -> ApiResult<Response> {
    check_id(&id)?;
    parse_date("date", &b.date)?;
    if let TestKind::Unknown(k) = &b.kind {
        return Err(ApiError::bad(format!("kind {k:?}: expected BeepTest, OneRepMax, MaxReps or TimedRun")));
    }
    let beep = b.beep.as_deref().map(str::parse::<BeepScore>).transpose()?;
    let all = st.read_fit(|s| s.list_tests(&b.kind))?;
    if all.iter().any(|t| t.id == id && t.user_id != me) {
        return Err(ApiError::forbidden("that result belongs to the other user"));
    }
    let r = TestResult { id: id.clone(), user_id: me, kind: b.kind.clone(), date: b.date, beep, value: b.value, exercise_id: b.exercise_id, deleted: false };
    let w = st.write_fit(|s| s.put_test(r))?;
    let stored = st.read_fit(|s| s.list_tests(&b.kind))?.into_iter().find(|t| t.id == id).ok_or_else(|| ApiError::internal("test vanished"))?;
    Ok(created(w == Write::Created, TestOut::from(stored)))
}

// ---- external activities (per user) ----

#[derive(Deserialize)]
struct ExternalIn {
    source: String,
    source_id: String,
    started_at: i64,
    duration_s: u32,
    distance_m: Option<f64>,
    linked_session_id: Option<String>,
}

async fn put_external(State(st): State<AppState>, Me(me): Me, Body(b): Body<ExternalIn>) -> ApiResult<Response> {
    check_text("source", &b.source, 64, true)?;
    check_text("source_id", &b.source_id, 200, true)?;
    check_num("distance_m", b.distance_m)?;
    let a = ExternalActivity {
        source: b.source, source_id: b.source_id, user_id: me, started_at: b.started_at,
        duration_s: b.duration_s, distance_m: b.distance_m, linked_session_id: b.linked_session_id,
    };
    let w = st.write_fit(|s| s.upsert_external(a.clone()))?;
    Ok(created(w == Write::Created, a))
}
