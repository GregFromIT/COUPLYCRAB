//! A doc written by v1 (tag `v1`) must load in v2 unchanged, and migrate.
//! Fixture made by the v1 `fitness-core` code (see docs/DECISIONS.md "Training model (v2-M1)").

use automerge::ReadDoc;
use fitness_core::*;

const V1: &[u8] = include_bytes!("fixtures/fitness-v1.automerge");

fn load() -> AmFitnessStore {
    AmFitnessStore::from_doc(Automerge::load(V1).unwrap()).unwrap()
}

#[test]
fn v1_doc_loads_in_v2() {
    let v1 = Automerge::load(V1).unwrap();
    let v1_heads = v1.get_heads();
    let s = AmFitnessStore::from_doc(v1).unwrap();

    // v1 data reads back intact.
    let squat = s.list_exercises().unwrap().into_iter().find(|e| e.name == "Squat").unwrap();
    assert_eq!((squat.tempo.as_deref(), squat.primary_muscles.len(), squat.equipment.clone()), (Some("2:1"), 0, None));
    let t = &s.list_templates().unwrap()[0];
    assert_eq!(t.ordered_items()[0].progression.as_ref().map(|p| p.per_week), Some(2.5));
    assert_eq!(s.get_plan("alice").unwrap().unwrap().slots.len(), 2);
    assert_eq!(s.list_blocks().unwrap()[0].deload_week, Some(6));
    let sess = &s.list_sessions().unwrap()[0];
    assert_eq!((sess.block_id.as_deref(), sess.program_id.clone(), sess.workout_id.clone()), (Some("block-1"), None, None));
    let set = sess.sets.values().next().unwrap();
    assert_eq!(
        (set.load, set.kind.clone(), set.rir, set.partials, set.rest_taken_s, set.recommendation_id.clone()),
        (Some(100.0), None, None, None, None, None)
    );

    // Upgrading only added the schema-3 init change.
    assert_eq!(s.schema_version().unwrap(), Some(3));
    assert!(v1_heads.iter().all(|h| s.doc.get_change_by_hash(h).is_some()));
    assert_eq!(s.doc.get_changes(&v1_heads).len(), 1);
    assert!(s.list_programs().unwrap().is_empty());
    assert!(s.list_gym_profiles("alice").unwrap().is_empty());
}

#[test]
fn v1_doc_and_fresh_v2_device_converge() {
    let mut upgraded = load();
    let mut fresh = AmFitnessStore::new();
    fresh.put_gym_profile(gym("g1", "alice", "Home", true)).unwrap();
    upgraded.doc.merge(&mut fresh.doc).unwrap();
    assert_eq!(upgraded.list_gym_profiles("alice").unwrap()[0].name, "Home");
    assert_eq!(upgraded.list_exercises().unwrap().len(), 2);
}

#[test]
fn migrate_turns_block_plan_and_template_into_a_program() {
    let mut s = load();
    let m = s.migrate_v1_blocks().unwrap();
    assert_eq!(m.created, 1);
    assert!(m.notes.iter().any(|n| n.contains("linear progression dropped")), "{:?}", m.notes);
    assert!(m.notes.iter().any(|n| n.contains("deload load percentage dropped")));

    let p = s.get_program("v1-block-1").unwrap();
    assert_eq!((p.name.as_str(), p.user_id.as_str(), p.start_date.as_str(), p.weeks), ("Base", "alice", "2026-01-05", 6));
    assert_eq!(p.migrated_from.as_deref(), Some("block-1"));
    let w: Vec<_> = p.ordered_workouts().into_iter().map(|w| (w.name.as_str(), w.slot.as_deref(), w.external_label.as_deref())).collect();
    assert_eq!(w, [("Legs", Some("mon_am"), None), ("Unit PT", Some("wed_pm"), Some("Unit PT"))]);

    let items = p.workouts["mon_am"].ordered_items();
    let squat = s.list_exercises().unwrap().into_iter().find(|e| e.name == "Squat").unwrap();
    // fixed reps r -> (r, r); RIR 2 by default; rest carried over.
    assert_eq!(
        (items[0].exercise_id.as_str(), items[0].sets, items[0].rep_min, items[0].rep_max, items[0].rir, items[0].rest_s),
        (squat.id.as_str(), 3, 5, 5, 2, Some(120))
    );
    assert_eq!((items[1].rep_min, items[1].rep_max), (8, 8));
    assert!(p.workouts["wed_pm"].items.is_empty());

    // v1 deload week 6 -> half the sets (rounded up) at RIR 4, for every item.
    assert_eq!(p.overrides.len(), 2);
    let o = &p.overrides[&format!("6:{}", items[0].id)];
    assert_eq!((o.week, o.sets, o.rir, o.rep_min), (6, Some(2), Some(4), None));

    // v1 data stays, untouched.
    assert_eq!(s.list_templates().unwrap().len(), 1);
    assert_eq!(s.list_blocks().unwrap().len(), 1);
}

