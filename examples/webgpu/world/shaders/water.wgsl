  override WAVES: i32 = 5;
  override RIPPLES: i32 = 3;

  // Value noise: one hash per lattice corner, smoothstepped between them.
  fn fhash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
  }
  fn fnoise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    return mix(mix(fhash(i), fhash(i + vec2f(1.0, 0.0)), w.x),
               mix(fhash(i + vec2f(0.0, 1.0)), fhash(i + vec2f(1.0, 1.0)), w.x), w.y);
  }

  // The spectrum, as five or six plane waves.
  //
  // A wind sea is broad in direction and in wavelength, and these were neither:
  // six waves inside two thirds of a radian of each other, on wavelengths in
  // near-halves. Waves that parallel and that harmonic add up to one shape
  // repeated, which is the corduroy the lake used to wear.
  //
  // Direction fans wider for the short waves — a swell holds its line, chop
  // does not — and the offsets are a sine of the golden angle, which never
  // comes back round on itself for any count of waves.
  fn wdir(i: i32) -> f32 {
    let fi = f32(i);
    return u.cam.z + (0.42 + 0.60 * fi / f32(WAVES)) * sin(fi * 2.39996323 * 3.0 + 1.3);
  }
  // Not halving: with an integer ratio every second crest lands on the one
  // below it and the sum has a period you can see. A jittered 0.72 does not,
  // and it keeps the eighth wave near 0.8 m rather than under the grid's cell.
  fn wlen(i: i32) -> f32 {
    let fi = f32(i);
    return 7.4 * pow(0.72, fi) * (0.84 + 0.32 * fhash(vec2f(fi, 3.0)));
  }
  // Steepness roughly constant down the spectrum, which is what keeps short
  // waves from looking like scale models of long ones.
  fn wamp(i: i32) -> f32 { return wlen(i) * 0.0086; }
  // Every wave starts somewhere else. They all started at zero before, so
  // every crest in the sum passed through the same line at the same moment.
  fn wphase(i: i32) -> f32 { return fhash(vec2f(f32(i), 17.0)) * 6.2831853; }

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) depth: f32,
    @location(3) crest: f32,
    @location(4) sets: f32,
  }

  @vertex fn vs(@location(0) xzd: vec3f, @location(1) slope: vec2f) -> VOut {
    let p0 = xzd.xy;
    let depth = xzd.z;
    let t = u.eye.w;

    // Which way the shore is. `slope` is the gradient of the depth, so it
    // points out into the lake; a wave runs the other way, up the bottom.
    let gl = length(slope);
    let shoreward = select(vec2f(0.0, 0.0), -slope / max(gl, 0.00001), gl > 0.00001);
    // Waves feel the bottom, and the wind: amplitude fades in the shallows
    // and follows the gusts.
    //
    // And the sea comes in sets. A slow noise field drifting across the lake
    // scales every wave at once, so there are patches of chop and patches
    // nearly glassy — without it the same amplitude covers the whole surface
    // and no amount of spectrum makes that look like water.
    let sets = 0.35 + 1.05 * fnoise(p0 * 0.032 + vec2f(t * 0.028, t * 0.016));
    let ampScale = clamp(depth * 0.55, 0.12, 1.0) * (0.35 + 0.75 * u.cam.w) * sets;
    var p = vec3f(p0.x, 0.0, p0.y);
    var n = vec3f(0.0, 1.0, 0.0);
    var crest = 0.0;
    var norm = 0.0;
    for (var i = 0; i < WAVES; i++) {
      let fi = f32(i);
      let lam = wlen(i);
      let k = 6.28318 / lam;

      // Refraction. A wave slows where the water is shallow, so the shallower
      // end of a crest falls behind and the whole line swings round until it
      // runs straight up the slope — which is why surf arrives parallel to a
      // beach whatever direction the wind came from. A long wave feels the
      // bottom from deeper down, so it starts turning further out.
      let ang = wdir(i);
      let open = vec2f(cos(ang), sin(ang));
      let feel = (1.0 - smoothstep(0.0, lam * 0.85, depth)) * smoothstep(0.01, 0.20, gl);
      let bent = mix(open, shoreward, feel * 0.85);
      let d = bent / max(length(bent), 0.0001);

      let a = wamp(i) * ampScale;
      let c = sqrt(9.8 / k);
      // And out in the open, crests wander. A plane wave's crest is a straight
      // line to the horizon; a lake's is not. Shifting the phase by a slow
      // noise field bends the line without breaking it — the shift is written
      // as a distance and turned into radians by this wave's own k, so every
      // wavelength wanders by the same metre and three quarters rather than short
      // ones tearing themselves apart.
      let wander = (fnoise(p0 * 0.030 + vec2f(fi * 7.3, fi * 4.1)) - 0.5) * 3.6 * k;
      let f = k * (dot(d, p0) - c * t) + wphase(i) + wander;
      let q = min(0.65 / (k * a * f32(WAVES) + 0.001), 1.0 / (k * a * f32(WAVES) + 0.001));
      p.x += q * a * d.x * cos(f);
      p.z += q * a * d.y * cos(f);
      p.y += a * sin(f);
      let wa = k * a;
      n.x -= d.x * wa * cos(f);
      n.z -= d.y * wa * cos(f);
      n.y -= q * wa * sin(f);
      crest += (sin(f) * 0.5 + 0.5) * a;
      norm += a;
    }
    var o: VOut;
    o.world = p;
    o.clip = u.vp * vec4f(p, 1.0);
    o.normal = normalize(n);
    o.depth = depth;
    o.crest = crest / max(norm, 0.001);
    o.sets = sets;
    return o;
  }

  // Ripple octaves: directional wavelets far below mesh resolution, summed
  // into the normal. This is where the sun's glitter comes from.
  //
  // The frequency ratio is 2.17 rather than 2.3 and the directions fan the
  // same way the waves do — an octave stack that stays lined up with the wind
  // combs the whole lake in one direction, and the glitter comes out in
  // stripes rather than in points.
  fn ripple(p: vec2f, t: f32, sets: f32) -> vec3f {
    var n = vec3f(0.0, 0.0, 0.0);
    var freq = 1.4;
    var amp = 0.20 * (0.45 + 0.75 * sets);
    for (var i = 0; i < RIPPLES; i++) {
      let fi = f32(i);
      let ang = u.cam.z + (0.35 + 0.75 * fi / f32(RIPPLES)) * sin(fi * 2.39996323 * 2.0 - 0.7);
      let d = vec2f(cos(ang), sin(ang));
      let ph = dot(d, p) * freq + u.wind.x * (0.35 + fi * 0.25) + fhash(vec2f(fi, 29.0)) * 6.28;
      n.x -= d.x * amp * cos(ph);
      n.z -= d.y * amp * cos(ph);
      freq *= 2.17;
      amp *= 0.55;
    }
    return n;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let t = u.eye.w;
    let n = normalize(v.normal + ripple(v.world.xz, t, v.sets));
    let view = v.world - u.eye.xyz;
    let vdir = normalize(view);
    let L = -normalize(u.sun.xyz);

    // Schlick's fresnel against the same sky the sky pass draws.
    let cosv = max(dot(-vdir, n), 0.0);
    let fresnel = 0.02 + 0.98 * pow(1.0 - cosv, 5.0);
    let reflDir = reflect(vdir, n);
    let refl = skyColor(vec3f(reflDir.x, abs(reflDir.y), reflDir.z), t);

    // The body of the water: absorption with depth.
    let body = mix(vec3f(0.11, 0.37, 0.40), vec3f(0.012, 0.10, 0.13), smoothstep(0.0, 5.5, v.depth));

    let sh = shadow(v.world, max(dot(n, L), 0.0));
    var col = mix(body, refl, clamp(fresnel * 1.6 + 0.06, 0.0, 1.0));

    // Sun glitter: a tight Blinn lobe on the rippled normal.
    let hv = normalize(L - vdir);
    col += SUN_COL * pow(max(dot(n, hv), 0.0), 240.0) * 2.4 * sh;

    // Foam: at the shoreline, where the depth runs out, and on the crests,
    // where the trochoids pinch.
    //
    // The froth is value noise, not a hash of a floored coordinate. A hash
    // gives every 14 cm square of lake one constant value, and a shoreline
    // painted with it is a grid of hard little tiles that stay put while the
    // water moves under them. Interpolated and scrolled, the same two lines
    // read as foam being carried.
    let drift = vec2f(cos(u.cam.z), sin(u.cam.z)) * t * 0.16;
    let froth = fnoise(v.world.xz * 2.6 - drift) * 0.65
              + fnoise(v.world.xz * 6.1 - drift * 1.9) * 0.35;

    // The surf runs up the beach in bands. Wobbling the depth first keeps the
    // bands off the lake bed's exact contour — a line of foam that traces the
    // bathymetry looks like a map, not like water.
    let d = v.depth + (fnoise(v.world.xz * 0.42 + vec2f(0.0, t * 0.04)) - 0.5) * 0.5;
    let edge = 1.0 - smoothstep(0.0, 1.25, d);
    let band = 0.5 + 0.5 * sin(d * 7.0 - t * 2.0);
    let shore = edge * edge * (0.30 + 0.70 * band);
    let crests = smoothstep(0.87, 1.0, v.crest) * 0.40 * (0.25 + 0.75 * u.cam.w);
    let foam = clamp((shore + crests) * (0.45 + 0.75 * froth), 0.0, 1.0);
    col = mix(col, vec3f(0.90, 0.94, 0.93) * (0.6 + 0.4 * sh), clamp(foam, 0.0, 0.85));

    col = mix(col, fogColor(view), fogAt(v.world, length(view)));
    let alpha = mix(0.78, 0.95, smoothstep(0.0, 4.0, v.depth));
    return vec4f(col, alpha);
  }
