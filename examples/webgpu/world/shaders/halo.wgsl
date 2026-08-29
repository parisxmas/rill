  // A light climbing round a crystal.
  //
  // One ribbon per crystal, wound as a helix and drawn as a strip of quads
  // that always turns its face to the eye — so it reads as a beam rather than
  // as a flat tape seen edge-on from half the angles you can stand at.
  //
  // The ribbon is not the whole helix. It is a length of it whose head climbs
  // from the crystal's foot to its tip and starts again, with the tail
  // trailing behind and thinning out: the shape is a comet on a spiral rather
  // than a barber's pole, and the difference is that a pole is a decoration
  // and a comet is going somewhere.
  //
  // It is light and nothing else: drawn in the main pass only, so it casts no
  // shadow, and the engine has never heard of it — `valley.rill` does not
  // contain the word — so there is nothing here for a body to walk into. Both
  // of those are properties of where the draw is, not flags on it, and this
  // note is here so they stay that way.
  //
  // Everything it needs is already on the crystal's own instance — where it
  // stands, how big it is, its colour, and a rotation that here does the work
  // of a phase, so three hundred and twenty-eight of these do not climb in
  // step. A crystal somebody has broken has its scale set to nought, and a
  // helix of radius nought is a point: it costs the vertices and draws
  // nothing, which is the right answer and needs no branch.

  struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) along: f32,     // 0 at the tail, 1 at the head
    @location(1) across: f32,    // -1 to 1 across the ribbon's width
    @location(2) color: vec3f,
    @location(3) rise: f32,      // how far up the climb this is
    @location(4) thru: f32,      // 0 in front of the stone, 1 behind it
  }

  const HALO_SEGS  = 28u;
  const HALO_TURNS = 2.4;
  const HALO_TRAIL = 0.34;       // how much of the climb the tail covers
                                 // (the rate lives in common.wgsl, shared)

  @vertex fn vs(
    @builtin(vertex_index) vi: u32,
    @location(2) ipos: vec3f, @location(3) scale: f32,
    @location(4) rot: f32, @location(5) icol: vec3f,
  ) -> VOut {
    let seg = vi / 6u;
    let c = vi % 6u;
    // Two triangles of a quad: (0,0) (1,0) (1,1) / (0,0) (1,1) (0,1).
    let ex = select(0.0, 1.0, c == 1u || c == 2u || c == 4u);
    let ey = select(0.0, 1.0, c == 2u || c == 4u || c == 5u);
    // Not `u`: that is the uniform block, and shadowing it here cost a
    // compile and a blank screen.
    let run = (f32(seg) + ex) / f32(HALO_SEGS);

    // The head, and this vertex's place behind it. Clamped rather than
    // wrapped: a wrapped tail hangs from the top of the crystal while its head
    // is at the foot, which draws a line straight down the middle of the thing
    // once a cycle. Clamped, the tail simply runs out at the bottom, and the
    // alpha below takes it from there.
    let head = crystalClimb(rot);
    let t = head - HALO_TRAIL * (1.0 - run);
    let tt = clamp(t, 0.0, 1.0);

    let ang = HALO_TURNS * 6.2831853 * tt + rot;
    let ca = cos(ang); let sa = sin(ang);
    // Foot to tip, on the stone's own measurements. `crystalMesh` puts the
    // bottom point at -0.25, the widest ring at 0.7 with a radius of 0.42, and
    // the top point at 2.4 — all in scales. So the climb runs the whole 2.65
    // of it, and the radius follows the taper rather than a cylinder drawn
    // through it: widest where the crystal is widest, closing to a hand's
    // breadth off the axis at either point, so the light gathers in at the tip
    // the way the stone does.
    let hh = -0.25 + 2.65 * tt;
    let body = select((2.4 - hh) / 1.7, (hh + 0.25) / 0.95, hh < 0.7);
    let rad = scale * (0.42 * body + 0.13);
    let centre = ipos + vec3f(ca * rad, scale * hh, sa * rad);

    // The ribbon's own direction, which is the helix's tangent: round by the
    // angle it turns through and up by the height it gains, in the same
    // parameter. Taking only the angular part leaves the width leaning wrong
    // wherever the climb is steep.
    let dang = HALO_TURNS * 6.2831853;
    let tang = normalize(vec3f(-sa * rad * dang, scale * 2.65, ca * rad * dang));
    let toEye = normalize(u.eye.xyz - centre);
    // Not normalised at all.
    //
    // Both of those are unit vectors, so their cross product already has the
    // length this wants: the sine of the angle between them, which is one when
    // the ribbon lies across the view and nothing when it runs straight at the
    // eye. That is exactly how wide a ribbon should look. Normalising it
    // instead divides by that sine, and dividing by nothing threw quads across
    // the sky; picking a fallback direction where the sine was small was worse,
    // because the two answers disagree and the quad straddling the switch came
    // out a wedge metres wide.
    let side = cross(tang, toEye);

    // Wide behind the head and thinning to nothing at the tail — and drawn in
    // again over the last tenth, so the ribbon closes to a bead instead of
    // stopping at a straight edge. A strip of quads ends with the edge of a
    // quad, and a flat cut across the brightest part of the thing is the one
    // place the eye goes.
    let bead = 1.0 - 0.72 * smoothstep(0.88, 1.0, run);
    let w = scale * 0.23 * (0.20 + 0.80 * run) * bead;
    let world = centre + side * (ey * 2.0 - 1.0) * w;

    // Which side of the stone this is on.
    //
    // The crystal is glass and writes no depth — it cannot, or the grass and
    // the water behind it would be punched out of the picture — so the depth
    // test will not hide anything passing behind it, and the ribbon came round
    // the back as bright as it went round the front. Which side it is on is
    // not something the depth buffer can be asked here, but it is two lines of
    // arithmetic: take the line from the eye to the crystal's axis, and see
    // how far along it this point lies. Past the axis is behind the stone, and
    // behind the stone the light has a crystal's thickness in front of it.
    let toAxis = ipos.xz - u.eye.xz;
    let dirA = toAxis / max(length(toAxis), 0.001);
    let past = dot(dirA, centre.xz - ipos.xz);

    var o: VOut;
    o.thru = smoothstep(-0.10 * scale, 0.30 * scale, past);
    o.clip = u.vp * vec4f(world, 1.0);
    o.along = run;
    o.across = ey * 2.0 - 1.0;
    o.color = icol;
    // Whether this vertex is on the ribbon at all: the tail that ran off the
    // bottom, and a fade at the top so the head goes out rather than being
    // cut off when it starts again at the foot.
    // And only on the crystals you are near. Three hundred and twenty-eight
    // of these winding away across a whole valley is a field of fireflies, not
    // a light on a stone; the ones at the far side of the water say nothing
    // about where you are. So they come up as you walk in on them: whole
    // inside fourteen metres, gone by forty.
    let near = 1.0 - smoothstep(14.0, 40.0, length(u.eye.xyz - ipos));
    o.rise = select(0.0, smoothstep(1.0, 0.82, tt) * near, t > 0.0);
    // Thrown out of the frustum rather than multiplied by nothing.
    //
    // Fading in the fragment is not not-drawing: every ribbon in the valley
    // still had its quads set up, rasterised and blended, and only then
    // multiplied to black. Three hundred of those behind you cost the same as
    // three hundred in front. Put outside the clip volume here, the whole
    // instance is discarded before any of that — and a crystal beyond forty
    // metres has nothing to say anyway.
    if (near <= 0.0) { o.clip = vec4f(0.0, 0.0, -2.0, 1.0); }
    return o;
  }

  @fragment fn fs(v: VOut) -> @location(0) vec4f {
    // Across the ribbon: a bright core with the edges falling away, which is
    // what a beam looks like and what a flat band does not.
    let a = 1.0 - abs(v.across);
    let core = a * a * a;
    // Along it: the head carries the light and the tail is what is left of it.
    let tail = v.along * v.along;
    // And the head itself burns, rather than merely being the bright end of a
    // gradient: the last fifth is lifted hard and goes white in the middle, so
    // what leads the trail is a spark and not a cut.
    let head = smoothstep(0.80, 1.0, v.along);
    let hot = head * head * core;
    let pulse = 0.85 + 0.15 * sin(u.eye.w * 3.7 + v.rise * 9.0);
    // Through the stone it is dimmer, the way anything seen through a coloured
    // solid is. Not extinguished: a crystal is glass, and you can see the far
    // wall of one.
    let lum = (core * tail + hot * 1.9) * v.rise * pulse * mix(1.0, 0.30, v.thru);
    let white = lum * lum * 0.35 + hot * hot * 0.9 * v.rise;
    return vec4f(v.color * lum * 1.6 + vec3f(white), 1.0);
  }
