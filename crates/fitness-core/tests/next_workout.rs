//! `next_workout`: pinned weekday first, then rotation through the unpinned workouts.

use chrono::{DateTime, NaiveDate};
use fitness_core::*;

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn utc_date(ms: i64) -> NaiveDate {
    DateTime::from_timestamp_millis(ms).unwrap().date_naive()
}

fn ms(date: &str) -> i64 {
    d(date).and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp_millis()
}

fn workout(id: &str, index: u32, slot: Option<&str>, external: Option<&str>, ex: &str) -> ProgramWorkout {
    let items = if external.is_some() {
        Default::default()
    } else {
        [(format!("{id}-i"), ProgramItem { id: format!("{id}-i"), index: 0, exercise_id: ex.into(), sets: 3, rep_min: 7, rep_max: 9, rir: 2, style: SetStyle::Straight, rest_s: None })].into()
    };
    ProgramWorkout { id: id.into(), index, name: id.to_uppercase(), slot: slot.map(Into::into), external_label: external.map(Into::into), items }
}

/// Program starting Monday 2026-10-05: A (Mon AM), B (Wed AM), PT (Wed PM, external), C and D unpinned.
fn setup(workouts: Vec<ProgramWorkout>) -> AmFitnessStore {
    let mut s = AmFitnessStore::new();
    let bench = Exercise { equipment: Some(Equipment::Barbell), ..Exercise::new("bench".into(), "Bench", MeasurementType::WeightReps) };
    s.put_exercise(bench).unwrap();
    let ex = "bench".to_string();
    let workouts = workouts.into_iter().map(|mut w| {
        for it in w.items.values_mut() {
            it.exercise_id = ex.clone();
        }
        (w.id.clone(), w)
    });
    s.put_program(Program {
        id: "p".into(),
        user_id: "alice".into(),
        name: "Prog".into(),
        start_date: "2026-10-05".into(),
        weeks: 4,
        gym_profile_id: None,
        warmups: true,
        workouts: workouts.collect(),
        overrides: Default::default(),
        migrated_from: None,
        deleted: false,
    })
    .unwrap();
    s
}

fn full() -> AmFitnessStore {
    setup(vec![
        workout("a", 0, Some("mon_am"), None, ""),
        workout("b", 1, Some("wed_am"), None, ""),
        workout("pt", 2, Some("wed_pm"), Some("Unit PT"), ""),
        workout("c", 3, None, None, ""),
        workout("d", 4, None, None, ""),
    ])
}

fn do_session(s: &mut AmFitnessStore, id: &str, workout: &str, date: &str, status: SessionStatus) {
    let mut sess = Session::start(id.into(), "alice");
    sess.started_at = ms(date);
    sess.program_id = Some("p".into());
    sess.workout_id = Some(workout.into());
    s.put_session(sess).unwrap();
    if status != SessionStatus::InProgress {
        s.finish_session("alice", id, status, None).unwrap();
    }
}

fn next(s: &AmFitnessStore, date: &str) -> NextWorkout {
    s.next_workout("alice", d(date), &utc_date).unwrap()
}

fn chosen(n: &NextWorkout) -> Option<&str> {
    n.workout.as_ref().map(|w| w.workout_id.as_str())
}

#[test]
fn pinned_workout_wins_and_done_detection() {
    let mut s = full();
    let n = next(&s, "2026-10-05"); // Monday
    assert_eq!((chosen(&n), n.reason.as_str(), n.week, n.weeks), (Some("a"), "Pinned for Monday AM.", Some(1), Some(4)));
    assert_eq!(n.workout.as_ref().unwrap().items.len(), 1);
    assert!(n.externals.is_empty() && n.todays_sessions.is_empty());

    // In progress: still the next workout; the session is reported so the UI can resume it.
    do_session(&mut s, "s1", "a", "2026-10-05", SessionStatus::InProgress);
    let n = next(&s, "2026-10-05");
    assert_eq!(chosen(&n), Some("a"));
    assert_eq!((n.todays_sessions[0].session_id.as_str(), n.todays_sessions[0].status.clone()), ("s1", SessionStatus::InProgress));

    // Finished: nothing else is planned today.
    s.finish_session("alice", "s1", SessionStatus::Partial, None).unwrap();
    let n = next(&s, "2026-10-05");
    assert_eq!((chosen(&n), n.reason.as_str()), (None, "Everything planned for today is done."));

    // Another day, or another user's session, doesn't count.
    assert_eq!(chosen(&next(&s, "2026-10-12")), Some("a"), "next Monday");
    let mut other = Session::start("o".into(), "bob");
    other.started_at = ms("2026-10-12");
    other.program_id = Some("p".into());
    other.workout_id = Some("a".into());
    s.put_session(other).unwrap_err(); // bob can't use alice's program
}

