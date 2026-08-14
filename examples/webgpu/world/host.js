// The renderer half of the valley. Rill (world.wasm) computes the world —
// mesh, placement, camera, matrices — and this file spends the GPU on it:
// a shadow pass under a fixed golden-hour sun, a 4x MSAA forward pass with
// procedural sky, flat-shaded terrain, instanced pines and crystals, animated
// water, additive crystal glow, and a compute-shader swarm of fireflies.
//
// The two halves meet at one block of wasm memory whose layout world.rill
// writes down at the top of the file.

// ---- just enough WASI for a reactor (browsers ship none) -------------------

function wasiShim(getMemory) {
  const text = new TextDecoder();
  let carry = "";
  return {
    fd_write(fd, iovs, iovcnt, nwritten) {
      const view = new DataView(getMemory().buffer);
      let written = 0, out = "";
      for (let i = 0; i < iovcnt; i++) {
        const ptr = view.getUint32(iovs + i * 8, true);
        const len = view.getUint32(iovs + i * 8 + 4, true);
        out += text.decode(new Uint8Array(getMemory().buffer, ptr, len));
        written += len;
      }
      carry += out;
      const lines = carry.split("\n");
      carry = lines.pop();
      for (const line of lines) console.log(`[rill] ${line}`);
      view.setUint32(nwritten, written, true);
      return 0;
    },
    fd_close: () => 0, fd_seek: () => 0,
    fd_prestat_get: () => 8, fd_prestat_dir_name: () => 8,
    args_sizes_get(a, b) { const v = new DataView(getMemory().buffer); v.setUint32(a, 0, true); v.setUint32(b, 0, true); return 0; },
    args_get: () => 0,
    environ_sizes_get(a, b) { const v = new DataView(getMemory().buffer); v.setUint32(a, 0, true); v.setUint32(b, 0, true); return 0; },
    environ_get: () => 0,
    proc_exit(code) { throw new Error(`wasm exited with code ${code}`); },
  };
}

// ---- shaders ---------------------------------------------------------------
//
// One uniform block for everything. `sun` points the way the light travels —
// the same three numbers world.rill builds its shadow matrix from.

const UNI = /* wgsl */`
  struct U {
    vp:   mat4x4f,
    lvp:  mat4x4f,
    eye:  vec4f,   // xyz eye, w time
    sun:  vec4f,   // xyz light direction, w aspect
    cam:  vec4f,   // yaw, pitch, unused, unused
  }
  @group(0) @binding(0) var<uniform> u: U;
`;

// The shadow pass cannot see these bindings: it *writes* the shadow map, and
// a texture cannot be sampled in the pass that renders into it. So the depth
// only shaders build on UNI, everything else on COMMON.
const COMMON = UNI + /* wgsl */`
  @group(0) @binding(1) var shadowMap: texture_depth_2d;
  @group(0) @binding(2) var shadowSamp: sampler_comparison;

  // No early return: textureSampleCompare demands uniform control flow, so
  // the samples always run and the out-of-map case is selected afterwards.
  fn shadow(world: vec3f, nl: f32) -> f32 {
    let sc = u.lvp * vec4f(world, 1.0);
    let uvz = vec3f(sc.xy * vec2f(0.5, -0.5) + 0.5, sc.z);
    let bias = clamp(0.0028 * (1.0 - nl) + 0.0009, 0.0009, 0.004);
    var acc = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
      for (var dx = -1; dx <= 1; dx++) {
        acc += textureSampleCompare(shadowMap, shadowSamp, uvz.xy + vec2f(f32(dx), f32(dy)) / 2048.0, uvz.z - bias);
      }
    }
    let inside = uvz.x >= 0.0 && uvz.x <= 1.0 && uvz.y >= 0.0 && uvz.y <= 1.0 && uvz.z <= 1.0;
    return select(1.0, acc / 9.0, inside);
  }

  const SKY_ZENITH  = vec3f(0.043, 0.055, 0.16);
  const SKY_HORIZON = vec3f(0.23, 0.19, 0.28);
  const SUN_WARM    = vec3f(1.0, 0.62, 0.24);
  const SUN_COL     = vec3f(1.0, 0.85, 0.6);

  fn skyColor(dir: vec3f, t: f32) -> vec3f {
    let up = max(dir.y, 0.0);
    var col = mix(SKY_HORIZON, SKY_ZENITH, pow(up, 0.55));
    let sa = max(dot(dir, -normalize(u.sun.xyz)), 0.0);
    col += SUN_WARM * pow(sa, 5.0) * 0.5;             // the warm side of the sky
    col += SUN_COL * pow(sa, 900.0) * 20.0;           // the disc
    col += SUN_WARM * pow(sa, 60.0) * 1.2;            // its halo
    // stars, fading out toward the bright horizon
    let cell = floor(dir * 260.0);
    let h = fract(sin(dot(cell, vec3f(127.1, 311.7, 74.7))) * 43758.5453);
    if (h > 0.9982 && dir.y > 0.08) {
      col += vec3f(0.8) * up * (0.5 + 0.5 * sin(t * 2.0 + h * 40.0));
    }
    return col;
  }

  fn fogAmount(dist: f32) -> f32 { return 1.0 - exp(-dist * 0.0065); }

  fn fogColor(view: vec3f) -> vec3f {
    let sa = max(dot(normalize(vec3f(view.x, 0.0, view.z)), -normalize(vec3f(u.sun.x, 0.0, u.sun.z))), 0.0);
    return mix(vec3f(0.10, 0.11, 0.19), vec3f(0.55, 0.35, 0.22), pow(sa, 3.0));
  }

  fn lit(albedo: vec3f, n: vec3f, world: vec3f) -> vec3f {
    let L = -normalize(u.sun.xyz);
    let nl = max(dot(n, L), 0.0);
    let sh = shadow(world, nl);
    let sky = mix(vec3f(0.10, 0.11, 0.18), vec3f(0.18, 0.19, 0.30), n.y * 0.5 + 0.5);
    var col = albedo * (SUN_COL * 1.25 * nl * sh + sky * 1.1);
    let view = world - u.eye.xyz;
    return mix(col, fogColor(view), fogAmount(length(view)));
  }
`;

