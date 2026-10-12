//! Behaviour every store implementation must share. Each function takes a fresh, empty store and
//! asserts through the trait only; the Automerge and SQLite crates both call them, so a rule that
//! is in one and not the other fails a test. (Automerge-only behaviour, such as merging peers,
//! stays in the Automerge crates' own tests.)

use fitness_core::*;
use shopping_core::{NewItem, ShoppingStore};
use todo_core::TodoStore;

pub fn todo_crud(mut s: impl TodoStore) {
    let t = s.add("milk", "", None).unwrap();
    s.edit(&t.id, "oat milk", "2L", Some("2026-01-02")).unwrap();
    s.set_completed(&t.id, true).unwrap();
    let got = &s.list().unwrap()[0];
    assert_eq!((got.title.as_str(), got.notes.as_str(), got.due.as_deref(), got.completed), ("oat milk", "2L", Some("2026-01-02"), true));
    s.set_completed(&t.id, false).unwrap();
    assert!(!s.list().unwrap()[0].completed);
    s.delete(&t.id).unwrap();
    assert!(s.list().unwrap().is_empty());
    assert!(matches!(s.edit("nope", "", "", None), Err(todo_core::Error::NotFound(_))));
    for bad in ["2026-13-01", "2026-1-01", "26-01-01", "2026-01-32", "２026-01-01", ""] {
        assert!(matches!(s.add("x", "", Some(bad)), Err(todo_core::Error::Invalid(_))), "{bad}");
    }
}

pub fn todo_put_is_idempotent(mut s: impl TodoStore) {
    assert!(s.put("t1", "milk", "", None).unwrap());
    assert!(!s.put("t1", "milk", "", None).unwrap());
    assert!(matches!(s.put("t1", "bread", "", None), Err(todo_core::Error::Conflict(_))));
    assert_eq!(s.get("t1").unwrap().title, "milk");
}

pub fn todo_sort_order(mut s: impl TodoStore) {
    s.add("undated", "", None).unwrap();
    s.add("later", "", Some("2030-01-01")).unwrap();
    s.add("overdue", "", Some("2000-01-01")).unwrap();
    s.add("soon", "", Some("2026-11-01")).unwrap();
    let titles: Vec<_> = s.list().unwrap().into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["overdue", "soon", "later", "undated"]);
}

fn item(id: &str, list: &str, name: &str) -> NewItem {
    NewItem { id: id.into(), list_id: list.into(), name: name.into(), quantity: None, note: None, category: None }
}

fn names(s: &impl ShoppingStore, list: &str) -> Vec<String> {
    s.items(list).unwrap().into_iter().map(|i| i.name).collect()
}

pub fn shopping_lists_and_items(mut s: impl ShoppingStore) {
    use shopping_core::{Error, Write};
    assert_eq!(s.put_list("g", "Groceries").unwrap(), Write::Created);
    assert_eq!(s.put_list("g", "Groceries").unwrap(), Write::Unchanged);
    assert!(matches!(s.put_list("g", "Other"), Err(Error::Conflict(_))));
    s.put_list("b", "bunnings").unwrap();
    assert!(s.put_list("x", "  ").is_err());
    assert_eq!(s.lists().unwrap().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["bunnings", "Groceries"]);
    s.delete_list("b").unwrap();
    assert_eq!(s.lists().unwrap().len(), 1);
    assert!(matches!(s.put_item("alice", item("i", "b", "screws")), Err(Error::NotFound(_))));

    let milk = NewItem { quantity: Some("2".into()), note: Some("full cream".into()), category: Some("dairy".into()), ..item("m", "g", " Milk ") };
    assert_eq!(s.put_item("alice", milk.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_item("alice", milk.clone()).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_item("alice", NewItem { quantity: Some("3".into()), ..milk }), Err(Error::Conflict(_))));
    assert!(s.put_item("alice", NewItem { category: Some("snacks".into()), ..item("x", "g", "Chips") }).is_err());
    let it = s.get_item("m").unwrap();
    assert_eq!((it.name.as_str(), it.quantity.as_deref(), it.category.as_deref(), it.added_by.as_str(), it.checked()), ("Milk", Some("2"), Some("dairy"), "alice", false));
}

