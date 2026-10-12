//! Store-level progression: `plan_workout` over a real doc (program + sessions + gym profile).

use chrono::NaiveDate;
use fitness_core::*;

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn exercise(s: &mut AmFitnessStore, id: &str, muscle: &str, eq: Equipment) {
    let mut e = Exercise::new(id.into(), id, MeasurementType::WeightReps);
    e.primary_muscles = vec![muscle.into()];
    e.equipment = Some(eq);
    s.put_exercise(e).unwrap();
}

fn item(id: &str, index: u32, ex: &str) -> ProgramItem {
    ProgramItem { id: id.into(), index, exercise_id: ex.into(), sets: 3, rep_min: 7, rep_max: 9, rir: 2, style: SetStyle::Straight, rest_s: Some(120) }
}

/// bench (chest, barbell), incline (chest, dumbbell), squat (quads, barbell), curl (biceps, never logged).
fn setup() -> AmFitnessStore {
    let mut s = AmFitnessStore::new();
    exercise(&mut s, "bench", "chest", Equipment::Barbell);
    exercise(&mut s, "incline", "chest", Equipment::Dumbbell);
    exercise(&mut s, "squat", "quads", Equipment::Barbell);
    exercise(&mut s, "curl", "biceps", Equipment::Dumbbell);
    s.put_gym_profile(default_gym("alice")).unwrap();
    let w = ProgramWorkout {
        id: "w1".into(),
        index: 0,
        name: "Upper A".into(),
        slot: Some("mon_am".into()),
        external_label: None,
        items: ["bench", "incline", "squat", "curl"].iter().enumerate().map(|(i, e)| (format!("i{i}"), item(&format!("i{i}"), i as u32, e))).collect(),
    };
    s.put_program(Program {
        id: "p1".into(),
        user_id: "alice".into(),
        name: "PPL".into(),
        start_date: "2026-10-05".into(),
        weeks: 4,
        gym_profile_id: None,
        warmups: true,
        workouts: [("w1".to_string(), w)].into(),
        overrides: Default::default(),
        migrated_from: None,
        deleted: false,
    })
    .unwrap();
    s
}

/// Logs a finished session of 80 kg × 10 @ 2 RIR on bench, squat and incline (32 kg × 10).
fn log_finished(s: &mut AmFitnessStore, id: &str, started_at: i64, bench_kg: f64) {
    let mut sess = Session::start(id.into(), "alice");
    sess.started_at = started_at;
    sess.program_id = Some("p1".into());
    sess.workout_id = Some("w1".into());
    s.put_session(sess).unwrap();
    for (ex, load) in [("bench", bench_kg), ("squat", 100.0), ("incline", 32.0)] {
        let set = SetLog { exercise_id: ex.into(), load: Some(load), reps: Some(10), rir: Some(2), kind: Some(SetKind::Working), ..Default::default() };
        s.put_set("alice", id, &format!("{id}-{ex}"), set).unwrap();
    }
    s.finish_session("alice", id, SessionStatus::Completed, None).unwrap();
}

#[test]
fn plan_recommends_from_history_with_reasons_and_warmups() {
    let mut s = setup();
    let date = d("2026-10-12");
    let before = s.plan_workout("alice", "p1", "w1", date).unwrap();
    assert_eq!((before.week, before.name.as_str(), before.items.len()), (Some(2), "Upper A", 4));
    assert!(before.items.iter().all(|i| i.recommendation.is_none() && i.warmups.is_empty()), "no history yet");
    assert_eq!(before.items[0].prescription.rest_s, Some(120));

    log_finished(&mut s, "s1", 1_000, 80.0);
    let plan = s.plan_workout("alice", "p1", "w1", date).unwrap();
    let rec = |i: usize| plan.items[i].recommendation.clone();
    // 80 × 10 @ 2 RIR beats 7–9 @ 2 (F 12 vs 10): bench 82.5. Squat 100 × 10 → e1RM 133.3 → 105.
    assert_eq!(rec(0).unwrap().load_kg, 82.5);
    assert_eq!(rec(2).unwrap().load_kg, 105.0);
    assert!(rec(0).unwrap().reason.contains("above the 7–9 @ 2 RIR target"), "{}", rec(0).unwrap().reason);
    assert_eq!(rec(3), None, "an exercise never logged has none");

    // Warm-ups: first exercise per primary muscle only. Incline is also chest, so none.
    assert_eq!(plan.items[0].warmups.iter().map(|w| (w.load_kg, w.reps)).collect::<Vec<_>>(), [(32.5, 8), (47.5, 5), (65.0, 3)]);
    assert!(plan.items[1].warmups.is_empty());
    assert_eq!(plan.items[2].warmups.len(), 3);
    assert!(plan.items[3].warmups.is_empty(), "no recommendation, no warm-ups");

    // Per-program switch.
    let mut p = s.get_program("p1").unwrap();
    p.warmups = false;
    s.update_program(p).unwrap();
    assert!(s.plan_workout("alice", "p1", "w1", date).unwrap().items.iter().all(|i| i.warmups.is_empty()));
}

