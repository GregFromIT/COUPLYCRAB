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

## v2 owner setup (PLAN-v2)

The v1 steps above still apply. v2 adds a second restart of the host (new binary), the assistant, and one decision about old data. Nothing here was done by the build run.

- [ ] **Rebuild and replace the binary on the host** (`cd web && npm install && npm run build`, then `cargo build --release -p peer`). Restart `peer serve`. Existing data loads unchanged (schema 2 → 3 only adds things); take a copy of the data dir first if you want a way back.
- [ ] **Move your v1 training data into programs.** Open the app → Fitness → Programs. If you have v1 blocks you'll see *Migrate to programs*; it lists what could not be carried over (v1's linear progression, fixed target loads and the deload percentage; v2 recommends loads from your logged sessions instead). Run it once, check the program, and edit it (rep ranges, RIR, days). Afterwards the old Plan/Blocks/Templates endpoints answer `410 Gone`. Past sessions are kept as they are. Same thing from a terminal: `peer fitness migrate-v1 --config <path>` (it can run while the server is up).
- [ ] **Set up your gym** (Programs → Gyms): bar weight, plates per side, dumbbell and machine increments. Recommendations are rounded down to loads that gym can actually make, so this matters before you trust them. Without a gym the app assumes a 20 kg bar, 1.25–25 kg plates, 2 kg dumbbell steps and 5 kg machine steps.
- [ ] **Give exercises muscles and equipment** (Exercises). Weekly volume and warm-up suggestions use the primary and secondary muscles; the equipment decides how loads are rounded.
- [ ] **If a v0 desktop app or a v1 host syncs with this host,** add `sync_shopping = false` to its `config.toml` (they drop the connection on the new shopping document; see the section below). Phones and laptops using the web app are not affected.
- [ ] **Assistant (optional).** It answers questions about training, todos and shopping from a local model and can only read.
  1. Install Ollama: https://ollama.com/download (on the machine that will run the model; it already runs on this laptop, version 0.40.3).
  2. If Ollama runs on a different machine from `peer serve`: make it listen on that machine's Tailscale address, `OLLAMA_HOST=<tailnet IPv4>:11434` (https://docs.ollama.com/faq, "How can I expose Ollama on my network?"), and add a Tailscale ACL so only the web host can reach port 11434 (https://tailscale.com/kb/1018/acls). **Ollama has no login**: never `0.0.0.0`, and never expose it beyond the tailnet. If it runs on the same machine, leave the default `127.0.0.1:11434`.
  3. Optional: `OLLAMA_NO_CLOUD=1` so it never calls a hosted model (https://docs.ollama.com/faq).
  4. Pull a model that supports tools: `ollama pull qwen3:8b` (the plan's default; size to taste and VRAM). Check with `ollama show <model>` that "tools" is among its capabilities.
  5. Add to `config.toml`: `[assistant]` with `url = "http://127.0.0.1:11434"` and `model = "qwen3:8b"` (`http://` only), restart `peer serve`, open the Assistant tab.
  6. Try the screen without a model first with `peer serve --fake-llm` (a keyword demo, clearly labelled).
  7. Measure a model on your data: `cargo run -p assistant-core --example eval -- --config <config.toml>` (24 questions, a few minutes; see `docs/DECISIONS.md` "Live eval (v2-M8)" for what was run here and how the models compared).

## Node.js 16 is end-of-life (not blocking)

`node --version` → v16.13.0. Current Vite needs Node 18+, so both frontends (`app/` v0, `web/` v1) pin Vite 4.5.14. To move to current Vite, install Node LTS from https://nodejs.org/ (GUI installer, so not done in a build run), then bump `vite` in `web/package.json`.

## Android SDK / NDK not installed (v0 M6; not needed for v1)

`ANDROID_HOME` and `NDK_HOME` unset, no SDK under `%LOCALAPPDATA%\Android\Sdk`, no Rust Android targets. v0 M6 was not attempted. v1's web app replaces the Android build, so this only matters if you want the native v0 app on a phone. Steps are in `docs/ANDROID.md`. Needs ~10 GB free disk.

## v0/v1 peers and a v2 host (v2-M4)

v2 adds a third synced document (shopping, wire kind 3). v0 and v1 builds close the connection when they receive a kind they do not know, which breaks sync of *all* documents with them. If a v0 desktop app or a v1 host is listed in `peers`, set `sync_shopping = false` in the v2 host's `config.toml`. Not tested against a real v0/v1 build (the old behaviour is read from the code, `crates/sync/src/lib.rs` at tag `v1`). Phones and laptops using the web app are unaffected.

## `../Fruit-Deck-Builder` was not present (v3-M0)

PLAN-v3 expects `GregFromIT/Fruit-Deck-Builder` as a sibling directory. At the start of the run `../` held only this repo (and an unrelated `LinuxTutorialGame`), so **v3-M6 (the deckbuilder changes on branch `household-integration`) was skipped**, and the parts of M7 that talk to the deckbuilder (build, prices, Archidekt) were built against the *contract* in PLAN-v3 §4.2 and tested with a fake deckbuilder server, not the real one. I did not clone it: that writes outside this repo, and the plan says to skip. To finish it, clone the repo next to this one and do §4.2 on a branch (the household side already expects `POST /api/build`, `POST /api/prices`, `GET /api/themes?commander=` and `GET /api/archidekt/deck/{id}` with the shapes in `docs/API.md` "MTG").
