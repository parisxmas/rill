// The boot, out of the bundle. index.html is a stub now: it streams
// valley.rpak, reads the directory by hand, slices this file out and imports
// it as a blob module — everything after that moment, this file does. It is
// stored in the bundle rather than compressed for the same reason host.js and
// world.js are: the LZ belongs to the engine, the engine is in world.wasm,
// and world.wasm is itself an entry in the directory this file slices it from.
//
// `takeStored` is the stub's directory reader, handed over so there is
// exactly one implementation of the format on the page.

export async function boot(pakBytes, takeStored) {
  // Modules out of bytes: blob URLs, with host.js's own import lines
  // rewritten to point at them — a blob module cannot resolve "./net.js".
  const asModule = (bytes) =>
    URL.createObjectURL(new Blob([bytes], { type: "text/javascript" }));
  const netURL = asModule(takeStored("net.js"));
  const worldURL = asModule(takeStored("world.js"));
  const hostSrc = new TextDecoder().decode(takeStored("host.js"))
    .replace(/["']\.\/world\.js[^"']*["']/, JSON.stringify(worldURL))
    .replace(/["']\.\/net\.js[^"']*["']/, JSON.stringify(netURL));
  const { run } = await import(asModule(hostSrc));
  // The engine itself, out of the same file. `load` in world.js does a plain
  // fetch and a compile, and a blob URL is a thing fetch can be pointed at —
  // so the module that unpacks the bundle is booted from bytes the bundle
  // carried, and nothing here asks the network for it.
  const wasmURL = URL.createObjectURL(
    new Blob([takeStored("world.wasm")], { type: "application/wasm" }));

  // Whose valley. `?join` takes the one this page is served from,
  // `?join=wss://box/valley` somebody else's, and no `join` at all is the
  // valley this tab has to itself — which is what a static page gets,
  // because there is no server behind one.
  //
  // The default follows the page it is on. A page served over TLS may not
  // open a plain socket, so it asks for `wss://` on the same host and lets
  // whatever is in front of it hand the connection on — one path, no port,
  // and the certificate is the one the page already came over. Served over
  // plain http, which is how this runs while it is being worked on, it goes
  // straight at the port the server listens on.
  const q = new URLSearchParams(location.search);
  const here = location.protocol === "https:"
    ? `wss://${location.host}/valley`
    : `ws://${location.hostname || "localhost"}:8091/`;
  const server = q.has("join") ? (q.get("join") || here) : null;

  // Who to put over this player's head. Only asked when there is somebody to
  // see it; the answer is kept, because nobody wants to type their own name
  // every time they reload.
  let pass = "";
  const askName = async () => {
    if (!server) return "";
    const form = document.getElementById("who");
    const field = document.getElementById("name");
    const word = document.getElementById("word");
    form.style.display = "flex";
    document.getElementById("whonote").style.display = "block";
    field.value = localStorage.getItem("valley.name") ?? "";
    field.focus();
    field.select();
    const given = await new Promise((res) => {
      form.addEventListener("submit", (e) => { e.preventDefault(); res(field.value.trim()); }, { once: true });
    });
    // The word travels in the query of the socket's own address, where the
    // server can refuse it before it answers the handshake — no protocol, no
    // round trip, and nothing allocated for somebody who does not have it.
    // Not kept: a name is worth remembering between visits and a shared word
    // is not.
    pass = word.value.trim();
    form.style.display = "none";
    document.getElementById("whonote").style.display = "none";
    // The same rule the server applies, applied here so that what somebody
    // types is what they see over their own head. The server still checks
    // it — this field is not where the rule lives, it is only where it is
    // polite about it — and the server may still hand back a number on the
    // end if the name was already somebody else's.
    const name = given.replace(/[^A-Za-z0-9]/g, "").slice(0, 8);
    if (name) localStorage.setItem("valley.name", name);
    return name;
  };
  const name = await askName();
  // Who this browser is, as far as the valley is concerned. The server mints
  // it on the first visit and hands it back in `hello`; from then on it
  // travels in the query beside the word, and it is what makes a name stay
  // yours between visits — see `who somebody is` in server.rill.
  //
  // Not a password: it is never typed, it cannot be recovered, and anybody
  // who copies it out of this browser is this player. It is kept here rather
  // than in a cookie because nothing else needs to see it, least of all
  // whatever is proxying the socket.
  const who = localStorage.getItem("valley.who") ?? "";
  // `?join` may already carry a query of its own.
  const query = [
    pass ? "p=" + encodeURIComponent(pass) : "",
    who ? "who=" + encodeURIComponent(who) : "",
  ].filter(Boolean).join("&");
  const door = server && query
    ? server + (server.includes("?") ? "&" : "?") + query
    : server;
  const shroud = document.getElementById("boot");
  const bar = document.getElementById("bar");
  const what = document.getElementById("what");
  const pct = document.getElementById("pct");

  // The HUD — the stat corner included — is the engine's now: host.js
  // pushes the lines straight into the wasm display list, so there is no
  // element here to fill and nothing to do twice a second.
  return run(document.getElementById("c"), wasmURL, () => {}, (done, total, label) => {
    bar.style.width = `${(done / total * 100).toFixed(0)}%`;
    pct.textContent = `${Math.round(done / total * 100)}%`;
    what.textContent = label;
  }, door, name, pakBytes).then(() => {
    // `run` resolves on the first submitted frame, so the fade is over
    // something rather than over nothing.
    shroud.classList.add("gone");
    // Removed on the transition, or on a timer if that never comes.
    //
    // A background tab does not run transitions, and it does not run
    // requestAnimationFrame either — so switching away during the load and
    // coming back left the curtain frozen half-faded over a running valley,
    // for ever, because the one event that was going to remove it had already
    // been missed. The timer is the same 700ms the CSS is, with room.
    const drop = () => shroud.remove();
    shroud.addEventListener("transitionend", drop, { once: true });
    setTimeout(drop, 1200);
  });
}
