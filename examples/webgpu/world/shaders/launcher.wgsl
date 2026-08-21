  // The launcher in the hands.
  //
  // Same idea as the rifle's viewmodel and deliberately not the same code: a
  // rifle is held at the shoulder with the sights on the eye's own axis, and
  // this is a tube carried *over* the shoulder with the eye looking past it.
  // What they do share is the frustum — `vmClip`, a near plane at a centimetre
  // and a tighter field of view than the valley's — because the reason for that
  // is the distance from the eye, which is the same for both.
  //
  // An RPG-7, CC0, out of OpenGameArt. It lies along +X like everything else
  // baked here.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) quv: vec2f,
  ) -> VOut {
    let m = u.rpgMin.xyz + qpos.xyz * u.rpgSize.xyz;
    let cam = rpgCam(m);

    var o: VOut;
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(rpgDir(qn.xyz));
    o.uv = quv;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let albedo = textureSample(rpgTex, barkSamp, v.uv).rgb;
    let n = normalize(v.normal);
    // Lit by the valley it is standing in, like the rifle: same sun, same
    // shadow map, same haze, so a weapon carried into shade goes into shade.
    return vec4f(lit(albedo, n, v.world), 1.0);
  }
