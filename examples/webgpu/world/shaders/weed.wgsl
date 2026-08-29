  // What grows on the bottom of the lake.
  //
  // The meadow's own card and the meadow's own atlas — one cell of it, the
  // fern — on the crystals' instance layout. What makes it weed rather than
  // grass is entirely in how it moves: grass is pushed one way by a wind that
  // gusts, and weed is *rocked*, by a swell that comes back. So the sway here
  // is a circle rather than a lean, slow, and much deeper — a frond in still
  // water still travels most of its own height.
  //
  // It reads `lit` like everything else, which is the whole reason it is worth
  // doing this way: the caustics and the water's fog arrive on it for free and
  // by the same arithmetic that puts them on the sand it stands in.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) color: vec3f,
    @location(2) uv: vec2f,
    @location(3) normal: vec3f,
  }

  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f, @location(6) uv: vec2f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    // The fern cell — bottom-left of the atlas's plant half — inset a hair so
    // a bilinear tap on the boundary cannot reach the cell next door.
    let cell = uv * vec2f(0.2450, 0.4900) + vec2f(0.0025, 0.5050);

    let c = cos(rot); let s = sin(rot);
    var p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;

    // Rocked, not blown. Two rates that do not divide into one another, so the
    // bed never comes back to the same shape; the phase runs off where the
    // plant stands, so a stretch of them leans as a body rather than each on
    // its own clock.
    let t = u.eye.w;
    let ph = ipos.x * 0.42 + ipos.z * 0.31;
    let swing = vec2f(sin(t * 0.85 + ph), cos(t * 0.61 + ph * 1.3));
    // Along the plant, not across it: the root stays where it was planted and
    // the tip travels, and squaring it keeps the bend in the upper half where
    // a stem actually bends.
    let up = pos.y * pos.y * scale;
    p.x += swing.x * 0.42 * up;
    p.z += swing.y * 0.42 * up;

    let nr = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.color = icol; o.uv = cell; o.normal = nr;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let t = textureSample(plants, foliageSamp, v.uv);
    if (t.a < 0.5) { discard; }
    // Greener and darker than the same leaf on land: what reaches the bottom
    // of a lake has been through several metres of water first, and water
    // takes the red out of light before it takes anything else.
    let albedo = t.rgb * vec3f(0.34, 0.72, 0.58) * (0.55 + v.color * 1.2);
    return vec4f(lit(albedo, normalize(v.normal), v.world), 1.0);
  }
