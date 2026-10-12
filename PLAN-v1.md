# PLAN-v1.md — Household organiser v1: web app + data API

You are Claude Code, running **unattended**. This builds on v0 (`PLAN.md`). Read this whole file, then v0's `docs/DECISIONS.md`, `docs/BLOCKERS.md` and `docs/LEARNING.md`, before starting.

## 0. How to run this session

All v0 run rules still apply (§0 of `PLAN.md`): don't ask questions; record decisions in `docs/DECISIONS.md`; log blockers in `docs/BLOCKERS.md` and move on; three-strike rule; commit per milestone (`v1-M<n>: <summary>`); stay inside the repo; cite sources; minimal implementations; pin exact versions.

Additional rules for v1:
- **Tag the current commit `v0` before changing anything.**
- **Don't change system or Tailscale config.** Running `tailscale serve`, enabling HTTPS certificates and deploying to another machine are owner tasks (§7). Build and test everything locally on `127.0.0.1`.
- **Schema changes are additive only** (v0 rules in `DECISIONS.md`). Bump `schema_version`. A v0 document must load in v1 unchanged; add a test proving it.

## 1. What changes from v0, and why

v1 delivers a **web app** both users open in a phone or laptop browser. It is served by one always-on host on the tailnet. This trades away v0's per-device offline use for something that works on every device today, with no Android build.

- The headless `peer` binary gains a `serve` subcommand: it runs v0 sync **and** an HTTP server (JSON API + static frontend) from the same process and the same Automerge docs.
- **The JSON API is the "exposed data" layer.** The web frontend uses it now; the v2 assistant's tools map onto its read endpoints.
- The v0 Tauri app is **frozen** at tag `v0`, not deleted, and not updated in v1. It still syncs with the host as a peer if someone runs it.

## 2. v1 scope (build this, nothing else)

