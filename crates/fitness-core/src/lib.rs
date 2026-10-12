//! Fitness data stored in an Automerge document.
//!
//! Doc shape:
//! `{ schema_version, exercises: {id: Exercise}, templates: {id: Template},
//!    sessions: {id: Session}, tests: {id: TestResult},
//!    plans: {user_id: WeeklyPlan}, blocks: {id: Block},            (schema 2)
//!    external_activities: {"source:source_id": ExternalActivity},   (schema 2)
//!    programs: {id: Program}, gym_profiles: {id: GymProfile} }`       (schema 3, v2)
//! Template items and session sets are maps keyed by ID with an explicit `index`.

mod plan;

pub use plan::{BlockContext, PlannedDay, PlannedItem, PlannedSlot, PlannedTemplate, SLOT_KEYS};

use automerge::{transaction::CommitOptions, ActorId, AutomergeError, ObjType, ReadDoc, ROOT};
use autosurgeon::{hydrate, hydrate_prop, reconcile::NoKey, reconcile_prop, Hydrate, HydrateError, Reconcile, Reconciler};
use chrono::{Datelike, NaiveDate, Weekday};
use std::{cmp::Reverse, collections::HashMap, fmt, str::FromStr};

pub use automerge::Automerge;

/// 1: v0. 2: v1 adds `plans`, `blocks`, `external_activities`, template item
/// `progression`, session `block_id`/`block_week`/`slot`/`label`.
/// 3: v2 adds `programs`, `gym_profiles`, exercise muscles/equipment/rest, set
/// `kind`/`rir`/`partials`/`rest_taken_s`/`recommendation_id`, session `program_id`/`workout_id`.
pub const SCHEMA_VERSION: u64 = 3;

/// String-backed enums: unknown strings (from newer builds) hydrate to
/// `Unknown(s)` and are written back unchanged, instead of failing.
macro_rules! string_enum {
    ($name:ident { $($v:ident),* }) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name { $($v,)* Unknown(String) }
        impl $name {
            pub fn as_str(&self) -> &str {
                match self { $(Self::$v => stringify!($v),)* Self::Unknown(s) => s }
            }
        }
        impl FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                Ok(match s { $(stringify!($v) => Self::$v,)* other => Self::Unknown(other.into()) })
            }
        }
        impl Reconcile for $name {
            type Key<'a> = NoKey;
            fn reconcile<R: Reconciler>(&self, mut r: R) -> std::result::Result<(), R::Error> {
                r.str(self.as_str())
            }
        }
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                Ok(String::deserialize(d)?.parse().unwrap())
            }
        }
        impl Hydrate for $name {
            fn hydrate_string(s: &str) -> std::result::Result<Self, HydrateError> {
                Ok(s.parse().unwrap())
            }
        }
    };
}

string_enum!(MeasurementType { WeightReps, Reps, Timed, Distance });
string_enum!(SessionStatus { InProgress, Completed, Partial });
string_enum!(TestKind { BeepTest, OneRepMax, MaxReps, TimedRun });
string_enum!(ProgressionKind { LinearLoad, LinearReps });

mod analytics;
mod migrate;
mod progression;
mod program;
pub use analytics::{e1rm_series, prs, week_start, weekly_volume, E1rmPoint, Pr};
pub use migrate::Migration;
pub use progression::{
    default_gym, epley, load_for, prescription, program_week, recommend, round_down_load, warmups, ItemPlan, Prescription, Recommendation, WarmupSet,
    WorkoutPlan, ASSUMED_RIR,
};
pub use program::{Equipment, GymProfile, Program, ProgramItem, ProgramWorkout, SetKind, SetStyle, WeekOverride, MUSCLES};

/// Shuttles per level, levels 1..=21, of the common 20 m beep test (level 1 at
/// 8.0 km/h). Source: https://www.topendsports.com/testing/beep-table.htm
/// Only used to plot scores; storage and sorting stay `(level, shuttle)`.
pub const SHUTTLES_PER_LEVEL: [u8; 21] = [7, 8, 8, 9, 9, 10, 10, 11, 11, 11, 12, 12, 13, 13, 13, 14, 14, 15, 15, 16, 16];

/// Beep test score. Derived `Ord` compares level then shuttle, so 8.10 > 8.5.
/// Never a float: as a float 8.10 == 8.1 < 8.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Reconcile, Hydrate, serde::Serialize)]
pub struct BeepScore {
    pub level: u8,
    pub shuttle: u8,
}

impl BeepScore {
    pub fn shuttles_in_level(level: u8) -> Option<u8> {
        SHUTTLES_PER_LEVEL.get(usize::from(level).checked_sub(1)?).copied()
    }

    /// y-axis value for charts: `level + shuttle / shuttles_in_level`.
    pub fn plot_value(&self) -> f64 {
        let n = Self::shuttles_in_level(self.level).unwrap_or(16);
        f64::from(self.level) + f64::from(self.shuttle.min(n)) / f64::from(n)
    }
}

impl fmt::Display for BeepScore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.level, self.shuttle)
    }
}

impl FromStr for BeepScore {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::Invalid(format!("beep score {s:?}, expected level.shuttle"));
        let (l, sh) = s.split_once('.').ok_or_else(bad)?;
        Ok(BeepScore { level: l.parse().map_err(|_| bad())?, shuttle: sh.parse().map_err(|_| bad())? })
    }
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct Exercise {
    pub id: String,
    pub name: String,
    pub measurement: MeasurementType,
    /// e.g. `2:1`.
    pub tempo: Option<String>,
    pub deleted: bool,
    /// Schema 3 (absent in older docs, hence `missing`). `MUSCLES` values.
    #[autosurgeon(missing = "Default::default")]
    pub primary_muscles: Vec<String>,
    #[autosurgeon(missing = "Default::default")]
    pub secondary_muscles: Vec<String>,
    #[autosurgeon(missing = "Default::default")]
    pub equipment: Option<Equipment>,
    #[autosurgeon(missing = "Default::default")]
    pub default_rest_s: Option<u32>,
}

impl Exercise {
    pub fn new(id: String, name: &str, measurement: MeasurementType) -> Self {
        Exercise {
            id,
            name: name.into(),
            measurement,
            tempo: None,
            deleted: false,
            primary_muscles: vec![],
            secondary_muscles: vec![],
            equipment: None,
            default_rest_s: None,
        }
    }
}

