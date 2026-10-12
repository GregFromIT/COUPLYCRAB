//! Router tests; the harness is in `common/mod.rs`.

mod common;

use axum::http::{header, Method, StatusCode};
use common::*;
use fitness_core::AmFitnessStore;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc, Mutex};
use todo_core::AmTodoStore;
use web::{AppState, WebConfig};

#[tokio::test]
async fn every_endpoint_happy_path() {
    let a = dev();
    assert_eq!(a.get("/api/v1/health").await, (StatusCode::OK, json!({"status": "ok"})));
    assert_eq!(a.get("/api/v1/me").await.1["user_id"], "alice");

    // Todos
    assert_eq!(a.put("/api/v1/todos/t1", json!({"title": "milk", "due": "2026-10-01"})).await.0, StatusCode::CREATED);
    a.put("/api/v1/todos/t2", json!({"title": "bread", "notes": "rye", "due": "2026-10-20"})).await;
    a.put("/api/v1/todos/t3", json!({"title": "someday"})).await;
    let titles = |v: Value| v.as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(titles(a.get("/api/v1/todos").await.1), ["milk", "bread", "someday"]);
    assert_eq!(titles(a.get("/api/v1/todos?filter=overdue").await.1), ["milk"], "today is 2026-10-12 in Sydney");
    assert_eq!(titles(a.get("/api/v1/todos?filter=undated").await.1), ["someday"]);
    assert_eq!(titles(a.get("/api/v1/todos?filter=due_between&from=2026-10-12&to=2026-10-31").await.1), ["bread"]);
    let (s, t) = a.send(P, "/api/v1/todos/t2", json!({"title": "sourdough", "due": null})).await;
    assert_eq!((s, &t["title"], &t["notes"], &t["due"]), (StatusCode::OK, &json!("sourdough"), &json!("rye"), &Value::Null));
    assert_eq!(a.send(POST, "/api/v1/todos/t1/complete", json!({})).await.1["completed"], true);
    assert_eq!(titles(a.get("/api/v1/todos?filter=open").await.1), ["sourdough", "someday"]);
    assert_eq!(a.send(POST, "/api/v1/todos/t1/uncomplete", json!({})).await.1["completed"], false);
    assert_eq!(a.req(Method::DELETE, "/api/v1/todos/t3", None, &[]).await.0, StatusCode::NO_CONTENT);
    assert_eq!(a.get("/api/v1/todos").await.1.as_array().unwrap().len(), 2);
    assert_eq!(a.get("/api/v1/todos?filter=bogus").await.0, StatusCode::BAD_REQUEST);

    // Exercises
    assert_eq!(a.put("/api/v1/exercises/squat", json!({"name": "Squat", "measurement": "WeightReps"})).await.0, StatusCode::CREATED);
    a.put("/api/v1/exercises/plank", json!({"name": "Plank", "measurement": "Timed"})).await;
    assert_eq!(a.send(P, "/api/v1/exercises/squat", json!({"tempo": "3:1"})).await.1["tempo"], "3:1");
    assert_eq!(a.get("/api/v1/exercises").await.1.as_array().unwrap().len(), 2);

    // Templates
    let tpl = json!({"name": "Legs", "target_minutes": 45, "items": [
        {"exercise_id": "squat", "target_sets": 3, "target_reps": 5, "target_load": 100.0,
         "progression": {"kind": "LinearLoad", "per_week": 2.5}},
        {"exercise_id": "plank", "target_sets": 3}
    ]});
    assert_eq!(a.put("/api/v1/templates/legs", tpl.clone()).await.0, StatusCode::CREATED);
    assert_eq!(a.get("/api/v1/templates/legs").await.1["items"].as_object().unwrap().len(), 2);
    assert_eq!(a.send(P, "/api/v1/templates/legs", json!({"target_minutes": 50})).await.1["target_minutes"], 50);
    assert_eq!(a.get("/api/v1/templates").await.1.as_array().unwrap().len(), 1);
    let bad_item = json!({"name": "X", "target_minutes": 1, "items": [{"exercise_id": "nope", "target_sets": 1}]});
    assert_eq!(a.put("/api/v1/templates/x", bad_item).await.0, StatusCode::BAD_REQUEST);

    // Plan + block → planned day
    let plan = json!({"slots": {"mon_am": {"template_id": "legs"}, "mon_pm": {"external_label": "Unit PT"}}});
    assert_eq!(a.put("/api/v1/plan", plan).await.0, StatusCode::OK);
    assert_eq!(a.get("/api/v1/plan").await.1["slots"]["mon_pm"]["external_label"], "Unit PT");
    let block = json!({"name": "Base", "start_date": "2026-09-28", "weeks": 6, "deload_week": 6, "deload_pct": 60.0});
    assert_eq!(a.put("/api/v1/blocks/b1", block).await.0, StatusCode::CREATED);
    assert_eq!(a.send(P, "/api/v1/blocks/b1", json!({"name": "Base 1"})).await.1["name"], "Base 1");
    assert_eq!(a.get("/api/v1/blocks").await.1.as_array().unwrap().len(), 1);
    a.put("/api/v1/blocks/b2", json!({"name": "Next", "start_date": "2026-11-09", "weeks": 4})).await;

    let (s, day) = a.get("/api/v1/planned-day").await;
    assert_eq!((s, &day["date"]), (StatusCode::OK, &json!("2026-10-12")), "default date is today in Sydney");
    assert_eq!(day["block"]["week"], 3);
    assert_eq!(day["slots"][0]["template"]["items"][0]["target_load"], 105.0);
    assert_eq!(day["slots"][1]["external_label"], "Unit PT");
    let deload = a.get("/api/v1/planned-day?date=2026-11-02").await.1;
    assert_eq!((deload["block"]["is_deload"].clone(), deload["slots"][0]["template"]["items"][0]["target_load"].clone()), (json!(true), json!(67.5)), "week 6: (100 + 5 × 2.5) × 60 %");
    assert_eq!(a.get("/api/v1/planned-day?date=2026-13-01").await.0, StatusCode::BAD_REQUEST);

    // Sessions
    let sess = json!({"template_id": "legs", "block_id": "b1", "block_week": 3, "slot": "AM"});
    assert_eq!(a.put("/api/v1/sessions/s1", sess).await.0, StatusCode::CREATED);
    let set = json!({"exercise_id": "squat", "reps": 5, "load": 105.0});
    assert_eq!(a.put("/api/v1/sessions/s1/sets/set1", set).await.0, StatusCode::CREATED);
    let (s, done) = a.send(POST, "/api/v1/sessions/s1/finish", json!({"status": "Completed", "note": "good"})).await;
    assert_eq!((s, &done["status"], done["sets"].as_object().unwrap().len()), (StatusCode::OK, &json!("Completed"), 1));
    assert_eq!(a.get("/api/v1/sessions/s1").await.1["block_week"], 3);
    assert_eq!(a.get("/api/v1/sessions?since=2026-10-01&limit=10").await.1.as_array().unwrap().len(), 1);
    assert_eq!(a.get("/api/v1/sessions?since=2099-01-01").await.1.as_array().unwrap().len(), 0);
    let st = a.get("/api/v1/program-status").await.1;
    assert_eq!((st["block"]["name"].clone(), st["sessions_this_week"].clone(), st["next_block"]["name"].clone()), (json!("Base 1"), json!(1), json!("Next")));

    // Tests
    assert_eq!(a.put("/api/v1/tests/r1", json!({"kind": "BeepTest", "date": "2026-10-01", "beep": "8.10"})).await.0, StatusCode::CREATED);
    a.put("/api/v1/tests/r2", json!({"kind": "BeepTest", "date": "2026-10-08", "beep": "9.1"})).await;
    a.put("/api/v1/tests/r3", json!({"kind": "TimedRun", "date": "2026-10-08", "value": 600})).await;
    let beeps = a.get("/api/v1/tests?kind=BeepTest").await.1;
    assert_eq!(beeps.as_array().unwrap().iter().map(|t| t["score"].as_str().unwrap()).collect::<Vec<_>>(), ["9.1", "8.10"]);
    assert!((beeps[1]["plot_value"].as_f64().unwrap() - (8.0 + 10.0 / 11.0)).abs() < 1e-9);
    assert_eq!(a.get("/api/v1/tests").await.1.as_array().unwrap().len(), 3);
    assert_eq!(a.get("/api/v1/tests?since=2026-10-05").await.1.as_array().unwrap().len(), 2);
    assert_eq!(a.put("/api/v1/tests/r4", json!({"kind": "BeepTest", "date": "2026-10-08", "beep": "8.12"})).await.0, StatusCode::BAD_REQUEST);

    // External activities
    let ext = json!({"source": "garmin", "source_id": "42", "started_at": 1791700000000_i64, "duration_s": 1800, "distance_m": 5000.0});
    assert_eq!(a.put("/api/v1/external-activities", ext.clone()).await.0, StatusCode::CREATED);
    assert_eq!(a.put("/api/v1/external-activities", ext).await.0, StatusCode::OK);

    // Unknown endpoint: JSON 404.
    let (s, v) = a.get("/api/v1/nope").await;
    assert_eq!((s, &v["error"]["code"]), (StatusCode::NOT_FOUND, &json!("not_found")));
    assert!(a.saves.load(Ordering::SeqCst) > 20, "every write persisted");
}