#[test]
fn externals_are_listed_with_their_state() {
    let mut s = full();
    let n = next(&s, "2026-10-07"); // Wednesday
    assert_eq!((chosen(&n), n.reason.as_str()), (Some("b"), "Pinned for Wednesday AM."));
    assert_eq!(n.externals, [ExternalToday { workout_id: "pt".into(), slot: "wed_pm".into(), label: "Unit PT".into(), done: false }]);
    do_session(&mut s, "pt1", "pt", "2026-10-07", SessionStatus::Completed);
    let n = next(&s, "2026-10-07");
    assert!(n.externals[0].done);
    assert_eq!(chosen(&n), Some("b"), "attending PT doesn't complete the lifting workout");
}

#[test]
fn rotation_over_unpinned_workouts() {
    let mut s = full();
    let n = next(&s, "2026-10-06"); // Tuesday: nothing pinned; C and D rotate
    assert_eq!((chosen(&n), n.reason.as_str()), (Some("c"), "First workout of the program: C."));
    do_session(&mut s, "r1", "c", "2026-10-06", SessionStatus::Completed);
    let n = next(&s, "2026-10-06");
    assert_eq!((chosen(&n), n.reason.as_str()), (Some("d"), "Next in rotation after C."));
    do_session(&mut s, "r2", "d", "2026-10-08", SessionStatus::Completed);
    assert_eq!(chosen(&next(&s, "2026-10-09")), Some("c"), "wraps around");
    // A pinned workout finishing doesn't move the rotation, and unfinished sessions don't count.
    do_session(&mut s, "r3", "a", "2026-10-12", SessionStatus::Completed);
    do_session(&mut s, "r4", "c", "2026-10-13", SessionStatus::InProgress);
    assert_eq!(chosen(&next(&s, "2026-10-14")), Some("b"), "Wednesday is pinned");
    assert_eq!(chosen(&next(&s, "2026-10-15")), Some("c"), "Thursday: still after D");
}

#[test]
fn rest_day_and_no_program() {
    let s = setup(vec![workout("a", 0, Some("mon_am"), None, ""), workout("b", 1, Some("wed_am"), None, "")]);
    let n = next(&s, "2026-10-06");
    assert_eq!((chosen(&n), n.reason.as_str()), (None, "Rest day: nothing is pinned for today."));
    assert_eq!(n.program_id.as_deref(), Some("p"));

    // Only an outside session pinned: still no lifting workout, and the reason says so.
    let s2 = setup(vec![workout("a", 0, Some("mon_am"), None, ""), workout("pt", 1, Some("wed_pm"), Some("Unit PT"), "")]);
    let n = next(&s2, "2026-10-07");
    assert_eq!((chosen(&n), n.reason.as_str(), n.externals.len()), (None, "Only an outside session is planned today.", 1));

    let n = next(&s, "2026-12-01"); // after the 4 weeks
    assert_eq!((n.program_id, n.workout, n.week), (None, None, None));
    assert_eq!(n.reason, "No program is running on this date.");
    let n = s.next_workout("bob", d("2026-10-06"), &utc_date).unwrap();
    assert_eq!(n.program_id, None);
}

#[test]
fn recommendations_come_with_the_workout() {
    let mut s = full();
    let ex = s.list_exercises().unwrap()[0].id.clone();
    do_session(&mut s, "h1", "a", "2026-10-05", SessionStatus::InProgress);
    s.put_set("alice", "h1", "x", SetLog { exercise_id: ex, load: Some(80.0), reps: Some(10), rir: Some(2), ..Default::default() }).unwrap();
    s.finish_session("alice", "h1", SessionStatus::Completed, None).unwrap();
    let n = next(&s, "2026-10-12");
    let rec = n.workout.unwrap().items[0].recommendation.clone().unwrap();
    assert_eq!(rec.load_kg, 82.5);
    assert!(rec.reason.contains("above"), "{}", rec.reason);
}
