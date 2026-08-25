  // The pick in your own hands: Kenney's, out of the Survival Kit (CC0),
  // baked to the rifle's sixteen-byte vertex with one change — the texture
  // slot carries a colour. Every sampled binding this renderer has is spent,
  // so the kit's colormap was read once at bake time instead: it is a palette
  // of flat patches, and a flat patch sampled per-vertex arrives whole.
  //
  // The swing is not an animation anybody made. The model's own +Y runs up
  // the handle, so the whole tool is one rotation about the grip — at rest a
  // carried lean, and on `u.wind.w` (the same decaying kick every weapon's
  // recoil rides, written 0.8 by the hand when the pick lands) it whips
  // forward and down and comes back up as the number dies.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }

  const PICK_MIN  = vec3f(-0.0893, 0.0, -0.0194);
  const PICK_SIZE = vec3f(0.1786, 0.24, 0.0387);
  const PICK_SCALE = 0.92;

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) col: vec4f,
  ) -> VOut {
    var m = (PICK_MIN + qpos.xyz * PICK_SIZE) * PICK_SCALE;
    var n = qn.xyz;
    // Seventy degrees about the haft first, so the head points down the swing
    // — at what is being dug — while keeping a sliver of its profile in view.
    // A full quarter turn put it exactly edge-on and the head read as a lump;
    // twenty degrees short of it is where it stays legibly a pick.
    let ch = 0.34202; let sh = 0.93969;
    m = vec3f(m.x * ch + m.z * sh, m.y, -m.x * sh + m.z * ch);
    n = vec3f(n.x * ch + n.z * sh, n.y, -n.x * sh + n.z * ch);
    // The kit authors the head across x and the haft up y. The head stays in
    // the screen plane — no turn about the handle's own axis, which is what
    // put the head at a false angle to the haft: any rotation that carries
    // the head into depth reads, through perspective, as the joint being
    // wrong rather than the tool being turned.
    //
    // So the pose is two rotations that both keep x in view. The swing
    // pitches about the camera's x — forward and down on `r`, the hand's
    // decaying kick — and a fixed roll about the camera's z lays the haft
    // across the frame, low right to high left, which is how a pick is
    // actually carried.
    let r = u.wind.w;
    let lean = 0.30 + 1.35 * r;
    let cl = cos(lean); let sl = sin(lean);
    m = vec3f(m.x, m.y * cl - m.z * sl, m.y * sl + m.z * cl);
    n = vec3f(n.x, n.y * cl - n.z * sl, n.y * sl + n.z * cl);
    let tilt = 0.34 + 0.22 * r;
    let ct = cos(tilt); let st = sin(tilt);
    m = vec3f(m.x * ct - m.y * st, m.x * st + m.y * ct, m.z);
    n = vec3f(n.x * ct - n.y * st, n.x * st + n.y * ct, n.z);
    // The grip, low right, pulled in a touch as it swings.
    let grip = vec3f(0.15 - 0.04 * r, -0.19 + 0.01 * r, 0.30 - 0.03 * r);
    let cam = m + grip;

    var o: VOut;
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(normalize(n));
    o.color = col.rgb;
    return o;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let nn = normalize(v.normal);
    let col = lit(v.color, select(-nn, nn, front), v.world);
    return vec4f(col, 1.0);
  }
