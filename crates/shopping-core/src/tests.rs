use super::*;

fn new_item(id: &str, list: &str, name: &str) -> NewItem {
    NewItem { id: id.into(), list_id: list.into(), name: name.into(), quantity: None, note: None, category: None }
}

fn store() -> AmShoppingStore {
    let mut s = AmShoppingStore::new();
    s.put_list("g", "Groceries").unwrap();
    s
}

fn names(s: &AmShoppingStore, list: &str) -> Vec<String> {
    s.items(list).unwrap().into_iter().map(|i| i.name).collect()
}

/// Two devices that start from the same doc.
fn pair(s: AmShoppingStore) -> (AmShoppingStore, AmShoppingStore) {
    let b = AmShoppingStore::from_doc(s.doc.fork()).unwrap();
    (s, b)
}

fn sync(a: &mut AmShoppingStore, b: &mut AmShoppingStore) {
    a.doc.merge(&mut b.doc).unwrap();
    b.doc.merge(&mut a.doc).unwrap();
    let heads = |d: &Automerge| {
        let mut h = d.get_heads();
        h.sort();
        h
    };
    assert_eq!(heads(&a.doc), heads(&b.doc), "peers converge to the same heads");
}

#[test]
fn lists_and_items_basics() {
    let mut s = AmShoppingStore::new();
    assert_eq!(s.put_list("g", "Groceries").unwrap(), Write::Created);
    assert_eq!(s.put_list("g", "Groceries").unwrap(), Write::Unchanged);
    assert!(matches!(s.put_list("g", "Other name"), Err(Error::Conflict(_))));
    s.put_list("b", "bunnings").unwrap();
    assert!(s.put_list("x", "  ").is_err());
    assert_eq!(s.lists().unwrap().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["bunnings", "Groceries"], "by name, ignoring case");
    s.rename_list("b", "Bunnings").unwrap();
    s.delete_list("b").unwrap();
    assert_eq!(s.lists().unwrap().len(), 1);
    assert!(matches!(s.put_item("alice", new_item("i", "b", "screws")), Err(Error::NotFound(_))), "no items on a deleted list");
    assert!(matches!(s.put_item("alice", new_item("i", "nope", "x")), Err(Error::NotFound(_))));
}

#[test]
fn put_item_is_idempotent_and_validates() {
    let mut s = store();
    let milk = NewItem { quantity: Some("2".into()), note: Some("full cream".into()), category: Some("dairy".into()), ..new_item("m", "g", " Milk ") };
    assert_eq!(s.put_item("alice", milk.clone()).unwrap(), Write::Created);
    let heads = s.doc.get_heads();
    assert_eq!(s.put_item("alice", milk.clone()).unwrap(), Write::Unchanged);
    assert_eq!(s.doc.get_heads(), heads, "a repeated put writes nothing");
    assert!(matches!(s.put_item("alice", NewItem { quantity: Some("3".into()), ..milk.clone() }), Err(Error::Conflict(_))));
    assert!(s.put_item("alice", NewItem { category: Some("snacks".into()), ..new_item("x", "g", "Chips") }).is_err());
    assert!(s.put_item("alice", new_item("y", "g", "   ")).is_err());
    let it = s.get_item("m").unwrap();
    assert_eq!((it.name.as_str(), it.quantity.as_deref(), it.category.as_deref(), it.added_by.as_str(), it.checked()), ("Milk", Some("2"), Some("dairy"), "alice", false));
}

#[test]
fn sorted_unchecked_first_then_category_then_name() {
    let mut s = store();
    for (id, name, cat) in [
        ("1", "Soap", Some("household")),
        ("2", "Apples", Some("produce")),
        ("3", "Bread", Some("bakery")),
        ("4", "Bananas", Some("produce")),
        ("5", "Mystery", None),
        ("6", "Milk", Some("dairy")),
    ] {
        s.put_item("alice", NewItem { category: cat.map(Into::into), ..new_item(id, "g", name) }).unwrap();
    }
    assert_eq!(names(&s, "g"), ["Apples", "Bananas", "Milk", "Bread", "Soap", "Mystery"]);
    s.set_checked("bob", "2", true).unwrap();
    assert_eq!(names(&s, "g"), ["Bananas", "Milk", "Bread", "Soap", "Mystery", "Apples"], "ticked items sink to the bottom");
    assert!(s.items("other-list").unwrap().is_empty());
}

