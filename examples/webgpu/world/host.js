// The renderer half of the valley. Rill (world.wasm) computes the world —
// mesh, placement, camera, physics, matrices — and this file spends the GPU
// on it: a shadow pass under a fixed golden-hour sun, a 4x MSAA forward pass
// with procedural sky, banded terrain, instanced pines, crystals and
// swaying grass, Gerstner-wave water with foam, additive crystal glow, and a
// compute-shader swarm of fireflies.
//
// Nobody's GPU is asked to take anyone's word for what it can do. The
// adapter's own description picks a starting tier, and then the only honest
// benchmark there is — this scene, on this machine — decides: frame times
// are measured, and the world is regenerated denser or sparser until the
// budget is spent. Rill rebuilds the whole valley in tens of milliseconds,
// which is what makes re-asking cheap.
//
// The plumbing — WASI shim, instantiation, init — is `world.js`, which
// `rill build` generates beside the module.

import { load } from "./world.js?v=390";   // keep in step with V below
import { join, others, me, sharkNow, everyone } from "./net.js?v=390";

// One number, appended to every asset URL.
//
// Not for the browser's cache — that would be reason enough — but for the CDN's.
// The images and audio went out under bare names, so replacing a texture at the
// origin changed nothing anyone could see: CloudFlare kept serving the old one
// and there is no purging it without a token. A version in the query string
// makes a new file a new URL, which is the only cache invalidation that needs
// nobody's permission. Bump it with `?v=` in index.html.
const V = "390";

// ---- what the machine can afford -------------------------------------------

const TIERS = [
  { name: "low",    grid:  72, pines: 300, crystals: 120, grass:  800, flies:  500, shadow: 1024, water:  48, dpr: 1.0, waves: 5, ripples: 2 },
  { name: "medium", grid:  96, pines: 420, crystals: 200, grass: 1500, flies: 1200, shadow: 2048, water:  72, dpr: 1.5, waves: 6, ripples: 3 },
  { name: "high",   grid: 128, pines: 650, crystals: 300, grass: 3000, flies: 2500, shadow: 2048, water:  96, dpr: 2.0, waves: 7, ripples: 3 },
  { name: "ultra",  grid: 160, pines: 900, crystals: 380, grass: 5000, flies: 4000, shadow: 4096, water: 128, dpr: 3.0, waves: 8, ripples: 4 },
];

// iPadOS and iOS, which need a ceiling the measurements are not allowed to
// lift. Safari on an iPad reports itself as a Mac and gives the adapter Apple's
// own name, so `guessTier` matched `/apple/` and handed a tablet the desktop
// tier — after which the tuner, seeing a perfectly good sixty frames, climbed
// it to `ultra`. That means a device pixel ratio of three and a 4096 shadow
// map. At three, a 12.9" iPad's canvas is 4096x3072, and MSAA 4x colour plus a
// 4x depth buffer over that is around four hundred megabytes of render target
// alone. Safari does not fail that allocation with an error you can read; it
// takes the GPU back, and every call after that is silently dropped. Black
// screen, no console, nothing in the page.
//
// The count of things — trees, crystals, grass — is not what costs a tablet
// anything, so the ceiling is on tier and the ratio separately: `high` keeps
// the geometry and the 2048 shadow map, and the ratio is held at 1.5, which on
// a retina panel is still more pixels than the screen has points and is a
// quarter of the memory that three was asking for.
//
// The Mac test is the standard one: iPadOS Safari says "MacIntel" and reports
// five touch points, where a real Mac reports none.
const IOS = /iPad|iPhone|iPod/.test(navigator.userAgent)
  || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
const MAX_TIER = IOS ? 2 : TIERS.length - 1;
const DPR_CAP  = IOS ? 1.5 : Infinity;

// A guess to start from, no more: the measurements take over immediately.
function guessTier(info) {
  const desc = `${info?.vendor ?? ""} ${info?.architecture ?? ""} ${info?.device ?? ""} ${info?.description ?? ""}`.toLowerCase();
  if (/swiftshader|llvmpipe|software/.test(desc)) return 0;
  // Start a tablet a step below what its GPU name suggests and let it earn the
  // rest: the climb costs one rebuild, the fall costs a lost device.
  if (IOS) return 1;
  if (/apple|nvidia|geforce|rtx|radeon rx/.test(desc)) return 2;
  if (/intel|mali|adreno|powervr/.test(desc)) return 1;
  return 1;
}

// ---- shaders ---------------------------------------------------------------
//
// `UNI` is the uniform block alone, for the shadow pass — which writes the
// shadow map and therefore must not be able to see it. Everything else
// builds on `COMMON`, which adds the map and the lighting helpers.

// ---- shaders ---------------------------------------------------------------
//
// WGSL, in `shaders/`, one file per pipeline. They are not standalone: every
// one is written against `uniforms.wgsl`, and all but the shadow pair against
// `common.wgsl` as well — the shadow pass writes the shadow map and so must
// not be able to see it. The composing happens here rather than in the files
// so that what each shader may reach is stated in one place.
//
// Thirteen small fetches rather than one large one, in parallel: they are
// 15 KB between them, and a file per shader is what makes them editable as
// shaders rather than as string literals.

const SHADERS = {
  uniforms: null, common: "uniforms",
  terrain: "common", instanced: "common", pine: "common", grass: "common",
  viewmodel: "common", tracer: "common", muzzle: "common", shard: "common",
  fireball: "common", blast: "common", meteor: "common", shark: "common",
  sky: "common", water: "common", glow: "common", "firefly-draw": "common",
  figure: "common", held: "common", prop: "common", crate: "common", zap: "common",
  mark: "common", spark: "common", ak: "common",
  // The HUD stands alone: it has a uniform block of its own, and giving it
  // `uniforms.wgsl` would declare group 0 binding 0 twice.
  hud: null,
  launcher: "common", rocket: "common", warhead: "common", stone: "common",
  "shadow-terrain": "uniforms", "shadow-instanced": "uniforms",
  "shadow-pine": "uniforms", "shadow-figure": "uniforms",
  "firefly-sim": null,
};

// The images, and the square they are decoded to. Listed here rather than at
// the point of use so that the loader can count them before it starts.
const TEXTURES = [
  ["./pine-twig.png", 256, 256],     // the needle atlas
  ["./pine-bark.jpg", 256, 256],
  ["./grass-ground.jpg", 256, 256],
  ["./plants.png", 512, 512],        // the undergrowth atlas, 2x2
  ["./rock-face.jpg", 256, 256],
  ["./shore-sand.jpg", 256, 256],
  ["./rifle-atlas.jpg", 1024, 512],  // the rifle's two materials, side by side
  ["./meteor.jpg", 256, 256],        // the stone inside a fireball
  // Jupiter, equirectangular: Cassini's imagery by way of Solar System Scope,
  // CC BY 4.0. A thousand across for a disc four hundred pixels wide — the
  // visible hemisphere is half the map, so five hundred texels cover four
  // hundred pixels and it is still oversampled. Two thousand was 219 KB for
  // detail that never reached the screen; this is 62.
  ["./jupiter.jpg", 1024, 512],
  // Saturn's globe. Half Jupiter's width on screen, so half the map does.
  ["./saturn.jpg", 1024, 512],
  // The four Galileans in one 2x2 atlas — Io and Europa on the top row,
  // Ganymede and Callisto beneath. USGS and Voyager/Galileo mosaics by way of
  // the Celestia project. Twenty pixels of moon does not need five hundred and
  // twelve, but the poles turn to mush below it and one atlas is one binding.
  ["./moons.jpg", 1024, 512],
  // The shark, seen only under water and mostly through it.
  ["./shark-skin.jpg", 512, 512],
  // The two boxes on the ground, side by side in one atlas: the ammunition
  // box's diffuse on the left, the rocket crate's on the right — both Poly
  // Haven photographs, both halved from the published 1k. One more thing to
  // draw did not get one more binding, because sixteen sampled textures a
  // stage is exactly what this scene already spends: the sheet trick the
  // rifle uses is free and the seventeenth binding is not there to give.
  ["./boxes-atlas.jpg", 1024, 512],
  // The launcher and the rocket it fires, off one sheet — they came out of one
  // file and they are the same object a moment apart.
  ["./rpg7.jpg", 512, 512],
];

async function loadShaders(step) {
  const src = Object.fromEntries(await Promise.all(
    Object.keys(SHADERS).map(async name => {
      const r = await fetch(`./shaders/${name}.wgsl?v=${V}`);
      if (!r.ok) throw new Error(`shaders/${name}.wgsl: ${r.status}`);
      const text = await r.text();
      step(`shaders/${name}.wgsl`);
      return [name, text];
    })
  ));
  // Each one preceded by whatever it was written against.
  const whole = name => (SHADERS[name] ? whole(SHADERS[name]) : "") + src[name];
  return {
    TERRAIN: whole("terrain"),
    INSTANCED: whole("instanced"),
    PINE: whole("pine"),
    GRASS: whole("grass"),
    VIEWMODEL: whole("viewmodel"),
    TRACER: whole("tracer"),
    MUZZLE: whole("muzzle"),
    SHARD: whole("shard"),
    FIREBALL: whole("fireball"),
    BLAST: whole("blast"),
    METEOR: whole("meteor"),
    SHARK: whole("shark"),
    PROP: whole("prop"),
    CRATE: whole("crate"),
    ZAP: whole("zap"),
    MARK: whole("mark"),
    SPARK: whole("spark"),
    HUD: whole("hud"),
    AK: whole("ak"),
    STONE: whole("stone"),
    LAUNCHER: whole("launcher"),
    WARHEAD: whole("warhead"),
    ROCKET: whole("rocket"),
    SHADOW_TERRAIN: whole("shadow-terrain"),
    SHADOW_INST: whole("shadow-instanced"),
    SHADOW_PINE: whole("shadow-pine"),
    FIGURE: whole("figure"),
    HELD: whole("held"),
    SHADOW_FIGURE: whole("shadow-figure"),
    SKY: whole("sky"),
    WATER: whole("water"),
    FIREFLY_SIM: whole("firefly-sim"),
    FIREFLY_DRAW: whole("firefly-draw"),
    GLOW: whole("glow"),
  };
}

// ---- small meshes, built here because they are renderer detail -------------
//
// Flat-shaded like the terrain: each triangle gets its face normal.

function flatMesh(tris) {
  const out = new Float32Array(tris.length * 3 * 6);
  let o = 0;
  for (const [p0, p1, p2] of tris) {
    const ax = p1[0]-p0[0], ay = p1[1]-p0[1], az = p1[2]-p0[2];
    const bx = p2[0]-p0[0], by = p2[1]-p0[1], bz = p2[2]-p0[2];
    let nx = ay*bz-az*by, ny = az*bx-ax*bz, nz = ax*by-ay*bx;
    const il = 1 / Math.hypot(nx, ny, nz);
    nx *= il; ny *= il; nz *= il;
    for (const p of [p0, p1, p2]) { out.set([...p, nx, ny, nz], o); o += 6; }
  }
  return out;
}

function coneRing(y0, y1, r0, r1, seg) {
  const tris = [];
  for (let i = 0; i < seg; i++) {
    const a0 = i / seg * Math.PI * 2, a1 = (i + 1) / seg * Math.PI * 2;
    const p00 = [Math.cos(a0)*r0, y0, Math.sin(a0)*r0], p10 = [Math.cos(a1)*r0, y0, Math.sin(a1)*r0];
    const p01 = [Math.cos(a0)*r1, y1, Math.sin(a0)*r1], p11 = [Math.cos(a1)*r1, y1, Math.sin(a1)*r1];
    tris.push([p00, p01, p10]);
    if (r1 > 0.001) tris.push([p10, p01, p11]);
  }
  return tris;
}

// The pine: a trunk, and branch cards carrying the needle atlas.
//
// Cones would be cheaper and were what stood here, but a conifer read from a
// distance is mostly silhouette, and a cone has none — every branch card is a
// hole with needles in it, which is what an alpha texture is for. Eight floats
// a vertex now: position, normal, and the atlas coordinate. The trunk has no
// atlas and says so with a negative u.
// `coarse` builds the same tree with far fewer, larger cards, for the shadow
// pass. Two things come out of that, and they are the two complaints.
//
// The shadows flickered because a shadow texel either caught a needle or did
// not, and the sun turns every frame — the ortho is fixed over the valley, so
// nothing swims when the camera moves, but the light's basis rotates and every
// texel lands somewhere new. Thin alpha-tested detail under that shimmers. Big
// cards do not: there is far less edge per texel to flip.
//
// And it was expensive because a depth-only pass that discards cannot use
// early-Z, so every covered pixel of every one of eighty-four crossed cards
// ran a texture fetch, on every pine, every frame. Sixteen cards instead, and
// the fetch takes a fixed coarse mip.
function pineMesh(coarse) {
  const out = [];
  const push = (p, n, uv) => out.push(p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1]);

  // The trunk. It runs the full height of the tree and tapers to a spire, so
  // the crown has something to hang off instead of ending in a cloud of loose
  // cards. Its u is shifted into [-2, -1] — negative u is what tells the
  // shader to reach for bark instead of the needle atlas, and the shift lets
  // the same two floats still carry a real coordinate.
  const top = 2.62, r0 = 0.10, r1 = 0.012, seg = 7;
  for (let i = 0; i < seg; i++) {
    const a0 = (i / seg) * Math.PI * 2, a1 = ((i + 1) / seg) * Math.PI * 2;
    const ring = (a, r, y) => [Math.cos(a) * r, y, Math.sin(a) * r];
    const p00 = ring(a0, r0, 0), p10 = ring(a1, r0, 0);
    const p01 = ring(a0, r1, top), p11 = ring(a1, r1, top);
    const u0 = -2 + i / seg, u1 = -2 + (i + 1) / seg;
    // Smooth around the trunk rather than flat-shaded: a five-sided post reads
    // as a post, and bark that lights per facet gives the flatness away.
    const nOf = a => [Math.cos(a), 0.16, Math.sin(a)];
    const n0 = nOf(a0), n1 = nOf(a1);
    push(p00, n0, [u0, 0]); push(p01, n0, [u0, 1]); push(p10, n1, [u1, 0]);
    push(p10, n1, [u1, 0]); push(p01, n0, [u0, 1]); push(p11, n1, [u1, 1]);
  }

  // Twelve whorls of branches up that trunk, each shorter and sparser than the
  // one below it — the conical silhouette comes out of the taper, not out of a
  // shape carved by hand. A card stands *upright*: its width is vertical and
  // its length reaches outward, with the twig's stem at the trunk and its tip
  // at the far end. A card lying flat would vanish when you look at it from
  // the side, which is where a walking camera looks at a tree from.
  const tiers = [];
  const whorls = coarse ? 4 : 12;
  for (let i = 0; i < whorls; i++) {
    const u = whorls === 1 ? 0 : i / (whorls - 1);  // 0 at the lowest whorl, 1 at the spire
    tiers.push({
      y: 0.95 + u * 1.60,
      len: 0.22 + 1.05 * (1 - u) ** 1.1,
      // Taller cards when there are fewer of them, so the canopy still covers
      // the ground it used to rather than turning into four thin discs.
      tall: (0.30 + 0.75 * (1 - u)) * (coarse ? 2.1 : 1.0),
      drop: 0.10 + 0.22 * (1 - u),
      n: coarse ? 4 : Math.round(9 - 4 * u),
    });
  }
  for (const [ti, t] of tiers.entries()) {
    for (let k = 0; k < t.n; k++) {
      const a = (k / t.n) * Math.PI * 2 + ti * 0.7;
      const d = [Math.cos(a), 0, Math.sin(a)];
      const perp = [-d[2], 0, d[0]];
      const r = r0 + (r1 - r0) * (t.y / top);   // the trunk's own radius here
      const base = [d[0] * r, t.y, d[2] * r];
      const tip = [d[0] * t.len, t.y - t.drop, d[2] * t.len];
      // Up-biased, so foliage is lit like foliage rather than like a wall and
      // the two faces of a card do not read as different materials.
      const n = [d[0] * 0.3, 0.86, d[2] * 0.3];
      // Two quads crossed about the branch's own axis. One alone disappears
      // when you look along it, and a canopy that thins out from some angles
      // is a canopy the trunk shows through.
      for (const roll of [0.30, 0.30 + Math.PI / 2]) {
        const w = [perp[0] * Math.sin(roll), Math.cos(roll), perp[2] * Math.sin(roll)];
        const at = (p, sgn, half) =>
          [p[0] + w[0] * sgn * half, p[1] + w[1] * sgn * half, p[2] + w[2] * sgn * half];
        const hIn = t.tall * 0.18, hOut = t.tall * 0.5;   // narrow at the stem
        const p00 = at(base, -1, hIn), p01 = at(base, 1, hIn);
        const p10 = at(tip, -1, hOut), p11 = at(tip, 1, hOut);
        // v = 1 at the stem, 0 at the tip: the atlas has the twig pointing up.
        push(p00, n, [0, 1]); push(p01, n, [1, 1]); push(p11, n, [1, 0]);
        push(p00, n, [0, 1]); push(p11, n, [1, 0]); push(p10, n, [0, 0]);
      }
    }
  }
  return new Float32Array(out);
}

// A block of the walls on the tops. Six faces, flat-shaded, in the same six
// floats a crystal's mesh uses so it can go through the same layout.
function blockMesh() {
  const out = [];
  const push = (p, n) => out.push(p[0], p[1], p[2], n[0], n[1], n[2]);
  // Each face as two triangles, wound so the outside faces out.
  const faces = [
    [[0, 0, 1], [[-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]]],
    [[0, 0, -1], [[1, -1, -1], [-1, -1, -1], [-1, 1, -1], [1, 1, -1]]],
    [[1, 0, 0], [[1, -1, 1], [1, -1, -1], [1, 1, -1], [1, 1, 1]]],
    [[-1, 0, 0], [[-1, -1, -1], [-1, -1, 1], [-1, 1, 1], [-1, 1, -1]]],
    [[0, 1, 0], [[-1, 1, 1], [1, 1, 1], [1, 1, -1], [-1, 1, -1]]],
    [[0, -1, 0], [[-1, -1, -1], [1, -1, -1], [1, -1, 1], [-1, -1, 1]]],
  ];
  for (const [n, q] of faces) {
    const h = q.map(p => [p[0] * 0.5, p[1] * 0.5, p[2] * 0.5]);
    push(h[0], n); push(h[1], n); push(h[2], n);
    push(h[0], n); push(h[2], n); push(h[3], n);
  }
  return new Float32Array(out);
}

function faceNormal(a, b, c) {
  const u = [b[0]-a[0], b[1]-a[1], b[2]-a[2]], v = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
  const n = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]];
  const l = Math.hypot(...n) || 1;
  return [n[0]/l, n[1]/l, n[2]/l];
}

// ---- the skinned figure ----------------------------------------------------
//
// `tools/bake_figure.py` writes the file this reads; the layout is documented
// at the top of that script. Everything here is a view onto the bytes that
// were fetched rather than a copy of them — the vertices and the indices go
// straight to the GPU, and only the skeleton and the clips are unpacked,
// because those are what the page does arithmetic with sixty times a second.
function readFigure(bytes) {
  const v = new DataView(bytes);
  const magic = new TextDecoder().decode(new Uint8Array(bytes, 0, 8));
  if (magic !== "RILLFIG4") throw new Error(`figure.mesh is not a figure this reads (${magic})`);
  const joints = v.getUint32(8, true), verts = v.getUint32(12, true);
  const indices = v.getUint32(16, true), clipCount = v.getUint32(20, true);
  // Which joint a weapon hangs off, and where on it — worked out at bake time
  // from the arm's own direction in the bind pose rather than by eye.
  const hand = v.getUint32(24, true);
  const grip = new Float32Array(bytes.slice(28, 92));
  const box = new Float32Array(bytes.slice(92, 116));
  // Which joints move with the arms rather than with the legs, one byte
  // apiece. Worked out at bake time, where something knows what a pelvis is.
  const upper = new Uint8Array(bytes.slice(116, 116 + joints));

  let at = 116 + ((joints + 3) & ~3);
  const parent = new Int32Array(joints);
  const bindT = new Float32Array(joints * 3);
  const bindR = new Float32Array(joints * 4);
  const invBind = new Float32Array(joints * 16);
  for (let j = 0; j < joints; j++) {
    parent[j] = v.getInt32(at, true); at += 4;
    for (let k = 0; k < 3; k++) { bindT[j * 3 + k] = v.getFloat32(at, true); at += 4; }
    for (let k = 0; k < 4; k++) { bindR[j * 4 + k] = v.getFloat32(at, true); at += 4; }
    for (let k = 0; k < 16; k++) { invBind[j * 16 + k] = v.getFloat32(at, true); at += 4; }
  }

  const vertexAt = at;
  at += verts * 20;
  const indexAt = at;
  at += indices * 2;

  const clips = new Map();
  for (let c = 0; c < clipCount; c++) {
    const len = v.getUint8(at); at += 1;
    const name = new TextDecoder().decode(new Uint8Array(bytes, at, len)); at += len;
    const frames = v.getUint32(at, true); at += 4;
    const fps = v.getFloat32(at, true); at += 4;
    const loops = v.getUint8(at) !== 0; at += 1;
    const root = v.getUint8(at); at += 1;
    const rootT = new Float32Array(bytes.slice(at, at + frames * 12)); at += frames * 12;
    const rot = new Int16Array(bytes.slice(at, at + frames * joints * 8)); at += frames * joints * 8;
    clips.set(name, { name, frames, fps, loops, root, rootT, rot, seconds: (frames - 1) / fps });
  }
  return { joints, verts, indices, box, hand, grip, upper, parent, bindT, bindR, invBind, clips, vertexAt, indexAt };
}

// A pose, worked out on the page and handed over as one matrix per joint.
//
// The alternative is to send the skeleton and the clips to the GPU and skin in
// a compute pass, which is what a game with a hundred people on screen does.
// Eight of them is fifty-three small matrix products each, which is nothing,
// and it keeps the whole animation in one place where it can be read.
function makeSkinner(figure, capacity) {
  const J = figure.joints;
  const local = new Float32Array(J * 16);
  const world = new Float32Array(J * 16);
  const out = new Float32Array(capacity * J * 16);
  // Where each person's rifle is. The hand's world matrix rather than its
  // skinning matrix: what a weapon wants is where the hand *is*, not the
  // difference between where it is and where it was bound.
  // One spare: everybody else's rifle is held in their hand, and the extra is
  // this player's own once they have dropped it. There is no figure for
  // yourself — you are the camera — so there is no hand to hang it off.
  const held = new Float32Array((capacity + 1) * 16);

  // Column-major, as WGSL wants it: o = a * b.
  const mul = (o, oi, a, ai, b, bi) => {
    for (let c = 0; c < 4; c++) {
      for (let r = 0; r < 4; r++) {
        o[oi + c * 4 + r] = a[ai + r] * b[bi + c * 4]
                          + a[ai + 4 + r] * b[bi + c * 4 + 1]
                          + a[ai + 8 + r] * b[bi + c * 4 + 2]
                          + a[ai + 12 + r] * b[bi + c * 4 + 3];
      }
    }
  };

  // A rotation and a translation straight into a matrix; nothing in this rig
  // is ever scaled, which the bake checks rather than assumes.
  const trs = (m, at, x, y, z, qx, qy, qz, qw) => {
    const x2 = qx + qx, y2 = qy + qy, z2 = qz + qz;
    const xx = qx * x2, xy = qx * y2, xz = qx * z2;
    const yy = qy * y2, yz = qy * z2, zz = qz * z2;
    const wx = qw * x2, wy = qw * y2, wz = qw * z2;
    m[at] = 1 - (yy + zz); m[at + 1] = xy + wz; m[at + 2] = xz - wy; m[at + 3] = 0;
    m[at + 4] = xy - wz; m[at + 5] = 1 - (xx + zz); m[at + 6] = yz + wx; m[at + 7] = 0;
    m[at + 8] = xz + wy; m[at + 9] = yz - wx; m[at + 10] = 1 - (xx + yy); m[at + 11] = 0;
    m[at + 12] = x; m[at + 13] = y; m[at + 14] = z; m[at + 15] = 1;
  };

  // Where the player is, as a matrix: turned about the vertical, then put down
  // on the ground.
  //
  // The angle is not the yaw and not the yaw plus half a turn. This rotation
  // takes the model's forward — the mannequin faces +z, which its toes and its
  // nose agree about — to (sin a, 0, cos a), and Rill's `walk` means
  // (sin yaw, 0, -cos yaw) by a yaw. Those meet at a = π - yaw, and π + yaw
  // meets them nowhere: it mirrors rather than turns, so everybody walked
  // sideways-on to the way they were looking, and worse, the mirror is
  // invisible on a figure that is roughly symmetric until it moves.
  const place = (m, x, y, z, yaw) => {
    const a = Math.PI - yaw, c = Math.cos(a), s = Math.sin(a);
    m[0] = c; m[1] = 0; m[2] = -s; m[3] = 0;
    m[4] = 0; m[5] = 1; m[6] = 0; m[7] = 0;
    m[8] = s; m[9] = 0; m[10] = c; m[11] = 0;
    m[12] = x; m[13] = y; m[14] = z; m[15] = 1;
  };

  const model = new Float32Array(16);
  const rooted = new Float32Array(16);
  // Two quaternions of scratch: the clip somebody is doing, and the armed pose
  // being mixed into it.
  const qa = new Float32Array(4), qb = new Float32Array(4);

  // One joint out of one clip: the two frames either side of the moment, the
  // short way round between them.
  const sample = (k, j, out) => {
    const a = (k.f0 * J + j) * 4, b = (k.f1 * J + j) * 4;
    const ax = k.rot[a] / 32767, ay = k.rot[a + 1] / 32767;
    const az = k.rot[a + 2] / 32767, aw = k.rot[a + 3] / 32767;
    let bx = k.rot[b] / 32767, by = k.rot[b + 1] / 32767;
    let bz = k.rot[b + 2] / 32767, bw = k.rot[b + 3] / 32767;
    if (ax * bx + ay * by + az * bz + aw * bw < 0) { bx = -bx; by = -by; bz = -bz; bw = -bw; }
    out[0] = ax + (bx - ax) * k.t; out[1] = ay + (by - ay) * k.t;
    out[2] = az + (bz - az) * k.t; out[3] = aw + (bw - aw) * k.t;
  };

  // `a` moves `w` of the way to `b`, the short way round. Straight lerp and a
  // normalise rather than a proper slerp: these two are never far apart — one
  // is a man running and the other is the same man holding a rifle — and at
  // that separation the difference is a fraction of a degree in the middle of
  // the arc, which nothing on a screen can tell.
  const mix = (a, b, w) => {
    if (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3] < 0) {
      b[0] = -b[0]; b[1] = -b[1]; b[2] = -b[2]; b[3] = -b[3];
    }
    for (let i = 0; i < 4; i++) a[i] += (b[i] - a[i]) * w;
  };

  return {
    matrices: out,
    held,
    // A rifle lying on the ground, on its side, pointing the way whoever was
    // holding it was facing and turned a little off it — a weapon that landed
    // square with its owner reads as placed rather than dropped.
    //
    // The scale comes off the grip, which is where the bake put the only
    // opinion about how long a rifle is. Nothing here has a second one.
    drop(slot, x, y, z, yaw, spin) {
      const s = Math.hypot(figure.grip[0], figure.grip[1], figure.grip[2]);
      const a = yaw + spin;
      const dx = Math.sin(a), dz = -Math.cos(a);
      const at = slot * 16;
      // A pace out in front of where they went down, rather than under them.
      // For everybody else that is the difference between a rifle on the grass
      // and a rifle inside a body; for this player it is the difference
      // between seeing it and not, because the camera they are left with lies
      // at their own feet looking where they were last looking.
      x += Math.sin(yaw) * 0.9;
      z -= Math.cos(yaw) * 0.9;
      // Barrel along the facing, the rifle's own up laid across it — so it is
      // on its side — and the third axis falls out pointing at the ground.
      held[at] = dx * s; held[at + 1] = 0; held[at + 2] = dz * s; held[at + 3] = 0;
      held[at + 4] = -dz * s; held[at + 5] = 0; held[at + 6] = dx * s; held[at + 7] = 0;
      held[at + 8] = 0; held[at + 9] = -s; held[at + 10] = 0; held[at + 11] = 0;
      held[at + 12] = x; held[at + 13] = y + 0.05; held[at + 14] = z; held[at + 15] = 1;
    },
    // One person: their clip, how far into it, and where they are standing.
    //
    // `over` is a second clip for the upper body, and it exists because the
    // library has a walk and an armed idle and no armed walk. The legs take
    // the clip somebody is actually doing; the spine and everything above it
    // takes the one where they are holding the rifle. The hips are below the
    // split, so the pelvis still sways with the stride and the torso is
    // carried by it rather than nailed to the world — which is the whole
    // reason a mask is done on the skeleton and not by playing two clips and
    // fading between them.
    pose(slot, clip, time, x, y, z, yaw, over = null, overTime = 0) {
      const cut = (c, s) => {
        const f = c.loops
          ? (Math.max(s, 0) * c.fps) % (c.frames - 1)
          : Math.min(Math.max(s, 0) * c.fps, c.frames - 1);
        const f0 = Math.floor(f);
        return { rot: c.rot, f0, f1: Math.min(f0 + 1, c.frames - 1), t: f - f0 };
      };
      const low = cut(clip, time);
      const up = over ? cut(over, overTime) : low;
      const { f0, f1, t } = low;

      for (let j = 0; j < J; j++) {
        // What they are doing, and then however much of the armed pose this
        // joint is asked to carry — all of it in the hands, none of it in the
        // legs, and a proportion in between. A weight and not a flag: taken as
        // a flag the whole torso came from the idle and the legs pumped
        // underneath a body that did not move.
        sample(low, j, qa);
        const w = over ? figure.upper[j] / 255 : 0;
        if (w > 0.001) { sample(up, j, qb); mix(qa, qb, w); }
        let qx = qa[0], qy = qa[1], qz = qa[2], qw = qa[3];
        const n = Math.hypot(qx, qy, qz, qw) || 1;
        qx /= n; qy /= n; qz /= n; qw /= n;

        let tx = figure.bindT[j * 3], ty = figure.bindT[j * 3 + 1], tz = figure.bindT[j * 3 + 2];
        if (j === clip.root) {
          const r0 = f0 * 3, r1 = f1 * 3;
          tx = clip.rootT[r0] + (clip.rootT[r1] - clip.rootT[r0]) * t;
          ty = clip.rootT[r0 + 1] + (clip.rootT[r1 + 1] - clip.rootT[r0 + 1]) * t;
          tz = clip.rootT[r0 + 2] + (clip.rootT[r1 + 2] - clip.rootT[r0 + 2]) * t;
        }
        trs(local, j * 16, tx, ty, tz, qx, qy, qz, qw);
      }

      place(model, x, y, z, yaw);
      for (let j = 0; j < J; j++) {
        const p = figure.parent[j];
        if (p < 0) mul(world, j * 16, model, 0, local, j * 16);
        else mul(world, j * 16, world, p * 16, local, j * 16);
      }
      for (let j = 0; j < J; j++) {
        mul(rooted, 0, world, j * 16, figure.invBind, j * 16);
        out.set(rooted, (slot * J + j) * 16);
      }
      mul(rooted, 0, world, figure.hand * 16, figure.grip, 0);
      held.set(rooted, slot * 16);
    },
  };
}

