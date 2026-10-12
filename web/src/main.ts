// The shell, the picker, todos and exercises. Training screens are in train.ts,
// programs.ts and progress.ts. Backend calls only through ./api.ts.

import { api, ApiError, newId } from "./api";
import { h, optNum, select, today, val } from "./dom";
import { app, clearError, hooks, run, showError, syncWakeLock, ui, wake, type Area, type Fit } from "./state";
import { programsScreen } from "./programs";
import { progressScreen } from "./progress";
import { logScreen, todayScreen } from "./train";

// ---- shell ----
async function render() {
  wake.want = false;
  let body: HTMLElement;
  try {
    ui.me = await api.me();
    if (!ui.me.user_id) body = picker();
    else {
      const screen = ui.area === "todos" ? await todosScreen() : await fitnessScreen();
      const tab = (area: Area, label: string) =>
        h("button", { class: ui.area === area ? "active" : "", onclick: () => { ui.area = area; render(); } }, label);
      const fitTabs: [Fit, string][] = [["today", "Today"], ["log", "Log"], ["programs", "Programs"], ["progress", "Progress"], ["exercises", "Exercises"]];
      body = h("div", {},
        h("nav", {}, tab("fitness", "Fitness"), tab("todos", "Todos"),
          h("span", { class: "muted grow", style: "text-align:right" }, ui.me.user_id,
            ui.me.users.length > 1 ? " · " : "",
            ui.me.users.length > 1 ? h("a", { href: "#", onclick: (e: Event) => { e.preventDefault(); ui.me.user_id = null; app.replaceChildren(picker()); } }, "switch") : null)),
        ui.area === "fitness" && h("nav", { class: "sub" }, fitTabs.map(([t, label]) =>
          h("button", { class: ui.fit === t ? "active" : "", onclick: () => { ui.fit = t; ui.draftProgram = null; render(); } }, label))),
        screen);
    }
  } catch (e) {
    showError(e instanceof ApiError || e instanceof Error ? e : String(e), render);
    body = h("div", { class: "muted" }, "Couldn't load. Check the connection and retry.");
  }
  app.replaceChildren(body);
  syncWakeLock();
}
hooks.render = render;

function picker() {
  return h("div", {},
    h("h2", {}, "Who's using this device?"),
    h("p", { class: "muted" }, "Remembered in a cookie on this browser."),
    (ui.me?.users ?? []).map((u: string) => h("button", { class: "primary", style: "width:100%;margin:4px 0", onclick: () => run(() => api.pickUser(u)) }, u)));
}

// ---- todos ----
async function todosScreen() {
  const todos = await api.todos();
  const editing = todos.find((t) => t.id === ui.editingTodo);
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const body = { title: val(form, "title"), notes: val(form, "notes"), due: val(form, "due") || null };
      const id = editing?.id ?? newId(); // fixed before the first attempt, so Retry reuses it
      run(async () => {
        if (editing) await api.patchTodo(id, body);
        else await api.putTodo(id, body);
        ui.editingTodo = null;
      });
    },
  },
    h("input", { name: "title", placeholder: "Todo", required: true, maxlength: 200, value: editing?.title ?? "" }),
    h("textarea", { name: "notes", placeholder: "Notes", maxlength: 2000 }, editing?.notes ?? ""),
    h("div", { class: "row" },
      h("input", { name: "due", type: "date", value: editing?.due ?? "" }),
      h("button", { class: "primary" }, editing ? "Save" : "Add"),
      editing && h("button", { type: "button", onclick: () => { ui.editingTodo = null; render(); } }, "Cancel")));
  const now = today();
  return h("div", {}, form, todos.map((t) => h("div", { class: "card" },
    h("div", { class: "row" },
      h("button", { "aria-label": t.completed ? "Mark not done" : "Mark done", onclick: () => run(() => api.setTodoDone(t.id, !t.completed)) }, t.completed ? "☑" : "☐"),
      h("div", { class: "grow" },
        h("div", { class: t.completed ? "done" : "" }, t.title),
        t.due && h("div", { class: !t.completed && t.due < now ? "bad muted" : "muted" }, `${t.due < now && !t.completed ? "Overdue · " : ""}${t.due}`),
        t.notes && h("div", { class: "muted" }, t.notes)),
      h("button", { onclick: () => { ui.editingTodo = t.id; render(); } }, "Edit"),
      h("button", { onclick: () => confirm(`Delete "${t.title}"?`) && run(() => api.deleteTodo(t.id)) }, "Delete")))));
}

