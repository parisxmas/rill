# Changelog

## Unreleased

- **A world you can walk through, in `examples/webgpu/world/`.** The proof
  that the wasm target and `export fn` carry real weight: a low-poly valley
  at dusk — terrain from three octaves of value noise with a lake carved out
  of the middle, two hundred and sixty pines and a scatter of glowing
  crystals placed by the same hash, a first-person camera that walks, falls
  and jumps against the terrain, and every matrix the GPU consumes — all
  computed in Rill. The mesh is fifty-five thousand flat-shaded vertices
  written straight into linear memory in seven milliseconds, through loops
  whose bodies return how far to advance, because on wasm the call that comes
  back around must be a tail call and nothing else.

  The renderer half exercises most of what WebGPU has: a shadow pass under a
  fixed golden-hour sun with PCF sampling, a 4x MSAA forward pass, instanced
  drawing fed directly from the buffers Rill wrote, alpha-blended animated
  water, additive crystal halos, a compute-shader swarm of twelve hundred
  fireflies, and a procedural sky with the sun and stars in it. Two lessons
  the debugging left behind: a texture cannot be sampled in the pass that
  renders into it, which poisons every frame silently unless an error scope
  is listening — and `textureSampleCompare` must be reached in uniform
  control flow, so the shadow lookup samples first and selects after.

  The demo sizes itself to whatever machine opens it. There is no API that
  says how fast a GPU is, so it is not asked: the adapter's description picks
  a starting tier, and measured frame times do the rest — the world is
  regenerated denser or sparser until the display's own frame budget is
  spent, which `init` taking the grid and the forest densities as arguments
  makes cheap enough to do live. The camera survives the world being rebuilt
  around it. On an Apple M4 the loop settles at 160x160 cells, nine hundred
  pines, five thousand grass tufts and four thousand fireflies, with the
  GPU reporting under three milliseconds a frame through `timestamp-query`.

  The water became water. Gerstner trochoids rather than summed sines — the
  horizontal displacement is what sharpens crests — with phase speed from
  the gravity relation, amplitude that dies in the shallows, ripple octaves
  in the fragment stage for the sun's glitter, Schlick fresnel against the
  same procedural sky the sky pass draws, absorption with depth, and foam
  where the depth runs out and where the trochoids pinch. Every water vertex
  carries the exact terrain depth beneath it, asked of Rill's own height
  function point by point — a 32-bit re-derivation in the shader would have
  put the shoreline foam where f32 thought the shore was.

- **`export fn`, and a triangle drawn through WebGPU.** The wasm target could
  run a program; now it can *be* a program the host keeps calling. `export fn
  frame(t: Float)` makes `frame` a wasm export, and a module with any export
  links as a reactor — no `_start`, no exit, the host drives it. `main`
  becomes `rill_init`, called once by the host before anything else, which is
  what `main` already meant with the process boundary taken away.

  The signature rules are the boundary's own: parameters `Int`, `Float`,
  `Bool` or `Ptr`, returns those or `Unit`, no generics — an export is one
  entry point, not a family waiting for a call site to pick its types. Wider
  data crosses through a `Buf` and `buf_data`, which the host reads straight
  out of linear memory. Each rule is refused with the reason, at the export's
  own line, and `export` on a native target is refused too: exports are
  functions the embedding host calls, and a native executable has no host.

  In a reactor, an `extern fn` that no library satisfies becomes a wasm
  import for the host to fill in — which turns out to be the entire browser
  FFI. `examples/webgpu/` draws a spinning triangle: WebGPU's setup is
  promise-based and a wasm module cannot await, so the JavaScript host does
  the ceremony once and supplies two imports, and Rill computes the frame —
  corners from `cosf`/`sinf` (resolved against wasi-libc, like any C call)
  into a `Buf(F32)`, handed over as a pointer. The host file is also the
  needed WASI shim, all sixty lines of it, because browsers ship none —
  `println` in a canvas tab arrives in the console by way of `fd_write`.
  Five million frames leave the module's memory byte-for-byte the same size:
  each frame's buffer recycles through the pool, and the 16 MB the pool maps
  up front is all it ever maps.

- **A strand that only computes no longer holds its worker to the end of the
  program.** It could before, and everything else on that worker waited for
  good: a started strand is pinned to the stack its frames live on, so no other
  worker may pick it up. Two strands and one worker was enough to hang — one
  spinning, one that had already run and could not be moved.

  Rill has no `while`, so a loop is a function calling itself in tail position,
  and that is where the check goes: a byte read, a branch that is not taken,
  and a call into the scheduler when it is. A timer thread raises the flag once
  a slice and the strand hands its worker back at the next turn of the loop,
  exactly as if it had blocked on a channel — the stack is saved the same way,
  at a point the compiler chose, so none of this needs a map of which words are
  references.

  Taking a turn away from a strand is worth nothing if nobody is waiting for
  one, and a server spends most of its life with nobody waiting, so the timer
  looks ten times less often while the queues are empty. It costs the first
  strand to start hogging a worker one slow look before it is noticed, and it
  is the difference between an idle program waking a sleeping core a thousand
  times a second and thirty times: three seconds of doing nothing measured
  10 ms of system time before, and none after.

  Two things keep it from costing anything. It is only emitted in programs that
  can reach a strand at all, decided by walking the call graph out from `main`
  before a line is generated, so `fib`, `trees` and `lcg` contain no trace of
  it. And within those programs it goes only on tail calls that come back
  round, which is direct recursion and, through the same call graph, two
  functions calling each other — a check on only the first would leave the
  second able to spin. Measured over 25 runs: `ring` 18.9 ms against 18.9 ms
  without, with identical fastest runs.

  The load has to be volatile, or it would be lifted out of the loop it exists
  to interrupt.

