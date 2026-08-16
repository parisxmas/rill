  override WAVES: i32 = 5;
  override RIPPLES: i32 = 3;

  // Directions as offsets from the wind's own angle.
  fn woff(i: i32) -> f32 {
    switch i {
      case 0:  { return -0.38; }
      case 1:  { return  0.26; }
      case 2:  { return -0.12; }
      case 3:  { return  0.47; }
      case 4:  { return  0.05; }
      default: { return -0.55; }
    }
  }
  fn wlen(i: i32) -> f32 {
    switch i {
      case 0:  { return 6.5; }
      case 1:  { return 4.0; }
      case 2:  { return 2.6; }
      case 3:  { return 1.7; }
      case 4:  { return 1.1; }
      default: { return 0.75; }
    }
  }
  fn wamp(i: i32) -> f32 {
    switch i {
      case 0:  { return 0.050; }
      case 1:  { return 0.036; }
      case 2:  { return 0.026; }
      case 3:  { return 0.018; }
      case 4:  { return 0.012; }
      default: { return 0.008; }
    }
  }

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) depth: f32,
    @location(3) crest: f32,
  }

  @vertex fn vs(@location(0) xzd: vec3f) -> VOut {
    let p0 = xzd.xy;
    let depth = xzd.z;
    let t = u.eye.w;
    // Waves feel the bottom, and the wind: amplitude fades in the shallows
    // and follows the gusts.
    let ampScale = clamp(depth * 0.55, 0.12, 1.0) * (0.35 + 0.75 * u.cam.w);
    var p = vec3f(p0.x, 0.0, p0.y);
    var n = vec3f(0.0, 1.0, 0.0);
    var crest = 0.0;
    var norm = 0.0;
    for (var i = 0; i < WAVES; i++) {
      let ang = u.cam.z + woff(i);
      let d = vec2f(cos(ang), sin(ang));
      let a = wamp(i) * ampScale;
      let k = 6.28318 / wlen(i);
      let c = sqrt(9.8 / k);
      let f = k * (dot(d, p0) - c * t);
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
    return o;
  }

  // Ripple octaves: directional wavelets far below mesh resolution, summed
  // into the normal. This is where the sun's glitter comes from.
  fn ripple(p: vec2f, t: f32) -> vec3f {
    var n = vec3f(0.0, 0.0, 0.0);
    var freq = 1.4;
    var amp = 0.20;
    let wd = vec2f(cos(u.cam.z), sin(u.cam.z));
    for (var i = 0; i < RIPPLES; i++) {
      let fi = f32(i);
      let d = normalize(mix(vec2f(sin(fi * 2.4 + 1.2), cos(fi * 1.7 - 0.4)), wd, 0.6));
      let ph = dot(d, p) * freq + u.wind.x * (0.35 + fi * 0.25);
      n.x -= d.x * amp * cos(ph);
      n.z -= d.y * amp * cos(ph);
      freq *= 2.3;
      amp *= 0.55;
    }
    return n;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let t = u.eye.w;
    let n = normalize(v.normal + ripple(v.world.xz, t));
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
    // where the trochoids pinch. Speckled so it reads as froth, not paint.
    let speck = fract(sin(dot(floor(v.world.xz * 7.0), vec2f(12.9898, 78.233))) * 43758.5453);
    let shore = (1.0 - smoothstep(0.0, 1.0, v.depth))
              * (0.55 + 0.45 * sin(v.depth * 9.0 - t * 2.1));
    let crests = smoothstep(0.84, 0.99, v.crest) * 0.45 * (0.25 + 0.75 * u.cam.w) * (0.5 + 0.5 * speck);
    let foam = clamp(shore + crests, 0.0, 1.0) * (0.55 + 0.45 * speck);
    col = mix(col, vec3f(0.90, 0.94, 0.93) * (0.6 + 0.4 * sh), clamp(foam, 0.0, 0.85));

    col = mix(col, fogColor(view), fogAmount(length(view)));
    let alpha = mix(0.78, 0.95, smoothstep(0.0, 4.0, v.depth));
    return vec4f(col, alpha);
  }