#[tokio::test]
async fn repeated_put_is_a_no_op() {
    let a = dev();
    let todo = json!({"title": "milk", "notes": "", "due": null});
    assert_eq!(a.put("/api/v1/todos/t1", todo.clone()).await.0, StatusCode::CREATED);
    let heads = a.todo.lock().unwrap().doc.get_heads();
    let (s, body) = a.put("/api/v1/todos/t1", todo).await;
    assert_eq!((s, &body["title"]), (StatusCode::OK, &json!("milk")));
    assert_eq!(a.todo.lock().unwrap().doc.get_heads(), heads, "no new Automerge change");
    let (s, body) = a.put("/api/v1/todos/t1", json!({"title": "bread"})).await;
    assert_eq!((s, &body["error"]["code"]), (StatusCode::CONFLICT, &json!("conflict")));

    // The mid-workout retry case: same set PUT twice → one set.
    a.put("/api/v1/exercises/e", json!({"name": "Row", "measurement": "Reps"})).await;
    a.put("/api/v1/sessions/s1", json!({})).await;
    let fit_heads = || a.fit.lock().unwrap().doc.get_heads();
    let set = json!({"exercise_id": "e", "reps": 12});
    assert_eq!(a.put("/api/v1/sessions/s1/sets/x", set.clone()).await.0, StatusCode::CREATED);
    let h = fit_heads();
    assert_eq!(a.put("/api/v1/sessions/s1/sets/x", set).await.0, StatusCode::OK);
    assert_eq!(a.put("/api/v1/sessions/s1", json!({})).await.0, StatusCode::OK);
    assert_eq!(fit_heads(), h);
    assert_eq!(a.get("/api/v1/sessions/s1").await.1["sets"].as_object().unwrap().len(), 1);
}

