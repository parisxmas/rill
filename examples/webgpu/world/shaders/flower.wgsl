  // The meadow's flowers.
  //
  // The undergrowth's own shader with two things changed: it draws from the
  // right half of the plant atlas, and it leans further and slower in the
  // wind. A tuft of grass is stiff and a flower on a stem is not — the head
  // is the heaviest thing on the plant and it is at the top of it, so what
  // the wind does to a flower is nod it, and the nod is most of what makes a
  // meadow look alive rather than printed on the ground.
  //
  // Nothing collides with these. They are drawn and nothing else: no ground
  // to stand on beyond the height they were placed at, no sphere, no entry in
  // any of the world's rings. Walking through a flower is walking through a
  // flower.

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
    // Which of the four it is — white, yellow, pink or blue. Off the position
    // rather than off anything the engine sends, for the same reason the
    // grass does it: one hash is cheaper than a float on the wire, and a
    // clump wants its neighbours to differ anyway.
    let r = fract(sin(dot(ipos.xz, vec2f(73.3, 157.9))) * 43758.5453);
    let alt  = select(0.0, 1.0, fract(r * 11.0) < 0.5);
    let tall = select(0.0, 1.0, fract(r * 29.0) < 0.5);
    // The right half of the atlas: two columns of two.
    let cell = uv * vec2f(0.2450, 0.4900) + vec2f(0.5025, 0.0050)
             + vec2f(alt * 0.25, tall * 0.5);

    let c = cos(rot); let s = sin(rot);
    var p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;

    // The nod. Slower than the grass's and further, and squared in the height
    // so the root stays put and the head does the moving — which is what a
    // stem does. The phase comes off the plant's own position, so a clump
    // moves together and the clump beyond it does not.
    let dirn = vec2f(cos(u.cam.z), sin(u.cam.z));
    let osc = sin(u.wind.x * 1.1 + ipos.x * 0.7 + ipos.z * 0.55);
    let lean = (u.cam.w * 0.30 + osc * (0.07 + 0.24 * u.cam.w)) * pos.y * pos.y * scale;
    p.x += dirn.x * lean; p.z += dirn.y * lean;

    let nr = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.color = icol; o.uv = cell; o.normal = nr;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let t = textureSample(plants, foliageSamp, v.uv);
    if (t.a < 0.5) { discard; }
    // Barely tinted. The four cells already differ, and a petal is the one
    // thing in this valley whose colour is the point of it — a per-clump
    // tint on top turns a white daisy grey.
    let albedo = t.rgb * (0.86 + v.color * 0.34);
    return vec4f(lit(albedo, normalize(v.normal), v.world), 1.0);
  }
