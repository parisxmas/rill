# OpenGL from Rill

A spinning triangle, drawn by the GPU, with no C written anywhere — no shim
library, no generated bindings, no build script. The whole path from `main` to
the driver is `extern` declarations plus raw buffers.

```sh
brew install glfw
rill run examples/gl/triangle.rill    # a window, and a frame counter
rill run examples/gl/offscreen.rill   # the same triangle as text, no window
rill run examples/gl/mazemap.rill     # just the maze carver, as text
```

## The files

The foreign declarations are collected the way a C project collects headers,
so each program is only its own logic.

| | |
|---|---|
| `gl.rill` | OpenGL: every `extern`, the constants a header would give, and the small bridges they need |
| `glfw.rill` | GLFW: windows, the event pump, the clock |
| `cgl.rill` | CGL: macOS's way to get a context with no window behind it |
| `scene.rill` | the triangle — shaders, vertex data, the draw call |
| `hud.rill` | seven-segment readouts, drawn as rectangles |
| `mazegen.rill` | the maze, carved by depth-first search |
| `triangle.rill` | the windowed program |
| `offscreen.rill` | the headless one, which is what the test suite runs |
| `mazemap.rill` | the carver on its own, printed as text — what the tests run |
| `reference/triangle.c` | the same program in C, for reading side by side |

The maze these bindings grew out of is its own project now, in
[`../../artvandal`](../../../artvandal): a torch-lit first-person maze, with
the paintings, the models and the sound. It carries its own copies of
`gl.rill`, `glfw.rill`, `hud.rill` and `mazegen.rill`.

The C version is there to be diffed, not to be beaten: same window, same
shaders, same vertex data, same counter, same twenty seconds. It is 204 lines
of code to Rill's 248 once the `extern` blocks — which C gets from headers —
are set aside, and it links a 36.4 KB binary against Rill's 37.6 KB.

```sh
cc -O2 -o triangle reference/triangle.c -I/opt/homebrew/include \
   -L/opt/homebrew/lib -lglfw -framework OpenGL
```

## What makes it work

**C's widths.** A foreign signature names the type the symbol was compiled
with, not Rill's: `U32` for a `GLenum`, `I32` for a `GLint`, `F32` for a
`GLfloat`. This is not a detail — `glClearColor` takes four 32-bit floats, and
declaring them `Float` would hand it doubles and clear the window to garbage.

**Typed buffers.** `buf_f32(n)` is a counted block of 32-bit floats: vertex
data, matrices, torch positions. `buf_u8` holds generated pixels, `buf_u32`
catches the name `glGenBuffers` writes into an out-parameter, `buf_ptr` builds
the array of string pointers `glShaderSource` wants. Indexing is checked,
`buf_data` hands C the address, and nothing here is freed by hand.
`ptr_offset(null_ptr(), 8)` is still the byte-offset-in-a-pointer that
`glVertexAttribPointer` expects.

**Linking.** A library string that starts with `-` goes to the linker as
written (`"-L/opt/homebrew/lib"`), and `framework:OpenGL` links a macOS
framework. Both are in the declarations, so `rill run` needs no extra flags.

## Notes

- macOS only, and OpenGL is deprecated there — Apple caps it at 4.1. It still
  works, and it is the shortest path from a language to a GPU.
- The window closes itself after `run_seconds()`, or on Escape.
- No mutation and no loops: the frame counter and the render state are carried
  through the tail-recursive `loop`, one frame at a time.
- Escape and window resizing arrive as **callbacks**: `on_key` and
  `on_resize` are plain Rill functions handed to GLFW as C function pointers.
  Nothing is captured, because C has nowhere to keep an environment — each one
  works from the arguments it is given.
