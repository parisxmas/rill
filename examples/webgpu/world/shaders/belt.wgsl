  // The belts of machine-gun rounds lying about the valley.
  //
  // The third kind of box on the ground, and the only one that is not a
  // photograph of a thing somebody scanned: it is twenty-two copies of the
  // same 7.62x54mmR cartridge the gun feeds (Kutejnikov, OpenGameArt CC-BY
  // 4.0) laid along a shallow S and baked as one mesh, so what is lying in
  // the grass is the same round that will be on the belt when it is picked
  // up. It wears the copper sheet the fed belt wears — ambientCG's Metal008,
  // in the atlas quarter kept for it — and it is lit like the fed belt too,
  // which is the point: a box of stamped steel goes flat in shade and a belt
  // of copper throws the sky back at you from two hundred metres.
  //
  // It rides the same shard-buffer instances the boxes and the crates do; the
  // draw takes the back range of the slots and `firstInstance` divides them.
  //
  // The quantization box is written down here rather than passed in a
  // uniform, the way `mgfx.wgsl` writes down the cartridge's: this mesh is
  // ours, baked at one length by `tools/bake_obj.py --length 0.13
  // --bore 0.005`, and these two constants are that bake's header.

  const BELT_LO = vec3f(-0.065, 0.0, -0.153);
  const BELT_SP = vec3f(0.130, 0.010, 0.305);

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) quv: vec2f,
    @location(3) ipos: vec3f, @location(4) rot: vec3f, @location(5) scale: f32,
  ) -> VOut {
    let m = BELT_LO + qpos.xyz * BELT_SP;
    let c = cos(rot.y);
    let s = sin(rot.y);
    let M = mat3x3f(vec3f(c, 0.0, -s), vec3f(0.0, 1.0, 0.0), vec3f(s, 0.0, c));
    let p = M * (m * scale) + ipos;

    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = M * qn.xyz;
    o.uv = quv;
    // Taken, and not back yet — past the far plane, like the boxes.
    if (scale < 0.0001) { o.clip = vec4f(0.0, 0.0, 2.0, 1.0); }
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let n = normalize(v.normal);
    let sheet = textureSample(gunTex, barkSamp, cuUV(v.uv)).rgb;
    let albedo = sheet * vec3f(1.05, 0.72, 0.50);
    var col = lit(albedo * 0.5, n, v.world);

    // The same copper as the fed belt: the environment is mirrored and then
    // tinted by the metal, because a coloured metal colours what it reflects.
    let toEye = normalize(u.eye.xyz - v.world);
    let day = dayness();
    let refl = reflect(-toEye, n);
    let sky = skyColor(vec3f(refl.x, max(refl.y, 0.0), refl.z), u.eye.w);
    let below = mix(vec3f(0.030, 0.032, 0.042), vec3f(0.26, 0.22, 0.17), day);
    let env = mix(below, sky, smoothstep(-0.22, 0.10, refl.y));
    let fres = 0.60 + 0.40 * pow(1.0 - max(dot(n, toEye), 0.0), 4.0);
    col += env * vec3f(0.98, 0.62, 0.40) * fres * 0.9 * (0.5 + 0.5 * sheet.r);

    let L = -normalize(u.sun.xyz);
    let h = normalize(L + toEye);
    col += SUN_COL * vec3f(1.0, 0.75, 0.55) * pow(max(dot(n, h), 0.0), 60.0) * 1.6 * day;

    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
