//! Shopping lists stored in an Automerge document (own doc, same patterns as `todo-core`).
//!
//! Doc shape: `{ schema_version: u64, lists: { <id>: List }, items: { <id>: Item } }`.
//! Collections are maps keyed by ID; order is computed on read. Deletes are soft.
//!
//! **Concurrency.** Two people ticking the same item at once is the normal case in a shop,
//! so the tick is ONE register (`Item::check`, `"<user>|<unix ms>"`), not three fields:
//! concurrent ticks resolve to a single writer, so `checked_by` and `checked_at` always
//! belong together, and every peer converges on "checked". "Clear checked" does not delete
//! anything: it records which tick it cleared (`Item::cleared`), and an item is hidden only
//! while `check == cleared`. An item unchecked (or re-ticked) concurrently on another peer
//! therefore stays on the list. See docs/LEARNING.md.

use automerge::{transaction::CommitOptions, ActorId, AutomergeError, ObjType, ReadDoc, ROOT};
use autosurgeon::{hydrate_prop, reconcile_prop, Hydrate, Reconcile};
use std::{cmp::Reverse, collections::HashMap, fmt};

pub use automerge::Automerge;

pub const SCHEMA_VERSION: u64 = 1;

/// Fixed item categories, in the order a list shows them.
pub const CATEGORIES: [&str; 8] = ["produce", "meat", "dairy", "bakery", "pantry", "frozen", "household", "other"];

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct List {
    pub id: String,
    pub name: String,
    /// Unix ms UTC.
    pub created_at: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate, serde::Serialize)]
pub struct Item {
    pub id: String,
    pub list_id: String,
    pub name: String,
    /// Free text: "2", "500 g".
    #[autosurgeon(missing = "Default::default")]
    pub quantity: Option<String>,
    #[autosurgeon(missing = "Default::default")]
    pub note: Option<String>,
    /// One of [`CATEGORIES`].
    #[autosurgeon(missing = "Default::default")]
    pub category: Option<String>,
    /// `"<user>|<unix ms>"` while ticked, else `None`. A single register on purpose; see the module docs.
    #[autosurgeon(missing = "Default::default")]
    pub check: Option<String>,
    /// The `check` value that "clear checked" removed: the item is hidden while `check == cleared`.
    #[autosurgeon(missing = "Default::default")]
    pub cleared: Option<String>,
    pub added_by: String,
    /// Unix ms UTC.
    pub added_at: i64,
    pub deleted: bool,
}

impl Item {
    pub fn checked(&self) -> bool {
        self.check.is_some()
    }

    pub fn checked_by(&self) -> Option<&str> {
        self.check.as_deref()?.rsplit_once('|').map(|(user, _)| user)
    }

    /// Unix ms.
    pub fn checked_at(&self) -> Option<i64> {
        self.check.as_deref()?.rsplit_once('|')?.1.parse().ok()
    }

    /// On the list: not deleted, and not ticked-then-cleared.
    pub fn visible(&self) -> bool {
        !self.deleted && !(self.check.is_some() && self.check == self.cleared)
    }
}

/// An item as entered (the store adds who and when).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct NewItem {
    pub id: String,
    pub list_id: String,
    pub name: String,
    #[serde(default)]
    pub quantity: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

/// Fields to change; `None` leaves a field alone, `Some(None)` clears an optional one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemEdit {
    pub name: Option<String>,
    pub quantity: Option<Option<String>>,
    pub note: Option<Option<String>>,
    pub category: Option<Option<String>>,
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Invalid(String),
    /// A `put_*` with an existing ID but different content.
    Conflict(String),
    Automerge(String),
    /// The database failed (SQLite store).
    Storage(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "{id} not found"),
            Error::Invalid(s) => write!(f, "invalid {s}"),
            Error::Conflict(id) => write!(f, "{id} already exists with different content"),
            Error::Automerge(e) => write!(f, "automerge: {e}"),
            Error::Storage(e) => write!(f, "storage: {e}"),
        }
    }
}
impl std::error::Error for Error {}