#[test]
fn migrate_is_idempotent_and_never_overwrites() {
    let mut s = load();
    s.migrate_v1_blocks().unwrap();
    let heads = s.doc.get_heads();
    assert_eq!(s.migrate_v1_blocks().unwrap().created, 0);
    assert_eq!(s.doc.get_heads(), heads, "second run writes nothing");

    let mut p = s.get_program("v1-block-1").unwrap();
    p.name = "Edited".into();
    s.update_program(p).unwrap();
    s.migrate_v1_blocks().unwrap();
    assert_eq!(s.get_program("v1-block-1").unwrap().name, "Edited");
}

#[test]
fn two_devices_migrating_converge() {
    let (mut a, mut b) = (load(), load());
    a.migrate_v1_blocks().unwrap();
    b.migrate_v1_blocks().unwrap();
    a.doc.merge(&mut b.doc).unwrap();
    b.doc.merge(&mut a.doc).unwrap();
    assert_eq!(a.list_programs().unwrap(), b.list_programs().unwrap());
    assert_eq!(a.list_programs().unwrap().len(), 1);
}

fn gym(id: &str, user: &str, name: &str, default: bool) -> GymProfile {
    GymProfile {
        id: id.into(),
        user_id: user.into(),
        name: name.into(),
        bar_kg: 20.0,
        plates_kg: vec![1.25, 2.5, 5.0, 10.0, 20.0],
        dumbbell_step_kg: 2.0,
        machine_step_kg: 5.0,
        is_default: default,
        deleted: false,
    }
}

fn item(id: &str, index: u32, ex: &str) -> ProgramItem {
    ProgramItem { id: id.into(), index, exercise_id: ex.into(), sets: 3, rep_min: 7, rep_max: 9, rir: 2, style: SetStyle::Straight, rest_s: None }
}

fn program(ex: &str) -> Program {
    let w = ProgramWorkout {
        id: "w1".into(),
        index: 0,
        name: "Upper A".into(),
        slot: Some("mon_am".into()),
        external_label: None,
        items: [("i1".to_string(), item("i1", 0, ex))].into_iter().collect(),
    };
    Program {
        id: "p1".into(),
        user_id: "alice".into(),
        name: "PPL".into(),
        start_date: "2026-01-05".into(),
        weeks: 4,
        gym_profile_id: None,
        warmups: true,
        workouts: [("w1".to_string(), w)].into_iter().collect(),
        overrides: Default::default(),
        migrated_from: None,
        deleted: false,
    }
}

fn ov(week: u32, item: &str) -> (String, WeekOverride) {
    (format!("{week}:{item}"), WeekOverride { week, item_id: item.into(), sets: Some(1), rep_min: None, rep_max: None, rir: None })
}

