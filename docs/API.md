# Data API

JSON over HTTP, served by `peer serve` under `/api/v1` (the path stays `v1`; v2 only adds endpoints and fields, and retires some). The web frontend uses only this API (`web/src/api.ts`), and the v2 assistant's tools are built from the **read-only** endpoints. Code: `crates/web/src/api.rs` (todos, exercises, sessions, tests), `crates/web/src/training.rs` (v2 training), `crates/web/src/shopping.rs` and `crates/web/src/events.rs` (v2 shopping and live updates), `crates/web/src/identity.rs`.

Every example below is a real request against `peer serve --dev-user alice` (`timezone = "Australia/Sydney"`, run on 2026-10-12), in the order shown. The v1 examples come from an empty data dir; the v2 section (at the end) from another fresh one, and its migration examples from a data dir holding a real v1 document. Long lists are cut to two entries plus `"…"`.

## Conventions

**Read-only endpoints** (safe for an assistant to call freely): every `GET`. That's `/health`, `/me`, `/todos`, `/exercises`, `/sessions`, `/sessions/{id}`, `/tests`, and in v2 `/gym-profiles`, `/programs`, `/programs/{id}`, `/workouts/next`, `/recommendations`, `/analytics/volume`, `/analytics/e1rm`, `/prs`, and the shopping endpoints `/lists`, `/lists/{id}/items`, `/lists/{id}/suggestions` and the `/events` stream (plus the v1 `/templates`, `/templates/{id}`, `/plan`, `/blocks`, `/planned-day`, `/program-status` until they are retired; see below). Recommendations and analytics are computed from your logs on every read: nothing is stored, and asking changes nothing.

**Identity.** The current user comes from `--dev-user`, else the `Tailscale-User-Login` header that `tailscale serve` adds (mapped through `[web.users]` in `config.toml`), else the picker cookie (`PUT /me`). Without one, every endpoint except `/health` and `/me` returns `401 no_user`. See `docs/LEARNING.md` v1.3.

**Shared vs per-user.** Todos, exercises, shopping lists (and v1 templates) are shared: either user can change them. Gym profiles, programs, sessions, tests and external activities (and v1 plans and blocks) belong to a user. Reads default to the current user; `?user=<id>` reads the other user's. Writing the other user's resource returns `403`.

**Creating = `PUT` with a client-generated ID** (e.g. `crypto.randomUUID()`; IDs are 1–64 chars of `A-Z a-z 0-9 - _`). First time: `201` + the resource. Same request again: `200`, nothing changes. Same ID with different content: `409 conflict` (use `PATCH` to change things). So a retried request can never create a duplicate. See `docs/LEARNING.md` v1.2.

**Writes** (`PUT`, `PATCH`, `POST`, `DELETE`) must send `Content-Type: application/json`, even with no body, else `415`. If the request has an `Origin` header it must be one of `[web] origins` (default `http://127.0.0.1:<port>` and `http://localhost:<port>`), else `403 bad_origin`. Bodies over 64 KiB get `413`. The server sends no CORS headers. See `docs/LEARNING.md` v1.4.

**Dates** are `YYYY-MM-DD`. "Today" is in `[web] timezone`. Timestamps (`started_at`, `created_at`, …) are Unix milliseconds, UTC. Loads are kg.

**Errors** always look like:

```json
{ "error": { "code": "conflict", "message": "todo t1 already exists with different content" } }
```

| Status | `code` | When |
|---|---|---|
| 400 | `invalid`, `bad_json` | Validation failed (bad date, empty title, string over its cap: titles/names/labels 200 chars, notes 2000), unparseable JSON |
| 401 | `no_user` | No identity; pick a user with `PUT /me` |
| 403 | `forbidden`, `bad_origin` | Writing the other user's data; foreign `Origin` |
| 404 | `not_found` | Unknown ID or endpoint |
| 410 | `gone` | A v1 template/plan/block endpoint after programs exist; body also has `replacement` |
| 409 | `conflict` | `PUT` with an existing ID but different content |
| 413 | `too_large` | Body over 64 KiB |
| 415 | `unsupported_media_type` | Write without `Content-Type: application/json` |
| 500 | `internal` | Saving to disk failed, or a bug |

## Endpoints

### GET /health
**Read-only.**
Liveness. No identity needed.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/health'
```
`200`
```json
{
  "status": "ok"
}
```

### GET /me
**Read-only.**
Current user, the users the picker may offer, and the decoded `Tailscale-User-Login` (null with `--dev-user`). `user_id` is null until a user is identified or picked; the frontend then shows the picker.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/me'
```
`200`
```json
{
  "user_id": "alice",
  "users": [
    "alice"
  ],
  "login": null
}
```

### PUT /me
User picker: sets an `HttpOnly; SameSite=Strict` cookie `user=<id>`. Only users allowed for this request (all known users, or the users mapped to a shared Tailscale login) can be picked; otherwise 403. With `--dev-user` the cookie is ignored.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/me \
  -H 'Content-Type: application/json' \
  -d '{"user_id": "alice"}'
```
`200`
```json
{
  "user_id": "alice",
  "users": [
    "alice"
  ],
  "login": null
}
```

### PUT /todos/{id}
Create a todo with a client-chosen ID. `title` (≤200 chars) required, `notes` (≤2000) and `due` (`YYYY-MM-DD`) optional. Repeating the same request returns 200 and changes nothing; the same ID with different content returns 409.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/todos/0199f2a0-0000-7000-8000-000000000001 \
  -H 'Content-Type: application/json' \
  -d '{"title": "Buy milk", "due": "2026-10-01"}'
```
`201` (created)
```json
{
  "id": "0199f2a0-0000-7000-8000-000000000001",
  "title": "Buy milk",
  "notes": "",
  "due": "2026-10-01",
  "completed": false,
  "deleted": false,
  "created_at": 1791789869869
}
```
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/todos/0199f2a0-0000-7000-8000-000000000001 \
  -H 'Content-Type: application/json' \
  -d '{"title": "Buy milk", "due": "2026-10-01"}'
```
`200` (repeat: unchanged)
```json
{
  "id": "0199f2a0-0000-7000-8000-000000000001",
  "title": "Buy milk",
  "notes": "",
  "due": "2026-10-01",
  "completed": false,
  "deleted": false,
  "created_at": 1791789869869
}
```
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/todos/0199f2a0-0000-7000-8000-000000000001 \
  -H 'Content-Type: application/json' \
  -d '{"title": "Buy bread"}'
```
`409` (same ID, different content)
```json
{
  "error": {
    "code": "conflict",
    "message": "todo 0199f2a0-0000-7000-8000-000000000001 already exists with different content"
  }
}
```

### GET /todos
**Read-only.**
Todos (shared by both users), dated ascending then undated. `filter`: omitted/`all`, `open` (not completed), `overdue` (open, due before today in the configured timezone), `undated` (open, no due date), `due_between` (open, `from` ≤ due ≤ `to`, either bound optional).
```sh
curl -s 'http://127.0.0.1:7430/api/v1/todos'
```
`200`
```json
[
  {
    "id": "0199f2a0-0000-7000-8000-000000000001",
    "title": "Buy milk",
    "notes": "",
    "due": "2026-10-01",
    "completed": false,
    "deleted": false,
    "created_at": 1791789869869
  },
  {
    "id": "0199f2a0-0000-7000-8000-000000000002",
    "title": "Book dentist",
    "notes": "",
    "due": "2026-10-20",
    "completed": false,
    "deleted": false,
    "created_at": 1791789869889
  },
  "…"
]
```
```sh
curl -s 'http://127.0.0.1:7430/api/v1/todos?filter=overdue'
```
`200`
```json
[
  {
    "id": "0199f2a0-0000-7000-8000-000000000001",
    "title": "Buy milk",
    "notes": "",
    "due": "2026-10-01",
    "completed": false,
    "deleted": false,
    "created_at": 1791789869869
  }
]
```
```sh
curl -s 'http://127.0.0.1:7430/api/v1/todos?filter=due_between&from=2026-10-12&to=2026-10-31'
```
`200`
```json
[
  {
    "id": "0199f2a0-0000-7000-8000-000000000002",
    "title": "Book dentist",
    "notes": "",
    "due": "2026-10-20",
    "completed": false,
    "deleted": false,
    "created_at": 1791789869889
  }
]
```

### PATCH /todos/{id}
Change any of `title`, `notes`, `due`. `"due": null` clears the due date; an absent field is left alone.
```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/todos/0199f2a0-0000-7000-8000-000000000002 \
  -H 'Content-Type: application/json' \
  -d '{"notes": "Call after 9", "due": null}'
```
`200`
```json
{
  "id": "0199f2a0-0000-7000-8000-000000000002",
  "title": "Book dentist",
  "notes": "Call after 9",
  "due": null,
  "completed": false,
  "deleted": false,
  "created_at": 1791789869889
}
```

### POST /todos/{id}/complete
Mark done. Idempotent. Needs the JSON content type even with no body.
```sh
curl -s -X POST http://127.0.0.1:7430/api/v1/todos/0199f2a0-0000-7000-8000-000000000001/complete -H 'Content-Type: application/json'
```
`200`
```json
{
  "id": "0199f2a0-0000-7000-8000-000000000001",
  "title": "Buy milk",
  "notes": "",
  "due": "2026-10-01",
  "completed": true,
  "deleted": false,
  "created_at": 1791789869869
}
```

### POST /todos/{id}/uncomplete
Mark not done. Idempotent.
```sh
curl -s -X POST http://127.0.0.1:7430/api/v1/todos/0199f2a0-0000-7000-8000-000000000001/uncomplete -H 'Content-Type: application/json'
```
`200`
```json
{
  "id": "0199f2a0-0000-7000-8000-000000000001",
  "title": "Buy milk",
  "notes": "",
  "due": "2026-10-01",
  "completed": false,
  "deleted": false,
  "created_at": 1791789869869
}
```

