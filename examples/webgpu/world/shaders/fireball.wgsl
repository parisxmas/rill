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
    @location(6) secs: f32,       // real seconds since this parcel was made
    @location(7) rocket: f32,     // one if this trail is behind a rocket
  }

  const BALL   = 0.62;            // the head, in metres
  // Enough of them, close enough together, to be a trail rather than a string
  // of beads: at fourteen metres a second, 2.9 seconds is forty metres of
  // flight, and eighty puffs put one every half metre against a head more than
  // half a metre across. Sixteen over thirteen metres did not, and it showed —
  // the count has to go up with the length, not just the length. It is set by
  // the rate, in fact, not chosen: RATE parcels a second for TRAIL seconds is
  // how many exist, and any fewer means the far end of the trail is missing.
  // A hundred and twenty, which is what a rocket needs and a crystal's fire
  // does not: the extra forty sit past the end of a meteor's trail, come out
  // at age one, and are discarded on their first fragment.
  const PUFFS  = 120u;
  const TRAIL  = 2.90;            // seconds of it behind the head, for fire
  // A rocket's own two. It goes five times as fast, and a trail spaced by time
  // is spaced by distance too: twenty-seven parcels a second at seventy metres
  // a second is one every two and a half metres, drawn at two thirds of a
  // metre across — a dotted line, which is what it looked like. Seventy a
  // second puts one every metre, and a hundred and twenty of them reach one
  // and seven tenths of a second back, which is a hundred and twenty metres of
  // smoke behind the thing.
  const RATE_R  = 70.0;
  const TRAIL_R = 1.70;
  // What the constants below were tuned against, so that changing TRAIL moves
  // only the length and leaves the near end of the trail exactly as it was.
  const WAS    = 1.75;
  const FIRE_G = 3.2;             // must match `fireball_g` in world.rill
  const RATE   = 27.0;            // parcels of smoke made a second

  fn phash(n: f32) -> f32 { return fract(sin(n * 127.1 + 311.7) * 43758.5453); }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3f, @location(1) vel: vec3f,
    @location(2) seed: f32, @location(3) life: f32, @location(4) flown: f32,
  ) -> VOut {
    let q = vi % 6u;
    let n = vi / 6u;                       // 0 the head, up to PUFFS-1 the tail
    let cx = select(-1.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cy = select(-1.0, 1.0, q == 2u || q == 4u || q == 5u);

    var o: VOut;
    o.uv = vec2f(cx, cy);
    o.seed = abs(seed);
    o.age = 0.0;
    o.world = pos;
    o.fade = 0.0;
    o.windward = 0.0;
    o.secs = 0.0;
    o.rocket = select(0.0, 1.0, seed < 0.0);
    // A negative seed is a rocket rather than a crystal's fire — see `launch`
    // in valley.rill. It flies through the same ring and gets the same trail,
    // which is the whole reason it is in this ring, but the head of it is a
    // tube of steel drawn by `rocket.wgsl` and not a ball of fire. So the head
    // quad is dropped here and what is left is the smoke.
    let rocket = seed < 0.0;
    if (life <= 0.0 || (rocket && n == 0u)) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    // Which parcel of smoke this is — not which slot in a list, which parcel.
    //
    // Drawing the trail by index is what made it read as a shape rather than a
    // fire: the puff at index five would take index six's size and colour and
    // wobble the moment the meteor moved on, so the whole pattern slid backward
    // through smoke that was otherwise sitting still. Nothing had an identity;
    // there was no parcel of anything, only a diagram of one.
    //
    // Anchored to when it was made instead, everything follows. Smoke is made
    // at a fixed rate, so `floor(flown * RATE) - n` names a parcel and keeps
    // naming it: as the meteor flies on, the parcel that was the head becomes
    // the second, then the third, and carries its own hash — its own crumple,
    // its own drift, its own size — the whole way. Its age is real time since
    // it was made, so it grows and cools rather than inheriting the look of
    // whatever is now that far behind.
    let rate = select(RATE, RATE_R, rocket);
    let span = select(TRAIL, TRAIL_R, rocket);
    let slot = floor(flown * rate) - f32(n);
    let raw = flown - slot / rate;

    // How long since the rocket arrived, and nothing if it is still flying.
    // A negative life is the world saying this slot holds smoke rather than a
    // rocket; `flown` stopped where it stopped, so `raw` is frozen with it.
    let since = max(0.0, -life);

    // Two ages, and the whole of the lingering is that they are two.
    //
    // Where a parcel *sits* is decided by how far back along the flight it
    // was made — and once the rocket has arrived that must stop changing, or
    // the column slides backwards along a path nothing is travelling any more.
    // How old it *is* keeps running, because smoke goes on spreading, drifting
    // and thinning after the thing that made it has gone.
    //
    // Freezing both was the obvious thing and it is wrong: the trail hangs
    // there unchanged for as long as the slot lasts and then blinks out.
    // Running both is what happened before, which is that the trail is thrown
    // away at the moment of impact — the head is what the position is measured
    // from, and there is no longer a head.
    let tp = clamp(raw, 0.0, span);
    let t = clamp(raw + since, 0.0, span);

    // Past the end of this kind's trail. A meteor's runs out at eighty parcels
    // and the draw asks for a hundred and twenty, which is what a rocket wants;
    // the difference is sent past the far plane here rather than left to come
    // out at age one and be discarded a fragment at a time. Smoke that has
    // finished fading goes the same way.
    if (raw > span || raw + since > span) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }
    let age = t / span;
    let born = slot >= 0.0;              // not made yet, at the start of a shot

    // Three numbers that belong to this parcel and to no other.
    let sd = abs(seed);
    let hA = phash(slot * 1.37 + sd * 91.0);
    let hB = phash(slot * 2.71 + sd * 57.0);
    let hC = phash(slot * 4.19 + sd * 23.0);

    // Backwards along the flight. The gravity term comes back with a minus,
    // which is what makes the trail sag below the straight line back from the
    // head by exactly what the head has already climbed.
    // `tp`, not `t`: this is where along the flight the parcel was made, and
    // that is a fact about a path already travelled.
    let back = pos - vel * tp - vec3f(0.0, 0.5 * FIRE_G * tp * tp, 0.0);
    o.age = age;
    // Real seconds as well as the fraction. Everything that is a fact about
    // the smoke — how long the wind has had it, how long it takes a flame to
    // become smoke, how far back the fire on the stone reaches — is a fact
    // about seconds, and if it is written against the fraction instead then
    // lengthening the trail stretches the flame along with it and the head
    // stops looking like a head. The fraction is for one thing only: fading to
    // nothing by the end, wherever the end has been put.
    o.secs = t;

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
    let collar = clamp(1.0 - t * (9.0 / WAS), 0.0, 1.0);
    let ca = hB * 6.2831;
    let lat = 0.30 + 1.05 * hA;
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
    // And its own crumple. Smoke does not expand evenly, it rolls: each parcel
    // takes a direction of its own and keeps turning that way for as long as it
    // lasts. Because the direction belongs to the parcel rather than to the
    // slot, the curl travels with the smoke instead of standing still in the
    // air while smoke slides through it.
    let curl = vec3f(hA - 0.5, (hB - 0.5) * 0.6, hC - 0.5) * 2.0;
    let roll = t * ((1.4 + hC * 2.2) / WAS);
    let churn = (curl + cross(curl, vec3f(0.0, 1.0, 0.0)) * sin(roll)) * t * t * (0.9 / (WAS * WAS));
    let drift = (carried + vec3f(0.0, 0.9, 0.0)) * t * t * (0.55 / (WAS * WAS)) + churn;

    // Bigger as it goes back, the way smoke opens out, and smaller again right
    // at the head where the flame is tight.
    // A plume opens out roughly in step with how long it has been out, so this
    // grows with the seconds and not with the fraction — and linearly, not as
    // the square it used to be: squared was indistinguishable over 1.75 seconds
    // and reaches three-metre puffs over 2.9, which is a cloud, not a trail.
    let old = t / WAS;
    // Half again as big for a rocket, and it has to be: the puffs are a metre
    // apart at that speed and anything smaller than that is a row of beads
    // rather than a column of smoke. A motor also makes a great deal more of
    // it than a stone dragging through the air does.
    //
    // And a sixth the size for an ember — a piece of a meteor that was shot
    // apart, seeded at two and above; see `ember_seed` in valley.rill. Five
    // of them fall together, and five meteors' worth of fire coming down
    // where one meteor was is a sky on fire rather than wreckage.
    //
    // Sized by walking it in from both ends. A third still read as meteors —
    // but that was measured while every client drew each ember twice and the
    // full sixty-centimetre stone core inside it, so the number took the
    // blame for two other bugs. With those gone, the twelfth it had been cut
    // to was a spark: a fist of burning rock, which is what a piece of a
    // broken meteor is, sits at about a third of the head.
    let ember = sd >= 2.0;
    let fat = select(select(1.0, 1.75, rocket), 0.30, ember);
    let size = BALL * fat * (0.55 + 1.7 * old + 0.45 * max(1.0 - old, 0.0)) * (0.72 + 0.55 * hC);

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
    o.fade = select(0.0, pow(1.0 - age, 2.2) * (0.7 + 0.6 * hB), born);
    // Zero for anything in the trail: the wake does not burn, it only glows
    // with what it was given on the way past.
    // And no bow of fire ahead of a rocket: the heat on a meteor is the air it
    // is compressing, and a rocket at seventy metres a second is not
    // compressing anything worth drawing.
    o.windward = collar * clamp(cos(lat), 0.0, 1.0) * (1.0 - o.rocket);
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
    // Flame becomes smoke after about a third of a second and is all smoke by
    // a second, however long the trail behind it has been made.
    //
    // A rocket's is smoke from the first parcel, and black. It was pale here
    // once, on the argument that a motor throws unburnt propellant rather than
    // soot — true of some motors and not of this one, which is the thing on
    // the shoulder of somebody in a valley, and what that leaves behind is a
    // dirty black column you can follow from the far side.
    //
    // Darker than the stone's, not lighter: the stone is at four hundred
    // degrees and glowing through its own smoke, and the motor is not.
    let smoke = max(smoothstep(0.22 * WAS, 0.62 * WAS, v.secs), v.rocket);
    let ash = mix(vec3f(0.30, 0.27, 0.26), vec3f(0.085, 0.082, 0.080), v.rocket);
    col = mix(col, ash, smoke);

    // The head is a hard little sun; the smoke is a soft blob that mostly is
    // not there at all.
    let core = (1.0 - k * k) * (1.0 - k * k) + 0.9 * exp(-k * k * 7.0);
    let soft = (1.0 - k * k) * 0.55;
    // Smoke claims much less of a pixel than a burning head does, and a
    // meteor's thin soot should. A motor's does not: the whole point of it is
    // that you can follow it across a valley, and knocking it down by
    // seven tenths left a smear you had to be told was there.
    var glow = mix(core, soft, smoke) * (1.0 - mix(0.72, 0.30, v.rocket) * smoke);

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
    let hot = pow(clamp(1.0 - v.secs / WAS, 0.0, 1.0), 4.0) + v.windward * v.windward * 1.6;
    // A third of a pixel each is right for a meteor's wake, which is thin soot
    // seen against the sky. A motor lays down something you are meant to follow
    // across a valley, and black smoke is only black because it is opaque —
    // thin black is not dark, it is nothing.
    let thick = mix(0.34, 0.88, v.rocket);
    let cover = clamp(glow * v.fade * mix(0.95, thick, smoke) * (1.0 + 0.7 * v.windward),
                      0.0, 1.0) * clear;
    return vec4f(col * (0.6 + 2.4 * hot) * cover, cover);
  }
