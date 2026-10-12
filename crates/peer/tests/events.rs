//! In-process: a second device syncs into the host, and an open browser (an SSE stream on the
//! host's web router) hears about it. Uses the same glue `peer serve` runs (`peer_app`).

use axum::{
    body::Body,
    http::{Method, Request},
};
use fitness_core::AmFitnessStore;
use http_body_util::BodyExt;
use peer_app::{doc_path, on_remote_change, poll_disk, sync_kind, Saver};
use serde_json::{json, Value};
use shopping_core::{AmShoppingStore, NewItem, ShoppingStore};
use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use sync::{bind, persist, Doc, SyncNode, FITNESS, SHOPPING, TODO};
use todo_core::{AmTodoStore, TodoStore};
use tower::ServiceExt;

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

struct Stores {
    todo: Arc<Mutex<AmTodoStore>>,
    fit: Arc<Mutex<AmFitnessStore>>,
    shop: Arc<Mutex<AmShoppingStore>>,
}

impl Stores {
    fn new() -> Self {
        Stores { todo: Arc::new(Mutex::new(AmTodoStore::new())), fit: Arc::new(Mutex::new(AmFitnessStore::new())), shop: Arc::new(Mutex::new(AmShoppingStore::new())) }
    }

    fn docs(&self) -> BTreeMap<u8, Doc> {
        BTreeMap::from([(TODO, self.todo.clone() as Doc), (FITNESS, self.fit.clone() as Doc), (SHOPPING, self.shop.clone() as Doc)])
    }
}

fn temp_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("household-peer-test-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An open `GET /events` response.
struct Sse {
    body: Body,
    buf: String,
}

impl Sse {
    async fn next(&mut self, wait: Duration) -> Option<Value> {
        loop {
            if let Some(i) = self.buf.find("\n\n") {
                let chunk: String = self.buf.drain(..i + 2).collect();
                if let Some(data) = chunk.lines().find_map(|l| l.strip_prefix("data:")) {
                    return Some(serde_json::from_str(data.trim()).unwrap());
                }
                continue;
            }
            match tokio::time::timeout(wait, self.body.frame()).await {
                Ok(Some(Ok(f))) => {
                    if let Some(d) = f.data_ref() {
                        self.buf.push_str(&String::from_utf8_lossy(d));
                    }
                }
                _ => return None,
            }
        }
    }

    /// The next message for `doc`, skipping others, within `wait`.
    async fn next_for(&mut self, doc: &str, wait: Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(tokio::time::Instant::now()) {
            let m = self.next(left).await?;
            if m["doc"] == doc {
                return Some(m);
            }
        }
        None
    }
}

struct Host {
    router: axum::Router,
    events: web::Events,
    saver: Arc<Saver>,
    node: SyncNode,
    stores: Stores,
    dir: PathBuf,
    guest: Stores,
    guest_node: SyncNode,
}

/// A host (web router + the glue from `peer serve`) and a guest device that syncs with it.
async fn host_and_guest() -> Host {
    let dir = temp_dir();
    let (host_l, guest_l) = (bind(LOCAL, 0).await.unwrap(), bind(LOCAL, 0).await.unwrap());
    let (host_addr, guest_addr) = (host_l.local_addr().unwrap().to_string(), guest_l.local_addr().unwrap().to_string());

    let stores = Stores::new();
    let (node, mut remote) = SyncNode::new(stores.docs(), vec![guest_addr], Duration::from_millis(200));
    tokio::spawn(node.clone().run(host_l));
    let saver = Arc::new(Saver::new(dir.clone(), stores.docs()));
    let events = web::Events::new();
    let (s, n) = (saver.clone(), node.clone());
    let saved: web::SavedHook = Arc::new(move |kind| {
        s.save(sync_kind(kind)).map_err(|e| e.to_string())?;
        n.local_change();
        Ok(())
    });
    let state = web::AppState::new(stores.todo.clone(), stores.fit.clone(), stores.shop.clone(), events.clone(), saved, web::WebConfig::default(), "alice", Some("alice".into())).unwrap();
    let router = web::router(state);
    // The loop `peer serve` runs for merges that arrive from peers.
    let (s, e) = (saver.clone(), events.clone());
    tokio::spawn(async move {
        while let Some(kind) = remote.recv().await {
            on_remote_change(&s, &e, kind).unwrap();
        }
    });

    let guest = Stores::new();
    let (guest_node, _rx) = SyncNode::new(guest.docs(), vec![host_addr], Duration::from_millis(200));
    tokio::spawn(guest_node.clone().run(guest_l));
    Host { router, events, saver, node, stores, dir, guest, guest_node }
}

impl Host {
    async fn open_events(&self) -> Sse {
        let req = Request::builder().method(Method::GET).uri("/api/v1/events").body(Body::empty()).unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), 200);
        Sse { body: res.into_body(), buf: String::new() }
    }

    async fn get(&self, uri: &str) -> Value {
        let req = Request::builder().method(Method::GET).uri(uri).body(Body::empty()).unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
    }
}

#[tokio::test]
async fn an_item_added_on_another_device_reaches_an_open_browser() {
    let h = host_and_guest().await;
    let mut browser = h.open_events().await;
    for _ in 0..3 {
        browser.next(Duration::from_secs(2)).await.expect("catch-up messages");
    }

    // The other device (a phone's own copy, or a second host) adds an item.
    {
        let mut g = h.guest.shop.lock().unwrap();
        g.put_list("groceries", "Groceries").unwrap();
        g.put_item("bob", NewItem { id: "milk".into(), list_id: "groceries".into(), name: "Milk".into(), quantity: None, note: None, category: Some("dairy".into()) }).unwrap();
    }
    h.guest_node.local_change();

    let m = browser.next_for("shopping", Duration::from_secs(10)).await.expect("an event after the merge, without any browser request");
    assert!(m["version"].as_u64().unwrap() >= 1, "{m}");
    // What the browser would now refetch has the item, and it is on disk.
    let items = h.get("/api/v1/lists/groceries/items").await;
    assert_eq!(items[0]["name"], "Milk");
    assert!(doc_path(&h.dir, SHOPPING).exists());
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn an_edit_made_on_disk_by_another_process_reaches_an_open_browser() {
    // `peer todo add` while `peer serve` runs: the CLI writes the file, the server's poll picks it up.
    let h = host_and_guest().await;
    h.saver.save(TODO).unwrap();
    let mut browser = h.open_events().await;
    for _ in 0..3 {
        browser.next(Duration::from_secs(2)).await.expect("catch-up messages");
    }
    assert!(poll_disk(&h.saver, &h.events, &h.node).unwrap().is_empty(), "nothing changed on disk yet");

    std::thread::sleep(Duration::from_millis(50)); // a distinct file mtime
    let path = doc_path(&h.dir, TODO);
    let mut cli = AmTodoStore::from_doc(persist::load(&path).unwrap()).unwrap();
    cli.add("from the command line", "", None).unwrap();
    persist::save(&path, &mut cli.doc).unwrap();

    assert_eq!(poll_disk(&h.saver, &h.events, &h.node).unwrap(), [TODO]);
    assert_eq!(browser.next_for("todo", Duration::from_secs(2)).await.unwrap()["version"], json!(1));
    assert_eq!(h.stores.todo.lock().unwrap().list().unwrap()[0].title, "from the command line");
    assert!(poll_disk(&h.saver, &h.events, &h.node).unwrap().is_empty(), "picked up once");
    let _ = std::fs::remove_dir_all(&h.dir);
}
