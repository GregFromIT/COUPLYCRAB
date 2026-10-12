# Household organiser (v1)

Todos and a fitness log (weekly plan, program blocks with progression, session logging, benchmark tests) for two people. **v1** is a web app: one always-on host on the tailnet runs `peer serve`, and both people open it in a phone or laptop browser over HTTPS via `tailscale serve`. Underneath, data still lives in two Automerge documents, and the host still syncs with v0 native peers.

```
crates/todo-core     Todo types, TodoStore trait + Automerge impl
crates/fitness-core  Exercises, templates, plans, blocks, planned_for, sessions, tests; FitnessStore + impl
crates/sync          v0 peer sync: TCP framing, peer loops, allowlist, persistence, config.toml
crates/web           JSON API /api/v1 (axum) + embedded frontend; identity, CSRF guard
crates/peer          the binary: `peer serve` (sync + web), `peer run` (sync only), small CLI
web/                 v1 frontend (plain TypeScript + Vite + uPlot)
app/                 v0 Tauri desktop app, frozen at tag v0 (still syncs as a peer)
deploy/peer.service  example systemd unit for a Linux host
docs/                API.md, DECISIONS.md, LEARNING.md, BLOCKERS.md, ANDROID.md (v0)
PLAN.md, PLAN-v1.md  the build plans
```

Start with `docs/BLOCKERS.md` (what you need to do), `docs/API.md` (the data API) and `docs/LEARNING.md` (concepts; v1 sections at the end).

## Requirements

- Rust stable (tested: cargo 1.95.0; MSVC toolchain on Windows).
- Node + npm for the frontend (tested on Node 16.13 with Vite 4.5.14; see BLOCKERS.md about upgrading).
- For the frozen v0 desktop app only: Tauri prerequisites (https://v2.tauri.app/start/prerequisites/) and `cargo install tauri-cli --version "=2.12.2" --locked`.

## Build and run

| What | Command |
|---|---|
| All tests | `cargo test --workspace` (no Node needed) |
| v1 dev: backend | `cargo run -p peer -- serve --config data-dev/config.toml --dev-user alice` |
| v1 dev: frontend | `cd web && npm install && npm run dev` → http://localhost:5173 (proxies `/api` to the backend) |
| v1 release (single binary) | `cd web && npm install && npm run build`, then `cargo build --release -p peer` → `target/release/peer[.exe]` |
| v1 run release | `target/release/peer serve --config <dir>/config.toml` → http://127.0.0.1:7430 |
| API by hand | `curl` examples in `docs/API.md` |
| v0 sync between two local peers | see "Two v0 peers on one machine" below |
| v0 desktop app | `git checkout v0 && cd app && npm install && cargo tauri dev` |

The release build embeds `web/dist`, so build the frontend **before** `cargo build --release`. Debug builds read `web/dist` from disk, and work without it (then `/` says "Frontend not built").

## Config

One `config.toml` per host, in its data dir (the directory holding the file you pass to `--config`). Data files live next to it: `todo.automerge`, `fitness.automerge`, `device_id`.

```toml
# v0 sync (always on)
user_id = "alice"                  # default user; also offered by the picker
device_name = "home-host"
listen_ip = "100.101.102.103"      # this host's Tailscale IPv4 (`tailscale ip -4`); never 0.0.0.0
listen_port = 7420                 # optional, default 7420
peers = []                         # v0 native peers (MagicDNS names, optional ":port"), if any

# v1 web app (`peer serve`)
[web]
listen = "127.0.0.1:7430"          # must be loopback: only `tailscale serve` may reach it
timezone = "Australia/Sydney"      # "today" for Today / planned-day / overdue
origins = ["https://home-host.tail1234.ts.net"]   # the URL tailscale serve gives you

[web.users]                        # Tailscale login → user id
"alice@example.com" = "alice"
"bob@example.com" = "bob"
# Both on one Tailscale account? Map it to a list; the app then asks who's using the device:
# "shared@example.com" = ["alice", "bob"]
```

For local development, a minimal config with `listen_ip = "127.0.0.1"`, `peers = []`, an empty `[web]` (defaults: `127.0.0.1:7430`, UTC) and `--dev-user alice` is enough.

## peer CLI

```sh
peer serve [--config <path>] [--dev-user <user_id>]          # sync + web app/API
peer run   [--config <path>]                                 # sync only (v0)
peer todo add "<title>" [--due YYYY-MM-DD] [--config <path>]
peer todo list [--config <path>]
peer fitness sessions [--config <path>]
```

`--config` defaults to `./config.toml`. CLI commands can run while `serve`/`run` uses the same data dir: writes merge under a file lock, and the server picks the change up within a second.

## Deployment

The owner steps are a checklist in `docs/BLOCKERS.md` ("v1 owner setup"). In short:

1. Build the release binary for the host (below) and copy it plus `config.toml` (with `[web.users]` and `[web] origins` filled in) to the host's data dir.
2. Run `peer serve --config <path>`. On Linux use `deploy/peer.service` (instructions at the top of the file). On a Windows laptop, run it in a terminal or as a Task Scheduler task "at log on".
3. Expose it on the tailnet over HTTPS: `tailscale serve --bg --https=443 127.0.0.1:7430` (https://tailscale.com/docs/reference/tailscale-cli/serve). The first run asks you to enable HTTPS certificates for the tailnet. Check with `tailscale serve status`; undo with `tailscale serve --https=443 off`.
4. Open `https://<host>.<tailnet>.ts.net` on both phones and check `https://<host>.<tailnet>.ts.net/api/v1/me` shows the right user.
5. Optional: a Tailscale ACL so only your two users' devices can reach the host (https://tailscale.com/kb/1018/acls).

### Building for a Raspberry Pi (aarch64): build on the Pi

Chosen over cross-compiling (see `docs/DECISIONS.md` "Deployment (v1-M5)"). Assumes 64-bit Raspberry Pi OS (`uname -m` prints `aarch64`) and a Pi with ≥ 2 GB RAM.

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

## Two v0 peers on one machine

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

**v1**
- **Real `tailscale serve`.** Identity headers, HTTPS and the `Origin` of the real `ts.net` URL were only simulated (headers set by tests and curl; the `Origin` check is unit-tested). Not run: the actual Serve proxy, certificate provisioning, `Tailscale-User-Login` values your account really produces.
- **Real phones.** The frontend was driven in headless Chrome with phone emulation (390×844, touch) on the dev machine. Not tested on Android/iOS browsers, including the wake lock over real HTTPS and the keyboard/viewport on small screens.
- **The Raspberry Pi.** No aarch64 build was made or run; `deploy/peer.service` was never installed.
- **A host running for days**: restarts, `tailscaled` coming up after `peer`, disk-full during a save.
- **A v0 Tauri app syncing with a v1 host** over the network. The schema rules are followed and a v0 doc is tested to load in v1, but the frozen app was never run against `peer serve`.

**v0** (still true)
- Sync between two real devices over the tailnet (all sync tests used 127.0.0.1).
- Android (never built; see `docs/ANDROID.md`).
- Installing the v0 MSI/NSIS installers (built, not run).
- Docs near the 16 MiB sync frame limit.
