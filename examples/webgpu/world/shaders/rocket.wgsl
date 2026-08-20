  // The rocket, in the air.
  //
  // It rides the fireballs' ring: a rocket is a fireball with somebody's aim
  // behind it, so it flies, drags, falls and craters through arithmetic the
  // crystals were already using, and it arrives on the wire as the same `t`
  // that a crystal's throw does. What tells the two apart is the sign of the
  // seed — negative is a rocket — which costs the ring no field and the
  // protocol no message. `fireball.wgsl` drops the head quad for those and
  // leaves the smoke; this draws what is at the head instead.
  //
  // The model is the same file the launcher came out of and shares its texture,
  // so the thing in the air is the thing that was in the tube a moment ago.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) quv: vec2f,
    @location(3) pos: vec3f, @location(4) vel: vec3f,
    @location(5) seed: f32, @location(6) life: f32,
  ) -> VOut {
    var o: VOut;
    o.world = pos;
    o.normal = vec3f(0.0, 1.0, 0.0);
    o.uv = quv;
    if (life <= 0.0 || seed >= 0.0) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    // Unpacked against its own bounding box, the way every baked mesh here is.
    // The model lies along +X — the same axis the rifle and the launcher use —
    // and its origin is not its nose, so it is pulled back to its own middle
    // before it is turned. Without that it swings about a point off to one side
    // and reads as a thing being flung rather than a thing flying.
    let m0 = u.rocketMin.xyz + qpos.xyz * u.rocketSize.xyz;
    let m = m0 - (u.rocketMin.xyz + u.rocketSize.xyz * 0.5);

    // Nose along the velocity. Any up will do for the second axis as long as it
    // is not the first: a rocket has no roll anybody can see, and picking the
    // world's up fails only when it is flying straight down, which is the one
    // case the fallback covers.
    let f = normalize(vel);
    let axis = select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(f.y) > 0.99);
    let r = normalize(cross(axis, f));
    let up = cross(f, r);
    let p = f * m.x + up * m.y + r * m.z + pos;
    let n = f * qn.x + up * qn.y + r * qn.z;

    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = n;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let albedo = textureSample(rpgTex, barkSamp, v.uv).rgb;
    let n = normalize(v.normal);
    // Lit like anything else in the valley, and then a little light of its own
    // at the tail: there is a motor burning in there, and a rocket that takes
    // only the sun looks like a thrown pipe.
    var col = lit(albedo, n, v.world);
    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