/// `LinearLoad`: `per_week` kg added per non-deload week.
/// `LinearReps`: `per_week` reps added per non-deload week (rounded down).
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
pub struct Progression {
    pub kind: ProgressionKind,
    pub per_week: f64,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct TemplateItem {
    pub index: u32,
    pub exercise_id: String,
    pub target_sets: u32,
    pub target_reps: Option<u32>,
    /// kg.
    pub target_load: Option<f64>,
    pub tempo: Option<String>,
    pub rest_s: Option<u32>,
    /// Schema 2. Absent in v0 docs, hence `missing`.
    #[autosurgeon(missing = "Default::default")]
    pub progression: Option<Progression>,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub target_minutes: u32,
    /// Keyed by item ID; order by `index`.
    pub items: HashMap<String, TemplateItem>,
    pub deleted: bool,
}

impl Template {
    pub fn ordered_items(&self) -> Vec<&TemplateItem> {
        let mut v: Vec<_> = self.items.values().collect();
        v.sort_by_key(|i| i.index);
        v
    }

    /// Build items from an ordered list; item IDs are generated.
    pub fn items_from(items: Vec<NewTemplateItem>) -> HashMap<String, TemplateItem> {
        items
            .into_iter()
            .enumerate()
            .map(|(i, it)| {
                (new_id(), TemplateItem {
                    index: i as u32,
                    exercise_id: it.exercise_id,
                    target_sets: it.target_sets,
                    target_reps: it.target_reps,
                    target_load: it.target_load,
                    tempo: it.tempo,
                    rest_s: it.rest_s,
                    progression: it.progression,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SetLog {
    pub index: u32,
    pub exercise_id: String,
    pub reps: Option<u32>,
    /// kg.
    pub load: Option<f64>,
    pub duration_s: Option<u32>,
    pub distance_m: Option<f64>,
    /// Schema 3 (absent in older docs, hence `missing`).
    #[autosurgeon(missing = "Default::default")]
    pub kind: Option<SetKind>,
    #[autosurgeon(missing = "Default::default")]
    pub rir: Option<u8>,
    #[autosurgeon(missing = "Default::default")]
    pub partials: Option<u8>,
    #[autosurgeon(missing = "Default::default")]
    pub rest_taken_s: Option<u32>,
    /// ID of the recommendation shown when the set was logged (`recommendations` are derived, not stored).
    #[autosurgeon(missing = "Default::default")]
    pub recommendation_id: Option<String>,
}

impl SetLog {
    /// Same logged values, ignoring the server-assigned `index`.
    fn same(&self, o: &SetLog) -> bool {
        (&self.exercise_id, self.reps, self.load, self.duration_s, self.distance_m, &self.kind, self.rir, self.partials, self.rest_taken_s, &self.recommendation_id)
            == (&o.exercise_id, o.reps, o.load, o.duration_s, o.distance_m, &o.kind, o.rir, o.partials, o.rest_taken_s, &o.recommendation_id)
    }
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub template_id: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: SessionStatus,
    pub note: Option<String>,
    /// Keyed by set ID; order by `index`.
    pub sets: HashMap<String, SetLog>,
    pub deleted: bool,
    /// Schema 2: the block, block week and slot (`AM`/`PM`) this session was planned in.
    /// Every schema-2 field needs `missing`: autosurgeon's derived `Hydrate`
    /// errors on an absent key, even for `Option`.
    #[autosurgeon(missing = "Default::default")]
    pub block_id: Option<String>,
    #[autosurgeon(missing = "Default::default")]
    pub block_week: Option<u32>,
    #[autosurgeon(missing = "Default::default")]
    pub slot: Option<String>,
    /// Schema 2: label of an external session (e.g. "Unit PT") logged as attended.
    #[autosurgeon(missing = "Default::default")]
    pub label: Option<String>,
    /// Schema 3: the program and workout this session was started from.
    #[autosurgeon(missing = "Default::default")]
    pub program_id: Option<String>,
    #[autosurgeon(missing = "Default::default")]
    pub workout_id: Option<String>,
}

impl Session {
    pub fn ordered_sets(&self) -> Vec<&SetLog> {
        let mut v: Vec<_> = self.sets.values().collect();
        v.sort_by_key(|s| s.index);
        v
    }

    /// A new in-progress session started now.
    pub fn start(id: String, user_id: &str) -> Session {
        Session {
            id,
            user_id: user_id.into(),
            template_id: None,
            started_at: now_ms(),
            finished_at: None,
            status: SessionStatus::InProgress,
            note: None,
            sets: HashMap::new(),
            deleted: false,
            block_id: None,
            block_week: None,
            slot: None,
            label: None,
            program_id: None,
            workout_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct TestResult {
    pub id: String,
    pub user_id: String,
    pub kind: TestKind,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// Only for `BeepTest`.
    pub beep: Option<BeepScore>,
    /// `OneRepMax`: kg, `MaxReps`: reps, `TimedRun`: seconds.
    pub value: Option<f64>,
    /// Which exercise a `OneRepMax` / `MaxReps` was for.
    pub exercise_id: Option<String>,
    pub deleted: bool,
}

/// One weekly slot: a template, or an external session with a label.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
pub struct PlanSlot {
    pub template_id: Option<String>,
    pub external_label: Option<String>,
}

/// Per-user weekly plan. Slot keys are [`SLOT_KEYS`] (`mon_am` … `sun_pm`);
/// missing keys are empty slots.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct WeeklyPlan {
    pub user_id: String,
    pub slots: HashMap<String, PlanSlot>,
}

/// A program block repeats its owner's weekly plan for `weeks` weeks.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct Block {
    pub id: String,
    pub user_id: String,
    pub name: String,
    /// `YYYY-MM-DD`, a Monday.
    pub start_date: String,
    pub weeks: u32,
    /// 1-based week number that is a deload week.
    pub deload_week: Option<u32>,
    /// Deload load as a percentage of the progressed load, e.g. `60.0`.
    pub deload_pct: Option<f64>,
    pub deleted: bool,
}

/// Activity from an outside source (watch, app). Keyed by `(source, source_id)`.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct ExternalActivity {
    pub source: String,
    pub source_id: String,
    pub user_id: String,
    pub started_at: i64,
    pub duration_s: u32,
    pub distance_m: Option<f64>,
    pub linked_session_id: Option<String>,
}

#[derive(Debug, Hydrate)]
struct FitnessDoc {
    exercises: HashMap<String, Exercise>,
    templates: HashMap<String, Template>,
    sessions: HashMap<String, Session>,
    tests: HashMap<String, TestResult>,
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    NotOwner,
    Invalid(String),
    /// A `put_*` with an existing ID but different content.
    Conflict(String),
    Automerge(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "{id} not found"),
            Error::NotOwner => write!(f, "only the owner can edit this"),
            Error::Invalid(s) => write!(f, "invalid {s}"),
            Error::Conflict(id) => write!(f, "{id} already exists with different content"),
            Error::Automerge(e) => write!(f, "automerge: {e}"),
        }
    }
}
impl std::error::Error for Error {}

macro_rules! from_err {
    ($($t:ty),*) => {$(impl From<$t> for Error { fn from(e: $t) -> Self { Error::Automerge(e.to_string()) } })*};
}
from_err!(AutomergeError, HydrateError, autosurgeon::ReconcileError);

pub type Result<T> = std::result::Result<T, Error>;

/// Outcome of an idempotent write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Created,
    Updated,
    /// Same content was already there; nothing written.
    Unchanged,
}

/// Template item as entered by the user (ID and index are assigned by the store).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct NewTemplateItem {
    pub exercise_id: String,
    pub target_sets: u32,
    pub target_reps: Option<u32>,
    pub target_load: Option<f64>,
    pub tempo: Option<String>,
    pub rest_s: Option<u32>,
    #[serde(default)]
    pub progression: Option<Progression>,
}

/// Domain operations. The seam if the CRDT choice changes.
///
/// `put_*` methods create with a caller-chosen ID (e.g. a browser's
/// `crypto.randomUUID()`): `Created`, or `Unchanged` if an item with that ID
/// and the same content exists, or `Error::Conflict` if the content differs.
/// That makes retries safe. `update_*` replace an existing item.
pub trait FitnessStore {
    /// Sorted by name.
    fn list_exercises(&self) -> Result<Vec<Exercise>>;
    fn get_exercise(&self, id: &str) -> Result<Exercise>;
    fn put_exercise(&mut self, e: Exercise) -> Result<Write>;
    fn update_exercise(&mut self, e: Exercise) -> Result<()>;

    /// Sorted by name.
    fn list_templates(&self) -> Result<Vec<Template>>;
    fn get_template(&self, id: &str) -> Result<Template>;
    fn put_template(&mut self, t: Template) -> Result<Write>;
    fn update_template(&mut self, t: Template) -> Result<()>;

    fn get_plan(&self, user_id: &str) -> Result<Option<WeeklyPlan>>;
    /// Replaces the user's plan.
    fn set_plan(&mut self, plan: WeeklyPlan) -> Result<Write>;

    /// All users' blocks, by start date.
    fn list_blocks(&self) -> Result<Vec<Block>>;
    fn get_block(&self, id: &str) -> Result<Block>;
    fn put_block(&mut self, b: Block) -> Result<Write>;
    fn update_block(&mut self, b: Block) -> Result<()>;

    /// Concrete targets for one user's day: progression and deload applied.
    /// Outside any block: no slots and no block context.
    fn planned_for(&self, user_id: &str, date: NaiveDate) -> Result<PlannedDay>;

    /// All users, newest first.
    fn list_sessions(&self) -> Result<Vec<Session>>;
    fn get_session(&self, id: &str) -> Result<Session>;
    /// Create; "same content" compares owner, template, block link, slot and label.
    fn put_session(&mut self, s: Session) -> Result<Write>;
    /// Add a set with a caller-chosen ID; `index` is assigned after the current last set.
    fn put_set(&mut self, user_id: &str, session_id: &str, set_id: &str, set: SetLog) -> Result<Write>;
    /// Idempotent: finishing again with the same status and note changes nothing.
    fn finish_session(&mut self, user_id: &str, session_id: &str, status: SessionStatus, note: Option<&str>) -> Result<()>;

    /// Best first: highest beep/1RM/reps, lowest `TimedRun` time.
    fn list_tests(&self, kind: &TestKind) -> Result<Vec<TestResult>>;
    fn put_test(&mut self, r: TestResult) -> Result<Write>;

    /// Insert or replace by `(source, source_id)`.
    fn upsert_external(&mut self, a: ExternalActivity) -> Result<Write>;

    /// A user's profiles, default first then by name.
    fn list_gym_profiles(&self, user_id: &str) -> Result<Vec<GymProfile>>;
    fn get_gym_profile(&self, id: &str) -> Result<GymProfile>;
    fn put_gym_profile(&mut self, g: GymProfile) -> Result<Write>;
    /// Replaces; setting `is_default` clears it on the user's other profiles.
    fn update_gym_profile(&mut self, g: GymProfile) -> Result<()>;

    /// All users' programs, by start date.
    fn list_programs(&self) -> Result<Vec<Program>>;
    fn get_program(&self, id: &str) -> Result<Program>;
    fn put_program(&mut self, p: Program) -> Result<Write>;
    fn update_program(&mut self, p: Program) -> Result<()>;

    /// Turns each v1 block (with its owner's plan and templates) into a Program
    /// with ID `v1-<block id>`. Idempotent; an existing program is never overwritten.
    fn migrate_v1_blocks(&mut self) -> Result<Migration>;

    /// The user's program whose mesocycle contains `date`, and the week number (the latest start wins).
    fn active_program(&self, user_id: &str, date: NaiveDate) -> Result<Option<(Program, u32)>> {
        Ok(self
            .list_programs()?
            .into_iter()
            .filter(|p| p.user_id == user_id)
            .filter_map(|p| program_week(&p, date).map(|w| (p, w)))
            .max_by(|(a, _), (b, _)| (&a.start_date, &a.id).cmp(&(&b.start_date, &b.id))))
    }

    /// The gym profile a program uses: its own, else the user's default, else [`default_gym`].
    fn gym_for(&self, program: &Program) -> Result<GymProfile> {
        if let Some(g) = program.gym_profile_id.as_deref().and_then(|id| self.get_gym_profile(id).ok()).filter(|g| !g.deleted) {
            return Ok(g);
        }
        Ok(self.list_gym_profiles(&program.user_id)?.into_iter().next().unwrap_or_else(|| default_gym(&program.user_id)))
    }

    /// One workout with prescriptions for `date`'s week, recommendations (each with its
    /// reason) and warm-ups. Derived from the session log; nothing is stored.
    fn plan_workout(&self, user_id: &str, program_id: &str, workout_id: &str, date: NaiveDate) -> Result<WorkoutPlan> {
        let program = self.get_program(program_id)?;
        if program.user_id != user_id {
            return Err(Error::NotOwner);
        }
        let workout = program.workouts.get(workout_id).ok_or_else(|| Error::NotFound(workout_id.into()))?;
        let exercises: HashMap<String, Exercise> = self.list_exercises()?.into_iter().map(|e| (e.id.clone(), e)).collect();
        Ok(progression::plan_workout(&program, workout, date, &exercises, &self.list_sessions()?, &self.gym_for(&program)?))
    }

    // ---- v0 conveniences: the store generates the ID ----

    fn add_exercise(&mut self, name: &str, measurement: MeasurementType, tempo: Option<&str>) -> Result<Exercise> {
        let e = Exercise { tempo: tempo.map(Into::into), ..Exercise::new(new_id(), name, measurement) };
        self.put_exercise(e.clone())?;
        Ok(e)
    }

    fn add_template(&mut self, name: &str, target_minutes: u32, items: Vec<NewTemplateItem>) -> Result<Template> {
        let t = Template { id: new_id(), name: name.into(), target_minutes, items: Template::items_from(items), deleted: false };
        self.put_template(t.clone())?;
        Ok(t)
    }

    fn start_session(&mut self, user_id: &str, template_id: Option<&str>) -> Result<Session> {
        let mut s = Session::start(new_id(), user_id);
        s.template_id = template_id.map(Into::into);
        self.put_session(s.clone())?;
        Ok(s)
    }

    fn log_set(&mut self, user_id: &str, session_id: &str, set: SetLog) -> Result<String> {
        let id = new_id();
        self.put_set(user_id, session_id, &id, set)?;
        Ok(id)
    }

    fn record_test(&mut self, mut r: TestResult) -> Result<TestResult> {
        r.id = new_id();
        self.put_test(r.clone())?;
        Ok(self.list_tests(&r.kind)?.into_iter().find(|t| t.id == r.id).unwrap_or(r))
    }
}

pub struct AmFitnessStore {
    pub doc: Automerge,
}

impl AsMut<Automerge> for AmFitnessStore {
    fn as_mut(&mut self) -> &mut Automerge {
        &mut self.doc
    }
}

/// Same idea as `todo_core::init_doc`: identical changes on every device so
/// the collection maps have the same object IDs everywhere.
///
/// Change 1 is byte-for-byte v0's init (schema 1). Change 2 (schema 2) adds the
/// v1 collections; change 3 (schema 3) adds the v2 ones. Each is deterministic
/// (same actor, next seq, time 0, deps = the previous change), so an older doc
/// that merges them and a fresh doc agree on the maps.
pub fn init_doc() -> Automerge {
    use automerge::transaction::Transactable;
    let mut doc = Automerge::new().with_actor(ActorId::from([0u8; 16]));
    let opts = |_: &_| CommitOptions::default().with_time(0);
    doc.transact_with::<_, _, AutomergeError, _>(opts, |tx| {
        tx.put(ROOT, "schema_version", 1u64)?;
        for c in ["exercises", "templates", "sessions", "tests"] {
            tx.put_object(ROOT, c, ObjType::Map)?;
        }
        Ok(())
    })
    .expect("init on empty doc cannot fail");
    doc.transact_with::<_, _, AutomergeError, _>(opts, |tx| {
        tx.put(ROOT, "schema_version", 2u64)?;
        for c in ["plans", "blocks", "external_activities"] {
            tx.put_object(ROOT, c, ObjType::Map)?;
        }
        Ok(())
    })
    .expect("init on fresh doc cannot fail");
    doc.transact_with::<_, _, AutomergeError, _>(opts, |tx| {
        tx.put(ROOT, "schema_version", 3u64)?;
        for c in ["programs", "gym_profiles"] {
            tx.put_object(ROOT, c, ObjType::Map)?;
        }
        Ok(())
    })
    .expect("init on fresh doc cannot fail");
    doc.set_actor(ActorId::random());
    doc
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Error::Invalid(format!("date {s:?}, expected YYYY-MM-DD")))
}

impl AmFitnessStore {
    pub fn new() -> Self {
        Self { doc: init_doc() }
    }

