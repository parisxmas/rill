/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// Toasts — a tiny notification system. Call toast() from anywhere; render
// <Toaster/> once (inside your .zap root so it inherits theme tokens).
//
//   import { toast, Toaster } from "./components/toast.js";
//   toast("Saved!", { kind: "success" });
//   html`… ${Toaster()}`;   // fixed stack, bottom-right
//
// State is a module-level signal (the store pattern); keyed by id so each toast
// animates in/out independently and auto-dismisses after `duration` ms.
import { signal, html, forEach } from "../index.js";

const toasts = signal([]);
let tid = 0;

export function toast(message, opts = {}) {
  const id = ++tid;
  toasts.set([...toasts(), { id, message, kind: opts.kind || "info" }]);
  const ttl = opts.duration ?? 3000;
  if (ttl) setTimeout(() => dismiss(id), ttl);
  return id;
}

export function dismiss(id) {
  toasts.set(toasts().filter((t) => t.id !== id));
}

export function Toaster() {
  return html`<div class="zap-toaster">${forEach(toasts, (t) => html`<div class=${"zap-toast zap-toast--" + t.kind}><span class="zap-toast__msg">${t.message}</span><button class="zap-toast__x" aria-label="Dismiss" onclick=${() => dismiss(t.id)}>✕</button></div>`)}</div>`;
}
