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
    // Rock, projected two ways and chosen between by how steep the ground is.
    // Flat x/z alone would smear the stone into vertical streaks down every
    // cliff, which is the one place the texture is meant to be doing work.
    let n0 = normalize(v.normal);
    let rFlat = textureSample(rockTex, barkSamp, v.world.xz * 0.16).rgb;
    let rWall = textureSample(rockTex, barkSamp, vec2f(v.world.x + v.world.z, v.world.y) * 0.13).rgb;
    let rock = mix(rWall, rFlat, smoothstep(0.35, 0.80, abs(n0.y)));
    // The beach. Two tilings again, and finer than the meadow's — a shore is
    // the one surface here you are always standing right on top of.
    let sand = textureSample(sandTex, barkSamp, v.world.xz * 0.55).rgb * 0.6
             + textureSample(sandTex, barkSamp, v.world.xz * 0.14).rgb * 0.4;

    // How much of each band this pixel is, read back out of the colour Rill
    // painted. Weights rather than a test: the bands themselves now fade into
    // one another, and a texture that switched on at a threshold would put a
    // hard edge back in the middle of a soft gradient.
    //
    // Sand is the one band with red over blue; of what is left, blue over
    // green is stone and the rest is meadow. The three sum to one, so a face
    // halfway between meadow and sand is fully both rather than half of
    // neither — halfway between was where the texture used to disappear.
    let wSand   = smoothstep(0.05, 0.16, v.color.r - v.color.b);
    let wStone  = smoothstep(-0.02, 0.06, v.color.b - v.color.g) * (1.0 - wSand);
    let wMeadow = 1.0 - wSand - wStone;

    // Each photograph is daylight and this valley is not; taking one whole
    // would drag its band toward the photograph's own colour. So they are
    // split: brightness drives the variation, hue is folded in a third of the
    // way, and the face's own colour still decides what the valley looks like.
    let LUM = vec3f(0.34, 0.5, 0.16);
    let lum = dot(g, LUM);
    let rlum = dot(rock, LUM);
    let slum = dot(sand, LUM);
    let grassy = v.color * (0.15 + 2.1 * lum) * mix(vec3f(1.0), g / max(lum, 0.001), 0.32);
    let stony = v.color * (0.30 + 1.5 * rlum) * mix(vec3f(1.0), rock / max(rlum, 0.001), 0.30);
    let sandy = v.color * (0.35 + 1.4 * slum) * mix(vec3f(1.0), sand / max(slum, 0.001), 0.30);
    let albedo = sandy * wSand + grassy * wMeadow + stony * wStone;

    // The same brightness, read a second time as a height. Rock gets most of
    // it — cracks and ledges are what makes a cliff read as rock rather than
    // as a painted slope — and grass only a little, because blades of grass at
    // this distance are texture, not relief.
    let h = lum * wMeadow + rlum * wStone + slum * wSand;
    let strength = 0.12 * wMeadow + 0.55 * wStone + 0.25 * wSand;
    return vec4f(lit(albedo, bumped(n0, v.world, h, strength), v.world), 1.0);
  }