- **What abandoned strands were holding is released.** `main` returning ends
  the program whether or not the strands it started have finished, which is the
  semantics we want — but a strand that never reaches the end of its body never
  reaches the `rill_env_release` there, so its environment and everything the
  environment captured stayed held. `spawn`ing a worker and letting `main`
  return was enough to see it: three live allocations in `examples/loops.rill`,
  four in `showcase`.

  Nothing knew where the strands were. A parked one is in a channel's wait
  queue, a fresh one in a run queue, and neither is reachable from the other —
  the `next` field is already the link for whichever queue the strand is
  sitting in. So strand records now go on a second, permanent list as they are
  allocated, and the shutdown path walks it and releases the environment of
  anything that had not finished. They are recycled through their worker's free
  list and never handed back, so the list is only ever as long as the most
  strands that existed at once. A strand that did finish has already released
  its own environment and is marked dead, so nothing is released twice.

  Walking that list is a cache miss per strand, though, and `ring` makes a
  hundred thousand of them and leaves none of them running — it paid 6 ms to
  find nothing. A count of the strands that have started and not finished
  answers that without looking, and the walk happens only when it is not zero.

  What a strand that had *started* left on its stack stays. Those references
  are owned by that stack, and finding them would need a map of which words are
  references — precisely what the copy-on-block scheduler is built to do
  without. So the report names them instead of implying a mistake: `live
  allocations: 2 (1 strand(s) were still running at exit)`. Every program that
  abandons nothing now reports a clean zero, which is what makes the number
  worth asserting on again.

  With more than one worker the sweep first has to know that nobody is still
  looking at a strand. `STOP` became an atomic and is now read at the top of
  every scheduler iteration rather than only when the queue runs dry, which
  also settles a difference that should never have been there: one worker
  already abandoned everything the moment `main` died, and several workers
  went on running however many strands happened to be queued. They now do the
  same thing. Each worker records that it has left its loop, and shutdown wakes
  them and waits for that — a join in all but name, and not `pthread_join` for
  the reason joining was rejected before: a strand that never blocks would
  never let go of its worker. So the wait is bounded and giving up is a normal
  outcome, leaving the strands exactly as they were.

- **Rill compiles to WebAssembly.** `rill build foo.rill --target wasm32-wasip1`
  writes a `.wasm` that runs under any WASI host. Codegen needed nothing: the
  data layout already comes from the target machine, so a 32-bit pointer is
  simply what LLVM lays out. The runtime grew a third arm beside the unix and
  windows ones — wasm has one thread and no way to make another, so the
  scheduler's primitives are all the same answer, and `mmap` is `malloc`
  because the pool only ever bump-allocates chunks and never gives one back.

  Three things had to agree that never had to before. wasi-libc's startup
  calls `__main_argc_argv`, not `main`, and there is no shimming around it —
  clang renames a C `main` the same way, so a hand-written forwarder forwards
  to itself. `rill_rt_run_direct` took its entry as a one-argument function
  pointer and called `rill_main`, which takes none; a call is just a jump on
  every other target, but wasm checks the signature of an indirect call
  against the callee's and traps. And `rill_env_release` found the closure's
  drop function by counting one `usize` past the refcount, which is eight
  bytes only where a pointer is eight bytes wide — the environment is
  `{ i64 rc, ptr drop_fn, ... }`, so the offset is now written in bytes. The
  same assumption was in `rill_peek_ptr` and `rill_poke_ptr` as a literal `8`.
  All three were correct on every 64-bit target and wrong on the first 32-bit
  one; nothing that shipped was affected.

  Strands do not come along, and cannot. A context switch means saving the
  registers and the stack pointer, and wasm exposes neither — the call stack
  belongs to the engine, and the copy-on-block model has nothing to copy. The
  saving grace is that `needs_scheduler()` already decides this per program:
  one that never spawns links `rill_rt_run_direct` and drops the scheduler
  entirely, so single-threaded wasm is a subset rather than a fork.

  A program that does spawn is refused, and refused where it spawns rather
  than at the abort the runtime's stubs would otherwise reach. Finding the
  line meant knowing which functions are the author's: the prelude is merged
  in ahead of them, so `Module` and `TModule` now record how many of their
  functions came from it, and a channel reached only through a prelude wrapper
  is still refused but without citing a line the author cannot go and read.
  A named library is refused the same way — a wasm module imports from its
  host rather than linking a shared object — while a bare `extern fn` is left
  alone, because it resolves against wasi-libc like any other C symbol.

  What that last allowance lets through is a declaration that disagrees with
  the C function it names, which is easy to write when `size_t` is 32 bits
  here and Rill's `Int` is 64. `wasm-ld` calls it a warning and emits a stub
  that traps when called, so the link is run with `--fatal-warnings` and it
  becomes an error naming the function and both signatures.

  Sixteen of the twenty-seven examples run. The other eleven are refused with
  a diagnostic — nine by the compiler, two by the linker — and none of them
  reaches a trap at run time. Stripped, `hello.wasm` is 28.5 KB against
  32.9 KB for the native binary.

