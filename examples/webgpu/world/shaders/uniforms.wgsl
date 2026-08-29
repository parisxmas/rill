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
    // `akMin.w` is not a bound either: it is how far through a magazine change
    // the hands are, nought when there is none, which is what animates the
    // magazine out of the well and a new one back into it.
    akMin:      vec4f,
    akSize:     vec4f,
    // Things that are not anybody's bounding box.
    //
    //   x  the muzzle flash, one at the shot and gone in eighty milliseconds
    //
    // The flash used to be the recoil raised to the seventh — one number
    // serving both, the faster of the two given the steeper curve — and that
    // held only while every weapon kicked the same amount. The AK damps its
    // model kick so the barrel stays in frame, and a half raised to the
    // seventh is a hundred and twenty-eighth: its muzzle flash went out.
    // The flash has its own number now and what a weapon does with the kick
    // is no longer any of its business.
    //
    //   y  how far down on one knee the body is — the weapon ducks with it
    //   z  which tool is up: 1 the pick, 2 a stone, 0 neither (pick.wgsl)
    //   w  how far along its own barrel the weapon's muzzle is, in the model
    //      coordinates that weapon is unquantised into — see `muzzle.wgsl`.
    //      A constant would do for one gun and there are three, and they are
    //      not the same length: the flash sat a hand past the MG3's barrel,
    //      which from behind the sights is a flash floating on the crosshair.
    fx:         vec4f,
    // The lamp on this player's head: x is one while it is lit and nought
    // while it is not, and it starts at nought. It used to come on by itself
    // once the sun went under the world, which meant every player carried the
    // same light and nobody could turn theirs off to stop being the brightest
    // thing on a dark hillside. `F` is the switch now. y and z carry the
    // muzzle's height and its offset across the bore, which go with `fx.w`
    // above; w is the rounds left on the MG3's belt, for `mgfx.wgsl`.
    lamp:       vec4f,
    // And everybody else's, six at a time: two vectors each, `xyz` where the
    // lamp is with `w` saying whether it is lit, then `xyz` which way it is
    // pointed. Six because that is what a light in a shared valley is worth —
    // the beams are gathered nearest-first in host.js and the seventh player
    // in a heap simply does not cast one. A slot with `w` at nought costs the
    // fragment a compare and nothing else, so an empty valley pays nothing.
    lamps:      array<vec4f, 12>,
  }
  @group(0) @binding(0) var<uniform> u: U;

  // The pine twig, and the sampler for it. Declared here rather than in
  // `common.wgsl` because the shadow pass needs them too: a card that is a
  // hole in its texture has to be a hole in the shadow map as well.
  @group(0) @binding(3) var foliage: texture_2d<f32>;
  @group(0) @binding(4) var foliageSamp: sampler;
