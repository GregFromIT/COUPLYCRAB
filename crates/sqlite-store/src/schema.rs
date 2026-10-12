//! The schema, as numbered migrations (`PRAGMA user_version` is the number applied).
//! Only the Rust server writes this database (docs/LEARNING.md "single-writer rule").
//! Rules: IDs are UUID text, timestamps are Unix ms integers, enums are text with CHECK
//! constraints, soft delete is a `deleted` column. Derived data (recommendations, analytics,
//! autocomplete) has no tables.

use rusqlite_migration::{Migrations, M};

/// Migration 1: everything the Automerge documents held (todos, fitness, shopping).
const V1: &str = r#"
CREATE TABLE todos (
    id         TEXT PRIMARY KEY NOT NULL,
    title      TEXT NOT NULL,
    notes      TEXT NOT NULL DEFAULT '',
    due        TEXT,
    completed  INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    deleted    INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE exercises (
    id             TEXT PRIMARY KEY NOT NULL,
    name           TEXT NOT NULL,
    measurement    TEXT NOT NULL CHECK (measurement IN ('WeightReps', 'Reps', 'Timed', 'Distance')),
    tempo          TEXT,
    deleted        INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
    equipment      TEXT CHECK (equipment IS NULL OR equipment IN ('Barbell', 'Dumbbell', 'Machine', 'Cable', 'Bodyweight', 'Other')),
    default_rest_s INTEGER
) STRICT;

CREATE TABLE exercise_muscles (
    exercise_id TEXT NOT NULL REFERENCES exercises (id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('primary', 'secondary')),
    muscle      TEXT NOT NULL CHECK (muscle IN ('chest', 'back', 'shoulders', 'biceps', 'triceps', 'quads', 'hamstrings', 'glutes', 'calves', 'abs', 'forearms')),
    position    INTEGER NOT NULL,
    PRIMARY KEY (exercise_id, role, muscle)
) STRICT;

CREATE TABLE templates (
    id             TEXT PRIMARY KEY NOT NULL,
    name           TEXT NOT NULL,
    target_minutes INTEGER NOT NULL,
    deleted        INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1))
) STRICT;

CREATE TABLE template_items (
    template_id   TEXT NOT NULL REFERENCES templates (id) ON DELETE CASCADE,
    item_id       TEXT NOT NULL,
    idx           INTEGER NOT NULL,
    exercise_id   TEXT NOT NULL REFERENCES exercises (id),
    target_sets   INTEGER NOT NULL,
    target_reps   INTEGER,
    target_load   REAL,
    tempo         TEXT,
    rest_s        INTEGER,
    prog_kind     TEXT CHECK (prog_kind IS NULL OR prog_kind IN ('LinearLoad', 'LinearReps')),
    prog_per_week REAL,
    PRIMARY KEY (template_id, item_id)
) STRICT;

CREATE TABLE plans (user_id TEXT PRIMARY KEY NOT NULL) STRICT;

CREATE TABLE plan_slots (
    user_id        TEXT NOT NULL REFERENCES plans (user_id) ON DELETE CASCADE,
    slot           TEXT NOT NULL CHECK (slot IN ('mon_am', 'mon_pm', 'tue_am', 'tue_pm', 'wed_am', 'wed_pm', 'thu_am', 'thu_pm', 'fri_am', 'fri_pm', 'sat_am', 'sat_pm', 'sun_am', 'sun_pm')),
    template_id    TEXT REFERENCES templates (id),
    external_label TEXT,
    CHECK ((template_id IS NULL) <> (external_label IS NULL)),
    PRIMARY KEY (user_id, slot)
) STRICT;

CREATE TABLE blocks (
    id         TEXT PRIMARY KEY NOT NULL,
    user_id    TEXT NOT NULL,
    name       TEXT NOT NULL,
    start_date TEXT NOT NULL,
    weeks      INTEGER NOT NULL CHECK (weeks BETWEEN 1 AND 104),
    deload_week INTEGER,
    deload_pct REAL,
    deleted    INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
    CHECK ((deload_week IS NULL) = (deload_pct IS NULL))
) STRICT;

CREATE TABLE gym_profiles (
    id               TEXT PRIMARY KEY NOT NULL,
    user_id          TEXT NOT NULL,
    name             TEXT NOT NULL,
    bar_kg           REAL NOT NULL CHECK (bar_kg > 0),
    dumbbell_step_kg REAL NOT NULL CHECK (dumbbell_step_kg > 0),
    machine_step_kg  REAL NOT NULL CHECK (machine_step_kg > 0),
    is_default       INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    deleted          INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1))
) STRICT;

CREATE TABLE gym_plates (
    gym_id   TEXT NOT NULL REFERENCES gym_profiles (id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    plate_kg REAL NOT NULL CHECK (plate_kg > 0),
    PRIMARY KEY (gym_id, position)
) STRICT;

CREATE TABLE programs (
    id             TEXT PRIMARY KEY NOT NULL,
    user_id        TEXT NOT NULL,
    name           TEXT NOT NULL,
    start_date     TEXT NOT NULL,
    weeks          INTEGER NOT NULL CHECK (weeks BETWEEN 1 AND 104),
    gym_profile_id TEXT REFERENCES gym_profiles (id),
    warmups        INTEGER NOT NULL CHECK (warmups IN (0, 1)),
    migrated_from  TEXT,
    deleted        INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1))
) STRICT;

