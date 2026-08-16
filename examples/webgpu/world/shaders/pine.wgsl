  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
    @location(3) uv: vec2f,
  }
  @vertex fn vs(
    @location(0) pos: vec3f, @location(1) n: vec3f, @location(6) uv: vec2f,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    let c = cos(rot); let s = sin(rot);
    var p = vec3f(pos.x * c - pos.z * s, pos.y, pos.x * s + pos.z * c) * scale + ipos;
    let hgt = max(pos.y, 0.0) / 3.6;
    let dirn = vec2f(cos(u.cam.z), sin(u.cam.z));
    let flutter = sin(u.wind.x * 1.6 + ipos.x * 1.31 + ipos.z * 0.97);
    let sway = (u.cam.w * hgt * hgt * 0.45 + flutter * 0.09 * u.cam.w * hgt) * scale;
    p = vec3f(p.x + dirn.x * sway, p.y, p.z + dirn.y * sway);
    let nr = vec3f(n.x * c - n.z * s, n.y, n.x * s + n.z * c);
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p; o.normal = nr; o.color = icol; o.uv = uv;
    return o;
  }
  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    // Both texture lookups happen before any branch: WGSL asks for them in
    // uniform control flow, because a sampler needs the derivatives of its
    // coordinate and those come from the neighbouring fragments.
    let t = textureSample(foliage, foliageSamp, v.uv);
    // The trunk's u was shifted into [-2, -1] to mark it; shift it back, and
    // tile the bark twice around and four times up the trunk.
    let b = textureSample(barkTex, barkSamp, (v.uv + vec2f(2.0, 0.0)) * vec2f(2.0, 4.0));
    let trunk = v.uv.x < 0.0;
    // Both atlases carry their own colour; the instance's tint keeps one tree
    // from looking exactly like the next.
    // Bark at dusk is a vertical surface facing away from a low sun, which is
    // to say black. A little wrap — light that reaches round the trunk — is
    // what keeps it reading as wood rather than as a hole in the forest.
    let bark = b.rgb * (1.15 + v.color.g * 0.7);
    let albedo = select(t.rgb * (0.7 + v.color * 1.5), bark, trunk);
    // Cards are drawn from both sides, so a trunk's far wall arrives with its
    // normal pointing away and would be lit as if it were the inside of the
    // tree. Foliage keeps its up-biased normal either way — a needle looks
    // like a needle from underneath.
    let nn = normalize(v.normal);
    let n = select(nn, select(-nn, nn, front), trunk);
    var col = lit(albedo, n, v.world);
    if (trunk) { col += albedo * 0.10; }
    if (!trunk && t.a < 0.5) { discard; }
    return vec4f(col, 1.0);
  }
