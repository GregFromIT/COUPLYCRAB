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

- **Deterministic initial change.** Each doc's root objects (`todos`, `exercises`, …) are created in one change with a fixed all-zero actor ID and timestamp 0 (`init_doc()` in each core), then the doc switches to a random actor. Every device therefore produces the byte-identical first change, so the maps share one object ID. Alternative: let the first device create the doc and have others only ever receive it — fails when two fresh devices both start offline (one `todos` map would hide the other on merge; tested in `fresh_devices_converge`). Background: https://automerge.org/docs/reference/under-the-hood/merge_rules/ (conflicting puts on the same key).
- **Actor ID per process, not per device.** `Automerge` picks a random actor on each load. Reusing the device ID as actor would break if the `peer` CLI and `peer run` open the same data dir at once (same actor, two different changes with the same sequence number). The stable device ID is stored in `<data_dir>/device_id` per PLAN §3.13 but is informational in v0.
- **Edits go through `autosurgeon::reconcile_prop` on a single item**, not a whole-doc reconcile. Automerge's `put` of an identical scalar is a no-op, so only changed fields produce ops; this is what makes "concurrent edits to different fields both survive" work (tested).
- **Overdue-first sort.** Overdue todos have `due < today`, so sorting dated ascending already puts them first; no "today" parameter needed. Completed todos are not moved; the plan doesn't say to.
- **Convergence assertions** compare sorted heads + hydrated structs. `save()` bytes were *not* identical across two converged replicas in automerge 0.12 (actor-table ordering differs), so byte equality isn't a usable check; PLAN §6 allows either.
- **Data ownership (assumption, PLAN §3.8).** Exercise library and templates are shared and editable by both users. Sessions and test results carry `user_id`; both users see them, only the owner can edit them in the UI. This is enforced in the UI only — any peer can write anything to the doc. Tailscale + the peer allowlist are the trust boundary.

## Schema evolution rules (PLAN §3.15)

- Every doc has root `schema_version: u32` (currently 1).
- Additive changes only. New fields are `Option<T>` (autosurgeon hydrates a missing key as `None`).
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
- **`cargo tauri build` not completed**: disk full, see BLOCKERS.md.

## Suggested for later

- (filled in as the run goes)