    pub fn from_doc(mut doc: Automerge) -> Result<Self> {
        doc.merge(&mut init_doc())?;
        Ok(Self { doc })
    }

    pub fn schema_version(&self) -> Result<Option<u64>> {
        Ok(hydrate_prop(&self.doc, ROOT, "schema_version")?)
    }

    fn collection(&self, name: &str) -> Result<automerge::ObjId> {
        match self.doc.get(ROOT, name)? {
            Some((_, id)) => Ok(id),
            None => Err(Error::Automerge(format!("missing {name} map"))),
        }
    }

    fn find<T: Hydrate>(&self, coll: &str, id: &str) -> Result<Option<T>> {
        let obj = self.collection(coll)?;
        if self.doc.get(&obj, id)?.is_none() {
            return Ok(None);
        }
        Ok(Some(hydrate_prop(&self.doc, &obj, id)?))
    }

    fn get<T: Hydrate>(&self, coll: &str, id: &str) -> Result<T> {
        self.find(coll, id)?.ok_or_else(|| Error::NotFound(id.into()))
    }

    fn put<T: Reconcile>(&mut self, coll: &str, id: &str, value: &T) -> Result<()> {
        let obj = self.collection(coll)?;
        self.doc
            .transact::<_, _, autosurgeon::ReconcileError>(|tx| reconcile_prop(tx, &obj, id, value))
            .map_err(|f| f.error)?;
        Ok(())
    }

