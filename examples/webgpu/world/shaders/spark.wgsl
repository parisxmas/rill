  // What comes off the surface when a round arrives.
  //
  // The same ring as the chips, read a second time. A spark burst and a
  // bullet hole are one event seen at two speeds — where the round stopped
  // and the line it came in on is the whole of what either needs — so this
  // adds no buffer, no ring, no wire traffic and nothing for the CPU to step.
  // The instance is the mark; the arithmetic below is what it looks like in
  // its first quarter second.
  //
  // Fourteen fragments per strike, thrown into a cone off the face the round
  // came through, each with its own speed and its own life, all of them
  // falling. Built in screen space for the reason the tracer is: sparks off a
  // wall you have just shot are coming more or less straight at you, and a
  // world-space billboard is exactly degenerate in that case.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) local: vec2f,   // along the streak and across it
    @location(2) len: f32,
    @location(3) fade: f32,
  }

  const SPARKS = 14u;
  const SPARK_LIFE = 0.26;     // the longest one; each takes a share of it
  const SPARK_G = 11.0;        // heavier than gravity: they are small and hot
  const TRAIL = 0.022;         // how far back of itself each one is drawn
  const HALFW = 0.0018;        // angular, so distance does not thin them away

  fn sparkRnd(seed: f32) -> f32 {
    return fract(sin(seed * 12.9898 + 78.233) * 43758.5453);
  }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) at: vec3f, @location(1) dir: vec3f,
    @location(2) born: f32, @location(3) spare: f32,
  ) -> VOut {
    // Fourteen quads out of one instance: the high bits of the vertex index
    // pick the spark, the low three pick its corner.
    let k = vi / 6u;
    let c = vi % 6u;
    let cx = select(0.0, 1.0, c == 1u || c == 2u || c == 4u);
    let cy = select(-1.0, 1.0, c == 2u || c == 4u || c == 5u);

    var o: VOut;
    o.world = at;
    o.local = vec2f(0.0);
    o.len = 0.0;
    o.fade = 0.0;

    // Every spark of one strike shares the strike's own seed, and each takes
    // its own slice of the hash — so a burst is stable frame to frame and two
    // strikes side by side are not the same fourteen sparks twice.
    let base = dot(at, vec3f(127.1, 311.7, 74.7)) + f32(k) * 19.19;
    let life = SPARK_LIFE * mix(0.45, 1.0, sparkRnd(base * 1.13));
    let age = u.eye.w - born;
    // `SPARKS_PER_MARK` in host.js is what the draw asks for and this is what
    // the shader will answer to; the guard is so the two disagreeing is a
    // missing spark rather than a burst that walks off the end of the hash.
    if (k >= SPARKS || born <= 0.0 || age < 0.0 || age > life) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    // The same frame `mark.wgsl` builds, off the same vector: the outward
    // normal of the surface the round stopped on. `other` is whichever axis
    // it is least parallel to, so the cross product never collapses.
    let n = normalize(dir);
    let other = select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(n.y) > 0.9);
    let rgt = normalize(cross(other, n));
    let upv = cross(n, rgt);

    // Out of the surface, into a wide cone about the normal. `ct` is the
    // cosine of the angle off it — never one, or a spark would leave straight
    // up the middle and the burst would have a hole down its centre.
    let ang = sparkRnd(base * 2.71) * 6.2831853;
    let ct = mix(0.20, 0.96, sparkRnd(base * 4.37));
    let st = sqrt(max(0.0, 1.0 - ct * ct));
    let d = n * ct + rgt * (st * cos(ang)) + upv * (st * sin(ang));
    let speed = mix(2.0, 7.0, sparkRnd(base * 6.11));

    // Where it is now, and where it was a moment ago. Both off the surface by
    // the same couple of centimetres the chip is, so a spark thrown along a
    // wall does not start inside it.
    let fly = sparkAt(at + n * 0.02, d, speed, age);
    let tail = sparkAt(at + n * 0.02, d, speed, max(age - TRAIL, 0.0));

    o.fade = 1.0 - age / life;
    o.world = mix(tail, fly, cx);

    let hc = u.vp * vec4f(fly, 1.0);
    let tc = u.vp * vec4f(tail, 1.0);
    if (hc.w <= 0.02 && tc.w <= 0.02) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }
    var hcv = hc;
    var tcv = tc;
    if (hcv.w < 0.05) { hcv = mix(tc, hc, clamp((0.05 - tc.w) / (hc.w - tc.w), 0.0, 1.0)); }
    if (tcv.w < 0.05) { tcv = mix(hc, tc, clamp((0.05 - hc.w) / (tc.w - hc.w), 0.0, 1.0)); }

    let a = u.sun.w;
    let hs = vec2f(hcv.x / hcv.w * a, hcv.y / hcv.w);
    let ts = vec2f(tcv.x / tcv.w * a, tcv.y / tcv.w);
    let seg = hs - ts;
    let len = length(seg);
    let dirs = select(vec2f(1.0, 0.0), seg / max(len, 0.000001), len > 0.000001);
    let nor = vec2f(-dirs.y, dirs.x);

    let along = mix(-HALFW, len + HALFW, cx);
    let q = ts + dirs * along + nor * (cy * HALFW);
    o.clip = vec4f(q.x / a, q.y, mix(tcv.z / tcv.w, hcv.z / hcv.w, cx), 1.0);
    o.local = vec2f(along, cy * HALFW);
    o.len = len;
    return o;
  }

  // Thrown, and falling. Written out because it is wanted twice — once for
  // where the spark is and once for where it was.
  fn sparkAt(origin: vec3f, d: vec3f, speed: f32, t: f32) -> vec3f {
    return origin + d * (speed * t) - vec3f(0.0, 0.5 * SPARK_G * t * t, 0.0);
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.len <= 0.0) { discard; }
    // Round ends rather than square ones: the distance to the segment, not to
    // the centre line, so a spark shorter than it is wide is a dot.
    let x = clamp(v.local.x, 0.0, v.len);
    let d = length(vec2f(v.local.x - x, v.local.y));
    let g = pow(clamp(1.0 - d / HALFW, 0.0, 1.0), 1.7);
    if (g < 0.004) { discard; }

    // White-hot at the strike, orange on the way out, and the last of it red.
    // Squaring the fade is what makes a spark look like it is cooling rather
    // than like it is being turned down.
    let heat = v.fade * v.fade;
    let col = mix(vec3f(1.0, 0.24, 0.04), vec3f(1.0, 0.96, 0.82), heat);
    let view = v.world - u.eye.xyz;
    return vec4f(col * g * (0.35 + 1.65 * heat) * (1.0 - fogAt(v.world, length(view))), 1.0);
  }
