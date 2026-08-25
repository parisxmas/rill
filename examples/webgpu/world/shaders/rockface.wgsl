  // The inside of the digging.
  //
  // A terrain mesh is a lattice of heights, and a lattice of heights has no
  // word for a roof: the moment somebody cuts into a wall and leaves the rock
  // above it standing, there is a surface in this valley with stone over it
  // and air under it that no height anywhere can describe. So the tunnels are
  // drawn out of quads instead — one for every face of rock with air against
  // it — and the terrain blanks itself where they take over.
  //
  // Nothing is in the buffer but the low corner of each face, which way it
  // points, and its colour. The corners come out of the direction: six cases,
  // and each of them is the same unit square laid on a different pair of axes.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }

  const RF_Q = 0.3;

  // Four corners of a unit square, twice round so a triangle strip of six
  // vertices comes out wound the same way on every face.
  const RF_UV = array<vec2f, 6>(
    vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
    vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
  );

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) lo: vec4f,      // xyz the low corner, w the direction
    @location(1) col: vec4f,     // rgb the colour
  ) -> VOut {
    let uv = RF_UV[vi % 6u] * RF_Q;
    let dir = u32(lo.w + 0.5);
    var p = lo.xyz;
    var n = vec3f(0.0);
    // Wound so the front face looks the way the normal does. Two of the six
    // swap u and v to keep that true without a second table.
    if (dir == 0u)      { p += vec3f(0.0, uv.y, uv.x);  n = vec3f(-1.0, 0.0, 0.0); }
    else if (dir == 1u) { p += vec3f(0.0, uv.y, RF_Q - uv.x); n = vec3f(1.0, 0.0, 0.0); }
    else if (dir == 2u) { p += vec3f(uv.x, 0.0, uv.y);  n = vec3f(0.0, 1.0, 0.0); }
    else if (dir == 3u) { p += vec3f(uv.y, 0.0, uv.x);  n = vec3f(0.0, -1.0, 0.0); }
    else if (dir == 4u) { p += vec3f(RF_Q - uv.x, uv.y, 0.0); n = vec3f(0.0, 0.0, -1.0); }
    else                { p += vec3f(uv.x, uv.y, 0.0);  n = vec3f(0.0, 0.0, 1.0); }

    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = n;
    o.color = col.rgb;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let nn = normalize(v.normal);
    // A tunnel is lit by what reaches it, which is very little. The faces are
    // lit the way everything else is and then taken down towards nothing as
    // they get further under the hill — the depth is already in the colour
    // the engine wrote, so this only has to keep the sun off the ceiling.
    let col = lit(v.color, select(-nn, nn, front), v.world);
    return vec4f(col, 1.0);
  }