    /// Create-or-identical: the core of every `put_*`.
    fn create<T: Reconcile + Hydrate>(&mut self, coll: &str, id: &str, value: &T, same: impl Fn(&T, &T) -> bool) -> Result<Write> {
        match self.find::<T>(coll, id)? {
            Some(existing) if same(&existing, value) => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(id.into())),
            None => self.put(coll, id, value).map(|_| Write::Created),
        }
    }

    fn update<T: Reconcile + Hydrate>(&mut self, coll: &str, id: &str, value: &T) -> Result<()> {
        self.get::<T>(coll, id)?;
        self.put(coll, id, value)
    }

    fn all(&self) -> Result<FitnessDoc> {
        Ok(hydrate(&self.doc)?)
    }

    fn all_of<T: Hydrate>(&self, coll: &str) -> Result<HashMap<String, T>> {
        Ok(hydrate_prop(&self.doc, ROOT, coll)?)
    }

    fn clear_other_defaults(&mut self, keep: &GymProfile) -> Result<()> {
        for other in self.list_gym_profiles(&keep.user_id)? {
            if other.id != keep.id && other.is_default {
                self.put("gym_profiles", &other.id.clone(), &GymProfile { is_default: false, ..other })?;
            }
        }
        Ok(())
    }

    fn check_program(&self, p: &Program) -> Result<()> {
        program::validate_program(p)?;
        if let Some(g) = &p.gym_profile_id {
            if self.get_gym_profile(g)?.user_id != p.user_id {
                return Err(Error::NotOwner);
            }
        }
        for it in p.workouts.values().flat_map(|w| w.items.values()) {
            self.get_exercise(&it.exercise_id)?;
        }
        Ok(())
    }

