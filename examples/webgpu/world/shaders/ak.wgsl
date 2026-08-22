  // The AK in the hands.
  //
  // Its own shader rather than the rifle's, and the reason is that the model
  // arrived as geometry and nothing else: a high-poly AK-47 published as a
  // raw OBJ, CC0, with no texture coordinates and no normals in the file.
  // The normals are worked out at bake time; the surface is worked out here,
  // from where a point sits along the weapon. That turns out to be the right
  // way round for this one — an AK is four materials in four places, and
  // where a thing is says which it is more reliably than a photograph
  // stretched over a model that was never unwrapped for one.
  //
  // The four, measured off the model's own profile:
  //
  //   the butt              behind -0.26
  //   the pistol grip       under the receiver, around the middle
  //   the handguard         over the barrel, forward of +0.11
  //   everything else       steel — and the barrel is the point of it

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) model: vec3f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) uv: vec2f,
  ) -> VOut {
    let m = u.akMin.xyz + qpos.xyz * u.akSize.xyz;
    let cam = vmCam(m);
    var o: VOut;
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(vmDir(qn.xyz));
    o.model = m;
    return o;
  }

  // Wood: laminated birch, dark with the varnish, grain running the length of
  // the piece. Two frequencies, because one reads as stripes.
  fn akWood(m: vec3f) -> vec3f {
    let grain = sin(m.x * 210.0 + m.z * 14.0) * 0.5 + sin(m.x * 61.0) * 0.5;
    let t = 0.5 + 0.22 * grain;
    return mix(vec3f(0.196, 0.098, 0.043), vec3f(0.35, 0.185, 0.082), t);
  }

  // Steel, blued. Nearly black to look at and almost entirely what the light
  // does to it — which is why the barrel needs the specular below to exist at
  // all, and why a photograph of one is mostly a photograph of the room.
  fn akSteel(m: vec3f) -> vec3f {
    let mill = sin(m.x * 900.0) * 0.5 + 0.5;      // the turn of the lathe
    return vec3f(0.048, 0.050, 0.056) + vec3f(0.016) * mill;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let m = v.model;
    let wood = (m.x < -0.26)
            || (m.x > -0.06 && m.x < 0.035 && m.y < -0.02)
            || (m.x > 0.11 && m.x < 0.31 && m.y > -0.01);
    let albedo = select(akSteel(m), akWood(m), wood);

    let nn = normalize(v.normal);
    let n = select(-nn, nn, front);
    var col = lit(albedo, n, v.world);

    // What makes it metal. Wood scatters and steel reflects, so the barrel
    // gets a tight highlight where the sun is and a wide one off the sky —
    // the second is what keeps it from going to a black stick after dark,
    // which is what the old skin did and why the barrel could not be found.
    if (!wood) {
      let toEye = normalize(u.eye.xyz - v.world);
      let L = -normalize(u.sun.xyz);
      let h = normalize(L + toEye);
      let day = dayness();
      let sun = pow(max(dot(n, h), 0.0), 55.0) * 1.3 * day;
      // A rim off the sky, which is the light that is always there: brightest
      // where the surface turns away, so a round barrel gets a line down its
      // length and reads as a cylinder rather than as a dark bar.
      // Tight, and weak. At a wide exponent and nine tenths this washed the
      // whole weapon to bone — every triangle facing away from the eye lit up,
      // which on a model this dense is most of them. What is wanted is a line
      // down the edge of a round thing, not a coat of paint.
      let rim = pow(1.0 - max(dot(n, toEye), 0.0), 5.0);
      col += SUN_COL * sun;
      col += mix(vec3f(0.10, 0.11, 0.17), vec3f(0.30, 0.32, 0.42), day) * rim * 0.10;
    }

    // The flash lights what is holding it, the same way the rifle's does.
    let flash = pow(clamp(u.wind.w, 0.0, 1.0), 7.0);
    col += albedo * vec3f(1.0, 0.62, 0.26) * flash * 2.2;
    return vec4f(col, 1.0);
  }
