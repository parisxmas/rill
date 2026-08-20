  // The thing in the lake.
  //
  // The model has no skeleton and does not need one. A fish does not swim by
  // rotating bones, it swims by passing a wave down its own length — so that is
  // what this does: one travelling sine, its amplitude rising toward the tail,
  // applied to the vertex before anything else. Everything a swim cycle is
  // supposed to convey comes out of that single line, and it costs no rig, no
  // keyframes and no animation file.
  //
  // The wave goes the right way round on purpose. It travels from the nose
  // toward the tail, which is the direction a real one propagates and the
  // reason the tail appears to be pushing water rather than being wagged: the
  // tip is always a beat behind the body it is trailing.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) tint: vec3f,
    @location(4) textured: f32,
  }

  const LEN   = 4.6;              // nose to tail, metres — matches the bake
  const SWAY  = 0.30;             // how far the tail throws, in metres
  const WAVE  = 2.3;              // wavelengths down the body

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f,
    @location(2) quv: vec2f, @location(3) tint: vec4f,
  ) -> VOut {
    // Unpack: position is unorm16 across the model's own box, normal snorm8.
    var m = u.sharkMin.xyz + qpos.xyz * u.sharkSize.xyz;

    // How far back along the body, 0 at the nose and 1 at the tail. The model
    // faces -Z, so the tail is at +Z.
    let back = clamp((m.z + LEN * 0.5) / LEN, 0.0, 1.0);

    // The jaw. There is no separate jaw in the mesh, so the gape is made by
    // pushing the front of the head apart about the mouth line — everything
    // near the nose moves away from it, more the nearer it is. Crude, and at
    // the moment a shark is close enough for you to see its mouth you are not
    // examining the topology.
    let nose = clamp(1.0 - back * 4.2, 0.0, 1.0);
    let gape = u.shark.w;
    m.y += sign(m.y + 0.06) * nose * nose * gape * 0.42;

    // And the swim. Amplitude cubed in `back` so the front third barely moves
    // — a shark's head is nearly rigid and the whole animal is the last third.
    let phase = u.shark.z;
    let amp = SWAY * back * back * back;
    let bend = sin(phase - back * WAVE * 6.2831853) * amp;
    m.x += bend;

    // The normal has to lean with it or the lighting stays flat while the body
    // moves, which reads as a texture sliding over a rigid shape. The slope of
    // the bend along the body is the angle the surface has turned through.
    let slope = cos(phase - back * WAVE * 6.2831853) * amp * (WAVE * 6.2831853 / LEN);
    var n = qn.xyz;
    n = normalize(vec3f(n.x - n.z * slope, n.y, n.z + n.x * slope));

    // Into the world: yaw about Y, then the shark's own position.
    //
    // Half a circle is added on the way in, and it is not a fudge. The world
    // moves the animal toward `(sin hd, cos hd)`, and this rotation carries the
    // model's +Z onto exactly that — but +Z is the tail. The head is at -Z:
    // 1,600 of the model's 2,396 vertices are in the front eighth of it, which
    // is the jaw, the teeth and the eyes, against 120 in the back eighth, which
    // is a caudal fin a seventh as wide and twice as tall. Without the turn the
    // shark swam at people tail first — so the beat that runs down its length
    // was leading, which reads as an animal that is not swimming at all, and
    // the only part of it that ever reached anybody was the tail.
    let yaw = u.shark.x + 3.14159265;
    let c = cos(yaw); let sn = sin(yaw);
    let p = vec3f(m.x * c + m.z * sn, m.y, -m.x * sn + m.z * c) + u.sharkAt.xyz;
    let nw = vec3f(n.x * c + n.z * sn, n.y, -n.x * sn + n.z * c);

    var o: VOut;
    o.world = p;
    o.clip = u.vp * vec4f(p, 1.0);
    o.normal = nw;
    // The texture coordinate came in as snorm16 holding uv times 4096, so the
    // attribute arrives as uv/8 and eight puts it back. Sixteen bits over the
    // range this model needs is more precision than a 512-pixel skin can use.
    o.uv = quv * 8.0;
    o.tint = tint.rgb;
    // Negative u is the mesh's way of saying "no texture here" — the teeth,
    // the gums and the eyes carry a flat colour instead. Same trick the pine
    // trunk uses against the needle atlas.
    o.textured = select(0.0, 1.0, quv.x >= 0.0);
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let skin = textureSample(sharkTex, barkSamp, v.uv).rgb;
    let albedo = mix(v.tint, skin * v.tint, v.textured);
    let n = normalize(v.normal);
    var col = lit(albedo, n, v.world);

    // Under water and lit from above: the surface is the only bright thing, so
    // the top of the animal takes the light and the underside goes green-black.
    // This is most of why a shark seen from below is a silhouette and one seen
    // from above is a shadow, and it costs a dot product.
    let down = clamp(0.5 - n.y * 0.5, 0.0, 1.0);
    col *= mix(1.0, 0.42, down);
    // And the water itself between you and it. Green, and thicker than the air
    // fog by a long way — twelve metres of lake is a long way to see.
    let view = v.world - u.eye.xyz;
    let d = length(view);
    let murk = 1.0 - exp(-d * 0.085);
    col = mix(col, vec3f(0.05, 0.14, 0.15), murk * 0.85);
    return vec4f(col, 1.0);
  }
