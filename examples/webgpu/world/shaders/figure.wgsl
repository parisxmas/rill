  // Somebody else, with a skeleton in them.
  //
  // The joint matrices arrive already in world space: where the player stands,
  // which way they are turned, and whatever frame of whatever clip they are on
  // are all folded into them before they leave the page. So a vertex here is
  // four matrices and their weights, and nothing else — no model transform, no
  // bind pose, no per-instance rotation. It costs the page fifty-three small
  // matrix products per person and saves the GPU from being told the same
  // skeleton twice.
  //
  // Positions come in as unorm16 across the model's own box, the way the rifle
  // and the shark do; the box is baked in below rather than passed, so the
  // decode is two constants and a mix.

  override BOX_MIN_X: f32 = 0.0;
  override BOX_MIN_Y: f32 = 0.0;
  override BOX_MIN_Z: f32 = 0.0;
  override BOX_MAX_X: f32 = 1.0;
  override BOX_MAX_Y: f32 = 1.0;
  override BOX_MAX_Z: f32 = 1.0;
  override JOINTS: u32 = 53u;

  @group(1) @binding(0) var<storage, read> pose: array<mat4x4f>;

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
  }

  fn skin(inst: u32, j: vec4u, w: vec4f) -> mat4x4f {
    let base = inst * JOINTS;
    return pose[base + j.x] * w.x + pose[base + j.y] * w.y
         + pose[base + j.z] * w.z + pose[base + j.w] * w.w;
  }

  fn unbox(q: vec4f) -> vec3f {
    return vec3f(mix(BOX_MIN_X, BOX_MAX_X, q.x),
                 mix(BOX_MIN_Y, BOX_MAX_Y, q.y),
                 mix(BOX_MIN_Z, BOX_MAX_Z, q.z));
  }

  @vertex fn vs(
    @builtin(instance_index) inst: u32,
    @location(0) qpos: vec4f,     // position, unorm16 across the box
    @location(1) qn: vec4f,       // normal in snorm8, and the material in w
    @location(2) j: vec4u,        // which four joints
    @location(3) w: vec4f,        // and how much each of them
    @location(4) icol: vec3f,     // whose figure this is
  ) -> VOut {
    let m = skin(inst, j, w);
    let p = (m * vec4f(unbox(qpos), 1.0)).xyz;
    var o: VOut;
    o.clip = u.vp * vec4f(p, 1.0);
    o.world = p;
    // The rotation part is enough for a normal, and these joints are rigid:
    // no scale anywhere in the rig, which the bake checks rather than assumes,
    // so there is no inverse transpose to take.
    o.normal = (m * vec4f(qn.xyz, 0.0)).xyz;
    // The mannequin comes in two materials — the body and the ball joints —
    // and the second is kept as a darker version of whatever colour this
    // player was given, so the figure reads as jointed rather than as one
    // block of colour.
    o.color = select(icol, icol * 0.45, qn.w > 0.001);
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let n = normalize(v.normal);
    let view = v.world - u.eye.xyz;
    // Findable after dark. Nothing by day.
    //
    // A tenth was tried first and did nothing, for a reason worth writing
    // down: the night ambient in `lit` is (0.040, 0.048, 0.082), so a figure
    // with an albedo of 0.6 lands around 0.03 — eight of two hundred and
    // fifty-five. A tenth of that is another eight thousandths, and a player
    // stayed a name floating over nothing. Ten per cent of invisible is
    // invisible; what was wanted was a floor, not a percentage.
    //
    // So: a light of its own that only exists in the dark. Not `lit` scaled —
    // `lit` returns with the haze already mixed in, and scaling that lifts the
    // air in front of a player as much as the player.
    let dark = 1.0 - dayness();
    var base = lit(v.color, n, v.world);
    base += v.color * 0.16 * dark;
    // And a rim, because a floor alone makes a flat grey cut-out. Light along
    // the silhouette is what separates a body from the rock behind it when
    // there is no key light to model it with — and the edge is exactly where a
    // player needs to be told there is somebody.
    let rim = pow(1.0 - max(dot(n, -normalize(view)), 0.0), 3.0);
    base += v.color * rim * 0.30 * dark;
    // And their own lamp on their own head. See `lampSpill`: the cone points
    // away from the wearer, so without this the one person a torch never lit
    // was the one carrying it.
    base += v.color * lampSpill(v.world, 1.0 - dark);
    return vec4f(mix(base, fogColor(view), fogAt(v.world, length(view))), 1.0);
  }
