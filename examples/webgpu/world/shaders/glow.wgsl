  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec3f,
    @location(2) dist: f32,
    @location(3) phase: f32,
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
    o.uv = corner * 2.0; o.color = icol; o.phase = rot;
    o.dist = length(world - u.eye.xyz);
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let d = length(v.uv);
    let a = max(1.0 - d, 0.0);
    // On the climb, not on how far off you are standing.
    let pulse = 0.62 + 0.75 * crystalFlare(v.phase);
    return vec4f(v.color * a * a * a * 0.5 * pulse, 1.0);
  }
