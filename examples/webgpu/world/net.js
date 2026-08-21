// The connection to a valley somebody else is running.
//
// This file knows nothing about drawing and nothing about wasm. It opens the
// socket, turns the server's frames into calls, and turns key presses back
// into frames — so the renderer can be read without the protocol in it, and
// the protocol can be read without the renderer.
//
// The protocol is binary: one message per frame, little-endian, first byte
// says which message. The byte is the letter the old text protocol used, so a
// hexdump of the wire still reads — a snapshot still starts with an `s`. The
// layouts live in one place, `server.rill`'s "bytes on the wire" section, and
// this file mirrors them exactly:
//
//   in     h  u16 id, u32 pines, u32 crystals, u32 grass, f32 clock, token…
//          g  u16 crystal indices    crystals already shot
//          W  u16 stone indices      stones already broken
//          s  f32 clock, f32 shark x y z heading mode, u8 phase, f32 left,
//             u8 count, then 37 bytes a player: u16 id, f32 x y z yaw pitch
//             health, u8 pose, u16 hit, u16 shot, u8 seen, i8 ammo,
//             u16 kills, u16 deaths
//          r  f32 ×7                 a round left somebody's rifle
//          t  f32 ×8                 a crystal threw
//          k  f32 ×4                 one landed
//          b  u16                    crystal i came apart
//          w  u16                    stone i left a wall
//          d  u16                    a meteor was shot out of the air
//          y  u16                    pine i was felled by a warhead
//          P  u16 pine indices       pines already down, at the door
//          m  u8 box, u32 times     where a box of ammunition is, as a count
//          x  u16 who, u16 by whom, f32 how much, u8 down
//          u  u16 id, name…          what to call somebody
//          p  the ping token, straight back
//          n  the words of a refusal
//
//   out    n  name…                  what to call this one
//          i  u16 keys, f32 yaw, f32 pitch — bits 7-8 of keys are not keys,
//             they are which weapon is up: 0 rifle, 1 launcher, 2 AK
//          f  f32 dx dy dz           and where they just fired
//
// A name is sent once and comes back as a `u`, rather than riding in every
// snapshot: it changes about as often as a person arrives.

const OP = {
  hello: 104, no: 110, gone: 103, wall: 87, mag: 109, who: 117, pong: 112,
  snap: 115, round: 114, thrown: 116, crater: 107, hurt: 120,
  downed: 100, stone: 119, broke: 98, fell: 121, pineList: 80, zap: 122,
  input: 105, fire: 102, name: 110, ping: 112,
};

const utf8 = { enc: new TextEncoder(), dec: new TextDecoder() };

