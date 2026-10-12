// Today and Log Workout: the screens used in the gym. Recommendations come from the
// server (derived from your logged history); this file only shows and logs.

import { api, newId } from "./api";
import { h, kg, localDate, mmss, optNum, select, stepper, storage } from "./dom";
import { render, run, ui, wake } from "./state";

// ---- helpers ----

const DEFAULT_GYM = { bar_kg: 20, plates_kg: [1.25, 2.5, 5, 10, 15, 20, 25], dumbbell_step_kg: 2, machine_step_kg: 5 };
export const gymFor = (program: any, gyms: any[]) => gyms.find((g) => g.id === program?.gym_profile_id) ?? gyms[0] ?? DEFAULT_GYM;

const STYLE: Record<string, string> = { Straight: "", DropSet: "drop set on the last set", ToFailure: "last set to failure" };
const KINDS = ["Working", "Warmup", "Drop", "Failure"];
const counts = (s: any) => !s.kind || s.kind === "Working" || s.kind === "Failure";

export function prescriptionText(p: any) {
  const reps = p.rep_min === p.rep_max ? `${p.rep_min}` : `${p.rep_min}–${p.rep_max}`;
  return [`${p.sets} × ${reps} @ ${p.rir} RIR`, p.rest_s != null && `rest ${p.rest_s}s`, STYLE[p.style]].filter(Boolean).join(" · ");
}

/** "→ 82.5 kg × 8", with the reason one tap away. */
function recommendationEl(rec: any) {
  return h("div", {},
    h("div", { class: "rec" }, `→ ${kg(rec.load_kg)} kg × ${rec.reps} @ ${rec.rir} RIR`),
    h("details", {}, h("summary", { class: "muted" }, "Why? ▸"), h("div", { class: "muted" }, rec.reason)));
}

/** Plates for one side of the bar, exact (fewest plates). Closest load below if the target can't be made. */
export function platesFor(load: number, gym: { bar_kg: number; plates_kg: number[] }) {
  const perSide = (load - gym.bar_kg) / 2;
  if (!(perSide > 0)) return { plates: [] as number[], made: gym.bar_kg, exact: load === gym.bar_kg };
  const units = gym.plates_kg.map((p) => Math.round(p * 100)).filter((p) => p > 0);
  const target = Math.min(Math.floor(perSide * 100 + 1e-6), 100000);
  const best = new Array<number>(target + 1).fill(Infinity);
  const last = new Array<number>(target + 1).fill(0);
  best[0] = 0;
  for (let x = 1; x <= target; x++) for (const p of units) if (x >= p && best[x - p] + 1 < best[x]) { best[x] = best[x - p] + 1; last[x] = p; }
  let x = target;
  while (x > 0 && best[x] === Infinity) x--;
  const plates: number[] = [];
  for (let y = x; y > 0; y -= last[y]) plates.push(last[y] / 100);
  return { plates: plates.sort((a, b) => b - a), made: gym.bar_kg + (2 * x) / 100, exact: x === target };
}

function platesText(load: number, gym: any) {
  if (!(load > 0)) return "Enter a load.";
  const r = platesFor(load, gym);
  if (!r.plates.length) return load < gym.bar_kg ? `Below the ${kg(gym.bar_kg)} kg bar.` : `Just the ${kg(gym.bar_kg)} kg bar.`;
  const side = r.plates.map(kg).join(" + ");
  return `Bar ${kg(gym.bar_kg)} kg + per side: ${side}${r.exact ? "" : ` (closest below ${kg(load)} kg is ${kg(r.made)} kg)`}`;
}

// ---- rest timer ----
// One fixed bar outside #app, so a re-render doesn't recreate it (and eat a tap).

const rest = { endsAt: 0, label: "", timer: 0, buzzed: false, bar: null as null | { root: HTMLElement; text: HTMLElement } };

function restBar() {
  if (rest.bar) return rest.bar;
  const text = h("span", { class: "grow" });
  const root = h("div", { id: "rest", role: "timer" }, text,
    h("button", { onclick: () => { rest.endsAt = Math.max(rest.endsAt, Date.now()) + 30000; rest.buzzed = false; tickRest(); } }, "+30 s"),
    h("button", { onclick: stopRest }, "Skip"));
  document.body.append(root);
  return (rest.bar = { root, text });
}