- **Live graphs in the task manager.** A strip per CPU core and one for
  memory, drawn by Rill: a view class of our own with `drawRect:`, which is a
  fifth callback shape — one that receives a struct by value, since an NSRect
  on arm64 is four doubles in v0-v3, the same reasoning `send_rect` uses in
  the other direction. Bars are `NSRectFill` in the system's own colours, so
  they follow the light and dark appearances. Memory also gets a real
  `NSProgressIndicator` for where it stands now.

  Two things had to be worked out along the way. `drawRect:` is handed the
  rectangle that needs repainting, which can be *larger* than the view — the
  whole window's, in practice — so it cannot be used as the bounds; and Rill
  cannot receive a struct as a result, so a view cannot simply be asked how
  big it is. KVC answers it: `[v valueForKey:@"frame"]` is an NSValue, an
  object Rill can hold, and `getValue:size:` writes the struct into memory the
  caller owns. Overriding `setFrameSize:` looked like the way to be told about
  a resize, but autoresizing does not route through an added method — nothing
  ever fired. `windowDidResize:` on the window delegate does, continuously,
  including while a corner is being dragged.

  The two halves sit in an `NSSplitView`, so the divider between them can be
  dragged, and a wider window adds the same number of points to each side
  rather than the proportion springs would have chosen. Its divider position
  has to be set after the window is on screen: asked before that, a split view
  has not worked out its own geometry and clamps it to nearly nothing.

- **A process monitor, in `examples/cocoa/taskman.rill`.** A native AppKit
  window listing every process on the machine with its CPU share and resident
  memory, refreshed on a timer, with a button that sends SIGTERM to the
  selected one. `procs.rill` reads the kernel's table through libproc —
  `proc_listpids` into a `Buf`, then `proc_pidinfo` per pid — and the window's
  table pulls its contents from a data source written in Rill. It exists
  because it needs everything below at once: a callback that answers with a
  count, one that answers with an object, both carried into `class_addMethod`
  as parameters. Three things worth knowing are written down where they bit.
  The CPU counters are in mach absolute time units rather than nanoseconds, so
  elapsed time is measured with the same clock and the ratio needs no
  conversion. The memory figure is `ri_phys_footprint` and not the resident
  size — that is the number Activity Monitor shows, and the resident one puts
  Chrome at 481 MB where the system says 339. And a process owned by another
  user cannot be read at all, not even its name, while leaving the buffer
  holding the previous process's numbers: a refusal means leave it out, not
  read zeroes. About a third of the machine is missing for want of the
  privileged helper Activity Monitor has, and the status line says so rather
  than quietly showing a short list. Clicking a column header sorts by it and clicking again
  turns it around, through `tableView:didClickTableColumn:` — a fourth
  callback shape (two objects, no answer) on the same class that already
  carries the data source. Re-sorting is not re-reading: the loop keeps the
  rows it last built, so the numbers on screen do not move when the order
  does, and every process is sorted rather than only the hundred and twenty
  on screen.

- **Callbacks can answer.** A foreign signature may now write `Fn(Ptr, Ptr) ->
  I32`, and the trampoline carries the result back out, narrowed to the
  declared width the same way an argument is narrowed on the way in. Without
  the arrow a callback still hands nothing back, so every existing declaration
  means what it did. The result has to be something C can hold: a heap value
  would be discarded with nothing left to release it, and a callback returning
  another callback would need a trampoline built at run time. This unblocks
  the signatures that were previously unspellable — a `qsort` comparator, a
  table view asking how many rows it has, a `windowShouldClose:` that can say
  no. `examples/callbacks.rill` sorts an array through one.
- **Callbacks travel as parameters.** A callback no longer has to be named at
  the `extern` call itself, so a helper can wrap the registration instead of
  every call site repeating it. What crosses is still a bare code pointer, so
  the function is settled while compiling: a helper that forwards a callback
  is emitted once per callback it is called with, each copy with its own
  trampoline built in — the same monomorphization the language already does
  for type parameters, keyed on a function instead of a type. Only parameters
  that actually reach C are treated this way, so nothing else duplicates. A
  lambda, or a function picked at run time, is still refused, and now the
  diagnostic names the parameter it travelled under rather than the C
  signature.
- **The AppKit demo uses both.** `examples/cocoa/` grew the parts that were
  previously unreachable: a `windowShouldClose:` delegate that refuses to let
  the window close until the button has been pressed, and an `NSTableView`
  whose rows come from a data source written in Rill — one method answering
  with a count, the other with an object. `cocoa.rill` now names the four
  method shapes it can install (`does`, `answers`, `counts`, `supplies`),
  each a helper taking the callback as a parameter, and the three-step
  `responder_class` / `void_sig` / `class_addMethod` dance at the call site
  is gone.

## 0.2.11 — multi-core servers

Two bugs made `--parallel` unusable for anything that allocates or talks to a
socket. Both were found while chasing the HTTP benchmark's tail latency, which
turned out not to be a Rill problem at all.

- **The allocator was not thread-safe.** One global set of free lists and one
  bump pointer, mutated without synchronization: two workers allocating at the
  same time corrupted the free list and the program died with a segmentation
  fault. Each worker now owns its pool, so the fast path still takes no lock,
  and a block freed on another worker simply joins that worker's list.
  `benchmarks/parallel_alloc.rill` is the regression test — it crashes on the
  old runtime at four workers and passes now.
- **Network wakeups went to the wrong worker.** A strand that has run is
  tied to its worker's stack, but every worker shared one poller, so a
  readiness event woke whichever worker happened to be asleep rather than the
  one that could resume the strand. The real owner then waited out the
  20 ms poll timeout — on nearly every request. Each worker now has its own
  poller and is woken directly; spawning also wakes a sleeper so it can steal.
  The HTTP server went from 2,219 req/s on four workers to 39,158, next to
  40,903 on one.
