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

import { load } from "./world.js";

// ---- what the machine can afford -------------------------------------------

const TIERS = [
  { name: "low",    grid:  72, pines: 300, crystals: 120, grass:  800, flies:  500, shadow: 1024, water:  48, dpr: 1.0, waves: 5, ripples: 2 },
  { name: "medium", grid:  96, pines: 420, crystals: 200, grass: 1500, flies: 1200, shadow: 2048, water:  72, dpr: 1.5, waves: 6, ripples: 3 },
  { name: "high",   grid: 128, pines: 650, crystals: 300, grass: 3000, flies: 2500, shadow: 2048, water:  96, dpr: 2.0, waves: 7, ripples: 3 },
  { name: "ultra",  grid: 160, pines: 900, crystals: 380, grass: 5000, flies: 4000, shadow: 4096, water: 128, dpr: 3.0, waves: 8, ripples: 4 },
];

// A guess to start from, no more: the measurements take over immediately.
function guessTier(info) {
  const desc = `${info?.vendor ?? ""} ${info?.architecture ?? ""} ${info?.device ?? ""} ${info?.description ?? ""}`.toLowerCase();
  if (/swiftshader|llvmpipe|software/.test(desc)) return 0;
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
  fireball: "common", blast: "common", meteor: "common",
  sky: "common", water: "common", glow: "common", "firefly-draw": "common",
  "shadow-terrain": "uniforms", "shadow-instanced": "uniforms",
  "shadow-pine": "uniforms",
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
];

