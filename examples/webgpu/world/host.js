// The renderer half of the valley. Rill (world.wasm) computes the world —
// mesh, placement, camera, physics, matrices — and this file spends the GPU
// on it: a shadow pass under a fixed golden-hour sun, a 4x MSAA forward pass
// with procedural sky, flat-shaded terrain, instanced pines, crystals and
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
  { name: "low",    grid:  72, pines: 300, crystals: 120, grass:  800, flies:  500, shadow: 1024, water:  48, dpr: 1.0, waves: 4, ripples: 2 },
  { name: "medium", grid:  96, pines: 420, crystals: 200, grass: 1500, flies: 1200, shadow: 2048, water:  72, dpr: 1.5, waves: 5, ripples: 3 },
  { name: "high",   grid: 128, pines: 650, crystals: 300, grass: 3000, flies: 2500, shadow: 2048, water:  96, dpr: 2.0, waves: 6, ripples: 3 },
  { name: "ultra",  grid: 160, pines: 900, crystals: 380, grass: 5000, flies: 4000, shadow: 4096, water: 128, dpr: 3.0, waves: 6, ripples: 4 },
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

// Pines and crystals share one instanced shader; the `emissive` constant is
// what tells them apart. Grass gets its own vertex stage for the wind.
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

// Grass: normals point up so a blade is lit like the ground it stands on,
// and the top of each blade leans with a slow wind.
const GRASS = COMMON + /* wgsl */`
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) color: vec3f,
  }
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    let c = cos(rot); let s = sin(rot);
    var p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;
    let t = u.eye.w;
    let sway = sin(t * 1.7 + ipos.x * 0.9 + ipos.z * 0.7) * 0.16 * pos.y * scale;
    p.x += sway; p.z += sway * 0.6;
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.color = icol;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    return vec4f(lit(v.color, vec3f(0.0, 1.0, 0.0), v.world), 1.0);
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

// The water. Gerstner waves — trochoids, not sine sheets: each wave moves
// vertices horizontally toward its own crests, which is what sharpens them,
// and its phase speed comes from the gravity relation c = sqrt(g/k). Every
// vertex carries the exact terrain depth beneath it, computed by Rill, so
// waves die out in the shallows the way real ones do, the water darkens with
// depth, and the foam sits on the actual shoreline. The fragment stage adds
// ripple octaves on top of the geometric normal — detail the mesh never had —
// then Schlick fresnel against the procedural sky, the sun's glitter, and
// crest foam where the trochoids pinch.
const WATER = COMMON + /* wgsl */`
  override WAVES: i32 = 5;
  override RIPPLES: i32 = 3;

  fn wdir(i: i32) -> vec2f {
    switch i {
      case 0:  { return vec2f( 0.96,  0.28); }
      case 1:  { return vec2f( 0.60, -0.80); }
      case 2:  { return vec2f(-0.35,  0.94); }
      case 3:  { return vec2f( 0.05,  1.00); }
      case 4:  { return vec2f(-0.87, -0.49); }
      default: { return vec2f( 0.71,  0.71); }
    }
  }
  fn wlen(i: i32) -> f32 {
    switch i {
      case 0:  { return 31.0; }
      case 1:  { return 17.0; }
      case 2:  { return 10.0; }
      case 3:  { return  5.9; }
      case 4:  { return  3.6; }
      default: { return  2.3; }
    }
  }
  fn wamp(i: i32) -> f32 {
    switch i {
      case 0:  { return 0.34; }
      case 1:  { return 0.21; }
      case 2:  { return 0.13; }
      case 3:  { return 0.07; }
      case 4:  { return 0.045; }
      default: { return 0.028; }
    }
  }

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) depth: f32,
    @location(3) crest: f32,
  }

  @vertex fn vs(@location(0) xzd: vec3f) -> VOut {
    let p0 = xzd.xy;
    let depth = xzd.z;
    let t = u.eye.w;
    // Waves feel the bottom: amplitude fades as the water shallows.
    let ampScale = clamp(depth * 0.55, 0.12, 1.0);
    var p = vec3f(p0.x, 0.0, p0.y);
    var n = vec3f(0.0, 1.0, 0.0);
    var crest = 0.0;
    var norm = 0.0;
    for (var i = 0; i < WAVES; i++) {
      let d = wdir(i);
      let a = wamp(i) * ampScale;
      let k = 6.28318 / wlen(i);
      let c = sqrt(9.8 / k);
      let f = k * (dot(d, p0) - c * t);
      let q = min(0.65 / (k * a * f32(WAVES) + 0.001), 1.0 / (k * a * f32(WAVES) + 0.001));
      p.x += q * a * d.x * cos(f);
      p.z += q * a * d.y * cos(f);
      p.y += a * sin(f);
      let wa = k * a;
      n.x -= d.x * wa * cos(f);
      n.z -= d.y * wa * cos(f);
      n.y -= q * wa * sin(f);
      crest += (sin(f) * 0.5 + 0.5) * a;
      norm += a;
    }
    var o: VOut;
    o.world = p;
    o.clip = u.vp * vec4f(p, 1.0);
    o.normal = normalize(n);
    o.depth = depth;
    o.crest = crest / max(norm, 0.001);
    return o;
  }

  // Ripple octaves: directional wavelets far below mesh resolution, summed
  // into the normal. This is where the sun's glitter comes from.
  fn ripple(p: vec2f, t: f32) -> vec3f {
    var n = vec3f(0.0, 0.0, 0.0);
    var freq = 0.9;
    var amp = 0.22;
    for (var i = 0; i < RIPPLES; i++) {
      let fi = f32(i);
      let d = normalize(vec2f(sin(fi * 2.4 + 1.2), cos(fi * 1.7 - 0.4)));
      let ph = dot(d, p) * freq + t * (1.1 + fi * 0.6);
      n.x -= d.x * amp * cos(ph);
      n.z -= d.y * amp * cos(ph);
      freq *= 2.3;
      amp *= 0.55;
    }
    return n;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let t = u.eye.w;
    let n = normalize(v.normal + ripple(v.world.xz, t));
    let view = v.world - u.eye.xyz;
    let vdir = normalize(view);
    let L = -normalize(u.sun.xyz);

    // Schlick's fresnel against the same sky the sky pass draws.
    let cosv = max(dot(-vdir, n), 0.0);
    let fresnel = 0.02 + 0.98 * pow(1.0 - cosv, 5.0);
    let reflDir = reflect(vdir, n);
    let refl = skyColor(vec3f(reflDir.x, abs(reflDir.y), reflDir.z), t);

    // The body of the water: absorption with depth.
    let body = mix(vec3f(0.11, 0.37, 0.40), vec3f(0.012, 0.10, 0.13), smoothstep(0.0, 5.5, v.depth));

    let sh = shadow(v.world, max(dot(n, L), 0.0));
    var col = mix(body, refl, clamp(fresnel * 1.6 + 0.06, 0.0, 1.0));

    // Sun glitter: a tight Blinn lobe on the rippled normal.
    let hv = normalize(L - vdir);
    col += SUN_COL * pow(max(dot(n, hv), 0.0), 240.0) * 2.4 * sh;

    // Foam: at the shoreline, where the depth runs out, and on the crests,
    // where the trochoids pinch. Speckled so it reads as froth, not paint.
    let speck = fract(sin(dot(floor(v.world.xz * 7.0), vec2f(12.9898, 78.233))) * 43758.5453);
    let shore = (1.0 - smoothstep(0.0, 1.0, v.depth))
              * (0.55 + 0.45 * sin(v.depth * 9.0 - t * 2.1));
    let crests = smoothstep(0.80, 0.99, v.crest) * 0.55 * (0.5 + 0.5 * speck);
    let foam = clamp(shore + crests, 0.0, 1.0) * (0.55 + 0.45 * speck);
    col = mix(col, vec3f(0.90, 0.94, 0.93) * (0.6 + 0.4 * sh), clamp(foam, 0.0, 0.85));

    col = mix(col, fogColor(view), fogAmount(length(view)));
    let alpha = mix(0.78, 0.95, smoothstep(0.0, 4.0, v.depth));
    return vec4f(col, alpha);
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

// Three crossed blades, normals up so a tuft is lit like the ground.
function grassMesh() {
  const out = [];
  for (let b = 0; b < 3; b++) {
    const a = b * Math.PI / 3;
    const dx = Math.cos(a) * 0.09, dz = Math.sin(a) * 0.09;
    out.push(
      -dx, 0, -dz, 0, 1, 0,
       dx, 0,  dz, 0, 1, 0,
      dz * 0.3, 0.9, -dx * 0.3, 0, 1, 0,
    );
  }
  return new Float32Array(out);
}

// ---- the program -----------------------------------------------------------

export async function run(canvas, wasmURL, hud) {
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

  const context = canvas.getContext("webgpu");
  const format = navigator.gpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "opaque" });

  // The world, from Rill; the generated loader does the ceremony.
  const E = await load(wasmURL);

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

  const bglMain = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: {} },
    { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: "depth" } },
    { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: { type: "comparison" } },
  ]});
  const bglShadow = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: {} },
  ]});
  const bgShadow = device.createBindGroup({ layout: bglShadow, entries: [
    { binding: 0, resource: { buffer: uni } },
  ]});

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

  const pipe = (label, code, buffers, opts = {}) => device.createRenderPipeline({
    label, layout,
    vertex: { module: device.createShaderModule({ code }), buffers, constants: opts.constants },
    fragment: { module: device.createShaderModule({ code }), targets: [{ format, blend: opts.blend }], constants: opts.constants },
    primitive: { cullMode: opts.cull ?? "back" },
    depthStencil: { ...depth, ...opts.depth },
    multisample: MS,
  });

  const skyPipe = pipe("sky", SKY, [], { depth: { depthWriteEnabled: false, depthCompare: "less-equal" }, cull: "none" });
  const terrainPipe = pipe("terrain", TERRAIN, terrainLayout);
  const pinePipe = pipe("pine", INSTANCED, [meshLayout, instLayout], { constants: { emissive: 0.0 } });
  const crystalPipe = pipe("crystal", INSTANCED, [meshLayout, instLayout], { constants: { emissive: 0.72 } });
  const grassPipe = pipe("grass", GRASS, [meshLayout, instLayout], { cull: "none" });
  const glowPipe = pipe("glow", GLOW, [instLayout], { blend: additive, cull: "none", depth: { depthWriteEnabled: false } });

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
  const simBgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: "storage" } },
    { binding: 1, visibility: GPUShaderStage.COMPUTE, buffer: {} },
  ]});
  const simPipe = device.createComputePipeline({
    label: "fly-sim",
    layout: device.createPipelineLayout({ bindGroupLayouts: [simBgl] }),
    compute: { module: device.createShaderModule({ code: FIREFLY_SIM }) },
  });

  const shadowLayout = device.createPipelineLayout({ bindGroupLayouts: [bglShadow] });
  const shadowPipe = (label, code, buffers) => device.createRenderPipeline({
    label, layout: shadowLayout,
    vertex: { module: device.createShaderModule({ code }), buffers },
    primitive: { cullMode: "none" },
    depthStencil: { format: "depth32float", depthWriteEnabled: true, depthCompare: "less" },
  });
  const shTerrain = shadowPipe("shadow-terrain", SHADOW_TERRAIN, terrainLayout);
  const shInst = shadowPipe("shadow-inst", SHADOW_INST, [meshLayout, instLayout]);

  const pineData = pineMesh(), crystalData = crystalMesh(), grassData = grassMesh();
  const pineBuf = gpuBuf(pineData), crystalBuf = gpuBuf(crystalData), grassBuf = gpuBuf(grassData);
  const pineVerts = pineData.length / 6, crystalVerts = crystalData.length / 6, grassVerts = grassData.length / 6;

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
    const verts = new Float32Array(seg * seg * 6 * 3);
    const step = (half * 2) / seg;
    const d = (x, z) => Math.max(-E.ground(x, z), 0.0);
    let o = 0;
    for (let i = 0; i < seg; i++) for (let j = 0; j < seg; j++) {
      const x0 = -half + i * step, z0 = -half + j * step, x1 = x0 + step, z1 = z0 + step;
      for (const [x, z] of [[x0, z0], [x0, z1], [x1, z1], [x0, z0], [x1, z1], [x1, z0]]) {
        verts[o++] = x; verts[o++] = z; verts[o++] = d(x, z);
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
    ]});

    // The water pipeline carries this tier's wave and ripple counts.
    W.waterPipe = pipe("water", WATER,
      [{ arrayStride: 12, attributes: [{ shaderLocation: 0, offset: 0, format: "float32x3" }] }],
      { blend: { color: { srcFactor: "src-alpha", dstFactor: "one-minus-src-alpha" }, alpha: {} },
        cull: "none", depth: { depthWriteEnabled: false },
        constants: { WAVES: T.waves, RIPPLES: T.ripples } });

    W.tris = Math.round((counts.terrain + pineVerts * counts.pines + crystalVerts * counts.crystals + grassVerts * counts.grass) / 3);
  }
  buildWorld(tierIdx);

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
    sh.setPipeline(shInst);
    sh.setVertexBuffer(0, pineBuf); sh.setVertexBuffer(1, W.pineInst); sh.draw(pineVerts, W.counts.pines);
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
      hud({ fps, gpuMs: qs ? gpuMs : null, tier: T.name, gpuName, tris: W.tris, locked });
    }
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
}