- The connection-per-request tail latency that started all this is a
  measurement effect, not a defect: at matched connection rates Rill's p99 is
  lower than Go's (10.9 ms against 15.7 ms), and the spikes happen while the
  server sits completely idle. `benchmarks/BENCHMARKS.md` shows the evidence
  and the README claim that Go held an advantage there has been corrected.
- Recorded ring memory corrected to 53.4 MB; the old 45.7 MB no longer
  reproduces on either the current or the previous runtime.

## Unreleased — typed buffers

`alloc_bytes` and `poke_f32` are how C's memory is reached, but they are a
poor thing to write a program in: no length, no element type, and a `free` to
remember. `Buf(w)` is the same memory with all three.

- **`Buf(w)`** is a counted block of elements of one C width — `Buf(F32)`,
  `Buf(U8)`, `Buf(Ptr)` and the rest. `buf_f32(n)` and its siblings allocate,
  `buf_get`/`buf_set` read and write one element, `buf_len` reports the count
  and `buf_data` hands the first element's address to C.
- **Indexing is checked.** An index outside the buffer ends the program naming
  the index and the length, instead of reading past the end. The comparison is
  unsigned, so a negative index fails the same test.
- **Counted like a string**, header and all (`{rc, len, elements}`), so it is
  released when the last reference goes. `examples/gl/texgen.rill` lost its
  `free_ptr` and generates its textures into a `Buf(U8)`.
- The width is part of the type rather than a type variable, so each width has
  its own constructor and a function taking a buffer annotates it. The error
  when it cannot be inferred says so.
- **`examples/gl/` was rewritten on top of it**, and there is now no manual
  memory anywhere in it: the maze grid and its search stack are `Buf(U8)` and
  `Buf(I32)`, the matrices and every mesh are `Buf(F32)`, textures and pixel
  readback are `Buf(U8)`, and the out-parameters C writes handles into are
  one-element buffers. Two `free_ptr` calls remain in the whole directory, both
  for strings `to_cstr` allocated.
- **The maze's world became records.** Where it kept forty untyped slots in a
  raw block, it now has `Scene`, `Mesh`, `Torches`, `Overlay`, `Eye` and a
  `World` holding them: every field has a type the compiler checks, and the
  buffers among them are *held* rather than pointed at, which is what makes
  the counting correct. The mutable parts — the camera, the two matrices, the
  cursor reading — stay buffers inside those records, which is the division
  the language is built around: immutable structure, explicit mutable memory.

## Unreleased — a maze to walk around in

- **`examples/gl/maze.rill`**: a first-person maze, lit by torches, with
  nothing loaded from disk. The layout is carved by depth-first search over a
  raw-buffer grid with its own stack; the brick and flagstone textures are
  generated pixel by pixel; up to sixteen torches light the walls, each
  flickering at its own rate, drawn as quads that turn to face the camera;
  distance fades to the clear colour. **W A S D** walks, the mouse looks,
  walls are checked one axis at a time so a glancing collision slides.
- **Lighting with nothing made up in it.** The ambient is a thousandth, the
  wrap-around fill is gone, and every torch is a point source falling off with
  the inverse square of its distance, lit by the cosine of the surface's angle
  to it. Corridors out of reach of a flame are dark, and light along a wall
  rakes across the brick.
- **Fog that behaves like air, not like a filter.** Mixing towards a fixed
  colour lifted the far dark to grey. It is now absorption on what a surface
  sends you, plus in-scattering of the torches' light computed from where the
  *eye* is and saturating within a few strides. Unlit distance stays black.
- **A gait.** The camera bobs twice a stride and sways once, on distance
  covered rather than on time, easing in and out with a smoothed speed; the
  hand torch swings a little out of phase.
- **Bump mapping for free**: the textures carry their height in the alpha
  channel, and the shader tilts the normal by that channel's slope. Four extra
  samples turn flat quads into brickwork with edges that catch the light.
- **`to_float` and `to_int`**, compiled to a single instruction each. The two
  numeric types still never mix on their own, but crossing between them no
  longer means going through a string.
- **Sound**, through OpenAL: a looping fire on every torch, positioned in the
  world so it pans and fades as you move, and music under all of it —
  Rachmaninoff conducting his own *Isle of the Dead*, which is the painting
  hanging on one of the walls. OpenAL was chosen
  because it wants integer handles and raw pointers and never calls back from
  its own thread, which is the only shape safe next to a non-atomic reference
  counter.
- **A bloody steel edge.** The axe's head is repainted as steel — the patch of
  its palette sheet the edge points at, found from the mesh rather than by
  hand — and the blood over it is worked out per fragment from a noise field
  that creeps down the blade, because nine pixels of texture cannot hold a
  smear. Both catch the torchlight through the same gloss the wet blood on the
  walls uses.
- **Space swings the axe**: a wind-up, a fast chop that overshoots, and a slow
  settle back, with the arm reaching forward and dropping as it goes. The
  whole state is one float — when the swing started — and a second swing is
  refused until the first is done.
- **The flame in your hand is bigger and leans away from where you are going**
  — only its top two corners move, so the fire still leaves the wood where it
  was, and the amount is the walk's own, so it stands up when you stop. It
  also stands a little way towards you rather than through the middle of the
  torch: a billboard down the axis of the stick has the near half of the head
  in front of it, which cut the bottom of the fire off.
