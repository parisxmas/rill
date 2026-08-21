  // The walls on the tops: blocks of stone, laid in an L.
  //
  // No texture file. The grain is a function of where the stone *is* in the
  // world, which costs nothing to store, never repeats across a wall the way a
  // tiled image does, and — because it is a height rather than a picture — is
  // the same field the bump reads. A photograph would have needed a normal map
  // beside it and the two would have had to agree; this way there is only one
  // thing and the surface cannot disagree with itself.
  //
  // Every block gets its own turn about the vertical, so no two are laid the
  // same way and the grain does not run continuously from one to the next.

  struct SOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) tint: vec3f,
    @location(3) local: vec3f,
  }

  fn sHash(p: vec3f) -> f32 {
    return fract(sin(dot(p, vec3f(127.1, 311.7, 74.7))) * 43758.5453);
  }

  // Value noise in three dimensions. Trilinear between the eight corners of
  // the cell, smoothstepped so the cell edges do not show as creases.
  fn sNoise(p: vec3f) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let c000 = sHash(i + vec3f(0.0, 0.0, 0.0));
    let c100 = sHash(i + vec3f(1.0, 0.0, 0.0));
    let c010 = sHash(i + vec3f(0.0, 1.0, 0.0));
    let c110 = sHash(i + vec3f(1.0, 1.0, 0.0));
    let c001 = sHash(i + vec3f(0.0, 0.0, 1.0));
    let c101 = sHash(i + vec3f(1.0, 0.0, 1.0));
    let c011 = sHash(i + vec3f(0.0, 1.0, 1.0));
    let c111 = sHash(i + vec3f(1.0, 1.0, 1.0));
    let x00 = mix(c000, c100, u.x);
    let x10 = mix(c010, c110, u.x);
    let x01 = mix(c001, c101, u.x);
    let x11 = mix(c011, c111, u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
  }

  // The height the bump reads and the grain the colour reads: one field.
  //
  // Three octaves for the body of the rock, and then a much finer one held
  // back to a tenth, which is the grit. Without it the stone reads as a
  // beanbag; with it at full strength it reads as gravel glued to a box.
  fn rock(p: vec3f) -> f32 {
    var h = sNoise(p * 1.7) * 0.55;
    h = h + sNoise(p * 4.3) * 0.28;
    h = h + sNoise(p * 11.0) * 0.13;
    h = h + sNoise(p * 34.0) * 0.10;
    return h;
  }

  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f,
    @location(2) ipos: vec3f, @location(3) size: f32,
    @location(4) rot: f32, @location(5) tint: vec3f,
  ) -> SOut {
    let c = cos(rot); let s = sin(rot);
    let turned = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c);
    let tn = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);

    var o: SOut;
    let world = turned * size + ipos;
    o.world = world;
    o.normal = tn;
    o.tint = tint;
    // Where on this block we are, so the grain belongs to the stone rather
    // than to the place it happens to be standing.
    o.local = turned * size + vec3f(ipos.x * 0.37, ipos.y * 0.19, ipos.z * 0.29);
    o.clip = u.vp * vec4f(world, 1.0);
    return o;
  }

  @fragment fn fs(v: SOut) -> @location(0) vec4f {
    let h = rock(v.local);
    // Darker where the rock is pitted, and a little warm where it stands
    // proud — the two together are what reads as a face that has been out in
    // the weather rather than a grey box with noise on it.
    let shade = 0.62 + 0.62 * h;
    let albedo = v.tint * shade + vec3f(0.05, 0.035, 0.02) * max(h - 0.55, 0.0) * 2.2;
    let n = bumped(normalize(v.normal), v.world, h, 0.55);
    return vec4f(lit(albedo, n, v.world), 1.0);
  }