CREATE TABLE program_workouts (
    program_id     TEXT NOT NULL REFERENCES programs (id) ON DELETE CASCADE,
    workout_id     TEXT NOT NULL,
    idx            INTEGER NOT NULL,
    name           TEXT NOT NULL,
    slot           TEXT CHECK (slot IS NULL OR slot IN ('mon_am', 'mon_pm', 'tue_am', 'tue_pm', 'wed_am', 'wed_pm', 'thu_am', 'thu_pm', 'fri_am', 'fri_pm', 'sat_am', 'sat_pm', 'sun_am', 'sun_pm')),
    external_label TEXT,
    PRIMARY KEY (program_id, workout_id)
) STRICT;

CREATE TABLE program_items (
    program_id  TEXT NOT NULL,
    item_id     TEXT NOT NULL,
    workout_id  TEXT NOT NULL,
    idx         INTEGER NOT NULL,
    exercise_id TEXT NOT NULL REFERENCES exercises (id),
    sets        INTEGER NOT NULL,
    rep_min     INTEGER NOT NULL,
    rep_max     INTEGER NOT NULL,
    rir         INTEGER NOT NULL CHECK (rir BETWEEN 0 AND 5),
    style       TEXT NOT NULL CHECK (style IN ('Straight', 'DropSet', 'ToFailure')),
    rest_s      INTEGER,
    PRIMARY KEY (program_id, item_id),
    FOREIGN KEY (program_id, workout_id) REFERENCES program_workouts (program_id, workout_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE program_overrides (
    program_id TEXT NOT NULL,
    week       INTEGER NOT NULL,
    item_id    TEXT NOT NULL,
    sets       INTEGER,
    rep_min    INTEGER,
    rep_max    INTEGER,
    rir        INTEGER,
    PRIMARY KEY (program_id, week, item_id),
    FOREIGN KEY (program_id, item_id) REFERENCES program_items (program_id, item_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY NOT NULL,
    user_id     TEXT NOT NULL,
    template_id TEXT REFERENCES templates (id),
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    status      TEXT NOT NULL CHECK (status IN ('InProgress', 'Completed', 'Partial')),
    note        TEXT,
    deleted     INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
    block_id    TEXT REFERENCES blocks (id),
    block_week  INTEGER,
    slot        TEXT,
    label       TEXT,
    program_id  TEXT REFERENCES programs (id),
    workout_id  TEXT
) STRICT;
CREATE INDEX sessions_by_user_time ON sessions (user_id, started_at);

CREATE TABLE session_sets (
    session_id        TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    set_id            TEXT NOT NULL,
    idx               INTEGER NOT NULL,
    exercise_id       TEXT NOT NULL REFERENCES exercises (id),
    reps              INTEGER,
    load              REAL,
    duration_s        INTEGER,
    distance_m        REAL,
    kind              TEXT CHECK (kind IS NULL OR kind IN ('Warmup', 'Working', 'Drop', 'Failure')),
    rir               INTEGER,
    partials          INTEGER,
    rest_taken_s      INTEGER,
    recommendation_id TEXT,
    PRIMARY KEY (session_id, set_id)
) STRICT;

CREATE TABLE tests (
    id           TEXT PRIMARY KEY NOT NULL,
    user_id      TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('BeepTest', 'OneRepMax', 'MaxReps', 'TimedRun')),
    date         TEXT NOT NULL,
    beep_level   INTEGER,
    beep_shuttle INTEGER,
    value        REAL,
    exercise_id  TEXT REFERENCES exercises (id),
    deleted      INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
    CHECK ((beep_level IS NULL) = (beep_shuttle IS NULL))
) STRICT;

CREATE TABLE external_activities (
    source            TEXT NOT NULL,
    source_id         TEXT NOT NULL,
    user_id           TEXT NOT NULL,
    started_at        INTEGER NOT NULL,
    duration_s        INTEGER NOT NULL,
    distance_m        REAL,
    linked_session_id TEXT,
    PRIMARY KEY (source, source_id)
) STRICT;

CREATE TABLE shopping_lists (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    deleted    INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1))
) STRICT;

CREATE TABLE shopping_items (
    id         TEXT PRIMARY KEY NOT NULL,
    list_id    TEXT NOT NULL REFERENCES shopping_lists (id),
    name       TEXT NOT NULL,
    quantity   TEXT,
    note       TEXT,
    category   TEXT CHECK (category IS NULL OR category IN ('produce', 'meat', 'dairy', 'bakery', 'pantry', 'frozen', 'household', 'other')),
    checked_by TEXT,
    checked_at INTEGER,
    -- "Clear checked" hides a ticked item; unticking or ticking again shows it again.
    hidden     INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1)),
    added_by   TEXT NOT NULL,
    added_at   INTEGER NOT NULL,
    deleted    INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
    CHECK ((checked_by IS NULL) = (checked_at IS NULL))
) STRICT;
CREATE INDEX shopping_items_by_list ON shopping_items (list_id);
"#;

/// Every migration, in order. Append only; never edit one that has shipped.
const ALL: &[&str] = &[V1];

pub const MIGRATION_COUNT: usize = ALL.len();

pub fn migrations() -> Migrations<'static> {
    Migrations::new(ALL.iter().map(|s| M::up(s)).collect())
}
