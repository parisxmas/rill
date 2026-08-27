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

  // CreativeTrio's pickaxe, out of Poly Pizza, CC0. Baked by the same script
  // as before — and its texture turned out to be a palette too, a nine-by-five
  // grid of flat swatches, so the colour still rides in the vertices and no
  // image ships. There is nothing in that file a texture binding would draw
  // that a vertex colour does not.
  const PICK_MIN  = vec3f(-0.0063, -0.0011, -0.0034);
  const PICK_SIZE = vec3f(0.0127, 0.0021, 0.0194);
  // The model is nineteen millimetres end to end; a pick is a fifth of a
  // metre. And it lies flat in the xz plane with its length along z and the
  // head at the near end of it, where this shader wants the haft up y with the
  // head on top. Both are settled once, below, rather than by asking every
  // rotation afterwards to remember it.
  const PICK_SCALE = 11.3;

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) col: vec4f,
  ) -> VOut {
    let raw = PICK_MIN + qpos.xyz * PICK_SIZE;
    // Length along z becomes height along y, head to the top; the model's own
    // thin axis becomes depth. A quarter turn about x, and the haft's foot put
    // on the origin so the grip below can hold it there.
    let zt = PICK_MIN.z + PICK_SIZE.z;
    var m = vec3f(raw.x, zt - raw.z, raw.y) * PICK_SCALE;
    var n = vec3f(qn.x, -qn.z, qn.y);
    // Forty degrees about the haft, the *other* way, so the point leads.
    //
    // Which end is which, measured off the mesh: walking out along +x the arm
    // narrows in both height and depth, 0.84 x 1.28 mm down to 0.24 x 0.37 —
    // it tapers to a spike. Walking out along -x the height collapses to 0.11
    // but the depth *widens* to 2.12: that one is the chisel. So +x is the
    // point.
    //
    // And the sign of the turn decides where it goes. `m.x * ch + m.z * sh`
    // with a positive angle carries +x toward -z, and -z is behind the eye —
    // so seventy degrees, and then sixty, were both swinging the spike back at
    // the digger and presenting the flat of the chisel to the rock. Negative,
    // it goes to +z, which is where the crosshair is looking and where the
    // swing lands. Forty rather than ninety: at sixty the head foreshortened
    // to a stub on top of the haft, and a pick you cannot see is no better
    // than one aimed backwards. Forty keeps three quarters of its width across
    // the screen with the spike still leading.
    let ch = 0.76604; let sh = 0.0 - 0.64279;   // forty degrees
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
    // The swing itself. It was 1.35 radians of it, which is a wrist: the head
    // came down about sixty degrees and stopped a hand's breadth in front of
    // the face, while the rock being struck is the better part of two metres
    // away and below. Nothing on the screen ever met anything. This is most of
    // a right angle more, so the head goes past the horizontal and finishes
    // pointing down the aim.
    let lean = 0.30 + 1.20 * r;
    let cl = cos(lean); let sl = sin(lean);
    m = vec3f(m.x, m.y * cl - m.z * sl, m.y * sl + m.z * cl);
    n = vec3f(n.x, n.y * cl - n.z * sl, n.y * sl + n.z * cl);
    // The carried roll *unwinds* as it comes down. Held, the haft lies across
    // the frame low right to high left; swung, it straightens onto the line of
    // the blow. Adding to it instead rolled the head further out to the right
    // and the last third of the swing left the frame entirely — which is why
    // there was nothing to see at the moment it landed.
    // No roll at rest, and none worth the name in the swing.
    //
    // Measured off the mesh: the two arms leave the eye and fall away by 8.5
    // and 9.2 degrees — the tips at x = ±6.33 mm and y = 17.5, the eye at
    // 18.5. That is all the droop there is, and any roll about the camera's z
    // is subtracted from it on the high side. Nineteen and a half degrees of
    // carried lean therefore took one end up above the horizontal, where a
    // pick's ends never are; six was still close enough to eat most of the
    // margin. The haft stands square and both ends point at the ground.
    let tilt = 0.0 - 0.06 * r;
    let ct = cos(tilt); let st = sin(tilt);
    m = vec3f(m.x * ct - m.y * st, m.x * st + m.y * ct, m.z);
    n = vec3f(n.x * ct - n.y * st, n.x * st + n.y * ct, n.z);
    // The grip, low right — and thrown forward and down as it swings, because
    // an arm goes with a pick. Pulling it *in* was the rest of why the head
    // never reached anything.
    let grip = vec3f(0.15 - 0.10 * r, -0.19 + 0.02 * r, 0.30 + 0.14 * r);
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
