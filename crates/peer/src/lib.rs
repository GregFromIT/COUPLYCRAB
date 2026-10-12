//! Pieces of the `peer` binary that its tests also use: the data-dir file layout, saving docs,
//! and the glue that turns "a peer's changes were merged" or "someone edited a file on disk"
//! into a change event for open browsers (`web::Events`).

use std::{
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};
use sync::{persist, Doc, SyncNode, FITNESS, SHOPPING, TODO};
use web::{DocKind, Events};

pub type Res<T> = Result<T, Box<dyn Error>>;

pub fn doc_path(dir: &Path, kind: u8) -> PathBuf {
    dir.join(match kind {
        TODO => "todo.automerge",
        SHOPPING => "shopping.automerge",
        _ => "fitness.automerge",
    })
}

pub fn kind_label(kind: u8) -> &'static str {
    match kind {
        TODO => "todos",
        SHOPPING => "shopping",
        _ => "fitness",
    }
}

/// The sync wire kind for a doc the web layer knows by `DocKind`.
pub fn sync_kind(kind: DocKind) -> u8 {
    match kind {
        DocKind::Todo => TODO,
        DocKind::Fitness => FITNESS,
        DocKind::Shopping => SHOPPING,
    }
}

pub fn doc_kind(kind: u8) -> DocKind {
    match kind {
        TODO => DocKind::Todo,
        SHOPPING => DocKind::Shopping,
        _ => DocKind::Fitness,
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Writes docs to disk and remembers each file's mtime, so polling only re-merges files
/// someone else (e.g. `peer todo add`) changed.
pub struct Saver {
    dir: PathBuf,
    docs: BTreeMap<u8, Doc>,
    seen: Mutex<BTreeMap<u8, Option<SystemTime>>>,
}

impl Saver {
    /// `docs` is every doc that lives in the data dir, whether or not it is synced to peers.
    pub fn new(dir: PathBuf, docs: BTreeMap<u8, Doc>) -> Self {
        Saver { dir, docs, seen: Mutex::new(BTreeMap::new()) }
    }

    pub fn kinds(&self) -> Vec<u8> {
        self.docs.keys().copied().collect()
    }

    /// Returns whether the file on disk had changes the in-memory doc lacked.
    pub fn save(&self, kind: u8) -> Res<bool> {
        let path = doc_path(&self.dir, kind);
        let mut d = self.docs[&kind].lock().map_err(|_| "doc mutex poisoned")?;
        let merged = persist::save(&path, d.as_mut())?;
        self.seen.lock().map_err(|_| "poisoned")?.insert(kind, mtime(&path));
        Ok(merged)
    }

    pub fn changed_on_disk(&self, kind: u8) -> bool {
        let seen = self.seen.lock().map(|s| s.get(&kind).copied().flatten()).unwrap_or(None);
        mtime(&doc_path(&self.dir, kind)) != seen
    }
}

/// A peer's changes were merged into doc `kind`: persist them and tell open browsers.
pub fn on_remote_change(saver: &Saver, events: &Events, kind: u8) -> Res<()> {
    saver.save(kind)?;
    events.notify(doc_kind(kind));
    Ok(())
}

/// Pick up edits other processes made to the doc files (e.g. `peer todo add` while the
/// server runs): merge them in, and for each doc that changed, tell connected peers and
/// open browsers. Returns the kinds that changed. Call about once a second.
// ponytail: polls file mtimes; a file watcher if this ever matters.
pub fn poll_disk(saver: &Saver, events: &Events, node: &SyncNode) -> Res<Vec<u8>> {
    let mut changed = vec![];
    for kind in saver.kinds() {
        if saver.changed_on_disk(kind) && saver.save(kind)? {
            node.local_change();
            events.notify(doc_kind(kind));
            changed.push(kind);
        }
    }
    Ok(changed)
}
