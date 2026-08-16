  struct U {
    vp:   mat4x4f,
    lvp:  mat4x4f,
    eye:  vec4f,   // xyz eye, w time
    sun:  vec4f,   // xyz light direction, w aspect
    cam:  vec4f,   // yaw, pitch, wind angle, wind strength
    wind: vec4f,   // x the wind's phase, y the player's speed, z the walk's,
                   // w the recoil left in the rifle
    vmMin:  vec4f, // the rifle's bounding box, which its vertices were
    vmSize: vec4f, // quantized against — see `rifle.mesh`
  }
  @group(0) @binding(0) var<uniform> u: U;

  // The pine twig, and the sampler for it. Declared here rather than in
  // `common.wgsl` because the shadow pass needs them too: a card that is a
  // hole in its texture has to be a hole in the shadow map as well.
  @group(0) @binding(3) var foliage: texture_2d<f32>;
  @group(0) @binding(4) var foliageSamp: sampler;