### DELETE /todos/{id}
Soft delete (`deleted: true`; hidden from lists, recoverable in the doc). 204, no body.
```sh
curl -s -X DELETE http://127.0.0.1:7430/api/v1/todos/0199f2a0-0000-7000-8000-000000000003 -H 'Content-Type: application/json'
```
`204`

### PUT /exercises/{id}
Create an exercise (shared library). `measurement`: `WeightReps`, `Reps`, `Timed`, `Distance`. Optional `tempo` (≤20 chars).
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/exercises/squat \
  -H 'Content-Type: application/json' \
  -d '{"name": "Back squat", "measurement": "WeightReps", "tempo": "2:1"}'
```
`201`
```json
{
  "id": "squat",
  "name": "Back squat",
  "measurement": "WeightReps",
  "tempo": "2:1",
  "deleted": false
}
```

### PATCH /exercises/{id}
Change `name`, `measurement`, `tempo` (`null` clears).
```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/exercises/squat \
  -H 'Content-Type: application/json' \
  -d '{"tempo": "3:1"}'
```
`200`
```json
{
  "id": "squat",
  "name": "Back squat",
  "measurement": "WeightReps",
  "tempo": "3:1",
  "deleted": false
}
```

### GET /exercises
**Read-only.**
All exercises, by name.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/exercises'
```
`200`
```json
[
  {
    "id": "squat",
    "name": "Back squat",
    "measurement": "WeightReps",
    "tempo": "3:1",
    "deleted": false
  },
  {
    "id": "plank",
    "name": "Plank",
    "measurement": "Timed",
    "tempo": null,
    "deleted": false
  },
  "…"
]
```

### PUT /templates/{id}

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
Create a workout template (shared). `items` is ordered; each has `exercise_id` (must exist), `target_sets` (1–100) and optional `target_reps`, `target_load` (kg), `tempo`, `rest_s`, `progression` (`{"kind": "LinearLoad", "per_week": kg}` or `{"kind": "LinearReps", "per_week": reps}`). At most 50 items. The response's `items` is a map keyed by generated item ID, ordered by `index`.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/templates/strength-a \
  -H 'Content-Type: application/json' \
  -d '{"name": "Strength A", "target_minutes": 50, "items": [{"exercise_id": "squat", "target_sets": 3, "target_reps": 5, "target_load": 100.0, "rest_s": 180, "progression": {"kind": "LinearLoad", "per_week": 2.5}}, {"exercise_id": "pushup", "target_sets": 3, "target_reps": 10, "progression": {"kind": "LinearReps", "per_week": 1}}, {"exercise_id": "plank", "target_sets": 2}]}'
```
`201`
```json
{
  "id": "strength-a",
  "name": "Strength A",
  "target_minutes": 50,
  "items": {
    "01a12eff-aba5-73b8-a6a5-f0bd969aa17a": {
      "index": 1,
      "exercise_id": "pushup",
      "target_sets": 3,
      "target_reps": 10,
      "target_load": null,
      "tempo": null,
      "rest_s": null,
      "progression": {
        "kind": "LinearReps",
        "per_week": 1.0
      }
    },
    "01a12eff-aba5-73b8-a6a5-f0be1d755284": {
      "index": 2,
      "exercise_id": "plank",
      "target_sets": 2,
      "target_reps": null,
      "target_load": null,
      "tempo": null,
      "rest_s": null,
      "progression": null
    },
    "01a12eff-aba5-73b8-a6a5-f0bc3808f383": {
      "index": 0,
      "exercise_id": "squat",
      "target_sets": 3,
      "target_reps": 5,
      "target_load": 100.0,
      "tempo": null,
      "rest_s": 180,
      "progression": {
        "kind": "LinearLoad",
        "per_week": 2.5
      }
    }
  },
  "deleted": false
}
```

### GET /templates/{id}

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
**Read-only.**
One template.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/templates/strength-a'
```
`200`
```json
{
  "id": "strength-a",
  "name": "Strength A",
  "target_minutes": 50,
  "items": {
    "01a12eff-aba5-73b8-a6a5-f0bc3808f383": {
      "index": 0,
      "exercise_id": "squat",
      "target_sets": 3,
      "target_reps": 5,
      "target_load": 100.0,
      "tempo": null,
      "rest_s": 180,
      "progression": {
        "kind": "LinearLoad",
        "per_week": 2.5
      }
    },
    "01a12eff-aba5-73b8-a6a5-f0bd969aa17a": {
      "index": 1,
      "exercise_id": "pushup",
      "target_sets": 3,
      "target_reps": 10,
      "target_load": null,
      "tempo": null,
      "rest_s": null,
      "progression": {
        "kind": "LinearReps",
        "per_week": 1.0
      }
    },
    "01a12eff-aba5-73b8-a6a5-f0be1d755284": {
      "index": 2,
      "exercise_id": "plank",
      "target_sets": 2,
      "target_reps": null,
      "target_load": null,
      "tempo": null,
      "rest_s": null,
      "progression": null
    }
  },
  "deleted": false
}
```

### PATCH /templates/{id}

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
Change `name`, `target_minutes`, or replace all `items`.
```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/templates/strength-a \
  -H 'Content-Type: application/json' \
  -d '{"target_minutes": 55}'
```
`200`
```json
{
  "id": "strength-a",
  "name": "Strength A",
  "target_minutes": 55,
  "items": {
    "01a12eff-aba5-73b8-a6a5-f0bd969aa17a": {
      "index": 1,
      "exercise_id": "pushup",
      "target_sets": 3,
      "target_reps": 10,
      "target_load": null,
      "tempo": null,
      "rest_s": null,
      "progression": {
        "kind": "LinearReps",
        "per_week": 1.0
      }
    },
    "01a12eff-aba5-73b8-a6a5-f0be1d755284": {
      "index": 2,
      "exercise_id": "plank",
      "target_sets": 2,
      "target_reps": null,
      "target_load": null,
      "tempo": null,
      "rest_s": null,
      "progression": null
    },
    "01a12eff-aba5-73b8-a6a5-f0bc3808f383": {
      "index": 0,
      "exercise_id": "squat",
      "target_sets": 3,
      "target_reps": 5,
      "target_load": 100.0,
      "tempo": null,
      "rest_s": 180,
      "progression": {
        "kind": "LinearLoad",
        "per_week": 2.5
      }
    }
  },
  "deleted": false
}
```

### GET /templates

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
**Read-only.**
All templates, by name.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/templates'
```
`200`
```json
[
  {
    "id": "strength-a",
    "name": "Strength A",
    "target_minutes": 55,
    "items": {
      "01a12eff-aba5-73b8-a6a5-f0be1d755284": {
        "index": 2,
        "exercise_id": "plank",
        "target_sets": 2,
        "target_reps": null,
        "target_load": null,
        "tempo": null,
        "rest_s": null,
        "progression": null
      },
      "01a12eff-aba5-73b8-a6a5-f0bc3808f383": {
        "index": 0,
        "exercise_id": "squat",
        "target_sets": 3,
        "target_reps": 5,
        "target_load": 100.0,
        "tempo": null,
        "rest_s": 180,
        "progression": {
          "kind": "LinearLoad",
          "per_week": 2.5
        }
      },
      "01a12eff-aba5-73b8-a6a5-f0bd969aa17a": {
        "index": 1,
        "exercise_id": "pushup",
        "target_sets": 3,
        "target_reps": 10,
        "target_load": null,
        "tempo": null,
        "rest_s": null,
        "progression": {
          "kind": "LinearReps",
          "per_week": 1.0
        }
      }
    },
    "deleted": false
  }
]
```

### PUT /plan

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
Replace the current user's weekly plan. `slots` keys: `mon_am`, `mon_pm`, … `sun_pm`. Each slot has exactly one of `template_id` or `external_label` (≤200 chars, e.g. a unit PT session). Missing keys are empty slots.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/plan \
  -H 'Content-Type: application/json' \
  -d '{"slots": {"mon_am": {"template_id": "strength-a"}, "mon_pm": {"external_label": "Unit PT"}, "thu_am": {"template_id": "strength-a"}}}'
```
`200`
```json
{
  "user_id": "alice",
  "slots": {
    "mon_am": {
      "template_id": "strength-a",
      "external_label": null
    },
    "mon_pm": {
      "template_id": null,
      "external_label": "Unit PT"
    },
    "thu_am": {
      "template_id": "strength-a",
      "external_label": null
    }
  }
}
```

### GET /plan

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
**Read-only.**
Weekly plan of the current user, or `?user=<id>` for the other user. Empty `slots` if none.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/plan'
```
`200`
```json
{
  "user_id": "alice",
  "slots": {
    "mon_pm": {
      "template_id": null,
      "external_label": "Unit PT"
    },
    "thu_am": {
      "template_id": "strength-a",
      "external_label": null
    },
    "mon_am": {
      "template_id": "strength-a",
      "external_label": null
    }
  }
}
```

### PUT /blocks/{id}

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
Create a program block for the current user. `start_date` must be a Monday; `weeks` 1–104; `deload_week` (1-based) and `deload_pct` (0–100, % of the progressed load) go together or not at all. The block repeats the owner's weekly plan.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/blocks/base-1 \
  -H 'Content-Type: application/json' \
  -d '{"name": "Base 1", "start_date": "2026-09-28", "weeks": 6, "deload_week": 6, "deload_pct": 60}'
```
`201`
```json
{
  "id": "base-1",
  "user_id": "alice",
  "name": "Base 1",
  "start_date": "2026-09-28",
  "weeks": 6,
  "deload_week": 6,
  "deload_pct": 60.0,
  "deleted": false
}
```

### PATCH /blocks/{id}

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
Change `name`, `start_date`, `weeks`, `deload_week`, `deload_pct` (`null` clears), `deleted`. Only the owner; otherwise 403.
```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/blocks/base-1 \
  -H 'Content-Type: application/json' \
  -d '{"name": "Base block 1"}'
```
`200`
```json
{
  "id": "base-1",
  "user_id": "alice",
  "name": "Base block 1",
  "start_date": "2026-09-28",
  "weeks": 6,
  "deload_week": 6,
  "deload_pct": 60.0,
  "deleted": false
}
```

