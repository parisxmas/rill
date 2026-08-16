  // A ball of fire on its way over, with what it has left behind it: a lick of
  // flame off the back, and then smoke, going grey and spreading as it falls
  // behind and cools.
  //
  // The trail is not recorded. Nothing is stored per fireball but where it is,
  // how fast it is going and how long it has been going, and that is enough,
  // because the flight is a known curve: run it backwards.
  //
  //     p(T)   = p₀ + v₀T − ½gT²        v(T) = v₀ − gT
  //     p(T−τ) = p(T) − v(T)·τ − ½gτ²
  //
  // The gravity term comes back with a *minus*, which is worth deriving rather
  // than guessing: a ball at the top of its arc was lower a second ago, not
  // higher, even though its velocity there is horizontal. So the trail sags
  // below the straight line back from the head by exactly what the head has
  // already climbed — the same amount, the same way round, as the flight.
  //
  // For no memory at all, and right on the first frame of a shot rather than
  // filling in over the next twenty.
  //
  // Each fireball is drawn as a row of quads down that curve: the head, then
  // flame, then smoke. One instance, many quads, indexed off the vertex.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) uv: vec2f,       // -1 to 1 across the quad
    @location(2) seed: f32,
    @location(3) age: f32,        // 0 at the head, 1 at the end of the smoke
    @location(4) fade: f32,
  }

  const BALL   = 0.62;            // the head, in metres
  // Enough of them, close enough together, to be a trail rather than a string
  // of beads: at fourteen metres a second, 1.1 seconds is fifteen metres of
  // flight, and thirty-two puffs put one every half metre against a head that
  // is more than half a metre across. Sixteen did not, and it showed.
  const PUFFS  = 32u;
  const TRAIL  = 1.10;            // seconds of it behind the head
  const FIRE_G = 3.2;             // must match `fireball_g` in world.rill

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3f, @location(1) vel: vec3f,
    @location(2) seed: f32, @location(3) life: f32, @location(4) flown: f32,
  ) -> VOut {
    let q = vi % 6u;
    let n = vi / 6u;                       // 0 the head, up to PUFFS-1 the tail
    let cx = select(-1.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cy = select(-1.0, 1.0, q == 2u || q == 4u || q == 5u);
    let age = f32(n) / f32(PUFFS - 1u);

    var o: VOut;
    o.uv = vec2f(cx, cy);
    o.seed = seed;
    o.age = age;
    o.world = pos;
    o.fade = 0.0;
    if (life <= 0.0) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    // Backwards along the flight. The gravity term comes back with a plus,
    // which is what makes the trail sag under the head exactly as much as the
    // head has already risen above where it was.
    // Never further back than the thing has actually flown: a fireball a
    // quarter of a second old has a quarter of a second of smoke behind it,
    // not a second and a tenth reaching back past the crystal that threw it.
    let span = min(TRAIL, flown);
    let t = age * span;
    let back = pos - vel * t - vec3f(0.0, 0.5 * FIRE_G * t * t, 0.0);

    // Drift: smoke lifts and spreads rather than staying on the line the fire
    // took. One direction per fireball, taken from the seed and held for the
    // whole trail — the earlier version swung the angle along the trail as
    // well, which put every puff somewhere else and made the smoke read as a
    // scatter rather than as a column. What varies along it is only how far,
    // and that grows smoothly, so the trail stays a trail.
    let dir = seed * 6.2831;
    let lean = vec3f(cos(dir), 0.0, sin(dir));
    let drift = (lean * 0.45 + vec3f(0.0, 0.9, 0.0)) * age * age * 0.55;

    // Bigger as it goes back, the way smoke opens out, and smaller again right
    // at the head where the flame is tight.
    let size = BALL * (0.55 + 1.7 * age * age + 0.45 * (1.0 - age));

    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let up  = cross(rgt, fwd);

    let centre = back + drift;
    let world = centre + (rgt * cx + up * cy) * size;

    o.world = world;
    o.clip = u.vp * vec4f(world, 1.0);
    // Dimming the whole way back, and reaching nothing rather than levelling
    // off. It has to fall this steeply because the puffs overlap: thirty-two
    // of them over fifteen metres, added together, hold a near-constant
    // brightness across the first half of the trail unless each one is already
    // most of the way down by the time the next one arrives.
    o.fade = select(0.0, pow(1.0 - age, 2.2), span > 0.02);
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.fade <= 0.0) { discard; }
    let r = length(v.uv);
    let a = atan2(v.uv.y, v.uv.x) + v.seed * 6.2831 + u.eye.w * 0.7;

    // The edge, pushed about by three rates that share no factor, so the
    // outline never comes back round to where it was. The smoke's is softer:
    // a hard-edged puff reads as a stone.
    let ragged = mix(1.0, 0.55, v.age);
    let wobble = ragged * (0.09 * sin(a * 3.0 + u.eye.w * 5.1)
                         + 0.06 * sin(a * 5.0 - u.eye.w * 3.7)
                         + 0.04 * sin(a * 8.0 + u.eye.w * 8.3));
    let edge = 0.82 + wobble;
    if (r > edge) { discard; }
    let k = clamp(r / edge, 0.0, 1.0);

    // White at the middle, orange at the rim, dark red where the flame is
    // thinnest — and then, further back, none of that: grey, and thinning.
    var col = mix(vec3f(1.0, 0.96, 0.80), vec3f(1.0, 0.44, 0.08), pow(k, 0.7));
    col = mix(col, vec3f(0.85, 0.12, 0.02), smoothstep(0.72, 1.0, k));
    let smoke = smoothstep(0.22, 0.62, v.age);
    col = mix(col, vec3f(0.30, 0.27, 0.26), smoke);

    // The head is a hard little sun; the smoke is a soft blob that mostly is
    // not there at all.
    let core = (1.0 - k * k) * (1.0 - k * k) + 0.9 * exp(-k * k * 7.0);
    let soft = (1.0 - k * k) * 0.55;
    var glow = mix(core, soft, smoke) * (1.0 - 0.72 * smoke);

    let view = v.world - u.eye.xyz;
    return vec4f(col * glow * 2.0 * v.fade * (1.0 - fogAt(v.world, length(view))), 1.0);
  }
