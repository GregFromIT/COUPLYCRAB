//! Todos stored in an Automerge document.
//!
//! Doc shape: `{ schema_version: u32, todos: { <uuid>: Todo } }`.
//! Collections are maps keyed by ID; order is computed in [`TodoStore::list`].

use automerge::{transaction::CommitOptions, ActorId, AutomergeError, ObjType, ReadDoc, ROOT};
use autosurgeon::{hydrate, hydrate_prop, reconcile_prop, Hydrate, Reconcile};
use std::collections::HashMap;

pub use automerge::Automerge;

pub const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct Todo {
    pub id: String,
    pub title: String,
    pub notes: String,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    pub completed: bool,
    /// Soft delete: see docs/LEARNING.md "Soft delete".
    pub deleted: bool,
    /// Unix ms UTC.
    pub created_at: i64,
}

#[derive(Debug, Hydrate)]
struct TodoDoc {
    todos: HashMap<String, Todo>,
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Automerge(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "todo {id} not found"),
            Error::Automerge(e) => write!(f, "automerge: {e}"),
        }
    }
}
impl std::error::Error for Error {}

macro_rules! from_err {
    ($($t:ty),*) => {$(impl From<$t> for Error { fn from(e: $t) -> Self { Error::Automerge(e.to_string()) } })*};
}
from_err!(AutomergeError, autosurgeon::HydrateError, autosurgeon::ReconcileError);

pub type Result<T> = std::result::Result<T, Error>;

/// Domain operations. The seam if the CRDT choice changes.
pub trait TodoStore {
    fn add(&mut self, title: &str, notes: &str, due: Option<&str>) -> Result<Todo>;
    fn edit(&mut self, id: &str, title: &str, notes: &str, due: Option<&str>) -> Result<()>;
    fn set_completed(&mut self, id: &str, completed: bool) -> Result<()>;
    fn delete(&mut self, id: &str) -> Result<()>;
    /// Non-deleted todos: dated ascending (so overdue come first), then undated.
    fn list(&self) -> Result<Vec<Todo>>;
}

pub struct AmTodoStore {
    pub doc: Automerge,
}

impl AsMut<Automerge> for AmTodoStore {
    fn as_mut(&mut self) -> &mut Automerge {
        &mut self.doc
    }
}

/// Every device creates the same initial change (fixed actor + time), so the
/// `todos` map has the same object ID everywhere. Without this, two fresh devices
/// would each create their own `todos` map and one would hide the other on merge.
pub fn init_doc() -> Automerge {
    let mut doc = Automerge::new().with_actor(ActorId::from([0u8; 16]));
    doc.transact_with::<_, _, AutomergeError, _>(
        |_| CommitOptions::default().with_time(0),
        |tx| {
            use automerge::transaction::Transactable;
            tx.put(ROOT, "schema_version", SCHEMA_VERSION)?;
            tx.put_object(ROOT, "todos", ObjType::Map)?;
            Ok(())
        },
    )
    .expect("init on empty doc cannot fail");
    doc.set_actor(ActorId::random());
    doc
}

impl AmTodoStore {
    pub fn new() -> Self {
        Self { doc: init_doc() }
    }

    /// Wrap a loaded doc. An empty doc (new file) gets the shared init change merged in.
    pub fn from_doc(mut doc: Automerge) -> Result<Self> {
        doc.merge(&mut init_doc())?;
        Ok(Self { doc })
    }

    fn todos_obj(&self) -> Result<automerge::ObjId> {
        match self.doc.get(ROOT, "todos")? {
            Some((_, id)) => Ok(id),
            None => Err(Error::Automerge("missing todos map".into())),
        }
    }

    fn get(&self, id: &str) -> Result<Todo> {
        let obj = self.todos_obj()?;
        if self.doc.get(&obj, id)?.is_none() {
            return Err(Error::NotFound(id.into()));
        }
        Ok(hydrate_prop(&self.doc, &obj, id)?)
    }

    fn put(&mut self, todo: &Todo) -> Result<()> {
        let obj = self.todos_obj()?;
        self.doc
            .transact::<_, _, autosurgeon::ReconcileError>(|tx| reconcile_prop(tx, &obj, todo.id.as_str(), todo))
            .map_err(|f| f.error)?;
        Ok(())
    }

    fn modify(&mut self, id: &str, f: impl FnOnce(&mut Todo)) -> Result<()> {
        let mut t = self.get(id)?;
        f(&mut t);
        self.put(&t)
    }
}

impl Default for AmTodoStore {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl TodoStore for AmTodoStore {
    fn add(&mut self, title: &str, notes: &str, due: Option<&str>) -> Result<Todo> {
        let todo = Todo {
            id: uuid::Uuid::now_v7().to_string(),
            title: title.into(),
            notes: notes.into(),
            due: due.map(Into::into),
            completed: false,
            deleted: false,
            created_at: now_ms(),
        };
        self.put(&todo)?;
        Ok(todo)
    }

