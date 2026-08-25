  @group(0) @binding(1) var shadowMap: texture_depth_2d;
  @group(0) @binding(2) var shadowSamp: sampler_comparison;

  // Pine bark. Unlike the needle atlas above it, this one is not in
  // `uniforms.wgsl`: the shadow pass has no use for it, because a trunk is
  // opaque and its silhouette is all the shadow map needs.
  @group(0) @binding(5) var barkTex: texture_2d<f32>;
  @group(0) @binding(6) var barkSamp: sampler;
  // The meadow. Tiled from world x/z, so it needs the same repeating sampler
  // the bark uses and the terrain needs no texture coordinate of its own.
  @group(0) @binding(7) var groundTex: texture_2d<f32>;
  // Rock, for both the dark bands and the pale peaks — they are the same
  // stone under different light and different snow, so they are one texture
  // with the band's own colour still deciding which.
  @group(0) @binding(8) var rockTex: texture_2d<f32>;
  // Wet shore gravel, for the strip the water reaches.
  @group(0) @binding(9) var sandTex: texture_2d<f32>;
  // Four plants in a 2x2 atlas: two tufts of grass, a fern frond, a nettle.
  // Shares the needle atlas's sampler, which clamps rather than repeats.
  @group(0) @binding(10) var plants: texture_2d<f32>;
  // The rifle's two materials side by side: accessories left, rifle right.
  @group(0) @binding(11) var rifleTex: texture_2d<f32>;
  // The stone inside a fireball. Dark and pitted; the glow in its cracks is
  // worked out from the texture rather than painted into it.
  @group(0) @binding(12) var meteorTex: texture_2d<f32>;
  // The valley itself, rendered once from a point in the middle of it onto the
  // six faces of a cube. Only the still parts are in it — ground, trees,
  // crystals, water — because it is baked at load and never redrawn: what is
  // in a reflection at a hundred and twenty-eight pixels a face is colour and
  // rough shape, and neither of those changes when a pine sways.
  //
  // The sky is deliberately not in it. Drawing sky into a cube face would need
  // a version of the sky shader that works at ninety degrees and straight up,
  // and there is no reason to store what can be evaluated: the faces are
  // cleared transparent instead, and anything reading this blends its own
  // `skyColor` in through the alpha. Cheaper, sharper, and the sun disc stays
  // a disc instead of becoming four pixels.
  @group(0) @binding(13) var envCube: texture_cube<f32>;
  // Jupiter, equirectangular — Cassini's own map, wrapped onto the sphere by
  // latitude and longitude rather than painted on as bands. Everything that is
  // actually up there is in it: the belts and zones with the right widths in
  // the right places, the festoons hanging off the equatorial band, the white
  // ovals, and a Great Red Spot that is where the Great Red Spot is.
  @group(0) @binding(14) var jupiterTex: texture_2d<f32>;
  // Saturn's globe, the same way. Its rings stay drawn rather than sampled:
  // they are an annulus seen at a tilt, and the shape is the whole of what
  // reads at this size — a map of them would be a hundred pixels of gradient
  // doing what two smoothsteps already do.
  @group(0) @binding(15) var saturnTex: texture_2d<f32>;
  // The four Galileans in a 2x2 atlas: Io and Europa across the top, Ganymede
  // and Callisto below. USGS and Voyager/Galileo mosaics — Io's sulphur
  // yellows, Europa's cracked ice, Ganymede's grooves, Callisto's craters, all
  // of it in about twenty pixels a moon, which is what they are worth from
  // here and is still the difference between four moons and four dots.
  @group(0) @binding(16) var moonsTex: texture_2d<f32>;
  // The shark's skin. Only ever sampled from inside the water, which is the
  // one place the animal is drawn at all.
  @group(0) @binding(17) var sharkTex: texture_2d<f32>;

  // What is lying on the ground and is not a chip of crystal. One texture, one
  // prop: there is a single thing in this valley you can pick up and it is an
  // ammunition box, so an atlas of props would be an atlas with one entry in
  // it.
  @group(0) @binding(18) var propTex: texture_2d<f32>;

  // The launcher and the rocket it fires, off one 512-pixel sheet — they came
  // out of one file and they are the same object a moment apart.
  @group(0) @binding(19) var rpgTex: texture_2d<f32>;

  // Bump without a normal map and without a tangent frame. The screen-space
  // derivatives of the height and of the surface position are between them
  // enough to recover the gradient in world space, which is the one thing a
  // tangent frame would have been for. (Mikkelsen, 2010.)
  fn bumped(n: vec3f, world: vec3f, h: f32, strength: f32) -> vec3f {
    let px = dpdx(world);
    let py = dpdy(world);
    let r1 = cross(py, n);
    let r2 = cross(n, px);
    let det = dot(px, r1);
    let s = select(-1.0, 1.0, det >= 0.0);
    let g = s * (dpdx(h) * r1 + dpdy(h) * r2);
    return normalize(max(abs(det), 0.00000001) * n - strength * g);
  }

  // No early return: textureSampleCompare demands uniform control flow, so
  // the samples always run and the out-of-map case is selected afterwards.
  //
  // The kernel is a 5x5 disc, and its step is a texel of whatever map this
  // tier actually allocated — 1024, 2048 or 4096 — asked for rather than
  // assumed. A fixed 1/2048 was half a texel on the small map, which is no
  // filter at all, and two texels on the large one, which samples the gaps and
  // leaves the staircase standing. Corners are dropped: a square kernel gives
  // a soft edge square corners, and at this radius you can see them.
  //
  // The whole disc is then turned by an angle hashed from the ground position.
  // Taps on a fixed grid put their own stairs back — every pixel crossing the
  // edge steps through the same 25 levels in the same places, which is what
  // the banding was. Turned per point, the levels land differently either side
  // of a footstep, and the edge dissolves into grain the eye reads as soft.
  fn shadow(world: vec3f, nl: f32) -> f32 {
    let sc = u.lvp * vec4f(world, 1.0);
    let uvz = vec3f(sc.xy * vec2f(0.5, -0.5) + 0.5, sc.z);
    let bias = clamp(0.0028 * (1.0 - nl) + 0.0009, 0.0009, 0.004);
    let step = 2.4 / f32(textureDimensions(shadowMap).x);
    let a = fract(sin(dot(floor(world.xz * 37.0), vec2f(127.1, 311.7))) * 43758.5453) * 6.2831853;
    let rot = vec2f(cos(a), sin(a));
    var acc = 0.0;
    var wsum = 0.0;
    for (var dy = -2; dy <= 2; dy++) {
      for (var dx = -2; dx <= 2; dx++) {
        let o = vec2f(f32(dx), f32(dy));
        let w = select(1.0, 0.0, dot(o, o) > 6.0);   // 21 taps of the 25
        let r = vec2f(o.x * rot.x - o.y * rot.y, o.x * rot.y + o.y * rot.x);
        acc += w * textureSampleCompare(shadowMap, shadowSamp, uvz.xy + r * step, uvz.z - bias);
        wsum += w;
      }
    }
    let inside = uvz.x >= 0.0 && uvz.x <= 1.0 && uvz.y >= 0.0 && uvz.y <= 1.0 && uvz.z <= 1.0;
    return select(1.0, acc / wsum, inside);
  }

  const SKY_ZENITH  = vec3f(0.043, 0.055, 0.16);
  const SKY_HORIZON = vec3f(0.23, 0.19, 0.28);
  const SUN_WARM    = vec3f(1.0, 0.62, 0.24);
  const SUN_COL     = vec3f(1.0, 0.85, 0.6);

  // ---- what else is up there -------------------------------------------
  //
  // The moon, Jupiter and Saturn, each a sphere lit by the same sun that is
  // lighting the valley. Not sprites: the phase on each one is the real dot
  // product of the sun's direction with a sphere normal recovered from the
  // disc, so the moon's terminator faces the sunset and moves if the sun does.
  // That is the one thing that gives a painted-on moon away, and it is a line
  // of arithmetic to get right.
  //
  // Bigger than life, all three — the real moon is half a degree across and
  // would be four pixels here. What is being drawn is the moon you remember
  // rather than the one a protractor finds, which is the usual bargain in a
  // sky, and the planets are scaled to keep the same ratio to it that makes
  // them read as further away.

  // How fast the world turns under the sky: one revolution in ninety seconds,
  // because this is a small planet and that is the only way the size of a world
  // shows from standing on it. Fast enough that a pine's shadow visibly travels
  // while you stand still, and that a whole day happens inside a visit. The same number lives in `host.js`, which swings the sun and the
  // shadow matrix by it — they have to agree, or the moon's terminator drifts
  // away from the sun that is supposed to be casting it.
  const SKY_SPIN = 0.0174532925;      // 2π / 360, and the host's must agree

  // The pole the whole sky turns about — the same one the sun runs round in
  // `host.js`, all but horizontal, so the sky wheels overhead the way it does
  // at the equator rather than sliding round the rim. Everything up there
  // shares it: that is what a planet's rotation is, and it is why the moon
  // rises and sets on the same slant the sun does rather than sliding round
  // the horizon on a rail of its own.
  const SKY_AXIS = vec3f(0.990268, 0.139173, 0.0);   // sin 82°, cos 82°

  // A direction, taken out of the world and into the sky's own frame — the one
  // the stars and the bodies are nailed to. Rodrigues about the tilted pole,
  // backwards, which is what turns "where is this in the world now" into
  // "which piece of the sky is this".
  fn skySpace(dir: vec3f, t: f32) -> vec3f {
    let a = -t * SKY_SPIN;
    let c = cos(a); let s = sin(a);
    let k = SKY_AXIS;
    return dir * c + cross(k, dir) * s + k * dot(k, dir) * (1.0 - c);
  }

  fn skyHash(p: vec2f) -> f32 { return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453); }

  // Four separate calls, and they have to stay separate. It is tempting to
  // project the cell once and reach the other three corners by adding the
  // constants the projection would have added — the arithmetic is the same, and
  // it turns four sines into one on a vector. It was tried, and the clouds came
  // apart into squares.
  //
  // The reason is the thing this hash is built on: `fract(sin(x) * 43758)` is
  // chaotic, so two values of `x` that differ in their last bit give unrelated
  // answers. A corner is shared by four cells, and interpolated noise is only
  // continuous because all four agree about it. Projecting once and adding
  // reaches that corner by a different route than the neighbouring cell does,
  // the two disagree in the last bit, and every cell becomes its own little
  // patch with a seam round it — which is exactly what a cell boundary looks
  // like when you can see it.
  //
  // The shared corner must therefore be computed the same way from either side:
  // as the projection of the corner itself. That is what `skyHash` does, and
  // why it is called four times.
  fn skyNoise(p: vec2f) -> f32 {
    let i = floor(p); let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(skyHash(i), skyHash(i + vec2f(1.0, 0.0)), u.x),
               mix(skyHash(i + vec2f(0.0, 1.0)), skyHash(i + vec2f(1.0, 1.0)), u.x), u.y);
  }

  // Where `dir` falls on a body sitting at `at`: xy across its face, z how far
  // round the sky it is from the body. Only meaningful near the body, which is
  // the only place it is asked.
  fn diskAt(dir: vec3f, at: vec3f) -> vec3f {
    let right = normalize(cross(vec3f(0.0, 1.0, 0.0), at));
    let up = cross(at, right);
    return vec3f(dot(dir, right), dot(dir, up), dot(dir, at));
  }

  // A light direction expressed in a body's own frame, so a phase can be
  // worked out in flat disc coordinates. Which light gets handed in is the
  // caller's business, and it differs by body: the planets are nailed to the
  // sky and want the sun brought into the sky's frame, while the moon is
  // nailed to the world and wants it as it is.
  fn sunInFrame(at: vec3f, L: vec3f) -> vec3f {
    let right = normalize(cross(vec3f(0.0, 1.0, 0.0), at));
    let up = cross(at, right);
    return normalize(vec3f(dot(L, right), dot(L, up), dot(L, at)));
  }

  // Our moon does not rise or set. It is locked to this world and hangs in the
  // same quarter of the sky for ever, which is a thing a close satellite can
  // actually do — and it changes everything about what it is for. A moon that
  // wheels past with the stars is scenery; one that is always in the same place
  // is a landmark you can navigate by, and it goes through its phases in front
  // of you as the sun comes round, because the sun is the only thing moving.
  const MOON_AT = vec3f(0.62, 0.46, -0.64);
  // What its light is worth on the ground. Real moonlight is four hundred
  // thousand times weaker than sunlight and would be nothing here; this is the
  // strength at which it does the job it is being asked to do, which is to give
  // the valley a direction after dark. Cold, because reflected sunlight off
  // grey rock is, and because a warm night light reads as a lamp.
  const MOON_COL = vec3f(0.42, 0.50, 0.72);

  // How much of the moon's face is lit, as seen from here: one at full, nothing
  // when it is between us and the sun. The sun moves and the moon does not, so
  // this is a real cycle — full at midnight when the sun is under the world
  // directly opposite it, and down to a rind at noon.
  fn moonPhase() -> f32 {
    let L = -normalize(u.sun.xyz);
    return clamp(0.5 + 0.5 * dot(-normalize(MOON_AT), L), 0.0, 1.0);
  }

  // The two planets on the sun's side of the sky, forty to seventy degrees off
  // it. (The moon is elsewhere and is declared above, because it is fixed to
  // this world rather than to the sky and everything about it follows from
  // that.)
  //
  // Not an arbitrary arrangement — it is what makes the lighting legible. The
  // phase was being computed correctly before this and still read as wrong,
  // because the bodies sat a hundred and twelve degrees from the sun: you
  // could see a moon lit from above, or you could turn round and see the sun,
  // and never both at once, so there was nothing on screen for the terminator
  // to agree with. Put them where a single look takes in the body and the sun
  // together and the same arithmetic reads as obviously right.
  //
  // Forty degrees is about the near limit: closer and the disc is nearly full
  // and there is no terminator to see. Past seventy it is a thin crescent and
  // the body stops being recognisable.
  //
  // High, the planets — one low over a ridge reads as a lamp on the far hill,
  // and it is only well clear of the horizon that anyone takes it for something
  // out beyond the sky. The moon stays lower, where a moon belongs.
  const JUP_AT  = vec3f(-0.42, 0.86, -0.29);   // 51°
  const SAT_AT  = vec3f(0.10, 0.80, 0.59);     // 42°
  // How big each one is drawn is no longer three numbers picked by eye: it is a
  // radius over a distance, which is the small-angle form of the angle a sphere
  // actually subtends. Change where a body is and its size follows.
  //
  // The radii are true, in units of the moon's — Jupiter is 40.2 moon-radii,
  // Saturn 33.5. The distances are not. The planets are pulled about sixteen
  // times nearer than they really are, because at their real remove each is a
  // single pixel and the point of drawing a thing is that it can be seen. What
  // the numbers do keep is the order and the ratio: Saturn is further out than
  // Jupiter and smaller in the sky than it, by the right amount.
  const MOON_RAD = 1.0;    const MOON_DIST = 20.8;
  // Jupiter is brought right in — two hundred moon-radii out, ten times nearer
  // than Saturn, which puts a disc four times the moon's width overhead. Wrong
  // by any almanac and the whole point of the picture: something that size in
  // the sky is not an object in the scene, it is a fact about where you are.
  // The size still comes out of the distance rather than being set, so it and
  // Saturn stay on the one scale.
  const JUP_RAD  = 40.2;   const JUP_DIST  = 201.0;
  // Saturn brought in behind Jupiter — four hundred and twenty out against
  // Jupiter's two hundred, so it comes to about half the width on screen. Near
  // enough to read the rings and the banding, far enough that the two of them
  // are plainly at different distances, which is the only thing the numbers
  // have to carry.
  const SAT_RAD  = 33.5;   const SAT_DIST  = 420.0;
  const MOON_R = MOON_RAD / MOON_DIST;
  const JUP_R  = JUP_RAD / JUP_DIST;
  const SAT_R  = SAT_RAD / SAT_DIST;

  // What the sun does to a body it is lighting: its own colour where the light
  // lands, and the cold of the sky it is hanging in where it does not.
  fn sunTint(lam: f32) -> vec3f {
    // The unlit side of a body in space is nearly black. There is no air on it
    // to scatter light round the limb and nothing nearby to bounce any back, so
    // what it gets is what the stars give it, which is almost nothing. It was
    // sitting at 0.13 here, which is a quarter-lit night side — enough to fill
    // the shadow in and leave every planet reading as a flat textured circle
    // with a slightly dim edge, rather than as a ball with a sun on one side.
    return mix(vec3f(0.030, 0.035, 0.068), SUN_COL * 1.30, lam);
  }

  // How much sunlight reaches something that far out, in Earth-distances.
  //
  // Inverse square, and then compressed hard. Jupiter gets a twenty-seventh of
  // what the moon does and Saturn a ninetieth; drawn anywhere near straight,
  // those are a brown smudge and nothing at all.
  //
  // The exponent was 0.35 first — about what a dark-adapted eye makes of a
  // ratio — and it was a mistake worth writing down, because the physics was
  // right and the picture got worse. Jupiter came out at a third of its old
  // brightness, and a third of the brightness of a banded disc is not a
  // dimmer banded disc, it is a grey oval: the contrast between the belts goes
  // with the level, and the belts were the whole reason to draw it.
  //
  // At 0.12 the ordering survives and the subject does too. The moon is
  // brightest, Jupiter is a sixth down from it and Saturn a sixth down again —
  // enough that the sky says which is further without any of them being
  // sacrificed to say it. The falloff here is doing a compositional job, not
  // a radiometric one, and pretending otherwise cost a good-looking planet.
  const AU_MOON = 1.00;      // ours, near enough the Earth's distance
  const AU_JUP  = 5.20;
  const AU_SAT  = 9.54;
  fn sunlightAt(au: f32) -> f32 { return 1.75 * pow(1.0 / (au * au), 0.12); }

  // Premultiplied: colour already scaled by coverage, so the caller adds it
  // over whatever the sky was without having to know what is in it.
  fn nightSky(dirWorld: vec3f, t: f32) -> vec4f {
    let dir = skySpace(dirWorld, t);

    // Is this pixel near any of them at all? One question, asked before any of
    // the three bodies is looked at, and it is the difference between a sky
    // that costs four milliseconds and one that costs half of one.
    //
    // Measured, because it was not what anyone would have guessed: with the
    // bodies removed entirely the sky pass went from 12.4 ms to 8.0, on a full
    // screen of sky at the top tier. The moon, Jupiter and Saturn together
    // cover something under a fiftieth of the sky — and the other forty-nine
    // fiftieths were paying for all three of them, every frame.
    //
    // Each body's own `if` was already there, so the *bodies* were not being
    // drawn; what was being paid was everything in front of those tests, three
    // times over, in a function the compiler flattens into one long stretch of
    // straight-line arithmetic. The way out is a single early return, so that
    // a warp with no lane near anything leaves at once.
    //
    // The bounds are the bodies' own. `diskAt` returns the cosine of the angle
    // to the body in `z`, so each test here is exactly the radius test further
    // down, written as the cosine it already is: 1.35 moon-radii, Jupiter plus
    // eleven and a half degrees to Jupiter's own limb, and Saturn out past
    // the rings.
    let nearMoon = dot(dirWorld, normalize(MOON_AT)) > 0.9975;
    let nearJup  = dot(dir, normalize(JUP_AT)) > 0.97;
    let nearSat  = dot(dir, normalize(SAT_AT)) > 0.975;
    if (!nearMoon && !nearJup && !nearSat) { return vec4f(0.0); }

    var out = vec4f(0.0);

    // ---- the moon
    {
      // In the world's own directions, not the sky's: this one does not turn
      // with the stars.
      let at = normalize(MOON_AT);
      let f = diskAt(dirWorld, at);
      let r = length(f.xy) / MOON_R;
      if (f.z > 0.0 && r < 1.35) {
        // The sphere, recovered: on a disc of unit radius the surface normal is
        // (x, y, ±sqrt(1 - x² - y²)) — and the sign is the whole thing. The
        // half of the sphere you can see is the half facing you, so its normal
        // runs back along the line of sight, against the frame's third axis,
        // which points away from the eye and out at the body. Written with a
        // plus it computes the phase of the far side instead, and the
        // terminator comes out mirrored through the middle of the disc: a
        // gibbous moon renders as a dark disc with a bright rim.
        let q = f.xy / MOON_R;
        let nrm = vec3f(q, -sqrt(max(1.0 - r * r, 0.0)));
        let lam = clamp(dot(nrm, sunInFrame(at, -normalize(u.sun.xyz))), 0.0, 1.0);
        // The maria: darker, flatter plains, and the reason a full moon has a
        // face rather than being a plain disc.
        let mare = smoothstep(0.42, 0.72, skyNoise(q * 2.6 + 11.0)
                                        + 0.4 * skyNoise(q * 6.1 + 3.0));
        var mc = mix(vec3f(0.86, 0.85, 0.80), vec3f(0.52, 0.53, 0.55), mare * 0.85);
        // Craters, and the dust the whole thing is under.
        mc *= 0.86 + 0.28 * skyNoise(q * 15.0 + 41.0);
        // No atmosphere means no soft terminator: the shadow line on the moon
        // is famously hard, and softening it is what makes one look like a
        // sticker. Only the very limb is eased, and that for the pixels.
        let day = smoothstep(0.0, 0.06, lam) * (0.25 + 0.75 * pow(lam, 0.6));
        let edge = 1.0 - smoothstep(0.985, 1.0, r);
        let a = edge;
        // Warm where the sun is on it, and very nearly out where it is not:
        // a body with no air has nothing to scatter light round its own limb,
        // which is why the dark side of a crescent moon is a hole in the sky
        // rather than a dim disc.
        out += vec4f(mc * mix(vec3f(0.045, 0.050, 0.090), SUN_COL * sunlightAt(AU_MOON), day) * a, a);
        // And the glow it puts on the air around it.
        let hz = clamp(1.35 - r, 0.0, 1.0);
        out += vec4f(vec3f(0.30, 0.32, 0.40) * (hz * hz * hz) * 0.30, 0.0);
      }
    }

    // ---- Jupiter: bands, and the spot
    //
    // The frame is built once here. It used to be shared with the four moons
    // that went round it; they are gone — see below — and it is Jupiter's
    // alone again.
    let jupAt = normalize(JUP_AT);
    let fJ = diskAt(dir, jupAt);
    let rJ = length(fJ.xy);
    {
      let at = jupAt;
      let f = fJ;
      let r = rJ / JUP_R;
      if (f.z > 0.0 && r < 1.0) {
        let q = f.xy / JUP_R;
        let nrm = vec3f(q, -sqrt(max(1.0 - r * r, 0.0)));   // the sign: see the moon, above
        // Lit by the same sun as everything below, and barely filled in — a
        // twentieth, which is only there so the limb of a gas giant does not go
        // to the hard black edge an airless moon's does.
        //
        // It was a quarter, and a quarter is what makes a sphere read as a
        // circle: Jupiter sits fifty degrees off the sun, so a third of its
        // face should be in shadow, and propping that third up to a quarter-lit
        // leaves a flat textured disc with a slightly dim side. The terminator
        // is the only thing in the picture saying the planet is round.
        let lam = mix(clamp(dot(nrm, sunInFrame(at, skySpace(-normalize(u.sun.xyz), t))), 0.0, 1.0), 1.0, 0.05);
        // The real map, wrapped on. The sphere normal recovered above is also
        // the point on the surface, so latitude and longitude come straight out
        // of it — and because the normal is right, the bands compress toward
        // the limb on their own. The sine bands that used to be here were drawn
        // in flat disc coordinates and stayed the same width all the way across,
        // which is what made a sphere look like a painted circle.
        //
        // And it turns. Jupiter's day is under ten hours, the shortest of any
        // planet, which is why it is visibly flattened at the poles; here it is
        // a turn every hundred seconds, fast enough that the spot crosses the
        // face while you watch and slow enough not to be a spinning ball.
        let lonV = atan2(nrm.x, -nrm.z) / 6.2831853 + t * (1.0 / 100.0);
        let latV = 0.5 - asin(clamp(nrm.y, -1.0, 1.0)) / 3.14159265;
        // Sampled at an explicit level rather than by derivative: this runs
        // inside a branch, and a gradient asked for under non-uniform control
        // flow is undefined. The level is stepped up hard toward the limb,
        // where half a hemisphere is crushed into the last few pixels — the
        // sixth power keeps the middle of the disc at full sharpness and only
        // blurs the sliver where the map is being sampled at fifty texels a
        // pixel and would otherwise crawl with aliasing.
        let lod = 6.5 * pow(r, 6.0);
        var jc = textureSampleLevel(jupiterTex, barkSamp, vec2f(lonV, latV), lod).rgb;
        // Limb darkening — a gas giant has no edge, it just goes away.
        let limb = pow(clamp(1.0 - r * r, 0.0, 1.0), 0.22);
        let a = 1.0 - smoothstep(0.965, 1.0, r);
        // The sun's own colour on the lit side, and cold and nearly out on the
        // other. Multiplying by a grey `lam` would only have dimmed it; what a
        // low sun actually does is make the lit limb warm while the shadowed
        // one keeps the blue of the sky it is sitting in.
        out += vec4f(jc * sunTint(lam) * limb * sunlightAt(AU_JUP) * a, a);
      }
    }

    // ---- the Galileans: not drawn
    //
    // Io, Europa, Ganymede and Callisto were here, on compressed but real
    // orbits, and they were taken out for what they cost rather than for how
    // they looked. Measured on a full screen of sky at the top tier, the loop
    // that placed them was between three and six milliseconds — about half of
    // everything the sky spent — and no guard would shift it: the branch that
    // was supposed to skip them is one the compiler flattens, so the work was
    // being done for every pixel and thrown away for all but a few hundred.
    // Four dots a few pixels across is not worth half a sky.
    //
    // What is left of them: `moons.png` is still in the bundle and still
    // bound, and the arithmetic is in the history of this file. Putting them
    // back means drawing them in their own small pass rather than inside the
    // one that covers the screen.


    // ---- Saturn: the disc, and the rings across it
    {
      let at = normalize(SAT_AT);
      let f = diskAt(dir, at);
      let q = f.xy / SAT_R;
      let r = length(q);
      if (f.z > 0.0 && r < 2.6) {
        let nrm = vec3f(q, -sqrt(max(1.0 - min(r * r, 1.0), 0.0)));
        let sd = sunInFrame(at, skySpace(-normalize(u.sun.xyz), t));
        let lam = mix(clamp(dot(nrm, sd), 0.0, 1.0), 1.0, 0.05);   // see Jupiter, above
        // The real globe, wrapped on by latitude and longitude and turning on
        // its own — Saturn's day is a shade over ten hours, near enough
        // Jupiter's, so it is given a similar spin here.
        let lonV = atan2(nrm.x, -nrm.z) / 6.2831853 + t * (1.0 / 115.0);
        let latV = 0.5 - asin(clamp(nrm.y, -1.0, 1.0)) / 3.14159265;
        let lod = 5.5 * pow(min(r, 1.0), 6.0);
        let sc = textureSampleLevel(saturnTex, barkSamp, vec2f(lonV, latV), lod).rgb;
        let limb = pow(clamp(1.0 - r * r, 0.0, 1.0), 0.22);
        let planet = 1.0 - smoothstep(0.965, 1.0, r);

        // The rings, seen at a tilt: a circle squashed along one axis is what
        // an inclined annulus projects to, so the ring radius is measured in a
        // stretched frame rather than the disc's.
        let tilt = 0.30;                       // how open they are, 0 edge-on
        let rq = vec2f(q.x, q.y / tilt);
        let rr = length(rq);
        // Two bright bands with Cassini's gap between them, and the whole thing
        // fading at both edges.
        var ring = smoothstep(1.20, 1.28, rr) * (1.0 - smoothstep(2.22, 2.35, rr));
        ring *= 1.0 - 0.75 * (1.0 - smoothstep(0.02, 0.05, abs(rr - 1.95)));
        ring *= 0.55 + 0.45 * skyNoise(vec2f(rr * 22.0, 0.5));
        // The rings take the sun from the side, so the arc turned toward it is
        // the bright one. Ice is a better reflector than the planet, which is
        // why in every photograph the rings are the brightest thing there.
        let rl = clamp(0.42 + 0.58 * dot(normalize(rq + vec2f(1e-4)),
                                         normalize(sd.xy + vec2f(1e-4))), 0.0, 1.0);
        let rc = vec3f(0.80, 0.74, 0.62) * sunTint(rl) * sunlightAt(AU_SAT) * 1.35;
        // The near half passes in front of the planet, the far half goes
        // behind it. Which is which is just the sign of y in ring space.
        let front = step(q.y, 0.0);
        // Behind first, then the planet, then the near arc over the top.
        let ab = ring * (1.0 - front) * (1.0 - planet) * 0.85;
        out += vec4f(rc * ab, ab);
        let ap = planet;
        out += vec4f(sc * sunTint(lam) * limb * sunlightAt(AU_SAT) * ap, ap);
        let af = ring * front * 0.85;
        out += vec4f(rc * af, af);
      }
    }

    return out;
  }

  // Night, for the sky itself. Not black: a clear sky with a moon in it is a
  // deep blue, and the horizon keeps a little of the day's warmth longest.
  const NIGHT_ZENITH  = vec3f(0.006, 0.010, 0.032);
  const NIGHT_HORIZON = vec3f(0.022, 0.026, 0.058);

  // ---- the clouds ------------------------------------------------------
  //
  // A layer of weather at three hundred metres, and it is a *place*, not a
  // backdrop: the field is sampled by where a ray actually crosses the layer,
  // so the clouds sit still over the valley while you walk under them, and the
  // same crossing computed from the sun's side puts their shadows on the
  // ground — one field asked two questions, which is the only reason the
  // shadow under a cloud is under that cloud.
  //
  // Everything here runs on the world's own clock, which the server hands to
  // every browser: two people standing together are under the same sky, and
  // the shadow that slides over one of them slides over both.
  const CLOUD_H     = 300.0;
  const CLOUD_SCALE = 0.0021;            // metres of layer to noise units
  // High-altitude drift, fixed on purpose. The gusts that work the pines and
  // the water are the ground's wind; tying the whole cloud deck to them would
  // swing the sky round every time the breeze turned.
  const CLOUD_DRIFT = vec2f(0.86, 0.40); // where the weather comes from
  // Twenty-two, not five and a half. At five the deck was honest — that is a
  // calm day's wind — and it read as a painted ceiling, because from the
  // ground a cloud three hundred metres up moving five metres a second is a
  // minute of staring before anything has visibly gone anywhere. Aloft blows
  // harder than the ground does anyway, and at twenty-two a cloud crosses its
  // own width in about twenty seconds, which is the slowest thing an eye
  // still catches unprompted. The shadows sweep the valley at the same pace,
  // because they are the same field.
  const CLOUD_WIND  = 22.0;              // and how fast, metres a second

  fn cloudField(p: vec2f) -> f32 {
    var v = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 5; i++) {
      v += a * skyNoise(q);
      q = q * 2.03 + vec2f(19.0, 47.0);
      a *= 0.55;
    }
    return v;
  }

  // The same field at three octaves, for the fragments that only want to know
  // whether they are standing in a shadow: the two missing octaves are ten
  // metres of curl on a shadow edge already softened by distance.
  fn cloudFieldLow(p: vec2f) -> f32 {
    return 0.5 * skyNoise(p)
         + 0.275 * skyNoise(p * 2.03 + vec2f(19.0, 47.0))
         + 0.151 * skyNoise(p * 4.12 + vec2f(57.6, 142.4));
  }

  // Where the weather stands, nought a clear sky and one properly overcast.
  // Two sines against the world clock, out of phase, so the sky clears and
  // closes over a quarter of an hour and never repeats on a beat anyone could
  // hum — and every client computes the same weather, because it is the same
  // clock.
  fn cloudCover(t: f32) -> f32 {
    return clamp(0.46 + 0.26 * sin(t * 0.011) + 0.14 * sin(t * 0.031 + 2.0), 0.0, 1.0);
  }

  fn cloudWindow(t: f32) -> f32 { return 0.68 - 0.26 * cloudCover(t); }

  fn cloudDensity(uv: vec2f, t: f32) -> f32 {
    return smoothstep(cloudWindow(t), cloudWindow(t) + 0.30, cloudField(uv));
  }

  // The same, when the caller already knows where the window is. `cloudWindow`
  // is two sines of the clock — the same two for every pixel on the screen —
  // and `cloudsAt` was asking for it twice per pixel, once for the deck and
  // once for the step toward the sun.
  fn cloudDensityAt(uv: vec2f, cw: f32) -> f32 {
    return smoothstep(cw, cw + 0.30, cloudField(uv));
  }

  // Where a ray from `from` along `dir` crosses the layer, in noise units,
  // with the drift folded in.
  fn cloudUv(org: vec3f, dir: vec3f, t: f32) -> vec2f {
    let hit = org.xz + dir.xz * ((CLOUD_H - org.y) / max(dir.y, 0.03));
    return (hit + CLOUD_DRIFT * (t * CLOUD_WIND)) * CLOUD_SCALE;
  }

  // The deck as seen along `dir`: a colour and how much of it there is.
  fn cloudsAt(org: vec3f, dir: vec3f, t: f32) -> vec4f {
    if (dir.y < 0.015) { return vec4f(0.0); }
    let cw = cloudWindow(t);
    let uv = cloudUv(org, dir, t);
    let den = cloudDensityAt(uv, cw);
    if (den <= 0.003) { return vec4f(0.0); }

    let day = dayness();
    let L = -normalize(u.sun.xyz);
    let sa = max(dot(dir, L), 0.0);
    let glow = smoothstep(-0.34, 0.06, L.y);

    // Cheap self-shadow: the field again, a step toward the sun. Where the
    // step finds thinner cloud the sun is getting in, and that face is the
    // bright one; where it finds thicker, this is the underside of a pile.
    let denS = cloudDensityAt(uv + normalize(L.xz + vec2f(0.0001)) * 0.055, cw);
    let face = clamp(0.5 + 0.9 * (denS - den), 0.0, 1.0);   // 0 sunlit, 1 shaded

    // Lit tops by day; by night the deck goes to a dark slate that still
    // carries a little of whatever moon there is.
    let litc  = mix(vec3f(0.050, 0.058, 0.095) * (0.6 + 0.7 * moonPhase()),
                    vec3f(1.02, 1.00, 0.965), day);
    let shade = mix(vec3f(0.016, 0.019, 0.036),
                    vec3f(0.355, 0.360, 0.430), day);
    var c = mix(litc, shade, clamp(face * 0.8 + den * 0.35, 0.0, 1.0));

    // The sunset on their undersides. It outlives the sun the way the sky's
    // own afterglow does, and it lands hardest on the thin outlying scraps —
    // a solid deck is its own shadow and stays grey while the wisps burn.
    let s2 = sa * sa;
    let s3 = s2 * sa;
    c += SUN_WARM * s3 * glow * (1.0 - 0.75 * day) * (1.0 - 0.62 * den) * 1.35;
    // And the silver lining, which is the sun getting through the last metre
    // of a cloud's edge: strongest where the deck is neither absent nor thick.
    // Twenty-four is sixteen and eight, which is four squarings and a multiply
    // — cheaper than the log and the exp a `pow` costs, and the same number.
    let s4 = s2 * s2;
    let s8 = s4 * s4;
    c += SUN_COL * (s8 * s8 * s8) * den * (1.0 - den) * 3.2 * glow;

    // Swallowed by the horizon haze rather than cut: the far edge of a cloud
    // deck sinks into the air long before it reaches the ground.
    let a = den * smoothstep(0.015, 0.15, dir.y) * 0.94;
    return vec4f(c, a);
  }

  // How much sun reaches the ground through the deck. The same crossing the
  // eye's rays make, taken from the sun's side — so this is not a second
  // weather, it is the one overhead, and when a gap in it drifts across the
  // valley a patch of sunlight drifts with it.
  fn cloudShadow(world: vec3f, t: f32) -> f32 {
    let L = -normalize(u.sun.xyz);
    if (L.y < 0.05) { return 1.0; }
    let hit = world.xz + L.xz * ((CLOUD_H - world.y) / L.y);
    let uv = (hit + CLOUD_DRIFT * (t * CLOUD_WIND)) * CLOUD_SCALE;
    let cw = cloudWindow(t);
    let den = smoothstep(cw, cw + 0.30, cloudFieldLow(uv));
    return 1.0 - 0.60 * den;
  }

  // How much of a day it is: one where the sun is well up, zero once it is
  // properly under. The band either side of zero is the twilight, and it is
  // deliberately wide — the light going is the slowest thing in an evening and
  // a sky that switches off at the moment the sun touches the ridge is the
  // single most obvious way to get a day/night cycle wrong.
  fn dayness() -> f32 {
    return smoothstep(-0.20, 0.14, -normalize(u.sun.xyz).y);
  }

  // Every sky pixel in the valley comes through here — the fullscreen pass, and
  // the lake, which asks it again for what it is reflecting. So this function
  // is the one place in the renderer where a wasted transcendental is worth
  // going after by hand, and there were four kinds of waste in it:
  //
  //   * `pow` with a whole-number exponent, which is a log and an exp for what
  //     is three multiplies;
  //   * the sun's disc and halo, computed for every pixel in the sky when they
  //     are nothing at all beyond a few degrees of the sun;
  //   * the star lattice, whose hash was paid at noon to be multiplied by the
  //     nought that daylight leaves;
  //   * and the bodies, which is where most of it was — see `nightSky`.
  //
  // None of it changes the picture. What is gated is gated at a threshold
  // where the term it guards is smaller than a thousandth, and the rest is the
  // same arithmetic said in fewer instructions.
  fn skyColor(dir: vec3f, t: f32) -> vec3f {
    let up = max(dir.y, 0.0);
    let day = dayness();
    let sun = -normalize(u.sun.xyz);
    let grad = pow(up, 0.55);
    var col = mix(mix(NIGHT_HORIZON, NIGHT_ZENITH, grad),
                  mix(SKY_HORIZON, SKY_ZENITH, grad), day);
    // The air itself, thickest along the horizon. Looking level you look
    // through fifty times the atmosphere you look through overhead, and all
    // that air scatters the day back at you: the horizon whitens toward a
    // milk the zenith never reaches, and at night it holds the faint lift
    // that keeps a ridge legible against the sky.
    let a1 = 1.0 - up;
    let a2 = a1 * a1;
    let airy = a2 * a2 * a1;                          // (1 - up)^5, in multiplies
    col = mix(col, mix(vec3f(0.034, 0.040, 0.066), vec3f(0.315, 0.305, 0.355), day), airy * 0.55);
    let sa = max(dot(dir, sun), 0.0);
    // The warm side of the sky outlives the sun itself — that is what an
    // afterglow is — so it fades on a later, wider curve than the disc.
    let glow = smoothstep(-0.34, 0.06, sun.y);
    let s2 = sa * sa;
    col += SUN_WARM * (s2 * s2 * sa) * 0.5 * glow;    // sa^5, likewise
    // The disc and its halo, only where they are anything. At the threshold the
    // halo is already down to a ten-thousandth of its strength and the disc to
    // a number with sixty noughts in front of it; the branch turns off with the
    // sun's own angular size and is the same for every pixel in a wavefront
    // except along one small ring, so it costs nothing to take.
    if (sa > 0.86) {
      col += SUN_COL * pow(sa, 900.0) * 20.0 * day;   // the disc
      col += SUN_WARM * pow(sa, 60.0) * 1.2 * glow;   // its halo
    }
    // Stars, on the sky's frame rather than the world's, so they wheel with
    // the moon and the planets instead of hanging while everything else moves.
    //
    // Brighter, and a few of them much brighter: one lattice at one intensity
    // is a dusting of identical specks, which is what a star field never looks
    // like. Two passes over the same cells at different densities give a scatter
    // of ordinary ones and a handful that carry, and the brightest get a touch
    // of colour, because the bright ones are the only ones an eye can see any
    // colour in at all.
    // The two cheap facts first: below the horizon there are no stars, and in
    // daylight there is nothing left of them to add. Both were being tested
    // after the lattice hash rather than before it, so noon paid a sine per
    // pixel for a term it then multiplied by nought.
    if (dir.y > 0.04 && day < 0.999) {
      let sdir = skySpace(dir, t);
      let cell = floor(sdir * 240.0);
      let h = fract(sin(dot(cell, vec3f(127.1, 311.7, 74.7))) * 43758.5453);
      if (h > 0.9955) {
        let mag = (h - 0.9955) / 0.0045;               // 0 faint, 1 the brightest
        let twinkle = 0.62 + 0.38 * sin(t * 2.4 + h * 400.0);
        // Warm at one end of the scale and blue at the other, the way real ones
        // sort themselves — and the tint only shows on the bright ones.
        let tint = mix(vec3f(1.0), mix(vec3f(1.0, 0.78, 0.62), vec3f(0.72, 0.82, 1.0),
                                       fract(h * 733.0)), mag);
        // Out entirely in daylight and full once the sun is down. Stars do not
        // go anywhere at noon; the sky in front of them does.
        col += tint * up * twinkle * (0.55 + 2.6 * mag * mag) * (1.0 - day);
      }
    }
    // The moon and the two planets, over the stars and under nothing: they are
    // the furthest things in the world beyond the air.
    let b = nightSky(dir, t);
    col = col * (1.0 - b.a) + b.rgb;
    // And the clouds over the lot of it — sun, stars, moon and planets alike —
    // because they are the one thing up there that is *inside* the atmosphere.
    // A thin scrap lets the disc bleed through; a deck puts it out.
    let cl = cloudsAt(u.eye.xyz, dir, t);
    return mix(col, cl.rgb, cl.a);
  }

  // What is in the direction `dir` from the middle of the valley: whatever was
  // baked into the cube, over the live sky wherever nothing was. One sample and
  // one mix, and it is the difference between a surface that shines and one
  // that shows you where it is standing.
  fn envAt(dir: vec3f, t: f32) -> vec3f {
    let hit = textureSampleLevel(envCube, barkSamp, dir, 0.0);
    return mix(skyColor(dir, t), hit.rgb, hit.a);
  }

  // The same cube, corrected for where you are standing in it.
  //
  // A cube map is a picture taken from one point, and read straight it behaves
  // as if it were infinitely far away: every surface in the valley reflects the
  // identical image, and nothing in the reflection moves when the surface does.
  // That is the whole reason a probe reads as a painted-on tint rather than as
  // a mirror — a mirror's defining property is that what is in it depends on
  // where it is and where you are, and an uncorrected lookup has neither.
  //
  // The fix is to stop pretending the valley is at infinity. Fire the reflected
  // ray at a box drawn round the place, find where it actually lands, and look
  // *that* point up from the probe's position instead of looking up a bare
  // direction. Now a crystal on the west shore reflecting east gets the east
  // hillside, the one beside it gets a slightly different piece of it, and
  // walking past sweeps the image across the face. (Behrens' parallax-corrected
  // cubemap, and it is six lines.)
  const ENV_MIN = vec3f(-88.0, -12.0, -88.0);
  const ENV_MAX = vec3f( 88.0,  44.0,  88.0);
  const ENV_EYE = vec3f(0.0, 7.0, 0.0);      // must match `W.envAt` in host.js

  fn envHere(world: vec3f, R: vec3f, t: f32) -> vec3f {
    // Guard the reciprocal: a ray exactly parallel to an axis has an infinite
    // hit distance on it, and the min below has to be able to ignore that.
    let inv = 1.0 / (R + vec3f(0.000001));
    let far = max((ENV_MAX - world) * inv, (ENV_MIN - world) * inv);
    let d = min(min(far.x, far.y), far.z);
    return envAt(normalize(world + R * d - ENV_EYE), t);
  }

  // The lamp on your head. It comes on by itself once the sun is under the
  // world and goes off when it comes back, which is what anyone would do with
  // one and saves the demo a key nobody would find.
  //
  // A torch, not a point light. Three things make a beam read as a beam and all
  // three are one line each: a cone with a soft edge, so it ends somewhere; a
  // hot centre inside the cone, because the reflector behind a bulb concentrates
  // most of what it throws into a few degrees; and an inverse-square falloff
  // with a hard end, so the dark past the beam is genuinely dark. Leave out the
  // hotspot and it is a flat disc of light. Leave out the cone and it is the
  // sun, from your face.
  //
  // No shadows off it, which for a light that always shines exactly where you
  // are looking costs less than it sounds: the things it would have to shadow
  // are almost all behind whatever you are pointing it at.
  const LAMP_RANGE = 26.0;
  const LAMP_COS_IN  = 0.965;      // ~15°, the hot middle
  const LAMP_COS_OUT = 0.855;      // ~31°, where the beam gives out

  // One beam, wherever it is standing and whichever way it is pointed. Both
  // the lamp on your own head and everybody else's go through here: a light is
  // a light, and the only thing that made this player's special was that the
  // shader could read the camera and had nothing to read for anyone else.
  // (`lampAt` and not `from`: WGSL keeps that word, the way it keeps `ref`.)
  fn beamAt(n: vec3f, world: vec3f, lampAt: vec3f, fwd: vec3f, day: f32) -> vec3f {
    let on = 1.0 - day;
    if (on <= 0.001) { return vec3f(0.0); }
    let toLamp = lampAt - world;
    let d = length(toLamp);
    if (d > LAMP_RANGE) { return vec3f(0.0); }
    let L = toLamp / max(d, 0.0001);

    // How far off the axis of the beam this point is.
    let c = dot(-L, fwd);
    let cone = smoothstep(LAMP_COS_OUT, LAMP_COS_IN, c);
    // And the hotspot inside it — the same profile raised, so the middle is
    // several times the edge without the edge moving.
    let beam = cone * (0.42 + 1.35 * cone * cone);

    // Flat inside two and a half metres rather than following the inverse
    // square all the way in. A real lamp sits on your forehead and the barrel
    // is a foot below it; this one is at the eye exactly, and an inverse square
    // from a point thirty centimetres from the receiver blows the rifle to
    // white. Held flat, the gun reads as lit by something on your head, which
    // is the whole idea.
    let fall = 1.0 / (1.0 + pow(max(d, 2.6) / 5.5, 2.0));
    // And out entirely at the end of its range rather than trailing off, which
    // is what keeps the far hillside dark instead of faintly washed.
    let reach = 1.0 - smoothstep(LAMP_RANGE * 0.72, LAMP_RANGE, d);

    let ndl = max(dot(n, L), 0.0);
    return vec3f(1.0, 0.955, 0.88) * (1.25 * ndl * beam * fall * reach * on);
  }

  fn headlamp(n: vec3f, world: vec3f, day: f32) -> vec3f {
    // Off unless somebody has pressed `F`. It used to come on by itself at
    // dusk, which is a fine default for one person walking through a valley
    // and the wrong one for six: a light nobody chose is a light nobody can
    // put out, and on a dark hillside the thing that gives you away should be
    // something you decided to switch on.
    if (u.lamp.x < 0.5) { return vec3f(0.0); }
    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    return beamAt(n, world, u.eye.xyz, fwd, day);
  }

  // And the other people's, which is the whole point of a switch: a lamp you
  // can see coming over the ridge before its owner does. Six slots, gathered
  // nearest-first by the page — `w` is nought in a slot nobody is using, and
  // the loop is fixed-length because a uniform-length one costs the same and
  // reads worse.
  fn otherLamps(n: vec3f, world: vec3f, day: f32) -> vec3f {
    var sum = vec3f(0.0);

    for (var i = 0u; i < 6u; i = i + 1u) {
      let at = u.lamps[i * 2u];
      if (at.w > 0.5) {
        sum = sum + beamAt(n, world, at.xyz, u.lamps[i * 2u + 1u].xyz, day);
      }
    }
    return sum;
  }

  // Distance haze, thickest along the valley floor. The exponential alone
  // fogs a mountaintop as hard as the water in front of it, which is wrong in
  // both directions: the peaks lose their shape and the far shore never quite
  // recedes. Squaring it holds the near ground clear and lets the far end go,
  // and the height term keeps the air above the lake thicker than the air
  // around the summits.
  fn fogAt(world: vec3f, dist: f32) -> f32 {
    let d = 1.0 - exp(-dist * 0.019);
    let lift = mix(1.0, 0.30, smoothstep(2.0, 16.0, world.y));
    return clamp(d * d * 1.5 * lift, 0.0, 0.94);
  }

  fn fogColor(view: vec3f) -> vec3f {
    let sa = max(dot(normalize(vec3f(view.x, 0.0, view.z)), -normalize(vec3f(u.sun.x, 0.0, u.sun.z))), 0.0);
    let day = dayness();
    // The haze is lit by the sun like everything else, so at night it stops
    // being warm on one side and goes to the flat blue of air with only a moon
    // over it. Fog that keeps a sunset in it after dark is the thing that makes
    // a night look like a day with the brightness turned down.
    let lit2 = mix(vec3f(0.10, 0.11, 0.19), vec3f(0.55, 0.35, 0.22), pow(sa, 3.0));
    return mix(vec3f(0.020, 0.026, 0.052), lit2, day);
  }

  // The light an impact throws onto everything around it.
  //
  // This was missing and it was the largest thing missing. A fireball the size
  // of a house was going off against a hillside that took no notice of it: the
  // fire was a bright sprite in front of unchanged ground, which is exactly how
  // a sticker looks. What sells an explosion is not the fireball, it is the
  // half second where the grass, the trunks and the far slope all go orange and
  // then go back — because that is the only part of it that says the light is
  // in the world rather than on the screen.
  //
  // One point light with the inverse square it actually obeys, and a decay far
  // faster than the fireball's own: what lights a valley is the first second,
  // long before the smoke is finished. The `1 - exp` is the rise — the flash
  // does not exist and then does, it takes a frame or two to reach full, and
  // without that the whole valley changes brightness between two frames and
  // reads as a bug in the exposure rather than as an explosion.
  fn blastLight(n: vec3f, world: vec3f) -> vec3f {
    let age = u.blast.w;
    if (age < 0.0 || age > 2.6) { return vec3f(0.0); }
    let d = u.blast.xyz - world;
    let dist = length(d);
    let L = d / max(dist, 0.001);
    let fall = 1.0 / (1.0 + dist * dist * 0.0075);
    let burn = exp(-age * 2.1) * (1.0 - exp(-age * 55.0));
    // Warmer and redder than the sun by a long way, and with a floor under the
    // dot product: a fireball is a volume, not a point, so the faces turned
    // away from its centre are still standing inside its glow.
    let face = 0.18 + 0.82 * max(dot(n, L), 0.0);
    // Twice the sun at your feet and a fifth of it at twenty metres. This was
    // six and a half, which is what a fireball three metres away really does to
    // your eye and which turned the whole screen into one flat yellow field
    // with the trees, the lake and the far slope all the same value — the exact
    // headlight the puffs are blended rather than added to avoid. An explosion
    // has to light the valley and still leave it a valley; past about two it
    // stops lighting anything because there is no longer anything to light.
    return vec3f(1.0, 0.44, 0.13) * face * fall * burn * 2.1;
  }

  fn lit(albedo: vec3f, n: vec3f, world: vec3f) -> vec3f {
    let L = -normalize(u.sun.xyz);
    let day = dayness();
    let nl = max(dot(n, L), 0.0);
    let sh = shadow(world, nl);
    let sky = mix(vec3f(0.10, 0.11, 0.18), vec3f(0.18, 0.19, 0.30), n.y * 0.5 + 0.5);
    // Two lights, and at night only one of them is left. The sun's own term is
    // cut by `day` as well as by the geometry: a surface can face where the sun
    // was long after it has gone, and without this the hillside stays lit by a
    // sun that is under the world.
    //
    // What remains is the sky's own light, at a third of its daytime strength
    // and turned cold. Not a third chosen for realism — a moonlit valley you
    // cannot walk through is a black screen, and a sixth, which is what this
    // was, is exactly that. This is roughly the exposure an eye finds after a
    // few minutes outside, which is the honest thing to draw when the
    // alternative is drawing what a camera would get.
    let night = vec3f(0.040, 0.048, 0.082) * (0.62 + 0.38 * (n.y * 0.5 + 0.5));
    let amb = mix(night, sky * 1.1, day);

    // And the moon, which is the other half of it. A flat ambient makes a night
    // that is dim but shapeless — every face of every rock the same value, so
    // the valley goes to a grey mass with no form in it. What gives a night its
    // shape is that it does have a light in it, one light, weak and off to one
    // side, and rock turned toward it is brighter than rock turned away.
    //
    // Scaled by how much of the moon's face is lit, so a night near new moon is
    // genuinely darker than one near full — the moon is fixed and the sun is
    // not, so that cycle comes for free. And gone by day: it is up there at
    // noon too, but a hundredth of what the sun is putting down.
    let M = normalize(MOON_AT);
    let moon = MOON_COL * max(dot(n, M), 0.0) * moonPhase() * 0.55 * (1.0 - day);

    // The sun through the weather: a fragment under a cloud gets what the
    // cloud lets past, and the ambient stands in for the rest. The shadow
    // moves because the deck does.
    var col = albedo * (SUN_COL * 1.25 * nl * sh * day * cloudShadow(world, u.eye.w)
                        + moon + amb + headlamp(n, world, day)
                        + otherLamps(n, world, day)
                        + blastLight(n, world));
    let view = world - u.eye.xyz;
    return mix(col, fogColor(view), fogAt(world, length(view)));
  }

  // ---- where the rifle is held ----------------------------------------------
  //
  // Shared by the rifle itself and by the flash at its muzzle, which have to
  // agree to the millimetre or the fire comes out of the air beside the barrel.
  //
  // Aiming: the scope's own optical axis on the view axis, and the barrel
  // pointing where the eye points. The lens sits at y = +0.0859 in the model
  // and dead centre across it, so dropping the rifle by that much times the
  // scale puts the sight picture where the crosshair is. Nudged right and
  // canted besides, because it is held in a right hand — enough to see, not
  // enough to take the sights off what they are pointed at.
  const VM_SCALE = 0.46;
  const VM_AT    = vec3f(0.0002 * VM_SCALE + 0.016, -0.0859 * VM_SCALE, 0.26);
  // And where it goes with the right button held: seven centimetres nearer the
  // eye and dead centre, with the cant taken out of it. Nearer than this and
  // the eyepiece is most of what there is to see — which is honest, an eye
  // against a scope does see mostly scope, and it is also unplayable. The rest
  // of the effect is the page's: a vignette that closes in as the sights come
  // up, which is what the eye does anyway. The rifle already sits
  // with the scope on the view axis — it always did — so getting behind it is
  // a small move and a long look rather than a new pose. `vmSize.w` says how
  // far through that move it is, nought to one.
  const VM_AIM   = vec3f(0.0, -0.0859 * VM_SCALE, 0.15);
  const VM_YAW   = -0.055;
  const VM_PITCH =  0.0;
  const VM_ROLL  = -0.115;
  // Its own frustum. A near plane at a centimetre so the stock can sit right
  // against the eye without being cut in half, and a tighter field of view than
  // the valley's, because a wide angle applied to something at arm's length
  // stretches it into a caricature. Every shooter does both; the world's matrix
  // cannot, because a near plane this close would throw away the depth
  // precision the valley needs across four hundred metres.
  const VM_NEAR = 0.01;
  const VM_FAR  = 10.0;
  const VM_FOV  = 0.40;   // half-angle, against the world's 0.53

  // Model coordinates to the camera's own frame — right, up, forward — with the
  // walk and the recoil folded in.
  fn vmCam(m: vec3f) -> vec3f {
    // Recoil: one at the shot, most of it gone in a fifth of a second. A rifle
    // does not simply move back — the impulse runs along the bore, which is
    // above the shoulder holding it, so the muzzle climbs as well.
    //
    // Minus, and the sign is the whole of it. `Rx` is built from columns, so
    // it takes the barrel — +z once the model's axes are shuffled — to
    // (0, -sin a, cos a): a *positive* angle points the muzzle at the ground.
    // It was written with a plus and read as the stock rising while the
    // barrel dug in, which is the one thing recoil never does. Invisible on a
    // bolt gun at one shot every second and a half; unmissable on an AK at
    // five. That is the
    // whole of what a shot looks like from behind the sights.
    let rc = u.wind.w;
    let held = 1.0 - u.vmSize.w;
    let cy = cos(VM_YAW * held);        let sy = sin(VM_YAW * held);
    let cp = cos(VM_PITCH - 0.11 * rc); let sp = sin(VM_PITCH - 0.11 * rc);
    let cr = cos(VM_ROLL * held);       let sr = sin(VM_ROLL * held);
    let Ry = mat3x3f(vec3f(cy, 0.0, -sy), vec3f(0.0, 1.0, 0.0), vec3f(sy, 0.0, cy));
    let Rx = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, cp, sp), vec3f(0.0, -sp, cp));
    let Rz = mat3x3f(vec3f(cr, sr, 0.0), vec3f(-sr, cr, 0.0), vec3f(0.0, 0.0, 1.0));

    // The walk. `wind.y` is how fast the player is moving and `wind.z` a phase
    // integrated from that speed, so the bob keeps its place when the speed
    // changes instead of jumping. Small, because a rifle held on target is held
    // still — enough that it is not welded to the screen, not enough to spoil
    // the aim, with a slow idle sway on top.
    let amp = clamp(u.wind.y / 5.5, 0.0, 1.0);
    let bp = u.wind.z;
    let t = u.eye.w;
    // A quarter of it behind the sights: somebody aiming is holding their
    // breath, and a sway that reads as life at the hip reads as a shake
    // through a scope.
    let bob = vec3f(
      sin(bp) * 0.0060 * amp + sin(t * 0.7) * 0.0014,
      -abs(cos(bp)) * 0.0050 * amp + sin(t * 0.9 + 1.1) * 0.0011,
      -0.004 * amp,
    ) * (1.0 - 0.75 * u.vmSize.w);
    let kick = vec3f(0.0, 0.008 * rc, -0.048 * rc);
    // And out of frame while the hands are changing weapons. One number does
    // both weapons because only one of them is ever drawn: it runs to one as
    // what is held goes down, the draw switches to the other mesh at the
    // bottom, and it runs back to nought as that one comes up.
    let stow = vec3f(0.0, -0.42 * u.rpgMin.w, 0.10 * u.rpgMin.w);
    // Down on one knee. `fx.y` is how far into the crouch the body is, and
    // the weapon has to go with it or the whole thing reads as the world
    // sinking rather than the person: the eye drops sixty centimetres, and a
    // rifle that stayed exactly where it was on the screen while that
    // happened would be a rifle floating at chest height above a crouching
    // man. It comes down and in — the elbows tuck, which is what anyone does
    // going down — and a little of the muzzle rises, because the near hand
    // drops further than the far one.
    let crouch = clamp(u.fx.y, 0.0, 1.0);
    let duck = vec3f(0.004, -0.030, 0.020) * crouch;
    // The model is built with +x down the barrel, +y up and +z out to the
    // right, so reaching the camera's axes is a shuffle and then the rotations.
    return (Ry * Rx * Rz) * vec3f(m.z, m.y, m.x) * VM_SCALE
         + mix(VM_AT, VM_AIM, u.vmSize.w) + bob + kick + stow + duck;
  }

  // A direction, by the same rotation but without the offsets.
  fn vmDir(d: vec3f) -> vec3f {
    let rc = u.wind.w;
    let held = 1.0 - u.vmSize.w;
    let cy = cos(VM_YAW * held);        let sy = sin(VM_YAW * held);
    let cp = cos(VM_PITCH - 0.11 * rc); let sp = sin(VM_PITCH - 0.11 * rc);
    let cr = cos(VM_ROLL * held);       let sr = sin(VM_ROLL * held);
    let Ry = mat3x3f(vec3f(cy, 0.0, -sy), vec3f(0.0, 1.0, 0.0), vec3f(sy, 0.0, cy));
    let Rx = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, cp, sp), vec3f(0.0, -sp, cp));
    let Rz = mat3x3f(vec3f(cr, sr, 0.0), vec3f(-sr, cr, 0.0), vec3f(0.0, 0.0, 1.0));
    return (Ry * Rx * Rz) * vec3f(d.z, d.y, d.x);
  }

  // ---- the launcher, and what is loaded in it ------------------------------
  //
  // Where the tube sits, in the camera's own frame: out to the right, down a
  // little, and far enough forward that the warhead is in front of the eye
  // rather than through it. Nearer the middle than it looks — the frustum here
  // is a 0.40 half-angle, so at thirty centimetres the frame is only twelve
  // centimetres wide and an offset that would be modest in the world is off
  // the edge of the screen.
  //
  // It lives here rather than in `launcher.wgsl` because the warhead has to
  // agree with it exactly: the two meshes came out of one file and the rocket
  // was authored sitting in the muzzle, so drawing it through the same
  // transform puts it where the artist had it and nothing has to be measured.
  const RPG_AT    = vec3f(0.062, -0.098, 0.30);
  // Very nearly along the view axis, and that is the point. A rifle may sit at
  // an angle because you are looking down its sights and its barrel is thin;
  // a launcher is a metre and two thirds of tube and its axis is the most
  // obvious line on the screen. At seven degrees off — which is where this
  // was — the tube pointed five metres wide of where the rocket went at forty,
  // and the smoke came out of somewhere the weapon plainly was not aimed. The
  // rocket leaves along the eye's own axis, because that is the one thing the
  // browser and the server can both agree on without measuring a viewmodel, so
  // the tube is what has to come to it.
  const RPG_YAW   = -0.03;
  const RPG_PITCH =  0.0;
  const RPG_ROLL  = -0.05;
  const RPG_SCALE = 0.26;
  // And where it goes when the sights come up.
  //
  // This one has no glass either. The screen used to close down to a circle
  // around it — the rifle's scope, borrowed — which was drawing an optic that
  // is not on the weapon: an RPG-7's sight is a post at the muzzle and a
  // ladder behind it, both bolted to the *left* of the tube because the tube
  // is carried on the right shoulder and the eye has to see past it.
  //
  // So the launcher does what the AK does: it comes to the eye and the sights
  // do the work. The numbers are the model's own. The front post tops out at
  // y +0.2443 on a centreline of z -0.044, read off the mesh, and this is what
  // puts that point on the view axis: 0.2443 and -0.044 through the same
  // scale, negated. The tube axis sits at y +0.14, so it ends up two and a
  // half centimetres *below* the line — you look over it, which is how the
  // weapon is actually aimed and why the sights stand as tall as they do.
  //
  // The distance is left alone. Bringing it nearer would make the post bigger
  // and would also pull the back of the tube — which reaches to model -1.097,
  // a centimetre and a half in front of the eye as it is — through the near
  // plane, and a launcher cut open along its length is worse than a small post.
  const RPG_AIM   = vec3f(0.0114, -0.0635, 0.30);

  fn rpgRot(rc: f32, held: f32) -> mat3x3f {
    let cy = cos(RPG_YAW * held);        let sy = sin(RPG_YAW * held);
    let cp = cos(RPG_PITCH - 0.16 * rc); let sp = sin(RPG_PITCH - 0.16 * rc);
    let cr = cos(RPG_ROLL * held);       let sr = sin(RPG_ROLL * held);
    let Ry = mat3x3f(vec3f(cy, 0.0, -sy), vec3f(0.0, 1.0, 0.0), vec3f(sy, 0.0, cy));
    let Rx = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, cp, sp), vec3f(0.0, -sp, cp));
    let Rz = mat3x3f(vec3f(cr, sr, 0.0), vec3f(-sr, cr, 0.0), vec3f(0.0, 0.0, 1.0));
    return Ry * Rx * Rz;
  }

  fn rpgCam(m: vec3f) -> vec3f {
    let rc = u.wind.w;
    // How far through the move to the sights the hands are — the same number
    // the rifle and the AK ride, so all three weapons come up together.
    let held = 1.0 - u.vmSize.w;
    // The walk, at the rifle's amplitude: it is the same shoulders carrying it.
    let amp = clamp(u.wind.y / 5.5, 0.0, 1.0);
    let bp = u.wind.z;
    let t = u.eye.w;
    // A quarter of it behind the sights, for the rifle's reason: a sway that
    // reads as life at the hip reads as a shake along a sight line.
    let bob = vec3f(
      sin(bp) * 0.0060 * amp + sin(t * 0.7) * 0.0014,
      -abs(cos(bp)) * 0.0050 * amp + sin(t * 0.9 + 1.1) * 0.0011,
      -0.004 * amp,
    ) * (1.0 - 0.75 * u.vmSize.w);
    // Backwards and up when it goes off, harder than the rifle: the whole of
    // the propellant leaves the front of this one.
    let kick = vec3f(0.0, 0.014 * rc, -0.075 * rc);
    // And out of frame while the hands are changing weapons.
    let stow = vec3f(0.0, -0.42 * u.rpgMin.w, 0.10 * u.rpgMin.w);
    // Down on one knee. `fx.y` is how far into the crouch the body is, and
    // the weapon has to go with it or the whole thing reads as the world
    // sinking rather than the person: the eye drops sixty centimetres, and a
    // rifle that stayed exactly where it was on the screen while that
    // happened would be a rifle floating at chest height above a crouching
    // man. It comes down and in — the elbows tuck, which is what anyone does
    // going down — and a little of the muzzle rises, because the near hand
    // drops further than the far one.
    let crouch = clamp(u.fx.y, 0.0, 1.0);
    let duck = vec3f(0.004, -0.030, 0.020) * crouch;
    // The model's axes to the camera's, the same shuffle the rifle does.
    return rpgRot(rc, held) * vec3f(m.z, m.y, m.x) * RPG_SCALE
         + mix(RPG_AT, RPG_AIM, u.vmSize.w) + bob + kick + stow;
  }

  fn rpgDir(d: vec3f) -> vec3f {
    return rpgRot(u.wind.w, 1.0 - u.vmSize.w) * vec3f(d.z, d.y, d.x);
  }

  // The camera's axes in the world, rebuilt from the same yaw and pitch Rill
  // used to build the view matrix, so nothing held can drift out of alignment.
  fn camToWorld(cam: vec3f) -> vec3f {
    let yaw = u.cam.x; let pitch = u.cam.y;
    let cpi = cos(pitch);
    let fwd = vec3f(sin(yaw) * cpi, sin(pitch), -cos(yaw) * cpi);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let up  = cross(rgt, fwd);
    return u.eye.xyz + rgt * cam.x + up * cam.y + fwd * cam.z;
  }

  fn camDirToWorld(cam: vec3f) -> vec3f {
    let yaw = u.cam.x; let pitch = u.cam.y;
    let cpi = cos(pitch);
    let fwd = vec3f(sin(yaw) * cpi, sin(pitch), -cos(yaw) * cpi);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let up  = cross(rgt, fwd);
    return rgt * cam.x + up * cam.y + fwd * cam.z;
  }

  // The viewmodel's own projection. `cam` is already in the camera's frame,
  // which is the only thing a view matrix would have produced. Depth into the
  // front hundredth of the buffer, order among its own triangles intact: the
  // camera collides with the world at rather more than ten centimetres, which
  // is where a world fragment would have to be to land in front of it.
  fn vmClip(cam: vec3f) -> vec4f {
    let pb = 1.0 / tan(VM_FOV);
    let pa = pb / u.sun.w;                       // w carries the aspect
    let A = VM_FAR / (VM_FAR - VM_NEAR);
    let B = VM_NEAR * VM_FAR / (VM_FAR - VM_NEAR);
    return vec4f(pa * cam.x, pb * cam.y, (A * cam.z - B) * 0.012, cam.z);
  }

  // ---- working the bolt ------------------------------------------------------
  //
  // `vmMin.w` carries the cycle: zero when the rifle is closed and ready, and
  // running to one over the throw. A bolt action is four movements and they do
  // not overlap — lift the handle, draw it back, run it forward, turn it down —
  // which is exactly why it is slow, and why you cannot fire in the middle of
  // one. Each gets its own slice of the cycle, eased, because a bolt is worked
  // by a hand rather than by a servo: it starts and it stops.
  //
  // The axis of the thing is the bore's own line through the bolt body, and the
  // handle sticks out to the right of it. Turning about that line by minus a
  // right angle and a bit takes the handle from out to up, which is what
  // lifting a bolt is.
  const BOLT_AXIS  = vec2f(0.0522, 0.0022);   // y and z of the bolt's centre line
  const BOLT_THROW = 0.095;                   // how far back it comes
  const BOLT_LIFT  = -1.30;                   // radians, handle out to handle up

  fn ease(a: f32, b: f32, t: f32) -> f32 {
    let x = clamp((t - a) / (b - a), 0.0, 1.0);
    return x * x * (3.0 - 2.0 * x);
  }

  fn boltLift(t: f32) -> f32 {
    // Up over the first sixth, down again over the last; open in between.
    return BOLT_LIFT * (ease(0.00, 0.16, t) - ease(0.80, 0.97, t));
  }

  fn boltPos(m: vec3f) -> vec3f {
    let t = u.vmMin.w;
    if (t <= 0.0 || t >= 1.0) { return m; }
    // Back over the second quarter, forward over the third.
    let back = ease(0.16, 0.46, t) - ease(0.52, 0.80, t);
    let a = boltLift(t);
    let ca = cos(a); let sa = sin(a);
    let y = m.y - BOLT_AXIS.x;
    let z = m.z - BOLT_AXIS.y;
    return vec3f(m.x - BOLT_THROW * back,
                 BOLT_AXIS.x + y * ca - z * sa,
                 BOLT_AXIS.y + y * sa + z * ca);
  }

  fn boltDir(n: vec3f) -> vec3f {
    let t = u.vmMin.w;
    if (t <= 0.0 || t >= 1.0) { return n; }
    let a = boltLift(t);
    let ca = cos(a); let sa = sin(a);
    return vec3f(n.x, n.y * ca - n.z * sa, n.y * sa + n.z * ca);
  }

  // ---- and the empty case ----------------------------------------------------
  //
  // The model carries a 54 mm round in the magazine, below the receiver and out
  // of sight behind the stock. That is where it stays until the bolt draws
  // back: then it is lifted to the chamber, thrown out to the right and up, and
  // allowed to fall — the extractor's own arc, which is why brass always leaves
  // a bolt gun over your shoulder rather than in front of you. It shrinks away
  // at the end of the throw rather than vanishing on a frame, and by then it is
  // off the edge of the screen anyway.
  //
  // What flies out is the case alone. The model carries a whole cartridge — a
  // rifle at rest has a loaded round in it — so the brass and the bullet are
  // marked separately at the case mouth, and the bullet is simply not there
  // during the ejection: it left down the barrel. Both come back together
  // afterwards, which is the magazine feeding the next one, and a lie the
  // magazine tells for free because nobody counts.
  const CASE_AT   = vec3f(-0.2023, 0.0452, 0.0);   // its centre, at the chamber
  const CASE_LIFT = 0.100;                          // magazine to chamber

  fn caseK(t: f32) -> f32 { return (t - 0.20) / 0.42; }

  fn casePos(m: vec3f) -> vec3f {
    let t = u.vmMin.w;
    if (t < 0.20 || t > 0.62) { return m; }
    let k = caseK(t);
    var p = m + vec3f(0.0, CASE_LIFT, 0.0) - CASE_AT;
    // End over end as it goes, and turning as well: brass tumbles.
    let a = 9.0 * k; let b = 4.0 * k;
    let ca = cos(a); let sa = sin(a);
    let cb = cos(b); let sb = sin(b);
    p = vec3f(p.x * ca - p.y * sa, p.x * sa + p.y * ca, p.z);
    p = vec3f(p.x * cb + p.z * sb, p.y, -p.x * sb + p.z * cb);
    let shrink = 1.0 - smoothstep(0.80, 1.0, k);
    return CASE_AT + p * shrink
         + vec3f(-0.02 * k, 0.34 * k - 0.70 * k * k, 0.36 * k);
  }

  fn caseDir(n: vec3f) -> vec3f {
    let t = u.vmMin.w;
    if (t < 0.20 || t > 0.62) { return n; }
    let k = caseK(t);
    let a = 9.0 * k; let b = 4.0 * k;
    let ca = cos(a); let sa = sin(a);
    let cb = cos(b); let sb = sin(b);
    let q = vec3f(n.x * ca - n.y * sa, n.x * sa + n.y * ca, n.z);
    return vec3f(q.x * cb + q.z * sb, q.y, -q.x * sb + q.z * cb);
  }

  // The bullet, while its case is in the air: gone, collapsed to a point, which
  // costs one comparison and draws nothing.
  fn slugPos(m: vec3f) -> vec3f {
    let t = u.vmMin.w;
    if (t < 0.20 || t > 0.62) { return m; }
    return CASE_AT;
  }
