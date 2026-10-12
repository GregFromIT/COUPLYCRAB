// Plain DOM UI. All data is rendered with textContent (via `h`), never innerHTML.
// Backend calls only through ./api.ts.

import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { api, ApiError, newId } from "./api";

type Props = Record<string, unknown>;
type Child = Node | string | null | undefined | false;

function h(tag: string, props: Props = {}, ...children: (Child | Child[])[]): HTMLElement {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v as EventListener);
    else if (k === "class") el.className = String(v);
    else if (v === true) el.setAttribute(k, "");
    else if (v !== false && v != null) {
      if (k === "value") (el as HTMLInputElement).value = String(v);
      else el.setAttribute(k, String(v));
    }
  }
  for (const c of children.flat()) if (c) el.append(c);
  return el;
}

const pad = (n: number) => String(n).padStart(2, "0");
const localDate = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const today = () => localDate(new Date());
const val = (form: HTMLElement, name: string) => (form.querySelector(`[name="${name}"]`) as HTMLInputElement).value.trim();
const optNum = (s: string | undefined) => (s === undefined || s === "" ? null : Number(s));
const select = (name: string, options: [string, string][], selected = "", props: Props = {}) =>
  h("select", { name, ...props }, options.map(([v, label]) => h("option", { value: v, selected: v === selected }, label)));
const storage = {
  get: (k: string) => { try { return localStorage.getItem(k); } catch { return null; } },
  set: (k: string, v: string | null) => { try { v === null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* private mode */ } },
};

// ---- state ----
type Fit = "today" | "log" | "plan" | "blocks" | "templates" | "exercises" | "tests";
const ui = {
  area: "fitness" as "fitness" | "todos",
  fit: "today" as Fit,
  me: null as any,
  editingTodo: null as string | null,
  editingExercise: null as string | null,
  draftItems: [] as Record<string, string>[],
  draftTemplate: { name: "", minutes: "" }, // survives the re-render "+ Item" triggers
  planDraft: null as Record<string, { kind: string; value: string }> | null,
  testKind: "BeepTest",
  entry: {} as Record<string, Record<string, string>>, // per-exercise stepper values on Log Session
  pendingSet: {} as Record<string, { id: string; body: string }>, // client set IDs awaiting success
  finishNote: "",
};

const app = document.getElementById("app")!;
const errorBox = document.getElementById("error")!;

function showError(e: unknown, retry?: () => void) {
  const msg = e instanceof ApiError ? e.message : String(e);
  errorBox.replaceChildren(...[
    h("span", { class: "msg" }, msg),
    retry && h("button", { class: "primary", onclick: () => retry() }, "Retry"),
    h("button", { onclick: () => errorBox.replaceChildren() }, "Dismiss"),
  ].filter((x): x is HTMLElement => !!x));
}

/** Run a write. On success re-render. On failure leave the screen as is (entered
 * values stay) and offer Retry, which re-sends the exact same request: same
 * client ID, same values. Writes are idempotent, so a retry can't duplicate. */
async function run(fn: () => Promise<unknown>) {
  errorBox.replaceChildren();
  try {
    await fn();
  } catch (e) {
    showError(e, () => run(fn));
    return;
  }
  await render();
}

