# Learning notes

Each section: what it is, why this project needs it, where it lives, source.

## 1. Local-first vs client-server

**What:** every device holds the full data and is the authority for its own copy. Reads and writes hit the local file; network is only used to exchange changes with other devices, whenever two happen to be online together.

**What changes without a server:**
- No single place decides the order of writes. Two devices can edit the same todo while offline, so merging has to be automatic and give the same answer on both sides → a CRDT (section 2).
- No "the database" to migrate. Old and new app builds coexist and talk to each other → schema evolution rules (section 7).
- No accounts/auth server. Device identity comes from Tailscale (section 6).
- Backups: every device is one. Losing a phone loses nothing that was synced.

**Where:** `crates/*-core` own the data; `crates/sync` only moves changes; `crates/peer` and `app/` load from local files and run sync in the background.

**Source:** Kleppmann et al., "Local-first software" — https://www.inkandswitch.com/local-first/

## 2. CRDTs and Automerge

**What:** a Conflict-free Replicated Data Type is a data structure where any two replicas that have seen the same set of changes are in the same state, regardless of the order they saw them. Automerge is a JSON-like CRDT: maps, lists, text, counters. Every change is tagged with an actor ID and sequence number; changes form a DAG via their dependencies ("heads").

**How Automerge merges:**
- Edits to *different* keys never conflict. `crates/todo-core/src/lib.rs` test `concurrent_different_fields_keep_both`: one device edits `title`, the other `notes`; both survive.
- Concurrent `put`s to the *same* key conflict. Automerge keeps all values internally but picks one winner deterministically (highest op ID: Lamport counter, then actor ID), so every replica shows the same value. Test `concurrent_title_edit_is_deterministic`.
- Identical-value `put` is a no-op, which is why we reconcile one todo at a time and only changed fields become ops.

**Why maps keyed by ID, not lists:** a list position is relative to its neighbours. If two devices insert/delete around the same index, the result is well-defined but the order may surprise, and "move item" is a delete+insert that can duplicate items. With `todos: { <uuid>: Todo }` each item has a stable key, two devices adding todos never touch the same key, and order is computed when reading (`TodoStore::list`, sort by due date). Template items and session sets carry an explicit `index` field for the same reason (fitness-core).

**Why the initial change is deterministic:** `init_doc()` creates `todos` with a fixed actor and timestamp. If two fresh devices each created their own `todos` map, those are two different objects written to the same root key, a same-key conflict, and one whole map (with its todos) would be hidden after merge. Same bytes → same change hash → Automerge treats it as one change. Test `fresh_devices_converge`.

**Soft delete (`deleted: bool`):**
- *Buys:* deleting is just another field write. If device A deletes a todo while device B edits its title, both writes survive; the todo is hidden but B's edit is still in it, and setting `deleted = false` brings it back with the edit. With map-key removal, A's delete removes the key and B's concurrent edit lands on an object that's no longer reachable from the map. Test `soft_delete_with_concurrent_edit_is_recoverable`.
- *Costs:* deleted items stay in the doc forever (plus Automerge keeps history anyway), every read must filter `deleted`, and a concurrent delete + undelete is just a last-writer-wins conflict on `deleted`. There is no "purge" in v0.

**Where:** `crates/todo-core/src/lib.rs`, `crates/fitness-core/src/lib.rs`.

**Sources:** https://automerge.org/docs/hello/, https://docs.rs/automerge/0.12.0/automerge/, https://docs.rs/autosurgeon/0.14.0/autosurgeon/, Shapiro et al. "Conflict-free Replicated Data Types" (2011) https://inria.hal.science/inria-00609399

## 4. Fitness data is append-mostly: it wouldn't strictly need a CRDT

**What:** a logged set is written once by one person and almost never edited. A session is created, gets sets appended, then is finished. Two devices essentially never edit the *same* fitness record concurrently.

**The non-CRDT alternative:** each device keeps its own append-only log file (`<device_id>.log`, one JSON record per line, each with a UUID). Sync = "send me the lines of your log I don't have" (track a per-device line count / last ID). Merged view = union of all logs, deduplicated by ID, latest record per ID wins. Edits are new records with the same ID and a higher timestamp. That's simpler to inspect (`cat` the file), has no history bloat beyond the log itself, and needs no library.

**Why we still use Automerge:** one sync protocol, one persistence path, one mental model for both modules (PLAN §3.2). The cost is that docs keep full history and grow; at two users that's small. The `FitnessStore` trait in `crates/fitness-core/src/lib.rs` is the seam if this changes: an append-log implementation could replace `AmFitnessStore` without touching the app.

**Where it shows:** sets are a map keyed by set ID with an explicit `index` (`SetLog.index`), not an Automerge list, so two devices appending sets never fight over list positions. `log_set` assigns `index = max + 1`; if the same user logged on two devices at once, both sets survive (different keys) and may share an index — order between them is then by key, still deterministic.

**Beep test scores** are `BeepScore { level: u8, shuttle: u8 }` with derived `Ord` (field order = comparison order). As floats, "8.10" parses to 8.1 which is less than 8.5 — wrong, since shuttle 10 comes after shuttle 5. Test `beep_ordering`.

**Source:** https://docs.rs/automerge/0.12.0/automerge/ ; Rust derived `Ord` is lexicographic by field order: https://doc.rust-lang.org/std/cmp/trait.Ord.html#derivable
