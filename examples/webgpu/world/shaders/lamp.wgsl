  // The lamp itself, as a thing you can see.
  //
  // `otherLamps` in common.wgsl already puts everybody else's torch into the
  // lighting, so a beam lands on the hillside and on whoever is standing in
  // it. What was missing is the source: a light was falling on the rock with
  // nothing at the end of it, so on a dark ridge you saw the patch and not the
  // person carrying it. This draws the bulb.
  //
  // No vertex buffer and no instance buffer — the six lamp slots are already
  // in the uniform block for the lighting, so this is `draw(6, 6)` and an
  // index. A slot nobody is using has `w` of nought and its quad is thrown out
  // of the clip volume rather than drawn at zero size, so it costs nothing
  // downstream.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) face: f32,     // 1 looking into the lamp, 0 from behind it
  }

  @vertex fn vs(@builtin(vertex_index) vi: u32,
                @builtin(instance_index) inst: u32) -> VOut {
    let at = u.lamps[inst * 2u];
    let dir = u.lamps[inst * 2u + 1u].xyz;
    let corner = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) - 0.5;

    // On the brow, not in the skull. The uniform's lamp sits where the beam
    // leaves somebody — their eye height, which is the *middle* of their head —
    // and a billboard centred there is inside the mesh, so the depth test eats
    // it and the light has no source after all. Carried forward along the beam
    // it clears the face, which is also where a head torch is actually strapped:
    // seen from the front it is a lamp, and from behind the wearer's own head
    // occludes it, which is the truth about somebody walking away from you.
    let brow = at.xyz + dir * 0.24;

    // Billboarded on the camera's own right and up, so it is a disc from every
    // side rather than a card that thins out as you walk round it.
    let yaw = u.cam.x;
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    let toEye = normalize(u.eye.xyz - brow);
    // `right` is horizontal and `toEye` can run straight up it — somebody
    // directly overhead, or a lamp at the eye — and the cross of two parallel
    // unit vectors is nought, which normalise turns into a quad the size of the
    // sky. Both are unit, so world up is a fine stand-in on the degenerate frame.
    let axis = cross(right, toEye);
    let up = select(vec3f(0.0, 1.0, 0.0), normalize(axis), length(axis) > 0.05);
    // Small: a head torch is the size of a fist, and the glow around it is
    // what makes it findable, not the size of the quad.
    let size = 0.34;
    let world = brow + right * corner.x * size + up * corner.y * size;

    var o: VOut;
    o.clip = u.vp * vec4f(world, 1.0);
    o.uv = corner * 2.0;
    // Straight down the beam it is the bulb; from behind it is the back of a
    // housing with light leaking round it. A lamp coming over a ridge towards
    // you should be the brightest thing on the hillside, and the same lamp
    // walking away should be a spark.
    //
    // `toEye` runs from the lamp to the eye, so a beam pointed at you is a
    // beam whose direction agrees with it. It was written against `-toEye`,
    // which is the same statement backwards: the lamp was dimmest exactly
    // when its owner was looking straight at you, which is the one moment
    // it is supposed to be blinding.
    o.face = max(dot(dir, toEye), 0.0);
    // An empty slot, out of the frustum.
    if (at.w < 0.5) { o.clip = vec4f(0.0, 0.0, -2.0, 1.0); }
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let d = length(v.uv);
    let a = max(1.0 - d, 0.0);
    // A hot core in a soft halo: the core is the bulb and the halo is the air
    // around it, and one without the other reads as a sticker.
    let halo = a * a * a;
    let core = pow(a, 12.0);
    let lookingIn = 0.28 + 0.72 * v.face * v.face;
    // Mostly a night thing. It is still on in daylight — the switch is the
    // player's — but a torch at noon is not what anybody is looking for.
    let night = 1.0 - dayness() * 0.75;
    let warm = vec3f(1.0, 0.94, 0.80);
    return vec4f(warm * (halo * 0.80 + core * 2.6) * lookingIn * night, 1.0);
  }
