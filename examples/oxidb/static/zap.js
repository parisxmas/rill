/* Vendored from zap (https://github.com/barisakin/zap), MIT, unmodified.
   The catalogue serves it, so the example stands on its own. */
// zap — a tiny, fast, fine-grained reactive UI library (no virtual DOM)
// Core idea (like Solid/Svelte): signals track exactly which DOM nodes depend
// on them, so updates touch only what changed. No diffing, no re-renders.

// --- Dev warnings ----------------------------------------------------------
// On by default. Disable in production with: globalThis.__ZAP_DEV__ = false
const DEV = typeof globalThis === "undefined" || globalThis.__ZAP_DEV__ !== false;
const _warned = new Set();
function warn(msg) {
  if (!DEV || _warned.has(msg)) return;
  _warned.add(msg);
  console.warn("[zap] " + msg);
}

// --- Reactivity core -------------------------------------------------------
// Two ambient contexts:
//   activeEffect  → the computation currently TRACKING reads (for subscriptions)
//   currentOwner  → the scope currently OWNING new effects/cleanups (for disposal)
let activeEffect = null;
let currentOwner = null;

function makeScope(owner) {
  return { deps: new Set(), cleanups: [], owned: [], owner };
}

// Dispose a scope's contents: tear down child scopes, run cleanups, unsubscribe.
// Leaves the scope object reusable (used for both effect re-runs and full dispose).
function disposeScope(s) {
  for (const child of s.owned) disposeScope(child);
  s.owned.length = 0;
  for (let i = s.cleanups.length - 1; i >= 0; i--) s.cleanups[i]();
  s.cleanups.length = 0;
  for (const dep of s.deps) dep.delete(s);
  s.deps.clear();
}

export function signal(value) {
  const subscribers = new Set();

  const read = () => {
    if (activeEffect) {
      subscribers.add(activeEffect);
      activeEffect.deps.add(subscribers);
    }
    return value;
  };

  read.set = (next) => {
    next = typeof next === "function" ? next(value) : next;
    if (Object.is(next, value)) return;
    value = next;
    // Copy: effects may resubscribe while we notify.
    for (const eff of [...subscribers]) eff.run();
  };

  read.update = (fn) => read.set(fn(value));
  read.peek = () => value;
  return read;
}

// store(initial) — a reactive object for shared / app-wide state. Each top-level
// key becomes its own fine-grained signal: reading `s.k` inside an effect
// subscribes to JUST that key, and writing `s.k = v` updates only the nodes that
// read it (Object.is-equal writes are no-ops). This is the fast way to hold
// structured state — pair it with context (provide/useContext) for app-wide
// state, or just use a module-level singleton. Derive with `computed`, mutate
// with plain functions ("actions"). For nested data, replace the whole value
// (like signal: `s.user = { ...s.user, name }`, not `s.user.name = ...`).
export function store(initial) {
  const sigs = Object.create(null);
  for (const k in initial) sigs[k] = signal(initial[k]);
  return new Proxy(sigs, {
    get(t, k) {
      const s = t[k];
      return s ? s() : undefined; // reactive read (subscribes the active effect)
    },
    set(t, k, v) {
      (t[k] || (t[k] = signal())).set(v); // create-on-write for new keys
      return true;
    },
    has: (t, k) => k in t,
    ownKeys: (t) => Reflect.ownKeys(t),
    getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }),
  });
}

export function effect(fn) {
  const c = makeScope(currentOwner);
  c.fn = fn;
  if (currentOwner) currentOwner.owned.push(c);
  c.run = () => {
    disposeScope(c); // tear down previous run (children + cleanups + deps)
    const prevE = activeEffect, prevO = currentOwner;
    activeEffect = c;
    currentOwner = c;
    try {
      const r = c.fn();
      if (typeof r === "function") c.cleanups.push(r); // effect(fn) may return a cleanup
    } finally {
      activeEffect = prevE;
      currentOwner = prevO;
    }
  };
  c.run();
  return () => disposeScope(c);
}

// Run fn() WITHOUT subscribing the current effect to anything it reads.
export function untrack(fn) {
  const prev = activeEffect;
  activeEffect = null;
  try {
    return fn();
  } finally {
    activeEffect = prev;
  }
}

// Register a callback to run when the current scope is torn down (re-run or
// disposed). Like Solid's onCleanup / Vue's onUnmounted / Svelte's onDestroy.
export function onCleanup(fn) {
  if (currentOwner) currentOwner.cleanups.push(fn);
  // Outside any owner there is nothing to tear down — silently ignore.
}

