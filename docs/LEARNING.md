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

**Sources:** https://automerge.org/docs/hello/, conflict resolution: https://automerge.org/docs/reference/documents/conflicts/, https://docs.rs/automerge/0.12.0/automerge/, https://docs.rs/autosurgeon/0.14.0/autosurgeon/, Shapiro et al. "Conflict-free Replicated Data Types" (2011) https://inria.hal.science/inria-00609399

## 3. The Automerge sync protocol

**What:** two peers exchange `sync::Message`s until both have every change. Each message carries the sender's *heads* (hashes of its latest changes), a *Bloom filter* summarising the changes it has, the heads it thinks the other side needs, and any changes it believes the other side is missing.

**What `sync::State` tracks:** for one peer, what we've learned about *their* doc: their last known heads, what they've told us they have (Bloom filter), what we've already sent and are waiting to have acknowledged (`in_flight`). Without it every message would have to resend everything.

**Why one per peer (and per doc):** the state is a model of one remote replica of one document. Alice's phone and Bob's laptop are missing different changes, so they need separate states. In this code a connection holds one `State` per doc kind (`states: BTreeMap<u8, State>` in `crates/sync/src/lib.rs`, `SyncNode::connection`), freshly created per connection. A fresh state is always correct — the first round just re-establishes heads; persisting it (`State::encode`) would only save a round trip on reconnect.

**How two peers know they're done:** `generate_sync_message` returns `None` when, according to the state, the other side has everything we have and we have everything they've announced. When both sides return `None` they've converged. The connection loop in `SyncNode::connection` is: generate-and-send for every doc → wait for (incoming frame | local change | 30 s tick) → repeat. It never "finishes"; it idles until something changes.

**Sources:** https://docs.rs/automerge/0.12.0/automerge/sync/index.html ; the Bloom-filter sync design: Kleppmann & Howard, "Byzantine Eventual Consistency and the Fundamental Limits of Peer-to-Peer Databases", https://arxiv.org/abs/2012.00472

## 4. Fitness data is append-mostly: it wouldn't strictly need a CRDT

**What:** a logged set is written once by one person and almost never edited. A session is created, gets sets appended, then is finished. Two devices essentially never edit the *same* fitness record concurrently.

**The non-CRDT alternative:** each device keeps its own append-only log file (`<device_id>.log`, one JSON record per line, each with a UUID). Sync = "send me the lines of your log I don't have" (track a per-device line count / last ID). Merged view = union of all logs, deduplicated by ID, latest record per ID wins. Edits are new records with the same ID and a higher timestamp. That's simpler to inspect (`cat` the file), has no history bloat beyond the log itself, and needs no library.

**Why we still use Automerge:** one sync protocol, one persistence path, one mental model for both modules (PLAN §3.2). The cost is that docs keep full history and grow; at two users that's small. The `FitnessStore` trait in `crates/fitness-core/src/lib.rs` is the seam if this changes: an append-log implementation could replace `AmFitnessStore` without touching the app.

**Where it shows:** sets are a map keyed by set ID with an explicit `index` (`SetLog.index`), not an Automerge list, so two devices appending sets never fight over list positions. `log_set` assigns `index = max + 1`; if the same user logged on two devices at once, both sets survive (different keys) and may share an index — order between them is then by key, still deterministic.

**Beep test scores** are `BeepScore { level: u8, shuttle: u8 }` with derived `Ord` (field order = comparison order). As floats, "8.10" parses to 8.1 which is less than 8.5 — wrong, since shuttle 10 comes after shuttle 5. Test `beep_ordering`.

**Source:** https://docs.rs/automerge/0.12.0/automerge/ ; Rust derived `Ord` is lexicographic by field order: https://doc.rust-lang.org/std/cmp/trait.Ord.html#derivable

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

## 7. Schema evolution across peers running different builds

**What:** with no server, there's no moment where "the database is migrated". Your laptop may run a build with a new field while the phone still runs last month's build, and they sync the same document. Both must keep working, and neither may destroy data the other wrote.

**Rules this project follows** (also in DECISIONS.md):
1. Root key `schema_version` (currently 1) in each doc, so a future build can tell what it's looking at.
2. Additive changes only. A new field is `Option<T>` with `#[autosurgeon(missing = "Default::default")]`, so a new build reads old data where the key doesn't exist. The attribute is required: without it autosurgeon's derived `Hydrate` *fails* on a missing key even for `Option` (v0 wrongly said otherwise; the v1 fixture test `crates/fitness-core/tests/v0_doc.rs` caught it). Lesson: test schema evolution against bytes written by the old build, not against assumptions.
3. Never rename or repurpose a field. An old build would keep writing the old name/meaning.
4. Unknown enum variants don't fail. `MeasurementType`, `SessionStatus` and `TestKind` are generated by the `string_enum!` macro in `crates/fitness-core/src/lib.rs`: stored as strings, an unknown string hydrates to `Unknown("Laps")` and is written back unchanged. Test `unknown_enum_variant_survives_roundtrip`. With autosurgeon's derived enum `Hydrate`, an old build would fail to load the *whole* fitness doc the first time a new build used a new variant.
5. Old builds must not delete new fields. autosurgeon's derived struct `Reconcile` only writes the fields the struct knows and leaves other keys in the Automerge map alone, and we reconcile one item at a time with only changed fields producing ops. So an old build editing a todo's title leaves a newer build's `priority` field (say) intact.