    fn owned_session(&self, user_id: &str, session_id: &str) -> Result<Session> {
        let s: Session = self.get("sessions", session_id)?;
        if s.user_id != user_id {
            return Err(Error::NotOwner);
        }
        Ok(s)
    }
}

impl Default for AmFitnessStore {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_exercise(e: &Exercise) -> Result<()> {
    for m in e.primary_muscles.iter().chain(&e.secondary_muscles) {
        if !MUSCLES.contains(&m.as_str()) {
            return Err(Error::Invalid(format!("muscle {m:?}, expected one of {}", MUSCLES.join(", "))));
        }
    }
    Ok(())
}

fn validate_template(t: &Template) -> Result<()> {
    for it in t.items.values() {
        if let Some(p) = &it.progression {
            if !p.per_week.is_finite() || p.per_week < 0.0 {
                return Err(Error::Invalid("progression per_week, must be >= 0".into()));
            }
        }
    }
    Ok(())
}

fn same_template(a: &Template, b: &Template) -> bool {
    let items = |t: &Template| {
        t.ordered_items().into_iter().map(|i| TemplateItem { index: 0, ..i.clone() }).collect::<Vec<_>>()
    };
    (&a.name, a.target_minutes, a.deleted) == (&b.name, b.target_minutes, b.deleted) && items(a) == items(b)
}

fn validate_block(b: &Block) -> Result<()> {
    let start = parse_date(&b.start_date)?;
    if start.weekday() != Weekday::Mon {
        return Err(Error::Invalid(format!("start_date {}: blocks start on a Monday", b.start_date)));
    }
    if b.weeks == 0 || b.weeks > 104 {
        return Err(Error::Invalid("weeks, must be 1..=104".into()));
    }
    if b.deload_week.is_some_and(|w| w == 0 || w > b.weeks) {
        return Err(Error::Invalid("deload_week, must be within the block".into()));
    }
    if b.deload_pct.is_some_and(|p| !(p > 0.0 && p <= 100.0)) {
        return Err(Error::Invalid("deload_pct, must be in (0, 100]".into()));
    }
    if b.deload_week.is_some() != b.deload_pct.is_some() {
        return Err(Error::Invalid("deload: set both deload_week and deload_pct, or neither".into()));
    }
    Ok(())
}

fn validate_test(r: &mut TestResult) -> Result<()> {
    parse_date(&r.date)?;
    match (&r.kind, r.beep, r.value) {
        (TestKind::BeepTest, Some(b), _) => {
            match BeepScore::shuttles_in_level(b.level) {
                Some(n) if (1..=n).contains(&b.shuttle) => {}
                _ => return Err(Error::Invalid(format!("beep score {b}: level 1-21, shuttle within the level"))),
            }
            r.value = None;
        }
        (TestKind::BeepTest, None, _) => return Err(Error::Invalid("beep test needs a level.shuttle score".into())),
        (_, _, Some(v)) if v.is_finite() && v >= 0.0 => r.beep = None,
        _ => return Err(Error::Invalid("test result needs a non-negative value".into())),
    }
    Ok(())
}

impl FitnessStore for AmFitnessStore {
    fn list_exercises(&self) -> Result<Vec<Exercise>> {
        let mut v: Vec<_> = self.all()?.exercises.into_values().filter(|e| !e.deleted).collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(v)
    }

    fn get_exercise(&self, id: &str) -> Result<Exercise> {
        self.get("exercises", id)
    }

    fn put_exercise(&mut self, e: Exercise) -> Result<Write> {
        validate_exercise(&e)?;
        self.create("exercises", &e.id.clone(), &e, |a, b| a == b)
    }

    fn update_exercise(&mut self, e: Exercise) -> Result<()> {
        validate_exercise(&e)?;
        self.update("exercises", &e.id.clone(), &e)
    }

    fn list_templates(&self) -> Result<Vec<Template>> {
        let mut v: Vec<_> = self.all()?.templates.into_values().filter(|t| !t.deleted).collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(v)
    }

    fn get_template(&self, id: &str) -> Result<Template> {
        self.get("templates", id)
    }

    fn put_template(&mut self, t: Template) -> Result<Write> {
        validate_template(&t)?;
        self.create("templates", &t.id.clone(), &t, same_template)
    }

    fn update_template(&mut self, t: Template) -> Result<()> {
        validate_template(&t)?;
        self.update("templates", &t.id.clone(), &t)
    }

