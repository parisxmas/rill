  // What moves on the MG3: the belt, and the brass.
  //
  // Neither is in the gun's mesh. Both are the same real cartridge now — one
  // 7.62x54mmR out of Kutejnikov's SVD magazine (OpenGameArt, CC-BY 4.0),
  // baked alone into cartridge.mesh — drawn thirteen times by instance:
  // seven on the belt from the ammunition box's mouth into the feed tray,
  // six as casings tumbling out of the ejection port. The belt is stepped
  // one pitch per shot: `u.lamp.w` carries the rounds left, so the scroll is
  // a fact about the magazine rather than an animation timer. The casings
  // run on a fixed loop gated by the muzzle flash — while the gun fires the
  // stream runs, and within a couple of tenths of it stopping the last one
  // has fallen out of frame.
  //
  // The path is placed as fractions of the weapon's own box, not in metres:
  // the gun has been rebaked at three different lengths already, and a
  // hardcoded point survives none of them. The cartridge's own box below is
  // the one exception — it is this file's private mesh, baked at one length
  // by us, and these two constants are that bake's header.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) alpha: f32,
  }

  const BELT_N = 7u;
  const BRASS_N = 6u;
  // One shot per server tick — see `mg_gap` in valley.rill.
  const RATE = 20.0;

  // cartridge.mesh's baked box: tools/bake_obj.py --length 0.063.
  const CART_LO = vec3f(-0.031, -0.005, -0.005);
  const CART_SP = vec3f(0.063, 0.010, 0.010);

  // A point as fractions of the gun's box.
  fn atBox(f: vec3f) -> vec3f {
    return u.akMin.xyz + f * u.akSize.xyz;
  }

  // The belt's path: box mouth, up over the shoulder, into the tray. A
  // quadratic through three points measured off the mesh, as fractions.
  //
  // Measured twice, because the first set was wrong in a way worth writing
  // down: the mesh's vertices are unorm16 — fractions of the box already —
  // and reading them as metres put the whole belt at the barrel's root. The
  // ammunition box itself is the cluster at x 0.33..0.41, y 0.49..0.86,
  // z 0.06..0.37 (it rides high on the left flank, its mouth at the
  // receiver's own top), and the feed slot is the receiver top just behind
  // the sight, x ≈ 0.42, z ≈ 0.46.
  fn beltAt(t: f32) -> vec3f {
    let a = atBox(vec3f(0.368, 0.790, 0.200));   // inside the box's mouth
    let b = atBox(vec3f(0.390, 0.915, 0.320));   // the arc over the flank
    let c = atBox(vec3f(0.425, 0.880, 0.460));   // down into the feed slot
    let s = 1.0 - t;
    return a * s * s + b * 2.0 * s * t + c * t * t;
  }

  @vertex fn vs(
    @builtin(instance_index) ii: u32,
    @location(0) posw: vec4f,
    @location(1) nrm4: vec4f,
    @location(2) uv: vec2f,
  ) -> VOut {
    var o: VOut;
    o.alpha = 1.0;
    o.uv = uv;
    let flash = clamp(u.fx.x, 0.0, 1.0);

    // The cartridge in its own metres, centred on its middle.
    var local = CART_LO + posw.xyz * CART_SP;
    var n = nrm4.xyz;
    var centre: vec3f;

    if (ii < BELT_N) {
      // ---- the belt --------------------------------------------------------
      // Rounds left drive the scroll: one pitch per shot. The fractional
      // step rides the flash, so each shot visibly yanks the belt rather
      // than teleporting it between frames.
      let left = u.lamp.w;
      let pull = clamp(flash * 1.6, 0.0, 1.0);
      let t = (f32(ii) + 0.55 - pull * 0.5) / f32(BELT_N);
      centre = beltAt(clamp(t, 0.02, 0.98));
      // The belt is only as long as what is left on it — and what is left
      // hangs at the feed end, because that is the end being eaten: the tail
      // back by the box is what runs out first.
      if (f32(ii) < f32(BELT_N) - left) { local = vec3f(0.0); }
    } else {
      // ---- the brass -------------------------------------------------------
      let j = ii - BELT_N;
      // Six casings on a fixed loop: each lives K/RATE seconds, staggered a
      // sixth apart, so at full rate the port streams continuously without
      // any per-shot state at all.
      let cycle = f32(BRASS_N) / RATE;
      let age = fract(u.eye.w * RATE / f32(BRASS_N) + f32(j) / f32(BRASS_N)) * cycle;
      // Only while the gun speaks. The flash is retriggered every shot and
      // decays in about a fifth of a second, which is also about how long a
      // casing takes to fall out of frame: the stream dies with the gun.
      if (flash < 0.03) { local = vec3f(0.0); }
      // A spent case is a touch smaller than a live round on the belt.
      local *= 0.9;
      // The port is on the receiver's right flank, under the feed slot the
      // belt just came in by — measured off the same clusters as the path.
      let port = atBox(vec3f(0.420, 0.560, 0.600));
      // Up first, then over: from behind the gun the port's own flank is
      // hidden by the receiver, so a casing thrown flat is invisible until
      // it drops out under the stock. Thrown up it clears the silhouette,
      // hangs for a tenth, and falls — which is also what the real gun's
      // brass looks like from the shoulder.
      let v0 = vec3f(0.12, 1.60, 1.30);
      let g = vec3f(0.0, -11.0, 0.0);
      centre = port + v0 * age + g * (0.5 * age * age);
      // Tumble: pitched end over end as it flies.
      let a = age * 34.0 + f32(j) * 2.4;
      let c = cos(a); let s2 = sin(a);
      local = vec3f(local.x, local.y * c - local.z * s2, local.y * s2 + local.z * c);
      n = vec3f(n.x, n.y * c - n.z * s2, n.y * s2 + n.z * c);
      o.alpha = clamp(2.0 - age / cycle * 2.2, 0.0, 1.0);
    }

    let m = centre + local;
    let cam = vmCam(m);
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(vmDir(n));
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.alpha < 0.02) { discard; }
    let n = normalize(v.normal);
    // The downloaded copper sheet, in the atlas quarter kept for it. The
    // cartridge's own unwrap lands somewhere on the sheet; copper is copper
    // wherever it lands, and the grain is the point.
    let sheet = textureSample(gunTex, barkSamp, cuUV(v.uv)).rgb;
    let albedo = sheet * vec3f(1.05, 0.72, 0.50);
    var col = lit(albedo * 0.5, n, v.world);

    // The plating, the way the AK's chrome does it — reflect the ray, sky
    // down the top and ground up the underside — but through copper: a
    // coloured metal tints what it mirrors, so the environment is multiplied
    // by the metal's own orange rather than added neat.
    let toEye = normalize(u.eye.xyz - v.world);
    let day = dayness();
    let refl = reflect(-toEye, n);
    let sky = skyColor(vec3f(refl.x, max(refl.y, 0.0), refl.z), u.eye.w);
    let below = mix(vec3f(0.030, 0.032, 0.042), vec3f(0.26, 0.22, 0.17), day);
    let env = mix(below, sky, smoothstep(-0.22, 0.10, refl.y));
    let fres = 0.60 + 0.40 * pow(1.0 - max(dot(n, toEye), 0.0), 4.0);
    col += env * vec3f(0.98, 0.62, 0.40) * fres * 0.9 * (0.5 + 0.5 * sheet.r);

    // The sun, as the tight hot spot a polished case throws.
    let L = -normalize(u.sun.xyz);
    let h = normalize(L + toEye);
    col += SUN_COL * vec3f(1.0, 0.75, 0.55) * pow(max(dot(n, h), 0.0), 60.0) * 1.6 * day;

    // The flash lights what is feeding it, the same way it lights the gun.
    let flash = clamp(u.fx.x, 0.0, 1.0);
    col += albedo * vec3f(1.0, 0.62, 0.26) * flash * 2.2;
    return vec4f(col, v.alpha);
  }
