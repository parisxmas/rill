  @vertex fn vs(@location(0) pos: vec3f, @location(1) n: vec3f, @location(2) c: vec3f) -> @builtin(position) vec4f {
    return u.lvp * vec4f(pos, 1.0);
  }