// ---- wake lock (Log Session) ----
// https://developer.mozilla.org/en-US/docs/Web/API/Screen_Wake_Lock_API
// Needs a secure context (HTTPS, or localhost); elsewhere `navigator.wakeLock`
// is missing and we silently do nothing.
let wantAwake = false;
let wakeLock: any = null;
async function syncWakeLock() {
  const wl = (navigator as any).wakeLock;
  if (wantAwake && !wakeLock && wl && document.visibilityState === "visible") {
    try {
      wakeLock = await wl.request("screen");
      wakeLock.addEventListener("release", () => { wakeLock = null; awakeLabel(); });
    } catch {
      wakeLock = null;
    }
  } else if (!wantAwake && wakeLock) {
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

// ---- shell ----
async function render() {
  wantAwake = false;
  let body: HTMLElement;
  try {
    ui.me = await api.me();
    if (!ui.me.user_id) body = picker();
    else {
      const screen = ui.area === "todos" ? await todosScreen() : await fitnessScreen();
      const tab = (area: "fitness" | "todos", label: string) =>
        h("button", { class: ui.area === area ? "active" : "", onclick: () => { ui.area = area; render(); } }, label);
      const fitTabs: [Fit, string][] = [["today", "Today"], ["log", "Log"], ["plan", "Plan"], ["blocks", "Blocks"], ["templates", "Templates"], ["exercises", "Exercises"], ["tests", "Tests"]];
      body = h("div", {},
        h("nav", {}, tab("fitness", "Fitness"), tab("todos", "Todos"),
          h("span", { class: "muted grow", style: "text-align:right" }, ui.me.user_id,
            ui.me.users.length > 1 ? " · " : "",
            ui.me.users.length > 1 ? h("a", { href: "#", onclick: (e: Event) => { e.preventDefault(); ui.me.user_id = null; app.replaceChildren(picker()); } }, "switch") : null)),
        ui.area === "fitness" && h("nav", { class: "sub" }, fitTabs.map(([t, label]) =>
          h("button", { class: ui.fit === t ? "active" : "", onclick: () => { ui.fit = t; ui.planDraft = null; render(); } }, label))),
        screen);
    }
  } catch (e) {
    showError(e, render);
    body = h("div", { class: "muted" }, "Couldn't load. Check the connection and retry.");
  }
  app.replaceChildren(body);
  syncWakeLock();
}

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
const MEASUREMENTS = ["WeightReps", "Reps", "Timed", "Distance"];
// Which SetLog fields each measurement type logs, with stepper increments.
const FIELDS: Record<string, [string, string, number][]> = {
  WeightReps: [["reps", "Reps", 1], ["load", "kg", 2.5]],
  Reps: [["reps", "Reps", 1]],
  Timed: [["duration_s", "Seconds", 5]],
  Distance: [["distance_m", "Metres", 100]],
};

async function fitnessScreen(): Promise<HTMLElement> {
  switch (ui.fit) {
    case "today": return todayScreen();
    case "log": return logScreen();
    case "plan": return planScreen();
    case "blocks": return blocksScreen();
    case "templates": return templatesScreen();
    case "exercises": return exercisesScreen();
    case "tests": return testsScreen();
  }
}

function targetText(it: any, name: string) {
  return [name, `${it.target_sets}×${it.target_reps ?? "–"}`, it.target_load != null && `@ ${it.target_load} kg`,
    it.tempo && `tempo ${it.tempo}`, it.rest_s != null && `rest ${it.rest_s}s`].filter(Boolean).join(" ");
}

async function todayScreen() {
  const [day, sessions] = await Promise.all([api.plannedDay(), api.sessions(today())]);
  const b = day.block;
  const head = b
    ? h("div", { class: "card" }, h("strong", {}, b.name), h("div", { class: b.is_deload ? "bad" : "muted" }, `Week ${b.week} of ${b.weeks}${b.is_deload ? " · deload" : ""} · ${day.date}`))
    : h("div", { class: "card" }, h("strong", {}, "No program block today"), h("div", { class: "muted" }, `${day.date}. Add one under Blocks, with a weekly plan under Plan.`));
  const logged = (slot: string) => sessions.find((s) => b && s.block_id === b.block_id && s.block_week === b.week && s.slot === slot);

  const slots = day.slots.map((s: any) => {
    const done = logged(s.slot);
    const link = { block_id: b.block_id, block_week: b.week, slot: s.slot };
    if (s.template) {
      const t = s.template;
      const id = newId();
      const action = !done
        ? h("button", { class: "primary", onclick: () => run(async () => { await api.putSession(id, { template_id: t.template_id, ...link }); ui.fit = "log"; }) }, "Start")
        : done.status === "InProgress"
          ? h("button", { class: "primary", onclick: () => { ui.fit = "log"; render(); } }, "Continue")
          : h("div", { class: "ok" }, `Done ✓ ${done.status}${done.note ? ` · ${done.note}` : ""}`);
      return h("div", { class: "card" },
        h("div", {}, h("strong", {}, `${s.slot} · ${t.name}`), h("span", { class: "muted" }, ` · ${t.target_minutes} min`)),
        t.items.map((it: any) => h("div", { class: "muted" }, targetText(it, it.exercise_name ?? "(exercise)"))),
        action);
    }
    if (done) return h("div", { class: "card" }, h("strong", {}, `${s.slot} · ${s.external_label}`), h("div", { class: "ok" }, `Attended ✓${done.note ? ` · ${done.note}` : ""}`));
    const note = h("textarea", { placeholder: "Note (optional)", maxlength: 2000 }) as HTMLTextAreaElement;
    const id = newId();
    return h("div", { class: "card" }, h("strong", {}, `${s.slot} · ${s.external_label}`), note,
      h("button", { class: "primary", onclick: () => {
        const n = note.value.trim() || null;
        run(async () => { await api.putSession(id, { label: s.external_label, ...link }); await api.finish(id, "Completed", n); });
      } }, "Mark attended"));
  });
  return h("div", {}, head, slots, b && !day.slots.length && h("p", { class: "muted" }, "Nothing planned today."),
    h("button", { onclick: () => { ui.fit = "log"; render(); } }, "Log a session that isn't planned"));
}

function stepper(values: Record<string, string>, key: string, label: string, step: number) {
  const input = h("input", { type: "number", min: 0, step: "any", inputmode: "decimal", value: values[key] ?? "", "aria-label": label,
    oninput: (e: Event) => { values[key] = (e.target as HTMLInputElement).value; } }) as HTMLInputElement;
  const bump = (d: number) => () => { values[key] = String(Math.max(0, Math.round((Number(values[key] || 0) + d) * 100) / 100)); input.value = values[key]; };
  return h("div", { class: "stepper" }, h("span", { class: "muted" }, label),
    h("button", { type: "button", "aria-label": `${label} minus`, onclick: bump(-step) }, "−"), input,
    h("button", { type: "button", "aria-label": `${label} plus`, onclick: bump(step) }, "+"));
}

async function logScreen() {
  const [sessions, exercises, templates] = await Promise.all([api.sessions(), api.exercises(), api.templates()]);
  const exName = (id: string) => exercises.find((e) => e.id === id)?.name ?? "(unknown exercise)";
  const exOptions: [string, string][] = exercises.map((e) => [e.id, e.name]);
  const mine = sessions.find((s) => s.status === "InProgress");

  if (!mine) {
    const id = newId();
    const form = h("form", {
      onsubmit: (e: Event) => {
        e.preventDefault();
        const template_id = val(form, "template") || null;
        run(() => api.putSession(id, { template_id }));
      },
    },
      select("template", [["", "Blank session"], ...templates.map((t) => [t.id, t.name] as [string, string])]),
      h("button", { class: "primary" }, "Start session"));
    return h("div", {}, form, h("h2", {}, "Recent sessions"), sessions.slice(0, 20).map((s) => h("div", { class: "card" },
      h("div", {}, `${new Date(s.started_at).toLocaleString()} · ${s.label ?? templates.find((t) => t.id === s.template_id)?.name ?? "Blank"} · ${s.status}`),
      h("div", { class: "muted" }, `${Object.keys(s.sets).length} sets${s.block_week ? ` · block week ${s.block_week}` : ""}${s.note ? ` · ${s.note}` : ""}`))));
  }

  wantAwake = true;
  const template = templates.find((t) => t.id === mine.template_id);
  // Targets: the planned (progressed/deloaded) ones if the session came from the plan, else the template's base.
  let items: any[] = template ? Object.values(template.items).sort((a: any, b: any) => a.index - b.index) : [];
  if (template && mine.block_id && mine.slot) {
    const day = await api.plannedDay(localDate(new Date(mine.started_at)));
    const slot = day.slots.find((s: any) => s.slot === mine.slot && s.template?.template_id === template.id);
    if (slot) items = slot.template.items;
  }
  const sets = (Object.entries(mine.sets) as [string, any][]).sort((a, b) => a[1].index - b[1].index);
  const blocks: { key: string; item?: any }[] = items.length ? items.map((it, i) => ({ key: `${mine.id}:${i}`, item: it })) : [{ key: `${mine.id}:blank` }];

  const blockEl = (b: { key: string; item?: any }) => {
    const values = (ui.entry[b.key] ??= {});
    if (!b.item) values.exercise_id ??= exOptions[0]?.[0] ?? "";
    const exId = b.item?.exercise_id ?? values.exercise_id;
    const ex = exercises.find((e) => e.id === exId);
    const fields = FIELDS[ex?.measurement] ?? FIELDS.Reps;
    const done = sets.filter(([, s]) => s.exercise_id === exId).map(([, s]) => s);
    // Prefill: last set of this exercise in this session, else the target.
    const last = done[done.length - 1];
    for (const [k] of fields) values[k] ??= String(last?.[k] ?? (k === "reps" ? b.item?.target_reps : k === "load" ? b.item?.target_load : "") ?? "");
    const logSet = () => {
      const set: Record<string, unknown> = { exercise_id: exId };
      for (const [k] of fields) set[k] = optNum(values[k]);
      const body = JSON.stringify(set);
      // Same values as a failed attempt → same set ID (retry); changed values → a new set.
      const pending = ui.pendingSet[b.key];
      const setId = pending && pending.body === body ? pending.id : newId();
      ui.pendingSet[b.key] = { id: setId, body };
      run(async () => { await api.putSet(mine.id, setId, set); delete ui.pendingSet[b.key]; });
    };
    return h("div", { class: "card" },
      b.item
        ? h("div", {}, h("strong", {}, exName(exId)), h("div", { class: "muted" }, `${done.length}/${b.item.target_sets} sets · target ${targetText(b.item, "")}`))
        : select("", exOptions, exId, { onchange: (e: Event) => { ui.entry[b.key] = { exercise_id: (e.target as HTMLSelectElement).value }; render(); } }),
      done.map((s, i) => h("div", { class: "muted" }, `Set ${i + 1}: ${fields.map(([k, l]) => `${s[k] ?? "–"} ${l}`).join(" · ")}`)),
      fields.map(([k, l, step]) => stepper(values, k, l, step)),
      h("button", { class: "primary", disabled: !exId, onclick: logSet }, "Log set"));
  };

  const note = h("textarea", { placeholder: "Note (optional)", maxlength: 2000, oninput: (e: Event) => { ui.finishNote = (e.target as HTMLTextAreaElement).value; } }, ui.finishNote) as HTMLTextAreaElement;
  const finish = (status: string) => () => {
    const n = ui.finishNote.trim() || null;
    run(async () => { await api.finish(mine.id, status, n); ui.finishNote = ""; });
  };
  return h("div", {},
    h("div", { class: "row" }, h("h2", { class: "grow" }, template ? template.name : "Blank session"), h("span", { id: "awake", class: "muted" })),
    h("div", { class: "muted" }, `Started ${new Date(mine.started_at).toLocaleTimeString()}${mine.block_week ? ` · block week ${mine.block_week} ${mine.slot ?? ""}` : ""}${template ? ` · target ${template.target_minutes} min` : ""}`),
    blocks.map(blockEl),
    h("div", { class: "card" }, note,
      h("div", { class: "row" },
        h("button", { class: "primary", onclick: finish("Completed") }, "Finish: Completed"),
        h("button", { onclick: finish("Partial") }, "Finish: Partial"))));
}

const DAYS: [string, string][] = [["mon", "Mon"], ["tue", "Tue"], ["wed", "Wed"], ["thu", "Thu"], ["fri", "Fri"], ["sat", "Sat"], ["sun", "Sun"]];

async function planScreen() {
  const [plan, templates] = await Promise.all([api.plan(), api.templates()]);
  if (!ui.planDraft) {
    ui.planDraft = {};
    for (const [k, s] of Object.entries(plan.slots) as [string, any][]) {
      ui.planDraft[k] = s.template_id ? { kind: "template", value: s.template_id } : { kind: "external", value: s.external_label };
    }
  }
  const draft = ui.planDraft;
  const options: [string, string][] = [["", "Rest"], ...templates.map((t) => [`t:${t.id}`, t.name] as [string, string]), ["ext", "External…"]];
  const slotEl = (key: string) => {
    const d = draft[key];
    const current = !d ? "" : d.kind === "template" ? `t:${d.value}` : "ext";
    return h("div", {},
      select("", options, current, { "aria-label": key, onchange: (e: Event) => {
        const v = (e.target as HTMLSelectElement).value;
        if (!v) delete draft[key];
        else if (v === "ext") draft[key] = { kind: "external", value: "" };
        else draft[key] = { kind: "template", value: v.slice(2) };
        render();
      } }),
      d?.kind === "external" && h("input", { placeholder: "Label, e.g. Unit PT", maxlength: 200, value: d.value, oninput: (e: Event) => { d.value = (e.target as HTMLInputElement).value; } }));
  };
  const save = () => {
    const slots: Record<string, unknown> = {};
    for (const [k, d] of Object.entries(draft)) {
      if (d.kind === "template") slots[k] = { template_id: d.value };
      else if (d.value.trim()) slots[k] = { external_label: d.value.trim() };
    }
    run(async () => { await api.putPlan(slots); ui.planDraft = null; });
  };
  return h("div", {},
    h("p", { class: "muted" }, "Your week. Program blocks repeat it."),
    h("div", { class: "plan-row muted" }, h("span", {}), h("span", {}, "AM"), h("span", {}, "PM")),
    DAYS.map(([d, label]) => h("div", { class: "plan-row", style: "margin:6px 0" }, h("strong", {}, label), slotEl(`${d}_am`), slotEl(`${d}_pm`))),
    h("button", { class: "primary", style: "width:100%;margin-top:8px", onclick: save }, "Save plan"));
}

async function blocksScreen() {
  const blocks = await api.blocks();
  const now = today();
  const id = newId();
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const deload_week = optNum(val(form, "deload_week"));
      run(() => api.putBlock(id, {
        name: val(form, "name"), start_date: val(form, "start_date"), weeks: Number(val(form, "weeks")),
        deload_week, deload_pct: deload_week ? optNum(val(form, "deload_pct")) : null,
      }));
    },
  },
    h("input", { name: "name", placeholder: "Block name", required: true, maxlength: 200 }),
    h("div", { class: "row" },
      h("input", { name: "start_date", type: "date", required: true, "aria-label": "Start (a Monday)" }),
      h("input", { name: "weeks", type: "number", min: 1, max: 104, placeholder: "Weeks", required: true })),
    h("div", { class: "row" },
      h("input", { name: "deload_week", type: "number", min: 1, placeholder: "Deload week (optional)" }),
      h("input", { name: "deload_pct", type: "number", min: 1, max: 100, step: "any", placeholder: "Deload load %", value: 60 })),
    h("p", { class: "muted" }, "Starts on a Monday and repeats your weekly plan. Deload: load × %, no progression that week."),
    h("button", { class: "primary" }, "Add block"));
  const end = (b: any) => { const d = new Date(`${b.start_date}T00:00`); d.setDate(d.getDate() + b.weeks * 7 - 1); return localDate(d); };
  return h("div", {}, form, blocks.map((b) => h("div", { class: "card" },
    h("div", {}, h("strong", {}, b.name), b.start_date <= now && now <= end(b) ? h("span", { class: "ok" }, " · current") : null),
    h("div", { class: "muted" }, `${b.start_date} → ${end(b)} · ${b.weeks} weeks${b.deload_week ? ` · deload week ${b.deload_week} at ${b.deload_pct}%` : ""}`),
    h("button", { onclick: () => confirm(`Delete block "${b.name}"?`) && run(() => api.patchBlock(b.id, { deleted: true })) }, "Delete"))));
}