const TERRAIN = COMMON + /* wgsl */`
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }
  @vertex fn vs(@location(0) pos: vec3f, @location(1) n: vec3f, @location(2) c: vec3f) -> VOut {
    var o: VOut;
    o.clip = u.vp * vec4f(pos, 1.0);
    o.world = pos; o.normal = n; o.color = c;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    return vec4f(lit(v.color, normalize(v.normal), v.world), 1.0);
  }
`;

// Pines and crystals share one instanced shader; `emissive` in the instance
// colour's use is what tells them apart, chosen per pipeline by a constant.
const INSTANCED = COMMON + /* wgsl */`
  override emissive: f32 = 0.0;
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    let c = cos(rot); let s = sin(rot);
    let p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;
    let nr = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.normal = nr; o.color = icol;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let base = lit(v.color, normalize(v.normal), v.world);
    let pulse = 0.75 + 0.25 * sin(u.eye.w * 2.1 + v.world.x * 3.0);
    let glow = v.color * (1.35 * pulse);
    let view = v.world - u.eye.xyz;
    let glowFogged = mix(glow, fogColor(view), fogAmount(length(view)));
    return vec4f(mix(base, glowFogged, emissive), 1.0);
  }
`;

// Depth-only variants for the shadow pass.
const SHADOW_TERRAIN = UNI + /* wgsl */`
  @vertex fn vs(@location(0) pos: vec3f, @location(1) n: vec3f, @location(2) c: vec3f) -> @builtin(position) vec4f {
    return u.lvp * vec4f(pos, 1.0);
  }
`;
const SHADOW_INST = UNI + /* wgsl */`
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> @builtin(position) vec4f {
    let c = cos(rot); let s = sin(rot);
    let p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;
    return u.lvp * vec4f(p, 1.0);
  }
`;

const SKY = COMMON + /* wgsl */`
  struct VOut { @builtin(position) clip: vec4f, @location(0) ndc: vec2f }
  @vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let xy = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: VOut;
    o.clip = vec4f(xy, 1.0, 1.0);   // on the far plane
    o.ndc = xy;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    let upv = cross(right, fwd);
    let th = tan(0.53);
    let dir = normalize(fwd + right * v.ndc.x * th * u.sun.w + upv * v.ndc.y * th);
    return vec4f(skyColor(dir, u.eye.w), 1.0);
  }
`;

