  // What a fireball leaves where it lands.
  //
  // A real explosion is three things happening on three different clocks, and
  // getting the clocks right matters more than any amount of detail in the
  // sprites. First the flash: white, everywhere at once, gone in a twentieth of
  // a second. Then the fireball itself, which does not fly outward so much as
  // boil — it grows fast and then almost stops, because it is being slowed by
  // the air it is pushing. Then the smoke, which outlives both, keeps rising
  // after the fire has gone out, and spreads as it climbs.
  //
  // So the radius here goes as a root rather than a line, the colour walks from
  // white through orange to a dull red and then to grey, and the whole thing is
  // built out of a couple of dozen puffs thrown onto a hemisphere by the seed —
  // a hemisphere, not a sphere, because the ground is in the way.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) uv: vec2f,
    @location(2) heat: f32,       // 1 at the heart of it, 0 out at the edge
    @location(3) age: f32,        // seconds since it landed
    @location(4) alpha: f32,
  }

  const PUFFS = 26u;
  const LIFE  = 2.70;             // seconds before there is nothing left
  const REACH = 7.20;             // metres the fire gets to

  fn bhash(n: f32) -> f32 { return fract(sin(n * 127.1 + 311.7) * 43758.5453); }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3f, @location(1) seed: f32, @location(2) age: f32,
  ) -> VOut {
    let q = vi % 6u;
    let n = f32(vi / 6u);
    let cx = select(-1.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cy = select(-1.0, 1.0, q == 2u || q == 4u || q == 5u);

    var o: VOut;
    o.uv = vec2f(cx, cy);
    o.age = age;
    o.world = pos;
    o.heat = 0.0;
    o.alpha = 0.0;
    if (age < 0.0 || age > LIFE) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    // Where this puff sits in the ball. Two hashes give a direction on the
    // hemisphere and a third how far out it is — nearer the middle for most of
    // them, so the thing has a body rather than being a shell.
    let h1 = bhash(seed * 31.0 + n);
    let h2 = bhash(seed * 57.0 + n * 3.7);
    let h3 = bhash(seed * 83.0 + n * 9.1);
    let a = h1 * 6.2831;
    let up = 0.15 + 0.85 * h2 * h2;          // biased along the ground
    let flat = sqrt(max(1.0 - up * up, 0.0));
    let dir = vec3f(cos(a) * flat, up, sin(a) * flat);
    let depth = 0.25 + 0.75 * h3;

    // The fire's own growth: fast, then almost stopped. And the smoke keeps
    // going up long after the radius has settled.
    let t = age / LIFE;
    let grow = sqrt(clamp(age / 0.42, 0.0, 1.0));
    let rise = vec3f(0.0, 1.0, 0.0) * t * t * 4.4 * (0.4 + 0.6 * h2);
    let centre = pos + dir * (REACH * depth * grow) + rise;

    // Puffs open out as they cool, the way smoke does.
    let size = (0.85 + 0.60 * h3) * (0.55 + 1.6 * t);

    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let upv = cross(rgt, fwd);
    let world = centre + (rgt * cx + upv * cy) * size;

    o.world = world;
    o.clip = u.vp * vec4f(world, 1.0);
    // Deeper puffs stay hot longer: the middle of a fireball is the last part
    // of it to go out.
    o.heat = clamp(1.35 - depth - t * 1.5, 0.0, 1.0);
    o.alpha = (1.0 - t) * (1.0 - t);
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

    // Hard little sun while it burns, soft blob once it is smoke.
    let core = pow(1.0 - k * k, 2.0) + 0.8 * exp(-k * k * 6.0);
    let soft = (1.0 - k * k) * 0.5;
    let glow = mix(soft, core, v.heat) * (0.30 + 0.70 * v.heat);

    // And the flash: everything at once, over in a twentieth of a second. It is
    // what makes an explosion read as an explosion rather than as a fire.
    let flash = exp(-v.age * 34.0) * (1.0 - k * k);

    let view = v.world - u.eye.xyz;
    // Not named `lit`: that is a function in `common.wgsl`, and a local of the
    // same name shadows it in a way the compiler will not have.
    // Premultiplied alpha rather than additive, for the same reason the trail
    // is: twenty-six puffs on top of one another sum to a white blob with a
    // soft edge, which is not an explosion, it is a headlight. Blended, they
    // converge on the fire's own colour however many of them there are.
    let clear = 1.0 - fogAt(v.world, length(view));
    let a = clamp(glow * v.alpha * 0.9, 0.0, 1.0) * clear;
    let col2 = col * (0.55 + 2.6 * v.heat) + vec3f(1.0, 0.94, 0.86) * flash * 3.0;
    return vec4f(col2 * a, a);
  }