    fn get_plan(&self, user_id: &str) -> Result<Option<WeeklyPlan>> {
        self.find("plans", user_id)
    }

    fn set_plan(&mut self, plan: WeeklyPlan) -> Result<Write> {
        for (key, slot) in &plan.slots {
            if !SLOT_KEYS.contains(&key.as_str()) {
                return Err(Error::Invalid(format!("plan slot {key:?}, expected mon_am … sun_pm")));
            }
            if slot.template_id.is_some() == slot.external_label.is_some() {
                return Err(Error::Invalid(format!("plan slot {key}: set exactly one of template_id or external_label")));
            }
            if let Some(t) = &slot.template_id {
                self.get_template(t)?;
            }
        }
        let existing = self.get_plan(&plan.user_id)?;
        if existing.as_ref() == Some(&plan) {
            return Ok(Write::Unchanged);
        }
        self.put("plans", &plan.user_id.clone(), &plan)?;
        Ok(if existing.is_some() { Write::Updated } else { Write::Created })
    }

    fn list_blocks(&self) -> Result<Vec<Block>> {
        let mut v: Vec<Block> = self.all_of::<Block>("blocks")?.into_values().filter(|b| !b.deleted).collect();
        v.sort_by(|a, b| (&a.start_date, &a.id).cmp(&(&b.start_date, &b.id)));
        Ok(v)
    }

    fn get_block(&self, id: &str) -> Result<Block> {
        self.get("blocks", id)
    }

    fn put_block(&mut self, b: Block) -> Result<Write> {
        validate_block(&b)?;
        self.create("blocks", &b.id.clone(), &b, |a, b| a == b)
    }

    fn update_block(&mut self, b: Block) -> Result<()> {
        validate_block(&b)?;
        self.update("blocks", &b.id.clone(), &b)
    }

    fn planned_for(&self, user_id: &str, date: NaiveDate) -> Result<PlannedDay> {
        let blocks: Vec<Block> = self.list_blocks()?.into_iter().filter(|b| b.user_id == user_id).collect();
        let Some((block, week)) = plan::active_block(&blocks, date) else {
            return Ok(PlannedDay { date: date.to_string(), block: None, slots: vec![] });
        };
        let plan = self.get_plan(user_id)?;
        let doc = self.all()?;
        Ok(plan::resolve(block, week, date, plan.as_ref(), &doc.templates, &doc.exercises))
    }

    fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut v: Vec<_> = self.all()?.sessions.into_values().filter(|s| !s.deleted).collect();
        v.sort_by_key(|s| Reverse((s.started_at, s.id.clone())));
        Ok(v)
    }

    fn get_session(&self, id: &str) -> Result<Session> {
        self.get("sessions", id)
    }

    fn put_session(&mut self, s: Session) -> Result<Write> {
        if let Some(t) = &s.template_id {
            self.get_template(t)?;
        }
        if let Some(b) = &s.block_id {
            if self.get_block(b)?.user_id != s.user_id {
                return Err(Error::NotOwner);
            }
        }
        if let Some(p) = &s.program_id {
            if self.get_program(p)?.user_id != s.user_id {
                return Err(Error::NotOwner);
            }
        }
        let key = |s: &Session| {
            (s.user_id.clone(), s.template_id.clone(), s.block_id.clone(), s.block_week, s.slot.clone(), s.label.clone(), s.program_id.clone(), s.workout_id.clone())
        };
        self.create("sessions", &s.id.clone(), &s, |a, b| key(a) == key(b))
    }

    fn put_set(&mut self, user_id: &str, session_id: &str, set_id: &str, mut set: SetLog) -> Result<Write> {
        let mut s = self.owned_session(user_id, session_id)?;
        if let Some(existing) = s.sets.get(set_id) {
            return if existing.same(&set) { Ok(Write::Unchanged) } else { Err(Error::Conflict(set_id.into())) };
        }
        set.index = s.sets.values().map(|x| x.index + 1).max().unwrap_or(0);
        s.sets.insert(set_id.into(), set);
        self.put("sessions", session_id, &s)?;
        Ok(Write::Created)
    }

    fn finish_session(&mut self, user_id: &str, session_id: &str, status: SessionStatus, note: Option<&str>) -> Result<()> {
        if !matches!(status, SessionStatus::Completed | SessionStatus::Partial) {
            return Err(Error::Invalid("finish status, expected Completed or Partial".into()));
        }
        let mut s = self.owned_session(user_id, session_id)?;
        if s.status == status && s.note.as_deref() == note && s.finished_at.is_some() {
            return Ok(());
        }
        s.status = status;
        s.note = note.map(Into::into);
        s.finished_at = Some(now_ms());
        self.put("sessions", session_id, &s)
    }

    fn list_tests(&self, kind: &TestKind) -> Result<Vec<TestResult>> {
        let mut v: Vec<_> = self.all()?.tests.into_values().filter(|t| !t.deleted && &t.kind == kind).collect();
        let val = |t: &TestResult| t.value.unwrap_or(0.0);
        match kind {
            TestKind::BeepTest => v.sort_by_key(|t| Reverse(t.beep)),
            TestKind::TimedRun => v.sort_by(|a, b| val(a).total_cmp(&val(b))),
            _ => v.sort_by(|a, b| val(b).total_cmp(&val(a))),
        }
        Ok(v)
    }

    fn put_test(&mut self, mut r: TestResult) -> Result<Write> {
        validate_test(&mut r)?;
        r.deleted = false;
        self.create("tests", &r.id.clone(), &r, |a, b| a == b)
    }

    fn list_gym_profiles(&self, user_id: &str) -> Result<Vec<GymProfile>> {
        let mut v: Vec<_> = self.all_of::<GymProfile>("gym_profiles")?.into_values().filter(|g| !g.deleted && g.user_id == user_id).collect();
        v.sort_by(|a, b| (!a.is_default, a.name.to_lowercase(), &a.id).cmp(&(!b.is_default, b.name.to_lowercase(), &b.id)));
        Ok(v)
    }

    fn get_gym_profile(&self, id: &str) -> Result<GymProfile> {
        self.get("gym_profiles", id)
    }

