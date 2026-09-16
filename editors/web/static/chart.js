// A run's trace as the message sequence chart, built to be replayed: a
// lifeline per machine and per mailbox of the environment, an arrow per
// signal, a note per state change — each row knowing which event it came
// from, so the chart can be shown up to any moment of the run. The rules
// are the command line's (`rill msc`): a signal is drawn once, from the
// sender's `send` between machines and from the receiver's `recv` when it
// came from the environment, which logs nothing of its own.

const Chart = (() => {
  const SVG = "http://www.w3.org/2000/svg";
  const COL = 200, ROW = 34, HEAD = 40, TOP = 20, SIDE = 30;

  function el(name, attrs, parent) {
    const e = document.createElementNS(SVG, name);
    for (const k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  }

  // The lifelines and the items of a trace.
  function build(events) {
    const machines = {};
    for (const e of events) {
      if (e.kind !== "post") {
        const at = e.who.lastIndexOf("#");
        if (at >= 0) machines[e.who.slice(at + 1)] = e.who;
      }
    }
    const lifeline = id => machines[id] || "env#" + id;
    const order = [];
    const seen = name => { if (!order.includes(name)) order.push(name); };
    for (const e of events) {
      const f = e.fields;
      switch (e.kind) {
        case "post": seen(lifeline(e.who)); if (f[0]) seen(lifeline(f[0])); break;
        case "recv": case "drop": if (f.length) seen(lifeline(f[f.length - 1])); seen(e.who); break;
        case "send": seen(e.who); if (f[1]) seen(lifeline(f[1])); break;
        default: seen(e.who);
      }
    }
    const items = [];
    const lastState = {};
    events.forEach((e, i) => {
      const f = e.fields, me = e.who;
      const arrow = (from, to, label) => items.push({ kind: "arrow", from, to, label, at: i });
      const note = (who, text) => items.push({ kind: "note", who, text, at: i });
      switch (e.kind) {
        case "recv": case "drop": {
          const state = f[0], sig = e.kind === "recv" ? f[1] : null, fromId = e.kind === "recv" ? f[2] : f[1];
          const from = lifeline(fromId || "");
          const label = sig || "a signal";
          if (from === me) arrow(me, me, label + " (timer)");
          else if (!machines[fromId]) arrow(from, me, label);
          if (e.kind === "drop") note(me, "dropped in " + state);
          if (state) lastState[me] = state;
          break;
        }
        case "send": arrow(me, lifeline(f[1] || ""), f[0] || ""); break;
        case "next": if (f[0] !== "-") note(me, (lastState[me] || "") + " → " + f[0]); break;
        case "stop": note(me, "stop"); break;
        case "done": note(me, "done " + (f[0] || "")); break;
        case "after": note(me, "after " + f[1] + " ms in " + f[0]); lastState[me] = f[0]; break;
        case "when": note(me, "when " + f[1]); lastState[me] = f[0]; break;
        case "set": note(me, "set " + f[0] + " " + f[1] + " ms"); break;
        case "reset": note(me, "reset " + f[0]); break;
        case "save": note(me, "saved in " + f[0]); break;
      }
    });
    return { order, items };
  }

  // The chart as SVG; every row carries `data-at`, the event it came from.
  function draw(events) {
    const { order, items } = build(events);
    const xOf = name => SIDE + COL / 2 + order.indexOf(name) * COL;
    const width = SIDE * 2 + COL * Math.max(order.length, 1);
    const height = TOP + HEAD + ROW * (items.length + 1) + 20;
    const svg = el("svg", { xmlns: SVG, width, height, viewBox: `0 0 ${width} ${height}`, class: "chart" });
    const defs = el("defs", {}, svg);
    const m = el("marker", { id: "chart_arrow", markerWidth: 10, markerHeight: 8, refX: 9, refY: 4, orient: "auto" }, defs);
    el("path", { d: "M0,0 L10,4 L0,8 z", fill: "black" }, m);
    const bottom = TOP + HEAD + ROW * (items.length + 1);
    for (const name of order) {
      const x = xOf(name);
      el("rect", { x: x - COL / 2 + 10, y: TOP, width: COL - 20, height: HEAD, fill: "white", stroke: "black", "stroke-width": 1.2 }, svg);
      const t = el("text", { x, y: TOP + HEAD / 2 + 4, "text-anchor": "middle" }, svg);
      t.textContent = name;
      el("line", { x1: x, y1: TOP + HEAD, x2: x, y2: bottom, stroke: "black", "stroke-width": 1.2 }, svg);
    }
    let y = TOP + HEAD + ROW;
    for (const item of items) {
      const g = el("g", { class: "row", "data-at": item.at }, svg);
      if (item.kind === "arrow") {
        const x1 = xOf(item.from), x2 = xOf(item.to);
        if (item.from === item.to) {
          el("path", { d: `M${x1},${y - 7} h 30 v 14 h -30`, fill: "none", stroke: "black", "stroke-width": 1.2, "marker-end": "url(#chart_arrow)" }, g);
          const t = el("text", { x: x1 + 36, y: y + 4, "text-anchor": "start" }, g);
          t.textContent = item.label;
        } else {
          el("line", { x1, y1: y, x2, y2: y, stroke: "black", "stroke-width": 1.2, "marker-end": "url(#chart_arrow)" }, g);
          const t = el("text", { x: (x1 + x2) / 2, y: y - 5, "text-anchor": "middle" }, g);
          t.textContent = item.label;
        }
      } else {
        const x = xOf(item.who);
        const shown = item.text.length > 40 ? item.text.slice(0, 39) + "…" : item.text;
        const w = Math.max(shown.length * 7.3 + 16, 60);
        el("rect", { x: x - w / 2, y: y - 11, width: w, height: 22, fill: "#fff8dc", stroke: "black", "stroke-width": 1 }, g);
        const t = el("text", { x, y: y + 4, "text-anchor": "middle" }, g);
        const title = el("title", {}, t);
        title.textContent = item.text;
        t.appendChild(document.createTextNode(shown));
      }
      y += ROW;
    }
    return svg;
  }

  return { build, draw };
})();