// ---- fitness ----
async function fitnessScreen(): Promise<HTMLElement> {
  switch (ui.fit) {
    case "today": return todayScreen();
    case "log": return logScreen();
    case "programs": return programsScreen();
    case "progress": return progressScreen();
    case "exercises": return exercisesScreen();
  }
}

const MEASUREMENTS = ["WeightReps", "Reps", "Timed", "Distance"];
const EQUIPMENT = ["Barbell", "Dumbbell", "Machine", "Cable", "Bodyweight", "Other"];
const MUSCLES = ["chest", "back", "shoulders", "biceps", "triceps", "quads", "hamstrings", "glutes", "calves", "abs", "forearms"];

async function exercisesScreen() {
  const exercises = await api.exercises();
  const editing = exercises.find((e) => e.id === ui.editingExercise);
  const group = (name: string, label: string, selected: string[]) =>
    h("fieldset", { class: "muscles" }, h("legend", { class: "muted" }, label),
      MUSCLES.map((m) => h("label", { class: "chip-check" }, h("input", { type: "checkbox", name, value: m, checked: selected.includes(m) }), m)));
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const checked = (name: string) => [...form.querySelectorAll(`input[name="${name}"]:checked`)].map((i) => (i as HTMLInputElement).value);
      const body = {
        name: val(form, "name"), measurement: val(form, "measurement"), tempo: val(form, "tempo") || null,
        equipment: val(form, "equipment") || null, default_rest_s: optNum(val(form, "rest")),
        primary_muscles: checked("primary"), secondary_muscles: checked("secondary"),
      };
      const id = editing?.id ?? newId();
      run(async () => {
        if (editing) await api.patchExercise(id, body);
        else await api.putExercise(id, body);
        ui.editingExercise = null;
      });
    },
  },
    h("input", { name: "name", placeholder: "Exercise name", required: true, maxlength: 200, value: editing?.name ?? "" }),
    h("div", { class: "row" },
      select("measurement", MEASUREMENTS.map((m) => [m, m]), editing?.measurement ?? "WeightReps"),
      select("equipment", [["", "Equipment…"], ...EQUIPMENT.map((q) => [q, q] as [string, string])], editing?.equipment ?? "", { "aria-label": "Equipment" })),
    h("div", { class: "row" },
      h("input", { name: "tempo", placeholder: "Tempo (e.g. 2:1)", maxlength: 20, value: editing?.tempo ?? "" }),
      h("input", { name: "rest", type: "number", min: 0, max: 3600, placeholder: "Rest s", "aria-label": "Default rest seconds", value: editing?.default_rest_s ?? "" })),
    group("primary", "Primary muscles", editing?.primary_muscles ?? []),
    group("secondary", "Secondary muscles", editing?.secondary_muscles ?? []),
    h("div", { class: "row" },
      h("button", { class: "primary" }, editing ? "Save" : "Add exercise"),
      editing && h("button", { type: "button", onclick: () => { ui.editingExercise = null; render(); } }, "Cancel")));
  return h("div", {}, form, exercises.map((e) => h("div", { class: "card" },
    h("div", { class: "row" },
      h("div", { class: "grow" }, h("div", {}, e.name),
        h("div", { class: "muted" }, [e.measurement, e.equipment, e.tempo && `tempo ${e.tempo}`, e.default_rest_s != null && `rest ${e.default_rest_s}s`].filter(Boolean).join(" · ")),
        (e.primary_muscles.length > 0 || e.secondary_muscles.length > 0) && h("div", { class: "muted" }, `${e.primary_muscles.join(", ") || "–"}${e.secondary_muscles.length ? ` (+ ${e.secondary_muscles.join(", ")})` : ""}`)),
      h("button", { onclick: () => { ui.editingExercise = e.id; render(); } }, "Edit")))));
}

clearError();
render();
