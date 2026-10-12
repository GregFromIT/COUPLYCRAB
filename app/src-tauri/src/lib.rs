//! Tauri shell: commands wrap `TodoStore` / `FitnessStore`; sync runs in a background task.

use fitness_core::{AmFitnessStore, BeepScore, FitnessStore, NewTemplateItem, SessionStatus, SetLog, TestKind, TestResult};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fmt::Display,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use sync::{config::Config, persist, Doc, SyncNode, FITNESS, TODO};
use tauri::{Emitter, Manager, State};
use todo_core::{AmTodoStore, Todo, TodoStore};

type R<T> = Result<T, String>;

fn e(x: impl Display) -> String {
    x.to_string()
}

fn doc_path(dir: &Path, kind: u8) -> PathBuf {
    dir.join(if kind == TODO { "todo.automerge" } else { "fitness.automerge" })
}

struct App {
    dir: PathBuf,
    cfg: Mutex<Option<Config>>,
    todo: Arc<Mutex<AmTodoStore>>,
    fit: Arc<Mutex<AmFitnessStore>>,
    node: Mutex<Option<SyncNode>>,
    status: Arc<Mutex<String>>,
}

impl App {
    fn read<S, T, E: Display>(&self, store: &Mutex<S>, f: impl FnOnce(&S) -> Result<T, E>) -> R<T> {
        f(&*store.lock().map_err(e)?).map_err(e)
    }

    /// Run a store operation, persist, and tell peers.
    fn write<S: AsMut<todo_core::Automerge>, T, E: Display>(&self, store: &Mutex<S>, kind: u8, f: impl FnOnce(&mut S) -> Result<T, E>) -> R<T> {
        let out = {
            let mut s = store.lock().map_err(e)?;
            let out = f(&mut s).map_err(e)?;
            persist::save(&doc_path(&self.dir, kind), s.as_mut()).map_err(e)?;
            out
        };
        if let Some(n) = &*self.node.lock().map_err(e)? {
            n.local_change();
        }
        Ok(out)
    }

    fn persist(&self, kind: u8) -> R<()> {
        let path = doc_path(&self.dir, kind);
        if kind == TODO {
            persist::save(&path, &mut self.todo.lock().map_err(e)?.doc).map_err(e)?;
        } else {
            persist::save(&path, &mut self.fit.lock().map_err(e)?.doc).map_err(e)?;
        }
        Ok(())
    }

    fn user_id(&self) -> R<String> {
        let cfg = self.cfg.lock().map_err(e)?;
        cfg.as_ref().map(|c| c.user_id.clone()).ok_or_else(|| "finish setup first".into())
    }
}

fn set_status(status: &Mutex<String>, s: String) {
    if let Ok(mut g) = status.lock() {
        *g = s;
    }
}

fn start_sync(handle: &tauri::AppHandle, app: &App, cfg: &Config) -> R<()> {
    let docs: BTreeMap<u8, Doc> = BTreeMap::from([(TODO, app.todo.clone() as Doc), (FITNESS, app.fit.clone() as Doc)]);
    let (node, mut remote) = SyncNode::new(docs, cfg.peers.clone(), sync::POLL);
    *app.node.lock().map_err(e)? = Some(node.clone());

    let (ip, port, status) = (cfg.listen_ip, cfg.listen_port, app.status.clone());
    tauri::async_runtime::spawn(async move {
        let res = match sync::bind(ip, port).await {
            Ok(l) => {
                set_status(&status, format!("listening on {ip}:{port}"));
                node.run(l).await
            }
            Err(err) => Err(err),
        };
        if let Err(err) = res {
            set_status(&status, format!("sync stopped: {err}"));
        }
    });

    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(kind) = remote.recv().await {
            let app = handle.state::<App>();
            if let Err(err) = app.persist(kind) {
                set_status(&app.status, format!("save failed: {err}"));
            }
            let _ = handle.emit("doc-changed", kind);
        }
    });
    Ok(())
}

#[derive(Serialize)]
struct Setup {
    config: Option<Config>,
    data_dir: String,
    status: String,
}

#[tauri::command]
fn get_setup(app: State<App>) -> R<Setup> {
    Ok(Setup {
        config: app.cfg.lock().map_err(e)?.clone(),
        data_dir: app.dir.display().to_string(),
        status: app.status.lock().map_err(e)?.clone(),
    })
}

#[tauri::command]
fn save_config(handle: tauri::AppHandle, app: State<App>, config: Config) -> R<()> {
    if config.user_id.trim().is_empty() || config.device_name.trim().is_empty() {
        return Err("user and device name are required".into());
    }
    let mut cfg = app.cfg.lock().map_err(e)?;
    if cfg.is_some() {
        return Err(format!("already configured: edit {} and restart", app.dir.join(sync::config::FILE).display()));
    }
    config.save(&app.dir).map_err(e)?;
    *cfg = Some(config.clone());
    drop(cfg);
    start_sync(&handle, &app, &config)
}

// ---- todos ----

