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
  sky: "common", water: "common", glow: "common", "firefly-draw": "common",
  "shadow-terrain": "uniforms", "shadow-instanced": "uniforms",
  "shadow-pine": "uniforms",
  "firefly-sim": null,
};

// The images, and the square they are decoded to. Listed here rather than at
// the point of use so that the loader can count them before it starts.
const TEXTURES = [
  ["./pine-twig.png", 256],    // the needle atlas
  ["./pine-bark.jpg", 256],
  ["./grass-ground.jpg", 256],
  ["./plants.png", 512],       // the undergrowth atlas, 2x2
  ["./rock-face.jpg", 256],
  ["./shore-sand.jpg", 256],
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
  const TOTAL = 3 + Object.keys(SHADERS).length + TEXTURES.length;
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
  const uni = device.createBuffer({ size: 52 * 4, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
  const flyTime = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
  const shadowSamp = device.createSampler({ compare: "less" });

  // The two pine textures. Mip levels are made by asking the browser to decode
  // the file again at each size — cheaper to write than a blit pass, and alpha
  // tested foliage without mips shimmers the moment it is more than a few
  // trees away.
  async function mipped(url, size0) {
    const bytes = await (await fetch(url)).arrayBuffer();
    const levels = [];
    for (let size = size0; size >= 1; size >>= 1) {
      levels.push(await createImageBitmap(new Blob([bytes]),
        { resizeWidth: size, resizeHeight: size, resizeQuality: "high" }));
    }
    const tex = device.createTexture({
      size: [size0, size0], mipLevelCount: levels.length, format: "rgba8unorm",
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
  const [foliageView, barkView, groundView, plantView, rockView, sandView] =
    await Promise.all(TEXTURES.map(([url, size]) =>
      mipped(url, size).then(v => { step(url.slice(2)); return v; })));
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
    fragment: { module: device.createShaderModule({ code }), targets: [{ format, blend: opts.blend }], constants: opts.constants },
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

    W.tris = Math.round((counts.terrain + pineVerts * counts.pines + crystalVerts * counts.crystals + grassVerts * counts.grass) / 3);
  }
  buildWorld(tierIdx);
  step("the valley");

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
  addEventListener("keyup", e => { if (bit[e.code]) keys &= ~bit[e.code]; });
  canvas.addEventListener("click", () => canvas.requestPointerLock());
  addEventListener("mousemove", e => {
    if (document.pointerLockElement === canvas) { mdx += e.movementX; mdy += e.movementY; }
  });

  // The adaptive loop's bookkeeping. The display's own frame interval is the
  // budget — measured, not assumed, so a 120 Hz screen asks for more.
  const dts = [];
  let refresh = 1000 / 60, warm = 0, changes = 0, stable = 0, locked = false;
  const median = a => [...a].sort((x, y) => x - y)[a.length >> 1];

  const uf = new Float32Array(52);
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

    E.frame(state, dt, mdx, mdy, BigInt(keys), canvas.width / canvas.height);
    mdx = 0; mdy = 0;

    const st = stateF32();
    uf.set(st.subarray(0, 16), 0);        // vp
    uf.set(st.subarray(32, 48), 16);      // light vp
    uf.set(st.subarray(16, 19), 32);      // eye
    uf[35] = t;
    uf[36] = 0.58; uf[37] = -0.62; uf[38] = -0.52;
    uf[39] = canvas.width / canvas.height;
    uf[40] = st[19]; uf[41] = st[20];     // yaw, pitch
    uf[42] = st[26]; uf[43] = st[27];     // wind angle, wind strength
    uf[44] = st[29];                      // the wind's integrated phase
    device.queue.writeBuffer(uni, 0, uf);
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
    pass.end();

    if (qs) {
      enc.resolveQuerySet(qs, 0, 2, qResolve, 0);
      if (!qPending) enc.copyBufferToBuffer(qResolve, 0, qStage, 0, 16);
    }
    device.queue.submit([enc.finish()]);
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
