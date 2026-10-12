# PLAN-v2.md — v2: MacroFactor-style training, shopping lists, assistant agent

You are Claude Code, running **unattended**. This builds on v1 (`PLAN-v1.md`). Read this file, then `docs/DECISIONS.md`, `docs/BLOCKERS.md`, `docs/LEARNING.md` and `docs/API.md`, before starting. All v0/v1 run rules apply (don't ask questions, log decisions/blockers, three-strike rule, cite sources, minimal implementations, pin versions, stay in the repo, additive schema changes only). Commit as `v2-M<n>: <summary>`. Tag the starting commit `v1`.

Three tracks, built in this order, because the agent reads from the other two:
- **Track A — Training rework** modelled on MacroFactor Workouts
- **Track B — Shopping lists**
- **Track C — Assistant agent** (Ollama, read-only)

---

## Track A — Training rework (MacroFactor Workouts model)

### A.1 What we're copying, and what we aren't

Reference behaviour, from MacroFactor's own help centre (paraphrased; links are the source of truth):

- **Programs keep a fixed structure; only targets change.** A program's exercises, weekly layout and sets/reps stay as built unless the user edits it. Logged workouts update the *recommended weights/reps*, and that happens when a workout is finished, for that workout's next occurrence. https://help.macrofactorapp.com/en/articles/369-how-often-does-my-program-update
- **Targets are a rep range plus an RIR (reps in reserve) target.** Expected max performance = midpoint of the rep range + RIR target (e.g. 7–9 reps @ 2 RIR → 10 reps to failure). Beating that expectation signals progress; this still counts if the logged RIR differs from the target. https://help.macrofactorapp.com/en/articles/372-what-does-progressive-overload-mean-in-macrofactor-workouts
- **Progression is rule-based, not generative AI, and never punitive:** under-performing lowers the next recommendation slightly. https://help.macrofactorapp.com/en/articles/373-why-does-the-app-sometimes-recommend-lowering-weight-or-reps
- **Equipment limits:** when the next available weight jump is too large relative to the load (10 → 15 kg is +50%), it raises reps instead. Users set their equipment's increments. https://help.macrofactorapp.com/en/articles/392-why-am-i-being-recommended-very-high-reps
- **Every recommendation explains itself** (the "progression wand"), and the log can be pre-filled from smart recommendations or from previous values. https://help.macrofactorapp.com/en/articles/391-what-does-the-smart-progression-wand-mean, https://help.macrofactorapp.com/en/articles/305-understanding-and-using-smart-progressions
- **Smart progression is separate from periodization.** Week-to-week changes in sets/reps/RIR come from periodization on the program, not from the progression engine. https://help.macrofactorapp.com/en/articles/387-will-smart-progression-work-with-altered-or-customized-programs
- **Logging detail:** rest timers, warm-up suggestions, drop sets, failure sets, partials, RIR; volume tracking per muscle; PRs; plate calculator; multiple gym profiles. https://macrofactor.com/workouts/

**Build:** custom program builder, periodization, smart progression with explanations, detailed set logging, rest timer, warm-up suggestions, plate calculator, gym profiles, volume per muscle, e1RM/PR charts. Keep v1's benchmark tests and external sessions (unit PT), which MacroFactor doesn't have.

**Don't build:** program *generation* from a questionnaire (it depends on curated exercise/volume content we don't have; both users build their own programs), exercise swap suggestions, program imports, nutrition, body metrics, progress photos, social features. List these in `DECISIONS.md` under "Suggested for later".

### A.2 Data model changes (additive)

All weights in **kg**.

- **GymProfile (per user):** name (e.g. "Base gym", "Home"), bar weight, available plates (kg list), dumbbell increments, machine/cable increment. One profile marked default.
- **Exercise (shared):** add `primary_muscles`, `secondary_muscles` (fixed enum: chest, back, shoulders, biceps, triceps, quads, hamstrings, glutes, calves, abs, forearms), `equipment` (barbell, dumbbell, machine, cable, bodyweight, other), default rest seconds.
- **Program (per user)** replaces v1 template + weekly plan + block:
  - name, start date, mesocycle length in weeks, gym profile
  - ordered workouts (e.g. "Upper A"), each optionally pinned to weekday + AM/PM slot. v1 external slots stay as a slot type.
  - each workout item: exercise, sets, rep range `(min, max)`, RIR target, set style (`Straight` | `DropSet` | `ToFailure`), rest seconds
  - **periodization (optional):** per-week overrides of sets, rep range and RIR, e.g. RIR 3 → 2 → 1 → 0 then a deload week with half the sets at RIR 4. Week resolution reuses v1's `planned_for` week maths.