async function templatesScreen() {
  const [templates, exercises] = await Promise.all([api.templates(), api.exercises()]);
  const exName = (id: string) => exercises.find((e) => e.id === id)?.name ?? "(unknown exercise)";
  const exOptions: [string, string][] = exercises.map((e) => [e.id, e.name]);
  const id = newId();
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const items = ui.draftItems.map((d) => ({
        exercise_id: d.exercise_id, target_sets: Number(d.target_sets || 1), target_reps: optNum(d.target_reps),
        target_load: optNum(d.target_load), tempo: d.tempo || null, rest_s: optNum(d.rest_s),
        progression: d.prog ? { kind: d.prog, per_week: Number(d.per_week || 0) } : null,
      }));
      const body = { name: val(form, "name"), target_minutes: Number(val(form, "minutes") || 0), items };
      run(async () => { await api.putTemplate(id, body); ui.draftItems = []; ui.draftTemplate = { name: "", minutes: "" }; });
    },
  },
    h("div", { class: "row" },
      h("input", { name: "name", placeholder: "Template name", required: true, maxlength: 200, value: ui.draftTemplate.name,
        oninput: (e: Event) => { ui.draftTemplate.name = (e.target as HTMLInputElement).value; } }),
      h("input", { name: "minutes", type: "number", min: 0, placeholder: "Target minutes", value: ui.draftTemplate.minutes,
        oninput: (e: Event) => { ui.draftTemplate.minutes = (e.target as HTMLInputElement).value; } })),
    ui.draftItems.map((d, i) => {
      const bind = (k: string) => ({ value: d[k] ?? "", oninput: (e: Event) => { d[k] = (e.target as HTMLInputElement).value; } });
      return h("div", { class: "card" },
        select("", exOptions, d.exercise_id, { onchange: (e: Event) => { d.exercise_id = (e.target as HTMLSelectElement).value; } }),
        h("div", { class: "row" },
          h("input", { type: "number", min: 1, placeholder: "Sets", ...bind("target_sets") }),
          h("input", { type: "number", min: 0, placeholder: "Reps", ...bind("target_reps") }),
          h("input", { type: "number", min: 0, step: "any", placeholder: "Load kg", ...bind("target_load") })),
        h("div", { class: "row" },
          h("input", { placeholder: "Tempo", maxlength: 20, ...bind("tempo") }),
          h("input", { type: "number", min: 0, placeholder: "Rest s", ...bind("rest_s") })),
        h("div", { class: "row" },
          select("", [["", "No progression"], ["LinearLoad", "+ kg / week"], ["LinearReps", "+ reps / week"]], d.prog ?? "", { onchange: (e: Event) => { d.prog = (e.target as HTMLSelectElement).value; } }),
          h("input", { type: "number", min: 0, step: "any", placeholder: "per week", ...bind("per_week") }),
          h("button", { type: "button", onclick: () => { ui.draftItems.splice(i, 1); render(); } }, "Remove")));
    }),
    h("button", {
      type: "button", disabled: exOptions.length === 0,
      onclick: () => { ui.draftItems.push({ exercise_id: exOptions[0]?.[0] ?? "", target_sets: "3" }); render(); },
    }, exOptions.length ? "+ Item" : "Add exercises first"),
    h("button", { class: "primary" }, "Save template"));
  const prog = (p: any) => !p ? "" : p.kind === "LinearLoad" ? ` · +${p.per_week} kg/wk` : p.kind === "LinearReps" ? ` · +${p.per_week} reps/wk` : "";
  return h("div", {}, form, templates.map((t) => h("div", { class: "card" },
    h("div", {}, `${t.name} · ${t.target_minutes} min`),
    (Object.values(t.items) as any[]).sort((a, b) => a.index - b.index).map((it) => h("div", { class: "muted" }, targetText(it, exName(it.exercise_id)) + prog(it.progression))))));
}

