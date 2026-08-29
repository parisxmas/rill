  // The AK in the hands.
  //
  // Its own shader rather than the rifle's, and the reason is that the model
  // arrived as geometry and nothing else: a high-poly AK-47 published as a
  // raw OBJ, CC0, with no texture coordinates and no normals in the file.
  // The normals are worked out at bake time; the surface is worked out here,
  // from where a point sits along the weapon. That turns out to be the right
  // way round for this one — an AK is four materials in four places, and
  // where a thing is says which it is more reliably than a photograph
  // stretched over a model that was never unwrapped for one.
  //
  // The four, measured off the model's own profile:
  //
  //   the butt              behind -0.26
  //   the pistol grip       under the receiver, around the middle
  //   the handguard         over the barrel, forward of +0.11
  //   everything else       steel — and the barrel is the point of it

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) model: vec3f,
    // Whether this vertex belongs to the magazine. Flat, not interpolated: it
    // is a yes or a no per piece of geometry and a half of one, along the edge
    // where the magazine meets the well, would be a seam of the wrong metal.
    @location(3) @interpolate(flat) mag: f32,
  }

  // ---- changing the magazine ------------------------------------------------
  //
  // `u.akMin.w` runs from nought to one across the second and a half the hands
  // are busy, and is nought at every other moment — so for all but that second
  // and a half none of this does anything at all.
  //
  // Which vertices are the magazine is a flag the mesh carries in the spare
  // fourth component of its quantized position, the way the rifle's mesh says
  // which vertices are its bolt. It has to be a flag and not a test on where a
  // vertex is: the magazine's two side plates are each one piece running from
  // the floor of the magazine up inside the receiver, so any test that cuts
  // through the middle of the gun leaves half a plate hanging in the well
  // while the other half falls away. `tools/mark_ak_magazine.py` decides it
  // per connected piece, which cannot do that.
  //
  // An AK's magazine rocks. The lug at the front of it hooks into the front of
  // the well and a catch behind holds the rear down; it goes in nose-first and
  // is turned back until the catch takes it, and it comes out the same way
  // round. So this turns about that lug rather than sliding straight down,
  // which is the whole difference between a rifle and a drawer.
  const CHANGE_SECS = 1.45;   // must match `CHANGE_SECS` in host.js
  const MAG_HOOK = vec3f(-0.005, 0.017, -0.012);
  const MAG_TURN = 0.55;      // radians off the lug, about thirty degrees
  const MAG_OFF  = 0.11;      // when the old one comes free
  const MAG_GONE = 0.34;      // and when it is far enough down to stop drawing
  const MAG_BACK = 0.46;      // the new one is in the hand
  const MAG_SEAT = 0.80;      // and home

  fn zRot(a: f32) -> mat3x3f {
    let c = cos(a); let s = sin(a);
    return mat3x3f(vec3f(c, s, 0.0), vec3f(-s, c, 0.0), vec3f(0.0, 0.0, 1.0));
  }

  fn xRot(a: f32) -> mat3x3f {
    let c = cos(a); let s = sin(a);
    return mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, c, s), vec3f(0.0, -s, c));
  }

  // How far the magazine is turned off its lug.
  fn akMagTurn(p: f32) -> f32 {
    if (p < MAG_GONE) { return MAG_TURN * smoothstep(0.0, 0.13, p); }
    return MAG_TURN * (1.0 - smoothstep(MAG_BACK, MAG_SEAT, p));
  }

  // And where it has got to. Once it is off the lug it is not turning about
  // anything any more — it is falling, and the fall is the real one: a second
  // and a half of phase is a second and a half of clock, so `t` is seconds and
  // the term below is gravity in the model's own units.
  fn akMagShift(p: f32) -> vec3f {
    if (p < MAG_GONE) {
      let t = max(p - MAG_OFF, 0.0) * CHANGE_SECS;
      return vec3f(0.30 * t, -4.9 * t * t, 0.0);
    }
    let u = smoothstep(MAG_BACK, MAG_SEAT, p);
    return vec3f(0.035, -0.32, 0.0) * (1.0 - u);
  }

  // The weapon under it. Brought down and rolled so the well turns toward the
  // hand that is feeding it — a positive roll about the barrel takes the well
  // to the left, which is where that hand is. This is the half that reads as
  // somebody reloading; a magazine moving on its own in front of a gun held
  // rock steady reads as a bug. Up again by the end, so the sights are back
  // before the trigger is.
  fn akSwing(p: f32) -> f32 {
    return smoothstep(0.0, 0.30, p) * (1.0 - smoothstep(0.72, 1.0, p));
  }

  @vertex fn vs(
    @location(0) qpos: vec4f, @location(1) qn: vec4f, @location(2) uv: vec2f,
  ) -> VOut {
    // Where the vertex belongs on the weapon, which is what the fragment
    // shades from. It is kept whatever the hands are doing: a magazine that
    // turned to wood on the way out of the well would be a fine joke and the
    // wrong one.
    let rest = u.akMin.xyz + qpos.xyz * u.akSize.xyz;
    let p = u.akMin.w;

    var m = rest;
    var R = mat3x3f(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0));
    var o: VOut;
    if (p > 0.0) {
      if (qpos.w > 0.5) {
        // Between the two magazines there is no magazine. Behind the far
        // plane rather than merely a long way down: a card at nine metres is
        // still a card somebody could catch sight of.
        if (p >= MAG_GONE && p < MAG_BACK) {
          o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
          o.world = vec3f(0.0);
          o.normal = vec3f(0.0, 1.0, 0.0);
          o.model = rest;
          o.mag = 1.0;
          return o;
        }
        R = zRot(akMagTurn(p));
        m = MAG_HOOK + R * (m - MAG_HOOK) + akMagShift(p);
      }
      // The gun's own swing, applied over whatever the magazine just did — so
      // the magazine that has left the well still rides the weapon down for
      // the moment it is in front of it.
      let sw = akSwing(p);
      let G = xRot(0.62 * sw) * zRot(-0.20 * sw);
      R = G * R;
      m = G * m + vec3f(-0.03, -0.075, 0.0) * sw;
    }

    // Behind the sights, when the right button is down.
    //
    // The shared aim pose was built for the rifle, and what it puts on the
    // view axis is that one's optical axis — 0.0859 above the model's floor
    // and dead on its centreline. The AK has no optic. What it has is a post
    // and a notch, and the line they make is the top of the front post: this
    // model carries it at y +0.0825 on a centreline of z -0.0120, both read
    // off the muzzle's own cross-section and off the sight tower.
    //
    // So the AK is nudged until its iron line lies where the rifle's glass
    // line does, and then the pose that was already right for one is right for
    // the other. Three and a half millimetres up and twelve across, in the
    // model's units — small, because the two were nearly on top of each other
    // to begin with. `u.vmSize.w` is how far through the move the hands are,
    // so it arrives with the weapon rather than snapping on at the end.
    //
    // And pushed forward along its own barrel while it is up. The shared pose
    // brings the weapon to within fifteen centimetres because that is where an
    // eye goes behind an eyepiece; behind a notch it is too near, and the back
    // of the receiver rose up and took the bottom half of the screen with it.
    // Forward is free here — sliding along the barrel does not move the sight
    // line off the axis, only the distance to it — so the notch and the post
    // both get smaller together and the picture stays true.
    m = m + vec3f(0.085, 0.0034, 0.0120) * u.vmSize.w;

    let cam = vmCam(m);
    o.clip = vmClip(cam);
    o.world = camToWorld(cam);
    o.normal = camDirToWorld(vmDir(R * qn.xyz));
    o.model = rest;
    o.mag = qpos.w;
    return o;
  }

  // Wood: laminated birch, dark with the varnish, grain running the length of
  // the piece. Two frequencies, because one reads as stripes.
  fn akWood(m: vec3f) -> vec3f {
    let grain = sin(m.x * 210.0 + m.z * 14.0) * 0.5 + sin(m.x * 61.0) * 0.5;
    let t = 0.5 + 0.22 * grain;
    return mix(vec3f(0.196, 0.098, 0.043), vec3f(0.35, 0.185, 0.082), t);
  }

  // Steel, blued. Nearly black to look at and almost entirely what the light
  // does to it — which is why the barrel needs the specular below to exist at
  // all, and why a photograph of one is mostly a photograph of the room.
  // Chrome, which is not a colour.
  //
  // The blued finish this had was a dark grey and nothing else, and that is
  // honest for parkerised steel: what such a surface does with light is
  // scatter a little of it. Plating changes the question. A chromed part is
  // almost a mirror — its own colour is a faint blue-grey and nearly all of
  // what you see in it is the world reflected — so brightening the albedo
  // would only have made a pale plastic. What is added below in `fs` is the
  // reflection; this is the little that is left over, and the lathe's grain
  // is still in it because plating follows the metal under it rather than
  // hiding it. Half the depth it had, because polishing is what plating is.
  fn akSteel(m: vec3f) -> vec3f {
    let mill = sin(m.x * 900.0) * 0.5 + 0.5;      // the turn of the lathe
    return vec3f(0.062, 0.066, 0.076) + vec3f(0.006) * mill;
  }

  @fragment fn fs(v: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let m = v.model;
    // The magazine is steel, all of it, and it has to be said rather than left
    // to the rules below. Those decide a material from where a point sits, and
    // the second of them — the one that means the pistol grip — reaches across
    // the top half of the magazine as well, so the magazine came out of the
    // mesh painted wood at the back and steel at the front. That never showed
    // because the two-tone half was up inside the well; now that it comes out
    // of the well it would be the first thing anybody noticed.
    let mag = v.mag > 0.5;
    let wood = !mag && ((m.x < -0.26)
            || (m.x > -0.06 && m.x < 0.035 && m.y < -0.02)
            || (m.x > 0.11 && m.x < 0.31 && m.y > -0.01));
    let albedo = select(akSteel(m), akWood(m), wood);

    let nn = normalize(v.normal);
    let n = select(-nn, nn, front);
    var col = lit(albedo, n, v.world);

    // What makes it metal. Wood scatters and steel reflects, so the barrel
    // gets a tight highlight where the sun is and a wide one off the sky —
    // the second is what keeps it from going to a black stick after dark,
    // which is what the old skin did and why the barrel could not be found.
    if (!wood) {
      let toEye = normalize(u.eye.xyz - v.world);
      let L = -normalize(u.sun.xyz);
      let h = normalize(L + toEye);
      let day = dayness();
      let sun = pow(max(dot(n, h), 0.0), 55.0) * 1.3 * day;
      // A rim off the sky, which is the light that is always there: brightest
      // where the surface turns away, so a round barrel gets a line down its
      // length and reads as a cylinder rather than as a dark bar.
      // Tight, and weak. At a wide exponent and nine tenths this washed the
      // whole weapon to bone — every triangle facing away from the eye lit up,
      // which on a model this dense is most of them. What is wanted is a line
      // down the edge of a round thing, not a coat of paint.
      let rim = pow(1.0 - max(dot(n, toEye), 0.0), 5.0);
      col += SUN_COL * sun;
      col += mix(vec3f(0.10, 0.11, 0.17), vec3f(0.30, 0.32, 0.42), day) * rim * 0.10;

      // And the plating: the sky, in the metal.
      //
      // This is the whole of what makes chrome read as chrome. A mirror shows
      // what is in front of it, so the barrel carries the sky down its top and
      // the ground up its underside, and it changes as the weapon turns —
      // which is the thing a brighter grey can never do, however bright.
      //
      // What is reflected is a whole world, not half of one. Folding the ray
      // up to the sky — the way the water does, because a lake only ever
      // reflects upward — made the barrel one even blue, and an even anything
      // is the one thing chrome never is. What sells it is the horizon: sky
      // down the top of the barrel, ground up its underside, and a hard line
      // between them that travels as the weapon turns.
      //
      // The ground is a colour rather than a lookup. There is no cubemap here
      // and nothing to sample the valley with; what a chromed part actually
      // catches from below is the sand a metre under it, which is warm, dim
      // and — this is the part worth having — much darker than the sky.
      //
      // Fresnel, so the grazing edges are brightest. Metal reflects at every
      // angle: chrome's normal-incidence reflectance is about 0.55, not the
      // 0.04 of a dielectric, so this leans on a high floor rather than the
      // usual near-zero one.
      let refl = reflect(-toEye, n);
      let sky = skyColor(vec3f(refl.x, max(refl.y, 0.0), refl.z), u.eye.w);
      let below = mix(vec3f(0.030, 0.032, 0.042), vec3f(0.26, 0.22, 0.17), day);
      let env = mix(below, sky, smoothstep(-0.22, 0.10, refl.y));
      let fres = 0.55 + 0.45 * pow(1.0 - max(dot(n, toEye), 0.0), 4.0);
      col += env * fres * 0.85;
    }

    // The flash lights what is holding it, the same way the rifle's does.
    let flash = clamp(u.fx.x, 0.0, 1.0);
    col += albedo * vec3f(1.0, 0.62, 0.26) * flash * 2.2;
    return vec4f(col, 1.0);
  }
