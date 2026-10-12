//! Smart progression (v2): a pure function of logged history. Nothing here is
//! stored; recommendations are derived at read time (docs/DECISIONS.md
//! "Progression engine (v2-M2)"). Modelled on MacroFactor Workouts:
//! https://help.macrofactorapp.com/en/articles/372-what-does-progressive-overload-mean-in-macrofactor-workouts
//!
//! Terms: `F` = reps to failure = reps + RIR. e1RM = estimated one-rep max.

use super::*;

/// Epley: `e1RM = load × (1 + F / 30)`. https://en.wikipedia.org/wiki/One-repetition_maximum
pub const EPLEY_DIVISOR: f64 = 30.0;
/// The running strength estimate is the mean of this many most recent sessions,
/// so one bad day moves it by only a third (non-punitive).
pub const SESSION_WINDOW: usize = 3;
/// If rounding the ideal load down to the equipment loses more than this share
/// of it, keep the lower load and raise reps instead.
pub const FALLBACK_GAP: f64 = 0.10;
/// ... by at most this many reps above the top of the rep range.
pub const FALLBACK_EXTRA_REPS: u32 = 3;
/// Warm-up sets: (share of the working load, reps).
pub const WARMUP_STEPS: [(f64, u32); 3] = [(0.4, 8), (0.6, 5), (0.8, 3)];
/// RIR assumed for a logged set with none, when no prescription says otherwise (PRs, charts).
pub const ASSUMED_RIR: u8 = 2;

pub fn epley(load: f64, f: f64) -> f64 {
    load * (1.0 + f / EPLEY_DIVISOR)
}

/// Inverse of [`epley`]: the load that takes `e1rm` to `f` reps to failure.
pub fn load_for(e1rm: f64, f: f64) -> f64 {
    e1rm / (1.0 + f / EPLEY_DIVISOR)
}

/// kg for display: 82.5, 80 (no trailing zeros).
fn kg(x: f64) -> String {
    format!("{}", (x * 100.0).round() / 100.0)
}

/// What a program item asks for in a given week (overrides applied).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Prescription {
    pub sets: u32,
    pub rep_min: u32,
    pub rep_max: u32,
    pub rir: u8,
    pub style: SetStyle,
    pub rest_s: Option<u32>,
}

impl Prescription {
    pub fn midpoint(&self) -> f64 {
        f64::from(self.rep_min + self.rep_max) / 2.0
    }

    /// MacroFactor's rule: expected max performance = midpoint of the rep range + RIR target.
    /// 7–9 reps @ 2 RIR → 10 reps to failure.
    pub fn expected_f(&self) -> f64 {
        self.midpoint() + f64::from(self.rir)
    }
}

/// An item's prescription in `week` (`None`: outside the program, base values).
pub fn prescription(program: &Program, item: &ProgramItem, week: Option<u32>) -> Prescription {
    let o = week.and_then(|w| program.overrides.get(&format!("{w}:{}", item.id)));
    let rep_min = o.and_then(|o| o.rep_min).unwrap_or(item.rep_min);
    Prescription {
        sets: o.and_then(|o| o.sets).unwrap_or(item.sets),
        rep_min,
        rep_max: o.and_then(|o| o.rep_max).unwrap_or(item.rep_max).max(rep_min),
        rir: o.and_then(|o| o.rir).unwrap_or(item.rir),
        style: item.style.clone(),
        rest_s: item.rest_s,
    }
}

/// 1-based week of `date` in the program, or `None` outside its mesocycle.
pub fn program_week(program: &Program, date: NaiveDate) -> Option<u32> {
    plan::week_index(&program.start_date, program.weeks, date)
}

/// `Warmup` and `Drop` sets never feed the strength estimate; `None` is a v1 set (a working set).
pub(crate) fn counts_for_strength(kind: &Option<SetKind>) -> bool {
    matches!(kind, None | Some(SetKind::Working) | Some(SetKind::Failure))
}

/// A finished session's best working set for one exercise.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionBest {
    pub started_at: i64,
    pub session_id: String,
    pub e1rm: f64,
    pub load: f64,
    pub reps: u32,
    pub rir: u8,
    /// Reps to failure of that set.
    pub f: f64,
}

