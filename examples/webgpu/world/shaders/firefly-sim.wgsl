  struct P { anchor: vec4f, pos: vec4f }
  @group(0) @binding(0) var<storage, read_write> ps: array<P>;
  struct T { t: f32, dt: f32, pad0: f32, pad1: f32 }
  @group(0) @binding(1) var<uniform> tt: T;
  @compute @workgroup_size(64) fn cs(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if (i >= arrayLength(&ps)) { return; }
    let a = ps[i].anchor;
    let s = a.w;
    let t = tt.t;
    let p = a.xyz + vec3f(
      sin(t * 0.37 + s * 11.0) * 2.8,
      sin(t * 0.53 + s * 23.0) * 1.1 + 1.6,
      cos(t * 0.41 + s * 17.0) * 2.8,
    );
    let glow = 0.55 + 0.45 * sin(t * 2.4 + s * 37.0);
    ps[i].pos = vec4f(p, glow);
  }
