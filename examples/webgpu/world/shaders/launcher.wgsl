  // The launcher in the hands.
  //
  // Same idea as the rifle's viewmodel and deliberately not the same code: a
  // rifle is held at the shoulder with the sights on the eye's own axis, and
  // this is a tube carried *over* the shoulder with the eye looking past it.
  // What they do share is the frustum — `vmClip`, a near plane at a centimetre
  // and a tighter field of view than the valley's — because the reason for that
  // is the distance from the eye, which is the same for both.
  //
  // An RPG-7, CC0, out of OpenGameArt. It lies along +X like everything else
  // baked here.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
  }

  // Where the tube sits, in the camera's own frame: out to the right, down a
  // little, and far enough forward that the warhead is in front of the eye
  // rather than through it.
  // Nearer the middle than it looks: the frustum here is a 0.40 half-angle, so
  // at thirty centimetres the frame is only twelve centimetres wide and an
  // offset that would be modest in the world is off the edge of the screen.
  const RPG_AT   = vec3f(0.048, -0.105, 0.30);
  // Turned only a little off the view axis, like the rifle: a tube held across
  // the screen is a tube seen at its longest, and this one is a metre and two
  // thirds. Muzzle up a touch, and rolled so the sight block sits where a hand
  // would put it.
  const RPG_YAW   = -0.13;
  const RPG_PITCH =  0.045;
  const RPG_ROLL  = -0.05;
  const RPG_SCALE = 0.26;

  fn rpgRot(rc: f32) -> mat3x3f {
    let cy = cos(RPG_YAW);              let sy = sin(RPG_YAW);
    let cp = cos(RPG_PITCH + 0.16 * rc); let sp = sin(RPG_PITCH + 0.16 * rc);
    let cr = cos(RPG_ROLL);             let sr = sin(RPG_ROLL);
    let Ry = mat3x3f(vec3f(cy, 0.0, -sy), vec3f(0.0, 1.0, 0.0), vec3f(sy, 0.0, cy));
    let Rx = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, cp, sp), vec3f(0.0, -sp, cp));
    let Rz = mat3x3f(vec3f(cr, sr, 0.0), vec3f(-sr, cr, 0.0), vec3f(0.0, 0.0, 1.0));
    return Ry * Rx * Rz;
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) quv: vec2f,
  ) -> VOut {
    let m = u.rpgMin.xyz + qpos.xyz * u.rpgSize.xyz;
    let rc = u.wind.w;
    let R = rpgRot(rc);

    // The walk, at the rifle's amplitude: it is the same shoulders carrying it.
    let amp = clamp(u.wind.y / 5.5, 0.0, 1.0);
    let bp = u.wind.z;
    let t = u.eye.w;
    let bob = vec3f(
      sin(bp) * 0.0060 * amp + sin(t * 0.7) * 0.0014,
      -abs(cos(bp)) * 0.0050 * amp + sin(t * 0.9 + 1.1) * 0.0011,
      -0.004 * amp,
    );
    // Backwards and up when it goes off, harder than the rifle: the whole of
    // the propellant leaves the front of this one.
    let kick = vec3f(0.0, 0.014 * rc, -0.075 * rc);
    // And out of frame while the hands are changing weapons. Down and back,
    // which is where a thing goes when it is being put away, rather than a fade
    // — nothing in this valley fades.
    let swap = u.rpgMin.w;
    let stow = vec3f(0.0, -0.42 * swap, 0.10 * swap);

    // The model's axes to the camera's, the same shuffle the rifle does.
    let cam = R * vec3f(m.z, m.y, m.x) * RPG_SCALE + RPG_AT + bob + kick + stow;

    var o: VOut;
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(R * vec3f(qn.z, qn.y, qn.x));
    o.uv = quv;
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    let albedo = textureSample(rpgTex, barkSamp, v.uv).rgb;
    let n = normalize(v.normal);
    // Lit by the valley it is standing in, like the rifle: same sun, same
    // shadow map, same haze, so a weapon carried into shade goes into shade.
    return vec4f(lit(albedo, n, v.world), 1.0);
  }
