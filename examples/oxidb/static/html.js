/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// html — an Angular-style declarative template layer on top of zap's signals.
// You write markup, and interpolate signals DIRECTLY (no `() =>` wrapper):
//
//   html`<button onclick=${inc}>+</button> <span>${count}</span>`
//
// A signal passed into a ${...} hole is auto-tracked, just like {{ count() }}
// in an Angular template. Static values stay static. Built on zap's effect().

import { effect, setAttr, model } from "./zap.js";

// Local dev warnings (mirrors zap core; disable with globalThis.__ZAP_DEV__=false)
const DEV = typeof globalThis === "undefined" || globalThis.__ZAP_DEV__ !== false;
const _warned = new Set();
function warn(msg) {
  if (!DEV || _warned.has(msg)) return;
  _warned.add(msg);
  console.warn("[zap] " + msg);
}

export function html(strings, ...values) {
  let acc = "";
  const holes = [];
  let pendingQuote = null; // closing quote of a quoted attr to drop from next chunk

  for (let i = 0; i < strings.length; i++) {
    let s = strings[i];
    if (pendingQuote && s[0] === pendingQuote) s = s.slice(1);
    pendingQuote = null;
    acc += s;
    if (i >= values.length) continue;

    const insideTag = acc.lastIndexOf("<") > acc.lastIndexOf(">");
    if (insideTag) {
      // attribute hole:  onclick=${fn} / class=${expr}  (unquoted, preferred)
      let m = acc.match(/([^\s"'=<>]+)=$/);
      if (!m) {
        // quoted form:  class="${x}"  — supported, but nudge toward unquoted
        m = acc.match(/([^\s"'=<>]+)=(["'])$/);
        if (m) {
          pendingQuote = m[2];
          warn(`quoted dynamic attribute ${m[1]}="\${...}" — prefer unquoted ${m[1]}=\${...}`);
        }
      }
      if (!m) {
        warn("could not parse an attribute interpolation near: …" + acc.slice(-24));
        acc += `data-zh-${i}`;
        holes.push({ type: "attr", i, name: "" });
        continue;
      }
      const name = m[1];
      acc = acc.slice(0, m.index) + `data-zh-${i}`; // unique marker attribute
      holes.push({ type: "attr", i, name });
    } else {
      // child hole — anchor with a comment we can find after parsing
      acc += `<!--zh:${i}-->`;
      holes.push({ type: "child", i });
    }
  }

  const tpl = document.createElement("template");
  tpl.innerHTML = acc;
  const frag = tpl.content;

  for (const hole of holes) {
    if (hole.type === "attr") {
      const el = frag.querySelector(`[data-zh-${hole.i}]`);
      el.removeAttribute(`data-zh-${hole.i}`);
      bindAttr(el, hole.name, values[hole.i]);
    }
  }

  // child holes: walk comments once
  const walker = document.createTreeWalker(frag, NodeFilter.SHOW_COMMENT);
  const comments = [];
  while (walker.nextNode()) comments.push(walker.currentNode);
  for (const c of comments) {
    const m = c.data.match(/^zh:(\d+)$/);
    if (m) bindChild(c, values[+m[1]]);
  }

  // Return the single root if there's exactly one, else the fragment.
  return frag.childNodes.length === 1 ? frag.firstChild : frag;
}

function bindAttr(el, name, value) {
  if (name === "bind") {
    model(el, value); // two-way binding:  <input bind=${signal}>
  } else if (name.startsWith("on") && typeof value === "function") {
    el.addEventListener(name.slice(2).toLowerCase(), value);
  } else if (typeof value === "function") {
    effect(() => setAttr(el, name, value())); // signal → reactive attribute
  } else {
    setAttr(el, name, value);
  }
}

function bindChild(anchor, value) {
  if (typeof value === "function") {
    // signal / computed / any reactive getter → reactive text, node, OR array.
    // A getter returning a bare array of nodes (`${() => rows.map(r => html`…`)}`)
    // is rendered node-by-node between an end marker, re-rendered on each change.
    const end = document.createComment("zh");
    anchor.replaceWith(end);
    let rendered = [];
    effect(() => {
      if (!end.parentNode) return;
      const v = value();
      // Taking a node out of the document can run a handler on the spot — an
      // input being removed blurs, and a blur handler may set a signal, which
      // re-enters this very effect. So the list is handed over before any of
      // it is removed, and a node already gone is left alone: without that,
      // the inner run removes what the outer run is still holding and the
      // outer one throws, halfway through replacing the cell.
      const going = rendered;
      rendered = [];
      for (const n of going) if (n.parentNode) n.remove();
      const collect = (x) => {
        if (x == null || x === false || x === true) return;
        if (x instanceof Node) rendered.push(x);
        else if (Array.isArray(x)) x.forEach(collect);
        else rendered.push(document.createTextNode(String(x)));
      };
      collect(v);
      for (const n of rendered) end.parentNode.insertBefore(n, end);
    });
  } else if (value instanceof Node) {
    anchor.replaceWith(value);
  } else if (Array.isArray(value)) {
    const f = document.createDocumentFragment();
    for (const v of value) f.appendChild(v instanceof Node ? v : document.createTextNode(v == null ? "" : v));
    anchor.replaceWith(f);
  } else {
    anchor.replaceWith(document.createTextNode(value == null ? "" : value));
  }
}
