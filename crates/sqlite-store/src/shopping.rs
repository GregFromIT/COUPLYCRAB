//! `ShoppingStore` on SQLite.
//!
//! The Automerge store encoded a tick as one `"<user>|<ms>"` register and "clear checked" as
//! `cleared == check` because two peers could write at once. With one writer that is plain
//! columns: `checked_by` + `checked_at`, and `hidden` for a cleared tick. The `Item` the trait
//! returns is the same (`check` and `cleared` are rebuilt from the columns), so callers see no
//! difference.

use crate::{b, Db};
use rusqlite::{params, OptionalExtension, Row};
use shopping_core::{Error, Item, ItemEdit, List, NewItem, Result, ShoppingExport, ShoppingStore, Write, CATEGORIES};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

fn db<E: std::fmt::Display>(e: E) -> Error {
    Error::Storage(e.to_string())
}

pub struct SqliteShoppingStore {
    db: Db,
}

impl SqliteShoppingStore {
    pub fn new(db: Db) -> Self {
        SqliteShoppingStore { db }
    }

    /// Insert or replace a whole list / item, keeping timestamps (for the migration).
    pub fn import(&self, e: &ShoppingExport) -> Result<()> {
        for l in &e.lists {
            write_list(&self.db, l)?;
        }
        for i in &e.items {
            write_item(&self.db, i)?;
        }
        Ok(())
    }

    /// `(lists, items)`, deleted ones included.
    pub fn counts(&self) -> Result<(usize, usize)> {
        let n = |t: &str| self.db.with(|c| c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get::<_, i64>(0))).map(|n| n as usize).map_err(db);
        Ok((n("shopping_lists")?, n("shopping_items")?))
    }
}

fn list_row(r: &Row) -> rusqlite::Result<List> {
    Ok(List { id: r.get("id")?, name: r.get("name")?, created_at: r.get("created_at")?, deleted: r.get::<_, i64>("deleted")? != 0 })
}

fn item_row(r: &Row) -> rusqlite::Result<Item> {
    let (by, at): (Option<String>, Option<i64>) = (r.get("checked_by")?, r.get("checked_at")?);
    let check = by.zip(at).map(|(u, t)| format!("{u}|{t}"));
    let hidden = r.get::<_, i64>("hidden")? != 0;
    Ok(Item {
        id: r.get("id")?,
        list_id: r.get("list_id")?,
        name: r.get("name")?,
        quantity: r.get("quantity")?,
        note: r.get("note")?,
        category: r.get("category")?,
        cleared: if hidden { check.clone() } else { None },
        check,
        added_by: r.get("added_by")?,
        added_at: r.get("added_at")?,
        deleted: r.get::<_, i64>("deleted")? != 0,
    })
}

fn find_list(d: &Db, id: &str) -> Result<Option<List>> {
    d.with(|c| c.query_row("SELECT * FROM shopping_lists WHERE id = ?1", [id], list_row).optional()).map_err(db)
}

fn find_item(d: &Db, id: &str) -> Result<Option<Item>> {
    d.with(|c| c.query_row("SELECT * FROM shopping_items WHERE id = ?1", [id], item_row).optional()).map_err(db)
}

fn write_list(d: &Db, l: &List) -> Result<()> {
    d.with(|c| {
        c.execute(
            "INSERT INTO shopping_lists (id, name, created_at, deleted) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET name = ?2, created_at = ?3, deleted = ?4",
            params![l.id, l.name, l.created_at, b(l.deleted)],
        )
        .map(|_| ())
    })
    .map_err(db)
}

fn write_item(d: &Db, i: &Item) -> Result<()> {
    let (by, at) = (i.checked_by().map(String::from), i.checked_at());
    let hidden = i.check.is_some() && i.check == i.cleared;
    d.with(|c| {
        c.execute(
            "INSERT INTO shopping_items (id, list_id, name, quantity, note, category, checked_by, checked_at, hidden, added_by, added_at, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (id) DO UPDATE SET list_id = ?2, name = ?3, quantity = ?4, note = ?5, category = ?6, checked_by = ?7,
                 checked_at = ?8, hidden = ?9, added_by = ?10, added_at = ?11, deleted = ?12",
            params![i.id, i.list_id, i.name, i.quantity, i.note, i.category, by, at, b(hidden), i.added_by, i.added_at, b(i.deleted)],
        )
        .map(|_| ())
    })
    .map_err(db)
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
        Some(c) if !CATEGORIES.contains(&c.as_str()) => Err(Error::Invalid(format!("category {c:?}, expected one of {}", CATEGORIES.join(", ")))),
        _ => Ok(()),
    }
}

