# Household organiser (v0)

Local-first todos + fitness log for two people, four devices. No server: each device keeps full copies of two Automerge docs and syncs directly with peers over a Tailscale tailnet.

```
crates/todo-core     Todo types, TodoStore trait + Automerge impl
crates/fitness-core  Exercises, templates, sessions, sets, benchmark tests; FitnessStore + impl
crates/sync          TCP framing, peer loops, allowlist, persistence, config.toml
crates/peer          headless binary: sync daemon + small CLI (also the future NAS peer)
app/                 Tauri 2 desktop app (Rust in src-tauri/, plain TS + Vite frontend)
docs/                DECISIONS.md, LEARNING.md, BLOCKERS.md, ANDROID.md
```

Start with `docs/LEARNING.md` for the concepts, `docs/BLOCKERS.md` for what you need to do.

## Requirements

- Rust stable, MSVC toolchain on Windows (tested: cargo 1.95.0).
- Tauri prerequisites for the app: https://v2.tauri.app/start/prerequisites/ (WebView2 on Windows).
- Tauri CLI: `cargo install tauri-cli --version "=2.12.2" --locked`
- Node + npm for the app frontend (tested on Node 16.13 with Vite 4.5.14; see BLOCKERS.md about upgrading).

## Build and run, per milestone

| Milestone | Command |
|---|---|
| All tests | `cargo test --workspace` |
| M1 todo-core | `cargo test -p todo-core` |
| M2 fitness-core | `cargo test -p fitness-core` |
| M3 sync | `cargo test -p sync` (localhost only) |
| M4 peer | `cargo build -p peer`, then see "Two peers on one machine" |
| M5 desktop app | `cd app && npm install && cargo tauri dev` |
| M5 release | `cd app && cargo tauri build` → `target/release/bundle/msi/*.msi`, `target/release/bundle/nsis/*-setup.exe` |
| M6 Android | not attempted; see `docs/ANDROID.md` |

## Config

The app writes `config.toml` from its first-run screen into its data dir (`%APPDATA%\local.household.organiser\` on Windows). For `peer`, the data dir is the directory holding the config file given with `--config`.

```toml
user_id = "alice"                  # who owns sessions/test results logged on this device
device_name = "alice-laptop"
listen_ip = "100.101.102.103"      # this device's Tailscale IPv4 (`tailscale ip -4`); never 0.0.0.0
listen_port = 7420                 # optional, default 7420
peers = ["alice-phone", "bob-laptop", "bob-phone"]   # MagicDNS names, optional ":port"
```

`peers` is both the list of devices to dial and the allowlist for incoming connections. Only Tailscale (100.64.0.0/10) and loopback addresses are accepted for `listen_ip`.

Data files next to the config: `todo.automerge`, `fitness.automerge` (Automerge save format), `device_id`.

## peer CLI

```sh
peer run [--config <path>]                                   # sync daemon
peer todo add "<title>" [--due YYYY-MM-DD] [--config <path>]
peer todo list [--config <path>]
peer fitness sessions [--config <path>]
```

`--config` defaults to `./config.toml`. CLI commands can run while `peer run` uses the same data dir: writes merge under a file lock, and the daemon picks the change up within a second.

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

Terminal 2 prints `synced todos from a peer` and the list. More to try in `docs/LEARNING.md` → "Try it yourself".

## On the real tailnet

On each laptop: run the app, fill the first-run screen with that laptop's Tailscale IP and the other three devices' MagicDNS names. Optionally lock port 7420 down with a Tailscale ACL (https://tailscale.com/kb/1018/acls). A Raspberry Pi / NAS can run `peer run` as an always-on peer; add its name to every device's `peers`.

## What was not tested

- **Sync between two real devices over the tailnet.** All sync tests and demos used 127.0.0.1. Untested: MagicDNS resolution for the allowlist, binding to a real 100.x address, DERP-relayed connections, laptops sleeping mid-sync.
- **Android.** Not built (no SDK/NDK); keep-screen-awake options in `docs/ANDROID.md` are unverified.
- **Installing from the MSI / NSIS installers.** Both were built, and the release exe was launched once, but neither installer was run.
- **The desktop UI has no automated test.** It was driven by hand over the WebView2 DevTools protocol once (details in DECISIONS.md, "Desktop app (M5)").
- **Mixed-version peers.** Schema-evolution rules are followed and the unknown-enum path is unit-tested, but two different builds were never synced against each other.
- **Large docs.** Nothing tested near the 16 MiB frame limit or the ~10 MB revisit threshold.
- **Linux.** Not built; `peer` has no platform-specific code, the app would need webkit2gtk (https://v2.tauri.app/start/prerequisites/#linux).