**What's not handled:** a change that *can't* be additive (splitting a field, changing units). That would need a new field plus code in the new build that reads the old one as a fallback, keeping both until every device has upgraded.

**Sources:** autosurgeon `Hydrate` for `Option` / missing keys: https://docs.rs/autosurgeon/0.14.0/autosurgeon/hydrate/trait.Hydrate.html ; the same idea in protocol design (Protocol Buffers' "don't reuse field numbers", unknown-field preservation): https://protobuf.dev/programming-guides/proto3/#updating

## 8. Android background limits: why v0 only syncs in the foreground

**What:** Android aggressively limits what apps do when not on screen, to save battery. Background apps get their CPU and network deferred (Doze mode, App Standby buckets), and their process can be killed whenever memory is needed. A plain tokio task in the app process, like our sync loop, simply stops running a few minutes after the app is backgrounded.

**The proper ways to run in the background**, each with costs:
- *WorkManager* periodic jobs: minimum 15-minute interval, run when the OS chooses. Needs Kotlin code that starts the Rust sync, connects, syncs, exits.
- *Foreground service*: keeps running, but must show a permanent notification, and Android 14+ requires declaring a service type that matches the purpose (sync isn't a great fit for any).
- Both mean loading the doc and running sync from a context with no UI, which a Tauri app isn't set up for out of the box.

**v0 choice:** the phone syncs only while the app is open (startup, each change, every 30 s). Since a laptop or the future NAS peer is usually online, opening the app is enough to catch up. Tailscale itself has the same issue on Android: it runs as a VPN service so it stays up, but our app doesn't.

**Where:** no Android-specific code yet; see `docs/ANDROID.md`.

**Sources:** https://developer.android.com/training/monitoring-device-state/doze-standby , https://developer.android.com/develop/background-work/background-tasks/persistent , https://developer.android.com/develop/background-work/services/fgs

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

---

# v1 notes (web app + data API)

## v1.1 Local-first native peers vs a hosted web app

**What changed:** v0 put a full copy of the data on every device and synced peer-to-peer (sections 1–3). v1 puts the one live copy on an always-on host (`peer serve`), and phones and laptops are just browsers talking to its API over the tailnet (`crates/web`, `web/`).

**What v1 gave up:**
- *Offline use on phones.* No connection to the host means no app. The mid-workout case is softened, not solved: a failed write keeps your entries on screen and retries safely (v1.2), but you can't open the app at all without the host. A service worker (offline cache + queued writes) would bring some of it back; out of scope for v1.
- *No single point of failure.* If the host is off, nobody can use the app. The v0 design had no server to go down.
- *Instant local reads.* Every screen is a round trip to the host (fast on a tailnet, but not zero).

**What v1 gained:**
- *Works on every device now*, with no Android build, SDK, app store or per-device install (the v0 Android build was blocked; see `docs/ANDROID.md`).
- *One place to update.* Deploy one binary on the host; every browser gets the new version on reload. In v0 every device had to be upgraded, and old and new builds had to coexist (v0 section 7).
- *Simpler mental model for edits:* one copy, applied in order (v1.6).
- *A data API.* The same endpoints the frontend uses are what the v2 assistant will call (`docs/API.md`).

**What stayed:** the Automerge docs, persistence and v0 sync all still run on the host, so a native v0 peer (or a second host) can still sync with it, and nothing about the stored data changed except additive fields (schema 2).

**Sources:** "Local-first software", section on cloud apps vs local-first: https://www.inkandswitch.com/local-first/ ; Service Workers (the offline route not taken): https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API

## v1.2 REST API design here, and why `PUT` with client IDs makes writes idempotent

**What:** the API (`crates/web/src/api.rs`, `docs/API.md`) is resource-shaped: nouns in the path (`/todos/{id}`, `/sessions/{id}/sets/{set_id}`), and the HTTP method says what happens (`GET` read, `PUT` create, `PATCH` change some fields, `DELETE` remove, `POST` for actions that aren't plain field writes: `/complete`, `/finish`). Status codes carry the outcome, so a client can react without parsing text: 201 created, 200 OK, 204 no body, 400 your input, 401 who are you, 403 not yours, 404 no such thing, 409 clashes with what's there, 413/415 wrong shape of request.

**Idempotent** means doing it twice has the same effect as doing it once. That matters because the network can fail *after* the server did the work but *before* the phone saw the response. The phone can't know whether the set was saved, so it must be able to send it again safely.
- `POST /sessions/s1/sets` with a server-generated ID would create a second set on retry.
- `PUT /sessions/s1/sets/<uuid>` with an ID the browser generated (`crypto.randomUUID()`) names the set before it exists. The retry carries the same ID; the server sees the set is already there with the same values and answers 200 without writing (`put_set` in `crates/fitness-core/src/lib.rs`). Different values under the same ID are a 409, never a silent overwrite.
- HTTP itself defines `PUT` and `DELETE` as idempotent and `POST` as not (https://www.rfc-editor.org/rfc/rfc9110#section-9.2.2). Our `POST /finish` is made idempotent by the store (same status + note → no-op).

**Test:** `repeated_put_is_a_no_op` (`crates/web/tests/api.rs`) checks that the second identical PUT leaves the Automerge heads unchanged.

**Sources:** https://developer.mozilla.org/en-US/docs/Web/API/Crypto/randomUUID , https://developer.mozilla.org/en-US/docs/Web/HTTP/Status

## v1.3 Reverse proxies and `tailscale serve`

**What:** a reverse proxy accepts the browser's connection and forwards the request to a backend. `tailscale serve 7430` makes `https://<host>.<tailnet>.ts.net` (reachable only on the tailnet) forward to `http://127.0.0.1:7430`, where `peer serve` listens.

**What it terminates:** TLS. The browser talks HTTPS to the Tailscale daemon on the host, which holds a real Let's Encrypt certificate for the `ts.net` name; the hop to our app is plain HTTP over loopback and never leaves the machine. The app has no TLS code and no certificates to manage.

**What it adds:** identity headers. For requests from a tailnet user's device, Serve sets `Tailscale-User-Login` (plus `-Name`, `-Profile-Pic`) and removes any copies the client sent. So the app learns who's calling without passwords (`crates/web/src/identity.rs`).

**Why the backend must bind localhost:** the headers are only trustworthy if *every* request passes through Serve. If `peer serve` listened on the tailnet IP, any tailnet device could connect directly and send `Tailscale-User-Login: alice@…` itself. Binding `127.0.0.1` means only processes on the host (i.e. Serve) can reach it. `AppState::new` refuses a non-loopback `[web] listen`.

**Sources:** https://tailscale.com/docs/features/tailscale-serve#identity-headers , https://tailscale.com/docs/reference/tailscale-cli/serve

## v1.4 CSRF through network identity, and the three defences

**The attack:** you're on your phone, on the tailnet, and open some random web page. Its JavaScript (or a hidden form) can make *your* browser send a request to `https://host.tailnet.ts.net/api/v1/todos/x`. Your browser can reach that URL because you're on the tailnet, and Serve attaches *your* identity header. Classic CSRF abuses your cookies; here the "credential" is your network position plus Serve's header, attached just as automatically. The page can't read the response (same-origin policy), but it doesn't need to in order to create or delete things.

**The defences** (`protect` in `crates/web/src/lib.rs`, tests in `crates/web/tests/api.rs`):
1. **Writes require `Content-Type: application/json`.** A plain HTML form can only send `application/x-www-form-urlencoded`, `multipart/form-data` or `text/plain`. JavaScript `fetch` *can* send `application/json` cross-origin, but that makes it a non-"simple" request, so the browser first sends a CORS preflight (`OPTIONS`) and only proceeds if the server's answer allows it. Test `writes_need_json_content_type`.
2. **No CORS headers, ever.** The server never answers a preflight with `Access-Control-Allow-Origin`, so the browser refuses to send the real cross-origin JSON request. `foreign_origin_is_rejected` checks no `access-control-*` header is sent.
3. **`Origin` check.** Browsers send `Origin` on cross-origin writes; if it's present and not our configured origin, the server returns 403. A backstop in case 1–2 ever leak (e.g. a browser bug). Absent `Origin` (curl, same-origin GETs) is allowed.

Plus `SameSite=Strict` on the picker cookie, so the browser doesn't send it on requests triggered from other sites at all.

**Sources:** https://developer.mozilla.org/en-US/docs/Web/HTTP/CORS#simple_requests , https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html

## v1.5 Secure contexts: why wake lock (and later service workers) need HTTPS

**What:** browsers expose some APIs only to pages in a *secure context*: loaded over HTTPS, or from `localhost` / `127.0.0.1` (treated as trustworthy because traffic never leaves the machine). `window.isSecureContext` tells you which you're in. Screen Wake Lock, service workers, `crypto.randomUUID`, geolocation and the clipboard API are all gated this way.

**Why:** these APIs are powerful or persistent. Over plain HTTP anyone on the network path (café Wi-Fi, a compromised router) could inject a script that keeps your screen on, installs a service worker that intercepts every future visit, or reads your location. HTTPS makes the page's origin trustworthy, so the browser can safely grant it more.

**What it means here:**
- Over `http://<host-tailnet-ip>:7430` (no TLS) the Log screen can't keep the phone awake, and `crypto.randomUUID` is missing. `web/src/main.ts` feature-detects `navigator.wakeLock` and does nothing if it's absent; `web/src/api.ts` `newId()` falls back to `crypto.getRandomValues` (available in any context).
- Behind `tailscale serve` the app is `https://<host>.<tailnet>.ts.net`, a secure context, so the wake lock works on phones.
- On the dev machine, `http://localhost:5173` is already a secure context, which is why the wake lock reported active in testing.
- A future offline mode (service worker, PWA install; out of scope for v1) has the same requirement.

**Sources:** https://developer.mozilla.org/en-US/docs/Web/Security/Secure_Contexts , https://developer.mozilla.org/en-US/docs/Web/API/Screen_Wake_Lock_API , https://developer.mozilla.org/en-US/docs/Web/Security/Secure_Contexts/features_restricted_to_secure_contexts

## v1.6 Why conflicts mostly vanish with a single server, and where the CRDT still matters

**What changed:** in v0 every device had its own copy and edited it offline, so two edits to the same todo could happen "at the same time" and Automerge had to merge them. In v1 both browsers talk to one `peer serve` process holding one copy of each doc behind a mutex. Requests are applied one after another; the second sees the first's result. Nothing needs merging: it behaves like an ordinary single-writer database.

**Where the CRDT still matters:**
- The host still runs v0 sync (`peer serve` = `peer run` + HTTP). A native peer (the frozen v0 Tauri app, a second `peer` on a NAS) can edit offline and sync later; those edits merge exactly as in v0.
- Moving hosts: copy the `.automerge` files, or run two hosts for a while and let them sync. Nothing is lost.
- The idempotency checks (`put_*`) are about *retries of the same request*, not concurrent edits. The CRDT doesn't help there, which is why they're separate.

**The remaining "conflict":** last write wins between two browsers editing the same field seconds apart (e.g. both rename a template); the second `PATCH` overwrites the first. With two people in one house that's acceptable. The fix would be version checks (`If-Match` + ETags).

**Source:** https://www.rfc-editor.org/rfc/rfc9110#name-if-match (the fix, not used)

## v1.7 Deterministic scheduling maths in `planned_for`

**What:** `FitnessStore::planned_for(user, date)` (`crates/fitness-core/src/plan.rs`) turns stored facts (weekly plan, blocks, templates with progression rules) plus a date into concrete targets. Nothing about "today's workout" is stored; it's recomputed every time.

**Why deterministic:** the result must be the same on the server, on a native v0/v1 peer, and in a test, today or in a year. So it depends only on the inputs: no "now", no randomness, no iteration over unordered hash maps where order matters (items are sorted by `index`, overlapping blocks are broken by `(start_date, id)`).

**The maths:**
- `days = date − block.start_date`. In the block iff `0 ≤ days < weeks × 7`. `week = days / 7 + 1` (integer division), so day 0–6 is week 1. Test `week_index_edges` covers day 1, the last day, the day after, the day before.
- Progression steps = completed non-deload weeks before this one: `(week − 1) − (1 if the deload week is earlier)`. Week 1 → 0 steps.
- `LinearLoad`: `load = base + per_week × steps`. `LinearReps`: `reps = base + floor(per_week × steps)`.
- Deload week: the same steps as a normal week would have (completed non-deload weeks), then `load × pct/100`; reps are not reduced. The week after a deload uses one step fewer than its week number suggests, which is what "deload doesn't advance progression" means. With +2.5 kg/week from 100 kg and a 60 % deload in week 4: week 3 = 105, week 4 = 107.5 × 0.6 = 64.5, week 5 = 107.5, week 8 = 115 (`deload_applies_pct_and_does_not_advance_progression`).
- Weekday from the date (`chrono::Datelike::weekday`), then slots `<day>_am`, `<day>_pm`.

**Why store rules instead of the computed targets:** changing the base load or the rule changes all future weeks consistently, and there's nothing to keep in sync. The cost: changing a template mid-block also changes what past weeks "were". Logged sessions keep what was actually done (`block_id`, `block_week`, the sets), so history is still accurate.

**Source:** chrono `NaiveDate` arithmetic: https://docs.rs/chrono/0.4.45/chrono/naive/struct.NaiveDate.html
