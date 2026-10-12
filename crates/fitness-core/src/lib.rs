//! Fitness data stored in an Automerge document.
//!
//! Doc shape:
//! `{ schema_version, exercises: {id: Exercise}, templates: {id: Template},
//!    sessions: {id: Session}, tests: {id: TestResult} }`.
//! Template items and session sets are maps keyed by ID with an explicit `index`.

use automerge::{transaction::CommitOptions, ActorId, AutomergeError, ObjType, ReadDoc, ROOT};
use autosurgeon::{hydrate, hydrate_prop, reconcile::NoKey, reconcile_prop, Hydrate, HydrateError, Reconcile, Reconciler};
use std::{cmp::Reverse, collections::HashMap, fmt, str::FromStr};

pub use automerge::Automerge;

pub const SCHEMA_VERSION: u64 = 1;

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

/// Beep test score. Derived `Ord` compares level then shuttle, so 8.10 > 8.5.
/// Never a float: as a float 8.10 == 8.1 < 8.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Reconcile, Hydrate)]
pub struct BeepScore {
    pub level: u8,
    pub shuttle: u8,
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

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct Exercise {
    pub id: String,
    pub name: String,
    pub measurement: MeasurementType,
    /// e.g. `2:1`.
    pub tempo: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct TemplateItem {
    pub index: u32,
    pub exercise_id: String,
    pub target_sets: u32,
    pub target_reps: Option<u32>,
    /// kg.
    pub target_load: Option<f64>,
    pub tempo: Option<String>,
    pub rest_s: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
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
}

#[derive(Debug, Clone, PartialEq, Default, Reconcile, Hydrate)]
pub struct SetLog {
    pub index: u32,
    pub exercise_id: String,
    pub reps: Option<u32>,
    /// kg.
    pub load: Option<f64>,
    pub duration_s: Option<u32>,
    pub distance_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
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
}

impl Session {
    pub fn ordered_sets(&self) -> Vec<&SetLog> {
        let mut v: Vec<_> = self.sets.values().collect();
        v.sort_by_key(|s| s.index);
        v
    }
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
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
    Automerge(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "{id} not found"),
            Error::NotOwner => write!(f, "only the owner can edit this"),
            Error::Invalid(s) => write!(f, "invalid {s}"),
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

/// Template item as entered by the user (ID and index are assigned by the store).
#[derive(Debug, Clone, PartialEq)]
pub struct NewTemplateItem {
    pub exercise_id: String,
    pub target_sets: u32,
    pub target_reps: Option<u32>,
    pub target_load: Option<f64>,
    pub tempo: Option<String>,
    pub rest_s: Option<u32>,
}

/// Domain operations. The seam if the CRDT choice changes.
/// `user_id` arguments enforce "only the owner edits their sessions/results".
pub trait FitnessStore {
    fn add_exercise(&mut self, name: &str, measurement: MeasurementType, tempo: Option<&str>) -> Result<Exercise>;
    /// Sorted by name.
    fn list_exercises(&self) -> Result<Vec<Exercise>>;
    fn add_template(&mut self, name: &str, target_minutes: u32, items: Vec<NewTemplateItem>) -> Result<Template>;
    /// Sorted by name.
    fn list_templates(&self) -> Result<Vec<Template>>;
    fn start_session(&mut self, user_id: &str, template_id: Option<&str>) -> Result<Session>;
    /// Appends a set; its `index` is assigned after the current last set.
    fn log_set(&mut self, user_id: &str, session_id: &str, set: SetLog) -> Result<String>;
    fn finish_session(&mut self, user_id: &str, session_id: &str, status: SessionStatus, note: Option<&str>) -> Result<()>;
    /// Newest first.
    fn list_sessions(&self) -> Result<Vec<Session>>;
    fn record_test(&mut self, result: TestResult) -> Result<TestResult>;
    /// Best first: highest beep/1RM/reps, lowest `TimedRun` time.
    fn list_tests(&self, kind: &TestKind) -> Result<Vec<TestResult>>;
}

pub struct AmFitnessStore {
    pub doc: Automerge,
}

impl AsMut<Automerge> for AmFitnessStore {
    fn as_mut(&mut self) -> &mut Automerge {
        &mut self.doc
    }
}

const COLLECTIONS: [&str; 4] = ["exercises", "templates", "sessions", "tests"];

/// Same idea as `todo_core::init_doc`: identical first change on every device
/// so the collection maps have the same object IDs everywhere.
pub fn init_doc() -> Automerge {
    let mut doc = Automerge::new().with_actor(ActorId::from([0u8; 16]));
    doc.transact_with::<_, _, AutomergeError, _>(
        |_| CommitOptions::default().with_time(0),
        |tx| {
            use automerge::transaction::Transactable;
            tx.put(ROOT, "schema_version", SCHEMA_VERSION)?;
            for c in COLLECTIONS {
                tx.put_object(ROOT, c, ObjType::Map)?;
            }
            Ok(())
        },
    )
    .expect("init on empty doc cannot fail");
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

impl AmFitnessStore {
    pub fn new() -> Self {
        Self { doc: init_doc() }
    }

    pub fn from_doc(mut doc: Automerge) -> Result<Self> {
        doc.merge(&mut init_doc())?;
        Ok(Self { doc })
    }

    fn collection(&self, name: &str) -> Result<automerge::ObjId> {
        match self.doc.get(ROOT, name)? {
            Some((_, id)) => Ok(id),
            None => Err(Error::Automerge(format!("missing {name} map"))),
        }
    }

    fn get<T: Hydrate>(&self, coll: &str, id: &str) -> Result<T> {
        let obj = self.collection(coll)?;
        if self.doc.get(&obj, id)?.is_none() {
            return Err(Error::NotFound(id.into()));
        }
        Ok(hydrate_prop(&self.doc, &obj, id)?)
    }

    fn put<T: Reconcile>(&mut self, coll: &str, id: &str, value: &T) -> Result<()> {
        let obj = self.collection(coll)?;
        self.doc
            .transact::<_, _, autosurgeon::ReconcileError>(|tx| reconcile_prop(tx, &obj, id, value))
            .map_err(|f| f.error)?;
        Ok(())
    }

    fn all(&self) -> Result<FitnessDoc> {
        Ok(hydrate(&self.doc)?)
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

impl FitnessStore for AmFitnessStore {
    fn add_exercise(&mut self, name: &str, measurement: MeasurementType, tempo: Option<&str>) -> Result<Exercise> {
        let e = Exercise { id: new_id(), name: name.into(), measurement, tempo: tempo.map(Into::into), deleted: false };
        self.put("exercises", &e.id, &e)?;
        Ok(e)
    }

    fn list_exercises(&self) -> Result<Vec<Exercise>> {
        let mut v: Vec<_> = self.all()?.exercises.into_values().filter(|e| !e.deleted).collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(v)
    }

    fn add_template(&mut self, name: &str, target_minutes: u32, items: Vec<NewTemplateItem>) -> Result<Template> {
        let items = items
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
                })
            })
            .collect();
        let t = Template { id: new_id(), name: name.into(), target_minutes, items, deleted: false };
        self.put("templates", &t.id, &t)?;
        Ok(t)
    }

    fn list_templates(&self) -> Result<Vec<Template>> {
        let mut v: Vec<_> = self.all()?.templates.into_values().filter(|t| !t.deleted).collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(v)
    }

    fn start_session(&mut self, user_id: &str, template_id: Option<&str>) -> Result<Session> {
        if let Some(t) = template_id {
            self.get::<Template>("templates", t)?;
        }
        let s = Session {
            id: new_id(),
            user_id: user_id.into(),
            template_id: template_id.map(Into::into),
            started_at: now_ms(),
            finished_at: None,
            status: SessionStatus::InProgress,
            note: None,
            sets: HashMap::new(),
            deleted: false,
        };
        self.put("sessions", &s.id, &s)?;
        Ok(s)
    }

    fn log_set(&mut self, user_id: &str, session_id: &str, mut set: SetLog) -> Result<String> {
        let mut s = self.owned_session(user_id, session_id)?;
        set.index = s.sets.values().map(|x| x.index + 1).max().unwrap_or(0);
        let id = new_id();
        s.sets.insert(id.clone(), set);
        self.put("sessions", session_id, &s)?;
        Ok(id)
    }

    fn finish_session(&mut self, user_id: &str, session_id: &str, status: SessionStatus, note: Option<&str>) -> Result<()> {
        if !matches!(status, SessionStatus::Completed | SessionStatus::Partial) {
            return Err(Error::Invalid("finish status, expected Completed or Partial".into()));
        }
        let mut s = self.owned_session(user_id, session_id)?;
        s.status = status;
        s.note = note.map(Into::into);
        s.finished_at = Some(now_ms());
        self.put("sessions", session_id, &s)
    }

    fn list_sessions(&self) -> Result<Vec<Session>> {
        let mut v: Vec<_> = self.all()?.sessions.into_values().filter(|s| !s.deleted).collect();
        v.sort_by_key(|s| Reverse((s.started_at, s.id.clone())));
        Ok(v)
    }

    fn record_test(&mut self, mut r: TestResult) -> Result<TestResult> {
        match (&r.kind, &r.beep, r.value) {
            (TestKind::BeepTest, Some(_), _) => r.value = None,
            (TestKind::BeepTest, None, _) => return Err(Error::Invalid("beep test needs a level.shuttle score".into())),
            (_, _, Some(v)) if v.is_finite() && v >= 0.0 => r.beep = None,
            _ => return Err(Error::Invalid("test result needs a non-negative value".into())),
        }
        r.id = new_id();
        r.deleted = false;
        self.put("tests", &r.id.clone(), &r)?;
        Ok(r)
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
    }

    #[test]
    fn template_session_partial() {
        let mut s = AmFitnessStore::new();
        let squat = s.add_exercise("Squat", MeasurementType::WeightReps, Some("2:1")).unwrap();
        let plank = s.add_exercise("Plank", MeasurementType::Timed, None).unwrap();
        let item = |e: &Exercise| NewTemplateItem {
            exercise_id: e.id.clone(), target_sets: 3, target_reps: Some(5), target_load: Some(100.0), tempo: None, rest_s: Some(120),
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
}
