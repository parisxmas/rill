  // The flash at the muzzle. Two quads: one square-on to the camera for the
  // fireball, one lying along the bore for the jet of gas that comes out with
  // it. Both live in the rifle's own frame, so they ride the recoil and the
  // walk with the barrel rather than sliding about on top of it.
  //
  // The whole thing is over in about eighty milliseconds. It is driven by the
  // same recoil the rifle is, raised to the seventh — a rifle kicks for a fifth
  // of a second and burns for a twentieth, and one number can serve both if the
  // faster of the two is given a steeper curve.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,      // -1 to 1 both ways
    @location(1) flash: f32,
    @location(2) kind: f32,      // 0 the fireball, 1 the jet
  }

  // Just past the crown of the barrel, in model coordinates.
  const MUZZLE = vec3f(0.60, 0.050, 0.0);

  @vertex fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    let q = vi % 6u;
    let kind = f32(vi / 6u);
    let cx = select(-1.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cy = select(-1.0, 1.0, q == 2u || q == 4u || q == 5u);

    // Its own number, not the recoil raised to the seventh: see `fx` in
    // uniforms.wgsl for what that cost the AK.
    let flash = clamp(u.fx.x, 0.0, 1.0);

    var o: VOut;
    o.uv = vec2f(cx, cy);
    o.flash = flash;
    o.kind = kind;
    if (flash < 0.004) {                    // nothing to draw between shots
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    // The fireball grows a little as it dies, the way an expanding gas does.
    let grow = 0.055 + 0.030 * (1.0 - flash);
    var cam: vec3f;
    if (kind < 0.5) {
      // Square to the camera: the fireball, at the muzzle.
      cam = vmCam(MUZZLE) + vec3f(cx, cy, 0.0) * grow;
    } else {
      // Along the bore: the jet, reaching out ahead of the muzzle and thin
      // across it. Built from the barrel's own axis so it points where the
      // barrel does, not where the camera does.
      let base = vmCam(MUZZLE);
      let ax = vmDir(vec3f(1.0, 0.0, 0.0));          // down the bore
      let up = vmDir(vec3f(0.0, 1.0, 0.0));
      cam = base + ax * ((cx * 0.5 + 0.5) * 0.30 * flash) + up * (cy * grow * 0.55);
    }
    o.clip = vmClip(cam);
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.flash < 0.004) { discard; }

    var glow = 0.0;
    var col = vec3f(1.0);
    if (v.kind < 0.5) {
      // A hot round core with a ragged star around it. The star is a few
      // harmonics of the angle rather than a texture: powder burns in whatever
      // shape it likes, and three cosines beating against each other never
      // repeat over a burst the way a sprite would.
      let r = length(v.uv);
      let a = atan2(v.uv.y, v.uv.x);
      let spikes = 0.55 + 0.45 * cos(a * 5.0 + 0.7) * cos(a * 3.0 - 1.9);
      let core = exp(-r * r * 14.0);
      let petal = pow(clamp(1.0 - r / max(spikes, 0.15), 0.0, 1.0), 2.2);
      glow = core * 1.6 + petal * 0.85;
      col = mix(vec3f(1.0, 0.40, 0.10), vec3f(1.0, 0.96, 0.85), clamp(core * 1.5, 0.0, 1.0));
    } else {
      // The jet: bright at the crown, gone by the tip, and tapering across.
      let along = v.uv.x * 0.5 + 0.5;
      let across = 1.0 - v.uv.y * v.uv.y;
      let taper = clamp(1.0 - along, 0.0, 1.0);
      glow = across * across * taper * taper * 1.5;
      col = mix(vec3f(1.0, 0.92, 0.74), vec3f(1.0, 0.36, 0.08), along);
    }
    return vec4f(col * glow * v.flash * 3.2, 1.0);
  }