#[test]
fn recommendation_changes_when_a_session_is_finished_not_before() {
    let mut s = setup();
    let date = d("2026-10-12");
    log_finished(&mut s, "s1", 1_000, 80.0);
    let first = s.plan_workout("alice", "p1", "w1", date).unwrap().items[0].recommendation.clone().unwrap();

    // A second session, in progress with a much heavier set: the program doesn't move yet.
    let mut sess = Session::start("s2".into(), "alice");
    sess.started_at = 2_000;
    s.put_session(sess).unwrap();
    s.put_set("alice", "s2", "x", SetLog { exercise_id: "bench".into(), load: Some(100.0), reps: Some(10), rir: Some(2), ..Default::default() }).unwrap();
    assert_eq!(s.plan_workout("alice", "p1", "w1", date).unwrap().items[0].recommendation.as_ref(), Some(&first));

    s.finish_session("alice", "s2", SessionStatus::Completed, None).unwrap();
    let after = s.plan_workout("alice", "p1", "w1", date).unwrap().items[0].recommendation.clone().unwrap();
    assert!(after.load_kg > first.load_kg, "{first:?} -> {after:?}");
    assert!(after.reason.contains("averaged over 2 sessions"), "{}", after.reason);
    // Another user's history never leaks in.
    assert_eq!(s.plan_workout("alice", "p1", "w1", date).unwrap(), s.plan_workout("alice", "p1", "w1", date).unwrap(), "deterministic");
}

#[test]
fn ownership_missing_ids_and_outside_the_program() {
    let s = setup();
    assert!(matches!(s.plan_workout("bob", "p1", "w1", d("2026-10-12")), Err(Error::NotOwner)));
    assert!(matches!(s.plan_workout("alice", "p1", "nope", d("2026-10-12")), Err(Error::NotFound(_))));
    assert!(matches!(s.plan_workout("alice", "nope", "w1", d("2026-10-12")), Err(Error::NotFound(_))));
    assert_eq!(s.plan_workout("alice", "p1", "w1", d("2027-01-01")).unwrap().week, None, "after the mesocycle: base prescriptions");
}

#[test]
fn active_program_and_gym_fallback() {
    let mut s = setup();
    assert_eq!(s.active_program("alice", d("2026-10-12")).unwrap().map(|(p, w)| (p.id, w)), Some(("p1".into(), 2)));
    assert_eq!(s.active_program("alice", d("2026-09-01")).unwrap(), None);
    assert_eq!(s.active_program("bob", d("2026-10-12")).unwrap(), None);
    // A later program for the same user wins when both contain the date.
    let mut p2 = s.get_program("p1").unwrap();
    p2.id = "p2".into();
    p2.start_date = "2026-10-12".into();
    s.put_program(p2).unwrap();
    assert_eq!(s.active_program("alice", d("2026-10-13")).unwrap().map(|(p, w)| (p.id, w)), Some(("p2".into(), 1)));

    // The program's own gym profile beats the default; without any profile, the built-in one is used.
    let big_steps = GymProfile { id: "g2".into(), name: "Hotel".into(), is_default: false, bar_kg: 15.0, ..default_gym("alice") };
    s.put_gym_profile(big_steps).unwrap();
    let mut p = s.get_program("p1").unwrap();
    p.gym_profile_id = Some("g2".into());
    s.update_program(p.clone()).unwrap();
    assert_eq!(s.gym_for(&p).unwrap().bar_kg, 15.0);
    let bob_prog = Program { id: "pb".into(), user_id: "bob".into(), gym_profile_id: None, ..p };
    assert_eq!(s.gym_for(&bob_prog).unwrap().id, "default");
}
