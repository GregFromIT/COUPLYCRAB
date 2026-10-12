// Progress: weekly volume per muscle, estimated 1RM over time, PRs, and benchmark tests.
// Everything shown here is derived on the server from the session log.

import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { api, newId } from "./api";
import { h, kg, select, storage, today, val } from "./dom";
import { render, run, ui, type Progress } from "./state";

const MUSCLE_ORDER = ["chest", "back", "shoulders", "biceps", "triceps", "quads", "hamstrings", "glutes", "calves", "abs", "forearms"];

function lineChart(xs: number[], ys: number[], label: string, target: number | null = null) {
  if (!xs.length) return null;
  const el = h("div", { class: "card chart" });
  const css = getComputedStyle(document.documentElement);
  const color = (v: string) => css.getPropertyValue(v).trim() || "#888";
  const data: number[][] = [xs, ys];
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

const secs = (date: string) => Date.parse(`${date}T00:00`) / 1000;

// ---- volume ----

async function volumeSection() {
  const { weeks } = await api.volume(8);
  const cur = weeks[weeks.length - 1];
  const total = (m: string) => weeks.reduce((s: number, w: any) => s + w.sets[m], 0);
  if (!ui.volumeMuscle) ui.volumeMuscle = [...MUSCLE_ORDER].sort((a, b) => total(b) - total(a))[0];
  const max = Math.max(10, ...MUSCLE_ORDER.map((m) => cur.sets[m]));
  const rows = MUSCLE_ORDER.map((m) => h("button", {
    class: `barrow${m === ui.volumeMuscle ? " sel" : ""}`, "aria-pressed": m === ui.volumeMuscle, onclick: () => { ui.volumeMuscle = m; render(); },
  }, h("span", {}, m), h("span", { class: "bartrack" }, h("span", { class: "bar", style: `width:${(cur.sets[m] / max) * 100}%` })), h("span", { class: "barval" }, kg(cur.sets[m]))));
  return h("div", {},
    h("p", { class: "muted" }, `Sets this week (from ${cur.week_start}). A primary muscle counts 1 per set, a secondary 0.5. Tap a muscle for its last 8 weeks.`),
    h("div", { class: "card bars" }, rows),
    h("h2", {}, `${ui.volumeMuscle}: sets per week`),
    lineChart(weeks.map((w: any) => secs(w.week_start)), weeks.map((w: any) => w.sets[ui.volumeMuscle]), "Sets") ?? h("p", { class: "muted" }, "No data yet."));
}

// ---- strength ----

async function strengthSection() {
  const exercises = (await api.exercises()).filter((e) => e.measurement === "WeightReps");
  if (!exercises.length) return h("p", { class: "muted" }, "Add a weighted exercise under Exercises, then log some sets.");
  if (!exercises.some((e) => e.id === ui.strengthExercise)) ui.strengthExercise = exercises[0].id;
  const { points } = await api.e1rm(ui.strengthExercise);
  return h("div", {},
    select("exercise", exercises.map((e) => [e.id, e.name]), ui.strengthExercise, { "aria-label": "Exercise", onchange: (e: Event) => { ui.strengthExercise = (e.target as HTMLSelectElement).value; render(); } }),
    h("p", { class: "muted" }, "Estimated one-rep max (Epley) from your best working set each session."),
    lineChart(points.map((p: any) => p.started_at / 1000), points.map((p: any) => Math.round(p.e1rm * 10) / 10), "e1RM kg") ?? h("p", { class: "muted" }, "No finished sessions with this exercise yet."),
    points.slice().reverse().slice(0, 10).map((p: any) => h("div", { class: "card" },
      h("div", {}, `${new Date(p.started_at).toLocaleDateString()} · ${kg(p.load_kg)} kg × ${p.reps} @ ${p.rir} RIR`),
      h("div", { class: "muted" }, `e1RM ${kg(Math.round(p.e1rm * 10) / 10)} kg`))));
}

// ---- PRs ----

async function prsSection() {
  const [prs, exercises] = await Promise.all([api.prs(), api.exercises()]);
  if (!prs.length) return h("p", { class: "muted" }, "No PRs yet: they appear once a session is finished.");
  const name = (id: string) => exercises.find((e) => e.id === id)?.name ?? "(unknown exercise)";
  const byEx = new Map<string, any[]>();
  for (const p of prs) byEx.set(p.exercise_id, [...(byEx.get(p.exercise_id) ?? []), p]);
  const date = (p: any) => new Date(p.started_at).toLocaleDateString();
  return h("div", {}, [...byEx.entries()].sort((a, b) => name(a[0]).localeCompare(name(b[0]))).map(([id, list]) => {
    const best = list.find((p) => p.kind === "e1rm");
    return h("div", { class: "card" }, h("strong", {}, name(id)),
      best && h("div", {}, `Est. 1RM ${kg(Math.round(best.e1rm * 10) / 10)} kg · ${kg(best.load_kg)} × ${best.reps} · ${date(best)}`),
      h("div", { class: "muted" }, "Most reps at a load or heavier:"),
      list.filter((p) => p.kind === "reps").map((p) => h("div", { class: "muted" }, `${kg(p.load_kg)} kg × ${p.reps} · ${date(p)}`)));
  }));
}

// ---- benchmark tests (from v1) ----

const KINDS: [string, string, string][] = [
  ["BeepTest", "Beep test", "level.shuttle, e.g. 8.10"], ["OneRepMax", "1 rep max", "kg"],
  ["MaxReps", "Max reps", "reps"], ["TimedRun", "Timed run", "seconds"],
];

async function testsSection() {
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
  const pts = results.slice().sort((a, b) => a.date.localeCompare(b.date));
  return h("div", {}, form,
    h("h2", {}, `${kind[1]} over time`),
    lineChart(pts.map((r) => secs(r.date)), pts.map((r) => r.plot_value), kind[1], target) ?? h("p", { class: "muted" }, "No results yet."),
    h("label", { class: "row" }, h("span", { class: "muted" }, "Target line"), targetInput),
    h("h2", {}, "Best first"),
    results.map((r, i) => h("div", { class: "card" },
      h("div", {}, `#${i + 1}  ${r.score}${r.beep ? "" : ` ${kind[2]}`}`),
      h("div", { class: "muted" }, [r.date, r.exercise_id && exName(r.exercise_id)].filter(Boolean).join(" · ")))));
}

// ---- screen ----

const SECTIONS: [Progress, string][] = [["volume", "Volume"], ["strength", "Strength"], ["prs", "PRs"], ["tests", "Tests"]];

export async function progressScreen() {
  const body = await ({ volume: volumeSection, strength: strengthSection, prs: prsSection, tests: testsSection }[ui.progress])();
  return h("div", {},
    h("nav", { class: "sub" }, SECTIONS.map(([k, label]) => h("button", { class: ui.progress === k ? "active" : "", onclick: () => { ui.progress = k; render(); } }, label))),
    body);
}
