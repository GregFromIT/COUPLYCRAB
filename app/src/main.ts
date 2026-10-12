// Plain DOM UI. All data from peers is rendered with textContent (via `h`), never innerHTML.

type Tauri = {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen(name: string, cb: (e: unknown) => void): Promise<unknown> };
};
const tauri = (window as unknown as { __TAURI__: Tauri }).__TAURI__;
const invoke = <T = void>(cmd: string, args?: Record<string, unknown>) => tauri.core.invoke<T>(cmd, args);

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

const today = () => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};
const val = (form: HTMLElement, name: string) => (form.querySelector(`[name="${name}"]`) as HTMLInputElement).value.trim();
const optNum = (s: string) => (s === "" ? null : Number(s));

// ---- state ----
type Tab = "todos" | "library" | "templates" | "log" | "tests";
const ui = {
  tab: "todos" as Tab,
  editing: null as string | null,
  draftItems: [] as Record<string, string>[],
  testKind: "BeepTest",
  entry: {} as Record<string, Record<string, string>>, // per-block stepper values
  error: "",
};

const app = document.getElementById("app")!;

async function act(fn: () => Promise<unknown>) {
  try {
    ui.error = "";
    await fn();
  } catch (e) {
    ui.error = String(e);
  }
  await render();
}

// Peer changes re-render, but not while the user is typing.
let pending = false;
tauri.event.listen("doc-changed", () => {
  const a = document.activeElement;
  if (a && ["INPUT", "TEXTAREA", "SELECT"].includes(a.tagName)) pending = true;
  else render();
});
document.addEventListener("focusout", () => {
  if (pending) setTimeout(() => { if (!document.activeElement || document.activeElement === document.body) { pending = false; render(); } });
});

async function render() {
  let body: HTMLElement;
  try {
    const setup = await invoke<any>("get_setup");
    if (!setup.config) body = setupScreen(setup.data_dir);
    else {
      const screen = ui.tab === "todos" ? await todosScreen() : ui.tab === "tests" ? await testsScreen() : await fitnessScreen();
      const tabs: [Tab, string][] = [["todos", "Todos"], ["library", "Library"], ["templates", "Templates"], ["log", "Log Session"], ["tests", "Tests"]];
      body = h("div", {},
        h("nav", {}, tabs.map(([t, label]) => h("button", { class: ui.tab === t ? "active" : "", onclick: () => { ui.tab = t; ui.error = ""; render(); } }, label))),
        h("div", { class: "muted" }, `${setup.config.user_id} · ${setup.config.device_name} · ${setup.status}`),
        screen);
    }
  } catch (e) {
    body = h("div", {}, String(e));
  }
  app.replaceChildren(h("div", { id: "error" }, ui.error), body);
}

// ---- first run ----
function setupScreen(dataDir: string) {
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const peers = val(form, "peers").split(",").map((s) => s.trim()).filter(Boolean);
      act(() => invoke("save_config", { config: {
        user_id: val(form, "user_id"), device_name: val(form, "device_name"), listen_ip: val(form, "listen_ip"),
        listen_port: Number(val(form, "listen_port") || 7420), peers,
      } }));
    },
  },
    h("h2", {}, "First run"),
    h("p", { class: "muted" }, `Writes config.toml to ${dataDir}`),
    h("input", { name: "user_id", placeholder: "Your user id (e.g. alice)", required: true }),
    h("input", { name: "device_name", placeholder: "Device name (e.g. alice-laptop)", required: true }),
    h("input", { name: "listen_ip", placeholder: "This device's Tailscale IP (100.x.y.z)", required: true }),
    h("input", { name: "listen_port", type: "number", value: 7420 }),
    h("input", { name: "peers", placeholder: "Peers, comma separated MagicDNS names" }),
    h("button", { class: "primary" }, "Save"));
  return form;
}