macro_rules! from_err {
    ($($t:ty),*) => {$(impl From<$t> for Error { fn from(e: $t) -> Self { Error::Automerge(e.to_string()) } })*};
}
from_err!(AutomergeError, autosurgeon::HydrateError, autosurgeon::ReconcileError);

pub type Result<T> = std::result::Result<T, Error>;

/// Outcome of an idempotent create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    Created,
    /// Same content was already there; nothing written.
    Unchanged,
}

/// Domain operations. The seam if the CRDT choice changes.
pub trait ShoppingStore {
    /// Non-deleted lists by name.
    fn lists(&self) -> Result<Vec<List>>;
    fn get_list(&self, id: &str) -> Result<List>;
    /// Create with a caller-chosen ID: `Created`, `Unchanged` if it exists with this name, else `Conflict`.
    fn put_list(&mut self, id: &str, name: &str) -> Result<Write>;
    fn rename_list(&mut self, id: &str, name: &str) -> Result<()>;
    /// Soft delete. Its items stay in the doc (and in suggestion history).
    fn delete_list(&mut self, id: &str) -> Result<()>;

    /// Items on the list (see [`Item::visible`]): unticked first, then by category, then name.
    fn items(&self, list_id: &str) -> Result<Vec<Item>>;
    fn get_item(&self, id: &str) -> Result<Item>;
    /// Create with a caller-chosen ID (`Created`, `Unchanged`, or `Conflict`, like `put_list`).
    fn put_item(&mut self, user: &str, new: NewItem) -> Result<Write>;
    fn edit_item(&mut self, id: &str, edit: ItemEdit) -> Result<()>;
    /// Tick or untick. Idempotent: ticking a ticked item changes nothing (the first tick stands).
    fn set_checked(&mut self, user: &str, id: &str, checked: bool) -> Result<()>;
    fn delete_item(&mut self, id: &str) -> Result<()>;
    /// Hide the ticked items; returns how many. See the module docs for the concurrency rule.
    fn clear_checked(&mut self, list_id: &str) -> Result<usize>;
    /// Names ever added to this list that match `q` (case-insensitive substring; prefix matches
    /// first, then most often added), excluding names currently on the list. Derived, not stored.
    fn suggestions(&self, list_id: &str, q: &str, limit: usize) -> Result<Vec<String>>;
}

pub struct AmShoppingStore {
    pub doc: Automerge,
}

impl AsMut<Automerge> for AmShoppingStore {
    fn as_mut(&mut self) -> &mut Automerge {
        &mut self.doc
    }
}

/// Every device creates the same initial change (fixed actor + time), so the `lists` and
/// `items` maps have the same object IDs everywhere (see `todo_core::init_doc`).
pub fn init_doc() -> Automerge {
    let mut doc = Automerge::new().with_actor(ActorId::from([0u8; 16]));
    doc.transact_with::<_, _, AutomergeError, _>(
        |_| CommitOptions::default().with_time(0),
        |tx| {
            use automerge::transaction::Transactable;
            tx.put(ROOT, "schema_version", SCHEMA_VERSION)?;
            tx.put_object(ROOT, "lists", ObjType::Map)?;
            tx.put_object(ROOT, "items", ObjType::Map)?;
            Ok(())
        },
    )
    .expect("init on empty doc cannot fail");
    doc.set_actor(ActorId::random());
    doc
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn check_name(what: &str, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::Invalid(format!("{what} name, must not be empty")));
    }
    Ok(())
}

fn check_category(c: &Option<String>) -> Result<()> {
    match c {
        Some(c) if !CATEGORIES.contains(&c.as_str()) => {
            Err(Error::Invalid(format!("category {c:?}, expected one of {}", CATEGORIES.join(", "))))
        }
        _ => Ok(()),
    }
}

fn category_rank(c: &Option<String>) -> usize {
    c.as_deref().and_then(|c| CATEGORIES.iter().position(|x| *x == c)).unwrap_or(CATEGORIES.len())
}

impl AmShoppingStore {
    pub fn new() -> Self {
        Self { doc: init_doc() }
    }

