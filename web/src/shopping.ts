// Shopping lists. Built for use in a shop: big tick targets, a quick-add box with autocomplete,
// and live updates: when someone else adds or ticks something, only the item list is redrawn
// (never the quick-add box), so it can't steal focus or eat what is being typed.

import { api, ApiError, newId } from "./api";
import { h } from "./dom";
import { render, run, ui } from "./state";

const CATEGORIES = ["produce", "meat", "dairy", "bakery", "pantry", "frozen", "household", "other"];
const cap = (s: string) => s[0].toUpperCase() + s.slice(1);
const catLabel = (c: string | null) => (c ? cap(c) : "Uncategorised");

/** Redraws the visible list (set while the shopping screen is shown). */
let refreshNow: (() => Promise<void>) | null = null;
let pending = 0;

/** Called by live.ts when the shopping document changed somewhere. Bursts become one redraw. */
export function shoppingChanged() {
  if (!refreshNow) return;
  clearTimeout(pending);
  pending = window.setTimeout(() => refreshNow?.(), 100);
}

export function leaveShopping() {
  refreshNow = null;
}

/** Two devices opening the app first both PUT the same ID, so they make one list. */
async function ensureDefaultList() {
  try {
    await api.putList("groceries", "Groceries");
  } catch (e) {
    // "groceries" was deleted earlier and cannot be reused: make a fresh one.
    if (e instanceof ApiError && e.status === 409) await api.putList(newId(), "Groceries");
    else throw e;
  }
}

