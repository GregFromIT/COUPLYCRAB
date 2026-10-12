//! Headless peer: runs sync for both docs, plus a small CLI.
//!
//! ```text
//! peer run [--config <path>]
//! peer serve [--config <path>] [--dev-user <user_id>]
//! peer todo add "<title>" [--due YYYY-MM-DD] [--config <path>]
//! peer todo list [--config <path>]
//! peer fitness sessions [--config <path>]
//! peer fitness migrate-v1 [--config <path>]
//! peer fitness recommend --workout <id> [--program <id>] [--user <id>] [--date YYYY-MM-DD] [--config <path>]
//! ```
//! The data dir is the directory containing the config file (default `./config.toml`).

use fitness_core::{AmFitnessStore, FitnessStore};
use peer_app::{doc_path, kind_label, on_remote_change, poll_disk, sync_kind, Res, Saver};
use shopping_core::AmShoppingStore;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use sync::{config::Config, persist, Doc, SyncNode, FITNESS, SHOPPING, TODO};
use todo_core::{AmTodoStore, TodoStore};

const USAGE: &str = "usage:
  peer run [--config <path>]
  peer serve [--config <path>] [--dev-user <user_id>]
  peer todo add \"<title>\" [--due YYYY-MM-DD] [--config <path>]
  peer todo list [--config <path>]
  peer fitness sessions [--config <path>]
  peer fitness migrate-v1 [--config <path>]
  peer fitness recommend --workout <id> [--program <id>] [--user <id>] [--date YYYY-MM-DD] [--config <path>]";

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
    let dev_user = take_opt(&mut args, "--dev-user")?;
    let (workout, program, user, date) = (
        take_opt(&mut args, "--workout")?,
        take_opt(&mut args, "--program")?,
        take_opt(&mut args, "--user")?,
        take_opt(&mut args, "--date")?,
    );
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["run"] => run(&dir, None),
        ["serve"] => run(&dir, Some(dev_user)),
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
        ["fitness", "migrate-v1"] => {
            let mut s = AmFitnessStore::from_doc(persist::load(&doc_path(&dir, FITNESS))?)?;
            let m = s.migrate_v1_blocks()?;
            persist::save(&doc_path(&dir, FITNESS), &mut s.doc)?;
            println!("migrated {} v1 block(s) into programs", m.created);
            for n in m.notes {
                println!("  note: {n}");
            }
            Ok(())
        }
        ["fitness", "recommend"] => {
            let cfg = Config::load(&dir)?.ok_or_else(|| format!("no {} in {}", sync::config::FILE, dir.display()))?;
            let user = user.unwrap_or(cfg.user_id);
            let text = std::fs::read_to_string(dir.join(sync::config::FILE))?;
            let tz = toml::from_str::<WebSection>(&text).map_err(|e| format!("[web]: {e}"))?.web.timezone;
            let date = match date {
                Some(d) => chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").map_err(|_| format!("--date {d:?}, expected YYYY-MM-DD"))?,
                None => web::today_in(&tz)?,
            };
            let s = AmFitnessStore::from_doc(persist::load(&doc_path(&dir, FITNESS))?)?;
            let program = match program {
                Some(p) => p,
                None => s.active_program(&user, date)?.map(|(p, _)| p.id).ok_or_else(|| format!("{user} has no program running on {date}; pass --program <id>"))?,
            };
            let workout = workout.ok_or("--workout <id> is required")?;
            let plan = s.plan_workout(&user, &program, &workout, date).map_err(|e| match e {
                fitness_core::Error::NotFound(_) => {
                    let ids = s.get_program(&program).map(|p| p.ordered_workouts().iter().map(|w| format!("{} ({})", w.id, w.name)).collect::<Vec<_>>().join(", "));
                    format!("workout {workout:?} not found in program {program}; its workouts: {}", ids.unwrap_or_default())
                }
                e => e.to_string(),
            })?;
            print_plan(&plan, date);
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn print_plan(p: &fitness_core::WorkoutPlan, date: chrono::NaiveDate) {
    let week = p.week.map_or("outside the program".to_string(), |w| format!("week {w}"));
    println!("{} · {date} · {week}", p.name);
    if let Some(l) = &p.external_label {
        println!("  external session: {l}");
    }
    for it in &p.items {
        let x = &it.prescription;
        let name = it.exercise_name.as_deref().unwrap_or(&it.exercise_id);
        let rest = x.rest_s.map(|r| format!(", rest {r}s")).unwrap_or_default();
        println!("{name}: {} sets × {}–{} reps @ {} RIR{rest}", x.sets, x.rep_min, x.rep_max, x.rir);
        for w in &it.warmups {
            println!("    warm-up: {} kg × {}", w.load_kg, w.reps);
        }
        match &it.recommendation {
            Some(r) => {
                println!("    → {} kg × {} @ {} RIR", r.load_kg, r.reps, r.rir);
                println!("      {}", r.reason);
            }
            None => println!("    no history yet: pick a load"),
        }
    }
}

fn print_todos(s: &AmTodoStore) -> Res<()> {
    for t in s.list()? {
        let check = if t.completed { "x" } else { " " };
        println!("[{check}] {:<10} {}", t.due.as_deref().unwrap_or(""), t.title);
    }
    Ok(())
}

#[derive(serde::Deserialize, Default)]
struct WebSection {
    #[serde(default)]
    web: web::WebConfig,
}

/// `peer run` (sync only) and `peer serve` (sync + HTTP on `[web] listen`).
/// `serve = Some(dev_user)` turns on the HTTP server.
#[tokio::main]
async fn run(dir: &Path, serve: Option<Option<String>>) -> Res<()> {
    let cfg = Config::load(dir)?.ok_or_else(|| format!("no {} in {}", sync::config::FILE, dir.display()))?;
    let device = sync::config::device_id(dir)?;
    let todo = Arc::new(Mutex::new(AmTodoStore::from_doc(persist::load(&doc_path(dir, TODO))?)?));
    let fit = Arc::new(Mutex::new(AmFitnessStore::from_doc(persist::load(&doc_path(dir, FITNESS))?)?));
    let shopping = Arc::new(Mutex::new(AmShoppingStore::from_doc(persist::load(&doc_path(dir, SHOPPING))?)?));
    // Every doc lives in the data dir; `sync_shopping = false` keeps the shopping doc off the
    // wire (v0 and v1 peers drop the connection on a doc kind they do not know).
    let all_docs: BTreeMap<u8, Doc> = BTreeMap::from([(TODO, todo.clone() as Doc), (FITNESS, fit.clone() as Doc), (SHOPPING, shopping.clone() as Doc)]);
    let mut sync_docs = all_docs.clone();
    if !cfg.sync_shopping {
        sync_docs.remove(&SHOPPING);
    }

    let listener = sync::bind(cfg.listen_ip, cfg.listen_port).await?;
    println!("{} ({device}) sync listening on {}, peers {:?}{}", cfg.device_name, listener.local_addr()?, cfg.peers, if cfg.sync_shopping { "" } else { ", shopping not synced" });
    let (node, mut remote) = SyncNode::new(sync_docs, cfg.peers.clone(), sync::POLL);
    let mut net = tokio::spawn(node.clone().run(listener));

    let saver = Arc::new(Saver::new(dir.to_path_buf(), all_docs));
    for kind in saver.kinds() {
        saver.save(kind)?;
    }
    let events = web::Events::new();

    let mut http = match serve {
        None => tokio::spawn(std::future::pending()),
        Some(dev_user) => {
            let text = std::fs::read_to_string(dir.join(sync::config::FILE))?;
            let web_cfg = toml::from_str::<WebSection>(&text).map_err(|e| format!("[web]: {e}"))?.web;
            let (s, n) = (saver.clone(), node.clone());
            // ponytail: blocking file write inside the request; move to spawn_blocking if saves get slow.
            let saved: web::SavedHook = Arc::new(move |kind| {
                s.save(sync_kind(kind)).map_err(|e| e.to_string())?;
                n.local_change();
                Ok(())
            });
            let state = web::AppState::new(todo.clone(), fit.clone(), shopping.clone(), events.clone(), saved, web_cfg.clone(), &cfg.user_id, dev_user.clone())?;
            let http_listener = tokio::net::TcpListener::bind(web_cfg.listen).await?;
            println!("http on http://{}{}", http_listener.local_addr()?, dev_user.map(|u| format!(" (dev user {u})")).unwrap_or_default());
            tokio::spawn(async move { axum::serve(http_listener, web::router(state)).await })
        }
    };

    // Persist after remote changes (and tell open browsers); pick up edits made by `peer todo add` etc.
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            r = &mut net => return Err(format!("sync stopped: {:?}", r?).into()),
            r = &mut http => return Err(format!("http stopped: {:?}", r?).into()),
            Some(kind) = remote.recv() => {
                on_remote_change(&saver, &events, kind)?;
                println!("synced {} from a peer", kind_label(kind));
            }
            _ = tick.tick() => {
                for kind in poll_disk(&saver, &events, &node)? {
                    println!("picked up local edit to {}", kind_label(kind));
                }
            }
        }
    }
}