**Fitness (carried over from the earlier v1 draft)**
1. **Weekly plan (per user):** 7 days × AM/PM slots. A slot holds a template ID, or an *external* session (e.g. unit PT, sport) with a label, logged as "attended" with a free-text note.
2. **Program blocks (per user):** name, start date (a Monday), length in weeks, the weekly plan it repeats, optional deload week number, deload load percentage.
3. **Progression (exactly two rules, per template item):** `LinearLoad { kg_per_week }`, `LinearReps { reps_per_week }`. Deload applies the percentage to load and does not advance progression.
4. **Resolver:** `fitness-core::planned_for(user_id, date: NaiveDate) -> PlannedDay` returns concrete targets per slot (progression/deload applied) plus block context (`name`, `week n of N`, `is_deload`). Empty day with no context outside a block. Single source of truth for "today's workout".
5. **Session linking:** sessions get optional `block_id`, `block_week`, `slot` (additive).
6. **Test charts** with an optional target line, using uPlot (https://github.com/leeoniya/uPlot). Beep scores plot as `level + shuttle / shuttles_in_level` on the y-axis only; storage and sorting stay `(level, shuttle)`. Keep the shuttles-per-level table in one place with a source.
7. **External activity seam:** `ExternalActivity` type (`source`, `source_id`, `started_at`, `duration_s`, optional `distance_m`, optional `linked_session_id`), upsert by `(source, source_id)`. No import code.

**Web app**
8. Screens: Todos (list/add/edit/complete/delete); Fitness → Today (default), Log Session, Weekly Plan, Blocks, Templates, Exercises, Tests (with charts).
9. **Mobile-first layout.** Tap targets ≥ 48 px on Log Session; +/- steppers prefilled from targets; minimal typing.
10. **Keep the screen awake on Log Session** with the Screen Wake Lock API: request on entering the screen, re-acquire on `visibilitychange`, release on leaving, show whether it's active. It only works over HTTPS (secure context), so it works behind `tailscale serve` (§4) but not over plain HTTP. Feature-detect and degrade silently. Source: https://developer.mozilla.org/en-US/docs/Web/API/Screen_Wake_Lock_API
11. **Connection drops mid-workout must not lose data.** On a failed write, keep the entered values on screen, show an error with a retry button. Writes are idempotent (§3.4), so retrying can't create duplicates.

**Data API**
12. Versioned JSON API under `/api/v1` covering everything the frontend does (§3.3), documented in `docs/API.md`.

**Out of scope:** assistant/LLM (that's v2), offline/service worker/PWA install, push notifications, shopping lists, inventory, budget, Google Calendar, Health Connect import, Android/Tauri changes, user accounts or passwords.

## 3. Design decisions

### 3.1 Crates
- `crates/web`: library exposing `fn router(state: AppState) -> axum::Router`. No `main`. Depends on `todo-core` and `fitness-core` via their store traits.
- `crates/peer`: gains `peer serve --config <path> [--dev-user <user_id>]`, which runs the v0 sync loop and the HTTP server on one tokio runtime with shared stores.
- `web/`: the frontend, copied from the v0 Tauri app's frontend (plain TypeScript + Vite). All backend calls go through one `api.ts` module using `fetch`.

Framework: `axum` (https://docs.rs/axum), since the project already runs on tokio. Static frontend embedded into the release binary with `rust-embed` (https://docs.rs/rust-embed), so deploying is copying one file. In dev, Vite's `server.proxy` forwards `/api` to the backend (https://vite.dev/config/server-options#server-proxy).

### 3.2 Shared state and writes
- `AppState` holds both stores behind `tokio::sync::Mutex` and the v0 "local change" sync trigger.
- Every write: validate → mutate store → atomic save (v0 path) → trigger sync → respond.
- With one host serving both users, most conflicts disappear: two browsers editing hit the same document sequentially. The CRDT still matters for merges with any native v0 peer. Explain this in `LEARNING.md`.

### 3.3 API surface
All under `/api/v1`. JSON in and out. Errors: `{"error": {"code": "...", "message": "..."}}` with correct status codes (400 validation, 403 not owner, 404, 409 id conflict, 413 body too large, 415 wrong content type).

| Area | Endpoints |
|---|---|
| Identity | `GET /me` |
| Todos | `GET /todos?filter=open\|overdue\|undated\|due_between&from=&to=`, `PUT /todos/{id}`, `PATCH /todos/{id}`, `POST /todos/{id}/complete`, `POST /todos/{id}/uncomplete`, `DELETE /todos/{id}` (soft) |
| Exercises | `GET /exercises`, `PUT /exercises/{id}`, `PATCH /exercises/{id}` |
| Templates | `GET /templates`, `GET /templates/{id}`, `PUT /templates/{id}`, `PATCH /templates/{id}` |
| Plan | `GET /plan`, `PUT /plan` (current user's weekly plan) |
| Blocks | `GET /blocks`, `PUT /blocks/{id}`, `PATCH /blocks/{id}` |
| Today / status | `GET /planned-day?date=YYYY-MM-DD` (default: today in the host's configured timezone), `GET /program-status` |
| Sessions | `GET /sessions?since=&limit=`, `GET /sessions/{id}`, `PUT /sessions/{id}`, `PUT /sessions/{id}/sets/{set_id}`, `POST /sessions/{id}/finish` |
| Tests | `GET /tests?kind=&since=`, `PUT /tests/{id}` |
| External | `PUT /external-activities` (upsert by source + source_id) |
| Health | `GET /health` (no identity required) |

- Per-user resources (plan, blocks, sessions, tests) default to the current user; `?user=<id>` allows reading the other user's data. Writes to another user's resources return 403.
- `docs/API.md`: one section per endpoint with a request/response example. Mark every `GET` as **read-only** — the v2 assistant's tools are built from those.

### 3.4 Idempotent writes
Creates use `PUT` with a **client-generated UUID** (`crypto.randomUUID()` in the browser; https://developer.mozilla.org/en-US/docs/Web/API/Crypto/randomUUID). Repeating the same `PUT` returns 200 with no change. This makes the retry in §2.11 safe. Explain idempotency in `LEARNING.md`.

### 3.5 Identity: who is the current user
Primary: **Tailscale Serve identity headers.** When the app sits behind `tailscale serve`, each request carries `Tailscale-User-Login`; Serve strips spoofed copies from incoming requests. Source: https://tailscale.com/docs/features/tailscale-serve#identity-headers
- `config.toml`: `[web] listen = "127.0.0.1:7430"`, `timezone = "Australia/Sydney"`, `[web.users]` mapping Tailscale login → `user_id`.
- **The server binds `127.0.0.1` only.** Tailscale's docs say this is required when trusting identity headers: otherwise anyone who can reach the port directly can forge them.
- Fallback: if the header is absent, or both users' devices share one Tailscale account (headers identical), show a user picker that sets a `user` cookie (`HttpOnly`, `SameSite=Strict`). Record in `DECISIONS.md` that the picker is convenience, not security: both users trust each other.
- Header values may be RFC 2047 encoded for non-ASCII; decode or compare raw consistently, and test it.
- `--dev-user <id>` sets the user for local development and tests.

### 3.6 Cross-site request protection
A web page you visit in your phone's browser can make that browser send requests to your tailnet URL, and Serve will attach *your* identity. That's cross-site request forgery via network identity. Mitigations:
- Writes (`PUT/PATCH/POST/DELETE`) require `Content-Type: application/json` (forces a CORS preflight from foreign origins); otherwise 415.
- Reject writes whose `Origin` header is present and not the configured app origin.
- Send no CORS headers.
- Request body limit 64 KiB (`DefaultBodyLimit`, https://docs.rs/axum/latest/axum/extract/struct.DefaultBodyLimit.html); string caps (title 200 chars, notes 2000).
Write this up in `LEARNING.md` (§6.4).

## 4. How it's reached (owner sets this up, §7)

```
phone/laptop browser ──HTTPS (tailnet only)──▶ tailscale serve on host ──▶ 127.0.0.1:7430 peer serve
                                                                              │
                                                     v0 sync on tailnet IP:7420 ◀──▶ other peers (optional)
```

## 5. Milestones (in order; each ends in something runnable)

**v1-M0 — Baseline**
- Tag `v0`. `cargo test --workspace` passes. Carry unresolved v0 blockers forward.

**v1-M1 — Fitness planning core**
- §2 items 1–7 in `fitness-core`.
- Tests: v0 doc loads in v1; week index (block day 1, last day, day after end, day before start); deload applies % and doesn't advance progression; `LinearLoad` / `LinearReps` at weeks 1/4/8; external slot passthrough; two users, different blocks, same date; `ExternalActivity` upsert idempotent; concurrent plan edits on two peers converge.

**v1-M2 — Data API (`crates/web`) + `peer serve`**
- All §3.3 endpoints, identity (§3.5), request protection (§3.6), idempotent writes (§3.4).
- Tests with `tower::ServiceExt::oneshot` against the router (pattern from https://github.com/tokio-rs/axum/tree/main/examples/testing), no real sockets: every endpoint happy path; 403 on writing another user's data; 415 without JSON content type; foreign `Origin` rejected; 413 oversized body; repeated `PUT` is a no-op; identity header → user mapping, unmapped login → picker flow; `planned-day` uses configured timezone.
- `docs/API.md` complete.
- Runnable: `peer serve --dev-user <id>`, then `curl` examples from `API.md` work.

**v1-M3 — Web frontend**
- All §2 screens via `api.ts`. Wake lock (§2.10) and retry-on-failure (§2.11).
- Runnable: Vite dev server + `peer serve --dev-user`, usable in a desktop browser and Chrome's mobile emulation.

**v1-M4 — Single binary**
- Release build embeds the frontend. `peer serve` serves it at `/`.
- Runnable: `cargo build --release -p peer`, run the binary, open `http://127.0.0.1:7430`.

**v1-M5 — Deployment docs**
- `deploy/peer.service`: example systemd unit for a Linux host (don't install it).
- `README.md` deployment section: owner steps from §7, plus building for a Raspberry Pi (aarch64): either build on the Pi or cross-compile; pick one, cite the source, record in `DECISIONS.md`.

**v1-M6 — Wrap-up**
- `README.md`, `DECISIONS.md`, `LEARNING.md`, `BLOCKERS.md` complete. "What was not tested": real `tailscale serve`, real phones, the Pi.

## 6. LEARNING.md — new sections

Same style as v0: point first, then why, file paths, source link.
1. Local-first native peers vs a hosted web app: what v1 gave up (offline on phones) and gained (works everywhere now).
2. REST API design here: resources, status codes, why `PUT` with client IDs makes writes idempotent.
3. Reverse proxies and `tailscale serve`: what it terminates (TLS), what it adds (identity headers), why the backend must bind localhost.
4. CSRF through network identity, and the three defences in §3.6.
5. Secure contexts: why wake lock (and later service workers) need HTTPS.
6. Why conflicts mostly vanish with a single server, and where the CRDT still matters.
7. Deterministic scheduling maths in `planned_for`.

## 7. Owner setup steps (write to `BLOCKERS.md` as a checklist)

On the always-on host (laptop for now, Raspberry Pi later):
1. Copy the release binary and `config.toml`; set `[web.users]` to both Tailscale logins.
2. Run `peer serve --config <path>` (on Linux, via `deploy/peer.service`).
3. Expose it on the tailnet with HTTPS: `tailscale serve 7430` (https://tailscale.com/docs/features/tailscale-serve). The first run prompts to enable HTTPS certificates. Run it in the background per the CLI reference (https://tailscale.com/docs/reference/tailscale-cli/serve).
4. Open `https://<host>.<tailnet>.ts.net` on both phones; check `/api/v1/me` shows the right user.
5. Optional: Tailscale ACL limiting the host to your two users' devices (https://tailscale.com/kb/1018/acls).

## 8. Done means

- `cargo test --workspace` passes, including v0-doc-loads-in-v1 and all API tests.
- Single release binary serves the app and API locally.
- Today screen shows correct progressed/deloaded targets for a configured block; a session can be logged from it.
- `docs/API.md` covers every endpoint with read-only endpoints marked.
- Docs updated; every milestone committed.
