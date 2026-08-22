  // Where an AK's round stopped.
  //
  // The rifle draws its rounds on the way and needs nothing here; the AK's
  // are not drawn at all — five streaks a second is a lit rope across the
  // valley — so the chip they leave is the whole of the evidence, and it is
  // worth ten seconds of somebody's attention.
  //
  // A card, not a projected decal. A decal wants the surface's own normal and
  // the bullet ring has no room to carry one; a card turned to face back
  // along the round's flight lands square to whoever fired it, which is the
  // one angle it will actually be looked at from. It is pushed a couple of
  // centimetres back along that line so it sits in front of the wall rather
  // than inside it.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) local: vec2f,
    @location(2) fade: f32,
    @location(3) seed: f32,
  }

  const MARK_LIFE = 10.0;      // must match `mark_life` in valley.rill
  const MARK_R = 0.055;        // a hole a hand's width across, near enough

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) at: vec3f, @location(1) dir: vec3f,
    @location(2) born: f32, @location(3) spare: f32,
  ) -> VOut {
    let cx = select(-1.0, 1.0, vi == 1u || vi == 2u || vi == 4u);
    let cy = select(-1.0, 1.0, vi == 2u || vi == 4u || vi == 5u);

    var o: VOut;
    let age = u.eye.w - born;
    o.fade = clamp(1.0 - age / MARK_LIFE, 0.0, 1.0);
    // The slot has never been used, or its ten seconds are up.
    if (born <= 0.0 || age < 0.0 || age > MARK_LIFE) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      o.local = vec2f(0.0);
      o.seed = 0.0;
      o.world = at;
      return o;
    }

    // A frame square to the incoming line. `other` is whichever axis the line
    // is least parallel to, so the cross product never collapses. (Not `ref`:
    // WGSL keeps that word, the way it keeps `from`.)
    let n = normalize(dir);
    let other = select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(n.y) > 0.9);
    let rgt = normalize(cross(other, n));
    let upv = cross(n, rgt);
    // Turned by its own position, so two chips side by side are not the same
    // stamp twice.
    let a = fract(sin(dot(at.xz, vec2f(127.1, 311.7))) * 43758.5453) * 6.2831853;
    let c = cos(a); let s = sin(a);
    let q = vec2f(cx * c - cy * s, cx * s + cy * c) * MARK_R;
    let p = at - n * 0.02 + rgt * q.x + upv * q.y;

    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.local = vec2f(cx, cy);
    o.seed = a;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let r = length(v.local);
    if (r > 1.0) { discard; }
    // A dark hole with a bruised rim and a ragged edge: the hole is what the
    // round took out, the rim is the dust it drove into the surface, and the
    // ragged edge is what keeps forty-eight of them from reading as forty-
    // eight identical circles.
    let ragged = 0.78 + 0.22 * sin(atan2(v.local.y, v.local.x) * 7.0 + v.seed * 5.0);
    let hole = 1.0 - smoothstep(0.30 * ragged, 0.52 * ragged, r);
    let rim = (1.0 - smoothstep(0.45, 1.0, r)) * (1.0 - hole);
    let a = clamp(hole * 0.88 + rim * 0.34, 0.0, 1.0) * v.fade;
    if (a < 0.01) { discard; }
    // Nearly black in the middle, pale grey dust around it, and the whole
    // thing under the same fog the surface it is stuck to is under.
    let col = mix(vec3f(0.44, 0.41, 0.37), vec3f(0.03, 0.028, 0.025), hole);
    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), a);
  }
