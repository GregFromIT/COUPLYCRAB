//! The household database: one SQLite file, written only by the Rust server.
//!
//! * [`Db`]: opens the file with WAL, foreign keys and a busy timeout, runs the migrations,
//!   and shares one connection behind a mutex (two users do not need a pool).
//! * [`SqliteTodoStore`], [`SqliteFitnessStore`], [`SqliteShoppingStore`]: the same traits as the
//!   Automerge stores (`todo_core::TodoStore` etc.), so the HTTP layer does not know which it has.
//! * [`migrate`]: `peer migrate-to-sqlite`, copying the Automerge documents in one transaction.
//!
//! Design and trade-offs: docs/DECISIONS.md "SQLite foundation (v3-M1)".

mod fitness;
pub mod migrate;
mod schema;
mod shopping;
mod todo;

pub use fitness::SqliteFitnessStore;
pub use shopping::SqliteShoppingStore;
pub use todo::SqliteTodoStore;

use rusqlite::Connection;
use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

/// Latest schema version (`PRAGMA user_version` after opening).
pub fn schema_version() -> usize {
    schema::MIGRATION_COUNT
}

/// A shared handle to the one connection.
///
/// Calls are short and local, and the callers (the HTTP handlers) are already synchronous around
/// the store traits, so this uses a plain mutex and no `spawn_blocking`; see DECISIONS for why that
/// departs from PLAN-v3 §1.2.
#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    /// Open (creating if needed) the database at `path` and bring it to the latest schema.
    pub fn open(path: &Path) -> Result<Db, String> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        Self::setup(Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?)
    }

    /// A private in-memory database (tests).
    pub fn open_in_memory() -> Result<Db, String> {
        Self::setup(Connection::open_in_memory().map_err(|e| e.to_string())?)
    }

    fn setup(mut conn: Connection) -> Result<Db, String> {
        // Per connection, every time: https://www.sqlite.org/wal.html, https://www.sqlite.org/foreignkeys.html
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_millis(5000)).map_err(|e| e.to_string())?;
        schema::migrations().to_latest(&mut conn).map_err(|e| format!("migrating the database: {e}"))?;
        Ok(Db(Arc::new(Mutex::new(conn))))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` on the connection, keeping the raw error (so a constraint failure can be told from a disk failure).
    pub fn run<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        f(&mut self.lock())
    }

    /// Run `f` on the connection (for other modules' stores and tests).
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        f(&mut self.lock()).map_err(|e| e.to_string())
    }

    /// `PRAGMA user_version`: the number of migrations applied.
    pub fn user_version(&self) -> Result<i64, String> {
        self.with(|c| c.pragma_query_value(None, "user_version", |r| r.get(0)))
    }

    /// A consistent copy of the live database in `out` (`VACUUM INTO`, https://www.sqlite.org/lang_vacuum.html#vacuuminto).
    /// `out` must not exist.
    pub fn backup_to(&self, out: &Path) -> Result<(), String> {
        if out.exists() {
            return Err(format!("{} already exists; refusing to overwrite a backup", out.display()));
        }
        if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let path = out.to_str().ok_or("backup path is not valid UTF-8")?.to_string();
        self.with(|c| c.execute("VACUUM INTO ?1", [&path]).map(|_| ()))
    }
}

/// A failed CHECK / FOREIGN KEY / UNIQUE: the caller sent something the schema does not allow.
pub(crate) fn is_constraint(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

/// `true` as 1, for the 0/1 columns.
pub(crate) fn b(x: bool) -> i64 {
    i64::from(x)
}

#[cfg(test)]
mod tests;
