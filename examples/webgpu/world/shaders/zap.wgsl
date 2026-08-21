  // A crystal's answer to being stood against: a line of its own light, from
  // the glass to the one leaning on it, for a third of a second.
  //
  // The tracer's trick, with both ends pinned. The beam is built in screen
  // space — a quad stretched between the two projected endpoints — because a
  // beam coming *at* the camera is exactly the case a world-space billboard
  // degenerates on, and being bitten is the one time you are guaranteed to be
  // looking down the beam.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) local: vec2f,
    @location(2) len: f32,
    @location(3) fade: f32,
    @location(4) tint: vec3f,
  }

  const ZAP_LIFE = 0.35;
  // Wide, because the quad is the canvas and the bolt lives inside it: the
  // fragment draws a jagged core wandering about the centre line and a soft
  // halo across the whole width, so the strip has to be as wide as the
  // wander plus the glow.
  const HALFW = 0.032;

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pa: vec3f, @location(1) pb: vec3f,
    @location(2) tint: vec3f, @location(3) born: f32,
  ) -> VOut {
    let cx = select(0.0, 1.0, vi == 1u || vi == 2u || vi == 4u);
    let cy = select(-1.0, 1.0, vi == 2u || vi == 4u || vi == 5u);

    let age = u.eye.w - born;
    let live = born > 0.0 && age >= 0.0 && age < ZAP_LIFE;

    var o: VOut;
    o.fade = 1.0 - age / ZAP_LIFE;
    o.tint = tint;
    o.world = mix(pa, pb, cx);

    let hc = u.vp * vec4f(pb, 1.0);
    let tc = u.vp * vec4f(pa, 1.0);
    if (!live || (hc.w <= 0.02 && tc.w <= 0.02)) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      o.local = vec2f(0.0);
      o.len = 0.0;
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
    let dir = select(vec2f(1.0, 0.0), seg / max(len, 0.000001), len > 0.000001);
    let nor = vec2f(-dir.y, dir.x);

    let along = mix(-HALFW, len + HALFW, cx);
    let q = ts + dir * along + nor * (cy * HALFW);
    o.clip = vec4f(q.x / a, q.y, mix(tcv.z / tcv.w, hcv.z / hcv.w, cx), 1.0);
    o.local = vec2f(along, cy * HALFW);
    o.len = len;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.len <= 0.0) { discard; }
    let x = clamp(v.local.x, 0.0, v.len);
    let t = u.eye.w;

    // Lightning, not a laser: the core zigzags about the centre line, two
    // frequencies of it, reshuffled every few frames — which is the whole of
    // what makes electricity read as electricity. The envelope pins both ends
    // to the endpoints, because a bolt that misses the crystal it came from
    // is a bolt drawn next to the story.
    let s = x / max(v.len, 0.000001);
    let env = clamp(s * (1.0 - s) * 5.0, 0.0, 1.0);
    let jolt = floor(t * 24.0);
    let wig = (sin(x * 130.0 + jolt * 37.7) * 0.55
             + sin(x * 313.0 - jolt * 91.3 + 1.7) * 0.30) * HALFW * 0.62 * env;

    // The bolt: a thin bright vein along the wander. The halo: the crystal's
    // own colour, wide and soft over the whole strip — the blend the eye
    // reads as air lit by the discharge rather than as a drawn line.
    let across = v.local.y - wig;
    let endcap = length(vec2f(v.local.x - x, across));
    let vein = pow(clamp(1.0 - endcap / (HALFW * 0.16), 0.0, 1.0), 1.4);
    let halo = pow(clamp(1.0 - abs(v.local.y) / HALFW, 0.0, 1.0), 2.2);
    let flick = 0.75 + 0.25 * sin(jolt * 17.9);

    let col = v.tint * (halo * 1.5) + vec3f(1.0, 1.0, 1.0) * (vein * 2.6);
    let view = v.world - u.eye.xyz;
    return vec4f(col * flick * 2.2 * v.fade * v.fade * (1.0 - fogAt(v.world, length(view))), 1.0);
  }
