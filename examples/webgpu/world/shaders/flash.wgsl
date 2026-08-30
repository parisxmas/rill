  // The flash at somebody else's barrel.
  //
  // Your own gun has had one since the beginning — a shape drawn on the
  // viewmodel, in the viewmodel's own coordinates, and it can only ever be
  // yours. Everybody else fired in the dark: the figure flinched, a round
  // appeared somewhere out in the valley, and nothing said where it had come
  // from. With guardians standing watch in cover that is the whole problem —
  // being shot at by a thing you cannot locate, from a direction you have to
  // guess at.
  //
  // So the same ring of rounds the tracer is drawn from is drawn a second
  // time, as a point of light where each one began. There is no new buffer
  // and nothing to keep up to date: a round already carries where it is, how
  // fast, and how much life it has left, and the muzzle is simply where it
  // was when it left — position minus velocity times the time it has been
  // flying. The flash lasts a twentieth of a second, which is about what a
  // camera catches, and it is gone long before the round is.
  //
  // It draws for every round in the valley, including the AK's, whose tracers
  // are deliberately dark: ten lit streaks a second is a rope, but ten
  // flashes a second at the far end of a barrel is what a rifle firing looks
  // like, and it is the thing that tells you where to shoot back.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) local: vec2f,
    @location(1) k: f32,        // how much of the flash is left, 1 to 0
    @location(2) world: vec3f,
  }

  const BULLET_LIFE = 3.2;      // must match `bullet_life` in valley.rill
  const FLASH = 0.05;           // seconds
  const SIZE = 0.30;            // metres across, at its brightest
  // Nearer than this to the eye and it is our own gun going off: the round
  // starts at the barrel, the barrel is a hand's breadth from the camera, and
  // a card that size that close is the whole screen. The viewmodel draws its
  // own flash and does not want this one on top of it. Asked as a distance
  // rather than by comparing owners because the id of whoever is looking is
  // not a thing this buffer carries — and a metre and a half is far enough
  // outside arm's reach that nobody else's muzzle is ever inside it.
  const MINE = 1.5;

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3f, @location(1) vel: vec3f,
    @location(2) owner: f32, @location(3) life: f32,
    @location(4) shotBy: f32,
  ) -> VOut {
    let cx = select(-1.0, 1.0, vi == 1u || vi == 2u || vi == 4u);
    let cy = select(-1.0, 1.0, vi == 2u || vi == 4u || vi == 5u);

    let flown = max(BULLET_LIFE - life, 0.0);
    let k = 1.0 - flown / FLASH;
    let muzzle = pos - vel * flown;

    var o: VOut;
    o.k = k;
    o.local = vec2f(cx, cy);
    o.world = muzzle;
    if (life <= 0.0 || k <= 0.0 || distance(muzzle, u.eye.xyz) < MINE) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }
    // Camera-facing, built from the view matrix's own axes so it is square to
    // the eye however the eye is turned. A flash is a ball of burning gas and
    // has no orientation of its own to respect.
    let right = vec3f(u.vp[0][0], u.vp[1][0], u.vp[2][0]);
    let up = vec3f(u.vp[0][1], u.vp[1][1], u.vp[2][1]);
    // It is at its widest the instant it goes and shrinks as it dies, which
    // is what keeps a burst from reading as one steady lamp.
    let r = SIZE * (0.45 + 0.55 * k);
    let at = muzzle + (normalize(right) * cx + normalize(up) * cy) * r;
    o.clip = u.vp * vec4f(at, 1.0);
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let d = length(v.local);
    if (d > 1.0 || v.k <= 0.0) { discard; }
    // A hot white core in a yellow flare, falling off fast: most of the light
    // is in the middle sixth of it.
    let glow = pow(clamp(1.0 - d, 0.0, 1.0), 2.2);
    let core = pow(clamp(1.0 - d, 0.0, 1.0), 8.0);
    let col = mix(vec3f(1.0, 0.52, 0.10), vec3f(1.0, 0.97, 0.86), core);
    let view = v.world - u.eye.xyz;
    return vec4f(col * (glow * 1.6 + core * 4.0) * v.k
                 * (1.0 - fogAt(v.world, length(view))), 1.0);
  }
