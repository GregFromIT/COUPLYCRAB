use super::*;
use store_conformance as c;

fn db() -> Db {
    Db::open_in_memory().unwrap()
}

macro_rules! conform {
    ($($name:ident: $store:ident),* $(,)?) => {$(
        #[test]
        fn $name() {
            c::$name($store::new(db()));
        }
    )*};
}

conform!(
    todo_crud: SqliteTodoStore,
    todo_put_is_idempotent: SqliteTodoStore,
    todo_sort_order: SqliteTodoStore,
    shopping_lists_and_items: SqliteShoppingStore,
    shopping_sorting_and_checks: SqliteShoppingStore,
    shopping_clear_checked_and_suggestions: SqliteShoppingStore,
    fitness_exercises_templates: SqliteFitnessStore,
    fitness_plan_and_blocks: SqliteFitnessStore,
    fitness_sessions_and_sets: SqliteFitnessStore,
    fitness_tests_and_external: SqliteFitnessStore,
    fitness_gyms_and_programs: SqliteFitnessStore,
);

#[test]
fn opens_with_wal_fks_and_latest_version() {
    let dir = std::env::temp_dir().join(format!("hh-{}", uuid::Uuid::new_v4()));
    let d = Db::open(&dir.join("h.sqlite")).unwrap();
    assert_eq!(d.user_version().unwrap() as usize, schema_version());
    assert_eq!(d.with(|c| c.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))).unwrap(), "wal");
    assert_eq!(d.with(|c| c.pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))).unwrap(), 1);
    drop(d);
    // reopening is a no-op migration
    assert_eq!(Db::open(&dir.join("h.sqlite")).unwrap().user_version().unwrap() as usize, schema_version());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn backup_is_a_consistent_copy_and_refuses_to_overwrite() {
    use todo_core::TodoStore;
    let d = db();
    SqliteTodoStore::new(d.clone()).add("milk", "", None).unwrap();
    let dir = std::env::temp_dir().join(format!("hh-{}", uuid::Uuid::new_v4()));
    let out = dir.join("b.sqlite");
    d.backup_to(&out).unwrap();
    let copy = rusqlite::Connection::open(&out).unwrap();
    assert_eq!(copy.query_row("SELECT COUNT(*) FROM todos", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert!(d.backup_to(&out).is_err());
    drop(copy);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn schema_rejects_bad_enum_text() {
    let d = db();
    let r = d.with(|c| c.execute("INSERT INTO sessions (id, user_id, started_at, status) VALUES ('s', 'a', 0, 'bogus')", []));
    assert!(r.is_err());
}

#[test]
fn migration_copies_everything_and_rolls_back_on_error() {
    use fitness_core::{AmFitnessStore, Exercise, FitnessStore, MeasurementType, Session, SetLog};
    use shopping_core::{AmShoppingStore, NewItem, ShoppingStore};
    use todo_core::{AmTodoStore, TodoStore};

    let mut t = AmTodoStore::new();
    t.add("a", "n", Some("2026-02-03")).unwrap();
    let done = t.add("b", "", None).unwrap();
    t.set_completed(&done.id, true).unwrap();
    let mut sh = AmShoppingStore::new();
    sh.put_list("g", "Groceries").unwrap();
    let new = |id: &str, name: &str, category: Option<&str>| NewItem { id: id.into(), list_id: "g".into(), name: name.into(), quantity: None, note: None, category: category.map(Into::into) };
    sh.put_item("alice", new("m", "Milk", Some("dairy"))).unwrap();
    sh.put_item("alice", new("e", "Eggs", None)).unwrap();
    sh.set_checked("bob", "e", true).unwrap();
    sh.clear_checked("g").unwrap();
    let mut f = AmFitnessStore::new();
    f.put_exercise(Exercise::new("e1".into(), "Squat", MeasurementType::WeightReps)).unwrap();
    f.put_session(Session::start("s1".into(), "alice")).unwrap();
    f.put_set("alice", "s1", "a", SetLog { exercise_id: "e1".into(), load: Some(80.0), reps: Some(5), ..Default::default() }).unwrap();

    let d = db();
    let (te, se, fe) = (t.export().unwrap(), sh.export().unwrap(), f.export().unwrap());
    let report = migrate::migrate(&d, &te, &se, &fe).unwrap();
    assert!(report.iter().all(|(_, a, b)| a == b));
    assert!(report.contains(&("session_sets".into(), 1, 1)));
    let todos = SqliteTodoStore::new(d.clone());
    assert_eq!(todos.list().unwrap().len(), 2);
    assert_eq!(todos.list().unwrap().iter().find(|x| x.title == "b").unwrap().created_at, done.created_at);
    let shop = SqliteShoppingStore::new(d.clone());
    assert_eq!(shop.items("g").unwrap().iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["Milk"], "the cleared tick stays cleared");
    assert_eq!(shop.get_item("e").unwrap().checked_by(), Some("bob"));
    assert_eq!(SqliteFitnessStore::new(d.clone()).get_session("s1").unwrap().sets["a"].load, Some(80.0));

    // A bad row (a set pointing at a missing exercise) rolls the whole copy back.
    let mut bad = fe.clone();
    bad.sessions[0].sets.get_mut("a").unwrap().exercise_id = "ghost".into();
    let d2 = db();
    assert!(migrate::migrate(&d2, &te, &se, &bad).is_err());
    assert_eq!(SqliteTodoStore::new(d2).count().unwrap(), 0, "nothing was left behind");
}

/// The same suites against the Automerge stores (they live here because `store-conformance`
/// depends on the core crates, so they cannot dev-depend on it).
mod automerge {
    use store_conformance as c;
    #[test]
    fn all() {
        c::todo_crud(todo_core::AmTodoStore::new());
        c::todo_put_is_idempotent(todo_core::AmTodoStore::new());
        c::todo_sort_order(todo_core::AmTodoStore::new());
        c::shopping_lists_and_items(shopping_core::AmShoppingStore::new());
        c::shopping_sorting_and_checks(shopping_core::AmShoppingStore::new());
        c::shopping_clear_checked_and_suggestions(shopping_core::AmShoppingStore::new());
        c::fitness_exercises_templates(fitness_core::AmFitnessStore::new());
        c::fitness_plan_and_blocks(fitness_core::AmFitnessStore::new());
        c::fitness_sessions_and_sets(fitness_core::AmFitnessStore::new());
        c::fitness_tests_and_external(fitness_core::AmFitnessStore::new());
        c::fitness_gyms_and_programs(fitness_core::AmFitnessStore::new());
    }
}
