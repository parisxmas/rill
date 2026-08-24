  // The pack's flame.
  //
  // One quad per player who is burning, hung from the small of the back and
  // falling away downwards — the exhaust, not the pack. Additive and without
  // depth writing, like the crystals' glow, because a flame is light rather
  // than a surface: what is behind it should come through it.
  //
  // The shape is not a texture. A jet of burning gas is a cone that narrows
  // as it slows, wobbles across its own axis, and comes apart into loose fire
  // at the tail; all three of those are cheaper to say in arithmetic than to
  // draw once and stretch. The instance carries where it starts, how hard it
  // is burning, and a seed — so two people flying side by side do not flicker
  // in step, which is the thing that gives a shader away.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) local: vec2f,   // x across the tongue, y down it: 0 nozzle, 1 tail
    @location(1) seed: f32,
    @location(2) heat: f32,
  }

  const REACH = 1.9;             // how far down the tongue goes, in metres
  const WIDE  = 0.34;            // and how far across at its widest

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(2) ipos: vec3f, @location(3) power: f32,
    @location(4) seed: f32, @location(5) icol: vec3f,
  ) -> VOut {
    // The quad: x from minus a half to a half across it, y from nought at the
    // nozzle to one at the tail. The two-and-a-half trick the other
    // billboards use gives nought or two per axis, so it is halved first —
    // written out because a quad that is a half-width off to one side and
    // twice as long as it should be still draws, and looks like a flame that
    // missed.
    let q = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) * 0.5;
    let corner = vec2f(q.x - 0.5, q.y);
    // Facing the camera about the vertical, which is what the glow does and
    // what is right here too: a flame going straight down has no orientation
    // of its own to give away, and the eye is never above it.
    let yaw = u.cam.x;
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    let world = ipos
      + right * corner.x * (WIDE * 2.0)
      - vec3f(0.0, corner.y * REACH * power, 0.0);
    var o: VOut;
    o.clip = u.vp * vec4f(world, 1.0);
    o.local = vec2f(corner.x * 2.0, corner.y);
    o.seed = seed;
    o.heat = power;
    return o;
  }

  fn jetNoise(a: f32) -> f32 {
    return fract(sin(a * 12.9898 + 78.233) * 43758.5453);
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let t = clamp(v.local.y, 0.0, 1.0);
    let clock = u.eye.w;
    // Two lean terms at different rates: one slow enough to see the tongue
    // sway, one fast enough to be turbulence rather than a wave. Both grow
    // with `t`, because the nozzle end is held and the tail is not.
    let sway = sin(clock * 9.0 + v.seed * 7.3 + t * 5.0) * 0.30 * t
             + sin(clock * 27.0 - v.seed * 3.1 + t * 13.0) * 0.14 * t * t;
    let x = v.local.x - sway;

    // How far down it is burning this instant. A jet does not end at a fixed
    // length: it gutters, and the gutter is most of what makes it read as
    // fire rather than as a painted cone.
    let flick = 0.74 + 0.26 * sin(clock * 31.0 + v.seed * 11.0)
                     + 0.10 * jetNoise(floor(clock * 40.0) + v.seed);
    let reach = clamp(1.0 - t / max(flick, 0.05), 0.0, 1.0);

    // The tongue: widest a third of the way down, closing to nothing at the
    // tail — a cone that has been squeezed at the nozzle by the throat it
    // came out of.
    let mouth = smoothstep(0.0, 0.16, t + 0.04);
    let halfw = pow(reach, 0.62) * mouth;
    let across = 1.0 - clamp(abs(x) / max(halfw, 0.0001), 0.0, 1.0);
    if (across <= 0.0) { return vec4f(0.0); }
    let body = across * across;

    // White at the throat, yellow, orange, and a red that gives out. The core
    // is what is left of `across` at its very middle, so the flame has a
    // bright line down it rather than being one flat colour with a soft edge.
    let core = pow(across, 6.0) * (1.0 - t);
    let col = mix(mix(vec3f(1.00, 0.86, 0.42), vec3f(1.00, 0.42, 0.06), smoothstep(0.05, 0.55, t)),
                  vec3f(0.55, 0.09, 0.02), smoothstep(0.5, 1.0, t))
            + vec3f(1.0, 0.95, 0.85) * core * 0.9;

    // Fading out along its length, and the whole thing scaled by how hard the
    // pack is going — which is one at full thrust and nothing when it is out,
    // so the flame dies with the tank rather than being switched off.
    let fade = body * (1.0 - t * 0.85) * v.heat;
    // Nine tenths rather than a third more. Seen from a metre — which is
    // where your own is, looking down past your feet — anything above this
    // blows out to a white sheet and stops being a flame.
    return vec4f(col * fade * 0.9, 1.0);
  }
