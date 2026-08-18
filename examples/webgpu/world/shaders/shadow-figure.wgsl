  // The same skin, from the sun's side. Position only: a shadow map holds
  // depth and nothing else, so there is no normal to rotate and no colour to
  // decide, and this pass must not be able to read the map it is writing.

  override BOX_MIN_X: f32 = 0.0;
  override BOX_MIN_Y: f32 = 0.0;
  override BOX_MIN_Z: f32 = 0.0;
  override BOX_MAX_X: f32 = 1.0;
  override BOX_MAX_Y: f32 = 1.0;
  override BOX_MAX_Z: f32 = 1.0;
  override JOINTS: u32 = 53u;

  @group(1) @binding(0) var<storage, read> pose: array<mat4x4f>;

  @vertex fn vs(
    @builtin(instance_index) inst: u32,
    @location(0) qpos: vec4f,
    @location(1) qn: vec4f,
    @location(2) j: vec4u,
    @location(3) w: vec4f,
  ) -> @builtin(position) vec4f {
    let base = inst * JOINTS;
    let m = pose[base + j.x] * w.x + pose[base + j.y] * w.y
          + pose[base + j.z] * w.z + pose[base + j.w] * w.w;
    let p = vec3f(mix(BOX_MIN_X, BOX_MAX_X, qpos.x),
                  mix(BOX_MIN_Y, BOX_MAX_Y, qpos.y),
                  mix(BOX_MIN_Z, BOX_MAX_Z, qpos.z));
    return u.lvp * (m * vec4f(p, 1.0));
  }