#[tokio::test]
async fn writing_other_users_data_is_403() {
    let a = app(users_config(), None);
    let (alice, bob) = ("alice@example.com", "bob@example.com");
    a.as_user(alice, Method::PUT, "/api/v1/exercises/e", Some(json!({"name": "Row", "measurement": "Reps"}))).await;
    a.as_user(alice, Method::PUT, "/api/v1/sessions/s1", Some(json!({}))).await;
    a.as_user(alice, Method::PUT, "/api/v1/blocks/b1", Some(json!({"name": "A", "start_date": "2026-10-05", "weeks": 4}))).await;
    a.as_user(alice, Method::PUT, "/api/v1/tests/r1", Some(json!({"kind": "MaxReps", "date": "2026-10-05", "value": 20}))).await;

    let forbidden = [
        (Method::PUT, "/api/v1/sessions/s1/sets/x", json!({"exercise_id": "e", "reps": 1})),
        (Method::POST, "/api/v1/sessions/s1/finish", json!({"status": "Completed"})),
        (Method::PUT, "/api/v1/sessions/s1", json!({})),
        (Method::PATCH, "/api/v1/blocks/b1", json!({"name": "mine now"})),
        (Method::PUT, "/api/v1/blocks/b1", json!({"name": "A", "start_date": "2026-10-05", "weeks": 4})),
        (Method::PUT, "/api/v1/tests/r1", json!({"kind": "MaxReps", "date": "2026-10-05", "value": 20})),
    ];
    for (m, uri, body) in forbidden {
        assert_eq!(a.as_user(bob, m.clone(), uri, Some(body)).await.0, StatusCode::FORBIDDEN, "{m} {uri}");
    }
    // Reading the other user's data is allowed with ?user=.
    let (s, v) = a.as_user(bob, Method::GET, "/api/v1/blocks?user=alice", None).await;
    assert_eq!((s, v.as_array().unwrap().len()), (StatusCode::OK, 1));
    assert_eq!(a.as_user(bob, Method::GET, "/api/v1/blocks", None).await.1, json!([]));
}

