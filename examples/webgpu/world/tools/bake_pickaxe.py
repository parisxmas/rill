#!/usr/bin/env python3
"""Bakes Kenney's pickaxe — Survival Kit 2.0, CC0 — into pick.mesh.

    python3 tools/bake_pickaxe.py path/to/tool-pickaxe.glb path/to/colormap.png

The format is the rifle's sixteen bytes a vertex with one difference worth
the word: the texture-coordinate slot carries a colour instead. Every sampled
binding this renderer has is spent — sixteen of sixteen — so the colormap
cannot ride along as a seventeenth texture. It does not need to: Kenney's
colormap is a palette of flat patches and a UV points into the middle of one,
so the colour is read here, once, per vertex, and baked in. What the shader
loses is nothing (there is no gradient to filter); what the bundle gains is a
mesh that needs no image at all.

    u32     vertices
    u32     indices
    f32[6]  bounding box, min xyz then size
    vertex  u16[4] position, unorm across the box, the fourth spare
            i8[4]  normal, snorm, the fourth spare
            u8[4]  colour, sRGB as sampled, alpha spare
    u16     indices
"""
import json
import struct
import sys
from pathlib import Path

from PIL import Image


def accessor(gltf, blob, index):
    comp = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
    count = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}
    acc = gltf["accessors"][index]
    view = gltf["bufferViews"][acc["bufferView"]]
    fmt = comp[acc["componentType"]]
    n = count[acc["type"]]
    size = struct.calcsize(fmt) * n
    start = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    stride = view.get("byteStride") or size
    out = []
    for i in range(acc["count"]):
        v = struct.unpack_from("<" + fmt * n, blob, start + i * stride)
        out.append(v[0] if n == 1 else v)
    return out


def main():
    glb = Path(sys.argv[1]).read_bytes()
    palette = Image.open(sys.argv[2]).convert("RGB")
    assert glb[:4] == b"glTF"
    jlen = struct.unpack_from("<I", glb, 12)[0]
    gltf = json.loads(glb[20:20 + jlen])
    # The binary chunk follows the JSON one, each with an 8-byte header.
    bat = 20 + jlen
    blen = struct.unpack_from("<I", glb, bat)[0]
    blob = glb[bat + 8:bat + 8 + blen]

    prim = gltf["meshes"][0]["primitives"][0]
    pos = accessor(gltf, blob, prim["attributes"]["POSITION"])
    nrm = accessor(gltf, blob, prim["attributes"]["NORMAL"])
    uv = accessor(gltf, blob, prim["attributes"]["TEXCOORD_0"])
    idx = accessor(gltf, blob, prim["indices"])

    lo = [min(p[i] for p in pos) for i in range(3)]
    hi = [max(p[i] for p in pos) for i in range(3)]
    size = [max(hi[i] - lo[i], 1e-6) for i in range(3)]

    out = bytearray()
    out += struct.pack("<II", len(pos), len(idx))
    out += struct.pack("<6f", *lo, *size)
    w, h = palette.size
    for p, n, t in zip(pos, nrm, uv):
        q = [round((p[i] - lo[i]) / size[i] * 65535) for i in range(3)]
        out += struct.pack("<4H", *q, 0)
        out += struct.pack("<4b", *(max(-127, min(127, round(c * 127))) for c in n), 0)
        px = palette.getpixel((min(w - 1, int(t[0] % 1.0 * w)), min(h - 1, int(t[1] % 1.0 * h))))
        out += struct.pack("<4B", *px, 255)
    for i in idx:
        out += struct.pack("<H", i)

    here = Path(__file__).resolve().parent.parent
    (here / "pick.mesh").write_bytes(out)
    print(f"pick.mesh written: {len(pos)} vertices, {len(idx)} indices, "
          f"box {[round(v, 3) for v in lo]} + {[round(v, 3) for v in size]}")


if __name__ == "__main__":
    main()
