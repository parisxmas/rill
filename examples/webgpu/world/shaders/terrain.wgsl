  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }
  @vertex fn vs(@location(0) pos: vec3f, @location(1) n: vec3f, @location(2) c: vec3f) -> VOut {
    var o: VOut;
    o.clip = u.vp * vec4f(pos, 1.0);
    o.world = pos; o.normal = n; o.color = c;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    // Two tilings of the meadow, one four times coarser, added rather than
    // chosen: a single tile at this scale repeats visibly across a valley,
    // and beating it against a slower copy of itself breaks the grid up for
    // the cost of one more sample.
    let g0 = textureSample(groundTex, barkSamp, v.world.xz * 0.32).rgb;
    let g1 = textureSample(groundTex, barkSamp, v.world.xz * 0.08).rgb;
    let g = g0 * 0.65 + g1 * 0.35;
    // Only the green faces are meadow. Sand, cliff and snow keep the flat
    // shading they were given — the valley reads as painted, and grass is the
    // one surface a walking camera is close enough to want detail from.
    let meadow = v.color.g > v.color.r + 0.03 && v.color.g > v.color.b + 0.03;
    let lum = dot(g, vec3f(0.34, 0.5, 0.16));
    // The photograph is a meadow in daylight and this valley is not; taking it
    // whole would drag the ground brown. So it is split: its brightness drives
    // the variation, its hue is folded in only a third of the way, and the
    // face's own colour still decides what green the valley is.
    let hue = g / max(lum, 0.001);
    let grassy = v.color * (0.15 + 2.1 * lum) * mix(vec3f(1.0), hue, 0.32);
    let albedo = select(v.color, grassy, meadow);
    return vec4f(lit(albedo, normalize(v.normal), v.world), 1.0);
  }
