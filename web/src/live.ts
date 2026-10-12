// Live updates: one EventSource on /api/v1/events. The server says "this document changed";
// we refetch what is on screen. EventSource reconnects by itself, and on every (re)connect the
// server first sends the current counters, which is our cue to refetch anything we missed.
// https://developer.mozilla.org/en-US/docs/Web/API/EventSource

import { render, ui } from "./state";
import { shoppingChanged } from "./shopping";

let source: EventSource | null = null;
let todoTimer = 0;

function setStatus(text: string) {
  const el = document.getElementById("live");
  if (el) el.textContent = text;
}

/** Don't redraw a screen under someone who is typing in it. */
const typing = () => document.activeElement instanceof HTMLElement && ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement.tagName) && !!document.getElementById("app")?.contains(document.activeElement);

function changed(doc: string) {
  if (doc === "shopping") shoppingChanged();
  else if (doc === "todo" && ui.area === "todos" && ui.editingTodo === null) {
    clearTimeout(todoTimer);
    todoTimer = window.setTimeout(() => { if (!typing()) render(); }, 150);
  }
}

/** Open the stream once the user is known (an unidentified stream gets a 401, and EventSource does not retry that). */
export function ensureLive() {
  if (source || typeof EventSource === "undefined") return;
  source = new EventSource("/api/v1/events");
  source.onopen = () => setStatus("● live");
  source.onerror = () => setStatus("○ reconnecting…");
  source.onmessage = (e) => {
    try { changed(JSON.parse(e.data).doc); } catch { /* ignore a malformed message */ }
  };
}

/** The shopping screen was drawn: show the current connection state in its header. */
export function showLiveStatus() {
  setStatus(source?.readyState === EventSource.OPEN ? "● live" : source ? "○ reconnecting…" : "");
}
