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

Six textures ride along. Two are alpha atlases: `pine-twig.png`, the needle
cutout the branch cards carry, and `plants.png`, a 2×2 grid of undergrowth —
two tufts of grass, a fern frond, a nettle leaf. Four are tiled: `pine-bark.jpg`
up and around the trunks, and `grass-ground.jpg`, `rock-face.jpg` and
`shore-sand.jpg` for the ground. The ground's three are tiled straight from
world x/z, which is why the terrain still needs no texture coordinate of its
own; rock is also projected sideways and crossfaded in by steepness, because
flat x/z alone smears stone into vertical streaks down exactly the cliffs it
was meant to describe. All six come from Poly Haven's CC0 library —
`pine_tree_01`, `pine_bark`, `leafy_grass`, `rock_face_03`, `coast_sand_03`,
and `grass_medium_01` / `fern_02` / `nettle_plant` for the plant atlas — 197 KB
between them against 30 KB of wasm. Mip levels are made at load by decoding
each file again at each size, because alpha tested foliage without them
shimmers from a few trees away.

One square card serves all four plants. Each was pasted into its atlas cell at
its own aspect and stood on the cell's floor, so a tuft comes out low and wide
and a fern comes out tall and narrow from the same four corners — the rest of
the card is transparent and gets discarded. Which cell an instance draws is
rolled against the size Rill gave it: the taller half of the meadow is more
likely to be a fern or a nettle than a tuft, but only likely, because a meadow
sorted strictly by size reads as sorted.

None of the photographs is used as it was shot: they are daylight and this
valley is dusk. Each is split — brightness drives the variation, hue is folded
in a third of the way, and the face's own colour still decides what the valley
looks like. The brightness is then read a second time as a height, and the
normal bent along its gradient: rock strongly, sand less, grass barely. That
needs no normal map and no tangent frame, only the screen-space derivatives of
the height and of the surface position, which is what a tangent frame would
have been for.

Colour is per corner, and so is the normal. The bands — sand, meadow, rock,
snow — are what makes the valley read as low-poly, and they stay; but they are
weighed rather than chosen. A threshold puts the whole of a snowline at one
triangle's edge, and that edge is a metre long. The shader reads the weights
back out of the colour so the textures crossfade on the same gradient the
colours do, and the three of them sum to one — halfway between meadow and sand
is fully both, not half of neither, which is where the texture used to vanish.

The normal is a lighting question rather than a style one, so Rill takes each
corner's from the slope of its own height lattice — two subtractions, not a
cross product. Four cells meeting at a corner agree about it, and the light
crosses the seam.

The lake is a sum of plane waves, and it used to look it: five or six of them
inside two thirds of a radian of each other, on wavelengths in near-halves,
all starting at phase zero. Waves that parallel and that harmonic add up to one
shape repeated, which is the corduroy the water wore. The spectrum is broad
now — direction fans wider for the shorter waves, because a swell holds its
line and chop does not, and the offsets are sines of the golden angle, which
never come back round on themselves for any count of waves. Wavelengths fall by
a jittered 0.72 rather than a half, and every wave starts somewhere else.

What actually makes it read as water, though, is that the sea comes in sets: a
slow noise field drifting across the lake scales every wave at once, so there
are patches of chop and patches nearly glassy. Without it the same amplitude
covers the whole surface, and no amount of spectrum saves that.
