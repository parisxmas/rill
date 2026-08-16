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
    @location(5) windward: f32,   // how much this puff faces the way it is going
  }

  const BALL   = 0.62;            // the head, in metres
  // Enough of them, close enough together, to be a trail rather than a string
  // of beads: at fourteen metres a second, 1.75 seconds is twenty-four metres
  // of flight, and forty-eight puffs put one every half metre against a head
  // more than half a metre across. Sixteen over thirteen metres did not, and
  // it showed — the count has to go up with the length, not just the length.
  const PUFFS  = 48u;
  const TRAIL  = 1.75;            // seconds of it behind the head
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

    // The fire on the stone itself, which is not a ring round it.
    //
    // Friction is not the same in every direction. The heat is where the air is
    // being compressed, and that is the face going forwards — a bow of fire
    // ahead of and around the nose, thinning off toward the sides, and nothing
    // at all behind, where the stone is moving away from the air and leaves a
    // wake rather than a flame. The wake is the trail; it is already drawn.
    //
    // So these puffs sit on a cap tilted onto the front rather than a ring
    // round the middle: `lat` is how far off the nose each one is, never past a
    // right angle, and `cos(lat)` — how square-on to the airflow it is — is
    // handed to the fragment to burn by.
    let collar = clamp(1.0 - age * 9.0, 0.0, 1.0);
    let ca = seed * 13.0 + age * 47.0;
    let lat = 0.30 + 1.05 * fract(seed * 31.0 + f32(n) * 0.37);
    let axis = normalize(vel + vec3f(0.0, 0.0001, 0.0));
    let side = normalize(cross(axis, vec3f(0.0, 1.0, 0.0)) + vec3f(0.0001, 0.0, 0.0));
    let other = cross(axis, side);
    let radial = (side * cos(ca) + other * sin(ca)) * sin(lat);

    // The air, as the stone sees it. Standing flame around a body moving at
    // fourteen metres a second is the thing that gives a fire away as painted
    // on: air does not sit still to be burnt in, it is coming past at speed,
    // and what it does to the flame is drag it backwards. Most of that is the
    // stone's own motion, but the valley's wind is in it too — the same wind
    // that is moving the pines and the water, off the same two numbers.
    let airIn = vec3f(cos(u.cam.z), 0.0, sin(u.cam.z)) * (1.4 + 4.0 * u.cam.w);
    let airflow = normalize(airIn - vel);

    // A puff square onto the airflow sits where it started; one out at the
    // shoulder has had the air run past it the whole way round and is dragged
    // much further back. That difference is the shape of a bow shock.
    let expose = 1.0 - cos(lat);
    let ring = (axis * cos(lat) * 0.75 + radial * 0.70) * collar * 0.62
             + airflow * collar * (0.22 + 0.95 * expose);

    // Drift: smoke lifts, and it goes where the wind takes it. That direction
    // used to be picked per fireball out of the seed, which looked plausible
    // and was wrong — every trail in the sky leant a different way at the same
    // moment. It is the valley's own wind now, so the smoke lies down with the
    // trees. What varies along the trail is only how far it has been carried,
    // and that grows smoothly, so the trail stays a trail.
    let carried = vec3f(cos(u.cam.z), 0.0, sin(u.cam.z)) * (0.30 + 1.5 * u.cam.w);
    let drift = (carried + vec3f(0.0, 0.9, 0.0)) * age * age * 0.55;

    // Bigger as it goes back, the way smoke opens out, and smaller again right
    // at the head where the flame is tight.
    let size = BALL * (0.55 + 1.7 * age * age + 0.45 * (1.0 - age));

    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let up  = cross(rgt, fwd);

    let centre = back + drift + ring;
    let world = centre + (rgt * cx + up * cy) * size;

    o.world = world;
    o.clip = u.vp * vec4f(world, 1.0);
    // Dimming the whole way back, and reaching nothing rather than levelling
    // off. It has to fall this steeply because the puffs overlap: thirty-two
    // of them over fifteen metres, added together, hold a near-constant
    // brightness across the first half of the trail unless each one is already
    // most of the way down by the time the next one arrives.
    o.fade = select(0.0, pow(1.0 - age, 2.2), span > 0.02);
    // Zero for anything in the trail: the wake does not burn, it only glows
    // with what it was given on the way past.
    o.windward = collar * clamp(cos(lat), 0.0, 1.0);
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

    // Premultiplied alpha, not additive. Additive is right for a spark and
    // wrong for a column of smoke: forty-eight puffs seen end-on — which is
    // exactly what happens when a fireball is coming at you — land on the same
    // pixels and sum to forty-eight times the brightness, and the screen goes
    // white. Blended, the same forty-eight converge on the colour instead of
    // running away from it, which is what smoke actually does. The head still
    // burns because its colour is allowed over one; what is bounded is how much
    // of a pixel each puff may claim, not how bright it is.
    let view = v.world - u.eye.xyz;
    let clear = 1.0 - fogAt(v.world, length(view));
    // Not `a`: that name is taken further up by the angle this puff's edge is
    // wobbled against, and WGSL does not let a scope redeclare its own names.
    // What burns is what faces the airflow. A puff square onto it is white and
    // opaque; one out at the shoulder is half that; one behind the stone is
    // just the trail, and gets none of it.
    let hot = pow(1.0 - v.age, 4.0) + v.windward * v.windward * 1.6;
    let cover = clamp(glow * v.fade * mix(0.95, 0.34, smoke) * (1.0 + 0.7 * v.windward),
                      0.0, 1.0) * clear;
    return vec4f(col * (0.6 + 2.4 * hot) * cover, cover);
  }