// ---- todos ----
async function todosScreen() {
  const todos = await invoke<any[]>("todo_list");
  const editing = todos.find((t) => t.id === ui.editing);
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const args = { title: val(form, "title"), notes: val(form, "notes"), due: val(form, "due") || null };
      act(async () => {
        if (editing) await invoke("todo_edit", { id: editing.id, ...args });
        else await invoke("todo_add", args);
        ui.editing = null;
      });
    },
  },
    h("input", { name: "title", placeholder: "Todo", required: true, value: editing?.title ?? "" }),
    h("textarea", { name: "notes", placeholder: "Notes" }, editing?.notes ?? ""),
    h("div", { class: "row" },
      h("input", { name: "due", type: "date", value: editing?.due ?? "" }),
      h("button", { class: "primary" }, editing ? "Save" : "Add"),
      editing && h("button", { type: "button", onclick: () => { ui.editing = null; render(); } }, "Cancel")));
  const now = today();
  return h("div", {}, form, todos.map((t) => h("div", { class: "card" },
    h("div", { class: "row" },
      h("button", { "aria-label": t.completed ? "Mark not done" : "Mark done", onclick: () => act(() => invoke("todo_set_completed", { id: t.id, completed: !t.completed })) }, t.completed ? "☑" : "☐"),
      h("div", { class: "grow" },
        h("div", { class: t.completed ? "done" : "" }, t.title),
        t.due && h("div", { class: !t.completed && t.due < now ? "bad muted" : "muted" }, `${t.due < now && !t.completed ? "Overdue · " : ""}${t.due}`),
        t.notes && h("div", { class: "muted" }, t.notes)),
      h("button", { onclick: () => { ui.editing = t.id; render(); } }, "Edit"),
      h("button", { onclick: () => confirm(`Delete "${t.title}"?`) && act(() => invoke("todo_delete", { id: t.id })) }, "Delete")))));
}

// ---- fitness ----
const MEASUREMENTS = ["WeightReps", "Reps", "Timed", "Distance"];
const select = (name: string, options: [string, string][], selected = "", props: Props = {}) =>
  h("select", { name, ...props }, options.map(([v, label]) => h("option", { value: v, selected: v === selected }, label)));

async function fitnessScreen() {
  const st = await invoke<any>("fitness_state");
  const exName = (id: string) => st.exercises.find((e: any) => e.id === id)?.name ?? "(unknown exercise)";
  const exOptions: [string, string][] = st.exercises.map((e: any) => [e.id, e.name]);
  if (ui.tab === "library") return libraryScreen(st);
  if (ui.tab === "templates") return templatesScreen(st, exName, exOptions);
  return logScreen(st, exName, exOptions);
}

function libraryScreen(st: any) {
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      act(() => invoke("exercise_add", { name: val(form, "name"), measurement: val(form, "measurement"), tempo: val(form, "tempo") || null }));
    },
  },
    h("input", { name: "name", placeholder: "Exercise name", required: true }),
    h("div", { class: "row" },
      select("measurement", MEASUREMENTS.map((m) => [m, m])),
      h("input", { name: "tempo", placeholder: "Tempo (e.g. 2:1)" })),
    h("button", { class: "primary" }, "Add exercise"));
  return h("div", {}, form, st.exercises.map((e: any) =>
    h("div", { class: "card" }, h("div", {}, e.name), h("div", { class: "muted" }, [e.measurement, e.tempo && `tempo ${e.tempo}`].filter(Boolean).join(" · ")))));
}

function itemText(it: any, exName: (id: string) => string) {
  return [exName(it.exercise_id), `${it.target_sets}×${it.target_reps ?? "–"}`, it.target_load != null && `@${it.target_load}kg`,
    it.tempo && `tempo ${it.tempo}`, it.rest_s != null && `rest ${it.rest_s}s`].filter(Boolean).join(" ");
}

