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