    /// Wrap a loaded doc. An empty doc (new file) gets the shared init change merged in.
    pub fn from_doc(mut doc: Automerge) -> Result<Self> {
        doc.merge(&mut init_doc())?;
        Ok(Self { doc })
    }

    fn coll(&self, name: &str) -> Result<automerge::ObjId> {
        match self.doc.get(ROOT, name)? {
            Some((_, id)) => Ok(id),
            None => Err(Error::Automerge(format!("missing {name} map"))),
        }
    }

    fn find<T: Hydrate>(&self, coll: &str, id: &str) -> Result<Option<T>> {
        let obj = self.coll(coll)?;
        if self.doc.get(&obj, id)?.is_none() {
            return Ok(None);
        }
        Ok(Some(hydrate_prop(&self.doc, &obj, id)?))
    }

    fn get<T: Hydrate>(&self, coll: &str, id: &str) -> Result<T> {
        self.find(coll, id)?.ok_or_else(|| Error::NotFound(id.into()))
    }

    fn write<T: Reconcile>(&mut self, coll: &str, id: &str, value: &T) -> Result<()> {
        let obj = self.coll(coll)?;
        self.doc.transact::<_, _, autosurgeon::ReconcileError>(|tx| reconcile_prop(tx, &obj, id, value)).map_err(|f| f.error)?;
        Ok(())
    }

    fn all_items(&self) -> Result<HashMap<String, Item>> {
        Ok(hydrate_prop(&self.doc, ROOT, "items")?)
    }

    /// Read the item, apply `f`, and write only if something changed.
    fn modify_item(&mut self, id: &str, f: impl FnOnce(&mut Item)) -> Result<()> {
        let mut it: Item = self.get("items", id)?;
        let before = it.clone();
        f(&mut it);
        if it != before {
            self.write("items", id, &it)?;
        }
        Ok(())
    }
}

/// Everything in a shopping document, soft-deleted rows included (for `peer migrate-to-sqlite`).
#[derive(Debug, Clone, Default)]
pub struct ShoppingExport {
    pub lists: Vec<List>,
    pub items: Vec<Item>,
}

impl AmShoppingStore {
    pub fn export(&self) -> Result<ShoppingExport> {
        let lists: HashMap<String, List> = hydrate_prop(&self.doc, ROOT, "lists")?;
        let mut lists: Vec<List> = lists.into_values().collect();
        lists.sort_by(|a, b| a.id.cmp(&b.id));
        let mut items: Vec<Item> = self.all_items()?.into_values().collect();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(ShoppingExport { lists, items })
    }
}

impl Default for AmShoppingStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ShoppingStore for AmShoppingStore {
    fn lists(&self) -> Result<Vec<List>> {
        let all: HashMap<String, List> = hydrate_prop(&self.doc, ROOT, "lists")?;
        let mut v: Vec<List> = all.into_values().filter(|l| !l.deleted).collect();
        v.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
        Ok(v)
    }

    fn get_list(&self, id: &str) -> Result<List> {
        self.get("lists", id)
    }

