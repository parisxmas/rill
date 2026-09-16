// The page: a file picked on the left, its diagrams in the middle, its
// code on the right, and every diagram symbol a click away from the line
// it came from — and back, since the cursor's line lights the symbols
// written there. The server does what the compiler does.

(() => {
  const $ = id => document.getElementById(id);
  const files = $("files"), diagrams = $("diagrams"), drawn = $("drawn"), editor = $("editor"), gutter = $("gutter"), colors = $("colors");
  const problems = $("problems"), status = $("status"), fileName = $("file-name"), dirty = $("dirty");

  let current = null;       // the path of the file shown
  let protectedFiles = new Set();   // the examples: edited and run as they stand, saved under another name
  let readOnly = false;             // the site saves nothing: everything is edited and run as it stands
  const isProtected = () => readOnly || protectedFiles.has(current);
  // What the compiler works on: the file as saved, or — for an example —
  // the buffer, sent along, so nothing of it is written back.
  const subject = () => isProtected() ? editor.value : null;
  const verb = () => isProtected() ? "POST" : "GET";
  let model = null;         // its model as last drawn
  let saved = "";           // its text as last saved
  let notesByLine = {};     // line -> [problem]

  const api = async (method, path, body) => {
    const res = await fetch(path, { method, body, headers: body != null ? { "Content-Type": "text/plain" } : {} });
    const text = await res.text();
    let data = null;
    try { data = JSON.parse(text); } catch (_) { data = { text }; }
    return { ok: res.ok, status: res.status, data, text };
  };

  const say = msg => { status.textContent = msg; };

  // ---- files ---------------------------------------------------------------

  async function loadFiles() {
    const all = $("all-files").checked ? "?all=1" : "";
    const { data } = await api("GET", "/api/files" + all);
    protectedFiles = new Set(data.protected || []);
    readOnly = !!data.readonly;
    $("btn-save").hidden = readOnly;
    $("btn-new").hidden = readOnly;
    const list = (data.files || []).sort();
    files.innerHTML = "";
    let lastDir = null;
    for (const f of list) {
      const dir = f.includes("/") ? f.slice(0, f.lastIndexOf("/")) : ".";
      if (dir !== lastDir) {
        const d = document.createElement("div");
        d.className = "dir";
        d.textContent = dir;
        files.appendChild(d);
        lastDir = dir;
      }
      const item = document.createElement("div");
      item.className = "file";
      item.textContent = f.slice(f.lastIndexOf("/") + 1);
      item.title = protectedFiles.has(f) ? f + " — an example: edit and run it as you like, save it under another name" : f;
      if (protectedFiles.has(f)) item.classList.add("example");
      item.dataset.path = f;
      item.onclick = () => open(f);
      files.appendChild(item);
    }
  }

  async function open(path) {
    if (current && editor.value !== saved && !confirm("Leave " + current + " without saving?")) return;
    const { ok, data } = await api("GET", "/api/file?path=" + encodeURIComponent(path));
    if (!ok) { say(data.error || "cannot read " + path); return; }
    current = path;
    saved = data.text;
    editor.value = data.text;
    renderColors();
    fileName.textContent = path + (readOnly ? "" : protectedFiles.has(path) ? "  (example)" : "");
    $("btn-save").textContent = protectedFiles.has(path) ? "Save as…" : "Save";
    for (const item of files.querySelectorAll(".file")) item.classList.toggle("current", item.dataset.path === path);
    history.replaceState(null, "", "#" + path);
    markDirty();
    renderGutter();
    await refresh();
  }

  function markDirty() { dirty.textContent = editor.value === saved ? "" : readOnly ? "changed here only" : isProtected() ? "changed — Save as… keeps it" : "unsaved"; }

  // The file as the server wrote it after an edit made on a diagram.
  async function reloadFile(text) {
    if (!isProtected()) saved = text;   // an example's edits live in the buffer
    editor.value = text;
    renderColors();
    markDirty();
    renderGutter();
    await refresh();
  }

  async function save() {
    if (!current) return;
    if (readOnly) return;
    if (isProtected()) return saveAs();
    const { ok, data } = await api("PUT", "/api/file?path=" + encodeURIComponent(current), editor.value);
    if (!ok) { say(data.error || "cannot save"); return; }
    saved = editor.value;
    markDirty();
    say("saved " + current);
    await refresh();
  }

  // An example kept: the buffer written under a name of the user's, and
  // that file opened in its place.
  async function saveAs() {
    const name = prompt("Save this model as, under the root:", "models/" + current.slice(current.lastIndexOf("/") + 1).replace(/\.rill$/, "") + "_mine.rill");
    if (!name) return;
    const path = name.endsWith(".rill") ? name : name + ".rill";
    if (protectedFiles.has(path)) { say(path + " is an example; pick another name"); return; }
    const text = editor.value;
    const { ok, data } = await api("PUT", "/api/file?path=" + encodeURIComponent(path), text);
    if (!ok) { say(data.error || "cannot save " + path); return; }
    saved = text;   // so the switch does not ask about leaving
    await loadFiles();
    await open(path);
    say("saved " + path);
  }

  // ---- the diagrams and the problems ----------------------------------------

  // The model and the check, both from the file as saved.
  async function refresh() {
    if (!current) return;
    notesByLine = {};
    const check = await api(verb(), "/api/check?path=" + encodeURIComponent(current), subject());
    const list = (check.data.problems || []);
    problems.innerHTML = "";
    for (const p of list) {
      (notesByLine[p.line] ||= []).push(p);
      const li = document.createElement("li");
      li.className = p.kind;
      const glyph = p.kind === "error" ? "✕" : p.kind === "warning" ? "!" : "i";
      li.innerHTML = `<span class="icon ${p.kind}" title="${p.kind}">${glyph}</span><span class="line">${p.line ? "line " + p.line : ""}</span>`;
      li.appendChild(document.createTextNode(p.text));
      li.onclick = () => goToLine(p.line);
      problems.appendChild(li);
    }
    renderGutter();
    const model = await api(verb(), "/api/model?path=" + encodeURIComponent(current), subject());
    drawn.innerHTML = "";
    const empty = diagrams.querySelector(".empty");
    if (!model.ok) {
      empty.textContent = model.data.problems ? "The file does not parse; the problems are under the code." : (model.data.error || "nothing to draw");
      empty.hidden = false;
      $("editbar").hidden = true;
      Simulate.load(current, null, subject());
      say(list.length ? list.length + " problem(s)" : "");
      return;
    }
    const m = model.data;
    window.__model = m;
    Simulate.load(current, m, subject());
    const noteCount = {};
    for (const line in notesByLine) noteCount[line] = notesByLine[line].filter(p => p.kind === "note").length || undefined;
    for (const sd of m.systems || []) {
      const h = document.createElement("h2");
      h.innerHTML = `<span class="kind">system</span>${sd.name}`;
      drawn.appendChild(h);
      drawn.appendChild(Diagram.system(sd));
    }
    for (const mc of m.machines || []) {
      const h = document.createElement("h2");
      h.innerHTML = `<span class="kind">machine</span>${mc.name}`;
      drawn.appendChild(h);
      drawn.appendChild(Diagram.process(mc, noteCount, true));
    }
    const nothing = !(m.systems || []).length && !(m.machines || []).length;
    empty.textContent = "No system or machine in this file yet: add a machine above, or write one on the right.";
    empty.hidden = !nothing;
    $("editbar").hidden = false;
    for (const g of drawn.querySelectorAll(".sym[data-line]")) {
      g.addEventListener("click", () => goToLine(Number(g.dataset.line)));
    }
    Edit.attach(drawn, m, isProtected() ? editor.value : saved, current, reloadFile, isProtected());
    say(list.length ? list.filter(p => p.kind === "error").length + " error(s), " + list.filter(p => p.kind !== "error").length + " note(s)" : "compiles cleanly");
    lightLine(cursorLine());
  }

  // ---- the code ---------------------------------------------------------------

  // The colours behind the text: the same characters, coloured, kept in
  // step with the textarea's scroll.
  function renderColors() {
    colors.innerHTML = Highlight.html(editor.value) + "\n";
    syncScroll();
  }

  function syncScroll() {
    gutter.scrollTop = editor.scrollTop;
    colors.style.transform = `translate(${-editor.scrollLeft}px, ${-editor.scrollTop}px)`;
  }

  function renderGutter() {
    const n = editor.value.split("\n").length;
    const lit = cursorLine();
    const parts = [];
    for (let i = 1; i <= n; i++) {
      const ps = notesByLine[i] || [];
      const cls = [i === lit ? "lit" : "", ps.some(p => p.kind === "error") ? "err" : ps.length ? "note" : ""].filter(Boolean).join(" ");
      parts.push(cls ? `<span class="${cls}">${i}</span>` : String(i));
    }
    gutter.innerHTML = parts.join("\n");
    gutter.scrollTop = editor.scrollTop;
  }

  function cursorLine() {
    return editor.value.slice(0, editor.selectionStart).split("\n").length;
  }

  function goToLine(line) {
    if (!line) return;
    const lines = editor.value.split("\n");
    let pos = 0;
    for (let i = 0; i < line - 1 && i < lines.length; i++) pos += lines[i].length + 1;
    editor.focus();
    editor.setSelectionRange(pos, pos + (lines[line - 1] || "").length);
    const lineHeight = 18;
    editor.scrollTop = Math.max(0, (line - 6) * lineHeight);
    renderGutter();
    lightLine(line);
  }

  // The symbols written on this line, lit; the first scrolled into view.
  function lightLine(line) {
    let first = null;
    for (const g of diagrams.querySelectorAll(".sym[data-line]")) {
      const hit = Number(g.dataset.line) === line;
      g.classList.toggle("lit", hit);
      if (hit && !first) first = g;
    }
    if (first && !document.activeElement.isSameNode(diagrams)) first.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  editor.addEventListener("input", () => { markDirty(); renderGutter(); renderColors(); });
  editor.addEventListener("scroll", syncScroll);
  for (const ev of ["click", "keyup"]) editor.addEventListener(ev, () => { renderGutter(); lightLine(cursorLine()); });
  editor.addEventListener("keydown", e => {
    if (e.key === "Tab") {
      e.preventDefault();
      const s = editor.selectionStart;
      editor.setRangeText("  ", s, editor.selectionEnd, "end");
      markDirty();
      renderGutter();
      renderColors();
    }
  });

  // ---- the language server: hover, and a definition on ⌘-click ----------------

  const tip = document.createElement("div");
  tip.className = "hovertip";
  tip.hidden = true;
  document.body.appendChild(tip);
  let hoverTimer = null, hoverAt = null, charWidth = 0;

  // Where the mouse is in the text: the textarea is monospace, so a
  // position is a division.
  function positionAt(e) {
    if (!charWidth) {
      const probe = document.createElement("span");
      probe.textContent = "x".repeat(100);
      probe.style.font = getComputedStyle(editor).font;
      probe.style.position = "absolute";
      probe.style.visibility = "hidden";
      document.body.appendChild(probe);
      charWidth = probe.getBoundingClientRect().width / 100;
      probe.remove();
    }
    const rect = editor.getBoundingClientRect();
    const lineHeight = parseFloat(getComputedStyle(editor).lineHeight) || 18;
    const line = Math.floor((e.clientY - rect.top - 8 + editor.scrollTop) / lineHeight);
    const character = Math.floor((e.clientX - rect.left - 10 + editor.scrollLeft) / charWidth);
    return { line: Math.max(0, line), character: Math.max(0, character) };
  }

  async function askServer(method, at) {
    const res = await fetch(`/api/lsp?path=${encodeURIComponent(current)}&method=${method}&line=${at.line}&character=${at.character}`, { method: "POST", body: editor.value });
    if (!res.ok) return null;
    const data = await res.json();
    return data.result || null;
  }

  editor.addEventListener("mousemove", e => {
    if (!current) return;
    const at = positionAt(e);
    if (hoverAt && hoverAt.line === at.line && hoverAt.character === at.character) return;
    hoverAt = at;
    tip.hidden = true;
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(async () => {
      const r = await askServer("hover", at);
      if (!r || !r.contents || !hoverAt || hoverAt.line !== at.line) return;
      tip.textContent = r.contents.value.replace(/^```rill\n|\n```$/g, "");
      tip.style.left = (e.clientX + 12) + "px";
      tip.style.top = (e.clientY + 16) + "px";
      tip.hidden = false;
    }, 250);
  });
  for (const ev of ["mouseleave", "keydown", "scroll"]) editor.addEventListener(ev, () => { tip.hidden = true; hoverAt = null; clearTimeout(hoverTimer); });
  editor.addEventListener("click", async e => {
    if (!(e.metaKey || e.ctrlKey) || !current) return;
    const r = await askServer("definition", positionAt(e));
    if (r && r.range) goToLine(r.range.start.line + 1);
    else say("no definition here");
  });

  // ---- zoom: every diagram scaled from its natural size -------------------------

  let zoom = 1;
  try { zoom = Number(localStorage.getItem("rill-zoom")) || 1; } catch (_) {}

  function applyZoom() {
    for (const svg of document.querySelectorAll("#diagrams svg, #sim-diagrams svg")) {
      if (!svg.dataset.naturalWidth) {
        svg.dataset.naturalWidth = svg.getAttribute("width");
        svg.dataset.naturalHeight = svg.getAttribute("height");
      }
      svg.style.width = (Number(svg.dataset.naturalWidth) * zoom) + "px";
      svg.style.height = (Number(svg.dataset.naturalHeight) * zoom) + "px";
      svg.style.maxWidth = "none";
    }
    $("zoom-reset").textContent = Math.round(zoom * 100) + "%";
    try { localStorage.setItem("rill-zoom", String(zoom)); } catch (_) {}
  }

  function setZoom(z) {
    zoom = Math.min(3, Math.max(0.25, Math.round(z * 100) / 100));
    applyZoom();
  }
  $("zoom-in").onclick = () => setZoom(zoom * 1.2);
  $("zoom-out").onclick = () => setZoom(zoom / 1.2);
  $("zoom-reset").onclick = () => setZoom(1);
  document.addEventListener("keydown", e => {
    if (!e.altKey) return;
    if (e.key === "=" || e.key === "+" || e.key === "≠" || e.key === "±") { e.preventDefault(); setZoom(zoom * 1.2); }
    if (e.key === "-" || e.key === "–") { e.preventDefault(); setZoom(zoom / 1.2); }
    if (e.key === "0" || e.key === "º") { e.preventDefault(); setZoom(1); }
  });
  // ⌘/ctrl and the wheel over a diagram zooms it, as a map does
  for (const id of ["diagrams", "sim-diagrams"]) {
    $(id).addEventListener("wheel", e => {
      if (!(e.metaKey || e.ctrlKey)) return;
      e.preventDefault();
      setZoom(e.deltaY < 0 ? zoom * 1.1 : zoom / 1.1);
    }, { passive: false });
  }
  // new diagrams take the zoom they are drawn under
  new MutationObserver(() => applyZoom()).observe($("diagrams"), { childList: true });
  new MutationObserver(() => applyZoom()).observe($("sim-diagrams"), { childList: true });

  // ---- panes under a handle: their height dragged, and kept -------------------

  // A pane at the bottom of a column, resized by the handle above it.
  function resizable(handleId, pane, within, key) {
    const handle = $(handleId);
    let start = null;
    try { const h = Number(localStorage.getItem(key)); if (h) pane.style.height = h + "px"; } catch (_) {}
    handle.addEventListener("mousedown", e => {
      start = { y: e.clientY, h: pane.getBoundingClientRect().height };
      document.body.style.cursor = "row-resize";
      document.body.style.userSelect = "none";
      e.preventDefault();
    });
    document.addEventListener("mousemove", e => {
      if (!start) return;
      const most = within.getBoundingClientRect().height * 0.8;
      const h = Math.max(24, Math.min(most, start.h + (start.y - e.clientY)));
      pane.style.height = h + "px";
    });
    document.addEventListener("mouseup", () => {
      if (!start) return;
      start = null;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      try { localStorage.setItem(key, String(Math.round(pane.getBoundingClientRect().height))); } catch (_) {}
    });
  }
  resizable("problems-handle", problems, document.querySelector(".code"), "rill-problems-height");
  resizable("output-handle", $("sim-output"), $("simulate"), "rill-output-height");

  // A pane beside a handle, resized sideways; `side` says which side of
  // the handle the pane is on.
  function splitter(handleId, pane, within, key, side, onClick) {
    const handle = $(handleId);
    let start = null;
    try { const w = Number(localStorage.getItem(key)); if (w) { pane.style.flex = "none"; pane.style.width = w + "px"; } } catch (_) {}
    handle.addEventListener("mousedown", e => {
      start = { x: e.clientX, w: pane.getBoundingClientRect().width };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      e.preventDefault();
    });
    document.addEventListener("mousemove", e => {
      if (!start) return;
      if (Math.abs(e.clientX - start.x) < 6 && !start.moved) return;
      start.moved = true;
      if (pane.classList.contains("closed")) return;
      const most = within.getBoundingClientRect().width * 0.85;
      const delta = side === "left" ? e.clientX - start.x : start.x - e.clientX;
      const w = Math.max(120, Math.min(most, start.w + delta));
      pane.style.flex = "none";
      pane.style.width = w + "px";
    });
    document.addEventListener("mouseup", () => {
      if (!start) return;
      const clicked = !start.moved;
      start = null;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      if (clicked) { if (onClick) onClick(); return; }
      try { localStorage.setItem(key, String(Math.round(pane.getBoundingClientRect().width))); } catch (_) {}
    });
  }
  splitter("chart-handle", $("chart-pane"), document.querySelector(".sim-body"), "rill-chart-width", "left");

  // A pane at an edge that slides shut and open: by its header button, or
  // a click on its handle; the choice kept. The width is pinned in pixels
  // before it closes, so both ways slide from one number to another.
  function collapsible(pane, handleId, buttonId, earId, key) {
    const handle = $(handleId), button = $(buttonId), ear = $(earId);
    let open = true;
    try { open = localStorage.getItem(key) !== "closed"; } catch (_) {}
    const set = (o, quiet) => {
      open = o;
      if (!open && !pane.classList.contains("closed")) { pane.style.flex = "none"; pane.style.width = pane.getBoundingClientRect().width + "px"; }
      if (quiet) pane.style.transition = "none";
      pane.classList.toggle("closed", !open);
      handle.classList.toggle("shut", !open);
      ear.hidden = open;
      if (quiet) { void pane.offsetWidth; pane.style.transition = ""; }
      try { localStorage.setItem(key, open ? "open" : "closed"); } catch (_) {}
    };
    set(open, true);
    button.onclick = () => set(false);
    ear.onclick = () => set(true);
    return () => set(!open);
  }
  const toggleSide = collapsible(document.querySelector(".side"), "side-handle", "toggle-side", "ear-side", "rill-side-open");
  const toggleCode = collapsible(document.querySelector(".code"), "code-handle", "toggle-code", "ear-code", "rill-code-open");
  splitter("side-handle", document.querySelector(".side"), document.querySelector("main"), "rill-side-width", "left", toggleSide);
  splitter("code-handle", document.querySelector(".code"), document.querySelector("main"), "rill-code-width", "right", toggleCode);

  // ---- a new model, from a template ---------------------------------------------

  const TEMPLATE = `# A model: the signals, the states, a machine that answers them, the
# routes between it and the environment, and a scenario to run.
type Signal
  Start
  Ping
  Report
  Status(text: Str)

type State
  Idle
  Running(count: Int)

system model
  env -> worker: Start, Ping, Report
  worker -> env: Status

machine worker(me: Chan(Mail(Signal)))
  state Idle
    Start -> Running(0)
    Report ->
      reply(Status("idle"))
      stay
  state Running(count)
    Ping -> Running(count + 1)
    Report ->
      reply(Status("running, " + int_to_str(count) + " pings"))
      stay
    after 100 -> Idle

fn ask(w, env) =
  post(w, env, Report)
  match recv(env).signal
    Status(text) -> text
    _ -> "?"

fn main() =
  env = mailbox()
  w = mailbox()
  spawn worker(w, Idle)
  post(w, env, Start)
  post(w, env, Ping)
  post(w, env, Ping)
  println(ask(w, env))

fn test_pings_are_counted() =
  env = mailbox()
  w = mailbox()
  spawn worker(w, Idle)
  post(w, env, Start)
  post(w, env, Ping)
  assert_eq(ask(w, env), "running, 1 pings")
  sleep_ms(200)
  assert_eq(ask(w, env), "idle")
`;

  $("btn-new").onclick = async () => {
    const name = prompt("The new model's file, under the root:", "models/new_model.rill");
    if (!name) return;
    const path = name.endsWith(".rill") ? name : name + ".rill";
    const { ok, data } = await api("PUT", "/api/file?path=" + encodeURIComponent(path), TEMPLATE);
    if (!ok) { say(data.error || "cannot create " + path); return; }
    await loadFiles();
    await open(path);
  };
  $("all-files").onchange = loadFiles;
  $("btn-add-signal").onclick = e => Edit.addSignal({ x: e.clientX, y: e.clientY });
  $("btn-add-machine").onclick = e => Edit.addMachine({ x: e.clientX, y: e.clientY });
  $("btn-add-system").onclick = e => Edit.addSystem({ x: e.clientX, y: e.clientY });
  $("btn-scenario-edit").onclick = e => { const n = $("scenario").value; if (n) Edit.scenarioBox({ x: e.clientX, y: e.clientY }, n); };
  $("btn-scenario-add").onclick = e => Edit.addScenarioBox({ x: e.clientX, y: e.clientY });

  // ---- the toolbar ------------------------------------------------------------

  $("btn-save").onclick = save;
  $("btn-check").onclick = async () => { if (editor.value !== saved && !isProtected()) await save(); else await refresh(); };
  $("btn-fmt").onclick = async () => {
    if (!current) return;
    if (editor.value !== saved && !isProtected()) await save();
    const res = await api(verb(), "/api/fmt?path=" + encodeURIComponent(current), subject());
    if (!res.ok) { say(res.data.error || "cannot format"); return; }
    if (res.text !== editor.value) {
      editor.value = res.text;
      markDirty();
      renderGutter();
      renderColors();
      say("formatted; save to keep it");
    } else say("already formatted");
  };
  document.addEventListener("keydown", e => {
    if ((e.metaKey || e.ctrlKey) && e.key === "s") { e.preventDefault(); save(); }
  });
  for (const tab of document.querySelectorAll(".tab[data-view]")) {
    tab.onclick = () => {
      for (const t of document.querySelectorAll(".tab[data-view]")) t.classList.toggle("current", t === tab);
      for (const v of document.querySelectorAll(".view")) v.classList.toggle("current", v.id === tab.dataset.view);
    };
  }
  window.goToLine = goToLine;
  Simulate.wire();

  loadFiles().then(() => {
    const wanted = decodeURIComponent(location.hash.slice(1));
    if (wanted) open(wanted);
  });
})();
