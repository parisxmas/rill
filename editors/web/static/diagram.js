// The diagrams, drawn from the model `rill diagram --json` gives: the
// system as SDL's block diagram, and each machine as SDL drew a process —
// the state at the top, a column per input, the symbols top-down, a
// decision forking into columns, the next state or the stop mark at the
// bottom. The layout is the one the command line draws, so the page and
// a document show the same picture; every symbol carries the line it was
// written on, which is how a click finds the code.

const Diagram = (() => {
  const W = 160, W_MOST = 340, CHAR = 7.3, H = 40, GAP_X = 24, GAP_Y = 22, PAD = 30, STATE_GAP = 60, LABEL = 18;
  const SVG = "http://www.w3.org/2000/svg";

  function el(name, attrs, parent) {
    const e = document.createElementNS(SVG, name);
    for (const k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  }

  function shown(t) {
    const most = Math.floor((W_MOST - 40) / CHAR);
    return t.length > most ? t.slice(0, most - 1) + "…" : t;
  }

  function text(parent, x, y, t, anchor) {
    const e = el("text", { x, y, "text-anchor": anchor }, parent);
    const title = el("title", {}, e);
    title.textContent = t;
    e.appendChild(document.createTextNode(shown(t)));
    return e;
  }

  function line(parent, x1, y1, x2, y2) {
    el("line", { x1, y1, x2, y2, stroke: "black", "stroke-width": 1.2 }, parent);
  }

  // ---- the process diagram -------------------------------------------------

  // A symbol: its kind, its text, and the line it came from.
  function sym(kind, t, line) { return { kind, text: t, line }; }

  function symWidth(s) {
    const t = s.kind === "stop" ? "" : s.text;
    return Math.min(Math.max(t.length * CHAR + 40, W), W_MOST);
  }

  // A column of symbols top-down, which may fork under a decision.
  function columnOf(steps, end, line) {
    const syms = steps.map(s =>
      s.kind === "output" ? sym("output", `${s.signal} → ${s.to}`, s.line || line)
      : s.kind === "timer" ? sym("timertask", s.text, s.line || line)
      : sym("task", s.text, s.line || line));
    const forks = [];
    if (end.kind === "next") syms.push(sym("state", end.text, line));
    else if (end.kind === "stay") syms.push(sym("state", "-", line));
    else if (end.kind === "done") syms.push(sym("state", end.text, line));
    else if (end.kind === "stop") syms.push(sym("stop", "", line));
    else if (end.kind === "decision") {
      syms.push(sym("decision", end.question, line));
      for (const b of end.branches) forks.push([b.label, columnOf(b.steps, b.end, line)]);
    }
    return { syms, forks };
  }

  function own(c) { return c.syms.reduce((w, s) => Math.max(w, symWidth(s)), W); }
  function width(c) {
    if (c.forks.length === 0) return own(c);
    const sum = c.forks.reduce((a, [, f]) => a + width(f), 0);
    return Math.max(sum + GAP_X * (c.forks.length - 1), own(c));
  }
  function height(c) {
    const mine = c.syms.length * (H + GAP_Y);
    const below = c.forks.reduce((a, [, f]) => Math.max(a, height(f)), 0);
    return mine + (c.forks.length ? below + LABEL : 0);
  }

  function draw(parent, s, x, y, w) {
    const x2 = x + w, y2 = y + H, ym = y + H / 2;
    const g = el("g", { class: "sym", "data-line": s.line }, parent);
    const style = { fill: "white", stroke: "black", "stroke-width": 1.2, class: "shape" };
    switch (s.kind) {
      case "state":
        el("rect", { ...style, x, y, width: w, height: H, rx: 14 }, g);
        text(g, x + w / 2, ym + 4, s.text, "middle");
        break;
      case "input":
        el("polygon", { ...style, points: `${x},${y} ${x2},${y} ${x2},${y2} ${x},${y2} ${x + 12},${ym}` }, g);
        text(g, x + w / 2 + 6, ym + 4, s.text, "middle");
        break;
      case "timer":
        el("polygon", { ...style, points: `${x},${y} ${x2},${y} ${x2},${y2} ${x},${y2} ${x + 12},${ym}` }, g);
        el("polygon", { points: `${x + 16},${y + 8} ${x + 26},${y + 8} ${x + 16},${y2 - 8} ${x + 26},${y2 - 8}`, fill: "black" }, g);
        text(g, x + w / 2 + 12, ym + 4, s.text, "middle");
        break;
      case "when":
        el("polygon", { ...style, points: `${x},${y} ${x2},${y} ${x2},${y2} ${x},${y2} ${x + 12},${ym}`, "stroke-dasharray": "5,3" }, g);
        text(g, x + w / 2 + 6, ym + 4, s.text, "middle");
        break;
      case "output":
        el("polygon", { ...style, points: `${x},${y} ${x2 - 12},${y} ${x2},${ym} ${x2 - 12},${y2} ${x},${y2}` }, g);
        text(g, x + w / 2 - 6, ym + 4, s.text, "middle");
        break;
      case "task":
        el("rect", { ...style, x, y, width: w, height: H }, g);
        text(g, x + w / 2, ym + 4, s.text, "middle");
        break;
      case "timertask":
        el("rect", { ...style, x, y, width: w, height: H }, g);
        el("polygon", { points: `${x + 8},${y + 8} ${x + 18},${y + 8} ${x + 8},${y2 - 8} ${x + 18},${y2 - 8}`, fill: "black" }, g);
        text(g, x + w / 2 + 6, ym + 4, s.text, "middle");
        break;
      case "save":
        el("polygon", { ...style, points: `${x},${y} ${x2},${y} ${x2 - 12},${ym} ${x2},${y2} ${x},${y2}` }, g);
        text(g, x + w / 2 - 6, ym + 4, "save " + s.text, "middle");
        break;
      case "priority":
        el("polygon", { ...style, points: `${x},${y} ${x2},${y} ${x2},${y2} ${x},${y2} ${x + 12},${ym}` }, g);
        el("polygon", { ...style, points: `${x + 6},${y + 6} ${x2 - 6},${y + 6} ${x2 - 6},${y2 - 6} ${x + 6},${y2 - 6} ${x + 16},${ym}`, fill: "none" }, g);
        text(g, x + w / 2 + 8, ym + 4, s.text, "middle");
        break;
      case "decision": {
        const cx = x + w / 2;
        el("polygon", { ...style, points: `${cx},${y} ${x2},${ym} ${cx},${y2} ${x},${ym}` }, g);
        text(g, cx, ym + 4, s.text, "middle");
        break;
      }
      case "stop": {
        const cx = x + w / 2;
        el("line", { x1: cx - 12, y1: y + 6, x2: cx + 12, y2: y2 - 6, stroke: "black", "stroke-width": 2, class: "shape" }, g);
        el("line", { x1: cx + 12, y1: y + 6, x2: cx - 12, y2: y2 - 6, stroke: "black", "stroke-width": 2 }, g);
        break;
      }
      case "add-input":
        g.classList.add("adder");
        el("polygon", { ...style, points: `${x},${y} ${x2},${y} ${x2},${y2} ${x},${y2} ${x + 12},${ym}`, "stroke-dasharray": "4,3", stroke: "#999" }, g);
        text(g, x + w / 2 + 6, ym + 4, "+ input", "middle");
        break;
      case "add-state":
        g.classList.add("adder");
        el("rect", { ...style, x, y, width: w, height: H, rx: 14, "stroke-dasharray": "4,3", stroke: "#999" }, g);
        text(g, x + w / 2, ym + 4, "+ state", "middle");
        break;
    }
    return g;
  }

  function drawColumn(parent, c, x, y) {
    const cx = x + width(c) / 2, w = own(c), sx = cx - w / 2;
    let yy = y;
    c.syms.forEach((s, i) => {
      if (i > 0) line(parent, cx, yy - GAP_Y, cx, yy);
      draw(parent, s, sx, yy, w);
      yy += H + GAP_Y;
    });
    if (c.forks.length) {
      const total = c.forks.reduce((a, [, f]) => a + width(f), 0) + GAP_X * (c.forks.length - 1);
      let fx = cx - total / 2;
      const busY = yy - GAP_Y / 2, top = yy + LABEL;
      const first = fx + width(c.forks[0][1]) / 2;
      const last = fx + total - width(c.forks[c.forks.length - 1][1]) / 2;
      line(parent, cx, yy - GAP_Y, cx, busY);
      line(parent, first, busY, last, busY);
      for (const [label, f] of c.forks) {
        const fcx = fx + width(f) / 2;
        line(parent, fcx, busY, fcx, top);
        text(parent, fcx + 5, busY + 15, label, "start");
        drawColumn(parent, f, fx, top);
        fx += width(f) + GAP_X;
      }
    }
  }

  // The machine as one SVG. `notes` maps a state's line to how many notes
  // `rill check` had for it, shown as a badge on the state.
  // With `editable`, each state ends in a dashed "+ input" and the machine
  // in a dashed "+ state", for the page to hang its edits on.
  function process(m, notes, editable) {
    const blocks = m.states.map(st => {
      const cols = st.transitions.map(t => {
        const c = columnOf(t.steps, t.end, t.line);
        c.syms.unshift(sym(t.kind === "after" ? "timer" : t.kind === "when" ? "when" : "input", t.input, t.line));
        // what a replay matches a trace event against: the kind, and the
        // input's pattern without its guard, the `after`'s wait, the
        // `when`'s condition
        c.key = { kind: t.kind, input: t.kind === "input" ? t.input.split(" if ")[0] : t.kind === "after" ? t.input.replace(/^after | ms$/g, "") : t.input.replace(/^when /, "") };
        return c;
      });
      for (const s of st.saves) cols.push({ syms: [sym("save", s.text, s.line)], forks: [] });
      for (const p of st.priorities) cols.push({ syms: [sym("priority", "priority " + p.text, p.line)], forks: [] });
      if (editable) {
        const c = { syms: [sym("add-input", "", st.line)], forks: [] };
        c.adder = { state: st.name, line: st.line };
        cols.push(c);
      }
      return { st, cols };
    });
    const svg = el("svg", { xmlns: SVG });
    const title = el("title", {}, svg);
    title.textContent = "machine " + m.name;
    let y = PAD, wide = W + 2 * PAD;
    for (const { st, cols } of blocks) {
      const colsW = cols.reduce((a, c) => a + width(c), 0) + GAP_X * Math.max(cols.length - 1, 0);
      const blockW = Math.max(colsW, W);
      wide = Math.max(wide, blockW + 2 * PAD);
      const sx = PAD + (blockW - W) / 2;
      const g = draw(svg, sym("state", st.pattern === "*" ? "* (every state)" : st.pattern, st.line), sx, y, W);
      g.dataset.machine = m.name;
      g.dataset.state = st.name;
      g.dataset.pattern = st.pattern;
      const n = notes && notes[st.line];
      if (n) {
        g.classList.add("noted");
        el("circle", { cx: sx + W - 4, cy: y + 4, r: 9, class: "badge" }, g);
        const t = el("text", { x: sx + W - 4, y: y + 8, "text-anchor": "middle", class: "badge-text" }, g);
        t.textContent = String(n);
      }
      if (cols.length) {
        const busY = y + H + GAP_Y / 2;
        line(svg, sx + W / 2, y + H, sx + W / 2, busY);
        let x = PAD + (blockW - colsW) / 2;
        const firstX = x + width(cols[0]) / 2, lastX = x + colsW - width(cols[cols.length - 1]) / 2;
        if (cols.length > 1) line(svg, firstX, busY, lastX, busY);
        let tallest = 0;
        for (const c of cols) {
          const cx = x + width(c) / 2;
          line(svg, cx, busY, cx, y + H + GAP_Y);
          const tg = el("g", { class: "transition" }, svg);
          if (c.adder) {
            tg.dataset.addInput = c.adder.state;
            tg.dataset.stateLine = c.adder.line;
          }
          if (c.key) {
            tg.dataset.machine = m.name;
            tg.dataset.state = st.pattern;
            tg.dataset.kind = c.key.kind;
            tg.dataset.input = c.key.input;
          }
          drawColumn(tg, c, x, y + H + GAP_Y);
          tallest = Math.max(tallest, height(c));
          x += width(c) + GAP_X;
        }
        y += H + GAP_Y + tallest + STATE_GAP;
      } else {
        y += H + STATE_GAP;
      }
    }
    if (editable) {
      const g = draw(svg, sym("add-state", "", m.line), PAD, y - STATE_GAP + GAP_Y, W);
      g.dataset.addState = m.name;
      y += H + GAP_Y;
    }
    const high = y - STATE_GAP + PAD;
    svg.setAttribute("width", wide);
    svg.setAttribute("height", high);
    svg.setAttribute("viewBox", `0 0 ${wide} ${high}`);
    return svg;
  }

  // ---- the block diagram -----------------------------------------------------

  function system(sd) {
    const ends = [];
    for (const r of sd.routes) for (const e of [r.from, r.to]) if (!ends.includes(e)) ends.push(e);
    ends.sort((a, b) => (a === "env" ? 0 : 1) - (b === "env" ? 0 : 1));
    const COL = 260, BOX_W = 150, BOX_H = 50, SIDE = 50;
    const arcs = sd.routes.length;
    const top = 40 + 34 * Math.ceil(arcs / 2);
    const wide = SIDE * 2 + COL * Math.max(ends.length, 1);
    const high = top + BOX_H + 60 + 34 * Math.ceil(arcs / 2);
    const xOf = name => SIDE + COL / 2 + ends.indexOf(name) * COL;
    const svg = el("svg", { xmlns: SVG, width: wide, height: high, viewBox: `0 0 ${wide} ${high}` });
    const title = el("title", {}, svg);
    title.textContent = "system " + sd.name;
    const defs = el("defs", {}, svg);
    for (const [id, path, refX] of [["arrow", "M0,0 L10,4 L0,8 z", 9], ["arrow_back", "M10,0 L0,4 L10,8 z", 1]]) {
      const m = el("marker", { id, markerWidth: 10, markerHeight: 8, refX, refY: 4, orient: "auto" }, defs);
      el("path", { d: path, fill: "black" }, m);
    }
    for (const e of ends) {
      const x = xOf(e);
      const g = el("g", { class: "sym", "data-line": sd.line, "data-end": e }, svg);
      if (e === "env") {
        el("rect", { x: x - BOX_W / 2, y: top, width: BOX_W, height: BOX_H, rx: 25, fill: "white", stroke: "black", "stroke-width": 1.2, "stroke-dasharray": "6,4", class: "shape" }, g);
        text(g, x, top + BOX_H / 2 + 4, "environment", "middle");
      } else {
        el("rect", { x: x - BOX_W / 2, y: top, width: BOX_W, height: BOX_H, fill: "white", stroke: "black", "stroke-width": 1.2, class: "shape" }, g);
        text(g, x, top + BOX_H / 2 + 4, e, "middle");
      }
    }
    // Routes as SDL drew channels: up from one box, across in a lane of
    // their own, down into the other; the first above, the next below,
    // and so on outward, so no two labels share a line.
    const ends_at = {};
    sd.routes.forEach((r, i) => {
      const label = r.signals.join(", ");
      const x1 = xOf(r.from), x2 = xOf(r.to);
      const above = i % 2 === 0, lift = 30 + 34 * Math.floor(i / 2);
      // several routes at one box spread along its top or bottom edge
      const slot = end => { const k = end + (above ? "^" : "v"); ends_at[k] = (ends_at[k] || 0) + 1; return ends_at[k]; };
      const dx1 = (slot(r.from) - 1) * 14 - 21, dx2 = (slot(r.to) - 1) * 14 + 21;
      const g = el("g", { class: "sym route", "data-line": r.line, "data-route": i }, svg);
      const markers = r.both ? { "marker-start": "url(#arrow_back)", "marker-end": "url(#arrow)" } : { "marker-end": "url(#arrow)" };
      const style = { fill: "none", stroke: "black", "stroke-width": 1.2, class: "shape", ...markers };
      if (r.from === r.to) {
        const y = above ? top : top + BOX_H, ly = above ? y - lift : y + lift;
        el("path", { ...style, d: `M${x1 - 30},${y} V${ly} H${x1 + 30} V${y}` }, g);
        text(g, x1, above ? ly - 5 : ly + 14, label, "middle");
      } else {
        const y = above ? top : top + BOX_H, ly = above ? y - lift : y + lift;
        el("path", { ...style, d: `M${x1 + dx1},${y} V${ly} H${x2 + dx2 - (x2 > x1 ? 42 : 0)} V${y}` }, g);
        text(g, (x1 + x2) / 2, above ? ly - 5 : ly + 14, label, "middle");
      }
    });
    return svg;
  }

  return { process, system };
})();