- **The torches are models now**, in your hand and on all twelve walls — the
  painted quads that stood in for them are gone, and the wood is lit by the
  maze's own shader like everything else. The one on the wall is stood at the
  foot of its flame with its upright axis pointed out of the stone, which the
  mesh loader gained a second kind of copy for.
- **An axe in one hand and the torch in the other.** The torch moved to the
  left, with its shaft turned to run from the fist back towards the eye rather
  than away from it, and the axe is a model carried in the right — the one
  thing in the maze that moves with the eye. The maze's vertex shader gained a
  `model` uniform for it, the identity everywhere else.
- **Dirt and wear on the flagstones too**, and twice as many drag marks over
  them: stones from different quarry faces, corners broken into hollows, and
  dirt gathering against the joints and walked off the middle of each stone,
  which is what a floor nobody has swept looks like.
- **Dirt and wear on the brick**, generated like the brick itself: soot in
  patches with streaks running down out of them, bricks from light and dark
  batches, corners knocked off into the mortar behind — a hollow, since the
  fourth channel is height — and salt blooming through. A wall that is only
  brick and mortar reads as a pattern; what is wrong with a real one is what
  tells you it is one.
- **Models**, loaded and drawn: what is left of people on the floor of the
  maze — a body, a hand, a heart, kidneys, a slab of flesh — five glTF models
  converted by `tools/fetch_bodies.py` into `bodies.rmsh`
  — a table of meshes and then vertices in the maze's own eight-float layout,
  with the four textures packed onto one sheet and the texture coordinates
  rewritten into it. `mesh.rill` copies a mesh into the world's buffer with
  its rotation already applied rather than carrying a model matrix, so every
  remain in the maze is one buffer, one texture and one draw call. Their
  textures are treated on the way in: the ones that arrived as a single flat
  colour — measured, two of them — are given a generated flesh instead, and
  all of them are stained and given height in the alpha channel, so the bump
  mapping has something to work with. The atlas tile was 128 pixels for
  everything, which threw away most of what the good textures had. Models that
  are modelled standing are laid down on the way in; nothing in this maze is
  upright.
- **Blood**, in three kinds and all of it photographed rather than drawn
  (`tools/fetch_blood.py`, all CC0): splatter stains on wall faces, runs down
  the wall cut from strips of a blood-stained wall, and drag marks on the
  floor cut from a forensic photograph of a wipe trace, laid along the
  corridor. The runs were drawn by hand first and looked like candle wax —
  blood is a millimetre of liquid, not a rope of it. The photographs have no
  alpha channel, so one is keyed out of how much redder than everything else
  each pixel is, and every crop is chosen by measuring rather than by eye.
  Which kind a tile is comes from its shape, so the counts are not written
  down twice.
- **Wet blood.** The same alpha that gives a decal its relief drives a
  specular highlight off the same tilted normal, so a run catches the torch
  along its ridge and its dry thin edges do not. It costs the brick nothing:
  `decal` is zero over every wall fragment and the branch is skipped.
- Windows open **in the middle of the screen** rather than wherever the window
  manager last left one.
- **The resident set, in the top right corner** of the maze, beside the frame
  rate in the left — both labelled, in block letters, since seven segments
  cannot make an M: megabytes as the kernel counts them, through one Mach
  `task_info` call (`mem.rill`), sampled once a second rather than per frame.
  Checked against `ps`: the same number to the kilobyte.
- **Three cries**, played at intervals and from bearings the maze's own
  generator decides, quiet and falling off fast — a scream three corridors
  away should be a doubt rather than an event.
- **MP3 through libmpg123** (`mp3.rill`) for every recording: 336 KB of file
  where the samples would have been 2.6 MB, decoded once at startup into a
  `Buf(I16)`. The first read reports the format it settled on rather than a
  sample, which is not an end — reading it as one is why the first attempt
  decoded nothing. Fed from memory rather than opened by path, so the bytes
  come straight out of the pack.
- **One file for all the data** (`assets.rpak`, `pack.rill`,
  `tools/pack_assets.py`): a directory of names and a concatenation, holding
  the atlas and the five recordings. Everything in it arrives compressed, so
  it adds none of its own; it is read whole and handed out as pointers into
  that one block.
- Five public-domain recordings and `tools/fetch_sound.py` to reproduce them,
  measuring which window of each to keep and handing them to `lame`. The music
  is under a minute of the piece, cut at the quietest second near the minute
  mark and cross-faded into its own opening so that looping it has no seam;
  mono at 22050 Hz and forty kilobits, since that is all the program mixes at.
  Encoding costs neither loop its join: lame writes down its own delay and
  mpg123 reads it, so what comes back is sample for sample what went in.
- A third generated texture for the ceiling — boards across beams — with the
  floor and the ceiling built as two runs of one mesh so each can wear its own.
- **The torches have brackets**, drawn from the same buffer in an opaque pass
  before the fire's additive one, with the flame seated on the tip.
- **The torches burn in a shader.** The flat tapered quad is gone: each flame
  is now value noise scrolling upward through a narrowing shape, threshold-cut
  so its edge flickers, coloured red to white from rim to core, seeded per
  torch and blended additively.
- Paintings are spread by counting the wall faces first and hanging one every
  so many, rather than by a hash of the coordinates — which, with a modulus
  that divided one of its own coefficients, had been ignoring the x axis
  entirely and lighting up whole rows. A stone carries one fixture and its
  neighbours along the same wall carry none, so torches and canvases never
  share a face and two canvases never sit side by side.
