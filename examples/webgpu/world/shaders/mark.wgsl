  // Where an AK's round stopped.
  //
  // The rifle draws its rounds on the way and needs nothing here; the AK's
  // are not drawn at all — ten streaks a second is a lit rope across the
  // valley — so the chip they leave is the whole of the evidence, and it is
  // worth ten seconds of somebody's attention.
  //
  // A card lying in the surface, which is what the ring's second vector is:
  // the outward normal of whatever the round stopped on — the gradient of the
  // ground, or the line out of a wall block's centre. See `note_mark`.
  //
  // It was built on the round's *flight* before, square to the line it came
  // in on. That is cheap and it is wrong: a card square to the flight faces
  // whoever fired it, so it turned to follow you as you walked round it and
  // read as a sticker hanging in the air rather than as a hole in something.
  // On flat ground it stood up out of the grass at whatever angle you had
  // been shooting from.
  //
  // Still a card and not a projected decal — nothing here reprojects onto the
  // depth buffer — so it is pushed a few centimetres out along the normal to
  // sit on the surface rather than inside it.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) local: vec2f,
    @location(2) fade: f32,
    @location(3) seed: f32,
  }

  const MARK_LIFE = 10.0;      // must match `mark_life` in valley.rill
  // The card's half-width; the hole inside it is about a third of that again.
  // Doubled from 0.055. A hole a hand's width across is what a rifle round
  // actually leaves and it was nearly invisible at the range the valley is
  // fought at — this is a chip out of the rock rather than a bullet hole, and
  // reading where a burst went is the whole reason the marks are drawn.
  const MARK_R = 0.11;

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

    // A frame lying in the surface. `other` is whichever axis the normal is
    // least parallel to, so the cross product never collapses. (Not `ref`:
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
    // Out of the surface rather than back down the flight. Three centimetres,
    // not two: a card twice as wide reaches further across whatever it is
    // stuck to, and on a curved face its corners were sinking into the rock.
    let p = at + n * 0.03 + rgt * q.x + upv * q.y;

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
