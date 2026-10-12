// Plain DOM helpers. All data is rendered with textContent (via `h`), never innerHTML.

export type Props = Record<string, unknown>;
export type Child = Node | string | null | undefined | false;

export function h(tag: string, props: Props = {}, ...children: (Child | Child[])[]): HTMLElement {
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
  for (const c of (children as unknown[]).flat(Infinity) as Child[]) if (c) el.append(c);
  return el;
}

export const pad = (n: number) => String(n).padStart(2, "0");
export const localDate = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
export const today = () => localDate(new Date());
export const val = (form: HTMLElement, name: string) => (form.querySelector(`[name="${name}"]`) as HTMLInputElement).value.trim();
export const optNum = (s: string | undefined) => (s === undefined || s === "" ? null : Number(s));
export const select = (name: string, options: [string, string][], selected = "", props: Props = {}) =>
  h("select", { name, ...props }, options.map(([v, label]) => h("option", { value: v, selected: v === selected }, label)));

export const storage = {
  get: (k: string) => { try { return localStorage.getItem(k); } catch { return null; } },
  set: (k: string, v: string | null) => { try { v === null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* private mode */ } },
};

/** A number field with − / + buttons. `values[key]` holds the text; `step` is the bump. */
export function stepper(values: Record<string, string>, key: string, label: string, step: number, max = Infinity, onChange?: () => void) {
  const input = h("input", { type: "number", min: 0, max: Number.isFinite(max) ? max : undefined, step: "any", inputmode: "decimal", value: values[key] ?? "", "aria-label": label,
    oninput: (e: Event) => { values[key] = (e.target as HTMLInputElement).value; onChange?.(); } }) as HTMLInputElement;
  const bump = (d: number) => () => {
    values[key] = String(Math.min(max, Math.max(0, Math.round((Number(values[key] || 0) + d) * 100) / 100)));
    input.value = values[key];
    onChange?.();
  };
  return h("div", { class: "stepper" }, h("span", { class: "muted" }, label),
    h("button", { type: "button", "aria-label": `${label} minus`, onclick: bump(-step) }, "−"), input,
    h("button", { type: "button", "aria-label": `${label} plus`, onclick: bump(step) }, "+"));
}

/** 82.5, 80 (no trailing zeros). */
export const kg = (x: number) => String(Math.round(x * 100) / 100);

/** m:ss */
export const mmss = (s: number) => `${Math.floor(s / 60)}:${pad(s % 60)}`;
