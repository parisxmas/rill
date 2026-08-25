  // The sky: one triangle over the far plane, and a ray for every pixel of it.
  //
  // The ray used to be built in the fragment shader — a cosine and two sines of
  // the camera's own angles, a cross product and a tangent, per pixel. Every
  // one of those is the same number for the whole screen, and the screen at the
  // top tier is ten million pixels: the camera's basis was being recomputed ten
  // million times a frame to arrive at the same answer.
  //
  // It belongs in the vertex shader, and it can be moved there without changing
  // a pixel, because of what the ray actually is. Before it is normalized it is
  // `fwd + right * x + up * y` — linear in the screen position, and linear is
  // exactly what interpolation across a triangle preserves. So the three
  // corners work out their own unnormalized rays, the hardware interpolates
  // between them for free, and the fragment does the one part that is not
  // linear: the normalize.
  struct VOut { @builtin(position) clip: vec4f, @location(0) ray: vec3f }

  @vertex fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let xy = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    let yaw = u.cam.x; let pitch = u.cam.y;
    let cp = cos(pitch);
    let fwd = vec3f(sin(yaw) * cp, sin(pitch), -cos(yaw) * cp);
    let right = vec3f(cos(yaw), 0.0, sin(yaw));
    let upv = cross(right, fwd);
    let th = tan(0.53);
    var o: VOut;
    o.clip = vec4f(xy, 1.0, 1.0);   // on the far plane
    o.ray = fwd + right * xy.x * th * u.sun.w + upv * xy.y * th;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    return vec4f(skyColor(normalize(v.ray), u.eye.w), 1.0);
  }
