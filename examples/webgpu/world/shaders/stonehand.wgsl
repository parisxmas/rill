  // The stone in the building hand: one box, no buffers, corners out of the
  // vertex index. It wears the same grey the built rocks wear, so what is in
  // the hand is what goes down. It dips with `u.wind.w` when a rock is set.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
  }

  const SH_CORNER = array<vec3f, 8>(
    vec3f(-1.0, -1.0, -1.0), vec3f(1.0, -1.0, -1.0),
    vec3f(-1.0, 1.0, -1.0),  vec3f(1.0, 1.0, -1.0),
    vec3f(-1.0, -1.0, 1.0),  vec3f(1.0, -1.0, 1.0),
    vec3f(-1.0, 1.0, 1.0),   vec3f(1.0, 1.0, 1.0),
  );
  const SH_IDX = array<u32, 36>(
    0u, 2u, 1u, 1u, 2u, 3u,
    4u, 5u, 6u, 5u, 7u, 6u,
    0u, 1u, 4u, 1u, 5u, 4u,
    2u, 6u, 3u, 3u, 6u, 7u,
    0u, 4u, 2u, 2u, 4u, 6u,
    1u, 3u, 5u, 3u, 7u, 5u,
  );
  const SH_NRM = array<vec3f, 6>(
    vec3f(0.0, 0.0, -1.0), vec3f(0.0, 0.0, 1.0),
    vec3f(0.0, -1.0, 0.0), vec3f(0.0, 1.0, 0.0),
    vec3f(-1.0, 0.0, 0.0), vec3f(1.0, 0.0, 0.0),
  );

  @vertex fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    let corner = SH_CORNER[SH_IDX[vi % 36u]];
    let n0 = SH_NRM[(vi % 36u) / 6u];
    let r = u.wind.w;
    let m = corner * vec3f(0.052, 0.046, 0.055);
    let cam = m + vec3f(0.15, -0.16 - 0.06 * r, 0.27 + 0.05 * r);
    var o: VOut;
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(n0);
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let nn = normalize(v.normal);
    let col = lit(vec3f(0.42, 0.40, 0.38), select(-nn, nn, front), v.world);
    return vec4f(col, 1.0);
  }