// Create a detached reactive scope. fn receives a dispose() handle and its
// return value is passed back. Everything created inside is torn down on dispose.
// Detached for DISPOSAL (not pushed to prev.owned — its lifetime is managed via
// the returned dispose), but it inherits `prev` as its owner so context lookups
// (useContext) keep walking up past control-flow branches to outer providers.
export function createRoot(fn) {
  const prev = currentOwner;
  const root = makeScope(prev);
  currentOwner = root;
  try {
    return fn(() => disposeScope(root));
  } finally {
    currentOwner = prev;
  }
}

export function computed(fn) {
  const s = signal(undefined);
  effect(() => s.set(fn()));
  return s;
}

// resource(fetcher) | resource(source, fetcher) — bridge async data (fetch, etc.)
// into signals. Returns { data, loading, error, refetch }. With a reactive
// `source` getter, it re-fetches whenever the source changes (e.g. an id), and a
// version guard drops stale responses so the latest request always wins. Use the
// platform `fetch` as the fetcher — no HTTP-client dependency needed:
//
//   const users = resource(() => fetch("/api/users").then(r => r.json()));
//   when(users.loading, ...);  ${() => users.error() ? "…" : users.data()}
//   const user = resource(id, (id) => fetch(`/api/users/${id}`).then(r => r.json()));
export function resource(source, fetcher) {
  if (fetcher === undefined) { fetcher = source; source = null; }
  const data = signal(undefined);
  const error = signal(null);
  const loading = signal(false);
  let version = 0;

  const refetch = () => {
    const v = ++version;
    loading.set(true);
    error.set(null);
    Promise.resolve()
      .then(() => fetcher(source ? untrack(source) : undefined))
      .then(
        (res) => { if (v === version) { data.set(res); loading.set(false); } },
        (err) => { if (v === version) { error.set(err); loading.set(false); } }
      );
  };

  // re-run when the (tracked) source changes; with no source, runs once.
  effect(() => { if (source) source(); refetch(); });

  return { data, loading, error, refetch };
}

// --- DOM ------------------------------------------------------------------

// h("div", { class: () => cls(), onclick: fn }, child1, child2, ...)
export function h(tag, props, ...children) {
  const el = document.createElement(tag);

  for (const key in props) {
    const val = props[key];
    if (key === "bind") {
      model(el, val); // two-way binding: { bind: someSignal }
    } else if (key.startsWith("on") && typeof val === "function") {
      el.addEventListener(key.slice(2).toLowerCase(), val);
    } else if (typeof val === "function") {
      // reactive attribute/property
      effect(() => setAttr(el, key, val()));
    } else {
      setAttr(el, key, val);
    }
  }

  for (const child of children) append(el, child);
  return el;
}

// Two-way binding for form elements: keeps `sig` and the element in sync.
// Works with text/textarea/select, checkbox, radio, number/range.
export function model(el, sig) {
  if (DEV && (typeof sig !== "function" || typeof sig.set !== "function")) {
    warn("bind/model expects a signal, got " + typeof sig + ". Use bind=${aSignal}.");
    return;
  }
  const t = el.type;
  if (t === "radio") {
    effect(() => { el.checked = sig() === el.value; });
    el.addEventListener("change", () => { if (el.checked) sig.set(el.value); });
    return;
  }
  if (t === "checkbox") {
    effect(() => { if (el.checked !== !!sig()) el.checked = !!sig(); });
    el.addEventListener("change", () => sig.set(el.checked));
    return;
  }
  const num = t === "number" || t === "range";
  effect(() => {
    const s = sig() == null ? "" : String(sig());
    // guard avoids cursor jumps while typing; try/catch tolerates getter-only
    // value props (e.g. some inputs under a server DOM like linkedom).
    if (el.value !== s) { try { el.value = s; } catch { el.setAttribute("value", s); } }
  });
  const evt = el.tagName === "SELECT" ? "change" : "input";
  el.addEventListener(evt, () =>
    sig.set(num ? (el.value === "" ? null : Number(el.value)) : el.value)
  );
}

export function setAttr(el, key, val) {
  // SVG props are mostly read-only animated objects (viewBox, width, d, className…)
  // so assigning to them throws — always go through attributes for SVG.
  const isSvg = typeof SVGElement !== "undefined" && el instanceof SVGElement;
  if (key === "class") {
    if (!isSvg && typeof el.className === "string") el.className = val ?? "";
    else el.setAttribute("class", val ?? "");
  } else if (key === "style" && typeof val === "object") {
    Object.assign(el.style, val);
  } else if (!isSvg && key in el) {
    el[key] = val;
  } else if (val == null || val === false) {
    el.removeAttribute(key);
  } else {
    el.setAttribute(key, val === true ? "" : val);
  }
}

