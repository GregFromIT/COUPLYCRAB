// The assistant: a read-only chat. The conversation lives in this browser tab (sessionStorage) and is
// sent with each question; the server stores nothing. Under each answer, a collapsible list of the
// tools that ran and what they returned, so you can see where an answer came from.
//
// The chat updates in place (no screen re-render): asking is slow, and a re-render needs the
// server, so a failed question must not take the screen with it.

import { api, ApiError } from "./api";
import { h } from "./dom";
import { clearError, showError } from "./state";

type Turn = { role: "user" | "assistant"; content: string; tools?: any[]; capped?: boolean };

const KEY = "assistant-chat";
const load = (): Turn[] => { try { return JSON.parse(sessionStorage.getItem(KEY) ?? "[]"); } catch { return []; } };
const save = () => { try { sessionStorage.setItem(KEY, JSON.stringify(chat.slice(-40))); } catch { /* private mode */ } };

let chat: Turn[] = load();
let waiting = false;
let draft = "";
/** The pieces of the visible screen, so `paint` can update them without a re-render. */
let view: { messages: HTMLElement; box: HTMLTextAreaElement; send: HTMLButtonElement; clear: HTMLElement } | null = null;

const SUGGESTIONS = ["What's the workout today?", "Why did my bench recommendation change?", "How many chest sets this week?", "What's on the shopping list?", "What's overdue?"];

function toolsEl(tools: any[], capped?: boolean) {
  if (!tools.length && !capped) return null;
  return h("details", { class: "tools" },
    h("summary", { class: "muted" }, tools.length ? `Used ${tools.length} tool${tools.length === 1 ? "" : "s"}: ${tools.map((t) => t.name).join(", ")}` : "No tools used"),
    tools.map((t) => h("div", { class: "tool" },
      h("div", { class: t.error ? "bad" : "" }, `${t.name}(${Object.entries(t.arguments ?? {}).map(([k, v]) => `${k}: ${JSON.stringify(v)}`).join(", ")})${t.error ? " failed" : ""}`),
      h("pre", { class: "json" }, JSON.stringify(t.result, null, 2)))),
    capped && h("div", { class: "muted" }, "Stopped at the step limit."));
}

function paint() {
  if (!view) return;
  view.messages.replaceChildren(...([
    !chat.length && h("p", { class: "muted" }, "Read-only: it can look things up but never changes anything."),
    ...(!chat.length ? SUGGESTIONS.map((s) => h("button", { class: "chip", disabled: waiting, onclick: () => send(s) }, s)) : []),
    ...chat.map((t) => h("div", { class: `bubble ${t.role}` }, t.content, t.role === "assistant" && toolsEl(t.tools ?? [], t.capped))),
    waiting && h("div", { class: "bubble assistant muted", id: "thinking" }, "Thinking…"),
  ].filter(Boolean) as HTMLElement[]));
  view.box.disabled = waiting;
  view.box.value = draft;
  view.send.disabled = waiting;
  view.send.textContent = waiting ? "Waiting…" : "Send";
  view.clear.hidden = chat.length === 0;
}

async function send(text: string) {
  clearError();
  const history = chat.filter((t) => t.content).map((t) => ({ role: t.role, content: t.content }));
  draft = "";
  chat.push({ role: "user", content: text });
  waiting = true;
  paint();
  try {
    const r = await api.assistantChat(text, history);
    chat.push({ role: "assistant", content: r.answer || "(no answer)", tools: r.tools, capped: r.hit_iteration_cap });
  } catch (e) {
    // Not answered: take the question back out of the chat and into the box, and offer Retry.
    chat.pop();
    draft = text;
    showError(e instanceof ApiError ? e : String(e), () => send(text));
  }
  waiting = false;
  save();
  paint();
  window.scrollTo(0, document.body.scrollHeight);
}

export async function assistantScreen() {
  view = null;
  const status = await api.assistantStatus();
  if (!status.configured) {
    return h("div", { class: "card" }, h("strong", {}, "The assistant isn't set up"),
      h("p", { class: "muted" }, "It answers questions about your training, todos and shopping list using a local model (Ollama), and can only read. To turn it on, whoever runs the host adds an [assistant] section with url and model to config.toml (steps are in docs/BLOCKERS.md), or starts the server with --fake-llm to try this screen with a keyword demo."));
  }
  const submit = () => { const t = box.value.trim(); if (t && !waiting) send(t); };
  const box = h("textarea", { name: "message", rows: 2, maxlength: 2000, placeholder: "Ask about your training, todos or shopping list…", "aria-label": "Message",
    oninput: (e: Event) => { draft = (e.target as HTMLTextAreaElement).value; },
    onkeydown: (e: KeyboardEvent) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } } }) as HTMLTextAreaElement;
  const sendBtn = h("button", { class: "primary" }, "Send") as HTMLButtonElement;
  const messages = h("div", { class: "chat", id: "chat" });
  const clear = h("button", { onclick: () => { chat = []; save(); paint(); } }, "Clear");
  view = { messages, box, send: sendBtn, clear };
  paint();
  return h("div", {},
    h("div", { class: "row" }, h("h2", { class: "grow" }, "Assistant"), clear),
    h("div", { class: "muted" }, status.fake ? "Demo mode: matches keywords, no language model." : `Model: ${status.model}`),
    messages,
    h("form", { class: "card", onsubmit: (e: Event) => { e.preventDefault(); submit(); } }, box, sendBtn));
}
