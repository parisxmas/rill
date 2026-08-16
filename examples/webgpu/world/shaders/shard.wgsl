  // What is left of a crystal. The same prism the crystals are drawn from, an
  // eighth the size, tumbling on all three axes on the way down and lying
  // still once it has landed — Rill zeroes the spin along with the velocity
  // when a shard reaches the ground, so "at rest" needs no flag here.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }

  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f,
    @location(2) ipos: vec3f, @location(3) rot: vec3f,
    @location(4) scale: f32, @location(5) icol: vec3f,
  ) -> VOut {
    // Three axes, composed in the order they are written. Cheap enough at a
    // couple of hundred instances that a matrix in the buffer would cost more
    // bandwidth than it saved arithmetic.
    let cx = cos(rot.x); let sx = sin(rot.x);
    let cy = cos(rot.y); let sy = sin(rot.y);
    let cz = cos(rot.z); let sz = sin(rot.z);
    let Rx = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, cx, sx), vec3f(0.0, -sx, cx));
    let Ry = mat3x3f(vec3f(cy, 0.0, -sy), vec3f(0.0, 1.0, 0.0), vec3f(sy, 0.0, cy));
    let Rz = mat3x3f(vec3f(cz, sz, 0.0), vec3f(-sz, cz, 0.0), vec3f(0.0, 0.0, 1.0));
    let M = Rz * Ry * Rx;

    let p = M * (pos * scale) + ipos;
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = M * n;
    o.color = icol;
    // A shard whose scale is zero is an empty slot: send it past the far plane
    // rather than branching, which is the same thing the tracers do.
    if (scale < 0.0001) { o.clip = vec4f(0.0, 0.0, 2.0, 1.0); }
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    // Lit like the crystal it broke off, and glowing like it too, but less:
    // a chip lying in the grass is not a standing crystal, and the light in
    // these things evidently keeps some of its own once it is loose.
    let base = lit(v.color, normalize(v.normal), v.world);
    return vec4f(base + v.color * 0.34, 1.0);
  }
