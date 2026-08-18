// The connection to a valley somebody else is running.
//
// This file knows nothing about drawing and nothing about wasm. It opens the
// socket, turns the server's lines into calls, and turns key presses back into
// lines — so the renderer can be read without the protocol in it, and the
// protocol can be read without the renderer.
//
// The protocol is text, one message per frame, numbers separated by spaces.
// That is not the cheapest thing it could be and it is deliberate: the whole
// exchange is legible in a terminal with `websocat`, which is worth more
// during the week you are getting it right than the bytes are. A valley with
// eight people in it is under three kilobytes a second either way.
//
//   in     hello <id> <pines> <crystals> <grass> <clock>
//          gone <i>...                     crystals already shot
//          s <clock> <shark x y z heading mode> | <id x y z yaw pitch health>...
//          r <x y z vx vy vz owner>        a round left somebody's rifle
//          t <x y z vx vy vz seed life>    a crystal threw
//          k <x y z seed>                  one landed
//          b <i>                           crystal i came apart
//
//   out    i <keys> <yaw> <pitch>          what this player is doing
//          f <dx> <dy> <dz>                and where they just fired

export function join(url) {
  const sock = new WebSocket(url);
  sock.binaryType = "arraybuffer";

  // Filled in as the server says them. `hello` resolves the promise the caller
  // is waiting on before it builds the valley: how many pines and crystals is
  // not something a client may decide for itself when other people are already
  // standing among them.
  let ready, failed;
  const arrival = new Promise((res, rej) => { ready = res; failed = rej; });

  const net = {
    id: -1,
    arrival,
    // The last two snapshots, so that what is drawn can be a point between
    // them rather than the older of the two.
    prev: null,
    last: null,
    // Everything that has happened since the renderer last looked.
    events: [],
    // Set once the socket is gone, so the frame loop can say so and carry on
    // rather than throwing sixty times a second.
    lost: false,
    send(line) { if (sock.readyState === 1) sock.send(line); },
    input(keys, yaw, pitch) { net.send(`i ${keys} ${yaw.toFixed(4)} ${pitch.toFixed(4)}`); },
    fired(dx, dy, dz) { net.send(`f ${dx.toFixed(4)} ${dy.toFixed(4)} ${dz.toFixed(4)}`); },
    take() { const e = net.events; net.events = []; return e; },
    close() { sock.close(); },
  };

  sock.addEventListener("message", (ev) => {
    const line = typeof ev.data === "string" ? ev.data : new TextDecoder().decode(ev.data);
    const sp = line.indexOf(" ");
    const kind = sp < 0 ? line : line.slice(0, sp);
    if (kind === "s") { snapshot(net, line); return; }
    if (kind === "hello") { hello(net, line, ready); return; }
    if (kind === "gone") { net.events.push({ kind: "gone", which: nums(line, 1) }); return; }
    if (kind === "no") { failed(new Error(line)); return; }
    // r, t, k, b — one line each, applied in the order they arrived.
    net.events.push({ kind, n: nums(line, 1) });
  });

  sock.addEventListener("error", () => { net.lost = true; failed(new Error(`cannot reach ${url}`)); });
  sock.addEventListener("close", () => { net.lost = true; failed(new Error(`${url} closed`)); });

  return net;
}

// The first `from` words are the kind; everything after it is a number.
function nums(line, from) {
  const parts = line.split(" ");
  const out = [];
  for (let i = from; i < parts.length; i++) out.push(+parts[i]);
  return out;
}

function hello(net, line, ready) {
  const p = line.split(" ");
  net.id = +p[1];
  net.world = { pines: +p[2], crystals: +p[3], grass: +p[4], clock: +p[5] };
  ready(net.world);
}

// `s <clock> <shark×5> | <id x y z yaw pitch health> | ...`
//
// Kept as two snapshots and a time, because a twentieth of a second of stale
// position drawn as if it were current is a walk that arrives in steps. What
// the renderer asks for is the point between them.
function snapshot(net, line) {
  const [head, ...people] = line.split(" | ");
  const h = nums(head, 1);
  const snap = {
    at: performance.now(),
    clock: h[0],
    shark: { x: h[1], y: h[2], z: h[3], heading: h[4], mode: h[5] },
    players: people.map((p) => {
      const n = p.split(" ").map(Number);
      return { id: n[0], x: n[1], y: n[2], z: n[3], yaw: n[4], pitch: n[5], health: n[6] };
    }),
  };
  net.prev = net.last ?? snap;
  net.last = snap;
}

// Everybody except this player, at where they were `delay` milliseconds ago —
// which is how a position that arrives twenty times a second is drawn sixty.
//
// Interpolating between the last two rather than extrapolating past the newest
// is a choice: extrapolation is smoother while somebody walks in a straight
// line and overshoots every time they stop, and a figure that slides a step
// past a wall and snaps back is worse than one that is a snapshot behind.
export function others(net, now) {
  if (!net.last || !net.prev) return [];
  const span = net.last.at - net.prev.at;
  const t = span > 1 ? Math.min(1, Math.max(0, (now - net.prev.at) / span)) : 1;
  const out = [];
  for (const p of net.last.players) {
    if (p.id === net.id) continue;
    const q = net.prev.players.find((o) => o.id === p.id) ?? p;
    out.push({
      id: p.id,
      x: q.x + (p.x - q.x) * t,
      y: q.y + (p.y - q.y) * t,
      z: q.z + (p.z - q.z) * t,
      // The short way round: an aim that crosses due north must not spin the
      // whole way back through south.
      yaw: q.yaw + wrap(p.yaw - q.yaw) * t,
      pitch: q.pitch + (p.pitch - q.pitch) * t,
      health: p.health,
    });
  }
  return out;
}

// The shark, at the same point between the last two snapshots. It is the
// server's animal — a client running its own would have it hunting somebody
// who is not there — so between ticks it is interpolated rather than swum.
export function sharkNow(net, now) {
  if (!net.last || !net.prev) return null;
  const span = net.last.at - net.prev.at;
  const t = span > 1 ? Math.min(1, Math.max(0, (now - net.prev.at) / span)) : 1;
  const a = net.prev.shark, b = net.last.shark;
  return {
    x: a.x + (b.x - a.x) * t,
    y: a.y + (b.y - a.y) * t,
    z: a.z + (b.z - a.z) * t,
    heading: a.heading + wrap(b.heading - a.heading) * t,
    mode: b.mode,
  };
}

export function me(net) {
  if (!net.last) return null;
  return net.last.players.find((p) => p.id === net.id) ?? null;
}

function wrap(a) {
  while (a > Math.PI) a -= Math.PI * 2;
  while (a < -Math.PI) a += Math.PI * 2;
  return a;
}