const WATER = COMMON + /* wgsl */`
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
  }
  @vertex fn vs(@location(0) xz: vec2f) -> VOut {
    let t = u.eye.w;
    let y = 0.20 * sin(xz.x * 0.50 + t * 1.3)
          + 0.16 * sin(xz.y * 0.63 + t * 1.7)
          + 0.09 * sin((xz.x + xz.y) * 0.35 + t * 2.3);
    let dx = 0.20 * 0.50 * cos(xz.x * 0.50 + t * 1.3) + 0.09 * 0.35 * cos((xz.x + xz.y) * 0.35 + t * 2.3);
    let dz = 0.16 * 0.63 * cos(xz.y * 0.63 + t * 1.7) + 0.09 * 0.35 * cos((xz.x + xz.y) * 0.35 + t * 2.3);
    var o: VOut;
    o.world = vec3f(xz.x, y, xz.y);
    o.clip = u.vp * vec4f(o.world, 1.0);
    o.normal = normalize(vec3f(-dx, 1.0, -dz));
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let n = normalize(v.normal);
    let view = v.world - u.eye.xyz;
    let vdir = normalize(view);
    let L = -normalize(u.sun.xyz);
    let fresnel = pow(1.0 - max(dot(-vdir, n), 0.0), 3.5);
    let reflDir = reflect(vdir, n);
    let refl = skyColor(vec3f(reflDir.x, abs(reflDir.y), reflDir.z), u.eye.w);
    let deep = vec3f(0.035, 0.13, 0.16);
    let sh = shadow(v.world, max(dot(n, L), 0.0));
    var col = mix(deep, refl, 0.25 + 0.65 * fresnel);
    let spec = pow(max(dot(reflDir, L), 0.0), 90.0) * sh;
    col += SUN_COL * spec * 1.4;
    col = mix(col, fogColor(view), fogAmount(length(view)));
    return vec4f(col, 0.88);
  }
`;

// Fireflies live in a storage buffer the compute pass rewrites each frame,
// and the render pass turns each one into a camera-facing quad.
const FIREFLY_SIM = /* wgsl */`
  struct P { anchor: vec4f, pos: vec4f }
  @group(0) @binding(0) var<storage, read_write> ps: array<P>;
  struct T { t: f32, dt: f32, pad0: f32, pad1: f32 }
  @group(0) @binding(1) var<uniform> tt: T;
  @compute @workgroup_size(64) fn cs(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if (i >= arrayLength(&ps)) { return; }
    let a = ps[i].anchor;
    let s = a.w;
    let t = tt.t;
    let p = a.xyz + vec3f(
      sin(t * 0.37 + s * 11.0) * 2.8,
      sin(t * 0.53 + s * 23.0) * 1.1 + 1.6,
      cos(t * 0.41 + s * 17.0) * 2.8,
    );
    let glow = 0.55 + 0.45 * sin(t * 2.4 + s * 37.0);
    ps[i].pos = vec4f(p, glow);
  }
`;

const FIREFLY_DRAW = COMMON + /* wgsl */`
  struct P { anchor: vec4f, pos: vec4f }
  @group(1) @binding(0) var<storage, read> ps: array<P>;
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) glow: f32,
    @location(2) dist: f32,
  }
  @vertex fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    let p = ps[ii].pos;
    let corner = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) - 0.5;   // two-triangle quad
    let yaw = u.cam.x;
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    let size = 0.16;
    let world = p.xyz + right * corner.x * size + vec3f(0.0, corner.y * size, 0.0);
    var o: VOut;
    o.clip = u.vp * vec4f(world, 1.0);
    o.uv = corner * 2.0;
    o.glow = p.w;
    o.dist = length(world - u.eye.xyz);
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let d = length(v.uv);
    let a = max(1.0 - d, 0.0);
    let fade = exp(-v.dist * 0.02);
    return vec4f(vec3f(1.0, 0.82, 0.42) * a * a * v.glow * fade, 1.0);
  }
`;

// The crystals' halos reuse the billboard idea, fed straight from the
// instance buffer Rill wrote.
const GLOW = COMMON + /* wgsl */`
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec3f,
    @location(2) dist: f32,
  }
  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    let corner = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) - 0.5;
    let yaw = u.cam.x;
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    let size = scale * 4.0;
    let center = ipos + vec3f(0.0, scale * 1.1, 0.0);
    let world = center + right * corner.x * size + vec3f(0.0, corner.y * size, 0.0);
    var o: VOut;
    o.clip = u.vp * vec4f(world, 1.0);
    o.uv = corner * 2.0; o.color = icol;
    o.dist = length(world - u.eye.xyz);
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let d = length(v.uv);
    let a = max(1.0 - d, 0.0);
    let pulse = 0.8 + 0.2 * sin(u.eye.w * 2.1 + v.dist);
    return vec4f(v.color * a * a * a * 0.5 * pulse, 1.0);
  }
`;

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

