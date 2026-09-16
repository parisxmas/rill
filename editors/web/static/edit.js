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

  const machineNames = () => (model.machines || []).map(m => m.name);

  function routeBox(at, sd, i, asCode) {
    const r = sd.routes[i];
    const ls = linesOf();
    const line = r.line;
    const codeBox = () => open(at, `route of system ${sd.name}`, (ls[line - 1] || "").trim(),
      v => replaceLines(line, line, "  " + v.trim()),
      () => replaceLines(line, line, ""));
    if (asCode) return codeBox();
    Forms.route({
      at, title: `route of system ${sd.name}, line ${line}`, text, machines: machineNames(),
      route: { from: r.from, to: r.to, both: r.both, signals: [...r.signals] },
      onSave: code => replaceLines(line, line, code),
      onDelete: () => replaceLines(line, line, ""),
      asCode: codeBox,
    });
  }

  function addRouteBox(at, sd) {
    const last = Math.max(sd.line, ...sd.routes.map(r => r.line));
    const codeBox = () => open(at, `new route of system ${sd.name}`, "env -> machine: Signal",
      v => replaceLines(last + 1, last, "  " + v.trim()));
    Forms.route({
      at, title: `new route of system ${sd.name}`, text, machines: machineNames(), route: null,
      onSave: code => replaceLines(last + 1, last, code),
      asCode: codeBox,
    });
  }

  // An arm — an input, `after`, or `when` — with its body: the form when
  // it can say the arm, the lines when it cannot.
  function armBox(at, machine, line, asCode) {
    const end = blockEnd(line);
    const ls = linesOf();
    const src = ls.slice(line - 1, end).join("\n");
    const where = end > line ? `lines ${line}–${end}` : `line ${line}`;
    const codeBox = () => open(at, `arm of ${machine}, ${where}`, src,
      v => replaceLines(line, end, v.replace(/\n+$/, "")),
      () => replaceLines(line, end, ""), true);
    const mc = (model.machines || []).find(m => m.name === machine);
    const parsed = asCode ? null : Forms.parse(ls.slice(line - 1, end));
    if (!parsed || !mc) return codeBox();
    Forms.transition({
      at, title: `${parsed.kind === "save" || parsed.kind === "priority" ? parsed.kind + " of" : "arm of"} ${machine}, ${where}`, text, machine: mc, arm: parsed,
      onSave: code => replaceLines(line, end, code),
      onDelete: () => replaceLines(line, end, ""),
      asCode: codeBox,
    });
  }

  // A `save` or `priority` line: the same form as an arm, with no body.
  function keywordBox(at, machine, line) {
    armBox(at, machine, line);
  }

  function addInputBox(at, machine, state, stateLine) {
    const end = blockEnd(stateLine);
    const codeBox = () => open(at, `new arm of ${machine} in state ${state}`, "    Signal ->\n      stay",
      v => replaceLines(end + 1, end, v.replace(/\n+$/, "")), null, true);
    const mc = (model.machines || []).find(m => m.name === machine);
    if (!mc) return codeBox();
    Forms.transition({
      at, title: `new arm of ${machine} in state ${state}`, text, machine: mc, arm: null,
      onSave: code => replaceLines(end + 1, end, code),
      asCode: codeBox,
    });
  }

  // A new state: its constructor into the state type, and a `state` with
  // one arm at the end of the machine.
  // A new state: its constructor added to the state type, and the state
  // to the machine, with an arm that keeps it.
  function addStateBox(at, m) {
    const first = m.states.find(s => s.name !== "*");
    const add = async (ctor, pattern) => {
      const edits = [];
      const typeEnd = first ? stateTypeEnd(first.name) : 0;
      if (typeEnd) edits.push({ from: typeEnd + 1, to: typeEnd, text: "  " + ctor });
      const mEnd = blockEnd(m.line);
      edits.push({ from: mEnd + 1, to: mEnd, text: `  state ${pattern}\n    _ -> stay` });
      const data = await send(edits);
      await reload(data.text);
    };
    const codeBox = () => open(at, `new state of ${m.name} — the constructor, with its fields`, "Named(field: Int)", v => {
      const c = Forms.parseCtor(v);
      if (!c) throw new Error("a constructor: Named, or Named(field: Int)");
      return add(v.trim(), c.fields.length ? `${c.name}(${c.fields.map(f => f.name).join(", ")})` : c.name);
    });
    Forms.ctor({ at, title: `new state of ${m.name}`, text, what: "state", ctor: null, onSave: add, asCode: codeBox });
  }

  // A state: its name and fields in a form, which changes its constructor
  // in the state type and its `state` line together; its whole block as
  // code; deleted with its constructor, so the type does not keep a state
  // the machine no longer has.
  function stateBox(at, m, st) {
    const end = blockEnd(st.line);
    const ls = linesOf();
    const remove = async () => {
      const edits = [{ from: st.line, to: end, text: "" }];
      const ctorLine = stateCtorLine(st.name);
      if (ctorLine && st.name !== "*") edits.push({ from: ctorLine, to: ctorLine, text: "" });
      const data = await send(edits);
      await reload(data.text);
    };
    const codeBox = () => open(at, `state ${st.pattern} of ${m.name}, lines ${st.line}–${end}`, ls.slice(st.line - 1, end).join("\n"),
      v => replaceLines(st.line, end, v.replace(/\n+$/, "")), remove, true);
    const ctorLine = st.name !== "*" ? stateCtorLine(st.name) : 0;
    const ctor = ctorLine ? Forms.parseCtor(ls[ctorLine - 1]) : null;
    if (!ctor) return codeBox();
    Forms.ctor({
      at, title: `state ${st.pattern} of ${m.name}`, text, what: "state", ctor,
      onSave: async (line, pattern) => {
        const data = await send([{ from: ctorLine, to: ctorLine, text: "  " + line }, { from: st.line, to: st.line, text: "  state " + pattern }]);
        await reload(data.text);
      },
      onDelete: remove,
      asCode: codeBox,
    });
  }

  // The line in a type declaration that is this state's constructor.
  function stateCtorLine(state) {
    const ls = linesOf();
    for (let i = 0; i < ls.length; i++) {
      if (!/^type /.test(ls[i])) continue;
      const end = blockEnd(i + 1);
      for (let j = i + 1; j < end; j++) {
        if (new RegExp("^  " + state + "(\\(|$)").test(ls[j])) return j + 1;
      }
    }
    return 0;
  }

  // A whole machine or system: its block, to change or delete.
  function blockBox(at, what, name, line) {
    const end = blockEnd(line);
    const ls = linesOf();
    open(at, `${what} ${name}, lines ${line}–${end}`, ls.slice(line - 1, end).join("\n"),
      v => replaceLines(line, end, v.replace(/\n+$/, "")),
      () => replaceLines(line, end, ""), true);
  }

  // ---- the file's own additions: a signal, a machine, a system ------------------

  // The signal type: what a machine's mailbox carries, or the first type.
  function signalType() {
    const m = text.match(/Chan\(Mail\(([A-Z][A-Za-z0-9_]*)\)\)/);
    if (m) return m[1];
    const t = text.match(/^type ([A-Z][A-Za-z0-9_]*)/m);
    return t ? t[1] : null;
  }

  function typeEnd(name) {
    const ls = linesOf();
    const i = ls.findIndex(l => l === "type " + name || l.startsWith("type " + name + "("));
    return i < 0 ? 0 : blockEnd(i + 1);
  }

  function addSignal(at) {
    const type = signalType();
    if (!type) {
      const n = linesOf().length;
      open(at, "no signal type yet — this makes one, with a first signal", "type Signal\n  Start", v => replaceLines(n + 1, n, "\n" + v.replace(/\n+$/, "")), null, true);
      return;
    }
    const add = v => { const end = typeEnd(type); return replaceLines(end + 1, end, "  " + v.trim()); };
    const codeBox = () => open(at, `new signal, a constructor of ${type}`, "Named(field: Int)", add);
    Forms.ctor({ at, title: `new signal, a constructor of ${type}`, text, what: "signal", ctor: null, onSave: add, asCode: codeBox });
  }

  function addMachine(at) {
    const sig = signalType() || "Signal";
    open(at, "new machine — its name", "worker", v => {
      const name = v.trim().replace(/[^a-z0-9_]/gi, "").toLowerCase() || "worker";
      const cap = name[0].toUpperCase() + name.slice(1);
      const n = linesOf().length;
      // constructors are one namespace, so the first state carries the
      // machine's name: `ClockStart`, never a second `Start`
      const body = `\ntype ${cap}State\n  ${cap}Start\n\nmachine ${name}(me: Chan(Mail(${sig})))\n  state ${cap}Start\n    _ -> stay`;
      return replaceLines(n + 1, n, body);
    }, null, false);
  }

  function addSystem(at) {
    const first = (model && model.machines && model.machines[0]) ? model.machines[0].name : "worker";
    const sig = signalType() || "Signal";
    const firstSignal = (text.match(new RegExp("^type " + sig + "\\n  ([A-Z][A-Za-z0-9_]*)", "m")) || [])[1] || "Start";
    const append = v => { const n = linesOf().length; return replaceLines(n + 1, n, "\n" + v.replace(/\n+$/, "")); };
    const codeBox = () => open(at, "new system — its routes", `system model\n  env -> ${first}: ${firstSignal}`, append, null, true);
    Forms.route({
      at, title: "new system — its name, and its first route", text, machines: machineNames(), route: null, system: "model",
      onSave: append,
      asCode: codeBox,
    });
  }

  // ---- scenarios -------------------------------------------------------------------------

  // The line of `fn <name>(` in the file, 0 when there is none.
  function fnLine(name) {
    const i = linesOf().findIndex(l => l.startsWith("fn " + name + "("));
    return i < 0 ? 0 : i + 1;
  }

  // A scenario: the `test_` function opened in the scenario form when it
  // says what the form can, as code otherwise; `main` always as code.
  function scenarioBox(at, name) {
    const line = fnLine(name);
    if (!line) return;
    const end = blockEnd(line);
    const ls = linesOf();
    const codeBox = () => open(at, `${name}, lines ${line}–${end}`, ls.slice(line - 1, end).join("\n"),
      v => replaceLines(line, end, v.replace(/\n+$/, "")),
      () => replaceLines(line, end, ""), true);
    const parsed = name.startsWith("test_") ? Forms.parseScenario(ls.slice(line - 1, end)) : null;
    if (!parsed) return codeBox();
    Forms.scenario({
      at, title: `scenario ${name}, lines ${line}–${end}`, text, model, scenario: parsed,
      onSave: code => replaceLines(line, end, code),
      onDelete: () => replaceLines(line, end, ""),
      asCode: codeBox,
    });
  }

  function addScenarioBox(at) {
    const append = v => { const n = linesOf().length; return replaceLines(n + 1, n, "\n" + v.replace(/\n+$/, "")); };
    const first = (model && model.machines && model.machines[0]) || null;
    const codeBox = () => open(at, "new scenario — a test_ function", `fn test_scenario() =\n  env = mailbox()\n  a = mailbox()\n  spawn ${first ? first.name : "machine"}(a, ${first && first.states[0] ? first.states[0].name : "Start"})`, append, null, true);
    Forms.scenario({ at, title: "new scenario", text, model, scenario: null, onSave: append, asCode: codeBox });
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
    const ICONS = {
      route: '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><circle cx="3" cy="8" r="1.8" fill="currentColor"/><path d="M5 8h7" stroke="currentColor" stroke-width="1.4"/><path d="M10.5 5.5 13 8l-2.5 2.5" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>',
      edit: '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M3 13l1-4 7-7 3 3-7 7z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/><path d="M10 3l3 3" stroke="currentColor" stroke-width="1.4"/></svg>',
    };
    const button = (h, label, title, fn) => {
      const b = document.createElement("button");
      b.className = "adder-button iconic";
      b.innerHTML = (ICONS[label.replace("+ ", "")] || "") + "<span>" + label + "</span>";
      b.title = title;
      b.onclick = e => fn({ x: e.clientX, y: e.clientY });
      h.appendChild(b);
    };
    for (const sd of systems) {
      const h = [...container.querySelectorAll("h2")].find(h => h.textContent.endsWith(sd.name) && h.textContent.startsWith("system"));
      if (!h) continue;
      button(h, "+ route", "a route added to this system", at => addRouteBox(at, sd));
      button(h, "edit", "the system's lines, to change or delete", at => blockBox(at, "system", sd.name, sd.line));
    }
    for (const mc of m.machines || []) {
      const h = [...container.querySelectorAll("h2")].find(h => h.textContent.startsWith("machine") && h.textContent.endsWith(mc.name));
      const svg = h && h.nextElementSibling;
      if (!svg) continue;
      button(h, "edit", "the machine's lines, to change or delete", at => blockBox(at, "machine", mc.name, mc.line));
      for (const st of mc.states) {
        const g = svg.querySelector(`.sym[data-machine="${mc.name}"][data-state="${st.name}"]`);
        if (g) g.addEventListener("click", e => { e.stopPropagation(); stateBox({ x: e.clientX, y: e.clientY }, mc, st); });
      }
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
  return { attach, close, addSignal, addMachine, addSystem, scenarioBox, addScenarioBox };
})();
