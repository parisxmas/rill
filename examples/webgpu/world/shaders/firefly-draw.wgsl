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
