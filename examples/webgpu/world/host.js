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

import { load } from "./world.js?v=412";   // keep in step with V below
import { join, WH } from "./net.js?v=412";

// One number, appended to every asset URL.
//
// Not for the browser's cache — that would be reason enough — but for the CDN's.
// The images and audio went out under bare names, so replacing a texture at the
// origin changed nothing anyone could see: CloudFlare kept serving the old one
// and there is no purging it without a token. A version in the query string
// makes a new file a new URL, which is the only cache invalidation that needs
// nobody's permission. Bump it with `?v=` in index.html.
const V = "456";

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
  mark: "common", spark: "common", ak: "common", jet: "common",
  flower: "common",
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
  // Both trees' foliage in one texture, because sixteen sampled textures a
  // stage is exactly what this scene already spends and there is no
  // seventeenth binding to give — see the note on `boxes-atlas`. Two 512
  // cells side by side: a mass of needles and a mass of leaves. Both are
  // deep — thirty-odd sprays, six hundred leaves — because that is what lets
  // a tree be made of a handful of cards instead of a hundred.
  ["./pine-twig.png", 1024, 512],
  ["./pine-bark.jpg", 256, 256],
  ["./grass-ground.jpg", 256, 256],
  // Eight cells: the undergrowth's four on the left — two tufts, a fern, a
  // nettle — and the meadow's four flowers on the right, white, yellow, pink
  // and blue. One texture because there is no seventeenth binding to give.
  ["./plants.png", 1024, 512],
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

