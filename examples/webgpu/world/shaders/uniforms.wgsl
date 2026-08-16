  struct U {
    vp:   mat4x4f,
    lvp:  mat4x4f,
    eye:  vec4f,   // xyz eye, w time
    sun:  vec4f,   // xyz light direction, w aspect
    cam:  vec4f,   // yaw, pitch, wind angle, wind strength
    wind: vec4f,   // x: the wind's own phase, integrated — see `frame`
  }
  @group(0) @binding(0) var<uniform> u: U;