async function exercisesScreen() {
  const exercises = await api.exercises();
  const editing = exercises.find((e) => e.id === ui.editingExercise);
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const body = { name: val(form, "name"), measurement: val(form, "measurement"), tempo: val(form, "tempo") || null };
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
      h("input", { name: "tempo", placeholder: "Tempo (e.g. 2:1)", maxlength: 20, value: editing?.tempo ?? "" })),
    h("div", { class: "row" },
      h("button", { class: "primary" }, editing ? "Save" : "Add exercise"),
      editing && h("button", { type: "button", onclick: () => { ui.editingExercise = null; render(); } }, "Cancel")));
  return h("div", {}, form, exercises.map((e) => h("div", { class: "card" },
    h("div", { class: "row" },
      h("div", { class: "grow" }, h("div", {}, e.name), h("div", { class: "muted" }, [e.measurement, e.tempo && `tempo ${e.tempo}`].filter(Boolean).join(" · "))),
      h("button", { onclick: () => { ui.editingExercise = e.id; render(); } }, "Edit")))));
}

// ---- benchmark tests ----
const KINDS: [string, string, string][] = [
  ["BeepTest", "Beep test", "level.shuttle, e.g. 8.10"], ["OneRepMax", "1 rep max", "kg"],
  ["MaxReps", "Max reps", "reps"], ["TimedRun", "Timed run", "seconds"],
];

