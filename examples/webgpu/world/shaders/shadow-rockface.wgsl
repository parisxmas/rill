  // The tunnels, into the shadow map.
  //
  // Without this the inside of a hillside is the one thing in the valley the
  // sun cannot miss: every other surface is tested against the depth the
  // shadow pass wrote, and faces that were never in that pass come back lit
  // from any angle. A mine mouth at dusk came out as a bright warm scar on a
  // hillside that was itself in shadow — the geometry was right to the
  // centimetre and the light was nonsense.
  //
  // The corners are built the same way the drawn pass builds them, from the
  // direction and the height in the instance, because a shadow caster that
  // is not the same shape as the thing it stands for is worse than none.

  const SRF_Q = 0.3;

  const SRF_UV = array<vec2f, 6>(
    vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    vec2f(0.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0),
  );

  fn srfAxes(dir: u32) -> mat2x3f {
    switch dir {
      case 0u:  { return mat2x3f(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 1.0, 0.0)); }
      case 1u:  { return mat2x3f(vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)); }
      case 2u:  { return mat2x3f(vec3f(0.0, 0.0, 1.0), vec3f(1.0, 0.0, 0.0)); }
      case 3u:  { return mat2x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 0.0, 1.0)); }
      case 4u:  { return mat2x3f(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0)); }
      default:  { return mat2x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)); }
    }
  }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) lo: vec4f,
    @location(1) col: vec4f,
    @location(2) nrm: vec4f,
    @location(3) hs: vec4f,
  ) -> @builtin(position) vec4f {
    let uv = SRF_UV[vi % 6u];
    let dir = u32(lo.w + 0.5);
    if (dir == 6u) {
      let gy = mix(mix(hs.x, hs.y, uv.x), mix(hs.z, hs.w, uv.x), uv.y);
      return u.lvp * vec4f(lo.x + uv.x * SRF_Q, gy, lo.z + uv.y * SRF_Q, 1.0);
    }
    let ax = srfAxes(dir);
    let p = lo.xyz + ax[0] * (uv.x * SRF_Q) + ax[1] * (uv.y * col.w);
    return u.lvp * vec4f(p, 1.0);
  }