#[test]
fn check_is_idempotent_and_records_who_and_when() {
    let mut s = store();
    s.put_item("alice", new_item("m", "g", "Milk")).unwrap();
    s.set_checked("bob", "m", true).unwrap();
    let it = s.get_item("m").unwrap();
    assert_eq!((it.checked(), it.checked_by()), (true, Some("bob")));
    assert!(it.checked_at().unwrap() > 1_700_000_000_000);
    let heads = s.doc.get_heads();
    s.set_checked("alice", "m", true).unwrap();
    assert_eq!(s.doc.get_heads(), heads, "ticking a ticked item changes nothing");
    assert_eq!(s.get_item("m").unwrap().checked_by(), Some("bob"), "the first tick stands");
    s.set_checked("alice", "m", false).unwrap();
    let it = s.get_item("m").unwrap();
    assert_eq!((it.checked(), it.checked_by(), it.checked_at()), (false, None, None));
    let heads = s.doc.get_heads();
    s.set_checked("alice", "m", false).unwrap();
    assert_eq!(s.doc.get_heads(), heads);
    assert!(matches!(s.set_checked("bob", "nope", true), Err(Error::NotFound(_))));
}

#[test]
fn user_ids_with_a_bar_still_parse() {
    let mut s = store();
    s.put_item("a", new_item("m", "g", "Milk")).unwrap();
    s.set_checked("we|rd", "m", true).unwrap();
    let it = s.get_item("m").unwrap();
    assert_eq!(it.checked_by(), Some("we|rd"));
    assert!(it.checked_at().is_some());
}

#[test]
fn concurrent_ticks_converge_on_one_coherent_check() {
    let mut s = store();
    s.put_item("alice", new_item("m", "g", "Milk")).unwrap();
    let (mut a, mut b) = pair(s);
    a.set_checked("alice", "m", true).unwrap();
    b.set_checked("bob", "m", true).unwrap();
    sync(&mut a, &mut b);
    let (ia, ib) = (a.get_item("m").unwrap(), b.get_item("m").unwrap());
    assert_eq!(ia, ib, "both peers read the same item");
    assert!(ia.checked(), "both agree it is ticked");
    // who and when come from ONE writer, never mixed: the register holds one "<user>|<ms>".
    assert!(matches!(ia.checked_by(), Some("alice") | Some("bob")));
    assert_eq!(ia.check.as_deref().unwrap().matches('|').count(), 1);
    assert_eq!(a.items("g").unwrap(), b.items("g").unwrap());
}

#[test]
fn concurrent_tick_and_untick_converge() {
    let mut s = store();
    s.put_item("alice", new_item("m", "g", "Milk")).unwrap();
    s.set_checked("alice", "m", true).unwrap();
    let (mut a, mut b) = pair(s);
    a.set_checked("alice", "m", false).unwrap(); // untick
    b.set_checked("bob", "m", false).unwrap();
    b.set_checked("bob", "m", true).unwrap(); // untick then tick again
    sync(&mut a, &mut b);
    assert_eq!(a.get_item("m").unwrap(), b.get_item("m").unwrap(), "whatever wins, both agree");
}

#[test]
fn clear_checked_hides_ticked_items_and_a_new_tick_shows_again() {
    let mut s = store();
    for (id, name) in [("1", "Milk"), ("2", "Eggs"), ("3", "Bread")] {
        s.put_item("alice", new_item(id, "g", name)).unwrap();
    }
    s.set_checked("alice", "1", true).unwrap();
    s.set_checked("alice", "2", true).unwrap();
    assert_eq!(s.clear_checked("g").unwrap(), 2);
    assert_eq!(names(&s, "g"), ["Bread"]);
    assert_eq!(s.clear_checked("g").unwrap(), 0, "nothing left to clear");
    // The item still exists; ticking it again (a different tick) puts it back on the list.
    assert!(!s.get_item("1").unwrap().visible());
    std::thread::sleep(std::time::Duration::from_millis(2));
    s.set_checked("bob", "1", false).unwrap();
    s.set_checked("bob", "1", true).unwrap();
    assert_eq!(names(&s, "g"), ["Bread", "Milk"]);
    assert!(matches!(s.clear_checked("nope"), Err(Error::NotFound(_))));
}

#[test]
fn clear_checked_keeps_items_unchecked_concurrently_on_another_peer() {
    let mut s = store();
    for (id, name) in [("1", "Milk"), ("2", "Eggs"), ("3", "Bread")] {
        s.put_item("alice", new_item(id, "g", name)).unwrap();
    }
    s.set_checked("alice", "1", true).unwrap();
    s.set_checked("alice", "2", true).unwrap();
    let (mut a, mut b) = pair(s);
    assert_eq!(a.clear_checked("g").unwrap(), 2); // alice clears the ticked items ...
    b.set_checked("bob", "1", false).unwrap(); // ... while bob, in the shop, unticks Milk (he still needs it)
    sync(&mut a, &mut b);
    for s in [&a, &b] {
        assert_eq!(names(s, "g"), ["Bread", "Milk"], "Milk was unticked concurrently, so it stays; Eggs were cleared");
        assert!(!s.get_item("2").unwrap().visible());
    }
}

