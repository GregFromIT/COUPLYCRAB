//! `FitnessStore` on SQLite. Same rules as the Automerge store (validation, idempotent `put_*`,
//! `Conflict` on different content, ownership checks); the shared validators live in `fitness-core`.
//! Everything derived (recommendations, planned days, next workout, migration) is a provided method
//! of the trait and works unchanged on top of these primitives.

use crate::{b, is_constraint, Db};
use fitness_core::*;
use rusqlite::{params, Connection, OptionalExtension};
use std::{cmp::Reverse, collections::HashMap};

type R<T> = fitness_core::Result<T>;

fn db(e: rusqlite::Error) -> Error {
    if is_constraint(&e) { Error::Invalid(format!("the database rejected that: {e}")) } else { Error::Storage(e.to_string()) }
}

pub struct SqliteFitnessStore {
    db: Db,
}

impl SqliteFitnessStore {
    pub fn new(db: Db) -> Self {
        SqliteFitnessStore { db }
    }

    /// Copy a whole export in, keeping IDs and timestamps. Runs inside the caller's transaction.
    pub(crate) fn import_in(c: &Connection, e: &FitnessExport) -> rusqlite::Result<()> {
        for x in &e.exercises {
            save_exercise(c, x)?;
        }
        for x in &e.templates {
            save_template(c, x)?;
        }
        for x in &e.gym_profiles {
            save_gym(c, x)?;
        }
        for x in &e.plans {
            save_plan(c, x)?;
        }
        for x in &e.blocks {
            save_block(c, x)?;
        }
        for x in &e.programs {
            save_program(c, x)?;
        }
        for x in &e.sessions {
            save_session(c, x)?;
        }
        for x in &e.tests {
            save_test(c, x)?;
        }
        for x in &e.external_activities {
            save_external(c, x)?;
        }
        Ok(())
    }

    /// Row counts per table, in a fixed order (for verifying a migration).
    pub fn counts(&self) -> R<Vec<(&'static str, usize)>> {
        const T: [&str; 12] = [
            "exercises", "templates", "plans", "blocks", "sessions", "session_sets", "tests", "external_activities", "gym_profiles", "programs", "program_items", "template_items",
        ];
        T.iter()
            .map(|t| self.db.run(|c| c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get::<_, i64>(0))).map(|n| (*t, n as usize)).map_err(db))
            .collect()
    }
}

fn i(x: Option<u32>) -> Option<i64> {
    x.map(i64::from)
}

// ---- exercises ----

fn save_exercise(c: &Connection, e: &Exercise) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO exercises (id, name, measurement, tempo, deleted, equipment, default_rest_s) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET name = ?2, measurement = ?3, tempo = ?4, deleted = ?5, equipment = ?6, default_rest_s = ?7",
        params![e.id, e.name, e.measurement.as_str(), e.tempo, b(e.deleted), e.equipment.as_ref().map(|q| q.as_str()), i(e.default_rest_s)],
    )?;
    c.execute("DELETE FROM exercise_muscles WHERE exercise_id = ?1", [&e.id])?;
    for (role, list) in [("primary", &e.primary_muscles), ("secondary", &e.secondary_muscles)] {
        for (pos, m) in list.iter().enumerate() {
            c.execute("INSERT INTO exercise_muscles (exercise_id, role, muscle, position) VALUES (?1, ?2, ?3, ?4)", params![e.id, role, m, pos as i64])?;
        }
    }
    Ok(())
}

