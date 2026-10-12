//! A doc written by v0 (tag `v0`) must load in v1 unchanged.
//! Fixture made by the v0 `fitness-core` code; see docs/DECISIONS.md "Baseline (v1-M0)".

use automerge::ReadDoc;
use fitness_core::{AmFitnessStore, Automerge, FitnessStore, SessionStatus, TestKind};

const V0: &[u8] = include_bytes!("fixtures/fitness-v0.automerge");

#[test]
fn v0_doc_loads_in_v1() {
    let v0 = Automerge::load(V0).unwrap();
    let v0_heads = v0.get_heads();
    let s = AmFitnessStore::from_doc(v0).unwrap();

    // v0 data reads back intact.
    let ex = s.list_exercises().unwrap();
    assert_eq!(ex.len(), 1);
    assert_eq!((ex[0].name.as_str(), ex[0].tempo.as_deref()), ("Squat", Some("2:1")));
    let t = &s.list_templates().unwrap()[0];
    let item = t.ordered_items()[0].clone();
    assert_eq!((t.name.as_str(), item.target_load, item.progression), ("Legs", Some(100.0), None));
    let sess = &s.list_sessions().unwrap()[0];
    assert_eq!((sess.status.clone(), sess.note.as_deref(), sess.sets.len()), (SessionStatus::Completed, Some("v0 session"), 1));
    assert_eq!((sess.block_id.clone(), sess.slot.clone(), sess.label.clone()), (None, None, None), "new v1 fields are None");
    assert_eq!(s.list_tests(&TestKind::BeepTest).unwrap()[0].beep.unwrap().to_string(), "8.10");

    // Upgrading only added the shared schema-2 init change; no v0 change was rewritten.
    assert_eq!(s.schema_version().unwrap(), Some(2));
    assert!(v0_heads.iter().all(|h| s.doc.get_change_by_hash(h).is_some()));
    assert_eq!(s.doc.get_changes(&v0_heads).len(), 1);
    assert!(s.list_blocks().unwrap().is_empty());
    assert_eq!(s.get_plan("alice").unwrap(), None);
}

#[test]
fn v0_doc_and_fresh_v1_device_converge() {
    // A fresh v1 device and an upgraded v0 doc share the same collection maps.
    let mut upgraded = AmFitnessStore::from_doc(Automerge::load(V0).unwrap()).unwrap();
    let mut fresh = AmFitnessStore::new();
    fresh.add_exercise("Row", fitness_core::MeasurementType::Reps, None).unwrap();
    upgraded.doc.merge(&mut fresh.doc).unwrap();
    let names: Vec<_> = upgraded.list_exercises().unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["Row", "Squat"]);
}
