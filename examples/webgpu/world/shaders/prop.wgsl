  // The ammunition lying about the valley.
  //
  // A real box: Poly Haven's `ammo_box`, CC0, photoscanned, and the same
  // library the rifle came out of. Twenty-six centimetres of stamped steel,
  // and the world scales it up from there — sixteen of them are spread over a
  // hundred and fifty metres of valley, and at life size a box in that grass
  // is something you walk past rather than something you find. How far up is
  // Rill's to say and is in the instance, not here.
  //
  // It rides the shard buffer. A magazine is a shard as far as Rill is
  // concerned: the same twenty floats, a position, an orientation and a scale,
  // and the same trick for being gone — scale zero while it is on its way
  // back. So this pipeline is the shard pipeline's instances read a second
  // time, and the two draws divide the buffer between them rather than each
  // keeping their own copy of where things are.
  //
  // Only the yaw is used. These do not tumble: a box that has been put down
  // has been put down flat, and the other two angles are left at zero by the
  // world rather than ignored here.

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
    // Unpacked against the box the bake quantized it into, the same way the
    // rifle and the shark are.
    let m = u.propMin.xyz + qpos.xyz * u.propSize.xyz;
    let c = cos(rot.y);
    let s = sin(rot.y);
    let M = mat3x3f(vec3f(c, 0.0, -s), vec3f(0.0, 1.0, 0.0), vec3f(s, 0.0, c));
    let p = M * (m * scale) + ipos;

    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = M * qn.xyz;
    o.uv = quv;
    // Taken, and not back yet. Past the far plane rather than branched on,
    // which is what the shards and the tracers do with an empty slot.
    if (scale < 0.0001) { o.clip = vec4f(0.0, 0.0, 2.0, 1.0); }
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let albedo = textureSample(propTex, barkSamp, v.uv).rgb;
    let n = normalize(v.normal);
    let col = lit(albedo, n, v.world);
    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
