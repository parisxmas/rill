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
    @location(3) stone: f32,
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

  // Each direction as a pair of axes: the first is the width the face runs
  // across, the second is what its height is measured along. The engine always
  // writes the low corner of the face, which is the corner these two run from.
  //
  // The second axis has to be *up* for all four walls, and it was not. `+x`
  // used (y, z) and `-z` used (y, x) — chosen because their cross product came
  // out pointing the right way — so on those two faces the engine's height was
  // laid along the ground and a flat three tenths was stood up in the air.
  //
  // Which is invisible for as long as a face is a stone tall, because a
  // three-tenths square transposed is the same square. It is exactly the *cap*
  // faces that are not: the topmost face of a standing column is trimmed to
  // reach the ground the terrain draws beside it, and a trimmed face on those
  // two sides came out `tall` wide instead of `tall` high — short by
  // `0.3 - tall` along the rim, and standing that much proud of the hillside
  // above it. Every hole in this valley had two of its four sides open, and
  // what you could see through the slot was the lake.
  //
  // The cross product is answered by reversing the run instead: see `rfFlip`.
  fn rfAxes(dir: u32) -> mat2x3f {
    switch dir {
      case 0u:  { return mat2x3f(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 1.0, 0.0)); }  // -x
      case 1u:  { return mat2x3f(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 1.0, 0.0)); }  // +x
      case 2u:  { return mat2x3f(vec3f(0.0, 0.0, 1.0), vec3f(1.0, 0.0, 0.0)); }  // +y
      case 3u:  { return mat2x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 0.0, 1.0)); }  // -y
      case 4u:  { return mat2x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)); }  // -z
      default:  { return mat2x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)); }  // +z
    }
  }

  // With the height on y for every wall, two of the four now cross the wrong
  // way. Their run is reversed and their normal negated, which is the same
  // quad seen from the same side — and is what the axes used to be bent to
  // avoid, at the cost above.
  fn rfFlip(dir: u32) -> bool { return dir == 1u || dir == 4u; }

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
      // And wound the other way round from the six that *are* directions.
      // Running the corners x-then-z traces clockwise as seen from above, so
      // the rasteriser called every one of these quads a back face: culling
      // threw them all away, and a terrain cell that had stepped aside for the
      // voxels was left drawn by nobody. One stone of digging — three tenths
      // of a metre — took a whole metre and six of hillside out of the
      // picture, and what showed through the gap was the lake.
      //
      // Reversing the run answers both halves of it, because `front_facing`
      // is also what the fragment flips the shading normal on: with culling
      // merely turned off these quads still came back lit from underneath,
      // which is a cell-sized dark patch instead of a cell-sized hole.
      //
      // The six that are directions were wound right — see the note over
      // RF_UV, where the same mistake had already been paid for once. This
      // seventh case was added afterwards and did not inherit the lesson.
      let uv6 = RF_UV[5u - (vi % 6u)];
      // And not a whole stone wide, necessarily: a piece of hillside is cut to
      // the terrain cell that handed it over, because a column at a cell's edge
      // hangs over into the next one and the next one may still be drawing its
      // own ground. `col.w` carries the width and `nrm.w` the depth — a flat
      // quad has no height to put in the one, and its stone is nought by
      // definition, which is why the other is free and why this says so here
      // rather than reading it.
      let gx = lo.x + uv6.x * col.w;
      let gz = lo.z + uv6.y * nrm.w;
      let gy = mix(mix(hs.x, hs.y, uv6.x), mix(hs.z, hs.w, uv6.x), uv6.y);
      var g: VOut;
      g.clip = u.vp * vec4f(gx, gy, gz, 1.0);
      g.world = vec3f(gx, gy, gz);
      g.normal = nrm.xyz;
      g.color = col.rgb;
      g.stone = 0.0;
      return g;
    }
    let flip = rfFlip(dir);
    let uvw = RF_UV[select(vi % 6u, 5u - (vi % 6u), flip)];
    let ax = rfAxes(dir);
    let t1 = ax[0];
    let t2 = ax[1];
    // A stone wide, and as tall as the engine says — where "as tall" is two
    // numbers and not one: `col.w` where the run starts and `hs.x` where it
    // ends. Everything that is a stone tall from end to end gets the same
    // value in both. A cap face is not: it is trimmed to reach the ground the
    // terrain draws beside it, and that ground is a ramp between two corner
    // heights. One number for it was a flat line meeting a slope, and the
    // triangle left between them was open — a slot of daylight round the rim
    // of every hole, measured at five centimetres on gentle ground and more
    // than that on the steep ground the digging happens on.
    let hgt = mix(col.w, hs.x, uvw.x);
    let p = lo.xyz + t1 * (uvw.x * RF_Q) + t2 * (uvw.y * hgt);
    // The inside of a tunnel lights by the face it is; the top of an
    // untouched column lights by the hillside it belongs to, or a dug patch
    // reads as a bright lump stuck on a dark slope.
    let axis = select(cross(t1, t2), -cross(t1, t2), flip);
    let n = select(axis, nrm.xyz, dot(nrm.xyz, nrm.xyz) > 0.001);

    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = n;
    o.color = col.rgb;
    o.stone = nrm.w;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let nn = normalize(select(-v.normal, v.normal, front));

    // The same photographs the hillside samples, read the same way, because a
    // dug face is the same rock — and a face without them was a smooth plate
    // laid on a grainy slope, which the eye finds long before it finds any
    // geometry. `rockGrain` is the terrain's own, projection and all.
    let grain = rockGrain(v.world, nn);
    let rock = grain.rgb;
    let g0 = textureSample(groundTex, barkSamp, v.world.xz * 0.32).rgb;
    let g1 = textureSample(groundTex, barkSamp, v.world.xz * 0.08).rgb;
    let g = g0 * 0.65 + g1 * 0.35;
    let sand = textureSample(sandTex, barkSamp, v.world.xz * 0.55).rgb * 0.6
             + textureSample(sandTex, barkSamp, v.world.xz * 0.14).rgb * 0.4;

    // Which band this is, read out of the colour the engine painted, exactly
    // as the terrain reads it — except that rock somebody opened is stone
    // whatever band it was cut out of. Broken stone is a warm grey, and a warm
    // grey read as a band comes out a third sand.
    let wSand0 = smoothstep(0.05, 0.16, v.color.r - v.color.b);
    let wStone0 = smoothstep(-0.02, 0.06, v.color.b - v.color.g) * (1.0 - wSand0);
    let wSand = wSand0 * (1.0 - v.stone);
    let wStone = mix(wStone0, 1.0, v.stone);
    let wMeadow = max(1.0 - wSand - wStone, 0.0);

    let LUM = vec3f(0.34, 0.5, 0.16);
    let lum = dot(g, LUM);
    let rlum = dot(rock, LUM);
    let slum = dot(sand, LUM);
    let grassy = v.color * (0.15 + 2.1 * lum) * mix(vec3f(1.0), g / max(lum, 0.001), 0.32);
    let stony = v.color * (0.30 + 1.5 * rlum) * mix(vec3f(1.0), rock / max(rlum, 0.001), 0.30);
    let sandy = v.color * (0.35 + 1.4 * slum) * mix(vec3f(1.0), sand / max(slum, 0.001), 0.30);
    let albedo = sandy * wSand + grassy * wMeadow + stony * wStone;

    // And the same relief on the hillside a cap face stands for — but none at
    // all on rock somebody opened, which is what `v.stone` turns off here.
    //
    // `bumped` recovers a gradient from the screen-space derivatives of the
    // height and of the position, and divides by the area of the pixel quad
    // measured on the surface. That is sound on the terrain, whose triangles
    // are metres across and whose lighting is a smooth function of the slope.
    // On the inside of a shaft it is neither. The faces are three tenths of a
    // metre, and the lighting there is a knife edge: nothing casts a shadow
    // over a hole — the terrain cell above it is blanked, so the hillside that
    // would have shaded it draws nothing into the shadow map, and the dug
    // faces have their own casting turned off in host.js — so the whole
    // inside of a shaft is in full sun as far as the map is concerned, and a
    // wall is dark only because its true normal faces away from it. Give that
    // normal a shove of a few tens of degrees and the face lights up. Per
    // face, in whatever direction the local gradient happens to point: a
    // chequerboard of lit and unlit stones.
    //
    // Bisected rather than guessed. Returning the raw stone sample: smooth.
    // The albedo: smooth. `lit` with the face's own normal: smooth, and dark
    // in the way a shaft should be. `bumped` returning its argument
    // untouched: smooth. The pattern arrives with this one call. Clamping the
    // gradient — at forty-five degrees, then at seventeen — dimmed it and did
    // not remove it, because at these magnifications the quotient simply sits
    // at whatever the clamp is.
    //
    // The same word is in host.js, over the line that stops these faces
    // casting: somebody met this chequerboard from the other side, took the
    // casters out of the shadow map, and the pattern stayed — because the
    // casters were never what was driving it.
    let h = lum * wMeadow + grain.w * wStone + slum * wSand;
    let strength = (0.12 * wMeadow + 0.55 * wStone + 0.25 * wSand) * mix(1.0, 0.25, v.stone);
    let col = lit(albedo, bumped(nn, v.world, h, strength), v.world);

    // The floor a shaft is never allowed to fall below. `lit` has a sky term,
    // but a shaft sees almost none of the sky and its walls face each other:
    // against a hillside in full sun the inside came out at nought, and a hole
    // that renders at nought is not dark, it is absent — a rectangle cut out
    // of the picture.
    //
    // What this floor must not be is a copy of the albedo, which is what it
    // was, and which is where the brown came from. The arithmetic is worth
    // writing down, because the colour was nobody's choice: a wall of a shaft
    // gets (0.059, 0.063, 0.100) out of `lit` — dim, and *cold*, which is
    // right — and the old floor handed back 0.38 of the albedo, which is
    // (0.144, 0.141, 0.139). That is larger in every channel, so the max threw
    // the sky away and drew the albedo instead. And the albedo carries two
    // things the light was supposed to have taken out of it: the stone
    // photograph's own warmth, and a band formula that swings two and a half
    // times in brightness from one texel to the next. Flat and brown and
    // roaring with speckle, at the exact hour the hillside a metre away is
    // pale and quiet.
    //
    // So it is a light now rather than a copy, and it is added rather than
    // taken the maximum of. A floor that wins is a floor that flattens: it beat
    // the sun term and the relief along with the colour, and a shaft drawn at
    // its floor everywhere is an evenly painted box. Bounced light adds.
    //
    // Most of the colour swing is pulled out of it — what one wall of a shaft
    // sees is the other wall, and a surface lit by a surface is soft — and what
    // is left is tinted the way `lit`'s own sky term is tinted, because the
    // strip of sky over the hole is where the little light down there comes
    // from.
    //
    // Only on rock somebody opened. A cap face is hillside and sits flush
    // against the terrain mesh, which has no such term: give it one and the lip
    // of every hole gets a bright rim the width of one quad.
    //
    // And only while there is daylight to bounce, which is the correction this
    // term needed most and the one it is easiest to leave out — the valley has
    // an hour of the day and a constant does not. The numbers say it plainly:
    // at night `lit` hands flat ground albedo times (0.040, 0.048, 0.082),
    // which is about (0.016, 0.019, 0.033), and an unscaled fill of (0.094,
    // 0.101, 0.129) is five times that *added on top*. Every dug face came out
    // a pale blue-grey panel lying in a black hillside — brighter than the
    // world, which is not what the inside of a hole is. The old floor this
    // replaced was nine times the night ground and had the same fault.
    //
    // Bounced light is light. At night there is none, the term goes to nought,
    // and a hole at night is darker than the ground around it, which is the
    // whole of what a hole at night is.
    let fill = mix(albedo, vec3f(dot(albedo, LUM)), 0.55) * vec3f(0.28, 0.27, 0.25);
    return vec4f(col + fill * v.stone * dayness(), 1.0);
  }
