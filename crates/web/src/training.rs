//! v2 training endpoints: gym profiles, programs, next workout, recommendations,
//! analytics and PRs. Same conventions as `api.rs`; see docs/API.md.
//! Recommendations and analytics are derived from the session log on every read.

use crate::{
    api::{check_id, check_opt, check_text, created, parse_date, some, target_user, Q, TITLE},
    identity::Me,
    ApiError, ApiResult, AppState, Body,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate};
use fitness_core::{
    e1rm_series, week_start, weekly_volume, GymProfile, Migration, NextWorkout, Pr, Program, ProgramItem, ProgramWorkout, SetStyle,
    WeekOverride, WorkoutPlan, Write,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/gym-profiles", get(list_gyms))
        .route("/gym-profiles/{id}", put(put_gym).patch(patch_gym))
        .route("/programs", get(list_programs))
        .route("/programs/migrate-v1", post(migrate_v1))
        .route("/programs/{id}", get(get_program).put(put_program).patch(patch_program))
        .route("/workouts/next", get(next_workout))
        .route("/recommendations", get(recommendations))
        .route("/analytics/volume", get(volume))
        .route("/analytics/e1rm", get(e1rm))
        .route("/prs", get(get_prs))
}

fn not_found(id: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", format!("{id} not found"))
}

fn local_date(tz: chrono_tz::Tz, ms: i64) -> NaiveDate {
    DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&tz).date_naive()).unwrap_or_default()
}

fn date_or_today(st: &AppState, q: &HashMap<String, String>, key: &str) -> ApiResult<NaiveDate> {
    q.get(key).map_or_else(|| Ok(st.today()), |d| parse_date(key, d))
}

// ---- gym profiles (per user) ----

