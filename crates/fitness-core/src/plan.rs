//! `planned_for`: weekly plan + block + progression → concrete targets for one day.
//! Pure functions of the stored data and the date, so every device (and the
//! server) computes the same answer. See docs/LEARNING.md "Deterministic scheduling".

use crate::{Block, Exercise, Progression, ProgressionKind, Template, WeeklyPlan};
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;

/// Weekly plan slot keys, Monday AM first.
pub const SLOT_KEYS: [&str; 14] = [
    "mon_am", "mon_pm", "tue_am", "tue_pm", "wed_am", "wed_pm", "thu_am", "thu_pm",
    "fri_am", "fri_pm", "sat_am", "sat_pm", "sun_am", "sun_pm",
];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PlannedDay {
    pub date: String,
    /// `None` outside any block (and then `slots` is empty).
    pub block: Option<BlockContext>,
    pub slots: Vec<PlannedSlot>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BlockContext {
    pub block_id: String,
    pub name: String,
    /// 1-based: "week `week` of `weeks`".
    pub week: u32,
    pub weeks: u32,
    pub is_deload: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PlannedSlot {
    /// `AM` or `PM`.
    pub slot: String,
    pub template: Option<PlannedTemplate>,
    pub external_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PlannedTemplate {
    pub template_id: String,
    pub name: String,
    pub target_minutes: u32,
    pub items: Vec<PlannedItem>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PlannedItem {
    pub exercise_id: String,
    pub exercise_name: Option<String>,
    pub target_sets: u32,
    pub target_reps: Option<u32>,
    /// kg, progression and deload applied, rounded to 0.01.
    pub target_load: Option<f64>,
    pub tempo: Option<String>,
    pub rest_s: Option<u32>,
}

/// The block containing `date` and the 1-based week number. If blocks overlap,
/// the one that started last wins (ties: higher ID), so the answer is stable.
pub(crate) fn active_block(blocks: &[Block], date: NaiveDate) -> Option<(&Block, u32)> {
    blocks
        .iter()
        .filter_map(|b| week_of(b, date).map(|w| (b, w)))
        .max_by(|(a, _), (b, _)| (&a.start_date, &a.id).cmp(&(&b.start_date, &b.id)))
}

/// Week number of `date` in `block`, or `None` if outside it.
pub(crate) fn week_of(block: &Block, date: NaiveDate) -> Option<u32> {
    week_index(&block.start_date, block.weeks, date)
}

/// 1-based week of `date` in a run of `weeks` weeks starting `start_date`, or `None` outside it.
/// Shared by v1 blocks and v2 programs.
pub(crate) fn week_index(start_date: &str, weeks: u32, date: NaiveDate) -> Option<u32> {
    let start = NaiveDate::parse_from_str(start_date, "%Y-%m-%d").ok()?;
    let days = (date - start).num_days();
    if days < 0 || days >= i64::from(weeks) * 7 {
        return None;
    }
    Some((days / 7) as u32 + 1)
}

/// Non-deload weeks completed before `week`: how far progression has advanced.
pub(crate) fn progressed_weeks(week: u32, deload_week: Option<u32>) -> u32 {
    (week - 1) - u32::from(deload_week.is_some_and(|d| d < week))
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

pub(crate) fn resolve(
    block: &Block,
    week: u32,
    date: NaiveDate,
    plan: Option<&WeeklyPlan>,
    templates: &HashMap<String, Template>,
    exercises: &HashMap<String, Exercise>,
) -> PlannedDay {
    let is_deload = block.deload_week == Some(week);
    let steps = f64::from(progressed_weeks(week, block.deload_week));
    let deload = if is_deload { block.deload_pct.map(|p| p / 100.0) } else { None };
    let day = &SLOT_KEYS[date.weekday().num_days_from_monday() as usize * 2][..3];

    let mut slots = vec![];
    for (half, label) in [("am", "AM"), ("pm", "PM")] {
        let Some(slot) = plan.and_then(|p| p.slots.get(&format!("{day}_{half}"))) else { continue };
        // A slot pointing at a missing/deleted template is skipped, not an error.
        let template = slot.template_id.as_ref().and_then(|id| templates.get(id)).filter(|t| !t.deleted).map(|t| {
            let items = t
                .ordered_items()
                .into_iter()
                .map(|it| {
                    let (mut load, mut reps) = (it.target_load, it.target_reps);
                    match &it.progression {
                        Some(Progression { kind: ProgressionKind::LinearLoad, per_week }) => {
                            load = load.map(|l| l + per_week * steps);
                        }
                        Some(Progression { kind: ProgressionKind::LinearReps, per_week }) => {
                            reps = reps.map(|r| r + (per_week * steps).floor() as u32);
                        }
                        _ => {} // none, or a kind from a newer build: base targets
                    }
                    if let Some(f) = deload {
                        load = load.map(|l| l * f);
                    }
                    PlannedItem {
                        exercise_id: it.exercise_id.clone(),
                        exercise_name: exercises.get(&it.exercise_id).map(|e| e.name.clone()),
                        target_sets: it.target_sets,
                        target_reps: reps,
                        target_load: load.map(round2),
                        tempo: it.tempo.clone(),
                        rest_s: it.rest_s,
                    }
                })
                .collect();
            PlannedTemplate { template_id: t.id.clone(), name: t.name.clone(), target_minutes: t.target_minutes, items }
        });
        if template.is_none() && slot.external_label.is_none() {
            continue;
        }
        slots.push(PlannedSlot { slot: label.into(), template, external_label: slot.external_label.clone() });
    }

    PlannedDay {
        date: date.to_string(),
        block: Some(BlockContext { block_id: block.id.clone(), name: block.name.clone(), week, weeks: block.weeks, is_deload }),
        slots,
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn block(id: &str, user: &str, start: &str, weeks: u32, deload: Option<(u32, f64)>) -> Block {
        Block {
            id: id.into(),
            user_id: user.into(),
            name: format!("Block {id}"),
            start_date: start.into(),
            weeks,
            deload_week: deload.map(|x| x.0),
            deload_pct: deload.map(|x| x.1),
            deleted: false,
        }
    }

    /// Store with: squat (LinearLoad +2.5 kg/wk from 100 kg × 5),
    /// pushup (LinearReps +1/wk from 10), a template, and `user`'s plan with
    /// the template on Monday AM and "Unit PT" on Monday PM.
    fn setup(user: &str) -> (AmFitnessStore, String) {
        let mut s = AmFitnessStore::new();
        let squat = s.add_exercise("Squat", MeasurementType::WeightReps, None).unwrap();
        let push = s.add_exercise("Pushup", MeasurementType::Reps, None).unwrap();
        let item = |e: &Exercise, reps, load, kind, per_week| NewTemplateItem {
            exercise_id: e.id.clone(), target_sets: 3, target_reps: Some(reps), target_load: load, tempo: None, rest_s: None,
            progression: Some(Progression { kind, per_week }),
        };
        let t = s
            .add_template("Strength", 50, vec![
                item(&squat, 5, Some(100.0), ProgressionKind::LinearLoad, 2.5),
                item(&push, 10, None, ProgressionKind::LinearReps, 1.0),
            ])
            .unwrap();
        plan(&mut s, user, &t.id);
        (s, t.id)
    }

    fn plan(s: &mut AmFitnessStore, user: &str, template_id: &str) {
        let slots = HashMap::from([
            ("mon_am".to_string(), PlanSlot { template_id: Some(template_id.into()), external_label: None }),
            ("mon_pm".to_string(), PlanSlot { template_id: None, external_label: Some("Unit PT".into()) }),
        ]);
        s.set_plan(WeeklyPlan { user_id: user.into(), slots }).unwrap();
    }

    /// (squat load, pushup reps) planned for `date`.
    fn targets(s: &AmFitnessStore, user: &str, date: &str) -> (Option<f64>, Option<u32>) {
        let day = s.planned_for(user, d(date)).unwrap();
        let items = &day.slots[0].template.as_ref().unwrap().items;
        (items[0].target_load, items[1].target_reps)
    }

    #[test]
    fn week_index_edges() {
        // 2026-10-05 is a Monday. 4 weeks: through Sunday 2026-11-01.
        let b = block("b", "u", "2026-10-05", 4, None);
        assert_eq!(super::week_of(&b, d("2026-10-05")), Some(1), "day 1");
        assert_eq!(super::week_of(&b, d("2026-10-11")), Some(1), "sunday of week 1");
        assert_eq!(super::week_of(&b, d("2026-10-12")), Some(2));
        assert_eq!(super::week_of(&b, d("2026-11-01")), Some(4), "last day");
        assert_eq!(super::week_of(&b, d("2026-11-02")), None, "day after end");
        assert_eq!(super::week_of(&b, d("2026-10-04")), None, "day before start");
    }

    #[test]
    fn outside_a_block_is_an_empty_day() {
        let (s, _) = setup("u");
        let day = s.planned_for("u", d("2026-10-05")).unwrap();
        assert_eq!(day, PlannedDay { date: "2026-10-05".into(), block: None, slots: vec![] });
    }

    #[test]
    fn linear_progression_weeks_1_4_8() {
        let (mut s, _) = setup("u");
        s.put_block(block("b", "u", "2026-10-05", 8, None)).unwrap();
        // Mondays of weeks 1, 4, 8.
        assert_eq!(targets(&s, "u", "2026-10-05"), (Some(100.0), Some(10)));
        assert_eq!(targets(&s, "u", "2026-10-26"), (Some(107.5), Some(13)));
        assert_eq!(targets(&s, "u", "2026-11-23"), (Some(117.5), Some(17)));
        let ctx = s.planned_for("u", d("2026-11-23")).unwrap().block.unwrap();
        assert_eq!((ctx.week, ctx.weeks, ctx.is_deload), (8, 8, false));
    }

    #[test]
    fn deload_applies_pct_and_does_not_advance_progression() {
        let (mut s, _) = setup("u");
        s.put_block(block("b", "u", "2026-10-05", 8, Some((4, 60.0)))).unwrap();
        // Week 3: 2 steps. Week 4 (deload): still 3 steps' worth of base, × 60 %, reps not reduced.
        assert_eq!(targets(&s, "u", "2026-10-19"), (Some(105.0), Some(12)));
        assert_eq!(targets(&s, "u", "2026-10-26"), (Some(64.5), Some(13)));
        assert!(s.planned_for("u", d("2026-10-26")).unwrap().block.unwrap().is_deload);
        // Week 5: the deload week didn't count, so 3 steps (not 4).
        assert_eq!(targets(&s, "u", "2026-11-02"), (Some(107.5), Some(13)));
        // Week 8: 6 steps.
        assert_eq!(targets(&s, "u", "2026-11-23"), (Some(115.0), Some(16)));
    }

    #[test]
    fn external_slot_passes_through() {
        let (mut s, _) = setup("u");
        s.put_block(block("b", "u", "2026-10-05", 4, None)).unwrap();
        let day = s.planned_for("u", d("2026-10-12")).unwrap();
        assert_eq!(day.slots.len(), 2);
        assert_eq!((day.slots[0].slot.as_str(), day.slots[1].slot.as_str()), ("AM", "PM"));
        assert_eq!(day.slots[1], PlannedSlot { slot: "PM".into(), template: None, external_label: Some("Unit PT".into()) });
        // Tuesday has nothing planned, but is still inside the block.
        let tue = s.planned_for("u", d("2026-10-13")).unwrap();
        assert!(tue.slots.is_empty() && tue.block.is_some());
    }

    #[test]
    fn two_users_different_blocks_same_date() {
        let (mut s, t) = setup("alice");
        plan(&mut s, "bob", &t);
        s.put_block(block("a", "alice", "2026-10-05", 8, None)).unwrap();
        s.put_block(block("b", "bob", "2026-10-19", 4, Some((1, 50.0)))).unwrap();
        let date = "2026-10-19";
        assert_eq!(targets(&s, "alice", date), (Some(105.0), Some(12)), "alice week 3");
        assert_eq!(targets(&s, "bob", date), (Some(50.0), Some(10)), "bob week 1, deload");
        assert_eq!(s.planned_for("alice", d(date)).unwrap().block.unwrap().name, "Block a");
        assert_eq!(s.planned_for("bob", d(date)).unwrap().block.unwrap().name, "Block b");
        assert!(s.planned_for("carol", d(date)).unwrap().block.is_none());
    }

    #[test]
    fn block_validation() {
        let mut s = AmFitnessStore::new();
        assert!(s.put_block(block("x", "u", "2026-10-06", 4, None)).is_err(), "Tuesday start");
        assert!(s.put_block(block("x", "u", "2026-10-05", 0, None)).is_err());
        assert!(s.put_block(block("x", "u", "2026-10-05", 4, Some((5, 60.0)))).is_err());
        assert!(s.put_block(block("x", "u", "2026-10-05", 4, Some((4, 0.0)))).is_err());
        assert!(s.put_block(block("x", "u", "2026-10-05", 4, Some((4, 60.0)))).is_ok());
    }

    #[test]
    fn plan_validation() {
        let (mut s, t) = setup("u");
        let bad = |key: &str, slot: PlanSlot| WeeklyPlan { user_id: "u".into(), slots: HashMap::from([(key.to_string(), slot)]) };
        let tpl = PlanSlot { template_id: Some(t.clone()), external_label: None };
        assert!(s.set_plan(bad("monday", tpl.clone())).is_err());
        assert!(s.set_plan(bad("mon_am", PlanSlot { template_id: Some(t), external_label: Some("x".into()) })).is_err());
        assert!(s.set_plan(bad("mon_am", PlanSlot { template_id: None, external_label: None })).is_err());
        assert!(s.set_plan(bad("mon_am", PlanSlot { template_id: Some("missing".into()), external_label: None })).is_err());
        assert_eq!(s.set_plan(bad("tue_pm", tpl.clone())).unwrap(), Write::Updated);
        assert_eq!(s.set_plan(bad("tue_pm", tpl)).unwrap(), Write::Unchanged);
    }

    #[test]
    fn external_activity_upsert_is_idempotent() {
        let mut s = AmFitnessStore::new();
        let a = ExternalActivity {
            source: "garmin".into(), source_id: "123".into(), user_id: "u".into(),
            started_at: 1_700_000_000_000, duration_s: 1800, distance_m: Some(5000.0), linked_session_id: None,
        };
        assert_eq!(s.upsert_external(a.clone()).unwrap(), Write::Created);
        let heads = s.doc.get_heads();
        assert_eq!(s.upsert_external(a.clone()).unwrap(), Write::Unchanged);
        assert_eq!(s.doc.get_heads(), heads);
        assert_eq!(s.upsert_external(ExternalActivity { duration_s: 1900, ..a.clone() }).unwrap(), Write::Updated);
        let stored: HashMap<String, ExternalActivity> = autosurgeon::hydrate_prop(&s.doc, automerge::ROOT, "external_activities").unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored["garmin:123"].duration_s, 1900);
        assert!(matches!(s.upsert_external(ExternalActivity { user_id: "other".into(), ..a.clone() }), Err(Error::NotOwner)));
        assert!(s.upsert_external(ExternalActivity { source: "a:b".into(), ..a }).is_err());
    }

    #[test]
    fn concurrent_plan_edits_converge() {
        let (mut a, t) = setup("u");
        let mut b = AmFitnessStore::from_doc(a.doc.fork()).unwrap();
        let mut pa = a.get_plan("u").unwrap().unwrap();
        pa.slots.insert("wed_am".into(), PlanSlot { template_id: Some(t), external_label: None });
        a.set_plan(pa).unwrap();
        let mut pb = b.get_plan("u").unwrap().unwrap();
        pb.slots.insert("fri_pm".into(), PlanSlot { template_id: None, external_label: Some("Football".into()) });
        pb.slots.remove("mon_pm");
        b.set_plan(pb).unwrap();
        a.doc.merge(&mut b.doc).unwrap();
        b.doc.merge(&mut a.doc).unwrap();
        let (ga, gb) = (a.get_plan("u").unwrap().unwrap(), b.get_plan("u").unwrap().unwrap());
        assert_eq!(ga, gb);
        let mut keys: Vec<_> = ga.slots.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["fri_pm", "mon_am", "wed_am"], "both edits kept, b's removal applied");
    }
}
