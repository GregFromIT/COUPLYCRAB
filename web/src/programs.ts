// Programs: the builder (workouts, items, rep ranges, RIR, rest, periodization by week),
// gym profiles, and the one-off migration from v1 blocks.

import { api, newId } from "./api";
import { h, kg, localDate, optNum, select, today, val } from "./dom";
import { render, run, ui } from "./state";

const DAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const cap = (s: string) => s[0].toUpperCase() + s.slice(1);
const SLOTS: [string, string][] = [["", "Not pinned (rotation)"], ...DAYS.flatMap((d) => [[`${d}_am`, `${cap(d)} AM`], [`${d}_pm`, `${cap(d)} PM`]] as [string, string][])];
const slotLabel = (s: string | null) => SLOTS.find(([k]) => k === s)?.[1] ?? "";
const STYLES: [string, string][] = [["Straight", "Straight sets"], ["DropSet", "Drop set on the last set"], ["ToFailure", "Last set to failure"]];

type DItem = { id: string; exercise_id: string; sets: string; rep_min: string; rep_max: string; rir: string; style: string; rest_s: string };
type DWorkout = { id: string; name: string; slot: string; external: boolean; label: string; items: DItem[] };
type Draft = {
  id: string; isNew: boolean; name: string; start_date: string; weeks: string; gym: string; warmups: boolean;
  workouts: DWorkout[];
  /** Keyed "<week>:<item id>"; strings, empty = no override. */
  overrides: Record<string, Record<string, string>>;
  /** Whole-week presets being typed: week -> { rir, sets }. Not saved. */
  presets: Record<number, Record<string, string>>;
};

const nextMonday = (from = new Date()) => {
  const d = new Date(from);
  d.setDate(d.getDate() + ((8 - d.getDay()) % 7));
  return localDate(d);
};
const isMonday = (s: string) => /^\d{4}-\d{2}-\d{2}$/.test(s) && new Date(`${s}T00:00`).getDay() === 1;
const str = (x: unknown) => (x == null ? "" : String(x));

function newDraft(): Draft {
  return { id: newId(), isNew: true, name: "", start_date: nextMonday(), weeks: "4", gym: "", warmups: true, workouts: [], overrides: {}, presets: {} };
}

function fromProgram(p: any): Draft {
  const overrides: Draft["overrides"] = {};
  for (const o of p.overrides) overrides[`${o.week}:${o.item_id}`] = { sets: str(o.sets), rep_min: str(o.rep_min), rep_max: str(o.rep_max), rir: str(o.rir) };
  return {
    id: p.id, isNew: false, name: p.name, start_date: p.start_date, weeks: String(p.weeks), gym: p.gym_profile_id ?? "", warmups: p.warmups, overrides, presets: {},
    workouts: p.workouts.map((w: any) => ({
      id: w.id, name: w.name, slot: w.slot ?? "", external: !!w.external_label, label: w.external_label ?? "",
      items: w.items.map((i: any) => ({ id: i.id, exercise_id: i.exercise_id, sets: String(i.sets), rep_min: String(i.rep_min), rep_max: String(i.rep_max), rir: String(i.rir), style: i.style, rest_s: str(i.rest_s) })),
    })),
  };
}

function toBody(d: Draft) {
  const weeks = Number(d.weeks);
  const ids = new Set(d.workouts.flatMap((w) => (w.external ? [] : w.items.map((i) => i.id))));
  const overrides = Object.entries(d.overrides).flatMap(([key, o]) => {
    const [week, item_id] = key.split(":");
    const ov = { week: Number(week), item_id, sets: optNum(o.sets), rep_min: optNum(o.rep_min), rep_max: optNum(o.rep_max), rir: optNum(o.rir) };
    const empty = ov.sets == null && ov.rep_min == null && ov.rep_max == null && ov.rir == null;
    return empty || !ids.has(item_id) || ov.week > weeks ? [] : [ov];
  });
  return {
    name: d.name.trim(), start_date: d.start_date, weeks, gym_profile_id: d.gym || null, warmups: d.warmups,
    workouts: d.workouts.map((w) => ({
      id: w.id, name: w.name.trim() || w.label.trim(), slot: w.slot || null, external_label: w.external ? w.label.trim() || w.name.trim() : null,
      items: w.external ? [] : w.items.map((i) => ({
        id: i.id, exercise_id: i.exercise_id, sets: Number(i.sets || 1), rep_min: Number(i.rep_min || 1), rep_max: Number(i.rep_max || i.rep_min || 1),
        rir: Number(i.rir || 0), style: i.style || "Straight", rest_s: optNum(i.rest_s),
      })),
    })),
    overrides,
  };
}