    fn put_list(&mut self, id: &str, name: &str) -> Result<Write> {
        check_name("list", name)?;
        match self.find::<List>("lists", id)? {
            Some(l) if l.name == name && !l.deleted => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(id.into())),
            None => {
                self.write("lists", id, &List { id: id.into(), name: name.into(), created_at: now_ms(), deleted: false })?;
                Ok(Write::Created)
            }
        }
    }

    fn rename_list(&mut self, id: &str, name: &str) -> Result<()> {
        check_name("list", name)?;
        let mut l: List = self.get("lists", id)?;
        if l.name != name {
            l.name = name.into();
            self.write("lists", id, &l)?;
        }
        Ok(())
    }

    fn delete_list(&mut self, id: &str) -> Result<()> {
        let mut l: List = self.get("lists", id)?;
        if !l.deleted {
            l.deleted = true;
            self.write("lists", id, &l)?;
        }
        Ok(())
    }

    fn items(&self, list_id: &str) -> Result<Vec<Item>> {
        let mut v: Vec<Item> = self.all_items()?.into_values().filter(|i| i.list_id == list_id && i.visible()).collect();
        v.sort_by_key(|i| (i.checked(), category_rank(&i.category), i.name.to_lowercase(), i.added_at, i.id.clone()));
        Ok(v)
    }

    fn get_item(&self, id: &str) -> Result<Item> {
        self.get("items", id)
    }

    fn put_item(&mut self, user: &str, new: NewItem) -> Result<Write> {
        check_name("item", &new.name)?;
        check_category(&new.category)?;
        let list: List = self.get("lists", &new.list_id)?;
        if list.deleted {
            return Err(Error::NotFound(new.list_id));
        }
        let key = |i: &Item| (i.list_id.clone(), i.name.clone(), i.quantity.clone(), i.note.clone(), i.category.clone());
        let item = Item {
            id: new.id.clone(),
            list_id: new.list_id,
            name: new.name.trim().into(),
            quantity: new.quantity,
            note: new.note,
            category: new.category,
            check: None,
            cleared: None,
            added_by: user.into(),
            added_at: now_ms(),
            deleted: false,
        };
        match self.find::<Item>("items", &new.id)? {
            Some(existing) if key(&existing) == key(&item) && !existing.deleted => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(new.id)),
            None => {
                self.write("items", &new.id, &item)?;
                Ok(Write::Created)
            }
        }
    }

    fn edit_item(&mut self, id: &str, edit: ItemEdit) -> Result<()> {
        if let Some(n) = &edit.name {
            check_name("item", n)?;
        }
        if let Some(c) = &edit.category {
            check_category(c)?;
        }
        self.modify_item(id, |it| {
            if let Some(n) = edit.name {
                it.name = n.trim().into();
            }
            if let Some(q) = edit.quantity {
                it.quantity = q;
            }
            if let Some(n) = edit.note {
                it.note = n;
            }
            if let Some(c) = edit.category {
                it.category = c;
            }
        })
    }

    fn set_checked(&mut self, user: &str, id: &str, checked: bool) -> Result<()> {
        self.modify_item(id, |it| match (checked, it.check.is_some()) {
            (true, false) => it.check = Some(format!("{user}|{}", now_ms())),
            (false, true) => it.check = None,
            _ => {} // already in that state: the first tick stands
        })
    }

    fn delete_item(&mut self, id: &str) -> Result<()> {
        self.modify_item(id, |it| it.deleted = true)
    }

    fn clear_checked(&mut self, list_id: &str) -> Result<usize> {
        self.get_list(list_id)?;
        let ticked: Vec<Item> = self.all_items()?.into_values().filter(|i| i.list_id == list_id && i.visible() && i.checked()).collect();
        for it in &ticked {
            self.modify_item(&it.id, |i| i.cleared = i.check.clone())?;
        }
        Ok(ticked.len())
    }

    fn suggestions(&self, list_id: &str, q: &str, limit: usize) -> Result<Vec<String>> {
        let q = q.trim().to_lowercase();
        let all: Vec<Item> = self.all_items()?.into_values().filter(|i| i.list_id == list_id).collect();
        let on_list: std::collections::HashSet<String> = all.iter().filter(|i| i.visible()).map(|i| i.name.to_lowercase()).collect();
        // name (lowercase) -> (times added, newest spelling by added_at)
        let mut seen: HashMap<String, (usize, i64, String)> = HashMap::new();
        for i in &all {
            let e = seen.entry(i.name.to_lowercase()).or_insert((0, i64::MIN, i.name.clone()));
            e.0 += 1;
            if i.added_at >= e.1 {
                (e.1, e.2) = (i.added_at, i.name.clone());
            }
        }
        let mut v: Vec<(String, usize, String)> = seen
            .into_iter()
            .filter(|(k, _)| !on_list.contains(k) && k.contains(&q))
            .map(|(k, (n, _, shown))| (k, n, shown))
            .collect();
        v.sort_by_key(|(k, n, _)| (!k.starts_with(&q), Reverse(*n), k.clone()));
        Ok(v.into_iter().take(limit).map(|(_, _, shown)| shown).collect())
    }
}

#[cfg(test)]
mod tests;
