  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) color: vec3f,
    @location(2) uv: vec2f,
    @location(3) normal: vec3f,
  }
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f, @location(6) uv: vec2f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    // Which of the four plants this one is. Height decides it, but loosely:
    // Rill hands out a scale between 0.55 and 1.35, and the taller half is
    // more likely to be a fern or a nettle than a tuft of grass. Loosely,
    // because a meadow sorted strictly by size reads as sorted — so the roll
    // is against the size rather than decided by it, and a big tuft and a
    // small fern both still happen.
    let r = fract(sin(dot(ipos.xz, vec2f(41.7, 289.1))) * 43758.5453);
    let tall = select(0.0, 1.0, r < smoothstep(0.70, 1.25, scale));
    let alt  = select(0.0, 1.0, fract(r * 17.0) < 0.5);
    // Inset a hair: the cells share edges, and a bilinear tap on the boundary
    // would fetch the neighbouring plant.
    let cell = uv * 0.49 + 0.005 + vec2f(alt, tall) * 0.5;

    // What is in a cell is not all the same size in life. The nettle cell
    // holds one leaf, and one leaf at the scale of a clump of grass is a leaf
    // a metre across; the fern is a whole frond and stands taller than either
    // tuft. So the card is sized by which plant it turned out to be.
    let size = scale * select(select(1.0, 1.12, alt > 0.5),
                              select(1.30, 0.62, alt > 0.5), tall > 0.5);

    let c = cos(rot); let s = sin(rot);
    var p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * size + ipos;
    let dirn = vec2f(cos(u.cam.z), sin(u.cam.z));
    let osc = sin(u.wind.x * 1.5 + ipos.x * 0.9 + ipos.z * 0.7);
    let sway = (u.cam.w * 0.20 + osc * (0.05 + 0.16 * u.cam.w)) * pos.y * size;
    p.x += dirn.x * sway; p.z += dirn.y * sway;

    let nr = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.color = icol; o.uv = cell; o.normal = nr;
    return o;
  }
  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let t = textureSample(plants, foliageSamp, v.uv);
    if (t.a < 0.5) { discard; }
    // The atlas carries the plant's own colour; the instance's tint keeps one
    // clump from looking exactly like the next.
    let albedo = t.rgb * (0.55 + v.color * 1.5);
    // Cards are drawn from both sides, and a leaf lit from underneath is still
    // a leaf, so the up-biased normal is kept either way.
    return vec4f(lit(albedo, normalize(v.normal), v.world), 1.0);
  }