export function join(url, name) {
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
    // id -> what to call them, for everybody here including this player.
    names: new Map(),
    // The last two snapshots, so that what is drawn can be a point between
    // them rather than the older of the two.
    prev: null,
    last: null,
    // How long a snapshot takes to arrive, smoothed — the length of the walk
    // between two of them. See `snapshot`.
    span: 0,
    // Everything that has happened since the renderer last looked.
    events: [],
    // Set once the socket is gone, so the frame loop can say so and carry on
    // rather than throwing sixty times a second.
    lost: false,
    // Round trip to the hub and back, in milliseconds, smoothed. Null until
    // the first one comes home. See the ping below for what it is measuring.
    ping: null,
    send(buf) { if (sock.readyState === 1) sock.send(buf); },
    input(keys, yaw, pitch) {
      look = [yaw, pitch];
      const dv = new DataView(new ArrayBuffer(11));
      dv.setUint8(0, OP.input);
      dv.setUint16(1, keys, true);
      dv.setFloat32(3, yaw, true);
      dv.setFloat32(7, pitch, true);
      net.send(dv.buffer);
    },
    fired(dx, dy, dz) { net.send(floats(OP.fire, [dx, dy, dz])); },
    take() { const e = net.events; net.events = []; return e; },
    close() { sock.close(); },
  };

  sock.addEventListener("open", () => {
    if (!name) return;
    const bytes = utf8.enc.encode(name);
    const out = new Uint8Array(1 + bytes.length);
    out[0] = OP.name;
    out.set(bytes, 1);
    net.send(out.buffer);
  });

  sock.addEventListener("message", (ev) => {
    // This wire is binary; a text frame on it is not ours to guess about.
    if (typeof ev.data === "string") return;
    const dv = new DataView(ev.data);
    const op = dv.getUint8(0);
    if (op === OP.snap) { snapshot(net, dv); return; }
    if (op === OP.hello) { hello(net, dv, ready); return; }
    if (op === OP.gone) { net.events.push({ kind: "gone", which: u16s(dv, 1) }); return; }
    if (op === OP.wall) { net.events.push({ kind: "wall", n: u16s(dv, 1) }); return; }
    if (op === OP.who) { net.names.set(dv.getUint16(1, true), text(dv, 3)); return; }
    if (op === OP.pong) {
      const rtt = performance.now() - dv.getFloat64(1, true);
      // A quarter weight on each new one. Enough to show a link going bad
      // within a couple of seconds and not so much that one late packet
      // behind a scheduler makes the number jump about while it is read.
      net.ping = net.ping === null ? rtt : net.ping * 0.75 + rtt * 0.25;
      return;
    }
    // A refusal, and the only one the server sends: there is no room. What
    // arrives is protocol — the words behind an `n` — and the person who just
    // clicked join is not reading protocol, so it becomes a sentence here.
    if (op === OP.no) {
      const said = text(dv, 1);
      const why = said === "the valley is full"
        ? "the valley is full — try again in a minute"
        : said;
      const e = new Error(why);
      // Marked as the server's own answer rather than something guessed at
      // from a socket that went quiet: a page may show this to the person who
      // asked to join, where "cannot reach ws://..." is only worth a console.
      e.refused = why;
      failed(e);
      return;
    }
    // r, t, k — a handful of floats each, applied in the order they arrived.
    if (op === OP.round) { net.events.push({ kind: "r", n: f32s(dv, 1, 7) }); return; }
    if (op === OP.thrown) { net.events.push({ kind: "t", n: f32s(dv, 1, 8) }); return; }
    if (op === OP.crater) { net.events.push({ kind: "k", n: f32s(dv, 1, 4) }); return; }
    // b, w, d — one index each.
    if (op === OP.broke) { net.events.push({ kind: "b", n: [dv.getUint16(1, true)] }); return; }
    if (op === OP.stone) { net.events.push({ kind: "w", n: [dv.getUint16(1, true)] }); return; }
    if (op === OP.downed) { net.events.push({ kind: "d", n: [dv.getUint16(1, true)] }); return; }
    // A pine a warhead took, and at the door the whole list of them.
    if (op === OP.fell) { net.events.push({ kind: "y", n: [dv.getUint16(1, true)] }); return; }
    if (op === OP.pineList) { net.events.push({ kind: "pines", n: u16s(dv, 1) }); return; }
    // A crystal bit somebody: who, and which crystal.
    if (op === OP.zap) { net.events.push({ kind: "z", n: [dv.getUint16(1, true), dv.getUint16(3, true)] }); return; }
    if (op === OP.mag) { net.events.push({ kind: "m", n: [dv.getUint8(1), dv.getUint32(2, true)] }); return; }
    if (op === OP.hurt) {
      net.events.push({ kind: "x", n: [dv.getUint16(1, true), dv.getUint16(3, true), dv.getFloat32(5, true), dv.getUint8(9)] });
      return;
    }
  });

  // A heartbeat, on a timer rather than on the frame loop.
  //
  // Input is sent once a frame, and a browser stops running frames the moment
  // its tab is hidden — so a tab left in the background stopped saying
  // anything at all and the server, which drops whoever it has not heard from,
  // quite correctly threw it out of a valley it was still connected to.
  //
  // What goes out is the last look with no keys held: it says "still here" and
  // "standing still", which is the right thing for somebody who has looked
  // away — otherwise a tab hidden mid-stride would keep walking, because the
  // server walks from the last input it was given until it is given another.
  let look = [0, 0];
  const beat = setInterval(() => net.input(0, look[0], look[1]), 2000);

  // The ping. The token is this browser's own clock as eight bytes, sent out
  // and handed straight back, so nothing has to be remembered here and two
  // pings in the air at once cannot be confused for each other.
  //
  // Once a second: a round trip that is worth knowing about does not change
  // faster than that, and the reply is queued behind the ticks like everything
  // else, so asking ten times a second would measure the queue this ping
  // added.
  const pulse = setInterval(() => {
    const dv = new DataView(new ArrayBuffer(9));
    dv.setUint8(0, OP.ping);
    dv.setFloat64(1, performance.now(), true);
    net.send(dv.buffer);
  }, 1000);
  const stop = () => { clearInterval(beat); clearInterval(pulse); };

  sock.addEventListener("error", () => { stop(); net.lost = true; failed(new Error(`cannot reach ${url}`)); });
  sock.addEventListener("close", () => { stop(); net.lost = true; failed(new Error(`${url} closed`)); });

  return net;
}

