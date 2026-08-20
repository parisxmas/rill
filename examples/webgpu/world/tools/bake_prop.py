#!/usr/bin/env python3
"""Bakes a static glTF prop into the one file the page loads.

    python3 tools/bake_prop.py ammo_box_1k.gltf ammo-box.mesh
    python3 tools/bake_prop.py rpg7.gltf rpg7.mesh --node RPG7_node --scale 100

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
welded into one buffer here rather than drawn as five, with each node's own
transform applied on the way in.

`--node NAME` takes only the nodes named, which is how one file holding a
launcher and the rocket it fires becomes two meshes off one texture. `--scale`
divides the positions afterwards: a glTF converted out of FBX arrives in
centimetres, and this valley is in metres.

`--align` turns the model so that its own long axis is +X, which is the
convention every shader here reads a baked mesh with. A model authored at an
angle in its own space is not a rendering problem to be corrected with two more
constants in a shader — it is a fact about the file, and the place to settle it
is once, here, where it can be measured rather than guessed at.
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


def matmul(m, v):
    """glTF stores a node matrix column-major; this is m * (v, 1) in that layout."""
    return tuple(
        m[0 + i] * v[0] + m[4 + i] * v[1] + m[8 + i] * v[2] + m[12 + i]
        for i in range(3)
    )


def matmul3(m, v):
    """The same without the translation — for normals. Renormalised by the
    caller, which is right for a rotation with a uniform scale and is all any
    node in these files has."""
    return tuple(m[0 + i] * v[0] + m[4 + i] * v[1] + m[8 + i] * v[2] for i in range(3))


def node_matrix(node):
    if "matrix" in node:
        return node["matrix"]
    tx, ty, tz = node.get("translation", (0.0, 0.0, 0.0))
    assert "rotation" not in node and "scale" not in node, node.get("name")
    return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, tx, ty, tz, 1]


def long_axis(pts):
    """The direction the model mostly lies along, by power iteration on the
    covariance. A launcher is a tube: one axis carries nearly all of the
    variance and the answer is not close."""
    n = len(pts)
    mean = [sum(p[i] for p in pts) / n for i in range(3)]
    cov = [[sum((p[a] - mean[a]) * (p[b] - mean[b]) for p in pts) / n for b in range(3)]
           for a in range(3)]
    v = [1.0, 0.3, 0.1]
    for _ in range(200):
        w = [sum(cov[a][b] * v[b] for b in range(3)) for a in range(3)]
        m = max(abs(x) for x in w) or 1.0
        v = [x / m for x in w]
    ln = sum(x * x for x in v) ** 0.5
    return [x / ln for x in v]


def onto_x(a):
    """The shortest rotation taking `a` onto +X, as a 3x3 by rows. Rodrigues
    about the axis perpendicular to both; the antiparallel case cannot arise
    here because the power iteration is seeded pointing along +X."""
    ax = [0.0, a[2], -a[1]]                       # cross(a, +X)
    s = sum(c * c for c in ax) ** 0.5
    if s < 1e-9:
        return [[1, 0, 0], [0, 1, 0], [0, 0, 1]]
    k = [c / s for c in ax]
    c = a[0]                                       # dot(a, +X)
    K = [[0, -k[2], k[1]], [k[2], 0, -k[0]], [-k[1], k[0], 0]]
    R = []
    for i in range(3):
        row = []
        for j in range(3):
            kk = k[i] * k[j]
            row.append((1 if i == j else 0) * c + K[i][j] * s + kk * (1 - c))
        R.append(row)
    return R


def main(src, dst, want=None, scale=1.0, align=False):
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
        if want and node.get("name") not in want:
            continue
        m = node_matrix(node)
        for prim in gltf["meshes"][node["mesh"]]["primitives"]:
            a = prim["attributes"]
            pos = read(gltf, blob, a["POSITION"])
            nor = read(gltf, blob, a["NORMAL"])
            uv = read(gltf, blob, a["TEXCOORD_0"])
            base = len(verts)
            for p, n, t in zip(pos, nor, uv):
                wp = matmul(m, p)
                wn = matmul3(m, n)
                ln = max((wn[0] ** 2 + wn[1] ** 2 + wn[2] ** 2) ** 0.5, 1e-9)
                verts.append(
                    (tuple(c / scale for c in wp), tuple(c / ln for c in wn), t)
                )
            idx.extend(base + i for i in read(gltf, blob, prim["indices"]))
    assert verts, f"no node matched {want}"

    if align:
        a = long_axis([v[0] for v in verts])
        R = onto_x(a)
        turn = lambda v: tuple(sum(R[i][j] * v[j] for j in range(3)) for i in range(3))
        verts = [(turn(p), turn(n), t) for (p, n, t) in verts]
        print(f"  aligned {[round(c, 3) for c in a]} onto +x")

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
    args = sys.argv[1:]
    want, scale, align = set(), 1.0, False
    while len(args) > 2:
        flag = args.pop(2)
        if flag == "--node":
            want.add(args.pop(2))
        elif flag == "--scale":
            scale = float(args.pop(2))
        elif flag == "--align":
            align = True
        else:
            raise SystemExit(f"unknown flag {flag}")
    main(args[0], args[1], want, scale, align)