function templatesScreen(st: any, exName: (id: string) => string, exOptions: [string, string][]) {
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      const items = ui.draftItems.map((d) => ({
        exercise_id: d.exercise_id, target_sets: Number(d.target_sets || 1), target_reps: optNum(d.target_reps ?? ""),
        target_load: optNum(d.target_load ?? ""), tempo: d.tempo || null, rest_s: optNum(d.rest_s ?? ""),
      }));
      act(async () => {
        await invoke("template_add", { name: val(form, "name"), minutes: Number(val(form, "minutes") || 0), items });
        ui.draftItems = [];
      });
    },
  },
    h("div", { class: "row" },
      h("input", { name: "name", placeholder: "Template name", required: true }),
      h("input", { name: "minutes", type: "number", min: 0, placeholder: "Target minutes" })),
    ui.draftItems.map((d, i) => {
      const bind = (k: string) => ({ value: d[k] ?? "", oninput: (e: Event) => { d[k] = (e.target as HTMLInputElement).value; } });
      return h("div", { class: "card" },
        select("", exOptions, d.exercise_id, { onchange: (e: Event) => { d.exercise_id = (e.target as HTMLSelectElement).value; } }),
        h("div", { class: "row" },
          h("input", { type: "number", min: 1, placeholder: "Sets", ...bind("target_sets") }),
          h("input", { type: "number", min: 0, placeholder: "Reps", ...bind("target_reps") }),
          h("input", { type: "number", min: 0, step: "any", placeholder: "Load kg", ...bind("target_load") })),
        h("div", { class: "row" },
          h("input", { placeholder: "Tempo", ...bind("tempo") }),
          h("input", { type: "number", min: 0, placeholder: "Rest s", ...bind("rest_s") }),
          h("button", { type: "button", onclick: () => { ui.draftItems.splice(i, 1); render(); } }, "Remove")));
    }),
    h("button", {
      type: "button", disabled: exOptions.length === 0,
      onclick: () => { ui.draftItems.push({ exercise_id: exOptions[0]?.[0] ?? "", target_sets: "3" }); render(); },
    }, exOptions.length ? "+ Item" : "Add exercises in Library first"),
    h("button", { class: "primary" }, "Save template"));
  return h("div", {}, form, st.templates.map((t: any) => h("div", { class: "card" },
    h("div", {}, `${t.name} · ${t.target_minutes} min`),
    [...Object.values(t.items)].sort((a: any, b: any) => a.index - b.index).map((it: any) => h("div", { class: "muted" }, itemText(it, exName))))));
}

// Which SetLog fields each measurement type logs, with stepper increments.
const FIELDS: Record<string, [string, string, number][]> = {
  WeightReps: [["reps", "Reps", 1], ["load", "kg", 2.5]],
  Reps: [["reps", "Reps", 1]],
  Timed: [["duration_s", "Seconds", 5]],
  Distance: [["distance_m", "Metres", 100]],
};

function stepper(values: Record<string, string>, key: string, label: string, step: number) {
  const input = h("input", { type: "number", min: 0, step: "any", inputmode: "decimal", value: values[key] ?? "", "aria-label": label,
    oninput: (e: Event) => { values[key] = (e.target as HTMLInputElement).value; } }) as HTMLInputElement;
  const bump = (d: number) => () => { values[key] = String(Math.max(0, Number(values[key] || 0) + d)); input.value = values[key]; };
  return h("div", { class: "stepper" }, h("span", { class: "muted" }, label),
    h("button", { type: "button", "aria-label": `${label} minus`, onclick: bump(-step) }, "−"), input,
    h("button", { type: "button", "aria-label": `${label} plus`, onclick: bump(step) }, "+"));
}

