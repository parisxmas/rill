  // The bottom of a pit that went too deep.
  //
  // One horizontal quad per burned-through cell, sitting a hair over the
  // carved floor. The glow is arithmetic, not a texture: two octaves of the
  // sky's own value noise crawling in opposite directions make the crust, a
  // slow pulse under it makes the melt breathe, and the edges of the quad
  // fade so neighbouring cells run together into one pool instead of tiling.
  //
  // Unlit on purpose. Magma is a light source, not a surface — the sun has no
  // opinion about it — so the fragment writes its own colour and no shading.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) local: vec2f,     // -1..1 across the cell
    @location(1) world: vec2f,     // xz, so the noise is continuous across cells
    @location(2) seed: f32,
  }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) at: vec4f,        // x, y, z, seed
  ) -> VOut {
    let q = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) - 0.5;
    // A cell is a metre; the quad reaches out to 0.85 each way so two magma
    // cells side by side overlap and read as one melt.
    let w = vec3f(at.x + q.x * 1.7, at.y, at.z + q.y * 1.7);
    var o: VOut;
    o.clip = u.vp * vec4f(w, 1.0);
    o.local = q * 2.0;
    o.world = w.xz;
    o.seed = at.w;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let t = u.eye.w;
    // The crust: bright veins where two drifting noise fields disagree.
    let a = skyNoise(v.world * 1.9 + vec2f(t * 0.05, -t * 0.03));
    let b = skyNoise(v.world * 3.7 - vec2f(t * 0.04, t * 0.06));
    let vein = 1.0 - abs(a - b) * 2.2;
    let glow = clamp(vein, 0.0, 1.0);
    // The melt under it, breathing slowly and never in step across the pool.
    let pulse = 0.72 + 0.28 * sin(t * 1.7 + v.seed * 9.0 + a * 4.0);
    // Deep red floor, orange veins, a white-hot thread where they cross hardest.
    var col = vec3f(0.30, 0.03, 0.005)
            + vec3f(1.00, 0.32, 0.03) * glow * glow * pulse
            + vec3f(1.00, 0.85, 0.55) * pow(glow, 6.0) * pulse;
    // Out at the rim, so the pool has a shore rather than a square edge.
    let rim = 1.0 - smoothstep(0.55, 1.0, max(abs(v.local.x), abs(v.local.y)));
    return vec4f(col * rim, rim);
  }