#[test]
fn program_validation_and_idempotent_put() {
    let mut s = AmFitnessStore::new();
    let bench = s.add_exercise("Bench", MeasurementType::WeightReps, None).unwrap();
    let p = program(&bench.id);
    assert_eq!(s.put_program(p.clone()).unwrap(), Write::Created);
    let heads = s.doc.get_heads();
    assert_eq!(s.put_program(p.clone()).unwrap(), Write::Unchanged);
    assert_eq!(s.doc.get_heads(), heads);
    assert!(matches!(s.put_program(Program { name: "Other".into(), ..p.clone() }), Err(Error::Conflict(_))));

    let mut bad = |what: &str, f: &dyn Fn(&mut Program)| {
        let mut q = Program { id: format!("bad-{what}"), ..p.clone() };
        f(&mut q);
        assert!(matches!(s.put_program(q), Err(Error::Invalid(_) | Error::NotFound(_))), "{what} should be rejected");
    };
    bad("not a Monday", &|q| q.start_date = "2026-01-06".into());
    bad("zero weeks", &|q| q.weeks = 0);
    bad("min > max", &|q| q.workouts.get_mut("w1").unwrap().items.get_mut("i1").unwrap().rep_min = 10);
    bad("rir 6", &|q| q.workouts.get_mut("w1").unwrap().items.get_mut("i1").unwrap().rir = 6);
    bad("bad slot", &|q| q.workouts.get_mut("w1").unwrap().slot = Some("noday_am".into()));
    bad("missing exercise", &|q| q.workouts.get_mut("w1").unwrap().items.get_mut("i1").unwrap().exercise_id = "missing".into());
    bad("missing gym", &|q| q.gym_profile_id = Some("missing".into()));
    bad("override week", &|q| {
        q.overrides.extend([ov(9, "i1")]);
    });
    bad("override item", &|q| {
        q.overrides.extend([ov(1, "nope")]);
    });
    bad("external with items", &|q| q.workouts.get_mut("w1").unwrap().external_label = Some("PT".into()));

    let mut ok = Program { id: "with-override".into(), ..p };
    ok.overrides.extend([ov(2, "i1")]);
    assert_eq!(s.put_program(ok).unwrap(), Write::Created);
}

#[test]
fn exercise_muscles_validated() {
    let mut s = AmFitnessStore::new();
    let mut e = Exercise::new("e1".into(), "Bench", MeasurementType::WeightReps);
    e.primary_muscles = vec!["chest".into()];
    e.secondary_muscles = vec!["triceps".into(), "shoulders".into()];
    e.equipment = Some(Equipment::Barbell);
    e.default_rest_s = Some(150);
    s.put_exercise(e.clone()).unwrap();
    assert_eq!(s.get_exercise("e1").unwrap(), e);
    e.id = "e2".into();
    e.primary_muscles = vec!["pecs".into()];
    assert!(s.put_exercise(e).is_err());
}

#[test]
fn one_default_gym_profile_per_user() {
    let mut s = AmFitnessStore::new();
    s.put_gym_profile(gym("g1", "alice", "Base gym", true)).unwrap();
    s.put_gym_profile(gym("g2", "alice", "Home", true)).unwrap();
    s.put_gym_profile(gym("g3", "bob", "Bob's", true)).unwrap();
    let a: Vec<_> = s.list_gym_profiles("alice").unwrap().into_iter().map(|g| (g.name, g.is_default)).collect();
    assert_eq!(a, [("Home".to_string(), true), ("Base gym".to_string(), false)]);
    assert!(s.list_gym_profiles("bob").unwrap()[0].is_default, "other users are unaffected");
    assert!(s.put_gym_profile(GymProfile { bar_kg: 0.0, ..gym("g4", "alice", "x", false) }).is_err());
}

#[test]
fn set_log_v2_fields_roundtrip() {
    let mut s = AmFitnessStore::new();
    s.put_session(Session::start("s1".into(), "alice")).unwrap();
    let set = SetLog {
        exercise_id: "e1".into(),
        reps: Some(8),
        load: Some(80.0),
        kind: Some(SetKind::Failure),
        rir: Some(0),
        partials: Some(3),
        rest_taken_s: Some(150),
        recommendation_id: Some("rec-1".into()),
        ..Default::default()
    };
    assert_eq!(s.put_set("alice", "s1", "x", set.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_set("alice", "s1", "x", set.clone()).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_set("alice", "s1", "x", SetLog { rir: Some(1), ..set.clone() }), Err(Error::Conflict(_))));
    let got = s.get_session("s1").unwrap().sets["x"].clone();
    assert_eq!(
        (got.kind, got.rir, got.partials, got.rest_taken_s, got.recommendation_id.as_deref()),
        (Some(SetKind::Failure), Some(0), Some(3), Some(150), Some("rec-1"))
    );
}
