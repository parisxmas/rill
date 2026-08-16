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
    return mix(vec3f(0.10, 0.11, 0.19), vec3f(0.55, 0.35, 0.22), pow(sa, 3.0));
  }

  fn lit(albedo: vec3f, n: vec3f, world: vec3f) -> vec3f {
    let L = -normalize(u.sun.xyz);
    let nl = max(dot(n, L), 0.0);
    let sh = shadow(world, nl);
    let sky = mix(vec3f(0.10, 0.11, 0.18), vec3f(0.18, 0.19, 0.30), n.y * 0.5 + 0.5);
    var col = albedo * (SUN_COL * 1.25 * nl * sh + sky * 1.1);
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
    // above the shoulder holding it, so the muzzle climbs as well. That is the
    // whole of what a shot looks like from behind the sights.
    let rc = u.wind.w;
    let cy = cos(VM_YAW);               let sy = sin(VM_YAW);
    let cp = cos(VM_PITCH + 0.11 * rc); let sp = sin(VM_PITCH + 0.11 * rc);
    let cr = cos(VM_ROLL);              let sr = sin(VM_ROLL);
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
    let bob = vec3f(
      sin(bp) * 0.0060 * amp + sin(t * 0.7) * 0.0014,
      -abs(cos(bp)) * 0.0050 * amp + sin(t * 0.9 + 1.1) * 0.0011,
      -0.004 * amp,
    );
    let kick = vec3f(0.0, 0.008 * rc, -0.048 * rc);
    // The model is built with +x down the barrel, +y up and +z out to the
    // right, so reaching the camera's axes is a shuffle and then the rotations.
    return (Ry * Rx * Rz) * vec3f(m.z, m.y, m.x) * VM_SCALE + VM_AT + bob + kick;
  }

  // A direction, by the same rotation but without the offsets.
  fn vmDir(d: vec3f) -> vec3f {
    let rc = u.wind.w;
    let cy = cos(VM_YAW);               let sy = sin(VM_YAW);
    let cp = cos(VM_PITCH + 0.11 * rc); let sp = sin(VM_PITCH + 0.11 * rc);
    let cr = cos(VM_ROLL);              let sr = sin(VM_ROLL);
    let Ry = mat3x3f(vec3f(cy, 0.0, -sy), vec3f(0.0, 1.0, 0.0), vec3f(sy, 0.0, cy));
    let Rx = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, cp, sp), vec3f(0.0, -sp, cp));
    let Rz = mat3x3f(vec3f(cr, sr, 0.0), vec3f(-sr, cr, 0.0), vec3f(0.0, 0.0, 1.0));
    return (Ry * Rx * Rz) * vec3f(d.z, d.y, d.x);
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
  // It is the same round that comes back for the next shot, which is a lie a
  // magazine tells for free: nobody counts.
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
