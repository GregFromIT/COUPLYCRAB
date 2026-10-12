//! v2 training API: gym profiles, programs, next workout, recommendations, analytics, 410s.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use fitness_core::{FitnessStore, Session, SessionStatus, SetLog};
use serde_json::{json, Value};

/// 2099-01-01 12:00 UTC. Program weeks in these tests are in 2099 so that sessions made
/// through the API (stamped with the real clock) never fall on a program day.
const JAN_1_2099: i64 = 4_070_952_000_000;

/// A finished session for `user` with `(exercise, load, reps, rir)` sets, written straight
/// into the store so `started_at` is controlled.
fn seed(a: &App, user: &str, id: &str, started_at: i64, sets: &[(&str, f64, u32, u8)]) {
    let mut f = a.fit.lock().unwrap();
    let mut s = Session::start(id.into(), user);
    s.started_at = started_at;
    f.put_session(s).unwrap();
    for (i, (ex, load, reps, rir)) in sets.iter().enumerate() {
        let set = SetLog { exercise_id: ex.to_string(), load: Some(*load), reps: Some(*reps), rir: Some(*rir), ..Default::default() };
        f.put_set(user, id, &format!("{id}-{i}"), set).unwrap();
    }
    f.finish_session(user, id, SessionStatus::Completed, None).unwrap();
}

fn program_body() -> Value {
    json!({
        "name": "Upper/Lower", "start_date": "2099-01-05", "weeks": 4, "gym_profile_id": "g1",
        "workouts": [
            {"id": "w1", "name": "Upper A", "slot": "mon_am", "items": [
                {"id": "i1", "exercise_id": "bench", "sets": 3, "rep_min": 7, "rep_max": 9, "rir": 2, "rest_s": 150},
                {"id": "i2", "exercise_id": "row", "sets": 3, "rep_min": 8, "rep_max": 12}
            ]},
            {"id": "w2", "name": "Lower", "items": [{"id": "i3", "exercise_id": "squat", "sets": 4, "rep_min": 5, "rep_max": 8, "style": "ToFailure"}]},
            {"id": "pt", "name": "Unit PT", "slot": "wed_pm", "external_label": "Unit PT"}
        ],
        "overrides": [{"week": 4, "item_id": "i1", "sets": 2, "rir": 4}]
    })
}

async fn alice(a: &App, m: Method, uri: &str, b: Option<Value>) -> (StatusCode, Value) {
    a.as_user("alice@example.com", m, uri, b).await
}

async fn bob(a: &App, m: Method, uri: &str, b: Option<Value>) -> (StatusCode, Value) {
    a.as_user("bob@example.com", m, uri, b).await
}

fn gym_body() -> Value {
    json!({"name": "Base gym", "bar_kg": 20.0, "plates_kg": [1.25, 2.5, 5.0, 10.0, 20.0], "dumbbell_step_kg": 2.0, "machine_step_kg": 5.0, "is_default": true})
}

