// The boot: the last part of page.js, and the part that runs.
//
// This file is not loaded on its own. `pack.rill` concatenates world.js,
// net.js, host.js and this one into a single module — page.js — stripping the
// import lines that joined them, because inside one module they are already
// in each other's scope. So `run` and `load` below are simply here. The stub
// in index.html streams valley.rpak, reads the directory by hand, slices out
// that one module and imports it; everything after that moment happens here.
//
// The four are stored in the bundle rather than compressed, for the same
// reason world.wasm is: the LZ belongs to the engine, and the engine is what
// the bundle is being opened to reach.
//
// `takeStored` is the stub's directory reader, handed over so there is
// exactly one implementation of the format on the page.

export async function boot(pakBytes, takeStored) {
  // The engine itself, out of the same file — as bytes, not as a URL. The
  // generated loader takes either, so the module that unpacks the bundle is
  // handed memory the bundle is already holding and the network is never
  // asked for anything again.
  const wasmBytes = takeStored("world.wasm");

  // Whose valley. The one this page is served from, unless told otherwise:
  // `?join=wss://box/valley` points at somebody else's, and `?solo` asks for
  // the valley this tab has to itself.
  //
  // Shared is the default because it is what the address is for. It used to
  // need `?join`, which meant the plain link — the one people paste to each
  // other — dropped everybody into a private world where nobody else ever
  // appeared, and there was nothing on screen to say why.
  //
  // A static copy with no server behind it is not a problem: the socket fails
  // to open, and `run` says so and plays alone. That path was already there
  // for a valley that is full or a word that was wrong.
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
  const server = q.has("solo") ? null : (q.get("join") || here);

  // Who to put over this player's head. Only asked when there is somebody to
  // see it; the answer is kept, because nobody wants to type their own name
  // every time they reload.
  let pass = "";

  // ---- which valley ---------------------------------------------------------
  //
  // Several of them run behind one address now, and they write themselves down
  // in a store they share — see `registrar` in server.rill. Any of them will
  // read the list back, so the page asks the one it was going to open anyway.
  //
  // A list that cannot be fetched is not an error: it means one valley, the
  // one this page is served from, which is exactly what a single server is.
  // So the picker is shown when there is something to pick between and stays
  // out of the way when there is not.
  let chosen = null;
  let asking = false;
  const askValley = async () => {
    if (!server || asking) return;
    asking = true;
    try { await askValleyNow(); } finally { asking = false; }
  };
  const askValleyNow = async () => {
    let list = [];
    try {
      // From the root, not from beside the page: the valleys are at `/valley1`
      // and the list of them belongs with them, not in whatever directory this
      // page happens to be served out of. Relative, it asked for
      // `/rillwebgpu/valleys` and got a 404 — and a 404 here reads exactly
      // like "there is only one valley".
      const r = await fetch(new URL("/valleys", location.origin), { cache: "no-store" });
      list = await r.json();
    } catch (e) { return; }
    if (!Array.isArray(list) || list.length < 2) return;
    // Full first is the wrong order: what somebody wants is the one they can
    // get into, and after that the busiest, because an empty valley is a
    // lonely one.
    list.sort((a, b) => (a.here >= a.max) - (b.here >= b.max) || b.here - a.here);
    const box = document.getElementById("valleys");
    box.style.display = "flex";
    // Redrawn whenever the account panel changes what exists, so start clean:
    // appending twice would show every valley as its own ghost.
    box.innerHTML = "";
    const last = localStorage.getItem("valley.pick");
    const rows = list.map((v) => {
      const full = v.here >= v.max;
      const row = document.createElement("b");
      row.className = full ? "full" : "";
      row.innerHTML = `<span>${v.name}</span><i>${v.here}/${v.max}${full ? " — full" : ""}</i>`;
      if (!full) row.addEventListener("click", () => pick(v, row));
      box.appendChild(row);
      return { v, row, full };
    });
    const pick = (v, row) => {
      chosen = v;
      localStorage.setItem("valley.pick", v.name);
      for (const r of rows) r.row.classList.toggle("on", r.row === row);
    };
    // The one they were in last time, or the emptiest that is open.
    const want = rows.find((r) => !r.full && r.v.name === last)
              || rows.find((r) => !r.full);
    if (want) pick(want.v, want.row);
  };

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
  // The account panel, beside the doors. It can change what doors exist —
  // making a valley adds one — so it is handed the way to redraw them; the
  // new door registers itself only once its container is up, hence the
  // patient second and third look.
  //
  // No import: pack.rill concatenates account.js into this same module.
  if (server) {
    mountAccount(() => {
      let looks = 0;
      const again = async () => {
        await askValley();
        if (++looks < 3) setTimeout(again, 4000);
      };
      again();
    });
  }
  await askValley();
  // The counts are alive: somebody joining `guney` while this card is up
  // should turn its 0/16 into 1/16 without a reload — the list was fetched
  // once and then stood still, which read as nobody ever being anywhere.
  // Every three seconds is one small JSON per open entry card, and it stops
  // the moment a name is given, because after that the player is looking at
  // the valley itself.
  const doorsAlive = setInterval(askValley, 3000);
  const name = await askName();
  clearInterval(doorsAlive);
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
  // A picked valley has its own address; without one this is the valley the
  // page is served from, which is what `server` already is.
  const where = chosen && chosen.path
    ? `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}${chosen.path}`
    : server;
  const door = where && query
    ? where + (where.includes("?") ? "&" : "?") + query
    : where;
  const shroud = document.getElementById("boot");
  const bar = document.getElementById("bar");
  const what = document.getElementById("what");
  const pct = document.getElementById("pct");

  // The HUD — the stat corner included — is the engine's now: host.js
  // pushes the lines straight into the wasm display list, so there is no
  // element here to fill and nothing to do twice a second.
  return run(document.getElementById("c"), wasmBytes, () => {}, (done, total, label) => {
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