/// Every strength-counting set of a session as `(load, reps, rir-used, e1rm)`.
pub(crate) fn strength_sets(s: &Session, exercise_id: &str, assumed_rir: u8) -> Vec<(f64, u32, u8, f64)> {
    s.ordered_sets()
        .into_iter()
        .filter(|x| x.exercise_id == exercise_id && counts_for_strength(&x.kind))
        .filter_map(|x| {
            let (load, reps) = (x.load?, x.reps?);
            if !(load.is_finite() && load > 0.0 && reps > 0) {
                return None;
            }
            let rir = x.rir.unwrap_or(assumed_rir);
            Some((load, reps, rir, epley(load, f64::from(reps) + f64::from(rir))))
        })
        .collect()
}

/// The best of `sets` by e1RM; ties go to the heavier load, then more reps, so the
/// choice never depends on the order sets were logged.
pub(crate) fn best_of(sets: Vec<(f64, u32, u8, f64)>) -> Option<(f64, u32, u8, f64)> {
    sets.into_iter().max_by(|a, b| a.3.total_cmp(&b.3).then(a.0.total_cmp(&b.0)).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)))
}

/// Finished (Completed or Partial) sessions of `user_id` with a counting set for the
/// exercise, oldest first (ties by session ID).
pub(crate) fn session_bests(sessions: &[Session], user_id: &str, exercise_id: &str, assumed_rir: u8) -> Vec<SessionBest> {
    let mut v: Vec<SessionBest> = sessions
        .iter()
        .filter(|s| s.user_id == user_id && !s.deleted && matches!(s.status, SessionStatus::Completed | SessionStatus::Partial))
        .filter_map(|s| {
            let (load, reps, rir, e1rm) = best_of(strength_sets(s, exercise_id, assumed_rir))?;
            Some(SessionBest { started_at: s.started_at, session_id: s.id.clone(), e1rm, load, reps, rir, f: f64::from(reps) + f64::from(rir) })
        })
        .collect();
    v.sort_by(|a, b| (a.started_at, &a.session_id).cmp(&(b.started_at, &b.session_id)));
    v
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Recommendation {
    /// Describes what was shown (`<exercise>@<load>x<reps>@<rir>`); stored on a set as
    /// `recommendation_id`. Not a stored record: the recommendation is recomputed on read.
    pub id: String,
    pub load_kg: f64,
    pub reps: u32,
    pub rir: u8,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WarmupSet {
    pub load_kg: f64,
    pub reps: u32,
}

/// Largest load ≤ `load` the gym can make, never below the lightest it can make
/// (the bar, or one dumbbell/machine step).
pub fn round_down_load(load: f64, equipment: Option<&Equipment>, gym: &GymProfile) -> f64 {
    match equipment {
        Some(Equipment::Barbell) => barbell_floor(load, gym),
        Some(Equipment::Dumbbell) => step_floor(load, gym.dumbbell_step_kg),
        // Machine, cable, bodyweight (added load), other, and unset: the machine increment.
        _ => step_floor(load, gym.machine_step_kg),
    }
}

fn step_floor(load: f64, step: f64) -> f64 {
    let n = (load / step + 1e-9).floor().max(1.0);
    (n * step * 1000.0).round() / 1000.0
}

/// bar + 2 × the largest sum of plates (any number of each, one stack per side) ≤ (load − bar) / 2.
/// Exact (a knapsack over 0.01 kg), not greedy: with plates {3, 5}, 7 per side is 6, not 5.
fn barbell_floor(load: f64, gym: &GymProfile) -> f64 {
    let per_side = (load - gym.bar_kg) / 2.0;
    if per_side <= 0.0 || gym.plates_kg.is_empty() {
        return gym.bar_kg;
    }
    let target = ((per_side * 100.0 + 1e-6).floor() as usize).min(100_000);
    let plates: Vec<usize> = gym.plates_kg.iter().map(|p| (p * 100.0).round() as usize).filter(|p| *p > 0).collect();
    let mut reachable = vec![false; target + 1];
    reachable[0] = true;
    for u in 1..=target {
        reachable[u] = plates.iter().any(|&p| u >= p && reachable[u - p]);
    }
    let best = (0..=target).rev().find(|&u| reachable[u]).unwrap_or(0);
    ((gym.bar_kg + 2.0 * best as f64 / 100.0) * 1000.0).round() / 1000.0
}

/// 40 / 60 / 80 % of the working load at 8 / 5 / 3 reps, made with the gym's equipment.
/// Steps that round to the same load as the previous one, or to the working load, are dropped.
pub fn warmups(working_kg: f64, equipment: Option<&Equipment>, gym: &GymProfile) -> Vec<WarmupSet> {
    let mut out: Vec<WarmupSet> = vec![];
    for (share, reps) in WARMUP_STEPS {
        let load_kg = round_down_load(working_kg * share, equipment, gym);
        if load_kg < working_kg && out.last().map(|w| w.load_kg) != Some(load_kg) {
            out.push(WarmupSet { load_kg, reps });
        }
    }
    out
}

/// The recommendation for one exercise, or `None` without finished history for it
/// (the UI then asks for a load and pre-fills previous values).
pub fn recommend(exercise: &Exercise, p: &Prescription, sessions: &[Session], user_id: &str, gym: &GymProfile) -> Option<Recommendation> {
    let history = session_bests(sessions, user_id, &exercise.id, p.rir);
    let recent = &history[history.len().saturating_sub(SESSION_WINDOW)..];
    let last = recent.last()?;
    let running = recent.iter().map(|b| b.e1rm).sum::<f64>() / recent.len() as f64;

    let expected_f = p.expected_f();
    let ideal = load_for(running, expected_f);
    let load_kg = round_down_load(ideal, exercise.equipment.as_ref(), gym);
    let mid_reps = p.midpoint().round() as u32;
    let fallback = (ideal - load_kg).abs() > FALLBACK_GAP * ideal;
    let reps = if fallback {
        // Same reps-to-failure target at the load we can actually make.
        let needed = EPLEY_DIVISOR * (running / load_kg - 1.0) - f64::from(p.rir);
        (needed.round().max(0.0) as u32).clamp(p.rep_min, p.rep_max + FALLBACK_EXTRA_REPS)
    } else {
        mid_reps
    };

    let when = if recent.len() == 1 {
        "Last session".to_string()
    } else {
        format!("Last session (strength estimate averaged over {} sessions)", recent.len())
    };
    let diff = last.f - expected_f;
    let vs = if diff >= 1.0 {
        format!("above the {}–{} @ {} RIR target by {} rep(s) to failure", p.rep_min, p.rep_max, p.rir, diff.round())
    } else if diff <= -1.0 {
        format!("below the {}–{} @ {} RIR target by {} rep(s) to failure", p.rep_min, p.rep_max, p.rir, (-diff).round())
    } else {
        format!("on the {}–{} @ {} RIR target", p.rep_min, p.rep_max, p.rir)
    };
    let delta = load_kg - last.load;
    let change = if delta.abs() < 0.005 {
        "same load".to_string()
    } else {
        format!("{}{} kg", if delta > 0.0 { "+" } else { "" }, kg(delta))
    };
    let note = if fallback {
        format!("; the nearest load this equipment makes below {} kg is {} kg, so reps are raised to {reps}", kg(ideal), kg(load_kg))
    } else {
        String::new()
    };
    let reason = format!("{when}: {} reps @ {} kg at {} RIR, {vs} → {change}{note}", last.reps, kg(last.load), last.rir);

    Some(Recommendation { id: format!("{}@{}x{}@{}", exercise.id, kg(load_kg), reps, p.rir), load_kg, reps, rir: p.rir, reason })
}

/// One program item resolved for a date.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ItemPlan {
    pub item_id: String,
    pub exercise_id: String,
    pub exercise_name: Option<String>,
    pub prescription: Prescription,
    pub recommendation: Option<Recommendation>,
    pub warmups: Vec<WarmupSet>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WorkoutPlan {
    pub program_id: String,
    pub workout_id: String,
    pub name: String,
    pub slot: Option<String>,
    /// Week of the program (`None`: `date` is outside its mesocycle; base prescriptions).
    pub week: Option<u32>,
    pub external_label: Option<String>,
    pub items: Vec<ItemPlan>,
}

/// Fallback when a user has no gym profile: a standard 20 kg bar and metric plates.
pub fn default_gym(user_id: &str) -> GymProfile {
    GymProfile {
        id: "default".into(),
        user_id: user_id.into(),
        name: "Default".into(),
        bar_kg: 20.0,
        plates_kg: vec![1.25, 2.5, 5.0, 10.0, 15.0, 20.0, 25.0],
        dumbbell_step_kg: 2.0,
        machine_step_kg: 5.0,
        is_default: true,
        deleted: false,
    }
}

/// Build one workout's plan: prescriptions for `date`'s week, recommendations from
/// `sessions`, and warm-ups for the first exercise per primary muscle.
pub(crate) fn plan_workout(
    program: &Program,
    workout: &ProgramWorkout,
    date: NaiveDate,
    exercises: &HashMap<String, Exercise>,
    sessions: &[Session],
    gym: &GymProfile,
) -> WorkoutPlan {
    let week = program_week(program, date);
    let mut seen: std::collections::HashSet<&str> = Default::default();
    let items = workout
        .ordered_items()
        .into_iter()
        .map(|it| {
            let ex = exercises.get(&it.exercise_id);
            let p = prescription(program, it, week);
            let recommendation = ex.and_then(|e| recommend(e, &p, sessions, &program.user_id, gym));
            let first_for_muscle = ex.is_some_and(|e| {
                let new = e.primary_muscles.iter().any(|m| !seen.contains(m.as_str()));
                seen.extend(e.primary_muscles.iter().map(String::as_str));
                new
            });
            let warmups = match (&recommendation, ex) {
                (Some(r), Some(e)) if program.warmups && first_for_muscle => warmups(r.load_kg, e.equipment.as_ref(), gym),
                _ => vec![],
            };
            ItemPlan {
                item_id: it.id.clone(),
                exercise_id: it.exercise_id.clone(),
                exercise_name: ex.map(|e| e.name.clone()),
                prescription: p,
                recommendation,
                warmups,
            }
        })
        .collect();
    WorkoutPlan {
        program_id: program.id.clone(),
        workout_id: workout.id.clone(),
        name: workout.name.clone(),
        slot: workout.slot.clone(),
        week,
        external_label: workout.external_label.clone(),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gym() -> GymProfile {
        default_gym("alice")
    }

    fn ex(equipment: Equipment) -> Exercise {
        Exercise { equipment: Some(equipment), ..Exercise::new("ex1".into(), "Lift", MeasurementType::WeightReps) }
    }

    fn rx(rep_min: u32, rep_max: u32, rir: u8) -> Prescription {
        Prescription { sets: 3, rep_min, rep_max, rir, style: SetStyle::Straight, rest_s: None }
    }

    /// A finished session `n` (older = smaller) with one working set.
    fn session(n: i64, load: f64, reps: u32, rir: Option<u8>) -> Session {
        let mut s = Session::start(format!("s{n:03}"), "alice");
        s.started_at = n * 1000;
        s.status = SessionStatus::Completed;
        s.finished_at = Some(n * 1000 + 1);
        s.sets.insert("a".into(), SetLog { exercise_id: "ex1".into(), load: Some(load), reps: Some(reps), rir, ..Default::default() });
        s
    }

    fn rec(e: &Exercise, p: &Prescription, sessions: &[Session]) -> Option<Recommendation> {
        recommend(e, p, sessions, "alice", &gym())
    }

    #[test]
    fn macrofactor_worked_example() {
        // 7–9 reps @ 2 RIR → 10 reps to failure.
        assert_eq!(rx(7, 9, 2).expected_f(), 10.0);
        assert_eq!(rx(5, 6, 0).expected_f(), 5.5);
        // Epley: 100 kg for 5 reps to failure ≈ 116.7.
        assert!((epley(100.0, 5.0) - 116.6667).abs() < 1e-3);
        assert!((load_for(epley(100.0, 5.0), 5.0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn beating_the_target_raises_the_load() {
        // 80 kg × 10 @ 2 RIR: F = 12 vs expected 10. e1RM 112 → ideal 84 → bar+plates 82.5.
        let r = rec(&ex(Equipment::Barbell), &rx(7, 9, 2), &[session(1, 80.0, 10, Some(2))]).unwrap();
        assert_eq!((r.load_kg, r.reps, r.rir), (82.5, 8, 2));
        assert!(r.reason.contains("above the 7–9 @ 2 RIR target by 2 rep(s)"), "{}", r.reason);
        assert!(r.reason.ends_with("→ +2.5 kg"), "{}", r.reason);
        assert!(r.reason.starts_with("Last session: 10 reps @ 80 kg at 2 RIR"), "{}", r.reason);
    }

    #[test]
    fn a_miss_lowers_the_load_a_little() {
        // 80 kg × 6 @ 2 RIR: F = 8 vs 10. e1RM 101.3 → ideal 76 → 75.
        let r = rec(&ex(Equipment::Barbell), &rx(7, 9, 2), &[session(1, 80.0, 6, Some(2))]).unwrap();
        assert_eq!((r.load_kg, r.reps), (75.0, 8));
        assert!(r.reason.contains("below the 7–9 @ 2 RIR target by 2 rep(s)"), "{}", r.reason);
        assert!(r.reason.ends_with("→ -5 kg"), "{}", r.reason);
    }

    #[test]
    fn on_target_keeps_the_load() {
        // F = 10 exactly: ideal = the load used.
        let r = rec(&ex(Equipment::Barbell), &rx(7, 9, 2), &[session(1, 80.0, 8, Some(2))]).unwrap();
        assert_eq!(r.load_kg, 80.0);
        assert!(r.reason.contains("on the 7–9 @ 2 RIR target"), "{}", r.reason);
        assert!(r.reason.ends_with("→ same load"), "{}", r.reason);
    }

    #[test]
    fn three_session_mean_dampens_one_bad_day() {
        let e = ex(Equipment::Barbell);
        let good = |n| session(n, 80.0, 10, Some(2));
        let after_bad_day = [good(1), good(2), session(3, 80.0, 6, Some(2))];
        let r = rec(&e, &rx(7, 9, 2), &after_bad_day).unwrap();
        // Mean e1RM (112 + 112 + 101.3) / 3 = 108.4 → ideal 81.3 → 80, not the 75 a single bad day gives.
        assert_eq!(r.load_kg, 80.0);
        assert!(r.reason.contains("averaged over 3 sessions"), "{}", r.reason);
        assert_eq!(rec(&e, &rx(7, 9, 2), &after_bad_day[2..]).unwrap().load_kg, 75.0);
        // Only the last 3 count: an old great session falls out of the window.
        let old = session(0, 120.0, 10, Some(2));
        let with_old = [old, good(1), good(2), good(3)];
        assert_eq!(rec(&e, &rx(7, 9, 2), &with_old).unwrap(), rec(&e, &rx(7, 9, 2), &with_old[1..]).unwrap());
    }

    #[test]
    fn rounds_down_to_plates_exactly() {
        let g = gym();
        let bb = Some(&Equipment::Barbell);
        assert_eq!(round_down_load(82.6, bb, &g), 82.5);
        assert_eq!(round_down_load(84.0, bb, &g), 82.5, "31.25 per side beats 32.5 (too heavy)");
        assert_eq!(round_down_load(21.0, bb, &g), 20.0);
        assert_eq!(round_down_load(19.0, bb, &g), 20.0, "never below the bar");
        assert_eq!(round_down_load(100.0, bb, &g), 100.0);
        // Not greedy: plates {3, 5}, 7 kg a side → 3 + 3 = 6, greedy would stop at 5.
        let odd = GymProfile { bar_kg: 20.0, plates_kg: vec![3.0, 5.0], ..g.clone() };
        assert_eq!(round_down_load(34.0, bb, &odd), 32.0);
        assert_eq!(round_down_load(60.0, Some(&Equipment::Dumbbell), &g), 60.0);
        assert_eq!(round_down_load(61.9, Some(&Equipment::Dumbbell), &g), 60.0);
        assert_eq!(round_down_load(0.5, Some(&Equipment::Dumbbell), &g), 2.0, "at least one step");
        assert_eq!(round_down_load(47.4, Some(&Equipment::Machine), &g), 45.0);
        assert_eq!(round_down_load(47.4, None, &g), 45.0);
    }

    #[test]
    fn light_dumbbell_fallback_raises_reps() {
        // 12–15 @ 1 RIR → expected F 14.5. 8 kg × 14 @ 1 RIR → e1RM 12 → ideal 8.09 kg.
        let s = [session(1, 8.0, 14, Some(1))];
        let big_steps = GymProfile { dumbbell_step_kg: 5.0, ..gym() };
        let e = ex(Equipment::Dumbbell);
        let r = recommend(&e, &rx(12, 15, 1), &s, "alice", &big_steps).unwrap();
        // 5 kg is 38% below the ideal → keep 5 kg, raise reps, capped at 15 + 3.
        assert_eq!((r.load_kg, r.reps), (5.0, 18));
        assert!(r.reason.contains("so reps are raised to 18"), "{}", r.reason);
        // With 2 kg steps 8 kg is within 10% → midpoint reps (13.5 rounds to 14), no fallback.
        let r = recommend(&e, &rx(12, 15, 1), &s, "alice", &gym()).unwrap();
        assert_eq!((r.load_kg, r.reps), (8.0, 14));
        assert!(!r.reason.contains("raised"));
        // 24 kg × 8 @ 2 RIR → e1RM 32, ideal 24 kg; 10 kg steps give 20 kg (17% below).
        // Matching F needs 30×(32/20−1) − 2 = 16 reps, capped at 9 + 3 = 12.
        let heavy = [session(1, 24.0, 8, Some(2))];
        let g = GymProfile { dumbbell_step_kg: 10.0, ..gym() };
        let r = recommend(&e, &rx(7, 9, 2), &heavy, "alice", &g).unwrap();
        assert_eq!((r.load_kg, r.reps), (20.0, 12));
        // A smaller gap needs fewer reps than the cap: 22 kg steps of 2 → no fallback; step 4 → 24 → no fallback.
        let g = GymProfile { dumbbell_step_kg: 4.0, ..gym() };
        assert_eq!(recommend(&e, &rx(7, 9, 2), &heavy, "alice", &g).unwrap().load_kg, 24.0);
    }

    #[test]
    fn no_history_no_recommendation() {
        let e = ex(Equipment::Barbell);
        assert_eq!(rec(&e, &rx(7, 9, 2), &[]), None);
        // Not finished, someone else's, deleted, warm-up, drop set, no load: none of these count.
        let mut in_progress = session(1, 80.0, 8, Some(2));
        in_progress.status = SessionStatus::InProgress;
        let mut other_user = session(2, 80.0, 8, Some(2));
        other_user.user_id = "bob".into();
        let mut deleted = session(3, 80.0, 8, Some(2));
        deleted.deleted = true;
        let mut kinds = session(4, 80.0, 8, Some(2));
        kinds.sets.get_mut("a").unwrap().kind = Some(SetKind::Warmup);
        kinds.sets.insert("b".into(), SetLog { exercise_id: "ex1".into(), load: Some(60.0), reps: Some(12), kind: Some(SetKind::Drop), ..Default::default() });
        kinds.sets.insert("c".into(), SetLog { exercise_id: "ex1".into(), reps: Some(12), ..Default::default() });
        kinds.sets.insert("d".into(), SetLog { exercise_id: "other".into(), load: Some(80.0), reps: Some(8), ..Default::default() });
        assert_eq!(rec(&e, &rx(7, 9, 2), &[in_progress, other_user, deleted, kinds]), None);
    }

    #[test]
    fn missing_rir_assumes_the_target() {
        let e = ex(Equipment::Barbell);
        let with = rec(&e, &rx(7, 9, 2), &[session(1, 80.0, 8, Some(2))]).unwrap();
        let without = rec(&e, &rx(7, 9, 2), &[session(1, 80.0, 8, None)]).unwrap();
        assert_eq!(with.load_kg, without.load_kg);
        // v1 sets (kind None) count as working sets.
        assert_eq!(without.load_kg, 80.0);
    }

    #[test]
    fn deload_week_from_periodization() {
        let item = ProgramItem { id: "i1".into(), index: 0, exercise_id: "ex1".into(), sets: 4, rep_min: 7, rep_max: 9, rir: 2, style: SetStyle::Straight, rest_s: Some(120) };
        let workout = ProgramWorkout { id: "w".into(), index: 0, name: "A".into(), slot: None, external_label: None, items: [("i1".to_string(), item.clone())].into() };
        let mut program = Program {
            id: "p".into(),
            user_id: "alice".into(),
            name: "P".into(),
            start_date: "2026-01-05".into(),
            weeks: 4,
            gym_profile_id: None,
            warmups: true,
            workouts: [("w".to_string(), workout)].into(),
            overrides: Default::default(),
            migrated_from: None,
            deleted: false,
        };
        program.overrides.insert("4:i1".into(), WeekOverride { week: 4, item_id: "i1".into(), sets: Some(2), rep_min: None, rep_max: None, rir: Some(4) });
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();

        assert_eq!(program_week(&program, d("2026-01-04")), None);
        assert_eq!(program_week(&program, d("2026-01-05")), Some(1));
        assert_eq!(program_week(&program, d("2026-01-31")), Some(4));
        assert_eq!(program_week(&program, d("2026-02-02")), None);
        let (w1, w4) = (prescription(&program, &item, Some(1)), prescription(&program, &item, Some(4)));
        assert_eq!((w1.sets, w1.rir, w1.rest_s), (4, 2, Some(120)));
        assert_eq!((w4.sets, w4.rir, (w4.rep_min, w4.rep_max), w4.rest_s), (2, 4, (7, 9), Some(120)));
        assert_eq!(prescription(&program, &item, None), w1, "outside the program: base values");

        // Higher RIR target → lighter recommended load from the same history.
        let e = ex(Equipment::Barbell);
        let hist = [session(1, 80.0, 10, Some(2))];
        let (normal, deload) = (rec(&e, &w1, &hist).unwrap(), rec(&e, &w4, &hist).unwrap());
        assert_eq!((normal.load_kg, deload.load_kg), (82.5, 80.0));
        assert_eq!(deload.rir, 4);

        // A rep-range override that crosses the other bound is clamped, not invalid.
        program.overrides.insert("2:i1".into(), WeekOverride { week: 2, item_id: "i1".into(), sets: None, rep_min: Some(12), rep_max: None, rir: None });
        let w2 = prescription(&program, &item, Some(2));
        assert_eq!((w2.rep_min, w2.rep_max), (12, 12));
    }

    #[test]
    fn result_does_not_depend_on_log_order() {
        let e = ex(Equipment::Barbell);
        let sessions = [session(1, 80.0, 10, Some(2)), session(2, 82.5, 8, Some(1)), session(3, 80.0, 9, Some(2)), session(4, 85.0, 6, Some(3))];
        let want = rec(&e, &rx(7, 9, 2), &sessions).unwrap();
        let mut rev = sessions.clone();
        rev.reverse();
        assert_eq!(rec(&e, &rx(7, 9, 2), &rev).unwrap(), want);
        rev.swap(0, 2);
        assert_eq!(rec(&e, &rx(7, 9, 2), &rev).unwrap(), want);
        // Sets of one session logged in the opposite order (their `index` swapped).
        let mut s = session(5, 80.0, 8, Some(2));
        s.sets.insert("b".into(), SetLog { index: 1, exercise_id: "ex1".into(), load: Some(82.5), reps: Some(6), rir: Some(2), ..Default::default() });
        let mut s2 = s.clone();
        s2.sets.get_mut("a").unwrap().index = 1;
        s2.sets.get_mut("b").unwrap().index = 0;
        assert_eq!(rec(&e, &rx(7, 9, 2), &[s.clone()]), rec(&e, &rx(7, 9, 2), &[s2]));
    }

    #[test]
    fn warmups_are_40_60_80_percent() {
        let g = gym();
        let w = warmups(100.0, Some(&Equipment::Barbell), &g);
        assert_eq!(w, [WarmupSet { load_kg: 40.0, reps: 8 }, WarmupSet { load_kg: 60.0, reps: 5 }, WarmupSet { load_kg: 80.0, reps: 3 }]);
        // Light barbell work: rounding to the bar collapses steps; never at or above the working load.
        let w = warmups(40.0, Some(&Equipment::Barbell), &g);
        assert_eq!(w.iter().map(|x| x.load_kg).collect::<Vec<_>>(), [20.0, 22.5, 30.0]);
        assert!(warmups(20.0, Some(&Equipment::Barbell), &g).is_empty());
    }
}
