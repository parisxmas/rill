  struct SOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
  }
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f, @location(6) uv: vec2f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> SOut {
    // The felled pose, exactly as `pine.wgsl` computes it: a shadow that
    // stayed standing over a tree lying down would give the trick away.
    let felled = scale < 0.0;
    let sc = abs(scale);
    let h1 = fract(sin(dot(ipos.xz, vec2f(127.1, 311.7))) * 43758.5453);
    let yaw = select(rot, h1 * 6.28318, felled);
    let c = cos(yaw); let s = sin(yaw);
    var m = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * sc;
    let k0 = clamp((u.eye.w - rot) / 1.3, 0.0, 1.0);
    let k = select(0.0, k0 * k0 * (3.0 - 2.0 * k0), felled);
    let fd = h1 * 251.0;
    let ax = vec3f(-sin(fd), 0.0, cos(fd));
    let ca = cos(k * 1.5); let sa = sin(k * 1.5);
    m = m * ca + cross(ax, m) * sa + ax * dot(ax, m) * (1.0 - ca);
    let p = m + ipos;
    var o: SOut;
    o.clip = u.lvp * vec4f(p, 1.0);
    o.uv = uv;
    return o;
  }
  // Depth only, but it has to be able to say "nothing here": a branch card is
  // mostly gaps, and a shadow map that does not know puts a solid slab on the
  // ground under every tree.
  @fragment fn fs(v: SOut) {
    // A fixed mip rather than whatever the derivatives ask for. Two reasons and
    // they are both the point of this pass: it needs no derivatives, and the
    // needle detail that a full-resolution fetch brings back is exactly the
    // high-frequency edge that flips on and off as the sun turns. A blurred
    // level is what lets the cut move smoothly instead of crawling.
    //
    // The level and the cut together are how much light a branch keeps off the
    // ground, and they were set to draw the needles honestly: level two at a
    // half-way cut shadows 13.3% of the card, against the twig's own 12.2%
    // coverage. Faithful, and far too bright — a pine is 85% holes by area, so
    // a canopy of them let seven eighths of the sun straight through and
    // standing under a tree was no darker than standing beside it.
    //
    // A branch is not a lattice you can see the sky through, it is a mass. So:
    // blurrier, and a cut low enough that anywhere with foliage near it counts.
    // Level four at 0.06 shadows 27.5% of the card — a little over double —
    // and the compounding is where it is felt: four overlapping branches used
    // to pass 57% of the light and now pass 28%. Sixteen texels square still
    // carries the shape of a branch, which eight does not.
    let a = textureSampleLevel(foliage, foliageSamp, v.uv, 4.0).a;
    if (v.uv.x >= 0.0 && a < 0.06) { discard; }
  }