export function startRest(seconds: number, label: string) {
  const bar = restBar();
  rest.endsAt = Date.now() + seconds * 1000;
  rest.label = label;
  rest.buzzed = false;
  bar.root.hidden = false;
  clearInterval(rest.timer);
  rest.timer = window.setInterval(tickRest, 250);
  tickRest();
}

function tickRest() {
  const bar = rest.bar!;
  const left = Math.ceil((rest.endsAt - Date.now()) / 1000);
  if (left > 0) {
    bar.text.textContent = `Rest ${mmss(left)} · ${rest.label}`;
    bar.root.classList.remove("over");
  } else {
    bar.text.textContent = `Rest over · ${rest.label}`;
    bar.root.classList.add("over");
    if (!rest.buzzed) { rest.buzzed = true; navigator.vibrate?.([200, 100, 200]); }
  }
}

export function stopRest() {
  clearInterval(rest.timer);
  if (rest.bar) rest.bar.root.hidden = true;
}

// ---- Today ----

export async function todayScreen() {
  const n = await api.nextWorkout();
  if (!n.program_id) {
    return h("div", {},
      h("div", { class: "card" }, h("strong", {}, "No program is running today"),
        h("div", { class: "muted" }, `${n.date}. Build one under Programs${"."}`)),
      h("button", { class: "primary", onclick: () => { ui.fit = "programs"; render(); } }, "Programs"),
      h("button", { onclick: () => { ui.fit = "log"; render(); } }, "Log a session that isn't planned"));
  }
  const head = h("div", { class: "card" }, h("strong", {}, n.program_name),
    h("div", { class: "muted" }, `Week ${n.week} of ${n.weeks} · ${n.date}`), h("div", { class: "muted" }, n.reason));
  const w = n.workout;
  let workoutEl: HTMLElement | null = null;
  if (w) {
    const open = n.todays_sessions.find((s: any) => s.status === "InProgress" && s.workout_id === w.workout_id);
    const id = newId();
    const action = open
      ? h("button", { class: "primary", onclick: () => { ui.fit = "log"; render(); } }, "Continue")
      : h("button", { class: "primary", onclick: () => run(async () => { await api.putSession(id, { program_id: n.program_id, workout_id: w.workout_id }); ui.fit = "log"; }) }, "Start");
    workoutEl = h("div", { class: "card" },
      h("div", {}, h("strong", {}, w.name), w.slot && h("span", { class: "muted" }, ` · ${w.slot.replace("_", " ").toUpperCase()}`)),
      w.items.map((it: any) => h("div", { class: "item" },
        h("div", {}, h("strong", {}, it.exercise_name ?? "(exercise)"), h("span", { class: "muted" }, ` · ${prescriptionText(it.prescription)}`)),
        it.recommendation ? recommendationEl(it.recommendation) : h("div", { class: "muted" }, "No history yet: pick a load."),
        it.warmups.length > 0 && h("div", { class: "muted" }, `Warm-up: ${it.warmups.map((x: any) => `${kg(x.load_kg)}×${x.reps}`).join(", ")}`))),
      action);
  }
  const externals = n.externals.map((x: any) => {
    if (x.done) return h("div", { class: "card" }, h("strong", {}, `${x.slot.replace("_", " ").toUpperCase()} · ${x.label}`), h("div", { class: "ok" }, "Attended ✓"));
    const note = h("textarea", { placeholder: "Note (optional)", maxlength: 2000 }) as HTMLTextAreaElement;
    const id = newId();
    return h("div", { class: "card" }, h("strong", {}, `${x.slot.replace("_", " ").toUpperCase()} · ${x.label}`), note,
      h("button", { class: "primary", onclick: () => {
        const text = note.value.trim() || null;
        run(async () => { await api.putSession(id, { program_id: n.program_id, workout_id: x.workout_id, label: x.label }); await api.finish(id, "Completed", text); });
      } }, "Mark attended"));
  });
  const doneToday = n.todays_sessions.filter((s: any) => s.status !== "InProgress" && s.workout_id);
  return h("div", {}, head, workoutEl, externals,
    !w && h("div", { class: "card" }, h("div", { class: doneToday.length ? "ok" : "muted" }, doneToday.length ? "Done for today ✓" : "Nothing planned today.")),
    h("button", { onclick: () => { ui.fit = "log"; render(); } }, "Log a different workout"));
}

// ---- Log Workout ----

