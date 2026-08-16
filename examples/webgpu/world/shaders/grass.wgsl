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
    let dirn = vec2f(cos(u.cam.z), sin(u.cam.z));
    let osc = sin(t * (1.5 + 3.0 * u.cam.w) + ipos.x * 0.9 + ipos.z * 0.7);
    let sway = (u.cam.w * 0.20 + osc * (0.05 + 0.16 * u.cam.w)) * pos.y * scale;
    p.x += dirn.x * sway; p.z += dirn.y * sway;
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.color = icol;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    return vec4f(lit(v.color, vec3f(0.0, 1.0, 0.0), v.world), 1.0);
  }