fn load_exercises(c: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<Exercise>> {
    let mut st = c.prepare("SELECT id, name, measurement, tempo, deleted, equipment, default_rest_s FROM exercises WHERE (?1 IS NULL OR id = ?1) ORDER BY id")?;
    let mut out: Vec<Exercise> = st
        .query_map([id], |r| {
            Ok(Exercise {
                id: r.get(0)?,
                name: r.get(1)?,
                measurement: r.get::<_, String>(2)?.parse::<MeasurementType>().unwrap(),
                tempo: r.get(3)?,
                deleted: r.get::<_, i64>(4)? != 0,
                primary_muscles: vec![],
                secondary_muscles: vec![],
                equipment: r.get::<_, Option<String>>(5)?.map(|s| s.parse::<Equipment>().unwrap()),
                default_rest_s: r.get::<_, Option<i64>>(6)?.map(|n| n as u32),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<String, usize> = out.iter().enumerate().map(|(n, e)| (e.id.clone(), n)).collect();
    let mut st = c.prepare("SELECT exercise_id, role, muscle FROM exercise_muscles WHERE (?1 IS NULL OR exercise_id = ?1) ORDER BY exercise_id, position")?;
    let rows = st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    for row in rows {
        let (ex, role, muscle) = row?;
        if let Some(&n) = index.get(&ex) {
            if role == "primary" { out[n].primary_muscles.push(muscle) } else { out[n].secondary_muscles.push(muscle) }
        }
    }
    Ok(out)
}

// ---- templates ----

fn save_template(c: &Connection, t: &Template) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO templates (id, name, target_minutes, deleted) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (id) DO UPDATE SET name = ?2, target_minutes = ?3, deleted = ?4",
        params![t.id, t.name, i64::from(t.target_minutes), b(t.deleted)],
    )?;
    c.execute("DELETE FROM template_items WHERE template_id = ?1", [&t.id])?;
    for (item_id, it) in &t.items {
        c.execute(
            "INSERT INTO template_items (template_id, item_id, idx, exercise_id, target_sets, target_reps, target_load, tempo, rest_s, prog_kind, prog_per_week)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                t.id, item_id, i64::from(it.index), it.exercise_id, i64::from(it.target_sets), i(it.target_reps), it.target_load, it.tempo, i(it.rest_s),
                it.progression.as_ref().map(|p| p.kind.as_str()), it.progression.as_ref().map(|p| p.per_week)
            ],
        )?;
    }
    Ok(())
}

fn load_templates(c: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<Template>> {
    let mut st = c.prepare("SELECT id, name, target_minutes, deleted FROM templates WHERE (?1 IS NULL OR id = ?1) ORDER BY id")?;
    let mut out: Vec<Template> = st
        .query_map([id], |r| Ok(Template { id: r.get(0)?, name: r.get(1)?, target_minutes: r.get::<_, i64>(2)? as u32, items: HashMap::new(), deleted: r.get::<_, i64>(3)? != 0 }))?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<String, usize> = out.iter().enumerate().map(|(n, t)| (t.id.clone(), n)).collect();
    let mut st = c.prepare(
        "SELECT template_id, item_id, idx, exercise_id, target_sets, target_reps, target_load, tempo, rest_s, prog_kind, prog_per_week
         FROM template_items WHERE (?1 IS NULL OR template_id = ?1)",
    )?;
    let rows = st.query_map([id], |r| {
        let kind: Option<String> = r.get(9)?;
        let per_week: Option<f64> = r.get(10)?;
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            TemplateItem {
                index: r.get::<_, i64>(2)? as u32,
                exercise_id: r.get(3)?,
                target_sets: r.get::<_, i64>(4)? as u32,
                target_reps: r.get::<_, Option<i64>>(5)?.map(|n| n as u32),
                target_load: r.get(6)?,
                tempo: r.get(7)?,
                rest_s: r.get::<_, Option<i64>>(8)?.map(|n| n as u32),
                progression: kind.zip(per_week).map(|(k, p)| Progression { kind: k.parse::<ProgressionKind>().unwrap(), per_week: p }),
            },
        ))
    })?;
    for row in rows {
        let (t, item_id, item) = row?;
        if let Some(&n) = index.get(&t) {
            out[n].items.insert(item_id, item);
        }
    }
    Ok(out)
}

// ---- plans and blocks ----

fn save_plan(c: &Connection, p: &WeeklyPlan) -> rusqlite::Result<()> {
    c.execute("INSERT OR IGNORE INTO plans (user_id) VALUES (?1)", [&p.user_id])?;
    c.execute("DELETE FROM plan_slots WHERE user_id = ?1", [&p.user_id])?;
    for (slot, s) in &p.slots {
        c.execute("INSERT INTO plan_slots (user_id, slot, template_id, external_label) VALUES (?1, ?2, ?3, ?4)", params![p.user_id, slot, s.template_id, s.external_label])?;
    }
    Ok(())
}

fn load_plans(c: &Connection, user: Option<&str>) -> rusqlite::Result<Vec<WeeklyPlan>> {
    let mut st = c.prepare("SELECT user_id FROM plans WHERE (?1 IS NULL OR user_id = ?1) ORDER BY user_id")?;
    let mut out: Vec<WeeklyPlan> = st.query_map([user], |r| Ok(WeeklyPlan { user_id: r.get(0)?, slots: HashMap::new() }))?.collect::<rusqlite::Result<_>>()?;
    let index: HashMap<String, usize> = out.iter().enumerate().map(|(n, p)| (p.user_id.clone(), n)).collect();
    let mut st = c.prepare("SELECT user_id, slot, template_id, external_label FROM plan_slots WHERE (?1 IS NULL OR user_id = ?1)")?;
    let rows = st.query_map([user], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, PlanSlot { template_id: r.get(2)?, external_label: r.get(3)? })))?;
    for row in rows {
        let (u, slot, s) = row?;
        if let Some(&n) = index.get(&u) {
            out[n].slots.insert(slot, s);
        }
    }
    Ok(out)
}