function logScreen(st: any, exName: (id: string) => string, exOptions: [string, string][]) {
  const mine = st.sessions.find((s: any) => s.user_id === st.me && s.status === "InProgress");
  if (!mine) {
    const form = h("form", {
      onsubmit: (e: Event) => {
        e.preventDefault();
        ui.entry = {};
        act(() => invoke("session_start", { templateId: val(form, "template") || null }));
      },
    },
      select("template", [["", "Blank session"], ...st.templates.map((t: any) => [t.id, t.name] as [string, string])]),
      h("button", { class: "primary" }, "Start session"));
    return h("div", {}, form, h("h2", {}, "Recent sessions"), st.sessions.slice(0, 20).map((s: any) => h("div", { class: "card" },
      h("div", {}, `${new Date(s.started_at).toLocaleString()} · ${s.user_id} · ${s.status}`),
      h("div", { class: "muted" }, `${Object.keys(s.sets).length} sets${s.note ? ` · ${s.note}` : ""}`))));
  }

  const template = st.templates.find((t: any) => t.id === mine.template_id);
  const sets = [...Object.values(mine.sets)].sort((a: any, b: any) => a.index - b.index) as any[];
  const blocks: { key: string; item?: any }[] = template
    ? [...Object.values(template.items)].sort((a: any, b: any) => a.index - b.index).map((it: any, i) => ({ key: `t${i}`, item: it }))
    : [{ key: "blank" }];

  const blockEl = (b: { key: string; item?: any }) => {
    const values = (ui.entry[b.key] ??= {});
    if (!b.item) values.exercise_id ??= exOptions[0]?.[0] ?? "";
    const exId = b.item?.exercise_id ?? values.exercise_id;
    const ex = st.exercises.find((e: any) => e.id === exId);
    const fields = FIELDS[ex?.measurement] ?? FIELDS.Reps;
    const done = sets.filter((s) => s.exercise_id === exId);
    // Prefill from the last logged set, else from template targets.
    const last = done[done.length - 1];
    for (const [k] of fields) values[k] ??= String(last?.[k] ?? (k === "reps" ? b.item?.target_reps : k === "load" ? b.item?.target_load : "") ?? "");
    return h("div", { class: "card" },
      b.item ? h("div", {}, h("strong", {}, exName(exId)), h("div", { class: "muted" }, `${done.length}/${b.item.target_sets} sets · target ${itemText(b.item, () => "")}`))
        : select("", exOptions, exId, { onchange: (e: Event) => { ui.entry[b.key] = { exercise_id: (e.target as HTMLSelectElement).value }; render(); } }),
      done.map((s, i) => h("div", { class: "muted" }, `Set ${i + 1}: ${fields.map(([k, l]) => `${s[k] ?? "–"} ${l}`).join(" · ")}`)),
      fields.map(([k, l, step]) => stepper(values, k, l, step)),
      h("button", {
        class: "primary", disabled: !exId,
        onclick: () => {
          const set: Record<string, unknown> = { exercise_id: exId };
          for (const [k] of fields) set[k] = optNum(values[k] ?? "");
          act(() => invoke("session_log_set", { sessionId: mine.id, set }));
        },
      }, "Log set"));
  };

  const note = h("textarea", { placeholder: "Note (optional)" }) as HTMLTextAreaElement;
  const finish = (status: string) => () => act(() => invoke("session_finish", { sessionId: mine.id, status, note: note.value.trim() || null }));
  return h("div", {},
    h("h2", {}, template ? template.name : "Blank session"),
    template && h("div", { class: "muted" }, `Target ${template.target_minutes} min · started ${new Date(mine.started_at).toLocaleTimeString()}`),
    blocks.map(blockEl),
    h("div", { class: "card" }, note,
      h("div", { class: "row" },
        h("button", { class: "primary", onclick: finish("Completed") }, "Finish: Completed"),
        h("button", { onclick: finish("Partial") }, "Finish: Partial"))));
}

// ---- benchmark tests ----
const KINDS: [string, string, string][] = [
  ["BeepTest", "Beep test", "level.shuttle, e.g. 8.10"], ["OneRepMax", "1 rep max", "kg"],
  ["MaxReps", "Max reps", "reps"], ["TimedRun", "Timed run", "seconds"],
];

async function testsScreen() {
  const [results, st] = await Promise.all([invoke<any[]>("test_list", { kind: ui.testKind }), invoke<any>("fitness_state")]);
  const kind = KINDS.find((k) => k[0] === ui.testKind)!;
  const needsExercise = ui.testKind === "OneRepMax" || ui.testKind === "MaxReps";
  const exName = (id: string) => st.exercises.find((e: any) => e.id === id)?.name ?? "";
  const form = h("form", {
    onsubmit: (e: Event) => {
      e.preventDefault();
      act(() => invoke("test_record", { kind: ui.testKind, date: val(form, "date"), score: val(form, "score"), exerciseId: needsExercise ? val(form, "exercise") || null : null }));
    },
  },
    select("kind", KINDS.map(([k, l]) => [k, l]), ui.testKind, { onchange: (e: Event) => { ui.testKind = (e.target as HTMLSelectElement).value; render(); } }),
    h("div", { class: "row" },
      h("input", { name: "date", type: "date", value: today(), required: true }),
      h("input", { name: "score", placeholder: kind[2], inputmode: "decimal", required: true })),
    needsExercise && select("exercise", [["", "(no exercise)"], ...st.exercises.map((e: any) => [e.id, e.name] as [string, string])]),
    h("button", { class: "primary" }, "Record"));
  const score = (r: any) => (r.beep ? `${r.beep.level}.${r.beep.shuttle}` : `${r.value} ${kind[2]}`);
  return h("div", {}, form, h("h2", {}, `${kind[1]}: best first`), results.map((r, i) => h("div", { class: "card" },
    h("div", {}, `#${i + 1}  ${score(r)}`),
    h("div", { class: "muted" }, [r.date, r.user_id, r.exercise_id && exName(r.exercise_id)].filter(Boolean).join(" · ")))));
}

render();