- **A texture packer** (`tools/pack_art.py`): shelf packing, with every sheet
  width worth trying tried and the smallest area kept, so twenty 160×160 tiles
  come out as a 640×800 sheet with no waste. The result is deflated into one
  `.rtex` file — 2 MB of pixels in 917 KB, losslessly — and read back with a
  file read, one call into the system's zlib through the FFI, and one upload.
  Sharing a sheet also collapses twenty draw calls into one. (PNG-style row
  filtering was measured first: 1.6% over deflate alone on these images, not
  worth the loop on the reading side.)
- **Paintings on the walls.** Twenty public-domain works — Goya, Caravaggio,
  Bruegel, Gentileschi, Géricault, Rembrandt and more — hang on wall faces the
  corridors can see, each exactly once, lit by the torches through the maze's
  own shader. There is no image decoder in the language, so `tools/fetch_art.py`
  fetches the scans from Wikimedia Commons, frames them and writes the raw
  160×160 RGBA bytes; `art.rill` reads a file and hands the bytes straight to
  `glTexImage2D`, which is what a counted string being binary-safe buys.
- `examples/gl/mazemap.rill` prints the carved maze as text, with no window
  and no OpenGL, which is how the generator is tested.

## Unreleased — smaller binaries

A program that uses no concurrency was still paying for all of it. Measured
against the same program written in C (`examples/gl/reference/triangle.c`),
which links the same GLFW and OpenGL: **54.6 KB → 37.6 KB, against C's
36.4 KB**. The gap went from 18 KB to 1.2 KB.

- **No scheduler unless something can wait.** If nothing in the emitted module
  calls into `spawn`, a channel, `select` or a socket, `main` runs straight on
  the process stack through a new `rill_rt_run_direct`, and the linker drops
  the worker loop, the run queues, the poller and the stack switching with it.
  The question is asked of the finished module, so a channel in a function
  nothing reaches costs nothing.
- **The worker table no longer sits in the executable.** One non-zero byte —
  a statically initialised mutex, a `-1` sentinel — put all 9 KB of it in
  `__DATA`, which is written to the file. The statics start zeroed and
  `rill_rt_run` fills in what it uses, so the table lives in `__bss` instead
  and costs nothing on disk.
- **String literals and nullary constructors are read-only.** They were
  writable only because reference counting wrote to them, so counting now
  skips anything already at the immortal count. Being constants, they move out
  of `__DATA` entirely — which is what removes the segment, and with it a
  whole 16 KB page from every binary.
- The skip is a branch, not a conditional store address. The first attempt
  redirected the store to a scratch word to stay branchless and cost **28%**
  on `trees`: a store through a selected pointer takes away what the optimizer
  knew about what it touches. Written as a branch, the common path is
  byte-for-byte what it was, and `trees`, `fib` and `lcg` all measure
  unchanged.

## Unreleased — C widths and raw buffers

The FFI could only describe C functions written in 64-bit numbers, which is
almost none of them. Both halves of that gap are closed.

- **Foreign signatures can name a width.** `I8`, `U8`, `I16`, `U16`, `I32`,
  `U32`, `I64`, `U64`, `F32` and `F64` are spellable inside an `extern`, and
  the boundary converts: arguments truncate or `fptrunc` on the way in,
  results sign- or zero-extend on the way back according to the declared
  signedness. `Int` and `Float` keep meaning the 64-bit forms. This matters
  most for `float`: a C function declared `Float` and handed a double read the
  wrong half of the register and quietly returned nonsense — `sqrtf(2.0)` came
  back 0. Integer arguments happened to work by accident before, since the
  callee reads the low half of the register; they are now correct by
  construction.
- **Raw buffers**: `alloc_bytes`, `ptr_offset`, `poke_i8/i16/i32/i64`,
  `poke_f32/f64`, `peek_i8/u8/i16/u16/i32/u32/i64` and `peek_f32/f64`. Plain
  C memory outside the reference counter, freed with `free_ptr`, indexed by
  element rather than byte, and unaligned-safe so an offset pointer stays
  usable. This is what lets a program hand an array — vertex data, a pixel
  buffer, a matrix — to a C library.
- **Pointer slots**: `poke_ptr` and `peek_ptr`, for the `char**` a C API takes
  and the handle it writes back through an out-parameter. `ptr_offset` may now
  be applied to null, because a byte offset carried in a pointer-shaped
  argument is how OpenGL and friends describe a location in a buffer.
- **Linking**: a library string starting with `-` goes to the linker verbatim,
  so a program can say where its libraries live, and `framework:Name` links a
  macOS framework. Search paths are emitted before the libraries that need
  them, whichever declaration carried them.
- **Callbacks**: `Fn(...)` in a foreign signature is a C function pointer, and
  a top-level Rill function can be passed as one — codegen emits a C-ABI
  trampoline per (function, instantiation, signature) that widens C's
  arguments and calls through. Closures are refused with a message that says
  why (an environment C cannot hold), a callback returning a heap value is
  refused because nothing would release it, and `Fn` as a *return* type is
  refused too. This closes the last gap that forced polling: GLFW key and
  resize events are delivered now.
- `extern fn f(x: Unit)` used to panic the compiler instead of being rejected.
- `examples/ffi_buffers.rill` covers all of it, with an e2e test; the first
  line it prints is the one that fails on the old compiler.
