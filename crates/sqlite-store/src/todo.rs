//! `TodoStore` on SQLite.

use crate::{b, Db};
use rusqlite::{params, OptionalExtension, Row};
use todo_core::{valid_date, Error, Result, Todo, TodoStore};

fn db<E: std::fmt::Display>(e: E) -> Error {
    Error::Storage(e.to_string())
}

pub struct SqliteTodoStore {
    db: Db,
}

impl SqliteTodoStore {
    pub fn new(db: Db) -> Self {
        SqliteTodoStore { db }
    }

    /// Insert or replace a whole todo, keeping its timestamps (for the migration).
    pub fn import(&self, t: &Todo) -> Result<()> {
        write(&self.db, t)
    }

    pub fn count(&self) -> Result<usize> {
        self.db.with(|c| c.query_row("SELECT COUNT(*) FROM todos", [], |r| r.get::<_, i64>(0))).map(|n| n as usize).map_err(db)
    }
}

fn row(r: &Row) -> rusqlite::Result<Todo> {
    Ok(Todo {
        id: r.get("id")?,
        title: r.get("title")?,
        notes: r.get("notes")?,
        due: r.get("due")?,
        completed: r.get::<_, i64>("completed")? != 0,
        deleted: r.get::<_, i64>("deleted")? != 0,
        created_at: r.get("created_at")?,
    })
}

fn find(d: &Db, id: &str) -> Result<Option<Todo>> {
    d.with(|c| c.query_row("SELECT * FROM todos WHERE id = ?1", [id], row).optional()).map_err(db)
}

fn write(d: &Db, t: &Todo) -> Result<()> {
    d.with(|c| {
        c.execute(
            "INSERT INTO todos (id, title, notes, due, completed, deleted, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET title = ?2, notes = ?3, due = ?4, completed = ?5, deleted = ?6, created_at = ?7",
            params![t.id, t.title, t.notes, t.due, b(t.completed), b(t.deleted), t.created_at],
        )
        .map(|_| ())
    })
    .map_err(db)
}

fn check_due(due: Option<&str>) -> Result<()> {
    match due {
        Some(d) if !valid_date(d) => Err(Error::Invalid(format!("due date {d:?}, expected YYYY-MM-DD"))),
        _ => Ok(()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl SqliteTodoStore {
    fn modify(&mut self, id: &str, f: impl FnOnce(&mut Todo)) -> Result<()> {
        let mut t = find(&self.db, id)?.ok_or_else(|| Error::NotFound(id.into()))?;
        f(&mut t);
        write(&self.db, &t)
    }
}

impl TodoStore for SqliteTodoStore {
    fn add(&mut self, title: &str, notes: &str, due: Option<&str>) -> Result<Todo> {
        let id = uuid::Uuid::now_v7().to_string();
        self.put(&id, title, notes, due)?;
        self.get(&id)
    }

    fn put(&mut self, id: &str, title: &str, notes: &str, due: Option<&str>) -> Result<bool> {
        check_due(due)?;
        match find(&self.db, id)? {
            Some(t) if (t.title.as_str(), t.notes.as_str(), t.due.as_deref()) == (title, notes, due) => Ok(false),
            Some(_) => Err(Error::Conflict(id.into())),
            None => {
                let todo = Todo { id: id.into(), title: title.into(), notes: notes.into(), due: due.map(Into::into), completed: false, deleted: false, created_at: now_ms() };
                write(&self.db, &todo)?;
                Ok(true)
            }
        }
    }

    fn get(&self, id: &str) -> Result<Todo> {
        find(&self.db, id)?.ok_or_else(|| Error::NotFound(id.into()))
    }

    fn edit(&mut self, id: &str, title: &str, notes: &str, due: Option<&str>) -> Result<()> {
        check_due(due)?;
        self.modify(id, |t| {
            t.title = title.into();
            t.notes = notes.into();
            t.due = due.map(Into::into);
        })
    }

    fn set_completed(&mut self, id: &str, completed: bool) -> Result<()> {
        self.modify(id, |t| t.completed = completed)
    }

    fn delete(&mut self, id: &str) -> Result<()> {
        self.modify(id, |t| t.deleted = true)
    }

    fn list(&self) -> Result<Vec<Todo>> {
        // Dated ascending (so overdue come first), then undated; IDs (UUID v7) break ties by creation time.
        self.db
            .with(|c| {
                let mut st = c.prepare("SELECT * FROM todos WHERE deleted = 0 ORDER BY due IS NULL, due, id")?;
                let rows = st.query_map([], row)?;
                rows.collect()
            })
            .map_err(db)
    }
}