- **SetLog** additions: `kind` (`Warmup` | `Working` | `Drop` | `Failure`), `rir: Option<u8>`, `partials: Option<u8>`, `rest_taken_s: Option<u32>`, `recommendation_id: Option<Uuid>` (which recommendation was shown).
- **Migration:** a pure function maps v1 templates/plans/blocks into Programs (fixed reps `r` → range `(r, r)`; RIR 2 default; v1 deload → periodization deload week; v1 linear progression → dropped and noted). v1 fields stay in the doc and aren't written to after migration. Test it on a v1 fixture document.

### A.3 Smart progression engine (`fitness-core::progression`)

**Pure function of history:** `recommend(exercise, prescription, history, gym_profile) -> Recommendation { load_kg, reps, rir, reason }`. Recommendations are **derived at read time, never stored**: no sync conflicts, and fixing a bug in the engine fixes all past outputs. Record this in `DECISIONS.md`.

Rules (v2 is deliberately simple; document every constant):
1. **Performance per working set:** reps-to-failure `F = reps + rir` (logged RIR; if missing, assume the target RIR).
2. **Strength estimate:** e1RM per set by Epley, `load × (1 + F/30)` (https://en.wikipedia.org/wiki/One-repetition_maximum). Session estimate = best working set. Running estimate = mean of the last 3 session estimates. Taking the mean makes it non-punitive: one bad day moves it only a third.
3. **Target:** expected F = `midpoint(rep range) + RIR target` (MacroFactor's rule). Solve Epley for the load that gives expected F at the running e1RM; round **down** to the nearest achievable load for the exercise's equipment in the gym profile.
4. **Equipment fallback:** if the rounded load is more than 10% below the ideal, keep the lower load and raise target reps (capped at the range max + 3) so that F still matches. Otherwise target reps = range midpoint.
5. **No history:** no recommendation. The UI asks for a load and pre-fills previous values once they exist.
6. **`reason`** is a short sentence naming the cause, e.g. "Last 3 sessions averaged 8 reps @ 80 kg at 2 RIR, above the 7–9 @ 2 target → +2.5 kg". Every recommendation has one.
7. **Warm-ups:** for the first exercise per primary muscle group, suggest 40%/60%/80% of the working load at 8/5/3 reps, rounded to the equipment. Users can disable this per program.

Tests: the MacroFactor worked example (7–9 @ 2 RIR → expected 10); progression after beating the target; small drop after a miss; 3-session mean dampens one bad session; rounding to plates (bar 20 kg, plates 1.25–25); light-dumbbell fallback raising reps; no-history case; deload week from periodization; output is deterministic regardless of log order.

### A.4 Analytics (all derived at read time)
- **Weekly volume per muscle:** working sets per muscle; secondary muscles count 0.5. Week starts Monday.
- **PRs:** e1RM PR per exercise; rep PR at a given load.
- **Charts (uPlot, already in v1):** e1RM over time per exercise; weekly sets per muscle for the last 8 weeks. Benchmark-test charts stay.

### A.5 Training screens
- **Today:** the next workout from the program (pinned weekday, otherwise next in rotation), each exercise showing its recommendation and reason (tap to expand).
- **Log Workout:** pre-filled from recommendations (setting: `smart` | `previous`). Per set: load, reps, RIR stepper, kind. Add set / drop set; partials field hidden behind a toggle. **Rest timer** starts automatically when a working set is ticked, using the exercise's rest seconds, and shows a visible countdown. **Plate calculator** button per barbell exercise. Wake lock and retry behaviour from v1 still apply.
- **Programs:** builder (workouts, items, ranges, RIR, rest, periodization table by week), gym profiles.
- **Progress:** volume per muscle, e1RM charts, PR list, benchmark tests.

### A.6 API additions
`/programs`, `/gym-profiles`, `/workouts/next?date=`, `/recommendations?program_id=&workout_id=`, `/analytics/volume?week=`, `/analytics/e1rm?exercise_id=`, `/prs`. Same conventions as v1: `PUT` with client IDs, JSON only, documented in `docs/API.md` with read-only endpoints marked. v1 endpoints for templates/plans/blocks return 410 Gone after migration, with a body pointing to the replacement.

---

## Track B — Shopping lists

### B.1 Scope
- Multiple shared named lists (e.g. "Groceries", "Bunnings").
- Item: name, optional quantity (free text: "2", "500 g"), optional note, optional category (fixed list: produce, meat, dairy, bakery, pantry, frozen, household, other), `checked`, `checked_by`, `checked_at`, `added_by`.
- Add, edit, check/uncheck, delete; "clear checked" (soft delete); list sorted unchecked-first, then by category.
- **Autocomplete** item names from items previously added to that list (derived from history, not stored).
- **Live updates:** when one person adds or ticks an item, the other's open list updates within ~2 s without refreshing.

**Out of scope:** grocery inventory, prices/budget, recipes, store-specific aisle maps, push notifications.

### B.2 Design
- New crate `shopping-core`, own Automerge doc, same patterns as `todo-core` (maps keyed by ID, soft delete, store trait).
- **Concurrency rule:** two people ticking the same item at once is the normal case in a shop. Check state is a single field written with the actor and time; last writer wins, and both results agree on "checked". Test it.
- **Live updates via Server-Sent Events:** `GET /api/v1/events` streams `{"doc": "shopping" | "todo" | "fitness", "version": ...}` whenever a doc changes (local write or merged from a peer). The frontend refetches the affected view. axum SSE: https://docs.rs/axum/latest/axum/response/sse/index.html. SSE over WebSockets because traffic is one-way server → client and it works through `tailscale serve` as ordinary HTTP. Send a keep-alive comment every 15 s; reconnect automatically on the client (`EventSource` does this; https://developer.mozilla.org/en-US/docs/Web/API/EventSource).
- API: `/lists`, `/lists/{id}/items`, `PUT /lists/{id}/items/{item_id}`, `POST /lists/{id}/clear-checked`, `GET /lists/{id}/suggestions?q=`.
- Screen: big tick targets, quick-add box at the top with autocomplete, category headings.

Tests: concurrent check from two peers converges; clear-checked doesn't remove items unchecked concurrently on another peer; suggestions exclude items currently on the list; SSE emits after a local write and after a peer merge (in-process test).

---

## Track C — Assistant agent (Ollama)

Unchanged design from the previous v2 draft, extended with tools for Tracks A and B.

### C.1 Decisions
1. **Local Ollama, no cloud model.** Data never leaves the tailnet. The assistant only works while the Ollama host is on; nothing else depends on it. https://docs.ollama.com/capabilities/tool-calling
2. **The agent runs inside `peer serve`**, at `POST /api/v1/assistant/chat`, inheriting v1 identity and CSRF protection.
3. **Tools wrap the same service functions as the read-only `GET` endpoints.** No new data paths.
4. **Plain `reqwest` against Ollama's `/api/chat`**, not an agent framework.
5. **`LlmClient` trait**, impls `OllamaClient` and `FakeLlmClient` (scripted, for tests).
6. **Model `qwen3`**, size per host; `options.num_ctx = 8192` (default is 4096: https://docs.ollama.com/faq).
7. **Chat history** lives in the browser and is sent with each request; not stored or synced.
8. **Read-only.** No create/edit/delete, including shopping items (§C.4).

### C.2 Tools

| Tool | Backed by |
|---|---|
| `get_next_workout(date?)` | `/workouts/next` + `/recommendations` (includes each `reason`) |
| `get_exercise_history(exercise, since?)` | sessions + `/analytics/e1rm` |
| `get_weekly_volume(week?)` | `/analytics/volume` |
| `get_prs(exercise?)` | `/prs` |
| `get_test_history(kind, since?)` | `/tests` |
| `get_todos(filter)` | `/todos` |
| `get_shopping_list(list?)` | `/lists/{id}/items` (default: "Groceries") |

Rules: current user only for training data; todos and shopping are shared. Bad args or unknown tool → `{"error": ...}` back to the model. Results capped at 8 KB with `"truncated": true`.

Example questions to support: "what's the workout today", "why did my bench recommendation go down" (answered from `reason`), "how many chest sets this week", "what's on the shopping list", "what's overdue".

### C.3 Loop
1. System prompt: role, user's name, today's date and weekday in the configured timezone, use tools for every fact, say when data is missing.
2. Send messages + tool schemas; run each returned tool call; append `{"role":"tool","tool_name":...,"content":...}`; repeat.
3. Stop with no tool calls, or after 5 iterations.
4. 60 s timeout; unreachable host → 503 `assistant_offline`.

The model only chooses tools and phrases answers. Numbers, dates, progression and volume come from deterministic code. Small local models get arithmetic and dates wrong; the design keeps them away from both.

### C.4 Why read-only
Tool results include free text written by users (todo titles, shopping item notes, session notes). With write tools, text in that data could steer the model into making changes (prompt injection). Read-only makes the impact zero. Writes with a confirm step are a later decision.

---

## Milestones (in order; each ends in something runnable)

**v2-M0 — Baseline:** tag `v1`; `cargo test --workspace` passes; carry blockers forward.

**v2-M1 — Training data model + migration** (A.2). Runnable: tests, including v1 fixture migration.

**v2-M2 — Progression engine + analytics** (A.3, A.4). Runnable: `cargo test -p fitness-core`; `peer fitness recommend --workout <id>` prints recommendations with reasons.

**v2-M3 — Training API + screens** (A.5, A.6). Runnable: build a program, log a workout with rest timer and plate calculator, see the next recommendation change after finishing.

**v2-M4 — Shopping core + SSE** (B.2). Runnable: two `curl` clients — one streaming `/events`, one adding items.

**v2-M5 — Shopping screen.** Runnable: two browser windows as two users; ticks appear in the other within ~2 s.

**v2-M6 — `assistant-core` with fake LLM** (C). Tests: "workout today" → `get_next_workout` with today's date, result reaches the answer; "why did X go down" surfaces `reason`; multi-tool turn; unknown tool; malformed args; iteration cap; truncation; unreachable host.

**v2-M7 — Chat endpoint + UI.** Shows which tools ran under each answer (collapsible). Runnable with `--fake-llm`.

**v2-M8 — Live eval harness.** `crates/assistant-core/evals/questions.toml` (~20 questions across training, todos and shopping, each with expected tools and a fact the answer must contain, against a fixed seed dataset). `cargo run -p assistant-core --example eval -- --config <path>`. Not part of `cargo test`. No reachable Ollama → owner steps to `BLOCKERS.md`; the harness must still compile.

**v2-M9 — Wrap-up:** README, `API.md`, `DECISIONS.md`, `LEARNING.md`, `BLOCKERS.md`, "What was not tested".

## LEARNING.md — new sections
Point first, then why, file paths, source link.
1. RIR, rep ranges and e1RM: how the engine turns a logged set into a strength estimate.
2. Derived vs stored data: why recommendations and analytics are computed from logs, and the trade-off (CPU at read time vs no stale or conflicting state).
3. Rule-based progression vs ML: why a transparent rule with a `reason` beats a black box for a two-person app, and where it's weak (fixed Epley curve, no per-user strength/endurance profile).
4. Server-Sent Events vs polling vs WebSockets.
5. Last-writer-wins on a single field, and why it's fine for a checkbox.
6. Tool calling and the agent loop; why the model only routes and phrases; how the eval harness measures it.
7. Context windows and why tool results are capped.
8. Prompt injection and why the agent is read-only.
9. Ollama has no authentication: bind localhost or tailnet IP, never `0.0.0.0`; restrict with Tailscale ACLs.

## Owner setup steps (write to `BLOCKERS.md` as a checklist)
1. Install Ollama: https://ollama.com/download
2. If Ollama runs on a different machine from `peer serve`: `OLLAMA_HOST=<tailnet IPv4>:11434` (https://docs.ollama.com/faq, "How can I expose Ollama on my network?") and a Tailscale ACL allowing only the web host to reach port 11434 (https://tailscale.com/kb/1018/acls). Otherwise leave the default `127.0.0.1`.
3. Optional: `OLLAMA_NO_CLOUD=1` (https://docs.ollama.com/faq).
4. `ollama pull qwen3:<size>`.
5. Set `[assistant] url` and `model` in `config.toml`.
6. Set up gym profiles in the app (plates and increments) before relying on recommendations.

## Done means
- `cargo test --workspace` passes, including v1 → v2 migration and progression tests.
- A program can be built, logged against, and produces explained recommendations for the next session.
- Shopping list updates live between two browsers.
- Assistant answers the example questions with the fake client; live eval results recorded if a host was available.
- Docs updated; every milestone committed.