// An opcode and a run of little-endian f32s, which is most of what goes out.
function floats(op, nums) {
  const dv = new DataView(new ArrayBuffer(1 + nums.length * 4));
  dv.setUint8(0, op);
  for (let i = 0; i < nums.length; i++) dv.setFloat32(1 + i * 4, nums[i], true);
  return dv.buffer;
}

// Every u16 from `at` to the end of the frame.
function u16s(dv, at) {
  const out = [];
  for (let o = at; o + 2 <= dv.byteLength; o += 2) out.push(dv.getUint16(o, true));
  return out;
}

function f32s(dv, at, count) {
  const out = [];
  for (let i = 0; i < count; i++) out.push(dv.getFloat32(at + i * 4, true));
  return out;
}

// Everything from `at` to the end of the frame, as words.
function text(dv, at) {
  return utf8.dec.decode(new Uint8Array(dv.buffer, dv.byteOffset + at, dv.byteLength - at));
}

function hello(net, dv, ready) {
  net.id = dv.getUint16(1, true);
  net.world = {
    pines: dv.getUint32(3, true),
    crystals: dv.getUint32(7, true),
    grass: dv.getUint32(11, true),
    clock: dv.getFloat32(15, true),
  };
  // Who this browser is. Sent back in the query next time, which is what makes
  // the name it took stay its own. Stored only when it looks like one of the
  // server's — sixteen hex characters — so a frame from something else cannot
  // put anything it likes in this browser's storage under that key.
  const token = text(dv, 19);
  if (/^[0-9a-f]{16}$/.test(token)) {
    try { localStorage.setItem("valley.who", token); } catch {}
  }
  ready(net.world);
}

