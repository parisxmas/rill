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

  // The four corners of a face, wound so the front of it looks the way its
  // normal does. Getting this wrong is invisible in the buffer and total on
  // the screen: with the winding reversed every top face pointed at the
  // ground, back-face culling threw all of them away, and the terrain cells
  // that had stepped aside for them showed nothing at all — a hillside with a
  // ragged black hole in it where the tunnel mouth should have been.
  const RF_UV = array<vec2f, 6>(
    vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
    vec2f(0.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0),
  );

  // Each direction as a pair of axes whose cross product is the normal, so one
  // winding serves all six. The engine always writes the low corner of the
  // face, which is the corner these two run from.
  fn rfAxes(dir: u32) -> mat2x3f {
    switch dir {
      case 0u:  { return mat2x3f(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 1.0, 0.0)); }  // -x
      case 1u:  { return mat2x3f(vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)); }  // +x
      case 2u:  { return mat2x3f(vec3f(0.0, 0.0, 1.0), vec3f(1.0, 0.0, 0.0)); }  // +y
      case 3u:  { return mat2x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 0.0, 1.0)); }  // -y
      case 4u:  { return mat2x3f(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0)); }  // -z
      default:  { return mat2x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)); }  // +z
    }
  }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) lo: vec4f,      // xyz the low corner, w the direction
    @location(1) col: vec4f,     // rgb the colour, w how tall the face is
    @location(2) nrm: vec4f,     // a normal of its own, or nought for none
    @location(3) hs: vec4f,      // for a ground quad, its four corner heights
  ) -> VOut {
    let uv = RF_UV[vi % 6u];
    let dir = u32(lo.w + 0.5);
    // Six is not a direction, it is a piece of hillside: a quad lying across
    // x and z whose four corners carry the heights the terrain lattice would
    // have given them. Neighbouring pieces share those corners exactly, so
    // the surface runs on unbroken and no risers stand between them — which
    // is the difference between a hole in a hill and a staircase glued to it.
    if (dir == 6u) {
      let gx = lo.x + uv.x * RF_Q;
      let gz = lo.z + uv.y * RF_Q;
      let gy = mix(mix(hs.x, hs.y, uv.x), mix(hs.z, hs.w, uv.x), uv.y);
      var g: VOut;
      g.clip = u.vp * vec4f(gx, gy, gz, 1.0);
      g.world = vec3f(gx, gy, gz);
      g.normal = nrm.xyz;
      g.color = col.rgb;
      return g;
    }
    let ax = rfAxes(dir);
    let t1 = ax[0];
    let t2 = ax[1];
    // A stone wide, and as tall as the engine says. Usually that is a stone
    // too; at the top of a standing column it is whatever reaches the ground
    // the terrain mesh draws beside it, because a fixed height leaves a slot
    // of daylight round the rim of every tunnel mouth.
    let p = lo.xyz + t1 * (uv.x * RF_Q) + t2 * (uv.y * col.w);
    // The inside of a tunnel lights by the face it is; the top of an
    // untouched column lights by the hillside it belongs to, or a dug patch
    // reads as a bright lump stuck on a dark slope.
    let axis = cross(t1, t2);
    let n = select(axis, nrm.xyz, dot(nrm.xyz, nrm.xyz) > 0.001);

    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = n;
    o.color = col.rgb;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let nn = normalize(v.normal);
    let col = lit(v.color, select(-nn, nn, front), v.world);
    // A little of the rock's own colour underneath whatever the sun does.
    // Without it a mouth in a hillside is not a dark opening, it is a hole
    // cut out of the picture: `lit` has nothing to give a ceiling, so every
    // face under the hill came out at nought and the entrance read as a
    // rectangle of pure black rather than as somewhere you could walk into.
    return vec4f(max(col, v.color * 0.22), 1.0);
  }