#[tauri::command]
fn todo_list(app: State<App>) -> R<Vec<Todo>> {
    app.read(&app.todo, |s| s.list())
}

#[tauri::command]
fn todo_add(app: State<App>, title: String, notes: String, due: Option<String>) -> R<Todo> {
    app.write(&app.todo, TODO, |s| s.add(&title, &notes, due.as_deref()))
}

#[tauri::command]
fn todo_edit(app: State<App>, id: String, title: String, notes: String, due: Option<String>) -> R<()> {
    app.write(&app.todo, TODO, |s| s.edit(&id, &title, &notes, due.as_deref()))
}

#[tauri::command]
fn todo_set_completed(app: State<App>, id: String, completed: bool) -> R<()> {
    app.write(&app.todo, TODO, |s| s.set_completed(&id, completed))
}

#[tauri::command]
fn todo_delete(app: State<App>, id: String) -> R<()> {
    app.write(&app.todo, TODO, |s| s.delete(&id))
}

// ---- fitness ----

#[derive(Serialize)]
struct FitnessState {
    me: String,
    exercises: Vec<fitness_core::Exercise>,
    templates: Vec<fitness_core::Template>,
    sessions: Vec<fitness_core::Session>,
}

#[tauri::command]
fn fitness_state(app: State<App>) -> R<FitnessState> {
    let me = app.user_id().unwrap_or_default();
    app.read(&app.fit, |s| {
        Ok::<_, fitness_core::Error>(FitnessState { me, exercises: s.list_exercises()?, templates: s.list_templates()?, sessions: s.list_sessions()? })
    })
}

#[tauri::command]
fn exercise_add(app: State<App>, name: String, measurement: fitness_core::MeasurementType, tempo: Option<String>) -> R<()> {
    app.write(&app.fit, FITNESS, |s| s.add_exercise(&name, measurement, tempo.as_deref()).map(drop))
}

#[tauri::command]
fn template_add(app: State<App>, name: String, minutes: u32, items: Vec<NewTemplateItem>) -> R<()> {
    app.write(&app.fit, FITNESS, |s| s.add_template(&name, minutes, items).map(drop))
}

#[tauri::command]
fn session_start(app: State<App>, template_id: Option<String>) -> R<String> {
    let me = app.user_id()?;
    app.write(&app.fit, FITNESS, |s| s.start_session(&me, template_id.as_deref()).map(|x| x.id))
}

#[tauri::command]
fn session_log_set(app: State<App>, session_id: String, set: SetLog) -> R<()> {
    let me = app.user_id()?;
    app.write(&app.fit, FITNESS, |s| s.log_set(&me, &session_id, set).map(drop))
}

#[tauri::command]
fn session_finish(app: State<App>, session_id: String, status: SessionStatus, note: Option<String>) -> R<()> {
    let me = app.user_id()?;
    app.write(&app.fit, FITNESS, |s| s.finish_session(&me, &session_id, status, note.as_deref()))
}

#[tauri::command]
fn test_list(app: State<App>, kind: TestKind) -> R<Vec<TestResult>> {
    app.read(&app.fit, |s| s.list_tests(&kind))
}

#[tauri::command]
fn test_record(app: State<App>, kind: TestKind, date: String, score: String, exercise_id: Option<String>) -> R<()> {
    if !todo_core::valid_date(&date) {
        return Err(format!("invalid date {date:?}, expected YYYY-MM-DD"));
    }
    let (beep, value) = if kind == TestKind::BeepTest {
        (Some(score.trim().parse::<BeepScore>().map_err(e)?), None)
    } else {
        (None, Some(score.trim().parse::<f64>().map_err(|_| format!("invalid number {score:?}"))?))
    };
    let r = TestResult { id: String::new(), user_id: app.user_id()?, kind, date, beep, value, exercise_id, deleted: false };
    app.write(&app.fit, FITNESS, |s| s.record_test(r).map(drop))
}

fn init(handle: &tauri::AppHandle) -> Result<App, Box<dyn std::error::Error>> {
    let dir = handle.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    sync::config::device_id(&dir)?;
    Ok(App {
        cfg: Mutex::new(Config::load(&dir)?),
        todo: Arc::new(Mutex::new(AmTodoStore::from_doc(persist::load(&doc_path(&dir, TODO))?)?)),
        fit: Arc::new(Mutex::new(AmFitnessStore::from_doc(persist::load(&doc_path(&dir, FITNESS))?)?)),
        node: Mutex::new(None),
        status: Arc::new(Mutex::new("sync not configured".into())),
        dir,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let state = init(&handle)?;
            let cfg = state.cfg.lock().map_err(e)?.clone();
            app.manage(state);
            if let Some(cfg) = cfg {
                start_sync(&handle, &handle.state::<App>(), &cfg)?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_setup, save_config,
            todo_list, todo_add, todo_edit, todo_set_completed, todo_delete,
            fitness_state, exercise_add, template_add, session_start, session_log_set, session_finish,
            test_list, test_record,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
