  // The MG3 in the hands: mightydinosaurcol's M60 (OpenGameArt, CC-BY 4.0),
  // baked by bake_obj.py with the barrel on +x, standing in for the German
  // gun the key is named after. One material end to end — a belt gun is
  // steel wearing steel — shaded by the AK's own recipe: a dark base with
  // the world mirrored into it, because gunmetal is a poor mirror but a
  // mirror all the same.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) model: vec3f,
    @location(3) uv: vec2f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) uv: vec2f,
  ) -> VOut {
    // `akMin/akSize` carry whichever automatic gun is in the hands — only
    // one viewmodel draws per frame, so the page points these at the MG's
    // box while it is up. See the fill beside `akBox` in host.js.
    let m = u.akMin.xyz + qpos.xyz * u.akSize.xyz;
    let cam = vmCam(m);
    var o: VOut;
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(vmDir(qn.xyz));
    o.model = m;
    o.uv = uv;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    // The gun's own photograph, off the right half of the shared sheet.
    //
    // It was painted rather than textured until now: one flat gunmetal for
    // the whole weapon, which read as a black silhouette with a highlight on
    // it. The model came with its sheet all along — what was missing was the
    // coordinates, which `bake_obj.py` wrote as zeros because the first model
    // it was asked to bake had none.
    let albedo = textureSample(gunTex, barkSamp, mgUV(v.uv)).rgb;
    let nn = normalize(v.normal);
    let n = select(-nn, nn, front);
    var col = lit(albedo, n, v.world);

    let toEye = normalize(u.eye.xyz - v.world);
    let L = -normalize(u.sun.xyz);
    let day = dayness();
    let hlf = normalize(L + toEye);
    // The highlight belongs to the metal, and the sheet says which parts are
    // metal: the dark ones. Wood and webbing take a fraction of it.
    let lum = dot(albedo, vec3f(0.299, 0.587, 0.114));
    let metal = 1.0 - smoothstep(0.10, 0.34, lum);
    col += SUN_COL * pow(max(dot(n, hlf), 0.0), 55.0) * 1.1 * day * metal;
    // The world, in the metal — oiled steel is a poor mirror but a mirror.
    let refl = reflect(-toEye, n);
    let sky = skyColor(vec3f(refl.x, max(refl.y, 0.0), refl.z), u.eye.w);
    let below = mix(vec3f(0.030, 0.032, 0.042), vec3f(0.26, 0.22, 0.17), day);
    let env = mix(below, sky, smoothstep(-0.22, 0.10, refl.y));
    let fres = 0.35 + 0.45 * pow(1.0 - max(dot(n, toEye), 0.0), 4.0);
    col += env * fres * 0.28 * metal;

    // The flash lights what is holding it, and this one flashes a lot.
    let flash = clamp(u.fx.x, 0.0, 1.0);
    col += albedo * vec3f(1.0, 0.62, 0.26) * flash * 2.6;
    return vec4f(col, 1.0);
  }
