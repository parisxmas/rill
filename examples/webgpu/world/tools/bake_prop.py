#!/usr/bin/env python3
"""Bakes a static glTF prop into the one file the page loads.

    python3 tools/bake_prop.py ammo_box_1k.gltf ammo-box.mesh

The format is the rifle's, because the rifle already had it and a second
format would have been a second loader for no reason:

    u32     vertices
    u32     indices
    f32[6]  bounding box, min xyz then its size — not its max. The shader
            unpacks a position as `min + q * size`, and the rifle and the
            shark are both written that way.
    vertex  u16[4] position, unorm across that box, the fourth spare
            i8[4]  normal, snorm, the fourth spare
            u16[2] texture coordinate, unorm
    u16     indices

Sixteen bytes a vertex against a float's thirty-two, and indexed.

A prop is several nodes — a box, a handle, a latch, two rings — and they are
welded into one buffer here rather than drawn as five. Node translations are
applied on the way in; rotation and scale are asserted away rather than
supported, because no prop in this valley has either and code for a case that
never arrives is code nobody has ever run.
"""
import json
import struct
import sys
from pathlib import Path

COMPONENT = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
COUNT = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}


def read(gltf, blob, index):
    """One accessor, as a list of tuples (or of scalars for SCALAR)."""
    acc = gltf["accessors"][index]
    view = gltf["bufferViews"][acc["bufferView"]]
    fmt = COMPONENT[acc["componentType"]]
    n = COUNT[acc["type"]]
    size = struct.calcsize(fmt) * n
    start = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    stride = view.get("byteStride") or size
    out = []
    for i in range(acc["count"]):
        at = start + i * stride
        v = struct.unpack_from("<" + fmt * n, blob, at)
        out.append(v[0] if n == 1 else v)
    return out


def main(src, dst):
    src = Path(src)
    gltf = json.loads(src.read_text())
    blobs = {}
    for i, buf in enumerate(gltf["buffers"]):
        blobs[i] = (src.parent / buf["uri"]).read_bytes()
    # One buffer in every file this has been pointed at; the loader above is
    # general and this is the check that says so.
    blob = blobs[0]

    verts, idx = [], []
    for node in gltf["nodes"]:
        if "mesh" not in node:
            continue
        assert "rotation" not in node and "scale" not in node, node.get("name")
        tx, ty, tz = node.get("translation", (0.0, 0.0, 0.0))
        for prim in gltf["meshes"][node["mesh"]]["primitives"]:
            a = prim["attributes"]
            pos = read(gltf, blob, a["POSITION"])
            nor = read(gltf, blob, a["NORMAL"])
            uv = read(gltf, blob, a["TEXCOORD_0"])
            base = len(verts)
            for p, n, t in zip(pos, nor, uv):
                verts.append(((p[0] + tx, p[1] + ty, p[2] + tz), n, t))
            idx.extend(base + i for i in read(gltf, blob, prim["indices"]))

    assert len(verts) < 65536, f"{len(verts)} vertices will not index in u16"

    lo = [min(v[0][i] for v in verts) for i in range(3)]
    hi = [max(v[0][i] for v in verts) for i in range(3)]
    span = [max(hi[i] - lo[i], 1e-9) for i in range(3)]

    out = bytearray()
    out += struct.pack("<II6f", len(verts), len(idx), *lo, *span)
    for (p, n, t) in verts:
        q = [min(65535, max(0, round((p[i] - lo[i]) / span[i] * 65535))) for i in range(3)]
        # snorm8 is -128..127 and -128 is not -1.0 after the usual decode, so
        # the round is clamped at -127 rather than left to wrap.
        s = [min(127, max(-127, round(n[i] * 127))) for i in range(3)]
        # Texture coordinates outside the unit square would wrap here rather
        # than tile; nothing in this asset does, and the assert is cheaper than
        # finding out from a smeared face.
        assert -0.001 <= t[0] <= 1.001 and -0.001 <= t[1] <= 1.001, t
        u = [min(65535, max(0, round(min(max(c, 0.0), 1.0) * 65535))) for c in t]
        out += struct.pack("<4H4b2H", q[0], q[1], q[2], 0, s[0], s[1], s[2], 0, u[0], u[1])
    for i in idx:
        out += struct.pack("<H", i)

    Path(dst).write_bytes(out)
    print(f"{dst}: {len(verts)} vertices, {len(idx) // 3} triangles, {len(out)} bytes")
    print(f"  box {['%.3f' % v for v in lo]} + {['%.3f' % v for v in span]}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