    fn put_gym_profile(&mut self, g: GymProfile) -> Result<Write> {
        program::validate_gym(&g)?;
        let w = self.create("gym_profiles", &g.id.clone(), &g, |a, b| a == b)?;
        if w == Write::Created && g.is_default {
            self.clear_other_defaults(&g)?;
        }
        Ok(w)
    }

    fn update_gym_profile(&mut self, g: GymProfile) -> Result<()> {
        program::validate_gym(&g)?;
        self.update("gym_profiles", &g.id.clone(), &g)?;
        if g.is_default {
            self.clear_other_defaults(&g)?;
        }
        Ok(())
    }

    fn list_programs(&self) -> Result<Vec<Program>> {
        let mut v: Vec<_> = self.all_of::<Program>("programs")?.into_values().filter(|p| !p.deleted).collect();
        v.sort_by(|a, b| (&a.start_date, &a.id).cmp(&(&b.start_date, &b.id)));
        Ok(v)
    }

    fn get_program(&self, id: &str) -> Result<Program> {
        self.get("programs", id)
    }

    fn put_program(&mut self, p: Program) -> Result<Write> {
        self.check_program(&p)?;
        self.create("programs", &p.id.clone(), &p, |a, b| a == b)
    }

    fn update_program(&mut self, p: Program) -> Result<()> {
        self.check_program(&p)?;
        self.update("programs", &p.id.clone(), &p)
    }

    fn migrate_v1_blocks(&mut self) -> Result<Migration> {
        let (programs, mut notes) = migrate::v1_to_programs(
            &self.list_blocks()?,
            &self.all_of::<WeeklyPlan>("plans")?,
            &self.all_of::<Template>("templates")?,
        );
        let mut created = 0;
        for p in programs {
            if self.find::<Program>("programs", &p.id)?.is_some() {
                continue;
            }
            let from = p.migrated_from.clone();
            self.put_program(p).map_err(|e| Error::Invalid(format!("migrating block {from:?}: {e}")))?;
            created += 1;
        }
        // Notes describe what this call dropped; a re-run that created nothing dropped nothing.
        if created == 0 {
            notes.clear();
        }
        notes.sort();
        notes.dedup();
        Ok(Migration { created, notes })
    }

