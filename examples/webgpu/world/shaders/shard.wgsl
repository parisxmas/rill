  // What is left of a crystal. The same prism the crystals are drawn from, an
  // eighth the size, tumbling on all three axes on the way down and lying
  // still once it has landed — Rill zeroes the spin along with the velocity
  // when a shard reaches the ground, so "at rest" needs no flag here.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }

  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f,
    @location(2) ipos: vec3f, @location(3) rot: vec3f,
    @location(4) scale: f32, @location(5) icol: vec3f,
  ) -> VOut {
    // Three axes, composed in the order they are written. Cheap enough at a
    // couple of hundred instances that a matrix in the buffer would cost more
    // bandwidth than it saved arithmetic.
    let cx = cos(rot.x); let sx = sin(rot.x);
    let cy = cos(rot.y); let sy = sin(rot.y);
    let cz = cos(rot.z); let sz = sin(rot.z);
    let Rx = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, cx, sx), vec3f(0.0, -sx, cx));
    let Ry = mat3x3f(vec3f(cy, 0.0, -sy), vec3f(0.0, 1.0, 0.0), vec3f(sy, 0.0, cy));
    let Rz = mat3x3f(vec3f(cz, sz, 0.0), vec3f(-sz, cz, 0.0), vec3f(0.0, 0.0, 1.0));
    let M = Rz * Ry * Rx;

    let p = M * (pos * scale) + ipos;
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = M * n;
    o.color = icol;
    // A shard whose scale is zero is an empty slot: send it past the far plane
    // rather than branching, which is the same thing the tracers do.
    if (scale < 0.0001) { o.clip = vec4f(0.0, 0.0, 2.0, 1.0); }
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let n = normalize(v.normal);
    let view = v.world - u.eye.xyz;
    let V = normalize(view);

    // The diffuse half, written out rather than taken from `lit`, because the
    // reflection has to go on before the fog does. A facet forty metres off
    // shows you a hazed sky; it does not show you a clear one and then have
    // haze laid over the top. That is the difference between a shard that
    // recedes into the valley and one that looks pasted onto it.
    //
    // Lit like the crystal it broke off, and glowing like it too, but less: a
    // chip lying in the grass is not a standing crystal, and the light in these
    // things evidently keeps some of its own once it is loose.
    let L = -normalize(u.sun.xyz);
    let nl = max(dot(n, L), 0.0);
    let sh = shadow(v.world, nl);
    let amb = mix(vec3f(0.10, 0.11, 0.18), vec3f(0.18, 0.19, 0.30), n.y * 0.5 + 0.5);
    let day = dayness();
    var col = v.color * (SUN_COL * 1.25 * nl * sh * day + MOON_COL * max(dot(n, normalize(MOON_AT)), 0.0) * moonPhase() * 0.55 * (1.0 - day)
                          + mix(vec3f(0.042, 0.050, 0.086), amb * 1.1, day))
            + v.color * 0.30;

    // Fresnel. A facet turned edge-on to you is a mirror; one facing you is
    // mostly its own colour. One dot product, and it is the entire difference
    // between glass and painted plastic — without it a reflection added at
    // constant strength just reads as the whole thing having been washed pale.
    //
    // Absolute value, not a clamp: these tumble, and half the facets you can
    // see at any moment are being looked at from behind.
    let fres = 0.10 + 0.90 * pow(1.0 - abs(dot(V, n)), 5.0);

    // What the facet is actually pointed at — the valley, out of the probe.
    // This is the difference between a shard that shines and one that is a
    // mirror: sky alone gives a facet turned down at the grass the colour of
    // the horizon, which is the one direction it certainly is not looking. Out
    // of the cube it gets green, and the hillside to the side of it, and the
    // lake, and all of it swings as you walk around the shard because that is
    // what a reflection does and a sheen does not.
    let R = reflect(V, n);
    col = mix(col, envAt(R, u.eye.w), fres * 0.80);

    // What actually reads as glass is the highlight. Two lobes off the same
    // sun: a broad one that gives a facet its sheen and is visible from most
    // angles, and a tight one on top that is a hard white spark and is not.
    // One alone is either a dull plastic gloss or an invisible pinprick; the
    // pair is what a polished surface does, because a real one is smooth at
    // two scales at once.
    let H = normalize(L - V);
    let nh = max(dot(n, H), 0.0);
    // Never wholly dead in shadow. A crystal in the shade still catches the
    // sky, and a shard that goes flat the moment a cloud of smoke crosses it
    // looks switched off rather than shaded.
    // And gone with the sun. A chip of glass has nothing of its own to catch:
    // the highlight is the sun in it, so when the sun is under the world the
    // shard keeps only the glow it was cut from.
    let key = (0.30 + 0.70 * sh) * day;
    col += SUN_COL * pow(nh, 42.0) * 1.15 * key;
    col += vec3f(1.0, 0.97, 0.92) * pow(nh, 380.0) * 4.5 * key;

    // And the edge. Every facet you can see nearly side-on goes bright, which
    // is the one cue that survives at any distance and in any light: it is
    // geometry, not illumination. Half the crystal's own colour and half white,
    // so a shard's edges glow in its colour without going to paint.
    let rim = pow(1.0 - abs(dot(V, n)), 3.0);
    col += mix(v.color, vec3f(1.0), 0.45) * rim * 0.75;

    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
