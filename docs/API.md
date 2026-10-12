# Data API v1

JSON over HTTP, served by `peer serve` under `/api/v1`. The web frontend uses only this API (`web/src/api.ts`), and the v2 assistant's tools will be built from the **read-only** endpoints. Code: `crates/web/src/api.rs`, `crates/web/src/identity.rs`.

Every example below is a real request against `peer serve --dev-user alice` (empty data dir, `timezone = "Australia/Sydney"`, run on 2026-10-12), in the order shown. Long lists are cut to two entries plus `"…"`.

## Conventions

**Read-only endpoints** (safe for an assistant to call freely): every `GET`. That's `/health`, `/me`, `/todos`, `/exercises`, `/templates`, `/templates/{id}`, `/plan`, `/blocks`, `/planned-day`, `/program-status`, `/sessions`, `/sessions/{id}`, `/tests`.

**Identity.** The current user comes from `--dev-user`, else the `Tailscale-User-Login` header that `tailscale serve` adds (mapped through `[web.users]` in `config.toml`), else the picker cookie (`PUT /me`). Without one, every endpoint except `/health` and `/me` returns `401 no_user`. See `docs/LEARNING.md` v1.3.

**Shared vs per-user.** Todos, exercises and templates are shared: either user can change them. Plans, blocks, sessions, tests and external activities belong to a user. Reads default to the current user; `?user=<id>` reads the other user's. Writing the other user's resource returns `403`.

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