function crystalMesh() {
  const tris = [];
  const top = [0, 2.4, 0], bot = [0, -0.25, 0], seg = 5, r = 0.42, ym = 0.7;
  for (let i = 0; i < seg; i++) {
    const a0 = i / seg * Math.PI * 2, a1 = (i + 1) / seg * Math.PI * 2;
    const m0 = [Math.cos(a0)*r, ym, Math.sin(a0)*r], m1 = [Math.cos(a1)*r, ym, Math.sin(a1)*r];
    tris.push([m0, top, m1], [bot, m0, m1]);
  }
  return flatMesh(tris);
}

// Three crossed blades, normals up so a tuft is lit like the ground.
// The undergrowth: two crossed cards carrying one cell of the plant atlas.
//
// One square card serves all four plants. Each was pasted into its cell at
// its own aspect and stood on the cell's floor, so a tuft comes out low and
// wide and a fern comes out tall and narrow from the same four corners — the
// rest of the card is transparent and gets discarded. The alternative, a mesh
// shaped per plant, would need a draw call per plant.
function grassMesh() {
  const out = [];
  const push = (p, n, uv) => out.push(p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1]);
  for (const a of [0.0, Math.PI / 2]) {
    const dx = Math.cos(a) * 0.5, dz = Math.sin(a) * 0.5;
    // Up-biased, so a leaf is lit like a leaf from either side.
    const n = [Math.sin(a) * 0.32, 0.95, -Math.cos(a) * 0.32];
    const p00 = [-dx, 0, -dz], p10 = [dx, 0, dz];
    const p01 = [-dx, 1, -dz], p11 = [dx, 1, dz];
    // v = 1 at the root: the atlas has each plant standing on its cell floor.
    push(p00, n, [0, 1]); push(p10, n, [1, 1]); push(p11, n, [1, 0]);
    push(p00, n, [0, 1]); push(p11, n, [1, 0]); push(p01, n, [0, 0]);
  }
  return new Float32Array(out);
}

// ---- the program -----------------------------------------------------------

