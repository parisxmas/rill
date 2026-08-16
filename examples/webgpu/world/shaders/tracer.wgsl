  // One round in flight, drawn as the streak it leaves rather than as the
  // object it is. A 7.62 bullet is eight millimetres across and moving at four
  // hundred metres a second; at sixty frames a second it crosses seven metres
  // between one frame and the next, so what a camera records is a smear along
  // those seven metres, not a dot somewhere in the middle of them.
  //
  // The smear is built in screen space, not in the world. A billboard stretched
  // along the velocity is the obvious way to do it and it is wrong here: this
  // is a first-person view, so the round flies almost straight away from the
  // camera, and a quad pointing away from the camera projects to nothing at
  // all. In screen space the same round comes out as a bright point when it is
  // going away from you and as a streak when it crosses your view, which is
  // exactly what it looks like.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) local: vec2f,   // along the streak and across it
    @location(2) len: f32,       // the streak's own length, same units
    @location(3) life: f32,
  }

  // How much of the flight the streak covers, and how wide it is on screen.
  // The width is angular — a fixed slice of the frame — so a round a hundred
  // metres out is still a pixel or two wide rather than nothing.
  const TRAIL = 0.07;
  const HALFW = 0.005;

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3f, @location(1) vel: vec3f,
    @location(2) spare: f32, @location(3) life: f32,
  ) -> VOut {
    // Two triangles over the four corners, without an array to index.
    let cx = select(0.0, 1.0, vi == 1u || vi == 2u || vi == 4u);
    let cy = select(-1.0, 1.0, vi == 2u || vi == 4u || vi == 5u);

    let head = pos;
    // The streak reaches back along the flight path, but never nearer the eye
    // than a couple of metres. Cutting it at the near plane instead — which is
    // the obvious thing — draws a line that starts at your own eyeball and
    // reads as a beam rather than as a bullet; this keeps it out at the muzzle
    // where the round actually was, and collapses it to a point for the first
    // frame or two, which is right.
    let speed = max(length(vel), 1.0);
    let reach = min(TRAIL, max(distance(head, u.eye.xyz) - 2.2, 0.0) / speed);
    let tail = pos - vel * reach;
    let hc = u.vp * vec4f(head, 1.0);
    let tc = u.vp * vec4f(tail, 1.0);

    var o: VOut;
    o.life = life;
    o.world = mix(tail, head, cx);
    // The head behind the eye, or the slot empty, and there is nothing honest
    // to draw: put it past the far plane and let the clipper have it.
    if (hc.w <= 0.02 || life <= 0.0) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      o.local = vec2f(0.0, 0.0);
      o.len = 0.0;
      return o;
    }
    // A safety only: turning fast can still swing the tail behind the eye.
    var tcv = tc;
    if (tcv.w < 0.05) {
      tcv = mix(hc, tc, clamp((0.05 - hc.w) / (tc.w - hc.w), 0.0, 1.0));
    }

    // Aspect-corrected screen coordinates, so a round streak stays round.
    let a = u.sun.w;
    let hs = vec2f(hc.x / hc.w * a, hc.y / hc.w);
    let ts = vec2f(tcv.x / tcv.w * a, tcv.y / tcv.w);
    let seg = hs - ts;
    let len = length(seg);
    let dir = select(vec2f(1.0, 0.0), seg / max(len, 0.000001), len > 0.000001);
    let nor = vec2f(-dir.y, dir.x);

    // The quad runs half a width past each end, which is what leaves room for
    // the round caps the fragment draws.
    let along = mix(-HALFW, len + HALFW, cx);
    let q = ts + dir * along + nor * (cy * HALFW);

    o.clip = vec4f(q.x / a, q.y, mix(tcv.z / tcv.w, hc.z / hc.w, cx), 1.0);
    o.local = vec2f(along, cy * HALFW);
    o.len = len;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.life <= 0.0) { discard; }
    // Distance to the segment, in half-widths: a capsule, so the ends are
    // round and a round flying straight away is a point rather than a stub.
    let x = clamp(v.local.x, 0.0, v.len);
    let d = length(vec2f(v.local.x - x, v.local.y)) / HALFW;
    let glow = pow(clamp(1.0 - d, 0.0, 1.0), 2.0);

    // Hot at the front, cooling toward the tail.
    let front = clamp(v.local.x / max(v.len, 0.000001), 0.0, 1.0);
    let col = mix(vec3f(1.0, 0.30, 0.06), vec3f(1.0, 0.94, 0.72), front * front);
    let view = v.world - u.eye.xyz;
    // Additive, and thinned by the haze rather than tinted by it.
    return vec4f(col * glow * (1.6 + 5.0 * front) * (1.0 - fogAt(v.world, length(view))), 1.0);
  }
