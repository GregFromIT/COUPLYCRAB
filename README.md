# Household organiser (v2)

Todos, a training log with smart progression, shared shopping lists, and a read-only assistant, for two people. One always-on host on the tailnet runs `peer serve`; both people use it from a phone or laptop browser over HTTPS via `tailscale serve`. Underneath, data lives in three Automerge documents (todos, fitness, shopping), and the host still syncs with other `peer` hosts.

- **Training** (modelled on MacroFactor Workouts): programs you build yourself (workouts, rep ranges, RIR targets, periodization by week, gym profiles), a workout log with rest timer, warm-ups, drop sets and plate calculator, and recommendations that explain themselves ("Last session: 7 reps @ 85 kg at 2 RIR, below the target by 1 rep to failure → -5 kg"). Weekly volume per muscle, e1RM charts, PRs, and the v1 benchmark tests.
- **Shopping**: shared lists with categories, autocomplete from your own history, and live updates (what one person ticks shows up on the other's screen in a fraction of a second).
- **Assistant**: ask in plain language ("what's the workout today", "how many chest sets last week", "what's on the shopping list"). A local model (Ollama) picks read-only tools and phrases the answer; the numbers come from code. It can look things up and never change anything.

```
crates/todo-core       Todo types, TodoStore trait + Automerge impl
crates/fitness-core    Exercises, programs, gym profiles, sessions, tests; progression engine, analytics,
                       next-workout, v1 → v2 migration, and the v1 planning code (still used for migration)
crates/shopping-core   Lists and items; the single-register tick, "clear checked", suggestions
crates/assistant-core  LlmClient (Ollama, fake), the seven read-only tools, the agent loop, the live eval
crates/sync            Peer sync: TCP framing, peer loops, allowlist, persistence, config.toml
crates/web             JSON API /api/v1 (axum), Server-Sent Events, embedded frontend, identity, CSRF guard
crates/peer            The binary: `peer serve` (sync + web), `peer run` (sync only), a small CLI
web/                   Frontend (plain TypeScript + Vite + uPlot)
app/                   v0 Tauri desktop app, frozen at tag v0
deploy/peer.service    Example systemd unit for a Linux host
docs/                  API.md, DECISIONS.md, LEARNING.md, BLOCKERS.md, ANDROID.md (v0)
PLAN.md, PLAN-v1.md, PLAN-v2.md   The build plans
```

Start with `docs/BLOCKERS.md` (what you need to do), `docs/API.md` (the data API), `docs/DECISIONS.md` (what was chosen and why, including each milestone's test and browser checks) and `docs/LEARNING.md` (concepts; v1 and v2 sections at the end).

## Requirements

- Rust stable (tested: cargo 1.95.0; MSVC toolchain on Windows).
- Node + npm for the frontend (tested on Node 16.13 with Vite 4.5.14; see BLOCKERS.md about upgrading).
- Optional, for the assistant: [Ollama](https://ollama.com/download) and a model with tool support.
- For the frozen v0 desktop app only: Tauri prerequisites (https://v2.tauri.app/start/prerequisites/) and `cargo install tauri-cli --version "=2.12.2" --locked`.

## Build and run

| What | Command |
|---|---|
| All tests | `cargo test --workspace` (no Node, no Ollama needed) |
| Dev: backend | `cargo run -p peer -- serve --config data-dev/config.toml --dev-user alice` |
| Dev: frontend | `cd web && npm install && npm run dev` → http://localhost:5173 (proxies `/api` to the backend) |
| Release (single binary) | `cd web && npm install && npm run build`, then `cargo build --release -p peer` → `target/release/peer[.exe]` |
| Run release | `target/release/peer serve --config <dir>/config.toml` → http://127.0.0.1:7430 |
| Try the assistant screen with no model | add `--fake-llm` to `peer serve` (a labelled keyword demo, not a language model) |
| Measure a real model | `cargo run -p assistant-core --example eval -- --config <dir>/config.toml` (needs Ollama; not part of `cargo test`) |
| API by hand | `curl` examples in `docs/API.md` |
| v0 desktop app | `git checkout v0 && cd app && npm install && cargo tauri dev` |

The release build embeds `web/dist`, so build the frontend **before** `cargo build --release`. Debug builds read `web/dist` from disk, and work without it (then `/` says "Frontend not built").

## Config

One `config.toml` per host, in its data dir (the directory holding the file you pass to `--config`). Data files live next to it: `todo.automerge`, `fitness.automerge`, `shopping.automerge`, `device_id`.

```toml
# sync (always on)
user_id = "alice"                  # default user; also offered by the picker
device_name = "home-host"
listen_ip = "100.101.102.103"      # this host's Tailscale IPv4 (`tailscale ip -4`); never 0.0.0.0
listen_port = 7420                 # optional, default 7420
peers = []                         # other peer hosts (MagicDNS names, optional ":port"), if any
sync_shopping = true               # the shopping doc on the wire; set false if a peer is a v0/v1 build (see below)

# the web app (`peer serve`)
[web]
listen = "127.0.0.1:7430"          # must be loopback: only `tailscale serve` may reach it
timezone = "Australia/Sydney"      # "today" for workouts, overdue todos and the assistant
origins = ["https://home-host.tail1234.ts.net"]   # the URL tailscale serve gives you

[web.users]                        # Tailscale login → user id
"alice@example.com" = "alice"
"bob@example.com" = "bob"
# Both on one Tailscale account? Map it to a list; the app then asks who's using the device:
# "shared@example.com" = ["alice", "bob"]

# the assistant (optional; see docs/BLOCKERS.md "v2 owner setup")
[assistant]
url = "http://127.0.0.1:11434"     # Ollama; plain http only, bind it to localhost or the tailnet IP
model = "qwen3:8b"                 # any model that lists "tools" under `ollama show <model>`
```

**Syncing with v0 or v1 builds:** they drop the connection on a document kind they do not know, and v2 adds one (shopping). If any entry in `peers` is a v0 desktop app or a v1 host, set `sync_shopping = false` on the v2 host. Phones and laptops using the web app are not peers and are unaffected.

For local development, a minimal config with `listen_ip = "127.0.0.1"`, `peers = []`, an empty `[web]` (defaults: `127.0.0.1:7430`, UTC) and `--dev-user alice` is enough.

## peer CLI

```sh
peer serve [--config <path>] [--dev-user <user_id>] [--fake-llm]   # sync + web app/API
peer run   [--config <path>]                                       # sync only
peer todo add "<title>" [--due YYYY-MM-DD] [--config <path>]
peer todo list [--config <path>]
peer fitness sessions [--config <path>]
peer fitness migrate-v1 [--config <path>]                          # v1 blocks → programs (once)
peer fitness recommend --workout <id> [--program <id>] [--user <id>] [--date YYYY-MM-DD] [--config <path>]
```

`--config` defaults to `./config.toml`. CLI commands can run while `serve` uses the same data dir: writes merge under a file lock, and the server picks the change up within a second and tells open browsers.

## Upgrading from v1

The data loads unchanged (schema 2 → 3 only adds things); take a copy of the data dir if you want a way back. Then open Fitness → Programs: the app offers to turn your v1 blocks, weekly plan and templates into programs (or run `peer fitness migrate-v1`) and lists what cannot be carried over (linear progression, fixed target loads, the deload percentage: v2 recommends loads from your logged sessions). After a program exists, the old Plan/Blocks/Templates endpoints answer `410 Gone`. Set up your gym (bar, plates, increments) and give your exercises muscles and equipment before relying on recommendations. Full checklist: `docs/BLOCKERS.md` "v2 owner setup".

## Deployment

The owner steps are a checklist in `docs/BLOCKERS.md` ("v1 owner setup" and "v2 owner setup"). In short:

1. Build the release binary for the host (below) and copy it plus `config.toml` (with `[web.users]` and `[web] origins` filled in) to the host's data dir.
2. Run `peer serve --config <path>`. On Linux use `deploy/peer.service` (instructions at the top of the file). On a Windows laptop, run it in a terminal or as a Task Scheduler task "at log on".
3. Expose it on the tailnet over HTTPS: `tailscale serve --bg --https=443 127.0.0.1:7430` (https://tailscale.com/docs/reference/tailscale-cli/serve). The first run asks you to enable HTTPS certificates for the tailnet. Check with `tailscale serve status`; undo with `tailscale serve --https=443 off`.
4. Open `https://<host>.<tailnet>.ts.net` on both phones and check `https://<host>.<tailnet>.ts.net/api/v1/me` shows the right user.
5. Optional: a Tailscale ACL so only your two users' devices can reach the host; the assistant's Ollama setup (own ACL if it is on another machine).

### Building for a Raspberry Pi (aarch64): build on the Pi

Chosen over cross-compiling (see `docs/DECISIONS.md` "Deployment (v1-M5)"). Assumes 64-bit Raspberry Pi OS (`uname -m` prints `aarch64`) and a Pi with ≥ 2 GB RAM. A Pi is too small to run Ollama usefully, so run it elsewhere (point `[assistant] url` at it, with the ACL from BLOCKERS.md) or skip the assistant.

```sh
# On the Pi: get the source.
git clone <your repo URL> ~/household

# On the laptop: build the frontend (so the Pi doesn't need Node) and copy it over;
# git doesn't carry web/dist. scp ships with Windows' OpenSSH client.
cd web && npm install && npm run build && cd ..
scp -r web/dist pi@home-pi:household/web/

# On the Pi:
sudo apt install build-essential            # C linker, needed by rustc
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # https://rustup.rs
cd ~/household && cargo build --release -p peer --locked
# → target/release/peer. First build takes a while; on a 1 GB Pi add swap or use `-j 1`.
```

Then follow `deploy/peer.service`. Sources: https://www.rust-lang.org/tools/install , https://rust-lang.github.io/rustup/installation/other.html

## Two peers on one machine

```sh
cargo build -p peer
mkdir data-a data-b
printf 'user_id = "alice"\ndevice_name = "peer-a"\nlisten_ip = "127.0.0.1"\nlisten_port = 7421\npeers = ["127.0.0.1:7422"]\n' > data-a/config.toml
printf 'user_id = "bob"\ndevice_name = "peer-b"\nlisten_ip = "127.0.0.1"\nlisten_port = 7422\npeers = ["127.0.0.1:7421"]\n' > data-b/config.toml

target/debug/peer run --config data-a/config.toml     # terminal 1
target/debug/peer run --config data-b/config.toml     # terminal 2
target/debug/peer todo add "buy milk" --config data-a/config.toml   # terminal 3
```

Terminal 2 prints `synced todos from a peer`. Replace `run` with `serve` on one of them (plus a `[web]` section) to watch a change made in the browser reach the other peer. More in `docs/LEARNING.md` → "Try it yourself".

## What was not tested

**v2**
- **Real phones.** Everything on the frontend was driven in headless Chrome with phone emulation (390×844, touch), including two separate browser profiles as two users. Not tested on Android or iOS browsers: the wake lock, the vibration at the end of a rest, on-screen keyboards, `<datalist>` autocomplete on mobile.
- **Real `tailscale serve`.** Identity headers, HTTPS, and above all **the live-update stream through the proxy** were not run (a proxy that buffers responses would hold the event stream back; Serve should not). The stream was verified with `curl -N` and two browsers against the binary directly.
- **The assistant with the plan's default model (`qwen3`).** It was not installed; the live eval ran against the models already on this machine (see `docs/DECISIONS.md` "Live eval (v2-M8)" for which, and what they scored). `OllamaClient` against a real Ollama was exercised through that eval; its request shape is also tested against an in-process mock of Ollama's documented API.
- **A v0 desktop app or v1 host syncing with a v2 host.** The compatibility break (kind 3) and the `sync_shopping = false` workaround are read from the code and tested between v2 nodes, not against a real old build.
- **Long runs:** a host up for days, an event stream held open for hours, very large histories (recommendations are computed from the whole log on every read).
- **An un-migrated v1 instance in daily use through the v2 frontend:** the migration was run on the v1 test document and a seeded instance, not on real data.

**v1** (still true)
- The Raspberry Pi: no aarch64 build was made, and `deploy/peer.service` was never installed.
- Tailscale identity headers as your account really produces them, and certificate provisioning.

**v0** (still true)
- Sync between two real devices over the tailnet (all sync tests used 127.0.0.1).
- Android (never built; see `docs/ANDROID.md`).
- Installing the v0 MSI/NSIS installers (built, not run).
- Docs near the 16 MiB sync frame limit.
