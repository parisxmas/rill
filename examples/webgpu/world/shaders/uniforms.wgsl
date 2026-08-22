  struct U {
    vp:   mat4x4f,
    lvp:  mat4x4f,
    eye:  vec4f,   // xyz eye, w time
    sun:  vec4f,   // xyz light direction, w aspect
    cam:  vec4f,   // yaw, pitch, wind angle, wind strength
    wind: vec4f,   // x the wind's phase, y the player's speed, z the walk's,
                   // w the recoil left in the rifle
    vmMin:  vec4f, // xyz the rifle's bounding box, which its vertices were
                   // quantized against; w how far through the bolt's throw
    vmSize: vec4f, // the box's size — see `rifle.mesh`
    // The shark: x yaw, y unused, z where the tail is in its beat, w how far
    // the jaw is open. Its position and its own bounding box follow.
    shark:     vec4f,
    sharkAt:   vec4f,
    sharkMin:  vec4f,
    sharkSize: vec4f,
    // The impact that is currently lighting the valley: xyz where it landed,
    // w how many seconds ago. Negative w means nothing is burning. Only one,
    // and the newest — two blasts close enough together to both matter is rare
    // enough that the second one taking over is not something you can see, and
    // a light per blast would cost a loop in every fragment in the world.
    blast: vec4f,
    // The ammunition box's own bounding box, which its vertices were
    // quantized against — see `ammo-box.mesh` and `tools/bake_prop.py`.
    propMin:  vec4f,
    propSize: vec4f,
    // The rocket's, and the launcher's. `rpgMin.w` is not a bound: it is how
    // far through a weapon swap the hands are, nought with a weapon up and one
    // with it out of frame, which both viewmodels read to duck out of the way.
    rocketMin:  vec4f,
    rocketSize: vec4f,
    rpgMin:     vec4f,
    rpgSize:    vec4f,
    // The rocket crate's, for the second kind of box on the ground.
    crateMin:   vec4f,
    crateSize:  vec4f,
    // The AK's, which `ak.wgsl` unpacks against — it has a shader of its own.
    akMin:      vec4f,
    akSize:     vec4f,
  }
  @group(0) @binding(0) var<uniform> u: U;

  // The pine twig, and the sampler for it. Declared here rather than in
  // `common.wgsl` because the shadow pass needs them too: a card that is a
  // hole in its texture has to be a hole in the shadow map as well.
  @group(0) @binding(3) var foliage: texture_2d<f32>;
  @group(0) @binding(4) var foliageSamp: sampler;
