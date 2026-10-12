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

- **`rust-embed =8.12.0`** with the `mime-guess` feature (for `Content-Type`). 8.13.0 was five days old, so 8.12.0 (2026-07-08) was pinned, and `rust-embed-impl` / `rust-embed-utils` were pinned to 8.12.0 in `Cargo.lock` with `cargo update -p … --precise 8.12.0` (the crate's caret dependency had pulled 8.13.0). Changelog: https://docs.rs/crate/rust-embed/8.12.0/source/CHANGELOG.md (repo: https://pyrossh.dev/repos/rust-embed)
- **Embedding is in `crates/web/src/assets.rs`**, as the router's fallback (anything not under `/api/v1`). `#[allow_missing = true]` so `cargo build`/`cargo test` work on a fresh clone without Node; `/` then answers 404 with "run npm run build". Release builds must run `npm run build` in `web/` **first**, since the files are read at compile time. Debug builds read `web/dist` from disk per request (rust-embed default without `debug-embed`).
- **Cache headers:** `assets/*` (content-hashed by Vite) `public, max-age=31536000, immutable`; everything else `no-cache`, so a new deploy is picked up on the next load.
- **No SPA fallback:** the app has no client-side routes, so unknown paths are 404.
- **Verified (2026-10-12):** `cargo build --release -p peer` → `target/release/peer.exe` (7.4 MB). Copied to another directory and run with `web/dist` renamed away: `/` and the hashed JS were served from the binary with the headers above; without `--dev-user`, `GET /me` offered the picker (`alice`, `bob`) and `Tailscale-User-Login: bob@example.com` mapped to `bob`; in headless Chrome the picker → Today (Done ✓ from earlier) → an unplanned session started, a set logged, finished Partial.

## Deployment (v1-M5)

- **Raspberry Pi: build on the Pi, not cross-compile.** Native `cargo build --release -p peer` after installing rustup (https://rust-lang.github.io/rustup/installation/other.html) and `build-essential` for the linker. The frontend is built on the laptop and `web/dist` copied over, so the Pi needs no Node. Why: no extra tooling, and the binary links against the Pi's own glibc, so there's no version mismatch. The alternative, `cross build --target aarch64-unknown-linux-gnu` (https://github.com/cross-rs/cross), needs Docker plus `cargo install cross`, and its images use an older glibc; worth it only if Pi builds are too slow or run out of memory (≤ 1 GB RAM). Cost of building on the Pi: the first build is slow and wants ~2 GB RAM (swap or `-j 1` otherwise).
- **`deploy/peer.service`:** runs as a dedicated `household` user with `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, `NoNewPrivileges`, and write access only to `/var/lib/household`. `After=tailscaled.service` + `Restart=on-failure` (5 s): if the Tailscale IP isn't assigned yet at boot, the v0 sync bind fails and systemd retries. Not installed or tested (PLAN-v1: don't install it). Docs: https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html
- **`tailscale serve` command** taken from the CLI reference: `tailscale serve --bg --https=443 127.0.0.1:7430`; the docs say proxies only support `http://127.0.0.1` targets, which matches `[web] listen`. Serve strips client-supplied identity headers, Q-encodes non-ASCII values (RFC 2047, handled), and sends no identity headers for tagged devices (then the picker shows). Sources: https://tailscale.com/docs/reference/tailscale-cli/serve , https://tailscale.com/docs/features/tailscale-serve
- **Windows host:** no service wrapper. Run in a terminal or a Task Scheduler task; a Windows service would need extra code or a third-party wrapper.

## Suggested for later (v1)

Not built (PLAN-v1 says minimal); listed so they aren't forgotten.

- **Edit templates and blocks in the UI** (the API already has `PATCH`). Today a template change means creating a new one.
- **Delete for exercises, templates, sessions and test results** (API + UI). Fields exist (`deleted`); endpoints don't.
- **Today for another date / the week ahead** (`GET /planned-day?date=` already supports it; the screen only shows today).
- **Store test targets in the doc** instead of `localStorage`, so they follow you across devices.
- **`GET` for external activities** (only the upsert exists), and linking them to sessions in the UI.
- **Optimistic concurrency** (`ETag` / `If-Match`) if two people editing the same template ever becomes a problem (LEARNING v1.6).
- **Offline queue** via a service worker, to log a workout with no signal and send it later (writes are already idempotent, so a queue could just replay them).
- **`spawn_blocking` for saves** if writes ever feel slow (`ponytail:` in `crates/peer/src/main.rs`).
- **A schema-evolution test for the reverse direction**: a v1 doc opened by the v0 build. Expected to work (v0 ignores unknown keys), but only reasoned, not tested.
- **Automated browser test** for the frontend (the M3/M4 checks were scripted by hand over CDP).

## Baseline (v2-M0, 2026-10-12)

- Tagged `v1` at `a035abf` (the last v1 commit). `cargo test --workspace`: 48 passed. PLAN-v2.md saved verbatim and committed. Blockers carry forward (Node 16 EOL, Android SDK); v2 owner steps for Ollama are added in M8/M9.

## Training model (v2-M1)

- **Schema 3 = a third deterministic init change** (same actor, seq 3, time 0, deps = change 2) adding `programs` and `gym_profiles`. Same mechanism as v1's schema 2: a v0 or v1 doc merges it and agrees with a fresh doc on the map object IDs. Checked by `tests/v1_doc.rs` (`v1_doc_and_fresh_v2_device_converge`) and the updated `tests/v0_doc.rs` (a v0 doc now gains 2 init changes, was 1).
- **Every new field on an existing type carries `#[autosurgeon(missing = "Default::default")]`** (exercise muscles/equipment/rest, set `kind`/`rir`/`partials`/`rest_taken_s`/`recommendation_id`, session `program_id`/`workout_id`), per the v1 finding that autosurgeon errors on a missing key even for `Option`. `v1_doc_loads_in_v2` is the test that would catch a forgotten one.
- **New types** in `crates/fitness-core/src/program.rs`: `GymProfile`, `Program`, `ProgramWorkout`, `ProgramItem`, `WeekOverride`, enums `Equipment`, `SetStyle`, `SetKind` (same `Unknown(String)` round-trip as the v0/v1 enums). Muscles are strings validated against the fixed `MUSCLES` list rather than an enum, so the list can grow without a schema change.
- **Periodization is a flat map** `overrides` keyed `"{week}:{item_id}"`, each override changing `sets` / rep range / `rir` for one item in one week (`None` keeps the item's value). Flat, because nested maps of maps are awkward to reconcile and a per-item-per-week table is exactly what the builder screen edits.
- **Plates are unlimited per side.** `GymProfile.plates_kg` lists plate weights; there's no count per weight. A real rack has finite plates, but modelling counts adds a field and rounding branch for a case two home users rarely hit. Revisit if a plate calculator suggests a load the gym can't build.
- **External sessions stay as workouts with `external_label`** and no items (slot required), matching the v1 plan slot.
- **Migration (`migrate_v1_blocks`)**: one Program per non-deleted v1 block, ID `v1-<block id>`, using the owner's weekly plan and templates. Item and workout IDs are derived (`<slot>` / `<slot>-<n>`), so two devices migrating produce identical programs (tested) and re-running writes nothing (tested). An existing program is never overwritten, so edits survive a re-run. Mapping: reps `r` → `(r, r)` (8 if the template had none, noted); RIR 2; `Straight`; rest carried over; v1 deload week → override with half the sets (rounded up) at RIR 4. **Dropped and reported in `Migration.notes`:** linear progression, fixed target loads (v2 recommends loads from history), and the deload load percentage. v1 data stays in the doc, unwritten. Past v1 sessions keep their `block_id` and are not re-linked to programs.
- **Migration is a store method, not run automatically.** Nothing calls it yet; M3 decides when (planned: explicit `POST /programs/migrate-v1`, and the v1 template/plan/block endpoints return 410 once programs exist).
- **v1 fixture** `tests/fixtures/fitness-v1.automerge` was produced by the v1 code (tag `v1`) with a throwaway test, then the generator was deleted, same as v0's fixture.
- **Tests:** 9 new in `tests/v1_doc.rs` (57 workspace tests in total).

## Progression engine (v2-M2)

Code: `crates/fitness-core/src/progression.rs` (engine), `analytics.rs` (volume, e1RM series, PRs). Spec: PLAN-v2 A.3, A.4; behaviour modelled on MacroFactor Workouts (https://help.macrofactorapp.com/en/articles/372-what-does-progressive-overload-mean-in-macrofactor-workouts).

- **Recommendations are derived at read time, never stored.** `recommend(exercise, prescription, sessions, user, gym)` is a pure function of the session log. Why: nothing to sync or conflict (two phones can't disagree about a recommendation), and a fix to the rules corrects every future read at once. Cost: it re-scans the session list per read (fine at household scale; the web API will cache nothing). Consequence for `SetLog.recommendation_id`: it can't point at a stored record, so it's a descriptive string `<exercise>@<load>x<reps>@<rir>` (not a UUID as the plan sketched) saying what the user was shown.
- **Constants (all in `progression.rs`):** Epley divisor 30; running estimate = mean of the last 3 finished sessions (`SESSION_WINDOW`); equipment fallback when rounding loses > 10 % of the ideal load (`FALLBACK_GAP`), reps capped at range max + 3 (`FALLBACK_EXTRA_REPS`); warm-ups 40/60/80 % × 8/5/3 reps; sets with no RIR count as the prescription's target RIR (PRs and charts, which have no prescription: 2, `ASSUMED_RIR`).
- **Which sets feed the strength estimate:** `Working` and `Failure`, and v1 sets (no `kind`). `Warmup` and `Drop` don't (a drop set is lighter and already fatigued; counting it would drag the estimate down). **Volume** counts everything but warm-ups (a drop set is real work for the muscle). Sets without a load or reps are ignored, so reps-only bodyweight work gets no recommendation (known limit; weighted bodyweight works through its added load).
- **Only finished sessions count** (Completed or Partial), so the program changes when a workout is finished, as in MacroFactor (https://help.macrofactorapp.com/en/articles/369-how-often-does-my-program-update). Tested: an in-progress session with a heavy set changes nothing until `finish_session`.
- **Order independence:** sessions sort by `(started_at, id)`; the best set in a session ties on heavier load, then more reps, then higher RIR, so the same answer comes out whatever order sets or sessions were logged in (tested by reversing and swapping).
- **Rounding is exact, not greedy.** Barbell: bar + 2 × the largest plate sum ≤ (load − bar) / 2, found by a knapsack over 0.01 kg (plates {3, 5}, 7 kg a side → 6, where greedy gives 5). Never below the bar. Dumbbells: multiples of `dumbbell_step_kg`; machine/cable/bodyweight/other/unset: `machine_step_kg`; at least one step. Plates are unlimited per side (see M1).
- **Reps when not falling back** = midpoint of the range, rounded half up (5–6 → 6). In a fallback, reps solve Epley for the same reps-to-failure at the lower load, clamped to `[rep_min, rep_max + 3]`.
- **The deload falls out of the RIR target.** A deload week's override (RIR 4, half the sets) lowers the expected reps-to-failure, so the same history gives a lighter load; no special case in the engine (MacroFactor keeps periodization separate from progression: https://help.macrofactorapp.com/en/articles/387-will-smart-progression-work-with-altered-or-customized-programs). Outside the mesocycle, base prescriptions apply (no overrides) and the plan's `week` is `null`.
- **Reason text** names the last session's best set, how it compared to the target (`above` / `below` / `on`, in reps to failure), the load change vs last time, and, when the mean over several sessions was used or reps were raised for equipment, says so. Every recommendation has one (tested).
- **Warm-ups** go on the first exercise in the workout order that has a primary muscle not yet seen, only when it has a recommendation, and drop steps that round to the previous step or the working load. Per program via `Program.warmups` (added in M2 to the M1 type: programs exist in no deployed doc yet, so a required field is safe).
- **No gym profile:** the program's profile, else the user's default (or first), else a built-in 20 kg bar with 1.25–25 kg plates, 2 kg dumbbell and 5 kg machine steps (`default_gym`).
- **PRs:** one e1RM PR per exercise, plus rep PRs = sets nothing beats on both load and reps (the Pareto front). Ties go to the earliest set.
- **CLI:** `peer fitness recommend --workout <id> [--program <id>] [--user <id>] [--date D]` (program defaults to the user's running one; date to today in `[web] timezone`) and `peer fitness migrate-v1`. The migration is explicit, not automatic.
- **Tests:** 16 in the engine/analytics modules, 4 in `tests/workout_plan.rs` (including the engine on a real doc and "recommendation changes only when a session is finished").

## Training API and screens (v2-M3)

Code: `crates/web/src/training.rs` (endpoints), `crates/fitness-core/src/schedule.rs` (`next_workout`), `web/src/{train,programs,progress,dom,state,main}.ts` (screens). Examples for every endpoint are in `docs/API.md` ("v2 endpoints").

**API**
- **Arrays in, maps stored.** Programs are written and read as ordered arrays of workouts and items; the stored form (maps keyed by ID with an `index`) stays an implementation detail. Clients choose every workout and item ID, because overrides refer to items. Item IDs must be unique across the program (400 otherwise), which the core validation alone did not check.
- **`PATCH /programs/{id}` replaces `workouts` and `overrides` whole.** Merging nested lists by position would silently reorder or drop things; the builder always has the full list anyway.
- **`PUT` rules unchanged:** 201 / 200 (identical) / 409 (different) / 403 (other user's). References are checked up front and come back as 400 (`unknown exercise_id`, `unknown gym_profile_id`) rather than the 404 the core would raise.
- **`GET /workouts/next`** = the pinned workout for the weekday (the first one not yet done that day), else rotation over the program's **unpinned** workouts after the last finished one, else a rest day. Outside sessions are listed separately with a `done` flag. "Done" for any workout means a *finished* session for that `program_id` + `workout_id` on that local date. A program whose workouts are all pinned (every migrated v1 program) therefore has rest days and no rotation. `todays_sessions` lets a client resume an in-progress session instead of starting a second one.
- **One function builds a plan for both `/workouts/next` and `/recommendations`,** so they cannot disagree. `?user=` reads the other user's program; without it, someone else's program is 403.
- **v1 template/plan/block endpoints answer `410 Gone` once any program exists,** from a middleware (`gone_v1` in `lib.rs`) that runs before the body is read, so a stale client with a now-invalid body still gets a 410. The first version checked inside the handlers and a bad body got 422 first; a test found that. The 410 body carries `replacement: "/api/v1/programs"`. Before any program exists they keep working. "Any program" rather than "this user's": the doc is shared, so a per-user cutover would leave one user's v1 data writable through endpoints the other user already lost.
- **`POST /programs/migrate-v1` is explicit,** not automatic, and covers both users' blocks (blocks are readable by both anyway). The web app offers it as a button when the user has v1 blocks and no programs, and shows what was not carried over.
- **Volume and strength endpoints take `?user=`** like the rest and default to the current user; `weeks` is capped at 26 (the chart uses 8).
- **Set fields validated:** `kind` must be a known kind (400 for an unknown one, unlike the lenient stored form), `rir` at most 10, `partials` at most 50, `rest_taken_s` at most 7200.
- **Tests:** `crates/web/tests/training.rs` (6 tests); the old harness moved to `tests/common/mod.rs` and is shared with `api.rs`. Sessions made through the API are stamped by the real clock, so tests that care about dates seed sessions straight into the store (2099 dates) instead.

**Frontend**
- **Structure:** `main.ts` is the shell, todos and exercises; training lives in `train.ts` (Today, Log Workout, rest timer, plate calculator), `programs.ts` (builder, gym profiles, migration) and `progress.ts` (volume, strength, PRs, benchmark tests). `state.ts` holds shared UI state, the error banner, `run` (write + Retry) and the wake lock; `dom.ts` the helpers. The v1 Plan, Blocks and Templates screens are gone.
- **Retry behaviour kept from v1:** a failed write keeps what was typed and offers Retry, which resends the identical request. A second manual tap of "Log set" after a failure reuses the same set ID too: the pending-set key excludes `rest_taken_s` (which changes every second) and the stored body is reused whole.
- **Rest timer** is one fixed bar outside `#app`, so re-renders do not recreate it and swallow a tap. It starts after a successful `Working` or `Failure` set (not warm-ups or drop sets), using the item's `rest_s`, else the exercise's `default_rest_s`, else 90 s. It shows a countdown with +30 s and Skip, and vibrates once at zero where the browser allows. `rest_taken_s` is the time since the previous set was logged in this browser (reset when a session finishes).
- **Plate calculator** runs in the browser (`platesFor` in `train.ts`): the fewest plates making exactly the load, or the closest load below if it cannot be made. This is a second implementation next to Rust's `round_down_load` (the heaviest load at or below a target); they answer different questions. Checked in the browser: 100 kg is 20 + 20 a side; 81.3 kg gives 20 + 10 and says the closest below is 80.
- **Drop set** button sets the next set to kind `Drop` at 80 % of the last working set's load, rounded to the exercise's increment (barbell: twice the smallest plate; dumbbell/machine: the gym's step). It only pre-fills; the user taps Log set.
- **Pre-fill setting** (`recommended` or `last time`, remembered in `localStorage`): "recommended" falls back to last time when there is no recommendation, and the other way round.
- **"Next time" card** after finishing: for each exercise, what was shown during the session and what the recommendation is now (same date, so the week's prescription does not change), each with its reason. It is often "same load": the 3-session mean and plate rounding absorb small beats (the engine test with a bigger jump covers an actual change).
- **Builder periodization** is two tools over the same table: a per-exercise week grid (blank = the exercise's own value) and whole-week presets (RIR, sets change) that write an override for every lifting exercise. The grid is the truth; presets only fill it. A migrated deload week (half the sets at RIR 4) shows up in the grid as stored.
- **Progress:** volume bars for this week with tap-through to an 8-week line per muscle (one chart, not eleven lines); an e1RM line per exercise; the PR list; the v1 benchmark tests moved here unchanged (target lines are still saved per device).

**Bugs found by driving the UI in headless Chrome (390x844, touch):**
1. `h()` flattened nested child arrays one level, so a workout's exercise cards rendered as `[object HTMLDivElement]`. It now flattens fully.
2. A script of mine selected the wrong workout ("Lower" also matches the program name "Upper/Lower"). Not an app bug; noted because it looked like one.

**Checked in the browser (all passed):** Today with recommendations, reasons and warm-ups. Start, log a set (RIR, kind), rest countdown, warm-up chip (timer keeps running), plates, drop set, partials, finish, "Next time". Killing the server mid-session, tapping Log set twice, restarting and pressing Retry stored exactly one set. Building a program from scratch (two lifting workouts, an outside session, a week-3 preset) and reading back what was saved. Editing an existing program (week-4 override pre-filled). Exercises with muscles and equipment (add, edit, clear). All four Progress views. The migration prompt on a real v1 document, then 410 on `/templates`.

**Not checked:** a real phone; real HTTPS (the wake lock label says "kept awake" on localhost only); the vibration at the end of a rest; long programs on small screens.

## Shopping core and live updates (v2-M4)

Code: `crates/shopping-core` (model and Automerge store), `crates/web/src/{shopping,events}.rs` (API and SSE), `crates/peer/src/lib.rs` (the glue from sync to browser events), `crates/sync` (a third doc kind). Examples are in `docs/API.md` ("Shopping lists and live updates").

- **Own crate and own document,** like `todo-core`: `{ schema_version, lists: {id}, items: {id} }`, maps keyed by ID, soft deletes, a store trait, the same deterministic init change so two fresh devices share the collection maps.
- **The tick is one register.** `Item.check` is `"<user>|<unix ms>"` or nothing. With three fields (`checked`, `checked_by`, `checked_at`), two people ticking at once could end up with `checked_by` from one and `checked_at` from the other, because Automerge resolves each field on its own. One value always comes from one writer, and everyone converges on "ticked". Ticking a ticked item writes nothing (the first tick stands, and a double tap does not churn the document). Tested: concurrent ticks converge to one coherent value; concurrent tick and untick converge (whichever wins, both peers agree).
- **"Clear checked" hides, it does not delete.** Plain soft deletion would remove an item another person unticked a moment earlier. Instead the clear records which tick it saw (`Item.cleared = check`), and an item is on the list unless `check == cleared`. So an item unticked, or ticked again, on another peer concurrently stays; items ticked before the clear are gone on both peers. Tested both ways. Cleared items stay in the document as suggestion history. Costs one extra field per item and a rule that is slightly less obvious than a delete flag; `LEARNING.md` explains it.
- **Item delete wins over a concurrent edit** (same as todos); a deleted list cannot be restored (no un-delete in the API).
- **Suggestions are derived,** from every item ever added to the list including cleared and deleted ones: case-insensitive substring match, prefix matches first, then how often the name was added, the newest spelling shown; names currently on the list are left out ("on the list" means not deleted and not cleared, ticked or not). Nothing is stored.
- **Item order** is computed on read: unticked first, then category in the fixed order (uncategorised last), then name. Categories are validated against the fixed list (400 otherwise).
- **List IDs:** the web app uses the fixed ID `groceries` for the first list, so two devices creating "Groceries" at the same moment make one list (the same key written twice with the same content) rather than two.
- **Server-Sent Events, not WebSockets or polling** (rationale in `LEARNING.md`): traffic is one-way, it is plain HTTP through `tailscale serve`, and `EventSource` reconnects on its own. `GET /api/v1/events` sends `{"doc", "version"}` whenever a doc changes. The hub is `web::Events` (a `tokio::sync::broadcast` channel plus a counter per doc). Three sources notify it: every successful API write (inside `write_todo`/`write_fit`/`write_shopping`, after the save hook), changes merged in from a peer, and edits other processes made to the files on disk (found by the existing once-a-second poll). Failed writes send nothing.
- **Catch-up on connect.** A new connection first gets one message per doc with the current counter. `EventSource` reconnects after a drop and the stream would otherwise be silent about changes made while it was away; the first messages are the cue to refetch. A client that lags behind the 64-message channel gets the same catch-up messages instead of being dropped. The counter is per process, restarts at 0 with the server, and means only "it moved". A stream needs an identity (401 otherwise), and `EventSource` does not retry after an error status, so the frontend only opens it once it knows who is using the device.
- **Keep-alive** is a comment line every 15 s (axum's `KeepAlive`), so `tailscale serve` does not close an idle stream. Not unit-tested (the test runtime has no paused clock); seen in a real `curl -N` run on the actual binary, which also showed the catch-up messages and one event per write.
- **`futures-util` is now a direct dependency of `web`,** pinned to 0.3.34, the version already in the lockfile through axum, for `stream::unfold` (axum's `Sse` takes any `Stream`; there is nothing smaller in the standard library).
- **Compatibility break with v0 and v1 peers.** The sync wire format is `[kind][len][payload]`, and a peer that gets a kind it does not know drops the connection (`unknown doc kind`). A v2 host that sends the shopping doc (kind 3) to a v0 desktop app or a v1 host therefore breaks that connection, and since the old peers reconnect, it breaks every doc's sync with them, not only shopping. There is no handshake to detect it (any new frame would be rejected the same way). Mitigation: **`sync_shopping = false` in `config.toml`** keeps the shopping doc off the wire (it is still saved and served by the web app); the default is `true`. Anyone syncing a v0 or v1 peer with a v2 host must set it. Two v2 hosts sync all three docs. The web path (phones and laptops through the host's API) is unaffected, and is the path v1 and v2 are built around; the v0 desktop app is legacy. Noted in `BLOCKERS.md` and the README.
- **`peer` is now a library plus a binary** (`peer_app`): file layout (`todo.automerge`, `fitness.automerge`, `shopping.automerge`), `Saver`, and the two glue functions `on_remote_change` and `poll_disk`. The only reason is testing: the plan asks that an event after a peer merge be tested in-process, and that glue used to live inside `main`.
- **Tests:** 14 in `shopping-core` (including every concurrency rule above), 6 in `web/tests/shopping.rs` (API end to end, and the stream: catch-up, one event per write, nothing for rejected writes, a peer-merge notification reaching two subscribers, reconnect counters, a lagging reader), 2 in `peer/tests/events.rs` (a second device adds an item and an open browser stream hears it with no browser request; a file edited by another process is picked up and announced), 1 in `sync` (the shopping doc as a third kind, with concurrent ticks converging across two nodes).
- **Not built:** push notifications, per-store aisle order, inventory (out of scope in the plan); a shopping CLI (`peer shopping ...`); un-deleting a list.
