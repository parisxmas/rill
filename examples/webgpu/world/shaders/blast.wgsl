  // What a fireball leaves where it lands.
  //
  // A ground burst is not one cloud. It is three things on three clocks in two
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
  //   THE BASE SURGE, which is the one that was missing. When a fireball hits
  //   ground it cannot expand downward, so the energy goes sideways: a low
  //   ring of dirt and dust that runs out along the surface far further and far
  //   faster than the fireball ever climbs, and hangs about long after the fire
  //   is out. It is the most recognisable part of a ground burst and the least
  //   often drawn. Half the puffs here are it.
  //
  // So the radius goes as a root rather than a line, the colour walks from
  // white through orange to a dull red and then to grey — except down in the
  // surge, which was never on fire and is the colour of the hillside it tore
  // up.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) uv: vec2f,
    @location(2) heat: f32,       // 1 at the heart of it, 0 out at the edge
    @location(3) age: f32,        // seconds since it landed
    @location(4) alpha: f32,
    @location(5) dust: f32,       // 1 for the surge, 0 for the fireball
  }

  const PUFFS = 56u;
  const SURGE = 24u;              // the first this many are the ground wave
  const LIFE  = 4.60;             // seconds before there is nothing left
  const REACH = 12.5;             // metres the fireball gets to
  const RUN   = 27.0;             // metres the surge runs along the ground

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

    if (idx < SURGE) {
      // ---- the base surge
      //
      // Out along the ground, fast at first and then coasting: a root, not a
      // line, because it is spending its speed on the air in front of it. It
      // reaches more than twice as far as the fireball and takes the whole life
      // to do it.
      dust = 1.0;
      let depth = 0.30 + 0.70 * h3;
      let run = pow(clamp(age / 1.30, 0.0, 1.0), 0.55) * RUN * depth
              + max(age - 1.30, 0.0) * 0.9;
      // Barely off the ground, and lifting only as it slows — dust rises when
      // it stops being driven, not while it is.
      let lift = 0.35 + 2.6 * t * t * (0.5 + h2);
      centre = pos + horiz * run + vec3f(0.0, lift, 0.0);
      size = (1.5 + 2.2 * h3) * (0.45 + 2.3 * t);
      // Only the very first moment of it is hot: this is thrown ground, not
      // flame, and it takes its colour from the hillside.
      heat = clamp(0.55 - age * 2.2, 0.0, 1.0);
      // Comes in over the first fifth of a second rather than existing at once,
      // then outlives the fire.
      alpha = smoothstep(0.0, 0.16, age) * (1.0 - t) * (1.0 - t * 0.35);
    } else {
      // ---- the fireball, rolling
      //
      // The arc each puff runs: out, up, and back in over the top. `roll` is
      // where it has got to. Under a right angle it is still opening out; past
      // one it is coming in again, which is the underside of the cap folding
      // under, and it is what a mushroom is.
      let depth = 0.32 + 0.68 * h3;
      let roll = clamp(age / 1.45, 0.0, 1.15) * 2.15 * (0.75 + 0.5 * h2);
      let grow = sqrt(clamp(age / 0.38, 0.0, 1.0));
      let out = sin(roll);
      let up = 1.0 - cos(roll);
      centre = pos + horiz * (REACH * depth * grow * out)
             + vec3f(0.0, REACH * 0.78 * depth * grow * up + t * t * 5.2, 0.0);
      size = (0.95 + 0.85 * h3) * (0.55 + 1.9 * t);
      // The middle of a fireball is the last part of it to go out, and the
      // parts that have climbed highest have had the longest to cool.
      heat = clamp(1.40 - depth - t * 1.7 - up * 0.22, 0.0, 1.0);
      alpha = (1.0 - t) * (1.0 - t);
      dust = 0.0;
    }

    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let upv = cross(rgt, fwd);
    let world = centre + (rgt * cx + upv * cy) * size;

    o.world = world;
    o.clip = u.vp * vec4f(world, 1.0);
    o.heat = heat;
    o.alpha = alpha;
    o.dust = dust;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    if (v.alpha <= 0.0) { discard; }
    let r = length(v.uv);
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

    // Hard little sun while it burns, soft blob once it is smoke — and the
    // dust is soft from the start, because it never was a sun.
    let core = pow(1.0 - k * k, 2.0) + 0.8 * exp(-k * k * 6.0);
    let soft = (1.0 - k * k) * 0.5;
    let glow = mix(mix(soft, core, v.heat), soft * 0.85, v.dust) * (0.30 + 0.70 * v.heat + 0.45 * v.dust);

    // And the flash: everything at once, over in a twentieth of a second. It is
    // what makes an explosion read as an explosion rather than as a fire.
    let flash = exp(-v.age * 30.0) * (1.0 - k * k) * (1.0 - v.dust);

    let view = v.world - u.eye.xyz;
    // Not named `lit`: that is a function in `common.wgsl`, and a local of the
    // same name shadows it in a way the compiler will not have.
    // Premultiplied alpha rather than additive, for the same reason the trail
    // is: fifty puffs on top of one another sum to a white blob with a soft
    // edge, which is not an explosion, it is a headlight. Blended, they
    // converge on the fire's own colour however many of them there are.
    let clear = 1.0 - fogAt(v.world, length(view));
    let a = clamp(glow * v.alpha * mix(0.9, 0.62, v.dust), 0.0, 1.0) * clear;
    let col2 = col * (0.55 + 2.6 * v.heat) + vec3f(1.0, 0.94, 0.86) * flash * 3.4;
    return vec4f(col2 * a, a);
  }
