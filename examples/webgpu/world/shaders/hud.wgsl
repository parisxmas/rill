  // The HUD, as the engine laid it out.
  //
  // One instanced draw over everything: each instance is a rectangle in CSS
  // pixels with a kind, and the kinds are the handful of things a HUD is made
  // of — a solid, a glyph out of the atlas, the scope's vignette, a ring, a
  // gradient bar, the overlays' dim, a rocket-shaped pip. The layout came out
  // of wasm; nothing here decides where anything goes.
  //
  // Its own uniform, because the world's is about the world: just the screen
  // in CSS pixels and the atlas in texels.

  struct HudU {
    screen: vec4f,   // w, h in CSS px; z, w spare
    atlas:  vec4f,   // atlas w, h in texels
  }
  @group(0) @binding(0) var<uniform> hu: HudU;
  @group(0) @binding(1) var hudAtlas: texture_2d<f32>;
  @group(0) @binding(2) var hudSamp: sampler;

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) @interpolate(flat) kind: f32,
    @location(3) local: vec2f,          // 0..1 across the instance rect
    @location(4) @interpolate(flat) p: vec3f,
    @location(5) @interpolate(flat) rect: vec4f,
  }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) xywh: vec4f,
    @location(1) uvwh: vec4f,
    @location(2) color: vec4f,
    @location(3) kp: vec4f,
  ) -> VOut {
    let q = vi % 6u;
    let cx = select(0.0, 1.0, q == 1u || q == 2u || q == 4u);
    let cy = select(0.0, 1.0, q == 2u || q == 4u || q == 5u);

    let px = xywh.x + cx * xywh.z;
    let py = xywh.y + cy * xywh.w;

    var o: VOut;
    o.clip = vec4f(px / hu.screen.x * 2.0 - 1.0, 1.0 - py / hu.screen.y * 2.0, 0.0, 1.0);
    // Glyph instances carry the atlas cell's top-left in texels in uv, and
    // its size rides in the rect: the cell is exactly the quad, so the local
    // corner scaled by the quad's own size in texels walks the cell.
    o.uv = uvwh.xy + vec2f(cx, cy) * uvwh.zw;
    o.color = color;
    o.kind = kp.x;
    o.local = vec2f(cx, cy);
    o.p = kp.yzw;
    o.rect = xywh;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let k = u32(v.kind + 0.5);

    // A glyph. The atlas carries two facts per texel — the letter in the red
    // channel and its baked shadow in the alpha — so the text arrives with
    // the same dark halo the CSS text-shadow gave it, whatever is behind.
    if (k == 1u) {
      let t = textureSampleLevel(hudAtlas, hudSamp, v.uv / hu.atlas.xy, 0.0);
      let glyph = t.a * t.r;
      let shadow = t.a * (1.0 - t.r);
      // The halo, at full strength rather than a little under it. It is the
      // only thing separating a letter from whatever it is lying over, and
      // what it is lying over is a valley — hillsides in full sun as often as
      // the near-black the palette was picked against.
      let a = v.color.a * (glyph + shadow);
      return vec4f(v.color.rgb * glyph * v.color.a, a);
    }

    // The scope: clear inside the sight picture, black past it, with the
    // CSS gradient's own stops — p.x is the radius in px, p.y a vmin.
    if (k == 2u) {
      let c = (v.local - 0.5) * v.rect.zw;
      let d = length(c);
      let r = v.p.x;
      let vmin = v.p.y;
      var a = 0.0;
      if (d > r + 12.0 * vmin) { a = 1.0; }
      else if (d > r + 6.0 * vmin) { a = mix(0.97, 1.0, (d - r - 6.0 * vmin) / (6.0 * vmin)); }
      else if (d > r + 1.2 * vmin) { a = mix(0.45, 0.97, (d - r - 1.2 * vmin) / (4.8 * vmin)); }
      else if (d > r) { a = mix(0.0, 0.45, (d - r) / (1.2 * vmin)); }
      let out = a * v.color.a;
      return vec4f(0.0, 0.0, 0.0, out);
    }

    // A ring: the reticle's centre circle, and the roster's dots when p.y is
    // nought (a filled disc is a ring whose hole has no size).
    if (k == 3u || k == 7u) {
      let c = (v.local - 0.5) * v.rect.zw;
      let d = length(c);
      let half = v.rect.z * 0.5;
      var a = 0.0;
      if (k == 7u) { a = 1.0 - smoothstep(half - 1.0, half, d); }
      else {
        let mid = v.p.x + v.p.y * 0.5;
        a = 1.0 - smoothstep(v.p.y * 0.5, v.p.y * 0.5 + 1.0, abs(d - mid));
      }
      let out = v.color.a * a;
      return vec4f(v.color.rgb * out, out);
    }

    // A horizontal gradient — the health and breath bars. The second colour
    // rides in the params.
    if (k == 4u) {
      let c = mix(v.color.rgb, v.p, v.local.x);
      return vec4f(c * v.color.a, v.color.a);
    }

    // The overlays' dim: the CSS radial-gradient, darker at the edges.
    if (k == 5u) {
      let c = (v.local - 0.5) * 2.0;
      let a = mix(0.62, 0.92, clamp(length(c), 0.0, 1.0)) * v.color.a / 0.8;
      return vec4f(v.color.rgb * a, a);
    }

    // A rocket pip: the CSS clip-path — a body with a nose that starts a
    // third of the way down.
    if (k == 6u) {
      let nose = 0.32;
      var inside = 1.0;
      if (v.local.y < nose) {
        let spread = v.local.y / nose * 0.5;
        if (abs(v.local.x - 0.5) > spread) { inside = 0.0; }
      }
      let out = v.color.a * inside;
      return vec4f(v.color.rgb * out, out);
    }

    // A solid.
    return vec4f(v.color.rgb * v.color.a, v.color.a);
  }
