  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec3f,
    @location(2) dist: f32,
  }
  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    let corner = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) - 0.5;
    let yaw = u.cam.x;
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    // Wider than the crystal, but not so wide that the card runs into the
    // hillside. This is a camera-facing quad, and where the terrain cuts
    // through it the cut is a straight line — so a nine-times halo on a slope
    // stopped reading as light in the air and started reading as a hard-edged
    // triangle lying on the ground beside the crystal. The glow was soft; the
    // place it was being sliced was not.
    //
    // Five and a half keeps the halo generous and mostly clear of the slope,
    // and the centre is lifted further off the ground for the same reason.
    let size = scale * 5.5;
    let center = ipos + vec3f(0.0, scale * 1.9, 0.0);
    let world = center + right * corner.x * size + vec3f(0.0, corner.y * size, 0.0);
    var o: VOut;
    o.clip = u.vp * vec4f(world, 1.0);
    o.uv = corner * 2.0; o.color = icol;
    o.dist = length(world - u.eye.xyz);
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let d = length(v.uv);
    let a = max(1.0 - d, 0.0);
    // Two falls, not one. The cube alone gives a tight core with a hard-ish
    // shoulder — recognisably a sprite with a curve on it. A near-linear term
    // reaching all the way to the edge underneath it is what air does with
    // light, and the sum has a bright middle *and* a wide faint surround with
    // no edge you can point at.
    let core = a * a * a;
    let haze = a * a * (0.5 + 0.5 * a);
    let pulse = 0.8 + 0.2 * sin(u.eye.w * 2.1 + v.dist);
    // Brighter at night and pulled back in daylight, because a glow you can
    // see against a lit hillside at noon is a glow that swallows the valley
    // after dark — and this now covers nine times the area it used to.
    let night = 1.0 - 0.55 * dayness();
    // And the haze pulled well down. What is left of the card out at its edge
    // has to be near enough nothing that clipping it against the ground is
    // invisible — the softness has to be in the falloff, not in the size.
    return vec4f(v.color * (core * 0.78 + haze * 0.12) * pulse * night, 1.0);
  }
