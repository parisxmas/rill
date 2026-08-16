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

  fn skyColor(dir: vec3f, t: f32) -> vec3f {
    let up = max(dir.y, 0.0);
    var col = mix(SKY_HORIZON, SKY_ZENITH, pow(up, 0.55));
    let sa = max(dot(dir, -normalize(u.sun.xyz)), 0.0);
    col += SUN_WARM * pow(sa, 5.0) * 0.5;             // the warm side of the sky
    col += SUN_COL * pow(sa, 900.0) * 20.0;           // the disc
    col += SUN_WARM * pow(sa, 60.0) * 1.2;            // its halo
    // stars, fading out toward the bright horizon
    let cell = floor(dir * 260.0);
    let h = fract(sin(dot(cell, vec3f(127.1, 311.7, 74.7))) * 43758.5453);
    if (h > 0.9982 && dir.y > 0.08) {
      col += vec3f(0.8) * up * (0.5 + 0.5 * sin(t * 2.0 + h * 40.0));
    }
    return col;
  }

  fn fogAmount(dist: f32) -> f32 { return 1.0 - exp(-dist * 0.0065); }

  fn fogColor(view: vec3f) -> vec3f {
    let sa = max(dot(normalize(vec3f(view.x, 0.0, view.z)), -normalize(vec3f(u.sun.x, 0.0, u.sun.z))), 0.0);
    return mix(vec3f(0.10, 0.11, 0.19), vec3f(0.55, 0.35, 0.22), pow(sa, 3.0));
  }

  fn lit(albedo: vec3f, n: vec3f, world: vec3f) -> vec3f {
    let L = -normalize(u.sun.xyz);
    let nl = max(dot(n, L), 0.0);
    let sh = shadow(world, nl);
    let sky = mix(vec3f(0.10, 0.11, 0.18), vec3f(0.18, 0.19, 0.30), n.y * 0.5 + 0.5);
    var col = albedo * (SUN_COL * 1.25 * nl * sh + sky * 1.1);
    let view = world - u.eye.xyz;
    return mix(col, fogColor(view), fogAmount(length(view)));
  }