function chart(results: any[], target: number | null, label: string) {
  const el = h("div", { class: "card chart" });
  const pts = results.slice().sort((a, b) => a.date.localeCompare(b.date));
  if (!pts.length) return null;
  const css = getComputedStyle(document.documentElement);
  const color = (v: string) => css.getPropertyValue(v).trim() || "#888";
  const xs = pts.map((r) => Date.parse(`${r.date}T00:00`) / 1000);
  const data: number[][] = [xs, pts.map((r) => r.plot_value)];
  const series: uPlot.Series[] = [{}, { label, stroke: color("--accent"), width: 2, points: { show: true, size: 7 } }];
  if (target !== null) {
    data.push(xs.map(() => target));
    series.push({ label: "Target", stroke: color("--bad"), dash: [6, 4], width: 1.5, points: { show: false } });
  }
  const axis = { stroke: color("--muted"), grid: { stroke: color("--line"), width: 1 }, ticks: { stroke: color("--line") } };
  // Render after the element is in the DOM so we know its width.
  requestAnimationFrame(() => new uPlot({ width: Math.max(260, el.clientWidth - 24), height: 220, series, axes: [axis, axis], scales: { x: { time: true } } }, data as uPlot.AlignedData, el));
  return el;
}

async function testsScreen() {
  const [results, exercises] = await Promise.all([api.tests(ui.testKind), api.exercises()]);
  const kind = KINDS.find((k) => k[0] === ui.testKind)!;
  const needsExercise = ui.testKind === "OneRepMax" || ui.testKind === "MaxReps";
  const exName = (id: string) => exercises.find((e) => e.id === id)?.name ?? "";
  const targetKey = `target:${ui.me.user_id}:${ui.testKind}`;
  const targetRaw = storage.get(targetKey);
  const target = targetRaw === null || targetRaw === "" || isNaN(Number(targetRaw)) ? null : Number(targetRaw);
  const id = newId();
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const score = val(form, "score");
      const body: Record<string, unknown> = { kind: ui.testKind, date: val(form, "date"), exercise_id: needsExercise ? val(form, "exercise") || null : null };
      if (ui.testKind === "BeepTest") body.beep = score;
      else body.value = Number(score);
      run(() => api.putTest(id, body));
    },
  },
    select("kind", KINDS.map(([k, l]) => [k, l]), ui.testKind, { onchange: (e: Event) => { ui.testKind = (e.target as HTMLSelectElement).value; render(); } }),
    h("div", { class: "row" },
      h("input", { name: "date", type: "date", value: today(), required: true }),
      h("input", { name: "score", placeholder: kind[2], inputmode: "decimal", required: true })),
    needsExercise && select("exercise", [["", "(no exercise)"], ...exercises.map((e) => [e.id, e.name] as [string, string])]),
    h("button", { class: "primary" }, "Record"));
  const targetInput = h("input", {
    type: "number", step: "any", placeholder: ui.testKind === "BeepTest" ? "Target line, e.g. 10 = level 10" : `Target line (${kind[2]})`,
    value: targetRaw ?? "", onchange: (e: Event) => { storage.set(targetKey, (e.target as HTMLInputElement).value || null); render(); },
  });
  return h("div", {}, form,
    h("h2", {}, `${kind[1]} over time`),
    chart(results, target, kind[1]) ?? h("p", { class: "muted" }, "No results yet."),
    h("label", { class: "row" }, h("span", { class: "muted" }, "Target line"), targetInput),
    h("h2", {}, "Best first"),
    results.map((r, i) => h("div", { class: "card" },
      h("div", {}, `#${i + 1}  ${r.score}${r.beep ? "" : ` ${kind[2]}`}`),
      h("div", { class: "muted" }, [r.date, r.exercise_id && exName(r.exercise_id)].filter(Boolean).join(" · ")))));
}

render();