const FIELDS: Record<string, [string, string][]> = {
  WeightReps: [["load", "kg"], ["reps", "Reps"]],
  Reps: [["reps", "Reps"]],
  Timed: [["duration_s", "Seconds"]],
  Distance: [["distance_m", "Metres"]],
};

export async function logScreen() {
  const [sessions, exercises, programs, gyms] = await Promise.all([api.sessions(), api.exercises(), api.programs(), api.gymProfiles()]);
  const mine = sessions.find((s) => s.status === "InProgress");
  if (!mine) return startScreen(sessions, programs);

  wake.want = true;
  const program = programs.find((p) => p.id === mine.program_id);
  const gym = gymFor(program, gyms);
  const plan = program && mine.workout_id ? await api.recommendations(program.id, mine.workout_id, localDate(new Date(mine.started_at))) : null;
  const workoutName = plan?.name ?? mine.label ?? "Blank session";
  const sets = (Object.entries(mine.sets) as [string, any][]).sort((a, b) => a[1].index - b[1].index).map(([, s]) => s);
  const exOptions: [string, string][] = exercises.map((e) => [e.id, e.name]);
  const exName = (id: string) => exercises.find((e) => e.id === id)?.name ?? "(unknown exercise)";
  const prevSets = sessions.filter((s) => s.id !== mine.id && s.status !== "InProgress");
  const previousFor = (exId: string) => {
    for (const s of prevSets) {
      const w = (Object.values(s.sets) as any[]).filter((x) => x.exercise_id === exId && counts(x) && x.load != null && x.reps != null).sort((a, b) => b.index - a.index);
      if (w.length) return w[0];
    }
    return null;
  };

  const blocks: { item?: any }[] = plan?.items.length ? plan.items.map((item: any) => ({ item })) : [{}];
  const card = (b: { item?: any }) => {
    const item = b.item;
    const key = `${mine.id}:${item?.item_id ?? "blank"}`;
    const values = (ui.entry[key] ??= {});
    if (!item) values.exercise_id ??= exOptions[0]?.[0] ?? "";
    const exId: string = item?.exercise_id ?? values.exercise_id;
    const ex = exercises.find((e) => e.id === exId);
    const fields = FIELDS[ex?.measurement] ?? FIELDS.Reps;
    const withRir = fields.some(([k]) => k === "reps");
    const rx = item?.prescription;
    const rec = item?.recommendation;
    if (rec) ui.shown[exId] = { load_kg: rec.load_kg, reps: rec.reps };
    const done = sets.filter((s) => s.exercise_id === exId);
    const workingDone = done.filter(counts).length;

    const prev = previousFor(exId);
    const smart = rec ? { load: rec.load_kg, reps: rec.reps, rir: rec.rir } : null;
    const previous = prev ? { load: prev.load, reps: prev.reps, rir: prev.rir ?? rx?.rir } : null;
    const pick: any = ui.prefill === "smart" ? smart ?? previous : previous ?? smart;
    values.load ??= String(pick?.load ?? "");
    values.reps ??= String(pick?.reps ?? (rx ? Math.round((rx.rep_min + rx.rep_max) / 2) : ""));
    values.rir ??= String(pick?.rir ?? rx?.rir ?? 2);
    values.duration_s ??= ""; values.distance_m ??= ""; values.partials ??= "1";
    if (!values.kindTouched) values.kind = rx?.style === "ToFailure" && workingDone === rx.sets - 1 ? "Failure" : "Working";

    const loadStep = ex?.equipment === "Barbell" ? 2 * Math.min(...(gym.plates_kg.length ? gym.plates_kg : [1.25])) : ex?.equipment === "Dumbbell" ? gym.dumbbell_step_kg : gym.machine_step_kg;
    const platesEl = h("div", { class: "muted plates", hidden: values.platesOpen !== "1" });
    const refreshPlates = () => { platesEl.textContent = platesText(Number(values.load), gym); };
    refreshPlates();

    const send = (set: Record<string, unknown>, pendKey: string, startTimer: boolean) => {
      // Same values as a failed attempt -> the same set ID (and body), so a retry can't duplicate.
      const idKey = JSON.stringify(set);
      const pending = ui.pendingSet[pendKey];
      let id: string, body: Record<string, unknown>;
      if (pending && pending.key === idKey) ({ id, set: body } = pending);
      else {
        const now = Date.now();
        body = ui.lastSetAt && now - ui.lastSetAt < 3_600_000 ? { ...set, rest_taken_s: Math.round((now - ui.lastSetAt) / 1000) } : set;
        id = newId();
        ui.pendingSet[pendKey] = { id, key: idKey, set: body };
      }
      run(async () => {
        await api.putSet(mine.id, id, body);
        delete ui.pendingSet[pendKey];
        delete values.kindTouched;
        ui.lastSetAt = Date.now();
        if (startTimer) startRest(rx?.rest_s ?? ex?.default_rest_s ?? 90, exName(exId));
      });
    };
    const logSet = () => {
      const set: Record<string, unknown> = { exercise_id: exId };
      for (const [k] of fields) set[k] = optNum(values[k]);
      if (withRir) {
        set.rir = optNum(values.rir);
        set.kind = values.kind || "Working";
        if (values.partialsOn === "1") set.partials = optNum(values.partials);
      }
      if (rec) set.recommendation_id = rec.id;
      send(set, key, withRir && set.kind !== "Warmup" && set.kind !== "Drop");
    };
    const logWarmup = (w: any, i: number) => send({ exercise_id: exId, load: w.load_kg, reps: w.reps, kind: "Warmup", ...(rec ? { recommendation_id: rec.id } : {}) }, `${key}:wu${i}`, false);
    const dropSet = () => {
      const lastLoad = Number(done.filter(counts).slice(-1)[0]?.load ?? values.load);
      values.load = String(Math.max(loadStep, Math.round((lastLoad * 0.8) / loadStep) * loadStep));
      values.kind = "Drop"; values.kindTouched = "1";
      render();
    };

    const setLine = (s: any, i: number) => {
      const bits = fields.map(([k, l]) => `${s[k] ?? "–"} ${l}`);
      if (s.rir != null) bits.push(`${s.rir} RIR`);
      if (s.kind && s.kind !== "Working") bits.push(s.kind === "Warmup" ? "warm-up" : s.kind.toLowerCase());
      if (s.partials) bits.push(`+${s.partials} partials`);
      return h("div", { class: "muted" }, `Set ${i + 1}: ${bits.join(" · ")}`);
    };
    const warm = (item?.warmups ?? []) as any[];
    return h("div", { class: "card" },
      item
        ? h("div", {}, h("strong", {}, exName(exId)), h("div", { class: "muted" }, `${workingDone}/${rx.sets} sets · ${prescriptionText(rx)}`))
        : select("", exOptions, exId, { onchange: (e: Event) => { ui.entry[key] = { exercise_id: (e.target as HTMLSelectElement).value }; render(); } }),
      rec && recommendationEl(rec),
      !rec && item && h("div", { class: "muted" }, "No history yet: pick a load."),
      warm.length > 0 && h("div", { class: "row" }, h("span", { class: "muted" }, "Warm-up"),
        warm.map((w, i) => {
          const logged = done.some((s) => s.kind === "Warmup" && s.load === w.load_kg);
          return h("button", { type: "button", class: logged ? "chip ok" : "chip", "aria-label": `Log warm-up ${kg(w.load_kg)} kg × ${w.reps}`, onclick: () => logWarmup(w, i) }, `${logged ? "✓ " : ""}${kg(w.load_kg)}×${w.reps}`);
        })),
      done.map(setLine),
      fields.map(([k, l]) => stepper(values, k, l, k === "load" ? loadStep : k === "reps" ? 1 : k === "duration_s" ? 5 : 100, Infinity, k === "load" ? refreshPlates : undefined)),
      withRir && stepper(values, "rir", "RIR", 1, 10),
      withRir && h("div", { class: "row" },
        select("", KINDS.map((k) => [k, k === "Warmup" ? "Warm-up" : k]), values.kind, { "aria-label": "Set type", onchange: (e: Event) => { values.kind = (e.target as HTMLSelectElement).value; values.kindTouched = "1"; } }),
        h("button", { type: "button", onclick: dropSet }, "Drop set"),
        ex?.equipment === "Barbell" && h("button", { type: "button", "aria-label": "Plate calculator", onclick: () => { values.platesOpen = values.platesOpen === "1" ? "0" : "1"; platesEl.hidden = values.platesOpen !== "1"; } }, "Plates")),
      platesEl,
      withRir && h("label", { class: "check" },
        h("input", { type: "checkbox", checked: values.partialsOn === "1", onchange: (e: Event) => { values.partialsOn = (e.target as HTMLInputElement).checked ? "1" : "0"; render(); } }), "Partials"),
      withRir && values.partialsOn === "1" && stepper(values, "partials", "Partials", 1, 50),
      h("button", { class: "primary", disabled: !exId, onclick: logSet }, "Log set"));
  };

  const note = h("textarea", { placeholder: "Note (optional)", maxlength: 2000, oninput: (e: Event) => { ui.finishNote = (e.target as HTMLTextAreaElement).value; } }, ui.finishNote) as HTMLTextAreaElement;
  const finish = (status: string) => () => {
    const text = ui.finishNote.trim() || null;
    run(async () => {
      await api.finish(mine.id, status, text);
      ui.finishNote = "";
      for (const k of Object.keys(ui.entry)) if (k.startsWith(`${mine.id}:`)) delete ui.entry[k];
      ui.lastFinished = program && mine.workout_id
        ? { sessionId: mine.id, programId: program.id, workoutId: mine.workout_id, date: localDate(new Date(mine.started_at)), name: workoutName }
        : null;
      ui.lastSetAt = 0;
      stopRest();
    });
  };
  return h("div", {},
    h("div", { class: "row" }, h("h2", { class: "grow" }, workoutName), h("span", { id: "awake", class: "muted" })),
    h("div", { class: "row" }, h("span", { class: "muted grow" }, `Started ${new Date(mine.started_at).toLocaleTimeString()}${plan?.week ? ` · week ${plan.week}` : ""}`),
      h("label", { class: "muted" }, "Pre-fill ", select("prefill", [["smart", "recommended"], ["previous", "last time"]], ui.prefill, {
        onchange: (e: Event) => {
          ui.prefill = (e.target as HTMLSelectElement).value as "smart" | "previous";
          storage.set("prefill", ui.prefill);
          for (const k of Object.keys(ui.entry)) if (k.startsWith(`${mine.id}:`)) delete ui.entry[k];
          render();
        },
      }))),
    blocks.map(card),
    h("div", { class: "card" }, note,
      h("div", { class: "row" },
        h("button", { class: "primary", onclick: finish("Completed") }, "Finish: Completed"),
        h("button", { onclick: finish("Partial") }, "Finish: Partial"))));
}