pub fn shopping_sorting_and_checks(mut s: impl ShoppingStore) {
    s.put_list("g", "Groceries").unwrap();
    for (id, name, cat) in [("1", "Soap", Some("household")), ("2", "Apples", Some("produce")), ("3", "Bread", Some("bakery")), ("4", "Bananas", Some("produce")), ("5", "Mystery", None), ("6", "Milk", Some("dairy"))] {
        s.put_item("alice", NewItem { category: cat.map(Into::into), ..item(id, "g", name) }).unwrap();
    }
    assert_eq!(names(&s, "g"), ["Apples", "Bananas", "Milk", "Bread", "Soap", "Mystery"]);
    s.set_checked("bob", "2", true).unwrap();
    assert_eq!(names(&s, "g"), ["Bananas", "Milk", "Bread", "Soap", "Mystery", "Apples"]);
    let it = s.get_item("2").unwrap();
    assert_eq!((it.checked(), it.checked_by()), (true, Some("bob")));
    assert!(it.checked_at().unwrap() > 1_700_000_000_000);
    s.set_checked("alice", "2", true).unwrap();
    assert_eq!(s.get_item("2").unwrap().checked_by(), Some("bob"), "the first tick stands");
    s.set_checked("alice", "2", false).unwrap();
    assert!(!s.get_item("2").unwrap().checked());
}

pub fn shopping_clear_checked_and_suggestions(mut s: impl ShoppingStore) {
    s.put_list("g", "Groceries").unwrap();
    for (id, name) in [("1", "Milk"), ("2", "Eggs"), ("3", "Bread")] {
        s.put_item("alice", item(id, "g", name)).unwrap();
    }
    s.set_checked("alice", "1", true).unwrap();
    s.set_checked("alice", "2", true).unwrap();
    assert_eq!(s.clear_checked("g").unwrap(), 2);
    assert_eq!(names(&s, "g"), ["Bread"]);
    assert_eq!(s.clear_checked("g").unwrap(), 0);
    assert!(!s.get_item("1").unwrap().visible());
    std::thread::sleep(std::time::Duration::from_millis(2));
    s.set_checked("bob", "1", false).unwrap();
    s.set_checked("bob", "1", true).unwrap();
    assert_eq!(names(&s, "g"), ["Bread", "Milk"]);
    assert!(s.clear_checked("nope").is_err());
    // Eggs were cleared, so they are suggested again; Bread is on the list, so it is not.
    assert_eq!(s.suggestions("g", "e", 5).unwrap(), ["Eggs"]);
    assert!(s.suggestions("g", "bre", 5).unwrap().is_empty());
}

// ---- fitness ----

pub fn fitness_exercises_templates(mut s: impl FitnessStore) {
    let mut e = Exercise::new("e1".into(), "Bench press", MeasurementType::WeightReps);
    e.primary_muscles = vec!["chest".into()];
    e.secondary_muscles = vec!["triceps".into()];
    e.equipment = Some(Equipment::Barbell);
    assert_eq!(s.put_exercise(e.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_exercise(e.clone()).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_exercise(Exercise { name: "Other".into(), ..e.clone() }), Err(Error::Conflict(_))));
    s.put_exercise(Exercise::new("e0".into(), "arm curl", MeasurementType::WeightReps)).unwrap();
    assert_eq!(s.list_exercises().unwrap().iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["e0", "e1"], "by name, ignoring case");
    assert_eq!(s.get_exercise("e1").unwrap(), e);
    s.update_exercise(Exercise { deleted: true, ..e.clone() }).unwrap();
    assert_eq!(s.list_exercises().unwrap().len(), 1);
    assert!(matches!(s.get_exercise("nope"), Err(Error::NotFound(_))));
    assert!(s.put_exercise(Exercise { primary_muscles: vec!["elbow".into()], ..Exercise::new("e2".into(), "x", MeasurementType::WeightReps) }).is_err());

    let item = |index| TemplateItem {
        index,
        exercise_id: "e0".into(),
        target_sets: 3,
        target_reps: Some(8),
        target_load: Some(20.0),
        tempo: None,
        rest_s: Some(90),
        progression: Some(Progression { kind: ProgressionKind::LinearLoad, per_week: 2.5 }),
    };
    let t = Template { id: "t1".into(), name: "Push".into(), target_minutes: 45, items: [("i1".to_string(), item(0)), ("i2".to_string(), item(1))].into(), deleted: false };
    assert_eq!(s.put_template(t.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_template(t.clone()).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_template(Template { name: "Pull".into(), ..t.clone() }), Err(Error::Conflict(_))));
    assert_eq!(s.get_template("t1").unwrap(), t);
    assert_eq!(s.list_templates().unwrap().len(), 1);
}

