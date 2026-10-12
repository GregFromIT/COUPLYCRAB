// Shared UI state, the error banner, `run` (write + retry), and the wake lock.

import { ApiError } from "./api";
import { h, storage } from "./dom";

export type Fit = "today" | "log" | "programs" | "progress" | "exercises";
export type Area = "fitness" | "shopping" | "todos";
export type Progress = "volume" | "strength" | "prs" | "tests";

export const ui = {
  area: "fitness" as Area,
  fit: "today" as Fit,
  me: null as any,
  editingTodo: null as string | null,
  editingExercise: null as string | null,
  progress: "volume" as Progress,
  testKind: "BeepTest",
  strengthExercise: "",
  volumeMuscle: "",
  /** Builder state for a program being created or edited; see programs.ts. */
  draftProgram: null as any,
  editingGym: null as string | null,
  /** Per-exercise entry values on Log Workout, keyed `<session>:<item or blank>`. */
  entry: {} as Record<string, Record<string, string>>,
  /** Client set IDs awaiting success, so a re-tap after a failure reuses the ID (idempotent). */
  pendingSet: {} as Record<string, { id: string; key: string; set: Record<string, unknown> }>,
  finishNote: "",
  /** What the user was shown per exercise during the session, for the "next time" summary. */
  shown: {} as Record<string, { load_kg: number; reps: number }>,
  lastFinished: null as null | { sessionId: string; programId: string; workoutId: string; date: string; name: string },
  lastSetAt: 0,
  prefill: (storage.get("prefill") === "previous" ? "previous" : "smart") as "smart" | "previous",
  /** Shopping: the open list and the draft in the quick-add box (survives a re-render). */
  list: null as string | null,
  quickAdd: "",
};

/** Set by main.ts, so screens can ask for a re-render without importing it. */
export const hooks = { render: (async () => {}) as () => Promise<void> };
export const render = () => hooks.render();

export const app = document.getElementById("app")!;
const errorBox = document.getElementById("error")!;

export function showError(e: unknown, retry?: () => void) {
  const msg = e instanceof ApiError ? e.message : String(e);
  errorBox.replaceChildren(...[
    h("span", { class: "msg" }, msg),
    retry && h("button", { class: "primary", onclick: () => retry() }, "Retry"),
    h("button", { onclick: () => errorBox.replaceChildren() }, "Dismiss"),
  ].filter((x): x is HTMLElement => !!x));
}

export const clearError = () => errorBox.replaceChildren();

/** Run a write. On success re-render. On failure leave the screen as is (entered
 * values stay) and offer Retry, which re-sends the exact same request: same
 * client ID, same values. Writes are idempotent, so a retry can't duplicate. */
export async function run(fn: () => Promise<unknown>) {
  clearError();
  try {
    await fn();
  } catch (e) {
    showError(e, () => run(fn));
    return;
  }
  await render();
}

// ---- wake lock (Log Workout) ----
// https://developer.mozilla.org/en-US/docs/Web/API/Screen_Wake_Lock_API
// Needs a secure context (HTTPS, or localhost); elsewhere `navigator.wakeLock`
// is missing and we silently do nothing.
export const wake = { want: false };
let wakeLock: any = null;
export async function syncWakeLock() {
  const wl = (navigator as any).wakeLock;
  if (wake.want && !wakeLock && wl && document.visibilityState === "visible") {
    try {
      wakeLock = await wl.request("screen");
      wakeLock.addEventListener("release", () => { wakeLock = null; awakeLabel(); });
    } catch {
      wakeLock = null;
    }
  } else if (!wake.want && wakeLock) {
    const l = wakeLock;
    wakeLock = null;
    await l.release().catch(() => {});
  }
  awakeLabel();
}
function awakeLabel() {
  const el = document.getElementById("awake");
  if (el) el.textContent = wakeLock ? "Screen kept awake" : "Screen may sleep";
}
// The browser drops the lock when the tab is hidden; take it again on return.
document.addEventListener("visibilitychange", syncWakeLock);