function loadShaders(step, pakText) {
  const src = Object.fromEntries(Object.keys(SHADERS).map(name => {
    const text = pakText(`shaders/${name}.wgsl`);
    step(`shaders/${name}.wgsl`);
    return [name, text];
  }));
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
    JET: whole("jet"),
    FLOWER: whole("flower"),
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
  // Seven whorls of five, not twelve of eight.
  //
  // A branch here is two quads crossed about its own axis — four triangles —
  // so eighty-four branches was three hundred and fifty triangles a tree and
  // three hundred thousand across nine hundred of them: the largest thing in
  // the valley by a long way, and all of it paying in geometry for cover a
  // texture can give. The needle cell carries thirty-odd sprays now, so one
  // card is a branch's worth of needles rather than a twig's, and half as
  // many of them at a third again the size cover the same cone.
  const tiers = [];
  const whorls = coarse ? 4 : 7;
  for (let i = 0; i < whorls; i++) {
    const u = whorls === 1 ? 0 : i / (whorls - 1);  // 0 at the lowest whorl, 1 at the spire
    tiers.push({
      y: 0.95 + u * 1.60,
      len: 0.26 + 1.18 * (1 - u) ** 1.1,
      // Taller cards when there are fewer of them, so the canopy still covers
      // the ground it used to rather than turning into four thin discs — but
      // a branch is longer than it is deep, and a card taller than it is long
      // stops being one.
      tall: (0.34 + 0.86 * (1 - u)) * (coarse ? 2.1 : 1.0),
      drop: 0.10 + 0.22 * (1 - u),
      n: coarse ? 4 : Math.round(6 - 2 * u),
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
        // The needle cell: the left half of the atlas, inset a little so the
        // sampler cannot reach across the seam at the coarser mips.
        const nu0 = 0.003, nu1 = 0.497, nv0 = 0.003, nv1 = 0.997;
        push(p00, n, [nu0, nv1]); push(p01, n, [nu1, nv1]); push(p11, n, [nu1, nv0]);
        push(p00, n, [nu0, nv1]); push(p11, n, [nu1, nv0]); push(p10, n, [nu0, nv0]);
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


// A poplar: a pale post with a narrow column of leaves up it.
//
// The opposite silhouette to the pine, which is the only reason to have a
// second tree at all — a valley of two cones is a valley of one tree drawn
// twice. This one is tall and vertical and mostly empty air at the edges, so
// a stand of them reads as a line rather than as a mass, and the lake behind
// still shows through.
//
// It rides the pine's own pipeline: the same vertex layout, the same shader,
// so it inherits the wind, the shadow pass and the way a tree lies down when
// it is felled without any of that being written twice. Two conventions come
// from there — a negative u means "this is bark, not foliage", and the
// instance colour is a tint the shader applies to both.
//
// The leaves are cards pointed at the plant atlas's nettle cell, which is a
// single broad leaf. At this size and this distance that is what poplar
// foliage looks like: not needles, and not a texture anybody has to make.
function poplarMesh(coarse) {
  const out = [];
  const push = (p, n, uv) => out.push(p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1]);

  // The trunk, thinner and half again as tall as the pine's, and barely
  // tapered: a poplar goes up like a mast.
  const top = 4.9, r0 = 0.085, r1 = 0.040, seg = 6;
  for (let i = 0; i < seg; i++) {
    const a0 = (i / seg) * Math.PI * 2, a1 = ((i + 1) / seg) * Math.PI * 2;
    const ring = (a, r, y) => [Math.cos(a) * r, y, Math.sin(a) * r];
    const p00 = ring(a0, r0, 0), p10 = ring(a1, r0, 0);
    const p01 = ring(a0, r1, top), p11 = ring(a1, r1, top);
    const u0 = -2 + i / seg, u1 = -2 + (i + 1) / seg;
    const nOf = a => [Math.cos(a), 0.12, Math.sin(a)];
    const n0 = nOf(a0), n1 = nOf(a1);
    push(p00, n0, [u0, 0]); push(p01, n0, [u0, 1]); push(p10, n1, [u1, 0]);
    push(p10, n1, [u1, 0]); push(p01, n0, [u0, 1]); push(p11, n1, [u1, 1]);
  }

  // The column. Rings of upright cards hugging the trunk, widest a little
  // under half way and closing to a point at the top — the lens shape a
  // poplar cuts against the sky. The cards face outward and are drawn from
  // both sides, like the pine's.
  //
  // The poplar cell: the 512 square beside the needles, six hundred leaves
  // deep, cut from ambientCG's LeafSet004 (CC0) and scattered by
  // `tools/bake_poplar.py` with the density thinning towards the edges so the
  // card's own rectangle never shows.
  //
  // One card is a piece of crown, not a twig. Twenty of them make this tree.
  // It was ninety-eight, which is the mistake of paying in geometry for
  // something a texture can say: a leaf costs pixels and a card costs
  // triangles, so the leaves belong in the picture and the cards should be as
  // few as will still turn as you walk round them.
  const CU = 0.503, CV = 0.003, SPAN = 0.494, VSPAN = 0.994;
  const rings = coarse ? 2 : 5;
  const perRing = coarse ? 2 : 4;
  for (let i = 0; i < rings; i++) {
    const u = rings === 1 ? 0 : i / (rings - 1);   // 0 at the lowest ring, 1 at the tip
    const y = 1.15 + u * 3.60;
    // Widest at two fifths up, nothing at the very top.
    // Widest two fifths up, nothing at all at the tip — `u` reaches one on
    // the last ring, and one is where this has to be zero or the spire is
    // blunt.
    const lens = Math.max(0, 1 - Math.pow(Math.abs(u - 0.40) / 0.60, 1.6));
    const rad = Math.max(0.05, 0.30 * lens);
    // The cards shrink with the column, which the first pass forgot: the
    // radius tapered towards the tip and the cards did not, so the last ring
    // was four full-sized squares of leaves sitting on a point — a tree with
    // a box on top. A poplar ends in a spire, and a spire is what a card that
    // narrows with its ring draws for nothing.
    //
    // A card a metre and a bit across at the widest, which is what puts the
    // leaves painted on it at five to ten centimetres — a poplar leaf.
    const w = (0.26 + 0.86 * lens) * (coarse ? 1.7 : 1.0);
    const hgt = (0.30 + 1.05 * lens) * (coarse ? 1.9 : 1.0);
    for (let j = 0; j < perRing; j++) {
      // Turned ring by ring by an angle that never repeats, so the column
      // does not stack into visible columns of card.
      const a = (j / perRing) * Math.PI * 2 + i * 0.9;
      const ox = Math.cos(a), oz = Math.sin(a);
      const tx = -Math.sin(a), tz = Math.cos(a);
      const cx = ox * rad, cz = oz * rad;
      const n = [ox * 0.6, 0.72, oz * 0.6];
      const half = w * 0.5;
      const yb = y - hgt * 0.5, yt = y + hgt * 0.5;
      const p00 = [cx - tx * half, yb, cz - tz * half];
      const p10 = [cx + tx * half, yb, cz + tz * half];
      const p01 = [cx - tx * half, yt, cz - tz * half];
      const p11 = [cx + tx * half, yt, cz + tz * half];
      // v runs 1 at the bottom, as the atlas has each plant standing on its
      // cell floor.
      const uv = (s, t) => [CU + s * SPAN, CV + (1 - t) * VSPAN];
      push(p00, n, uv(0, 0)); push(p10, n, uv(1, 0)); push(p11, n, uv(1, 1));
      push(p00, n, uv(0, 0)); push(p11, n, uv(1, 1)); push(p01, n, uv(0, 1));
    }
  }
  return new Float32Array(out);
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
export async function run(canvas, wasmURL, hud, progress = () => {}, server = null, name = "", pakBytes = null) {
  // Everything the first frame needs, counted up front so the loader can say
  // how far along it is: the adapter, the module, one step per shader, one per
  // texture, and the valley itself. `run` does not resolve until a frame has
  // actually been submitted, so whatever is covering the canvas comes off over
  // a painted one rather than over black.
  // Nine fetch/build steps, one per shader, one per texture, six decodes —
  // and one for the bundle the assets all arrive in.
  const TOTAL = 16 + Object.keys(SHADERS).length + TEXTURES.length + 19 + (server ? 1 : 0);
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
  // A developer's handle on the engine, for the console and for nothing else:
  // every state block is reachable through it, which is what debugging a
  // wasm-side game needs and what module scope otherwise forbids.
  // A developer's handle on the engine, and on what the renderer is holding
  // this frame: every state block is reachable through the first, and `W` and
  // `T` are what a frame is actually made of — the instance counts each draw
  // is given and the tier's own numbers. Both are there so a question like
  // "which of these is costing the milliseconds" can be answered by setting a
  // count to nought for a second rather than by guessing.
  window.rill = { E, get state() { return state; }, get world() { return world; },
                  get net() { return net; }, get W() { return W; }, get T() { return T; },
                  // Draw a frame on demand, which is the only way to measure
                  // this renderer honestly. A tab that is not the one in front
                  // gets no `requestAnimationFrame` at all — Chrome simply
                  // stops calling it — so every number the HUD shows freezes,
                  // and a profile taken through the automation tools is a
                  // profile of a still picture. This drives the same frame the
                  // clock would have.
                  frame: (n = 1) => { for (let i = 0; i < n; i++) tick(performance.now()); },
                  // How long the orders sat blank waiting for the audio to wake.
                  get briefWait() { return briefWait; } };
  step("world.wasm");

  // Every texture, mesh, sound, shader — and this very file — in one
  // request: valley.rpak, the format pack.rill writes. The boot in
  // index.html fetched it before this module existed (it was sliced out of
  // the same bytes) and hands it in; the unpacking — the LZ — is the
  // engine's (pak.rill), never the page's.
  if (!pakBytes) throw new Error("run() needs the valley.rpak bytes the boot fetched");
  // Into the engine's memory, once; the directory is read back out of it by
  // name, and each asset is pulled as fresh bytes and copied out at once, so
  // the wasm heap holds one decompressed asset at a time and not the pile.
  const pakP = Number(E.pak_alloc(BigInt(pakBytes.length)));
  new Uint8Array(E.memory.buffer, pakP, pakBytes.length).set(pakBytes);
  if (!Number(E.pak_ok(pakP))) throw new Error("valley.rpak is not an RPAK file this build reads");
  const PAK = new Map();
  let pakRawTotal = 0, pakWasmRaw = 0;
  {
    const dec = new TextDecoder();
    const nPak = Number(E.pak_count(pakP));
    for (let i = 0; i < nPak; i++) {
      const name = dec.decode(new Uint8Array(E.memory.buffer,
        Number(E.pak_name_ptr(pakP, BigInt(i))), Number(E.pak_name_len(pakP, BigInt(i)))));
      PAK.set(name, i);
      const raw = Number(E.pak_raw(pakP, BigInt(i)));
      pakRawTotal += raw;
      // The engine's own size, noted here: the module arrives from a blob
      // URL now, so the browser's resource log never hears of it.
      if (name === "world.wasm") pakWasmRaw = raw;
    }
  }
  const pakWire = pakBytes.length;
  const pak = (name) => {
    const i = PAK.get(name);
    if (i === undefined) throw new Error(`${name} is not in valley.rpak — add it to assets.list and re-run pack.rill`);
    const raw = Number(E.pak_raw(pakP, BigInt(i)));
    const ptr = Number(E.pak_unpack(pakP, BigInt(i)));
    const out = new Uint8Array(raw);
    out.set(new Uint8Array(E.memory.buffer, ptr, raw));
    E.pak_free(ptr);
    return out;
  };
  const pakBuf = (name) => pak(name).buffer;
  const pakText = (name) => new TextDecoder().decode(pak(name));
  step("valley.rpak");

  const S = loadShaders(step, pakText);

  // How big the module is, read off the browser's own record of the request
  // the loader already made — no second one. `decodedBodySize` rather than
  // `encodedBodySize`: what is wanted on screen is the size of the thing, not
  // what a CDN's gzip happened to squeeze it to on the way. The entry lands a
  // moment after the fetch resolves, so this asks again until it is there.
  let wasmBytes = 0;
  const wasmSize = () => {
    if (wasmBytes) return wasmBytes;
    if (pakWasmRaw) { wasmBytes = pakWasmRaw; return wasmBytes; }
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
    const bytes = pakBuf(url.slice(2));
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
      mipped(url, w, h).then(v => { step(url.slice(2)); return v; })));
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
  // The flowers, on the undergrowth's own mesh and layout — two crossed cards
  // — and its own shader, which draws from the other half of the atlas and
  // leans further in the wind.
  const flowerPipe = pipe("flower", S.FLOWER, [pineLayout, instLayout], { cull: "none" });
  const glowPipe = pipe("glow", S.GLOW, [instLayout], { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  // The pack's flame, on the crystals' own instance layout — where it starts,
  // how hard it is burning, a seed, and a colour it does not read. Additive
  // and depth-tested but not depth-writing, because it is light.
  const jetPipe = pipe("jet", S.JET, [instLayout], { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  const JET_CAP = 33;
  const jetBuf = device.createBuffer({
    size: JET_CAP * 32, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });
  let jetCount = 0;
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
  // The second face, and the only thing in this HUD not drawn in the page's
  // own monospace: Share Tech Mono, by Ralph du Carrois, under the SIL Open
  // Font License — the file is in the bundle and the licence beside it in the
  // repository. Monospaced on purpose. The briefing types itself out with a
  // block cursor that has to sit exactly where the next letter will land, and
  // a proportional face makes that a measurement rather than a multiplication.
  const TECH_FONT = "ShareTechMono, ui-monospace, monospace";
  const HUD_FACES = [HUD_FONT, TECH_FONT];
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
    const rows = stripDefs.map(([size, weight, fam]) => {
      const c = document.createElement("canvas").getContext("2d");
      c.font = `${weight} ${size * hudDpr}px ${HUD_FACES[fam] ?? HUD_FONT}`;
      const adv = c.measureText("M").width / hudDpr;
      const cellW = Math.ceil((adv + pad * 2) * hudDpr);
      const cellH = Math.ceil((size * 1.35 + pad * 2) * hudDpr);
      return { size, weight, fam, adv, cellW, cellH };
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
      ctx.font = `${r.weight} ${r.size * hudDpr}px ${HUD_FACES[r.fam] ?? HUD_FONT}`;
      // Wider than the CSS's own two and a half. A halo is what makes small
      // text survive a bright hillside behind it, and the atlas is where it
      // has to be decided: the shader can only spread what was baked.
      ctx.shadowBlur = 3.4 * hudDpr;
      for (let i = 0; i < HUD_CHARS.length; i++) {
        const cx = (i % r.perRow) * r.cellW, cy = r.y0 + Math.floor(i / r.perRow) * r.cellH;
        // Three passes, then a fourth with the shadow off.
        //
        // One pass gives what a CSS text-shadow gives, which was right when
        // this HUD lay over a page and is not right over a valley: a letter
        // in quiet grey on sunlit rock has a soft grey edge against a bright
        // background and cannot be read at all. Drawing it again and again
        // accumulates the halo's alpha into something closer to an outline —
        // the letter keeps its own shape and gains a dark rim it carries
        // wherever it goes. The last pass is clean, so the core stays crisp
        // rather than being three blurs deep.
        for (let k = 0; k < 3; k++) ctx.fillText(HUD_CHARS[i], cx + pad * hudDpr, cy + pad * hudDpr);
        ctx.shadowColor = "transparent";
        ctx.fillText(HUD_CHARS[i], cx + pad * hudDpr, cy + pad * hudDpr);
        ctx.shadowColor = "rgba(0,0,0,0.95)";
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
    { binding: 3, visibility: GPUShaderStage.FRAGMENT, texture: {} },
    { binding: 4, visibility: GPUShaderStage.FRAGMENT, texture: {} },
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
  // The briefing's sheet of glass. Kenney's sci-fi UI pack, CC0 — a hundred
  // and twenty-eight pixels square with notched corners, drawn nine-sliced so
  // it stretches to any panel without the notches stretching with it. No mip
  // chain: it is only ever drawn larger than it is stored.
  const flatTex = async (name) => {
    const bmp = await createImageBitmap(new Blob([pakBuf(name)]));
    const tex = device.createTexture({
      label: name, size: [bmp.width, bmp.height], format: "rgba8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
    });
    device.queue.copyExternalImageToTexture({ source: bmp }, { texture: tex }, [bmp.width, bmp.height]);
    return tex.createView();
  };
  const panelView = await flatTex("panel-glass.png");
  step("panel-glass.png");
  // The three weapons, drawn by game-icons.net under CC BY 3.0 — skoll's
  // Winchester and AK47, delapouite's missile launcher — rendered out of
  // their SVGs into one strip by tools/bake_icons.py.
  const iconView = await flatTex("weapon-icons.png");
  step("weapon-icons.png");
  let hudAtlas = null, hudBG = null, hudOff = null;

  // Wired after every (re)join: the block is new, the atlas is not.
  function hudWire(st) {
    E.hud_init(st);
    if (!hudAtlas) {
      const defs = [];
      for (let i = 0; i < Number(E.hud_strips()); i++) {
        defs.push([E.hud_strip_size(st, BigInt(i)), E.hud_strip_weight(st, BigInt(i)),
                   Math.round(E.hud_strip_font(st, BigInt(i)))]);
      }
      hudAtlas = bakeHudAtlas(defs);
      hudBG = device.createBindGroup({ layout: hudBGL, entries: [
        { binding: 0, resource: { buffer: hudUni } },
        { binding: 1, resource: hudAtlas.tex.createView() },
        { binding: 2, resource: hudSampler },
        { binding: 3, resource: panelView },
        { binding: 4, resource: iconView },
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
               rosterN: 32, boardN: 33, tagN: 34, cross: 36, brief: 40, briefCur: 41 };
  const HSLOT = { stat: 0, log: 8, roster: 20, board: 36, tag: 52,
                  warn: 68, took: 69, crier: 70, over1: 71, over2: 72, over3: 73,
                  brief0: 74 };

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
  const rifleBytes = pakBuf("rifle.mesh");
  const rifleHead = new DataView(rifleBytes, 0, 32);
  const rifleVerts = rifleHead.getUint32(0, true);
  const rifleIdx = rifleHead.getUint32(4, true);
  const rifleBox = new Float32Array(rifleBytes, 8, 6);
  // The AK: GGBotNet's soviet assault rifle, CC0, baked from its OBJ into the
  // rifle's own sixteen-byte format with the barrel turned onto +x.
  const akBytes = pakBuf("ak47.mesh");
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
  const propBytes = pakBuf("ammo-box.mesh");
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
  const crateBytes = pakBuf("rocket-crate.mesh");
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
    const bytes = pakBuf(url.slice(2));
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
  const sharkBytes = pakBuf("shark.mesh");
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
  const figBytes = pakBuf("figure.mesh");
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
  let audio = null, shotBuf = null, breakBuf = null, blastSnd = null, musicBuf = null, welcomeBuf = null;
  let roarBuf = null, roars = null, pickBuf = null, hurtBuf = null, zapSndBuf = null, akBuf = null, akReloadBuf = null, rpgBuf = null, musicStarted = false;
  // Off to begin with. A valley somebody has just walked into should sound
  // like a valley; the theme is something you put on, not something that is
  // put on you. `M` turns it up, and the HUD has said so all along.
  let musicGain = null, musicOn = false;
  const shotBytes = pakBuf("rifle-shot.mp3");
  step("rifle-shot.mp3");
  const breakBytes = pakBuf("crystal-break.mp3");
  step("crystal-break.mp3");
  // Two explosions, 1.9 seconds each, laid end to end and played by offset.
  const blastBytes = pakBuf("meteor-blast.mp3");
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
  const roarBytes = pakBuf("meteor-air.mp3");
  step("meteor-air.mp3");
  // A chip of crystal going in. Three quarters of a second off the front of a
  // CC0 harp figure by Julie Damsgaard — the rest of it is a two-second tail,
  // which is a lovely thing to have and the wrong shape entirely for a sound
  // that fires four times while you cross a patch of debris.
  const pickBytes = pakBuf("pickup.mp3");
  step("pickup.mp3");
  // What it sounds like to be shot. Four of qubodup's CC0 recordings of
  // himself straining, a second each end to end and played by offset like the
  // glass — four is enough that a burst does not sound like a loop of one man.
  const hurtBytes = pakBuf("hurt.mp3");
  step("hurt.mp3");
  // The theme. Eighty seconds that begin and end in silence, which is what
  // makes it loop without a seam: no crossfade, no loop points, just the tail
  // running into the head the way the piece was already written.
  const musicBytes = pakBuf("valley-theme.mp3");
  step("valley-theme.mp3");
  // A crystal's bite. Kenney's digital SFX set by way of OpenGameArt, CC0 —
  // the same shelf half this valley's sounds came off.
  const zapBytes = pakBuf("crystal-zap.mp3");
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
  const akSndBytes = pakBuf("ak-shot.m4a");
  step("ak-shot.m4a");
  // The magazine change: SpringySpringo's assault rifle reload, CC0. A second
  // and a half of it, which is the pause it is played over.
  const akReloadBytes = pakBuf("ak-reload.m4a");
  step("ak-reload.m4a");
  // The launcher going off. A rocket motor lighting, from OpenGameArt's CC0
  // `rocket-launch`, cut to the ignition and given a decay: what the pack
  // holds is four seconds of sustained burn, which is a rocket leaving a pad
  // rather than a tube leaving a shoulder.
  const rpgSndBytes = pakBuf("rpg-launch.m4a");
  step("rpg-launch.m4a");
  // The welcome, spoken once and only to somebody who has never been here.
  // Whether they have is a cookie — see `firstVisit` — and the line plays on
  // the click that starts the game, because that is the gesture a browser
  // wants before it will let anything be heard at all.
  const welcomeBytes = pakBuf("welcome.mp3");
  step("welcome.mp3");
  // The briefing's face, out of the bundle and into the page's font list, so
  // that a canvas can be asked to draw in it when the HUD atlas is baked.
  //
  // Awaited here rather than left to load in its own time: the atlas is baked
  // once, and a face that arrives after the baking is a face nothing is ever
  // drawn in. If it fails — and this is the only asset the page hands to a
  // subsystem that may refuse it — the stack behind it is the monospace
  // everything else uses, so the orders come out in the HUD's own letters
  // rather than not at all.
  try {
    const face = new FontFace("ShareTechMono", pakBuf("share-tech-mono.ttf"));
    await face.load();
    document.fonts.add(face);
  } catch (e) {
    console.warn("the briefing's face did not load; falling back", e);
  }
  step("share-tech-mono.ttf");
  // The last asset is out; the bundle's three megabytes go back to the
  // engine's heap before the valley is built in it.
  E.pak_free(pakP);
  PAK.clear();
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
    [shotBuf, breakBuf, blastSnd, roarBuf, pickBuf, hurtBuf, musicBuf, zapSndBuf, akBuf, akReloadBuf, rpgBuf,
     welcomeBuf] = await Promise.all([
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
      one(welcomeBytes, "decoding welcome"),
    ]);
  }

  // ---- the welcome ----------------------------------------------------------
  //
  // Half a minute of somebody saying hello, played once in a person's life
  // rather than once a session — so what remembers it has to outlive the tab.
  // A cookie does; `sessionStorage` would forget on every reload and
  // `localStorage` would be per-origin-per-browser with no expiry to reason
  // about. Ten years is "never again" written in the only unit a cookie has.
  //
  // `SameSite=Lax` because nothing about this needs to travel on a request
  // somebody else's page made, and no `Secure`, because this is served over
  // plain http while it is being worked on and a Secure cookie would simply
  // never be set there — which would mean the line played every single time
  // on the development build and never told anyone why.
  const SEEN = "valley.seen";
  const firstVisit = () => !document.cookie.split("; ").some(c => c.startsWith(SEEN + "="));
  const markVisited = () => {
    document.cookie = `${SEEN}=1; max-age=${60 * 60 * 24 * 365 * 10}; path=/; SameSite=Lax`;
  };
  // Asked once, here, and not at the click: `sayWelcome` sets the cookie, so
  // by the time the click comes round the answer to "is this their first
  // time" would already be no.
  const newcomer = firstVisit();
  let welcomed = false;
  // The voice while it is playing, so the left button can stop it.
  let welcomeSrc = null, welcomeGain = null;
  // Straight to the destination and at nearly full level: this is speech, and
  // speech mixed down to where a rifle shot sits is speech nobody makes out.
  // It does not go through `musicGain`, which starts at nought and is the
  // player's own switch — the greeting is not the music and must not be
  // silenced by it being off.
  //
  // The cookie is written when the sound actually starts, not when it is
  // asked for. A browser that has not yet allowed audio would otherwise have
  // the greeting marked as heard by somebody who heard nothing, and there is
  // no second chance at a first visit — so if the context will not wake, this
  // does nothing and the next gesture tries again.
  async function sayWelcome() {
    if (welcomed || !welcomeBuf || !audio) return;
    if (audio.state === "suspended") { try { await audio.resume(); } catch { return; } }
    if (audio.state !== "running") return;
    welcomed = true;
    markVisited();
    const src = audio.createBufferSource();
    src.buffer = welcomeBuf;
    const g = audio.createGain();
    g.gain.value = 0.9;
    src.connect(g).connect(audio.destination);
    src.start();
    // Kept so the left button can stop it — see `cutBriefing`. The gain goes
    // with it: a source stopped dead is a click, and the last thing a welcome
    // should do is make a noise on the way out.
    welcomeSrc = src;
    welcomeGain = g;
    // The clock the words are read off. The context's own, not the page's:
    // it is the one the sound is actually playing against, so the two cannot
    // drift apart however the frames go.
    briefAt = audio.currentTime;
    // How long the sheet sat there with nothing on it, for the record — see
    // `briefOpen`. Anything but a few frames means the resume was slow.
    if (briefOpen >= 0) briefWait = performance.now() / 1000 - briefOpen;
  }

  // Enough of that. The voice fades over a fifth of a second and the card
  // goes with it; the cookie stays set, because they were told — they simply
  // decided they had heard enough, and being made to sit through it a second
  // time is not what that means.
  function cutBriefing() {
    if (welcomeSrc) {
      const now = audio.currentTime;
      welcomeGain.gain.cancelScheduledValues(now);
      welcomeGain.gain.setValueAtTime(welcomeGain.gain.value, now);
      welcomeGain.gain.linearRampToValueAtTime(0, now + 0.2);
      try { welcomeSrc.stop(now + 0.25); } catch { /* already done */ }
      welcomeSrc = null;
      welcomeGain = null;
    }
    endBriefing();
  }

  // The orders, as they are spoken. Wrapped by hand rather than by a measuring
  // routine, because where a briefing breaks its lines is a thing to decide,
  // not to compute: nothing here runs past the edge and no sentence starts on
  // the last word of a line.
  const BRIEF = [
    "YOU HAVE BEEN SENT TO AN UNKNOWN PLANET",
    "THAT HAS ENTERED THE SOLAR SYSTEM.",
    "",
    "YOUR MISSION, ASSIGNED BY THE COMPANY,",
    "IS TO DESTROY CRYSTALS BELIEVED TO POSE",
    "A HOSTILE THREAT TO EARTH.",
    "",
    "REMEMBER, OTHER COMPANIES HAVE BEEN GIVEN",
    "THE SAME MISSION, AND YOU MAY ENCOUNTER",
    "AGENTS FROM RIVAL COMPANIES.",
    "",
    "GOOD LUCK.",
  ];
  // A letter at a time, so the count that matters is letters. A blank line
  // costs nothing and is skipped over in no time at all, which is what a
  // paragraph break should do: it is a breath, not a pause to be typed.
  const BRIEF_CHARS = BRIEF.reduce((n, l) => n + l.length, 0);

  // Where every letter of the briefing is, as one run: which line and which
  // column. The typing works in this flat stream and the drawing works in
  // lines, and this is the one place that knows both.
  const BRIEF_FLAT = [];
  BRIEF.forEach((l, li) => { for (let c = 0; c < l.length; c++) BRIEF_FLAT.push([li, c]); });

  // Somewhere the typing is allowed to stop. The end of the text, the start of
  // a line, or a space — which is to say, never in the middle of a word.
  //
  // This is the whole of what keeps a half-typed word off the screen. The
  // voice pauses ten times in half a minute, for the better part of a second
  // each; without this the letters stop wherever the pause found them and the
  // reader is left staring at ENTER with a cursor after it, which reads as
  // something having gone wrong rather than as somebody drawing breath.
  const briefStops = new Uint32Array(BRIEF_CHARS + 1);
  {
    const isStop = (p) => {
      if (p >= BRIEF_CHARS) return true;
      const [li, c] = BRIEF_FLAT[p];
      return c === 0 || BRIEF[li][c] === " ";
    };
    let next = BRIEF_CHARS;
    for (let p = BRIEF_CHARS; p >= 0; p--) { if (isStop(p)) next = p; briefStops[p] = next; }
  }

  // Which sentence is spoken when.
  //
  // The silences come out of `ffmpeg -af silencedetect=noise=-35dB:d=0.20`,
  // and the four long ones in the middle of them are where the reading stops
  // for a full stop — so the clip divides into four sentences and the text
  // divides into the same four, and each one is typed over its own stretch of
  // speech rather than all of them over the average.
  //
  // The difference is not small and it is not constant, which is what made it
  // worth doing properly: spread evenly, "GOOD LUCK" was given the seven
  // tenths of a second its ten letters are worth out of the whole, when the
  // voice actually spends a second and a fifth on it — and every sentence
  // before it borrowed a little of that, so the writing arrived later and
  // later the further in it got. Anchored a sentence at a time, the two
  // cannot drift: each one starts and finishes where the voice does.
  const BRIEF_PARTS = [
    { lines: [0, 1],       said: [[0.00, 2.85], [3.26, 5.37]] },
    { lines: [3, 4, 5],    said: [[6.25, 7.05], [7.36, 8.80], [9.36, 14.43]] },
    { lines: [7, 8, 9],    said: [[15.30, 15.96], [16.66, 19.37], [20.02, 21.28], [21.48, 24.30]] },
    { lines: [11],         said: [[25.10, 26.29]] },
  ];
  const BRIEF_END = 26.29;
  // Each part, worked out once: where its letters start and end in the flat
  // stream, and how many seconds of speech it has.
  for (const part of BRIEF_PARTS) {
    let at = 0;
    for (let li = 0; li < part.lines[0]; li++) at += BRIEF[li].length;
    part.from = at;
    part.to = at + part.lines.reduce((n, li) => n + BRIEF[li].length, 0);
    part.total = part.said.reduce((n, [a, b]) => n + (b - a), 0);
    part.start = part.said[0][0];
  }
  // How much of one part has been spoken by `t`: its own clock, with its own
  // silences taken out.
  const spokenIn = (part, t) => {
    let s = 0;
    for (const [a, b] of part.said) { if (t <= a) break; s += Math.min(t, b) - a; }
    return s;
  };
  // And where the writing has got to, in letters.
  const lettersBy = (t) => {
    let n = 0;
    for (const part of BRIEF_PARTS) {
      if (t <= part.start) break;
      n = part.from + (part.to - part.from) * Math.min(1, spokenIn(part, t) / part.total);
    }
    return n;
  };

  // Letters a second, once a word has been started. Well above the reading
  // itself — which averages fourteen — because this is not the pace of the
  // briefing, it is the pace of finishing the word the briefing has reached.
  // The pace of the briefing is the voice, and it is in `lettersBy`.
  const BRIEF_RATE = 40;

  // How far ahead of the voice the typing runs.
  //
  // Without it the text lands late, and the reason is worth writing down: a
  // word's letters are spread over the time that word is being *said*, so the
  // word is not finished on screen until the voice has finished saying it.
  // Reading and hearing the same word at the same moment therefore means the
  // writing has to start first. Half a second is about a word at this pace,
  // which is what a caption is normally cut to and what stops the two feeling
  // like two different readings of the same sentence.
  const BRIEF_LEAD = 0.4;

  // Two clocks, and they are not the same one.
  //
  // `briefOpen` is when the card was *asked* for — the page's own clock, set
  // the instant the button goes down. `briefAt` is when the voice actually
  // started, on the audio context's clock, which is the only clock the typing
  // can be run off.
  //
  // They are apart because waking an audio context takes as long as it takes:
  // the browser will not start one until a gesture, and then the resume is a
  // promise that may resolve in a millisecond or may not. Nothing visible
  // should be waiting on it. So the sheet comes up on the click, with its
  // heading and a cursor waiting on the first line, and the words begin when
  // the voice does — which is also what the thing is: a device coming on, and
  // then somebody talking.
  //
  // The other half of arriving late was the fade, which was six tenths of a
  // second and is now a quarter: the first words used to be typed while the
  // card was still coming up, so what you saw first was faint text on faint
  // glass rather than a screen that had opened.
  let briefOpen = -1;        // when the card was asked for, on the page clock
  let briefAt = -1;          // when the reading started, on the audio clock
  let briefLast = -1;        // that clock at the frame before this one
  let briefWait = 0;         // and how long the card waited for the voice
  let briefShown = 0;        // letters on screen, carried across frames
  let briefDrawn = -1;       // and how many were last written into the slots

  // Called every frame while the orders are up. Everything it decides is a
  // letter count, a cursor line and a fade; the HUD draws the rest.
  function stepBriefing() {
    if (briefOpen < 0 && briefAt < 0) return;
    // Up on the click, whatever the voice is doing.
    if (briefAt < 0) {
      const up = Math.min(1, (performance.now() / 1000 - briefOpen) / 0.25);
      hudHead(HH.brief, up);
      hudHead(HH.briefCur, 0);
      return;
    }
    if (!audio) return;
    const t = audio.currentTime - briefAt;
    const dt = briefLast < 0 ? 0 : Math.max(0, t - briefLast);
    briefLast = t;
    // A second and a half of fade at each end, and then it is gone for good.
    const a = Math.min(1, t / 0.25) * (1 - Math.min(1, Math.max(0, (t - BRIEF_END) / 1.5)));
    if (t > BRIEF_END + 1.6) { endBriefing(); return; }
    hudHead(HH.brief, Math.max(0, a));
    // Where the voice has got to, rounded on to the end of the word it is in
    // the middle of, and then typed towards at a steady clip.
    const want = lettersBy(Math.max(0, t + BRIEF_LEAD));
    const target = briefStops[Math.min(BRIEF_CHARS, Math.round(want))];
    briefShown = Math.min(target, briefShown + dt * BRIEF_RATE);
    const n = Math.floor(briefShown);
    if (n === briefDrawn) return;
    briefDrawn = n;
    let left = n, cur = -1;
    for (let li = 0; li < BRIEF.length; li++) {
      const take = Math.max(0, Math.min(BRIEF[li].length, left));
      // The cursor sits at the end of whatever line is being typed — the first
      // one that is not yet full. A finished line does not hold it and a line
      // that has not started does not either, which together mean it steps to
      // the next line on the letter that fills the last one.
      if (cur < 0 && take < BRIEF[li].length && BRIEF[li].length > 0) cur = li;
      left -= take;
      if (take === 0) hudClear(HSLOT.brief0 + li);
      else hudSlot(HSLOT.brief0 + li, [[HC.text, BRIEF[li].slice(0, take)]]);
    }
    hudHead(HH.briefCur, cur);
  }

  function endBriefing() {
    briefOpen = -1;
    briefAt = -1;
    briefLast = -1;
    briefShown = 0;
    briefDrawn = -1;
    welcomeSrc = null;
    welcomeGain = null;
    hudHead(HH.brief, 0);
    hudHead(HH.briefCur, -1);
    for (let i = 0; i < BRIEF.length; i++) hudClear(HSLOT.brief0 + i);
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
  // The twenty-eight-metre edge itself now lives with the rest of the
  // arithmetic in ears.rill (`roar_reach`); the voices are all that is here.
  function startRoars() {
    // Four, not three: the last one is the pack on this player's back, which
    // the engine drives out of the same block of targets. Same construction,
    // because it is the same sound — a body forcing air past itself.
    roars = [0, 1, 2, 3].map(() => {
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
  // Where a sound is — how loud, how far round to the side, which variant and
  // at what pitch — is the engine's now (ears.rill). What used to be decided
  // here arrives each frame as a ring of finished cues, kind mapped to buffer:
  //   1 a crystal breaking   2 a fireball landing   3 a crystal's bite
  //   4 a chip picked up     5 this player hurt over the wire

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
  // The rig and the clips go into the engine once per (re)join; the skinner
  // itself is skin.rill. Clip roles ride in a fixed order the block knows.
  const SKIN_ROLES = ["idle", "run", "crouch", "die", "hit", "fire"];
  function skinWire(st) {
    E.skin_init(st);
    const base = Number(E.skin_ptr(st));
    const F = (at, arr) => new Float32Array(E.memory.buffer, base + at * 4, arr.length).set(arr);
    const J = figure.joints;
    const head = new Float32Array(E.memory.buffer, base, 16);
    head[0] = J; head[2] = figure.hand;
    F(Number(E.skin_grip_at()), figure.grip);
    F(Number(E.skin_parent_at()), Float32Array.from(figure.parent));
    F(Number(E.skin_upper_at()), Float32Array.from(figure.upper, v => v / 255));
    F(Number(E.skin_bindt_at()), figure.bindT);
    F(Number(E.skin_inv_at()), figure.invBind);
    // The pool: each clip's root track as floats, its rotations as the same
    // int16s the file carried.
    let poolF = Number(E.skin_pool_at());          // f32 index for root tracks
    const names = [...figure.clips.keys()];
    head[1] = names.length;
    names.forEach((name, c) => {
      const k = figure.clips.get(name);
      const t = new Float32Array(E.memory.buffer, base + Number(E.skin_clip_at(BigInt(c))) * 4, 16);
      t[0] = k.frames; t[1] = k.fps; t[2] = k.loops ? 1 : 0; t[3] = k.root;
      t[4] = poolF; t[6] = k.seconds;
      F(poolF, k.rootT); poolF += k.rootT.length;
      // Rotations start at the next even f32 so the int16 view stays aligned.
      poolF = (poolF + 1) & ~1;
      t[5] = poolF * 4;                            // byte index
      new Int16Array(E.memory.buffer, base + poolF * 4, k.rot.length).set(k.rot);
      poolF += (k.rot.length + 1) >> 1;
    });
    SKIN_ROLES.forEach((r, i) => { head[4 + i] = Math.max(0, names.indexOf(r)); });
  }
  const poseBuf = device.createBuffer({
    size: FIGURE_CAP * figure.joints * 64,
    usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
  });
  const poseBg = device.createBindGroup({
    layout: poseBgl, entries: [{ binding: 0, resource: { buffer: poseBuf } }],
  });
  const heldBuf = device.createBuffer({
    // One more than there are people: the spare is this player's own, on the
    // ground, once they have been knocked off their feet. Ninety-six bytes an
    // entry now, not sixty-four: the matrix, then the mesh's own quantization
    // box, because three different weapons were each baked against their own.
    size: (FIGURE_CAP + 1) * 96, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
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
  // Where each weapon's run of instances ends in the grouped upload:
  // [rifles, rifles+launchers, all]. The draw walks these with firstInstance.
  let heldSplit = [0, 0, 0];


  const pineData = pineMesh(false), crystalData = crystalMesh(), grassData = grassMesh();
  const pineBuf = gpuBuf(pineData), crystalBuf = gpuBuf(crystalData), grassBuf = gpuBuf(grassData);
  const pineVerts = pineData.length / 8, crystalVerts = crystalData.length / 6, grassVerts = grassData.length / 8;
  const poplarData = poplarMesh(false);
  const poplarBuf = gpuBuf(poplarData);
  const poplarVerts = poplarData.length / 8;
  const poplarShadowData = poplarMesh(true);
  const poplarShadowBuf = gpuBuf(poplarShadowData);
  const poplarShadowVerts = poplarShadowData.length / 8;
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

  // ---- the round -----------------------------------------------------------
  //
  // How long is left, and the board between rounds. Both read the same two
  // numbers off the world — the phase and the seconds remaining — so a browser
  // alone in its own valley shows exactly what a shared one does, without a
  // server having to be there to say so.

  // The board's rows. In somebody else's valley the wire fills them; alone,
  // the one row there is comes from here.
  function drawRound(phase) {
    if (net) return;
    if (phase !== 1) { hudHead(HH.boardN, 0); return; }
    hudSlot(HSLOT.board, [[-1, "you"]],
            Number(E.kills(state)), Number(E.deaths(state)), 1);
    hudHead(HH.boardN, 1);
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
  // How many interpolated players the wire handed over this frame, and the
  // little per-player objects the two remaining consumers — the zap beam's
  // target and the head-lamps — build on demand.
  let wireN = 0;
  const wireSeen = () => {
    const wi = wireInterp();
    const out = [];
    for (let i = 0; i < wireN; i++) {
      const o = i * 12;
      out.push({ id: wi[o], x: wi[o + 1], y: wi[o + 2], z: wi[o + 3],
                 yaw: wi[o + 4], pitch: wi[o + 5], health: wi[o + 6],
                 torch: (wi[o + 8] % 2) >= 1 });
    }
    return out;
  };
  const wireF32 = () => new Float32Array(E.memory.buffer, Number(net.wire), 64);
  const earsF32 = () => new Float32Array(E.memory.buffer, Number(earsPtr), 256);
  const EARS = { n: 0, roar: Number(E.ears_roar_at()), cues: Number(E.ears_cues_at()) };
  const wireInterp = () => new Float32Array(E.memory.buffer, Number(net.wire) + Number(E.wire_interp_at()) * 4, 32 * 12);
  const wirePev = () => new Float32Array(E.memory.buffer, Number(net.wire) + Number(E.wire_pev_at()) * 4, 16 * 4);
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
  let held = 2, wantHeld = 2, swap = 0, loadedTube = false;
  // The ears block, remade with each player block it hangs off.
  let earsPtr = 0n;

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
    // And a fresh hand for the fresh player block; the weapon carries over
    // because `set_weapon` ran just above and `make` reads it back.
    E.hand_init(state);
    skinWire(state);
    earsPtr = E.ears_init(state);
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
      E.wire_replay(net.wire, world);
    }
    if (cam) stateF32().set(cam, 19);

    if (W) for (const k of ["terrainBuf", "pineInst", "poplarInst", "flowerInst", "crystalInst", "grassInst", "waterBuf", "flyBuf", "shadowTex"]) W[k]?.destroy?.();
    const counts = {
      terrain: Number(E.terrain_verts(state)),
      pines: Number(E.pine_count(state)),
      crystals: Number(E.crystal_count(state)),
      grass: Number(E.grass_count(state)),
      poplars: Number(E.poplar_count(state)),
      flowers: Number(E.flower_count(state)),
    };
    W = { counts };
    W.terrainBuf = gpuBuf(new Float32Array(E.memory.buffer, Number(E.terrain(state)), counts.terrain * 9));
    W.blocks = Number(E.blocks_count(state));
    W.blockInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.blocks(state)), Math.max(W.blocks, 1) * 8));
    W.pineInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.pines(state)), counts.pines * 8));
    W.poplarInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.poplars(state)),
                                           Math.max(counts.poplars, 1) * 8));
    W.flowerInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.flowers(state)),
                                           Math.max(counts.flowers, 1) * 8));
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

    W.tris = Math.round((counts.terrain + pineVerts * counts.pines + poplarVerts * counts.poplars
                         + crystalVerts * counts.crystals
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
      net = join(server, name, E);
      await net.arrival;
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
      if (W.counts.poplars) {
        p.setVertexBuffer(0, poplarBuf); p.setVertexBuffer(1, W.poplarInst);
        p.draw(poplarVerts, W.counts.poplars);
      }
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
    // Cutting the orders short is the same click that takes the pointer, so
    // the lock is asked for first and this comes after it: one press both
    // stops the briefing and puts you in the valley.
    //
    // The same cut lives on `mousedown` as well, for the ordinary case where
    // the pointer is already locked — there it also has to swallow the shot.
    // Here it does not: with no lock there is no trigger to swallow, and this
    // is the path a click takes when the browser refused the lock.
    if (briefOpen >= 0 || briefAt >= 0) { cutBriefing(); return; }
    // The card, now — before the audio is so much as asked to wake up, which
    // is the whole point: see the two clocks by `briefOpen`.
    if (newcomer && !welcomed) briefOpen = performance.now() / 1000;
    // The first gesture: a browser will start audio on it, and if this is
    // somebody's first time in the valley it is also when they are greeted.
    ensureAudio(() => { if (newcomer) sayWelcome(); });
  });
  addEventListener("mousemove", e => {
    if (document.pointerLockElement === canvas) { mdx += e.movementX; mdy += e.movementY; }
  });

  // The trigger. Only once the mouse is captured — the first click is what
  // captures it, and firing on that one would put a round downrange before
  // anyone had a chance to aim.
  // A bolt action fires once and then has to be worked. `boltT` runs from zero
  // The launcher is slower to bring back on target than a bolt is to work: the
  // whole of the propellant leaves the front of it and there is a tube to
  // reload. Two and a half seconds is what stops a rocket being a rifle.
  const RPG_CYCLE = 2.5;
  let wantShot = false, wantReload = false, fireHeld = false, wasReady = true, zapSeen = 0;
  // The hand block's fixed indices, hand.rill's own — mirrored once.
  const HAND = { held: 0, swap: 2, bolt: 4, aim: 5, recoil: 6, flash: 7,
                 hold: 14, zoom: 16, mdx: 17, mdy: 18, evN: 19, ev0: 20,
                 ready: 36, cph: 37, boltFrac: 38 };
  const handF = () => new Float32Array(E.memory.buffer, Number(E.hand_ptr(state)), 64);
  // Mirrored out of the hand each frame — everything below that keys off the
  // weapon or the sights reads these.
  let recoil = 0, flash = 0, boltT = 0;
  let ended = false, lastStanding = -1, wantRestart = false, lastOver = 0;


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
  // When this player last told the server anything. See the send below.
  let lastSent = -1e9;
  // Left to fire, right held to aim. Both while the mouse is captured, and
  // both at once: the whole point of holding the right button is to press the
  // left one while it is down.
  addEventListener("mousedown", e => {
    if (document.pointerLockElement !== canvas) return;
    // The orders are read to you, not at you: the left button cuts them off
    // and hands the valley over. It swallows that click rather than also
    // firing, because the button was pressed to stop the briefing and a round
    // going downrange as well would be a second thing nobody asked for.
    if (e.button === 0 && (briefOpen >= 0 || briefAt >= 0)) { cutBriefing(); return; }
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
    // The hands, in the engine: the swap machine, the bolt clock, the aim
    // and its zoom, the cone, the climb and its debt, the refusals — one call.
    // What comes back is the adjusted mouse, the uniform values, and a ring
    // of sound cues, each the page's only remaining share of the trigger.
    {
      const flags = (wantShot ? 1 : 0) | (fireHeld ? 2 : 0) | (wantAim ? 4 : 0) | (wantReload ? 8 : 0);
      const evn = Number(E.hand_frame(state, dt, performance.now(), t, mdx, mdy, BigInt(flags), BigInt(wantHeld)));
      wantShot = false; wantReload = false;
      const hf = handF();
      held = hf[HAND.held];
      aim = hf[HAND.aim]; swap = hf[HAND.swap];
      recoil = hf[HAND.recoil]; flash = hf[HAND.flash]; boltT = hf[HAND.bolt];
      mdx = hf[HAND.mdx]; mdy = hf[HAND.mdy];
      const ready = hf[HAND.ready] > 0.5;
      if (ready !== wasReady) { document.body.classList.toggle("cycling", !ready); wasReady = ready; }
      for (let i = 0; i < evn; i++) {
        const kind = hf[HAND.ev0 + i * 2];
        if (kind === 1) bang();
        else if (kind === 2) akBang();
        else if (kind === 3) rpgBang();
        else if (kind === 4) akReload();
        else if (kind === 5) dryClick();
        else if (kind === 6 && net) net.fired(hf[40], hf[41], hf[42]);
        else if (kind === 7 && net) net.reloaded();
      }
    }

    if (ended && wantRestart && !net) {
      // A new valley deserves a new corner of it. `restart` resets the world's
      // clock along with everything else, so without a fresh roll the second
      // run would begin exactly where the first one did.
      spawnRoll = Math.random() * 4096;
      E.spawn_salt(state, spawnRoll);
      E.restart(state);
      ended = false;
      lastHp = -1; lastStanding = -1;
      E.ears_hush(state);
      zapSeen = Number(E.zaps(state));
      hudHead(HH.over, 0);
    }
    wantRestart = false;
    wantRestart = false;


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
        // And the eleventh is the pack, for the same reason again and more
        // sharply: arming it is a *pair* of presses, and a pair is two edges,
        // and an edge is exactly what a twenty-a-second tick drops. The page
        // sees both at sixty and sends the answer — see `jet_told` in
        // valley.rill.
        net.input(keys | (held << 7) | (torchOn ? 512 : 0)
                       | (Number(E.jet_armed_now(state)) ? 1024 : 0),
                  look[19], look[20]);
      }
      // One call is the whole of what used to be applyEvents + reconcile +
      // the shark + the roster + the tags + the board: the wire drains what
      // arrived into the world, squares this player with the newest snapshot,
      // interpolates everybody and fills the HUD's rows.
      wireN = Number(E.wire_tick(net.wire, world, state, now,
                                 performance.now() < handF()[HAND.hold] ? 1n : 0n));
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
      // The shark is the server's too, interpolated by the wire alongside the
      // players; the header carries where it is right now.
      const wf = wireF32();
      if (wf[WH.hello] > 0.5) {
        const mode = wf[WH.sharkX + 4];
        E.set_shark(world, wf[WH.sharkX], wf[WH.sharkX + 1], wf[WH.sharkX + 2],
                    wf[WH.sharkX + 3], mode, mode > 2.5 ? 0.0 : (mode > 0.5 ? 0.85 : 0.0));
      }
      E.look(state, canvas.width / canvas.height);
    } else {
      E.frame(state, dt, mdx, mdy, BigInt(keys), canvas.width / canvas.height);
    }
    mdx = 0; mdy = 0;

    // The other people, as instances: where they are, how big, which way they
    // are facing, and their colour. Written every frame because it is a
    // kilobyte and they are always moving.
    if (net) {
      // The whole figure pass is the engine's now: clocks, poses, drops,
      // ghosts and instance floats in one call, three regions to upload.
      figureCount = Number(E.skin_frame(state, net.wire, now / 1000));
      const sb = Number(E.skin_ptr(state));
      heldCount = new Float32Array(E.memory.buffer, sb + 11 * 4, 1)[0];
      if (figureCount) {
        device.queue.writeBuffer(figureInst, 0,
          new Float32Array(E.memory.buffer, sb + Number(E.skin_inst_at()) * 4, figureCount * 8));
        device.queue.writeBuffer(poseBuf, 0,
          new Float32Array(E.memory.buffer, sb + Number(E.skin_out_at()) * 4, figureCount * figure.joints * 16));
      }
      if (heldCount) {
        // The engine leaves one matrix and one kind per slot; the page groups
        // them — rifles, then launchers, then AKs — so each mesh is one draw
        // with a firstInstance, and lays the right quantization box beside
        // each matrix on the way through.
        const mats = new Float32Array(E.memory.buffer, sb + Number(E.skin_held_at()) * 4, heldCount * 16);
        const kinds = new Float32Array(E.memory.buffer, sb + Number(E.skin_heldk_at()) * 4, heldCount);
        const boxes = [rifleBox, rpg.box, akBox];
        const grouped = new Float32Array(heldCount * 24);
        let at = 0;
        for (let k = 0; k < 3; k++) {
          for (let i = 0; i < heldCount; i++) {
            const ki = kinds[i] | 0;
            if ((ki === k) || (k === 0 && (ki < 0 || ki > 2))) {
              const o = at * 24;
              grouped.set(mats.subarray(i * 16, i * 16 + 16), o);
              const box = boxes[k];
              grouped[o + 16] = box[0]; grouped[o + 17] = box[1]; grouped[o + 18] = box[2];
              grouped[o + 19] = k;
              grouped[o + 20] = box[3]; grouped[o + 21] = box[4]; grouped[o + 22] = box[5];
              at++;
            }
          }
          heldSplit[k] = at;
        }
        device.queue.writeBuffer(heldBuf, 0, grouped);
      }
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

    // The packs that are burning — whoever the wire says, and this player,
    // whom no snapshot mentions. Gathered in both kinds of valley, because a
    // browser on its own has no figures and still has a pack.
    {
      const jb = Number(E.skin_ptr(state));
      jetCount = Math.min(Number(E.skin_jets(state, net ? net.wire : 0, t)), JET_CAP);
      if (jetCount) {
        device.queue.writeBuffer(jetBuf, 0,
          new Float32Array(E.memory.buffer, jb + Number(E.skin_jets_at()) * 4, jetCount * 8));
      }
    }

    // The ears. Everything the page used to work out before playing a sound —
    // watching the world's counters, measuring each event against the
    // listener, choosing gain, pan, pitch and variant, tracking the three
    // nearest rounds in the air — is the engine's (ears.rill). One call, then
    // two reads: a ring of finished cues to play verbatim, and per-voice
    // targets for the roars to ease toward.
    E.ears_frame(state, net ? net.wire : 0, t);
    const ears = earsF32();
    const cueN = ears[EARS.n];
    if (cueN) ensureAudio(() => {
      const bufs = [null, breakBuf, blastSnd, zapSndBuf, pickBuf, hurtBuf];
      for (let i = 0; i < cueN; i++) {
        const a = EARS.cues + i * 8;
        playAt(bufs[ears[a] | 0], ears[a + 1], ears[a + 2],
               ears[a + 3], ears[a + 4], ears[a + 5]);
      }
    });
    if (roars) {
      const rnow = audio.currentTime;
      for (let v = 0; v < roars.length; v++) {
        const a = EARS.roar + v * 4;
        roars[v].gain.gain.setTargetAtTime(ears[a], rnow, 0.10);
        // A voice with nothing to follow keeps its last pan and filter while
        // its gain falls, exactly as before: easing toward a default is
        // audible, easing toward silence is not.
        if (ears[a + 3] > 0) {
          roars[v].pan.pan.setTargetAtTime(ears[a + 1], rnow, 0.10);
          roars[v].filt.frequency.setTargetAtTime(ears[a + 2], rnow, 0.15);
        }
      }
    }
    // What the wire left as page events: the ears have read theirs above (the
    // hurt), the skinner's flinch clock takes its own here, and only then is
    // the ring cleared.
    if (net) {
      const pev = wirePev();
      const pn = wireF32()[18];
      for (let i = 0; i < pn; i++) {
        if (pev[i * 4] === 1) E.skin_fired(state, BigInt(pev[i * 4 + 1]), now / 1000);
      }
      if (pn) wireF32()[18] = 0;
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
          const q = wireSeen().find(p => p.id === zwho);
          if (!q) continue;
          tx = q.x; ty = q.y + 0.12; tz = q.z;
        }
        zapsLive.push({ ax: cryF[ca], ay: cryF[ca + 1] + cryF[ca + 3] * 1.05, az: cryF[ca + 2],
                        bx: tx, by: ty, bz: tz,
                        r: cryF[ca + 5], g: cryF[ca + 6], b: cryF[ca + 7], born: sky });
        if (zapsLive.length > ZAP_MAX) zapsLive.shift();
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
    // Only what the feet are doing. The bob is a stride — the weapon swinging
    // because a leg swung — and there is no stride in the air: flying across
    // the valley on the pack moved the gun exactly as walking across it did,
    // which read as running on nothing. Slot 25 is the world's own word for
    // whether the feet are down, so a jump stills it too. Eased rather than
    // cut, or the gun would stop dead the frame you left the ground.
    const onFoot = st[25] > 0.5;
    speed += ((onFoot ? Math.min(inst, 12) : 0) - speed) * 0.15;
    bobPhase += speed * dt * 2.4;
    uf[45] = speed; uf[46] = bobPhase; uf[47] = recoil;
    uf[116] = flash;                      // and the muzzle's own, see `fx`
    // How far down on one knee, for the weapon in the hands — see `vmCam`.
    uf[117] = E.crouch_of(state);
    uf.set(rifleBox.subarray(0, 3), 48);  // the rifle's bounding box, to unquantize against
    uf.set(rifleBox.subarray(3, 6), 52);
    uf.set(akBox.subarray(0, 3), 108);    // and the AK's, for its own pipeline
    uf.set(akBox.subarray(3, 6), 112);
    // How far through a magazine change the hands are, in the box's spare
    // fourth slot — nought when there is nothing going on, which is what the
    // shader tests. Floored a hair above nought on the first frame so that
    // "just started" is never mistaken for "not happening"; nothing has moved
    // at that phase anyway.
    uf[111] = handF()[HAND.cph];          // how far through the magazine change
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
    uf[51] = handF()[HAND.boltFrac];      // the AK has no bolt to work
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
      const lit = wireSeen().filter(p => p.torch && p.health > 0.001);
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
    if (world) drawRound(Number(E.match_phase(world)));
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
      // The orders, if they are being read: which words are out, and how far
      // the card is up. Before the layout runs, so the frame that draws them
      // is the frame they were decided in.
      stepBriefing();
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
    if (W.counts.poplars) {
      sh.setVertexBuffer(0, poplarShadowBuf); sh.setVertexBuffer(1, W.poplarInst);
      sh.draw(poplarShadowVerts, W.counts.poplars);
    }
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
    if (W.counts.poplars) {
      pass.setVertexBuffer(0, poplarBuf); pass.setVertexBuffer(1, W.poplarInst);
      pass.draw(poplarVerts, W.counts.poplars);
    }
    pass.setPipeline(grassPipe);
    pass.setVertexBuffer(0, grassBuf); pass.setVertexBuffer(1, W.grassInst); pass.draw(grassVerts, W.counts.grass);
    if (W.counts.flowers) {
      pass.setPipeline(flowerPipe);
      pass.setVertexBuffer(0, grassBuf); pass.setVertexBuffer(1, W.flowerInst);
      pass.draw(grassVerts, W.counts.flowers);
    }
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
      const runs = [
        [rifleVB, rifleIB, rifleIdx, 0, heldSplit[0]],
        [rpg.vb, rpg.ib, rpg.count, heldSplit[0], heldSplit[1]],
        [akVB, akIB, akIdx, heldSplit[1], heldSplit[2]],
      ];
      for (const [vb, ib, count, a, b] of runs) {
        if (b > a) {
          pass.setVertexBuffer(0, vb);
          pass.setIndexBuffer(ib, "uint16");
          pass.drawIndexed(count, b - a, 0, 0, a);
        }
      }
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
    if (jetCount) {
      pass.setPipeline(jetPipe);
      pass.setVertexBuffer(0, jetBuf);
      pass.draw(6, jetCount);
    }
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
        if (ms > 0 && ms < 100) {
          gpuMs = gpuMs * 0.9 + ms * 0.1;
          // A hook for measuring the renderer against itself. The smoothed
          // figure is the right one for the HUD and the wrong one to profile
          // with: it needs a second of frames to answer, and a tab that is not
          // in front is not producing frames. Set `window.__gpuRaw = []` and
          // every frame's own number lands in it, to be taken a median of.
          if (window.__gpuRaw) window.__gpuRaw.push(ms);
        }
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
      const kb = n => `${(n / 1024).toLocaleString("en", { maximumFractionDigits: 0 })} KB`;
      const here = net ? Number(E.wire_here(net.wire)) || null : null;
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
      hudSlot(HSLOT.stat + 4, [[D, "assets "], [G, kb(pakRawTotal)], [D, " → "], [G, kb(pakWire)],
        [D, " in one file, unpacked in wasm"]]);
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
