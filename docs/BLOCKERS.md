# Blockers / things the owner must do

## v1 owner setup (PLAN-v1 §7)

Nothing here was done by the build run: it changes Tailscale config or another machine. Do it on the always-on host (laptop for now, Raspberry Pi later).

- [ ] **Build the release binary** for the host: `cd web && npm install && npm run build`, then `cargo build --release -p peer` (Windows/Linux x86-64), or on the Pi per README "Building for a Raspberry Pi".
- [ ] **Copy the binary and `config.toml`** to the host's data dir. In `config.toml`:
  - `listen_ip` = the host's Tailscale IPv4 (`tailscale ip -4`)
  - `[web] timezone = "Australia/Sydney"` (or yours)
  - `[web.users]` = both Tailscale logins → `"alice"` / `"bob"` (your real user IDs). If both phones are on one Tailscale account, map that login to a list (`["alice", "bob"]`) and the app will ask who's using the device.
  - `[web] origins = ["https://<host>.<tailnet>.ts.net"]` (the URL from the next-but-one step). Without it, writes from the phones are rejected with `403 bad_origin`.
  - Example in README "Config".
- [ ] **Run `peer serve --config <path>`.** Linux: install `deploy/peer.service` (steps at the top of the file). Windows: a terminal, or Task Scheduler "At log on" running `peer.exe serve --config …`.
- [ ] **Expose it with HTTPS on the tailnet:** `tailscale serve --bg --https=443 127.0.0.1:7430` (https://tailscale.com/docs/reference/tailscale-cli/serve). The first run asks to enable HTTPS certificates for the tailnet (https://tailscale.com/docs/features/tailscale-serve). Check: `tailscale serve status`.
- [ ] **Open `https://<host>.<tailnet>.ts.net` on both phones** and check `https://<host>.<tailnet>.ts.net/api/v1/me` shows the right `user_id` and `login`. If `user_id` is `null`, the login in `[web.users]` doesn't match: copy the `login` value from that response into the config. Note: Serve doesn't send identity headers for *tagged* devices, so a tagged phone gets the picker.
- [ ] **Log Session keeps the screen on** only over the HTTPS URL (not `http://<ip>:7430`); check the "Screen kept awake" label on a phone.
- [ ] Optional: **Tailscale ACL** so only your two users' devices can reach the host (https://tailscale.com/kb/1018/acls).
- [ ] Optional: **move existing v0 data** to the host by copying `todo.automerge` / `fitness.automerge` from a v0 device's data dir, or by adding the host to that device's `peers` and letting v0 sync merge them.

## Node.js 16 is end-of-life (not blocking)

`node --version` → v16.13.0. Current Vite needs Node 18+, so both frontends (`app/` v0, `web/` v1) pin Vite 4.5.14. To move to current Vite, install Node LTS from https://nodejs.org/ (GUI installer, so not done in a build run), then bump `vite` in `web/package.json`.

## Android SDK / NDK not installed (v0 M6; not needed for v1)

`ANDROID_HOME` and `NDK_HOME` unset, no SDK under `%LOCALAPPDATA%\Android\Sdk`, no Rust Android targets. v0 M6 was not attempted. v1's web app replaces the Android build, so this only matters if you want the native v0 app on a phone. Steps are in `docs/ANDROID.md`. Needs ~10 GB free disk.

## v0/v1 peers and a v2 host (v2-M4)

v2 adds a third synced document (shopping, wire kind 3). v0 and v1 builds close the connection when they receive a kind they do not know, which breaks sync of *all* documents with them. If a v0 desktop app or a v1 host is listed in `peers`, set `sync_shopping = false` in the v2 host's `config.toml`. Not tested against a real v0/v1 build (the old behaviour is read from the code, `crates/sync/src/lib.rs` at tag `v1`). Phones and laptops using the web app are unaffected.
