#!/usr/bin/env python3
"""A photoscanned rock, down to something a valley can afford, as an OBJ.

    python3 tools/rock_obj.py rock_09_1k.gltf rock-b.obj --cells 13

Poly Haven's rocks are scans: twelve to sixty-six thousand triangles each,
which is one boulder for a quarter of everything this valley draws. Ninety of
them at that price is not a decision anybody has to think about.

So they are clustered. The bounding box is cut into a grid, every vertex is
replaced by the centre of mass of its cell, and a triangle whose three corners
land in the same cell or on the same edge is dropped for having no area left.
On a blob — which is what a rock is — this is the right simplification and it
is four lines of arithmetic: there is nothing to preserve here except the
silhouette, and the silhouette is what a grid keeps. On a mechanical part it
would be the wrong tool entirely, which is why this is `rock_obj.py` and not
`decimate.py`.

Texture coordinates are dropped on the way through. The rocks are drawn
triplanar off the terrain's own rock photograph, so a scan's unwrap would be
carried through the whole pipeline to be ignored at the end of it — and the
valley gets boulders that match the cliffs they are lying against, which is
worth more than a scan's own colour would be.

Normals are left to `bake_obj.py`, which recomputes them by crease angle: the
clustering has moved every vertex, so the scan's normals describe a surface
that no longer exists.
"""
import json
import struct
import sys
from pathlib import Path

COMPONENT = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
COUNT = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}


def accessor(gltf, blob, index):
    acc = gltf["accessors"][index]
    view = gltf["bufferViews"][acc["bufferView"]]
    fmt = COMPONENT[acc["componentType"]]
    n = COUNT[acc["type"]]
    size = struct.calcsize(fmt) * n
    start = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    stride = view.get("byteStride") or size
    out = []
    for i in range(acc["count"]):
        v = struct.unpack_from("<" + fmt * n, blob, start + i * stride)
        out.append(v[0] if n == 1 else v)
    return out


def load(path):
    """Every primitive of the file, welded into one vertex list."""
    p = Path(path)
    gltf = json.loads(p.read_text())
    blob = (p.parent / gltf["buffers"][0]["uri"]).read_bytes()
    vs, tris = [], []
    for mesh in gltf["meshes"]:
        for prim in mesh["primitives"]:
            base = len(vs)
            vs += accessor(gltf, blob, prim["attributes"]["POSITION"])
            idx = accessor(gltf, blob, prim["indices"])
            for k in range(0, len(idx), 3):
                tris.append(tuple(base + idx[k + j] for j in range(3)))
    return vs, tris


def cluster(vs, tris, cells):
    lo = [min(v[i] for v in vs) for i in range(3)]
    hi = [max(v[i] for v in vs) for i in range(3)]
    span = [max(hi[i] - lo[i], 1e-9) for i in range(3)]
    # The grid is cubic in world units rather than in the box, so a flat rock
    # is not cut finely across its thin axis and coarsely along its long one.
    step = max(span) / cells

    def cell_of(v):
        return tuple(min(int((v[i] - lo[i]) / step), int(span[i] / step)) for i in range(3))

    acc = {}
    for v in vs:
        c = cell_of(v)
        a = acc.setdefault(c, [0.0, 0.0, 0.0, 0])
        for i in range(3):
            a[i] += v[i]
        a[3] += 1
    order, point = {}, []
    for c, a in acc.items():
        order[c] = len(point)
        point.append(tuple(a[i] / a[3] for i in range(3)))

    out, seen = [], set()
    for t in tris:
        c = tuple(order[cell_of(vs[i])] for i in t)
        if c[0] == c[1] or c[1] == c[2] or c[0] == c[2]:
            continue
        # The same three cells can be hit by dozens of the scan's triangles.
        key = tuple(sorted(c))
        if key in seen:
            continue
        seen.add(key)
        out.append(c)
    return point, out


def main(src, dst, cells):
    vs, tris = load(src)
    point, out = cluster(vs, tris, cells)
    with open(dst, "w") as f:
        f.write(f"# {Path(src).name}, clustered to {cells} cells across\n")
        f.write(f"# {len(vs)} vertices and {len(tris)} triangles in, "
                f"{len(point)} and {len(out)} out\n")
        for p in point:
            f.write("v %.5f %.5f %.5f\n" % p)
        for t in out:
            f.write("f %d %d %d\n" % (t[0] + 1, t[1] + 1, t[2] + 1))
    print(f"{dst}: {len(vs)} -> {len(point)} vertices, "
          f"{len(tris)} -> {len(out)} triangles")


if __name__ == "__main__":
    a = sys.argv[1:]
    n = int(a[a.index("--cells") + 1]) if "--cells" in a else 12
    main(a[0], a[1], n)
