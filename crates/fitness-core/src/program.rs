//! v2 training model: gym profiles and programs (schema 3). All weights in kg.
//! Programs replace v1 templates + weekly plans + blocks; v1 data is migrated by
//! `migrate.rs` and left in place.

use super::*;

/// Muscles for `Exercise::primary_muscles` / `secondary_muscles`. Fixed list.
pub const MUSCLES: [&str; 11] = ["chest", "back", "shoulders", "biceps", "triceps", "quads", "hamstrings", "glutes", "calves", "abs", "forearms"];

string_enum!(Equipment { Barbell, Dumbbell, Machine, Cable, Bodyweight, Other });
string_enum!(SetStyle { Straight, DropSet, ToFailure });
string_enum!(SetKind { Warmup, Working, Drop, Failure });

/// Per user. Plates are the plate weights available per side; plates are
/// assumed unlimited. Increments are for dumbbells and machines/cables.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
pub struct GymProfile {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub bar_kg: f64,
    pub plates_kg: Vec<f64>,
    pub dumbbell_step_kg: f64,
    pub machine_step_kg: f64,
    pub is_default: bool,
    pub deleted: bool,
}

/// One exercise slot in a workout: `sets` × `rep_min..=rep_max` at `rir` reps in reserve.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
pub struct ProgramItem {
    pub id: String,
    pub index: u32,
    pub exercise_id: String,
    pub sets: u32,
    pub rep_min: u32,
    pub rep_max: u32,
    pub rir: u8,
    pub style: SetStyle,
    pub rest_s: Option<u32>,
}

/// A workout within the program. `slot` pins it to a weekday and AM/PM (`mon_am`…`sun_pm`).
/// `external_label` marks an outside session (e.g. "Unit PT"): no items.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
pub struct ProgramWorkout {
    pub id: String,
    pub index: u32,
    pub name: String,
    pub slot: Option<String>,
    pub external_label: Option<String>,
    /// Keyed by item ID; order by `index`.
    pub items: HashMap<String, ProgramItem>,
}

/// Periodization: overrides one item in one week. `None` fields keep the item's value.
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
pub struct WeekOverride {
    pub week: u32,
    pub item_id: String,
    pub sets: Option<u32>,
    pub rep_min: Option<u32>,
    pub rep_max: Option<u32>,
    pub rir: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize, serde::Deserialize)]
pub struct Program {
    pub id: String,
    pub user_id: String,
    pub name: String,
    /// `YYYY-MM-DD`, a Monday.
    pub start_date: String,
    pub weeks: u32,
    pub gym_profile_id: Option<String>,
    pub workouts: HashMap<String, ProgramWorkout>,
    /// Keyed `"{week}:{item_id}"`.
    pub overrides: HashMap<String, WeekOverride>,
    /// v1 block ID, for programs made by `migrate_v1_blocks`.
    pub migrated_from: Option<String>,
    pub deleted: bool,
}

impl Program {
    pub fn ordered_workouts(&self) -> Vec<&ProgramWorkout> {
        let mut v: Vec<_> = self.workouts.values().collect();
        v.sort_by_key(|w| w.index);
        v
    }
}

impl ProgramWorkout {
    pub fn ordered_items(&self) -> Vec<&ProgramItem> {
        let mut v: Vec<_> = self.items.values().collect();
        v.sort_by_key(|i| i.index);
        v
    }
}

pub(crate) fn validate_gym(g: &GymProfile) -> Result<()> {
    let ok = |x: f64| x.is_finite() && x > 0.0;
    if !ok(g.bar_kg) || !ok(g.dumbbell_step_kg) || !ok(g.machine_step_kg) || g.plates_kg.iter().any(|p| !ok(*p)) {
        return Err(Error::Invalid("gym profile: weights and steps must be positive".into()));
    }
    Ok(())
}

pub(crate) fn validate_program(p: &Program) -> Result<()> {
    let start = parse_date(&p.start_date)?;
    if start.weekday() != Weekday::Mon {
        return Err(Error::Invalid(format!("start_date {}: programs start on a Monday", p.start_date)));
    }
    if p.weeks == 0 || p.weeks > 104 {
        return Err(Error::Invalid("weeks, must be 1..=104".into()));
    }
    let mut item_ids = std::collections::HashSet::new();
    for w in p.workouts.values() {
        if w.slot.as_deref().is_some_and(|s| !SLOT_KEYS.contains(&s)) {
            return Err(Error::Invalid(format!("workout slot {:?}, expected mon_am … sun_pm", w.slot)));
        }
        if w.external_label.is_some() && (!w.items.is_empty() || w.slot.is_none()) {
            return Err(Error::Invalid("external workout needs a slot and no items".into()));
        }
        for it in w.items.values() {
            if it.sets == 0 || it.rep_min == 0 || it.rep_min > it.rep_max || it.rir > 5 {
                return Err(Error::Invalid(format!("item {}: sets >= 1, 1 <= rep_min <= rep_max, rir <= 5", it.id)));
            }
            item_ids.insert(it.id.clone());
        }
    }
    for (key, o) in &p.overrides {
        if *key != format!("{}:{}", o.week, o.item_id) || o.week == 0 || o.week > p.weeks || !item_ids.contains(&o.item_id) {
            return Err(Error::Invalid(format!("override {key}: week within the program and an item of this program")));
        }
        if o.rir.is_some_and(|r| r > 5) || o.sets == Some(0) {
            return Err(Error::Invalid(format!("override {key}: rir <= 5, sets >= 1")));
        }
    }
    Ok(())
}
