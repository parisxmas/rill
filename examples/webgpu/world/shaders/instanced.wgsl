  override emissive: f32 = 0.0;
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    let c = cos(rot); let s = sin(rot);
    let p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;
    let nr = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.normal = nr; o.color = icol;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let n = normalize(v.normal);
    let view = v.world - u.eye.xyz;
    let V = normalize(view);
    let base = lit(v.color, n, v.world);
    let pulse = 0.75 + 0.25 * sin(u.eye.w * 2.1 + v.world.x * 3.0);
    let glow = v.color * (1.35 * pulse);
    let glowFogged = mix(glow, fogColor(view), fogAt(v.world, length(view)));
    var col = mix(base, glowFogged, emissive);

    // The same glass the broken pieces are made of.
    //
    // The chips got this and the standing crystals did not, which is backwards:
    // a shard is a thumbnail and one of these is three metres of polished
    // prism, so whatever a facet does with the light it does at three metres of
    // it. They were flat-shaded lumps of colour sitting in a valley they took
    // no notice of.
    //
    // Fresnel decides how much: a face turned edge-on is a mirror, one square
    // to you is mostly its own colour. Absolute value rather than a clamp, for
    // the same reason as the shards — the prisms are transparent enough in
    // spirit that you see faces from behind.
    let fres = 0.10 + 0.90 * pow(1.0 - abs(dot(V, n)), 5.0);
    // What that face is pointed at, out of the probe: grass below, hillside
    // across, lake beyond it. This is the whole difference between a crystal
    // that is standing in the valley and one that has been pasted onto it.
    let R = reflect(V, n);
    col = mix(col, envAt(R, u.eye.w), fres * 0.62);

    // And the sun on it. Two lobes, the broad one for sheen and the tight one
    // for the spark — a single lobe is either plastic gloss or an invisible
    // pinprick. Cut with the day, because a highlight is the sun in the surface
    // and there is no sun at night.
    let L = -normalize(u.sun.xyz);
    let H = normalize(L - V);
    let nh = max(dot(n, H), 0.0);
    let key = (0.30 + 0.70 * shadow(v.world, max(dot(n, L), 0.0))) * dayness();
    col += SUN_COL * pow(nh, 40.0) * 0.95 * key;
    col += vec3f(1.0, 0.97, 0.92) * pow(nh, 360.0) * 3.4 * key;

    // The edge. Geometry rather than illumination, so it survives any distance
    // and any hour, and it is what makes a prism read as having edges at all
    // against a hillside of the same brightness.
    col += mix(v.color, vec3f(1.0), 0.40) * pow(1.0 - abs(dot(V, n)), 3.0) * 0.55;
    return vec4f(col, 1.0);
  }