/// Exercises with muscles/equipment, and the default gym `g1`.
async fn training_setup(a: &App) {
    let ex = |name: &str, primary: Value, secondary: Value, eq: &str| {
        json!({"name": name, "measurement": "WeightReps", "primary_muscles": primary, "secondary_muscles": secondary, "equipment": eq, "default_rest_s": 150})
    };
    assert_eq!(a.put("/api/v1/exercises/bench", ex("Bench", json!(["chest"]), json!(["triceps"]), "Barbell")).await.0, StatusCode::CREATED);
    a.put("/api/v1/exercises/row", ex("Row", json!(["back"]), json!(["biceps"]), "Cable")).await;
    a.put("/api/v1/exercises/squat", ex("Squat", json!(["quads", "glutes"]), json!([]), "Barbell")).await;
    assert_eq!(a.put("/api/v1/gym-profiles/g1", gym_body()).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn gym_profiles_and_exercise_fields() {
    let a = dev();
    training_setup(&a).await;
    let e = a.get("/api/v1/exercises").await.1;
    let bench = e.as_array().unwrap().iter().find(|x| x["id"] == "bench").unwrap().clone();
    assert_eq!((&bench["primary_muscles"], &bench["equipment"], &bench["default_rest_s"]), (&json!(["chest"]), &json!("Barbell"), &json!(150)));
    let (s, v) = a.send(P, "/api/v1/exercises/bench", json!({"secondary_muscles": ["triceps", "shoulders"], "default_rest_s": null})).await;
    assert_eq!((s, &v["secondary_muscles"], &v["default_rest_s"]), (StatusCode::OK, &json!(["triceps", "shoulders"]), &Value::Null));
    assert_eq!(a.send(P, "/api/v1/exercises/bench", json!({"primary_muscles": ["pecs"]})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(a.send(P, "/api/v1/exercises/bench", json!({"equipment": "Kettlebell"})).await.0, StatusCode::BAD_REQUEST);

    // Profiles: validation, idempotent PUT, one default at a time.
    let home = json!({"name": "Home", "bar_kg": 15.0, "plates_kg": [2.5, 5.0], "dumbbell_step_kg": 2.5, "machine_step_kg": 5.0, "is_default": true});
    assert_eq!(a.put("/api/v1/gym-profiles/g2", home.clone()).await.0, StatusCode::CREATED);
    assert_eq!(a.put("/api/v1/gym-profiles/g2", home).await.0, StatusCode::OK);
    let names = |v: Value| v.as_array().unwrap().iter().map(|g| (g["name"].as_str().unwrap().to_string(), g["is_default"].as_bool().unwrap())).collect::<Vec<_>>();
    assert_eq!(names(a.get("/api/v1/gym-profiles").await.1), [("Home".to_string(), true), ("Base gym".to_string(), false)]);
    let (s, v) = a.send(P, "/api/v1/gym-profiles/g1", json!({"is_default": true, "plates_kg": [1.25, 2.5]})).await;
    assert_eq!((s, &v["plates_kg"]), (StatusCode::OK, &json!([1.25, 2.5])));
    assert_eq!(names(a.get("/api/v1/gym-profiles").await.1)[0], ("Base gym".to_string(), true));
    for bad in [json!({"bar_kg": 0}), json!({"plates_kg": [-1.0]}), json!({"machine_step_kg": 5000.0}), json!({"name": " "})] {
        assert_eq!(a.send(P, "/api/v1/gym-profiles/g1", bad).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(a.send(P, "/api/v1/gym-profiles/nope", json!({"name": "x"})).await.0, StatusCode::NOT_FOUND);
    a.send(P, "/api/v1/gym-profiles/g2", json!({"deleted": true})).await;
    assert_eq!(a.get("/api/v1/gym-profiles").await.1.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn programs_workouts_recommendations_and_analytics() {
    let a = dev();
    training_setup(&a).await;
    let (s, p) = a.put("/api/v1/programs/p1", program_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    assert_eq!(p["workouts"].as_array().unwrap().len(), 3);
    assert_eq!((p["workouts"][0]["items"][1]["rir"].clone(), p["workouts"][1]["items"][0]["style"].clone()), (json!(2), json!("ToFailure")));
    assert_eq!(p["workouts"][0]["items"][0]["rest_s"], 150);
    assert_eq!((p["overrides"][0]["week"].clone(), p["overrides"][0]["rir"].clone(), p["warmups"].clone()), (json!(4), json!(4), json!(true)));
    assert_eq!(a.put("/api/v1/programs/p1", program_body()).await.0, StatusCode::OK, "same body: no-op");
    assert_eq!(a.get("/api/v1/programs").await.1.as_array().unwrap().len(), 1);
    assert_eq!(a.get("/api/v1/programs/p1").await.1["name"], "Upper/Lower");

    // No history yet: prescriptions only. 2099-01-05 is a Monday, week 1.
    let n = a.get("/api/v1/workouts/next?date=2099-01-05").await.1;
    assert_eq!((n["program_id"].clone(), n["week"].clone(), n["weeks"].clone(), n["reason"].clone()), (json!("p1"), json!(1), json!(4), json!("Pinned for Monday AM.")));
    assert_eq!(n["workout"]["workout_id"], "w1");
    assert_eq!(n["workout"]["items"][0]["recommendation"], Value::Null);
    assert_eq!(n["workout"]["items"][0]["prescription"]["rep_max"], 9);
    // Wednesday: PT is external and nothing is pinned for lifting, so the unpinned workout rotates in.
    let wed = a.get("/api/v1/workouts/next?date=2099-01-07").await.1;
    assert_eq!(
        (wed["externals"][0]["label"].clone(), wed["externals"][0]["done"].clone(), wed["workout"]["workout_id"].clone()),
        (json!("Unit PT"), json!(false), json!("w2"))
    );
    assert_eq!(a.get("/api/v1/workouts/next?date=2099-01-06").await.1["reason"], "First workout of the program: Lower.");
    // The mocked "today" (2026-10-12) is outside the program.
    let today = a.get("/api/v1/workouts/next").await.1;
    assert_eq!((today["program_id"].clone(), today["workout"].clone()), (Value::Null, Value::Null));

    // History: 80 kg × 10 @ 2 RIR on bench beats 7–9 @ 2 → 82.5 kg; the reason says why.
    seed(&a, "alice", "h1", JAN_1_2099, &[("bench", 80.0, 10, 2), ("squat", 100.0, 10, 2)]);
    let n = a.get("/api/v1/workouts/next?date=2099-01-12").await.1;
    let rec = &n["workout"]["items"][0]["recommendation"];
    assert_eq!((rec["load_kg"].clone(), rec["reps"].clone(), rec["rir"].clone()), (json!(82.5), json!(8), json!(2)));
    assert!(rec["reason"].as_str().unwrap().contains("above the 7–9 @ 2 RIR target"), "{rec}");
    assert_eq!(n["workout"]["items"][0]["warmups"][0], json!({"load_kg": 32.5, "reps": 8}));
    assert_eq!(n["workout"]["items"][1]["warmups"], json!([]), "row has no history; no warm-ups");
    // Week 4 is the deload from periodization: 2 sets at RIR 4, so a lighter load.
    let w4 = a.get("/api/v1/recommendations?program_id=p1&workout_id=w1&date=2099-01-26").await.1;
    assert_eq!(
        (w4["week"].clone(), w4["items"][0]["prescription"]["sets"].clone(), w4["items"][0]["recommendation"]["load_kg"].clone()),
        (json!(4), json!(2), json!(80.0))
    );
    assert_eq!(a.get("/api/v1/recommendations?program_id=p1&workout_id=w1&date=2099-01-12").await.1["items"][0]["recommendation"], *rec);

    // Analytics.
    let v = a.get("/api/v1/analytics/volume?week=2099-01-01&weeks=2").await.1;
    let weeks = v["weeks"].as_array().unwrap();
    assert_eq!((weeks[0]["week_start"].clone(), weeks[1]["week_start"].clone()), (json!("2098-12-22"), json!("2098-12-29")));
    assert_eq!(
        (weeks[1]["sets"]["chest"].clone(), weeks[1]["sets"]["triceps"].clone(), weeks[1]["sets"]["quads"].clone(), weeks[0]["sets"]["chest"].clone()),
        (json!(1.0), json!(0.5), json!(1.0), json!(0.0))
    );
    assert_eq!(weeks[1]["sets"].as_object().unwrap().len(), 11);
    let e = a.get("/api/v1/analytics/e1rm?exercise_id=bench").await.1;
    assert_eq!((e["points"].as_array().unwrap().len(), e["points"][0]["load_kg"].clone()), (1, json!(80.0)));
    assert!((e["points"][0]["e1rm"].as_f64().unwrap() - 80.0 * (1.0 + 12.0 / 30.0)).abs() < 1e-9);
    let prs = a.get("/api/v1/prs?exercise_id=bench").await.1;
    assert_eq!(prs.as_array().unwrap().iter().map(|p| p["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["e1rm", "reps"]);
    assert_eq!(a.get("/api/v1/prs").await.1.as_array().unwrap().len(), 4, "bench and squat: an e1RM and a rep PR each");
}

#[tokio::test]
async fn program_validation_and_ownership() {
    let a = app(users_config(), None);
    for (id, name, mus) in [("bench", "Bench", "chest"), ("row", "Row", "back"), ("squat", "Squat", "quads")] {
        let body = json!({"name": name, "measurement": "WeightReps", "primary_muscles": [mus], "equipment": "Barbell"});
        alice(&a, Method::PUT, &format!("/api/v1/exercises/{id}"), Some(body)).await;
    }
    assert_eq!(alice(&a, Method::PUT, "/api/v1/gym-profiles/g1", Some(gym_body())).await.0, StatusCode::CREATED);
    assert_eq!(alice(&a, Method::PUT, "/api/v1/programs/p1", Some(program_body())).await.0, StatusCode::CREATED);

    // Different content under the same ID: 409. Bob can't touch or take over alice's data.
    let mut changed = program_body();
    changed["name"] = json!("Other");
    assert_eq!(alice(&a, Method::PUT, "/api/v1/programs/p1", Some(changed.clone())).await.0, StatusCode::CONFLICT);
    assert_eq!(bob(&a, Method::PUT, "/api/v1/programs/p1", Some(changed)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob(&a, Method::PATCH, "/api/v1/programs/p1", Some(json!({"name": "mine"}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob(&a, Method::PATCH, "/api/v1/gym-profiles/g1", Some(json!({"name": "mine"}))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob(&a, Method::PUT, "/api/v1/gym-profiles/g1", Some(gym_body())).await.0, StatusCode::FORBIDDEN);
    // Bob can read alice's program, and its recommendations with ?user=; his own list is empty.
    assert_eq!(bob(&a, Method::GET, "/api/v1/programs/p1", None).await.0, StatusCode::OK);
    assert_eq!(bob(&a, Method::GET, "/api/v1/programs", None).await.1, json!([]));
    assert_eq!(bob(&a, Method::GET, "/api/v1/programs?user=alice", None).await.1.as_array().unwrap().len(), 1);
    let rec = "/api/v1/recommendations?program_id=p1&workout_id=w1&date=2099-01-05";
    assert_eq!(bob(&a, Method::GET, rec, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(bob(&a, Method::GET, &format!("{rec}&user=alice"), None).await.0, StatusCode::OK);
    // Bob can't start a session against alice's program.
    assert_eq!(bob(&a, Method::PUT, "/api/v1/sessions/bs", Some(json!({"program_id": "p1", "workout_id": "w1"}))).await.0, StatusCode::FORBIDDEN);

    // Bad programs.
    let with = |f: &dyn Fn(&mut Value)| {
        let mut b = program_body();
        f(&mut b);
        b
    };
    let bad = [
        ("start not a Monday", with(&|b| b["start_date"] = json!("2099-01-06"))),
        ("zero weeks", with(&|b| b["weeks"] = json!(0))),
        ("unknown exercise", with(&|b| b["workouts"][0]["items"][0]["exercise_id"] = json!("nope"))),
        ("unknown gym", with(&|b| b["gym_profile_id"] = json!("nope"))),
        ("duplicate item id", with(&|b| b["workouts"][0]["items"][1]["id"] = json!("i1"))),
        ("duplicate workout id", with(&|b| b["workouts"][1]["id"] = json!("w1"))),
        ("min > max", with(&|b| b["workouts"][0]["items"][0]["rep_min"] = json!(12))),
        ("rir 6", with(&|b| b["workouts"][0]["items"][0]["rir"] = json!(6))),
        ("bad style", with(&|b| b["workouts"][0]["items"][0]["style"] = json!("Giant"))),
        ("bad slot", with(&|b| b["workouts"][0]["slot"] = json!("mon_noon"))),
        ("external with items", with(&|b| b["workouts"][2]["items"] = json!([{"id": "i9", "exercise_id": "bench", "sets": 1, "rep_min": 1, "rep_max": 1}]))),
        ("override week 5", with(&|b| b["overrides"][0]["week"] = json!(5))),
        ("override unknown item", with(&|b| b["overrides"][0]["item_id"] = json!("zzz"))),
        ("override twice", with(&|b| b["overrides"] = json!([{"week": 2, "item_id": "i1", "sets": 1}, {"week": 2, "item_id": "i1", "sets": 2}]))),
        ("bad id", with(&|b| b["workouts"][0]["id"] = json!("a b"))),
    ];
    for (what, body) in bad {
        let (s, v) = alice(&a, Method::PUT, "/api/v1/programs/bad", Some(body)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{what}: {v}");
    }
    assert_eq!(alice(&a, Method::GET, "/api/v1/programs/bad", None).await.0, StatusCode::NOT_FOUND, "nothing was written");

    // PATCH: replace workouts and overrides, clear the gym, then soft delete.
    let patch = json!({"gym_profile_id": null, "warmups": false, "overrides": [], "workouts": [
        {"id": "w9", "name": "Full body", "slot": "fri_am", "items": [{"id": "n1", "exercise_id": "squat", "sets": 3, "rep_min": 5, "rep_max": 5}]}]});
    let (s, v) = alice(&a, Method::PATCH, "/api/v1/programs/p1", Some(patch)).await;
    assert_eq!(
        (s, v["workouts"].as_array().unwrap().len(), v["overrides"].clone(), v["gym_profile_id"].clone(), v["warmups"].clone()),
        (StatusCode::OK, 1, json!([]), Value::Null, json!(false))
    );
    assert_eq!(alice(&a, Method::PATCH, "/api/v1/programs/p1", Some(json!({"deleted": true}))).await.0, StatusCode::OK);
    assert_eq!(alice(&a, Method::GET, "/api/v1/programs", None).await.1, json!([]));
    assert_eq!(alice(&a, Method::PATCH, "/api/v1/programs/nope", Some(json!({}))).await.0, StatusCode::NOT_FOUND);

    // Query validation.
    for uri in [
        "/api/v1/recommendations?workout_id=w1",
        "/api/v1/analytics/e1rm",
        "/api/v1/analytics/volume?weeks=0",
        "/api/v1/analytics/volume?weeks=27",
        "/api/v1/analytics/volume?week=2099-13-01",
        "/api/v1/workouts/next?date=x",
    ] {
        assert_eq!(alice(&a, Method::GET, uri, None).await.0, StatusCode::BAD_REQUEST, "{uri}");
    }
    assert_eq!(alice(&a, Method::GET, "/api/v1/recommendations?program_id=p1&workout_id=nope&date=2099-01-05", None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn set_fields_are_validated_and_roundtrip() {
    let a = dev();
    training_setup(&a).await;
    a.put("/api/v1/sessions/s1", json!({})).await;
    let set = json!({"exercise_id": "bench", "reps": 8, "load": 80.0, "kind": "Failure", "rir": 0, "partials": 3, "rest_taken_s": 150, "recommendation_id": "bench@82.5x8@2"});
    let (s, v) = a.put("/api/v1/sessions/s1/sets/x1", set.clone()).await;
    assert_eq!(s, StatusCode::CREATED);
    let got = &v["sets"]["x1"];
    assert_eq!(
        (&got["kind"], &got["rir"], &got["partials"], &got["rest_taken_s"], &got["recommendation_id"]),
        (&json!("Failure"), &json!(0), &json!(3), &json!(150), &json!("bench@82.5x8@2"))
    );
    assert_eq!(a.put("/api/v1/sessions/s1/sets/x1", set.clone()).await.0, StatusCode::OK, "retry is a no-op");
    let mut other = set.clone();
    other["rir"] = json!(1);
    assert_eq!(a.put("/api/v1/sessions/s1/sets/x1", other).await.0, StatusCode::CONFLICT);
    for (k, v) in [("kind", json!("Heroic")), ("rir", json!(11)), ("partials", json!(51)), ("rest_taken_s", json!(7201))] {
        let mut b = set.clone();
        b[k] = v;
        assert_eq!(a.put("/api/v1/sessions/s1/sets/x2", b).await.0, StatusCode::BAD_REQUEST, "{k}");
    }
    // A set without the v2 fields still works (v1 clients).
    assert_eq!(a.put("/api/v1/sessions/s1/sets/x3", json!({"exercise_id": "bench", "reps": 5, "load": 60.0})).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn session_from_a_program_workout_and_attending_an_external_session() {
    let a = dev();
    training_setup(&a).await;
    a.put("/api/v1/programs/p1", program_body()).await;
    let (s, v) = a.put("/api/v1/sessions/s1", json!({"program_id": "p1", "workout_id": "w1"})).await;
    assert_eq!((s, v["program_id"].clone(), v["workout_id"].clone()), (StatusCode::CREATED, json!("p1"), json!("w1")));
    assert_eq!(a.put("/api/v1/sessions/s1", json!({"program_id": "p1", "workout_id": "w1"})).await.0, StatusCode::OK);
    assert_eq!(a.put("/api/v1/sessions/s1", json!({"program_id": "p1", "workout_id": "w2"})).await.0, StatusCode::CONFLICT, "different content");
    assert_eq!(a.put("/api/v1/sessions/s2", json!({"program_id": "nope"})).await.0, StatusCode::NOT_FOUND);
    // "Attended" for the external Unit PT slot: a session with the label, finished.
    a.put("/api/v1/sessions/pt1", json!({"program_id": "p1", "workout_id": "pt", "label": "Unit PT"})).await;
    let (s, v) = a.send(POST, "/api/v1/sessions/pt1/finish", json!({"status": "Completed", "note": "legs day"})).await;
    assert_eq!((s, v["label"].clone(), v["status"].clone()), (StatusCode::OK, json!("Unit PT"), json!("Completed")));
}

#[tokio::test]
async fn v1_endpoints_are_gone_once_programs_exist_and_migration_moves_data() {
    let a = dev();
    // v1 data through the v1 API.
    a.put("/api/v1/exercises/squat", json!({"name": "Squat", "measurement": "WeightReps"})).await;
    let tpl = json!({"name": "Legs", "target_minutes": 45, "items": [
        {"exercise_id": "squat", "target_sets": 3, "target_reps": 5, "target_load": 100.0, "progression": {"kind": "LinearLoad", "per_week": 2.5}}]});
    assert_eq!(a.put("/api/v1/templates/legs", tpl).await.0, StatusCode::CREATED);
    a.put("/api/v1/plan", json!({"slots": {"mon_am": {"template_id": "legs"}}})).await;
    let block = json!({"name": "Base", "start_date": "2099-01-05", "weeks": 6, "deload_week": 6, "deload_pct": 60.0});
    assert_eq!(a.put("/api/v1/blocks/b1", block).await.0, StatusCode::CREATED);
    assert_eq!(a.get("/api/v1/templates").await.0, StatusCode::OK, "v1 keeps working until programs exist");

    let (s, m) = a.send(POST, "/api/v1/programs/migrate-v1", json!({})).await;
    assert_eq!((s, m["created"].clone()), (StatusCode::OK, json!(1)));
    assert!(m["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("linear progression dropped")));
    let p = a.get("/api/v1/programs/v1-b1").await.1;
    assert_eq!(
        (p["name"].clone(), p["migrated_from"].clone(), p["workouts"][0]["slot"].clone(), p["workouts"][0]["items"][0]["rep_min"].clone()),
        (json!("Base"), json!("b1"), json!("mon_am"), json!(5))
    );
    assert_eq!(p["overrides"][0]["rir"], 4);
    assert_eq!(a.send(POST, "/api/v1/programs/migrate-v1", json!({})).await.1, json!({"created": 0, "notes": []}), "idempotent");

    // Now the replaced endpoints answer 410 and point to /programs.
    for (m, uri) in [
        (Method::GET, "/api/v1/templates"),
        (Method::GET, "/api/v1/templates/legs"),
        (Method::PUT, "/api/v1/templates/legs"),
        (Method::PATCH, "/api/v1/templates/legs"),
        (Method::GET, "/api/v1/plan"),
        (Method::PUT, "/api/v1/plan"),
        (Method::GET, "/api/v1/blocks"),
        (Method::PUT, "/api/v1/blocks/b1"),
        (Method::PATCH, "/api/v1/blocks/b1"),
        (Method::GET, "/api/v1/planned-day"),
        (Method::GET, "/api/v1/program-status"),
    ] {
        let body = (m != Method::GET).then(|| json!({}));
        let (s, v, _) = a.req(m.clone(), uri, body, &[]).await;
        assert_eq!((s, v["error"]["code"].clone(), v["replacement"].clone()), (StatusCode::GONE, json!("gone"), json!("/api/v1/programs")), "{m} {uri}");
    }
    // Exercises, sessions, tests and todos are untouched.
    assert_eq!(a.get("/api/v1/exercises").await.0, StatusCode::OK);
    assert_eq!(a.put("/api/v1/sessions/s1", json!({"label": "Walk"})).await.0, StatusCode::CREATED);
}
