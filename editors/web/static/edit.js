// Editing on the diagrams. Every edit is a range of lines replaced in the
// file, sent to the server, which keeps it only if the result still
// parses and writes it back formatted; the page then reloads the file
// and draws it again. A route, an arm, a `save` or a `priority` opens in
// a small box with its own lines to change or delete; the dashed "+"
// symbols add one. The code is the model, so what the box holds is code.

const Edit = (() => {
  const $ = id => document.getElementById(id);
  let text = "", model = null, file = null, reload = null;

  // ---- lines ---------------------------------------------------------------

  const linesOf = () => text.split("\n");
  const indentOf = l => l.match(/^ */)[0].length;

  // The last line of the block that starts at `line`: every following
  // line indented deeper, blank lines within it included.
  function blockEnd(line) {
    const ls = linesOf();
    const base = indentOf(ls[line - 1] || "");
    let end = line;
    for (let i = line; i < ls.length; i++) {
      const l = ls[i];
      if (l.trim() === "") continue;
      if (indentOf(l) <= base) break;
      end = i + 1;
    }
    return end;
  }

  // The line of the type declaration whose constructors include `state`.
  function stateTypeEnd(state) {
    const ls = linesOf();
    for (let i = 0; i < ls.length; i++) {
      if (!/^type /.test(ls[i])) continue;
      const end = blockEnd(i + 1);
      for (let j = i + 1; j < end; j++) {
        if (new RegExp("^  " + state + "(\\(|$)").test(ls[j])) return end;
      }
    }
    return 0;
  }

  const send = async edits => {
    const res = await fetch("/api/edit?path=" + encodeURIComponent(file), { method: "POST", body: JSON.stringify({ edits }) });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || "the edit was refused");
    return data;
  };

  // ---- the box ------------------------------------------------------------------

  let box = null;

  function close() { if (box) box.remove(); box = null; }

  // A box with the text to change, near the click; `apply(newText)` and
  // `remove()` are what its buttons do.
  function open(at, title, initial, apply, remove, multiline) {
    close();
    box = document.createElement("div");
    box.className = "editbox";
    box.innerHTML = `<div class="editbox-title">${title}</div>`;
    const field = document.createElement(multiline ? "textarea" : "input");
    field.value = initial;
    field.spellcheck = false;
    if (multiline) field.rows = Math.min(12, Math.max(3, initial.split("\n").length + 1));
    box.appendChild(field);
    const row = document.createElement("div");
    row.className = "editbox-row";
    const status = document.createElement("span");
    status.className = "editbox-status";
    const mk = (label, cls, fn) => { const b = document.createElement("button"); b.textContent = label; b.className = cls; b.onclick = fn; row.appendChild(b); };
    mk("Save", "primary", async () => {
      status.textContent = "…";
      try { await apply(field.value); close(); } catch (e) { status.textContent = e.message; }
    });
    if (remove) mk("Delete", "danger", async () => {
      status.textContent = "…";
      try { await remove(); close(); } catch (e) { status.textContent = e.message; }
    });
    mk("Cancel", "", close);
    row.appendChild(status);
    box.appendChild(row);
    document.body.appendChild(box);
    const x = Math.min(at.x, window.innerWidth - 460), y = Math.min(at.y, window.innerHeight - 220);
    box.style.left = Math.max(8, x) + "px";
    box.style.top = Math.max(8, y) + "px";
    field.focus();
    field.addEventListener("keydown", e => {
      if (e.key === "Escape") close();
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey || !multiline)) { e.preventDefault(); row.querySelector(".primary").click(); }
    });
  }

  // ---- what each symbol does ----------------------------------------------------

  async function replaceLines(from, to, newText) {
    const data = await send([{ from, to, text: newText }]);
    await reload(data.text);
  }

  function routeBox(at, sd, i) {
    const r = sd.routes[i];
    const ls = linesOf();
    const line = r.line;
    open(at, `route of system ${sd.name}`, (ls[line - 1] || "").trim(),
      v => replaceLines(line, line, "  " + v.trim()),
      () => replaceLines(line, line, ""));
  }

  function addRouteBox(at, sd) {
    const last = Math.max(sd.line, ...sd.routes.map(r => r.line));
    open(at, `new route of system ${sd.name}`, "env -> machine: Signal",
      v => replaceLines(last + 1, last, "  " + v.trim()));
  }

  // An arm — an input, `after`, or `when` — with its body.
  function armBox(at, machine, line) {
    const end = blockEnd(line);
    const ls = linesOf();
    const src = ls.slice(line - 1, end).join("\n");
    open(at, `arm of ${machine}, lines ${line}–${end}`, src,
      v => replaceLines(line, end, v.replace(/\n+$/, "")),
      () => replaceLines(line, end, ""), true);
  }

  // A `save` or `priority` line.
  function keywordBox(at, machine, line) {
    const ls = linesOf();
    open(at, `${machine}, line ${line}`, (ls[line - 1] || "").trim(),
      v => replaceLines(line, line, "    " + v.trim()),
      () => replaceLines(line, line, ""));
  }

  function addInputBox(at, machine, state, stateLine) {
    const end = blockEnd(stateLine);
    open(at, `new arm of ${machine} in state ${state}`, "    Signal ->\n      stay",
      v => replaceLines(end + 1, end, v.replace(/\n+$/, "")), null, true);
  }

  // A new state: its constructor into the state type, and a `state` with
  // one arm at the end of the machine.
  function addStateBox(at, m) {
    const first = m.states.find(s => s.name !== "*");
    open(at, `new state of ${m.name} — the constructor, with its fields`, "Named(field: Int)", async v => {
      const ctor = v.trim();
      const name = ctor.split("(")[0];
      const pattern = ctor.includes("(") ? name + "(" + ctor.slice(ctor.indexOf("(") + 1, -1).split(",").map(f => f.split(":")[0].trim()).join(", ") + ")" : name;
      const edits = [];
      const typeEnd = first ? stateTypeEnd(first.name) : 0;
      if (typeEnd) edits.push({ from: typeEnd + 1, to: typeEnd, text: "  " + ctor });
      const mEnd = blockEnd(m.line);
      edits.push({ from: mEnd + 1, to: mEnd, text: `  state ${pattern}\n    _ -> stay` });
      const data = await send(edits);
      await reload(data.text);
    });
  }

  // ---- attaching to a drawn model ------------------------------------------------

  function attach(container, m, currentText, currentFile, reloader) {
    text = currentText;
    model = m;
    file = currentFile;
    reload = reloader;
    const systems = m.systems || [];
    container.querySelectorAll("svg").forEach((svg, k) => {
      const sd = systems[k];
      if (!sd) return;
      svg.querySelectorAll(".route").forEach(g => {
        g.addEventListener("click", e => { e.stopPropagation(); routeBox({ x: e.clientX, y: e.clientY }, sd, Number(g.dataset.route)); });
      });
    });
    for (const sd of systems) {
      const b = document.createElement("button");
      b.className = "adder-button";
      b.textContent = "+ route";
      b.onclick = e => addRouteBox({ x: e.clientX, y: e.clientY }, sd);
      const h = [...container.querySelectorAll("h2")].find(h => h.textContent.endsWith(sd.name) && h.textContent.startsWith("system"));
      if (h) h.appendChild(b);
    }
    for (const mc of m.machines || []) {
      const h = [...container.querySelectorAll("h2")].find(h => h.textContent.startsWith("machine") && h.textContent.endsWith(mc.name));
      const svg = h && h.nextElementSibling;
      if (!svg) continue;
      svg.querySelectorAll(".transition").forEach(tg => {
        if (tg.dataset.addInput) {
          tg.addEventListener("click", e => { e.stopPropagation(); addInputBox({ x: e.clientX, y: e.clientY }, mc.name, tg.dataset.addInput, Number(tg.dataset.stateLine)); });
        } else if (tg.dataset.kind) {
          const first = tg.querySelector(".sym");
          if (first) first.addEventListener("click", e => { e.stopPropagation(); armBox({ x: e.clientX, y: e.clientY }, mc.name, Number(first.dataset.line)); });
        } else {
          const first = tg.querySelector(".sym");
          if (first) first.addEventListener("click", e => { e.stopPropagation(); keywordBox({ x: e.clientX, y: e.clientY }, mc.name, Number(first.dataset.line)); });
        }
      });
      const adder = svg.querySelector("[data-add-state]");
      if (adder) adder.addEventListener("click", e => { e.stopPropagation(); addStateBox({ x: e.clientX, y: e.clientY }, mc); });
    }
  }

  document.addEventListener("keydown", e => { if (e.key === "Escape") close(); });
  return { attach, close };
})();
