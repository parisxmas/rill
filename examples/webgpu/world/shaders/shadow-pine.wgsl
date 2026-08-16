  struct SOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
  }
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f, @location(6) uv: vec2f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> SOut {
    let c = cos(rot); let s = sin(rot);
    let p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;
    var o: SOut;
    o.clip = u.lvp * vec4f(p, 1.0);
    o.uv = uv;
    return o;
  }
  // Depth only, but it has to be able to say "nothing here": a branch card is
  // mostly gaps, and a shadow map that does not know puts a solid slab on the
  // ground under every tree.
  @fragment fn fs(v: SOut) {
    let a = textureSample(foliage, foliageSamp, v.uv).a;
    if (v.uv.x >= 0.0 && a < 0.5) { discard; }
  }