function append(parent, child) {
  if (child == null || child === false) return;
  if (typeof child === "function") {
    // reactive: null | text | Node | array (mixed Nodes/text) — all supported.
    // Renders before the marker (end); replaces old nodes on every run.
    const end = document.createComment("");
    parent.appendChild(end);
    let rendered = [];
    effect(() => {
      if (!end.parentNode) return;
      const v = child();
      for (const n of rendered) n.remove();
      rendered = [];
      const collect = (x) => {
        if (x == null || x === false || x === true) return;
        if (x instanceof Node) rendered.push(x);
        else if (Array.isArray(x)) x.forEach(collect);
        else rendered.push(document.createTextNode(String(x)));
      };
      collect(v);
      for (const n of rendered) end.parentNode.insertBefore(n, end);
    });
  } else if (child instanceof Node) {
    parent.appendChild(child);
  } else if (Array.isArray(child)) {
    for (const c of child) append(parent, c);
  } else {
    parent.appendChild(document.createTextNode(child));
  }
}

// Build a branch in its own disposable root, isolated from the caller's
// tracking. Returns { node, dispose } so list/conditional helpers can tear
// down a branch's effects when its node is removed.
function renderBranch(builder) {
  let dispose = () => {};
  const node = createRoot((d) => {
    dispose = d;
    return untrack(builder);
  });
  if (DEV && node && node.nodeType === 11) {
    warn("when/each/forEach branch should return ONE root node, not a fragment — swapping/removal may misbehave.");
  }
  return { node, dispose };
}

// --- Control flow ----------------------------------------------------------

// Keyed list: reuses nodes, disposes removed items' effects.
export function each(listSignal, render) {
  const marker = document.createComment("each");
  const frag = document.createDocumentFragment();
  frag.appendChild(marker);
  let rendered = new Map(); // key -> { node, dispose }
  onCleanup(() => { for (const e of rendered.values()) e.dispose(); });

  effect(() => {
    const items = listSignal();
    const next = new Map();
    let prev = marker;
    items.forEach((item, i) => {
      if (DEV && item && typeof item === "object" && item.id === undefined) {
        warn("each/forEach: items have no `id` — falling back to array index as key (reorders may reuse the wrong node).");
      }
      const key = item?.id ?? i;
      const entry = rendered.get(key) || renderBranch(() => render(item, i));
      next.set(key, entry);
      if (prev.nextSibling !== entry.node) prev.parentNode.insertBefore(entry.node, prev.nextSibling);
      prev = entry.node;
    });
    for (const [key, entry] of rendered) if (!next.has(key)) { entry.node.remove(); entry.dispose(); }
    rendered = next;
  });

  return frag;
}

// when(cond, then, [else]) — @if. Branches build once (cached) and swap when the
// condition flips; branch internals stay reactive. Disposed on teardown.
export function when(cond, then, otherwise) {
  const anchor = document.createComment("when");
  const frag = document.createDocumentFragment();
  frag.appendChild(anchor);
  let current = anchor;
  const cache = new Map(); // boolean -> { node, dispose }
  onCleanup(() => { for (const e of cache.values()) e.dispose(); });

  effect(() => {
    const key = !!cond();
    let entry = cache.get(key);
    if (!entry) {
      entry = renderBranch(() => (key ? then() : otherwise ? otherwise() : document.createComment("")));
      cache.set(key, entry);
    }
    if (current !== entry.node) {
      current.replaceWith(entry.node);
      current = entry.node;
    }
  });

  return frag;
}

// forEach(list, render, [empty]) — @for with optional @empty. Keyed by item.id.
export function forEach(listSignal, render, empty) {
  const marker = document.createComment("for");
  const frag = document.createDocumentFragment();
  frag.appendChild(marker);
  let rendered = new Map();
  let emptyEntry = null;
  onCleanup(() => {
    for (const e of rendered.values()) e.dispose();
    if (emptyEntry) emptyEntry.dispose();
  });

  effect(() => {
    const items = listSignal();
    if (emptyEntry) { emptyEntry.node.remove(); emptyEntry.dispose(); emptyEntry = null; }
    const next = new Map();
    let prev = marker;
    items.forEach((item, i) => {
      const key = item?.id ?? i;
      // Same key, but a different object under it — a list refetched from a
      // server hands back equal ids on new objects, and a row built from the
      // old one goes on showing what that one said. The key decides whether
      // this is the same ROW; the item decides whether it has to be drawn
      // again.
      let entry = rendered.get(key);
      if (entry && !Object.is(entry.item, item)) {
        entry.node.remove();
        entry.dispose();
        entry = null;
      }
      if (!entry) {
        entry = renderBranch(() => render(item, i));
        entry.item = item;
      }
      next.set(key, entry);
      if (prev.nextSibling !== entry.node) prev.parentNode.insertBefore(entry.node, prev.nextSibling);
      prev = entry.node;
    });
    for (const [key, entry] of rendered) if (!next.has(key)) { entry.node.remove(); entry.dispose(); }
    rendered = next;
    if (items.length === 0 && empty) {
      emptyEntry = renderBranch(() => empty());
      marker.parentNode.insertBefore(emptyEntry.node, marker.nextSibling);
    }
  });

  return frag;
}

