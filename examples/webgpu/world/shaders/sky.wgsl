  struct VOut { @builtin(position) clip: vec4f, @location(0) ndc: vec2f }
  @vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let xy = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: VOut;
    o.clip = vec4f(xy, 1.0, 1.0);   // on the far plane
    o.ndc = xy;
    return o;
  }
  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    let upv = cross(right, fwd);
    let th = tan(0.53);
    let dir = normalize(fwd + right * v.ndc.x * th * u.sun.w + upv * v.ndc.y * th);
    return vec4f(skyColor(dir, u.eye.w), 1.0);
  }
