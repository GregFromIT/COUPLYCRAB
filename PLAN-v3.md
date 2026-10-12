# PLAN-v3.md — v3: SQLite common store, media lists + wheel, MTG integration, per-user themes

You are Claude Code, running **unattended**. This builds on v2 (`PLAN-v2.md`). Read this file, then `docs/DECISIONS.md`, `docs/BLOCKERS.md`, `docs/LEARNING.md` and `docs/API.md`, before starting. All earlier run rules apply (don't ask questions, log decisions and blockers, three-strike rule, cite sources, minimal implementations, pin versions, commit per milestone). Commit as `v3-M<n>: <summary>`. Tag the starting commit `v2`.

This run touches **two repos**:
- this repo (household)
- `GregFromIT/Fruit-Deck-Builder`, expected as a sibling directory `../Fruit-Deck-Builder`. Work there **only on a new branch `household-integration`**. Never push either repo. If the sibling directory is missing, skip v3-M6 and v3-M7's build flow, log it in `BLOCKERS.md`, and still build the parts of M7 that don't need the deckbuilder.

---

## 1. Common data store: SQLite

### 1.1 Decision and its cost
All household data moves from per-device Automerge documents into **one SQLite database on the web host**. The Python deckbuilder reads the same file.

Write this into `DECISIONS.md` plainly: this ends the peer-to-peer, local-first design. Since v1 the app has been a hosted web app, and Automerge only served the frozen v0 Tauri app. SQLite gives real queries, joins and constraints across modules, and one file both Rust and Python can open. The costs:
- the host is now a single point of failure, so backups become mandatory (§1.6)
- the frozen Tauri app can no longer sync with anything

### 1.2 Crates
- `rusqlite` with the `bundled` feature, so SQLite compiles in and the binary doesn't depend on a system library on Windows or the Pi (https://docs.rs/rusqlite).
- `rusqlite_migration` for versioned migrations tracked in `PRAGMA user_version` (https://docs.rs/rusqlite_migration).
- One connection behind `tokio::sync::Mutex`; calls run in `tokio::task::spawn_blocking`. Two users don't need a pool. Record this.

### 1.3 Connection settings (every connection, with sources)
- `PRAGMA journal_mode=WAL`: readers (including the Python process) don't block the writer, and vice versa (https://www.sqlite.org/wal.html).
- `PRAGMA foreign_keys=ON`: off by default and per-connection (https://www.sqlite.org/foreignkeys.html).
- `PRAGMA busy_timeout=5000`.

### 1.4 Single-writer rule
**Only the Rust server writes.** The Python deckbuilder opens the database **read-only** via a URI (`file:<path>?mode=ro`, `uri=True`; https://docs.python.org/3/library/sqlite3.html#sqlite3-uri-tricks, https://www.sqlite.org/uri.html) and returns results over HTTP. The Rust server stores them. This means:
- one owner of the schema and migrations
- no cross-language write contention
- one place to validate data

Explain this in `LEARNING.md`.

### 1.5 The store-trait seam pays off
v0 put every module's storage behind a trait (`TodoStore`, `FitnessStore`, shopping store). Add `Sqlite*Store` implementations of the **same traits**. The existing store tests become generic over the trait and run against both the Automerge and SQLite implementations, proving parity before the cutover. The HTTP API contract (`docs/API.md`) doesn't change; existing API tests must pass unchanged after the cutover. Write this up in `LEARNING.md` as the payoff of the v0 decision.

Schema: derive tables from the existing core types. IDs stay UUID text; timestamps stay Unix ms integers; enums as text with `CHECK` constraints; soft delete stays a `deleted` column. Derived data (progression recommendations, analytics, autocomplete) stays derived; don't add tables for it.

### 1.6 Migration, cutover and backups
- `peer migrate-to-sqlite --config <path>`: reads the Automerge docs, writes the database in one transaction, prints per-table counts, verifies them against the docs, and leaves the `.automerge` files untouched.
- `peer serve` uses SQLite from then on. Remove the Automerge sync loop from `serve`. Keep the `sync` crate compiling but unused; record that it's retired.
- `peer backup --out <path>` uses `VACUUM INTO` for a consistent copy of a live database (https://www.sqlite.org/lang_vacuum.html#vacuuminto).
- `deploy/peer-backup.service` + `deploy/peer-backup.timer`: nightly example; don't install.

---

## 2. Media lists and the spinning wheel

### 2.1 Scope
- Shared lists of kind `game`, `show`, `movie`.
- Item fields: title, status (`backlog` | `active` | `done` | `dropped`), `added_by`, notes, year, runtime minutes (movies/shows), platform (games), `tmdb_id` (optional).
- **Per-user rating** 1–5 on `done` items.
- **Vibe tags:** a fixed preset list (cosy, funny, tense, scary, mind-bending, feel-good, romantic, epic, dark, comfort-rewatch, short) plus free-text tags.
- Screens: one tab per kind; filters by status, vibe and max runtime; quick-add.

### 2.2 Spinning wheel
- Picks from the **current filtered list** (e.g. movies, backlog, "cosy", under 120 min).
- **Choose the result first, then animate to it.** Pick uniformly with `crypto.getRandomValues` and rejection sampling, which avoids the modulo bias of `% n` (https://developer.mozilla.org/en-US/docs/Web/API/Crypto/getRandomValues). The wheel then decelerates to land on the chosen segment.
- If more than 16 items match, sample 16 uniformly (always including the winner) for display. Every item still has an equal chance; explain why in `LEARNING.md`.
- `prefers-reduced-motion` → show the result without animation.
- Result card actions: "Start" (status → `active`) and "Spin again" (excludes the last result).
- Client-side only; no table, no endpoint.

---

## 3. Movie recommendations by vibe

### 3.1 Flow
1. **Input:** free-text vibe ("something cosy and funny, not too long") plus optional preset vibe chips and max runtime.
2. **Candidates:**
   - always: backlog movies matching chips/runtime
   - if TMDB is configured: up to 20 movies from TMDB `/discover/movie` using genre IDs from a static vibe → genre map in code, excluding titles already marked `done`
   - Discover: https://developer.themoviedb.org/reference/discover-movie; genre IDs: https://developer.themoviedb.org/reference/genre-movie-list
3. **Ranking:**
   - **If Ollama is configured** (from v2): send the vibe text and candidates (id, title, year, genres, overview ≤ 300 chars). Use a structured-output schema whose `id` field is an `enum` of the candidate IDs, so the model **can only pick from real candidates** and can't invent films (`format` with a JSON schema; https://docs.ollama.com/capabilities/structured-outputs). Return the top 3 with a one-line reason each. Temperature 0. Validate server-side.
   - **Otherwise**, or on invalid output: rank by vibe-tag overlap, then TMDB vote average. Never fail just because Ollama is down.
4. **Result cards:** "Add to watchlist", or "Spin these 3".

### 3.2 TMDB rules
- Free for non-commercial use with attribution. Add an About page with an approved TMDB logo and the required notice, as specified in the TMDB FAQ (https://developer.themoviedb.org/docs/faq). Use the logo less prominently than the app's own name.
- The API token lives in `config.toml` on the server only; it is never sent to the browser. Auth: https://developer.themoviedb.org/docs/authentication-application
- Cache TMDB responses in a `tmdb_cache` table for 24 h. Respect rate limits: https://developer.themoviedb.org/docs/rate-limiting

### 3.3 Prompt injection note
TMDB overviews are third-party text the model reads. Impact is limited because the model has no tools and its output is constrained to candidate IDs plus a short reason. The UI renders the reason as text, never HTML. Record this in `LEARNING.md`.

---

## 4. MTG: Fruit-Deck-Builder integration, deck projects, buylists

### 4.1 What the deckbuilder is today (read its README and code to confirm)
- Python + Flask on `127.0.0.1:5000`, with an HTML form UI.
- `deckbuilder.build(spec, taste, log=True)` returns a deck dict (commander, cards with role/cmc/price/score, lands, curve, roles, warnings, `build_id`).
- `to_decklist()` exports Moxfield/Archidekt text.
- Feedback (kept/cut/added) goes to append-only `feedback.jsonl`; `train_readout.py` trains the fly readout from it and `decks/mine/`.
- Prices are Scryfall **USD** per card; Scryfall has no AUD. Show USD and label it.

### 4.2 Changes in `../Fruit-Deck-Builder` (branch `household-integration`, v3-M6)
1. **JSON API** in `app.py`, keeping the existing HTML UI working:
   - `GET /api/themes?commander=` → themes and budget subsets
   - `POST /api/build` with a `DeckSpec` JSON body → the deck dict **plus** the pool card names, with `log=False` (the household app records builds; §1.4)
   - `POST /api/prices` with `{"names": [...]}` → `{name: price_usd | null}`, using the existing Scryfall client (keeps its required headers and cache)
2. **Read feedback from SQLite:** add a loader in `feedback.py` for the household `mtg_feedback` table, opened read-only. Add `train_readout.py --db <path>`. Keep `feedback.jsonl` support so the standalone flow still works.
3. **Run safely:** `app.run(host="127.0.0.1", debug=False)` unless `FLASK_DEBUG=1`. The Werkzeug debugger allows running arbitrary code from the browser and must never be reachable (https://flask.palletsprojects.com/en/stable/debugging/).
4. **Repo hygiene:** the repo has a committed `.venv/`. Remove it from the index with `git rm -r --cached .venv` and add it to `.gitignore` (https://git-scm.com/docs/git-rm). Note it in that repo's README.
5. **Tests:** pytest with Flask's test client (https://flask.palletsprojects.com/en/stable/testing/) for the three endpoints, with EDHREC and Scryfall mocked.
6. Provide `deploy/deckbuilder.service` (systemd example; don't install).
7. **Archidekt deck read:** `GET /api/archidekt/deck/{id}` returns `{name, commander, cards: [{name, qty, category}]}` for a **public** deck. Reuse and verify the Archidekt code in `fetch_training_decks.py` (its README flags the response shapes as guessed). Archidekt's API is undocumented (https://www.npmjs.com/package/archidekt), so treat it like EDHREC: cache responses for 24 h, send a descriptive `User-Agent`, and return a clear error rather than crashing when the shape changes. Read-only; no login, no private decks, no writes to Archidekt.

### 4.3 Household side (v3-M7)
#### Collection source of truth: ManaBox (shared account)
Both users keep one shared collection in a single ManaBox account. ManaBox has **no public API**: its Cloud Sync only syncs ManaBox's own devices and isn't even visible on ManaBox's web version (https://manabox.app/guides/sync/cloud-sync/). The only supported way out is a **CSV export of the whole collection** (https://manabox.app/guides/collection/import-export/). So:
- **ManaBox stays the source of truth; the household app holds a read-only snapshot.** Sync is one-way, ManaBox → household, by uploading a fresh export. Do not reverse-engineer ManaBox's sync API or automate its app: that's fragile, likely against its terms, and needs account credentials on the server.
- **Import replaces the snapshot.** It doesn't add to it, because ManaBox exports the whole collection, so re-importing never double-counts. Before committing, show a diff (cards added/removed, quantity changes) and require confirmation.
- **Parse by header name, not column position.** Known export headers include Name, Set Code, Set Name, Collector Number, Foil, Rarity, Quantity, ManaBox ID, Scryfall ID, Purchase Price, Misprint, Altered, Condition, Language, Purchase Price Currency, but the columns vary with how the collection is organised (https://tappedout.net/mtg-forum/tappedout/importing-csv-from-manabox). Require Name + Quantity; use Scryfall ID when present; ignore unknown columns; skip blank lines. Use the `csv` crate (https://docs.rs/csv). Record the observed header set in `DECISIONS.md`.
- **Matching:** the snapshot keys on Scryfall ID (exact printing). Decks and the buylist match by **card name**, summing all printings, because a deck needs "a Sol Ring", not a specific printing.

**Tables** (collection shared; decks per user, readable by both):
- `mtg_collection`: scryfall_id (nullable), name, set_code, collector_number, foil, condition, language, qty, import_id
- `mtg_collection_imports`: id, imported_at, imported_by, file SHA-256, row count, added/removed/changed counts
- `mtg_pending_adds`: name, qty, added_by, added_at, reason (`bought`)
- `mtg_decks`: owner, name, commander, status (`idea` | `building` | `built` | `retired`), notes, optional `source_build_id`, optional `archidekt_id`
- `mtg_deck_cards`: deck, card name, qty, role/category, board (`main` | `maybe`)
- `mtg_builds`: id, spec JSON, proposed deck JSON, pool names JSON, created_at
- `mtg_feedback`: build_id, card name, label (`kept` | `cut` | `added`), at
- `mtg_prices`: card name, `price_usd`, checked_at

**Derived, not stored:**
- **Owned qty per card name** = snapshot qty + pending adds.
- **Buylist** = across decks in `building`, cards needed minus owned qty, summed, with price and total. Shared, since the collection is shared.
- "Owned" / "pending" badges on deck cards.

#### Pending adds: the gap a one-way sync leaves
The app can't write to ManaBox, so "Bought" on the buylist creates a **pending add**. It counts as owned immediately and shows on a "Scan into ManaBox" list. On the next ManaBox import, each pending add whose card name now appears with enough quantity is cleared automatically; anything still missing stays pending and is flagged. Explain this reconciliation in `LEARNING.md`.

#### Archidekt (deck projects only)
- **Import:** paste an Archidekt deck URL or ID → the server calls the deckbuilder's `/api/archidekt/deck/{id}` (§4.2 item 7) → creates or updates a deck project with `archidekt_id`. "Refresh from Archidekt" re-pulls it and shows a diff before applying. Public decks only; the UI says so.
- **Export:** the deck export text (same format as `to_decklist()`, `1 Card Name` lines) goes into Archidekt's own text importer. Verify that format against Archidekt's importer during the run if reachable; otherwise note it as untested.
- **No writes to Archidekt and no Archidekt credentials** on the server.
- **Collection → Archidekt** is out of scope for the app. If the owner wants the collection in Archidekt too, they upload the ManaBox CSV to Archidekt's collection importer (https://archidekt.com/collections/import) themselves. Document that ManaBox-CSV compatibility with Archidekt is unverified.

**Flows:**
1. **Build:** the Build screen calls `POST /api/v1/mtg/build`; the server proxies to the deckbuilder (`[mtg] deckbuilder_url`), stores `mtg_builds`, and returns the deck. Builds can take a minute or more (EDHREC + Scryfall), so use a 180 s timeout and show progress.
2. **Review:** a tick-box list like the Flask UI, plus an "added cards" box. Submit writes `mtg_feedback` and creates a deck project from the kept + added cards.
3. **Decks:** edit cards, move to/from maybe-board, export; import/refresh from Archidekt.
4. **Collection:** upload a ManaBox CSV → diff preview → confirm. A search view over the snapshot. An import history list.
5. **Buylist:** grouped by card with deck names, prices and total. "Bought" creates a pending add. "Refresh prices" calls the deckbuilder's `/api/prices` and caches results for 24 h.
6. **Scan into ManaBox:** the list of pending adds, cleared by the next import.
7. **Legacy import:** `peer mtg import-legacy --feedback <feedback.jsonl> --decks <decks/mine>` (optional; skip if the files aren't present).

**Upload limits:** the CSV upload is the one endpoint that takes a file. Accept `multipart/form-data` up to 10 MiB on that route only (keep the 64 KiB JSON limit elsewhere), and keep the v1 `Origin` check. Since this route can't require a JSON content type, also require a custom header `X-Requested-With: household` that a cross-site form can't send without a CORS preflight. Explain why in `LEARNING.md`.

**API:** `/mtg/decks`, `/mtg/decks/{id}/cards`, `POST /mtg/decks/import-archidekt`, `POST /mtg/decks/{id}/refresh-archidekt`, `/mtg/collection?q=`, `POST /mtg/collection/import` (upload → preview with an `import_token`), `POST /mtg/collection/import/{token}/commit`, `/mtg/collection/imports`, `/mtg/pending-adds`, `/mtg/buylist`, `PUT /mtg/buylist/bought`, `/mtg/themes?commander=`, `POST /mtg/build`, `PUT /mtg/builds/{id}/feedback`, `POST /mtg/prices/refresh`. Same conventions as v1.

**Deckbuilder unavailable** → 503 `deckbuilder_offline`; everything else in MTG still works.

---

## 5. Per-user themes

- `user_prefs`: theme preset, mode (`light` | `dark` | `system`), accent colour.
- Presets are sets of CSS custom properties (the frontend already uses tokens). Ship ~5 presets plus a custom accent.
- **No flash of the wrong theme:** the server knows the user from the identity header at page load, so it injects `data-theme`, `data-mode` and the accent onto `<html>` in the served HTML. `system` follows `prefers-color-scheme`.
- **Contrast check:** reject an accent that fails WCAG 2.1 AA contrast (4.5:1 for text) against its background, with a message showing the ratio (https://www.w3.org/TR/WCAG21/#contrast-minimum). Formula in code with the source cited; unit-tested.
- Settings screen with a live preview.
- API: `GET/PUT /me/prefs`.

---

## 6. Assistant agent additions (read-only, as in v2)

| Tool | Backed by |
|---|---|
| `get_media_list(kind, status?, vibe?, max_runtime?)` | `/media` |
| `recommend_movie(vibe, max_runtime?)` | §3 pipeline |
| `get_buylist()` | `/mtg/buylist` |
| `get_deck(name)` | `/mtg/decks` |

Add ~8 questions to the eval set ("what should we watch tonight, something cosy", "what's on my MTG buylist and what's it cost", "what games are in the backlog").

---

## 7. Milestones (in order; each ends in something runnable)

**v3-M0 — Baseline:** tag `v2`; `cargo test --workspace` passes; carry blockers forward.

**v3-M1 — SQLite foundation:** schema, migrations, `Sqlite*Store` impls; generic store tests pass on both implementations.

**v3-M2 — Cutover:** `migrate-to-sqlite` (tested on a v2 fixture: counts and spot values match), `serve` on SQLite, sync retired from `serve`, `backup` command + timer example. All existing API tests pass unchanged.

**v3-M3 — Per-user themes** (§5).

**v3-M4 — Media lists + wheel** (§2). Tests: filters; the spin picker is uniform (chi-square over 10,000 picks on a seeded RNG); sampling for display always includes the winner.

**v3-M5 — Vibe recommendations** (§3). Tests with a fake TMDB server and the v2 `FakeLlmClient`: the enum schema contains exactly the candidate IDs; an invalid model output falls back to the deterministic ranking; `done` titles are excluded; the token never appears in any response.

**v3-M6 — Deckbuilder repo changes** (§4.2), on branch `household-integration` in `../Fruit-Deck-Builder`.

**v3-M7 — MTG module** (§4.3). Tests:
- ManaBox import: header-name parsing with reordered and extra columns; blank lines; missing Scryfall ID; missing required columns rejected; re-importing the same file is a no-op diff; replace (not add) semantics.
- Buylist maths: multiple decks, partial ownership, several printings of one card summed by name.
- Pending adds: count as owned; cleared by an import that contains them; kept and flagged when the import doesn't.
- Archidekt: fixture response → deck project; refresh diff; deckbuilder offline → 503.
- Upload route rejects requests without `X-Requested-With`, with a foreign `Origin`, or over 10 MiB.
- Decklist round-trip parse/export; feedback rows match the review submission.

**v3-M8 — Agent tools + eval questions** (§6).

**v3-M9 — Wrap-up:** README (both repos), `API.md`, `DECISIONS.md`, `LEARNING.md`, `BLOCKERS.md`, "What was not tested".

## 8. LEARNING.md — new sections
Point first, then why, file paths, source link.
1. Leaving local-first: what SQLite gained, what was given up, and why backups now matter.
2. WAL mode and concurrent readers across processes.
3. The single-writer rule and read-only URIs.
4. How the store traits made the storage swap a contained change.
5. Migrations and `user_version`.
6. Fair randomness: modulo bias, rejection sampling, why decide-then-animate is honest.
7. Constrained generation: using a JSON-schema `enum` so a model can only choose real options.
8. Proxying a second service (the deckbuilder) behind one API, and why it stays on localhost.
9. Why `debug=True` in Flask is a remote-code-execution risk.
10. Contrast ratios and why theme validation exists.
11. One-way sync from a system with no API: snapshot-replace imports, diffs before commit, and reconciling pending local changes.
12. Why the file-upload route needs a different CSRF defence than JSON routes.
13. Building on undocumented APIs (Archidekt, EDHREC): caching, graceful failure, and never sending credentials.

## 9. Owner setup steps (write to `BLOCKERS.md` as a checklist)
1. Clone `GregFromIT/Fruit-Deck-Builder` next to this repo and set up its Python environment per its README; review and merge the `household-integration` branch yourself.
2. Run the deckbuilder on the host on `127.0.0.1:5000` (example: `deploy/deckbuilder.service`). Don't expose it with `tailscale serve`; the household server proxies it.
3. Set `[mtg] deckbuilder_url` and `db_path` in `config.toml`.
4. Export the shared collection from ManaBox (collection tab → top-right menu → export CSV; https://manabox.app/guides/collection/import-export/) and upload it on the Collection screen. Repeat after scanning sessions; the "Scan into ManaBox" list shows what's still missing.
5. For Archidekt imports, make sure the decks are public.
6. Optional: get a TMDB API read token (account settings → API; https://developer.themoviedb.org/docs/faq) and set `[tmdb] token`.
7. Run `peer migrate-to-sqlite` once, check the counts, then restart `peer serve`.
8. Install the backup timer and copy backups to a second device.

## 10. Done means
- `cargo test --workspace` passes on SQLite, with v1/v2 API tests unchanged.
- Migration from a v2 fixture verified; backup command works.
- Media lists, wheel and vibe recommendations work, with and without TMDB/Ollama.
- MTG build → review → deck → buylist flow works against the deckbuilder (or is documented as blocked); deckbuilder repo changes are on their branch with tests.
- A ManaBox CSV imports with a diff preview; the buylist reflects the shared collection plus pending adds; Archidekt public decks import as deck projects.
- Per-user themes apply on first paint and enforce contrast.
- Docs updated; every milestone committed.
