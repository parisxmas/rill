  // The boulders lying about the stony slopes.
  //
  // Poly Haven's `rock_07` and `boulder_01`, CC0, photoscanned — and then cut
  // down by `tools/rock_obj.py` from fifteen and sixty-six thousand triangles
  // to six hundred and a thousand, because ninety of either at scan weight is
  // more geometry than everything else in this valley put together.
  //
  // They wear no photograph of their own. The stone is `rockAt`, which is the
  // terrain's own rock projected on whichever plane the face lies nearest —
  // so a boulder resting against a cliff is made of the cliff, and a valley
  // that changes its rock changes all of it at once. It also costs nothing:
  // there is no seventeenth texture binding to be had, and a scan's own
  // colour would have wanted one.
  //
  // One shader, two pipelines, two meshes: the quantization box comes in as a
  // pipeline constant, which is what lets both meshes share this file.

  override boxLoX: f32 = 0.0;
  override boxLoY: f32 = 0.0;
  override boxLoZ: f32 = 0.0;
  override boxSpX: f32 = 1.0;
  override boxSpY: f32 = 1.0;
  override boxSpZ: f32 = 1.0;
  // Which of the two this pipeline draws.
  override wantSecond: f32 = 0.0;

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) ground: vec3f,
    @location(3) shade: f32,
  }

  // One angle, three numbers. The rotation a stone was placed at is a hash to
  // begin with, so its own digits are as good a hash as any — and taking the
  // shape out of the instance is what freed those three slots to carry the
  // colour of the ground, which is where every other instanced thing in this
  // valley keeps its colour.
  fn spin(rot: f32, k: f32) -> f32 {
    return fract(sin(rot * 12.9898 + k * 78.233) * 43758.5453);
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    var o: VOut;
    let second = select(0.0, 1.0, spin(rot, 4.0) < 0.5);
    if (second != wantSecond) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      o.world = vec3f(0.0);
      o.normal = vec3f(0.0, 1.0, 0.0);
      o.ground = vec3f(0.5);
      o.shade = 1.0;
      return o;
    }

    // Out of the box it was quantized into, and stood on its own base: the
    // two meshes were scanned lying at different heights in their own space
    // and the world says where the ground is, not where the scan's origin
    // was.
    let lo = vec3f(boxLoX, boxLoY, boxLoZ);
    let sp = vec3f(boxSpX, boxSpY, boxSpZ);
    var m = lo + qpos.xyz * sp;
    m.y = m.y - boxLoY;

    // No two alike out of one mesh: each stone is squashed on its own axes
    // before it is turned, so the silhouette changes rather than only the
    // size. The normal takes the inverse of the same squash, or a stone
    // flattened by a third would be lit as though it were still round.
    let sq = vec3f(0.78 + 0.50 * spin(rot, 1.0),
                   0.72 + 0.42 * spin(rot, 2.0),
                   0.82 + 0.34 * spin(rot, 3.0));
    m = m * sq;
    var n = qn.xyz / sq;

    let c = cos(rot);
    let s = sin(rot);
    let p = vec3f(m.x * c - m.z * s, m.y, m.x * s + m.z * c) * scale + ipos;
    n = normalize(vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c));

    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = n;
    o.ground = icol;
    // A stone is not quite the same as the one beside it. Half a stop either
    // way, off the same hash that decided its shape.
    o.shade = 0.82 + 0.30 * spin(rot, 5.0);
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let n = normalize(v.normal);
    // The same stone the hillside is made of, at the same two rates —
    // `rockGrain`, which the terrain and the walls of a dug tunnel both read
    // out of, so a boulder resting against a cliff is the same rock by
    // construction rather than by two shaders agreeing about a number.
    let grain = rockGrain(v.world, n);

    // And the same treatment, which is the part that was missing twice over.
    //
    // The photograph is brown. It is a daylight photograph of a warm rock and
    // this valley is neither, so the terrain never uses it whole: a band
    // colour decides what the ground is, the photograph's *brightness*
    // supplies the variation, and its hue is folded in a third of the way.
    // Drawn raw — which is what these boulders did at first — a rock comes
    // out the photograph's own brown on a hillside that is not brown.
    //
    // Then the band was a constant, the stone band, on the reasoning that a
    // boulder only ever lies on stony ground. Two things were wrong with
    // that. A stone can lie in a meadow, which is now where half of them are;
    // and the high slopes are not the stone band at all — they are painted
    // with *snow* and textured with rock, which comes out a warm pale tan, so
    // a grey-blue boulder on one read as a hole cut in the hillside. That is
    // the screenshot this paragraph was written from.
    //
    // So the ground's own band arrives with the instance, and the stone is
    // half of it: a boulder is the colour of the ground it came out of,
    // desaturated toward its own grey. That keeps it in the same light as
    // whatever it is lying on — tan on the tops, dark on the meadow — while
    // still reading as stone rather than as a lump of turf.
    let rlum = grain.w;
    let glum = dot(v.ground, vec3f(0.34, 0.5, 0.16));
    let base = mix(v.ground, vec3f(glum), 0.5);
    let stone = base * (0.30 + 1.5 * rlum)
              * mix(vec3f(1.0), grain.rgb / max(rlum, 0.001), 0.30);

    let col = lit(stone * v.shade, n, v.world);
    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
