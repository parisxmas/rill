  // The rocket sitting in the tube, before it is anybody's problem.
  //
  // The launcher and the rocket came out of one file as two nodes, and the
  // rocket was authored where a loaded one sits: it runs from 0.277 to 1.234
  // along the launcher's own axis, which overlaps the last 0.29 of the tube
  // and stands two thirds of a metre out of the muzzle. So it is drawn through
  // the launcher's own transform, `rpgCam`, and lands exactly where the artist
  // put it. Nothing here had to be measured, and nothing can drift: move the
  // launcher and the warhead moves with it, because it is the same arithmetic
  // and not a copy of it.
  //
  // Whether it is drawn at all is the page's decision, not this shader's — it
  // is there when the tube is loaded and gone from the shot until the next one
  // is in, which is what makes the two and a half seconds of reloading visible
  // instead of being a number the trigger silently obeys.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) quv: vec2f,
  ) -> VOut {
    let m = u.rocketMin.xyz + qpos.xyz * u.rocketSize.xyz;
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
    return vec4f(lit(albedo, n, v.world), 1.0);
  }
