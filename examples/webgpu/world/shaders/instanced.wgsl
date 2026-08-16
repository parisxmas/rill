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
    let glowFogged = mix(glow, fogColor(view), fogAt(v.world, length(view)));
    return vec4f(mix(base, glowFogged, emissive), 1.0);
  }
