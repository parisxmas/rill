/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <DatePicker> — the "select" form of the calendar: a trigger field that opens
// the Calendar in a popover, shows the chosen date, and closes on pick. Reuses
// the Calendar component and the Select's popover/click-outside chrome.
//
//   const date = signal(null);
//   DatePicker({ value: date, placeholder: "Pick a date…" })
//
// Props: value (Signal<Date|null>), placeholder?, format?(date)→string, weekStart?

import { signal, html, when, onCleanup } from "../index.js";
import { Calendar } from "./calendar.js";
import { anchor } from "./anchor.js";

export function DatePicker(props) {
  const value = props.value;
  const open = signal(false);
  const fmt = props.format || ((d) => d.toLocaleDateString());
  const text = () => (value() ? fmt(value()) : props.placeholder || "Select a date…");
  const isEmpty = () => !value();

  const root =
    html`<div class=${() => "zap-sel zap-date" + (open() ? " is-open" : "")}><button type="button" class="zap-sel__trigger" onclick=${() => open.update((o) => !o)}><span class=${() => "zap-sel__val" + (isEmpty() ? " is-placeholder" : "")}>${text}</span><span class="zap-sel__caret">📅</span></button>${when(open, () => {
      const panel = html`<div class="zap-sel__panel zap-date__panel">${Calendar({ value, weekStart: props.weekStart, onSelect: () => open.set(false) })}</div>`;
      onCleanup(anchor(panel, root.querySelector(".zap-sel__trigger")));
      return panel;
    })}</div>`;

  const onDown = (e) => { if (!root.contains(e.target)) open.set(false); };
  document.addEventListener("mousedown", onDown);
  onCleanup(() => document.removeEventListener("mousedown", onDown));

  return root;
}
