// The host half of the WebGPU demo: everything asynchronous or stateful that
// a wasm module cannot do for itself. It owns the device, the pipeline and
// the frame loop; Rill owns what happens inside a frame. The two meet at the
// imports `gpu_clear` and `gpu_triangle`, and at the exports `rill_init` and
// `frame`. The plumbing between them — WASI shim, instantiation, init — is
// `triangle.js`, which `rill build` generates beside the module.

import { load } from "./triangle.js";

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

  const rill = await load(wasmURL, env);
  instance = { exports: rill };   // the imports above read its memory

  const t0 = performance.now();
  function tick() {
    instance.exports.frame((performance.now() - t0) / 1000);
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
}
