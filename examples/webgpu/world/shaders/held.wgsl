  // The rifle in somebody else's hands.
  //
  // The same file the viewmodel draws, put somewhere else: one matrix per
  // person, worked out on the page as the hand's own world matrix times a grip
  // that was derived at bake time from the direction the arm points. So the
  // weapon follows the arm through every clip without the page knowing
  // anything about the rig beyond which joint it is.
  //
  // Nothing here animates the bolt or the round. Those belong to the gun in
  // your own hands, where they are ten centimetres from the eye; at the
  // distance somebody else is, a bolt is two pixels.

  @group(1) @binding(0) var<storage, read> held: array<mat4x4f>;

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  @vertex fn vs(
    @builtin(instance_index) inst: u32,
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) uv: vec2f,
  ) -> VOut {
    let m = held[inst];
    let p = (m * vec4f(u.vmMin.xyz + qpos.xyz * u.vmSize.xyz, 1.0)).xyz;
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    // The grip scales evenly, so the rotation part carries a normal as it is.
    o.normal = (m * vec4f(qn.xyz, 0.0)).xyz;
    o.uv = uv;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    // The top half of the sheet: the AK's skin moved in below the rifle's
    // materials, and everybody else is drawn carrying the rifle.
    let albedo = textureSample(rifleTex, barkSamp, vec2f(v.uv.x, v.uv.y * 0.5)).rgb;
    let nn = normalize(v.normal);
    let col = lit(albedo, select(-nn, nn, front), v.world);
    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
