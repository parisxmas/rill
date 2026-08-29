  // The rifle in the hands. It lives in the world — same sun, same shadow map,
  // same haze — but it is placed relative to the eye rather than to the valley,
  // and it is projected through its own frustum. Where it sits and how it is
  // held are in `common.wgsl`, because the muzzle flash has to agree with them
  // to the millimetre.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) uv: vec2f,
  ) -> VOut {
    // Unpack: position is unorm16 across the model's own bounding box, normal
    // is snorm8. Sixteen bytes a vertex instead of thirty-two.
    let m0 = u.vmMin.xyz + qpos.xyz * u.vmSize.xyz;
    // `qpos.w` says which part a vertex belongs to: one for the bolt, three
    // quarters for the bullet, a half for its case, zero for the rifle proper.
    // A flag baked into the spare component of a position that was already
    // being unpacked, so it costs the file nothing.
    let bolt = qpos.w > 0.85;
    let slug = qpos.w > 0.62 && qpos.w < 0.85;
    let brass = qpos.w > 0.25 && qpos.w < 0.62;
    var m = m0;
    if (bolt) { m = boltPos(m0); }
    else if (slug) { m = slugPos(m0); }
    else if (brass) { m = casePos(m0); }
    let cam = vmCam(m);

    var o: VOut;
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    var nm = qn.xyz;
    if (bolt) { nm = boltDir(nm); } else if (brass) { nm = caseDir(nm); }
    o.normal = camDirToWorld(vmDir(nm));
    o.uv = uv;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    // The whole sheet. Its lower half carried a painted AK for a while; the
    // AK is shaded from where it is now — see `ak.wgsl` — so the image went
    // back to the rifle's own two materials and the coordinates with it.
    let tex = textureSample(gunTex, barkSamp, rifleUV(v.uv)).rgb;

    // Which of the rifle's three materials this texel is, read off the sheet
    // itself rather than off a flag the mesh does not carry. The atlas is
    // unambiguous about it, measured: the wood is warm (red about three and a
    // half times the blue), the parkerised metal is a near-grey olive (red
    // only a quarter over the blue), and the scope with the dark fittings is
    // simply dark. Warm stays wood, dark stays black — the scope was asked to
    // — and what is left, grey and bright, is the metal that takes the plate.
    // The dark line sits at 0.26: the scope's texels measure 0.17 to 0.22,
    // the parkerised metal starts at 0.30. The first draft drew it at 0.16
    // and the scope came out chromed — which is exactly the kind of border
    // that has to come from measuring the sheet, not from a round number.
    let lum = dot(tex, vec3f(0.299, 0.587, 0.114));
    let warm = tex.r > tex.b * 1.7;
    let metal = !warm && lum > 0.26;

    // Chrome is not a colour, it is a mirror — see `ak.wgsl`, which went
    // first. The texel keeps a third of itself so the sheet's own shading
    // still reads as machining, and the brightness comes from the world.
    let albedo = select(tex, tex * 0.35, metal);
    let nn = normalize(v.normal);
    let n = select(-nn, nn, front);
    var col = lit(albedo, n, v.world);

    if (metal) {
      let toEye = normalize(u.eye.xyz - v.world);
      let L = -normalize(u.sun.xyz);
      let day = dayness();
      // The sun, as a tight highlight that travels as the rifle turns.
      let hlf = normalize(L + toEye);
      col += SUN_COL * pow(max(dot(n, hlf), 0.0), 55.0) * 1.3 * day;
      // And the world, mirrored: sky down the top of the barrel, the warm
      // dark of the ground up its underside, a horizon between them. The
      // fresnel floor is a metal's — chrome reflects at every angle.
      let refl = reflect(-toEye, n);
      let sky = skyColor(vec3f(refl.x, max(refl.y, 0.0), refl.z), u.eye.w);
      let below = mix(vec3f(0.030, 0.032, 0.042), vec3f(0.26, 0.22, 0.17), day);
      let env = mix(below, sky, smoothstep(-0.22, 0.10, refl.y));
      let fres = 0.55 + 0.45 * pow(1.0 - max(dot(n, toEye), 0.0), 4.0);
      col += env * fres * 0.85;
    }

    // The flash lights what is holding it. A rifle firing in the dark is the
    // brightest thing for a hundred metres, and none of that reaches the world
    // here — but the barrel and the hand are twenty centimetres from it, and
    // leaving them unlit is what makes a muzzle flash look like a decal.
    let flash = clamp(u.fx.x, 0.0, 1.0);
    col += albedo * vec3f(1.0, 0.62, 0.26) * flash * 2.2;
    return vec4f(col, 1.0);
  }