    fn edit(&mut self, id: &str, title: &str, notes: &str, due: Option<&str>) -> Result<()> {
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
        let d: TodoDoc = hydrate(&self.doc)?;
        let mut v: Vec<Todo> = d.todos.into_values().filter(|t| !t.deleted).collect();
        // None sorts after Some via the bool; UUID v7 ids break ties by creation time.
        v.sort_by(|a, b| (a.due.is_none(), &a.due, &a.id).cmp(&(b.due.is_none(), &b.due, &b.id)));
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync_pair(a: &mut AmTodoStore, b: &mut AmTodoStore) {
        a.doc.merge(&mut b.doc).unwrap();
        b.doc.merge(&mut a.doc).unwrap();
    }

    /// Same change set and same hydrated state. (`save()` bytes can differ in
    /// actor-table order, so heads + hydrated structs are the comparison.)
    fn assert_converged(a: &AmTodoStore, b: &AmTodoStore) {
        let heads = |d: &Automerge| { let mut h = d.get_heads(); h.sort(); h };
        assert_eq!(heads(&a.doc), heads(&b.doc));
        assert_eq!(a.list().unwrap(), b.list().unwrap());
    }

    #[test]
    fn crud() {
        let mut s = AmTodoStore::new();
        let t = s.add("milk", "", None).unwrap();
        s.edit(&t.id, "oat milk", "2L", Some("2026-01-02")).unwrap();
        s.set_completed(&t.id, true).unwrap();
        let got = &s.list().unwrap()[0];
        assert_eq!((got.title.as_str(), got.notes.as_str(), got.due.as_deref(), got.completed),
                   ("oat milk", "2L", Some("2026-01-02"), true));
        s.set_completed(&t.id, false).unwrap();
        assert!(!s.list().unwrap()[0].completed);
        s.delete(&t.id).unwrap();
        assert!(s.list().unwrap().is_empty());
        assert!(matches!(s.edit("nope", "", "", None), Err(Error::NotFound(_))));
    }

    #[test]
    fn sort_order() {
        let mut s = AmTodoStore::new();
        s.add("undated", "", None).unwrap();
        s.add("later", "", Some("2030-01-01")).unwrap();
        s.add("overdue", "", Some("2000-01-01")).unwrap();
        s.add("soon", "", Some("2026-11-01")).unwrap();
        let titles: Vec<_> = s.list().unwrap().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["overdue", "soon", "later", "undated"]);
    }

    #[test]
    fn save_load_roundtrip() {
        let mut s = AmTodoStore::new();
        s.add("x", "", None).unwrap();
        let loaded = AmTodoStore::from_doc(Automerge::load(&s.doc.save()).unwrap()).unwrap();
        assert_eq!(loaded.list().unwrap(), s.list().unwrap());
    }

    #[test]
    fn fresh_devices_converge() {
        // Two devices that never saw each other each create a doc and a todo.
        let mut a = AmTodoStore::new();
        let mut b = AmTodoStore::from_doc(Automerge::new()).unwrap();
        a.add("from a", "", None).unwrap();
        b.add("from b", "", None).unwrap();
        sync_pair(&mut a, &mut b);
        assert_eq!(a.list().unwrap().len(), 2);
        assert_converged(&a, &b);
    }

    #[test]
    fn concurrent_different_fields_keep_both() {
        let mut a = AmTodoStore::new();
        let t = a.add("t", "", None).unwrap();
        let mut b = AmTodoStore::from_doc(a.doc.fork()).unwrap();
        a.edit(&t.id, "new title", "", None).unwrap();
        b.edit(&t.id, "t", "new notes", None).unwrap();
        sync_pair(&mut a, &mut b);
        let la = a.list().unwrap();
        assert_converged(&a, &b);
        assert_eq!((la[0].title.as_str(), la[0].notes.as_str()), ("new title", "new notes"));
    }

    #[test]
    fn concurrent_title_edit_is_deterministic() {
        let mut a = AmTodoStore::new();
        let t = a.add("t", "", None).unwrap();
        let mut b = AmTodoStore::from_doc(a.doc.fork()).unwrap();
        a.edit(&t.id, "A", "", None).unwrap();
        b.edit(&t.id, "B", "", None).unwrap();
        // Merge in opposite orders: winner must be the same either way.
        let (mut a2, mut b2) = (a.doc.fork(), b.doc.fork());
        a2.merge(&mut b.doc).unwrap();
        b2.merge(&mut a.doc).unwrap();
        let (sa, sb) = (AmTodoStore { doc: a2 }, AmTodoStore { doc: b2 });
        assert_converged(&sa, &sb);
        assert!(["A", "B"].contains(&sa.list().unwrap()[0].title.as_str()));
    }

    #[test]
    fn soft_delete_with_concurrent_edit_is_recoverable() {
        let mut a = AmTodoStore::new();
        let t = a.add("t", "", None).unwrap();
        let mut b = AmTodoStore::from_doc(a.doc.fork()).unwrap();
        a.delete(&t.id).unwrap();
        b.edit(&t.id, "edited", "", None).unwrap();
        sync_pair(&mut a, &mut b);
        assert!(a.list().unwrap().is_empty());
        assert_converged(&a, &b);
        // The edit survived under the tombstone; undeleting brings it back.
        a.modify(&t.id, |t| t.deleted = false).unwrap();
        assert_eq!(a.list().unwrap()[0].title, "edited");
    }
}