// `server` is a WebSocket URL, and with one the valley belongs to whoever is
// running it: this browser walks, aims and shoots, and is told what any of
// that came to. Without one the valley is this tab's alone and nothing here
// changes — which is the version that goes on a static page, where there is
// no server to talk to. `name` is what the others will see over this player's
// head.
export async function run(canvas, wasmURL, hud, progress = () => {}, server = null, name = "") {
  // Everything the first frame needs, counted up front so the loader can say
  // how far along it is: the adapter, the module, one step per shader, one per
  // texture, and the valley itself. `run` does not resolve until a frame has
  // actually been submitted, so whatever is covering the canvas comes off over
  // a painted one rather than over black.
  // Nine fetch/build steps, one per shader, one per texture, and six decodes.
  const TOTAL = 15 + Object.keys(SHADERS).length + TEXTURES.length + 14 + (server ? 1 : 0);
  let done = 0;
  // Never past the end. This is a count kept by hand of steps taken elsewhere,
  // and the two drift every time something is added — a bar that says 104% is
  // a smaller problem than the one it points at, but it is the visible one.
  const step = (what) => progress(Math.min(++done, TOTAL), TOTAL, what);

  // Named, because the message is the whole of what a visitor gets. Safari
  // only shipped WebGPU on iPhone and iPad in version 26, so an older tablet
  // lands here and deserves to be told which of the two it is rather than
  // being left to guess from the word "WebGPU".
  if (!navigator.gpu)
    throw new Error("this browser has no WebGPU — on iPhone or iPad it needs " +
                    "Safari 26 or newer, and on a desktop a current Chrome, " +
                    "Edge or Safari");
  // Ask more than once before giving up.
  //
  // `requestAdapter()` with no options returns null on a machine that has a
  // perfectly good GPU more often than you would think, and the usual reason on
  // Windows is that the browser has been told not to use one — Edge and Chrome
  // both have a "use graphics acceleration when available" switch, and with it
  // off there is no adapter to hand out however capable the card is. Remote
  // Desktop does the same thing, and so does a driver on the browser's
  // blocklist.
  //
  // So: the default, then explicitly the low-power one in case the discrete
  // card is the thing being refused, and finally the fallback adapter, which is
  // the browser's own software rasteriser. That last one is not fast — it is a
  // CPU drawing four hundred thousand triangles — but the tiering already
  // recognises it by name and starts at the bottom, and a valley at six frames
  // a second is a different thing from a page that will not open.
  let adapter = await navigator.gpu.requestAdapter();
  if (!adapter) adapter = await navigator.gpu.requestAdapter({ powerPreference: "low-power" });
  let software = false;
  if (!adapter) {
    adapter = await navigator.gpu.requestAdapter({ forceFallbackAdapter: true });
    software = !!adapter;
  }
  if (!adapter) throw new Error(
    "no GPU adapter.\n\n" +
    "the browser has WebGPU but would not give this page a GPU, and would not " +
    "give it a software one either. that is nearly always graphics " +
    "acceleration being switched off in the browser rather than anything to do " +
    "with this page:\n\n" +
    "  Edge    edge://settings/system — \"use graphics acceleration when available\"\n" +
    "  Chrome  chrome://settings/system — the same switch\n\n" +
    "then restart the browser. edge://gpu or chrome://gpu will say what it " +
    "thinks of your card. Remote Desktop and some virtual machines have no GPU " +
    "to give at all.");

  // The one feature worth asking for: GPU timestamps, so the HUD can show
  // what a frame costs the GPU rather than what rAF implies.
  const canTime = adapter.features.has("timestamp-query");
  const device = await adapter.requestDevice({
    requiredFeatures: canTime ? ["timestamp-query"] : [],
  });
  // Say it on the screen, not only in the console.
  //
  // On a phone or a tablet there is no console to open, so anything that only
  // reaches `console.error` may as well not have happened. Two things used to
  // go there and nowhere else, and between them they produce the one failure
  // that tells you nothing at all: a black screen.
  //
  // The worse of the two is a lost device, which was not being listened for in
  // any form. WebGPU does not throw when a device dies — every call after it is
  // quietly dropped — so the canvas simply stops changing and keeps whatever
  // was last drawn on it, which at startup is black. Nothing in the page says
  // anything, the frame loop goes on calling into a device that is gone, and
  // the report you get from the other end is "it is black on my iPad".
  //
  // Its own dim backdrop rather than the page's `.err`, because by the time
  // this fires there is usually a half-drawn valley underneath it.
  // Checked by the frame loop. There is nothing to be gained by going on: every
  // command after a loss is discarded, so the loop would burn a core redrawing
  // nothing behind the message that says so.
  let gone = false;
  // Accumulating rather than first-only. The first message a driver gives you
  // is rarely the useful one: a pipeline that fails to build takes every draw
  // that uses it down with it, and what you want to read is the list, because
  // the shape of the list is the diagnosis. Capped, because a failing pipeline
  // can report once per frame and a wall of the same line says nothing the
  // first one did not.
  let panel = null, lines = 0;
  const said = new Set();
  const shout = (what, detail) => {
    // Once each. Every pipeline builds its vertex and its fragment module from
    // the same source, so a bad shader reports itself twice before anything
    // else gets a line; and an invalid pipeline complains again on every frame
    // that tries to use it. Neither repetition carries information, and both
    // will happily fill the panel before the message you needed arrives.
    const key = `${what}\0${detail ?? ""}`;
    if (said.has(key)) return;
    said.add(key);
    if (lines >= 8) return;
    if (!panel) {
      // Above the loading curtain, and take the curtain down with it. The
      // curtain is z-index 10 and opaque, and it only lifts once the first
      // frame has been drawn — which is exactly what does not happen when a
      // pipeline has failed to build. The first version of this sat at 9 and
      // was therefore invisible behind the one thing guaranteed to still be
      // there: the message about why nothing was coming was hidden by the
      // screen that says something is coming.
      document.querySelector(".boot")?.remove();
      panel = document.createElement("div");
      panel.style.cssText = "position:fixed;inset:0;overflow:auto;" +
        "background:rgba(6,6,12,0.94);color:#c9c3e4;font:13px/1.6 ui-monospace,monospace;" +
        "white-space:pre-wrap;padding:2rem;z-index:11";
      document.body.appendChild(panel);
    }
    lines++;
    const row = document.createElement("div");
    row.style.cssText = "margin:0 0 1.1rem";
    row.textContent = detail ? `${what}\n  ${detail}` : what;
    panel.appendChild(row);
  };
  // If we got here on the fallback adapter, say so. It will run, and it will
  // run badly, and a visitor who is not told will read that as the page being
  // slow rather than as their browser drawing it on the CPU.
  if (software) shout("running on a software renderer",
    "the browser would not give this page a real GPU, so this is being drawn " +
    "on the processor and will be slow. graphics acceleration is probably " +
    "switched off: edge://settings/system or chrome://settings/system, then " +
    "restart the browser.");
  device.addEventListener("uncapturederror", e => {
    console.error("WEBGPU:", e.error.message);
    shout("the gpu rejected something", e.error.message);
  });
  // Resolves rather than rejects, and resolves on a deliberate `destroy()` too
  // — which this never does, so anything arriving here is a real loss.
  device.lost.then(info => {
    console.error("WEBGPU DEVICE LOST:", info.reason, info.message);
    gone = true;
    shout("the gpu device was lost", info.message ||
      "the browser took the gpu back. on iPadOS and iOS this is usually the " +
      "tab going over its memory budget: try closing other tabs and reloading.");
  });
  // Which one.
  //
  // "vertex library failed creation" — Metal's way of saying a shader would not
  // compile for the device — arrives through `uncapturederror` with nothing
  // attached to say which of the twenty pipelines it came from, which makes it
  // almost useless. Both of these attach the name.
  //
  // `mod` asks the module itself: WGSL is compiled by the browser, and any
  // complaint it has is available with a line and a column. That catches
  // anything wrong with the source.
  //
  // `watched` catches what `mod` cannot. A module can be perfectly valid WGSL
  // and still fail when the backend turns it into MSL for this particular
  // device — a driver limit, a construct the Metal compiler will not take. That
  // surfaces as an `internal` error against the pipeline, so the pipeline is
  // built inside an error scope named after it. Both scopes, because a bad
  // vertex layout is a `validation` error and a bad shader is an `internal`
  // one, and from a black screen you cannot tell which you are looking at.
  // Bake the pipeline constants into the source instead of overriding them.
  //
  // WGSL lets a module declare `override X: f32 = 0.0;` and a pipeline supply
  // the value, so one shader can serve several pipelines. Four here did that —
  // the crystals' emissive fraction, and the water's wave and ripple counts,
  // each in a main-pass and an environment-probe copy — and those four are
  // exactly the four that would not build on iPadOS. Every other pipeline in
  // the valley came up. The diagnosis was the list: not one of them reported a
  // WGSL error, so the source was fine; all four failed with Metal's "vertex
  // library failed creation", which is the backend refusing to turn the module
  // into a Metal library. WebKit implements overrides as Metal function
  // constants and does not get a vertex function out the other side.
  //
  // There is nothing to be gained by fighting that. An override is a
  // convenience for reusing one module across pipelines, and the substitution
  // it saves is a string replacement — so do the replacement. `override N: T =
  // d;` becomes `const N: T = value;`, the pipeline is built with no constants
  // at all, and the backend sees an ordinary shader with an ordinary literal in
  // it. Applied to every module whether it has constants or not, so the word
  // never reaches a driver: a shader that keeps its default is just one whose
  // default got written down.
  //
  // The literal has to match the declared type or the module will not compile —
  // an f32 written `1` is an abstract-int in WGSL and will not go into an f32.
  const wgslLit = (type, v) => {
    if (type === "bool") return v ? "true" : "false";
    if (type === "u32") return `${Math.round(v)}u`;
    if (type === "i32") return `${Math.round(v)}`;
    return Number.isInteger(v) ? `${v}.0` : String(v);
  };
  const spec = (code, values) => code.replace(
    /^([ \t]*)(?:@id\(\s*\d+\s*\)[ \t]*)?override[ \t]+([A-Za-z_]\w*)[ \t]*:[ \t]*(\w+)[ \t]*=[ \t]*([^;]+);/gm,
    (_, pad, name, type, dflt) => {
      const given = values && Object.prototype.hasOwnProperty.call(values, name);
      return `${pad}const ${name}: ${type} = ${given ? wgslLit(type, values[name]) : dflt.trim()};`;
    });

  // One module per distinct source. Each pipeline asks for its vertex and its
  // fragment stage separately and both come from the same file, so without this
  // every shader in the valley is compiled twice — which is a startup cost paid
  // most heavily by the machines least able to afford it.
  const modCache = new Map();
  const mod = (code, label, constants) => {
    const src = spec(code, constants);
    const key = `${label}\0${src}`;
    const had = modCache.get(key);
    if (had) return had;
    const m = device.createShaderModule({ code: src, label });
    modCache.set(key, m);
    m.getCompilationInfo?.().then(info => {
      const bad = (info.messages ?? []).filter(x => x.type === "error");
      if (bad.length) shout(`shader "${label}" did not compile`,
        bad.map(x => `line ${x.lineNum}:${x.linePos}  ${x.message}`).join("\n  "));
    }).catch(() => {});
    return m;
  };
  const watched = (label, build) => {
    device.pushErrorScope("internal");
    device.pushErrorScope("validation");
    const made = build();
    const say = kind => e => { if (e) { console.error(`PIPELINE ${label}:`, e.message);
                                        shout(`pipeline "${label}" (${kind})`, e.message); } };
    device.popErrorScope().then(say("validation")).catch(() => {});
    device.popErrorScope().then(say("internal")).catch(() => {});
    return made;
  };

  const gpuInfo = adapter.info ?? {};
  const gpuName = [gpuInfo.vendor, gpuInfo.architecture].filter(Boolean).join(" ") || "gpu";
  const maxTex = adapter.limits.maxTextureDimension2D;
  step(gpuName);

  const context = canvas.getContext("webgpu");
  const format = navigator.gpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "opaque" });

  // The world, from Rill; the generated loader does the ceremony.
  const E = await load(wasmURL);
  step("world.wasm");
  const S = await loadShaders(step);

  // How big the module is, read off the browser's own record of the request
  // the loader already made — no second one. `decodedBodySize` rather than
  // `encodedBodySize`: what is wanted on screen is the size of the thing, not
  // what a CDN's gzip happened to squeeze it to on the way. The entry lands a
  // moment after the fetch resolves, so this asks again until it is there.
  let wasmBytes = 0;
  const wasmSize = () => {
    if (wasmBytes) return wasmBytes;
    const e = performance.getEntriesByType("resource").find(r => r.name.includes(".wasm"));
    wasmBytes = e ? e.decodedBodySize || e.encodedBodySize || 0 : 0;
    return wasmBytes;
  };

  const gpuBuf = (data) => {
    const b = device.createBuffer({ size: (data.byteLength + 3) & ~3, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
    device.queue.writeBuffer(b, 0, data);
    return b;
  };

  // Static GPU objects: uniforms, sampler, small meshes, layouts, and every
  // pipeline that does not depend on the tier.
  // Sized from the struct, not from a number that has to be remembered. It was
  // 56 floats and the shark added sixteen more; a staging array that grew while
  // the GPU buffer did not is a `writeBuffer` overrun, which is a validation
  // error, an invalid pipeline and a black screen with nothing in the console
  // that names the cause.
  // 116 for the boxes and the world, plus four for `fx` — the muzzle flash and
  // three spare. Grow this and the struct in uniforms.wgsl together: a staging
  // array that grows while the GPU buffer does not is a `writeBuffer` overrun,
  // which is a validation error and a black screen.
  //
  // Then four for this player's lamp and forty-eight for everybody else's —
  // six beams, two vectors each. See `lamps` in uniforms.wgsl. A vec4 array in
  // a uniform block is packed sixteen bytes to the element, which is what
  // these already are, so the offsets are simply 124 onward with nothing to
  // pad.
  const LAMP_CAP = 6;
  const UNI_FLOATS = 124 + LAMP_CAP * 8;
  const uni = device.createBuffer({ size: UNI_FLOATS * 4, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
  const flyTime = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
  const shadowSamp = device.createSampler({ compare: "less" });

  // The two pine textures. Mip levels are made by asking the browser to decode
  // the file again at each size — cheaper to write than a blit pass, and alpha
  // tested foliage without mips shimmers the moment it is more than a few
  // trees away.
  async function mipped(url, w0, h0) {
    const bytes = await (await fetch(url)).arrayBuffer();
    const levels = [];
    for (let w = w0, h = h0; w >= 1 && h >= 1; w >>= 1, h >>= 1) {
      levels.push(await createImageBitmap(new Blob([bytes]),
        { resizeWidth: w, resizeHeight: h, resizeQuality: "high" }));
    }
    const tex = device.createTexture({
      size: [w0, h0], mipLevelCount: levels.length, format: "rgba8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
    });
    levels.forEach((bmp, i) => device.queue.copyExternalImageToTexture(
      { source: bmp }, { texture: tex, mipLevel: i }, [bmp.width, bmp.height]));
    return tex.createView();
  }
  // All six at once. Each one is a fetch and then a decode per mip level, and
  // the decodes are the browser's work rather than ours; done one after
  // another they were the longest thing between the page opening and the
  // valley appearing.
  const [foliageView, barkView, groundView, plantView, rockView, sandView, rifleView, meteorView,
         jupiterView, saturnView, moonsView, sharkView, propView, rpgView] =
    await Promise.all(TEXTURES.map(([url, w, h]) =>
      mipped(`${url}?v=${V}`, w, h).then(v => { step(url.slice(2)); return v; })));
  const foliageSamp = device.createSampler({
    magFilter: "linear", minFilter: "linear", mipmapFilter: "linear",
    addressModeU: "clamp-to-edge", addressModeV: "clamp-to-edge",
  });
  // Bark tiles up and around the trunk, so it repeats where the atlas clamps.
  const barkSamp = device.createSampler({
    magFilter: "linear", minFilter: "linear", mipmapFilter: "linear",
    addressModeU: "repeat", addressModeV: "repeat",
  });

  const bglMain = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: {} },
    { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: "depth" } },
    { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "comparison" } },
    { binding: 3, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 4, visibility: GPUShaderStage.FRAGMENT, sampler: {} },
    { binding: 5, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 6, visibility: GPUShaderStage.FRAGMENT, sampler: {} },
    { binding: 7, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 8, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 9, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 10, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 11, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 12, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    // The valley, seen from a point in the middle of it and stored on the six
    // faces of a cube. Anything that wants to know what is around it rather
    // than what is lighting it reads this.
    { binding: 13, visibility: GPUShaderStage.FRAGMENT, texture: { viewDimension: "cube" } },
    { binding: 14, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 15, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 16, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 17, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 18, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 19, visibility: GPUShaderStage.FRAGMENT, texture: {} },
  ]});
  const bglShadow = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: {} },
    { binding: 3, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 4, visibility: GPUShaderStage.FRAGMENT, sampler: {} },
  ]});
  const bgShadow = device.createBindGroup({ layout: bglShadow, entries: [
    { binding: 0, resource: { buffer: uni } },
    { binding: 3, resource: foliageView },
    { binding: 4, resource: foliageSamp },
  ]});

  const terrainLayout = [{ arrayStride: 36, attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
    { shaderLocation: 2, offset: 24, format: "float32x3" },
  ]}];
  // Position, normal, atlas coordinate.
  const pineLayout = { arrayStride: 32, attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
    { shaderLocation: 6, offset: 24, format: "float32x2" },
  ] };
  const meshLayout = { arrayStride: 24, attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
  ]};
  // The rifle, quantized: position as three unorm16 across the model's own
  // bounding box, normal as three snorm8, the atlas coordinate as two
  // unorm16. Sixteen bytes a vertex against thirty-two, and indexed, which
  // between them take the file from 1.9 MB to 423 KB.
  const vmLayout = [{ arrayStride: 16, attributes: [
    { shaderLocation: 0, offset: 0, format: "unorm16x4" },
    { shaderLocation: 1, offset: 8, format: "snorm8x4" },
    { shaderLocation: 2, offset: 12, format: "unorm16x2" },
  ]}];
  // One bullet per instance: position, velocity, a spare, and the life left.
  const bulletLayout = [{ arrayStride: 32, stepMode: "instance", attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
    { shaderLocation: 2, offset: 24, format: "float32" },
    { shaderLocation: 3, offset: 28, format: "float32" },
  ]}];
  // A shard: twenty floats in Rill's pool, of which the renderer wants four —
  // where it is, how it is turned, how big it is, and what colour it was.
  const shardLayout = { arrayStride: 80, stepMode: "instance", attributes: [
    { shaderLocation: 2, offset: 0, format: "float32x3" },    // position
    { shaderLocation: 3, offset: 24, format: "float32x3" },   // orientation
    { shaderLocation: 4, offset: 48, format: "float32" },     // scale
    { shaderLocation: 5, offset: 52, format: "float32x3" },   // colour
  ]};
  // Nine floats a fireball, not the bullet's eight: the trail needs to know how
  // long the thing has been in the air, or it draws smoke from before the
  // fireball existed.
  // A blast: where it landed, a seed, and how long ago. Eight floats in Rill,
  // of which the renderer wants five.
  const blastLayout = [{ arrayStride: 32, stepMode: "instance", attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32" },
    { shaderLocation: 2, offset: 16, format: "float32" },
  ]}];
  const fireLayout = [{ arrayStride: 36, stepMode: "instance", attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
    { shaderLocation: 2, offset: 24, format: "float32" },
    { shaderLocation: 3, offset: 28, format: "float32" },
    { shaderLocation: 4, offset: 32, format: "float32" },
  ]}];
  const instLayout = { arrayStride: 32, stepMode: "instance", attributes: [
    { shaderLocation: 2, offset: 0, format: "float32x3" },
    { shaderLocation: 3, offset: 12, format: "float32" },
    { shaderLocation: 4, offset: 16, format: "float32" },
    { shaderLocation: 5, offset: 20, format: "float32x3" },
  ]};

  const layout = device.createPipelineLayout({ bindGroupLayouts: [bglMain] });
  const MS = { count: 4 };
  const depth = { format: "depth24plus", depthWriteEnabled: true, depthCompare: "less" };
  const additive = { color: { srcFactor: "one", dstFactor: "one" }, alpha: { srcFactor: "one", dstFactor: "one" } };

  const pipe = (label, code, buffers, opts = {}) => watched(label, () => device.createRenderPipeline({
    label, layout,
    // No `constants` on either stage: the values are already in the source that
    // `mod` handed to the driver. See `spec` for why they cannot be passed here.
    vertex: { module: mod(code, label, opts.constants), buffers },
    fragment: { module: mod(code, label, opts.constants),
                targets: [{ format: opts.format ?? format, blend: opts.blend }] },
    primitive: { cullMode: opts.cull ?? "back" },
    depthStencil: { ...depth, ...opts.depth },
    multisample: MS,
  }));

  // Premultiplied alpha: the shader hands over colour already multiplied by
  // coverage, and coverage is what limits how much of a pixel each thing may
  // claim. Additive has no such limit, which is fine for one spark and ruinous
  // for fifty of them stacked along a line of sight — or for a crystal, which
  // would simply add its colour to the hillside behind it and glow white.
  //
  // Declared up here because the crystals want it too, and `const` in a
  // temporal dead zone is a runtime error rather than a compile one: the
  // pipeline below would have thrown on the first frame with nothing rendered
  // and nothing said.
  const overPre = { color: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
                    alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" } };

  const skyPipe = pipe("sky", S.SKY, [], { depth: { depthWriteEnabled: false, depthCompare: "less-equal" }, cull: "none" });
  const terrainPipe = pipe("terrain", S.TERRAIN, terrainLayout);
  const pinePipe = pipe("pine", S.PINE, [pineLayout, instLayout], { cull: "none" });
  // Glass, so: blended, both faces drawn, and writing no depth of its own.
  //
  // All three are needed together and each fixes something the others cannot.
  // Blending is what makes it see-through. Drawing the back faces is what puts
  // anything *inside* it to see — a transparent prism with its far side culled
  // is a coloured window, not a solid of glass. And a transparent thing must
  // not write depth, or it punches a hole in everything drawn after it: the
  // grass and the water behind a crystal would simply be missing.
  const stonePipe = pipe("stone", S.STONE, [meshLayout, instLayout]);
  const crystalPipe = pipe("crystal", S.INSTANCED, [meshLayout, instLayout],
    { constants: { emissive: 0.72 }, blend: overPre, cull: "none",
      depth: { depthWriteEnabled: false } });
  const grassPipe = pipe("grass", S.GRASS, [pineLayout, instLayout], { cull: "none" });
  const glowPipe = pipe("glow", S.GLOW, [instLayout], { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  // Backfaces culled, unlike the foliage: the butt sits behind the eye and is
  // cut open by the near plane, and a culled interior reads as "not there"
  // where an unculled one reads as a dark wall across the bottom of the screen.
  const vmPipe = pipe("viewmodel", S.VIEWMODEL, vmLayout);
  // Its own shader: the model came without texture coordinates, so the wood
  // and the steel are worked out from where a point sits on the weapon.
  const akPipe = pipe("ak", S.AK, vmLayout);
  // The shark. Culled normally, depth-tested and depth-writing like any solid
  // — it is only unusual in when it is drawn at all, which is decided in the
  // frame rather than here.
  const sharkLayout = [{ arrayStride: 20, attributes: [
    { shaderLocation: 0, offset: 0,  format: "unorm16x4" },   // position in the box
    { shaderLocation: 1, offset: 8,  format: "snorm8x4" },    // normal
    { shaderLocation: 2, offset: 12, format: "snorm16x2" },   // uv, times 4096
    { shaderLocation: 3, offset: 16, format: "unorm8x4" },    // flat colour
  ]}];
  const sharkPipe = pipe("shark", S.SHARK, sharkLayout);
  // Additive and depth-tested but not depth-writing: a tracer is light in the
  // air, so the ground hides it and it hides nothing.
  const tracerPipe = pipe("tracer", S.TRACER, bulletLayout,
    { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  // The crystals' bites: a handful of pinned beams, uploaded fresh each frame.
  const zapLayout = [{ arrayStride: 40, stepMode: "instance", attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
    { shaderLocation: 2, offset: 24, format: "float32x3" },
    { shaderLocation: 3, offset: 36, format: "float32" },
  ]}];
  const zapPipe = pipe("zap", S.ZAP, zapLayout,
    { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  // Where the AK's rounds have stopped: one card each, read straight off the
  // ring Rill keeps, like the tracers and the shards are.
  const markCap = Number(E.marks_cap());
  const markLayout = [{ arrayStride: 32, stepMode: "instance", attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
    { shaderLocation: 2, offset: 24, format: "float32" },
    { shaderLocation: 3, offset: 28, format: "float32" },
  ]}];
  const markPipe = pipe("mark", S.MARK, markLayout,
    { blend: { color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha" }, alpha: {} },
      cull: "none", depth: { depthWriteEnabled: false } });
  // The strike itself, off the same ring and the same buffer: a chip is what a
  // round leaves and a spark burst is the quarter second before it. Fourteen
  // sparks per strike, so the draw is fourteen quads deep per instance — see
  // `spark.wgsl`, which is where all of the arithmetic lives.
  const SPARKS_PER_MARK = 14;
  const sparkPipe = pipe("spark", S.SPARK, markLayout,
    { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  const markInst = device.createBuffer({ size: markCap * 32, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });

  // ---- the HUD ---------------------------------------------------------------
  //
  // The layout lives in the engine now — hud.rill emits a display list of
  // 16-float instances and this side does exactly two jobs: bake the page's
  // own monospace font into an atlas once, and draw whatever the wasm said.
  // The font stack is the same string the CSS carried, so the letterforms
  // did not change when the DOM went.
  const HUD_FONT = "ui-monospace, SFMono-Regular, Menlo, monospace";
  // Cells 0..94 are ASCII; then the four non-ASCII guests the HUD uses.
  const HUD_CHARS = [...Array.from({ length: 95 }, (_, i) => String.fromCharCode(32 + i)),
                     "·", "→", "†", "…"];
  const hudDpr = Math.min(devicePixelRatio || 1, 2);

  // One strip per (size, weight) the theme asks for. Each cell is the glyph
  // drawn white over transparent with the CSS text-shadow baked in: the red
  // channel is the letter, the alpha is letter plus halo, and hud.wgsl
  // separates them again. Cell 99 carries the strip's advance and padding.
  function bakeHudAtlas(stripDefs) {
    const pad = 3;
    const rows = stripDefs.map(([size, weight]) => {
      const c = document.createElement("canvas").getContext("2d");
      c.font = `${weight} ${size * hudDpr}px ${HUD_FONT}`;
      const adv = c.measureText("M").width / hudDpr;
      const cellW = Math.ceil((adv + pad * 2) * hudDpr);
      const cellH = Math.ceil((size * 1.35 + pad * 2) * hudDpr);
      return { size, weight, adv, cellW, cellH };
    });
    const texW = 2048;
    // Cells wrap into as many texture rows as a strip needs.
    let y = 0;
    for (const r of rows) {
      r.perRow = Math.max(1, Math.floor(texW / r.cellW));
      r.y0 = y;
      y += Math.ceil(HUD_CHARS.length / r.perRow) * r.cellH;
    }
    const texH = y;
    const cv = document.createElement("canvas");
    cv.width = texW; cv.height = texH;
    const ctx = cv.getContext("2d", { willReadFrequently: true });
    ctx.clearRect(0, 0, texW, texH);
    ctx.fillStyle = "#fff";
    ctx.textBaseline = "top";
    ctx.shadowColor = "rgba(0,0,0,0.95)";
    for (const r of rows) {
      ctx.font = `${r.weight} ${r.size * hudDpr}px ${HUD_FONT}`;
      ctx.shadowBlur = 2.5 * hudDpr;
      for (let i = 0; i < HUD_CHARS.length; i++) {
        const cx = (i % r.perRow) * r.cellW, cy = r.y0 + Math.floor(i / r.perRow) * r.cellH;
        ctx.fillText(HUD_CHARS[i], cx + pad * hudDpr, cy + pad * hudDpr);
      }
    }
    const tex = device.createTexture({
      label: "hud atlas", size: [texW, texH],
      format: "rgba8unorm", usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
    });
    device.queue.writeTexture({ texture: tex },
      ctx.getImageData(0, 0, texW, texH).data, { bytesPerRow: texW * 4 }, [texW, texH]);
    // The metrics hud.rill lays out by: per cell, texel origin and CSS-pixel
    // cell size; cell 99 is the strip's advance and pad.
    const metrics = new Float32Array(stripDefs.length * 100 * 4);
    rows.forEach((r, s) => {
      for (let i = 0; i < HUD_CHARS.length && i < 99; i++) {
        const at = (s * 100 + i) * 4;
        metrics[at] = (i % r.perRow) * r.cellW;
        metrics[at + 1] = r.y0 + Math.floor(i / r.perRow) * r.cellH;
        metrics[at + 2] = r.cellW / hudDpr;
        metrics[at + 3] = r.cellH / hudDpr;
      }
      const meta = (s * 100 + 99) * 4;
      metrics[meta] = r.adv;
      metrics[meta + 1] = pad;
    });
    return { tex, metrics, texW, texH };
  }

  const hudUni = device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
  const HUD_CAP = 4096;
  const hudInst = device.createBuffer({ size: HUD_CAP * 64, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
  const hudBGL = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: {} },
    { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: {} },
  ]});
  const hudLayout = [{ arrayStride: 64, stepMode: "instance", attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x4" },
    { shaderLocation: 1, offset: 16, format: "float32x4" },
    { shaderLocation: 2, offset: 32, format: "float32x4" },
    { shaderLocation: 3, offset: 48, format: "float32x4" },
  ]}];
  const hudPipe = watched("hud", () => device.createRenderPipeline({
    label: "hud",
    layout: device.createPipelineLayout({ bindGroupLayouts: [hudBGL] }),
    vertex: { module: mod(S.HUD, "hud"), buffers: hudLayout },
    fragment: { module: mod(S.HUD, "hud"),
                targets: [{ format, blend: overPre }] },
    primitive: { cullMode: "none" },
    depthStencil: { ...depth, depthWriteEnabled: false, depthCompare: "always" },
    multisample: MS,
  }));
  const hudSampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
  let hudAtlas = null, hudBG = null, hudOff = null;

  // Wired after every (re)join: the block is new, the atlas is not.
  function hudWire(st) {
    E.hud_init(st);
    if (!hudAtlas) {
      const defs = [];
      for (let i = 0; i < Number(E.hud_strips()); i++) {
        defs.push([E.hud_strip_size(st, BigInt(i)), E.hud_strip_weight(st, BigInt(i))]);
      }
      hudAtlas = bakeHudAtlas(defs);
      hudBG = device.createBindGroup({ layout: hudBGL, entries: [
        { binding: 0, resource: { buffer: hudUni } },
        { binding: 1, resource: hudAtlas.tex.createView() },
        { binding: 2, resource: hudSampler },
      ]});
      device.queue.writeBuffer(hudUni, 16, new Float32Array([hudAtlas.texW, hudAtlas.texH, 0, 0]));
    }
    hudOff = {
      base: Number(E.hud_ptr(st)),
      inst: Number(E.hud_inst_at()), meta: Number(E.hud_meta_at()),
      bytes: Number(E.hud_bytes_at()), slotB: Number(E.hud_slot_bytes()),
      met: Number(E.hud_met_at()),
    };
    new Float32Array(E.memory.buffer, hudOff.base + hudOff.met * 4, hudAtlas.metrics.length)
      .set(hudAtlas.metrics);
  }

  // The push side: text becomes bytes in a slot, colour switches become the
  // control codes hud.rill reads (a byte under 32 selects theme colour
  // byte - 1). `hudText` takes segments of [colourIndex, string].
  const hudEnc = new TextEncoder();
  function hudSlot(slot, segs, ...meta) {
    const view = new Uint8Array(E.memory.buffer, hudOff.base + hudOff.bytes + slot * hudOff.slotB, hudOff.slotB);
    let n = 0;
    for (const [color, text] of segs) {
      if (color >= 0 && n < hudOff.slotB) view[n++] = 1 + color;
      const b = hudEnc.encode(text);
      view.set(b.subarray(0, hudOff.slotB - n), n);
      n = Math.min(hudOff.slotB, n + b.length);
    }
    const m = new Float32Array(E.memory.buffer, hudOff.base + (hudOff.meta + slot * 8) * 4, 8);
    m[0] = n;
    for (let i = 0; i < meta.length && i < 7; i++) m[i + 1] = meta[i];
  }
  const hudClear = (slot) => {
    new Float32Array(E.memory.buffer, hudOff.base + (hudOff.meta + slot * 8) * 4, 1)[0] = 0;
  };
  const hudHead = (idx, v) => {
    new Float32Array(E.memory.buffer, hudOff.base + idx * 4, 1)[0] = v;
  };
  // Theme colour indices, hud.rill's own — for building coloured segments.
  const HC = { text: 0, gold: 1, sub: 2, dim: 3, bright: 4, green: 5, red: 6,
               orange: 7, white: 8, lav: 9 };
  // Header indices, hud.rill's own.
  const HH = { sw: 1, sh: 2, dpr: 3, now: 4, aim: 5, fps: 6, ping: 7, net: 8,
               warnAt: 9, tookAt: 10, crierAt: 11, over: 12, chat: 13,
               tookMine: 14, crierOut: 15, cycling: 16,
               tallyStand: 29, tallyBroke: 30, tallyAll: 31,
               rosterN: 32, boardN: 33, tagN: 34, cross: 36 };
  const HSLOT = { stat: 0, log: 8, roster: 20, board: 36, tag: 52,
                  warn: 68, took: 69, crier: 70, over1: 71, over2: 72, over3: 73 };

  const ZAP_MAX = 16;
  const zapGpu = device.createBuffer({ size: ZAP_MAX * 40, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
  const zapData = new Float32Array(ZAP_MAX * 10);
  const zapsLive = [];
  // Same treatment as the tracers: additive, depth-tested so the land hides
  // them, never writing depth of their own.
  const firePipe = pipe("fireball", S.FIREBALL, fireLayout,
    { blend: overPre, cull: "none", depth: { depthWriteEnabled: false } });
  const blastPipe = pipe("blast", S.BLAST, blastLayout,
    { blend: overPre, cull: "none", depth: { depthWriteEnabled: false } });
  // The stone is solid, so it blends rather than adds — and it goes down before
  // the flames, which then add over its edges and wrap round it.
  const meteorPipe = pipe("meteor", S.METEOR, fireLayout,
    { blend: { color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha" },
               alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" } },
      cull: "none", depth: { depthWriteEnabled: false } });
  // The fireballs take the same eight floats a bullet does, and the same
  // treatment: additive, depth-tested so the land hides them, never writing.

  // The flash sits in front of the rifle, so it is drawn after it and does not
  // write depth: it is fire, and fire occludes nothing.
  const shardPipe = pipe("shard", S.SHARD, [meshLayout, shardLayout]);
  // The props on the ground. The rifle's vertex layout, and the shards' own
  // instance buffer for the second slot — a magazine is a shard in Rill and
  // there is nothing to copy across.
  const propLayout = [
    { arrayStride: 16, attributes: [
      { shaderLocation: 0, offset: 0, format: "unorm16x4" },
      { shaderLocation: 1, offset: 8, format: "snorm8x4" },
      { shaderLocation: 2, offset: 12, format: "unorm16x2" },
    ]},
    // The shards' own buffer, read at different locations: the mesh has taken
    // 0, 1 and 2 for itself, so what the shard pipeline calls 2, 3 and 4 is 3,
    // 4 and 5 here. Same eighty bytes, same offsets — only the names differ.
    { arrayStride: 80, stepMode: "instance", attributes: [
      { shaderLocation: 3, offset: 0, format: "float32x3" },    // position
      { shaderLocation: 4, offset: 24, format: "float32x3" },   // orientation
      { shaderLocation: 5, offset: 48, format: "float32" },     // there or not
    ]},
  ];
  const propPipe = pipe("prop", S.PROP, propLayout);
  const cratePipe = pipe("crate", S.CRATE, propLayout);
  // The launcher in the hands: the rifle's vertex layout, its own frustum, and
  // no instances — there is only ever one pair of hands.
  const rpgPipe = pipe("launcher", S.LAUNCHER, vmLayout);
  // What is loaded in it, drawn through the launcher's own transform.
  const warheadPipe = pipe("warhead", S.WARHEAD, vmLayout);
  // And the rocket in the air, one instance per slot of the fireballs' ring.
  const rocketPipe = pipe("rocket", S.ROCKET, [
    { arrayStride: 16, attributes: [
      { shaderLocation: 0, offset: 0, format: "unorm16x4" },
      { shaderLocation: 1, offset: 8, format: "snorm8x4" },
      { shaderLocation: 2, offset: 12, format: "unorm16x2" },
    ]},
    { arrayStride: 36, stepMode: "instance", attributes: [
      { shaderLocation: 3, offset: 0, format: "float32x3" },    // where
      { shaderLocation: 4, offset: 12, format: "float32x3" },   // and how fast
      { shaderLocation: 5, offset: 24, format: "float32" },     // seed: sign says rocket
      { shaderLocation: 6, offset: 28, format: "float32" },     // life
    ]},
  ]);
  const muzzlePipe = pipe("muzzle", S.MUZZLE, [],
    { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });

  // The rifle: one file, its own header. Two counts, then the bounding box the
  // positions were quantized against, then the vertices, then the indices.
  const rifleBytes = await (await fetch(`./rifle.mesh?v=${V}`)).arrayBuffer();
  const rifleHead = new DataView(rifleBytes, 0, 32);
  const rifleVerts = rifleHead.getUint32(0, true);
  const rifleIdx = rifleHead.getUint32(4, true);
  const rifleBox = new Float32Array(rifleBytes, 8, 6);
  // The AK: GGBotNet's soviet assault rifle, CC0, baked from its OBJ into the
  // rifle's own sixteen-byte format with the barrel turned onto +x.
  const akBytes = await (await fetch(`./ak47.mesh?v=${V}`)).arrayBuffer();
  const akHead = new DataView(akBytes, 0, 32);
  const akVerts = akHead.getUint32(0, true);
  const akIdx = akHead.getUint32(4, true);
  const akBox = new Float32Array(akBytes, 8, 6);
  const akVB = gpuBuf(new Uint8Array(akBytes, 32, akVerts * 16));
  const akIdxBytes = new Uint8Array((akIdx * 2 + 3) & ~3);
  akIdxBytes.set(new Uint8Array(akBytes, 32 + akVerts * 16, akIdx * 2));
  const akIB = device.createBuffer({
    size: akIdxBytes.byteLength, usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
  });
  device.queue.writeBuffer(akIB, 0, akIdxBytes);
  step("ak47.mesh");
  const rifleVB = gpuBuf(new Uint8Array(rifleBytes, 32, rifleVerts * 16));
  // An odd triangle count leaves an odd number of uint16s, and `writeBuffer`
  // wants a multiple of four bytes either way, so the tail is padded.
  const rifleIdxBytes = new Uint8Array((rifleIdx * 2 + 3) & ~3);
  rifleIdxBytes.set(new Uint8Array(rifleBytes, 32 + rifleVerts * 16, rifleIdx * 2));
  const rifleIB = device.createBuffer({
    size: rifleIdxBytes.byteLength, usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
  });
  device.queue.writeBuffer(rifleIB, 0, rifleIdxBytes);
  step("rifle.mesh");

  // The ammunition box, in the rifle's own format and read by the same six
  // lines. Poly Haven's `ammo_box`, CC0, welded out of its five nodes by
  // `tools/bake_prop.py`: 3,978 vertices and 4,382 triangles in 88 KB.
  const propBytes = await (await fetch(`./ammo-box.mesh?v=${V}`)).arrayBuffer();
  const propHead = new DataView(propBytes, 0, 32);
  const propVerts = propHead.getUint32(0, true);
  const propIdx = propHead.getUint32(4, true);
  const propBox = new Float32Array(propBytes, 8, 6);
  const propVB = gpuBuf(new Uint8Array(propBytes, 32, propVerts * 16));
  const propIdxBytes = new Uint8Array((propIdx * 2 + 3) & ~3);
  propIdxBytes.set(new Uint8Array(propBytes, 32 + propVerts * 16, propIdx * 2));
  const propIB = device.createBuffer({
    size: propIdxBytes.byteLength, usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
  });
  device.queue.writeBuffer(propIB, 0, propIdxBytes);
  step("ammo-box.mesh");

  // The rocket crate: Poly Haven's `old_military_crate`, CC0, the `a` crate
  // of the pair welded by the same baker, recentred so its footprint sits on
  // the instance point. Same format, same six lines.
  const crateBytes = await (await fetch(`./rocket-crate.mesh?v=${V}`)).arrayBuffer();
  const crateHead = new DataView(crateBytes, 0, 32);
  const crateVerts = crateHead.getUint32(0, true);
  const crateIdx = crateHead.getUint32(4, true);
  const crateBox = new Float32Array(crateBytes, 8, 6);
  const crateVB = gpuBuf(new Uint8Array(crateBytes, 32, crateVerts * 16));
  const crateIdxBytes = new Uint8Array((crateIdx * 2 + 3) & ~3);
  crateIdxBytes.set(new Uint8Array(crateBytes, 32 + crateVerts * 16, crateIdx * 2));
  const crateIB = device.createBuffer({
    size: crateIdxBytes.byteLength, usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
  });
  device.queue.writeBuffer(crateIB, 0, crateIdxBytes);
  step("rocket-crate.mesh");

  // The launcher and its rocket. One file, one texture, two meshes, all in the
  // rifle's own format — so this is the same six lines a third and a fourth
  // time, which is what a format earns you.
  const oneMesh = async (url) => {
    const bytes = await (await fetch(`${url}?v=${V}`)).arrayBuffer();
    const head = new DataView(bytes, 0, 32);
    const verts = head.getUint32(0, true);
    const count = head.getUint32(4, true);
    const box = new Float32Array(bytes, 8, 6);
    const vb = gpuBuf(new Uint8Array(bytes, 32, verts * 16));
    const idxBytes = new Uint8Array((count * 2 + 3) & ~3);
    idxBytes.set(new Uint8Array(bytes, 32 + verts * 16, count * 2));
    const ib = device.createBuffer({
      size: idxBytes.byteLength, usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
    });
    device.queue.writeBuffer(ib, 0, idxBytes);
    step(url.slice(2));
    return { vb, ib, count, box };
  };
  const rpg = await oneMesh("./rpg7.mesh");
  const rocket = await oneMesh("./rocket.mesh");

  // The shark. Same shape of file as the rifle, one header longer: a magic,
  // the two counts, and the box the positions were quantized against.
  //
  // Twenty bytes a vertex — position in unorm16 across that box, normal in
  // snorm8, texture coordinate in snorm16, and a flat colour for the parts
  // the skin does not cover. 2,396 vertices and 2,816 triangles in 64 KB.
  const sharkBytes = await (await fetch(`./shark.mesh?v=${V}`)).arrayBuffer();
  const sharkHead = new DataView(sharkBytes, 0, 40);
  const sharkVerts = sharkHead.getUint32(4, true);
  const sharkIdx = sharkHead.getUint32(8, true);
  const sharkBox = new Float32Array(sharkBytes, 16, 6);
  const sharkVB = gpuBuf(new Uint8Array(sharkBytes, 40, sharkVerts * 20));
  const sharkIdxBytes = new Uint8Array((sharkIdx * 2 + 3) & ~3);
  sharkIdxBytes.set(new Uint8Array(sharkBytes, 40 + sharkVerts * 20, sharkIdx * 2));
  const sharkIB = device.createBuffer({
    size: sharkIdxBytes.byteLength, usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
  });
  device.queue.writeBuffer(sharkIB, 0, sharkIdxBytes);
  step("shark.mesh");

  // Somebody else, with a skeleton in them: Quaternius' mannequin off the
  // Universal Animation Library (CC0), baked by `tools/bake_figure.py` down to
  // the six clips this valley has any use for. Fifty-three joints, 13,744
  // triangles and 389 KB, of which the animation is a third: nothing is scaled
  // and only the hips translate, so a frame is a root position and a rotation
  // per joint in snorm16.
  const figBytes = await (await fetch(`./figure.mesh?v=${V}`)).arrayBuffer();
  const figure = readFigure(figBytes);
  const figVB = gpuBuf(new Uint8Array(figBytes, figure.vertexAt, figure.verts * 20));
  const figIdxBytes = new Uint8Array((figure.indices * 2 + 3) & ~3);
  figIdxBytes.set(new Uint8Array(figBytes, figure.indexAt, figure.indices * 2));
  const figIB = device.createBuffer({
    size: figIdxBytes.byteLength, usage: GPUBufferUsage.INDEX | GPUBufferUsage.COPY_DST,
  });
  device.queue.writeBuffer(figIB, 0, figIdxBytes);
  step("figure.mesh");

  // The bullets live in Rill's memory and are copied up whole every frame:
  // thirty-two rounds is a kilobyte, which is less than the call it would take
  // to work out whether any of them had moved.
  // The shot, recorded rather than synthesised: a Mosin Nagant, which is the
  // rifle in the hands, taken outdoors with its own echo off the hills already
  // in the tail. Decoded once and played from the same buffer every time, so
  // firing costs nothing but a node.
  //
  // The context is not started here — a browser will not let it be, before the
  // page has been clicked on — so this only fetches. `bang` builds it on the
  // first shot, which is by definition after a click.
  //
  // The shatter is three glass breaks laid end to end in one file, a second
  // each, played by offset. One fetch and one decode for three, and three is
  // enough that a row of crystals does not sound like a loop.
  let audio = null, shotBuf = null, breakBuf = null, blastSnd = null, musicBuf = null;
  let roarBuf = null, roars = null, pickBuf = null, hurtBuf = null, zapSndBuf = null, akBuf = null, akReloadBuf = null, rpgBuf = null, musicStarted = false;
  // Off to begin with. A valley somebody has just walked into should sound
  // like a valley; the theme is something you put on, not something that is
  // put on you. `M` turns it up, and the HUD has said so all along.
  let musicGain = null, musicOn = false;
  const shotBytes = await (await fetch(`./rifle-shot.mp3?v=${V}`)).arrayBuffer();
  step("rifle-shot.mp3");
  const breakBytes = await (await fetch(`./crystal-break.mp3?v=${V}`)).arrayBuffer();
  step("crystal-break.mp3");
  // Two explosions, 1.9 seconds each, laid end to end and played by offset.
  const blastBytes = await (await fetch(`./meteor-blast.mp3?v=${V}`)).arrayBuffer();
  step("meteor-blast.mp3");
  // What something falling through the sky sounds like from underneath.
  //
  // Not a recording of a fire. That was the first attempt at this and it was
  // wrong twice over: a fireplace filtered down to its rumble is inaudible on a
  // laptop — measured 60 dB down in the band a small speaker can move air in —
  // and the part of it that does come through is crackle, which is the sound of
  // wood splitting. Nothing is splitting up there.
  //
  // A body going through the air at speed makes turbulence, and turbulence is
  // broadband noise: a roar with a low body and a tearing edge, breathing as
  // the flow round it changes. So this is built rather than found — three bands
  // of noise, each shaped and each swelling on its own slow clock, so the
  // timbre wanders instead of sitting still the way one static band of hiss
  // does.
  //
  // Built in the frequency domain, which is what makes it loop: every component
  // is a whole number of cycles per four seconds, so the waveform is periodic
  // by construction and the join is a step of 4046 against a mean sample-to-
  // sample step of 2437 — indistinguishable from the noise's own motion, with
  // no crossfade needed anywhere.
  const roarBytes = await (await fetch(`./meteor-air.mp3?v=${V}`)).arrayBuffer();
  step("meteor-air.mp3");
  // A chip of crystal going in. Three quarters of a second off the front of a
  // CC0 harp figure by Julie Damsgaard — the rest of it is a two-second tail,
  // which is a lovely thing to have and the wrong shape entirely for a sound
  // that fires four times while you cross a patch of debris.
  const pickBytes = await (await fetch(`./pickup.mp3?v=${V}`)).arrayBuffer();
  step("pickup.mp3");
  // What it sounds like to be shot. Four of qubodup's CC0 recordings of
  // himself straining, a second each end to end and played by offset like the
  // glass — four is enough that a burst does not sound like a loop of one man.
  const hurtBytes = await (await fetch(`./hurt.mp3?v=${V}`)).arrayBuffer();
  step("hurt.mp3");
  // The theme. Eighty seconds that begin and end in silence, which is what
  // makes it loop without a seam: no crossfade, no loop points, just the tail
  // running into the head the way the piece was already written.
  const musicBytes = await (await fetch(`./valley-theme.mp3?v=${V}`)).arrayBuffer();
  step("valley-theme.mp3");
  // A crystal's bite. Kenney's digital SFX set by way of OpenGameArt, CC0 —
  // the same shelf half this valley's sounds came off.
  const zapBytes = await (await fetch(`./crystal-zap.mp3?v=${V}`)).arrayBuffer();
  step("crystal-zap.mp3");
  // Four real AK-47 reports, a second each end to end and played by offset
  // like the glass and the hurt — at five rounds a second one sample on
  // repeat is a machine gun made of a single click, and four is enough that a
  // magazine does not sound like a loop.
  //
  // Cut from the Free Firearm Sound Library (CC0), from the take that holds
  // four singles rather than a burst, and taken off the near channel of the
  // pair. The cut this replaces was the same library and sounded like a
  // cardboard box: everything below 300 Hz had been filtered out of it —
  // measured at 0.3% of the energy against 33% here — so what was left was a
  // mid-range pop with no chest in it and no crack on top. It was also
  // running 5 dB below the rifle it stands next to.
  //
  // The transient sits 7 ms into each slot and the hills have finished
  // answering by 600, so a slot is a second and the offset is just the index.
  const akSndBytes = await (await fetch(`./ak-shot.m4a?v=${V}`)).arrayBuffer();
  step("ak-shot.m4a");
  // The magazine change: SpringySpringo's assault rifle reload, CC0. A second
  // and a half of it, which is the pause it is played over.
  const akReloadBytes = await (await fetch(`./ak-reload.m4a?v=${V}`)).arrayBuffer();
  step("ak-reload.m4a");
  // The launcher going off. A rocket motor lighting, from OpenGameArt's CC0
  // `rocket-launch`, cut to the ignition and given a decay: what the pack
  // holds is four seconds of sustained burn, which is a rocket leaving a pad
  // rather than a tube leaving a shoulder.
  const rpgSndBytes = await (await fetch(`./rpg-launch.m4a?v=${V}`)).arrayBuffer();
  step("rpg-launch.m4a");
  // One node per sound, built and thrown away: cheaper than it looks, and the
  // only way to have two of the same sample overlapping.
  function playAt(buf, offset, dur, gain, pan, rate) {
    if (!buf) return;
    const src = audio.createBufferSource();
    src.buffer = buf;
    // A hair of detune and level each time: the same file twice in a row is
    // the one thing that gives a sample away.
    src.playbackRate.value = rate;
    const g = audio.createGain();
    g.gain.value = gain;
    const p = audio.createStereoPanner();
    p.pan.value = pan;
    src.connect(g).connect(p).connect(audio.destination);
    if (dur > 0) { src.start(0, offset, dur); } else { src.start(); }
  }
  // Everything decoded while the bar is still on screen, not on the first
  // click. A browser will not let audio *play* before a gesture, but it will
  // decode: the context is made here and left suspended, and six files turn
  // into six buffers during the load rather than in the first second of play.
  //
  // That is worth a step each on the bar. Decoding is the slow half of getting
  // a sound ready — the theme is eighty seconds of mp3 — and doing it on the
  // click meant the click that started the game also stalled it, with the
  // loading screen already gone and nothing to say why.
  async function decodeAll(step) {
    audio = new (globalThis.AudioContext ?? globalThis.webkitAudioContext)();
    musicGain = audio.createGain();
    musicGain.gain.value = 0;
    musicGain.connect(audio.destination);
    const one = async (bytes, label) => {
      const b = await audio.decodeAudioData(bytes.slice(0));
      step(label);
      return b;
    };
    [shotBuf, breakBuf, blastSnd, roarBuf, pickBuf, hurtBuf, musicBuf, zapSndBuf, akBuf, akReloadBuf, rpgBuf] = await Promise.all([
      one(shotBytes, "decoding rifle-shot"),
      one(breakBytes, "decoding crystal-break"),
      one(blastBytes, "decoding meteor-blast"),
      one(roarBytes, "decoding meteor-air"),
      one(pickBytes, "decoding pickup"),
      one(hurtBytes, "decoding hurt"),
      one(musicBytes, "decoding valley-theme"),
      one(zapBytes, "decoding crystal-zap"),
      one(akSndBytes, "decoding ak-shot"),
      one(akReloadBytes, "decoding ak-reload"),
      one(rpgSndBytes, "decoding rpg-launch"),
    ]);
  }

  // The gesture only has to wake it up now, and start the one thing that runs
  // for the life of the page. The theme is no longer among them: it begins
  // when somebody asks for it, which by default is never — see `setMusic`.
  function ensureAudio(then) {
    if (!audio) { then(); return; }
    if (audio.state === "suspended") audio.resume();
    if (!roars && roarBuf) startRoars();
    if (musicOn && !musicStarted && musicBuf) { musicStarted = true; startMusic(); }
    then();
  }

  // Three voices, started once and left running for the life of the page, each
  // following one fireball. Not one voice per round — twenty-four looping
  // sources for something you can only ever hear three of is a waste, and the
  // fourth-nearest is inaudible under the other three anyway. Each frame the
  // rounds in the air are sorted by distance and the nearest three are handed
  // to the voices; a voice with nothing to follow is turned down rather than
  // stopped, because starting and stopping a loop is what clicks.
  // Where the roar gives out entirely — twenty-eight metres, a fraction of the
  // seventy a crystal has to stand off to throw at all. A round is silent for
  // almost its whole flight and only speaks up in the last two seconds, coming
  // in. That is the point of it: three of these in the air at forty metres is a
  // wash of noise with nothing to attach it to, and it makes the valley sound
  // like it is on fire rather than like something is coming at you.
  //
  // A hard edge and a steep curve do different jobs here. The edge is what
  // stops the rounds scattered across the valley adding up to a permanent
  // hiss; the curve — cubed, so nine tenths of the loudness is spent in the
  // last ten metres — is what makes the approach itself audible.
  //
  // The explosions are the opposite case and are tuned the opposite way: a
  // half-second impact carries across a valley and should, so `boom` rolls off
  // over a hundred and ten metres. Continuous and distant is noise; loud and
  // brief and distant is an event.
  const REACH = 28.0;
  function startRoars() {
    roars = [0, 1, 2].map(() => {
      const src = audio.createBufferSource();
      src.buffer = roarBuf;
      src.loop = true;
      // Looping the inside of the buffer rather than the whole of it. MP3 is
      // not sample-accurate: the encoder puts a short silence at the front and
      // pads the end, and a roar that is meant to be continuous would drop out
      // for a few dozen milliseconds every time round. Skipping past both means
      // the loop no longer lands on the period exactly — which for noise costs
      // nothing, because the two ends were never going to line up audibly and
      // the whole point of the construction is that they do not have to.
      src.loopStart = 0.06;
      src.loopEnd = roarBuf.duration - 0.06;
      const filt = audio.createBiquadFilter();
      filt.type = "lowpass";
      filt.frequency.value = 700;
      const gain = audio.createGain();
      gain.gain.value = 0;
      const pan = audio.createStereoPanner();
      src.connect(filt).connect(gain).connect(pan).connect(audio.destination);
      // Each at its own speed, or three rounds in the air beat against each
      // other and the whole sky sounds like one engine.
      src.playbackRate.value = 0.88 + Math.random() * 0.3;
      src.start();
      return { gain, pan, filt };
    });
  }

  // Started once, when it is first asked for, and left running after that.
  // Fading it up over four seconds rather than cutting it in matters more than
  // it sounds like it should: the theme opens on a held chord, and a held
  // chord that begins at full volume the instant you press the key reads as a
  // jingle rather than as somewhere you have arrived.
  function startMusic() {
    const src = audio.createBufferSource();
    src.buffer = musicBuf;
    src.loop = true;
    src.connect(musicGain);
    src.start();
  }
  function setMusic(on) {
    musicOn = on;
    if (!musicGain) return;
    // The piece begins here rather than at the first click, and that is the
    // whole of what starting silent costs to do properly. Leaving the source
    // running at nought gain from the beginning would have been one line
    // fewer and would have meant that turning the music on ten minutes in
    // dropped you into the middle of a bar. Started on the key, M always
    // opens on the opening chord — which is what the four seconds below are
    // there to carry.
    if (on && !musicStarted && musicBuf) { musicStarted = true; startMusic(); }
    const now = audio.currentTime;
    musicGain.gain.cancelScheduledValues(now);
    musicGain.gain.setValueAtTime(musicGain.gain.value, now);
    musicGain.gain.linearRampToValueAtTime(on ? 0.17 : 0.0, now + (on ? 4.0 : 0.6));
  }
  function bang() {
    ensureAudio(() => playAt(shotBuf, 0, 0, 0.55 + Math.random() * 0.12, 0,
                             0.97 + Math.random() * 0.06));
  }
  // The AK's report, once per round leaving. A different one of the four each
  // time, on top of the detune the rest of the guns get.
  function akBang() {
    ensureAudio(() => playAt(akBuf, Math.floor(Math.random() * 4), 0.95,
                             0.42 + Math.random() * 0.08, 0,
                             0.95 + Math.random() * 0.1));
  }
  // The magazine change. Played a shade fast — the clip is a second and a
  // half and the pause is 1.4 — so the click of the magazine seating lands on
  // the frame the gun is ready again rather than a breath after it.
  // The launcher. Heavier and slower than either gun, and it does not want
  // the rifle's crack: what leaves the tube is a motor lighting.
  function rpgBang() {
    ensureAudio(() => playAt(rpgBuf, 0, 0, 0.62 + Math.random() * 0.08, 0,
                             0.97 + Math.random() * 0.06));
  }
  function akReload() {
    ensureAudio(() => playAt(akReloadBuf, 0, 0, 0.5, 0, 1.556 / 1.4));
  }
  // A hammer falling on nothing. Not a sample: it is thirty milliseconds of
  // noise with no tail, and a file for that would be mostly silence and one
  // more thing to fetch. Quiet, because it is the sound of an absence — loud
  // enough to say the trigger was heard and not loud enough to be mistaken for
  // something happening.
  function dryClick() {
    ensureAudio(() => {
      const n = Math.floor(audio.sampleRate * 0.03);
      const buf = audio.createBuffer(1, n, audio.sampleRate);
      const d = buf.getChannelData(0);
      for (let i = 0; i < n; i++) d[i] = (Math.random() * 2 - 1) * Math.pow(1 - i / n, 9);
      const src = audio.createBufferSource();
      src.buffer = buf;
      const bp = audio.createBiquadFilter();
      bp.type = "bandpass";
      bp.frequency.value = 2200;
      bp.Q.value = 1.4;
      const g = audio.createGain();
      g.gain.value = 0.22;
      src.connect(bp).connect(g).connect(audio.destination);
      src.start();
    });
  }
  // Where a sound is, from the ear's point of view: how loud, and how far round
  // to the side. `near` is the distance at which it is half as loud, which is
  // the only number that distinguishes a crystal breaking at your feet from a
  // meteor coming down across the valley.
  function place(x, y, z, near) {
    const st = stateF32();
    const dx = x - st[16], dy = y - st[17], dz = z - st[18];
    const d = Math.hypot(dx, dy, dz);
    const yaw = st[19];
    const pan = Math.max(-1, Math.min(1,
      (Math.cos(yaw) * dx + Math.sin(yaw) * dz) / Math.max(d, 0.001)));
    return { d, gain: near / (near + d), pan: pan * 0.8 };
  }
  // A crystal going. Quieter with distance and set where it happened: which
  // one broke is worth hearing when there are three of them in front of you.
  function shatter(x, y, z) {
    ensureAudio(() => {
      const p = place(x, y, z, 12);
      playAt(breakBuf, Math.floor(Math.random() * 3), 0.95, 0.75 * p.gain, p.pan,
             0.94 + Math.random() * 0.14);
    });
  }
  // Picking one up. This is called from the health going up, which is a test
  // made every frame, so walking over a fresh break — where the chips come in
  // one after another for as long as it takes to cross them — used to play a
  // note *per frame*. Sixty a second is not a run, it is a buzz, and it was
  // worst exactly where there was most to pick up.
  //
  // The answer is a count and not a rate. Two chips are two notes, a semitone
  // apart, which is the little rising figure a pickup should be; past two the
  // chips still count, they simply stop speaking. Slowing the notes down was
  // tried first and it is the wrong lever — a heap spread thinner is still a
  // heap of notes, and what there is too much of is the number of them.
  //
  // A run ends after three quarters of a second with nothing picked up, and
  // not before: `chimeAt` is pushed forward on every chip whether it is
  // sounded or not, so crossing a long trail is one run of two notes rather
  // than two notes every time the trail thins for a frame.
  const CHIME_MOST = 2;
  let chimeStep = 0, chimeAt = -9, chimeSaid = 0;
  function chime(now) {
    ensureAudio(() => {
      const fresh = now - chimeAt >= 0.75;
      chimeAt = now;
      if (fresh) { chimeStep = 0; chimeSaid = 0; }
      else if (chimeSaid >= CHIME_MOST) return;
      else chimeStep = Math.min(chimeStep + 1, 7);
      chimeSaid++;
      // Half the amplitude it had, which is six decibels down. It was sitting
      // at 0.46, level with the AK's own report — a great deal of sound for a
      // chip of crystal going in, and the one sound in the valley that fires
      // while you are doing something else. It is a note under the walking
      // now rather than an announcement over it.
      playAt(pickBuf, 0, 0, 0.23, 0, Math.pow(2, chimeStep / 12));
    });
  }

  // How far an impact carries, and nothing past it.
  //
  // Thirty-two metres, which is close. Measured over half a minute of play the
  // impacts land between one and forty-four metres out — they are aimed at you,
  // so they all land somewhere near — and at fifty-eight every single one was
  // audible. That is a continuous distant bombardment with nothing to look at,
  // and it flattens the ones that matter. Cut to thirty-two, about half of them
  // go quiet and the near ones land.
  //
  // Still wider than the roar's, because a bang does travel further than a
  // rush of air.
  const BOOM_REACH = 32.0;
  // A fireball landing. Squared falloff inside the cutoff, so nearly all of the
  // loudness is in the last twenty metres — and slower and deeper the further
  // off it is, which is most of what makes a big noise sound far away.
  function boom(x, y, z) {
    ensureAudio(() => {
      const p = place(x, y, z, 40);
      const k = Math.max(0, 1 - p.d / BOOM_REACH);
      // Not worth a node: past the cutoff there is nothing to hear, and an
      // explosion a hundred metres off would otherwise still allocate a source,
      // a panner and a gain to play silence for two seconds.
      if (k <= 0.02) return;
      const far = Math.min(p.d / BOOM_REACH, 1);
      playAt(blastSnd, Math.floor(Math.random() * 2) * 1.9, 1.85,
             0.95 * k * k, p.pan, 1.06 - 0.22 * far);
    });
  }

  const bulletCap = Number(E.bullets_cap(0));
  const bulletBuf = device.createBuffer({
    size: bulletCap * 32, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });
  const fireCap = Number(E.fireballs_cap(0));
  const fireInst = device.createBuffer({
    size: fireCap * 36, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });
  const blastCap = Number(E.blasts_cap(0));
  const blastInst = device.createBuffer({
    size: blastCap * 32, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });
  const shardCap = Number(E.shards_cap(0));
  // How many of those slots are ammunition rather than crystal. The world
  // decides; the renderer only needs to know where the line is.
  const magCount = Number(E.mag_count());
  const magsOnly = Number(E.mags_only());
  const shardBuf = device.createBuffer({
    size: shardCap * 80, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });

  const flyBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: { type: "read-only-storage" } },
  ]});
  const flyDrawPipe = watched("fireflies", () => device.createRenderPipeline({
    label: "fireflies",
    layout: device.createPipelineLayout({ bindGroupLayouts: [bglMain, flyBgl] }),
    vertex: { module: mod(S.FIREFLY_DRAW, "fireflies") },
    fragment: { module: mod(S.FIREFLY_DRAW, "fireflies"), targets: [{ format, blend: additive }] },
    primitive: { cullMode: "none" },
    depthStencil: { ...depth, depthWriteEnabled: false },
    multisample: MS,
  }));
  const simBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: "storage" } },
    { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: {} },
  ]});
  const simPipe = watched("fly-sim", () => device.createComputePipeline({
    label: "fly-sim",
    layout: device.createPipelineLayout({ bindGroupLayouts: [simBgl] }),
    compute: { module: mod(S.FIREFLY_SIM, "fly-sim") },
  }));

  const shadowLayout = device.createPipelineLayout({ bindGroupLayouts: [bglShadow] });
  const shadowPipe = (label, code, buffers, cutout = false) => watched(label, () => device.createRenderPipeline({
    label, layout: shadowLayout,
    vertex: { module: mod(code, label), buffers },
    // Depth only, unless the thing being drawn is mostly holes.
    fragment: cutout
      ? { module: mod(code, label), entryPoint: "fs", targets: [] }
      : undefined,
    primitive: { cullMode: "none" },
    depthStencil: { format: "depth32float", depthWriteEnabled: true, depthCompare: "less" },
  }));
  const shTerrain = shadowPipe("shadow-terrain", S.SHADOW_TERRAIN, terrainLayout);
  const shInst = shadowPipe("shadow-inst", S.SHADOW_INST, [meshLayout, instLayout]);
  const shPine = shadowPipe("shadow-pine", S.SHADOW_PINE, [pineLayout, instLayout], true);

  // The people. Twenty bytes a vertex — position quantized across the model's
  // own box, normal and material, four joints and four weights — and a second
  // buffer, per instance, for nothing but the colour: where each of them is
  // standing is already inside the matrices.
  const figLayout = { arrayStride: 20, attributes: [
    { shaderLocation: 0, offset: 0, format: "unorm16x4" },
    { shaderLocation: 1, offset: 8, format: "snorm8x4" },
    { shaderLocation: 2, offset: 12, format: "uint8x4" },
    { shaderLocation: 3, offset: 16, format: "unorm8x4" },
  ]};
  const figInstLayout = { arrayStride: 32, stepMode: "instance", attributes: [
    { shaderLocation: 4, offset: 20, format: "float32x3" },
  ]};
  const figConst = {
    BOX_MIN_X: figure.box[0], BOX_MIN_Y: figure.box[1], BOX_MIN_Z: figure.box[2],
    BOX_MAX_X: figure.box[3], BOX_MAX_Y: figure.box[4], BOX_MAX_Z: figure.box[5],
    JOINTS: figure.joints,
  };
  const poseBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: { type: "read-only-storage" } },
  ]});
  const figurePipe = watched("figure", () => device.createRenderPipeline({
    label: "figure",
    layout: device.createPipelineLayout({ bindGroupLayouts: [bglMain, poseBgl] }),
    vertex: { module: mod(S.FIGURE, "figure", figConst), buffers: [figLayout, figInstLayout] },
    fragment: { module: mod(S.FIGURE, "figure", figConst), targets: [{ format }] },
    primitive: { cullMode: "back" },
    depthStencil: depth,
    multisample: MS,
  }));
  const shFigure = watched("shadow-figure", () => device.createRenderPipeline({
    label: "shadow-figure",
    layout: device.createPipelineLayout({ bindGroupLayouts: [bglShadow, poseBgl] }),
    vertex: { module: mod(S.SHADOW_FIGURE, "shadow-figure", figConst), buffers: [figLayout] },
    primitive: { cullMode: "back" },
    depthStencil: { format: "depth32float", depthWriteEnabled: true, depthCompare: "less" },
  }));

  // And the rifle each of them is carrying: one matrix apiece, the hand's own
  // world matrix times the grip. Drawn from the same file the viewmodel uses,
  // so there is one rifle in this valley and not two.
  const heldPipe = watched("held", () => device.createRenderPipeline({
    label: "held",
    layout: device.createPipelineLayout({ bindGroupLayouts: [bglMain, poseBgl] }),
    vertex: { module: mod(S.HELD, "held"), buffers: vmLayout },
    fragment: { module: mod(S.HELD, "held"), targets: [{ format }] },
    primitive: { cullMode: "none" },
    depthStencil: depth,
    multisample: MS,
  }));

  // One matrix per joint per person, rebuilt on the page every frame and
  // handed over whole. Thirty-two people is a hundred and eight kilobytes,
  // which is a sixth of what the terrain is.
  const FIGURE_CAP = 32;
  const skinner = makeSkinner(figure, FIGURE_CAP);
  const poseBuf = device.createBuffer({
    size: FIGURE_CAP * figure.joints * 64,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
  });
  const poseBg = device.createBindGroup({
    layout: poseBgl, entries: [{ binding: 0, resource: { buffer: poseBuf } }],
  });
  const heldBuf = device.createBuffer({
    // One more than there are people: the spare is this player's own, on the
    // ground, once they have been knocked off their feet.
    size: (FIGURE_CAP + 1) * 64, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
  });
  const heldBg = device.createBindGroup({
    layout: poseBgl, entries: [{ binding: 0, resource: { buffer: heldBuf } }],
  });
  // What a frame costs the GPU, when the adapter will say. Two timestamps
  // around the passes, resolved into a buffer and read back a frame later so
  // that nothing waits on it.
  let qs = null, qResolve = null, qStage = null, qPending = false, gpuMs = 0;
  if (canTime) {
    qs = device.createQuerySet({ type: "timestamp", count: 2 });
    qResolve = device.createBuffer({ size: 16, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
    qStage = device.createBuffer({ size: 16, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
  }

  // Nothing per instance but the colour; where each of them is standing is
  // already inside the matrices. The record keeps the old eight-float shape so
  // that one stride serves both this and anything else instanced.
  const figureInst = device.createBuffer({
    size: FIGURE_CAP * 32, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });
  const figures = new Float32Array(FIGURE_CAP * 8);
  let figureCount = 0;
  // Rifles to draw. One per person, plus one on the ground for this player
  // when they are down and have no figure to hang it off.
  let heldCount = 0;

  // Where each person is in their animation, and what they were doing last
  // time. Kept here rather than sent, because a clock is exactly the sort of
  // thing that should not go over a network: everybody's legs can be at a
  // different point in the same walk without anybody disagreeing about
  // anything that matters.
  const clocks = new Map();
  // What the server says somebody is doing, as a clip.
  //
  // Nobody in this valley walks. Moving without the sprint key is seven and a
  // half metres a second, which is faster than anybody has ever run, and the
  // walk clip under it made the legs stroll while the body flew — the thing
  // that reads as broken, more than any amount of jitter. So the run clip
  // covers both, at two rates.
  const CLIP_FOR = ["idle", "run", "run", "crouch", "die"];

  // How fast to play it, which is not one.
  //
  // The clip is a run at about four metres a second — two steps in seven tenths
  // of a second, which is what a person does. This valley moves at five and a
  // half, and at twelve with the sprint key, so played at its own speed the
  // feet skate: the body covers more ground than the legs are asking for.
  //
  // Cadence goes about as the square root of speed — most of the rest is a
  // longer stride, which is why a sprinter does not pedal twice as fast as a
  // jogger. √(5.625/4) and √(12/4) are these two numbers, and they put the
  // feet near enough under the body at both speeds.
  const CLIP_RATE = [1, 1.19, 1.73, 1, 1];
  // The clips that only have legs worth keeping. Quaternius' library has a
  // walk, a run and a crouch, and an armed idle, and no armed anything else —
  // so in these three the arms swing like the empty arms they were animated
  // as, and a rifle swung with them. The upper body comes from the armed idle
  // instead, from the spine up; see `pose`. Firing, being hit and going down
  // are whole-body and are left alone, and so is the idle, which is where the
  // upper body is coming from.
  const LEGS_ONLY = new Set(["walk", "run", "crouch"]);

  // Firing and being hit are events rather than states, so they play over
  // whatever the legs are doing for as long as their clip lasts.
  //
  // A hit is noticed rather than announced: health only ever goes down for one
  // reason at a time and the snapshot carries it, so a drop since the last one
  // is somebody having just been shot — which also covers the shark and a
  // crater without either needing to say anything. The cost is that it arrives
  // a tick late, which is the same tick the health bar moves on.
  function clipFor(p, now) {
    const c = clocks.get(p.id) ?? { name: "idle", since: now, at: 0, seen: now, firedAt: -9, hitAt: -9, health: p.health };
    if (p.health < c.health - 0.001 && p.health > 0.001) c.hitAt = now;
    // And coming apart, on the one tick their health reaches zero. The pieces
    // are the world's own — the same ones a crystal breaks into — so nothing
    // is sent for this and nothing new is drawn: the snapshot is where a death
    // is learned, and this is the moment it is learned on.
    if (p.health < 0.001 && c.health >= 0.001 && world) {
      E.gib_at(world, p.x, p.y, p.z, p.yaw, BigInt(p.id));
    }
    c.health = p.health;
    const shooting = now - c.firedAt < figure.clips.get("fire").seconds * 1000;
    const hurt = now - c.hitAt < figure.clips.get("hit").seconds * 1000;
    // Down first: somebody on the ground is not flinching or shooting.
    const want = p.health < 0.001 ? "die" : hurt ? "hit" : shooting ? "fire" : (CLIP_FOR[p.pose] ?? "idle");
    // How far into the clip, kept as a running total rather than worked out
    // from when it started. The two agree until the rate changes, and then the
    // subtraction jumps: somebody who has been running for ten seconds and
    // presses sprint would be thrown four and a half seconds down the clip
    // mid-stride. Adding up the frames cannot do that.
    //
    // The step is capped, because a tab that was in the background comes back
    // with a gap of minutes in its hand.
    const step = Math.min((now - c.seen) / 1000, 0.1);
    c.seen = now;
    const rate = want === "run" ? (CLIP_RATE[p.pose] ?? 1) : 1;
    if (want !== c.name) { c.name = want; c.since = now; c.at = 0; } else c.at += step * rate;
    clocks.set(p.id, c);
    return c;
  }

  const pineData = pineMesh(false), crystalData = crystalMesh(), grassData = grassMesh();
  const pineBuf = gpuBuf(pineData), crystalBuf = gpuBuf(crystalData), grassBuf = gpuBuf(grassData);
  const pineVerts = pineData.length / 8, crystalVerts = crystalData.length / 6, grassVerts = grassData.length / 8;
  // The same tree with a fifth of the cards, for casting only.
  const blockData = blockMesh();
  const blockBuf = gpuBuf(blockData);
  const blockVerts = blockData.length / 6;
  const pineShadowData = pineMesh(true);
  const pineShadowBuf = gpuBuf(pineShadowData);
  const pineShadowVerts = pineShadowData.length / 8;

  // ---- being told, rather than deciding ------------------------------------
  //
  // Everything in this block is only reached with a server. Without one it is
  // dead code and the valley below runs exactly as it did.

  // The snapshot this player's own position was last squared with, so that a
  // twentieth of a second's answer is not applied sixty times.
  let appliedSnap = null;

  // What has happened since the last frame, in the order it happened. Each of
  // these is the same write the world made where it happened: a round entering
  // the ring, a crystal coming apart, a crater. The sounds need no help —
  // `blasts_fired` and `breaks` are counters the frame already watches, and
  // these push them along exactly as the real thing does.
  function applyEvents(evs) {
    for (const e of evs) {
      const n = e.n;
      if (e.kind === "gone") { net.gone = e.which; for (const i of e.which) E.put_gone(world, BigInt(i)); }
      // A round of this player's own is already in the air: it was fired here
      // the moment the mouse went down, because a tracer that waits for the
      // server is a rifle that fires late. This is the same round coming back.
      else if (e.kind === "r") {
        if (n[6] !== net.id) E.put_bullet(world, n[0], n[1], n[2], n[3], n[4], n[5], n[6]);
        // Whoever fired it plays the shot, this player included: their own
        // round never comes back, but the tag over their head is not the only
        // thing other people see of them.
        const c = clocks.get(n[6]);
        if (c) c.firedAt = performance.now();
      }
      else if (e.kind === "t") E.put_fireball(world, n[0], n[1], n[2], n[3], n[4], n[5], n[6], n[7]);
      else if (e.kind === "k") E.put_blast(world, n[0], n[1], n[2], n[3]);
      else if (e.kind === "b") E.put_break(world, BigInt(n[0]));
      // A stone off a wall. The blast that did it arrives as its own `k`; this
      // says which block went, because nothing on this side decided it.
      else if (e.kind === "w") E.drop_block(world, BigInt(n[0]));
      // A meteor somebody shot out of the air. Nothing on this side decided
      // it, so the pieces are spawned from the same call the shooter's world
      // made.
      else if (e.kind === "d") E.down_fire(world, BigInt(n[0]));
      // A pine a warhead took. The instance buffer goes up with every frame,
      // so applying it to the world is the whole of the work.
      else if (e.kind === "y") E.put_fell(world, BigInt(n[0]));
      // And every pine already down, one frame at the door.
      else if (e.kind === "pines") for (const i of e.n) E.put_fell(world, BigInt(i));
      // A crystal bit somebody. Applying it moves the ring and the counter,
      // and the watcher in the frame loop draws the light and plays the sound
      // from there — the same road a bite this world decided takes.
      else if (e.kind === "z") E.put_zap(world, BigInt(n[0]), BigInt(n[1]));
      // Somebody said something. Nothing in the valley moves because of it;
      // it goes on the screen and that is all it does.
      else if (e.kind === "say") sayLine(net.names.get(e.who) ?? "somebody", e.text);
      // The wall's whole history, one line at the door: every stone broken
      // before this client arrived. Each lands exactly where it did for
      // everybody else, because the resting place is computed from the index.
      else if (e.kind === "wall") for (const i of e.n) E.drop_block(world, BigInt(i));
      // A box of ammunition is somewhere else. What arrives is how many times
      // box `i` has been taken, not where it now is: where that puts it is the
      // valley's own arithmetic, run identically here and on the server, so
      // the wire carries a count rather than a position. Said as a total and
      // not a step, which is what makes it safe to hear twice — and this
      // player hears their own pickup come back exactly once too many.
      else if (e.kind === "m") E.set_mag(world, BigInt(n[0]), BigInt(n[1]));
      // A round found somebody. Everybody is told, and the two people it
      // concerns each get their own sentence: one of them has been shot and
      // the other has hit something, which are not the same news.
      else if (e.kind === "x" && (n[0] === net.id || n[1] === net.id)) {
        const mine = n[1] === net.id;
        const them = net.names.get(mine ? n[0] : n[1]) ?? "somebody";
        const much = Math.round(n[2] * 100);
        if (!mine) {
          // Out loud as well, and only for the one being shot. Louder for a
          // bigger hit and a hair of detune each time, because the same file
          // twice in a row is what gives a sample away.
          if (audio && hurtBuf) {
            playAt(hurtBuf, Math.floor(Math.random() * 4), 0.95,
                   0.55 + 0.45 * Math.min(1, n[2] * 2), 0, 0.94 + Math.random() * 0.12);
          }
        }
        // A kill is not something the shooter could work out: the snapshot
        // that says somebody is down says nothing about who put them there,
        // so the server says it here. The engine pops and fades it on the
        // timestamps; this side only says the words.
        hudSlot(HSLOT.took, mine
          ? (n[3]
              ? [[-1, "† you killed "], [HC.gold, them]]
              : [[-1, "you hit "], [HC.gold, them], [-1, " for "], [HC.white, much]])
          : [[HC.gold, them], [-1, " hit you for "], [HC.white, much]]);
        hudHead(HH.tookAt, performance.now() / 1000);
        hudHead(HH.tookMine, mine ? 1 : 0);
        // And into the log, from whoever's point of view it is not.
        const shooter = mine ? "you" : (net.names.get(n[1]) ?? "somebody");
        const hitWho = mine ? them : "you";
        note(n[3]
          ? [[HC.gold, shooter], [HC.sub, " † "], [HC.text, hitWho]]
          : [[HC.gold, shooter], [HC.sub, " → "], [HC.text, hitWho], [HC.red, " " + much]]);
      }
    }
  }

  // The server's word against this browser's guess.
  //
  // Both ran the same `walk` on the same keys, so they mostly agree, and what
  // is left is the tenth of a second the answer took to come back. Snapping to
  // it would be a walk that stutters twenty times a second; ignoring it would
  // be a player who is somewhere else as far as everyone else is concerned. So
  // it is eased toward, once per snapshot rather than once per frame — unless
  // it is far enough out that easing would take seconds, which is what being
  // thrown by a blast or respawning looks like from here, and then it is taken
  // as given.
  function reconcile() {
    const m = me(net);
    if (!m || net.last === appliedSnap) return;
    appliedSnap = net.last;
    E.set_health(state, m.health);
    // The count is the browser's own between snapshots, or it would jump back
    // up for the tenth of a second between pulling the trigger and hearing
    // about it. Two rounds of slack covers everything in flight; past that the
    // two have genuinely disagreed — a magazine one side walked over and the
    // other did not — and the server is the one that is right.
    // Both conversions are load-bearing: an `Int` across the wasm boundary is
    // an i64, which arrives as a BigInt and will not be subtracted from a
    // number, and will not be passed back as one either.
    // A correction may not undo a magazine change this browser has just
    // made. Without the pause the two fought: the client's magazine ran out
    // and it changed to a fresh one, the next snapshot still had the server
    // three rounds from the end and pulled the count back down, the client
    // changed again — and again, twenty times a second, eating the belt and
    // playing the reload over and over. The server is making the same change
    // a round trip behind; it is given that long to catch up.
    if (m.ammo !== null && performance.now() > reloadHold
        && Math.abs(m.ammo - Number(E.held_ammo(state))) > 2) {
      E.set_held_ammo(state, BigInt(m.ammo));
    }
    const x = E.player_x(state), y = E.player_y(state), z = E.player_z(state);
    const d = Math.hypot(x - m.x, y - m.y, z - m.z);
    if (d > 4) E.set_at(state, m.x, m.y, m.z);
    else if (d > 0.03) E.set_at(state, x + (m.x - x) * 0.35, y + (m.y - y) * 0.35, z + (m.z - z) * 0.35);
    // The clock is let run locally and only pulled back when it has drifted
    // further than the wind can notice. Writing it every snapshot would step
    // the weather backwards and forwards twenty times a second.
    //
    // `net.last.clock` and not `m.clock`. The clock is on the snapshot's
    // header — it is the same fact for everybody in the valley — and `m` is
    // this player's own record, which has never carried one. So the comparison
    // was against `undefined`, which makes a NaN, and `NaN > 0.5` is false:
    // the correction had never once run. Every browser was keeping its own
    // time from the moment it joined.
    //
    // That drifts, and only one way. `dt` is clamped to a twentieth of a
    // second a frame, so every frame slower than twenty a second — every
    // stutter, every alt-tab, every minute in a background tab where the frame
    // loop stops altogether — is time the valley does not get back. A day here
    // is six minutes long. Two people who joined an hour apart, or one of whom
    // looked away for a while, were standing in the same valley at different
    // hours of it: one in daylight, one at night, watching the same sun from
    // opposite sides of the sky.
    const clock = net.last.clock;
    if (Math.abs(E.world_time(world) - clock) > 0.5) E.set_world_time(world, clock);
  }

  // ---- names over heads ----------------------------------------------------
  //
  // Names over heads: who and where go into the tag slots, and the engine
  // does the projecting with the same matrix it drew the valley by. What used
  // to be one DOM element per player is now sixteen floats per player.
  function drawTags(seen) {
    let n = 0;
    for (const p of seen) {
      if (!p.name || n >= 16) continue;
      hudSlot(HSLOT.tag + n, [[-1, p.name]], p.x, p.y, p.z, p.health);
      n++;
    }
    hudHead(HH.tagN, n);
  }

  // ---- the roster ----------------------------------------------------------
  //
  // The same list the tags come from, stood still down the left: who is here,
  // in the colour their figure is drawn in, and how much of them is left. The
  // tag over a head answers "who is that"; this answers "who am I in here
  // with", which is a different question and is not one you can answer by
  // turning round.
  //
  // Rebuilt only when the shape of it changes — a row arriving or leaving, or
  // a name landing after the player did. Health moves every snapshot, so that
  // goes straight onto the element it belongs to and the browser is left to
  // ease it; rewriting the markup twenty times a second would restart every
  // one of those transitions and the bars would step instead of slide.
  function drawRoster(list) {
    // Somebody is announced once their name has arrived, which is a moment
    // after they have: the id comes in the snapshot and the name follows on a
    // `who` line, and "somebody joined" is not worth saying. Never about this
    // player — you know when you arrived.
    for (const p of list) {
      if (!p.name || told.has(p.id)) continue;
      told.set(p.id, p.name);
      if (!firstRoster && p.id !== net.id) { cry(p.name, "joined", "in"); note([[HC.gold, p.name], [HC.sub, " joined"]]); }
    }
    for (const [id, name] of told) {
      if (list.some(p => p.id === id)) continue;
      told.delete(id);
      if (id !== net.id) { cry(name, "left", "out"); note([[HC.dim, name + " left"]]); }
    }
    firstRoster = false;
    // What they have done and what has been done to them. Two numbers rather
    // than a ratio: a ratio hides how much has happened, and in a valley of
    // four people that is most of the story. Sixteen floats a row and the
    // engine draws the dot, the bar and the strike-through.
    let n = 0;
    for (const p of list) {
      if (n >= 16) break;
      const c = hueOf(p.id, 1);
      hudSlot(HSLOT.roster + n, [[-1, p.name || `player ${p.id}`]],
              c[0], c[1], c[2],
              // Self rides in the health's sign — a slot only has eight
              // floats and the engine reads the magnitude.
              p.self ? -Math.max(0.001, p.health) : p.health,
              p.hit, p.shot, p.health <= 0.001 ? 1 : 0);
      n++;
    }
    hudHead(HH.rosterN, n);
  }

  // ---- the round -----------------------------------------------------------
  //
  // How long is left, and the board between rounds. Both read the same two
  // numbers off the world — the phase and the seconds remaining — so a browser
  // alone in its own valley shows exactly what a shared one does, without a
  // server having to be there to say so.
  // Which snapshot the round's clock was last set from, so it is corrected
  // once per arrival rather than every frame.
  let syncedAt = -1;

  // A browser alone in its valley still has a round, and one player in it.
  const soloBoard = () => [{
    id: (net && net.id) ?? 0, name: "you",
    kills: Number(E.kills(state)), deaths: Number(E.deaths(state)),
  }];

  // The clock and the dark sheet are the engine's — it holds the phase and
  // the seconds itself — so the page's whole share of the board is the rows.
  function drawRound(phase, left, list, myId) {
    if (phase !== 1) { hudHead(HH.boardN, 0); return; }
    // Most kills first, then fewest deaths, then whoever arrived first — a
    // board that reordered itself every tick would be unreadable.
    const rank = list.slice().sort((a, b) =>
      (b.kills | 0) - (a.kills | 0) || (a.deaths | 0) - (b.deaths | 0) || a.id - b.id);
    let n = 0;
    for (const p of rank) {
      if (n >= 16) break;
      hudSlot(HSLOT.board + n, [[-1, p.name ?? ("player" + p.id)]],
              p.kills | 0, p.deaths | 0, p.id === myId ? 1 : 0);
      n++;
    }
    hudHead(HH.boardN, n);
  }

  // A colour per player, off the golden angle round the hue circle: the second
  // person to arrive is a different colour rather than a slightly different
  // green. It pales as they run out of health, which is the only tell there is
  // that somebody across the meadow is nearly down.
  function hueOf(id, health) {
    const h = (id * 0.61803398875) % 1, s = 0.35 + 0.4 * Math.max(health, 0), v = 0.55 + 0.35 * Math.max(health, 0);
    const k = (n) => (n + h * 6) % 6;
    const f = (n) => v - v * s * Math.max(0, Math.min(Math.min(k(n) , 4 - k(n)), 1));
    return [f(5), f(3), f(1)];
  }

  // ---- everything the tier decides, rebuilt when the tier changes ----------

  let tierIdx = Math.min(guessTier(gpuInfo), MAX_TIER);
  let T = null;       // the active tier
  // Two pointers into wasm memory now, and the difference between them is the
  // whole of what makes more than one person possible. `world` owns the
  // terrain, the crystals, the rounds in the air and the shark; `state` is
  // this player — a look, a walk, a health. Everything below asks `state`,
  // because a player block knows which world it is in and the exports follow
  // that pointer; only building and freeing the valley needs `world` itself.
  let world = 0;
  let state = 0;
  // Where a browser on its own puts this player down.
  //
  // The valley is a function of its seed and always has been, which is the
  // point of it — but so was the spawn, and with nobody else here the two
  // terms that were meant to spread it are both nought: player id nought, in a
  // world whose clock has not started. Every run began in the same tree
  // looking at the same ridge. One roll, kept for the life of the page so a
  // change of quality — which rebuilds the valley and rejoins — does not
  // teleport anybody, and rolled again when a finished valley is started over.
  let spawnRoll = Math.random() * 4096;
  // How many instances the engine's HUD emitted this frame — settled before
  // the pass begins, drawn at its end.
  let hudCount = 0;
  let W = null;       // tier-dependent GPU resources
  const stateF32 = () => new Float32Array(E.memory.buffer, Number(state), 64);
  // The world's own block, which is not the player's. The shark lives here —
  // it is one animal in a valley that may have eight people in it, and in
  // somebody else's valley the server is what says where it is. This was being
  // read out of the player block, sixty-four floats long, at indices in the
  // eighties: `undefined` every one of them, which lands in the uniform as NaN
  // and puts the animal at no position at all. It has been invisible since the
  // day the shark moved out of a player and into the world.

  // The water grid: x, z, and the exact depth of the terrain below each
  // vertex — Rill's own height function, asked point by point.
  function waterMesh(half, seg) {
    const w = seg + 1, step = (half * 2) / seg;
    // One depth per lattice corner rather than one per vertex. A corner
    // belongs to four cells and each of the six vertices of a cell repeats
    // one, so this is six times fewer calls into a function twelve sines
    // deep — and the lattice is also where the bottom's slope comes from,
    // which the waves need in order to bend as they come ashore.
    const dep = new Float32Array(w * w);
    for (let j = 0; j < w; j++) for (let i = 0; i < w; i++) {
      dep[j * w + i] = Math.max(-E.ground(-half + i * step, -half + j * step), 0.0);
    }
    const clamp = (v) => v < 0 ? 0 : v > w - 1 ? w - 1 : v;
    const at = (i, j) => dep[clamp(j) * w + clamp(i)];

    const verts = new Float32Array(seg * seg * 6 * 5);
    let o = 0;
    for (let i = 0; i < seg; i++) for (let j = 0; j < seg; j++) {
      for (const [ii, jj] of [[i, j], [i, j+1], [i+1, j+1], [i, j], [i+1, j+1], [i+1, j]]) {
        verts[o++] = -half + ii * step;
        verts[o++] = -half + jj * step;
        verts[o++] = at(ii, jj);
        verts[o++] = (at(ii + 1, jj) - at(ii - 1, jj)) / (2 * step);
        verts[o++] = (at(ii, jj + 1) - at(ii, jj - 1)) / (2 * step);
      }
    }
    return verts;
  }

  // Which weapon is up, which one has been asked for, and how far through the
  // change of hands we are. `swap` runs nought to one and back; `held` only
  // changes at the bottom of it, which is what makes it read as one weapon
  // being put away and another brought up rather than a substitution.
  //
  // Declared up here rather than beside the rest of the frame loop's state
  // because `buildWorld` reads it, and `buildWorld` runs while the page is
  // still starting up — a `let` further down is in its dead zone at that
  // point and the first frame throws.
  let held = 2, wantHeld = 2, swap = 0, swapping = 0, loadedTube = false;

  function buildWorld(idx) {
    T = TIERS[idx];
    // The camera survives the world being rebuilt around it.
    let cam = null;
    if (state) { cam = stateF32().slice(19, 26); E.free_world(world); }
    // How many pines and crystals is the server's to decide when there is one.
    // The grid is not: it is the terrain mesh's resolution and nothing else
    // samples it, so a slow machine draws a coarser valley and still stands on
    // exactly the same ground and shoots exactly the same crystals. That is the
    // whole of what re-tiering may still change while other people are here.
    const pines = net ? net.world.pines : T.pines;
    const crystals = net ? net.world.crystals : T.crystals;
    const grass = net ? net.world.grass : T.grass;
    world = E.init(BigInt(T.grid), BigInt(pines), BigInt(crystals), BigInt(grass));
    state = E.join(world);
    // `join` has already placed them; this stirs the roll in and places them
    // again. Only when there is nobody else — in somebody else's valley the
    // spawn is the server's, and a client that moved itself would be corrected
    // by the next snapshot anyway.
    if (!net) { E.spawn_salt(state, spawnRoll); E.respawn(state); }
    // A new player block means a new HUD block hanging off it: the atlas
    // survives a re-tier, the metrics are written again into the fresh block.
    hudWire(state);
    // A re-tier is a new valley and a new player block in it, and `join` hands
    // out a player carrying the rifle. What is in the hands is the page's
    // business and did not change because the terrain got coarser — without
    // this the launcher stayed on screen and fired rounds.
    E.set_weapon(state, BigInt(held));
    if (net) {
      // Rejoining after a tier change: the valley is new, the connection is
      // not. Where everybody is arrives again with the next snapshot, but the
      // clock and the crystals already shot have to be right before the first
      // frame — otherwise the wind jumps and three hundred crystals stand back
      // up for a twentieth of a second.
      E.set_world_time(world, net.last ? net.last.clock : net.world.clock);
      for (const i of net.gone ?? []) E.put_gone(world, BigInt(i));
      appliedSnap = null;
    }
    if (cam) stateF32().set(cam, 19);

    if (W) for (const k of ["terrainBuf", "pineInst", "crystalInst", "grassInst", "waterBuf", "flyBuf", "shadowTex"]) W[k]?.destroy?.();
    const counts = {
      terrain: Number(E.terrain_verts(state)),
      pines: Number(E.pine_count(state)),
      crystals: Number(E.crystal_count(state)),
      grass: Number(E.grass_count(state)),
    };
    W = { counts };
    W.terrainBuf = gpuBuf(new Float32Array(E.memory.buffer, Number(E.terrain(state)), counts.terrain * 9));
    W.blocks = Number(E.blocks_count(state));
    W.blockInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.blocks(state)), Math.max(W.blocks, 1) * 8));
    W.pineInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.pines(state)), counts.pines * 8));
    W.crystalInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.crystals(state)), counts.crystals * 8));
    W.grassInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.grass(state)), counts.grass * 8));
    W.waterBuf = gpuBuf(waterMesh(86, T.water));
    W.waterVerts = T.water * T.water * 6;

    // Fireflies at this tier's density.
    const flyData = new Float32Array(T.flies * 8);
    for (let i = 0; i < T.flies; i++) {
      const a = Math.random() * Math.PI * 2, r = 12 + Math.random() * 32;
      flyData[i * 8] = Math.cos(a) * r;
      flyData[i * 8 + 1] = 1.0 + Math.random() * 3.5;
      flyData[i * 8 + 2] = Math.sin(a) * r;
      flyData[i * 8 + 3] = Math.random();
    }
    W.flyBuf = device.createBuffer({ size: flyData.byteLength, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
    device.queue.writeBuffer(W.flyBuf, 0, flyData);
    W.flyDrawBg = device.createBindGroup({ layout: flyBgl, entries: [{ binding: 0, resource: { buffer: W.flyBuf } }] });
    W.simBg = device.createBindGroup({ layout: simBgl, entries: [
      { binding: 0, resource: { buffer: W.flyBuf } },
      { binding: 1, resource: { buffer: flyTime } },
    ]});

    // The shadow map at this tier's size, within what the adapter allows,
    // and the bind group that samples it.
    const shSize = Math.min(T.shadow, maxTex);
    W.shadowTex = device.createTexture({
      size: [shSize, shSize], format: "depth32float",
      usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING,
    });
    W.shadowView = W.shadowTex.createView();
    // ---- the environment probe -------------------------------------------
    //
    // One point in the middle of the valley, six ninety-degree views from it,
    // stored on the faces of a cube. This is what lets a surface reflect the
    // place it is standing in rather than a guess at it: a chip of glass on the
    // grass shows green underneath and hillside to the side, and turns as you
    // walk around it, because the cube knows which way is which.
    //
    // A single probe is wrong for everything not standing at its centre, and
    // that is the accepted trade: parallax in a reflection is only legible on a
    // surface large enough to see two things in at once. On a shard the size of
    // a thumbnail it is not, and one bake at load is the entire cost.
    //
    // 128 pixels a face. Reflections in something faceted and moving carry
    // colour and rough shape and nothing finer, and the whole cube at this size
    // is 384 KB.
    const ENV = 128;
    W.envTex = device.createTexture({
      label: "env-probe", size: [ENV, ENV, 6], format: "rgba8unorm",
      usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING,
    });
    W.envView = W.envTex.createView({ dimension: "cube" });
    // Multisampled once and resolved into each face in turn. Reusing the
    // existing pipelines rather than building unsampled copies of every one of
    // them costs a 128-pixel scratch target and saves four pipelines.
    const envMs = device.createTexture({
      size: [ENV, ENV], sampleCount: 4, format: "rgba8unorm",
      usage: GPUTextureUsage.RENDER_ATTACHMENT,
    });
    const envMsView = envMs.createView();
    const envDepth = device.createTexture({
      size: [ENV, ENV], sampleCount: 4, format: "depth24plus",
      usage: GPUTextureUsage.RENDER_ATTACHMENT,
    }).createView();
    // The bake cannot read the cube it is writing, so it gets a black one.
    const blankCube = device.createTexture({
      size: [1, 1, 6], format: "rgba8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
    }).createView({ dimension: "cube" });

    W.bgMain = device.createBindGroup({ layout: bglMain, entries: [
      { binding: 0, resource: { buffer: uni } },
      { binding: 1, resource: W.shadowView },
      { binding: 2, resource: shadowSamp },
      { binding: 3, resource: foliageView },
      { binding: 4, resource: foliageSamp },
      { binding: 5, resource: barkView },
      { binding: 6, resource: barkSamp },
      { binding: 7, resource: groundView },
      { binding: 8, resource: rockView },
      { binding: 9, resource: sandView },
      { binding: 10, resource: plantView },
      { binding: 11, resource: rifleView },
      { binding: 12, resource: meteorView },
      { binding: 13, resource: W.envView },
      { binding: 14, resource: jupiterView },
      { binding: 15, resource: saturnView },
      { binding: 16, resource: moonsView },
      { binding: 17, resource: sharkView },
      { binding: 18, resource: propView },
      { binding: 19, resource: rpgView },
    ]});

    // The water pipeline carries this tier's wave and ripple counts.
    W.waterPipe = pipe("water", S.WATER,
      [{ arrayStride: 20, attributes: [
        { shaderLocation: 0, offset: 0, format: "float32x3" },   // x, z, depth
        { shaderLocation: 1, offset: 12, format: "float32x2" },  // the bottom's slope
      ]}],
      { blend: { color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha" }, alpha: {} },
        cull: "none", depth: { depthWriteEnabled: false },
        constants: { WAVES: T.waves, RIPPLES: T.ripples } });

    // The same four things the main pass draws first, into a face of the cube.
    // Not the sky, which is evaluated at read time; not the fireflies, tracers
    // or fire, which are not there when this is baked and would be wrong a
    // second later if they were.
    W.envBg = device.createBindGroup({ layout: bglMain, entries: [
      { binding: 0, resource: { buffer: uni } },
      { binding: 1, resource: W.shadowView },
      { binding: 2, resource: shadowSamp },
      { binding: 3, resource: foliageView },
      { binding: 4, resource: foliageSamp },
      { binding: 5, resource: barkView },
      { binding: 6, resource: barkSamp },
      { binding: 7, resource: groundView },
      { binding: 8, resource: rockView },
      { binding: 9, resource: sandView },
      { binding: 10, resource: plantView },
      { binding: 11, resource: rifleView },
      { binding: 12, resource: meteorView },
      { binding: 13, resource: blankCube },
      { binding: 14, resource: jupiterView },
      { binding: 15, resource: saturnView },
      { binding: 16, resource: moonsView },
      { binding: 17, resource: sharkView },
      { binding: 18, resource: propView },
      { binding: 19, resource: rpgView },
    ]});
    const envFmt = { format: "rgba8unorm" };
    W.envPipes = {
      terrain: pipe("env-terrain", S.TERRAIN, terrainLayout, envFmt),
      pine:    pipe("env-pine", S.PINE, [pineLayout, instLayout], { ...envFmt, cull: "none" }),
      crystal: pipe("env-crystal", S.INSTANCED, [meshLayout, instLayout],
                    { ...envFmt, constants: { emissive: 0.72 } }),
      water:   pipe("env-water", S.WATER,
        [{ arrayStride: 20, attributes: [
          { shaderLocation: 0, offset: 0, format: "float32x3" },
          { shaderLocation: 1, offset: 12, format: "float32x2" },
        ]}],
        { ...envFmt,
          blend: { color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha" }, alpha: {} },
          cull: "none", depth: { depthWriteEnabled: false },
          constants: { WAVES: T.waves, RIPPLES: T.ripples } }),
    };

    // Where the probe stands, and the six directions from it. The ups are the
    // standard cube-face ups, which are flipped against the world's because a
    // cube map's own space has y running the other way — get this wrong and the
    // reflections are right in every respect except upside down, which is the
    // sort of mistake that looks like a lighting bug for an hour.
    W.envAt = [0, 7, 0];
    W.envFaces = [
      { dir: [ 1, 0, 0], up: [0, -1,  0] },
      { dir: [-1, 0, 0], up: [0, -1,  0] },
      { dir: [ 0, 1, 0], up: [0,  0,  1] },
      { dir: [ 0,-1, 0], up: [0,  0, -1] },
      { dir: [ 0, 0, 1], up: [0, -1,  0] },
      { dir: [ 0, 0,-1], up: [0, -1,  0] },
    ];
    W.envMsView = envMsView;
    W.envDepth = envDepth;
    // Not a flag on the run: `buildWorld` is called again every time the
    // quality tier moves, and it makes a new cube along with every other
    // buffer. A `let envBaked` set once at startup means the first bake goes
    // into a texture that is thrown away moments later and the one that
    // replaces it stays blank for the life of the page — with no error, since
    // sampling an unwritten texture is perfectly legal and returns nothing.
    W.envBaked = false;
    W.envWhen = -99;   // not `envAt`: that is where the probe stands

    W.tris = Math.round((counts.terrain + pineVerts * counts.pines + crystalVerts * counts.crystals
                       + grassVerts * counts.grass + rifleIdx) / 3);
  }
  // The valley someone else is running, if this page was pointed at one. The
  // welcome has to arrive before the world is built: how much valley there is
  // is not a decision a client gets to make while other people are standing
  // in it.
  let net = null;
  // Null while nothing has gone wrong; otherwise the line to show for it.
  let turnedAway = null;
  if (server) {
    step("asking to join");
    try {
      net = join(server, name);
      await net.arrival;
      net.gone = [];
    } catch (e) {
      // A valley that cannot be reached is not a reason to show nothing: the
      // single-player one is right here and works. But it has to say so, and
      // in the same letters that say the socket went — a player who gave the
      // wrong word and was quietly dropped into a valley of their own would
      // wander around it wondering where everybody was.
      //
      // What it says depends on who decided it. A server that answered `no`
      // gave a reason and that reason is shown, because "the valley is full"
      // and "wrong word" send the person to do different things: one waits a
      // few minutes, the other checks what they typed. A socket that simply
      // never opened said nothing worth repeating — `cannot reach ws://…` is
      // for the console — so that keeps the old guess.
      console.warn(`[valley] ${e.message} — playing alone`);
      turnedAway = e.refused || "wrong word, or nobody is home";
      net = null;
    }
  }

  buildWorld(tierIdx);
  step("the valley");
  // Last, because it is the one thing that can be done while the GPU work
  // above is already in flight, and because six decodes finishing is a fair
  // definition of "everything is loaded".
  await decodeAll(step);

  // A ninety-degree view from a point, as one column-major matrix. Written out
  // by hand because this is the only place in the whole program that builds a
  // matrix in JavaScript — every other one comes out of world.rill — and a
  // library for two functions used once would be the larger cost.
  //
  // At ninety degrees the projection is almost all zeroes: the focal term is
  // 1/tan(45°), which is one, and the aspect is one, so the whole of it is the
  // depth range.
  function faceVP(eye, dir, up, near, far) {
    const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    const cross = (a, b) => [a[1]*b[2] - a[2]*b[1], a[2]*b[0] - a[0]*b[2], a[0]*b[1] - a[1]*b[0]];
    const dot = (a, b) => a[0]*b[0] + a[1]*b[1] + a[2]*b[2];
    const norm = a => { const l = Math.hypot(...a); return [a[0]/l, a[1]/l, a[2]/l]; };
    const z = norm(sub([0, 0, 0], dir));          // the camera looks down -z
    const x = norm(cross(up, z));
    const y = cross(z, x);
    // The view, column-major: the basis in the first three columns and the
    // translation, in that basis, in the fourth.
    // `view`, not `V`: that is the asset version at module scope, and a local
    // of the same name shadowing it here is the sort of thing that is harmless
    // until someone adds a fetch to this function.
    const view = [ x[0], y[0], z[0], 0,
                   x[1], y[1], z[1], 0,
                   x[2], y[2], z[2], 0,
                   -dot(x, eye), -dot(y, eye), -dot(z, eye), 1 ];
    const d = near - far;
    const P = [ 1, 0, 0, 0,
                0, 1, 0, 0,
                0, 0, far / d, -1,
                0, 0, far * near / d, 0 ];
    const out = new Float32Array(16);
    for (let c = 0; c < 4; c++) for (let r = 0; r < 4; r++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += P[k * 4 + r] * view[c * 4 + k];
      out[c * 4 + r] = s;
    }
    return out;
  }

  // Six passes, six submits. Separate submits rather than one encoder because
  // the uniform buffer holds one view-projection at a time and `writeBuffer`
  // lands on the queue, not in the encoder: six writes before a single submit
  // would leave every face drawn from the sixth camera.
  function bakeEnv(uf) {
    const u = uf.slice();
    u.set([W.envAt[0], W.envAt[1], W.envAt[2]], 32);      // the eye, for the fog
    for (let f = 0; f < 6; f++) {
      const face = W.envFaces[f];
      u.set(faceVP(W.envAt, face.dir, face.up, 0.4, 400), 0);
      device.queue.writeBuffer(uni, 0, u);
      const e = device.createCommandEncoder({ label: `env-face-${f}` });
      const p = e.beginRenderPass({
        colorAttachments: [{
          view: W.envMsView,
          resolveTarget: W.envTex.createView({ dimension: "2d", baseArrayLayer: f, arrayLayerCount: 1 }),
          // Cleared to nothing, alpha and all: the sky is not drawn here, it is
          // evaluated when the cube is read, and a zero alpha is how a face
          // says "nothing of the valley is this way, use the sky".
          loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 0 },
        }],
        depthStencilAttachment: {
          view: W.envDepth, depthClearValue: 1, depthLoadOp: "clear", depthStoreOp: "discard",
        },
      });
      p.setBindGroup(0, W.envBg);
      p.setPipeline(W.envPipes.terrain);
      p.setVertexBuffer(0, W.terrainBuf); p.draw(W.counts.terrain);
      p.setPipeline(W.envPipes.pine);
      p.setVertexBuffer(0, pineBuf); p.setVertexBuffer(1, W.pineInst); p.draw(pineVerts, W.counts.pines);
      p.setPipeline(W.envPipes.crystal);
      p.setVertexBuffer(0, crystalBuf); p.setVertexBuffer(1, W.crystalInst); p.draw(crystalVerts, W.counts.crystals);
      p.setPipeline(W.envPipes.water);
      p.setVertexBuffer(0, W.waterBuf); p.draw(W.waterVerts);
      p.end();
      device.queue.submit([e.finish()]);
    }
  }

  // MSAA targets, remade when the canvas resizes or the tier's pixel ratio
  // changes.
  let msaaTex, depthTex;
  function resize() {
    const dpr = Math.min(devicePixelRatio, T.dpr, DPR_CAP);
    const w = Math.floor(canvas.clientWidth * dpr), h = Math.floor(canvas.clientHeight * dpr);
    if (w !== canvas.width || h !== canvas.height) {
      canvas.width = w; canvas.height = h;
      msaaTex?.destroy(); depthTex?.destroy();
      msaaTex = device.createTexture({ size: [w, h], sampleCount: 4, format, usage: GPUTextureUsage.RENDER_ATTACHMENT });
      depthTex = device.createTexture({ size: [w, h], sampleCount: 4, format: "depth24plus", usage: GPUTextureUsage.RENDER_ATTACHMENT });
    }
  }
  resize();
  addEventListener("resize", resize);

  // Input.
  let keys = 0, mdx = 0, mdy = 0;
  // 64 is the crouch. Rill's `walk` reads the first six bits and ignores this
  // one, so crouching changes what everybody sees and not where anybody goes —
  // which is the honest half of it to ship first.
  const bit = { KeyW: 1, KeyS: 2, KeyA: 4, KeyD: 8, Space: 16, ShiftLeft: 32, ShiftRight: 32, KeyC: 64 };
  addEventListener("keydown", e => { if (bit[e.code]) { keys |= bit[e.code]; e.preventDefault(); } });
  addEventListener("keydown", e => { if (e.code === "KeyM") ensureAudio(() => setMusic(!musicOn)); });
  // One and two, the rifle and the launcher. Asked for rather than done: what
  // happens next is an animation, and pressing the key you are already holding
  // should do nothing rather than play it.
  addEventListener("keydown", e => {
    if (e.repeat) return;
    if (e.code === "Digit1" || e.code === "Numpad1") wantHeld = 0;
    if (e.code === "Digit2" || e.code === "Numpad2") wantHeld = 1;
    if (e.code === "Digit3" || e.code === "Numpad3") wantHeld = 2;
  });
  // On the press, not held: read from the event rather than from the key mask,
  // so leaning on space at the end does not restart over and over.
  addEventListener("keydown", e => { if (e.code === "Space" && !e.repeat) wantRestart = true; });
  // A fresh magazine, asked for rather than waited for. `!e.repeat` because a
  // held key repeats and a magazine change is not a thing to queue up; the
  // frame below is where it is answered, because that is where the world and
  // the clock the animation runs on both are.
  addEventListener("keydown", e => { if (e.code === "KeyR" && !e.repeat) wantReload = true; });
  // The lamp. Off to begin with, and off is the honest default: it used to
  // come on by itself the moment the sun went under the world, so everybody in
  // the valley carried the same light and nobody could put theirs out. A light
  // is a thing you decide to show, and on a dark hillside it is the loudest
  // thing about you — see `lamps` in uniforms.wgsl for who else can see it.
  let torchOn = false;
  addEventListener("keydown", e => { if (e.code === "KeyF" && !e.repeat) torchOn = !torchOn; });

  // ---- saying something ------------------------------------------------------
  //
  // Return opens the field, return sends what is in it, escape gives up on it.
  // Only where there is somebody to hear: alone in a valley the key does
  // nothing, because a chat box with one person in it is a diary.
  //
  // The guard is one listener in the *capture* phase rather than a test at the
  // top of each of the eight keydown handlers below. Every one of those is on
  // the window and so runs in the bubble phase; this one sees the key first
  // and, while the field is open, stops it there. Without it, typing "was"
  // walks you forward and left, "f" puts your lamp on and off, and "3" changes
  // weapons — all while the letters are also going into the box.
  const chatEl = document.getElementById("chat");
  const chatIn = document.getElementById("chatin");
  let chatting = false;

  function openChat() {
    chatting = true;
    chatEl.classList.add("on");
    chatIn.value = "";
    // The keys that were down stay down otherwise: open the box with a foot on
    // W and you type a sentence while walking into the lake.
    keys = 0;
    // And the mouse comes back, which is what makes the caret real. The
    // `pointerlockchange` handler below takes the aim and the trigger off with
    // it, so leaving the box does not leave a held button behind.
    document.exitPointerLock();
    chatIn.focus();
  }

  function closeChat() {
    chatting = false;
    chatEl.classList.remove("on");
    chatIn.blur();
    // Back to the valley. A key press is a gesture, so the browser will
    // usually give the lock straight back; when it will not, the canvas is
    // still one click away and the page already says `click to look`.
    canvas.requestPointerLock?.();
  }

  addEventListener("keydown", e => {
    const enter = e.code === "Enter" || e.code === "NumpadEnter";
    if (chatting) {
      if (enter) {
        const text = chatIn.value.trim();
        // Empty is what pressing return twice looks like, and it closes the
        // box rather than sending nothing. What may actually be said, and how
        // often, is the server's — see `say_one` in server.rill.
        if (text && net) net.say(text);
        closeChat();
      } else if (e.code === "Escape") closeChat();
      // Everything else belongs to the field and to nothing else.
      e.stopImmediatePropagation();
      return;
    }
    if (enter && net && !e.repeat) {
      openChat();
      e.preventDefault();
      e.stopImmediatePropagation();
    }
  }, true);
  // A click anywhere else is a change of mind: the canvas takes the lock back
  // on its own click handler, and a field left open behind it would swallow
  // the next thing typed.
  canvas.addEventListener("mousedown", () => { if (chatting) closeChat(); });
  addEventListener("keyup", e => { if (bit[e.code]) keys &= ~bit[e.code]; });
  canvas.addEventListener("click", () => {
    canvas.requestPointerLock();
    ensureAudio(() => {});     // the first gesture: a browser will start audio on it
  });
  addEventListener("mousemove", e => {
    if (document.pointerLockElement === canvas) { mdx += e.movementX; mdy += e.movementY; }
  });

  // The trigger. Only once the mouse is captured — the first click is what
  // captures it, and firing on that one would put a round downrange before
  // anyone had a chance to aim.
  // A bolt action fires once and then has to be worked. `boltT` runs from zero
  // to BOLT_CYCLE after every shot; while it is running the rifle is open and
  // the trigger does nothing, which is the whole of what a bolt action is.
  const BOLT_CYCLE = 1.15;
  // The launcher is slower to bring back on target than a bolt is to work: the
  // whole of the propellant leaves the front of it and there is a tube to
  // reload. Two and a half seconds is what stops a rocket being a rifle.
  const RPG_CYCLE = 2.5;
  // Ten rounds a second, which is about what the gun really cycles at and
  // empties the magazine in three. Must stay in step with `ak_gap` in
  // valley.rill — that is the gate the world holds every client to, and a
  // browser that asked for rounds faster than this would simply be refused
  // them.
  const AK_CYCLE = 0.1;
  const cycleNow = () => (held === 1 ? RPG_CYCLE : held === 2 ? AK_CYCLE : BOLT_CYCLE);
  let wantShot = false, wantReload = false, fireHeld = false, akBurst = 0, lastAkLoaded = -1, reloadHold = 0, kickUp = 0, kickSide = 0, recoil = 0, flash = 0, boltT = BOLT_CYCLE, wasReady = true, breaks = 0, blastsHeard = 0, zapSeen = 0;
  // What the climb has taken off the aim and not yet handed back. `kickUp` and
  // `kickSide` are this frame's kick and are spent every frame; these are the
  // running total across a burst, and they are what the settle gives back.
  let owedUp = 0, owedSide = 0;
  // How long the hands are busy changing an AK's magazine, and when they last
  // started. The number is the fire clock's own — the trigger is dead for
  // exactly this long — and `ak.wgsl` animates against the same one, so the
  // magazine is home on the frame the rifle will fire again and not a moment
  // either side of it.
  const CHANGE_SECS = 1.45;
  let changeAt = -1e9;
  let ended = false, lastStanding = -1, wantRestart = false, lastOver = 0;
  // Arrivals and departures, across the middle. Announced from the roster
  // rather than from a message of their own: the roster already knows, and a
  // valley that has to be told twice about the same fact will one day be told
  // once.
  const told = new Map();
  let firstRoster = true;

  // The log, bottom left: everything that happened, in the order it happened,
  // oldest at the top. Lines are segments of [colourIndex, text] now rather
  // than markup — the engine draws them, this side only keeps the ring. A
  // line somebody typed goes in as one plain segment: whatever is in it is
  // bytes to the atlas, never markup to anything.
  const LOG_KEEP = 12;
  const logRing = [];

  function pushLog(segs, said) {
    logRing.push({ segs, said, born: performance.now() / 1000 });
    // Drop what has both scrolled off and faded; the engine fades by age, so
    // the ring only has to stay short.
    while (logRing.length > LOG_KEEP) logRing.shift();
    for (let i = 0; i < LOG_KEEP; i++) {
      const l = logRing[i];
      if (l) hudSlot(HSLOT.log + i, l.segs, l.said ? 1 : 0, l.born);
      else hudClear(HSLOT.log + i);
    }
  }
  const note = (segs) => pushLog(segs, false);
  const sayLine = (who, text) => pushLog([[HC.green, who], [HC.lav, " " + text]], true);

  function cry(who, what, kind) {
    hudSlot(HSLOT.crier, kind === "in"
      ? [[HC.gold, who], [HC.text, " " + what]]
      : [[HC.sub, who], [HC.dim, " " + what]]);
    hudHead(HH.crierAt, performance.now() / 1000);
    hudHead(HH.crierOut, kind === "out" ? 1 : 0);
  }
  // A refusal, and the hit line: pushed once, timed by the engine.
  function sayWarn(text) {
    hudSlot(HSLOT.warn, [[HC.orange, text.toUpperCase()]]);
    hudHead(HH.warnAt, performance.now() / 1000);
  }
  let lastAmmo = -1, lastRockets = -1, lastHeld = -1, lastAkMags = -1;
  let wasDown = false;
  let lastHp = -1;
  // The sights. `wantAim` is the button; `aim` is how far through the move,
  // eased, because a field of view that changes between one frame and the next
  // reads as a cut rather than as somebody raising a rifle.
  let wantAim = false, aim = 0;
  const SWAP_SECS = 0.19;
  // What the scope does to the world. Five times: the far ridge is four
  // hundred metres off and somebody standing on it is a figure rather than a
  // suggestion. It is a lot of magnification for a rifle, and the mouse is
  // divided by the same number, so what it costs is how fast you can swing —
  // which is what a scope costs.
  const AIM_ZOOM = 5;
  // And what the AK's sights do to it, which is very little. There is no glass
  // on that one — there is a notch and a post — and a notch magnified five
  // times is a scope with a strange reticle rather than a rifle held properly.
  //
  // Twice. Dead flat is the honest number for a notch and a post and it reads
  // as the button doing nothing at all: the weapon comes up, the picture does
  // not change, and the hand goes looking for what it just pressed. This is
  // the number that came out of trying it, by halves and then by wholes.
  //
  // Two things follow from it being this large. The mouse is divided by it, so
  // the swing halves while the button is down — the AK now costs something to
  // aim, where at a fifth it cost almost nothing. And the weapon does not grow
  // with the world: the viewmodel has a frustum of its own, so the sights stay
  // the size they were while what is behind them doubles. That is what a
  // magnified sight picture looks like and it is the right way round — the
  // post is a post, and it is the valley that has come closer.
  const IRON_ZOOM = 2.0;
  // The launcher takes the same number for the same reason: its sight is a
  // post and a ladder, not glass. Five times behind an iron post was a scope
  // that happened to have a rocket in front of it.
  const zoomNow = () => (held === 0 ? AIM_ZOOM : IRON_ZOOM);
  // When this player last told the server anything. See the send below.
  let lastSent = -1e9;
  // Left to fire, right held to aim. Both while the mouse is captured, and
  // both at once: the whole point of holding the right button is to press the
  // left one while it is down.
  addEventListener("mousedown", e => {
    if (document.pointerLockElement !== canvas) return;
    if (e.button === 0) { wantShot = true; fireHeld = true; }
    if (e.button === 2) { wantAim = true; e.preventDefault(); }
  });
  addEventListener("mouseup", e => { if (e.button === 2) wantAim = false; if (e.button === 0) fireHeld = false; });
  // A right button that is held is not a request for a menu. The browser will
  // not open one over a locked pointer anyway, but it will the moment the lock
  // is lost — mid-aim, which is the worst possible moment for a menu.
  addEventListener("contextmenu", e => e.preventDefault());
  // Letting go of the pointer lets go of the sights: a button that is held
  // cannot report itself released once the page has stopped hearing about it.
  document.addEventListener("pointerlockchange", () => {
    if (document.pointerLockElement !== canvas) { wantAim = false; fireHeld = false; }
  });

  // The adaptive loop's bookkeeping. The display's own frame interval is the
  // budget — measured, not assumed, so a 120 Hz screen asks for more.
  const dts = [];
  let refresh = 1000 / 60, warm = 0, changes = 0, stable = 0, locked = false;
  const median = a => [...a].sort((x, y) => x - y)[a.length >> 1];

  // The sun's path across the sky, and how fast the world runs along it: one
  // turn in a minute and a half.
  //
  // This is a small planet. Not a decision about the terrain — the valley is
  // the size it always was — but about how long it takes to carry you round,
  // and that is the only way the size of a world is visible from standing on
  // it. A day and a night in ninety seconds means the sun comes up while you
  // are still working out where you are, and the whole cycle is something you
  // watch rather than something you wait for. It was four minutes, which was
  // long enough that most of a visit happened under one lighting. `shaders/common.wgsl` carries the same axis and the
  // same rate, because the stars and the planets wheel about the same pole.
  //
  // The pole is very nearly horizontal, so the sun's path is a great circle
  // that passes near enough overhead: it rises on one side, stands at the top
  // of the sky at midday, and sets on the other. That is what a day is on a
  // planet whose axis is not leaning much and whose observer is not far from
  // its equator — which is to say, it is Earth's, at the equinox.
  //
  // Turned about the vertical instead the sun would keep its height and the
  // evening would never end, which is what this did at first: a fair trade for
  // holding the golden hour, until it was obvious that a world where the sun
  // goes round and round at a fixed altitude is not a world with a sky, it is
  // a world with a lamp on a turntable. Then thirty-five degrees, which bought
  // a night but capped the sun a third of the way up, so it was permanently
  // late afternoon and never actually daytime.
  //
  // Eighty-two rather than a clean ninety for one arithmetic reason: at exactly
  // ninety the sun passes through the zenith, and a light straight overhead has
  // no horizontal component for the shadow matrix to build its basis from. Eight
  // degrees off is invisible and keeps that term away from zero. `light_vp`
  // guards it as well, because a constant here and an assumption there is how
  // this sort of thing comes back.
  const SKY_TILT = 82 * Math.PI / 180;
  // The axis, and the two directions that span the plane the sun runs round in.
  const SKY_AXIS = [Math.sin(SKY_TILT), Math.cos(SKY_TILT), 0];
  const SUN_E1 = [0, 0, 1];
  const SUN_E2 = [Math.cos(SKY_TILT), -Math.sin(SKY_TILT), 0];
  // Where in the day it starts: a few degrees below the horizon and climbing,
  // so the first thing that happens is sunrise. The sky is still dawn-blue when
  // the curtain lifts, the ridge lights up within a couple of seconds, and the
  // shadows swing out across the meadow while you are still working out which
  // way you are facing.
  //
  // π minus a tenth. y = -sin(θ) on this path, so θ = π is the horizon exactly
  // and cos(θ) < 0 there is what makes it the rising crossing rather than the
  // setting one — the same y, the other side of the day.
  const SUN_PHASE = Math.PI - 0.10;
  // A day, in seconds. Ninety was a good number for somebody who would look at
  // this for a minute; it is a poor one for somebody living in it, because the
  // shadows visibly crawl and the sun sets while you are crossing the meadow.
  // Six minutes is still nothing like a day and still shows a whole one inside
  // a session somebody stays in, and the sky has stopped being something you
  // can watch move — the light changes while you are busy and you notice it
  // afterwards, which is what daylight does.
  const SKY_SPIN = 2 * Math.PI / 360;
  // The sun's circle is lifted by half its amplitude, which is the geometry of
  // a wider world: on a planet fat enough, the day arc runs long and the sun
  // dips under the horizon for exactly a third of the turn instead of half.
  // sin(30°) of the amplitude is what makes the split 2/3 to 1/3 — solve
  // elevation = A·sin(θ) + A/2 > 0 and θ spends 240 of 360 degrees above.
  const SUN_LIFT = Math.sin(SKY_TILT) * 0.5;
  const uf = new Float32Array(UNI_FLOATS);
  let lastX = 0, lastZ = 0, speed = 0, bobPhase = 0;
  // Resolved by the frame loop the first time it actually submits, so `run`
  // does not return on a canvas that has nothing on it yet.
  let painted = null;
  const firstFrame = new Promise(r => { painted = r; });

  // What the page actually pulled down, asked of the browser rather than
  // written down here. `decodedBodySize` is the asset as the decoder sees it,
  // `transferSize` is what crossed the wire — the gap between them is what
  // gzip and brotli did, and it is the only honest way to report it: a number
  // typed into a comment goes stale the first time a texture is re-encoded.
  //
  // Recomputed on each HUD tick rather than cached, because it costs a walk
  // over forty entries twice a second and it catches anything that loads late.
  const IMG = /\.(jpg|png)(\?|$)/;
  function weights() {
    let raw = 0, wire = 0, tex = 0;
    for (const e of performance.getEntriesByType("resource")) {
      raw += e.decodedBodySize || 0;
      wire += e.transferSize || 0;
      if (IMG.test(e.name)) tex += e.decodedBodySize || 0;
    }
    return { raw, wire, tex };
  }

  let last = performance.now(), fps = 0, frames = 0, fpsT = last, startT = 0;

  function tick(now) {
    // The device died. Every command from here is discarded, so the only thing
    // left to do is stop asking for frames and leave the message on the screen.
    if (gone) return;
    resize();
    const dt = Math.min((now - last) / 1000, 0.05);
    const dtMs = now - last;
    last = now;
    // The world's clock starts at the first frame, not at the first byte.
    //
    // It was `performance.now()`, which begins when the page does — so the ten
    // seconds of loading were ten seconds of the day going by, and where the
    // sun stood when anyone first saw the valley depended on how fast their
    // connection was. Anchored here, the opening is the opening.
    if (!startT) startT = now;
    const t = (now - startT) / 1000;

    // Settle the tier: warm up, measure, step, lock.
    warm++;
    if (warm > 20) {
      dts.push(dtMs);
      refresh = Math.min(refresh, Math.max(dtMs, 4));
      if (!locked && dts.length >= 48) {
        const med = median(dts);
        dts.length = 0;
        if (med < refresh * 1.25 && tierIdx < MAX_TIER) { tierIdx++; changes++; buildWorld(tierIdx); }
        else if (med > refresh * 1.6 && tierIdx > 0) { tierIdx--; changes++; buildWorld(tierIdx); }
        else stable++;
        if (changes >= 3 || stable >= 2) locked = true;
      }
    }

    // The shot, before the frame that will fly it. Only with the bolt closed,
    // and only on a press: holding the button does nothing, because `wantShot`
    // is set by the event rather than read from the state.
    boltT = Math.min(boltT + dt, cycleNow());
    // Say so on the page. The bolt itself is the honest indicator, but it is
    // off to the right of where anyone aiming is looking, so the crosshair
    // goes quiet while the rifle is open and comes back when it will fire.
    const ready = boltT >= Math.min(cycleNow(), BOLT_CYCLE);
    if (ready !== wasReady) { document.body.classList.toggle("cycling", !ready); wasReady = ready; }
    // Nothing fires from in the lake. Refused here rather than inside `fire`
    // so the bolt does not cycle and the bang does not play: a rifle that goes
    // off and does nothing is worse than one that will not go off.
    // Space starts again once it is over. The same key jumps while you are
    // alive, which is why this is gated on `ended` rather than being a key of
    // its own: at the end of it there is nothing to jump for, and the one key
    // anyone presses when a screen says GAME OVER is the big one.
    // Only alone. In somebody else's valley there is nothing here to restart:
    // the crystals are the server's and it is the one that decides whether a
    // valley that has run out of them begins again.
    if (ended && wantRestart && !net) {
      // A new valley deserves a new corner of it. `restart` resets the world's
      // clock along with everything else, so without a fresh roll the second
      // run would begin exactly where the first one did.
      spawnRoll = Math.random() * 4096;
      E.spawn_salt(state, spawnRoll);
      E.restart(state);
      ended = false;
      lastHp = -1; lastStanding = -1;
      breaks = Number(E.breaks(state));
      blastsHeard = Number(E.blasts_fired(state));
      zapSeen = Number(E.zaps(state));
      hudHead(HH.over, 0);
    }
    wantRestart = false;

    // Nothing fires from in the lake — and it says so. A control that silently
    // does nothing is indistinguishable from a broken one: you pull the
    // trigger, get no bolt, no bang and no round, and the only conclusion
    // available is that the game has stopped working. One line turns a fault
    // into a rule.
    const wading = Number(E.wading(state));
    if (wantShot && wading) sayWarn("you cannot fire from the water");

    // An empty rifle is the same kind of fault as one held under water: the
    // trigger is pulled and nothing happens, and without a word for it the only
    // conclusion available is that the game has stopped working. `fire` refuses
    // on its own — it is the one gate both the browser and the server come
    // through — but the bang, the recoil, the bolt and the message to the
    // server all live out here and were happening anyway, so a rifle with
    // nothing in it went off in every way except the one that counted.
    // Asked for by hand, before the counts below are read — so the number on
    // the screen is the one the key just made rather than the one it replaced.
    //
    // The world answers whether it took. A full magazine, no spare on the belt,
    // the wrong weapon up, in the water, on the ground, or the hands already
    // busy all come back as no, and then the key does nothing at all: no sound,
    // no animation, and nothing sent. Starting a change that is not happening
    // is worse than a key that appears dead, because the second one is honest.
    if (wantReload) {
      wantReload = false;
      if (held === 2 && Number(E.ask_reload(state))) {
        boltT = cycleNow() - CHANGE_SECS;
        akReload();
        reloadHold = performance.now() + 900;
        changeAt = t;
        // And the same request to the server, whose copy of the magazine is
        // the one that decides whether a round leaves. It runs the same
        // `ask_reload`, so the two cannot disagree about whether it took.
        if (net) net.reloaded();
      }
    }
    const loaded = held === 1 ? Number(E.rockets(state))
                 : held === 2 ? Number(E.ak_rounds(state))
                 : Number(E.rounds(state));
    const akMags = Number(E.ak_mags(state));
    // The change, seen from here: the world swapped magazines on a pull, the
    // count jumped from nothing back to thirty, and the crosshair owes the
    // second and a half the hands are busy.
    if (held === 2 && lastAkLoaded === 0 && loaded > 0) { boltT = cycleNow() - CHANGE_SECS; akReload(); reloadHold = performance.now() + 900; changeAt = t; }
    lastAkLoaded = held === 2 ? loaded : -1;
    // The AK is automatic: while the trigger is held and there is anything in
    // the magazine, every twentieth millisecond is a shot. The press itself
    // set wantShot; this keeps setting it for as long as the button is down.
    if (held === 2 && fireHeld && !wading && (loaded > 0 || akMags > 0) && boltT >= cycleNow()) wantShot = true;
    // The burst ends when the trigger lifts or the magazine does. The count
    // is what the climb and the cone are measured from, so it resets here.
    if (akBurst > 0 && (!fireHeld || loaded === 0 || held !== 2)) akBurst = 0;
    if (wantShot && boltT >= cycleNow() && !wading && loaded === 0 && !(held === 2 && akMags > 0)) {
      sayWarn(held === 1 ? "the tube is empty" : "the magazine is empty");
      dryClick();
    } else if (wantShot && boltT >= cycleNow() && !wading) {
      const st0 = stateF32();
      const yaw = st0[19], pit = st0[20], cp = Math.cos(pit);
      let dx = Math.sin(yaw) * cp, dy = Math.sin(pit), dz = -Math.cos(yaw) * cp;
      // The AK does not shoot down a line. Every round leaves a little wide of
      // the last, and the longer the trigger is held the wider it walks — a
      // cone that opens from about two thirds of a degree to two and a half
      // over a magazine. The scattered direction is the one that crosses the
      // wire, so everybody sees the round go where it actually went.
      if (held === 2) {
        // Per round, not per second — so when the rate doubled these had to
        // halve to keep the cone opening at the same speed on a clock. It
        // still reaches two and a half degrees, and it still takes about two
        // and a half seconds of held trigger to get there; there are simply
        // twice as many rounds inside that.
        const spread = 0.007 + Math.min(akBurst, 24) * 0.00035;
        dx += (Math.random() * 2 - 1) * spread;
        dy += (Math.random() * 2 - 1) * spread;
        dz += (Math.random() * 2 - 1) * spread;
        const l = Math.hypot(dx, dy, dz);
        dx /= l; dy /= l; dz /= l;
      }
      // Fired here for the tracer and the bang, and asked for there for what it
      // hits. A rifle whose report waits for a round trip is a rifle that fires
      // when you have stopped pulling the trigger; a round that decides what it
      // hit locally is a client deciding whether it shot somebody. So both, and
      // the server's copy of this round is dropped on the way back in.
      // An empty magazine turns this pull into the change rather than a
      // shot — the world decides that inside `fire` — so the report below
      // belongs to the rounds that leave and not to the magazine that
      // arrives.
      const changing = held === 2 && loaded === 0;
      E.fire(state, dx, dy, dz);
      if (net) net.fired(dx, dy, dz);
      boltT = 0;
      // What the *model* does, as against what the aim does. The rifle's own
      // 0.11 radians and five centimetres were measured on a gun fired once
      // every second and a half; at five a second they swing the AK's muzzle
      // clean out of the frame and take the sights with it. A third of that
      // still reads as a kick and leaves the barrel where it can be seen.
      //
      // Half rather than a third now, with the climb. What the shoulder does
      // and what the gun does have to agree — an aim that walks thirteen
      // degrees up a magazine over a barrel that barely stirs reads as the
      // screen drifting rather than as a rifle being fired. It does not go
      // to the rifle's whole amount: the decay is most of it gone in a fifth
      // of a second and rounds arrive every tenth, so the AK's jolt stacks
      // on itself in a way a bolt gun's never does.
      // The climb the aim takes is separate — that is `kickUp`, below.
      recoil = held === 2 ? 0.5 : 1;
      // And the flash, which is not the model's business and is lit whole
      // whatever the weapon damps its kick to. The magazine change is not a
      // shot and nothing comes out of the muzzle for it.
      if (!changing) flash = 1;
      if (held === 2 && !changing) {
        akBang();
        akBurst++;
        // The climb. Not a wobble painted on the screen — it is fed to the
        // same look the mouse drives, so the muzzle really is pointing higher
        // afterwards and the shot after it goes where the barrel now is. The
        // first rounds of a burst climb hardest and it eases off as the
        // shoulder catches up, which is what a held trigger actually does.
        //
        // These were halved when the rate doubled, to hold the climb per
        // second where it had been tuned. That was the cautious answer and it
        // was the wrong one: a gun that empties a magazine in three seconds
        // and asks no more of the shoulder than one that took six does not
        // feel like it is doing anything. Back to what they were.
        //
        // In pixels of the same look the mouse drives, so 0.0018 radians
        // each. Over a magazine, ease and all, that is 127 pixels — the
        // muzzle climbs about thirteen degrees across thirty rounds, against
        // six and a half before. It is fought by pulling down, and three
        // seconds is long enough to learn to.
        //
        // The ease keeps its stretched shape. It eases off over sixteen
        // rounds rather than eight because what the shoulder catches up with
        // is a span of time, not a count, and sixteen rounds at ten a second
        // is the second and a half that eight at five used to be.
        const ease = 1 - Math.min(akBurst, 16) / 32;
        kickUp += (5.5 + Math.random() * 2.5) * ease;
        kickSide += (Math.random() * 2 - 1) * 2.6;
      } else if (held === 1) rpgBang();
      // The bolt rifle, named rather than left as the end of a chain. It was
      // `else`, and that `else` had a hole in it: the pull that changes an
      // AK's magazine is not a shot, so it failed the first test above, and
      // `held === 1` is false for an AK — so it fell through to here and
      // fired the *rifle's* report out of an AK. Once a magazine, which at
      // five rounds a second was every six seconds and easy to miss; at ten
      // it is every three and quite plain.
      //
      // The change has a sound of its own and it is played elsewhere, off the
      // magazine count rather than off the trigger — see `akReload` above. So
      // there is nothing to play here, and the answer is to say which weapon
      // this branch belongs to instead of catching whatever is left.
      else if (held === 0) bang();
    }
    wantShot = false;
    recoil *= Math.pow(0.02, dt);   // most of it gone in a fifth of a second
    // And the flash on its own clock. The same curve it always had — the
    // recoil's seventh power, written here as the seventh of the same decay —
    // but lit at one for every weapon rather than at whatever that weapon
    // damps its model kick to. See `fx` in uniforms.wgsl for what the old way
    // did to the AK.
    flash *= Math.pow(0.02, 7 * dt);

    // Changing hands. Down over `SWAP_SECS`, over at the bottom, up again — so
    // the weapon that is going away is the one that is drawn on the way down
    // and the new one is drawn on the way up. Firing is refused throughout by
    // the bolt clock, which is set to the new weapon's cycle at the changeover.
    if (swapping === 0 && wantHeld !== held) { swapping = 1; }
    if (swapping === 1) {
      swap = Math.min(1, swap + dt / SWAP_SECS);
      if (swap >= 1) {
        held = wantHeld;
        E.set_weapon(state, BigInt(held));
        // A moment to be ready, not a whole reload. Setting this to nought
        // meant raising the launcher cost the two and a half seconds it takes
        // to load one — you stood there with a visibly empty tube, having
        // changed weapons rather than fired. What a swap costs is the third of
        // a second it takes to get the thing up.
        boltT = Math.max(0, cycleNow() - 0.35);
        swapping = 2;
      }
    } else if (swapping === 2) {
      swap = Math.max(0, swap - dt / SWAP_SECS);
      if (swap <= 0) swapping = 0;
    }

    // The sights, eased both ways and framerate independent so they take the
    // same time on every screen.
    //
    // Nine tenths of the way in about seventy milliseconds and settled inside
    // an eighth of a second, measured at sixty and at a hundred and
    // forty-four. That is about as fast as a rifle can be brought up and is
    // what the hand expects; the ease that was here before took four times as
    // long and felt like the game thinking about it. Still an ease and not a
    // cut — the field of view has to travel, or it reads as an edit — but the
    // travelling is over before it is noticed.
    aim += ((wantAim && E.health(state) > 0.001 ? 1 : 0) - aim) * (1 - Math.exp(-dt / 0.024));
    if (aim < 0.001) aim = 0;
    const zoom = 1 + (zoomNow() - 1) * aim;
    E.set_zoom(state, zoom);
    // And the mouse slows with it, or the sights are unusable: the same
    // movement of the hand has to cover the same angle on the screen, and the
    // screen is showing a narrower slice of the world.
    mdx /= zoom; mdy /= zoom;
    // The shooter's own hand, before the gun's kick is mixed in, and what it
    // does to what the recoil owes.
    //
    // The climb moves the aim and it is meant to: the round after it goes
    // where the barrel now is. But a rifle does not stay pointing at the sky
    // once the trigger is let go, and this one did — a burst left the muzzle
    // thirteen degrees up and there it stayed until you pulled it down
    // yourself. So the climb is now a debt: taken while firing, given back
    // when the firing stops.
    //
    // The hand pays first, and this is the part that cannot be left out. A
    // shooter who rides a burst down onto the target has already returned
    // what the recoil took; if the settle below then also ran, letting go of
    // the trigger would sink the muzzle *below* where the burst began — the
    // gun would fight you for having fought it. So downward movement clears
    // the debt, and only as far as nought: a pull further than the recoil
    // pushed is aiming, not correcting.
    if (mdy > 0 && owedUp > 0) owedUp = Math.max(0, owedUp - mdy);
    if (owedSide > 0 && mdx < 0) owedSide = Math.max(0, owedSide + mdx);
    else if (owedSide < 0 && mdx > 0) owedSide = Math.min(0, owedSide + mdx);
    // And the weapon's own kick on top, in the same pixels the mouse speaks —
    // after the zoom, because a scope magnifies the world and not the rifle's
    // objection to being fired. `mdy` is positive downward, so the climb is a
    // subtraction.
    mdy -= kickUp; mdx += kickSide;
    owedUp += kickUp; owedSide += kickSide;
    kickUp = 0; kickSide = 0;
    // Settling back, once the burst is over. Not between rounds: at ten a
    // second a settle that ran in the gaps would give the climb back as fast
    // as it was made, which is the same as not having a climb. An empty
    // magazine counts as over — the hands are busy for a second and a half
    // and the muzzle comes down while they are.
    //
    // A hundredth left after a second, so two thirds of it is back in a
    // quarter of a second and it is done inside half of one. Framerate
    // independent, like the sights and the recoil, or the gun settles at a
    // different speed on a different screen.
    if (!(held === 2 && fireHeld && loaded > 0) && (owedUp !== 0 || owedSide !== 0)) {
      const back = 1 - Math.pow(0.01, dt);
      const giveUp = owedUp * back, giveSide = owedSide * back;
      owedUp -= giveUp; owedSide -= giveSide;
      mdy += giveUp; mdx -= giveSide;
    }

    if (net) {
      // Sixty times a second at most, whatever the screen is doing. That is
      // already more often than the server ticks — it walks from the last
      // thing it heard, so an input arriving between two ticks is the one that
      // counts at the next — and on a hundred-and-twenty hertz screen, one a
      // frame is sixty wasted messages a second.
      //
      // Worse than wasted. Over the server's allowance its reader stops
      // listening until the second is up, and everything sent after that waits
      // behind what did not get read: a player on a fast screen watched their
      // own round trip climb past half a minute, which is the backlog and not
      // the network. Twice the tick rate is plenty and leaves room under the
      // allowance for the shots and the pings.
      if (now - lastSent >= 15) {
        lastSent = now;
        const look = stateF32();
        // The eighth bit is what is in the hands, not a key. It rides in the
        // same integer because the message is already being sent and what a
        // player is holding is part of what they are doing this frame; the
        // server masks the keys off it and reads the bit as the weapon. A
        // server old enough not to know about it drops it and gets the rifle.
        // And the tenth is the lamp, on the same reasoning: state rather than
        // a press, so a dropped packet costs a frame of light and not a round
        // of the two ends disagreeing about whether it is on.
        net.input(keys | (held << 7) | (torchOn ? 512 : 0), look[19], look[20]);
      }
      applyEvents(net.take());
      // Walked here as well as there, so that turning and walking answer the
      // key rather than the round trip. What the server made of it arrives a
      // moment later and is eased in.
      E.walk(state, dt, mdx, mdy, BigInt(keys));
      // Everything moves; nothing here decides. What a round hit, what a
      // crater cost and which crystal went are the server's, and have already
      // been applied above.
      E.step_visuals(world, dt);
      // Swept up but not healed by. What a chip is worth is the server's to
      // give, and it arrives in the next snapshot; doing it here as well meant
      // the snapshot's own figure — from before the pickup — came back as a
      // drop, and a drop is how the world decides somebody has been shot. Every
      // chip threw the player's head.
      E.pick_up_quiet(state);
      reconcile();
      // The shark is the server's too, and it only speaks twenty times a
      // second — so it is drawn at a point between the last two, like a
      // player.
      const sk = sharkNow(net, now);
      if (sk) E.set_shark(world, sk.x, sk.y, sk.z, sk.heading, sk.mode, sk.mode > 2.5 ? 0.0 : (sk.mode > 0.5 ? 0.85 : 0.0));
      E.look(state, canvas.width / canvas.height);
    } else {
      E.frame(state, dt, mdx, mdy, BigInt(keys), canvas.width / canvas.height);
    }
    mdx = 0; mdy = 0;

    // The other people, as instances: where they are, how big, which way they
    // are facing, and their colour. Written every frame because it is a
    // kilobyte and they are always moving.
    if (net) {
      const seen = others(net, now);
      figureCount = Math.min(seen.length, FIGURE_CAP);
      for (let i = 0; i < figureCount; i++) {
        const p = seen[i], o = i * 8;
        const c = hueOf(p.id, p.health);
        figures[o] = p.x; figures[o + 1] = p.y; figures[o + 2] = p.z;
        figures[o + 3] = 1;
        figures[o + 4] = p.yaw;
        figures[o + 5] = c[0]; figures[o + 6] = c[1]; figures[o + 7] = c[2];
        const clock = clipFor(p, now);
        const clip = figure.clips.get(clock.name) ?? figure.clips.get("idle");
        // The armed idle runs on its own clock rather than on the walk's, so
        // the rifle keeps breathing at its own rate instead of bobbing once
        // per footfall.
        const over = LEGS_ONLY.has(clock.name) ? figure.clips.get("idle") : null;
        skinner.pose(i, clip, clock.at, p.x, p.y, p.z, p.yaw, over, now / 1000);
        // Whoever is down has let go of it. The die clip puts the hand on the
        // ground and the rifle would have followed it there still gripped,
        // which is a body holding its weapon at attention while lying dead.
        if (p.health < 0.001) skinner.drop(i, p.x, p.y, p.z, p.yaw, 0.7 + (p.id % 5) * 0.4);
      }
      // And the same positions again, into the world this time, so that a
      // round stops at the body it hit instead of carrying on through it.
      // Only the drawing knows where anybody is on a client — everybody but
      // this player arrives as a snapshot — so this is the only place that
      // can say. This player goes in as well: a round of somebody else's
      // ought to stop at their chest too, even if the chest is the camera.
      for (let i = 0; i < figureCount; i++) {
        const p = seen[i];
        E.set_ghost(world, BigInt(i), p.x, p.y, p.z);
      }
      if (figureCount < FIGURE_CAP) {
        E.set_ghost(world, BigInt(figureCount), E.player_x(state), E.player_y(state), E.player_z(state));
        E.set_ghosts(world, BigInt(figureCount + 1));
      } else E.set_ghosts(world, BigInt(figureCount));

      // Whoever has gone is forgotten, or the map grows for the life of the
      // page in a valley people keep passing through.
      if (clocks.size > seen.length) {
        const here = new Set(seen.map(p => p.id));
        for (const id of clocks.keys()) if (!here.has(id)) clocks.delete(id);
      }
      // And this player's own, once they are on the ground: the only rifle in
      // the valley with nobody to hang it off, drawn after the others in the
      // spare slot the skinner keeps for it.
      heldCount = figureCount;
      if (E.health(state) <= 0.001) {
        skinner.drop(figureCount, E.player_x(state), E.player_y(state), E.player_z(state), stateF32()[19], 1.1);
        heldCount = figureCount + 1;
      }
      if (figureCount) {
        device.queue.writeBuffer(figureInst, 0, figures, 0, figureCount * 8);
        device.queue.writeBuffer(poseBuf, 0, skinner.matrices, 0, figureCount * figure.joints * 16);
      }
      if (heldCount) device.queue.writeBuffer(heldBuf, 0, skinner.held, 0, heldCount * 16);
      drawTags(seen, stateF32());
      drawRoster(everyone(net));
    }

    // The socket went. Said in the middle and in the largest letters on the
    // page, because everything else carries on looking fine — the valley is
    // still drawn, the wind still blows, this player still walks — and a
    // browser quietly playing on its own in a world nobody else can see is a
    // worse thing than a stopped one. Nothing is torn down: what is on screen
    // is the last thing everybody agreed about, and it stays there.
    // The crosshair fades as the scope comes up: two sights at once is one
    // more than anybody needs, and the rifle's own is the one that is aimed.
    // The page's crosshair is for the hip. It steps back as the rifle comes up
    // and is gone by the time the scope has the screen, which has a reticle of
    // its own and no room for a second opinion about where the middle is.


    // The socket went. The engine keeps the caption up; this only raises it.
    if (net && net.lost && lastOver !== 2) {
      lastOver = 2;
      hudSlot(HSLOT.over1, [[-1, "DISCONNECTED"]]);
      hudSlot(HSLOT.over2, [[-1, "THE VALLEY WENT ON WITHOUT YOU"]]);
      hudSlot(HSLOT.over3, [[-1, "RELOAD TO GO BACK"]]);
      hudHead(HH.over, 2);
    }

    // Down, and how long for. The clock is Rill's — the same one the server
    // counts against to decide when somebody may stand up — so the number on
    // the screen and the moment they actually get up are the same fact rather
    // than two timers that agree for a while.
    // Your own, which no snapshot tells you about: the same edge, off the
    // block you are walking. Alone in the valley as well as in company, and
    // the camera is thrown when you go down so there is something to see.
    const downNow = E.health(state) <= 0.001;
    if (downNow && !wasDown && world) {
      E.gib_at(world, E.player_x(state), E.player_y(state), E.player_z(state),
               E.player_yaw(state), BigInt(E.player_id(state)));
    }
    wasDown = downNow;

    // Anything break in there? Rill counts them; the host only has to notice.
    // Not `>`: a tier change rebuilds the world and puts the tally back to
    // zero, and a mirror that only ever climbs would go deaf until it caught
    // up again.
    // The roar of whatever is in the air. Air swallows the top of a sound
    // before it swallows the bottom, so the filter opens as a round gets
    // closer: near ones hiss, far ones are only a rumble. That is most of what
    // makes distance audible, more than loudness is.
    if (roars) {
      const fb = new Float32Array(E.memory.buffer, Number(E.fireballs(state)), fireCap * 9);
      const flying = [];
      for (let i = 0; i < fireCap; i++) {
        if (fb[i * 9 + 7] > 0) flying.push(place(fb[i * 9], fb[i * 9 + 1], fb[i * 9 + 2], 34));
      }
      flying.sort((a, b) => a.d - b.d);
      const now = audio.currentTime;
      for (let v = 0; v < roars.length; v++) {
        const f = flying[v];
        // Nothing at all at the far end, and climbing steeply as it closes.
        //
        // Not the inverse-distance curve the impacts and the crystals use. That
        // one never reaches zero, so a round launched forty-five metres out is
        // already faintly audible the instant it leaves, and three of them at
        // that range are a constant hiss with nothing to attach it to. Silence
        // is the whole point of a sound that arrives: you notice a meteor
        // because you did not hear it a moment ago.
        //
        // Squared, so almost all of the loudness is spent in the last fifteen
        // metres — which is also the physically honest shape, being what an
        // inverse-square law looks like once you have said where it ends.
        const k = f ? Math.max(0, 1 - f.d / REACH) : 0;
        const near = k * k * k;
        roars[v].gain.gain.setTargetAtTime(0.62 * near, now, 0.10);
        if (f) {
          roars[v].pan.pan.setTargetAtTime(f.pan, now, 0.10);
          roars[v].filt.frequency.setTargetAtTime(500 + 5200 * near, now, 0.15);
        }
      }
    }

    // Anything land? Same trick as the breaks: Rill counts, the host notices,
    // and the newest entry in the ring says where.
    const nbl = Number(E.blasts_fired(state));
    if (nbl !== blastsHeard) {
      const grew = nbl > blastsHeard;
      blastsHeard = nbl;
      if (grew) {
        const b = new Float32Array(E.memory.buffer, Number(E.blasts(state)), blastCap * 8);
        const at = ((nbl - 1 + blastCap) % blastCap) * 8;
        boom(b[at], b[at + 1], b[at + 2]);
      }
    }

    const nb = Number(E.breaks(state));
    if (nb !== breaks) {
      const broke = nb > breaks;
      breaks = nb;
      if (broke) shatter(E.break_x(state), E.break_y(state), E.break_z(state));
    }

    // The valley is on a planet and the planet turns. Around the world's own
    // axis rather than along a tilted one, which keeps the sun's height fixed
    // and so keeps the hour: the light swings round the valley all evening and
    // it stays evening. A tilted axis would be more honest and would put the
    // sun in the ground within ten minutes, taking the whole look with it.
    //
    // One turn in a hundred and eighty seconds — two degrees a second, which is
    // fast enough to watch a shadow travel and slow enough not to strobe.
    //
    // The clock it runs on is the *world's*, not the page's. In a shared valley
    // that clock belongs to the server and arrives in every snapshot, which is
    // the only way two people standing next to each other agree about what time
    // of day it is — on the page's own clock they each ran their own sunset,
    // and one of them was shooting into a glare the other could not see. It is
    // the same number the wind and the water already ride on, so this is one
    // clock for the whole sky rather than a second one beside it.
    //
    // Alone in a valley there is no server and it is this browser's own count
    // of the frames it has drawn, which is what it always was: a day does not
    // pass while a tab is hidden and nobody is watching it.
    const sky = E.world_time(world);
    // The crystals' bites. The counter moves — whether this world decided the
    // bite (alone) or was told about one (`z`) — and the light and the sound
    // follow from the ring: which crystal, in its own colour, to whom.
    const nz = Number(E.zaps(state));
    if (nz !== zapSeen) {
      const cryF = new Float32Array(E.memory.buffer, Number(E.crystals(state)), Number(E.crystal_count(state)) * 8);
      for (let k = Math.max(zapSeen, nz - 8); k < nz; k++) {
        const zwho = Number(E.zap_who(state, BigInt(k)));
        const zci = Number(E.zap_which(state, BigInt(k)));
        const ca = zci * 8;
        let tx, ty, tz;
        const sf = stateF32();
        // Your foot, not your chest and certainly not your eye. Ending it at
        // the camera put the last stretch of the bolt behind the near plane,
        // so a hit you were not already looking at arrived as damage with
        // nothing on screen; the chest fixed that and left the bolt reaching
        // out of the middle of the view, from nowhere, at nothing.
        //
        // The ground is where it belongs. A discharge goes to earth, and what
        // you are standing on is the earth — so it runs from the crystal down
        // to where you are touching it, crosses the whole bottom of the view
        // on the way, and reads as something arriving rather than something
        // emitted. Slot 22 is the foot the world walks on, jump and all, so
        // this is right in the air as well as on the ground.
        if (!net || zwho === net.id) { tx = sf[16]; ty = sf[22] + 0.12; tz = sf[18]; }
        else {
          const q = net.last && net.last.players.find(p => p.id === zwho && p.seen);
          if (!q) continue;
          tx = q.x; ty = q.y + 0.12; tz = q.z;
        }
        zapsLive.push({ ax: cryF[ca], ay: cryF[ca + 1] + cryF[ca + 3] * 1.05, az: cryF[ca + 2],
                        bx: tx, by: ty, bz: tz,
                        r: cryF[ca + 5], g: cryF[ca + 6], b: cryF[ca + 7], born: sky });
        if (zapsLive.length > ZAP_MAX) zapsLive.shift();
        const pl = place(cryF[ca], cryF[ca + 1], cryF[ca + 2], 10);
        ensureAudio(() => playAt(zapSndBuf, 0, 0, 0.15 + 0.8 * pl.gain, pl.pan, 0.96 + Math.random() * 0.1));
      }
      zapSeen = nz;
    }
    // Where the sun is now: a point on the tilted circle. What goes into the
    // uniform and into the shadow matrix is the direction the light travels,
    // which is the other way round from where the sun is.
    const th = SUN_PHASE + sky * SKY_SPIN;
    const cs = Math.cos(th), sn = Math.sin(th);
    let sunX = -(SUN_E1[0] * cs + SUN_E2[0] * sn);
    let sunY = -(SUN_E1[1] * cs + SUN_E2[1] * sn) - SUN_LIFT;
    let sunZ = -(SUN_E1[2] * cs + SUN_E2[2] * sn);
    // Lifted, so no longer unit; everything downstream — the shadow basis,
    // the sky, the fog — assumes a direction of length one.
    const sl = Math.hypot(sunX, sunY, sunZ);
    sunX /= sl; sunY /= sl; sunZ /= sl;
    // Into the shadow matrix first, so what is copied out of the state below is
    // this frame's sun and not the last one's. There is one sun in this world:
    // the disc in the sky, the direction `lit` shades from and the matrix the
    // shadows are cast with are all the same three numbers.
    E.set_sun(state, sunX, sunY, sunZ);

    const st = stateF32();
    uf.set(st.subarray(0, 16), 0);        // vp
    uf.set(st.subarray(32, 48), 16);      // light vp
    uf.set(st.subarray(16, 19), 32);      // eye
    uf[35] = sky;
    uf[36] = sunX; uf[37] = sunY; uf[38] = sunZ;
    uf[39] = canvas.width / canvas.height;
    uf[40] = st[19]; uf[41] = st[20];     // yaw, pitch
    // Off the *world*, not the player. `step_wind` writes the valley's weather
    // into the world block, and these read slots 26, 27 and 29 of the player —
    // which are not the wind and are not anything, so every shader that sways
    // has been given an angle of zero, a strength of zero and a phase that
    // never moves. The pines stood still, the grass stood still, the water lay
    // flat, and the smoke went nowhere. One letter's worth of difference, and
    // it was there for as long as the wind has been.
    // (`wind` is taken further down for the player's breath, which is a
    // different thing wearing the same word.)
    const weather = new Float32Array(E.memory.buffer, Number(world), 32);
    uf[42] = weather[26]; uf[43] = weather[27]; // wind angle, wind strength
    uf[44] = weather[29];                       // the wind's integrated phase
    // How fast the player is going, and a phase integrated from it. The gun's
    // bob reads both: the speed sets how far it swings, the phase where it is
    // in the swing — a bob driven by the clock alone jumps whenever the speed
    // changes, which is every time you start or stop walking.
    const fx = st[21], fz = st[23];
    const inst = Math.hypot(fx - lastX, fz - lastZ) / Math.max(dt, 0.0001);
    lastX = fx; lastZ = fz;
    speed += (Math.min(inst, 12) - speed) * 0.15;
    bobPhase += speed * dt * 2.4;
    uf[45] = speed; uf[46] = bobPhase; uf[47] = recoil;
    uf[116] = flash;                      // and the muzzle's own, see `fx`
    uf.set(rifleBox.subarray(0, 3), 48);  // the rifle's bounding box, to unquantize against
    uf.set(rifleBox.subarray(3, 6), 52);
    uf.set(akBox.subarray(0, 3), 108);    // and the AK's, for its own pipeline
    uf.set(akBox.subarray(3, 6), 112);
    // How far through a magazine change the hands are, in the box's spare
    // fourth slot — nought when there is nothing going on, which is what the
    // shader tests. Floored a hair above nought on the first frame so that
    // "just started" is never mistaken for "not happening"; nothing has moved
    // at that phase anyway.
    const cph = (t - changeAt) / CHANGE_SECS;
    uf[111] = (held === 2 && cph >= 0 && cph < 1) ? Math.max(cph, 1e-4) : 0;
    // The shark: which way it faces, where the tail is in its beat, how wide
    // the jaw is, and where it is. Straight out of Rill — the renderer is told
    // where the animal is, it does not decide.
    const wf = new Float32Array(E.memory.buffer, Number(world), 96);
    uf[56] = wf[89];                      // heading
    uf[58] = wf[92];                      // where the tail is in its beat
    uf[59] = wf[93];                      // how wide the jaw is
    uf[60] = wf[86]; uf[61] = wf[87]; uf[62] = wf[88];
    uf.set(sharkBox.subarray(0, 3), 64);
    uf.set(sharkBox.subarray(3, 6), 68);
    uf.set(propBox.subarray(0, 3), 76);   // and the ammunition box's
    uf.set(propBox.subarray(3, 6), 80);
    uf.set(rocket.box.subarray(0, 3), 84);
    uf.set(rocket.box.subarray(3, 6), 88);
    uf.set(rpg.box.subarray(0, 3), 92);
    uf.set(rpg.box.subarray(3, 6), 96);
    uf.set(crateBox.subarray(0, 3), 100); // and the rocket crate's
    uf.set(crateBox.subarray(3, 6), 104);
    // How far through a weapon swap the hands are. One number for both, since
    // only one weapon is drawn: it runs to one as what is held goes down, the
    // draw changes over at the bottom, and it runs back to nought.
    uf[95] = swap;
    // The bolt's throw, as a fraction. The shot itself is the first instant of
    // it, so the recoil and the lift start together the way they do in a hand.
    uf[51] = held === 2 ? 0 : boltT / BOLT_CYCLE;   // the AK has no bolt to work
    uf[55] = aim;                         // how far behind the sights

    // This player's lamp, and then everybody else's whose is lit.
    //
    // Nearest first and only six, because the beams are summed per fragment
    // over the whole world and a valley of thirty-two people would be paying
    // for lights that are a hundred metres behind them. The ones that are cut
    // are the ones furthest from this eye, which are also the ones whose light
    // is most likely already past its twenty-six metre reach.
    //
    // A lamp is at the eye and not at the feet: 1.75 above the ground, the
    // same figure `place_player` uses, so a beam leaves somebody's head rather
    // than their boots. The direction is their own yaw and pitch, built the
    // way `camToWorld` builds this player's.
    uf[120] = torchOn ? 1 : 0;
    uf.fill(0, 124, 124 + LAMP_CAP * 8);
    if (net) {
      const lit = others(net, performance.now()).filter(p => p.torch && p.health > 0.001);
      if (lit.length > LAMP_CAP) {
        const ex = st[16], ey = st[17], ez = st[18];
        lit.sort((a, b) => ((a.x - ex) ** 2 + (a.y - ey) ** 2 + (a.z - ez) ** 2)
                         - ((b.x - ex) ** 2 + (b.y - ey) ** 2 + (b.z - ez) ** 2));
      }
      for (let i = 0; i < Math.min(lit.length, LAMP_CAP); i++) {
        const p = lit[i], o = 124 + i * 8, cp = Math.cos(p.pitch);
        uf[o] = p.x; uf[o + 1] = p.y + 1.75; uf[o + 2] = p.z; uf[o + 3] = 1;
        uf[o + 4] = Math.sin(p.yaw) * cp;
        uf[o + 5] = Math.sin(p.pitch);
        uf[o + 6] = -Math.cos(p.yaw) * cp;
      }
    }

    // The impact that is lighting the valley: the youngest one still burning.
    // Rill keeps the ages, so this is a scan of eight floats and not a clock of
    // the host's own — which matters on the frame after a restart, when every
    // blast is cleared and a host-side timer would still be counting one down.
    {
      const bl = new Float32Array(E.memory.buffer, Number(E.blasts(state)), blastCap * 8);
      let best = -1;
      for (let i = 0; i < blastCap; i++) {
        const a = bl[i * 8 + 4];
        if (a >= 0 && a < 2.6 && (best < 0 || a < bl[best * 8 + 4])) best = i;
      }
      if (best < 0) { uf[75] = -1; }
      else {
        uf[72] = bl[best * 8]; uf[73] = bl[best * 8 + 1]; uf[74] = bl[best * 8 + 2];
        uf[75] = bl[best * 8 + 4];
      }
    }
    device.queue.writeBuffer(uni, 0, uf);
    device.queue.writeBuffer(bulletBuf, 0,
      new Float32Array(E.memory.buffer, Number(E.bullets(state)), bulletCap * 8));
    device.queue.writeBuffer(shardBuf, 0,
      new Float32Array(E.memory.buffer, Number(E.shards(state)), shardCap * 20));
    device.queue.writeBuffer(fireInst, 0,
      new Float32Array(E.memory.buffer, Number(E.fireballs(state)), fireCap * 9));
    device.queue.writeBuffer(blastInst, 0,
      new Float32Array(E.memory.buffer, Number(E.blasts(state)), blastCap * 8));
    // The crystals are no longer static — a hit sets one's scale to zero — so
    // their instances go up with everything else now rather than once at build.
    const cry = new Float32Array(E.memory.buffer, Number(E.crystals(state)), W.counts.crystals * 8);
    device.queue.writeBuffer(W.crystalInst, 0, cry);
    // The walls go the same way the crystals do — a block that has been shot
    // has its size set to nought — so they are re-uploaded for the same reason.
    if (W.blocks > 0) {
      device.queue.writeBuffer(W.blockInst, 0,
        new Float32Array(E.memory.buffer, Number(E.blocks(state)), W.blocks * 8));
    }
    // And the pines, now that a warhead can fell one: the scale flips negative
    // and the yaw slot starts carrying the moment it fell, and the shader lays
    // the tree down from those two numbers.
    device.queue.writeBuffer(W.pineInst, 0,
      new Float32Array(E.memory.buffer, Number(E.pines(state)), W.counts.pines * 8));
    // Health, straight off Rill: it is the world that decides what a blast
    // does to you, not the renderer. Only touched when it actually moves,
    // for the same reason the tally is.
    // Breath, straight off Rill for the same reason health is: what running
    // costs is the world's business and not the renderer's. It moves every
    // frame you are running, so the threshold is tighter than the vital bar's
    // and the transition on it is short enough not to lag the key.

    // The round's clock and, between rounds, the board. Here rather than
    // beside the roster because the roster only exists when there is a
    // server, and a browser alone in its valley plays rounds too.
    //
    // Every frame, and not — as it was — inside the breath bar's own test for
    // having moved. Nested there it only redrew while somebody was running or
    // getting their wind back: stand still with a full bar and the clock
    // stopped, which read as a match that had frozen rather than as a HUD
    // that was not being asked. The two have nothing to do with each other.
    //
    // The world's own clock is what is read, on both sides: it runs in
    // `step_visuals` now, so it counts down smoothly between snapshots rather
    // than stepping twenty times a second. A snapshot only corrects it, and
    // only when a new one has actually arrived.
    const snap = net && net.last;
    if (snap && snap.left != null && snap.at !== syncedAt) {
      syncedAt = snap.at;
      // `phase` is an `Int` on the other side, which is an i64 here: a plain
      // number will not cross.
      E.set_match(world, BigInt(snap.phase | 0), snap.left);
    }
    if (world) {
      drawRound(Number(E.match_phase(world)), E.match_left(world),
                snap ? everyone(net) : soloBoard(), net && net.id);
    }
    // The rifle and the tube, each its own row. Whole numbers that change on
    // the frame the trigger goes down, so they are compared rather than
    // eased: nothing here moves unless a round left or a magazine arrived.
    const rounds = held === 2 ? Number(E.ak_rounds(state)) : Number(E.rounds(state));
    const gunCap = held === 2 ? Number(E.ak_full()) : Number(E.mag_full());
    const akMagsNow = Number(E.ak_mags(state));
    const rkts = Number(E.rockets(state));
    // A warhead is in the tube when there is one left to load and the reload
    // has finished. `boltT` is the same clock the trigger is gated on, so what
    // you see and what the trigger will do cannot disagree.
    loadedTube = held === 1 && rkts > 0 && boltT >= RPG_CYCLE;
    const hp = E.health(state);
    if (Math.abs(hp - lastHp) > 0.002) {
      if (hp > lastHp + 0.001 && lastHp >= 0) chime(t);
      lastHp = hp;
      // Alone, running out of health is the end of the thing and there is a
      // screen for it. In somebody else's valley it is five seconds on your
      // back, so it gets a different screen and a number counting down — being
      // dead has to be told apart from being broken, and the count is the only
      // thing on the page that can do it.
      if (hp <= 0.001 && !ended && !net) {
        ended = true;
        hudSlot(HSLOT.over1, [[-1, "GAME OVER"]]);
        hudSlot(HSLOT.over2, [[-1, "THE VALLEY KEPT ITS CRYSTALS"]]);
        hudSlot(HSLOT.over3, [[-1, "SPACE TO BEGIN AGAIN"]]);
        hudHead(HH.over, 1);
      }
    }

    // The tally, and with it the end of the thing. A crystal that has been shot
    // has its scale set to zero, so counting the ones that still have a size is
    // both the score and the win condition, off the same array that was about
    // to be uploaded anyway.
    //
    // Every frame, because a few hundred floats is nothing next to what has
    // just been written to the GPU — but the DOM is only touched when the
    // number actually moves. Writing the same string into an element sixty
    // times a second is how a text node ends up costing more than the valley.
    let standing = 0;
    for (let i = 0; i < W.counts.crystals; i++) if (cry[i * 8 + 3] > 0.001) standing++;
    if (!standing && !ended) {
      ended = true;
      hudSlot(HSLOT.over1, [[-1, "GAME OVER"]]);
      hudSlot(HSLOT.over2, [[-1, "THE VALLEY IS DARK"]]);
      hudSlot(HSLOT.over3, [[-1, "SPACE TO BEGIN AGAIN"]]);
      hudHead(HH.over, 1);
    }
    device.queue.writeBuffer(flyTime, 0, new Float32Array([t, dt, 0, 0]));

    // The HUD's frame: the live numbers into the block's header, the engine
    // lays everything out, and whatever it emitted goes up as one buffer.
    hudCount = 0;
    if (hudOff) {
      hudHead(HH.sw, canvas.clientWidth); hudHead(HH.sh, canvas.clientHeight);
      hudHead(HH.dpr, hudDpr); hudHead(HH.now, now / 1000);
      hudHead(HH.aim, aim); hudHead(HH.cross, aim > 0.9 ? 0 : aim > 0.35 ? 0.3 : 1);
      hudHead(HH.net, net ? 1 : 0);
      hudHead(HH.cycling, document.body.classList.contains("cycling") ? 1 : 0);
      hudHead(HH.tallyStand, standing);
      hudHead(HH.tallyBroke, W.counts.crystals - standing);
      hudHead(HH.tallyAll, W.counts.crystals);
      hudCount = Math.min(Number(E.hud_frame(state, world)), HUD_CAP);
      if (hudCount > 0) {
        device.queue.writeBuffer(hudInst, 0,
          new Float32Array(E.memory.buffer, hudOff.base + hudOff.inst * 4, hudCount * 16));
      }
      device.queue.writeBuffer(hudUni, 0,
        new Float32Array([canvas.clientWidth, canvas.clientHeight, 0, 0]));
    }

    const enc = device.createCommandEncoder();

    const sim = enc.beginComputePass();
    sim.setPipeline(simPipe); sim.setBindGroup(0, W.simBg);
    sim.dispatchWorkgroups(Math.ceil(T.flies / 64));
    sim.end();

    const sh = enc.beginRenderPass({
      colorAttachments: [],
      depthStencilAttachment: { view: W.shadowView, depthClearValue: 1, depthLoadOp: "clear", depthStoreOp: "store" },
      ...(qs ? { timestampWrites: { querySet: qs, beginningOfPassWriteIndex: 0 } } : {}),
    });
    sh.setBindGroup(0, bgShadow);
    sh.setPipeline(shTerrain); sh.setVertexBuffer(0, W.terrainBuf); sh.draw(W.counts.terrain);
    sh.setPipeline(shPine);
    sh.setVertexBuffer(0, pineShadowBuf); sh.setVertexBuffer(1, W.pineInst); sh.draw(pineShadowVerts, W.counts.pines);
    sh.setPipeline(shInst);
    if (W.blocks > 0) {
      sh.setVertexBuffer(0, blockBuf); sh.setVertexBuffer(1, W.blockInst); sh.draw(blockVerts, W.blocks);
    }
    sh.setVertexBuffer(0, crystalBuf); sh.setVertexBuffer(1, W.crystalInst); sh.draw(crystalVerts, W.counts.crystals);
    // A person standing in the sun has a shadow, and without one they read as
    // pasted onto the hillside rather than standing on it.
    if (figureCount > 0) {
      sh.setPipeline(shFigure);
      sh.setBindGroup(1, poseBg);
      sh.setVertexBuffer(0, figVB);
      sh.setIndexBuffer(figIB, "uint16");
      sh.drawIndexed(figure.indices, figureCount);
    }
    sh.end();

    const pass = enc.beginRenderPass({
      colorAttachments: [{
        view: msaaTex.createView(), resolveTarget: context.getCurrentTexture().createView(),
        loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 1 },
      }],
      depthStencilAttachment: {
        view: depthTex.createView(), depthClearValue: 1, depthLoadOp: "clear", depthStoreOp: "discard",
      },
      ...(qs ? { timestampWrites: { querySet: qs, endOfPassWriteIndex: 1 } } : {}),
    });
    pass.setBindGroup(0, W.bgMain);
    pass.setPipeline(terrainPipe); pass.setVertexBuffer(0, W.terrainBuf); pass.draw(W.counts.terrain);
    pass.setPipeline(pinePipe);
    pass.setVertexBuffer(0, pineBuf); pass.setVertexBuffer(1, W.pineInst); pass.draw(pineVerts, W.counts.pines);
    pass.setPipeline(grassPipe);
    pass.setVertexBuffer(0, grassBuf); pass.setVertexBuffer(1, W.grassInst); pass.draw(grassVerts, W.counts.grass);
    if (figureCount > 0) {
      pass.setPipeline(figurePipe);
      pass.setBindGroup(1, poseBg);
      pass.setVertexBuffer(0, figVB); pass.setVertexBuffer(1, figureInst);
      pass.setIndexBuffer(figIB, "uint16");
      pass.drawIndexed(figure.indices, figureCount);
    }
    if (heldCount) {
      pass.setPipeline(heldPipe);
      pass.setBindGroup(1, heldBg);
      pass.setVertexBuffer(0, rifleVB);
      pass.setIndexBuffer(rifleIB, "uint16");
      pass.drawIndexed(rifleIdx, heldCount);
    }
    pass.setPipeline(skyPipe); pass.draw(3);
    pass.setPipeline(W.waterPipe); pass.setVertexBuffer(0, W.waterBuf); pass.draw(W.waterVerts);
    // The crystals go here rather than with the other solids: they are
    // transparent now, and a transparent thing has to be drawn after whatever
    // is meant to show through it. Before the water they were blending against
    // an empty lake.
    if (W.blocks > 0) {
      pass.setPipeline(stonePipe);
      pass.setVertexBuffer(0, blockBuf); pass.setVertexBuffer(1, W.blockInst);
      pass.draw(blockVerts, W.blocks);
    }
    pass.setPipeline(crystalPipe);
    pass.setVertexBuffer(0, crystalBuf); pass.setVertexBuffer(1, W.crystalInst); pass.draw(crystalVerts, W.counts.crystals);
    pass.setPipeline(glowPipe); pass.setVertexBuffer(0, W.crystalInst); pass.draw(6, W.counts.crystals);
    pass.setPipeline(flyDrawPipe); pass.setBindGroup(1, W.flyDrawBg); pass.draw(6, T.flies);

    // The shark, and only from in the water.
    //
    // Not faded, not hidden behind a fog term — simply not drawn. The whole
    // point of it is that standing on the shore tells you nothing: you can
    // look straight down at the lake and there is nothing there, and the only
    // way to find out is to get in. A version that dims with depth would still
    // put a shape on the water for anyone who looked hard, and then the water
    // would be a thing you inspect rather than a thing you enter.
    //
    // The surface is at zero, so the test is the eye's own height. Feet in the
    // shallows do not count — it is about what the camera is inside of.
    if (st[17] < 0.0) {
      pass.setPipeline(sharkPipe);
      pass.setVertexBuffer(0, sharkVB);
      pass.setIndexBuffer(sharkIB, "uint16");
      pass.drawIndexed(sharkIdx);
    }

    // The buffer is divided rather than copied. Rill lays the magazines out
    // first and throws every broken chip of crystal into what is left, so the
    // two draws are one buffer read twice: boxes over the front of it, prisms
    // over the rest. `firstInstance` is what makes that free.
    pass.setPipeline(propPipe);
    pass.setVertexBuffer(0, propVB); pass.setVertexBuffer(1, shardBuf);
    pass.setIndexBuffer(propIB, "uint16");
    pass.drawIndexed(propIdx, magsOnly);

    // The crates: the back range of the same sixteen slots, their own mesh
    // and photograph, and `firstInstance` again doing the dividing.
    pass.setPipeline(cratePipe);
    pass.setVertexBuffer(0, crateVB); pass.setVertexBuffer(1, shardBuf);
    pass.setIndexBuffer(crateIB, "uint16");
    pass.drawIndexed(crateIdx, magCount - magsOnly, 0, 0, magsOnly);

    pass.setPipeline(shardPipe);
    pass.setVertexBuffer(0, crystalBuf); pass.setVertexBuffer(1, shardBuf);
    pass.draw(crystalVerts, shardCap - magCount, 0, magCount);

    pass.setPipeline(tracerPipe); pass.setVertexBuffer(0, bulletBuf); pass.draw(6, bulletCap);
    // The chips, over the surfaces they are stuck to and under everything
    // that flies: blended rather than added, because a hole is darker than
    // what it is in and not brighter.
    device.queue.writeBuffer(markInst, 0,
      new Float32Array(E.memory.buffer, Number(E.marks(state)), markCap * 8));
    pass.setPipeline(markPipe);
    pass.setVertexBuffer(0, markInst);
    pass.draw(6, markCap);
    // And the strike, off the very same buffer — added rather than blended,
    // because a spark is the one thing here that is brighter than what it is
    // in front of. After the chip so it sits over its own hole.
    pass.setPipeline(sparkPipe);
    pass.draw(6 * SPARKS_PER_MARK, markCap);
    // The crystals' light, on the same additive terms the tracers get.
    let zn = 0;
    for (const zp of zapsLive) {
      if (sky - zp.born > 0.55 || zn >= ZAP_MAX) continue;
      zapData.set([zp.ax, zp.ay, zp.az, zp.bx, zp.by, zp.bz, zp.r, zp.g, zp.b, zp.born], zn * 10);
      zn++;
    }
    if (zn > 0) {
      device.queue.writeBuffer(zapGpu, 0, zapData, 0, zn * 10);
      pass.setPipeline(zapPipe);
      pass.setVertexBuffer(0, zapGpu);
      pass.draw(6, zn);
    }
    // A hundred and twenty parcels, which is what a rocket's trail wants; a
    // meteor's own runs out at eighty and the rest come out past the end of it
    // and are discarded. See PUFFS in fireball.wgsl.
    pass.setPipeline(firePipe); pass.setVertexBuffer(0, fireInst); pass.draw(6 * 120, fireCap);
    // The stone goes down after the flames, not before: drawn first it sat
    // under an additive fire that simply burned it out, and the leading face —
    // the one thing that says there is something solid in there — was the part
    // that vanished first.
    pass.setPipeline(meteorPipe); pass.draw(3 * 11, fireCap);
    // And the rockets, over the same ring: the shader keeps the slots whose
    // seed is negative and sends the rest past the far plane. Drawn after the
    // smoke and before the blast, which is the order they happen in.
    pass.setPipeline(rocketPipe);
    pass.setVertexBuffer(0, rocket.vb); pass.setVertexBuffer(1, fireInst);
    pass.setIndexBuffer(rocket.ib, "uint16");
    pass.drawIndexed(rocket.count, fireCap);
    pass.setPipeline(blastPipe); pass.setVertexBuffer(0, blastInst); pass.draw(6 * 96, blastCap);

    // Last, so it covers the water and the fireflies as well as the ground.
    //
    // Except when this player is on the ground themselves — a rifle held out
    // at arm's length in front of a camera lying in the grass is the one thing
    // on that screen still behaving as though nothing had happened; the figure
    // everybody else sees drops it with the body, so does this — and except at
    // the top of the aim, where the scope has become the screen and the rifle
    // behind it would only be in the way of what it is aiming at.
    //
    // Only the rifle. On the other two the aim *is* the weapon — a notch and
    // a post on the AK, a post and a ladder on the launcher — and taking them
    // off the screen at the top of the move leaves nothing to aim with at all.
    // Getting out of the way was drawn for the scope's sake, and the scope is
    // the rifle's alone.
    if (E.health(state) > 0.001 && (aim < 0.9 || held !== 0)) {
      pass.setBindGroup(0, W.bgMain);
      if (held === 1) {
        pass.setPipeline(rpgPipe);
        pass.setVertexBuffer(0, rpg.vb);
        pass.setIndexBuffer(rpg.ib, "uint16");
        pass.drawIndexed(rpg.count);
        // And the round in the tube, when there is one in it. It goes at the
        // shot and comes back when the tube is loaded again, which is the only
        // thing on screen that says what the two and a half seconds are for.
        if (loadedTube) {
          pass.setPipeline(warheadPipe);
          pass.setVertexBuffer(0, rocket.vb);
          pass.setIndexBuffer(rocket.ib, "uint16");
          pass.drawIndexed(rocket.count);
        }
      } else if (held === 2) {
        pass.setPipeline(akPipe);
        pass.setVertexBuffer(0, akVB);
        pass.setIndexBuffer(akIB, "uint16");
        pass.drawIndexed(akIdx);
        pass.setPipeline(muzzlePipe); pass.draw(12);
      } else {
        pass.setPipeline(vmPipe);
        pass.setVertexBuffer(0, rifleVB);
        pass.setIndexBuffer(rifleIB, "uint16");
        pass.drawIndexed(rifleIdx);
        // The flash belongs to the rifle. The launcher's own is the trail.
        pass.setPipeline(muzzlePipe); pass.draw(12);
      }
    }

    // The HUD, over everything: whatever the engine laid out this frame,
    // one instanced draw. `hudCount` was settled before the pass began.
    if (hudCount > 0) {
      pass.setPipeline(hudPipe);
      pass.setBindGroup(0, hudBG);
      pass.setVertexBuffer(0, hudInst);
      pass.draw(6, hudCount);
    }
    pass.end();

    if (qs) {
      enc.resolveQuerySet(qs, 0, 2, qResolve, 0);
      if (!qPending) enc.copyBufferToBuffer(qResolve, 0, qStage, 0, 16);
    }
    device.queue.submit([enc.finish()]);
    // Once, at the end of the first frame — by which point the shadow map has
    // been drawn, so the valley in the cube is lit the same way the valley is.
    // The probe, kept current.
    //
    // Baked once was right when the sun stood still. It does not any more: a
    // cube taken on the first frame is a photograph of the valley at sunrise,
    // and a crystal at noon was reflecting the night — measured at 12,19,24 out
    // of 255 across every face, which is why the reflections read as nothing at
    // all. The valley's own colour is now a function of the hour, so the
    // picture of it has to be too.
    //
    // Every six tenths of a second — about two degrees of sun between bakes,
    // which is under what anyone can catch stepping. Six passes at 128 pixels
    // is a fifth of a millisecond; the reason not to do it every frame is that
    // there is no reason to.
    if (!W.envBaked || t - W.envWhen > 0.6) {
      W.envBaked = true;
      W.envWhen = t;
      bakeEnv(uf);
    }
    if (painted) { painted(); painted = null; }

    if (qs && !qPending) {
      qPending = true;
      qStage.mapAsync(GPUMapMode.READ).then(() => {
        const [a, b] = new BigInt64Array(qStage.getMappedRange());
        qStage.unmap();
        const ms = Number(b - a) / 1e6;
        if (ms > 0 && ms < 100) gpuMs = gpuMs * 0.9 + ms * 0.1;
        qPending = false;
      }).catch(() => { qPending = false; });
    }

    frames++;
    if (now - fpsT > 500 && hudOff) {
      fps = Math.round(frames * 1000 / (now - fpsT));
      frames = 0; fpsT = now;
      // The stat corner, twice a second, straight into the slots the engine
      // right-aligns. The words the old innerHTML carried, in segments: the
      // values gold, the rest quiet.
      const wt = weights();
      const kb = n => `${(n / 1024).toLocaleString("en", { maximumFractionDigits: 0 })} KB`;
      const here = net && net.last ? net.last.players.length : null;
      const ping = net ? net.ping : null;
      const sv = net ? net.serverVersion : null;
      const D = HC.dim, G = HC.gold;
      hudSlot(HSLOT.stat + 0, [[G, gpuName], [D, " · quality "], [G, T.name],
        [D, locked ? "" : " (measuring…)"],
        ...(here === null ? [] : [[D, " · "], [G, `${here}`], [D, ` ${here === 1 ? "person" : "people"} in the valley`]])]);
      hudSlot(HSLOT.stat + 1, [[G, `${fps}`], [D, " fps"],
        [D, qs && gpuMs ? ` · GPU ${gpuMs.toFixed(1)} ms` : ""],
        [D, ` · ${Math.round(W.tris / 1000)}k triangles`],
        ...(ping === null ? [] : [[ping < 60 ? HC.green : ping < 150 ? HC.gold : HC.red, ` · ${Math.round(ping)}`], [D, " ms to the server"]])]);
      hudSlot(HSLOT.stat + 2, [[D, "terrain, placement, physics, matrices: "], [G, "valley.rill"],
        [D, " → "], [G, `${(wasmSize() / 1024).toFixed(1)} KB`], [D, " of wasm"]]);
      hudSlot(HSLOT.stat + 3, [[D, "shadows · MSAA 4x · instancing · gerstner water · compute particles"]]);
      hudSlot(HSLOT.stat + 4, [[D, "textures "], [G, kb(wt.tex)], [D, " · all assets "], [G, kb(wt.raw)],
        [D, " → "], [G, kb(wt.wire)], [D, wt.wire < wt.raw ? " deflated" : " over the wire"]]);
      const agree = sv === Number(V);
      hudSlot(HSLOT.stat + 5, sv === null && ping === null
        ? [[D, "build "], [G, V]]
        : [[D, "build "], [agree ? HC.green : HC.red, V], [D, " · server "],
           [agree ? HC.green : HC.red, `${sv ?? "?"}`]]);
    }
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
  await firstFrame;

  // Turned away at the door: said here rather than at the moment it happened,
  // because at that moment the loading screen is still over everything and
  // nobody would have seen it. Six seconds is long enough to read twice, and
  // then it goes: what is behind it is a valley that works.
  if (turnedAway) {
    hudSlot(HSLOT.over1, [[-1, "NOT IN"]]);
    hudSlot(HSLOT.over2, [[-1, turnedAway.toUpperCase()]]);
    hudSlot(HSLOT.over3, [[-1, "THIS VALLEY IS YOURS ALONE"]]);
    hudHead(HH.over, 3);
    setTimeout(() => hudHead(HH.over, 0), 6000);
  }
}