fn save_block(c: &Connection, x: &Block) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO blocks (id, user_id, name, start_date, weeks, deload_week, deload_pct, deleted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET user_id = ?2, name = ?3, start_date = ?4, weeks = ?5, deload_week = ?6, deload_pct = ?7, deleted = ?8",
        params![x.id, x.user_id, x.name, x.start_date, i64::from(x.weeks), i(x.deload_week), x.deload_pct, b(x.deleted)],
    )
    .map(|_| ())
}

fn load_blocks(c: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<Block>> {
    let mut st = c.prepare("SELECT id, user_id, name, start_date, weeks, deload_week, deload_pct, deleted FROM blocks WHERE (?1 IS NULL OR id = ?1) ORDER BY id")?;
    let rows = st.query_map([id], |r| {
        Ok(Block {
            id: r.get(0)?,
            user_id: r.get(1)?,
            name: r.get(2)?,
            start_date: r.get(3)?,
            weeks: r.get::<_, i64>(4)? as u32,
            deload_week: r.get::<_, Option<i64>>(5)?.map(|n| n as u32),
            deload_pct: r.get(6)?,
            deleted: r.get::<_, i64>(7)? != 0,
        })
    })?;
    rows.collect()
}

// ---- gym profiles ----

fn save_gym(c: &Connection, g: &GymProfile) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO gym_profiles (id, user_id, name, bar_kg, dumbbell_step_kg, machine_step_kg, is_default, deleted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET user_id = ?2, name = ?3, bar_kg = ?4, dumbbell_step_kg = ?5, machine_step_kg = ?6, is_default = ?7, deleted = ?8",
        params![g.id, g.user_id, g.name, g.bar_kg, g.dumbbell_step_kg, g.machine_step_kg, b(g.is_default), b(g.deleted)],
    )?;
    c.execute("DELETE FROM gym_plates WHERE gym_id = ?1", [&g.id])?;
    for (pos, p) in g.plates_kg.iter().enumerate() {
        c.execute("INSERT INTO gym_plates (gym_id, position, plate_kg) VALUES (?1, ?2, ?3)", params![g.id, pos as i64, p])?;
    }
    Ok(())
}

