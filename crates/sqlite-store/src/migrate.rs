//! `peer migrate-to-sqlite`: copy the Automerge documents into the database in ONE transaction,
//! then compare per-table row counts with the source. Any failure rolls everything back.

use crate::{Db, SqliteFitnessStore, SqliteShoppingStore, SqliteTodoStore};
use fitness_core::FitnessExport;
use shopping_core::ShoppingExport;
use todo_core::Todo;

/// `(table, rows in the source, rows in the database)`.
pub type Report = Vec<(String, usize, usize)>;

pub fn migrate(db: &Db, todos: &[Todo], shopping: &ShoppingExport, fitness: &FitnessExport) -> Result<Report, String> {
    let begin = db.with(|c| c.execute_batch("BEGIN IMMEDIATE"));
    begin?;
    let out = copy(db, todos, shopping, fitness);
    match &out {
        Ok(_) => db.with(|c| c.execute_batch("COMMIT"))?,
        Err(_) => {
            let _ = db.with(|c| c.execute_batch("ROLLBACK"));
        }
    }
    out
}

fn copy(db: &Db, todos: &[Todo], shopping: &ShoppingExport, fitness: &FitnessExport) -> Result<Report, String> {
    let t = SqliteTodoStore::new(db.clone());
    for x in todos {
        t.import(x).map_err(|e| e.to_string())?;
    }
    SqliteShoppingStore::new(db.clone()).import(shopping).map_err(|e| e.to_string())?;
    db.with(|c| SqliteFitnessStore::import_in(c, fitness))?;

    let f = SqliteFitnessStore::new(db.clone());
    let (lists, items) = SqliteShoppingStore::new(db.clone()).counts().map_err(|e| e.to_string())?;
    let mut rows: Report = vec![
        ("todos".into(), todos.len(), t.count().map_err(|e| e.to_string())?),
        ("shopping_lists".into(), shopping.lists.len(), lists),
        ("shopping_items".into(), shopping.items.len(), items),
    ];
    let sets: usize = fitness.sessions.iter().map(|s| s.sets.len()).sum();
    let tpl_items: usize = fitness.templates.iter().map(|t| t.items.len()).sum();
    let prog_items: usize = fitness.programs.iter().flat_map(|p| p.workouts.values()).map(|w| w.items.len()).sum();
    let expect = [
        ("exercises", fitness.exercises.len()),
        ("templates", fitness.templates.len()),
        ("plans", fitness.plans.len()),
        ("blocks", fitness.blocks.len()),
        ("sessions", fitness.sessions.len()),
        ("session_sets", sets),
        ("tests", fitness.tests.len()),
        ("external_activities", fitness.external_activities.len()),
        ("gym_profiles", fitness.gym_profiles.len()),
        ("programs", fitness.programs.len()),
        ("program_items", prog_items),
        ("template_items", tpl_items),
    ];
    let got = f.counts().map_err(|e| e.to_string())?;
    for (name, n) in expect {
        let g = got.iter().find(|(t, _)| *t == name).map(|(_, n)| *n).unwrap_or(0);
        rows.push((name.into(), n, g));
    }
    if let Some((t, a, b)) = rows.iter().find(|(_, a, b)| a != b) {
        return Err(format!("row count mismatch in {t}: {a} in the source, {b} in the database"));
    }
    Ok(rows)
}
