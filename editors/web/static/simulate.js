// The simulation view: a scenario of the file — `main`, or a `test_`
// function — run on the server with the trace on, and replayed here: the
// chart grows event by event, and on the process diagrams the state each
// machine is in and the transition it is taking light up. Play, pause,
// step, or drag to a moment.

const Simulate = (() => {
  const $ = id => document.getElementById(id);
  let events = [], pos = 0, timer = null, model = null, file = null;
  let buffer = null;   // the page's text, when the run is of it and not of the file
  let shown = null, follow = true;   // the machine whose diagram is up; whether the run picks it

  const api = async path => {
    const res = await fetch(path, buffer != null ? { method: "POST", body: buffer } : {});
    const text = await res.text();
    try { return { ok: res.ok, data: JSON.parse(text) }; } catch (_) { return { ok: res.ok, data: { error: text } }; }
  };

  // The file changed: its scenarios, and its machines drawn again.
  async function load(path, m, ownText) {
    file = path;
    model = m;
    buffer = ownText == null ? null : ownText;
    stop();
    events = [];
    pos = 0;
    const pick = $("scenario");
    pick.innerHTML = "";
    if (!path) return;
    const { data } = await api("/api/scenarios?path=" + encodeURIComponent(path));
    for (const name of data.scenarios || []) {
      const o = document.createElement("option");
      o.value = name;
      o.textContent = name;
      pick.appendChild(o);
    }
    $("chart-pane").innerHTML = '<div class="empty">Pick a scenario and press Run. The chart of the run grows here, event by event, and the machines on the right light the state and transition each is at; play, step, or drag to any moment.</div>';
    $("sim-output").textContent = "";
    drawMachines();
    show();
  }

  // The machines, a tab each: the tab names the machine and, in a run, the
  // state it is in; the run brings the machine that moved to the front
  // unless a tab was picked by hand.
  function drawMachines() {
    const pane = $("sim-diagrams");
    pane.innerHTML = "";
    const machines = (model && model.machines) || [];
    if (!machines.some(mc => mc.name === shown)) shown = machines[0] ? machines[0].name : null;
    const tabs = document.createElement("div");
    tabs.className = "tabs machine-tabs";
    for (const mc of machines) {
      const b = document.createElement("button");
      b.className = "tab";
      b.dataset.machine = mc.name;
      b.innerHTML = `<span class="kind">machine</span><span class="name">${mc.name}</span><span class="state"></span>`;
      b.onclick = () => { shown = mc.name; follow = false; $("follow").checked = false; pick(); };
      tabs.appendChild(b);
    }
    if (machines.length) {
      const grow = document.createElement("span");
      grow.className = "grow";
      tabs.appendChild(grow);
      const f = document.createElement("label");
      f.className = "follow";
      f.innerHTML = '<input type="checkbox" id="follow"> follow the run';
      f.title = "the tab of the machine that moved last comes to the front";
      tabs.appendChild(f);
      f.querySelector("input").checked = follow;
      f.querySelector("input").onchange = e => { follow = e.target.checked; if (follow) show(); };
      pane.appendChild(tabs);
    }
    for (const mc of machines) {
      const d = document.createElement("div");
      d.className = "machine-pane";
      d.dataset.machine = mc.name;
      d.appendChild(Diagram.process(mc, null));
      pane.appendChild(d);
    }
    pick();
  }

  function pick() {
    for (const t of $("sim-diagrams").querySelectorAll(".machine-tabs .tab")) t.classList.toggle("current", t.dataset.machine === shown);
    for (const d of $("sim-diagrams").querySelectorAll(".machine-pane")) d.hidden = d.dataset.machine !== shown;
  }

  async function run() {
    const scenario = $("scenario").value;
    if (!file || !scenario) return;
    stop();
    $("sim-status").textContent = "running " + scenario + "…";
    const virtual = $("virtual").checked ? "1" : "0";
    const { ok, data } = await api(`/api/run?path=${encodeURIComponent(file)}&scenario=${encodeURIComponent(scenario)}&virtual=${virtual}`);
    if (!ok) { $("sim-status").textContent = data.error || "the run failed"; return; }
    events = data.events || [];
    $("sim-output").textContent = (data.output || []).join("\n");
    $("chart-pane").innerHTML = "";
    $("chart-pane").appendChild(Chart.draw(events));
    $("position").max = events.length;
    pos = events.length;
    $("sim-status").textContent = `${events.length} events, exit ${data.status}`;
    show();
  }

  // Everything up to `pos` shown: the chart's rows, and on the diagrams the
  // state and transition each machine is at.
  function show() {
    $("position").value = pos;
    $("counter").textContent = events.length ? `${pos} / ${events.length}` : "";
    for (const row of document.querySelectorAll("#chart-pane .row")) {
      row.classList.toggle("future", Number(row.dataset.at) >= pos);
    }
    const current = {};   // machine kind -> { state, kind, input }
    for (let i = 0; i < pos; i++) {
      const e = events[i], f = e.fields;
      const kind = e.who.includes("#") ? e.who.slice(0, e.who.indexOf("#")) : e.who;
      switch (e.kind) {
        case "recv": current[kind] = { state: f[0], kind: "input", input: f[1] }; break;
        case "after": current[kind] = { state: f[0], kind: "after", input: f[1] }; break;
        case "when": current[kind] = { state: f[0], kind: "when", input: f[1] }; break;
        case "next": if (current[kind] && f[0] !== "-") current[kind] = { ...current[kind], next: f[0].split("(")[0] }; break;
        case "stop": case "done": if (current[kind]) current[kind] = { ...current[kind], next: null, ended: true }; break;
      }
    }
    const pane = $("sim-diagrams");
    for (const g of pane.querySelectorAll(".sym.lit, .transition.active")) g.classList.remove("lit", "active");
    const last = events[pos - 1];
    const mover = last ? (last.who.includes("#") ? last.who.slice(0, last.who.indexOf("#")) : last.who) : null;
    for (const t of pane.querySelectorAll(".machine-tabs .tab")) {
      const c = current[t.dataset.machine];
      t.querySelector(".state").textContent = c ? (c.ended ? "ended" : (c.next || c.state.split("(")[0])) : "";
      t.classList.toggle("moved", !!mover && t.dataset.machine === mover);
    }
    if (follow && mover && pane.querySelector(`.machine-pane[data-machine="${mover}"]`) && mover !== shown) { shown = mover; pick(); }
    for (const kind in current) {
      const c = current[kind];
      for (const g of pane.querySelectorAll(`.sym[data-machine="${kind}"]`)) {
        const here = g.dataset.pattern === c.state;
        const gone = c.next && g.dataset.state === c.next;
        if (here || gone) g.classList.add("lit");
      }
      for (const t of pane.querySelectorAll(`.transition[data-machine="${kind}"]`)) {
        if (t.dataset.state === c.state && t.dataset.kind === c.kind && t.dataset.input === c.input) t.classList.add("active");
      }
    }
    $("event").textContent = last ? `${last.seq}  ${last.kind}  ${last.who}  ${last.fields.join("  ")}` : "";
    const row = document.querySelector(`#chart-pane .row[data-at="${pos - 1}"]`);
    if (row) row.scrollIntoView({ block: "nearest" });
  }

  function step(d) { pos = Math.max(0, Math.min(events.length, pos + d)); show(); }
  function play() {
    if (timer) { stop(); return; }
    if (pos >= events.length) pos = 0;
    $("btn-play").textContent = "❚❚";
    timer = setInterval(() => { if (pos >= events.length) stop(); else step(1); }, 400);
  }
  function stop() { if (timer) clearInterval(timer); timer = null; $("btn-play").textContent = "▶"; }

  function wire() {
    $("btn-run").onclick = run;
    $("btn-play").onclick = play;
    $("btn-back").onclick = () => { stop(); step(-1); };
    $("btn-fwd").onclick = () => { stop(); step(1); };
    $("btn-start").onclick = () => { stop(); pos = 0; show(); };
    $("btn-end").onclick = () => { stop(); pos = events.length; show(); };
    $("position").oninput = () => { stop(); pos = Number($("position").value); show(); };
    for (const g of document.querySelectorAll("#sim-diagrams")) g.addEventListener("click", ev => {
      const s = ev.target.closest(".sym[data-line]");
      if (s && window.goToLine) window.goToLine(Number(s.dataset.line));
    });
  }

  return { load, wire };
})();
