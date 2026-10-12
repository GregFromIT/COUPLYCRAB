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

## 3. The Automerge sync protocol

**What:** two peers exchange `sync::Message`s until both have every change. Each message carries the sender's *heads* (hashes of its latest changes), a *Bloom filter* summarising the changes it has, the heads it thinks the other side needs, and any changes it believes the other side is missing.

**What `sync::State` tracks:** for one peer, what we've learned about *their* doc: their last known heads, what they've told us they have (Bloom filter), what we've already sent and are waiting to have acknowledged (`in_flight`). Without it every message would have to resend everything.

**Why one per peer (and per doc):** the state is a model of one remote replica of one document. Alice's phone and Bob's laptop are missing different changes, so they need separate states. In this code a connection holds one `State` per doc kind (`states: BTreeMap<u8, State>` in `crates/sync/src/lib.rs`, `SyncNode::connection`), freshly created per connection. A fresh state is always correct — the first round just re-establishes heads; persisting it (`State::encode`) would only save a round trip on reconnect.

**How two peers know they're done:** `generate_sync_message` returns `None` when, according to the state, the other side has everything we have and we have everything they've announced. When both sides return `None` they've converged. The connection loop in `SyncNode::connection` is: generate-and-send for every doc → wait for (incoming frame | local change | 30 s tick) → repeat. It never "finishes"; it idles until something changes.

**Sources:** https://docs.rs/automerge/0.12.0/automerge/sync/index.html ; the Bloom-filter sync design: Kleppmann & Howard, "Byzantine Eventual Consistency and the Fundamental Limits of Peer-to-Peer Databases", https://arxiv.org/abs/2012.00472

## 5. Framing a TCP stream, and why there's a size limit

**What:** TCP is a byte stream, not a message stream. One `write` of 1000 bytes can arrive as three reads of 400/400/200, or merged with the next message. Framing puts boundaries back: every frame is `[u8 doc_kind][u32 big-endian length][payload]`. The reader reads exactly 5 header bytes, then exactly `length` payload bytes (`read_frame` in `crates/sync/src/lib.rs`, using `read_exact`).

**Why the 16 MiB limit:** the length field is chosen by the remote peer. Without a limit, a buggy or compromised peer sends `length = 4 GiB` and we allocate 4 GiB (or try to) → out of memory. `read_frame` checks the length *before* allocating and returns an error, which closes the connection. Test `oversize_frame_closes_connection`. The same untrusted-input rule applies to the payload: decode errors and rejected changes return `Err` and close the connection; nothing on the network path uses `unwrap`. Test `malformed_payload_rejected_without_corruption` sends garbage, an unknown doc kind, and a real message with a corrupted tail, then checks the doc's heads are unchanged and it still saves/loads.

**Why the reader runs in its own task:** `read_exact` isn't cancel-safe. Inside `tokio::select!`, if the tick branch won halfway through reading a frame, the partially read bytes would be lost and the stream desynchronised. A dedicated reader task feeds whole frames through a channel instead.

**Limit to remember:** the initial sync of a large doc sends all changes in one message. A doc approaching 16 MiB would fail to sync (`write_frame` refuses rather than sending a frame the peer will reject). PLAN's "revisit at ~10 MB" keeps us under it.

**Sources:** https://docs.rs/tokio/1.53.2/tokio/io/trait.AsyncReadExt.html#method.read_exact ; cancel safety: https://docs.rs/tokio/1.53.2/tokio/macro.select.html#cancellation-safety ; CWE-400 uncontrolled resource consumption: https://cwe.mitre.org/data/definitions/400.html

## 6. What Tailscale gives us, and what the app still does

**Tailscale gives:**
- *Encryption:* every packet between devices goes through a WireGuard tunnel, so our plain TCP is encrypted on the wire.
- *Device identity:* a device is only on the tailnet if it was logged in to your account; the 100.x.y.z source address is bound to that device's WireGuard key, so it can't be spoofed from outside the tailnet.
- *NAT traversal:* phones on mobile data and laptops behind home routers can still reach each other directly (UDP hole punching).
- *DERP relays:* when no direct path works, traffic is relayed (still end-to-end encrypted) via Tailscale's DERP servers.
- *MagicDNS:* stable names like `bob-phone` instead of IPs.

**The app still has to:**
- Bind only the Tailscale interface, never `0.0.0.0` (`bind` in `crates/sync/src/lib.rs` refuses anything that isn't 100.64.0.0/10 or loopback), so the port isn't reachable from café Wi-Fi.
- Drop connections from tailnet devices that aren't in `peers` (`SyncNode::allowed`): a tailnet can contain other devices (a work laptop, a shared node) that shouldn't get your data.
- Treat peer data as untrusted (section 5).
- Optionally restrict port 7420 to these four devices in Tailscale ACLs as defence in depth: https://tailscale.com/kb/1018/acls

**Not done in v0:** application-level peer authentication (e.g. a shared key). Any device on your tailnet whose IP matches a configured peer name is trusted.

**Sources:** https://tailscale.com/blog/how-tailscale-works, https://tailscale.com/kb/1232/derp-servers, https://tailscale.com/kb/1081/magicdns

## Try it yourself: two peers on one machine

Each peer needs its own data dir and port. From the repo root (the `data-*` dirs are gitignored):

```sh
cargo build -p peer
mkdir data-a data-b
```

`data-a/config.toml`:
```toml
user_id = "alice"
device_name = "peer-a"
listen_ip = "127.0.0.1"
listen_port = 7421
peers = ["127.0.0.1:7422"]
```

`data-b/config.toml`:
```toml
user_id = "bob"
device_name = "peer-b"
listen_ip = "127.0.0.1"
listen_port = 7422
peers = ["127.0.0.1:7421"]
```

Terminal 1: `target/debug/peer run --config data-a/config.toml`
Terminal 2: `target/debug/peer run --config data-b/config.toml`
Terminal 3: `target/debug/peer todo add "buy milk" --due 2026-10-20 --config data-a/config.toml`

Within ~1 s terminal 1 prints `picked up local edit to todos` (the running peer noticed the file changed), and terminal 2 prints `synced todos from a peer` followed by the list. `target/debug/peer todo list --config data-b/config.toml` shows it too.

Things to try:
- Stop peer B (Ctrl-C), add two todos on A, start B again: they arrive on reconnect (B's dialer retries with backoff; A's dialer reconnects within 30 s).
- Delete `data-b/todo.automerge` with B stopped and restart it: B gets the whole doc back from A.
- Watch the bytes: `todo.automerge` is the output of `Automerge::save()`, the compressed change history.

What happens under the hood: `peer todo add` loads `todo.automerge`, makes the change, and calls `persist::save`, which locks the file, merges what's on disk and writes atomically (`crates/sync/src/persist.rs`). The running `peer run` polls the file's mtime every second (`crates/peer/src/main.rs`), merges it, and calls `SyncNode::local_change()`, which wakes every connection to generate a sync message.