- **`examples/gl/` drives OpenGL with no C anywhere**: GLFW opens a window,
  shaders are compiled, vertex data goes up in a raw buffer, and a triangle
  spins with a seven-segment frame counter drawn as rectangles. A headless
  variant renders the same scene through CGL into a renderbuffer and prints
  the pixels as text, which is what the test suite checks — no display, no
  window server, same GPU path. Escape and window resizing arrive as
  callbacks rather than being polled.
- `examples/callbacks.rill`: C calling back into Rill, during `main` through a
  signal handler and after it through `atexit`.

## Unreleased — editor support

- **A Visual Studio Code extension** (`editors/vscode/`): a TextMate grammar
  that highlights every construct in the language — declarations, `match` and
  `select` arms with `default`, lambdas, the pipeline operator, `import ... as`
  aliases and the qualified names they introduce — and separates built-in
  types, prelude types and constructors, built-in functions and prelude
  functions, so what the compiler treats differently also looks different. It
  marks the string escapes the lexer rejects as errors, sets two-space
  spaces-only indentation with automatic indent after `=`, `->`, `select`,
  `match x` and block headers, and ships snippets.
- The grammar has a test: `npm test` in `editors/vscode/` tokenizes
  `test/fixture.rill` — a compilable file exercising the whole language — and
  checks 43 scope expectations, that nothing falls through unscoped, and that
  nothing valid is flagged invalid.

## 0.2.10 — HTTP keep-alive, and an HTTP benchmark

- **Keep-alive in the HTTP server** (`examples/http/http.rill`): a connection
  now serves request after request until one side asks to stop, following
  HTTP/1.1's default and the client's `Connection:` header. The client side
  gained `get_all`, which sends every path down one connection and reads each
  response by `Content-Length` rather than by end of stream.
- **An HTTP benchmark against Go** (`benchmarks/http/`): the same one-route
  service on Rill's module and on Go's `net/http`, driven by `wrk` in both
  keep-alive and connection-per-request modes. Rill serves 213.8k req/s to
  Go's 197.8k with keep-alive, 42.9k to Go's 13.1k without, in 7× less memory
  and a 52 KB binary against 7.2 MB. Go keeps the better tail latency under
  connection churn, which the write-up says plainly.

## 0.2.9 — YAML

- **A YAML reader in Rill** (`examples/yaml/`) for what configuration files
  actually use: nested mappings by indentation, `- ` sequences, lists of
  mappings, comments, quoted and bare scalars. Anchors, aliases, multiple
  documents, flow collections and block scalars are out of scope, and said so.
- It parses into the JSON module's value type, so a YAML document prints as
  JSON without conversion — and the two modules import across directories.

## 0.2.8 — JSON

- **A JSON parser and printer in Rill** (`examples/json/`): a recursive-descent
  parser returning value-and-position, escapes including `\uXXXX` for the
  Latin-1 range, a printer that round-trips, and lookups (`get`, `path`,
  `items`, `as_str` / `as_float` / `as_bool`).
- `str_to_float`, which JSON numbers need.

## 0.2.7 — bit operations, and WebSocket

- **Bit operations**: `bit_and`, `bit_or`, `bit_xor`, `bit_not`, `shl`, `shr`,
  compiled to single instructions. Without them no binary protocol could be
  written in the language.
- **SHA-1 and base64 in Rill** (`examples/http/sha1.rill`), matching the
  published test vectors, including RFC 6455's example key.
- **A WebSocket server in Rill** (`examples/http/ws.rill`): the handshake and
  text frames both ways, masking included. Verified against an independent
  client for the accept-key computation and for both the 7-bit and 16-bit
  length paths.

## 0.2.6 — sockets and an HTTP server

- **TCP sockets integrated with the scheduler.** `tcp_listen`, `tcp_accept`,
  `tcp_connect`, `sock_read`, `sock_write` and `sock_close`, with `Result`
  wrappers in the prelude. Sockets are non-blocking underneath: a strand
  that would block parks and its descriptor is handed to `kqueue`, which the
  scheduler polls when it runs out of runnable strands. One strand per
  connection therefore costs nothing while it waits, and an idle server is not
  mistaken for a deadlock.
- **Functions are now checked in dependency order.** Mutually recursive
  functions are inferred as a group, and a function whose type is settled by a
  later definition comes out right — previously that quantified its return type
  into a variable nothing could satisfy, and the program failed to compile.
- **An HTTP/1.1 server written in Rill** (`examples/http/`): request parsing,
  routing, responses, and a client, in about 100 lines on top of the socket
  builtins. It answers `curl`, and serves 30 concurrent connections with a
  strand each.
- Strings understand `\r`, which HTTP needs.

## 0.2.5 — calling C

- **A foreign function interface.** `extern "m" fn sqrt(x: Float) -> Float`
  declares a C symbol and the library to link; the compiler passes `-lm` to the
  linker and calls it directly. Signatures accept the shapes the C ABI carries
  (`Int`, `Float`, `Bool`, `Ptr`, `Unit`) and reject the rest with an
  explanation.
- **`Ptr`**, an opaque C pointer that reference counting leaves alone, plus
  `to_cstr` / `from_cstr` / `free_ptr` / `null_ptr` / `is_null` for moving
  strings across the boundary — a Rill `Str` is a counted heap object, not a
  `char*`.
- An `extern` declaration shadows a prelude function of the same name, exactly
  as a definition does.

## 0.2.4 — source-level debugging, qualified imports, matching in parentheses