    fn upsert_external(&mut self, a: ExternalActivity) -> Result<Write> {
        if a.source.is_empty() || a.source.contains(':') || a.source_id.is_empty() {
            return Err(Error::Invalid("external activity: source (no ':') and source_id are required".into()));
        }
        let key = format!("{}:{}", a.source, a.source_id);
        match self.find::<ExternalActivity>("external_activities", &key)? {
            Some(e) if e == a => Ok(Write::Unchanged),
            Some(e) if e.user_id != a.user_id => Err(Error::NotOwner),
            existing => {
                self.put("external_activities", &key, &a)?;
                Ok(if existing.is_some() { Write::Updated } else { Write::Created })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beep(s: &str) -> BeepScore {
        s.parse().unwrap()
    }

    fn result(kind: TestKind, user: &str, beep: Option<&str>, value: Option<f64>) -> TestResult {
        TestResult {
            id: String::new(),
            user_id: user.into(),
            kind,
            date: "2026-10-12".into(),
            beep: beep.map(|b| b.parse().unwrap()),
            value,
            exercise_id: None,
            deleted: false,
        }
    }

    #[test]
    fn beep_ordering() {
        assert!(beep("8.10") > beep("8.5"));
        assert!(beep("9.1") > beep("8.12"));
        assert!(beep("8.5") == BeepScore { level: 8, shuttle: 5 });
        for s in ["8.10", "8.5", "12.1", "0.0"] {
            assert_eq!(beep(s).to_string(), s);
        }
        for bad in ["8", "8.", ".5", "8.5.1", "x.1", "300.1", "-1.2"] {
            assert!(bad.parse::<BeepScore>().is_err(), "{bad}");
        }
    }

    #[test]
    fn beep_plot_value_uses_shuttle_table() {
        // Level 8 has 11 shuttles: 8.10 plots above 8.5, below 9.1.
        assert!((beep("8.10").plot_value() - (8.0 + 10.0 / 11.0)).abs() < 1e-9);
        assert!(beep("8.10").plot_value() > beep("8.5").plot_value());
        assert!(beep("9.1").plot_value() > beep("8.10").plot_value());
        assert_eq!(BeepScore::shuttles_in_level(1), Some(7));
        assert_eq!(BeepScore::shuttles_in_level(21), Some(16));
        assert_eq!(BeepScore::shuttles_in_level(0), None);
        assert_eq!(BeepScore::shuttles_in_level(22), None);
    }

    #[test]
    fn tests_sorted_best_first() {
        let mut s = AmFitnessStore::new();
        for b in ["8.5", "9.1", "8.10"] {
            s.record_test(result(TestKind::BeepTest, "u", Some(b), None)).unwrap();
        }
        for v in [300.0, 280.5, 310.0] {
            s.record_test(result(TestKind::TimedRun, "u", None, Some(v))).unwrap();
        }
        for v in [100.0, 120.0, 90.0] {
            s.record_test(result(TestKind::OneRepMax, "u", None, Some(v))).unwrap();
        }
        let beeps: Vec<_> = s.list_tests(&TestKind::BeepTest).unwrap().iter().map(|t| t.beep.unwrap().to_string()).collect();
        assert_eq!(beeps, ["9.1", "8.10", "8.5"]);
        let vals = |k| s.list_tests(&k).unwrap().iter().map(|t| t.value.unwrap()).collect::<Vec<_>>();
        assert_eq!(vals(TestKind::TimedRun), [280.5, 300.0, 310.0]);
        assert_eq!(vals(TestKind::OneRepMax), [120.0, 100.0, 90.0]);
        assert!(vals(TestKind::MaxReps).is_empty());
        assert!(s.record_test(result(TestKind::BeepTest, "u", None, Some(8.5))).is_err());
        assert!(s.record_test(result(TestKind::MaxReps, "u", None, Some(f64::NAN))).is_err());
        assert!(s.record_test(result(TestKind::BeepTest, "u", Some("8.12"), None)).is_err(), "level 8 has 11 shuttles");
        assert!(s.record_test(result(TestKind::BeepTest, "u", Some("0.1"), None)).is_err());
    }

    #[test]
    fn template_session_partial() {
        let mut s = AmFitnessStore::new();
        let squat = s.add_exercise("Squat", MeasurementType::WeightReps, Some("2:1")).unwrap();
        let plank = s.add_exercise("Plank", MeasurementType::Timed, None).unwrap();
        let item = |e: &Exercise| NewTemplateItem {
            exercise_id: e.id.clone(), target_sets: 3, target_reps: Some(5), target_load: Some(100.0), tempo: None, rest_s: Some(120), progression: None,
        };
        let t = s.add_template("Legs", 45, vec![item(&squat), item(&plank)]).unwrap();
        let order: Vec<_> = s.list_templates().unwrap()[0].ordered_items().iter().map(|i| i.exercise_id.clone()).collect();
        assert_eq!(order, [squat.id.clone(), plank.id.clone()]);

        let sess = s.start_session("me", Some(&t.id)).unwrap();
        s.log_set("me", &sess.id, SetLog { exercise_id: squat.id.clone(), reps: Some(5), load: Some(100.0), ..Default::default() }).unwrap();
        s.log_set("me", &sess.id, SetLog { exercise_id: plank.id.clone(), duration_s: Some(60), ..Default::default() }).unwrap();
        assert!(matches!(s.log_set("partner", &sess.id, SetLog::default()), Err(Error::NotOwner)));
        assert!(s.finish_session("me", &sess.id, SessionStatus::InProgress, None).is_err());
        s.finish_session("me", &sess.id, SessionStatus::Partial, Some("ran out of time")).unwrap();

        let got = &s.list_sessions().unwrap()[0];
        assert_eq!(got.status, SessionStatus::Partial);
        assert_eq!(got.note.as_deref(), Some("ran out of time"));
        assert!(got.finished_at.is_some());
        let idx: Vec<_> = got.ordered_sets().iter().map(|x| (x.index, x.exercise_id.clone())).collect();
        assert_eq!(idx, [(0, squat.id), (1, plank.id)]);
        assert!(s.start_session("me", Some("missing")).is_err());
    }

    #[test]
    fn concurrent_sessions_from_both_users_merge() {
        let mut a = AmFitnessStore::new();
        let ex = a.add_exercise("Pushup", MeasurementType::Reps, None).unwrap();
        let mut b = AmFitnessStore::from_doc(a.doc.fork()).unwrap();
        for (store, user) in [(&mut a, "alice"), (&mut b, "bob")] {
            let s = store.start_session(user, None).unwrap();
            store.log_set(user, &s.id, SetLog { exercise_id: ex.id.clone(), reps: Some(20), ..Default::default() }).unwrap();
            store.finish_session(user, &s.id, SessionStatus::Completed, None).unwrap();
        }
        a.doc.merge(&mut b.doc).unwrap();
        b.doc.merge(&mut a.doc).unwrap();
        let heads = |d: &Automerge| { let mut h = d.get_heads(); h.sort(); h };
        assert_eq!(heads(&a.doc), heads(&b.doc));
        let (sa, sb) = (a.list_sessions().unwrap(), b.list_sessions().unwrap());
        assert_eq!(sa, sb);
        let mut users: Vec<_> = sa.iter().map(|s| s.user_id.as_str()).collect();
        users.sort();
        assert_eq!(users, ["alice", "bob"]);
    }

    #[test]
    fn fresh_devices_converge() {
        let mut a = AmFitnessStore::new();
        let mut b = AmFitnessStore::from_doc(Automerge::new()).unwrap();
        a.add_exercise("A", MeasurementType::Reps, None).unwrap();
        b.add_exercise("B", MeasurementType::Reps, None).unwrap();
        a.doc.merge(&mut b.doc).unwrap();
        assert_eq!(a.list_exercises().unwrap().len(), 2);
    }

    #[test]
    fn unknown_enum_variant_survives_roundtrip() {
        let mut s = AmFitnessStore::new();
        let mut e = s.add_exercise("Swim", MeasurementType::Distance, None).unwrap();
        // A newer build wrote a variant this build doesn't know.
        e.measurement = MeasurementType::Unknown("Laps".into());
        s.put("exercises", &e.id.clone(), &e).unwrap();
        let got = &s.list_exercises().unwrap()[0];
        assert_eq!(got.measurement, MeasurementType::Unknown("Laps".into()));
        assert_eq!(got.measurement.as_str(), "Laps");
    }

    #[test]
    fn put_is_idempotent_and_conflicts_on_different_content() {
        let mut s = AmFitnessStore::new();
        let e = Exercise::new("e1".into(), "Row", MeasurementType::Reps);
        assert_eq!(s.put_exercise(e.clone()).unwrap(), Write::Created);
        let heads = s.doc.get_heads();
        assert_eq!(s.put_exercise(e.clone()).unwrap(), Write::Unchanged);
        assert_eq!(s.doc.get_heads(), heads, "no new change on a repeated put");
        assert!(matches!(s.put_exercise(Exercise { name: "Other".into(), ..e }), Err(Error::Conflict(_))));

        let sess = Session::start("s1".into(), "me");
        assert_eq!(s.put_session(sess.clone()).unwrap(), Write::Created);
        // Retried later: started_at differs, but the client-chosen fields match.
        assert_eq!(s.put_session(Session { started_at: 1, ..sess }).unwrap(), Write::Unchanged);
        let set = SetLog { exercise_id: "e1".into(), reps: Some(8), ..Default::default() };
        assert_eq!(s.put_set("me", "s1", "set1", set.clone()).unwrap(), Write::Created);
        assert_eq!(s.put_set("me", "s1", "set1", set.clone()).unwrap(), Write::Unchanged);
        assert_eq!(s.get_session("s1").unwrap().sets.len(), 1);
        assert!(matches!(s.put_set("me", "s1", "set1", SetLog { reps: Some(9), ..set }), Err(Error::Conflict(_))));

        s.finish_session("me", "s1", SessionStatus::Completed, Some("n")).unwrap();
        let heads = s.doc.get_heads();
        s.finish_session("me", "s1", SessionStatus::Completed, Some("n")).unwrap();
        assert_eq!(s.doc.get_heads(), heads, "repeated finish is a no-op");
    }
}