// --- Components ------------------------------------------------------------
// A component is just a function `(props) => Node`. It runs ONCE — there is no
// re-render. Reactivity flows through props: pass a signal/getter for a reactive
// prop and read it by calling (`props.x()`), or drop it straight into a template
// hole (`${props.x}`) where html auto-tracks it. Static props are plain values.
//
//   const Hello = (props) => html`<p>Hi ${props.name}</p>`;   // reactive if name is a signal
//   html`${Hello({ name })}`                                   // compose: just call it
//
// mergeProps merges left-to-right (later wins), but `undefined` never overrides
// an earlier value — so omitted props fall back to defaults. Signals/getters are
// copied by reference, so reactivity is preserved.
//
//   const Btn = (props) => {
//     props = mergeProps({ kind: "primary" }, props);
//     return html`<button class=${() => props.kind}>${props.label}</button>`;
//   };
export function mergeProps(...sources) {
  const out = {};
  for (const src of sources) {
    if (!src) continue;
    for (const key in src) if (src[key] !== undefined) out[key] = src[key];
  }
  return out;
}

// Slots / content projection. A slot is just a prop holding a child (Node,
// array, reactive getter, or value) — drop it into a template hole and html
// renders it: `html`<div class="card">${props.children}</div>``. Named slots are
// simply more props (`header`, `footer`, ...). Projected content is built by the
// PARENT, so its effects/lifecycle belong to the parent's scope (like React
// children / Angular ng-content), not the child component.
//
// `slot(content, fallback?)` returns `content` when present, else `fallback`
// (a value or a builder fn) — for default slot content when none is passed:
//
//   const Card = (props) => html`
//     <section class="card">
//       <header>${slot(props.header, () => html`<b>Untitled</b>`)}</header>
//       ${slot(props.children, "nothing here yet")}
//     </section>`;
export function slot(content, fallback) {
  const empty =
    content == null ||
    content === false ||
    (Array.isArray(content) && content.length === 0);
  if (!empty) return content;
  return typeof fallback === "function" ? fallback() : fallback ?? null;
}

// --- Context / dependency injection ---------------------------------------
// Pass data down the ownership tree without threading it through every props.
// `provide` stamps a value onto a scope; `useContext` walks UP the owner chain
// to the nearest provider (else the context's default). Lookup is O(depth) with
// no subscriptions — for a *reactive* context, provide a signal and let
// consumers read it (so only the exact nodes that read it update).
//
//   const Theme = createContext("light");
//   provide(Theme, dark, () => html`${Toolbar()}`);   // Toolbar + descendants
//   function Toolbar() { const t = useContext(Theme); /* dark */ }
export function createContext(defaultValue) {
  return { id: Symbol("zap.ctx"), defaultValue };
}

// Provide `value` for `context` to everything created inside fn(). Returns fn().
export function provide(context, value, fn) {
  const scope = makeScope(currentOwner);
  scope.contexts = { [context.id]: value };
  if (currentOwner) currentOwner.owned.push(scope);
  const prev = currentOwner;
  currentOwner = scope;
  try {
    return fn();
  } finally {
    currentOwner = prev;
  }
}

// Read the nearest provided value for `context`, or its default if none.
export function useContext(context) {
  for (let o = currentOwner; o; o = o.owner) {
    if (o.contexts && context.id in o.contexts) return o.contexts[context.id];
  }
  return context.defaultValue;
}

// Mount a component into the DOM, optionally passing props. Returns a dispose()
// that tears the whole app down (effects, listeners, child scopes).
export function mount(component, root, props) {
  let dispose = () => {};
  const node = createRoot((d) => {
    dispose = d;
    return component(props);
  });
  root.appendChild(node);
  return dispose;
}

// Hydrate a server-rendered `root`: the SSR markup is already painted (fast first
// paint / SEO), so we build the live reactive tree and swap it in. The markup is
// identical, so the swap is visually seamless — the page becomes interactive
// without a blank flash. Same return as mount(). (Client takeover, not in-place
// node reuse — zap renders eagerly, so true in-place hydration needs a compiler.)
export function hydrate(component, root, props) {
  let dispose = () => {};
  const node = createRoot((d) => {
    dispose = d;
    return component(props);
  });
  root.replaceChildren(node);
  return dispose;
}