- **`--debug` builds carry DWARF line information**, so `lldb` stops on Rill
  source lines (`b fib` resolves to `fib.rill:2`) instead of raw symbols. The
  object file is kept next to the binary and `dsymutil` runs when available,
  which is how macOS reaches the DWARF.
- **`import "x" as m`** qualifies a module: everything it defines is reached as
  `m.name`, so two files may define the same function, type or constructor.
  Unaliased imports keep working exactly as before.
- **`match` and `select` now work inside parentheses**, where their arms follow
  one another on the line: `println(match o None -> 0 Some(n) -> n)`. The
  formatter prints nested matches in that form and no longer refuses to format
  a file that contains one.
- **The scheduler no longer shares one lock.** Each worker guards its own run
  queue and recycles its own strand records, so workers that are simply
  running strands never contend; channel operations keep a single lock,
  always taken before a run queue's. Deadlock detection now confirms the
  quiet state over many rounds rather than trusting one snapshot.

## 0.2.3 — select

- **`select`** waits on several channels at once, with `recv` arms that bind
  what arrives, `send` arms, and an optional `default` for polling. Arms are
  tried in order, so an earlier one wins a tie.
- Channel wait queues now hold per-case waiter nodes rather than strands, so
  one strand can wait in many queues; the first channel to reach it claims
  it and the rest are unlinked when it wakes.
- **Layout fix**: a `match` or `select` used as a non-final statement in a block
  no longer swallows the line break that separates it from the next statement.

## 0.2.2 — generic trait impls, buffered channels

- **`impl Show(List(a))`** and friends: a trait implementation may target a
  generic type by naming its parameters, covering every instantiation. Method
  bodies stay generic and resolve their own trait calls per element type, so
  `List(Box(Int))` composes two impls with no `where` clauses. An implementation
  that only works for one instantiation is rejected with an explanation.
- **Buffered channels**: `channel(n)` carries a ring buffer of `n` values in
  the same allocation as the channel, so a producer can run ahead of its
  consumer. `channel()` is unchanged — capacity zero is still a rendezvous.

## 0.2.1 — multi-core scheduler

- **Strands can run on several cores.** `rill build --parallel` makes
  reference counting multi-thread safe, and `RILL_THREADS=N` then spreads
  strands over N worker threads. Eight independent computations run 3.6×
  faster on this machine; a binary built without the flag says so and stays on
  one worker rather than running unsoundly.
- Workers steal strands that have not started yet. One that has already run
  is pinned to its worker, since the copy-on-block stack model restores its
  frames to that worker's addresses.
- Deadlock detection is now exact — no runnable strand anywhere and none
  executing — instead of the "every worker looks idle" snapshot, which could
  fire spuriously at startup.
- Reference-count updates are inlined into generated code, so single-worker
  programs no longer call into the runtime to retain a value.

## 0.2.0 — nested patterns, constrained generics, trait-driven operators

**Language**

- **Patterns nest freely.** `Cons(Some(0), rest)` and deeper now work anywhere a
  pattern does. Matches compile to a decision tree, so each value is tested once
  and no arm's body is duplicated.
- **Exhaustiveness checking names a missing case.** A non-exhaustive match
  reports an actual unmatched value — `no arm matches Cons(None, Cons(_, _))` —
  and arms that earlier arms already cover are rejected.
- **Constrained generics.** Requirements a body imposes (`<`, `+`, `println`)
  travel with the function's type instead of pinning it to `Int`. `fn sort(l)`
  is generic over ordered types; `fn announce(x) = println(x)` works for every
  printable type. Using a function at a type that doesn't satisfy a requirement
  is a compile error naming both.
- **Built-in traits `Show` and `Ord`.** `impl Show(T)` replaces a type's derived
  printing (including inside other values); `impl Ord(T)` gives it `<`, `<=`,
  `>`, `>=`. Strings are ordered without any impl.
- **`if` may span lines** — `then` and `else` can start a continuation line.
- A type and one of its constructors may share a name (`type Pair` with
  `Pair(x, y)`).

**Fixes**

- The formatter no longer drops the arms of a `match` nested inside an `if`
  branch, and refuses to format rather than corrupt a construct it cannot
  reproduce.
- `||` and `&&` chains no longer gain redundant parentheses when reformatted.

## 0.1.0 — the language

- **M0** Native pipeline: lexer with indentation layout, parser, LLVM backend,
  linked executables.
- **M1** Type inference, algebraic data types, `match` with exhaustiveness.
- **M2** Let-polymorphism, generic ADTs, lambdas and closures, monomorphization,
  tail-call elimination. C-sized binaries (~33 KB) via a `no_std` runtime.
- **M3** Perceus-style compile-time reference counting: no GC, no pauses,
  verified leak-free by a live-allocation counter.
- **M4** Go-style concurrency: `spawn`, typed rendezvous channels, a green-thread
  scheduler with hand-written arm64 context switching.
- **M5** A standard library written in Rill, and counted heap strings.
- **M6** Derived `show` and structural `==` for every data type;
  user-defined `trait` / `impl` with fully static dispatch.
- **M7** File and process IO, string processing, logical operators, a module
  system (`import`), `rill fmt`, `rill repl`, and `--debug` builds.

**Performance work** (see `benchmarks/BENCHMARKS.md`): arena strand stacks and
release-before-tail-call codegen took the 100k-strand ring from 321 ms to
13 ms; nullary-constructor singletons and a size-class pool allocator took
binary-trees from 125 ms to 34 ms. Rill leads Go and OCaml on all four
benchmarks in time, peak memory and binary size.
