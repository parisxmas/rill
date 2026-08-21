  // The rocket crates lying about the valley.
  //
  // The ammunition box's pipeline, read again with the crate's own mesh and
  // photograph: Poly Haven's `old_military_crate`, CC0, eighty centimetres of
  // olive wood and webbing, from the same collection the box and the rifle
  // came out of. It rides the same shard-buffer instances the boxes do — the
  // draw takes the back range of the first sixteen slots, the boxes take the
  // front — so there is nothing here the prop shader does not already say,
  // except which quantization box unpacks the vertices and which photograph
  // wraps them.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) quv: vec2f,
    @location(3) ipos: vec3f, @location(4) rot: vec3f, @location(5) scale: f32,
  ) -> VOut {
    let m = u.crateMin.xyz + qpos.xyz * u.crateSize.xyz;
    let c = cos(rot.y);
    let s = sin(rot.y);
    let M = mat3x3f(vec3f(c, 0.0, -s), vec3f(0.0, 1.0, 0.0), vec3f(s, 0.0, c));
    let p = M * (m * scale) + ipos;

    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    o.normal = M * qn.xyz;
    o.uv = quv;
    // Taken, and not back yet — past the far plane, like the boxes.
    if (scale < 0.0001) { o.clip = vec4f(0.0, 0.0, 2.0, 1.0); }
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    // The right half of the boxes' shared sheet; see `prop.wgsl` for the left.
    let albedo = textureSample(propTex, barkSamp, v.uv * vec2f(0.5, 1.0) + vec2f(0.5, 0.0)).rgb;
    let n = normalize(v.normal);
    let col = lit(albedo, n, v.world);
    let view = v.world - u.eye.xyz;
    return vec4f(mix(col, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