pub fn fitness_plan_and_blocks(mut s: impl FitnessStore) {
    s.put_template(Template { id: "t1".into(), name: "Legs".into(), target_minutes: 30, items: Default::default(), deleted: false }).unwrap();
    let slot = |t: Option<&str>, l: Option<&str>| PlanSlot { template_id: t.map(Into::into), external_label: l.map(Into::into) };
    let plan = WeeklyPlan { user_id: "alice".into(), slots: [("mon_am".to_string(), slot(Some("t1"), None)), ("tue_pm".to_string(), slot(None, Some("Unit PT")))].into() };
    assert_eq!(s.get_plan("alice").unwrap(), None);
    assert_eq!(s.set_plan(plan.clone()).unwrap(), Write::Created);
    assert_eq!(s.set_plan(plan.clone()).unwrap(), Write::Unchanged);
    assert_eq!(s.get_plan("alice").unwrap(), Some(plan.clone()));
    let mut changed = plan.clone();
    changed.slots.remove("tue_pm");
    assert_eq!(s.set_plan(changed.clone()).unwrap(), Write::Updated);
    assert_eq!(s.get_plan("alice").unwrap(), Some(changed));
    assert_eq!(s.list_plans().unwrap().len(), 1);
    let bad = WeeklyPlan { user_id: "alice".into(), slots: [("funday_am".to_string(), slot(Some("t1"), None))].into() };
    assert!(s.set_plan(bad).is_err());
    let none = WeeklyPlan { user_id: "alice".into(), slots: [("mon_am".to_string(), slot(Some("nope"), None))].into() };
    assert!(matches!(s.set_plan(none), Err(Error::NotFound(_))));

    let blk = Block { id: "b1".into(), user_id: "alice".into(), name: "Block".into(), start_date: "2026-01-05".into(), weeks: 6, deload_week: Some(6), deload_pct: Some(60.0), deleted: false };
    assert_eq!(s.put_block(blk.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_block(blk.clone()).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_block(Block { name: "Other".into(), ..blk.clone() }), Err(Error::Conflict(_))));
    assert_eq!(s.get_block("b1").unwrap(), blk);
    assert_eq!(s.list_blocks().unwrap().len(), 1);
}

pub fn fitness_sessions_and_sets(mut s: impl FitnessStore) {
    s.put_exercise(Exercise::new("e1".into(), "Squat", MeasurementType::WeightReps)).unwrap();
    let mut a = Session::start("s1".into(), "alice");
    a.started_at = 1000;
    let mut b = Session::start("s2".into(), "bob");
    b.started_at = 2000;
    assert_eq!(s.put_session(a.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_session(a.clone()).unwrap(), Write::Unchanged);
    s.put_session(b).unwrap();
    assert!(matches!(s.put_session(Session { slot: Some("AM".into()), ..a.clone() }), Err(Error::Conflict(_))));
    assert_eq!(s.list_sessions().unwrap().iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["s2", "s1"], "newest first");

    let set = SetLog { exercise_id: "e1".into(), load: Some(60.0), reps: Some(5), rir: Some(2), ..Default::default() };
    assert_eq!(s.put_set("alice", "s1", "a", set.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_set("alice", "s1", "a", set.clone()).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_set("alice", "s1", "a", SetLog { reps: Some(6), ..set.clone() }), Err(Error::Conflict(_))));
    assert!(matches!(s.put_set("bob", "s1", "z", set.clone()), Err(Error::NotOwner)));
    s.put_set("alice", "s1", "b", SetLog { reps: Some(4), ..set.clone() }).unwrap();
    let got = s.get_session("s1").unwrap();
    assert_eq!((got.sets["a"].index, got.sets["b"].index), (0, 1));
    assert_eq!(got.sets["a"].rir, Some(2));

    assert!(matches!(s.finish_session("bob", "s1", SessionStatus::Completed, None), Err(Error::NotOwner)));
    s.finish_session("alice", "s1", SessionStatus::Completed, Some("good")).unwrap();
    let got = s.get_session("s1").unwrap();
    assert_eq!((got.status, got.note.as_deref()), (SessionStatus::Completed, Some("good")));
    let fin = got.finished_at;
    assert!(fin.is_some());
    s.finish_session("alice", "s1", SessionStatus::Completed, Some("good")).unwrap();
    assert_eq!(s.get_session("s1").unwrap().finished_at, fin, "finishing twice changes nothing");
}

pub fn fitness_tests_and_external(mut s: impl FitnessStore) {
    let t = |id: &str, value: f64| TestResult { id: id.into(), user_id: "alice".into(), kind: TestKind::TimedRun, date: "2026-01-01".into(), beep: None, value: Some(value), exercise_id: None, deleted: false };
    s.put_test(t("r1", 600.0)).unwrap();
    s.put_test(t("r2", 540.0)).unwrap();
    assert_eq!(s.put_test(t("r2", 540.0)).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_test(t("r2", 500.0)), Err(Error::Conflict(_))));
    assert_eq!(s.list_tests(&TestKind::TimedRun).unwrap().iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["r2", "r1"], "faster first");

    let a = ExternalActivity { source: "strava".into(), source_id: "9".into(), user_id: "alice".into(), started_at: 5, duration_s: 100, distance_m: Some(500.0), linked_session_id: None };
    assert_eq!(s.upsert_external(a.clone()).unwrap(), Write::Created);
    assert_eq!(s.upsert_external(a.clone()).unwrap(), Write::Unchanged);
    assert_eq!(s.upsert_external(ExternalActivity { duration_s: 120, ..a.clone() }).unwrap(), Write::Updated);
    assert!(matches!(s.upsert_external(ExternalActivity { user_id: "bob".into(), ..a.clone() }), Err(Error::NotOwner)));
    assert!(s.upsert_external(ExternalActivity { source: "a:b".into(), ..a }).is_err());
}