fn load_gyms(c: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<GymProfile>> {
    let mut st = c.prepare("SELECT id, user_id, name, bar_kg, dumbbell_step_kg, machine_step_kg, is_default, deleted FROM gym_profiles WHERE (?1 IS NULL OR id = ?1) ORDER BY id")?;
    let mut out: Vec<GymProfile> = st
        .query_map([id], |r| {
            Ok(GymProfile {
                id: r.get(0)?,
                user_id: r.get(1)?,
                name: r.get(2)?,
                bar_kg: r.get(3)?,
                plates_kg: vec![],
                dumbbell_step_kg: r.get(4)?,
                machine_step_kg: r.get(5)?,
                is_default: r.get::<_, i64>(6)? != 0,
                deleted: r.get::<_, i64>(7)? != 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<String, usize> = out.iter().enumerate().map(|(n, g)| (g.id.clone(), n)).collect();
    let mut st = c.prepare("SELECT gym_id, plate_kg FROM gym_plates WHERE (?1 IS NULL OR gym_id = ?1) ORDER BY gym_id, position")?;
    for row in st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))? {
        let (g, p) = row?;
        if let Some(&n) = index.get(&g) {
            out[n].plates_kg.push(p);
        }
    }
    Ok(out)
}

// ---- programs ----

fn save_program(c: &Connection, p: &Program) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO programs (id, user_id, name, start_date, weeks, gym_profile_id, warmups, migrated_from, deleted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET user_id = ?2, name = ?3, start_date = ?4, weeks = ?5, gym_profile_id = ?6, warmups = ?7, migrated_from = ?8, deleted = ?9",
        params![p.id, p.user_id, p.name, p.start_date, i64::from(p.weeks), p.gym_profile_id, b(p.warmups), p.migrated_from, b(p.deleted)],
    )?;
    c.execute("DELETE FROM program_workouts WHERE program_id = ?1", [&p.id])?; // cascades to items and overrides
    for w in p.workouts.values() {
        c.execute(
            "INSERT INTO program_workouts (program_id, workout_id, idx, name, slot, external_label) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![p.id, w.id, i64::from(w.index), w.name, w.slot, w.external_label],
        )?;
        for it in w.items.values() {
            c.execute(
                "INSERT INTO program_items (program_id, item_id, workout_id, idx, exercise_id, sets, rep_min, rep_max, rir, style, rest_s)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![p.id, it.id, w.id, i64::from(it.index), it.exercise_id, i64::from(it.sets), i64::from(it.rep_min), i64::from(it.rep_max), i64::from(it.rir), it.style.as_str(), i(it.rest_s)],
            )?;
        }
    }
    for o in p.overrides.values() {
        c.execute(
            "INSERT INTO program_overrides (program_id, week, item_id, sets, rep_min, rep_max, rir) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![p.id, i64::from(o.week), o.item_id, i(o.sets), i(o.rep_min), i(o.rep_max), o.rir.map(i64::from)],
        )?;
    }
    Ok(())
}

fn load_programs(c: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<Program>> {
    let mut st = c.prepare("SELECT id, user_id, name, start_date, weeks, gym_profile_id, warmups, migrated_from, deleted FROM programs WHERE (?1 IS NULL OR id = ?1) ORDER BY id")?;
    let mut out: Vec<Program> = st
        .query_map([id], |r| {
            Ok(Program {
                id: r.get(0)?,
                user_id: r.get(1)?,
                name: r.get(2)?,
                start_date: r.get(3)?,
                weeks: r.get::<_, i64>(4)? as u32,
                gym_profile_id: r.get(5)?,
                warmups: r.get::<_, i64>(6)? != 0,
                workouts: HashMap::new(),
                overrides: HashMap::new(),
                migrated_from: r.get(7)?,
                deleted: r.get::<_, i64>(8)? != 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<String, usize> = out.iter().enumerate().map(|(n, p)| (p.id.clone(), n)).collect();
    let mut st = c.prepare("SELECT program_id, workout_id, idx, name, slot, external_label FROM program_workouts WHERE (?1 IS NULL OR program_id = ?1)")?;
    for row in st.query_map([id], |r| {
        Ok((r.get::<_, String>(0)?, ProgramWorkout { id: r.get(1)?, index: r.get::<_, i64>(2)? as u32, name: r.get(3)?, slot: r.get(4)?, external_label: r.get(5)?, items: HashMap::new() }))
    })? {
        let (p, w) = row?;
        if let Some(&n) = index.get(&p) {
            out[n].workouts.insert(w.id.clone(), w);
        }
    }
    let mut st = c.prepare("SELECT program_id, workout_id, item_id, idx, exercise_id, sets, rep_min, rep_max, rir, style, rest_s FROM program_items WHERE (?1 IS NULL OR program_id = ?1)")?;
    for row in st.query_map([id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            ProgramItem {
                id: r.get(2)?,
                index: r.get::<_, i64>(3)? as u32,
                exercise_id: r.get(4)?,
                sets: r.get::<_, i64>(5)? as u32,
                rep_min: r.get::<_, i64>(6)? as u32,
                rep_max: r.get::<_, i64>(7)? as u32,
                rir: r.get::<_, i64>(8)? as u8,
                style: r.get::<_, String>(9)?.parse::<SetStyle>().unwrap(),
                rest_s: r.get::<_, Option<i64>>(10)?.map(|n| n as u32),
            },
        ))
    })? {
        let (p, w, it) = row?;
        if let Some(&n) = index.get(&p) {
            if let Some(wk) = out[n].workouts.get_mut(&w) {
                wk.items.insert(it.id.clone(), it);
            }
        }
    }
    let mut st = c.prepare("SELECT program_id, week, item_id, sets, rep_min, rep_max, rir FROM program_overrides WHERE (?1 IS NULL OR program_id = ?1)")?;
    for row in st.query_map([id], |r| {
        let ov = |k: usize| -> rusqlite::Result<Option<u32>> { Ok(r.get::<_, Option<i64>>(k)?.map(|n| n as u32)) };
        Ok((
            r.get::<_, String>(0)?,
            WeekOverride { week: r.get::<_, i64>(1)? as u32, item_id: r.get(2)?, sets: ov(3)?, rep_min: ov(4)?, rep_max: ov(5)?, rir: r.get::<_, Option<i64>>(6)?.map(|n| n as u8) },
        ))
    })? {
        let (p, o) = row?;
        if let Some(&n) = index.get(&p) {
            out[n].overrides.insert(format!("{}:{}", o.week, o.item_id), o);
        }
    }
    Ok(out)
}

// ---- sessions ----

fn save_session(c: &Connection, s: &Session) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO sessions (id, user_id, template_id, started_at, finished_at, status, note, deleted, block_id, block_week, slot, label, program_id, workout_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT (id) DO UPDATE SET user_id = ?2, template_id = ?3, started_at = ?4, finished_at = ?5, status = ?6, note = ?7, deleted = ?8,
             block_id = ?9, block_week = ?10, slot = ?11, label = ?12, program_id = ?13, workout_id = ?14",
        params![s.id, s.user_id, s.template_id, s.started_at, s.finished_at, s.status.as_str(), s.note, b(s.deleted), s.block_id, i(s.block_week), s.slot, s.label, s.program_id, s.workout_id],
    )?;
    c.execute("DELETE FROM session_sets WHERE session_id = ?1", [&s.id])?;
    for (set_id, x) in &s.sets {
        insert_set(c, &s.id, set_id, x)?;
    }
    Ok(())
}

fn insert_set(c: &Connection, session: &str, set_id: &str, x: &SetLog) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO session_sets (session_id, set_id, idx, exercise_id, reps, load, duration_s, distance_m, kind, rir, partials, rest_taken_s, recommendation_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            session, set_id, i64::from(x.index), x.exercise_id, i(x.reps), x.load, i(x.duration_s), x.distance_m, x.kind.as_ref().map(|k| k.as_str()),
            x.rir.map(i64::from), x.partials.map(i64::from), i(x.rest_taken_s), x.recommendation_id
        ],
    )
    .map(|_| ())
}

fn load_sessions(c: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<Session>> {
    let mut st = c.prepare(
        "SELECT id, user_id, template_id, started_at, finished_at, status, note, deleted, block_id, block_week, slot, label, program_id, workout_id
         FROM sessions WHERE (?1 IS NULL OR id = ?1) ORDER BY id",
    )?;
    let mut out: Vec<Session> = st
        .query_map([id], |r| {
            Ok(Session {
                id: r.get(0)?,
                user_id: r.get(1)?,
                template_id: r.get(2)?,
                started_at: r.get(3)?,
                finished_at: r.get(4)?,
                status: r.get::<_, String>(5)?.parse::<SessionStatus>().unwrap(),
                note: r.get(6)?,
                sets: HashMap::new(),
                deleted: r.get::<_, i64>(7)? != 0,
                block_id: r.get(8)?,
                block_week: r.get::<_, Option<i64>>(9)?.map(|n| n as u32),
                slot: r.get(10)?,
                label: r.get(11)?,
                program_id: r.get(12)?,
                workout_id: r.get(13)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<String, usize> = out.iter().enumerate().map(|(n, s)| (s.id.clone(), n)).collect();
    let mut st = c.prepare(
        "SELECT session_id, set_id, idx, exercise_id, reps, load, duration_s, distance_m, kind, rir, partials, rest_taken_s, recommendation_id
         FROM session_sets WHERE (?1 IS NULL OR session_id = ?1)",
    )?;
    for row in st.query_map([id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            SetLog {
                index: r.get::<_, i64>(2)? as u32,
                exercise_id: r.get(3)?,
                reps: r.get::<_, Option<i64>>(4)?.map(|n| n as u32),
                load: r.get(5)?,
                duration_s: r.get::<_, Option<i64>>(6)?.map(|n| n as u32),
                distance_m: r.get(7)?,
                kind: r.get::<_, Option<String>>(8)?.map(|k| k.parse::<SetKind>().unwrap()),
                rir: r.get::<_, Option<i64>>(9)?.map(|n| n as u8),
                partials: r.get::<_, Option<i64>>(10)?.map(|n| n as u8),
                rest_taken_s: r.get::<_, Option<i64>>(11)?.map(|n| n as u32),
                recommendation_id: r.get(12)?,
            },
        ))
    })? {
        let (s, set_id, x) = row?;
        if let Some(&n) = index.get(&s) {
            out[n].sets.insert(set_id, x);
        }
    }
    Ok(out)
}

// ---- tests and external activities ----

fn save_test(c: &Connection, t: &TestResult) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO tests (id, user_id, kind, date, beep_level, beep_shuttle, value, exercise_id, deleted) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET user_id = ?2, kind = ?3, date = ?4, beep_level = ?5, beep_shuttle = ?6, value = ?7, exercise_id = ?8, deleted = ?9",
        params![t.id, t.user_id, t.kind.as_str(), t.date, t.beep.map(|b| i64::from(b.level)), t.beep.map(|b| i64::from(b.shuttle)), t.value, t.exercise_id, b(t.deleted)],
    )
    .map(|_| ())
}

fn load_tests(c: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<TestResult>> {
    let mut st = c.prepare("SELECT id, user_id, kind, date, beep_level, beep_shuttle, value, exercise_id, deleted FROM tests WHERE (?1 IS NULL OR id = ?1) ORDER BY id")?;
    let rows = st.query_map([id], |r| {
        let (l, s): (Option<i64>, Option<i64>) = (r.get(4)?, r.get(5)?);
        Ok(TestResult {
            id: r.get(0)?,
            user_id: r.get(1)?,
            kind: r.get::<_, String>(2)?.parse::<TestKind>().unwrap(),
            date: r.get(3)?,
            beep: l.zip(s).map(|(level, shuttle)| BeepScore { level: level as u8, shuttle: shuttle as u8 }),
            value: r.get(6)?,
            exercise_id: r.get(7)?,
            deleted: r.get::<_, i64>(8)? != 0,
        })
    })?;
    rows.collect()
}

fn save_external(c: &Connection, a: &ExternalActivity) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO external_activities (source, source_id, user_id, started_at, duration_s, distance_m, linked_session_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (source, source_id) DO UPDATE SET user_id = ?3, started_at = ?4, duration_s = ?5, distance_m = ?6, linked_session_id = ?7",
        params![a.source, a.source_id, a.user_id, a.started_at, i64::from(a.duration_s), a.distance_m, a.linked_session_id],
    )
    .map(|_| ())
}

fn load_external(c: &Connection, source: &str, source_id: &str) -> rusqlite::Result<Option<ExternalActivity>> {
    c.query_row(
        "SELECT source, source_id, user_id, started_at, duration_s, distance_m, linked_session_id FROM external_activities WHERE source = ?1 AND source_id = ?2",
        [source, source_id],
        |r| {
            Ok(ExternalActivity {
                source: r.get(0)?,
                source_id: r.get(1)?,
                user_id: r.get(2)?,
                started_at: r.get(3)?,
                duration_s: r.get::<_, i64>(4)? as u32,
                distance_m: r.get(5)?,
                linked_session_id: r.get(6)?,
            })
        },
    )
    .optional()
}

// ---- the store ----

impl SqliteFitnessStore {
    /// Run `f` in a transaction.
    fn tx<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> R<T> {
        self.db
            .run(|c| {
                let tx = c.transaction()?;
                let out = f(&tx)?;
                tx.commit()?;
                Ok(out)
            })
            .map_err(db)
    }

    fn read<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> R<T> {
        self.db.run(|c| f(c)).map_err(db)
    }

    /// Create-or-identical, like the Automerge store's `create`.
    fn create<T: PartialEq>(&self, existing: Option<T>, value: &T, id: &str, save: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> R<Write> {
        match existing {
            Some(e) if e == *value => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(id.into())),
            None => self.tx(save).map(|_| Write::Created),
        }
    }

    fn one<T>(v: Vec<T>, id: &str) -> R<T> {
        v.into_iter().next().ok_or_else(|| Error::NotFound(id.into()))
    }

    fn owned_session(&self, user_id: &str, session_id: &str) -> R<Session> {
        let s = self.get_session(session_id)?;
        if s.user_id != user_id {
            return Err(Error::NotOwner);
        }
        Ok(s)
    }

    fn check_program(&self, p: &Program) -> R<()> {
        validate_program(p)?;
        if let Some(g) = &p.gym_profile_id {
            if self.get_gym_profile(g)?.user_id != p.user_id {
                return Err(Error::NotOwner);
            }
        }
        for it in p.workouts.values().flat_map(|w| w.items.values()) {
            self.get_exercise(&it.exercise_id)?;
        }
        Ok(())
    }

    fn clear_other_defaults(&self, keep: &GymProfile) -> R<()> {
        for other in self.list_gym_profiles(&keep.user_id)? {
            if other.id != keep.id && other.is_default {
                let g = GymProfile { is_default: false, ..other };
                self.tx(|c| save_gym(c, &g))?;
            }
        }
        Ok(())
    }
}

impl FitnessStore for SqliteFitnessStore {
    fn list_exercises(&self) -> R<Vec<Exercise>> {
        let mut v: Vec<_> = self.read(|c| load_exercises(c, None))?.into_iter().filter(|e| !e.deleted).collect();
        v.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
        Ok(v)
    }

    fn get_exercise(&self, id: &str) -> R<Exercise> {
        Self::one(self.read(|c| load_exercises(c, Some(id)))?, id)
    }

    fn put_exercise(&mut self, e: Exercise) -> R<Write> {
        validate_exercise(&e)?;
        let existing = self.read(|c| load_exercises(c, Some(&e.id)))?.into_iter().next();
        self.create(existing, &e, &e.id.clone(), |c| save_exercise(c, &e))
    }

    fn update_exercise(&mut self, e: Exercise) -> R<()> {
        validate_exercise(&e)?;
        self.get_exercise(&e.id)?;
        self.tx(|c| save_exercise(c, &e))
    }

    fn list_templates(&self) -> R<Vec<Template>> {
        let mut v: Vec<_> = self.read(|c| load_templates(c, None))?.into_iter().filter(|t| !t.deleted).collect();
        v.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
        Ok(v)
    }

    fn get_template(&self, id: &str) -> R<Template> {
        Self::one(self.read(|c| load_templates(c, Some(id)))?, id)
    }

    fn put_template(&mut self, t: Template) -> R<Write> {
        validate_template(&t)?;
        match self.read(|c| load_templates(c, Some(&t.id)))?.into_iter().next() {
            Some(e) if same_template(&e, &t) => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(t.id)),
            None => self.tx(|c| save_template(c, &t)).map(|_| Write::Created),
        }
    }

    fn update_template(&mut self, t: Template) -> R<()> {
        validate_template(&t)?;
        self.get_template(&t.id)?;
        self.tx(|c| save_template(c, &t))
    }

    fn get_plan(&self, user_id: &str) -> R<Option<WeeklyPlan>> {
        Ok(self.read(|c| load_plans(c, Some(user_id)))?.into_iter().next())
    }

    fn list_plans(&self) -> R<Vec<WeeklyPlan>> {
        self.read(|c| load_plans(c, None))
    }

    fn set_plan(&mut self, plan: WeeklyPlan) -> R<Write> {
        for (key, slot) in &plan.slots {
            if !SLOT_KEYS.contains(&key.as_str()) {
                return Err(Error::Invalid(format!("plan slot {key:?}, expected mon_am … sun_pm")));
            }
            if slot.template_id.is_some() == slot.external_label.is_some() {
                return Err(Error::Invalid(format!("plan slot {key}: set exactly one of template_id or external_label")));
            }
            if let Some(t) = &slot.template_id {
                self.get_template(t)?;
            }
        }
        let existing = self.get_plan(&plan.user_id)?;
        if existing.as_ref() == Some(&plan) {
            return Ok(Write::Unchanged);
        }
        self.tx(|c| save_plan(c, &plan))?;
        Ok(if existing.is_some() { Write::Updated } else { Write::Created })
    }

    fn list_blocks(&self) -> R<Vec<Block>> {
        let mut v: Vec<_> = self.read(|c| load_blocks(c, None))?.into_iter().filter(|b| !b.deleted).collect();
        v.sort_by(|a, b| (&a.start_date, &a.id).cmp(&(&b.start_date, &b.id)));
        Ok(v)
    }

    fn get_block(&self, id: &str) -> R<Block> {
        Self::one(self.read(|c| load_blocks(c, Some(id)))?, id)
    }

    fn put_block(&mut self, x: Block) -> R<Write> {
        validate_block(&x)?;
        let existing = self.read(|c| load_blocks(c, Some(&x.id)))?.into_iter().next();
        self.create(existing, &x, &x.id.clone(), |c| save_block(c, &x))
    }

    fn update_block(&mut self, x: Block) -> R<()> {
        validate_block(&x)?;
        self.get_block(&x.id)?;
        self.tx(|c| save_block(c, &x))
    }

    fn list_sessions(&self) -> R<Vec<Session>> {
        let mut v: Vec<_> = self.read(|c| load_sessions(c, None))?.into_iter().filter(|s| !s.deleted).collect();
        v.sort_by_key(|s| Reverse((s.started_at, s.id.clone())));
        Ok(v)
    }

    fn get_session(&self, id: &str) -> R<Session> {
        Self::one(self.read(|c| load_sessions(c, Some(id)))?, id)
    }

    fn put_session(&mut self, s: Session) -> R<Write> {
        if let Some(t) = &s.template_id {
            self.get_template(t)?;
        }
        if let Some(bl) = &s.block_id {
            if self.get_block(bl)?.user_id != s.user_id {
                return Err(Error::NotOwner);
            }
        }
        if let Some(p) = &s.program_id {
            if self.get_program(p)?.user_id != s.user_id {
                return Err(Error::NotOwner);
            }
        }
        let key = |s: &Session| (s.user_id.clone(), s.template_id.clone(), s.block_id.clone(), s.block_week, s.slot.clone(), s.label.clone(), s.program_id.clone(), s.workout_id.clone());
        match self.read(|c| load_sessions(c, Some(&s.id)))?.into_iter().next() {
            Some(e) if key(&e) == key(&s) => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(s.id)),
            None => self.tx(|c| save_session(c, &s)).map(|_| Write::Created),
        }
    }

    fn put_set(&mut self, user_id: &str, session_id: &str, set_id: &str, mut set: SetLog) -> R<Write> {
        let s = self.owned_session(user_id, session_id)?;
        if let Some(existing) = s.sets.get(set_id) {
            return if existing.same(&set) { Ok(Write::Unchanged) } else { Err(Error::Conflict(set_id.into())) };
        }
        set.index = s.sets.values().map(|x| x.index + 1).max().unwrap_or(0);
        self.tx(|c| insert_set(c, session_id, set_id, &set))?;
        Ok(Write::Created)
    }

    fn finish_session(&mut self, user_id: &str, session_id: &str, status: SessionStatus, note: Option<&str>) -> R<()> {
        if !matches!(status, SessionStatus::Completed | SessionStatus::Partial) {
            return Err(Error::Invalid("finish status, expected Completed or Partial".into()));
        }
        let mut s = self.owned_session(user_id, session_id)?;
        if s.status == status && s.note.as_deref() == note && s.finished_at.is_some() {
            return Ok(());
        }
        s.status = status;
        s.note = note.map(Into::into);
        s.finished_at = Some(now_ms());
        self.tx(|c| c.execute("UPDATE sessions SET status = ?2, note = ?3, finished_at = ?4 WHERE id = ?1", params![s.id, s.status.as_str(), s.note, s.finished_at]).map(|_| ()))
    }

    fn list_tests(&self, kind: &TestKind) -> R<Vec<TestResult>> {
        let mut v: Vec<_> = self.read(|c| load_tests(c, None))?.into_iter().filter(|t| !t.deleted && &t.kind == kind).collect();
        let val = |t: &TestResult| t.value.unwrap_or(0.0);
        match kind {
            TestKind::BeepTest => v.sort_by_key(|t| (Reverse(t.beep), t.id.clone())),
            TestKind::TimedRun => v.sort_by(|a, b| val(a).total_cmp(&val(b)).then(a.id.cmp(&b.id))),
            _ => v.sort_by(|a, b| val(b).total_cmp(&val(a)).then(a.id.cmp(&b.id))),
        }
        Ok(v)
    }

    fn put_test(&mut self, mut r: TestResult) -> R<Write> {
        validate_test(&mut r)?;
        r.deleted = false;
        let existing = self.read(|c| load_tests(c, Some(&r.id)))?.into_iter().next();
        self.create(existing, &r, &r.id.clone(), |c| save_test(c, &r))
    }

    fn upsert_external(&mut self, a: ExternalActivity) -> R<Write> {
        if a.source.is_empty() || a.source.contains(':') || a.source_id.is_empty() {
            return Err(Error::Invalid("external activity: source (no ':') and source_id are required".into()));
        }
        match self.read(|c| load_external(c, &a.source, &a.source_id))? {
            Some(e) if e == a => Ok(Write::Unchanged),
            Some(e) if e.user_id != a.user_id => Err(Error::NotOwner),
            existing => {
                self.tx(|c| save_external(c, &a))?;
                Ok(if existing.is_some() { Write::Updated } else { Write::Created })
            }
        }
    }

    fn list_gym_profiles(&self, user_id: &str) -> R<Vec<GymProfile>> {
        let mut v: Vec<_> = self.read(|c| load_gyms(c, None))?.into_iter().filter(|g| !g.deleted && g.user_id == user_id).collect();
        v.sort_by(|a, b| (!a.is_default, a.name.to_lowercase(), &a.id).cmp(&(!b.is_default, b.name.to_lowercase(), &b.id)));
        Ok(v)
    }

    fn get_gym_profile(&self, id: &str) -> R<GymProfile> {
        Self::one(self.read(|c| load_gyms(c, Some(id)))?, id)
    }

    fn put_gym_profile(&mut self, g: GymProfile) -> R<Write> {
        validate_gym(&g)?;
        let existing = self.read(|c| load_gyms(c, Some(&g.id)))?.into_iter().next();
        let w = self.create(existing, &g, &g.id.clone(), |c| save_gym(c, &g))?;
        if w == Write::Created && g.is_default {
            self.clear_other_defaults(&g)?;
        }
        Ok(w)
    }

    fn update_gym_profile(&mut self, g: GymProfile) -> R<()> {
        validate_gym(&g)?;
        self.get_gym_profile(&g.id)?;
        self.tx(|c| save_gym(c, &g))?;
        if g.is_default {
            self.clear_other_defaults(&g)?;
        }
        Ok(())
    }

    fn list_programs(&self) -> R<Vec<Program>> {
        let mut v: Vec<_> = self.read(|c| load_programs(c, None))?.into_iter().filter(|p| !p.deleted).collect();
        v.sort_by(|a, b| (&a.start_date, &a.id).cmp(&(&b.start_date, &b.id)));
        Ok(v)
    }

    fn get_program(&self, id: &str) -> R<Program> {
        Self::one(self.read(|c| load_programs(c, Some(id)))?, id)
    }

    fn put_program(&mut self, p: Program) -> R<Write> {
        self.check_program(&p)?;
        let existing = self.read(|c| load_programs(c, Some(&p.id)))?.into_iter().next();
        self.create(existing, &p, &p.id.clone(), |c| save_program(c, &p))
    }

    fn update_program(&mut self, p: Program) -> R<()> {
        self.check_program(&p)?;
        self.get_program(&p.id)?;
        self.tx(|c| save_program(c, &p))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
