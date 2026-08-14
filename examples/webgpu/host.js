// The host half of the WebGPU demo: everything asynchronous or stateful that
// a wasm module cannot do for itself. It owns the device, the pipeline and
// the frame loop; Rill owns what happens inside a frame. The two meet at the
// imports `gpu_clear` and `gpu_triangle`, and at the exports `rill_init` and
// `frame`.

// Just enough WASI for a reactor built against wasi-libc: `println` arrives
// here as `fd_write`, and the rest only has to exist. Browsers ship no WASI,
// so this is the whole dependency.
function wasiShim(getMemory) {
  const text = new TextDecoder();
  let carry = "";
  return {
    fd_write(fd, iovs, iovcnt, nwritten) {
      const mem = getMemory();
      const view = new DataView(mem.buffer);
      let written = 0;
      let out = "";
      for (let i = 0; i < iovcnt; i++) {
        const ptr = view.getUint32(iovs + i * 8, true);
        const len = view.getUint32(iovs + i * 8 + 4, true);
        out += text.decode(new Uint8Array(mem.buffer, ptr, len));
        written += len;
      }
      // Lines go to the console whole; a trailing fragment waits for its \n.
      carry += out;
      const lines = carry.split("\n");
      carry = lines.pop();
      for (const line of lines) console.log(`[rill] ${line}`);
      view.setUint32(nwritten, written, true);
      return 0;
    },
    fd_close: () => 0,
    fd_seek: () => 0,
    fd_prestat_get: () => 8, // EBADF: no preopened directories
    fd_prestat_dir_name: () => 8,
    args_sizes_get(argc, argvBufSize) {
      const view = new DataView(getMemory().buffer);
      view.setUint32(argc, 0, true);
      view.setUint32(argvBufSize, 0, true);
      return 0;
    },
    args_get: () => 0,
    environ_sizes_get(count, bufSize) {
      const view = new DataView(getMemory().buffer);
      view.setUint32(count, 0, true);
      view.setUint32(bufSize, 0, true);
      return 0;
    },
    environ_get: () => 0,
    proc_exit(code) {
      throw new Error(`wasm exited with code ${code}`);
    },
  };
}

const WGSL = `
  @vertex fn vs(@location(0) pos: vec2f) -> @builtin(position) vec4f {
    return vec4f(pos, 0.0, 1.0);
  }
  @fragment fn fs() -> @location(0) vec4f {
    return vec4f(0.98, 0.75, 0.14, 1.0);   // kingfisher gold
  }
`;

export async function run(canvas, wasmURL) {
  if (!navigator.gpu) throw new Error("this browser has no WebGPU");
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) throw new Error("no GPU adapter");
  const device = await adapter.requestDevice();
  const context = canvas.getContext("webgpu");
  const format = navigator.gpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "opaque" });

  const module = device.createShaderModule({ code: WGSL });
  const pipeline = device.createRenderPipeline({
    layout: "auto",
    vertex: {
      module,
      buffers: [{
        arrayStride: 8,
        attributes: [{ shaderLocation: 0, offset: 0, format: "float32x2" }],
      }],
    },
    fragment: { module, targets: [{ format }] },
  });
  const vbuf = device.createBuffer({
    size: 64,
    usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
  });

  // What Rill's two imports do. `gpu_clear` remembers; `gpu_triangle` draws —
  // one render pass per frame, using whatever the module just computed.
  let clear = { r: 0, g: 0, b: 0, a: 1 };
  let instance; // set below; the imports need its memory
  const env = {
    gpu_clear(r, g, b) {
      clear = { r, g, b, a: 1 };
    },
    gpu_triangle(ptr, count) {
      const n = Number(count);
      const verts = new Float32Array(instance.exports.memory.buffer, ptr, n * 2);
      device.queue.writeBuffer(vbuf, 0, verts);
      const encoder = device.createCommandEncoder();
      const pass = encoder.beginRenderPass({
        colorAttachments: [{
          view: context.getCurrentTexture().createView(),
          clearValue: clear,
          loadOp: "clear",
          storeOp: "store",
        }],
      });
      pass.setPipeline(pipeline);
      pass.setVertexBuffer(0, vbuf);
      pass.draw(n);
      pass.end();
      device.queue.submit([encoder.finish()]);
    },
  };

  const bytes = await (await fetch(wasmURL)).arrayBuffer();
  const wasm = await WebAssembly.compile(bytes);
  instance = await WebAssembly.instantiate(wasm, {
    env,
    wasi_snapshot_preview1: wasiShim(() => instance.exports.memory),
  });
  instance.exports._initialize(); // libc setup
  instance.exports.rill_init();   // Rill's main, once

  const t0 = performance.now();
  function tick() {
    instance.exports.frame((performance.now() - t0) / 1000);
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
}
