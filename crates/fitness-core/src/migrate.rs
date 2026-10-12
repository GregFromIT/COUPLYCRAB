//! v1 → v2: blocks (+ the owner's weekly plan + templates) become Programs.
//! Pure, and every ID is derived from v1 IDs, so two devices migrating the same
//! block produce identical programs.

use super::*;

/// Result of `migrate_v1_blocks`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Migration {
    /// Programs written by this call (0 when everything was already migrated).
    pub created: usize,
    /// What the migration could not carry over, one line each.
    pub notes: Vec<String>,
}

/// Reps used when a v1 template item had no `target_reps`.
const DEFAULT_REPS: u32 = 8;
/// RIR given to migrated items (v1 had none).
const DEFAULT_RIR: u8 = 2;
/// Deload week: half the sets (rounded up) at this RIR.
const DELOAD_RIR: u8 = 4;

pub(crate) fn v1_to_programs(
    blocks: &[Block],
    plans: &HashMap<String, WeeklyPlan>,
    templates: &HashMap<String, Template>,
) -> (Vec<Program>, Vec<String>) {
    let mut notes = vec![];
    let programs = blocks
        .iter()
        .filter(|b| !b.deleted)
        .map(|b| {
            let mut workouts = HashMap::new();
            let mut overrides = HashMap::new();
            let plan = plans.get(&b.user_id);
            for (index, key) in SLOT_KEYS.iter().enumerate() {
                let Some(slot) = plan.and_then(|p| p.slots.get(*key)) else { continue };
                let mut w = ProgramWorkout {
                    id: (*key).into(),
                    index: index as u32,
                    name: String::new(),
                    slot: Some((*key).into()),
                    external_label: slot.external_label.clone(),
                    items: HashMap::new(),
                };
                match (&slot.template_id, &slot.external_label) {
                    (_, Some(label)) => w.name = label.clone(),
                    (Some(t), None) => {
                        let Some(t) = templates.get(t).filter(|t| !t.deleted) else { continue };
                        w.name = t.name.clone();
                        for (i, it) in t.ordered_items().into_iter().enumerate() {
                            let id = format!("{key}-{i}");
                            let reps = it.target_reps.unwrap_or_else(|| {
                                notes.push(format!("{}: item without target reps got {DEFAULT_REPS}", t.name));
                                DEFAULT_REPS
                            });
                            if it.progression.is_some() {
                                notes.push(format!("{}: linear progression dropped (v2 progresses from logged history)", t.name));
                            }
                            if it.target_load.is_some() {
                                notes.push(format!("{}: fixed target loads dropped (v2 recommends loads from history)", t.name));
                            }
                            if let Some(wk) = b.deload_week {
                                let key = format!("{wk}:{id}");
                                overrides.insert(
                                    key,
                                    WeekOverride { week: wk, item_id: id.clone(), sets: Some(it.target_sets.div_ceil(2)), rep_min: None, rep_max: None, rir: Some(DELOAD_RIR) },
                                );
                            }
                            w.items.insert(
                                id.clone(),
                                ProgramItem {
                                    id,
                                    index: i as u32,
                                    exercise_id: it.exercise_id.clone(),
                                    sets: it.target_sets,
                                    rep_min: reps,
                                    rep_max: reps,
                                    rir: DEFAULT_RIR,
                                    style: SetStyle::Straight,
                                    rest_s: it.rest_s,
                                },
                            );
                        }
                    }
                    (None, None) => continue,
                }
                workouts.insert(w.id.clone(), w);
            }
            if b.deload_week.is_some() {
                notes.push("deload load percentage dropped; deload week is half the sets at 4 RIR".into());
            }
            Program {
                id: format!("v1-{}", b.id),
                user_id: b.user_id.clone(),
                name: b.name.clone(),
                start_date: b.start_date.clone(),
                weeks: b.weeks,
                gym_profile_id: None,
                warmups: true,
                workouts,
                overrides,
                migrated_from: Some(b.id.clone()),
                deleted: false,
            }
        })
        .collect();
    (programs, notes)
}