### GET /blocks

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
**Read-only.**
Blocks of the current user (or `?user=`), by start date.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/blocks'
```
`200`
```json
[
  {
    "id": "base-1",
    "user_id": "alice",
    "name": "Base block 1",
    "start_date": "2026-09-28",
    "weeks": 6,
    "deload_week": 6,
    "deload_pct": 60.0,
    "deleted": false
  },
  {
    "id": "base-2",
    "user_id": "alice",
    "name": "Base 2",
    "start_date": "2026-11-09",
    "weeks": 4,
    "deload_week": null,
    "deload_pct": null,
    "deleted": false
  }
]
```

### GET /planned-day

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
**Read-only.**
The resolved plan for one day: block context (`week` of `weeks`, `is_deload`) and each slot's concrete targets with progression and deload applied. `date` defaults to today in the configured timezone; `?user=` for the other user. Outside any block: `block: null`, no slots.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/planned-day?date=2026-10-12'
```
`200`
```json
{
  "date": "2026-10-12",
  "block": {
    "block_id": "base-1",
    "name": "Base block 1",
    "week": 3,
    "weeks": 6,
    "is_deload": false
  },
  "slots": [
    {
      "slot": "AM",
      "template": {
        "template_id": "strength-a",
        "name": "Strength A",
        "target_minutes": 55,
        "items": [
          {
            "exercise_id": "squat",
            "exercise_name": "Back squat",
            "target_sets": 3,
            "target_reps": 5,
            "target_load": 105.0,
            "tempo": null,
            "rest_s": 180
          },
          {
            "exercise_id": "pushup",
            "exercise_name": "Push-up",
            "target_sets": 3,
            "target_reps": 12,
            "target_load": null,
            "tempo": null,
            "rest_s": null
          },
          {
            "exercise_id": "plank",
            "exercise_name": "Plank",
            "target_sets": 2,
            "target_reps": null,
            "target_load": null,
            "tempo": null,
            "rest_s": null
          }
        ]
      },
      "external_label": null
    },
    {
      "slot": "PM",
      "template": null,
      "external_label": "Unit PT"
    }
  ]
}
```
```sh
curl -s 'http://127.0.0.1:7430/api/v1/planned-day?date=2026-11-02'
```
`200` (deload week: load × 60 %)
```json
{
  "date": "2026-11-02",
  "block": {
    "block_id": "base-1",
    "name": "Base block 1",
    "week": 6,
    "weeks": 6,
    "is_deload": true
  },
  "slots": [
    {
      "slot": "AM",
      "template": {
        "template_id": "strength-a",
        "name": "Strength A",
        "target_minutes": 55,
        "items": [
          {
            "exercise_id": "squat",
            "exercise_name": "Back squat",
            "target_sets": 3,
            "target_reps": 5,
            "target_load": 67.5,
            "tempo": null,
            "rest_s": 180
          },
          {
            "exercise_id": "pushup",
            "exercise_name": "Push-up",
            "target_sets": 3,
            "target_reps": 15,
            "target_load": null,
            "tempo": null,
            "rest_s": null
          },
          {
            "exercise_id": "plank",
            "exercise_name": "Plank",
            "target_sets": 2,
            "target_reps": null,
            "target_load": null,
            "tempo": null,
            "rest_s": null
          }
        ]
      },
      "external_label": null
    },
    {
      "slot": "PM",
      "template": null,
      "external_label": "Unit PT"
    }
  ]
}
```
```sh
curl -s 'http://127.0.0.1:7430/api/v1/planned-day?date=2027-03-01'
```
`200` (outside any block)
```json
{
  "date": "2027-03-01",
  "block": null,
  "slots": []
}
```

### GET /program-status

> **Replaced in v2.** Once any program exists this answers `410 Gone` (see the end of this document); use `/programs`.
**Read-only.**
Where the user is today: current block context, number of sessions logged against the current block week, and the next block that hasn't started.
```sh
curl -s 'http://127.0.0.1:7430/api/v1/program-status'
```
`200`
```json
{
  "date": "2026-10-12",
  "block": {
    "block_id": "base-1",
    "name": "Base block 1",
    "week": 3,
    "weeks": 6,
    "is_deload": false
  },
  "sessions_this_week": 1,
  "next_block": {
    "block_id": "base-2",
    "name": "Base 2",
    "start_date": "2026-11-09"
  }
}
```

### PUT /sessions/{id}
Start a session for the current user. All fields optional: `template_id`, `block_id` (must be yours), `block_week`, `slot` (`AM`/`PM`), `label` (external session, e.g. "Unit PT"). Repeating returns 200; a session of the other user returns 403.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/sessions/0199f2b0-0000-7000-8000-000000000001 \
  -H 'Content-Type: application/json' \
  -d '{"template_id": "strength-a", "block_id": "base-1", "block_week": 3, "slot": "AM"}'
```
`201`
```json
{
  "id": "0199f2b0-0000-7000-8000-000000000001",
  "user_id": "alice",
  "template_id": "strength-a",
  "started_at": 1791789870087,
  "finished_at": null,
  "status": "InProgress",
  "note": null,
  "sets": {},
  "deleted": false,
  "block_id": "base-1",
  "block_week": 3,
  "slot": "AM",
  "label": null
}
```

### PUT /sessions/{id}/sets/{set_id}
Log one set with a client-chosen set ID. `exercise_id` required; `reps`, `load` (kg), `duration_s`, `distance_m` as applicable. The server assigns `index` (after the last set). Retrying the same set returns 200 and doesn't duplicate it.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/sessions/0199f2b0-0000-7000-8000-000000000001/sets/0199f2b0-0000-7000-8000-0000000000a1 \
  -H 'Content-Type: application/json' \
  -d '{"exercise_id": "squat", "reps": 5, "load": 105}'
```
`201`
```json
{
  "id": "0199f2b0-0000-7000-8000-000000000001",
  "user_id": "alice",
  "template_id": "strength-a",
  "started_at": 1791789870087,
  "finished_at": null,
  "status": "InProgress",
  "note": null,
  "sets": {
    "0199f2b0-0000-7000-8000-0000000000a1": {
      "index": 0,
      "exercise_id": "squat",
      "reps": 5,
      "load": 105.0,
      "duration_s": null,
      "distance_m": null
    }
  },
  "deleted": false,
  "block_id": "base-1",
  "block_week": 3,
  "slot": "AM",
  "label": null
}
```

### POST /sessions/{id}/finish
Finish with `status` `Completed` or `Partial` and an optional `note` (≤2000). Repeating with the same values changes nothing.
```sh
curl -s -X POST http://127.0.0.1:7430/api/v1/sessions/0199f2b0-0000-7000-8000-000000000001/finish \
  -H 'Content-Type: application/json' \
  -d '{"status": "Partial", "note": "Short on time"}'
```
`200`
```json
{
  "id": "0199f2b0-0000-7000-8000-000000000001",
  "user_id": "alice",
  "template_id": "strength-a",
  "started_at": 1791789870087,
  "finished_at": 1791789870116,
  "status": "Partial",
  "note": "Short on time",
  "sets": {
    "0199f2b0-0000-7000-8000-0000000000a1": {
      "index": 0,
      "exercise_id": "squat",
      "reps": 5,
      "load": 105.0,
      "duration_s": null,
      "distance_m": null
    }
  },
  "deleted": false,
  "block_id": "base-1",
  "block_week": 3,
  "slot": "AM",
  "label": null
}
```