async function loadShaders(step) {
  const src = Object.fromEntries(await Promise.all(
    Object.keys(SHADERS).map(async name => {
      const r = await fetch(`./shaders/${name}.wgsl`);
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
    SHADOW_TERRAIN: whole("shadow-terrain"),
    SHADOW_INST: whole("shadow-instanced"),
    SHADOW_PINE: whole("shadow-pine"),
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
function pineMesh() {
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
  for (let i = 0; i < 12; i++) {
    const u = i / 11;                       // 0 at the lowest whorl, 1 at the spire
    tiers.push({
      y: 0.95 + u * 1.60,
      len: 0.22 + 1.05 * (1 - u) ** 1.1,
      tall: 0.30 + 0.75 * (1 - u),
      drop: 0.10 + 0.22 * (1 - u),
      n: Math.round(9 - 4 * u),
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

function faceNormal(a, b, c) {
  const u = [b[0]-a[0], b[1]-a[1], b[2]-a[2]], v = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
  const n = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]];
  const l = Math.hypot(...n) || 1;
  return [n[0]/l, n[1]/l, n[2]/l];
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

export async function run(canvas, wasmURL, hud, progress = () => {}) {
  // Everything the first frame needs, counted up front so the loader can say
  // how far along it is: the adapter, the module, one step per shader, one per
  // texture, and the valley itself. `run` does not resolve until a frame has
  // actually been submitted, so whatever is covering the canvas comes off over
  // a painted one rather than over black.
  const TOTAL = 9 + Object.keys(SHADERS).length + TEXTURES.length;
  let done = 0;
  const step = (what) => progress(++done, TOTAL, what);

  if (!navigator.gpu) throw new Error("this browser has no WebGPU");
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) throw new Error("no GPU adapter");

  // The one feature worth asking for: GPU timestamps, so the HUD can show
  // what a frame costs the GPU rather than what rAF implies.
  const canTime = adapter.features.has("timestamp-query");
  const device = await adapter.requestDevice({
    requiredFeatures: canTime ? ["timestamp-query"] : [],
  });
  device.addEventListener("uncapturederror", e => console.error("WEBGPU:", e.error.message));
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
    const e = performance.getEntriesByType("resource").find(r => r.name.endsWith(".wasm"));
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
  const uni = device.createBuffer({ size: 56 * 4, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
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
  const [foliageView, barkView, groundView, plantView, rockView, sandView, rifleView, meteorView] =
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

  const pipe = (label, code, buffers, opts = {}) => device.createRenderPipeline({
    label, layout,
    vertex: { module: device.createShaderModule({ code }), buffers, constants: opts.constants },
    fragment: { module: device.createShaderModule({ code }),
                targets: [{ format: opts.format ?? format, blend: opts.blend }], constants: opts.constants },
    primitive: { cullMode: opts.cull ?? "back" },
    depthStencil: { ...depth, ...opts.depth },
    multisample: MS,
  });

  const skyPipe = pipe("sky", S.SKY, [], { depth: { depthWriteEnabled: false, depthCompare: "less-equal" }, cull: "none" });
  const terrainPipe = pipe("terrain", S.TERRAIN, terrainLayout);
  const pinePipe = pipe("pine", S.PINE, [pineLayout, instLayout], { cull: "none" });
  const crystalPipe = pipe("crystal", S.INSTANCED, [meshLayout, instLayout], { constants: { emissive: 0.72 } });
  const grassPipe = pipe("grass", S.GRASS, [pineLayout, instLayout], { cull: "none" });
  const glowPipe = pipe("glow", S.GLOW, [instLayout], { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  // Backfaces culled, unlike the foliage: the butt sits behind the eye and is
  // cut open by the near plane, and a culled interior reads as "not there"
  // where an unculled one reads as a dark wall across the bottom of the screen.
  const vmPipe = pipe("viewmodel", S.VIEWMODEL, vmLayout);
  // Additive and depth-tested but not depth-writing: a tracer is light in the
  // air, so the ground hides it and it hides nothing.
  const tracerPipe = pipe("tracer", S.TRACER, bulletLayout,
    { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });
  // Same treatment as the tracers: additive, depth-tested so the land hides
  // them, never writing depth of their own.
  // Premultiplied alpha: the shader hands over colour already multiplied by
  // coverage, and coverage is what limits how much of a pixel each puff may
  // claim. Additive has no such limit, which is fine for one spark and ruinous
  // for fifty of them stacked along a line of sight.
  const overPre = { color: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
                    alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" } };
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
  const muzzlePipe = pipe("muzzle", S.MUZZLE, [],
    { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });

  // The rifle: one file, its own header. Two counts, then the bounding box the
  // positions were quantized against, then the vertices, then the indices.
  const rifleBytes = await (await fetch("./rifle.mesh")).arrayBuffer();
  const rifleHead = new DataView(rifleBytes, 0, 32);
  const rifleVerts = rifleHead.getUint32(0, true);
  const rifleIdx = rifleHead.getUint32(4, true);
  const rifleBox = new Float32Array(rifleBytes, 8, 6);
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
  let roarBuf = null, roars = null;
  let musicGain = null, musicOn = true;
  const shotBytes = await (await fetch("./rifle-shot.mp3")).arrayBuffer();
  step("rifle-shot.mp3");
  const breakBytes = await (await fetch("./crystal-break.mp3")).arrayBuffer();
  step("crystal-break.mp3");
  // Two explosions, 1.9 seconds each, laid end to end and played by offset.
  const blastBytes = await (await fetch("./meteor-blast.mp3")).arrayBuffer();
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
  const roarBytes = await (await fetch("./meteor-air.mp3")).arrayBuffer();
  step("meteor-air.mp3");
  // The theme. Eighty seconds that begin and end in silence, which is what
  // makes it loop without a seam: no crossfade, no loop points, just the tail
  // running into the head the way the piece was already written.
  const musicBytes = await (await fetch("./valley-theme.mp3")).arrayBuffer();
  step("valley-theme.mp3");
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
  function ensureAudio(then) {
    if (!audio) {
      audio = new (globalThis.AudioContext ?? globalThis.webkitAudioContext)();
      musicGain = audio.createGain();
      musicGain.gain.value = 0;
      musicGain.connect(audio.destination);
      Promise.all([
        audio.decodeAudioData(shotBytes.slice(0)).then(b => { shotBuf = b; }),
        audio.decodeAudioData(breakBytes.slice(0)).then(b => { breakBuf = b; }),
        audio.decodeAudioData(blastBytes.slice(0)).then(b => { blastSnd = b; }),
        audio.decodeAudioData(roarBytes.slice(0)).then(b => { roarBuf = b; startRoars(); }),
        audio.decodeAudioData(musicBytes.slice(0)).then(b => { musicBuf = b; startMusic(); }),
      ]).then(then);
      return;
    }
    if (audio.state === "suspended") audio.resume();
    then();
  }

  // Three voices, started once and left running for the life of the page, each
  // following one fireball. Not one voice per round — twenty-four looping
  // sources for something you can only ever hear three of is a waste, and the
  // fourth-nearest is inaudible under the other three anyway. Each frame the
  // rounds in the air are sorted by distance and the nearest three are handed
  // to the voices; a voice with nothing to follow is turned down rather than
  // stopped, because starting and stopping a loop is what clicks.
  // Where the roar gives out entirely — just inside the forty-five metres of
  // `fireball_reach` in world.rill, which is the nearest a crystal may be and
  // still throw. So a round is silent for the whole first half of its flight
  // and only announces itself once it is genuinely close.
  //
  // A hard edge and a steep curve do different jobs here. The edge is what
  // stops six rounds scattered across the valley adding up to a permanent
  // hiss: three voices each following something forty metres off is a wash of
  // noise with nothing to attach it to, and it makes the place sound like it
  // is on fire rather than like something is coming at you. The curve — cubed,
  // so nine tenths of the loudness is spent in the last fifteen metres — is
  // what makes the approach itself audible.
  //
  // The explosions are the opposite case and are tuned the opposite way: a
  // half-second impact carries across a valley and should, so `boom` rolls off
  // over a hundred and ten metres. Continuous and distant is noise; loud and
  // brief and distant is an event.
  const REACH = 46.0;
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

  // Started once and left running. Fading it up over four seconds rather than
  // cutting it in matters more than it sounds like it should: the theme opens
  // on a held chord, and a held chord that begins at full volume the instant
  // you click reads as a jingle rather than as somewhere you have arrived.
  function startMusic() {
    const src = audio.createBufferSource();
    src.buffer = musicBuf;
    src.loop = true;
    src.connect(musicGain);
    src.start();
    setMusic(musicOn);
  }
  function setMusic(on) {
    musicOn = on;
    if (!musicGain) return;
    const now = audio.currentTime;
    musicGain.gain.cancelScheduledValues(now);
    musicGain.gain.setValueAtTime(musicGain.gain.value, now);
    musicGain.gain.linearRampToValueAtTime(on ? 0.17 : 0.0, now + (on ? 4.0 : 0.6));
  }
  function bang() {
    ensureAudio(() => playAt(shotBuf, 0, 0, 0.55 + Math.random() * 0.12, 0,
                             0.97 + Math.random() * 0.06));
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
  // A fireball landing. Carries much further than breaking glass does, so it
  // falls off over a much longer distance — and slower and deeper the further
  // off it is, which is most of what makes a big noise sound far away.
  function boom(x, y, z) {
    ensureAudio(() => {
      // Half as loud at a hundred and ten metres, which is a long way for a
      // half-second impact and deliberately so: the valley is bigger than the
      // range these were first tuned for, and an explosion you cannot hear
      // across it reads as a silent flash on the far hillside rather than as
      // something that happened. The pitch drop is spread over the same longer
      // distance, or everything past ninety metres arrives equally deep and
      // the cue stops carrying any information.
      const p = place(x, y, z, 110);
      const far = Math.min(p.d / 150, 1);
      playAt(blastSnd, Math.floor(Math.random() * 2) * 1.9, 1.85,
             0.85 * p.gain, p.pan, 1.06 - 0.22 * far);
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
  const shardBuf = device.createBuffer({
    size: shardCap * 80, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });

  const flyBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: { type: "read-only-storage" } },
  ]});
  const flyDrawPipe = device.createRenderPipeline({
    label: "fireflies",
    layout: device.createPipelineLayout({ bindGroupLayouts: [bglMain, flyBgl] }),
    vertex: { module: device.createShaderModule({ code: S.FIREFLY_DRAW }) },
    fragment: { module: device.createShaderModule({ code: S.FIREFLY_DRAW }), targets: [{ format, blend: additive }] },
    primitive: { cullMode: "none" },
    depthStencil: { ...depth, depthWriteEnabled: false },
    multisample: MS,
  });
  const simBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: "storage" } },
    { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: {} },
  ]});
  const simPipe = device.createComputePipeline({
    label: "fly-sim",
    layout: device.createPipelineLayout({ bindGroupLayouts: [simBgl] }),
    compute: { module: device.createShaderModule({ code: S.FIREFLY_SIM }) },
  });

  const shadowLayout = device.createPipelineLayout({ bindGroupLayouts: [bglShadow] });
  const shadowPipe = (label, code, buffers, cutout = false) => device.createRenderPipeline({
    label, layout: shadowLayout,
    vertex: { module: device.createShaderModule({ code }), buffers },
    // Depth only, unless the thing being drawn is mostly holes.
    fragment: cutout
      ? { module: device.createShaderModule({ code }), entryPoint: "fs", targets: [] }
      : undefined,
    primitive: { cullMode: "none" },
    depthStencil: { format: "depth32float", depthWriteEnabled: true, depthCompare: "less" },
  });
  const shTerrain = shadowPipe("shadow-terrain", S.SHADOW_TERRAIN, terrainLayout);
  const shInst = shadowPipe("shadow-inst", S.SHADOW_INST, [meshLayout, instLayout]);
  const shPine = shadowPipe("shadow-pine", S.SHADOW_PINE, [pineLayout, instLayout], true);

  const pineData = pineMesh(), crystalData = crystalMesh(), grassData = grassMesh();
  const pineBuf = gpuBuf(pineData), crystalBuf = gpuBuf(crystalData), grassBuf = gpuBuf(grassData);
  const pineVerts = pineData.length / 8, crystalVerts = crystalData.length / 6, grassVerts = grassData.length / 8;

  // GPU frame timing, when the adapter offers it.
  let qs = null, qResolve = null, qStage = null, qPending = false, gpuMs = 0;
  if (canTime) {
    qs = device.createQuerySet({ type: "timestamp", count: 2 });
    qResolve = device.createBuffer({ size: 16, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
    qStage = device.createBuffer({ size: 16, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
  }

  // ---- everything the tier decides, rebuilt when the tier changes ----------

  let tierIdx = guessTier(gpuInfo);
  let T = null;       // the active tier
  let state = 0;      // the world, as a pointer into wasm memory
  let W = null;       // tier-dependent GPU resources
  const stateF32 = () => new Float32Array(E.memory.buffer, Number(state), 64);

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

  function buildWorld(idx) {
    T = TIERS[idx];
    // The camera survives the world being rebuilt around it.
    let cam = null;
    if (state) { cam = stateF32().slice(19, 26); E.free_world(state); }
    state = E.init(BigInt(T.grid), BigInt(T.pines), BigInt(T.crystals), BigInt(T.grass));
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

    W.tris = Math.round((counts.terrain + pineVerts * counts.pines + crystalVerts * counts.crystals
                       + grassVerts * counts.grass + rifleIdx) / 3);
  }
  buildWorld(tierIdx);
  step("the valley");

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
    const V = [ x[0], y[0], z[0], 0,
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
      for (let k = 0; k < 4; k++) s += P[k * 4 + r] * V[c * 4 + k];
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
    const dpr = Math.min(devicePixelRatio, T.dpr);
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
  const bit = { KeyW: 1, KeyS: 2, KeyA: 4, KeyD: 8, Space: 16, ShiftLeft: 32, ShiftRight: 32 };
  addEventListener("keydown", e => { if (bit[e.code]) { keys |= bit[e.code]; e.preventDefault(); } });
  addEventListener("keydown", e => { if (e.code === "KeyM") setMusic(!musicOn); });
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
  let wantShot = false, recoil = 0, boltT = BOLT_CYCLE, wasReady = true, breaks = 0, blastsHeard = 0;
  let ended = false;
  addEventListener("mousedown", e => {
    if (e.button === 0 && document.pointerLockElement === canvas) wantShot = true;
  });

  // The adaptive loop's bookkeeping. The display's own frame interval is the
  // budget — measured, not assumed, so a 120 Hz screen asks for more.
  const dts = [];
  let refresh = 1000 / 60, warm = 0, changes = 0, stable = 0, locked = false;
  const median = a => [...a].sort((x, y) => x - y)[a.length >> 1];

  // Where the sun stands before the world has turned, and how fast it turns:
  // one revolution in half an hour. `shaders/common.wgsl` carries the same
  // rate, because the sky's own stars and bodies have to swing with it.
  const SUN_BASE = [0.58, -0.62, -0.52];
  const SKY_SPIN = 2 * Math.PI / 1800;
  const uf = new Float32Array(56);
  let lastX = 0, lastZ = 0, speed = 0, bobPhase = 0;
  // Resolved by the frame loop the first time it actually submits, so `run`
  // does not return on a canvas that has nothing on it yet.
  let painted = null;
  const firstFrame = new Promise(r => { painted = r; });

  let last = performance.now(), fps = 0, frames = 0, fpsT = last;

  function tick(now) {
    resize();
    const dt = Math.min((now - last) / 1000, 0.05);
    const dtMs = now - last;
    last = now;
    const t = now / 1000;

    // Settle the tier: warm up, measure, step, lock.
    warm++;
    if (warm > 20) {
      dts.push(dtMs);
      refresh = Math.min(refresh, Math.max(dtMs, 4));
      if (!locked && dts.length >= 48) {
        const med = median(dts);
        dts.length = 0;
        if (med < refresh * 1.25 && tierIdx < TIERS.length - 1) { tierIdx++; changes++; buildWorld(tierIdx); }
        else if (med > refresh * 1.6 && tierIdx > 0) { tierIdx--; changes++; buildWorld(tierIdx); }
        else stable++;
        if (changes >= 3 || stable >= 2) locked = true;
      }
    }

    // The shot, before the frame that will fly it. Only with the bolt closed,
    // and only on a press: holding the button does nothing, because `wantShot`
    // is set by the event rather than read from the state.
    boltT = Math.min(boltT + dt, BOLT_CYCLE);
    // Say so on the page. The bolt itself is the honest indicator, but it is
    // off to the right of where anyone aiming is looking, so the crosshair
    // goes quiet while the rifle is open and comes back when it will fire.
    const ready = boltT >= BOLT_CYCLE;
    if (ready !== wasReady) { document.body.classList.toggle("cycling", !ready); wasReady = ready; }
    // Nothing fires from in the lake. Refused here rather than inside `fire`
    // so the bolt does not cycle and the bang does not play: a rifle that goes
    // off and does nothing is worse than one that will not go off.
    if (wantShot && boltT >= BOLT_CYCLE && !Number(E.wading(state))) {
      const st0 = stateF32();
      const yaw = st0[19], pit = st0[20], cp = Math.cos(pit);
      E.fire(state, Math.sin(yaw) * cp, Math.sin(pit), -Math.cos(yaw) * cp);
      boltT = 0;
      recoil = 1;
      bang();
    }
    wantShot = false;
    recoil *= Math.pow(0.02, dt);   // most of it gone in a fifth of a second

    E.frame(state, dt, mdx, mdy, BigInt(keys), canvas.width / canvas.height);
    mdx = 0; mdy = 0;

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
    // One turn in half an hour — twelve degrees a minute. Watch the shadow of a
    // pine and it is moving; look up at the sky and it is still.
    const spin = t * SKY_SPIN;
    const cs = Math.cos(spin), sn = Math.sin(spin);
    const sunX = SUN_BASE[0] * cs + SUN_BASE[2] * sn;
    const sunZ = -SUN_BASE[0] * sn + SUN_BASE[2] * cs;
    // Into the shadow matrix first, so what is copied out of the state below is
    // this frame's sun and not the last one's. There is one sun in this world:
    // the disc in the sky, the direction `lit` shades from and the matrix the
    // shadows are cast with are all the same three numbers.
    E.set_sun(state, sunX, SUN_BASE[1], sunZ);

    const st = stateF32();
    uf.set(st.subarray(0, 16), 0);        // vp
    uf.set(st.subarray(32, 48), 16);      // light vp
    uf.set(st.subarray(16, 19), 32);      // eye
    uf[35] = t;
    uf[36] = sunX; uf[37] = SUN_BASE[1]; uf[38] = sunZ;
    uf[39] = canvas.width / canvas.height;
    uf[40] = st[19]; uf[41] = st[20];     // yaw, pitch
    uf[42] = st[26]; uf[43] = st[27];     // wind angle, wind strength
    uf[44] = st[29];                      // the wind's integrated phase
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
    uf.set(rifleBox.subarray(0, 3), 48);  // the rifle's bounding box, to unquantize against
    uf.set(rifleBox.subarray(3, 6), 52);
    // The bolt's throw, as a fraction. The shot itself is the first instant of
    // it, so the recoil and the lift start together the way they do in a hand.
    uf[51] = boltT / BOLT_CYCLE;
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
    // And the end of it: a crystal that has been shot has its scale set to
    // zero, so the last one going is the whole condition. Read off the same
    // array that was about to be uploaded anyway, and only until it happens —
    // after that there is nothing to keep counting.
    if (!ended) {
      let alive = 0;
      for (let i = 0; i < W.counts.crystals; i++) if (cry[i * 8 + 3] > 0.001) { alive = 1; break; }
      if (!alive) { ended = true; document.getElementById("over").classList.add("on"); }
    }
    device.queue.writeBuffer(flyTime, 0, new Float32Array([t, dt, 0, 0]));

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
    sh.setVertexBuffer(0, pineBuf); sh.setVertexBuffer(1, W.pineInst); sh.draw(pineVerts, W.counts.pines);
    sh.setPipeline(shInst);
    sh.setVertexBuffer(0, crystalBuf); sh.setVertexBuffer(1, W.crystalInst); sh.draw(crystalVerts, W.counts.crystals);
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
    pass.setPipeline(crystalPipe);
    pass.setVertexBuffer(0, crystalBuf); pass.setVertexBuffer(1, W.crystalInst); pass.draw(crystalVerts, W.counts.crystals);
    pass.setPipeline(grassPipe);
    pass.setVertexBuffer(0, grassBuf); pass.setVertexBuffer(1, W.grassInst); pass.draw(grassVerts, W.counts.grass);
    pass.setPipeline(skyPipe); pass.draw(3);
    pass.setPipeline(W.waterPipe); pass.setVertexBuffer(0, W.waterBuf); pass.draw(W.waterVerts);
    pass.setPipeline(glowPipe); pass.setVertexBuffer(0, W.crystalInst); pass.draw(6, W.counts.crystals);
    pass.setPipeline(flyDrawPipe); pass.setBindGroup(1, W.flyDrawBg); pass.draw(6, T.flies);

    pass.setPipeline(shardPipe);
    pass.setVertexBuffer(0, crystalBuf); pass.setVertexBuffer(1, shardBuf);
    pass.draw(crystalVerts, shardCap);

    pass.setPipeline(tracerPipe); pass.setVertexBuffer(0, bulletBuf); pass.draw(6, bulletCap);
    pass.setPipeline(firePipe); pass.setVertexBuffer(0, fireInst); pass.draw(6 * 80, fireCap);
    // The stone goes down after the flames, not before: drawn first it sat
    // under an additive fire that simply burned it out, and the leading face —
    // the one thing that says there is something solid in there — was the part
    // that vanished first.
    pass.setPipeline(meteorPipe); pass.draw(3 * 11, fireCap);
    pass.setPipeline(blastPipe); pass.setVertexBuffer(0, blastInst); pass.draw(6 * 26, blastCap);

    // Last, so it covers the water and the fireflies as well as the ground.
    pass.setBindGroup(0, W.bgMain);
    pass.setPipeline(vmPipe);
    pass.setVertexBuffer(0, rifleVB);
    pass.setIndexBuffer(rifleIB, "uint16");
    pass.drawIndexed(rifleIdx);
    pass.setPipeline(muzzlePipe); pass.draw(12);
    pass.end();

    if (qs) {
      enc.resolveQuerySet(qs, 0, 2, qResolve, 0);
      if (!qPending) enc.copyBufferToBuffer(qResolve, 0, qStage, 0, 16);
    }
    device.queue.submit([enc.finish()]);
    // Once, at the end of the first frame — by which point the shadow map has
    // been drawn, so the valley in the cube is lit the same way the valley is.
    if (!W.envBaked) { W.envBaked = true; bakeEnv(uf); }
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
    if (now - fpsT > 500) {
      fps = Math.round(frames * 1000 / (now - fpsT));
      frames = 0; fpsT = now;
      hud({ fps, gpuMs: qs ? gpuMs : null, tier: T.name, gpuName, tris: W.tris, locked, wasmBytes: wasmSize() });
    }
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
  await firstFrame;
}