const bind = (o: any, k: string) => ({ value: o[k] ?? "", oninput: (e: Event) => { o[k] = (e.target as HTMLInputElement).value; } });
const num = (o: any, k: string, label: string, extra: Record<string, unknown> = {}) =>
  h("input", { type: "number", min: 0, step: "any", inputmode: "decimal", placeholder: label, "aria-label": label, ...extra, ...bind(o, k) });
function move<T>(arr: T[], i: number, by: number) {
  const j = i + by;
  if (j < 0 || j >= arr.length) return;
  [arr[i], arr[j]] = [arr[j], arr[i]];
}
const reorder = (arr: any[], i: number) => [
  h("button", { type: "button", "aria-label": "Move up", onclick: () => { move(arr, i, -1); render(); } }, "↑"),
  h("button", { type: "button", "aria-label": "Move down", onclick: () => { move(arr, i, 1); render(); } }, "↓"),
];

// ---- builder ----

function builder(d: Draft, exercises: any[], gyms: any[]) {
  const exOptions: [string, string][] = exercises.map((e) => [e.id, e.name]);
  const weeks = Math.max(0, Math.min(104, Math.floor(Number(d.weeks) || 0)));
  const lifting = d.workouts.filter((w) => !w.external);

  const itemCard = (w: DWorkout, it: DItem, ii: number) => {
    const weeksGrid = Array.from({ length: weeks }, (_, k) => k + 1).map((wk) => {
      const o = (d.overrides[`${wk}:${it.id}`] ??= { sets: "", rep_min: "", rep_max: "", rir: "" });
      return h("div", { class: "weekrow" }, h("span", { class: "muted" }, `Wk ${wk}`),
        num(o, "sets", it.sets || "Sets"), num(o, "rep_min", it.rep_min || "Min"), num(o, "rep_max", it.rep_max || "Max"), num(o, "rir", it.rir || "RIR"));
    });
    return h("div", { class: "card inner" },
      h("div", { class: "row" }, select("", exOptions, it.exercise_id, { "aria-label": "Exercise", onchange: (e: Event) => { it.exercise_id = (e.target as HTMLSelectElement).value; } }),
        reorder(w.items, ii), h("button", { type: "button", "aria-label": "Remove exercise", onclick: () => { w.items.splice(ii, 1); render(); } }, "✕")),
      h("div", { class: "row" }, num(it, "sets", "Sets", { min: 1 }), num(it, "rep_min", "Reps min", { min: 1 }), num(it, "rep_max", "Reps max", { min: 1 }), num(it, "rir", "RIR", { max: 5 })),
      h("div", { class: "row" }, select("", STYLES, it.style, { "aria-label": "Set style", onchange: (e: Event) => { it.style = (e.target as HTMLSelectElement).value; } }), num(it, "rest_s", "Rest s")),
      h("details", {}, h("summary", { class: "muted" }, "Periodization for this exercise (blank = same as above)"), weeksGrid));
  };

  const workoutCard = (w: DWorkout, wi: number) => h("div", { class: "card" },
    h("div", { class: "row" }, h("input", { placeholder: "Workout name, e.g. Upper A", maxlength: 200, "aria-label": "Workout name", ...bind(w, "name") }),
      reorder(d.workouts, wi), h("button", { type: "button", "aria-label": "Remove workout", onclick: () => { if (confirm(`Remove "${w.name || "this workout"}"?`)) { d.workouts.splice(wi, 1); render(); } } }, "✕")),
    h("div", { class: "row" }, select("", SLOTS, w.slot, { "aria-label": "Day", onchange: (e: Event) => { w.slot = (e.target as HTMLSelectElement).value; render(); } }),
      h("label", { class: "check" }, h("input", { type: "checkbox", checked: w.external, onchange: (e: Event) => { w.external = (e.target as HTMLInputElement).checked; render(); } }), "Outside session (e.g. PT)")),
    w.external
      ? [h("input", { placeholder: "Label, e.g. Unit PT", maxlength: 200, "aria-label": "Label", ...bind(w, "label") }), !w.slot && h("div", { class: "bad" }, "Pick a day for an outside session.")]
      : [w.items.map((it, ii) => itemCard(w, it, ii)),
        h("button", { type: "button", disabled: !exOptions.length, onclick: () => { w.items.push({ id: newId(), exercise_id: exOptions[0]?.[0] ?? "", sets: "3", rep_min: "8", rep_max: "12", rir: "2", style: "Straight", rest_s: "" }); render(); } },
          exOptions.length ? "+ Exercise" : "Add exercises first (Exercises tab)")]);

  const preset = (wk: number) => {
    const p = (d.presets[wk] ??= { rir: "", sets: "" });
    const apply = () => {
      for (const w of lifting) for (const it of w.items) {
        const o = (d.overrides[`${wk}:${it.id}`] ??= { sets: "", rep_min: "", rep_max: "", rir: "" });
        if (p.rir !== "") o.rir = p.rir;
        if (p.sets !== "") o.sets = String(Math.max(1, Number(it.sets || 1) + Number(p.sets)));
      }
      render();
    };
    const clear = () => { for (const k of Object.keys(d.overrides)) if (k.startsWith(`${wk}:`)) delete d.overrides[k]; render(); };
    return h("div", { class: "weekrow" }, h("span", { class: "muted" }, `Wk ${wk}`), num(p, "rir", "RIR", { max: 5 }),
      h("input", { type: "number", step: 1, inputmode: "numeric", placeholder: "Sets ±", "aria-label": "Sets change", ...bind(p, "sets") }),
      h("button", { type: "button", onclick: apply }, "Apply"), h("button", { type: "button", onclick: clear }, "Clear"));
  };

  const save = () => {
    const body = toBody(d);
    if (!body.name) return alert("Give the program a name.");
    if (!body.workouts.length) return alert("Add at least one workout.");
    if (body.workouts.some((w) => w.external_label && !w.slot)) return alert("Pick a day for each outside session.");
    run(async () => {
      if (d.isNew) await api.putProgram(d.id, body);
      else await api.patchProgram(d.id, body);
      ui.draftProgram = null;
    });
  };
  return h("div", {},
    h("h2", {}, d.isNew ? "New program" : "Edit program"),
    h("div", { class: "card" },
      h("input", { placeholder: "Program name", maxlength: 200, "aria-label": "Program name", ...bind(d, "name") }),
      h("div", { class: "row" },
        h("input", { type: "date", "aria-label": "Start (a Monday)", ...bind(d, "start_date") }),
        num(d, "weeks", "Weeks", { min: 1, max: 104 })),
      !isMonday(d.start_date) && h("div", { class: "row" }, h("span", { class: "bad grow" }, "Programs start on a Monday."),
        h("button", { type: "button", onclick: () => { d.start_date = nextMonday(new Date(`${d.start_date || today()}T00:00`)); render(); } }, "Use next Monday")),
      h("div", { class: "row" },
        select("", [["", "Default gym"], ...gyms.map((g) => [g.id, g.name] as [string, string])], d.gym, { "aria-label": "Gym", onchange: (e: Event) => { d.gym = (e.target as HTMLSelectElement).value; } }),
        h("label", { class: "check" }, h("input", { type: "checkbox", checked: d.warmups, onchange: (e: Event) => { d.warmups = (e.target as HTMLInputElement).checked; } }), "Suggest warm-ups"))),
    d.workouts.map(workoutCard),
    h("button", { type: "button", onclick: () => { d.workouts.push({ id: newId(), name: "", slot: "", external: false, label: "", items: [] }); render(); } }, "+ Workout"),
    weeks > 1 && lifting.some((w) => w.items.length) && h("details", { class: "card" },
      h("summary", {}, "Periodization by week"),
      h("p", { class: "muted" }, "Set RIR and/or a sets change for a whole week, e.g. week 4 as a deload: RIR 4, sets −1. Fine-tune per exercise in each exercise's own section."),
      Array.from({ length: weeks - 1 }, (_, k) => preset(k + 2))),
    h("div", { class: "row" },
      h("button", { class: "primary", onclick: save }, "Save program"),
      h("button", { type: "button", onclick: () => { ui.draftProgram = null; render(); } }, "Cancel")));
}

