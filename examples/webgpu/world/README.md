# Rill Valley

A walkable low-poly valley at dusk, computed by Rill and rendered through
WebGPU. The terrain, its mesh, where every pine, crystal and grass tuft
stands, the walking camera with gravity and jumping, and the view and shadow
matrices are all computed in `world.rill`, compiled to WebAssembly. `host.js`
owns what a wasm module cannot: the promise-based device setup, the WGSL
shaders, and the render passes — a shadow pass under a fixed golden-hour sun,
a 4x MSAA forward pass with procedural sky and fog, instanced pines, crystals
and wind-swayed grass, Gerstner-wave water with shoreline and crest foam, and
a compute-shader swarm of fireflies.

The demo sizes itself to the machine. The adapter's description picks a
starting quality tier; then measured frame times take over, and the world is
regenerated denser or sparser — grid, forests, grass, fireflies, shadow map,
wave count, pixel ratio — until the display's own frame budget is spent.
Rill rebuilds the whole valley in about four milliseconds, which is what makes
re-asking cheap: the terrain is sampled once per lattice corner rather than
once per cell corner, and a candidate tree or crystal is rejected on its seed
before the ground under it is ever worked out — the ground being twelve sines
deep. When the adapter offers `timestamp-query`, the HUD shows
what a frame actually costs the GPU.

```sh
rill build world.rill -o world --target wasm32-wasip1   # writes world.wasm + world.js
python3 -m http.server 8000        # from this directory
# open http://localhost:8000 in a browser with WebGPU
```

`world.js` is the generated loader — the WASI shim, instantiation and init —
so `host.js` here is nothing but the scene: passes, pipelines and tiers. The
shaders themselves are in `shaders/`, one WGSL file per pipeline, fetched in
parallel at startup. They are not standalone: every one is written against
`uniforms.wgsl`, and all but the shadow pair against `common.wgsl` too — the
shadow pass writes the shadow map and so must not be able to see it. What each
may reach is stated in one place, the `SHADERS` map at the top of `host.js`.

Click to capture the mouse; WASD to walk, space to jump, shift to sprint.
Trees and crystals are solid — you slide around them, not through them — and
the wind that moves the water moves the pines and the grass, because all
three read the same two numbers Rill computes.

Three textures ride along: `pine-twig.png`, the alpha-tested needle atlas the
branch cards carry; `pine-bark.jpg`, tiled up and around the trunks; and
`grass-ground.jpg`, tiled from world x/z across the green faces of the terrain,
which is why the terrain still needs no texture coordinate of its own. All
three are cut to 256×256 from Poly Haven's CC0 library — `pine_tree_01`,
`pine_bark`, `leafy_grass` — 95 KB between them against 30 KB of wasm. Mip
levels are made at load by decoding each file again at each size, because
alpha tested foliage without them shimmers from a few trees away.

The meadow photograph is not used as it was shot: it is daylight, and this
valley is dusk. Its brightness drives the variation and its hue is folded in a
third of the way, so the ground gains grain without the palette drifting brown
— and sand, cliff and snow keep the flat shading they were given.
