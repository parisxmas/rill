# Rill Valley

A walkable low-poly valley at dusk, computed by Rill and rendered through
WebGPU. The terrain, its mesh, where every pine and crystal stands, the
walking camera with gravity and jumping, and the view and shadow matrices are
all computed in `world.rill`, compiled to WebAssembly. `host.js` owns what a
wasm module cannot: the promise-based device setup, the WGSL shaders, and the
render passes — a shadow pass under a fixed golden-hour sun, a 4x MSAA forward
pass with procedural sky and fog, instanced pines and crystals, animated
water, and a compute-shader swarm of fireflies.

```sh
rill build world.rill -o world --target wasm32-wasip1
python3 -m http.server 8000        # from this directory
# open http://localhost:8000 in a browser with WebGPU
```

Click to capture the mouse; WASD to walk, space to jump, shift to sprint.