// ---- gym profiles ----

function gymSection(gyms: any[]) {
  const editing = gyms.find((g) => g.id === ui.editingGym);
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const plates = val(form, "plates").split(/[\s,]+/).filter(Boolean).map(Number);
      if (plates.some((p) => !(p > 0))) return alert("Plates must be positive numbers, e.g. 1.25, 2.5, 5, 10, 20");
      const body = {
        name: val(form, "name"), bar_kg: Number(val(form, "bar")), plates_kg: plates,
        dumbbell_step_kg: Number(val(form, "db")), machine_step_kg: Number(val(form, "machine")),
        is_default: (form.querySelector('[name="default"]') as HTMLInputElement).checked,
      };
      const id = editing?.id ?? newId();
      run(async () => {
        if (editing) await api.patchGym(id, body);
        else await api.putGym(id, body);
        ui.editingGym = null;
      });
    },
  },
    h("input", { name: "name", placeholder: "Gym name, e.g. Base gym", required: true, maxlength: 200, value: editing?.name ?? "" }),
    h("div", { class: "row" },
      h("input", { name: "bar", type: "number", min: 0.5, step: "any", required: true, placeholder: "Bar kg", "aria-label": "Bar kg", value: editing?.bar_kg ?? 20 }),
      h("input", { name: "db", type: "number", min: 0.25, step: "any", required: true, placeholder: "Dumbbell step kg", "aria-label": "Dumbbell step kg", value: editing?.dumbbell_step_kg ?? 2 }),
      h("input", { name: "machine", type: "number", min: 0.25, step: "any", required: true, placeholder: "Machine step kg", "aria-label": "Machine step kg", value: editing?.machine_step_kg ?? 5 })),
    h("input", { name: "plates", placeholder: "Plates per side, kg: 1.25, 2.5, 5, 10, 20", required: true, "aria-label": "Plates", value: (editing?.plates_kg ?? [1.25, 2.5, 5, 10, 20]).join(", ") }),
    h("div", { class: "row" },
      h("label", { class: "check" }, h("input", { name: "default", type: "checkbox", checked: editing ? editing.is_default : gyms.length === 0 }), "Default gym"),
      h("button", { class: "primary" }, editing ? "Save gym" : "Add gym"),
      editing && h("button", { type: "button", onclick: () => { ui.editingGym = null; render(); } }, "Cancel")));
  return h("div", {}, h("h2", {}, "Gyms"),
    h("p", { class: "muted" }, "Plates and increments decide which loads the recommendations can use."),
    form,
    gyms.map((g) => h("div", { class: "card" },
      h("div", { class: "row" },
        h("div", { class: "grow" }, h("strong", {}, g.name), g.is_default && h("span", { class: "ok" }, " · default"),
          h("div", { class: "muted" }, `Bar ${kg(g.bar_kg)} kg · plates ${g.plates_kg.map(kg).join(", ")} · dumbbells ${kg(g.dumbbell_step_kg)} kg steps · machines ${kg(g.machine_step_kg)} kg steps`)),
        h("button", { onclick: () => { ui.editingGym = g.id; render(); } }, "Edit"),
        h("button", { onclick: () => confirm(`Delete gym "${g.name}"?`) && run(() => api.patchGym(g.id, { deleted: true })) }, "Delete")))));
}

