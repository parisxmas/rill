// The half of the catalogue that runs in the browser.
//
// The page arrives whole: the server rendered the form, a table and a pager
// from `catalogue.tmpl`, and with this file blocked or JavaScript off it stays
// a working page — every filter is a form submit and every pager step a link.
// What this adds is that none of them has to fetch a new document any more.
//
// Both halves of what you can touch are zap's own components: `Select` and
// `Button` in the toolbar, `DataGrid` for the table, and the theme they all
// read their colours from. Nothing here draws a control by hand.
//
// It is also live: a socket carries the skus on this screen to the server, and
// an edit anybody makes to one of them comes back and lights the cell.
//
// The table is editable: double-click a cell, or land on it with the arrows
// and press Enter. Which editor opens is the column's type — a number field
// for `stock`, the calendar for `added`, a Select for `role` and `region` —
// and what comes out is posted back to the document it came from.
//
// Two things divide the work, and the line between them is the interesting
// part of the example: the DATABASE holds a hundred thousand documents and
// answers which of them match, with an index behind every field the toolbar
// offers; the GRID holds one window of that answer and sorts, filters and
// pages inside it without asking anything. Narrowing a filter is a question
// for the server; sorting a column is not.
import { signal, computed, effect, hydrate, html } from "./index.js";
import { DataGrid } from "./components/grid.js";
import { Select } from "./components/select.js";
import { Button } from "./components/button.js";
import { toast, Toaster } from "./components/toast.js";

// How many rows the window holds. It is not in the URL: the URL is what the
// server renders for a browser with no JavaScript, and asking it for two
// hundred rows would mean painting two hundred rows before the grid takes
// them over — the whole table appearing and then folding into a pager. The
// page stays what it was; the window is a question this half asks on its own.
// The server caps a page at 200.
const WINDOW = 200;

// --- the state the page is in ---------------------------------------------------

// Exactly the parameters the server reads, so the URL means the same thing to
// both halves and a reload lands on what you were looking at.
const params = new URLSearchParams(location.search);
const at = (name, fallback) => signal(params.get(name) ?? fallback);
const filters = {
  role: at("role", ""),
  region: at("region", ""),
  sku: at("sku", ""),
  by: at("by", "sku"),
  dir: at("dir", "asc"),
  limit: at("limit", ""),
};
const nudge = signal(0); // the search button, for asking the same thing again

const query = computed(() => {
  const q = new URLSearchParams();
  for (const [name, s] of Object.entries(filters)) {
    const v = String(s()).trim();
    if (v) q.set(name, v);
  }
  return q.toString();
});

// The same question with a wider window, which is what this half asks for.
const windowed = computed(() => {
  const q = new URLSearchParams(query());
  q.set("limit", String(WINDOW));
  return q.toString();
});

// --- what the server said ----------------------------------------------------------

const seed = JSON.parse(document.getElementById("seed").textContent);
const result = signal(seed);
const failed = signal("");
const busy = signal(false);
let inflight = 0;

effect(() => {
  const wide = windowed();
  nudge();
  history.replaceState(null, "", query() ? "/?" + query() : "/");
  const mine = ++inflight;
  busy.set(true);
  // The page arrived with its own rows and they are on the screen already;
  // this is the rest of the window, and it lands under the same rows.
  fetch("/rows?" + wide)
    .then((r) => (r.ok ? r.json() : Promise.reject(new Error("HTTP " + r.status))))
    .then((data) => {
      if (mine !== inflight) return; // a later question has already been asked
      result.set(data);
      failed.set("");
      busy.set(false);
    })
    .catch((e) => {
      if (mine !== inflight) return;
      failed.set(String(e.message || e));
      busy.set(false);
    });
});

// --- the toolbar ---------------------------------------------------------------------

// A letter at a time is a question a keystroke apart, so typing settles before
// it is asked. A menu has nothing to settle.
const timers = {};
const typed = (name, value) => {
  clearTimeout(timers[name]);
  timers[name] = setTimeout(() => filters[name].set(value), 150);
};

// A menu is as wide as it is told to be: its list hangs under the trigger and
// takes its width, so a trigger sized to the word in it clips the words that
// are longer.
const field = (label, width, control) =>
  html`<div class="zap-field" style=${"width:" + width}>
    <span class="zap-label">${label}</span>${control}
  </div>`;

const text = (name, placeholder, width) =>
  field(
    name,
    width,
    html`<input class="zap-input" placeholder=${placeholder} value=${filters[name].peek()}
      oninput=${(e) => typed(name, e.target.value)} />`,
  );

function Filters() {
  return html`
    ${text("role", "eng, ops, sre, design", "180px")}
    ${text("region", "eu, us, apac", "150px")}
    ${text("sku", "a number", "120px")}
    ${field("sort by", "130px", Select({ options: ["sku", "score", "stock", "name"], value: filters.by }))}
    ${field("order", "110px", Select({ options: ["asc", "desc"], value: filters.dir }))}
    ${text("limit", "per page", "110px")}
    ${Button({ label: "search", onClick: () => nudge.update((n) => n + 1) })}
  `;
}

