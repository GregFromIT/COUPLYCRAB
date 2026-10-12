//! Headless peer: runs sync for both docs, plus a small CLI.
//!
//! ```text
//! peer run [--config <path>]
//! peer todo add "<title>" [--due YYYY-MM-DD] [--config <path>]
//! peer todo list [--config <path>]
//! peer fitness sessions [--config <path>]
//! ```
//! The data dir is the directory containing the config file (default `./config.toml`).

use fitness_core::{AmFitnessStore, FitnessStore};
use std::{
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use sync::{config::Config, persist, Doc, SyncNode, FITNESS, TODO};
use todo_core::{AmTodoStore, TodoStore};

type Res<T> = Result<T, Box<dyn Error>>;

const USAGE: &str = "usage:
  peer run [--config <path>]
  peer todo add \"<title>\" [--due YYYY-MM-DD] [--config <path>]
  peer todo list [--config <path>]
  peer fitness sessions [--config <path>]";

fn doc_path(dir: &Path, kind: u8) -> PathBuf {
    dir.join(if kind == TODO { "todo.automerge" } else { "fitness.automerge" })
}

/// Pull `--name value` out of `args`.
fn take_opt(args: &mut Vec<String>, name: &str) -> Res<Option<String>> {
    match args.iter().position(|a| a == name) {
        None => Ok(None),
        Some(i) if i + 1 < args.len() => {
            let v = args.remove(i + 1);
            args.remove(i);
            Ok(Some(v))
        }
        Some(_) => Err(format!("{name} needs a value").into()),
    }
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> Res<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let config = PathBuf::from(take_opt(&mut args, "--config")?.unwrap_or_else(|| "config.toml".into()));
    let dir = match config.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if config.file_name().is_some_and(|f| f != sync::config::FILE) {
        return Err(format!("config file must be named {}", sync::config::FILE).into());
    }
    let due = take_opt(&mut args, "--due")?;
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["run"] => run(&dir),
        ["todo", "add", title] => {
            let mut s = AmTodoStore::from_doc(persist::load(&doc_path(&dir, TODO))?)?;
            let t = s.add(title, "", due.as_deref())?;
            persist::save(&doc_path(&dir, TODO), &mut s.doc)?;
            println!("added {}", t.id);
            Ok(())
        }
        ["todo", "list"] => {
            print_todos(&AmTodoStore::from_doc(persist::load(&doc_path(&dir, TODO))?)?)
        }
        ["fitness", "sessions"] => {
            let s = AmFitnessStore::from_doc(persist::load(&doc_path(&dir, FITNESS))?)?;
            for x in s.list_sessions()? {
                println!("{}  {:<10} {:<10} {} sets  {}", x.started_at, x.user_id, x.status.as_str(), x.sets.len(), x.note.unwrap_or_default());
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn print_todos(s: &AmTodoStore) -> Res<()> {
    for t in s.list()? {
        let check = if t.completed { "x" } else { " " };
        println!("[{check}] {:<10} {}", t.due.as_deref().unwrap_or(""), t.title);
    }
    Ok(())
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

#[tokio::main]
async fn run(dir: &Path) -> Res<()> {
    let cfg = Config::load(dir)?.ok_or_else(|| format!("no {} in {}", sync::config::FILE, dir.display()))?;
    let device = sync::config::device_id(dir)?;
    let todo = Arc::new(Mutex::new(AmTodoStore::from_doc(persist::load(&doc_path(dir, TODO))?)?));
    let fit = Arc::new(Mutex::new(AmFitnessStore::from_doc(persist::load(&doc_path(dir, FITNESS))?)?));
    let docs: BTreeMap<u8, Doc> = BTreeMap::from([(TODO, todo.clone() as Doc), (FITNESS, fit.clone() as Doc)]);

    let listener = sync::bind(cfg.listen_ip, cfg.listen_port).await?;
    println!("{} ({device}) listening on {}, peers {:?}", cfg.device_name, listener.local_addr()?, cfg.peers);
    let (node, mut remote) = SyncNode::new(docs.clone(), cfg.peers.clone(), sync::POLL);
    let mut net = tokio::spawn(node.clone().run(listener));

    // Persist after remote changes; pick up edits made by `peer todo add` etc.
    // ponytail: polls file mtimes every second; a file watcher if this ever matters.
    let mut seen: BTreeMap<u8, Option<SystemTime>> = docs.keys().map(|k| (*k, None)).collect();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let save = |kind: u8, seen: &mut BTreeMap<u8, Option<SystemTime>>| -> Res<bool> {
        let path = doc_path(dir, kind);
        let mut d = docs[&kind].lock().map_err(|_| "doc mutex poisoned")?;
        let merged = persist::save(&path, d.as_mut())?;
        seen.insert(kind, mtime(&path));
        Ok(merged)
    };
    for kind in [TODO, FITNESS] {
        save(kind, &mut seen)?;
    }
    loop {
        tokio::select! {
            r = &mut net => return Err(format!("sync stopped: {:?}", r?).into()),
            Some(kind) = remote.recv() => {
                save(kind, &mut seen)?;
                println!("synced {} from a peer", if kind == TODO { "todos" } else { "fitness" });
                if kind == TODO {
                    print_todos(&*todo.lock().map_err(|_| "doc mutex poisoned")?)?;
                }
            }
            _ = tick.tick() => {
                for kind in [TODO, FITNESS] {
                    if mtime(&doc_path(dir, kind)) != seen[&kind] && save(kind, &mut seen)? {
                        println!("picked up local edit to {}", if kind == TODO { "todos" } else { "fitness" });
                        node.local_change();
                    }
                }
            }
        }
    }
}
