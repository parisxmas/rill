// The simulation view: a scenario of the file — `main`, or a `test_`
// function — run on the server with the trace on, and replayed here: the
// chart grows event by event, and on the process diagrams the state each
// machine is in and the transition it is taking light up. Play, pause,
// step, or drag to a moment.

const Simulate = (() => {
  const $ = id => document.getElementById(id);
  let events = [], pos = 0, timer = null, model = null, file = null;

  const api = async path => {
    const res = await fetch(path);
    const text = await res.text();
    try { return { ok: res.ok, data: JSON.parse(text) }; } catch (_) { return { ok: res.ok, data: { error: text } }; }
  };

  // The file changed: its scenarios, and its machines drawn again.
  async function load(path, m) {
    file = path;
    model = m;
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

  function drawMachines() {
    const pane = $("sim-diagrams");
    pane.innerHTML = "";
    for (const mc of (model && model.machines) || []) {
      const h = document.createElement("h2");
      h.innerHTML = `<span class="kind">machine</span>${mc.name}`;
      pane.appendChild(h);
      pane.appendChild(Diagram.process(mc, null));
    }
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
    const last = events[pos - 1];
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
