  override emissive: f32 = 0.0;
  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
    @location(3) phase: f32,
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
    o.world = p; o.normal = nr; o.color = icol; o.phase = rot;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let n = normalize(v.normal);
    let view = v.world - u.eye.xyz;
    let V = normalize(view);
    let base = lit(v.color, n, v.world);
    // Brightest as the light reaches the crystal's tip. See `crystalFlare`.
    let pulse = 0.68 + 0.62 * crystalFlare(v.phase);
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
    // A high floor, unlike the shards'. A crystal is 72 per cent its own glow
    // by the time this runs, so a Fresnel that starts at a tenth leaves the
    // reflection as a six per cent tint on a self-lit block — arithmetically
    // present and invisible. A quarter at dead-on and nearly all of it at the
    // limb is what a big polished prism actually does with a valley.
    let fres = 0.26 + 0.74 * pow(1.0 - abs(dot(V, n)), 4.0);
    // What that face is pointed at, out of the probe: grass below, hillside
    // across, lake beyond it. This is the whole difference between a crystal
    // that is standing in the valley and one that has been pasted onto it.
    let R = reflect(V, n);
    col = mix(col, envHere(v.world, R, u.eye.w), fres * 0.88);

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

    // How solid it is, off the same Fresnel that decided the reflection —
    // which is the physical truth and not a coincidence: the light a surface
    // does not let through is the light it sends back. Square to you a face is
    // mostly window; at the limb it is mostly mirror and mostly opaque, which
    // is why the silhouette of a glass object stays firm while its middle
    // disappears.
    //
    // Premultiplied, because the pipeline blends that way: the colour arrives
    // already scaled by coverage.
    // Barely any. This has come down twice: 0.40 let the hillside through the
    // middle of every prism and they stopped reading as solid objects standing
    // in the valley, and 0.82 still washed them. At 0.93 what is left is a
    // trace — enough that a face catches the light coming through from behind
    // and the far edges of the prism are faintly there, and not enough to see
    // the meadow through it.
    //
    // Which is the trap with glass: past a point transparency stops showing you
    // the inside of a thing and starts showing you that there is no thing.
    let a = clamp(0.93 + 0.07 * fres, 0.0, 1.0);
    return vec4f(col * a, a);
  }