// --- the results -----------------------------------------------------------------------

const rows = computed(() => (result().docs ?? []).map((d) => ({ ...d, id: d.sku })));
const total = computed(() => result().total ?? 0);

// What the rows cost to produce, as the server measured it: the time the
// database was busy, and the time spent writing the answer out. The grid shows
// it beside the row count because it is the number this whole example is
// about — a page of a hundred thousand documents, and where its time goes.
const took = () => {
  const r = result();
  if (r.db_us == null) return "";
  return `db ${ms(r.db_us)} · wrote ${ms(r.wrote_us)}`;
};
// How many rows the grid shows at a time. This is a reading setting and
// nothing else — the menu under the table changes it, and it has nothing to
// do with `limit`, which is how much of the answer the server hands over.
const ON_SCREEN = 25;

const ms = (us) => (us >= 1000 ? (us / 1000).toFixed(1) + " ms" : us + " µs");

// One cell of one document, written back where it came from. The grid marks
// the cell while this is in the air and puts the old value back if it fails,
// so nothing on the screen claims to be saved that is not.
const save = ({ row, field, value }) =>
  fetch("/doc/" + row.sku, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ field, value }),
  }).then(async (r) => {
    const answer = await r.json().catch(() => ({}));
    // The rules are the server's. What it refuses comes back as the reason,
    // and the grid puts the old value back and shows it on the cell.
    if (!r.ok || answer.error) throw new Error(answer.error || "HTTP " + r.status);
    toast(`${field} written in ${ms(answer.db_us)}`, { kind: "success" });
  });

// What may be edited, and how. The type is what decides the editor: `stock` is
// a number, `added` opens the calendar, and `role` and `region` are lists, so
// they are the same Select the toolbar uses. `sku` is what a document is found
// by and `score` is computed, so neither is editable — and the server says the
// same, since a page is not what decides that.
// --- what everyone else is doing ----------------------------------------------
//
// One socket, and it carries as little as it can. The page says which
// documents are on its screen — not which filter it has, the documents — and
// the server tells it about those and nothing else. A change to any of the
// other ninety-nine thousand eight hundred never reaches this browser at all.
const api = {};
let live = null;
let told = "";

const follow = () => {
  const skus = (api.visible ? api.visible() : []).map((r) => r.sku);
  const say = JSON.stringify({ skus });
  if (say === told || !live || live.readyState !== 1) return;
  told = say;
  live.send(say);
};

const connect = () => {
  live = new WebSocket((location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/live");
  live.onopen = () => { told = ""; follow(); };
  live.onmessage = (e) => arrived(JSON.parse(e.data));
  // A server that restarts, a laptop that slept: come back rather than stop.
  live.onclose = () => setTimeout(connect, 1500);
};

// Somebody else's edit, on a row this page is showing. The row is changed
// where it stands and the cell says so — the grid redraws that one cell and
// nothing else.
const arrived = (change) => {
  const row = (api.visible ? api.visible() : []).find((r) => r.sku === change.sku);
  if (!row) return;
  row[change.field] = change.value;
  api.refresh && api.refresh();
  api.flash && api.flash(row, change.field);
};

effect(follow);
connect();

const columns = [
  { field: "sku", header: "sku", width: 80, align: "right" },
  {
    field: "name",
    header: "name",
    width: 200,
    editable: true,
    maxLength: 60,
    validate: (v) => (String(v).trim() ? null : "a name cannot be empty"),
  },
  { field: "role", header: "role", width: 120, editable: true, options: ["eng", "ops", "sre", "design"] },
  { field: "region", header: "region", width: 120, editable: true, options: ["eu", "us", "apac"] },
  { field: "score", header: "score", width: 100, align: "right", format: (v) => v.toFixed(2) },
  {
    field: "stock",
    header: "stock",
    width: 90,
    align: "right",
    editable: true,
    type: "number",
    min: 0,
    max: 99999,
    validate: (v) => (v == null ? "stock is a number" : null),
  },
  { field: "added", header: "added", width: 150, editable: true, type: "date" },
];

// The grid holds a window and says so: a filter matching more than it can hold
// is a question for the toolbar above rather than for the grid's own search.
function Results() {
  return html`
    <p>
      <b>${total}</b> documents match${() =>
        total() > rows().length ? ` — the grid holds the first ${rows().length}` : ""}${() =>
        busy() ? " · asking…" : ""}
    </p>
    ${() =>
      failed()
        ? html`<p class="bad">${failed}</p>`
        : DataGrid({
            rows,
            columns,
            pageSize: ON_SCREEN,
            pageSizes: [10, 25, 50, 100],
            onCellEdit: save,
            note: took,
            apiRef: api,
          })}
    ${Toaster()}
  `;
}

// --- the takeover -------------------------------------------------------------------

const form = document.getElementById("filters");
form.addEventListener("submit", (e) => e.preventDefault());
hydrate(Filters, form);

const region = document.getElementById("results");
hydrate(Results, region);

// Both halves are in; the page may show what is there.
form.classList.add("live");
region.classList.add("live");
