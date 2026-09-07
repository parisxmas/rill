/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// <DataGrid> — a themed, fine-grained data grid on the zap engine.
// Not the whole of AG Grid, but the features you actually reach for:
//   • sortable columns (click header → asc → desc → off, with caret)
//   • quick-filter search across all columns
//   • row selection (per-row checkbox + select-all on page)
//   • drag-to-resize columns
//   • sticky header, zebra rows, hover, right-aligned numerics
//   • client-side pagination + range/“N selected” footer
//   • cell editing chosen by the column's type — number, date (the calendar),
//     boolean (a switch, no edit mode), a Select for a list of options — with
//     keyboard navigation, validation, and a save that may be a promise
//   • empty state
//
// Props:
//   rows:     Array | signal<Array>   row objects (give each a stable `id`)
//   columns:  Array<{
//               field, header,
//               width?:     number (px, resizable),
//               align?:     "right",
//               sortable?:  boolean (default true),
//               value?:     (row) => any   (derive instead of row[field]),
//               format?:    (value, row) => string|Node,
//               editable?:  boolean,
//               maxLength?: number   (characters; the editor stops there too),
//               min?, max?: number   (for a number column),
//               type?:      "text" | "number" | "boolean" | "date" | "select"
//                           (read from the value when not given),
//               options?:   for "select" — what Select takes,
//               validate?:  (value, row) => string | null   (a message refuses it),
//               parse?:     (text, row) => value            (for a typed editor),
//               editor?:    ({row, value, commit, cancel}) => Node
//             }>
//   pageSize?:   number (default 10; 0 = show all, no pager or footer)
//   pageSizes?:  number[] — what the footer's "Rows" menu offers
//                (default [10, 25, 50, 100]; one entry hides the menu)
//   selectable?: boolean (checkbox column)
//   filterable?: boolean (default true)
//   note?:       string | getter — shown in the bar beside the row count, for
//                whatever the caller knows about the rows that the grid does
//                not (where they came from, how long they took)
//   onSelectionChange?: (rows[]) => void
//   onCellEdit?: ({row, field, value, oldValue}) => void | Promise
//   apiRef?:     an object the grid fills in — `refresh()` redraws cells after
//                the host has changed a row in place, `visible()` is the rows
//                on the screen right now, `flash(row, field)` lights one cell
//                a promise marks the cell as saving; a rejection puts the old
//                value back and shows why

import { signal, computed, effect, html, forEach } from "../index.js";
import { Pagination } from "./pagination.js";
import { Select } from "./select.js";
import { Switch } from "./switch.js";
import { DatePicker } from "./datepicker.js";

const getVal = (row, col) => (col.value ? col.value(row) : row[col.field]);
// A date the way it was written down, not the way UTC sees it: `toISOString`
// on a local midnight east of Greenwich hands back the day before.
const ymd = (d) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const cmp = (a, b) => {
  if (a == null && b == null) return 0;
  if (a == null) return -1;
  if (b == null) return 1;
  if (typeof a === "number" && typeof b === "number") return a - b;
  return String(a).localeCompare(String(b));
};

