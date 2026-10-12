//! Shopping list API and the `/events` Server-Sent Events stream, in-process (no sockets).

mod common;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use common::*;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;
use web::DocKind;

fn names(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn lists_and_items_end_to_end() {
    let a = app(users_config(), None);
    let al = |m: Method, uri: String, b: Option<Value>| {
        let a = &a;
        async move { a.as_user("alice@example.com", m, &uri, b).await }
    };
    let bo = |m: Method, uri: String, b: Option<Value>| {
        let a = &a;
        async move { a.as_user("bob@example.com", m, &uri, b).await }
    };

    // Lists.
    let (s, l) = al(Method::PUT, "/api/v1/lists/groceries".into(), Some(json!({"name": "Groceries"}))).await;
    assert_eq!((s, l["name"].clone(), l["unchecked"].clone(), l["total"].clone()), (StatusCode::CREATED, json!("Groceries"), json!(0), json!(0)));
    assert_eq!(al(Method::PUT, "/api/v1/lists/groceries".into(), Some(json!({"name": "Groceries"}))).await.0, StatusCode::OK, "repeat: no-op");
    assert_eq!(al(Method::PUT, "/api/v1/lists/groceries".into(), Some(json!({"name": "Food"}))).await.0, StatusCode::CONFLICT);
    al(Method::PUT, "/api/v1/lists/bunnings".into(), Some(json!({"name": "Bunnings"}))).await;
    assert_eq!(names(&al(Method::GET, "/api/v1/lists".into(), None).await.1), ["Bunnings", "Groceries"]);
    for bad in [json!({"name": ""}), json!({"name": "x".repeat(201)}), json!({})] {
        assert!(al(Method::PUT, "/api/v1/lists/x".into(), Some(bad)).await.0.is_client_error());
    }

    // Items: alice adds, in any order; the list comes back unchecked-first, by category.
    let item = |name: &str, cat: Option<&str>| json!({"name": name, "category": cat, "quantity": null});
    let (s, it) = al(Method::PUT, "/api/v1/lists/groceries/items/i-soap".into(), Some(item("Soap", Some("household")))).await;
    assert_eq!((s, it["added_by"].clone(), it["checked"].clone(), it["checked_by"].clone()), (StatusCode::CREATED, json!("alice"), json!(false), Value::Null));
    al(Method::PUT, "/api/v1/lists/groceries/items/i-milk".into(), Some(json!({"name": "Milk", "category": "dairy", "quantity": "2 L", "note": "full cream"}))).await;
    al(Method::PUT, "/api/v1/lists/groceries/items/i-apples".into(), Some(item("Apples", Some("produce")))).await;
    al(Method::PUT, "/api/v1/lists/groceries/items/i-misc".into(), Some(item("Mystery", None))).await;
    assert_eq!(names(&al(Method::GET, "/api/v1/lists/groceries/items".into(), None).await.1), ["Apples", "Milk", "Soap", "Mystery"]);
    assert_eq!(al(Method::PUT, "/api/v1/lists/groceries/items/i-soap".into(), Some(item("Soap", Some("household")))).await.0, StatusCode::OK, "repeat: no-op");
    assert_eq!(al(Method::PUT, "/api/v1/lists/groceries/items/i-soap".into(), Some(item("Soap", Some("pantry")))).await.0, StatusCode::CONFLICT);
    assert_eq!(al(Method::PUT, "/api/v1/lists/groceries/items/i-new".into(), Some(item("Chips", Some("snacks")))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(al(Method::PUT, "/api/v1/lists/nope/items/i-new".into(), Some(item("Chips", None))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(al(Method::PUT, "/api/v1/lists/groceries/items/i-new".into(), Some(json!({"name": "x", "quantity": "q".repeat(51)}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(al(Method::PUT, "/api/v1/lists/groceries/items/i%20new".into(), Some(item("x", None))).await.0, StatusCode::BAD_REQUEST, "bad id");

    // Either user ticks; the tick records who and when. Ticked items sink.
    let (s, it) = bo(Method::PATCH, "/api/v1/lists/groceries/items/i-apples".into(), Some(json!({"checked": true}))).await;
    assert_eq!((s, it["checked"].clone(), it["checked_by"].clone()), (StatusCode::OK, json!(true), json!("bob")));
    assert!(it["checked_at"].as_i64().unwrap() > 1_700_000_000_000);
    assert_eq!(names(&al(Method::GET, "/api/v1/lists/groceries/items".into(), None).await.1), ["Milk", "Soap", "Mystery", "Apples"]);
    let again = al(Method::PATCH, "/api/v1/lists/groceries/items/i-apples".into(), Some(json!({"checked": true}))).await.1;
    assert_eq!(again["checked_by"], "bob", "ticking again changes nothing");
    let counts = al(Method::GET, "/api/v1/lists".into(), None).await.1;
    assert_eq!((counts[1]["unchecked"].clone(), counts[1]["total"].clone()), (json!(3), json!(4)));

    // Edit: partial, with null clearing.
    let (s, it) = al(Method::PATCH, "/api/v1/lists/groceries/items/i-milk".into(), Some(json!({"name": "Oat milk", "quantity": null, "category": "pantry"}))).await;
    assert_eq!((s, it["name"].clone(), it["quantity"].clone(), it["note"].clone(), it["category"].clone()), (StatusCode::OK, json!("Oat milk"), Value::Null, json!("full cream"), json!("pantry")));
    assert_eq!(al(Method::PATCH, "/api/v1/lists/groceries/items/i-milk".into(), Some(json!({"category": "snacks"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(al(Method::PATCH, "/api/v1/lists/bunnings/items/i-milk".into(), Some(json!({"name": "x"}))).await.0, StatusCode::NOT_FOUND, "that item is on another list");
    assert_eq!(al(Method::PATCH, "/api/v1/lists/groceries/items/nope".into(), Some(json!({"name": "x"}))).await.0, StatusCode::NOT_FOUND);

    // Suggestions come from history and skip what is on the list.
    assert!(al(Method::GET, "/api/v1/lists/groceries/suggestions?q=app".into(), None).await.1.as_array().unwrap().is_empty(), "Apples is on the list");
    assert_eq!(al(Method::POST, "/api/v1/lists/groceries/clear-checked".into(), Some(json!({}))).await.1, json!({"cleared": 1}));
    assert_eq!(names(&al(Method::GET, "/api/v1/lists/groceries/items".into(), None).await.1), ["Oat milk", "Soap", "Mystery"]);
    assert_eq!(al(Method::GET, "/api/v1/lists/groceries/suggestions?q=app".into(), None).await.1, json!(["Apples"]));
    assert_eq!(al(Method::GET, "/api/v1/lists/groceries/suggestions".into(), None).await.1, json!(["Apples"]), "no query: everything not on the list");
    assert_eq!(al(Method::GET, "/api/v1/lists/groceries/suggestions?limit=0".into(), None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(al(Method::GET, "/api/v1/lists/nope/suggestions?q=a".into(), None).await.0, StatusCode::NOT_FOUND);

    // Delete an item, then a list.
    assert_eq!(al(Method::DELETE, "/api/v1/lists/groceries/items/i-misc".into(), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(names(&al(Method::GET, "/api/v1/lists/groceries/items".into(), None).await.1), ["Oat milk", "Soap"]);
    assert_eq!(al(Method::DELETE, "/api/v1/lists/groceries/items/i-misc".into(), None).await.0, StatusCode::NO_CONTENT, "idempotent");
    let (s, l) = al(Method::PATCH, "/api/v1/lists/bunnings".into(), Some(json!({"name": "Hardware"}))).await;
    assert_eq!((s, l["name"].clone()), (StatusCode::OK, json!("Hardware")));
    assert_eq!(al(Method::PATCH, "/api/v1/lists/bunnings".into(), Some(json!({"deleted": false}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(al(Method::PATCH, "/api/v1/lists/bunnings".into(), Some(json!({"deleted": true}))).await.0, StatusCode::OK);
    assert_eq!(names(&al(Method::GET, "/api/v1/lists".into(), None).await.1), ["Groceries"]);
    assert_eq!(al(Method::GET, "/api/v1/lists/nope/items".into(), None).await.0, StatusCode::NOT_FOUND);

    // Every write was persisted (the saved hook ran).
    assert!(a.saves.load(std::sync::atomic::Ordering::SeqCst) >= 15);
}

#[tokio::test]
async fn writes_need_json_and_an_identity() {
    let a = app(users_config(), None);
    assert_eq!(a.get("/api/v1/lists").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(a.get("/api/v1/events").await.0, StatusCode::UNAUTHORIZED, "no stream without an identity");
    let (s, _, _) = a.req(Method::PUT, "/api/v1/lists/x", Some(json!({"name": "x"})), &[("Tailscale-User-Login", "alice@example.com"), ("Content-Type", "text/plain")]).await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

// ---- Server-Sent Events ----

/// A streaming `/events` response read frame by frame.
struct Stream {
    body: Body,
    buf: String,
}

impl Stream {
    async fn open(app: &App) -> Stream {
        let req = Request::builder().method(Method::GET).uri("/api/v1/events").header("Tailscale-User-Login", "alice@example.com").body(Body::empty()).unwrap();
        let res = app.router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-cache");
        Stream { body: res.into_body(), buf: String::new() }
    }

    /// The next `data:` message, skipping comments, or `None` after `wait` of silence.
    async fn next(&mut self, wait: Duration) -> Option<Value> {
        loop {
            if let Some(i) = self.buf.find("\n\n") {
                let chunk: String = self.buf.drain(..i + 2).collect();
                if let Some(data) = chunk.lines().find_map(|l| l.strip_prefix("data:")) {
                    return Some(serde_json::from_str(data.trim()).unwrap());
                }
                continue; // a comment (keep-alive)
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

    /// Reads the three catch-up messages a new connection starts with.
    async fn initial(&mut self) -> Vec<Value> {
        let mut v = vec![];
        for _ in 0..3 {
            v.push(self.next(Duration::from_secs(2)).await.expect("initial message"));
        }
        v
    }
}

const WAIT: Duration = Duration::from_secs(2);
const QUIET: Duration = Duration::from_millis(300);

#[tokio::test]
async fn events_follow_local_writes_and_not_failed_ones() {
    let a = app(users_config(), None);
    let mut s = Stream::open(&a).await;
    assert_eq!(s.initial().await, [json!({"doc": "todo", "version": 0}), json!({"doc": "fitness", "version": 0}), json!({"doc": "shopping", "version": 0})]);
    assert_eq!(s.next(QUIET).await, None, "nothing yet");

    let put = |uri: &str, b: Value| a.as_user("bob@example.com", Method::PUT, uri.to_string().leak(), Some(b));
    assert_eq!(put("/api/v1/lists/groceries", json!({"name": "Groceries"})).await.0, StatusCode::CREATED);
    assert_eq!(s.next(WAIT).await, Some(json!({"doc": "shopping", "version": 1})));
    put("/api/v1/lists/groceries/items/m", json!({"name": "Milk"})).await;
    assert_eq!(s.next(WAIT).await, Some(json!({"doc": "shopping", "version": 2})));
    assert_eq!(put("/api/v1/todos/t1", json!({"title": "Call mum"})).await.0, StatusCode::CREATED);
    assert_eq!(s.next(WAIT).await, Some(json!({"doc": "todo", "version": 1})));
    assert_eq!(put("/api/v1/exercises/bench", json!({"name": "Bench", "measurement": "WeightReps"})).await.0, StatusCode::CREATED);
    assert_eq!(s.next(WAIT).await, Some(json!({"doc": "fitness", "version": 1})));

    // A rejected write changes nothing, so it says nothing.
    assert_eq!(put("/api/v1/lists/groceries", json!({"name": "Different"})).await.0, StatusCode::CONFLICT);
    assert_eq!(put("/api/v1/lists/groceries/items/bad", json!({"name": "x", "category": "nope"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(s.next(QUIET).await, None);
}

#[tokio::test]
async fn events_follow_a_peer_merge_and_reach_every_subscriber() {
    let a = app(users_config(), None);
    let (mut s1, mut s2) = (Stream::open(&a).await, Stream::open(&a).await);
    s1.initial().await;
    s2.initial().await;
    // What `peer` does after another device's changes are merged into a doc (or an edit is
    // found on disk): it tells the hub. Nothing went through the API here.
    a.events.notify(DocKind::Shopping);
    a.events.notify(DocKind::Fitness);
    for s in [&mut s1, &mut s2] {
        assert_eq!(s.next(WAIT).await, Some(json!({"doc": "shopping", "version": 1})));
        assert_eq!(s.next(WAIT).await, Some(json!({"doc": "fitness", "version": 1})));
    }
}

#[tokio::test]
async fn a_new_connection_gets_the_current_counters() {
    // What a browser sees when EventSource reconnects after a dropped connection: it learns
    // that docs moved while it was away and refetches.
    let a = app(users_config(), None);
    for _ in 0..3 {
        a.events.notify(DocKind::Shopping);
    }
    a.events.notify(DocKind::Todo);
    let mut s = Stream::open(&a).await;
    assert_eq!(s.initial().await, [json!({"doc": "todo", "version": 1}), json!({"doc": "fitness", "version": 0}), json!({"doc": "shopping", "version": 3})]);
}

#[tokio::test]
async fn a_slow_reader_that_falls_behind_is_told_everything_moved() {
    let a = app(users_config(), None);
    let mut s = Stream::open(&a).await;
    s.initial().await;
    for _ in 0..200 {
        a.events.notify(DocKind::Todo); // far more than the channel holds while nobody reads
    }
    let mut seen = vec![];
    while let Some(m) = s.next(QUIET).await {
        seen.push(m);
    }
    let last = |doc: &str| seen.iter().rev().find(|m| m["doc"] == doc).cloned();
    assert_eq!(last("todo").unwrap()["version"], 200, "the newest counter arrives");
    assert!(last("shopping").is_some() && last("fitness").is_some(), "a lag report covers every doc: {seen:?}");
}
