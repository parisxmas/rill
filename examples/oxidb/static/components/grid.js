/* Vendored from zap (https://github.com/barisakin/zap), MIT. The grouping —
   `groupable`, `groupBy`, `source` and the group panel — was added here and
   is to go back upstream. The catalogue serves it, so the example stands on
   its own. */
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
//   • grouping: drag a column header onto the panel above the table and the
//     rows fold into groups, one level per header dropped; a group opens on
//     a click to the level under it, and the last level opens to the rows.
//     The grid holds none of it — every level is a question to `source`,
//     paged on its own — so the groups are what the whole data set comes
//     to and not what one window of it happens to hold
//   • empty state
//
// Props:
//   rows:     Array | signal<Array>   row objects (give each a stable `id`)
//   columns:  Array<{
//               field, header,
//               width?:     number (px, resizable),
//               align?:     "right",
//               sortable?:  boolean (default true),
//               groupable?: boolean (default true when the grid is groupable),
//               aggregate?: "sum" | "avg" | "min" | "max" | "count" — what a
//                           group row shows in this column, read from the
//                           group object under the column's field
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
//   groupable?:  boolean — headers can be dragged onto a group panel
//   groupBy?:    signal<string[]> — the grouped fields, outermost first; the
//                grid makes its own when none is given
//   source?:     (ask) => Promise<{rows, total}> — where a grouped grid gets
//                its rows. `ask` is {groupBy, keys, page, pageSize, sort}:
//                `keys` is the path to the level being asked for, as
//                [{field, value}] outermost first; when it is shorter than
//                `groupBy` the answer's rows are GROUP objects — {key, count,
//                ...} with an aggregate under each field a column asks for —
//                and when it is as long, they are the rows themselves. `page`
//                is 1-based and `sort` is {field, dir} or null, where the
//                field `key` is the group column. Without `source`, grouping
//                is off whatever `groupBy` says
//   onSelectionChange?: (rows[]) => void
//   onCellEdit?: ({row, field, value, oldValue}) => void | Promise
//   apiRef?:     an object the grid fills in — `refresh()` redraws cells after
//                the host has changed a row in place, `visible()` is the rows
//                on the screen right now, `flash(row, field)` lights one cell,
//                `reload()` asks `source` again for every open level
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
const thousands = (n) => (typeof n === "number" ? n.toLocaleString("en-US") : String(n ?? ""));

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
  // A width per column, kept by the column rather than by its place, because
  // grouping takes columns out of the row and puts one of its own in front.
  const widthOf = new Map(columns.map((c) => [c, signal(c.width ?? 150)]));

  const rowsArr = () => (typeof props.rows === "function" ? props.rows() : props.rows || []);

  const sortCol = signal(null);
  const sortDir = signal(1);
  const filter = signal("");
  const editing = signal(null); // { row, col } currently being edited
  const rev = signal(0);        // bumped after an edit so cell displays refresh
  const page = signal(0);
  const selected = signal(new Set()); // row object refs — stable across sort/filter

  // --- grouping ---------------------------------------------------------------
  //
  // The grouped fields, outermost first. While there are any and there is a
  // `source` to ask, the table is a tree rather than a list: a page of groups
  // at the top, each opening to the level under it, and the last level
  // opening to rows. Nothing of it is computed here — every level is one
  // question to the source, with the path down to it as the keys — so the
  // counts are the data set's own, and a page of groups is paged the way a
  // page of rows is.
  const source = props.source || null;
  const groupBy = props.groupBy ?? signal([]);
  const grouped = () => !!source && groupBy().length > 0;
  const labelOf = (f) => columns.find((c) => c.field === f)?.header ?? f;
  // The column that stands in front of a grouped table: the key, the count,
  // and the caret. Sorting by it is sorting by the key.
  const GROUP = { field: "key", header: () => groupBy().map(labelOf).join(" › "), group: true };
  widthOf.set(GROUP, signal(props.groupWidth ?? 240));
  const shown = computed(() => (grouped() ? [GROUP, ...columns.filter((c) => !groupBy().includes(c.field))] : columns));
  const setGroupBy = (list) => {
    groupBy.set(list);
    page.set(0);
  };

  // The tree. A node is one group — its path, the group object the source
  // answered with, and the level under it once it has been opened. The root
  // is the level with no path: the top page of groups. Which groups are open
  // is remembered by path rather than by node, so a reload — a sort, a page,
  // a change to what is grouped — builds new nodes and opens the same ones.
  const busy = signal(false);
  const openPaths = new Set();
  const pathKey = (path) => path.map((k) => k.field + "=" + String(k.value)).join("/");
  const top = { path: [], depth: -1, rows: signal([]), total: signal(0), asked: 0, busy, failed: signal("") };
  const makeNode = (parent, g) => {
    const path = [...parent.path, { field: groupBy.peek()[parent.depth + 1], value: g.key }];
    const key = pathKey(path);
    const node = {
      id: "g:" + key, kind: "group", path, depth: parent.depth + 1, group: g,
      open: signal(openPaths.has(key)),
      rows: signal([]), total: signal(0), page: signal(1), busy: signal(false), failed: signal(""), asked: 0,
    };
    node.more = { id: "m:" + key, kind: "more", node };
    if (node.open.peek()) load(node);
    return node;
  };
  // One level, asked for. A later question to the same node supersedes an
  // earlier one, whatever order the answers arrive in.
  const load = (node) => {
    const mine = ++node.asked;
    const gb = groupBy.peek();
    const leaves = node.path.length >= gb.length;
    const sc = sortCol.peek();
    node.busy.set(true);
    Promise.resolve()
      .then(() =>
        source({
          groupBy: gb,
          keys: node.path,
          page: node === top ? page.peek() + 1 : node.page.peek(),
          pageSize: pageSize.peek() || 0,
          sort: sc ? { field: sc.field, dir: sortDir.peek() } : null,
        }),
      )
      .then(
        (a) => {
          if (mine !== node.asked) return;
          const rows = a?.rows ?? [];
          node.total.set(a?.total ?? rows.length);
          node.rows.set(
            leaves
              ? rows.map((row, i) => ({ id: "l:" + pathKey(node.path) + "/" + (row.id ?? i), kind: "leaf", row, depth: node.depth + 1 }))
              : rows.map((g) => makeNode(node, g)),
          );
          node.failed.set("");
          node.busy.set(false);
        },
        (e) => {
          if (mine !== node.asked) return;
          node.failed.set(String(e?.message ?? e));
          node.busy.set(false);
        },
      );
  };
  const toggle = (node) => {
    const on = !node.open.peek();
    node.open.set(on);
    const key = pathKey(node.path);
    if (on) openPaths.add(key);
    else openPaths.delete(key);
    if (on && node.asked === 0) load(node);
  };
  const reload = signal(0);
  // The top level follows everything that changes what it is: the grouping,
  // the page, the page size, the sort. The levels under it are asked for as
  // they are opened and paged, and built again with the top.
  effect(() => {
    if (!grouped()) return;
    groupBy(); page(); pageSize(); sortCol(); sortDir(); reload();
    load(top);
  });
  // The tree laid out as the rows of the table: a group, then what is under
  // it if it is open, then the pager of that level if it needs one.
  const display = computed(() => {
    const out = [];
    const size = pageSize();
    const walk = (node) => {
      for (const it of node.rows()) {
        out.push(it);
        if (it.kind === "group" && it.open()) {
          walk(it);
          if ((size && it.total() > size) || it.failed() || (it.busy() && it.rows().length === 0)) out.push(it.more);
        }
      }
    };
    walk(top);
    return out;
  });

  // Optional imperative handle: let the host refresh default cells in place after it
  // mutates a row object (e.g. an API save), avoiding a full data reload/rebuild.
  if (props.apiRef) {
    props.apiRef.refresh = () => rev.update((n) => n + 1);
    props.apiRef.reload = () => reload.update((n) => n + 1);
    // What is on the screen, for a host that wants to follow only these —
    // reading it inside an effect follows the page, the sort and the filter.
    // In a grouped table it is the rows, not the groups around them.
    props.apiRef.visible = () => (grouped() ? display().filter((x) => x.kind === "leaf").map((x) => x.row) : paged());
    // Say that one cell has just changed under the reader's eyes.
    //
    // It is held as state rather than written onto the node: a cell's class is
    // computed from what the grid knows, so a class put there by hand is wiped
    // by the next redraw — and a table where somebody is editing redraws all
    // the time.
    //
    // Two names taken in turn, because a class removed and put back inside one
    // frame is a class that never went away as far as the browser is
    // concerned, and the animation carries on from where it was instead of
    // starting over. A different name is a different animation.
    props.apiRef.flash = (row, field) => {
      const col = columns.find((c) => c.field === field);
      if (!col) return;
      const had = markOf(row, col);
      const which = had && had.fresh === 1 ? 2 : 1;
      mark(row, col, { fresh: which });
      setTimeout(() => {
        const now = markOf(row, col);
        if (now && now.fresh === which) mark(row, col, null);
      }, 1600);
    };
  }

  // sort + filter pipeline (recomputes only when an input changes)
  const processed = computed(() => {
    let r = rowsArr();
    const f = filter().trim().toLowerCase();
    if (f) r = r.filter((row) => columns.some((c) => String(getVal(row, c) ?? "").toLowerCase().includes(f)));
    const sc = sortCol();
    if (sc && !sc.group) {
      const dir = sortDir();
      r = [...r].sort((a, b) => cmp(getVal(a, sc), getVal(b, sc)) * dir);
    }
    return r;
  });
  // In a grouped table the total is the count of top-level groups, which is
  // what the pager under it steps through.
  const total = computed(() => (grouped() ? top.total() : processed().length));
  const maxPage = () => (pageSize() ? Math.max(0, Math.ceil(total() / pageSize()) - 1) : 0);
  const paged = computed(() => {
    if (grouped()) return display();
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

  const startResize = (e, col) => {
    e.preventDefault(); e.stopPropagation();
    const w = widthOf.get(col);
    const startX = e.clientX;
    const startW = w.peek();
    const move = (ev) => w.set(Math.max(60, startW + (ev.clientX - startX)));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const colspan = () => shown().length + (selectable ? 1 : 0);

  // --- the group panel ---------------------------------------------------------
  //
  // A header is dragged onto the panel and its field joins the grouping; a
  // chip is dragged onto another to put it before that one, or back onto the
  // table to take it out. What is being dragged is held here rather than read
  // off the event, because the browser only hands the data over on the drop
  // and the panel wants to light up before that.
  const groupable = !!props.groupable && !!source;
  let dragging = null; // { field, chip: boolean }
  const over = signal(false);
  const canGroup = (col) => groupable && !col.group && col.groupable !== false && !!col.field;
  const dragStart = (e, field, chip) => {
    dragging = { field, chip };
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", field);
  };
  const dragEnd = () => { dragging = null; over.set(false); };
  // Dropped on the panel, or on the chip it should go in front of.
  const dropOn = (e, before) => {
    e.preventDefault(); e.stopPropagation();
    over.set(false);
    const d = dragging;
    dragging = null;
    if (!d || d.field === before) return;
    const list = groupBy().filter((f) => f !== d.field);
    const at = before ? list.indexOf(before) : list.length;
    list.splice(at < 0 ? list.length : at, 0, d.field);
    setGroupBy(list);
  };
  // Dropped back on the table: a chip leaves the grouping.
  const dropOut = (e) => {
    if (!dragging || !dragging.chip) return;
    e.preventDefault();
    const f = dragging.field;
    dragging = null;
    setGroupBy(groupBy().filter((x) => x !== f));
  };
  const chip = (f) =>
    html`<span class="zap-grid__chip" draggable=${true} ondragstart=${(e) => dragStart(e, f, true)} ondragend=${dragEnd} ondragover=${(e) => { if (dragging) e.preventDefault(); }} ondrop=${(e) => dropOn(e, f)}><span>${labelOf(f)}</span><button type="button" class="zap-grid__chip-x" title="ungroup" onclick=${() => setGroupBy(groupBy().filter((x) => x !== f))}>×</button></span>`;
  const panel = () =>
    html`<div class=${() => "zap-grid__groups" + (over() ? " is-over" : "")}
      ondragover=${(e) => { if (dragging) { e.preventDefault(); e.dataTransfer.dropEffect = "move"; over.set(true); } }}
      ondragleave=${(e) => { if (!e.currentTarget.contains(e.relatedTarget)) over.set(false); }}
      ondrop=${(e) => dropOn(e, null)}>${() =>
        groupBy().length
          ? groupBy().map((f, i) => [i ? html`<span class="zap-grid__chip-sep">›</span>` : "", chip(f)])
          : html`<span class="zap-grid__groups-hint">Drag a column header here to group by it</span>`}</div>`;

  const headCell = (col) =>
    html`<th class="zap-grid__hcell"><div class=${"zap-grid__th" + (col.sortable === false ? "" : " is-sortable")} draggable=${canGroup(col)} ondragstart=${(e) => dragStart(e, col.field, false)} ondragend=${dragEnd} onclick=${() => toggleSort(col)}><span>${col.header}</span><span class="zap-grid__caret">${() => (sortCol() === col ? (sortDir() === 1 ? "▲" : "▼") : "")}</span></div><span class="zap-grid__resize" onclick=${(e) => e.stopPropagation()} onpointerdown=${(e) => startResize(e, col)}></span></th>`;

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
  // same coordinates. A place with no cell — a group row, the group column —
  // is stepped over.
  const focused = signal(null); // { r, c }
  let table = null;
  const cellAt = (r, c) => table && table.querySelector(`td[data-r="${r}"][data-c="${c}"]`);
  const step = (dr, dc) => {
    const f = focused();
    if (!f) return;
    const n = paged().length;
    const m = shown.peek().length;
    let r = f.r;
    let c = f.c;
    if (dr) { do { r += dr; } while (r >= 0 && r < n && !cellAt(r, c)); }
    else if (dc) { do { c += dc; } while (c >= 0 && c < m && !cellAt(r, c)); }
    const td = cellAt(r, c) || cellAt(f.r, f.c);
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
    // `td` is assigned below; the class effect runs once while it is being
    // built, when a focused cell elsewhere (a group row just closed, say)
    // must not reach for it.
    let td = null;
    const classes = () => {
      const m = (rev(), markOf(row, col));
      const f = focused();
      const here = f && td && f.r === Number(td.dataset.r) && f.c === ci;
      return [
        col.align === "right" ? "zap-grid__num" : "",
        editable(col) ? "zap-grid__editable" : "",
        m && m.saving ? "is-saving" : "",
        m && m.error ? "is-invalid" : "",
        m && m.fresh === 1 ? "is-fresh" : "",
        m && m.fresh === 2 ? "is-fresh-b" : "",
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
    td = html`<td class=${classes} tabindex=${editable(col) ? "0" : "-1"}
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

  // --- the rows ---------------------------------------------------------------
  //
  // Three shapes, told apart by `kind`: a group, the pager under an open
  // group, and a row — which in a grouped table arrives wrapped with its
  // depth, and in a flat one is the row itself. A row is drawn once, for the
  // columns shown at the time; the columns only change with the grouping,
  // and a change to the grouping asks the source again and hands back new
  // objects, so nothing is ever drawn for columns it no longer has.
  const indent = (depth) => "padding-left:" + (12 + Math.max(0, depth) * 18) + "px";

  const groupRow = (node) => {
    const g = node.group;
    return html`<tr class="zap-grid__grow" onclick=${() => toggle(node)}>${selectable ? html`<td class="zap-grid__check"></td>` : ""}${shown.peek().map((col) =>
      col.group
        ? html`<td class="zap-grid__gcell" style=${indent(node.depth)}><span class="zap-grid__twist">${() => (node.busy() ? "…" : node.open() ? "▾" : "▸")}</span><span class="zap-grid__gkey">${String(g.key ?? "—")}</span><span class="zap-grid__gcount">(${thousands(g.count)})</span>${() => (node.failed() && !node.open() ? html`<span class="zap-grid__bad"> ${node.failed()}</span>` : "")}</td>`
        : html`<td class=${"zap-grid__agg" + (col.align === "right" ? " zap-grid__num" : "")} title=${col.aggregate ? `${col.aggregate} of ${col.header}` : ""}>${col.aggregate && g[col.field] != null ? (col.format ? col.format(g[col.field], g) : g[col.field]) : ""}</td>`,
    )}</tr>`;
  };

  const rangeOf = (node) => {
    const t = node.total();
    const size = pageSize() || t || 1;
    const s = (node.page() - 1) * size;
    return t ? `${thousands(s + 1)}–${thousands(Math.min(s + size, t))} of ${thousands(t)}` : "nothing under it";
  };
  const moreRow = (item) => {
    const node = item.node;
    const pages = () => Math.max(1, Math.ceil(node.total() / (pageSize() || 1)));
    return html`<tr class="zap-grid__more"><td colspan=${colspan} style=${indent(node.depth + 1)}><span class="zap-grid__more-range">${() =>
      node.failed() ? html`<span class="zap-grid__bad">${node.failed()} · </span>` : node.busy() ? "asking… " : ""}${() => rangeOf(node)}</span>${() =>
      pageSize() && node.total() > pageSize() ? Pagination({ page: node.page, totalPages: pages, onChange: () => load(node) }) : ""}</td></tr>`;
  };

  const bodyRow = (item, i) => {
    if (item.kind === "group") return groupRow(item);
    if (item.kind === "more") return moreRow(item);
    const row = item.kind === "leaf" ? item.row : item;
    const depth = item.kind === "leaf" ? item.depth : 0;
    return html`<tr class=${() => (selectable && isSel(row) ? "is-selected" : "")}>${selectable ? html`<td class="zap-grid__check"><input type="checkbox" checked=${() => isSel(row)} onchange=${() => toggleRow(row)} /></td>` : ""}${shown.peek().map((col, ci) => {
      if (col.group) return html`<td class="zap-grid__gutter" style=${indent(depth)}></td>`;
      const td = cell(row, col, ci);
      td.dataset.r = String(i);
      return td;
    })}</tr>`;
  };

  const rangeText = () => {
    const t = total();
    const what = grouped() ? " groups" : "";
    const sel = selectable && selected().size ? `  ·  ${selected().size} selected` : "";
    if (!t) return (grouped() ? "0 groups" : "0 rows") + sel;
    if (!pageSize()) return `${t} rows${sel}`;
    const s = Math.min(page(), maxPage()) * pageSize();
    return `${s + 1}–${Math.min(s + pageSize(), t)} of ${thousands(t)}${what}${sel}`;
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

  const tally = () => `${thousands(total())} ${grouped() ? "groups" : "rows"}${grouped() && busy() ? " · asking…" : ""}`;

  const root = html`<div class="zap-grid">${filterable || selectable ? html`<div class="zap-grid__bar"><input class="zap-input zap-grid__filter" style=${() => (grouped() ? "max-width:240px;visibility:hidden" : "max-width:240px")} placeholder="Filter…" oninput=${(e) => { filter.set(e.target.value); page.set(0); }} /><span class="zap-grid__tally">${props.note ? html`<span class="zap-grid__note">${props.note}</span>` : ""}<span class="muted" style="font-size:13px">${tally}</span></span></div>` : ""}${groupable ? panel() : ""}<div class="zap-grid__scroll" ondragover=${(e) => { if (dragging && dragging.chip) e.preventDefault(); }} ondrop=${dropOut}><table class="zap-grid__table"><colgroup>${selectable ? html`<col style="width:44px" />` : ""}${() => shown().map((col) => html`<col style=${() => "width:" + widthOf.get(col)() + "px"} />`)}</colgroup><thead><tr>${selectable ? html`<th class="zap-grid__check"><input type="checkbox" checked=${() => allOnPage()} onchange=${toggleAll} /></th>` : ""}${() => shown().map((col) => headCell(col))}</tr></thead><tbody>${forEach(paged, bodyRow, () => html`<tr><td class="zap-grid__empty" colspan=${colspan}>${() => (grouped() && busy() ? "asking…" : grouped() && top.failed() ? top.failed() : "No data")}</td></tr>`)}</tbody></table></div>${props.pageSize === 0 ? "" : html`<div class="zap-grid__foot"><span>${rangeText}</span><div class="zap-grid__foot-right">${sizes.length > 1 ? sizePicker() : ""}${Pagination({ page: page1, totalPages })}</div></div>`}</div>`;

  // The arrows need to find the cell they are stepping to, and a cell knows
  // where it sits rather than which node it is.
  table = root.querySelector(".zap-grid__table");
  return root;
}
