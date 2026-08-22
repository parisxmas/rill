  override AKV: f32 = 0.0;

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
    // Which gun this pipeline was built for: 0 the rifle, 1 the AK. Baked in
    // as a constant, so it is two pipelines off one shader rather than a
    // branch every vertex pays for.
    let m0 = select(u.vmMin.xyz + qpos.xyz * u.vmSize.xyz,
                    u.akMin.xyz + qpos.xyz * u.akSize.xyz, AKV > 0.5);
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
    // The top half of the sheet: the lower half was the AK's skin until the
    // AK got a shader of its own, and the image is left at its full height so
    // the rifle's own coordinates do not have to move.
    let albedo = textureSample(rifleTex, barkSamp, vec2f(v.uv.x, v.uv.y * 0.5)).rgb;
    let nn = normalize(v.normal);
    var col = lit(albedo, select(-nn, nn, front), v.world);
    // The flash lights what is holding it. A rifle firing in the dark is the
    // brightest thing for a hundred metres, and none of that reaches the world
    // here — but the barrel and the hand are twenty centimetres from it, and
    // leaving them unlit is what makes a muzzle flash look like a decal.
    let flash = pow(clamp(u.wind.w, 0.0, 1.0), 7.0);
    col += albedo * vec3f(1.0, 0.62, 0.26) * flash * 2.2;
    return vec4f(col, 1.0);
  }