function pineMesh() {
  return flatMesh([
    ...coneRing(0.0, 1.2, 0.16, 0.13, 5),      // trunk
    ...coneRing(0.9, 2.6, 1.15, 0.0, 6),       // lower boughs
    ...coneRing(2.0, 3.6, 0.85, 0.0, 6),       // upper boughs
  ]);
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

function waterMesh(half, seg) {
  const verts = [];
  const step = (half * 2) / seg;
  for (let i = 0; i < seg; i++) for (let j = 0; j < seg; j++) {
    const x0 = -half + i * step, z0 = -half + j * step, x1 = x0 + step, z1 = z0 + step;
    verts.push(x0, z0, x0, z1, x1, z1, x0, z0, x1, z1, x1, z0);
  }
  return new Float32Array(verts);
}

// ---- the program -----------------------------------------------------------

export async function run(canvas, wasmURL, hud) {
  if (!navigator.gpu) throw new Error("this browser has no WebGPU");
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) throw new Error("no GPU adapter");
  const device = await adapter.requestDevice();
  device.addEventListener("uncapturederror", e => console.error("WEBGPU:", e.error.message));
  const context = canvas.getContext("webgpu");
  const format = navigator.gpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "opaque" });

  // The world, from Rill.
  let inst;
  const bytes = await (await fetch(wasmURL, { cache: "no-store" })).arrayBuffer();
  const wasm = await WebAssembly.compile(bytes);
  inst = await WebAssembly.instantiate(wasm, {
    env: {},
    wasi_snapshot_preview1: wasiShim(() => inst.exports.memory),
  });
  inst.exports._initialize();
  inst.exports.rill_init();
  const E = inst.exports;
  const state = E.init();
  const stateF32 = () => new Float32Array(E.memory.buffer, Number(state), 64);

  const terrainVerts = Number(E.terrain_verts(state));
  const nPines = Number(E.pine_count(state));
  const nCrystals = Number(E.crystal_count(state));

  const gpuBuf = (data, usage) => {
    const b = device.createBuffer({ size: (data.byteLength + 3) & ~3, usage: usage | GPUBufferUsage.COPY_DST });
    device.queue.writeBuffer(b, 0, data);
    return b;
  };
  const V = GPUBufferUsage.VERTEX;
  const terrainBuf = gpuBuf(new Float32Array(E.memory.buffer, Number(E.terrain(state)), terrainVerts * 9), V);
  const pineInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.pines(state)), nPines * 8), V);
  const crystalInst = gpuBuf(new Float32Array(E.memory.buffer, Number(E.crystals(state)), nCrystals * 8), V);
  const pineBuf = gpuBuf(pineMesh(), V);
  const crystalBuf = gpuBuf(crystalMesh(), V);
  const waterBuf = gpuBuf(waterMesh(86, 56), V);
  const pineVerts = pineMesh().length / 6, crystalVerts = crystalMesh().length / 6;
  const waterVerts = waterMesh(86, 56).length / 2;

  // Fireflies: anchors scattered over the lower valley, positions computed
  // on the GPU every frame.
  const N_FLY = 1200;
  const flyData = new Float32Array(N_FLY * 8);
  for (let i = 0; i < N_FLY; i++) {
    const a = Math.random() * Math.PI * 2, r = 12 + Math.random() * 32;
    flyData[i * 8] = Math.cos(a) * r;
    flyData[i * 8 + 1] = 1.0 + Math.random() * 3.5;
    flyData[i * 8 + 2] = Math.sin(a) * r;
    flyData[i * 8 + 3] = Math.random();
  }
  const flyBuf = device.createBuffer({ size: flyData.byteLength, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
  device.queue.writeBuffer(flyBuf, 0, flyData);
  const flyTime = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });

  // Uniforms and the shadow map.
  const uni = device.createBuffer({ size: 48 * 4 + 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
  const shadowTex = device.createTexture({
    size: [2048, 2048], format: "depth32float",
    usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING,
  });
  const shadowView = shadowTex.createView();
  const shadowSamp = device.createSampler({ compare: "less" });

  const bglMain = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: {} },
    { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: "depth" } },
    { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "comparison" } },
  ]});
  const bgMain = device.createBindGroup({ layout: bglMain, entries: [
    { binding: 0, resource: { buffer: uni } },
    { binding: 1, resource: shadowView },
    { binding: 2, resource: shadowSamp },
  ]});

  const bglShadow = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: {} },
  ]});
  const bgShadow = device.createBindGroup({ layout: bglShadow, entries: [
    { binding: 0, resource: { buffer: uni } },
  ]});

  // Layouts.
  const terrainLayout = [{ arrayStride: 36, attributes: [
    { shaderLocation: 0, offset: 0, format: "float32x3" },
    { shaderLocation: 1, offset: 12, format: "float32x3" },
    { shaderLocation: 2, offset: 24, format: "float32x3" },
  ]}];
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

  const pipe = (code, buffers, opts = {}) => device.createRenderPipeline({
    label: opts.label ?? "unnamed",
    layout,
    vertex: { module: device.createShaderModule({ code }), buffers },
    fragment: { module: device.createShaderModule({ code }), targets: [{ format, blend: opts.blend }] },
    primitive: { cullMode: opts.cull ?? "back" },
    depthStencil: { ...depth, ...opts.depth },
    multisample: MS,
    ...(opts.constants ? { vertex: { module: device.createShaderModule({ code }), buffers, constants: opts.constants },
                           fragment: { module: device.createShaderModule({ code }), targets: [{ format, blend: opts.blend }], constants: opts.constants } } : {}),
  });

  const skyPipe = pipe(SKY, [], { label: "sky", depth: { depthWriteEnabled: false, depthCompare: "less-equal" }, cull: "none" });
  const terrainPipe = pipe(TERRAIN, terrainLayout, { label: "terrain" });
  const pinePipe = pipe(INSTANCED, [meshLayout, instLayout], { label: "pine", constants: { emissive: 0.0 } });
  const crystalPipe = pipe(INSTANCED, [meshLayout, instLayout], { label: "crystal", constants: { emissive: 0.72 } });
  const waterPipe = pipe(WATER, [{ arrayStride: 8, attributes: [{ shaderLocation: 0, offset: 0, format: "float32x2" }] }],
    { label: "water", blend: { color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha" }, alpha: {} }, cull: "none",
      depth: { depthWriteEnabled: false } });
  const glowPipe = pipe(GLOW, [instLayout], { label: "glow", blend: additive, cull: "none", depth: { depthWriteEnabled: false } });

  const flyBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: { type: "read-only-storage" } },
  ]});
  const flyDrawPipe = device.createRenderPipeline({
    label: "fireflies",
    layout: device.createPipelineLayout({ bindGroupLayouts: [bglMain, flyBgl] }),
    vertex: { module: device.createShaderModule({ code: FIREFLY_DRAW }) },
    fragment: { module: device.createShaderModule({ code: FIREFLY_DRAW }), targets: [{ format, blend: additive }] },
    primitive: { cullMode: "none" },
    depthStencil: { ...depth, depthWriteEnabled: false },
    multisample: MS,
  });
  const flyDrawBg = device.createBindGroup({ layout: flyBgl, entries: [{ binding: 0, resource: { buffer: flyBuf } }] });

  const simBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: "storage" } },
    { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: {} },
  ]});
  const simPipe = device.createComputePipeline({
    label: "fly-sim",
    layout: device.createPipelineLayout({ bindGroupLayouts: [simBgl] }),
    compute: { module: device.createShaderModule({ code: FIREFLY_SIM }) },
  });
  const simBg = device.createBindGroup({ layout: simBgl, entries: [
    { binding: 0, resource: { buffer: flyBuf } },
    { binding: 1, resource: { buffer: flyTime } },
  ]});

  // Shadow pipelines: depth only, front faces are enough.
  const shadowLayout = device.createPipelineLayout({ bindGroupLayouts: [bglShadow] });
  const shadowPipe = (code, buffers) => device.createRenderPipeline({
    label: "shadow",
    layout: shadowLayout,
    vertex: { module: device.createShaderModule({ code }), buffers },
    primitive: { cullMode: "none" },
    depthStencil: { format: "depth32float", depthWriteEnabled: true, depthCompare: "less" },
  });
  const shTerrain = shadowPipe(SHADOW_TERRAIN, terrainLayout);
  const shInst = shadowPipe(SHADOW_INST, [meshLayout, instLayout]);

  // MSAA targets, remade when the canvas resizes.
  let msaaTex, depthTex;
  function makeTargets() {
    msaaTex?.destroy(); depthTex?.destroy();
    msaaTex = device.createTexture({ size: [canvas.width, canvas.height], sampleCount: 4, format, usage: GPUTextureUsage.RENDER_ATTACHMENT });
    depthTex = device.createTexture({ size: [canvas.width, canvas.height], sampleCount: 4, format: "depth24plus", usage: GPUTextureUsage.RENDER_ATTACHMENT });
  }
  function resize() {
    const dpr = Math.min(devicePixelRatio, 2);
    const w = Math.floor(canvas.clientWidth * dpr), h = Math.floor(canvas.clientHeight * dpr);
    if (w !== canvas.width || h !== canvas.height) { canvas.width = w; canvas.height = h; makeTargets(); }
  }
  makeTargets(); resize();
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

  // The frame loop: Rill steps the world, the GPU draws what it says.
  const uf = new Float32Array(52);
  let last = performance.now(), fps = 0, frames = 0, fpsT = last;
  function tick(now) {
    resize();
    const dt = Math.min((now - last) / 1000, 0.05);
    last = now;
    const t = now / 1000;

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
    device.queue.writeBuffer(uni, 0, uf);
    device.queue.writeBuffer(flyTime, 0, new Float32Array([t, dt, 0, 0]));

    const enc = device.createCommandEncoder();

    const sim = enc.beginComputePass();
    sim.setPipeline(simPipe); sim.setBindGroup(0, simBg);
    sim.dispatchWorkgroups(Math.ceil(N_FLY / 64));
    sim.end();

    const sh = enc.beginRenderPass({
      colorAttachments: [],
      depthStencilAttachment: { view: shadowView, depthClearValue: 1, depthLoadOp: "clear", depthStoreOp: "store" },
    });
    sh.setBindGroup(0, bgShadow);
    sh.setPipeline(shTerrain); sh.setVertexBuffer(0, terrainBuf); sh.draw(terrainVerts);
    sh.setPipeline(shInst);
    sh.setVertexBuffer(0, pineBuf); sh.setVertexBuffer(1, pineInst); sh.draw(pineVerts, nPines);
    sh.setVertexBuffer(0, crystalBuf); sh.setVertexBuffer(1, crystalInst); sh.draw(crystalVerts, nCrystals);
    sh.end();

    const pass = enc.beginRenderPass({
      colorAttachments: [{
        view: msaaTex.createView(), resolveTarget: context.getCurrentTexture().createView(),
        loadOp: "clear", storeOp: "store", clearValue: { r: 0, g: 0, b: 0, a: 1 },
      }],
      depthStencilAttachment: {
        view: depthTex.createView(), depthClearValue: 1, depthLoadOp: "clear", depthStoreOp: "discard",
      },
    });
    pass.setBindGroup(0, bgMain);
    pass.setPipeline(terrainPipe); pass.setVertexBuffer(0, terrainBuf); pass.draw(terrainVerts);
    pass.setPipeline(pinePipe);
    pass.setVertexBuffer(0, pineBuf); pass.setVertexBuffer(1, pineInst); pass.draw(pineVerts, nPines);
    pass.setPipeline(crystalPipe);
    pass.setVertexBuffer(0, crystalBuf); pass.setVertexBuffer(1, crystalInst); pass.draw(crystalVerts, nCrystals);
    pass.setPipeline(skyPipe); pass.draw(3);
    pass.setPipeline(waterPipe); pass.setVertexBuffer(0, waterBuf); pass.draw(waterVerts);
    pass.setPipeline(glowPipe); pass.setVertexBuffer(0, crystalInst); pass.draw(6, nCrystals);
    pass.setPipeline(flyDrawPipe); pass.setBindGroup(1, flyDrawBg); pass.draw(6, N_FLY);
    pass.end();

    device.queue.submit([enc.finish()]);

    frames++;
    if (now - fpsT > 500) { fps = Math.round(frames * 1000 / (now - fpsT)); frames = 0; fpsT = now; hud(fps); }
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);

  return { tris: terrainVerts / 3 + (pineVerts * nPines + crystalVerts * nCrystals) / 3 };
}
