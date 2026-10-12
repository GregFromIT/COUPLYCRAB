// The only module that talks to the backend. See docs/API.md.

export class ApiError extends Error {
  constructor(public status: number, public code: string, message: string) {
    super(message);
  }
}

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`/api/v1${path}`, {
      method,
      credentials: "same-origin",
      // Every write sends the JSON content type (the server's CSRF guard requires it).
      headers: method === "GET" ? {} : { "Content-Type": "application/json" },
      body: method === "GET" ? undefined : JSON.stringify(body ?? {}),
    });
  } catch {
    throw new ApiError(0, "network", "Connection problem. Nothing was lost: retry when you're back online.");
  }
  if (res.status === 204) return undefined as T;
  const data = await res.json().catch(() => null);
  if (!res.ok) {
    if (data?.error) throw new ApiError(res.status, data.error.code, data.error.message);
    // No JSON error body: a proxy (Vite in dev, tailscale serve) couldn't reach the backend.
    throw new ApiError(res.status, "unreachable", `Can't reach the server (HTTP ${res.status}). Nothing was lost: retry in a moment.`);
  }
  return data as T;
}

/** Client-generated ID for idempotent PUTs. `crypto.randomUUID` needs a secure
 * context (HTTPS or localhost); fall back to a v4 UUID from getRandomValues. */
export function newId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

const q = (params: Record<string, string | undefined>) => {
  const s = new URLSearchParams(Object.entries(params).filter(([, v]) => v !== undefined) as [string, string][]).toString();
  return s ? `?${s}` : "";
};

export const api = {
  me: () => req<any>("GET", "/me"),
  pickUser: (user_id: string) => req<any>("PUT", "/me", { user_id }),

  todos: () => req<any[]>("GET", "/todos"),
  putTodo: (id: string, t: { title: string; notes: string; due: string | null }) => req<any>("PUT", `/todos/${id}`, t),
  patchTodo: (id: string, t: Record<string, unknown>) => req<any>("PATCH", `/todos/${id}`, t),
  setTodoDone: (id: string, done: boolean) => req<any>("POST", `/todos/${id}/${done ? "complete" : "uncomplete"}`),
  deleteTodo: (id: string) => req<void>("DELETE", `/todos/${id}`),

  exercises: () => req<any[]>("GET", "/exercises"),
  putExercise: (id: string, e: Record<string, unknown>) => req<any>("PUT", `/exercises/${id}`, e),
  patchExercise: (id: string, e: Record<string, unknown>) => req<any>("PATCH", `/exercises/${id}`, e),

  gymProfiles: () => req<any[]>("GET", "/gym-profiles"),
  putGym: (id: string, g: Record<string, unknown>) => req<any>("PUT", `/gym-profiles/${id}`, g),
  patchGym: (id: string, g: Record<string, unknown>) => req<any>("PATCH", `/gym-profiles/${id}`, g),

  programs: () => req<any[]>("GET", "/programs"),
  putProgram: (id: string, p: Record<string, unknown>) => req<any>("PUT", `/programs/${id}`, p),
  patchProgram: (id: string, p: Record<string, unknown>) => req<any>("PATCH", `/programs/${id}`, p),
  migrateV1: () => req<{ created: number; notes: string[] }>("POST", "/programs/migrate-v1"),
  /** v1 blocks, to offer the migration. 410 once programs exist. */
  v1Blocks: () => req<any[]>("GET", "/blocks"),

  nextWorkout: (date?: string) => req<any>("GET", `/workouts/next${q({ date })}`),
  recommendations: (program_id: string, workout_id: string, date?: string) =>
    req<any>("GET", `/recommendations${q({ program_id, workout_id, date })}`),
  volume: (weeks: number) => req<any>("GET", `/analytics/volume${q({ weeks: String(weeks) })}`),
  e1rm: (exercise_id: string) => req<any>("GET", `/analytics/e1rm${q({ exercise_id })}`),
  prs: () => req<any[]>("GET", "/prs"),

  sessions: (since?: string) => req<any[]>("GET", `/sessions${q({ since, limit: "50" })}`),
  putSession: (id: string, s: Record<string, unknown>) => req<any>("PUT", `/sessions/${id}`, s),
  putSet: (sessionId: string, setId: string, set: Record<string, unknown>) => req<any>("PUT", `/sessions/${sessionId}/sets/${setId}`, set),
  finish: (sessionId: string, status: string, note: string | null) => req<any>("POST", `/sessions/${sessionId}/finish`, { status, note }),

  tests: (kind: string) => req<any[]>("GET", `/tests${q({ kind })}`),
  putTest: (id: string, t: Record<string, unknown>) => req<any>("PUT", `/tests/${id}`, t),
};
