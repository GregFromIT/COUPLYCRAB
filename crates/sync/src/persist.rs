//! `<data_dir>/<doc>.automerge` files: `Automerge::save()` bytes, written
//! atomically (temp file + rename) under an exclusive lock on `<doc>.lock`.
//!
//! Saving first merges whatever is on disk, so two processes sharing a data
//! dir (e.g. `peer run` and `peer todo add`) never overwrite each other's changes.

use automerge::Automerge;
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

fn sidecar(path: &Path, ext: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(ext);
    PathBuf::from(p)
}

fn read(path: &Path) -> io::Result<Option<Automerge>> {
    match fs::read(path) {
        Ok(bytes) => Automerge::load(&bytes)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Load a doc, or an empty one if the file doesn't exist. A corrupt file is an
/// error, never silently replaced.
pub fn load(path: &Path) -> io::Result<Automerge> {
    Ok(read(path)?.unwrap_or_default())
}

/// Merge the on-disk doc into `doc`, then write `doc` back. Returns whether the
/// disk copy contained changes `doc` didn't have (caller should tell peers).
pub fn save(path: &Path, doc: &mut Automerge) -> io::Result<bool> {
    let lock = File::create(sidecar(path, ".lock"))?;
    lock.lock()?;
    let before = doc.get_heads();
    if let Some(mut disk) = read(path)? {
        doc.merge(&mut disk).map_err(io::Error::other)?;
    }
    let merged_new = doc.get_heads() != before;
    let tmp = sidecar(path, ".tmp");
    {
        let mut f = File::create(&tmp)?;
        io::Write::write_all(&mut f, &doc.save())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(merged_new)
    // `lock` dropped here → unlocked
}

#[cfg(test)]
mod tests {
    use super::*;
    use automerge::{transaction::Transactable, ROOT};

    #[test]
    fn save_merges_concurrent_writer() {
        let dir = std::env::temp_dir().join(format!("household-persist-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.automerge");
        assert!(load(&path).unwrap().get_heads().is_empty());

        let mut a = load(&path).unwrap();
        let mut b = load(&path).unwrap();
        a.transact::<_, _, automerge::AutomergeError>(|tx| tx.put(ROOT, "a", 1).map(drop)).unwrap();
        b.transact::<_, _, automerge::AutomergeError>(|tx| tx.put(ROOT, "b", 2).map(drop)).unwrap();
        assert!(!save(&path, &mut a).unwrap());
        assert!(save(&path, &mut b).unwrap(), "b should pick up a's change from disk");
        let disk = load(&path).unwrap();
        use automerge::ReadDoc;
        assert!(disk.get(ROOT, "a").unwrap().is_some() && disk.get(ROOT, "b").unwrap().is_some());

        fs::write(&path, b"not automerge").unwrap();
        assert!(load(&path).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