async function startScreen(sessions: any[], programs: any[]) {
  const options: [string, string][] = [["", "Blank session"]];
  for (const p of programs) for (const w of p.workouts) if (!w.external_label) options.push([`${p.id}|${w.id}`, `${p.name} · ${w.name}`]);
  const id = newId();
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const [program_id, workout_id] = ((form.querySelector("select") as HTMLSelectElement).value || "|").split("|");
      run(() => api.putSession(id, program_id ? { program_id, workout_id } : {}));
    },
  }, select("workout", options), h("button", { class: "primary" }, "Start session"));

  const lf = ui.lastFinished;
  let nextTime: HTMLElement | null = null;
  if (lf) {
    try {
      const plan = await api.recommendations(lf.programId, lf.workoutId, lf.date);
      const rows = plan.items.filter((it: any) => it.recommendation).map((it: any) => {
        const r = it.recommendation, before = ui.shown[it.exercise_id];
        const was = before ? `${kg(before.load_kg)} kg × ${before.reps} → ` : "";
        return h("div", { class: "item" }, h("div", {}, h("strong", {}, it.exercise_name ?? "(exercise)"), h("span", { class: "rec" }, ` ${was}${kg(r.load_kg)} kg × ${r.reps}`)),
          h("details", {}, h("summary", { class: "muted" }, "Why? ▸"), h("div", { class: "muted" }, r.reason)));
      });
      nextTime = h("div", { class: "card", id: "next-time" }, h("strong", {}, `Finished ✓ Next time · ${lf.name}`), rows,
        h("button", { onclick: () => { ui.lastFinished = null; render(); } }, "Dismiss"));
    } catch { /* the summary is optional */ }
  }
  const label = (s: any) => s.label ?? programs.flatMap((p) => p.workouts.filter((w: any) => w.id === s.workout_id && p.id === s.program_id).map((w: any) => w.name))[0] ?? "Blank";
  return h("div", {}, nextTime, form, h("h2", {}, "Recent sessions"), sessions.slice(0, 20).map((s) => h("div", { class: "card" },
    h("div", {}, `${new Date(s.started_at).toLocaleString()} · ${label(s)} · ${s.status}`),
    h("div", { class: "muted" }, `${Object.keys(s.sets).length} sets${s.note ? ` · ${s.note}` : ""}`))));
}