### GET /sessions/{id}
**Read-only.**
One session (either user's).
```sh
curl -s 'http://127.0.0.1:7430/api/v1/sessions/0199f2b0-0000-7000-8000-000000000001'
```
`200`
```json
{
  "id": "0199f2b0-0000-7000-8000-000000000001",
  "user_id": "alice",
  "template_id": "strength-a",
  "started_at": 1791789870087,
  "finished_at": 1791789870116,
  "status": "Partial",
  "note": "Short on time",
  "sets": {
    "0199f2b0-0000-7000-8000-0000000000a1": {
      "index": 0,
      "exercise_id": "squat",
      "reps": 5,
      "load": 105.0,
      "duration_s": null,
      "distance_m": null
    }
  },
  "deleted": false,
  "block_id": "base-1",
  "block_week": 3,
  "slot": "AM",
  "label": null
}
```

### GET /sessions
**Read-only.**
Sessions of the current user (or `?user=`), newest first. `since=YYYY-MM-DD` (from local midnight in the configured timezone), `limit` 1–500 (default 50).
```sh
curl -s 'http://127.0.0.1:7430/api/v1/sessions?since=2026-10-01&limit=10'
```
`200`
```json
[
  {
    "id": "0199f2b0-0000-7000-8000-000000000001",
    "user_id": "alice",
    "template_id": "strength-a",
    "started_at": 1791789870087,
    "finished_at": 1791789870116,
    "status": "Partial",
    "note": "Short on time",
    "sets": {
      "0199f2b0-0000-7000-8000-0000000000a1": {
        "index": 0,
        "exercise_id": "squat",
        "reps": 5,
        "load": 105.0,
        "duration_s": null,
        "distance_m": null
      }
    },
    "deleted": false,
    "block_id": "base-1",
    "block_week": 3,
    "slot": "AM",
    "label": null
  }
]
```

### PUT /tests/{id}
Record a benchmark result for the current user. `kind`: `BeepTest` (`beep`: `"level.shuttle"`, level 1–21, shuttle within the level), `OneRepMax` (`value` kg), `MaxReps` (`value` reps), `TimedRun` (`value` seconds). `date` `YYYY-MM-DD`; optional `exercise_id`.
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/tests/0199f2c0-0000-7000-8000-000000000001 \
  -H 'Content-Type: application/json' \
  -d '{"kind": "BeepTest", "date": "2026-09-01", "beep": "8.10"}'
```
`201`
```json
{
  "id": "0199f2c0-0000-7000-8000-000000000001",
  "user_id": "alice",
  "kind": "BeepTest",
  "date": "2026-09-01",
  "beep": {
    "level": 8,
    "shuttle": 10
  },
  "value": null,
  "exercise_id": null,
  "deleted": false,
  "score": "8.10",
  "plot_value": 8.909090909090908
}
```
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/tests/0199f2c0-0000-7000-8000-000000000003 \
  -H 'Content-Type: application/json' \
  -d '{"kind": "BeepTest", "date": "2026-10-01", "beep": "8.12"}'
```
`400` (level 8 has 11 shuttles)
```json
{
  "error": {
    "code": "invalid",
    "message": "invalid beep score 8.12: level 1-21, shuttle within the level"
  }
}
```

### GET /tests
**Read-only.**
Results of the current user (or `?user=`), best first per kind (lowest time first for `TimedRun`). `kind` optional (all kinds, grouped), `since=YYYY-MM-DD`. Each result adds `score` (display string) and `plot_value` (chart y-value; beep: `level + shuttle / shuttles_in_level`).
```sh
curl -s 'http://127.0.0.1:7430/api/v1/tests?kind=BeepTest'
```
`200`
```json
[
  {
    "id": "0199f2c0-0000-7000-8000-000000000002",
    "user_id": "alice",
    "kind": "BeepTest",
    "date": "2026-10-01",
    "beep": {
      "level": 9,
      "shuttle": 1
    },
    "value": null,
    "exercise_id": null,
    "deleted": false,
    "score": "9.1",
    "plot_value": 9.090909090909092
  },
  {
    "id": "0199f2c0-0000-7000-8000-000000000001",
    "user_id": "alice",
    "kind": "BeepTest",
    "date": "2026-09-01",
    "beep": {
      "level": 8,
      "shuttle": 10
    },
    "value": null,
    "exercise_id": null,
    "deleted": false,
    "score": "8.10",
    "plot_value": 8.909090909090908
  }
]
```

### PUT /external-activities
Insert or replace an activity from an outside source for the current user, keyed by `(source, source_id)`. `source` (no `:`), `source_id`, `started_at` (Unix ms), `duration_s` required; `distance_m`, `linked_session_id` optional. 201 when new, 200 when updated or unchanged. Nothing imports these yet (seam for later).
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/external-activities \
  -H 'Content-Type: application/json' \
  -d '{"source": "garmin", "source_id": "17723", "started_at": 1791500000000, "duration_s": 1800, "distance_m": 5000}'
```
`201`
```json
{
  "source": "garmin",
  "source_id": "17723",
  "user_id": "alice",
  "started_at": 1791500000000,
  "duration_s": 1800,
  "distance_m": 5000.0,
  "linked_session_id": null
}
```
```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/external-activities \
  -H 'Content-Type: application/json' \
  -d '{"source": "garmin", "source_id": "17723", "started_at": 1791500000000, "duration_s": 1800, "distance_m": 5000}'
```
`200` (same again: unchanged)
```json
{
  "source": "garmin",
  "source_id": "17723",
  "user_id": "alice",
  "started_at": 1791500000000,
  "duration_s": 1800,
  "distance_m": 5000.0,
  "linked_session_id": null
}
```

---

## v2 endpoints

Training is built around **programs** (see `docs/DECISIONS.md` "Training model (v2-M1)" and "Progression engine (v2-M2)"): a program has workouts and exercises with a rep range and a reps-in-reserve (RIR) target; periodization is a table of per-week overrides; progression is computed from your logged sets. Code: `crates/web/src/training.rs`, `crates/fitness-core/src/{program,progression,schedule,analytics,migrate}.rs`. All weights are kg.

### PUT /exercises/{id} (v2 fields)

v2 adds `primary_muscles` and `secondary_muscles` (any of `chest back shoulders biceps triceps quads hamstrings glutes calves abs forearms`), `equipment` (`Barbell Dumbbell Machine Cable Bodyweight Other`) and `default_rest_s` (0–3600). All optional; `PATCH` takes the same fields (`null` clears `equipment` and `default_rest_s`). Equipment decides how loads are rounded: a barbell uses the gym's plates, a dumbbell its step, anything else the machine step. Muscles drive weekly volume and warm-ups.

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/exercises/bench \
  -H 'Content-Type: application/json' \
  -d '{"name": "Bench Press", "measurement": "WeightReps", "equipment": "Barbell", "primary_muscles": ["chest"], "secondary_muscles": ["triceps", "shoulders"], "default_rest_s": 150}'
```

`201` (created)

```json
{
  "id": "bench",
  "name": "Bench Press",
  "measurement": "WeightReps",
  "tempo": null,
  "deleted": false,
  "primary_muscles": [
    "chest"
  ],
  "secondary_muscles": [
    "triceps",
    "shoulders"
  ],
  "equipment": "Barbell",
  "default_rest_s": 150
}
```

```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/exercises/bench \
  -H 'Content-Type: application/json' \
  -d '{"secondary_muscles": ["triceps"]}'
```

`200`

```json
{
  "id": "bench",
  "name": "Bench Press",
  "measurement": "WeightReps",
  "tempo": null,
  "deleted": false,
  "primary_muscles": [
    "chest"
  ],
  "secondary_muscles": [
    "triceps"
  ],
  "equipment": "Barbell",
  "default_rest_s": 150
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/exercises/pecs \
  -H 'Content-Type: application/json' \
  -d '{"name": "Pec Fly", "measurement": "WeightReps", "primary_muscles": ["pecs"]}'
```

`400` (unknown muscle)

```json
{
  "error": {
    "code": "invalid",
    "message": "invalid muscle \"pecs\", expected one of chest, back, shoulders, biceps, triceps, quads, hamstrings, glutes, calves, abs, forearms"
  }
}
```


### PUT /gym-profiles/{id}

A user's gym: `bar_kg`, `plates_kg` (plates per side; any number of each is assumed available), `dumbbell_step_kg`, `machine_step_kg`, `is_default`. Setting `is_default` clears it on the user's other profiles. Same `PUT` rules as everywhere (201 / 200 / 409).

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/gym-profiles/g1 \
  -H 'Content-Type: application/json' \
  -d '{"name": "Base gym", "bar_kg": 20, "plates_kg": [1.25, 2.5, 5, 10, 20], "dumbbell_step_kg": 2, "machine_step_kg": 5, "is_default": true}'
```

`201` (created)

```json
{
  "id": "g1",
  "user_id": "alice",
  "name": "Base gym",
  "bar_kg": 20.0,
  "plates_kg": [
    1.25,
    2.5,
    "…"
  ],
  "dumbbell_step_kg": 2.0,
  "machine_step_kg": 5.0,
  "is_default": true,
  "deleted": false
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/gym-profiles/g1 \
  -H 'Content-Type: application/json' \
  -d '{"name": "Base gym", "bar_kg": 20, "plates_kg": [1.25, 2.5, 5, 10, 20], "dumbbell_step_kg": 2, "machine_step_kg": 5, "is_default": true}'
```

`200` (repeat: unchanged)

```json
{
  "id": "g1",
  "user_id": "alice",
  "name": "Base gym",
  "bar_kg": 20.0,
  "plates_kg": [
    1.25,
    2.5,
    "…"
  ],
  "dumbbell_step_kg": 2.0,
  "machine_step_kg": 5.0,
  "is_default": true,
  "deleted": false
}
```


### PATCH /gym-profiles/{id}

Any of the `PUT` fields, plus `deleted`.

```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/gym-profiles/g2 \
  -H 'Content-Type: application/json' \
  -d '{"is_default": true, "plates_kg": [1.25, 2.5, 5]}'
```

`200`

```json
{
  "id": "g2",
  "user_id": "alice",
  "name": "Home",
  "bar_kg": 15.0,
  "plates_kg": [
    1.25,
    2.5,
    "…"
  ],
  "dumbbell_step_kg": 2.5,
  "machine_step_kg": 5.0,
  "is_default": true,
  "deleted": false
}
```


### GET /gym-profiles

**Read-only.**

A user's profiles (`?user=` for the other user's), default first, then by name.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/gym-profiles'
```

`200`

```json
[
  {
    "id": "g2",
    "user_id": "alice",
    "name": "Home",
    "bar_kg": 15.0,
    "plates_kg": [
      1.25,
      2.5,
      "…"
    ],
    "dumbbell_step_kg": 2.5,
    "machine_step_kg": 5.0,
    "is_default": true,
    "deleted": false
  },
  {
    "id": "g1",
    "user_id": "alice",
    "name": "Base gym",
    "bar_kg": 20.0,
    "plates_kg": [
      1.25,
      2.5,
      "…"
    ],
    "dumbbell_step_kg": 2.0,
    "machine_step_kg": 5.0,
    "is_default": false,
    "deleted": false
  }
]
```


### PUT /programs/{id}

Create a program. A program has ordered `workouts`, each with ordered `items`; **you choose every workout and item ID** (1–64 chars as usual; item IDs are unique across the program) so `overrides` can name items. A workout can be pinned to a weekday and half-day with `slot` (`mon_am` … `sun_pm`), or left unpinned to rotate. An *outside session* (e.g. Unit PT) has `external_label` and a `slot`, and no items.

An item is `sets` × `rep_min`–`rep_max` at `rir` reps in reserve (0–5, default 2), `style` (`Straight`, `DropSet`, `ToFailure`; default `Straight`), optional `rest_s`. `start_date` must be a Monday; `weeks` 1–104. `warmups` (default true) turns warm-up suggestions on or off. `gym_profile_id` is optional (the user's default gym is used otherwise).

`overrides` is the periodization table: for a week (1-based) and an item, any of `sets`, `rep_min`, `rep_max`, `rir` replaces the item's own value that week (`null` = unchanged). Here week 4 is a deload: 2 sets at RIR 4. Smart progression does not move targets week to week; this table does.

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/programs/p1 \
  -H 'Content-Type: application/json' \
  -d '{"name": "Upper/Lower", "start_date": "2026-10-12", "weeks": 4, "gym_profile_id": "g1", "warmups": true, "workouts": [{"id": "w1", "name": "Upper A", "slot": "mon_am", "items": [{"id": "i1", "exercise_id": "bench", "sets": 3, "rep_min": 7, "rep_max": 9, "rir": 2, "rest_s": 150}, {"id": "i2", "exercise_id": "incline", "sets": 3, "rep_min": 8, "rep_max": 12, "rir": 2, "rest_s": 90}, {"id": "i3", "exercise_id": "row", "sets": 3, "rep_min": 8, "rep_max": 12}]}, {"id": "w2", "name": "Lower", "slot": "thu_am", "items": [{"id": "i4", "exercise_id": "squat", "sets": 4, "rep_min": 5, "rep_max": 8, "style": "ToFailure"}]}, {"id": "pt", "name": "Unit PT", "slot": "wed_pm", "external_label": "Unit PT"}], "overrides": [{"week": 4, "item_id": "i1", "sets": 2, "rir": 4}, {"week": 4, "item_id": "i3", "sets": 2, "rir": 4}]}'
```

`201` (created)

```json
{
  "id": "p1",
  "user_id": "alice",
  "name": "Upper/Lower",
  "start_date": "2026-10-12",
  "weeks": 4,
  "gym_profile_id": "g1",
  "warmups": true,
  "workouts": [
    {
      "id": "w1",
      "name": "Upper A",
      "slot": "mon_am",
      "external_label": null,
      "items": [
        {
          "id": "i1",
          "index": 0,
          "exercise_id": "bench",
          "sets": 3,
          "rep_min": 7,
          "rep_max": 9,
          "rir": 2,
          "style": "Straight",
          "rest_s": 150
        },
        {
          "id": "i2",
          "index": 1,
          "exercise_id": "incline",
          "sets": 3,
          "rep_min": 8,
          "rep_max": 12,
          "rir": 2,
          "style": "Straight",
          "rest_s": 90
        },
        "…"
      ]
    },
    {
      "id": "w2",
      "name": "Lower",
      "slot": "thu_am",
      "external_label": null,
      "items": [
        {
          "id": "i4",
          "index": 0,
          "exercise_id": "squat",
          "sets": 4,
          "rep_min": 5,
          "rep_max": 8,
          "rir": 2,
          "style": "ToFailure",
          "rest_s": null
        }
      ]
    },
    "…"
  ],
  "overrides": [
    {
      "week": 4,
      "item_id": "i1",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    },
    {
      "week": 4,
      "item_id": "i3",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    }
  ],
  "migrated_from": null,
  "deleted": false
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/programs/p1 \
  -H 'Content-Type: application/json' \
  -d '{"name": "Other", "start_date": "2026-10-12", "weeks": 4, "gym_profile_id": "g1", "warmups": true, "workouts": [{"id": "w1", "name": "Upper A", "slot": "mon_am", "items": [{"id": "i1", "exercise_id": "bench", "sets": 3, "rep_min": 7, "rep_max": 9, "rir": 2, "rest_s": 150}, {"id": "i2", "exercise_id": "incline", "sets": 3, "rep_min": 8, "rep_max": 12, "rir": 2, "rest_s": 90}, {"id": "i3", "exercise_id": "row", "sets": 3, "rep_min": 8, "rep_max": 12}]}, {"id": "w2", "name": "Lower", "slot": "thu_am", "items": [{"id": "i4", "exercise_id": "squat", "sets": 4, "rep_min": 5, "rep_max": 8, "style": "ToFailure"}]}, {"id": "pt", "name": "Unit PT", "slot": "wed_pm", "external_label": "Unit PT"}], "overrides": [{"week": 4, "item_id": "i1", "sets": 2, "rir": 4}, {"week": 4, "item_id": "i3", "sets": 2, "rir": 4}]}'
```

`409` (same ID, different content)

```json
{
  "error": {
    "code": "conflict",
    "message": "p1 already exists with different content"
  }
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/programs/bad \
  -H 'Content-Type: application/json' \
  -d '{"name": "Upper/Lower", "start_date": "2026-10-13", "weeks": 4, "gym_profile_id": "g1", "warmups": true, "workouts": [{"id": "w1", "name": "Upper A", "slot": "mon_am", "items": [{"id": "i1", "exercise_id": "bench", "sets": 3, "rep_min": 7, "rep_max": 9, "rir": 2, "rest_s": 150}, {"id": "i2", "exercise_id": "incline", "sets": 3, "rep_min": 8, "rep_max": 12, "rir": 2, "rest_s": 90}, {"id": "i3", "exercise_id": "row", "sets": 3, "rep_min": 8, "rep_max": 12}]}, {"id": "w2", "name": "Lower", "slot": "thu_am", "items": [{"id": "i4", "exercise_id": "squat", "sets": 4, "rep_min": 5, "rep_max": 8, "style": "ToFailure"}]}, {"id": "pt", "name": "Unit PT", "slot": "wed_pm", "external_label": "Unit PT"}], "overrides": [{"week": 4, "item_id": "i1", "sets": 2, "rir": 4}, {"week": 4, "item_id": "i3", "sets": 2, "rir": 4}]}'
```

`400` (not a Monday)

```json
{
  "error": {
    "code": "invalid",
    "message": "invalid start_date 2026-10-13: programs start on a Monday"
  }
}
```


### PATCH /programs/{id}

Any of `name`, `start_date`, `weeks`, `gym_profile_id` (`null` clears), `warmups`, `deleted`. `workouts` and `overrides`, if present, **replace** the whole list (send the full list; keep IDs to keep your overrides valid).

```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/programs/p1 \
  -H 'Content-Type: application/json' \
  -d '{"name": "Upper/Lower v1", "warmups": true}'
```

`200`

```json
{
  "id": "p1",
  "user_id": "alice",
  "name": "Upper/Lower v1",
  "start_date": "2026-10-12",
  "weeks": 4,
  "gym_profile_id": "g1",
  "warmups": true,
  "workouts": [
    {
      "id": "w1",
      "name": "Upper A",
      "slot": "mon_am",
      "external_label": null,
      "items": [
        {
          "id": "i1",
          "index": 0,
          "exercise_id": "bench",
          "sets": 3,
          "rep_min": 7,
          "rep_max": 9,
          "rir": 2,
          "style": "Straight",
          "rest_s": 150
        },
        {
          "id": "i2",
          "index": 1,
          "exercise_id": "incline",
          "sets": 3,
          "rep_min": 8,
          "rep_max": 12,
          "rir": 2,
          "style": "Straight",
          "rest_s": 90
        },
        "…"
      ]
    },
    {
      "id": "w2",
      "name": "Lower",
      "slot": "thu_am",
      "external_label": null,
      "items": [
        {
          "id": "i4",
          "index": 0,
          "exercise_id": "squat",
          "sets": 4,
          "rep_min": 5,
          "rep_max": 8,
          "rir": 2,
          "style": "ToFailure",
          "rest_s": null
        }
      ]
    },
    "…"
  ],
  "overrides": [
    {
      "week": 4,
      "item_id": "i1",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    },
    {
      "week": 4,
      "item_id": "i3",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    }
  ],
  "migrated_from": null,
  "deleted": false
}
```


### GET /programs

**Read-only.**

A user's programs (`?user=` for the other user's), by start date, with workouts and items in order and `overrides` sorted by week.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/programs'
```

`200`

```json
[
  {
    "id": "p1",
    "user_id": "alice",
    "name": "Upper/Lower v1",
    "start_date": "2026-10-12",
    "weeks": 4,
    "gym_profile_id": "g1",
    "warmups": true,
    "workouts": [
      {
        "id": "w1",
        "name": "Upper A",
        "slot": "mon_am",
        "external_label": null,
        "items": [
          {
            "id": "i1",
            "index": 0,
            "exercise_id": "bench",
            "sets": 3,
            "rep_min": 7,
            "rep_max": 9,
            "rir": 2,
            "style": "Straight",
            "rest_s": 150
          },
          {
            "id": "i2",
            "index": 1,
            "exercise_id": "incline",
            "sets": 3,
            "rep_min": 8,
            "rep_max": 12,
            "rir": 2,
            "style": "Straight",
            "rest_s": 90
          },
          "…"
        ]
      },
      {
        "id": "w2",
        "name": "Lower",
        "slot": "thu_am",
        "external_label": null,
        "items": [
          {
            "id": "i4",
            "index": 0,
            "exercise_id": "squat",
            "sets": 4,
            "rep_min": 5,
            "rep_max": 8,
            "rir": 2,
            "style": "ToFailure",
            "rest_s": null
          }
        ]
      },
      "…"
    ],
    "overrides": [
      {
        "week": 4,
        "item_id": "i1",
        "sets": 2,
        "rep_min": null,
        "rep_max": null,
        "rir": 4
      },
      {
        "week": 4,
        "item_id": "i3",
        "sets": 2,
        "rep_min": null,
        "rep_max": null,
        "rir": 4
      }
    ],
    "migrated_from": null,
    "deleted": false
  }
]
```


### GET /programs/{id}

**Read-only.**

One program (either user's).

```sh
curl -s 'http://127.0.0.1:7430/api/v1/programs/p1'
```

`200`

```json
{
  "id": "p1",
  "user_id": "alice",
  "name": "Upper/Lower v1",
  "start_date": "2026-10-12",
  "weeks": 4,
  "gym_profile_id": "g1",
  "warmups": true,
  "workouts": [
    {
      "id": "w1",
      "name": "Upper A",
      "slot": "mon_am",
      "external_label": null,
      "items": [
        {
          "id": "i1",
          "index": 0,
          "exercise_id": "bench",
          "sets": 3,
          "rep_min": 7,
          "rep_max": 9,
          "rir": 2,
          "style": "Straight",
          "rest_s": 150
        },
        {
          "id": "i2",
          "index": 1,
          "exercise_id": "incline",
          "sets": 3,
          "rep_min": 8,
          "rep_max": 12,
          "rir": 2,
          "style": "Straight",
          "rest_s": 90
        },
        "…"
      ]
    },
    {
      "id": "w2",
      "name": "Lower",
      "slot": "thu_am",
      "external_label": null,
      "items": [
        {
          "id": "i4",
          "index": 0,
          "exercise_id": "squat",
          "sets": 4,
          "rep_min": 5,
          "rep_max": 8,
          "rir": 2,
          "style": "ToFailure",
          "rest_s": null
        }
      ]
    },
    "…"
  ],
  "overrides": [
    {
      "week": 4,
      "item_id": "i1",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    },
    {
      "week": 4,
      "item_id": "i3",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    }
  ],
  "migrated_from": null,
  "deleted": false
}
```


### GET /workouts/next

**Read-only.**

What to train on `date` (default today in `[web] timezone`): the workout pinned to that weekday (first one not yet done today), otherwise the next workout in the program's rotation of unpinned workouts, with every item's prescription for that week, its **recommendation with the reason**, and warm-ups. `externals` lists outside sessions pinned that day and whether they were attended. `todays_sessions` are the user's sessions for this program on that date, so a client can offer *Continue* for an in-progress one. `workout` is `null` (with a `reason`) on a rest day, when everything pinned is done, or with no program running.

Recommendations are derived from your finished sessions every time you ask (never stored), so they change as soon as you finish a workout.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/workouts/next?date=2026-10-12'
```

`200` (no history yet: recommendation is null)

```json
{
  "date": "2026-10-12",
  "program_id": "p1",
  "program_name": "Upper/Lower v1",
  "week": 1,
  "weeks": 4,
  "workout": {
    "program_id": "p1",
    "workout_id": "w1",
    "name": "Upper A",
    "slot": "mon_am",
    "week": 1,
    "external_label": null,
    "items": [
      {
        "item_id": "i1",
        "exercise_id": "bench",
        "exercise_name": "Bench Press",
        "prescription": {
          "sets": 3,
          "rep_min": 7,
          "rep_max": 9,
          "rir": 2,
          "style": "Straight",
          "rest_s": 150
        },
        "recommendation": null,
        "warmups": []
      },
      {
        "item_id": "i2",
        "exercise_id": "incline",
        "exercise_name": "Incline DB Press",
        "prescription": {
          "sets": 3,
          "rep_min": 8,
          "rep_max": 12,
          "rir": 2,
          "style": "Straight",
          "rest_s": 90
        },
        "recommendation": null,
        "warmups": []
      },
      "…"
    ]
  },
  "reason": "Pinned for Monday AM.",
  "externals": [],
  "todays_sessions": []
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/workouts/next?date=2026-10-19'
```

`200` (after a finished session)

```json
{
  "date": "2026-10-19",
  "program_id": "p1",
  "program_name": "Upper/Lower v1",
  "week": 2,
  "weeks": 4,
  "workout": {
    "program_id": "p1",
    "workout_id": "w1",
    "name": "Upper A",
    "slot": "mon_am",
    "week": 2,
    "external_label": null,
    "items": [
      {
        "item_id": "i1",
        "exercise_id": "bench",
        "exercise_name": "Bench Press",
        "prescription": {
          "sets": 3,
          "rep_min": 7,
          "rep_max": 9,
          "rir": 2,
          "style": "Straight",
          "rest_s": 150
        },
        "recommendation": {
          "id": "bench@82.5x8@2",
          "load_kg": 82.5,
          "reps": 8,
          "rir": 2,
          "reason": "Last session: 10 reps @ 80 kg at 2 RIR, above the 7–9 @ 2 RIR target by 2 rep(s) to failure → +2.5 kg"
        },
        "warmups": [
          {
            "load_kg": 32.5,
            "reps": 8
          },
          {
            "load_kg": 47.5,
            "reps": 5
          },
          "…"
        ]
      },
      {
        "item_id": "i2",
        "exercise_id": "incline",
        "exercise_name": "Incline DB Press",
        "prescription": {
          "sets": 3,
          "rep_min": 8,
          "rep_max": 12,
          "rir": 2,
          "style": "Straight",
          "rest_s": 90
        },
        "recommendation": {
          "id": "incline@30x10@2",
          "load_kg": 30.0,
          "reps": 10,
          "rir": 2,
          "reason": "Last session: 10 reps @ 30 kg at 2 RIR, on the 8–12 @ 2 RIR target → same load"
        },
        "warmups": []
      },
      "…"
    ]
  },
  "reason": "Pinned for Monday AM.",
  "externals": [],
  "todays_sessions": []
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/workouts/next?date=2026-10-14'
```

`200` (a Wednesday: only the outside session)

```json
{
  "date": "2026-10-14",
  "program_id": "p1",
  "program_name": "Upper/Lower v1",
  "week": 1,
  "weeks": 4,
  "workout": null,
  "reason": "Only an outside session is planned today.",
  "externals": [
    {
      "workout_id": "pt",
      "slot": "wed_pm",
      "label": "Unit PT",
      "done": false
    }
  ],
  "todays_sessions": []
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/workouts/next?date=2026-10-13'
```

`200` (a Tuesday: nothing pinned and no unpinned workouts)

```json
{
  "date": "2026-10-13",
  "program_id": "p1",
  "program_name": "Upper/Lower v1",
  "week": 1,
  "weeks": 4,
  "workout": null,
  "reason": "Rest day: nothing is pinned for today.",
  "externals": [],
  "todays_sessions": []
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/workouts/next?date=2027-03-01'
```

`200` (no program running)

```json
{
  "date": "2027-03-01",
  "program_id": null,
  "program_name": null,
  "week": null,
  "weeks": null,
  "workout": null,
  "reason": "No program is running on this date.",
  "externals": [],
  "todays_sessions": []
}
```


### GET /recommendations

**Read-only.**

The same plan for one workout of one program: `program_id` and `workout_id` required, `date` defaults to today (its week decides which `overrides` apply; outside the program the base prescription is used and `week` is `null`). `?user=` reads the other user's program (otherwise 403 for someone else's program).

Each `recommendation` is `{id, load_kg, reps, rir, reason}`. The rules (all constants are in `crates/fitness-core/src/progression.rs`): reps to failure `F = reps + RIR`; strength estimate `e1RM = load × (1 + F/30)` (Epley) from the best working set of each finished session, averaged over the last 3 sessions; target `F = midpoint of the rep range + RIR target`; the load that gives that `F` is rounded **down** to what the gym's plates/steps can make; if rounding loses more than 10 % of the load, the lower load is kept and reps are raised (to at most the range maximum + 3). No finished history for the exercise: `recommendation` is `null`. `id` describes what was shown; send it back as `recommendation_id` when logging the set.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/recommendations?program_id=p1&workout_id=w1&date=2026-10-26'
```

`200`

```json
{
  "program_id": "p1",
  "workout_id": "w1",
  "name": "Upper A",
  "slot": "mon_am",
  "week": 3,
  "external_label": null,
  "items": [
    {
      "item_id": "i1",
      "exercise_id": "bench",
      "exercise_name": "Bench Press",
      "prescription": {
        "sets": 3,
        "rep_min": 7,
        "rep_max": 9,
        "rir": 2,
        "style": "Straight",
        "rest_s": 150
      },
      "recommendation": {
        "id": "bench@82.5x8@2",
        "load_kg": 82.5,
        "reps": 8,
        "rir": 2,
        "reason": "Last session: 10 reps @ 80 kg at 2 RIR, above the 7–9 @ 2 RIR target by 2 rep(s) to failure → +2.5 kg"
      },
      "warmups": [
        {
          "load_kg": 32.5,
          "reps": 8
        },
        {
          "load_kg": 47.5,
          "reps": 5
        },
        "…"
      ]
    },
    {
      "item_id": "i2",
      "exercise_id": "incline",
      "exercise_name": "Incline DB Press",
      "prescription": {
        "sets": 3,
        "rep_min": 8,
        "rep_max": 12,
        "rir": 2,
        "style": "Straight",
        "rest_s": 90
      },
      "recommendation": {
        "id": "incline@30x10@2",
        "load_kg": 30.0,
        "reps": 10,
        "rir": 2,
        "reason": "Last session: 10 reps @ 30 kg at 2 RIR, on the 8–12 @ 2 RIR target → same load"
      },
      "warmups": []
    },
    "…"
  ]
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/recommendations?program_id=p1&workout_id=w1&date=2026-11-02'
```

`200` (week 4: deload from the override table (2 sets, RIR 4))

```json
{
  "program_id": "p1",
  "workout_id": "w1",
  "name": "Upper A",
  "slot": "mon_am",
  "week": 4,
  "external_label": null,
  "items": [
    {
      "item_id": "i1",
      "exercise_id": "bench",
      "exercise_name": "Bench Press",
      "prescription": {
        "sets": 2,
        "rep_min": 7,
        "rep_max": 9,
        "rir": 4,
        "style": "Straight",
        "rest_s": 150
      },
      "recommendation": {
        "id": "bench@80x8@4",
        "load_kg": 80.0,
        "reps": 8,
        "rir": 4,
        "reason": "Last session: 10 reps @ 80 kg at 2 RIR, on the 7–9 @ 4 RIR target → same load"
      },
      "warmups": [
        {
          "load_kg": 30.0,
          "reps": 8
        },
        {
          "load_kg": 47.5,
          "reps": 5
        },
        "…"
      ]
    },
    {
      "item_id": "i2",
      "exercise_id": "incline",
      "exercise_name": "Incline DB Press",
      "prescription": {
        "sets": 3,
        "rep_min": 8,
        "rep_max": 12,
        "rir": 2,
        "style": "Straight",
        "rest_s": 90
      },
      "recommendation": {
        "id": "incline@30x10@2",
        "load_kg": 30.0,
        "reps": 10,
        "rir": 2,
        "reason": "Last session: 10 reps @ 30 kg at 2 RIR, on the 8–12 @ 2 RIR target → same load"
      },
      "warmups": []
    },
    "…"
  ]
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/recommendations?program_id=p1&workout_id=nope'
```

`404`

```json
{
  "error": {
    "code": "not_found",
    "message": "nope not found"
  }
}
```


### PUT /sessions/{id} (v2 fields)

A session started from a program workout carries `program_id` and `workout_id` (the program must be the caller's). Attending an outside session is a session with `program_id`, `workout_id` and `label`, then `finish`.

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/sessions/s2 \
  -H 'Content-Type: application/json' \
  -d '{"program_id": "p1", "workout_id": "w1"}'
```

`201` (created)

```json
{
  "id": "s2",
  "user_id": "alice",
  "template_id": null,
  "started_at": 1791801585520,
  "finished_at": null,
  "status": "InProgress",
  "note": null,
  "sets": {},
  "deleted": false,
  "block_id": null,
  "block_week": null,
  "slot": null,
  "label": null,
  "program_id": "p1",
  "workout_id": "w1"
}
```


### PUT /sessions/{id}/sets/{set_id} (v2 fields)

v2 sets also take `kind` (`Warmup`, `Working`, `Drop`, `Failure`; missing = a working set), `rir` (0–10), `partials` (0–50), `rest_taken_s` (0–7200) and `recommendation_id` (≤200 chars). Only `Working`/`Failure` sets feed the strength estimate; `Warmup` sets are excluded from volume too.

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/sessions/s2/sets/x1 \
  -H 'Content-Type: application/json' \
  -d '{"exercise_id": "bench", "load": 82.5, "reps": 8, "rir": 2, "kind": "Working", "rest_taken_s": 150, "recommendation_id": "bench@82.5x8@2"}'
```

`201` (created)

```json
{
  "id": "s2",
  "user_id": "alice",
  "template_id": null,
  "started_at": 1791801585520,
  "finished_at": null,
  "status": "InProgress",
  "note": null,
  "sets": {
    "x1": {
      "index": 0,
      "exercise_id": "bench",
      "reps": 8,
      "load": 82.5,
      "duration_s": null,
      "distance_m": null,
      "kind": "Working",
      "rir": 2,
      "partials": null,
      "rest_taken_s": 150,
      "recommendation_id": "bench@82.5x8@2"
    }
  },
  "deleted": false,
  "block_id": null,
  "block_week": null,
  "slot": null,
  "label": null,
  "program_id": "p1",
  "workout_id": "w1"
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/sessions/s2/sets/x2 \
  -H 'Content-Type: application/json' \
  -d '{"exercise_id": "bench", "load": 65, "reps": 8, "kind": "Heroic"}'
```

`400` (unknown kind)

```json
{
  "error": {
    "code": "invalid",
    "message": "kind \"Heroic\": expected Warmup, Working, Drop or Failure"
  }
}
```


### GET /analytics/volume

**Read-only.**

Working sets per muscle per week (Monday start): a primary muscle counts 1 per set, a secondary 0.5; warm-ups don't count. `week` is any date in the last week wanted (default: this week); `weeks` (1–26, default 1) returns that many weeks ending there, oldest first. All 11 muscles are always present.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/analytics/volume?weeks=2'
```

`200`

```json
{
  "weeks": [
    {
      "week_start": "2026-10-05",
      "sets": {
        "abs": 0.0,
        "back": 0.0,
        "biceps": 0.0,
        "calves": 0.0,
        "chest": 0.0,
        "forearms": 0.0,
        "glutes": 0.0,
        "hamstrings": 0.0,
        "quads": 0.0,
        "shoulders": 0.0,
        "triceps": 0.0
      }
    },
    {
      "week_start": "2026-10-12",
      "sets": {
        "abs": 0.0,
        "back": 1.0,
        "biceps": 0.5,
        "calves": 0.0,
        "chest": 4.0,
        "forearms": 0.0,
        "glutes": 1.0,
        "hamstrings": 0.5,
        "quads": 1.0,
        "shoulders": 0.0,
        "triceps": 2.0
      }
    }
  ]
}
```


### GET /analytics/e1rm

**Read-only.**

`exercise_id` required. The best working-set e1RM of each finished session, oldest first; `rir` is the logged RIR (2 if none was logged).

```sh
curl -s 'http://127.0.0.1:7430/api/v1/analytics/e1rm?exercise_id=bench'
```

`200`

```json
{
  "exercise_id": "bench",
  "points": [
    {
      "session_id": "h1",
      "started_at": 1791801585178,
      "e1rm": 112.0,
      "load_kg": 80.0,
      "reps": 10,
      "rir": 2
    },
    {
      "session_id": "s2",
      "started_at": 1791801585520,
      "e1rm": 110.0,
      "load_kg": 82.5,
      "reps": 8,
      "rir": 2
    }
  ]
}
```


### GET /prs

**Read-only.**

PRs over finished sessions, optionally for one `exercise_id`. Per exercise: one `e1rm` PR (best estimated one-rep max) and `reps` PRs: sets that no other set beats on both load and reps (the most reps at a load or heavier), heaviest first. Ties go to the earliest set.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/prs?exercise_id=bench'
```

`200`

```json
[
  {
    "exercise_id": "bench",
    "kind": "e1rm",
    "load_kg": 80.0,
    "reps": 10,
    "e1rm": 112.0,
    "session_id": "h1",
    "started_at": 1791801585178
  },
  {
    "exercise_id": "bench",
    "kind": "reps",
    "load_kg": 82.5,
    "reps": 8,
    "e1rm": 110.0,
    "session_id": "s2",
    "started_at": 1791801585520
  },
  "…"
]
```

### POST /programs/migrate-v1

One-off migration of v1 blocks. Each non-deleted v1 block becomes a program with ID `v1-<block id>` using its owner's weekly plan and templates: a fixed rep target `r` becomes the range `r`–`r`, RIR 2, straight sets, rest carried over; outside sessions stay as outside sessions; the v1 deload week becomes half the sets (rounded up) at RIR 4. Not carried over, and listed in `notes`: linear progression, fixed target loads (loads now come from your history) and the deload load percentage. Idempotent (a second call creates nothing) and never overwrites a program. It covers **both users'** blocks. v1 data stays in the document, unread. Nothing runs it automatically; the web app offers it.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/blocks'
```

`200`

```json
[
  {
    "id": "block-1",
    "user_id": "alice",
    "name": "Base",
    "start_date": "2026-01-05",
    "weeks": 6,
    "deload_week": 6,
    "deload_pct": 60.0,
    "deleted": false
  }
]
```

```sh
curl -s -X POST http://127.0.0.1:7430/api/v1/programs/migrate-v1 \
  -H 'Content-Type: application/json' \
  -d '{}'
```

`200`

```json
{
  "created": 1,
  "notes": [
    "Legs: fixed target loads dropped (v2 recommends loads from history)",
    "Legs: linear progression dropped (v2 progresses from logged history)",
    "…"
  ]
}
```

```sh
curl -s -X POST http://127.0.0.1:7430/api/v1/programs/migrate-v1 \
  -H 'Content-Type: application/json' \
  -d '{}'
```

`200` (repeat)

```json
{
  "created": 0,
  "notes": []
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/programs/v1-block-1'
```

`200`

```json
{
  "id": "v1-block-1",
  "user_id": "alice",
  "name": "Base",
  "start_date": "2026-01-05",
  "weeks": 6,
  "gym_profile_id": null,
  "warmups": true,
  "workouts": [
    {
      "id": "mon_am",
      "name": "Legs",
      "slot": "mon_am",
      "external_label": null,
      "items": [
        {
          "id": "mon_am-0",
          "index": 0,
          "exercise_id": "01a12f93-b281-723d-a378-54a1508a95ed",
          "sets": 3,
          "rep_min": 5,
          "rep_max": 5,
          "rir": 2,
          "style": "Straight",
          "rest_s": 120
        },
        {
          "id": "mon_am-1",
          "index": 1,
          "exercise_id": "01a12f93-b282-7263-ad2e-0d2e99b097db",
          "sets": 3,
          "rep_min": 8,
          "rep_max": 8,
          "rir": 2,
          "style": "Straight",
          "rest_s": 120
        }
      ]
    },
    {
      "id": "wed_pm",
      "name": "Unit PT",
      "slot": "wed_pm",
      "external_label": "Unit PT",
      "items": []
    }
  ],
  "overrides": [
    {
      "week": 6,
      "item_id": "mon_am-0",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    },
    {
      "week": 6,
      "item_id": "mon_am-1",
      "sets": 2,
      "rep_min": null,
      "rep_max": null,
      "rir": 4
    }
  ],
  "migrated_from": "block-1",
  "deleted": false
}
```


### Replaced in v2: `410 Gone`

Once **any** program exists, the v1 endpoints for templates, weekly plans and blocks (`/templates`, `/templates/{id}`, `/plan`, `/blocks`, `/blocks/{id}`, `/planned-day`, `/program-status`, every method) answer `410` before the request is read, with the replacement in the body. Until then they keep working.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/templates'
```

`410`

```json
{
  "error": {
    "code": "gone",
    "message": "templates, weekly plans and blocks were replaced by programs"
  },
  "replacement": "/api/v1/programs"
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/blocks/b9 \
  -H 'Content-Type: application/json' \
  -d '{"name": "x", "start_date": "2026-10-12", "weeks": 4}'
```

`410` (write also answers 410)

```json
{
  "error": {
    "code": "gone",
    "message": "templates, weekly plans and blocks were replaced by programs"
  },
  "replacement": "/api/v1/programs"
}
```

---

## Shopping lists and live updates (v2)

Shared by both users. Lists have items; an item has a `name`, optional `quantity` (free text such as `2` or `500 g`), `note` and `category` (`produce meat dairy bakery pantry frozen household other`), and a tick (`checked`, `checked_by`, `checked_at`). A list comes back **unticked first, then by category (in that order, uncategorised last), then by name**. Code: `crates/web/src/shopping.rs`, `crates/shopping-core`. Item IDs are chosen by the client (same `PUT` rules as everywhere: 201 / 200 / 409) and are unique across all lists.

**Two people at once.** The tick is a single value (who and when together), so if both of you tick the same item at the same moment you both end up with the same result: ticked, by one of you. Ticking a ticked item changes nothing. *Clear checked* hides the items that were ticked when it ran; an item the other person unticked, or ticked again, a moment later stays on the list. See `docs/LEARNING.md` "Last-writer-wins on a single field".


### PUT /lists/{id}

Create a list. Same rules as other `PUT`s. The response counts the items on the list: `total` and `unchecked`. (The web app creates a list with the ID `groceries` the first time it is opened, so two devices doing so at once make the same list.)

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/lists/groceries \
  -H 'Content-Type: application/json' \
  -d '{"name": "Groceries"}'
```

`201` (created)

```json
{
  "id": "groceries",
  "name": "Groceries",
  "created_at": 1791802280628,
  "unchecked": 0,
  "total": 0
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/lists/groceries \
  -H 'Content-Type: application/json' \
  -d '{"name": "Groceries"}'
```

`200` (repeat: unchanged)

```json
{
  "id": "groceries",
  "name": "Groceries",
  "created_at": 1791802280628,
  "unchecked": 0,
  "total": 0
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/lists/groceries \
  -H 'Content-Type: application/json' \
  -d '{"name": "Food"}'
```

`409` (same ID, different name)

```json
{
  "error": {
    "code": "conflict",
    "message": "groceries already exists with different content"
  }
}
```


### GET /lists

**Read-only.**

Lists by name, with counts.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/lists'
```

`200`

```json
[
  {
    "id": "bunnings",
    "name": "Bunnings",
    "created_at": 1791802280652,
    "unchecked": 0,
    "total": 0
  },
  {
    "id": "groceries",
    "name": "Groceries",
    "created_at": 1791802280628,
    "unchecked": 0,
    "total": 0
  }
]
```


### PATCH /lists/{id}

`name`, and/or `deleted: true` (a deleted list cannot be restored; its items stay in the document).

```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/lists/bunnings \
  -H 'Content-Type: application/json' \
  -d '{"name": "Hardware"}'
```

`200`

```json
{
  "id": "bunnings",
  "name": "Hardware",
  "created_at": 1791802280652,
  "unchecked": 0,
  "total": 0
}
```


### PUT /lists/{id}/items/{item_id}

Add an item. `name` required (≤ 200 characters); `quantity` ≤ 50, `note` ≤ 500, `category` from the list above. The user who adds it is recorded as `added_by`.

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/lists/groceries/items/milk \
  -H 'Content-Type: application/json' \
  -d '{"name": "Milk", "quantity": "2 L", "category": "dairy"}'
```

`201` (created)

```json
{
  "id": "milk",
  "list_id": "groceries",
  "name": "Milk",
  "quantity": "2 L",
  "note": null,
  "category": "dairy",
  "checked": false,
  "checked_by": null,
  "checked_at": null,
  "added_by": "alice",
  "added_at": 1791802280671
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/lists/groceries/items/milk \
  -H 'Content-Type: application/json' \
  -d '{"name": "Milk", "quantity": "2 L", "category": "dairy"}'
```

`200` (repeat: unchanged)

```json
{
  "id": "milk",
  "list_id": "groceries",
  "name": "Milk",
  "quantity": "2 L",
  "note": null,
  "category": "dairy",
  "checked": false,
  "checked_by": null,
  "checked_at": null,
  "added_by": "alice",
  "added_at": 1791802280671
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/lists/groceries/items/milk \
  -H 'Content-Type: application/json' \
  -d '{"name": "Milk", "quantity": "3 L", "category": "dairy"}'
```

`409` (same ID, different content)

```json
{
  "error": {
    "code": "conflict",
    "message": "milk already exists with different content"
  }
}
```

```sh
curl -s -X PUT http://127.0.0.1:7430/api/v1/lists/groceries/items/chips \
  -H 'Content-Type: application/json' \
  -d '{"name": "Chips", "category": "snacks"}'
```

`400` (unknown category)

```json
{
  "error": {
    "code": "invalid",
    "message": "invalid category \"snacks\", expected one of produce, meat, dairy, bakery, pantry, frozen, household, other"
  }
}
```


### GET /lists/{id}/items

**Read-only.**

The items on the list (not deleted, and not cleared), in list order.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/lists/groceries/items'
```

`200`

```json
[
  {
    "id": "apples",
    "list_id": "groceries",
    "name": "Apples",
    "quantity": "6",
    "note": null,
    "category": "produce",
    "checked": false,
    "checked_by": null,
    "checked_at": null,
    "added_by": "alice",
    "added_at": 1791802280696
  },
  {
    "id": "milk",
    "list_id": "groceries",
    "name": "Milk",
    "quantity": "2 L",
    "note": null,
    "category": "dairy",
    "checked": false,
    "checked_by": null,
    "checked_at": null,
    "added_by": "alice",
    "added_at": 1791802280671
  },
  {
    "id": "soap",
    "list_id": "groceries",
    "name": "Dish soap",
    "quantity": null,
    "note": null,
    "category": "household",
    "checked": false,
    "checked_by": null,
    "checked_at": null,
    "added_by": "alice",
    "added_at": 1791802280707
  }
]
```


### PATCH /lists/{id}/items/{item_id}

Change `name`, `quantity`, `note` or `category` (`null` clears the last three), and/or tick or untick with `checked`. Ticking records the current user and the time.

```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/lists/groceries/items/apples \
  -H 'Content-Type: application/json' \
  -d '{"checked": true}'
```

`200` (tick)

```json
{
  "id": "apples",
  "list_id": "groceries",
  "name": "Apples",
  "quantity": "6",
  "note": null,
  "category": "produce",
  "checked": true,
  "checked_by": "alice",
  "checked_at": 1791802280727,
  "added_by": "alice",
  "added_at": 1791802280696
}
```

```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/lists/groceries/items/milk \
  -H 'Content-Type: application/json' \
  -d '{"name": "Oat milk", "note": "unsweetened", "quantity": null}'
```

`200`

```json
{
  "id": "milk",
  "list_id": "groceries",
  "name": "Oat milk",
  "quantity": null,
  "note": "unsweetened",
  "category": "dairy",
  "checked": false,
  "checked_by": null,
  "checked_at": null,
  "added_by": "alice",
  "added_at": 1791802280671
}
```

```sh
curl -s -X PATCH http://127.0.0.1:7430/api/v1/lists/bunnings/items/milk \
  -H 'Content-Type: application/json' \
  -d '{"name": "x"}'
```

`404` (that item is on another list)

```json
{
  "error": {
    "code": "not_found",
    "message": "milk not found"
  }
}
```


### DELETE /lists/{id}/items/{item_id}

Remove an item (it stays in suggestion history). `204`, also when repeated.

```sh
curl -s -X DELETE http://127.0.0.1:7430/api/v1/lists/groceries/items/typo -H 'Content-Type: application/json'
```

`204`


### POST /lists/{id}/clear-checked

Hide the ticked items from the list; returns how many. They stay in the document and are offered by `suggestions` again.

```sh
curl -s -X POST http://127.0.0.1:7430/api/v1/lists/groceries/clear-checked \
  -H 'Content-Type: application/json' \
  -d '{}'
```

`200`

```json
{
  "cleared": 1
}
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/lists/groceries/items'
```

`200` (Apples are gone)

```json
[
  {
    "id": "milk",
    "list_id": "groceries",
    "name": "Oat milk",
    "quantity": null,
    "note": "unsweetened",
    "category": "dairy",
    "checked": false,
    "checked_by": null,
    "checked_at": null,
    "added_by": "alice",
    "added_at": 1791802280671
  },
  {
    "id": "soap",
    "list_id": "groceries",
    "name": "Dish soap",
    "quantity": null,
    "note": null,
    "category": "household",
    "checked": false,
    "checked_by": null,
    "checked_at": null,
    "added_by": "alice",
    "added_at": 1791802280707
  }
]
```


### GET /lists/{id}/suggestions

**Read-only.**

Names to autocomplete the quick-add box, derived from every item ever added to this list (including cleared and deleted ones): `q` is matched case-insensitively anywhere in the name, names that **start** with `q` first, then the ones added most often; names currently on the list are left out. `limit` 1–20, default 8. Nothing is stored for this.

```sh
curl -s 'http://127.0.0.1:7430/api/v1/lists/groceries/suggestions?q=app'
```

`200`

```json
[
  "Apples"
]
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/lists/groceries/suggestions?q=milk'
```

`200` (Oat milk is on the list)

```json
[]
```

```sh
curl -s 'http://127.0.0.1:7430/api/v1/lists/groceries/suggestions'
```

`200`

```json
[
  "Apples",
  "Bred"
]
```


### GET /events

**Read-only.** A Server-Sent Events stream (`Content-Type: text/event-stream`) that says when a document changed, so an open screen can refetch instead of polling. Browsers: `new EventSource("/api/v1/events")` (it reconnects by itself). Needs an identity like every other endpoint (`401` otherwise; note that `EventSource` does not retry after an error status).

Each message is one JSON object, `{"doc": "todo" | "fitness" | "shopping", "version": <n>}`. It is sent whenever that document changes: a write through this API, changes merged in from another device, or an edit made on disk by another process (`peer todo add`). `version` is a per-document counter that restarts with the server; treat it only as "it moved".

* When a client connects (and so every time `EventSource` reconnects), it first gets one message per document with the current counter. That is the cue to refetch what was missed while disconnected.
* A client that cannot keep up is not dropped: it gets those same three messages again.
* A comment line `: keep-alive` is sent every 15 seconds so proxies (`tailscale serve`) leave an idle stream open.
* Rejected writes (400, 409, ...) change nothing and send nothing.

Captured from the real server with one terminal streaming and another adding things (`curl -s -N http://127.0.0.1:7430/api/v1/events`):

```
data: {"doc":"todo","version":0}

data: {"doc":"fitness","version":0}

data: {"doc":"shopping","version":0}

data: {"doc":"shopping","version":1}      <- PUT /lists/groceries

data: {"doc":"shopping","version":2}      <- PUT .../items/milk

data: {"doc":"todo","version":1}          <- PUT /todos/t1

: keep-alive
```
