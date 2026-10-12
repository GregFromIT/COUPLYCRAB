# Decisions

Format: decision — alternatives — why. Pre-made decisions from `PLAN.md` §3 are not repeated unless the run had to interpret them.

## Toolchain (M0, recorded 2026-10-12)

```
rustup show      -> stable-x86_64-pc-windows-msvc (active, default), host x86_64-pc-windows-msvc
cargo --version  -> cargo 1.95.0 (f2d3ce0bd 2026-03-21)
node --version   -> v16.13.0
npm --version    -> 8.1.0
cargo tauri --version -> tauri-cli 2.12.2
```

- MSVC toolchain active; VS 2019 Community with MSVC 14.29.30037 provides `link.exe` (https://rust-lang.github.io/rustup/installation/windows-msvc.html). A workspace `cargo build` succeeded.
- WebView2 runtime present (registry `EdgeUpdate\Clients\{F3017226-…}` pv = 155.0.4283.45). Prereqs: https://v2.tauri.app/start/prerequisites/
- Tauri CLI installed with `cargo install tauri-cli --version "=2.12.2" --locked` (https://v2.tauri.app/reference/cli/). crates.io's newest `tauri-cli` is `3.0.0-alpha.4`; the plan says Tauri 2, so the newest 2.x was pinned.
- **Node 16 is end-of-life and too old for current Vite (5+ needs Node 18+).** Pinned Vite `=4.5.14`, the last line supporting Node 14.18+/16 (https://v4.vitejs.dev/guide/#scaffolding-your-first-vite-project). Suggest upgrading Node (see BLOCKERS.md), then Vite.

## Pinned dependencies

| crate / package | pin | changelog |
|---|---|---|
| automerge | =0.12.0 | https://github.com/automerge/automerge/releases |
| autosurgeon | =0.14.0 | https://github.com/automerge/autosurgeon/releases |
| uuid (features v4, v7) | =1.28.0 | https://github.com/uuid-rs/uuid/releases |
| tokio | =1.53.2 | https://github.com/tokio-rs/tokio/blob/master/tokio/CHANGELOG.md |
| serde | =1.0.229 | https://github.com/serde-rs/serde/releases |
| toml | =1.1.8 | https://github.com/toml-rs/toml/blob/main/crates/toml/CHANGELOG.md |
| tauri | =2.x (see M5) | https://github.com/tauri-apps/tauri/releases |
| tauri-build | =2.x (see M5) | https://github.com/tauri-apps/tauri/releases |
| tauri-cli | =2.12.2 | https://github.com/tauri-apps/tauri/releases |
| vite (npm) | =4.5.14 | https://github.com/vitejs/vite/blob/v4.5.14/packages/vite/CHANGELOG.md |

No `anyhow`/`thiserror`/`clap`: each crate has a small error enum or uses `Box<dyn Error>`, and the `peer` CLI has four subcommands parsed by hand from `std::env::args`. Fewer crates to pin and audit.

## Fitness uses a CRDT too (PLAN §3.2)

Fitness logs are append-mostly and wouldn't strictly need a CRDT: per-device append logs merged by ID would work. Using Automerge for both modules means one sync protocol, one persistence path, one thing to learn. Cost: documents keep full history and grow; acceptable at two-user scale. Revisit if a doc exceeds ~10 MB.

## Data model (M1/M2)

- **Deterministic initial change.** Each doc's root objects (`todos`, `exercises`, …) are created in one change with a fixed all-zero actor ID and timestamp 0 (`init_doc()` in each core), then the doc switches to a random actor. Every device therefore produces the byte-identical first change, so the maps share one object ID. Alternative: let the first device create the doc and have others only ever receive it — fails when two fresh devices both start offline (one `todos` map would hide the other on merge; tested in `fresh_devices_converge`). Background: https://automerge.org/docs/reference/documents/conflicts/ (conflicting puts on the same key).
- **Actor ID per process, not per device.** `Automerge` picks a random actor on each load. Reusing the device ID as actor would break if the `peer` CLI and `peer run` open the same data dir at once (same actor, two different changes with the same sequence number). The stable device ID is stored in `<data_dir>/device_id` per PLAN §3.13 but is informational in v0.
- **Edits go through `autosurgeon::reconcile_prop` on a single item**, not a whole-doc reconcile. Automerge's `put` of an identical scalar is a no-op, so only changed fields produce ops; this is what makes "concurrent edits to different fields both survive" work (tested).
- **Overdue-first sort.** Overdue todos have `due < today`, so sorting dated ascending already puts them first; no "today" parameter needed. Completed todos are not moved; the plan doesn't say to.
- **Convergence assertions** compare sorted heads + hydrated structs. `save()` bytes were *not* identical across two converged replicas in automerge 0.12 (actor-table ordering differs), so byte equality isn't a usable check; PLAN §6 allows either.
- **Data ownership (assumption, PLAN §3.8).** Exercise library and templates are shared and editable by both users. Sessions and test results carry `user_id`; both users see them, only the owner can edit them in the UI. This is enforced in the UI only — any peer can write anything to the doc. Tailscale + the peer allowlist are the trust boundary.

## Schema evolution rules (PLAN §3.15)

- Every doc has root `schema_version: u32` (currently 1).
- Additive changes only. New fields are `Option<T>` **and** carry `#[autosurgeon(missing = "Default::default")]`. (Corrected in v1: the v0 note claimed autosurgeon hydrates a missing key as `None`. It doesn't: the derived `Hydrate` fails with "unexpected nothing at all, expected a ScalarValue::Null". Caught by the v1 test that loads a real v0 doc, `crates/fitness-core/tests/v0_doc.rs`. Attribute: autosurgeon-derive 0.14 `attrs.rs`, `missing`.)
- Never rename or repurpose a field.
- Unknown enum variants hydrate to `Unknown(String)` instead of failing; the original string is preserved and written back unchanged.
- autosurgeon's derived struct reconcile only writes known fields and never deletes unknown keys, so an older build editing a todo leaves newer fields intact (checked in `autosurgeon-derive-0.14.0/src/reconcile`: only enum reconcile calls `retain`).

## Sync (M3)

- **Duplicate connections are allowed.** Both peers dial each other and both accept, so a pair can have two connections. Each runs the sync protocol; the second is redundant but harmless. Deduplicating needs a stable peer identity on both sides (MagicDNS name vs. resolved IP); not worth it at four devices.
- **`sync::State` is per connection** (so per peer, per doc, for that connection's lifetime), not persisted across reconnects. A fresh state costs one extra round trip; persisting adds a file per peer.
- **Allowlist = `peers` list**, resolved with `tokio::net::lookup_host` on every accept (MagicDNS answers locally; picks up IP changes). https://docs.rs/tokio/1.53.2/tokio/net/fn.lookup_host.html
- **Bind check** rejects anything but 100.64.0.0/10 (Tailscale's address range, https://tailscale.com/kb/1015/100.x-addresses) and loopback. Loopback is allowed for tests and the one-machine demo.
- **`listen_port` config option (default 7420) and `host:port` peer entries.** Not in PLAN §3.13, but two peers on one machine (M4 demo) can't share a port.
- **Reconnect backoff:** 1 s doubling to a 30 s cap, reset after a successful connection. The 30 s poll tick also runs inside each connection.
- **Remote changes fan out:** a change received from peer A wakes every connection, so peer B gets it without waiting for the poll.
- **Config and persistence live in the `sync` crate** (`config.rs`, `persist.rs`). Both are docs-as-bytes concerns shared by `peer` and `app`; the cores stay free of I/O.
- **Persistence merges before writing, under a file lock.** `persist::save` takes an exclusive `std::fs::File::lock` (stable since Rust 1.89, https://doc.rust-lang.org/std/fs/struct.File.html#method.lock) on `<doc>.automerge.lock`, merges the on-disk doc, then writes temp + rename. So `peer todo add` while `peer run` is running can't lose either side's changes. `std::fs::rename` replaces the target on Windows and Unix: https://doc.rust-lang.org/std/fs/fn.rename.html
- **A corrupt doc file is an error**, never silently replaced with an empty doc.
- **No logging crate:** connection errors go to stderr with `eprintln!`.
- **No `catch_unwind` around Automerge.** If automerge itself panicked on hostile input, the doc mutex would be poisoned and that doc's sync would stop with an error rather than continue on a half-applied doc. Not observed in tests.

## Desktop app (M5)

- **Scaffolded with `cargo tauri init --ci`** (https://v2.tauri.app/reference/cli/#init), then trimmed: removed the template's `tauri-plugin-log` / `log` (unused), crate renamed `household-app`, identifier `local.household.organiser`. `app/src-tauri` is a workspace member, so it shares the pinned versions and `Cargo.lock`. Debug builds don't need `app/dist` (they load `devUrl`), so `cargo test --workspace` works on a fresh clone without npm (checked).
- **Pins:** `tauri =2.12.3`, `tauri-build =2.7.2`, `serde_json =1.0.154` (newest 2.x on crates.io on 2026-10-12; `3.0.0-alpha` exists and was skipped).
- **No `@tauri-apps/api` npm package.** `app.withGlobalTauri: true` exposes `window.__TAURI__.core.invoke` and `window.__TAURI__.event.listen` (https://v2.tauri.app/reference/config/#withglobaltauri). One less dependency to pin against Node 16.
- **No `typescript` npm package.** Vite strips TS types with esbuild without type-checking (https://v4.vitejs.dev/guide/features.html#typescript). The code was type-checked once with `npx -p typescript@5.9.3 tsc --noEmit --strict --target es2021 --lib es2021,dom src/main.ts` (clean). Add `typescript` as a devDependency if you want `npm run check` in CI.
- **CSP:** `default-src 'self'; connect-src 'self' ipc: http://ipc.localhost ws://localhost:1420; style-src 'self' 'unsafe-inline'` (https://v2.tauri.app/security/csp/). All peer data is rendered via `textContent`, never `innerHTML`, since it's untrusted.
- **Core types derive `serde::Serialize`** (and `Deserialize` for command inputs `SetLog`, `NewTemplateItem`) so commands can return them directly. The cores now depend on `serde` but still not on Tauri or each other. String enums (de)serialize as their string, keeping `Unknown(s)` round-trippable.
- **Commands** (`app/src-tauri/src/lib.rs`): each wraps one store method. `App::write` locks the store, runs the op, `persist::save`s, then calls `SyncNode::local_change()`. Remote changes are persisted and emitted as a `doc-changed` event; the UI re-renders, unless an input has focus, in which case it re-renders when focus leaves.
- **Owner-only edits** are enforced in `FitnessStore` (`log_set` / `finish_session` take `user_id` and return `NotOwner`). The UI only offers to continue *your* in-progress session; others' sessions are read-only in "Recent sessions".
- **First-run screen** writes `config.toml` once. Changing it later means editing the file and restarting (no settings screen in v0). If binding fails (e.g. Tailscale not up yet), the status line shows `sync stopped: …`; restart the app to retry.
- **Session logging**: steppers are prefilled from the last set logged for that exercise in this session, else from the template targets. Steps: reps 1, kg 2.5, seconds 5, metres 100. `Distance` exercises log metres only (no time).
- **Benchmark test value units**: `OneRepMax` kg, `MaxReps` reps, `TimedRun` seconds (lower is better). Optional exercise link for 1RM / max reps; lists are per kind, not per exercise.
- **Verified on this machine (2026-10-12):** `cargo tauri dev` with a loopback config next to a `peer` on 127.0.0.1. Driven through the WebView2 DevTools protocol (`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`): add todo, add exercises, create template, start session from template (prefill 5 reps / 100 kg), stepper +2.5 kg, log set, finish Partial with note, record beep tests 8.5/8.10/9.1 (listed 9.1, 8.10, 8.5), bad score `8.x` shown as an error. The peer received the sessions and todos; a todo added on the peer appeared in the open app via `doc-changed`. Not committed as an automated test (it needs a GUI session).
- **`cargo tauri build`** (2026-10-12, after disk space was freed): 3 min release build. Outputs: `target/release/household-app.exe`, `target/release/bundle/msi/Household_0.1.0_x64_en-US.msi` (4.2 MiB), `target/release/bundle/nsis/Household_0.1.0_x64-setup.exe` (2.8 MiB). First run downloaded WiX and NSIS into Tauri's cache. The release exe was launched once: it loads from `http://tauri.localhost/` and the first-run screen rendered via IPC, so the production CSP allows `ipc:`. The first attempt had failed with a full C: drive.

## Suggested for later

Not built (PLAN says minimal); listed so they aren't forgotten.

- **Edit/delete for exercises, templates, sessions and test results.** v0 has add + list only for fitness (the plan lists edits only for todos). The store already soft-deletes via `deleted`; it needs trait methods + UI.
- **Settings screen** to change `config.toml` after first run (now: edit the file, restart). Also a "retry sync" button instead of restarting when the bind fails.
- **Show completed todos last** or hide them behind a toggle; the plan's sort doesn't mention completion.
- **Per-exercise view of 1RM / max-reps results** (now one list per test kind, mixing exercises).
- **Persist `sync::State` per peer** (`State::encode`) to save a round trip on reconnect.
- **Tailscale ACL** restricting `tcp:7420` to your four devices (https://tailscale.com/kb/1018/acls).
- **Doc size check** surfaced in the UI (warn near 10 MB; the frame limit is 16 MiB, see LEARNING §5).
- **`typescript` devDependency + `npm run check`** once Node is upgraded (see Desktop app section).
- **Upgrade Node and Vite** (BLOCKERS.md).
- **Automated UI smoke test.** The M5 check was done by hand over CDP; a WebDriver test (https://v2.tauri.app/develop/tests/webdriver/) could make it repeatable.
- **Application-level peer auth** (pre-shared key in config) if the tailnet ever has devices you don't fully trust.

---

# v1 (web app + data API, `PLAN-v1.md`)

## Baseline (v1-M0, 2026-10-12)

- Tagged `v0` at `629b113` before any v1 change. `cargo test --workspace` passed (22 tests).
- Carried forward from v0's BLOCKERS.md: Node 16 is EOL (frontend stays on Vite 4.5.14), Android SDK not installed (irrelevant for v1: the web app replaces the Android build).
- `crates/fitness-core/tests/fixtures/fitness-v0.automerge` was produced by the v0 `fitness-core` code (unchanged since tag `v0`) with a throwaway example program: one exercise, a template, a finished session with one set, one beep test. It's the input of the "v0 doc loads in v1" test. `.gitignore` has an exception for `tests/fixtures/*.automerge`.

## Fitness planning core (v1-M1)

- **Schema 2 via a second deterministic init change.** `init_doc()` change 1 is byte-identical to v0's (schema 1, four maps). Change 2 (same all-zero actor, seq 2, time 0) sets `schema_version = 2` and creates `plans`, `blocks`, `external_activities`. Every device and every upgraded v0 doc produce the same change 2, so the new maps share one object ID (same reason as v0's init; tested by `v0_doc_and_fresh_v1_device_converge`). A v0 peer receiving change 2 ignores the unknown maps.
- **Every schema-2 field is `Option` + `#[autosurgeon(missing = "Default::default")]`** (see the corrected schema rule above).
- **One weekly plan per user**, stored at `plans.<user_id>`. A block "repeats its owner's weekly plan" implicitly, so there's no `plan_id` on `Block`: with one plan per user it would always equal `user_id`. Editing the plan mid-block changes the rest of that block. Add `plan_id` (additively) if users ever want several plans.
- **Plan slots** are a map keyed `mon_am` … `sun_pm` (`SLOT_KEYS`); each slot has exactly one of `template_id` / `external_label`. Missing key = empty slot. Keyed map, not list, so two devices editing different slots both survive (`concurrent_plan_edits_converge`).
- **Progression** is stored as `{ kind: "LinearLoad" | "LinearReps", per_week: f64 }` (the plan's `LinearLoad { kg_per_week }` / `LinearReps { reps_per_week }`), not a Rust data-carrying enum, so an unknown future `kind` hydrates to `Unknown` and the item falls back to base targets instead of failing the whole doc. `LinearReps` adds `floor(per_week × weeks)`, so 0.5/week gives +1 every two weeks.
- **Deload:** load × `deload_pct / 100` in the deload week; reps unchanged; the deload week doesn't count as a progression step. Block validation: start is a Monday, 1–104 weeks, deload week inside the block, `deload_week` and `deload_pct` set together. Loads are rounded to 0.01 kg.
- **Overlapping blocks:** the one that started last wins (ties: higher ID). Deterministic; no validation against overlap.
- **Outside a block, `planned_for` returns an empty day**, even if a weekly plan exists (PLAN-v1 §2.4).
- **A plan slot pointing at a deleted/missing template is skipped** in `planned_for` rather than failing the day. `set_plan` rejects unknown template IDs at write time.
- **External sessions** (plan slot with a label) are logged as a normal `Session` with `label` set, no sets, finished `Completed` with the note. Additive `label` field on `Session`, beyond the plan's `block_id`/`block_week`/`slot`, because otherwise "attended Unit PT" isn't distinguishable from an empty blank session.
- **`ExternalActivity`** has a `user_id` (not in the plan's field list): activities are per person, and upserting someone else's activity returns `NotOwner`. Key `"{source}:{source_id}"`; `source` may not contain `:`. No list/get API beyond the upsert (none is needed yet).
- **Idempotent creates in the store** (`put_*` → `Write::{Created, Unchanged}` or `Error::Conflict`). "Same content" ignores server-assigned fields: session `started_at`/status/sets, set `index`, template item IDs. The v0 methods (`add_exercise`, `start_session`, …) remain as default trait methods that generate an ID and call `put_*`, so the frozen v0 Tauri app and `peer` CLI still compile unchanged.
- **`finish_session` is idempotent**: same status + note again changes nothing.
- **Beep scores** are validated on new writes: level 1–21, shuttle 1..=shuttles in that level (old v0 results aren't re-validated). `SHUTTLES_PER_LEVEL` in `crates/fitness-core/src/lib.rs` is the single table, from https://www.topendsports.com/testing/beep-table.htm (20 m test, level 1 at 8.0 km/h). It's used for validation and for `BeepScore::plot_value` (`level + shuttle / shuttles_in_level`), which the API returns so the frontend never needs its own copy.
- **chrono `=0.4.45`** (`default-features = false`, `std` + `clock`) for `NaiveDate` / weekdays. Changelog: https://github.com/chronotope/chrono/releases

## Data API (v1-M2)

- **Pins:** `axum =0.8.9` (https://github.com/tokio-rs/axum/blob/main/axum/CHANGELOG.md), `chrono-tz =0.10.4` (https://github.com/chronotope/chrono-tz/releases); test-only `tower =0.5.3` (`util` feature, for `ServiceExt::oneshot`), `http-body-util =0.1.5`. All newest stable on 2026-10-12. Axum 0.8 path syntax is `/{id}` (https://docs.rs/axum/0.8.9/axum/routing/struct.Router.html#method.route).
- **`std::sync::Mutex`, not `tokio::sync::Mutex`** (PLAN-v1 §3.2 says tokio). The stores are already shared with the v0 sync loop through `sync::Doc = Arc<std::sync::Mutex<dyn AsMut<Automerge>>>`, and no lock is held across an `.await`, which is the case tokio's docs recommend std's mutex for: https://docs.rs/tokio/1.53.2/tokio/sync/struct.Mutex.html#which-kind-of-mutex-should-you-use . The same `Arc<Mutex<AmTodoStore>>` is coerced to `dyn TodoStore` for the API and `dyn AsMut<Automerge>` for sync.
- **`crates/web` doesn't depend on `sync`.** It takes a `SavedHook` (`Fn(DocKind) -> Result<(), String>`) that `peer` implements as "persist (v0 `persist::save`) + `SyncNode::local_change()`". Tests pass a counter.
- **PUT semantics:** create-or-identical (201 / 200), different content → 409; changes go through `PATCH`. PLAN-v1 lists 409 for "id conflict" and says a repeated PUT is a no-op; this is the reading where both hold. `PUT /plan` is a full replace (a singleton, idempotent by nature). `PUT /external-activities` is an upsert (201 new, 200 updated/unchanged) as specified.
- **"Same content"** ignores server-set fields (`created_at`, `started_at`, set `index`, template item IDs), so a retry minutes later still matches.
- **Todo filters** `overdue`, `undated`, `due_between` only return open (not completed) todos; `open` = not completed; no filter = everything not deleted. `due_between` accepts either bound alone.
- **`GET /tests` adds `score` and `plot_value`**, so the chart never needs its own beep table. Without `kind`, all kinds are returned, grouped by kind, each best-first.
- **`GET /program-status`** returns today's block context, sessions logged in the current block week, and the next block. PLAN-v1 doesn't define its fields; this is the minimum an assistant needs for "where am I in my program".
- **`PUT /me`** added for the picker (PLAN-v1 lists only `GET /me`, but the picker must set the cookie somehow). Cookie: `user=<id>; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000`. No `Secure` flag, so it also works over plain-HTTP dev on 127.0.0.1; behind `tailscale serve` the browser only ever uses HTTPS anyway.
- **Identity order** (`crates/web/src/identity.rs`): `--dev-user` → header mapped to one user → cookie among the allowed users → none (401 `no_user`). A header mapped to one user ignores the cookie. `[web.users]` values may be a string or a list; a list means a shared Tailscale account, and the picker offers only those users. Unmapped/absent header: the picker offers every known user (`user_id` + all mapped values). **The picker is convenience, not security:** the two users trust each other, and anyone on the tailnet who reaches the app can pick any user.
- **RFC 2047:** `Tailscale-User-Login` is decoded (`=?utf-8?B|Q?…?=`) before matching `[web.users]`, so config holds the readable login. Other charsets/malformed input are compared raw. Hand-written decoder (`decode_rfc2047`, ~40 lines) rather than a MIME crate. Spec: https://www.rfc-editor.org/rfc/rfc2047#section-4
- **CSRF guard** is a middleware on `/api/v1` (`protect` in `crates/web/src/lib.rs`): JSON content type on every non-GET/HEAD/OPTIONS request, `Origin` checked when present. Default allowed origins are `http://127.0.0.1:<port>` and `http://localhost:<port>`; setting `[web] origins` replaces them (put the `https://<host>.<tailnet>.ts.net` URL there). No CORS layer at all.
- **Limits:** `DefaultBodyLimit::max(64 KiB)` (https://docs.rs/axum/0.8.9/axum/extract/struct.DefaultBodyLimit.html); titles/names/labels 200 chars, notes 2000, tempo 20, ≤50 template items, `limit` 1–500. Json rejections (400/413/415) are re-wrapped into the standard error body by the `Body<T>` extractor.
- **`[web] listen` must be loopback**: `AppState::new` refuses anything else. PLAN-v1 §3.5: trusting identity headers is only safe if nothing but `tailscale serve` can reach the port.
- **`[web]` lives in the same `config.toml`.** `peer` parses it separately (`WebSection`), so the `sync` crate's `Config` is unchanged and the frozen v0 app's config still loads. Caveat: the v0 app's first-run screen writes `config.toml` without a `[web]` section; don't point it at the host's data dir.
- **Saves happen inside the request** (blocking file write under the doc lock). Fine at two users; marked `ponytail:` in `crates/peer/src/main.rs` with the upgrade path (`spawn_blocking`).
- **`docs/API.md` examples are real responses**, generated by a throwaway script against a fresh `peer serve --dev-user alice` (not committed). If an endpoint changes, re-run its `curl` by hand.

## Web frontend (v1-M3)

- **`web/` is a new copy, not a move.** The v0 Tauri frontend in `app/` stays frozen at tag `v0`; `web/src/main.ts` started from it (same `h()` helper, CSS, stepper) with Tauri `invoke` replaced by `web/src/api.ts` (`fetch`).
- **Pins:** `vite 4.5.14` (Node 16, see BLOCKERS), `uplot 1.6.32` (newest on npm; https://github.com/leeoniya/uPlot/releases). No TypeScript package: Vite strips types; checked once with `npx -p typescript@5.9.3 tsc --noEmit --strict … src/main.ts` (clean).
- **Dev proxy rewrites `Origin`.** `web/vite.config.js` forwards `/api` to `http://127.0.0.1:7430` with `headers: { origin: backend }`, because the backend's CSRF guard only accepts its own origin and the browser's is `http://localhost:5173`. Dev only; the production frontend is served by the backend itself (same origin). Source: https://v4.vitejs.dev/config/server-options.html#server-proxy (options are http-proxy's, which include `headers`).
- **Retry design (§2.11):** every write goes through `run(fn)` in `main.ts`. The client ID is generated *before* the first attempt and captured in `fn`; on failure nothing is re-rendered (typed values stay), and the error banner's Retry calls the same `fn`. For sets, a pending set ID is reused only if the values are unchanged since the failed attempt; changed values get a new ID (a new set), avoiding a 409.
- **Errors without a JSON body** (Vite or `tailscale serve` couldn't reach the backend) are shown as "Can't reach the server … retry".
- **Wake lock (§2.10):** requested when the Log screen shows an in-progress session, re-requested on `visibilitychange`, released on any other screen; the Log header shows "Screen kept awake" / "Screen may sleep". Feature-detected; failures are swallowed.
- **Log Session targets:** a session started from Today (has `block_id` + `slot`) uses that day's `planned-day` targets (progressed/deloaded); a session started from a template uses the template's base targets; blank sessions pick an exercise. Steppers prefill from the last set of that exercise in the session, else the target.
- **Today** shows the block context, each slot with targets, and Start / Continue / Done ✓; external slots get a note + "Mark attended" (`PUT /sessions` with `label`, then `finish Completed`). A session counts for a slot if `block_id`, `block_week` and `slot` match.
- **Test chart target line is stored in `localStorage`** per user + kind (`target:<user>:<kind>`), in chart units (for beep tests, `10` = start of level 10). It's a viewing preference, so it isn't in the API or the doc. Wrapped in try/catch; private mode just loses it.
- **Weekly Plan screen** edits a local draft and saves the whole plan (`PUT /plan`). An external slot with an empty label is saved as empty.
- **Blocks:** create and (soft) delete. Editing a block is API-only (`PATCH`); not needed for v1 screens.
- **Exercises:** create and edit. **Templates:** create (with per-item progression) and list; editing is API-only.
- **Verified (2026-10-12)** against `peer serve --dev-user alice` + `vite`, in headless Chrome with phone emulation (390×844, touch), driven over the DevTools protocol: Today showed week 3 targets (105 kg, 12 reps); Start → Log prefilled 5 / 105 / 12; `+` → 107.5 kg; set logged; wake lock reported active (localhost is a secure context); **server killed → Log set → error + Retry, entered value kept; server restarted → Retry → exactly one set saved**; finish Completed with note → Today shows Done ✓; Unit PT marked attended; Monday-only block validation error shown with input kept; template with progression created; exercise edited; three beep tests → best-first list and chart with target line; todos add/complete/edit. Found and fixed: "+ Item" re-render wiped the template name.

## Single binary (v1-M4)

- **`rust-embed =8.12.0`** with the `mime-guess` feature (for `Content-Type`). 8.13.0 was five days old, so 8.12.0 (2026-07-08) was pinned, and `rust-embed-impl` / `rust-embed-utils` were pinned to 8.12.0 in `Cargo.lock` with `cargo update -p … --precise 8.12.0` (the crate's caret dependency had pulled 8.13.0). Changelog: https://git.sr.ht/~pyrossh/rust-embed/tree/master/item/CHANGELOG.md
- **Embedding is in `crates/web/src/assets.rs`**, as the router's fallback (anything not under `/api/v1`). `#[allow_missing = true]` so `cargo build`/`cargo test` work on a fresh clone without Node; `/` then answers 404 with "run npm run build". Release builds must run `npm run build` in `web/` **first**, since the files are read at compile time. Debug builds read `web/dist` from disk per request (rust-embed default without `debug-embed`).
- **Cache headers:** `assets/*` (content-hashed by Vite) `public, max-age=31536000, immutable`; everything else `no-cache`, so a new deploy is picked up on the next load.
- **No SPA fallback:** the app has no client-side routes, so unknown paths are 404.
- **Verified (2026-10-12):** `cargo build --release -p peer` → `target/release/peer.exe` (7.4 MB). Copied to another directory and run with `web/dist` renamed away: `/` and the hashed JS were served from the binary with the headers above; without `--dev-user`, `GET /me` offered the picker (`alice`, `bob`) and `Tailscale-User-Login: bob@example.com` mapped to `bob`; in headless Chrome the picker → Today (Done ✓ from earlier) → an unplanned session started, a set logged, finished Partial.