// ---- screen ----

let migration: { created: number; notes: string[] } | null = null;

export async function programsScreen() {
  const [programs, gyms, exercises] = await Promise.all([api.programs(), api.gymProfiles(), api.exercises()]);
  if (ui.draftProgram) return builder(ui.draftProgram as Draft, exercises, gyms);

  let banner: HTMLElement | null = null;
  if (migration) {
    const m = migration;
    banner = h("div", { class: "card", id: "migrated" }, h("strong", {}, `Migrated ${m.created} v1 block${m.created === 1 ? "" : "s"} into programs`),
      m.notes.length > 0 && h("div", { class: "muted" }, "Not carried over:"), m.notes.map((n) => h("div", { class: "muted" }, `• ${n}`)),
      h("button", { onclick: () => { migration = null; render(); } }, "OK"));
  } else if (!programs.length) {
    const blocks = await api.v1Blocks().catch(() => []);
    if (blocks.length) {
      banner = h("div", { class: "card" }, h("strong", {}, `You have ${blocks.length} v1 training block${blocks.length === 1 ? "" : "s"}`),
        h("div", { class: "muted" }, "Programs replace templates, weekly plans and blocks. Migrating turns each block into a program; your old data stays in place."),
        h("button", { class: "primary", onclick: () => run(async () => { migration = await api.migrateV1(); }) }, "Migrate to programs"));
    }
  }
  const now = today();
  const end = (p: any) => { const d = new Date(`${p.start_date}T00:00`); d.setDate(d.getDate() + p.weeks * 7 - 1); return localDate(d); };
  return h("div", {}, banner,
    h("button", { class: "primary", style: "width:100%", onclick: () => { ui.draftProgram = newDraft(); render(); } }, "New program"),
    programs.map((p) => h("div", { class: "card" },
      h("div", {}, h("strong", {}, p.name), p.start_date <= now && now <= end(p) ? h("span", { class: "ok" }, " · current") : null),
      h("div", { class: "muted" }, `${p.start_date} → ${end(p)} · ${p.weeks} weeks`),
      p.workouts.map((w: any) => h("div", { class: "muted" }, `${w.name}${w.slot ? ` · ${slotLabel(w.slot)}` : ""}${w.external_label ? " · outside session" : ` · ${w.items.length} exercise${w.items.length === 1 ? "" : "s"}`}`)),
      h("div", { class: "row" },
        h("button", { onclick: () => { ui.draftProgram = fromProgram(p); render(); } }, "Edit"),
        h("button", { onclick: () => confirm(`Delete program "${p.name}"? Logged sessions are kept.`) && run(() => api.patchProgram(p.id, { deleted: true })) }, "Delete")))),
    !programs.length && !banner && h("p", { class: "muted" }, "No programs yet. Add your exercises under Exercises, then build one here."),
    gymSection(gyms));
}
