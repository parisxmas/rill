  // The stone inside the fire.
  //
  // Not a disc with a rock texture on it: a fan of triangles meeting at a
  // centre, each rim point at its own hashed distance, so the outline is angular
  // and every facet catches the light differently. That is what a lump of rock
  // tumbling through the air looks like on film, and it is eleven triangles.
  //
  // A fan is not a solid — it is one face of one — but the thing is spinning
  // inside its own fire and the only silhouette anyone reads is the outline. A
  // real convex hull would cost a vertex buffer and a draw call each, for a
  // shape that is on screen for two seconds at a time.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) uv: vec2f,
    @location(2) local: vec2f,    // where in the fan, for the shading
    @location(3) facet: f32,      // which triangle, so each catches light apart
  }

  const CORE  = 0.60;             // metres, at the widest
  const BLADES = 11u;

  fn mhash(n: f32) -> f32 { return fract(sin(n * 127.1 + 311.7) * 43758.5453); }

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec3f, @location(1) vel: vec3f,
    @location(2) seed: f32, @location(3) life: f32, @location(4) flown: f32,
  ) -> VOut {
    let tri = vi / 3u;              // which blade of the fan
    let corner = vi % 3u;           // 0 the centre, 1 and 2 the rim

    var o: VOut;
    o.facet = f32(tri);
    o.world = pos;
    o.uv = vec2f(0.5);
    o.local = vec2f(0.0);
    // A rocket flies through the same ring — see `launch` in valley.rill — and
    // there is no stone inside it. It has a mesh of its own.
    if (life <= 0.0 || seed < 0.0) {
      o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
      return o;
    }

    // Two rim points, each at its own angle and its own reach. The reaches are
    // hashed off the seed, so every stone is a different stone.
    let i0 = f32(tri);
    let i1 = f32((tri + 1u) % BLADES);
    let step = 6.2831853 / f32(BLADES);
    let r0 = 0.55 + 0.45 * mhash(seed * 41.0 + i0);
    let r1 = 0.55 + 0.45 * mhash(seed * 41.0 + i1);
    // A little jitter on the angles too, or the outline is a regular polygon
    // however much the radii vary.
    let a0 = (i0 + 0.35 * mhash(seed * 71.0 + i0)) * step;
    let a1 = (i1 + 0.35 * mhash(seed * 71.0 + i1)) * step;

    var p = vec2f(0.0, 0.0);
    if (corner == 1u) { p = vec2f(cos(a0), sin(a0)) * r0; }
    if (corner == 2u) { p = vec2f(cos(a1), sin(a1)) * r1; }

    // Turning, at its own rate and from its own starting angle.
    let sp = seed * 6.2831 + u.eye.w * (1.1 + seed * 2.2);
    let cs = cos(sp); let sn = sin(sp);
    let spun = vec2f(p.x * cs - p.y * sn, p.x * sn + p.y * cs);

    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let rgt = vec3f(cos(yaw), 0.0, sin(yaw));
    let up  = cross(rgt, fwd);

    // Ahead of the fire's own head, so the flames stream off behind and around
    // it rather than sitting on top of it.
    let nose = pos + normalize(vel + vec3f(0.0, 0.0001, 0.0)) * 0.34;
    let world = nose + (rgt * spun.x + up * spun.y) * CORE;

    o.world = world;
    o.clip = u.vp * vec4f(world, 1.0);
    o.uv = p * 0.5 + 0.5;           // the rock turns with the stone, not the screen
    o.local = p;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let rock = textureSample(meteorTex, barkSamp, v.uv).rgb;
    let lum = dot(rock, vec3f(0.34, 0.5, 0.16));
    let r = length(v.local);

    // Each blade takes the light a little differently, which is what makes a
    // fan of triangles read as a faceted lump rather than as a paper fan.
    let face = 0.55 + 0.45 * mhash(v.facet * 7.3 + 0.5);

    // Burnt nearly black, because a meteorite's crust is — and then the cracks,
    // which are where the texture is darkest and where the heat gets out.
    // Fissures glow; faces do not. Hotter toward the leading edge.
    let crust = rock * 0.34 * face;
    let crack = pow(clamp(1.0 - lum * 2.4, 0.0, 1.0), 1.8);
    let heat = vec3f(1.0, 0.36, 0.06) * crack * (1.3 + 2.4 * r);

    let view = v.world - u.eye.xyz;
    let col = mix(crust + heat, fogColor(view), fogAt(v.world, length(view)));
    // Solid through the middle and giving out before the rim: some of the fire
    // is in front of it, and a stone in a fire is seen through the fire.
    //
    // And gone entirely within a few metres of the eye. A fan of triangles is
    // one flat face, which is fine at forty metres and reads as a slab of card
    // at two — and at two metres the thing is about to land on you anyway, so
    // what should be filling the screen is the fire, not the stone.
    let near = smoothstep(1.8, 5.5, length(v.world - u.eye.xyz));
    return vec4f(col, (1.0 - smoothstep(0.55, 1.05, r)) * 0.82 * near);
  }