#[test]
fn clear_checked_does_not_hide_an_item_ticked_after_the_clear_on_another_peer() {
    let mut s = store();
    s.put_item("alice", new_item("1", "g", "Milk")).unwrap();
    s.put_item("alice", new_item("2", "g", "Eggs")).unwrap();
    s.set_checked("alice", "1", true).unwrap();
    let (mut a, mut b) = pair(s);
    a.clear_checked("g").unwrap();
    b.set_checked("bob", "2", true).unwrap(); // a different item, ticked meanwhile
    sync(&mut a, &mut b);
    assert_eq!(names(&a, "g"), ["Eggs"], "Eggs is on the list, ticked: the clear never saw that tick");
    assert!(a.get_item("2").unwrap().checked());
}

#[test]
fn concurrent_edits_to_different_fields_merge() {
    let mut s = store();
    s.put_item("alice", new_item("m", "g", "Milk")).unwrap();
    let (mut a, mut b) = pair(s);
    a.edit_item("m", ItemEdit { name: Some("Oat milk".into()), ..Default::default() }).unwrap();
    b.edit_item("m", ItemEdit { quantity: Some(Some("2 L".into())), category: Some(Some("dairy".into())), ..Default::default() }).unwrap();
    b.set_checked("bob", "m", true).unwrap();
    sync(&mut a, &mut b);
    let it = a.get_item("m").unwrap();
    assert_eq!((it.name.as_str(), it.quantity.as_deref(), it.category.as_deref(), it.checked()), ("Oat milk", Some("2 L"), Some("dairy"), true));
    assert_eq!(it, b.get_item("m").unwrap());
    // Clearing a field.
    a.edit_item("m", ItemEdit { quantity: Some(None), ..Default::default() }).unwrap();
    assert_eq!(a.get_item("m").unwrap().quantity, None);
    assert!(a.edit_item("m", ItemEdit { category: Some(Some("nope".into())), ..Default::default() }).is_err());
    assert!(a.edit_item("m", ItemEdit { name: Some(" ".into()), ..Default::default() }).is_err());
}

#[test]
fn delete_wins_over_a_concurrent_edit() {
    let mut s = store();
    s.put_item("alice", new_item("m", "g", "Milk")).unwrap();
    let (mut a, mut b) = pair(s);
    a.delete_item("m").unwrap();
    b.edit_item("m", ItemEdit { note: Some(Some("organic".into())), ..Default::default() }).unwrap();
    sync(&mut a, &mut b);
    assert!(names(&a, "g").is_empty() && names(&b, "g").is_empty());
}

#[test]
fn suggestions_come_from_history_and_skip_items_on_the_list() {
    let mut s = store();
    s.put_list("b", "Bunnings").unwrap();
    for (id, list, name) in [("1", "g", "Milk"), ("2", "g", "Bread"), ("3", "g", "Mince"), ("4", "g", "milk"), ("5", "g", "Oat milk"), ("6", "b", "Mitre saw")] {
        s.put_item("alice", new_item(id, list, name)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let sug = |s: &AmShoppingStore, list: &str, q: &str, n: usize| s.suggestions(list, q, n).unwrap();
    assert!(sug(&s, "g", "mi", 8).is_empty(), "everything matching is on the list right now");

    // Ticked and cleared: off the list, but remembered, so now suggested.
    for id in ["1", "4", "5"] {
        s.set_checked("alice", id, true).unwrap();
    }
    assert_eq!(s.clear_checked("g").unwrap(), 3);
    // "milk" was added twice ("Milk", then "milk": the newest spelling is shown); "Mince" is still on the list.
    assert_eq!(sug(&s, "g", "mi", 8), ["milk", "Oat milk"], "prefix matches first, then substring matches");
    assert_eq!(sug(&s, "g", "MILK", 8), ["milk", "Oat milk"], "case-insensitive");
    assert_eq!(sug(&s, "g", "oat", 8), ["Oat milk"]);
    assert_eq!(sug(&s, "g", "", 8), ["milk", "Oat milk"], "no query: most often added first");
    assert_eq!(sug(&s, "g", "", 1), ["milk"], "limit");
    assert!(sug(&s, "g", "zzz", 8).is_empty());

    // Lists are separate; a deleted item is history too.
    assert!(sug(&s, "b", "mi", 8).is_empty(), "Mitre saw is on its own list");
    s.delete_item("6").unwrap();
    assert_eq!(sug(&s, "b", "mi", 8), ["Mitre saw"]);
    assert!(sug(&s, "g", "mitre", 8).is_empty());
}

#[test]
fn fresh_devices_converge() {
    let mut a = AmShoppingStore::new();
    let mut b = AmShoppingStore::from_doc(Automerge::new()).unwrap();
    a.put_list("g", "Groceries").unwrap();
    b.put_list("h", "Bunnings").unwrap();
    a.put_item("alice", new_item("1", "g", "Milk")).unwrap();
    b.put_item("bob", new_item("2", "h", "Screws")).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.lists().unwrap().len(), 2);
    assert_eq!((names(&a, "g"), names(&a, "h")), (vec!["Milk".to_string()], vec!["Screws".to_string()]));
}