async fn list_gyms(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<Vec<GymProfile>>> {
    let user = target_user(&q, &me);
    Ok(Json(st.read_fit(|s| s.list_gym_profiles(&user))?))
}

#[derive(Deserialize)]
struct GymIn {
    name: String,
    bar_kg: f64,
    plates_kg: Vec<f64>,
    dumbbell_step_kg: f64,
    machine_step_kg: f64,
    #[serde(default)]
    is_default: bool,
}

fn check_gym(g: &GymProfile) -> ApiResult<()> {
    check_text("name", &g.name, TITLE, true)?;
    if g.plates_kg.len() > 30 {
        return Err(ApiError::bad("a gym profile has at most 30 plate sizes"));
    }
    for w in g.plates_kg.iter().copied().chain([g.bar_kg, g.dumbbell_step_kg, g.machine_step_kg]) {
        if !w.is_finite() || w <= 0.0 || w > 1000.0 {
            return Err(ApiError::bad("bar, plates and steps must be numbers above 0 and at most 1000 kg"));
        }
    }
    Ok(())
}

/// The profile with this ID, if any, must belong to `me`.
fn own_gym(st: &AppState, id: &str, me: &str) -> ApiResult<Option<GymProfile>> {
    match st.read_fit(|s| s.get_gym_profile(id)) {
        Ok(g) if g.user_id != me => Err(ApiError::forbidden("that gym profile belongs to the other user")),
        Ok(g) => Ok(Some(g)),
        Err(e) if e.status == StatusCode::NOT_FOUND => Ok(None),
        Err(e) => Err(e),
    }
}

async fn put_gym(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(b): Body<GymIn>) -> ApiResult<Response> {
    check_id(&id)?;
    own_gym(&st, &id, &me)?;
    let g = GymProfile {
        id: id.clone(),
        user_id: me,
        name: b.name,
        bar_kg: b.bar_kg,
        plates_kg: b.plates_kg,
        dumbbell_step_kg: b.dumbbell_step_kg,
        machine_step_kg: b.machine_step_kg,
        is_default: b.is_default,
        deleted: false,
    };
    check_gym(&g)?;
    let w = st.write_fit(|s| s.put_gym_profile(g))?;
    Ok(created(w == Write::Created, st.read_fit(|s| s.get_gym_profile(&id))?))
}

#[derive(Deserialize)]
struct GymPatch {
    name: Option<String>,
    bar_kg: Option<f64>,
    plates_kg: Option<Vec<f64>>,
    dumbbell_step_kg: Option<f64>,
    machine_step_kg: Option<f64>,
    is_default: Option<bool>,
    deleted: Option<bool>,
}

async fn patch_gym(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(p): Body<GymPatch>) -> ApiResult<Json<GymProfile>> {
    let mut g = own_gym(&st, &id, &me)?.ok_or_else(|| not_found(&id))?;
    g.name = p.name.unwrap_or(g.name);
    g.bar_kg = p.bar_kg.unwrap_or(g.bar_kg);
    g.plates_kg = p.plates_kg.unwrap_or(g.plates_kg);
    g.dumbbell_step_kg = p.dumbbell_step_kg.unwrap_or(g.dumbbell_step_kg);
    g.machine_step_kg = p.machine_step_kg.unwrap_or(g.machine_step_kg);
    g.is_default = p.is_default.unwrap_or(g.is_default);
    g.deleted = p.deleted.unwrap_or(g.deleted);
    check_gym(&g)?;
    st.write_fit(|s| s.update_gym_profile(g.clone()))?;
    Ok(Json(st.read_fit(|s| s.get_gym_profile(&id))?))
}

// ---- programs (per user) ----
// The API uses ordered arrays for workouts and items (the stored form is maps keyed
// by ID with an `index`); clients choose the IDs so overrides can name items.

#[derive(Serialize)]
struct WorkoutOut {
    id: String,
    name: String,
    slot: Option<String>,
    external_label: Option<String>,
    items: Vec<ProgramItem>,
}

#[derive(Serialize)]
struct ProgramOut {
    id: String,
    user_id: String,
    name: String,
    start_date: String,
    weeks: u32,
    gym_profile_id: Option<String>,
    warmups: bool,
    workouts: Vec<WorkoutOut>,
    overrides: Vec<WeekOverride>,
    migrated_from: Option<String>,
    deleted: bool,
}

impl From<Program> for ProgramOut {
    fn from(p: Program) -> Self {
        let workouts = p
            .ordered_workouts()
            .into_iter()
            .map(|w| WorkoutOut {
                id: w.id.clone(),
                name: w.name.clone(),
                slot: w.slot.clone(),
                external_label: w.external_label.clone(),
                items: w.ordered_items().into_iter().cloned().collect(),
            })
            .collect();
        let mut overrides: Vec<WeekOverride> = p.overrides.values().cloned().collect();
        overrides.sort_by(|a, b| (a.week, &a.item_id).cmp(&(b.week, &b.item_id)));
        ProgramOut {
            id: p.id,
            user_id: p.user_id,
            name: p.name,
            start_date: p.start_date,
            weeks: p.weeks,
            gym_profile_id: p.gym_profile_id,
            warmups: p.warmups,
            workouts,
            overrides,
            migrated_from: p.migrated_from,
            deleted: p.deleted,
        }
    }
}

fn default_rir() -> u8 {
    2
}
fn straight() -> SetStyle {
    SetStyle::Straight
}
fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct ItemIn {
    id: String,
    exercise_id: String,
    sets: u32,
    rep_min: u32,
    rep_max: u32,
    #[serde(default = "default_rir")]
    rir: u8,
    #[serde(default = "straight")]
    style: SetStyle,
    rest_s: Option<u32>,
}

#[derive(Deserialize)]
struct WorkoutIn {
    id: String,
    name: String,
    slot: Option<String>,
    external_label: Option<String>,
    #[serde(default)]
    items: Vec<ItemIn>,
}

#[derive(Deserialize)]
struct ProgramIn {
    name: String,
    start_date: String,
    weeks: u32,
    gym_profile_id: Option<String>,
    #[serde(default = "yes")]
    warmups: bool,
    workouts: Vec<WorkoutIn>,
    #[serde(default)]
    overrides: Vec<WeekOverride>,
}

fn build_workouts(st: &AppState, ws: Vec<WorkoutIn>) -> ApiResult<HashMap<String, ProgramWorkout>> {
    if ws.len() > 30 {
        return Err(ApiError::bad("a program has at most 30 workouts"));
    }
    let known: HashSet<String> = st.read_fit(|s| s.list_exercises())?.into_iter().map(|e| e.id).collect();
    let (mut out, mut item_ids) = (HashMap::new(), HashSet::new());
    for (wi, w) in ws.into_iter().enumerate() {
        check_id(&w.id)?;
        check_text("workout name", &w.name, TITLE, true)?;
        check_opt("external_label", &w.external_label, TITLE)?;
        if w.items.len() > 40 {
            return Err(ApiError::bad("a workout has at most 40 items"));
        }
        let mut items = HashMap::new();
        for (ii, it) in w.items.into_iter().enumerate() {
            check_id(&it.id)?;
            if !item_ids.insert(it.id.clone()) {
                return Err(ApiError::bad(format!("item id {:?} is used twice; item IDs are unique within a program", it.id)));
            }
            if !known.contains(&it.exercise_id) {
                return Err(ApiError::bad(format!("unknown exercise_id {:?}", it.exercise_id)));
            }
            if let SetStyle::Unknown(s) = &it.style {
                return Err(ApiError::bad(format!("style {s:?}: expected Straight, DropSet or ToFailure")));
            }
            if it.sets > 50 || it.rep_max > 500 || it.rest_s.is_some_and(|r| r > 3600) {
                return Err(ApiError::bad("sets must be at most 50, reps at most 500, rest_s at most 3600"));
            }
            items.insert(
                it.id.clone(),
                ProgramItem { id: it.id, index: ii as u32, exercise_id: it.exercise_id, sets: it.sets, rep_min: it.rep_min, rep_max: it.rep_max, rir: it.rir, style: it.style, rest_s: it.rest_s },
            );
        }
        let pw = ProgramWorkout { id: w.id.clone(), index: wi as u32, name: w.name, slot: w.slot, external_label: w.external_label, items };
        if out.insert(w.id.clone(), pw).is_some() {
            return Err(ApiError::bad(format!("workout id {:?} is used twice", w.id)));
        }
    }
    Ok(out)
}

fn build_overrides(os: Vec<WeekOverride>) -> ApiResult<HashMap<String, WeekOverride>> {
    if os.len() > 1500 {
        return Err(ApiError::bad("a program has at most 1500 week overrides"));
    }
    let mut out = HashMap::new();
    for o in os {
        let key = format!("{}:{}", o.week, o.item_id);
        if out.insert(key.clone(), o).is_some() {
            return Err(ApiError::bad(format!("override {key} is listed twice")));
        }
    }
    Ok(out)
}

fn check_gym_ref(st: &AppState, id: &Option<String>) -> ApiResult<()> {
    match id {
        Some(g) => st.read_fit(|s| s.get_gym_profile(g)).map(|_| ()).map_err(|_| ApiError::bad(format!("unknown gym_profile_id {g:?}"))),
        None => Ok(()),
    }
}

/// The program with this ID, if any, must belong to `me`.
fn own_program(st: &AppState, id: &str, me: &str) -> ApiResult<Option<Program>> {
    match st.read_fit(|s| s.get_program(id)) {
        Ok(p) if p.user_id != me => Err(ApiError::forbidden("that program belongs to the other user")),
        Ok(p) => Ok(Some(p)),
        Err(e) if e.status == StatusCode::NOT_FOUND => Ok(None),
        Err(e) => Err(e),
    }
}

async fn list_programs(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<Vec<ProgramOut>>> {
    let user = target_user(&q, &me);
    Ok(Json(st.read_fit(|s| s.list_programs())?.into_iter().filter(|p| p.user_id == user).map(Into::into).collect()))
}

async fn get_program(State(st): State<AppState>, _me: Me, Path(id): Path<String>) -> ApiResult<Json<ProgramOut>> {
    Ok(Json(st.read_fit(|s| s.get_program(&id))?.into()))
}

async fn put_program(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(b): Body<ProgramIn>) -> ApiResult<Response> {
    check_id(&id)?;
    check_text("name", &b.name, TITLE, true)?;
    own_program(&st, &id, &me)?;
    check_gym_ref(&st, &b.gym_profile_id)?;
    let p = Program {
        id: id.clone(),
        user_id: me,
        name: b.name,
        start_date: b.start_date,
        weeks: b.weeks,
        gym_profile_id: b.gym_profile_id,
        warmups: b.warmups,
        workouts: build_workouts(&st, b.workouts)?,
        overrides: build_overrides(b.overrides)?,
        migrated_from: None,
        deleted: false,
    };
    let w = st.write_fit(|s| s.put_program(p))?;
    Ok(created(w == Write::Created, ProgramOut::from(st.read_fit(|s| s.get_program(&id))?)))
}

#[derive(Deserialize)]
struct ProgramPatch {
    name: Option<String>,
    start_date: Option<String>,
    weeks: Option<u32>,
    #[serde(default, deserialize_with = "some")]
    gym_profile_id: Option<Option<String>>,
    warmups: Option<bool>,
    /// Replaces all workouts (and their items).
    workouts: Option<Vec<WorkoutIn>>,
    /// Replaces all week overrides.
    overrides: Option<Vec<WeekOverride>>,
    deleted: Option<bool>,
}

async fn patch_program(State(st): State<AppState>, Me(me): Me, Path(id): Path<String>, Body(b): Body<ProgramPatch>) -> ApiResult<Json<ProgramOut>> {
    let mut p = own_program(&st, &id, &me)?.ok_or_else(|| not_found(&id))?;
    if let Some(n) = b.name {
        check_text("name", &n, TITLE, true)?;
        p.name = n;
    }
    p.start_date = b.start_date.unwrap_or(p.start_date);
    p.weeks = b.weeks.unwrap_or(p.weeks);
    if let Some(g) = b.gym_profile_id {
        check_gym_ref(&st, &g)?;
        p.gym_profile_id = g;
    }
    p.warmups = b.warmups.unwrap_or(p.warmups);
    if let Some(w) = b.workouts {
        p.workouts = build_workouts(&st, w)?;
    }
    if let Some(o) = b.overrides {
        p.overrides = build_overrides(o)?;
    }
    p.deleted = b.deleted.unwrap_or(p.deleted);
    st.write_fit(|s| s.update_program(p.clone()))?;
    Ok(Json(st.read_fit(|s| s.get_program(&id))?.into()))
}

/// Turns every v1 block (with its plan and templates) into a program, once; see
/// docs/DECISIONS.md "Training model (v2-M1)". Covers both users' blocks.
async fn migrate_v1(State(st): State<AppState>, _me: Me, Body(_): Body<serde_json::Value>) -> ApiResult<Json<Migration>> {
    Ok(Json(st.write_fit(|s| s.migrate_v1_blocks())?))
}

// ---- workouts and recommendations (derived) ----

async fn next_workout(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<NextWorkout>> {
    let date = date_or_today(&st, &q, "date")?;
    let user = target_user(&q, &me);
    let tz = st.0.tz;
    Ok(Json(st.read_fit(|s| s.next_workout(&user, date, &|ms| local_date(tz, ms)))?))
}

async fn recommendations(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<WorkoutPlan>> {
    let need = |k: &str| q.get(k).cloned().ok_or_else(|| ApiError::bad(format!("{k} is required")));
    let (program, workout) = (need("program_id")?, need("workout_id")?);
    let date = date_or_today(&st, &q, "date")?;
    let user = target_user(&q, &me);
    Ok(Json(st.read_fit(|s| s.plan_workout(&user, &program, &workout, date))?))
}

// ---- analytics (derived) ----

#[derive(Serialize)]
struct WeekVolume {
    /// The Monday.
    week_start: String,
    /// Sets per muscle (primary 1, secondary 0.5); all muscles present.
    sets: BTreeMap<String, f64>,
}

#[derive(Serialize)]
struct VolumeOut {
    /// Oldest first, ending with the week containing `week`.
    weeks: Vec<WeekVolume>,
}

async fn volume(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<VolumeOut>> {
    let user = target_user(&q, &me);
    let date = date_or_today(&st, &q, "week")?;
    let n = match q.get("weeks") {
        Some(n) => n.parse::<i64>().ok().filter(|n| (1..=26).contains(n)).ok_or_else(|| ApiError::bad("weeks must be 1-26"))?,
        None => 1,
    };
    let (sessions, exercises) = st.read_fit(|s| Ok((s.list_sessions()?, s.list_exercises()?)))?;
    let exercises: HashMap<_, _> = exercises.into_iter().map(|e| (e.id.clone(), e)).collect();
    let tz = st.0.tz;
    let end = week_start(date);
    let weeks = (0..n)
        .rev()
        .map(|i| {
            let ws = end - chrono::Duration::weeks(i);
            WeekVolume { week_start: ws.to_string(), sets: weekly_volume(&sessions, &exercises, &user, ws, &|ms| local_date(tz, ms)) }
        })
        .collect();
    Ok(Json(VolumeOut { weeks }))
}

#[derive(Serialize)]
struct E1rmOut {
    exercise_id: String,
    points: Vec<fitness_core::E1rmPoint>,
}

async fn e1rm(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<E1rmOut>> {
    let exercise_id = q.get("exercise_id").cloned().ok_or_else(|| ApiError::bad("exercise_id is required"))?;
    let user = target_user(&q, &me);
    let sessions = st.read_fit(|s| s.list_sessions())?;
    Ok(Json(E1rmOut { points: e1rm_series(&sessions, &user, &exercise_id), exercise_id }))
}

async fn get_prs(State(st): State<AppState>, Me(me): Me, Query(q): Q) -> ApiResult<Json<Vec<Pr>>> {
    let user = target_user(&q, &me);
    let sessions = st.read_fit(|s| s.list_sessions())?;
    Ok(Json(fitness_core::prs(&sessions, &user, q.get("exercise_id").map(String::as_str))))
}