fn category_rank(c: &Option<String>) -> usize {
    c.as_deref().and_then(|c| CATEGORIES.iter().position(|x| *x == c)).unwrap_or(CATEGORIES.len())
}

impl SqliteShoppingStore {
    fn items_of(&self, list_id: &str) -> Result<Vec<Item>> {
        self.db
            .with(|c| {
                let mut st = c.prepare("SELECT * FROM shopping_items WHERE list_id = ?1")?;
                let rows = st.query_map([list_id], item_row)?;
                rows.collect()
            })
            .map_err(db)
    }

    fn modify_item(&mut self, id: &str, f: impl FnOnce(&mut Item)) -> Result<()> {
        let mut it = find_item(&self.db, id)?.ok_or_else(|| Error::NotFound(id.into()))?;
        let before = it.clone();
        f(&mut it);
        if it != before {
            write_item(&self.db, &it)?;
        }
        Ok(())
    }
}

impl ShoppingStore for SqliteShoppingStore {
    fn lists(&self) -> Result<Vec<List>> {
        self.db
            .with(|c| {
                let mut st = c.prepare("SELECT * FROM shopping_lists WHERE deleted = 0 ORDER BY lower(name), id")?;
                let rows = st.query_map([], list_row)?;
                rows.collect()
            })
            .map_err(db)
    }

    fn get_list(&self, id: &str) -> Result<List> {
        find_list(&self.db, id)?.ok_or_else(|| Error::NotFound(id.into()))
    }

    fn put_list(&mut self, id: &str, name: &str) -> Result<Write> {
        check_name("list", name)?;
        match find_list(&self.db, id)? {
            Some(l) if l.name == name && !l.deleted => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(id.into())),
            None => {
                write_list(&self.db, &List { id: id.into(), name: name.into(), created_at: now_ms(), deleted: false })?;
                Ok(Write::Created)
            }
        }
    }

    fn rename_list(&mut self, id: &str, name: &str) -> Result<()> {
        check_name("list", name)?;
        let mut l = self.get_list(id)?;
        if l.name != name {
            l.name = name.into();
            write_list(&self.db, &l)?;
        }
        Ok(())
    }

    fn delete_list(&mut self, id: &str) -> Result<()> {
        let mut l = self.get_list(id)?;
        if !l.deleted {
            l.deleted = true;
            write_list(&self.db, &l)?;
        }
        Ok(())
    }

    fn items(&self, list_id: &str) -> Result<Vec<Item>> {
        let mut v: Vec<Item> = self.items_of(list_id)?.into_iter().filter(|i| i.visible()).collect();
        v.sort_by_key(|i| (i.checked(), category_rank(&i.category), i.name.to_lowercase(), i.added_at, i.id.clone()));
        Ok(v)
    }

    fn get_item(&self, id: &str) -> Result<Item> {
        find_item(&self.db, id)?.ok_or_else(|| Error::NotFound(id.into()))
    }

    fn put_item(&mut self, user: &str, new: NewItem) -> Result<Write> {
        check_name("item", &new.name)?;
        check_category(&new.category)?;
        let list = self.get_list(&new.list_id)?;
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
        match find_item(&self.db, &new.id)? {
            Some(existing) if key(&existing) == key(&item) && !existing.deleted => Ok(Write::Unchanged),
            Some(_) => Err(Error::Conflict(new.id)),
            None => {
                write_item(&self.db, &item)?;
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
            (true, false) => {
                it.check = Some(format!("{user}|{}", now_ms()));
                it.cleared = None;
            }
            (false, true) => {
                it.check = None;
                it.cleared = None;
            }
            _ => {} // already in that state: the first tick stands
        })
    }

    fn delete_item(&mut self, id: &str) -> Result<()> {
        self.modify_item(id, |it| it.deleted = true)
    }

    fn clear_checked(&mut self, list_id: &str) -> Result<usize> {
        self.get_list(list_id)?;
        self.db
            .with(|c| {
                c.execute(
                    "UPDATE shopping_items SET hidden = 1 WHERE list_id = ?1 AND deleted = 0 AND hidden = 0 AND checked_by IS NOT NULL",
                    [list_id],
                )
            })
            .map_err(db)
    }

    fn suggestions(&self, list_id: &str, q: &str, limit: usize) -> Result<Vec<String>> {
        let q = q.trim().to_lowercase();
        let all = self.items_of(list_id)?;
        let on_list: HashSet<String> = all.iter().filter(|i| i.visible()).map(|i| i.name.to_lowercase()).collect();
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
