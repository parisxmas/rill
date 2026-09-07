/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <Select> — a themed single/multi select + combobox built on zap.
//
// Props:
//   options:    Array<string | {value, label}>  (or a signal/getter returning one)
//   value:      Signal  — single: the value; multi: an array of values
//   multiple?:  boolean — multi-select (stays open, toggles)
//   searchable?:boolean — combobox: filter options with a text field
//   placeholder?: string
//   onChange?:  (value) => void
//
//   const fruit = signal("apple");
//   Select({ options: ["apple","pear","plum"], value: fruit })
//   const picks = signal([]);
//   Select({ multiple: true, searchable: true, options: countries, value: picks })

import { signal, computed, html, when, forEach, onCleanup } from "../index.js";
import { anchor } from "./anchor.js";

const toOpt = (o) => (o && typeof o === "object" ? o : { value: o, label: String(o) });

export function Select(props) {
  const multiple = !!props.multiple;
  const searchable = !!props.searchable;
  const creatable = !!props.creatable;   // free entry: a value not in the list can be picked too
  const editable = !!props.editable && !multiple;  // trigger is a directly typable input (combobox); filters + sets the value
  const value = props.value;
  const open = signal(false);
  const query = signal("");

  const allOpts = () =>
    (typeof props.options === "function" ? props.options() : props.options || []).map(toOpt);

  const isSelected = (v) =>
    multiple ? (value() || []).some((x) => x === v) : value() === v;

  const pick = (v) => {
    if (multiple) {
      const cur = value() || [];
      value.set(cur.some((x) => x === v) ? cur.filter((x) => x !== v) : [...cur, v]);
    } else {
      value.set(v);
      open.set(false);
      query.set("");
    }
    props.onChange && props.onChange(value());
  };

  const triggerText = () => {
    const opts = allOpts();
    if (multiple) {
      const arr = value() || [];
      if (!arr.length) return props.placeholder || "Select…";
      return arr.map((v) => opts.find((o) => o.value === v)?.label ?? v).join(", ");
    }
    const o = opts.find((o) => o.value === value());
    if (o) return o.label;
    if (value()) return value();           // creatable: custom (not-in-list) value
    return props.placeholder || "Select…";
  };
  const isEmpty = () =>
    multiple ? !(value() || []).length : value() == null || value() === "";

  // filtered list, keyed by value so forEach reuses the right rows
  // Accent + Turkish İ/ı insensitive normalization (for search matching).
  const norm = (s) => String(s).normalize("NFD").replace(/[\u0300-\u036f]/g, "").toLowerCase().replace(/ı/g, "i");
  const filtered = computed(() => {
    // in editable mode the trigger itself (value) is the search text; otherwise the separate search box (query).
    const q = norm((editable ? (value() || "") : query()).trim());
    const opts = allOpts();
    const list = (q ? opts.filter((o) => norm(o.label).includes(q)) : opts).slice(0, 80);
    return list.map((o) => ({ id: o.value, value: o.value, label: o.label }));
  });

  const row = (o) =>
    html`<div class=${() => "zap-sel__opt" + (isSelected(o.value) ? " is-selected" : "")} onclick=${() => pick(o.value)}><span class="zap-sel__mark">${() => (isSelected(o.value) ? (multiple ? "☑" : "✓") : multiple ? "☐" : "")}</span><span class="zap-sel__lbl">${o.label}</span></div>`;

  // creatable: when the search text isn't in the list, offer a "use it" row; Enter picks it too
  const onSearchKey = (e) => {
    if (e.key === "Enter" && creatable) { const q = query().trim(); if (q) { e.preventDefault(); pick(q); } }
  };
  const createRow = () => {
    if (!creatable) return "";
    const q = query().trim();
    if (!q) return "";
    if (allOpts().some((o) => norm(o.label) === norm(q))) return "";
    return html`<div class="zap-sel__opt zap-sel__create" onclick=${() => pick(q)}><span class="zap-sel__mark">+</span><span class="zap-sel__lbl">${q}</span></div>`;
  };
  const emptyRow = () => (creatable || editable ? html`<span></span>` : html`<div class="zap-sel__empty">No matches</div>`);
  const panel = () => {
    const p = editable
      ? html`<div class="zap-sel__panel">${forEach(filtered, row, emptyRow)}</div>`
      : html`<div class="zap-sel__panel">${searchable ? html`<input class="zap-input zap-sel__search" placeholder=${props.searchPlaceholder || "Search…"} bind=${query} onkeydown=${onSearchKey} />` : ""}${() => createRow()}${forEach(filtered, row, emptyRow)}</div>`;
    // Out of anything that would clip it: a select in a scrolling table had
    // its list cut off at the edge of the scroll box.
    onCleanup(anchor(p, root.querySelector(".zap-sel__trigger, .zap-sel__input")));
    return p;
  };

  // editable: trigger = a directly typable input (bound to value). Opens on focus/typing;
  // outside clicks close via onDown (instead of blur → no race with option clicks).
  const editTrigger = () =>
    html`<input type="text" class="zap-input zap-sel__input" placeholder=${props.placeholder || ""} bind=${value} autocomplete="off" onfocus=${() => open.set(true)} oninput=${() => open.set(true)} />`;
  const buttonTrigger = () =>
    html`<button type="button" class="zap-sel__trigger" onclick=${() => open.update((o) => !o)}><span class=${() => "zap-sel__val" + (isEmpty() ? " is-placeholder" : "")}>${triggerText}</span><span class="zap-sel__caret">▾</span></button>`;

  const root =
    html`<div class=${() => "zap-sel" + (open() ? " is-open" : "") + (editable ? " zap-sel--edit" : "")}>${editable ? editTrigger() : buttonTrigger()}${when(open, panel)}</div>`;

  // close when clicking outside
  const onDown = (e) => { if (!root.contains(e.target)) open.set(false); };
  document.addEventListener("mousedown", onDown);
  onCleanup(() => document.removeEventListener("mousedown", onDown));

  return root;
}