export function DataGrid(props) {
  const columns = props.columns || [];
  const selectable = !!props.selectable;
  const filterable = props.filterable !== false;
  // How many rows a page holds. A number to start with, and — unless the
  // caller says otherwise — a menu under the table to change it, because
  // "twenty-five at a time" is the reader's business rather than the
  // component's.
  const sizes = props.pageSizes ?? [10, 25, 50, 100];
  const pageSize = signal(props.pageSize ?? 10);
  const colW = columns.map((c) => signal(c.width ?? 150));

  const rowsArr = () => (typeof props.rows === "function" ? props.rows() : props.rows || []);

  const sortCol = signal(null);
  const sortDir = signal(1);
  const filter = signal("");
  const editing = signal(null); // { row, col } currently being edited
  const rev = signal(0);        // bumped after an edit so cell displays refresh
  // Optional imperative handle: let the host refresh default cells in place after it
  // mutates a row object (e.g. an API save), avoiding a full data reload/rebuild.
  if (props.apiRef) {
    props.apiRef.refresh = () => rev.update((n) => n + 1);
    // What is on the screen, for a host that wants to follow only these —
    // reading it inside an effect follows the page, the sort and the filter.
    props.apiRef.visible = () => paged();
    // Say that one cell has just changed under the reader's eyes. The class
    // goes on for as long as the animation in the theme lasts.
    props.apiRef.flash = (row, field) => {
      const r = paged().indexOf(row);
      const c = columns.findIndex((col) => col.field === field);
      const td = r >= 0 && c >= 0 && cellAt(r, c);
      if (!td) return;
      td.classList.remove("is-fresh");
      void td.offsetWidth; // so a second change restarts the animation
      td.classList.add("is-fresh");
      setTimeout(() => td.classList.remove("is-fresh"), 1400);
    };
  }
  const page = signal(0);
  const selected = signal(new Set()); // row object refs — stable across sort/filter

  // sort + filter pipeline (recomputes only when an input changes)
  const processed = computed(() => {
    let r = rowsArr();
    const f = filter().trim().toLowerCase();
    if (f) r = r.filter((row) => columns.some((c) => String(getVal(row, c) ?? "").toLowerCase().includes(f)));
    const sc = sortCol();
    if (sc) {
      const dir = sortDir();
      r = [...r].sort((a, b) => cmp(getVal(a, sc), getVal(b, sc)) * dir);
    }
    return r;
  });
  const total = computed(() => processed().length);
  const maxPage = () => (pageSize() ? Math.max(0, Math.ceil(total() / pageSize()) - 1) : 0);
  const paged = computed(() => {
    if (!pageSize()) return processed();
    const p = Math.min(page(), maxPage());
    return processed().slice(p * pageSize(), p * pageSize() + pageSize());
  });

  const toggleSort = (col) => {
    if (col.sortable === false) return;
    if (sortCol() !== col) { sortCol.set(col); sortDir.set(1); }
    else if (sortDir() === 1) sortDir.set(-1);
    else { sortCol.set(null); sortDir.set(1); }
    page.set(0);
  };

  const emit = () => props.onSelectionChange && props.onSelectionChange([...selected()]);
  const isSel = (row) => selected().has(row);
  const toggleRow = (row) => {
    const s = new Set(selected());
    s.has(row) ? s.delete(row) : s.add(row);
    selected.set(s); emit();
  };
  const allOnPage = () => { const pg = paged(); return pg.length > 0 && pg.every((r) => selected().has(r)); };
  const toggleAll = () => {
    const s = new Set(selected());
    const pg = paged();
    const all = pg.every((r) => s.has(r));
    pg.forEach((r) => (all ? s.delete(r) : s.add(r)));
    selected.set(s); emit();
  };

  const startResize = (e, i) => {
    e.preventDefault(); e.stopPropagation();
    const startX = e.clientX;
    const startW = colW[i].peek();
    const move = (ev) => colW[i].set(Math.max(60, startW + (ev.clientX - startX)));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const colspan = columns.length + (selectable ? 1 : 0);

  const headCell = (col, i) =>
    html`<th class="zap-grid__hcell"><div class=${"zap-grid__th" + (col.sortable === false ? "" : " is-sortable")} onclick=${() => toggleSort(col)}><span>${col.header}</span><span class="zap-grid__caret">${() => (sortCol() === col ? (sortDir() === 1 ? "▲" : "▼") : "")}</span></div><span class="zap-grid__resize" onclick=${(e) => e.stopPropagation()} onpointerdown=${(e) => startResize(e, i)}></span></th>`;

  // --- editing ----------------------------------------------------------------
  //
  // Which editor a cell opens is decided by what the column holds rather than
  // by the grid: a number gets a number field, a date the calendar, a boolean
  // a switch that needs no edit mode at all, and a column with `options` the
  // same Select the rest of the page uses. `type` says it outright when the
  // value cannot be read from (an empty column, a date kept as a string), and
  // `editor` replaces the lot with your own.
  //
  // A cell is entered by double-click, by Enter or F2 on it, or by typing over
  // it. Enter commits and steps down, Tab commits and steps right, Escape puts
  // back what was there. `validate` refuses a value and says why; `onCellEdit`
  // may return a promise, and if it rejects the old value goes back — nothing
  // pretends to be saved that is not.

  const typeOf = (col, row) => {
    if (col.type) return col.type;
    if (col.options) return "select";
    const v = row ? getVal(row, col) : undefined;
    if (typeof v === "number") return "number";
    if (typeof v === "boolean") return "boolean";
    if (v instanceof Date) return "date";
    return "text";
  };
  const editable = (col) => !!col.editable && !!col.field;

  // What a column says about its own values, before whatever `validate` adds.
  const house = (col, v) => {
    if (typeof v === "string" && col.maxLength != null && v.length > col.maxLength) {
      return `at most ${col.maxLength} characters`;
    }
    if (typeof v === "number") {
      if (col.min != null && v < col.min) return `at least ${col.min}`;
      if (col.max != null && v > col.max) return `at most ${col.max}`;
    }
    return null;
  };

  // Per-cell state — what is being saved and what was refused. Held beside the
  // rows rather than in them, and read only where `rev` is read, so a change
  // to it redraws exactly the cells that show it.
  const marks = new WeakMap();
  const markOf = (row, col) => marks.get(row)?.[col.field] ?? null;
  const mark = (row, col, state) => {
    let m = marks.get(row);
    if (!m) marks.set(row, (m = {}));
    if (state) m[col.field] = state;
    else delete m[col.field];
    rev.update((n) => n + 1);
  };

  const commit = (row, col, next) => {
    const old = getVal(row, col);
    if (Object.is(next, old)) { editing.set(null); mark(row, col, null); return true; }
    // The limits a column states are checked here as well as by the editor:
    // an editor can be typed into, but a value can also arrive from a paste,
    // a `select`, or a caller's own `commit`.
    const why = house(col, next) ?? (col.validate ? col.validate(next, row) : null);
    if (why) { mark(row, col, { error: why }); return false; }
    row[col.field] = next;
    editing.set(null);
    mark(row, col, null);
    const out = props.onCellEdit && props.onCellEdit({ row, field: col.field, value: next, oldValue: old });
    if (out && typeof out.then === "function") {
      mark(row, col, { saving: true });
      out.then(
        () => mark(row, col, null),
        (err) => { row[col.field] = old; mark(row, col, { error: String(err?.message ?? err) }); },
      );
    }
    return true;
  };

  // Where the keyboard is. A cell knows its place so the arrows can find the
  // next one, and `paged` changing under it is simply a new place with the
  // same coordinates.
  const focused = signal(null); // { r, c }
  let table = null;
  const cellAt = (r, c) => table && table.querySelector(`td[data-r="${r}"][data-c="${c}"]`);
  const step = (dr, dc) => {
    const f = focused();
    if (!f) return;
    const r = Math.max(0, Math.min(paged().length - 1, f.r + dr));
    const c = Math.max(0, Math.min(columns.length - 1, f.c + dc));
    const td = cellAt(r, c);
    if (td) td.focus();
  };

  const editorFor = (row, col, first) => {
    const kind = typeOf(col, row);
    const v0 = getVal(row, col);
    const done = (v) => { if (commit(row, col, v)) { const f = focused(); if (f) Promise.resolve().then(() => step(0, 0)); } };

    if (col.editor) return col.editor({ row, value: v0, commit: done, cancel: () => editing.set(null) });

    if (kind === "select") {
      const sig = signal(v0);
      return html`<div class="zap-grid__editor" onkeydown=${(e) => e.stopPropagation()}>${Select({
        options: col.options,
        value: sig,
        onChange: (v) => done(v),
      })}</div>`;
    }
    if (kind === "date") {
      const sig = signal(v0 instanceof Date ? v0 : v0 ? new Date(v0) : null);
      let settled = false;
      effect(() => {
        const d = sig();
        if (!settled) { settled = true; return; }        // the value it opened with
        done(typeof v0 === "string" || v0 == null ? ymd(d) : d);
      });
      return html`<div class="zap-grid__editor" onkeydown=${(e) => e.stopPropagation()}>${DatePicker({ value: sig })}</div>`;
    }

    const inp = html`<input class="zap-input zap-grid__edit" type=${kind === "number" ? "number" : "text"}
      maxlength=${col.maxLength ?? false} min=${col.min ?? false} max=${col.max ?? false}
      value=${first ?? (v0 == null ? "" : v0)} />`;
    Promise.resolve().then(() => { inp.focus(); if (first == null && inp.select) inp.select(); });
    // Committing takes this input out of the document, and a node removed
    // while it has the focus blurs on the spot — which lands back here before
    // `commit` has returned. So the door is shut first and opened again only
    // if the value was refused: without that the second pass commits the same
    // value again and the cell is drawn twice, once by each.
    let over = false;
    const take = (after) => {
      if (over) return;
      over = true;
      const raw = inp.value;
      const next = kind === "number" ? (raw === "" ? null : Number(raw)) : raw;
      if (kind === "number" && raw !== "" && Number.isNaN(next)) {
        over = false;
        mark(row, col, { error: "not a number" });
        return;
      }
      if (!commit(row, col, col.parse ? col.parse(raw, row) : next)) {
        over = false; // refused, and the editor stays open with the reason on it
        return;
      }
      if (after) after();
    };
    inp.addEventListener("keydown", (e) => {
      // The cell underneath answers Enter by opening an editor. This is that
      // editor, so the key stops here — otherwise committing on Enter reopens
      // the very editor it just closed.
      e.stopPropagation();
      if (e.key === "Enter") { e.preventDefault(); take(() => step(1, 0)); }
      else if (e.key === "Tab") { e.preventDefault(); take(() => step(0, e.shiftKey ? -1 : 1)); }
      else if (e.key === "Escape") { e.preventDefault(); over = true; mark(row, col, null); editing.set(null); }
    });
    inp.addEventListener("blur", () => take(null));
    return inp;
  };

  const startEdit = (row, col, first) => {
    if (!editable(col)) return;
    if (typeOf(col, row) === "boolean") { commit(row, col, !getVal(row, col)); return; }
    editing.set({ row, col, first });
  };

  const cell = (row, col, ci) => {
    // custom cell renderer (AG Grid's cellRenderer): built ONCE per row; it owns
    // its reactivity, so live updates touch only this cell. Receives (row, td).
    if (col.cell) {
      const td = html`<td class=${col.align === "right" ? "zap-grid__num" : ""}></td>`;
      const n = col.cell(row, td);
      if (n != null) td.appendChild(n instanceof Node ? n : document.createTextNode(String(n)));
      return td;
    }
    const classes = () => {
      const m = (rev(), markOf(row, col));
      const f = focused();
      const here = f && f.r === Number(td.dataset.r) && f.c === ci;
      return [
        col.align === "right" ? "zap-grid__num" : "",
        editable(col) ? "zap-grid__editable" : "",
        m && m.saving ? "is-saving" : "",
        m && m.error ? "is-invalid" : "",
        here ? "is-focused" : "",
      ].filter(Boolean).join(" ");
    };
    // Whether THIS cell is the one being edited, rather than what is being
    // edited anywhere: a boolean that only flips for the two cells involved,
    // so an edit redraws those and no others.
    const mine = computed(() => {
      const e = editing();
      return !!(e && e.row === row && e.col === col);
    });
    const td = html`<td class=${classes} tabindex=${editable(col) ? "0" : "-1"}
      data-c=${String(ci)}
      title=${() => { const m = (rev(), markOf(row, col)); return m && m.error ? m.error : ""; }}
      onfocus=${() => focused.set({ r: Number(td.dataset.r), c: ci })}
      ondblclick=${() => startEdit(row, col)}
      onkeydown=${(e) => {
        // Only what the cell itself was told, never what an editor inside it
        // is still handling.
        if (e.target !== td || mine.peek()) return;
        if (e.key === "Enter" || e.key === "F2") { e.preventDefault(); startEdit(row, col); }
        else if (e.key === "ArrowDown") { e.preventDefault(); step(1, 0); }
        else if (e.key === "ArrowUp") { e.preventDefault(); step(-1, 0); }
        else if (e.key === "ArrowRight") { e.preventDefault(); step(0, 1); }
        else if (e.key === "ArrowLeft") { e.preventDefault(); step(0, -1); }
        else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && editable(col)) {
          e.preventDefault();
          startEdit(row, col, e.key);
        }
      }}>${() => {
      if (mine()) return editorFor(row, col, editing.peek().first);
      rev();
      const v = getVal(row, col);
      if (typeOf(col, row) === "boolean" && editable(col)) {
        const sig = signal(!!v);
        return html`<span class="zap-grid__bool">${Switch({
          value: sig,
          onChange: (on) => commit(row, col, on),
        })}</span>`;
      }
      return col.format ? col.format(v, row) : v;
    }}</td>`;
    return td;
  };

  const bodyRow = (row, i) =>
    html`<tr class=${() => (selectable && isSel(row) ? "is-selected" : "")}>${selectable ? html`<td class="zap-grid__check"><input type="checkbox" checked=${() => isSel(row)} onchange=${() => toggleRow(row)} /></td>` : ""}${columns.map((col, ci) => {
      const td = cell(row, col, ci);
      td.dataset.r = String(i);
      return td;
    })}</tr>`;

  const rangeText = () => {
    const t = total();
    const sel = selectable && selected().size ? `  ·  ${selected().size} selected` : "";
    if (!t) return "0 rows" + sel;
    if (!pageSize()) return `${t} rows${sel}`;
    const s = Math.min(page(), maxPage()) * pageSize();
    return `${s + 1}–${Math.min(s + pageSize(), t)} of ${t}${sel}`;
  };

  // 1-based adapter over the internal 0-based page, so the standalone
  // Pagination component can drive the grid directly.
  const page1 = () => page() + 1;
  page1.set = (n) => page.set(Math.max(0, n - 1));
  const totalPages = () => Math.max(1, Math.ceil(total() / (pageSize() || 1)));

  // Changing it keeps the reader where they were rather than where the old
  // page number happens to point: the first row on the screen stays put.
  const perPage = signal(pageSize.peek());
  effect(() => {
    const want = Number(perPage());
    const first = page.peek() * pageSize.peek();
    pageSize.set(want);
    page.set(want ? Math.floor(first / want) : 0);
  });
  const sizePicker = () =>
    html`<label class="zap-grid__per"><span>Rows</span>${Select({ options: sizes, value: perPage })}</label>`;

  const root = html`<div class="zap-grid">${filterable || selectable ? html`<div class="zap-grid__bar"><input class="zap-input zap-grid__filter" style="max-width:240px" placeholder="Filter…" oninput=${(e) => { filter.set(e.target.value); page.set(0); }} /><span class="zap-grid__tally">${props.note ? html`<span class="zap-grid__note">${props.note}</span>` : ""}<span class="muted" style="font-size:13px">${() => `${total()} rows`}</span></span></div>` : ""}<div class="zap-grid__scroll"><table class="zap-grid__table"><colgroup>${selectable ? html`<col style="width:44px" />` : ""}${colW.map((w) => html`<col style=${() => "width:" + w() + "px"} />`)}</colgroup><thead><tr>${selectable ? html`<th class="zap-grid__check"><input type="checkbox" checked=${() => allOnPage()} onchange=${toggleAll} /></th>` : ""}${columns.map((col, i) => headCell(col, i))}</tr></thead><tbody>${forEach(paged, bodyRow, () => html`<tr><td class="zap-grid__empty" colspan=${colspan}>No data</td></tr>`)}</tbody></table></div>${props.pageSize === 0 ? "" : html`<div class="zap-grid__foot"><span>${rangeText}</span><div class="zap-grid__foot-right">${sizes.length > 1 ? sizePicker() : ""}${Pagination({ page: page1, totalPages })}</div></div>`}</div>`;

  // The arrows need to find the cell they are stepping to, and a cell knows
  // where it sits rather than which node it is.
  table = root.querySelector(".zap-grid__table");
  return root;
}
