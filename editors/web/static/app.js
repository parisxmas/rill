// The page: a file picked on the left, its diagrams in the middle, its
// code on the right, and every diagram symbol a click away from the line
// it came from — and back, since the cursor's line lights the symbols
// written there. The server does what the compiler does.

(() => {
  const $ = id => document.getElementById(id);
  const files = $("files"), diagrams = $("diagrams"), editor = $("editor"), gutter = $("gutter"), colors = $("colors");
  const problems = $("problems"), status = $("status"), fileName = $("file-name"), dirty = $("dirty");

  let current = null;       // the path of the file shown
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
    const { data } = await api("GET", "/api/files");
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
      item.title = f;
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
    fileName.textContent = path;
    for (const item of files.querySelectorAll(".file")) item.classList.toggle("current", item.dataset.path === path);
    history.replaceState(null, "", "#" + path);
    markDirty();
    renderGutter();
    await refresh();
  }

  function markDirty() { dirty.textContent = editor.value === saved ? "" : "unsaved"; }

  // The file as the server wrote it after an edit made on a diagram.
  async function reloadFile(text) {
    saved = text;
    editor.value = text;
    renderColors();
    markDirty();
    renderGutter();
    await refresh();
  }

  async function save() {
    if (!current) return;
    const { ok, data } = await api("PUT", "/api/file?path=" + encodeURIComponent(current), editor.value);
    if (!ok) { say(data.error || "cannot save"); return; }
    saved = editor.value;
    markDirty();
    say("saved " + current);
    await refresh();
  }

  // ---- the diagrams and the problems ----------------------------------------

  // The model and the check, both from the file as saved.
  async function refresh() {
    if (!current) return;
    notesByLine = {};
    const check = await api("GET", "/api/check?path=" + encodeURIComponent(current));
    const list = (check.data.problems || []);
    problems.innerHTML = "";
    for (const p of list) {
      (notesByLine[p.line] ||= []).push(p);
      const li = document.createElement("li");
      li.className = p.kind;
      li.innerHTML = `<span class="line">${p.line ? "line " + p.line : ""}</span>`;
      li.appendChild(document.createTextNode(p.text));
      li.onclick = () => goToLine(p.line);
      problems.appendChild(li);
    }
    renderGutter();
    const model = await api("GET", "/api/model?path=" + encodeURIComponent(current));
    diagrams.innerHTML = "";
    if (!model.ok) {
      const d = document.createElement("div");
      d.className = "empty";
      d.textContent = model.data.problems ? "The file does not compile; the problems are under the code." : (model.data.error || "nothing to draw");
      diagrams.appendChild(d);
      Simulate.load(current, null);
      say(list.length ? list.length + " problem(s)" : "");
      return;
    }
    const m = model.data;
    window.__model = m;
    Simulate.load(current, m);
    const noteCount = {};
    for (const line in notesByLine) noteCount[line] = notesByLine[line].filter(p => p.kind === "note").length || undefined;
    for (const sd of m.systems || []) {
      const h = document.createElement("h2");
      h.innerHTML = `<span class="kind">system</span>${sd.name}`;
      diagrams.appendChild(h);
      diagrams.appendChild(Diagram.system(sd));
    }
    for (const mc of m.machines || []) {
      const h = document.createElement("h2");
      h.innerHTML = `<span class="kind">machine</span>${mc.name}`;
      diagrams.appendChild(h);
      diagrams.appendChild(Diagram.process(mc, noteCount, true));
    }
    if (!(m.systems || []).length && !(m.machines || []).length) {
      const d = document.createElement("div");
      d.className = "empty";
      d.textContent = "No system or machine in this file; the code is on the right.";
      diagrams.appendChild(d);
    }
    for (const g of diagrams.querySelectorAll(".sym[data-line]")) {
      g.addEventListener("click", () => goToLine(Number(g.dataset.line)));
    }
    Edit.attach(diagrams, m, saved, current, reloadFile);
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

  // ---- the toolbar ------------------------------------------------------------

  $("btn-save").onclick = save;
  $("btn-check").onclick = async () => { if (editor.value !== saved) await save(); else await refresh(); };
  $("btn-fmt").onclick = async () => {
    if (!current) return;
    if (editor.value !== saved) await save();
    const res = await api("GET", "/api/fmt?path=" + encodeURIComponent(current));
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
  for (const tab of document.querySelectorAll(".tab")) {
    tab.onclick = () => {
      for (const t of document.querySelectorAll(".tab")) t.classList.toggle("current", t === tab);
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
