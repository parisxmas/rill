// The forms: what the diagrams build without the code being written.
// The transition form makes an arm of a state — the input picked from the
// file's signals, a guard if any, the actions added one at a time from
// what SDL's symbols are (output, reply, a timer set or reset, a send, a
// task; a decision is left to code), and the next state picked from the
// machine's — or a `save` or a `priority`, which is an input with no
// body. The route form makes a route of a system: from, to, the signals
// ticked. The constructor form makes a signal or a state: a name, and
// fields with their types picked. The scenario form makes a `test_`
// function: instances spawned, signals posted, what the environment
// receives expected. Each writes the lines it would have been written as, shows them
// as it goes, and hands them to the same edit the box of lines makes; an
// arm the form cannot say — a body with an `if` or a `match` in it, a
// binding — opens as code instead, and every form has a way to code.

const Forms = (() => {
  // ---- what the file offers -------------------------------------------------------

  // The constructors of a type, with their field names: `Digit(n: Int)`
  // becomes `{ name: "Digit", fields: ["n"], types: ["Int"] }`.
  function ctorsOf(text, typeName) {
    const lines = text.split("\n");
    const start = lines.findIndex(l => l === "type " + typeName || l.startsWith("type " + typeName + "("));
    if (start < 0) return [];
    const out = [];
    for (let i = start + 1; i < lines.length; i++) {
      const l = lines[i];
      if (l.trim() === "") continue;
      if (!l.startsWith("  ")) break;
      const m = l.trim().match(/^([A-Z][A-Za-z0-9_]*)(?:\((.*)\))?$/);
      if (!m) continue;
      const fields = m[2] ? m[2].split(",").map(f => f.trim()) : [];
      out.push({ name: m[1], fields: fields.map(f => f.split(":")[0].trim()), types: fields.map(f => (f.split(":")[1] || "").trim()) });
    }
    return out;
  }

  function signalTypeOf(text) {
    const m = text.match(/Chan\(Mail\(([A-Z][A-Za-z0-9_]*)\)\)/);
    return m ? m[1] : null;
  }

  // A constructor as a pattern with its fields named, `Digit(n)`.
  const asPattern = c => c.fields.length ? `${c.name}(${c.fields.join(", ")})` : c.name;
  // The same with the fields unnamed, `Digit(_)`, for a `save` that binds nothing.
  const asShape = c => c.fields.length ? `${c.name}(${c.fields.map(() => "_").join(", ")})` : c.name;
  // A state's pattern read as a constructor: `Talking(caller)` has the field `caller`.
  const stateCtor = s => { const m = s.pattern.match(/^([A-Z][A-Za-z0-9_]*)(?:\((.*)\))?$/); return { name: s.name, fields: m && m[2] ? m[2].split(",").map(f => f.trim()) : [], types: [] }; };
  // `Status("busy")` split into its constructor and its arguments.
  const splitCall = v => { const m = (v || "").match(/^([A-Z][A-Za-z0-9_]*)\((.*)\)$/s); return m ? { name: m[1], args: m[2] } : { name: v || "", args: "" }; };

  // ---- reading an arm back into the form --------------------------------------------

  // The form's idea of an arm, or null when the lines say more than it can.
  function parse(lines) {
    if (!lines.length) return null;
    const head = lines[0].trim();
    const arm = { kind: "input", input: "", guard: "", ms: "", cond: "", actions: [], end: { kind: "stay", text: "" } };
    let m;
    if ((m = head.match(/^(save|priority) (.+)$/)) && !head.includes("->")) {
      arm.kind = m[1];
      const g = m[2].indexOf(" if ");
      arm.input = g < 0 ? m[2].trim() : m[2].slice(0, g).trim();
      arm.guard = g < 0 ? "" : m[2].slice(g + 4).trim();
      return lines.slice(1).every(l => l.trim() === "") ? arm : null;
    }
    if ((m = head.match(/^after (.+?) ->(.*)$/))) { arm.kind = "after"; arm.ms = m[1].trim(); }
    else if ((m = head.match(/^when (.+?) ->(.*)$/))) { arm.kind = "when"; arm.cond = m[1].trim(); }
    else if ((m = head.match(/^(.+?) ->(.*)$/))) {
      const pat = m[1].trim();
      const g = pat.indexOf(" if ");
      arm.input = g < 0 ? pat : pat.slice(0, g).trim();
      arm.guard = g < 0 ? "" : pat.slice(g + 4).trim();
    } else return null;
    const inline = m[2].trim();
    const body = inline ? [inline] : lines.slice(1).map(l => l.trim()).filter(l => l !== "");
    if (!body.length) return null;
    const last = body[body.length - 1];
    for (const l of body.slice(0, -1)) {
      let a;
      if ((a = l.match(/^output\((.+?),\s*(.+)\)$/))) arm.actions.push({ kind: "output", to: a[1].trim(), signal: a[2].trim() });
      else if ((a = l.match(/^reply\((.+)\)$/))) arm.actions.push({ kind: "reply", signal: a[1].trim() });
      else if ((a = l.match(/^set\((.+?),\s*(.+)\)$/))) arm.actions.push({ kind: "set", timer: a[1].trim(), ms: a[2].trim() });
      else if ((a = l.match(/^reset\((.+)\)$/))) arm.actions.push({ kind: "reset", timer: a[1].trim() });
      else if ((a = l.match(/^send\(([a-z_][A-Za-z0-9_]*),\s*(.+)\)$/))) arm.actions.push({ kind: "send", to: a[1].trim(), text: a[2].trim() });
      else if (/^(if |match |\w+ = |\(.*\) = )/.test(l) || l.endsWith("->")) return null;
      else arm.actions.push({ kind: "task", text: l });
    }
    if (last === "stay") arm.end = { kind: "stay", text: "" };
    else if (last === "stop") arm.end = { kind: "stop", text: "" };
    else if (/^done\(.*\)$/.test(last)) arm.end = { kind: "done", text: last.slice(5, -1) };
    else if (/^[A-Z][A-Za-z0-9_]*(\(.*\))?$/.test(last)) arm.end = { kind: "next", text: last };
    else return null;
    if (lines.some(l => /^\s*(if |match )/.test(l) && !l.includes("->"))) return null;
    return arm;
  }

  // ---- writing the form out as lines ----------------------------------------------------

  function write(arm) {
    if (arm.kind === "save" || arm.kind === "priority") return `    ${arm.kind} ${arm.input || "Signal"}${arm.guard ? " if " + arm.guard : ""}`;
    const head = arm.kind === "after" ? `after ${arm.ms || 0}`
      : arm.kind === "when" ? `when ${arm.cond || "true"}`
      : (arm.input || "Signal") + (arm.guard ? " if " + arm.guard : "");
    const steps = arm.actions.map(a =>
      a.kind === "output" ? `output(${a.to || "sender"}, ${a.signal || "Signal"})`
      : a.kind === "reply" ? `reply(${a.signal || "Signal"})`
      : a.kind === "set" ? `set(${a.timer || "Timer"}, ${a.ms || 0})`
      : a.kind === "reset" ? `reset(${a.timer || "Timer"})`
      : a.kind === "send" ? `send(${a.to || "chan"}, ${a.text || '""'})`
      : (a.text || "()"));
    const end = arm.end.kind === "stay" ? "stay" : arm.end.kind === "stop" ? "stop" : arm.end.kind === "done" ? `done(${arm.end.text || "0"})` : (arm.end.text || "stay");
    if (!steps.length) return `    ${head} -> ${end}`;
    return [`    ${head} ->`, ...steps.map(s => "      " + s), "      " + end].join("\n");
  }

  // ---- the form itself ---------------------------------------------------------------------

  let box = null;
  function close() { if (box) box.remove(); box = null; }

  const OTHER = "\u2026";   // the "other…" entry of a picker: a free text field opens

  const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

  // A select with the given options and a free "other…" that becomes a text field.
  function choice(options, value, onChange, placeholder) {
    const wrap = el("span", "choice");
    const sel = el("select");
    for (const o of options) { const op = el("option", null, o.label); op.value = o.value; sel.appendChild(op); }
    const other = el("option", null, "other…"); other.value = OTHER; sel.appendChild(other);
    const free = el("input"); free.placeholder = placeholder || ""; free.hidden = true;
    const known = options.some(o => o.value === value);
    if (known || value === "") { sel.value = known ? value : options[0]?.value ?? OTHER; }
    else { sel.value = OTHER; free.hidden = false; free.value = value; }
    sel.onchange = () => { if (sel.value === OTHER) { free.hidden = false; free.focus(); onChange(free.value); } else { free.hidden = true; onChange(sel.value); } };
    free.oninput = () => onChange(free.value);
    wrap.append(sel, free);
    if (!known && value !== "") onChange(value); else onChange(sel.value === OTHER ? "" : sel.value);
    return wrap;
  }

  // A constructor picked by name, its arguments typed in when it has fields:
  // the value is the call, `Status("busy")`.
  function ctorChoice(ctors, value, onChange, placeholder) {
    const wrap = el("span", "choice");
    const cur = splitCall(value);
    const sel = el("select");
    for (const c of ctors) { const op = el("option", null, c.name); op.value = c.name; sel.appendChild(op); }
    const other = el("option", null, "other…"); other.value = OTHER; sel.appendChild(other);
    const free = el("input"); free.placeholder = placeholder || ""; free.hidden = true;
    const args = el("input"); args.className = "args"; args.hidden = true;
    const known = ctors.some(c => c.name === cur.name);
    if (known) sel.value = cur.name;
    else if (cur.name === "" && ctors.length) sel.value = ctors[0].name;
    else { sel.value = OTHER; free.hidden = false; free.value = value || ""; }
    const say = () => {
      if (sel.value === OTHER) { args.hidden = true; onChange(free.value); return; }
      const c = ctors.find(c => c.name === sel.value);
      args.hidden = !c || !c.fields.length;
      if (c && c.fields.length) {
        args.placeholder = c.fields.map((f, i) => c.types[i] ? `${f}: ${c.types[i]}` : f).join(", ");
        onChange(`${c.name}(${args.value.trim()})`);
      } else onChange(c ? c.name : "");
    };
    if (known) args.value = cur.args;
    sel.onchange = () => { if (sel.value === OTHER) { free.hidden = false; free.focus(); } else free.hidden = true; say(); };
    free.oninput = say;
    args.oninput = say;
    wrap.append(sel, free, args);
    say();
    return wrap;
  }

  // `ctx`: { at, title, text, machine, state, arm (parsed or null), onSave(lines), asCode() }
  function open(ctx) {
    close();
    const sigType = signalTypeOf(ctx.text);
    const signals = sigType ? ctorsOf(ctx.text, sigType) : [];
    const states = ctx.machine.states.filter(s => s.name !== "*").map(stateCtor);
    const targets = [...ctx.machine.params.slice(1), "sender"];   // where an output can go
    const channels = ctx.machine.params.slice(1);                  // what a send can go on
    const arm = ctx.arm || { kind: "input", input: signals[0] ? asPattern(signals[0]) : "", guard: "", ms: "100", cond: "", actions: [], end: { kind: "stay", text: "" } };

    box = el("div", "editbox form");
    box.appendChild(el("div", "editbox-title", ctx.title));
    const body = el("div", "form-body");
    box.appendChild(body);
    const preview = el("pre", "form-preview");
    const row = el("div", "editbox-row");
    const status = el("span", "editbox-status");

    const refresh = () => { preview.textContent = write(arm); };
    const field = (label, control) => { const r = el("div", "form-row"); r.appendChild(el("label", null, label)); r.appendChild(control); return r; };

    function render() {
      body.innerHTML = "";
      // the input
      const kind = el("select");
      const KINDS = [["input", "a signal"], ["after", "after a wait"], ["when", "when a condition holds"], ["save", "save — a signal kept for a later state"], ["priority", "priority — a signal answered first"]];
      for (const [v, l] of KINDS) { const o = el("option", null, l); o.value = v; kind.appendChild(o); }
      kind.value = arm.kind;
      kind.onchange = () => { arm.kind = kind.value; if (arm.kind !== "input") arm.input = ""; render(); };
      body.appendChild(field("taken on", kind));
      const keyword = arm.kind === "save" || arm.kind === "priority";
      if (arm.kind === "input" || keyword) {
        const shape = keyword ? asShape : asPattern;
        const opts = [...signals.map(c => ({ value: shape(c), label: shape(c) })), ...(keyword ? [] : [{ value: "_", label: "_ (any other)" }])];
        body.appendChild(field("signal", choice(opts, arm.input, v => { arm.input = v; refresh(); }, "Pattern(n)")));
        const guard = el("input"); guard.value = arm.guard; guard.placeholder = "n >= 0 (optional)";
        guard.oninput = () => { arm.guard = guard.value.trim(); refresh(); };
        body.appendChild(field("only if", guard));
      }
      if (keyword) { refresh(); return; }
      if (arm.kind === "input") {
      } else if (arm.kind === "after") {
        const ms = el("input"); ms.type = "number"; ms.value = arm.ms || "100"; ms.min = 0;
        ms.oninput = () => { arm.ms = ms.value; refresh(); };
        body.appendChild(field("milliseconds", ms));
      } else {
        const cond = el("input"); cond.value = arm.cond; cond.placeholder = "len(items) >= 3";
        cond.oninput = () => { arm.cond = cond.value.trim(); refresh(); };
        body.appendChild(field("condition", cond));
      }
      // the actions
      const acts = el("div", "form-actions");
      body.appendChild(field("does", acts));
      arm.actions.forEach((a, i) => {
        const r = el("div", "form-action");
        const k = el("select");
        for (const [v, l] of [["output", "output"], ["reply", "reply"], ["set", "set timer"], ["reset", "reset timer"], ["send", "send on a channel"], ["task", "task"]]) { const o = el("option", null, l); o.value = v; k.appendChild(o); }
        k.value = a.kind;
        k.onchange = () => { arm.actions[i] = { kind: k.value, to: "", signal: "", timer: "", ms: "100", text: "" }; render(); };
        r.appendChild(k);
        const plain = signals.filter(c => !c.fields.length);   // a timer is a signal without fields
        if (a.kind === "output") {
          r.appendChild(choice(targets.map(t => ({ value: t, label: t })), a.to, v => { a.to = v; refresh(); }, "mailbox"));
          r.appendChild(ctorChoice(signals, a.signal, v => { a.signal = v; refresh(); }, "Signal(args)"));
        } else if (a.kind === "reply") {
          r.appendChild(ctorChoice(signals, a.signal, v => { a.signal = v; refresh(); }, "Signal(args)"));
        } else if (a.kind === "set") {
          r.appendChild(ctorChoice(plain.length ? plain : signals, a.timer, v => { a.timer = v; refresh(); }, "Timer"));
          const ms = el("input"); ms.type = "number"; ms.value = a.ms || "100"; ms.min = 0; ms.className = "ms";
          ms.oninput = () => { a.ms = ms.value; refresh(); };
          r.appendChild(ms);
        } else if (a.kind === "reset") {
          r.appendChild(ctorChoice(plain.length ? plain : signals, a.timer, v => { a.timer = v; refresh(); }, "Timer"));
        } else if (a.kind === "send") {
          r.appendChild(choice(channels.map(t => ({ value: t, label: t })), a.to, v => { a.to = v; refresh(); }, "channel"));
          const t = el("input"); t.value = a.text; t.placeholder = '"a line of the log"'; t.className = "wide";
          t.oninput = () => { a.text = t.value; refresh(); };
          r.appendChild(t);
        } else {
          const t = el("input"); t.value = a.text; t.placeholder = 'send(log, "…")'; t.className = "wide";
          t.oninput = () => { a.text = t.value; refresh(); };
          r.appendChild(t);
        }
        const del = el("button", "tiny", "×"); del.title = "remove"; del.onclick = () => { arm.actions.splice(i, 1); render(); };
        r.appendChild(del);
        acts.appendChild(r);
      });
      const add = el("button", "tiny", "+ action");
      add.onclick = () => { arm.actions.push({ kind: "output", to: targets[0] || "sender", signal: "", timer: "", ms: "100", text: "" }); render(); };
      acts.appendChild(add);
      // the end
      const endKind = el("select");
      for (const [v, l] of [["next", "goes to"], ["stay", "stays"], ["stop", "stops"], ["done", "is done, with a value"]]) { const o = el("option", null, l); o.value = v; endKind.appendChild(o); }
      endKind.value = arm.end.kind;
      endKind.onchange = () => { arm.end = { kind: endKind.value, text: "" }; render(); };
      const endRow = el("span", "choice");
      endRow.appendChild(endKind);
      if (arm.end.kind === "next") endRow.appendChild(ctorChoice(states, arm.end.text, v => { arm.end.text = v; refresh(); }, "State(fields)"));
      if (arm.end.kind === "done") { const v = el("input"); v.value = arm.end.text; v.placeholder = "value"; v.oninput = () => { arm.end.text = v.value; refresh(); }; endRow.appendChild(v); }
      body.appendChild(field("then", endRow));
      refresh();
    }
    render();
    box.appendChild(el("div", "form-preview-title", "as it will be written"));
    box.appendChild(preview);
    const mk = (label, cls, fn) => { const b = el("button", cls, label); b.onclick = fn; row.appendChild(b); };
    mk("Save", "primary", async () => { status.textContent = "…"; try { await ctx.onSave(write(arm)); close(); } catch (e) { status.textContent = e.message; } });
    if (ctx.onDelete) mk("Delete", "danger", async () => { status.textContent = "…"; try { await ctx.onDelete(); close(); } catch (e) { status.textContent = e.message; } });
    mk("as code", "", () => { close(); ctx.asCode(); });
    mk("Cancel", "", close);
    row.appendChild(status);
    box.appendChild(row);
    document.body.appendChild(box);
    place(ctx.at);
  }

  // ---- the route form ------------------------------------------------------------------

  const writeRoute = r => `  ${r.from || "env"} ${r.both ? "<->" : "->"} ${r.to || "env"}: ${r.signals.length ? r.signals.join(", ") : "Signal"}`;

  // `ctx`: { at, title, text, machines (names), route ({from,to,both,signals} or null),
  //          system (a name to ask for, when the route starts one), onSave(lines), onDelete?, asCode() }
  function route(ctx) {
    close();
    const sigType = signalTypeOf(ctx.text);
    const signals = sigType ? ctorsOf(ctx.text, sigType).map(c => c.name) : [];
    const ends = ["env", ...ctx.machines].map(n => ({ value: n, label: n }));
    const r = ctx.route || { from: "env", to: ctx.machines[0] || "env", both: false, signals: signals.slice(0, 1) };
    let name = ctx.system || "";
    box = el("div", "editbox form");
    box.appendChild(el("div", "editbox-title", ctx.title));
    const body = el("div", "form-body");
    box.appendChild(body);
    const preview = el("pre", "form-preview");
    const refresh = () => { preview.textContent = (ctx.system != null ? `system ${name || "model"}\n` : "") + writeRoute(r); };
    const field = (label, control) => { const row = el("div", "form-row"); row.appendChild(el("label", null, label)); row.appendChild(control); return row; };
    if (ctx.system != null) {
      const n = el("input"); n.value = name; n.placeholder = "model";
      n.oninput = () => { name = n.value.trim(); refresh(); };
      body.appendChild(field("system", n));
    }
    body.appendChild(field("from", choice(ends, r.from, v => { r.from = v; refresh(); }, "machine")));
    const dir = el("select");
    for (const [v, l] of [["one", "→  carries to"], ["both", "↔  both ways"]]) { const o = el("option", null, l); o.value = v; dir.appendChild(o); }
    dir.value = r.both ? "both" : "one";
    dir.onchange = () => { r.both = dir.value === "both"; refresh(); };
    body.appendChild(field("direction", dir));
    body.appendChild(field("to", choice(ends, r.to, v => { r.to = v; refresh(); }, "machine")));
    const ticks = el("div", "form-ticks");
    for (const sname of signals) {
      const l = el("label", "tick");
      const c = el("input"); c.type = "checkbox"; c.checked = r.signals.includes(sname);
      c.onchange = () => { r.signals = signals.filter(x => x === sname ? c.checked : r.signals.includes(x)).concat(r.signals.filter(x => !signals.includes(x))); refresh(); };
      l.append(c, document.createTextNode(sname));
      ticks.appendChild(l);
    }
    const more = el("input"); more.placeholder = "others, comma separated"; more.className = "wide";
    more.value = r.signals.filter(x => !signals.includes(x)).join(", ");
    more.oninput = () => { r.signals = r.signals.filter(x => signals.includes(x)).concat(more.value.split(",").map(x => x.trim()).filter(Boolean)); refresh(); };
    ticks.appendChild(more);
    body.appendChild(field("signals", ticks));
    box.appendChild(el("div", "form-preview-title", "as it will be written"));
    box.appendChild(preview);
    refresh();
    const row = el("div", "editbox-row");
    const status = el("span", "editbox-status");
    const mk = (label, cls, fn) => { const b = el("button", cls, label); b.onclick = fn; row.appendChild(b); };
    mk("Save", "primary", async () => { status.textContent = "…"; try { await ctx.onSave(preview.textContent); close(); } catch (e) { status.textContent = e.message; } });
    if (ctx.onDelete) mk("Delete", "danger", async () => { status.textContent = "…"; try { await ctx.onDelete(); close(); } catch (e) { status.textContent = e.message; } });
    mk("as code", "", () => { close(); ctx.asCode(); });
    mk("Cancel", "", close);
    row.appendChild(status);
    box.appendChild(row);
    document.body.appendChild(box);
    place(ctx.at);
  }

  // ---- the constructor form ------------------------------------------------------------

  // The types a field can have: the language's, the file's, a few shapes of them.
  function typeOptions(text, sigType) {
    const own = [...text.matchAll(/^type ([A-Z][A-Za-z0-9_]*)/gm)].map(m => m[1]);
    const out = ["Int", "Float", "Bool", "Str", ...own];
    if (sigType) out.push(`Chan(Mail(${sigType}))`);
    out.push("Chan(Str)", "List(Int)", "List(Str)", "Option(Int)");
    return [...new Set(out)];
  }

  // `Name(f: T, g: U)` read as { name, fields: [{ name, type }] }.
  function parseCtor(line) {
    const m = (line || "").trim().match(/^([A-Z][A-Za-z0-9_]*)(?:\((.*)\))?$/);
    if (!m) return null;
    const fields = m[2] ? m[2].split(",").map(f => { const [n, t] = f.split(":"); return { name: n.trim(), type: (t || "").trim() }; }) : [];
    return { name: m[1], fields };
  }
  const writeCtor = c => c.fields.length ? `${c.name || "Named"}(${c.fields.map(f => `${f.name || "field"}: ${f.type || "Int"}`).join(", ")})` : (c.name || "Named");
  const ctorPattern = c => c.fields.length ? `${c.name || "Named"}(${c.fields.map(f => f.name || "field").join(", ")})` : (c.name || "Named");

  // `ctx`: { at, title, text, what ("signal" | "state"), ctor ({name, fields} or null),
  //          onSave(ctorLine, pattern), onDelete?, asCode() }
  function ctor(ctx) {
    close();
    const types = typeOptions(ctx.text, signalTypeOf(ctx.text)).map(t => ({ value: t, label: t }));
    const c = ctx.ctor ? { name: ctx.ctor.name, fields: ctx.ctor.fields.map(f => ({ ...f })) } : { name: "", fields: [] };
    box = el("div", "editbox form");
    box.appendChild(el("div", "editbox-title", ctx.title));
    const body = el("div", "form-body");
    box.appendChild(body);
    const preview = el("pre", "form-preview");
    const refresh = () => { preview.textContent = ctx.what === "state" ? `  ${writeCtor(c)}\n  state ${ctorPattern(c)}` : `  ${writeCtor(c)}`; };
    const field = (label, control) => { const row = el("div", "form-row"); row.appendChild(el("label", null, label)); row.appendChild(control); return row; };
    function render() {
      body.innerHTML = "";
      const n = el("input"); n.value = c.name; n.placeholder = ctx.what === "state" ? "Waiting" : "Tick";
      n.oninput = () => { c.name = n.value.trim().replace(/[^A-Za-z0-9_]/g, ""); if (c.name && /^[a-z]/.test(c.name)) c.name = c.name[0].toUpperCase() + c.name.slice(1); refresh(); };
      body.appendChild(field("name", n));
      const rows = el("div", "form-actions");
      c.fields.forEach((f, i) => {
        const r = el("div", "form-action");
        const fname = el("input"); fname.value = f.name; fname.placeholder = "field"; fname.className = "ms";
        fname.oninput = () => { f.name = fname.value.trim(); refresh(); };
        r.appendChild(fname);
        r.appendChild(choice(types, f.type, v => { f.type = v; refresh(); }, "a type"));
        const del = el("button", "tiny", "×"); del.title = "remove"; del.onclick = () => { c.fields.splice(i, 1); render(); };
        r.appendChild(del);
        rows.appendChild(r);
      });
      const add = el("button", "tiny", "+ field");
      add.onclick = () => { c.fields.push({ name: "", type: "Int" }); render(); };
      rows.appendChild(add);
      body.appendChild(field("fields", rows));
      refresh();
      if (!c.name) n.focus();
    }
    render();
    box.appendChild(el("div", "form-preview-title", ctx.what === "state" ? "the constructor in the state type, and the state" : "the constructor in the signal type"));
    box.appendChild(preview);
    const row = el("div", "editbox-row");
    const status = el("span", "editbox-status");
    const mk = (label, cls, fn) => { const b = el("button", cls, label); b.onclick = fn; row.appendChild(b); };
    mk("Save", "primary", async () => { status.textContent = "…"; try { await ctx.onSave(writeCtor(c), ctorPattern(c)); close(); } catch (e) { status.textContent = e.message; } });
    if (ctx.onDelete) mk("Delete", "danger", async () => { status.textContent = "…"; try { await ctx.onDelete(); close(); } catch (e) { status.textContent = e.message; } });
    mk("as code", "", () => { close(); ctx.asCode(); });
    mk("Cancel", "", close);
    row.appendChild(status);
    box.appendChild(row);
    document.body.appendChild(box);
    place(ctx.at);
  }

  // ---- the scenario form -----------------------------------------------------------------

  // The machines of the file with their parameters typed, off the header
  // lines: `machine line(me: Chan(Mail(Signal)), log: Chan(Str))`.
  function machinesOf(text) {
    const out = [];
    for (const m of text.matchAll(/^machine ([a-z_][A-Za-z0-9_]*)\((.*)\)$/gm)) {
      const params = [];
      let depth = 0, cur = "";
      for (const ch of m[2] + ",") {
        if (ch === "(") depth++;
        if (ch === ")") depth--;
        if (ch === "," && depth === 0) { if (cur.trim()) { const [n, t] = cur.split(/:(.*)/); params.push({ name: n.trim(), type: (t || "").trim() }); } cur = ""; }
        else cur += ch;
      }
      out.push({ name: m[1], params });
    }
    return out;
  }
  const isMail = t => /^Chan\(Mail\(/.test(t);
  const isChan = t => /^Chan\(/.test(t);

  // The scenario as lines: a `test_` function that makes the instances,
  // posts to them, and expects what comes back to the environment.
  function writeScenario(sc, machines) {
    const out = [`fn test_${sc.name || "scenario"}() =`, "  env = mailbox()"];
    const channels = new Set();
    for (const st of sc.steps) {
      if (st.kind !== "spawn") continue;
      const mc = machines.find(m => m.name === st.machine);
      if (!mc) continue;
      mc.params.slice(1).forEach(p => { const v = st.args[p.name]; if (isChan(p.type) && !isMail(p.type) && v && /^[a-z_][a-z0-9_]*$/i.test(v)) channels.add(v); });
    }
    for (const c of channels) out.push(`  ${c} = channel(16)`);
    for (const st of sc.steps) {
      if (st.kind === "spawn") {
        const mc = machines.find(m => m.name === st.machine);
        const args = mc ? mc.params.slice(1).map(p => st.args[p.name] || (isMail(p.type) ? "env" : p.name)) : [];
        out.push(`  ${st.inst || "a"} = mailbox()`, `  spawn ${st.machine || "machine"}(${[st.inst || "a", ...args, st.state || "Start"].join(", ")})`);
      } else if (st.kind === "post") out.push(`  post(${st.to || "a"}, env, ${st.signal || "Signal"})`);
      else if (st.kind === "expect") out.push("  match recv(env).signal", `    ${st.signal || "Signal"} -> ()`, "    _ -> assert(false)");
      else if (st.kind === "wait") out.push(`  sleep_ms(${st.ms || 0})`);
      else out.push("  " + (st.text || "()"));
    }
    return out.join("\n");
  }

  // A `test_` function read back as steps, or null when it does more than the form says.
  function parseScenario(lines) {
    const m = (lines[0] || "").match(/^fn test_([a-z0-9_]*)\(\) =$/i);
    if (!m) return null;
    const sc = { name: m[1], steps: [] };
    const body = lines.slice(1).map(l => l.replace(/^  /, "")).filter(l => l.trim() !== "");
    if (body[0] !== "env = mailbox()") return null;   // the form's scenarios start with the environment
    let pendingBox = null;
    for (let i = 0; i < body.length; i++) {
      const l = body[i];
      let a;
      if (l === "env = mailbox()" || /^[a-z_][a-z0-9_]* = channel\(\d+\)$/i.test(l)) continue;
      if ((a = l.match(/^([a-z_][a-z0-9_]*) = mailbox\(\)$/i))) { pendingBox = a[1]; continue; }
      if ((a = l.match(/^spawn ([a-z_][a-z0-9_]*)\((.*)\)$/i))) {
        const parts = []; let depth = 0, cur = "";
        for (const ch of a[2] + ",") { if (ch === "(") depth++; if (ch === ")") depth--; if (ch === "," && depth === 0) { parts.push(cur.trim()); cur = ""; } else cur += ch; }
        if (parts[0] !== pendingBox) return null;
        sc.steps.push({ kind: "spawn", inst: parts[0], machine: a[1], argv: parts.slice(1, -1), args: {}, state: parts[parts.length - 1] });
        pendingBox = null;
        continue;
      }
      if (pendingBox) return null;
      if ((a = l.match(/^post\(([a-z_][a-z0-9_]*), env, (.+)\)$/i))) { sc.steps.push({ kind: "post", to: a[1], signal: a[2] }); continue; }
      if (l === "match recv(env).signal" && body[i + 2] === "  _ -> assert(false)" && (a = (body[i + 1] || "").match(/^  (.+) -> \(\)$/))) { sc.steps.push({ kind: "expect", signal: a[1] }); i += 2; continue; }
      if ((a = l.match(/^sleep_ms\((\d+)\)$/))) { sc.steps.push({ kind: "wait", ms: a[1] }); continue; }
      if (/^\s/.test(l) || /^(if |match |for |while )/.test(l)) return null;
      sc.steps.push({ kind: "line", text: l });
    }
    return sc;
  }

  // `ctx`: { at, title, text, model, scenario (parsed or null), onSave(lines), onDelete?, asCode() }
  function scenario(ctx) {
    close();
    const machines = machinesOf(ctx.text);
    const sigType = signalTypeOf(ctx.text);
    const signals = sigType ? ctorsOf(ctx.text, sigType) : [];
    const sc = ctx.scenario || { name: "", steps: [] };
    // arguments read positionally come back by parameter name
    for (const st of sc.steps) {
      if (st.kind !== "spawn" || !st.argv) continue;
      const mc = machines.find(m => m.name === st.machine);
      if (mc) mc.params.slice(1).forEach((p, i) => { if (st.argv[i] != null) st.args[p.name] = st.argv[i]; });
      delete st.argv;
    }
    box = el("div", "editbox form");
    box.appendChild(el("div", "editbox-title", ctx.title));
    const body = el("div", "form-body");
    box.appendChild(body);
    const preview = el("pre", "form-preview");
    const refresh = () => { preview.textContent = writeScenario(sc, machines); };
    const field = (label, control) => { const row = el("div", "form-row"); row.appendChild(el("label", null, label)); row.appendChild(control); return row; };
    const instancesBefore = i => sc.steps.slice(0, i).filter(s => s.kind === "spawn").map(s => s.inst).filter(Boolean);
    function render() {
      body.innerHTML = "";
      const n = el("input"); n.value = sc.name; n.placeholder = "a_text_goes_through";
      n.oninput = () => { sc.name = n.value.trim().replace(/[^A-Za-z0-9_]/g, "_").toLowerCase(); refresh(); };
      body.appendChild(field("test_", n));
      const rows = el("div", "form-actions");
      sc.steps.forEach((st, i) => {
        const r = el("div", "form-action");
        const k = el("select");
        for (const [v, l] of [["spawn", "spawn"], ["post", "post"], ["expect", "env receives"], ["wait", "wait"], ["line", "a line of code"]]) { const o = el("option", null, l); o.value = v; k.appendChild(o); }
        k.value = st.kind;
        k.onchange = () => { sc.steps[i] = { kind: k.value, inst: "", machine: machines[0]?.name || "", args: {}, state: "", to: instancesBefore(i)[0] || "", signal: "", ms: "100", text: "" }; render(); };
        r.appendChild(k);
        if (st.kind === "spawn") {
          const inst = el("input"); inst.value = st.inst; inst.placeholder = "a"; inst.className = "ms";
          inst.oninput = () => { st.inst = inst.value.trim(); refresh(); };
          r.appendChild(inst);
          r.appendChild(el("span", "form-word", "="));
          r.appendChild(choice(machines.map(m => ({ value: m.name, label: m.name })), st.machine, v => { if (v !== st.machine) { st.machine = v; st.args = {}; st.state = ""; render(); } }, "machine"));
          const mc = machines.find(m => m.name === st.machine);
          if (mc) {
            for (const p of mc.params.slice(1)) {
              const lab = el("span", "form-word", p.name + ":");
              r.appendChild(lab);
              if (isMail(p.type)) r.appendChild(choice(["env", ...instancesBefore(i)].map(x => ({ value: x, label: x })), st.args[p.name] || "env", v => { st.args[p.name] = v; refresh(); }, "mailbox"));
              else { const a = el("input"); a.value = st.args[p.name] || (isChan(p.type) ? p.name : ""); a.placeholder = p.type; a.className = "ms"; st.args[p.name] = a.value; a.oninput = () => { st.args[p.name] = a.value.trim(); refresh(); }; r.appendChild(a); }
            }
            const mm = (ctx.model.machines || []).find(m => m.name === mc.name);
            const states = mm ? mm.states.filter(s => s.name !== "*").map(stateCtor) : [];
            r.appendChild(el("span", "form-word", "in"));
            r.appendChild(ctorChoice(states, st.state, v => { st.state = v; refresh(); }, "State"));
          }
        } else if (st.kind === "post") {
          r.appendChild(choice(instancesBefore(i).map(x => ({ value: x, label: x })), st.to, v => { st.to = v; refresh(); }, "instance"));
          r.appendChild(ctorChoice(signals, st.signal, v => { st.signal = v; refresh(); }, "Signal(args)"));
        } else if (st.kind === "expect") {
          r.appendChild(choice(signals.map(c => ({ value: asShape(c), label: asShape(c) })), st.signal, v => { st.signal = v; refresh(); }, "Pattern"));
        } else if (st.kind === "wait") {
          const ms = el("input"); ms.type = "number"; ms.value = st.ms || "100"; ms.min = 0; ms.className = "ms";
          ms.oninput = () => { st.ms = ms.value; refresh(); };
          r.appendChild(ms); r.appendChild(el("span", "form-word", "ms"));
        } else {
          const t = el("input"); t.value = st.text; t.placeholder = 'println(recv(log))'; t.className = "wide";
          t.oninput = () => { st.text = t.value; refresh(); };
          r.appendChild(t);
        }
        const del = el("button", "tiny", "×"); del.title = "remove"; del.onclick = () => { sc.steps.splice(i, 1); render(); };
        r.appendChild(del);
        rows.appendChild(r);
      });
      const add = el("button", "tiny", "+ step");
      add.onclick = () => {
        const inst = instancesBefore(sc.steps.length);
        sc.steps.push(inst.length ? { kind: "post", to: inst[inst.length - 1], signal: "", args: {} } : { kind: "spawn", inst: "a", machine: machines[0]?.name || "", args: {}, state: "" });
        render();
      };
      rows.appendChild(add);
      body.appendChild(field("steps", rows));
      refresh();
    }
    render();
    box.appendChild(el("div", "form-preview-title", "as it will be written"));
    box.appendChild(preview);
    const row = el("div", "editbox-row");
    const status = el("span", "editbox-status");
    const mk = (label, cls, fn) => { const b = el("button", cls, label); b.onclick = fn; row.appendChild(b); };
    mk("Save", "primary", async () => { status.textContent = "…"; try { await ctx.onSave(writeScenario(sc, machines)); close(); } catch (e) { status.textContent = e.message; } });
    if (ctx.onDelete) mk("Delete", "danger", async () => { status.textContent = "…"; try { await ctx.onDelete(); close(); } catch (e) { status.textContent = e.message; } });
    mk("as code", "", () => { close(); ctx.asCode(); });
    mk("Cancel", "", close);
    row.appendChild(status);
    box.appendChild(row);
    document.body.appendChild(box);
    box.classList.add("scenario");
    place(ctx.at);
  }

  function place(at) {
    const x = Math.min(at.x, window.innerWidth - (box.offsetWidth || 540) - 20), y = Math.min(at.y, window.innerHeight - 420);
    box.style.left = Math.max(8, x) + "px";
    box.style.top = Math.max(8, y) + "px";
  }

  document.addEventListener("keydown", e => { if (e.key === "Escape") close(); });
  return { transition: open, route, ctor, parseCtor, scenario, parseScenario, writeScenario, machinesOf, close, parse, write };
})();