pub fn fitness_gyms_and_programs(mut s: impl FitnessStore) {
    let g = |id: &str, name: &str, default| GymProfile {
        id: id.into(),
        user_id: "alice".into(),
        name: name.into(),
        bar_kg: 20.0,
        plates_kg: vec![20.0, 10.0, 5.0, 2.5, 1.25],
        dumbbell_step_kg: 2.0,
        machine_step_kg: 5.0,
        is_default: default,
        deleted: false,
    };
    assert_eq!(s.put_gym_profile(g("g1", "Home", true)).unwrap(), Write::Created);
    assert_eq!(s.put_gym_profile(g("g1", "Home", true)).unwrap(), Write::Unchanged);
    assert!(matches!(s.put_gym_profile(g("g1", "Else", true)), Err(Error::Conflict(_))));
    s.put_gym_profile(g("g2", "Gym", true)).unwrap();
    let list = s.list_gym_profiles("alice").unwrap();
    assert_eq!(list.iter().map(|x| (x.id.as_str(), x.is_default)).collect::<Vec<_>>(), [("g2", true), ("g1", false)], "one default, listed first");
    s.update_gym_profile(g("g1", "Home", true)).unwrap();
    assert_eq!(s.get_gym_profile("g1").unwrap().plates_kg, vec![20.0, 10.0, 5.0, 2.5, 1.25]);
    assert!(!s.get_gym_profile("g2").unwrap().is_default);
    assert!(s.list_gym_profiles("bob").unwrap().is_empty());

    s.put_exercise(Exercise::new("e1".into(), "Squat", MeasurementType::WeightReps)).unwrap();
    let item = ProgramItem { id: "pi1".into(), index: 0, exercise_id: "e1".into(), sets: 3, rep_min: 6, rep_max: 8, rir: 2, style: SetStyle::Straight, rest_s: None };
    let w = ProgramWorkout { id: "w1".into(), index: 0, name: "Day A".into(), slot: Some("mon_am".into()), external_label: None, items: [("pi1".to_string(), item)].into() };
    let ov = WeekOverride { week: 2, item_id: "pi1".into(), sets: Some(4), rep_min: None, rep_max: None, rir: Some(1) };
    let p = Program {
        id: "p1".into(),
        user_id: "alice".into(),
        name: "Prog".into(),
        start_date: "2026-01-05".into(),
        weeks: 8,
        gym_profile_id: Some("g1".into()),
        warmups: true,
        workouts: [("w1".to_string(), w)].into(),
        overrides: [("2:pi1".to_string(), ov)].into(),
        migrated_from: None,
        deleted: false,
    };
    assert_eq!(s.put_program(p.clone()).unwrap(), Write::Created);
    assert_eq!(s.put_program(p.clone()).unwrap(), Write::Unchanged);
    assert_eq!(s.get_program("p1").unwrap(), p);
    assert!(matches!(s.put_program(Program { name: "x".into(), ..p.clone() }), Err(Error::Conflict(_))));
    assert!(matches!(s.put_program(Program { id: "p2".into(), user_id: "bob".into(), ..p.clone() }), Err(Error::NotOwner)), "someone else's gym");
    s.update_program(Program { weeks: 10, ..p.clone() }).unwrap();
    assert_eq!(s.get_program("p1").unwrap().weeks, 10);
    assert_eq!(s.list_programs().unwrap().len(), 1);
}
