// The socket, and nothing else.
//
// The protocol used to live here twice — parsed on this side, written on the
// server's — and now it lives in the engine: wire.rill parses every frame,
// applies it to the world, keeps the snapshots, does the interpolating and
// fills the HUD. What is left of this file is the part wasm cannot do: open a
// WebSocket, push the bytes that arrive into the wire's IN buffer, and send
// the bytes its builders leave in OUT.
//
// The header indices in `WH` are wire.rill's h_* constants, mirrored here
// once — the page reads its handful of facts (ping, version, who am I)
// straight out of the block.

export const WH = {
  id: 0, hello: 1, pines: 2, crystals: 3, grass: 4, clock0: 5,
  ping: 6, ver: 7, refused: 13, tokenLen: 14, interpN: 19, sharkX: 25,
};

const enc = new TextEncoder();
const dec = new TextDecoder();

export function join(url, name, E) {
  const wire = E.wire_new();
  const base = Number(wire);
  const inAt = base + Number(E.wire_in(wire));
  const inCap = Number(E.wire_in_cap());
  const outAt = base + Number(E.wire_out(wire));
  const tokenAt = base + Number(E.wire_token_at());
  const refuseAt = base + Number(E.wire_refuse_at());

  const sock = new WebSocket(url);
  sock.binaryType = "arraybuffer";

  let ready, failed;
  const arrival = new Promise((res, rej) => { ready = res; failed = rej; });
  let welcomed = false;

  // Fresh views every time: wasm memory can grow, and a grown memory detaches
  // every view that was made of the old one.
  const hf = (i) => new Float32Array(E.memory.buffer, base + i * 4, 1)[0];
  const sendOut = (len) => {
    if (sock.readyState === 1 && len > 0) {
      sock.send(new Uint8Array(E.memory.buffer, outAt, Number(len)));
    }
  };
  const sendText = (op, text) => {
    const b = enc.encode(text).subarray(0, 200);
    new Uint8Array(E.memory.buffer, outAt + 1, b.length).set(b);
    sendOut(E.wire_out_text(wire, BigInt(op), BigInt(b.length)));
  };

  let look = [0, 0];
  const net = {
    wire,
    arrival,
    lost: false,
    get id() { return hf(WH.id); },
    get ping() { const p = hf(WH.ping); return p < 0 ? null : p; },
    get serverVersion() { const v = hf(WH.ver); return v < 0 ? null : v; },
    get world() {
      return { pines: hf(WH.pines), crystals: hf(WH.crystals),
               grass: hf(WH.grass), clock: hf(WH.clock0) };
    },
    input(keys, yaw, pitch) {
      look = [yaw, pitch];
      sendOut(E.wire_out_input(wire, BigInt(keys), yaw, pitch));
    },
    fired(dx, dy, dz) { sendOut(E.wire_out_fire(wire, dx, dy, dz)); },
    // The tools: one byte each. The server has this player's stance and look
    // already — the ask carries nothing because nothing else is needed.
    mined() { sendOut(E.wire_out_mine(wire)); },
    builds() { sendOut(E.wire_out_build(wire)); },
    // What the server last said the pocket holds; -1 until it ever has.
    get pocket() { return Number(E.wire_pocket(wire)); },
    reloaded() { sendOut(E.wire_out_reload(wire)); },
    say(text) { sendText(99, text); },
    close() { sock.close(); },
  };

  sock.addEventListener("open", () => { if (name) sendText(110, name); });

  sock.addEventListener("message", (ev) => {
    // This wire is binary; a text frame on it is not ours to guess about.
    if (typeof ev.data === "string") return;
    const bytes = new Uint8Array(ev.data);
    if (bytes.length > inCap) return;
    new Uint8Array(E.memory.buffer, inAt, bytes.length).set(bytes);
    E.wire_frame(wire, BigInt(bytes.length), performance.now());
    if (!welcomed && hf(WH.hello) > 0.5) {
      welcomed = true;
      // Who this browser is. Stored only when it looks like one of the
      // server's — sixteen hex characters — so a frame from something else
      // cannot put anything it likes in this browser's storage.
      const tl = hf(WH.tokenLen);
      const token = dec.decode(new Uint8Array(E.memory.buffer, tokenAt, tl));
      if (/^[0-9a-f]{16}$/.test(token)) {
        try { localStorage.setItem("valley.who", token); } catch {}
      }
      ready(net.world);
    }
    const rl = hf(WH.refused);
    if (rl > 0) {
      const said = dec.decode(new Uint8Array(E.memory.buffer, refuseAt, rl));
      const why = said === "the valley is full"
        ? "the valley is full — try again in a minute" : said;
      const e = new Error(why);
      e.refused = why;
      failed(e);
    }
  });

  // A heartbeat, on a timer rather than on the frame loop: a hidden tab stops
  // running frames, and a tab that says nothing gets dropped. The last look
  // with no keys held says "still here, standing still".
  const beat = setInterval(() => net.input(0, look[0], look[1]), 2000);
  // The ping, once a second: the token is this page's own clock, handed back.
  const pulse = setInterval(() => sendOut(E.wire_out_ping(wire, performance.now())), 1000);
  const stop = () => { clearInterval(beat); clearInterval(pulse); };

  sock.addEventListener("error", () => { stop(); net.lost = true; failed(new Error(`cannot reach ${url}`)); });
  sock.addEventListener("close", () => { stop(); net.lost = true; failed(new Error(`${url} closed`)); });

  return net;
}