// Kept as two snapshots and a time, because a twentieth of a second of stale
// position drawn as if it were current is a walk that arrives in steps. What
// the renderer asks for is the point between them.
function snapshot(net, dv) {
  let o = 1;
  const f = () => { const v = dv.getFloat32(o, true); o += 4; return v; };
  const clock = f();
  const shark = { x: f(), y: f(), z: f(), heading: f(), mode: f() };
  // Which part of the round, and how many seconds of it are left.
  const phase = dv.getUint8(o); o += 1;
  const left = f();
  const count = dv.getUint8(o); o += 1;
  const players = [];
  for (let i = 0; i < count; i++) {
    const id = dv.getUint16(o, true); o += 2;
    const x = f(), y = f(), z = f(), yaw = f(), pitch = f();
    // The pose is what the server says they are doing — standing, walking,
    // running, crouching, down — rather than something guessed here from how
    // fast the figure seems to be sliding.
    // `hit` is how many rounds this player has landed on other people and
    // `shot` how many they have taken. Both are the server's count: a client
    // can see somebody flinch but has no way of knowing whose round it was.
    // `seen` is whether this snapshot says where they are. The server works
    // out what each player can actually look at and blanks the rest — so a
    // zero here is not "they left", it is "there is no line to them from
    // where you are standing", and the position fields are zeros. They stay
    // in the roster; they are simply not drawn.
    const health = f();
    const pose = dv.getUint8(o); o += 1;
    const hit = dv.getUint16(o, true); o += 2;
    const shot = dv.getUint16(o, true); o += 2;
    const seen = dv.getUint8(o) !== 0; o += 1;
    // Rounds left, for your own block only — everybody else's byte says minus
    // one, which means the same thing null does here: this snapshot does not
    // know, so do not touch the count.
    const ammoRaw = dv.getInt8(o); o += 1;
    // What the board at the end of a round is made of.
    const kills = dv.getUint16(o, true); o += 2;
    const deaths = dv.getUint16(o, true); o += 2;
    players.push({ id, x, y, z, yaw, pitch, health, pose, hit, shot, seen,
                   ammo: ammoRaw >= 0 ? ammoRaw : null, kills, deaths });
  }
  const snap = { at: performance.now(), clock, shark, phase, left, players };
  // How long one takes to arrive, smoothed. It is the length of the walk
  // between two of them, and one late packet should not make everybody sprint
  // to catch up — nor one early one leave them standing.
  const gap = net.last ? snap.at - net.last.at : 0;
  if (gap > 1 && gap < 500) net.span = net.span ? net.span * 0.8 + gap * 0.2 : gap;
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
  // A snapshot behind, and that is what makes it smooth: the moment one
  // arrives we are still showing the one before it, and over the next
  // twentieth of a second everybody walks to where this one puts them —
  // arriving just as the next one lands.
  //
  // This was measured from the *older* of the two, so by the first frame after
  // a snapshot `now` was already past the newer one and the clamp pinned it at
  // one. Nothing was interpolated at all: everybody stood still for a twentieth
  // of a second and then jumped, twenty times a second, which on a fast screen
  // reads as a flicker between positions rather than as walking.
  const span = net.span || (net.last.at - net.prev.at);
  const t = span > 1 ? Math.min(1, Math.max(0, (now - net.last.at) / span)) : 1;
  const out = [];
  for (const p of net.last.players) {
    if (p.id === net.id || !p.seen) continue;
    // Interpolating from a snapshot that did not say where they were would
    // walk them out of the origin and across the valley. Somebody coming round
    // a corner is drawn where they are, once, and interpolated from then on.
    const before = net.prev.players.find((o) => o.id === p.id);
    const q = before && before.seen ? before : p;
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
      pose: p.pose,
      name: net.names.get(p.id) ?? "",
    });
  }
  return out;
}

// The shark, at the same point between the last two snapshots. It is the
// server's animal — a client running its own would have it hunting somebody
// who is not there — so between ticks it is interpolated rather than swum.
export function sharkNow(net, now) {
  if (!net.last || !net.prev) return null;
  const span = net.span || (net.last.at - net.prev.at);
  const t = span > 1 ? Math.min(1, Math.max(0, (now - net.last.at) / span)) : 1;
  const a = net.prev.shark, b = net.last.shark;
  return {
    x: a.x + (b.x - a.x) * t,
    y: a.y + (b.y - a.y) * t,
    z: a.z + (b.z - a.z) * t,
    heading: a.heading + wrap(b.heading - a.heading) * t,
    mode: b.mode,
  };
}

// Everybody in the valley, this player included, in the order the server
// listed them — which is the order they arrived and does not shuffle under a
// reader's eye the way sorting by health would. Straight off the last
// snapshot: a roster does not want the interpolation the figures do, because
// nothing about a name or a health bar moves between snapshots.
export function everyone(net) {
  if (!net.last) return [];
  return net.last.players.map((p) => ({
    id: p.id,
    name: net.names.get(p.id) ?? "",
    health: p.health,
    hit: p.hit,
    shot: p.shot,
    // What the board between rounds is made of.
    kills: p.kills | 0,
    deaths: p.deaths | 0,
    self: p.id === net.id,
  }));
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