export async function shoppingScreen() {
  refreshNow = null;
  let lists = await api.lists();
  if (!lists.length) {
    await ensureDefaultList();
    lists = await api.lists();
  }
  if (!lists.some((l) => l.id === ui.list)) ui.list = lists[0].id;
  const listId = ui.list!;

  const tabs = h("div", { class: "row", id: "list-tabs" });
  const itemsEl = h("div", { id: "items" });
  let editing: string | null = null;
  let current: { lists: any[]; items: any[] } = { lists, items: await api.listItems(listId) };

  // ---- the item list ----
  const itemRow = (it: any) => {
    if (editing === it.id) return editForm(it);
    const meta = [it.quantity, it.note].filter(Boolean).join(" · ");
    return h("div", { class: `card item-row${it.checked ? " ticked" : ""}`, "data-id": it.id },
      h("button", { class: "tick", "aria-label": `${it.checked ? "Untick" : "Tick"} ${it.name}`, "aria-pressed": String(it.checked),
        onclick: (e: Event) => {
          (e.currentTarget as HTMLElement).closest(".item-row")!.classList.add("busy");
          run(() => api.patchItem(listId, it.id, { checked: !it.checked }), refresh);
        } }, it.checked ? "☑" : "☐"),
      h("div", { class: "grow" },
        h("div", { class: "name" }, it.name),
        meta && h("div", { class: "muted" }, meta),
        it.checked && h("div", { class: "muted" }, `ticked by ${it.checked_by}`)),
      h("button", { "aria-label": `Edit ${it.name}`, onclick: () => { editing = it.id; paint(); } }, "Edit"));
  };

  const editForm = (it: any) => {
    const form = h("form", {
      class: "card",
      onsubmit: (e: Event) => {
        e.preventDefault();
        const f = (n: string) => (form.querySelector(`[name="${n}"]`) as HTMLInputElement).value.trim();
        const body = { name: f("name"), quantity: f("quantity") || null, note: f("note") || null, category: f("category") || null };
        run(async () => { await api.patchItem(listId, it.id, body); editing = null; }, refresh);
      },
    },
      h("input", { name: "name", required: true, maxlength: 200, "aria-label": "Name", value: it.name }),
      h("div", { class: "row" },
        h("input", { name: "quantity", maxlength: 50, placeholder: "Quantity", "aria-label": "Quantity", value: it.quantity ?? "" }),
        h("select", { name: "category", "aria-label": "Category" },
          h("option", { value: "" }, "No category"),
          CATEGORIES.map((c) => h("option", { value: c, selected: c === it.category }, cap(c))))),
      h("input", { name: "note", maxlength: 500, placeholder: "Note", "aria-label": "Note", value: it.note ?? "" }),
      h("div", { class: "row" },
        h("button", { class: "primary" }, "Save"),
        h("button", { type: "button", onclick: () => { editing = null; paint(); } }, "Cancel"),
        h("button", { type: "button", onclick: () => confirm(`Remove "${it.name}"?`) && run(async () => { await api.deleteItem(listId, it.id); editing = null; }, refresh) }, "Remove")));
    return form;
  };

  /** The server sends unticked items first, by category; group them under headings. */
  const paintItems = (items: any[]) => {
    const open = items.filter((i) => !i.checked), done = items.filter((i) => i.checked);
    const nodes: (HTMLElement | false)[] = [];
    let last: string | null | undefined = undefined;
    for (const it of open) {
      if (it.category !== last) { nodes.push(h("h3", { class: "cat" }, catLabel(it.category))); last = it.category; }
      nodes.push(itemRow(it));
    }
    if (!items.length) nodes.push(h("p", { class: "muted" }, "Nothing on this list yet. Add something above."));
    if (done.length) {
      nodes.push(h("div", { class: "row" }, h("h3", { class: "cat grow" }, `Ticked (${done.length})`),
        h("button", { onclick: () => run(() => api.clearChecked(listId), refresh) }, "Clear ticked")));
      nodes.push(...done.map(itemRow));
    }
    itemsEl.replaceChildren(...(nodes.filter(Boolean) as HTMLElement[]));
  };

  const paintTabs = (all: any[]) => {
    tabs.replaceChildren(
      ...all.map((l) => h("button", { class: l.id === listId ? "active" : "", "aria-pressed": String(l.id === listId), onclick: () => { ui.list = l.id; render(); } },
        `${l.name} (${l.unchecked})`)),
      h("button", { "aria-label": "New list", onclick: () => {
        const name = prompt("Name of the new list (e.g. Bunnings)");
        if (name?.trim()) { const id = newId(); run(async () => { await api.putList(id, name.trim()); ui.list = id; }); }
      } }, "+"));
  };

  const paint = () => { paintTabs(current.lists); paintItems(current.items); };

  /** Refetch just this screen's data. Errors leave what is shown as it is (the next event retries). */
  let busy = false, again = false;
  const refresh = async () => {
    if (busy) { again = true; return; }
    busy = true;
    try {
      do {
        again = false;
        const [l, items] = await Promise.all([api.lists(), api.listItems(listId)]);
        current = { lists: l, items };
        // Don't redraw under someone editing an item.
        if (editing === null || !current.items.some((i) => i.id === editing)) { editing = null; paint(); }
        else paintTabs(l);
      } while (again);
    } catch { /* offline or restarting: keep what is shown */ }
    busy = false;
  };
  refreshNow = refresh;

  // ---- quick add ----
  const datalist = h("datalist", { id: "suggestions" });
  let timer = 0;
  const suggest = (text: string) => {
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      const found = await api.suggestions(listId, text).catch(() => [] as string[]);
      datalist.replaceChildren(...found.map((s) => h("option", { value: s })));
    }, 120);
  };
  let attempt: { key: string; id: string } | null = null;
  const nameInput = h("input", { name: "name", placeholder: "Add an item…", maxlength: 200, autocomplete: "off", "aria-label": "Add an item", list: "suggestions",
    value: ui.quickAdd, oninput: (e: Event) => { ui.quickAdd = (e.target as HTMLInputElement).value; suggest(ui.quickAdd); } }) as HTMLInputElement;
  const qty = h("input", { placeholder: "Quantity", maxlength: 50, "aria-label": "Quantity" }) as HTMLInputElement;
  const cat = h("select", { "aria-label": "Category" }, h("option", { value: "" }, "No category"), CATEGORIES.map((c) => h("option", { value: c }, cap(c)))) as HTMLSelectElement;
  const addForm = h("form", {
    class: "card",
    onsubmit: (e: Event) => {
      e.preventDefault();
      const name = nameInput.value.trim();
      if (!name) return;
      const body = { name, quantity: qty.value.trim() || null, category: cat.value || null };
      const key = JSON.stringify(body);
      if (attempt?.key !== key) attempt = { key, id: newId() }; // a retap after a failure reuses the ID
      const id = attempt.id;
      run(async () => { await api.putItem(listId, id, body); attempt = null; ui.quickAdd = ""; nameInput.value = ""; qty.value = ""; cat.value = ""; }, async () => {
        await refresh();
        suggest("");
        nameInput.focus(); // add the next one straight away
      });
    },
  },
    h("div", { class: "row" }, nameInput, h("button", { class: "primary" }, "Add")),
    datalist,
    h("details", {}, h("summary", { class: "muted" }, "Quantity and category"), h("div", { class: "row" }, qty, cat)));

  // ---- list options ----
  const options = h("details", { class: "card" }, h("summary", { class: "muted" }, "List options"),
    h("div", { class: "row" },
      h("button", { onclick: () => {
        const cur = lists.find((l) => l.id === listId);
        const name = prompt("Rename the list", cur?.name);
        if (name?.trim()) run(() => api.patchList(listId, { name: name.trim() }));
      } }, "Rename"),
      h("button", { onclick: () => {
        const cur = lists.find((l) => l.id === listId);
        if (confirm(`Delete the list "${cur?.name}" and everything on it?`)) run(() => api.patchList(listId, { deleted: true }));
      } }, "Delete list")));

  paint();
  suggest("");
  return h("div", {},
    h("div", { class: "row" }, h("h2", { class: "grow" }, "Shopping"), h("span", { id: "live", class: "muted" }, "")),
    tabs, addForm, itemsEl, options);
}
