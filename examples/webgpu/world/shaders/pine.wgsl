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
    var p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;
    let hgt = max(pos.y, 0.0) / 3.6;
    let dirn = vec2f(cos(u.cam.z), sin(u.cam.z));
    let flutter = sin(u.eye.w * (1.6 + 2.4 * u.cam.w) + ipos.x * 1.31 + ipos.z * 0.97);
    let sway = (u.cam.w * hgt * hgt * 0.45 + flutter * 0.09 * u.cam.w * hgt) * scale;
    p = vec3f(p.x + dirn.x * sway, p.y, p.z + dirn.y * sway);
    let nr = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.normal = nr; o.color = icol;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    return vec4f(lit(v.color, normalize(v.normal), v.world), 1.0);
  }