#[tokio::test]
async fn writes_need_json_content_type() {
    let a = dev();
    let (s, v, _) = a.req(Method::PUT, "/api/v1/todos/t1", Some(json!({"title": "x"})), &[("content-type", "text/plain")]).await;
    assert_eq!((s, &v["error"]["code"]), (StatusCode::UNSUPPORTED_MEDIA_TYPE, &json!("unsupported_media_type")));
    // A form post (what a foreign page can send without preflight) is refused before any parsing.
    let (s, ..) = a.req(Method::POST, "/api/v1/todos/t1/complete", None, &[("content-type", "application/x-www-form-urlencoded")]).await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (s, ..) = a.req(Method::PUT, "/api/v1/todos/t1", Some(json!({"title": "x"})), &[("content-type", "application/json; charset=utf-8")]).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(a.req(Method::GET, "/api/v1/todos", None, &[]).await.0, StatusCode::OK, "reads don't need it");
}

#[tokio::test]
async fn foreign_origin_is_rejected() {
    let a = dev();
    let body = Some(json!({"title": "x"}));
    let (s, v, h) = a.req(Method::PUT, "/api/v1/todos/t1", body.clone(), &[("origin", "https://evil.example")]).await;
    assert_eq!((s, &v["error"]["code"]), (StatusCode::FORBIDDEN, &json!("bad_origin")));
    assert!(h.keys().all(|k| !k.as_str().starts_with("access-control-")), "no CORS headers");
    let (s, ..) = a.req(Method::PUT, "/api/v1/todos/t1", body.clone(), &[("origin", "http://127.0.0.1:7430")]).await;
    assert_eq!(s, StatusCode::CREATED);
    // Configured origin (behind tailscale serve) replaces the defaults.
    let b = app(WebConfig { origins: vec!["https://host.tail1234.ts.net".into()], ..Default::default() }, Some("alice"));
    assert_eq!(b.req(Method::PUT, "/api/v1/todos/t1", body.clone(), &[("origin", "https://host.tail1234.ts.net")]).await.0, StatusCode::CREATED);
    assert_eq!(b.req(Method::PUT, "/api/v1/todos/t2", body, &[("origin", "http://127.0.0.1:7430")]).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn oversized_body_is_413() {
    let a = dev();
    let big = "x".repeat(70 * 1024);
    let (s, v) = a.put("/api/v1/todos/t1", json!({"title": big})).await;
    assert_eq!((s, &v["error"]["code"]), (StatusCode::PAYLOAD_TOO_LARGE, &json!("too_large")));
    // Under the body limit but over the string cap: 400.
    let (s, _) = a.put("/api/v1/todos/t1", json!({"title": "x".repeat(201)})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = a.put("/api/v1/todos/t1", json!({"title": "ok", "notes": "n".repeat(2001)})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn identity_header_maps_to_user() {
    let a = app(users_config(), None);
    for (login, user) in [
        ("alice@example.com", "alice"),
        ("bob@example.com", "bob"),
        // Non-ASCII login arrives RFC 2047 encoded and is decoded before matching.
        ("=?utf-8?q?Zo=C3=AB@example.com?=", "zoe"),
    ] {
        assert_eq!(a.as_user(login, Method::GET, "/api/v1/me", None).await.1["user_id"], user);
    }
    let (s, v) = a.as_user("bob@example.com", Method::PUT, "/api/v1/sessions/s1", Some(json!({}))).await;
    assert_eq!((s, &v["user_id"]), (StatusCode::CREATED, &json!("bob")));
}

#[tokio::test]
async fn unmapped_login_uses_picker_cookie() {
    let a = app(users_config(), None);
    let stranger = [("Tailscale-User-Login", "carol@example.com")];
    let (_, v, _) = a.req(Method::GET, "/api/v1/me", None, &stranger).await;
    assert_eq!(v["user_id"], Value::Null);
    assert_eq!(v["users"], json!(["alice", "bob", "zoe"]), "picker offers every known user");
    let (s, v, _) = a.req(Method::GET, "/api/v1/todos", None, &stranger).await;
    assert_eq!((s, &v["error"]["code"]), (StatusCode::UNAUTHORIZED, &json!("no_user")));

    let (s, _, h) = a.req(Method::PUT, "/api/v1/me", Some(json!({"user_id": "bob"})), &stranger).await;
    assert_eq!(s, StatusCode::OK);
    let cookie = h[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("user=bob;") && cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"), "{cookie}");
    let with_cookie = [("Tailscale-User-Login", "carol@example.com"), ("cookie", "user=bob")];
    assert_eq!(a.req(Method::GET, "/api/v1/me", None, &with_cookie).await.1["user_id"], "bob");
    assert_eq!(a.req(Method::GET, "/api/v1/todos", None, &with_cookie).await.0, StatusCode::OK);
    // Picking someone who isn't a user fails; so does a forged cookie.
    assert_eq!(a.req(Method::PUT, "/api/v1/me", Some(json!({"user_id": "mallory"})), &stranger).await.0, StatusCode::FORBIDDEN);
    assert_eq!(a.req(Method::GET, "/api/v1/me", None, &[("cookie", "user=mallory")]).await.1["user_id"], Value::Null);

    // Shared login: picker limited to that login's users; a mapped single login ignores the cookie.
    let shared = [("Tailscale-User-Login", "shared@example.com"), ("cookie", "user=zoe")];
    let v = a.req(Method::GET, "/api/v1/me", None, &shared).await.1;
    assert_eq!((v["user_id"].clone(), v["users"].clone()), (Value::Null, json!(["alice", "bob"])));
    let shared = [("Tailscale-User-Login", "shared@example.com"), ("cookie", "user=bob")];
    assert_eq!(a.req(Method::GET, "/api/v1/me", None, &shared).await.1["user_id"], "bob");
    let alice_with_bob_cookie = [("Tailscale-User-Login", "alice@example.com"), ("cookie", "user=bob")];
    assert_eq!(a.req(Method::GET, "/api/v1/me", None, &alice_with_bob_cookie).await.1["user_id"], "alice");
}

#[tokio::test]
async fn planned_day_uses_configured_timezone() {
    // Same instant (2026-10-11 14:30 UTC), two timezones.
    let sydney = dev();
    let utc = app(WebConfig::default(), Some("alice"));
    assert_eq!(sydney.get("/api/v1/planned-day").await.1["date"], "2026-10-12");
    assert_eq!(utc.get("/api/v1/planned-day").await.1["date"], "2026-10-11");
    assert_eq!(sydney.get("/api/v1/program-status").await.1["date"], "2026-10-12");
}

#[test]
fn config_must_be_loopback_and_timezone_valid() {
    let mk = |web: WebConfig| {
        AppState::new(
            Arc::new(Mutex::new(AmTodoStore::new())),
            Arc::new(Mutex::new(AmFitnessStore::new())),
            Arc::new(|_| Ok(())),
            web,
            "alice",
            None,
        )
    };
    assert!(mk(WebConfig { listen: "0.0.0.0:7430".parse().unwrap(), ..Default::default() }).is_err());
    assert!(mk(WebConfig { listen: "100.64.1.2:7430".parse().unwrap(), ..Default::default() }).is_err());
    assert!(mk(WebConfig { timezone: "Mars/Olympus".into(), ..Default::default() }).is_err());
    assert!(mk(WebConfig::default()).is_ok());
}
