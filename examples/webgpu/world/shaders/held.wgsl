  // The weapon in somebody else's hands.
  //
  // One matrix per person, worked out in the engine as the hand's own world
  // matrix times a grip derived at bake time — so the weapon follows the arm
  // through every clip without the page knowing anything about the rig beyond
  // which joint it is. What the matrix carries changed when the wire started
  // saying which weapon each player holds: an instance is now the matrix and
  // the mesh's own quantization box, because the rifle, the launcher and the
  // AK were each baked against their own bounds and a box from the uniform
  // block could only ever be right for one of them at a time.
  //
  // The kind rides in the box's spare fourth component: 0 the rifle, 1 the
  // launcher, 2 the AK. The page groups the instances by kind and draws each
  // group with its own mesh; the fragment shader picks the surface to match —
  // the rifle and the launcher off their photographs, the AK from where the
  // point sits on the weapon, exactly as the viewmodel does, because that
  // model has no texture coordinates to sample by.
  //
  // Nothing here animates the bolt or the round. Those belong to the gun in
  // your own hands, where they are ten centimetres from the eye; at the
  // distance somebody else is, a bolt is two pixels.

  struct Held {
    m: mat4x4f,
    bmin: vec4f,   // the box's corner; w is the weapon kind
    bsize: vec4f,
  }
  @group(1) @binding(0) var<storage, read> held: array<Held>;

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) model: vec3f,
    @location(4) @interpolate(flat) kind: f32,
  }

  @vertex fn vs(
    @builtin(instance_index) inst: u32,
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) uv: vec2f,
  ) -> VOut {
    let h = held[inst];
    let rest = h.bmin.xyz + qpos.xyz * h.bsize.xyz;
    let p = (h.m * vec4f(rest, 1.0)).xyz;
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    // The grip scales evenly, so the rotation part carries a normal as it is.
    o.normal = (h.m * vec4f(qn.xyz, 0.0)).xyz;
    o.uv = uv;
    o.model = rest;
    o.kind = h.bmin.w;
    return o;
  }

  // The AK's surface, said from where a point sits — the same four places the
  // viewmodel reads, without the magazine theatre: at the distance somebody
  // else stands, a magazine is the dark rectangle it always was.
  fn heldAkWood(m: vec3f) -> vec3f {
    let grain = sin(m.x * 210.0 + m.z * 14.0) * 0.5 + sin(m.x * 61.0) * 0.5;
    return mix(vec3f(0.196, 0.098, 0.043), vec3f(0.35, 0.185, 0.082), 0.5 + 0.22 * grain);
  }

  fn heldAkSteel(m: vec3f) -> vec3f {
    let mill = sin(m.x * 900.0) * 0.5 + 0.5;
    return vec3f(0.048, 0.050, 0.056) + vec3f(0.016) * mill;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    // Both photographs are read whatever the kind: a textureSample inside a
    // branch on a per-instance value is exactly the non-uniform control flow
    // WGSL forbids, and two small samples cost less than being clever.
    let aRifle = textureSample(rifleTex, barkSamp, v.uv).rgb;
    let aRpg = textureSample(rpgTex, barkSamp, v.uv).rgb;
    let m = v.model;
    let wood = (m.x < -0.26)
            || (m.x > -0.06 && m.x < 0.035 && m.y < -0.02)
            || (m.x > 0.11 && m.x < 0.31 && m.y > -0.01);
    let aAk = select(heldAkSteel(m), heldAkWood(m), wood);
    let albedo = select(select(aAk, aRpg, v.kind < 1.5), aRifle, v.kind < 0.5);
    let nn = normalize(v.normal);
    let col = lit(albedo, select(-nn, nn, front), v.world);
    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
