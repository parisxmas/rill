  // What a fireball leaves where it lands.
  //
  // A ground burst is not one cloud. It is four things on four clocks in three
  // different places, and drawing it as a single expanding ball of smoke is why
  // most of them look like a puff of grey rather than like something arriving:
  //
  //   THE FLASH. White, everywhere at once, over in a twentieth of a second.
  //   It is what makes an explosion read as an explosion rather than as a fire.
  //
  //   THE FIREBALL, which does not fly outward so much as roll. It opens fast,
  //   almost stops — it is being braked by the air it is shoving — and then
  //   lifts, curling in on itself as it goes. That curl is a vortex ring, and
  //   it is the whole reason the shape ends up a mushroom rather than a
  //   sphere that got bigger. Modelled here as a real rotation rather than
  //   faked with a rising sphere: each puff runs round an arc, out and up and
  //   back in, so the thing opens out low and closes over high without any
  //   part of it being told to be a mushroom.
  //
  //   THE BASE SURGE. When a fireball hits ground it cannot expand downward, so
  //   the energy goes sideways: a low ring of dirt and dust that runs out along
  //   the surface far further and far faster than the fireball ever climbs, and
  //   hangs about long after the fire is out. It is the most recognisable part
  //   of a ground burst and the least often drawn.
  //
  //   THE COLUMN, which was the one still missing. Fire is brief and smoke is
  //   not. What you actually watch after the first two seconds is a dark stem
  //   drawn up behind the cap, climbing and spreading and going the colour of
  //   soot, and it stands there long after there is nothing burning. Leaving it
  //   out was why an impact used to be over the moment the orange went: the
  //   valley had no memory of it. A quarter of the puffs are it, and they
  //   outlive the fire by two to one.
  //
  // Four clocks, not one. The fire is out in three seconds, the surge coasts
  // for five, the smoke stands for eight — running them all off a single
  // lifetime is what makes an explosion look like one object dissolving
  // rather than like a sequence of events.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) uv: vec2f,
    @location(2) heat: f32,       // 1 at the heart of it, 0 out at the edge
    @location(3) age: f32,        // seconds since it landed
    @location(4) alpha: f32,
    @location(5) dust: f32,       // 1 for the surge — thrown ground, not flame
    @location(6) soot: f32,       // 1 for the column — smoke, and dark
    @location(7) rnd: f32,        // this puff's own number, for its shape
  }

  const PUFFS  = 96u;
  const SURGE  = 34u;             // [0, SURGE)      the ground wave
  const FIRE   = 70u;             // [SURGE, FIRE)   the rolling fireball
                                  // [FIRE, PUFFS)   the column
  const LIFE   = 8.20;            // seconds before there is nothing left
  const BURN   = 3.10;            // seconds the fire itself lasts
  const SETTLE = 5.20;            // seconds the surge coasts for
  const REACH  = 18.0;            // metres the fireball gets to
  const RUN    = 38.0;            // metres the surge runs along the ground
  const CLIMB  = 30.0;            // metres the column stands

  fn bhash(n: f32) -> f32 { return fract(sin(n * 127.1 + 311.7) * 43758.5453); }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3f, @location(1) seed: f32, @location(2) age: f32,
  ) -> VOut {
    let q = vi % 6u;
    let idx = vi / 6u;
    let n = f32(idx);
    let cx = select(-1.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cy = select(-1.0, 1.0, q == 2u || q == 4u || q == 5u);

    var o: VOut;
    o.uv = vec2f(cx, cy);
    o.age = age;
    o.world = pos;
    o.heat = 0.0;
    o.alpha = 0.0;
    o.dust = 0.0;
    o.soot = 0.0;
    o.rnd = 0.0;
    if (age < 0.0 || age > LIFE) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    let h1 = bhash(seed * 31.0 + n);
    let h2 = bhash(seed * 57.0 + n * 3.7);
    let h3 = bhash(seed * 83.0 + n * 9.1);
    let a = h1 * 6.2831;
    let horiz = vec3f(cos(a), 0.0, sin(a));
    let t = age / LIFE;

    var centre: vec3f;
    var size: f32;
    var heat: f32;
    var alpha: f32;
    var dust: f32;
    var soot: f32;

    if (idx < SURGE) {
      // ---- the base surge
      //
      // Out along the ground, fast at first and then coasting: a root, not a
      // line, because it is spending its speed on the air in front of it. It
      // reaches more than twice as far as the fireball and takes its whole
      // life to do it.
      dust = 1.0;
      soot = 0.0;
      let ts = clamp(age / SETTLE, 0.0, 1.0);
      let depth = 0.30 + 0.70 * h3;
      let run = pow(clamp(age / 1.30, 0.0, 1.0), 0.55) * RUN * depth
              + max(age - 1.30, 0.0) * 1.15;
      // Barely off the ground, and lifting only as it slows — dust rises when
      // it stops being driven, not while it is.
      let lift = 0.35 + 3.4 * ts * ts * (0.5 + h2);
      centre = pos + horiz * run + vec3f(0.0, lift, 0.0);
      size = (2.0 + 2.8 * h3) * (0.45 + 2.6 * ts);
      // Only the very first moment of it is hot: this is thrown ground, not
      // flame, and it takes its colour from the hillside.
      heat = clamp(0.55 - age * 2.2, 0.0, 1.0);
      // Comes in over the first fifth of a second rather than existing at once,
      // then outlives the fire.
      alpha = smoothstep(0.0, 0.16, age) * (1.0 - ts) * (1.0 - ts * 0.35);
    } else if (idx < FIRE) {
      // ---- the fireball, rolling
      //
      // The arc each puff runs: out, up, and back in over the top. `roll` is
      // where it has got to. Under a right angle it is still opening out; past
      // one it is coming in again, which is the underside of the cap folding
      // under, and it is what a mushroom is.
      let tf = clamp(age / BURN, 0.0, 1.0);
      let depth = 0.32 + 0.68 * h3;
      let roll = clamp(age / 1.45, 0.0, 1.15) * 2.15 * (0.75 + 0.5 * h2);
      let grow = sqrt(clamp(age / 0.38, 0.0, 1.0));
      let out = sin(roll);
      let up = 1.0 - cos(roll);
      centre = pos + horiz * (REACH * depth * grow * out)
             + vec3f(0.0, REACH * 0.78 * depth * grow * up + tf * tf * 7.5, 0.0);
      size = (1.30 + 1.15 * h3) * (0.55 + 2.1 * tf);
      // The middle of a fireball is the last part of it to go out, and the
      // parts that have climbed highest have had the longest to cool.
      heat = clamp(1.40 - depth - tf * 1.7 - up * 0.22, 0.0, 1.0);
      alpha = (1.0 - tf) * (1.0 - tf);
      dust = 0.0;
      soot = 0.0;
    } else {
      // ---- the column
      //
      // A stem, not a ball. Each puff owns a height on it — the ones near the
      // top set off first and climb furthest, which is what draws the thing out
      // into a column instead of inflating it in place. The rise is a root: it
      // is buoyant, so it goes up fast while it is hottest and then slows as it
      // mixes with the air it has entrained.
      let up01 = f32(idx - FIRE) / f32(PUFFS - FIRE - 1u);
      // Nothing to see until the fire has made it. The top of the stem is under
      // way first because it is the fireball's own wake.
      let born = 0.14 + (1.0 - up01) * 0.45;
      let rise = pow(clamp((age - born) / 4.4, 0.0, 1.0), 0.62);
      // Narrow at the ground and wide at the head — the head is what the
      // fireball turned into, and it keeps spreading after it stops climbing.
      let girth = mix(1.6, 7.0, up01 * up01) * (0.45 + 1.5 * t);
      centre = pos
             + horiz * girth * (0.35 + 0.65 * h3) * rise
             + vec3f(0.0, CLIMB * up01 * rise + 1.0 + 3.0 * t, 0.0);
      size = (2.1 + 2.3 * h3) * (0.55 + 1.9 * t);
      // Barely any. The head of the column keeps an ember of the fireball in it
      // for the first second and then it is smoke, which has no light of its
      // own and is only ever as bright as what is shining on it.
      heat = clamp(0.42 - age * 0.85, 0.0, 1.0) * up01;
      alpha = smoothstep(born, born + 0.5, age) * (1.0 - t) * (1.0 - t * 0.55);
      dust = 0.0;
      soot = 1.0;
    }

    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let upv = cross(rgt, fwd);

    // Keep the card in front of the eye.
    //
    // A camera-facing quad whose corners straddle the eye plane does not simply
    // vanish: two of them project to one side of the screen and two to the
    // other, and what you get is a hard-edged wedge cutting across half the
    // view. With puffs this size — ten metres of radius once the thing is fully
    // open — a blast that lands on top of you tears the screen into triangles.
    //
    // Cutting every puff nearer than its own radius was the first fix and it
    // was wrong: standing in the middle of a fireball, every puff is nearer
    // than its radius, so the fire went out exactly when you were inside it,
    // which is the one moment it should be everything you can see. Shrinking is
    // the honest version. A card is never allowed to be wider than its distance
    // from the eye, so it can approach without ever crossing, and what you get
    // walking into a fireball is fire that fills the screen and then breaks up
    // into the close, small, fast-moving pieces it is actually made of.
    let ahead = dot(centre - u.eye.xyz, fwd);
    if (ahead < 0.20) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }
    size = min(size, ahead * 0.92);

    let world = centre + (rgt * cx + upv * cy) * size;

    o.world = world;
    o.clip = u.vp * vec4f(world, 1.0);
    o.heat = heat;
    o.alpha = alpha;
    o.dust = dust;
    o.soot = soot;
    o.rnd = h2;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.alpha <= 0.0) { discard; }

    // Not a disc. A round sprite is the single clearest tell that a cloud is
    // made of sprites, and the fix costs two sines: push the edge in and out
    // with the angle, by an amount this puff alone knows, so no two of them
    // have the same outline and none of them has a circle's.
    //
    // Scaled by how cold it is. The hot core of a fireball really is close to
    // round — it is a pressure front and the light drowns the detail — and it
    // is smoke, cooled and turbulent, that goes to cauliflower. So the lumps
    // arrive exactly as the fire leaves, which is what the eye expects.
    let ang = atan2(v.uv.y, v.uv.x);
    let lump = 1.0 - (0.17 * sin(ang * 5.0 + v.rnd * 6.2831)
                    + 0.10 * sin(ang * 11.0 - v.rnd * 9.7)) * (1.0 - v.heat);
    let r = length(v.uv) / max(lump, 0.55);
    if (r > 1.0) { discard; }
    let k = r;

    // White at the core, then orange, then a dull red as it cools, then grey.
    // Fire is a black body and this is what one looks like on the way down.
    // Most of it is orange. White is what the very middle of the very first
    // moment looks like and nothing else does — a fireball that is white all
    // over is a light bulb.
    var col = mix(vec3f(0.40, 0.36, 0.34), vec3f(0.68, 0.15, 0.02), clamp(v.heat * 2.2, 0.0, 1.0));
    col = mix(col, vec3f(1.0, 0.46, 0.07), clamp(v.heat * 1.6 - 0.35, 0.0, 1.0));
    col = mix(col, vec3f(1.0, 0.90, 0.72), clamp(v.heat * 3.0 - 2.3, 0.0, 1.0));
    // The surge is dirt, not smoke: warmer and browner, and it holds that
    // colour the whole way out. Grey dust reads as more of the same fire; the
    // hillside's own colour is what says the ground has been thrown.
    col = mix(col, mix(vec3f(0.30, 0.24, 0.19), vec3f(0.52, 0.44, 0.36), k), v.dust * (1.0 - v.heat));
    // And the column is soot — much darker than either, and darkest in the
    // middle where there is most of it to look through. A pale smoke column is
    // a cloud; what stands over a burning thing is nearly black, and reads as
    // black against a bright sky and as a shape against a dark one.
    col = mix(col, mix(vec3f(0.075, 0.068, 0.064), vec3f(0.30, 0.27, 0.25), k * k),
              v.soot * (1.0 - v.heat));

    // Hard little sun while it burns, soft blob once it is smoke — and the
    // dust is soft from the start, because it never was a sun.
    let core = pow(1.0 - k * k, 2.0) + 0.8 * exp(-k * k * 6.0);
    let soft = (1.0 - k * k) * 0.5;
    let glow = mix(mix(soft, core, v.heat), soft * 0.85, max(v.dust, v.soot))
             * (0.30 + 0.70 * v.heat + 0.45 * v.dust + 0.55 * v.soot);

    // And the flash: everything at once, over in a twentieth of a second. It is
    // what makes an explosion read as an explosion rather than as a fire.
    let flash = exp(-v.age * 30.0) * (1.0 - k * k) * (1.0 - v.dust) * (1.0 - v.soot);

    let view = v.world - u.eye.xyz;
    // Not named `lit`: that is a function in `common.wgsl`, and a local of the
    // same name shadows it in a way the compiler will not have.
    // Premultiplied alpha rather than additive, for the same reason the trail
    // is: a hundred puffs on top of one another sum to a white blob with a soft
    // edge, which is not an explosion, it is a headlight. Blended, they
    // converge on the fire's own colour however many of them there are.
    let clear = 1.0 - fogAt(v.world, length(view));
    let a = clamp(glow * v.alpha * mix(0.9, 0.62, max(v.dust, v.soot * 0.75)), 0.0, 1.0) * clear;
    let col2 = col * (0.55 + 2.6 * v.heat) + vec3f(1.0, 0.94, 0.86) * flash * 3.4;
    return vec4f(col2 * a, a);
  }
