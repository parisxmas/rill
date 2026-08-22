#!/usr/bin/env python3
"""An OBJ with no normals and no texture coordinates, into the mesh format.

    python3 tools/bake_obj.py hp_ak.obj ak47.mesh --length 0.95 --bore 0.0497

`bake_prop.py` is for glTF that arrives complete. This is for the other kind:
a model published as raw geometry, quads and all, with the shading left to
whoever uses it. Three things have to be worked out here that the other baker
is handed.

**Triangles.** Quads are split on the shorter diagonal, which is what keeps a
long thin face — the flat of a barrel — from folding.

**Normals.** Averaging every face at a vertex makes a gun look inflated; one
normal per face makes every triangle its own three vertices, and thirty
thousand triangles will not index in the u16 this format uses. So faces are
grouped at each vertex by the angle between them: under forty degrees they
share a normal and the barrel stays round, over it they split and the
receiver keeps its edges.

**Where it sits.** `--length` scales it, and `--bore` puts the middle of the
muzzle's cross-section at a given height, so a weapon drops into another's
viewmodel transform pointing where that one pointed.
"""
import math
import struct
import sys


def parse(path):
    vs, faces = [], []
    for line in open(path):
        p = line.split()
        if not p:
            continue
        if p[0] == "v":
            vs.append((float(p[1]), float(p[2]), float(p[3])))
        elif p[0] == "f":
            idx = [int(c.split("/")[0]) - 1 for c in p[1:]]
            if len(idx) == 3:
                faces.append(tuple(idx))
            elif len(idx) == 4:
                a, b, c, d = idx
                # the shorter diagonal, so a long face does not fold
                ac = sum((vs[a][i] - vs[c][i]) ** 2 for i in range(3))
                bd = sum((vs[b][i] - vs[d][i]) ** 2 for i in range(3))
                if ac <= bd:
                    faces += [(a, b, c), (a, c, d)]
                else:
                    faces += [(a, b, d), (b, c, d)]
    return vs, faces


def normal(vs, f):
    a, b, c = (vs[i] for i in f)
    u = tuple(b[i] - a[i] for i in range(3))
    v = tuple(c[i] - a[i] for i in range(3))
    n = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0])
    l = math.sqrt(sum(c * c for c in n)) or 1e-9
    return tuple(c / l for c in n)


def main(src, dst, length, bore, crease=40.0):
    vs, faces = parse(src)
    # the barrel onto +x: a turn about y, so it stays a rotation
    vs = [(-v[2], v[1], v[0]) for v in vs]

    xs = [v[0] for v in vs]
    zs = [v[2] for v in vs]
    scale = (max(xs) - min(xs)) / length
    cx = (max(xs) + min(xs)) / 2.0
    # and the cross-axis, because a model authored away from its own origin
    # comes out held half a metre to one side of where the hand is.
    cz = (max(zs) + min(zs)) / 2.0
    vs = [((v[0] - cx) / scale, v[1] / scale, (v[2] - cz) / scale) for v in vs]

    # the muzzle's own cross-section decides the height, so the barrel lands
    # where the weapon this is standing in for kept its barrel
    xmax = max(v[0] for v in vs)
    span = xmax - min(v[0] for v in vs)
    nose = [v for v in vs if v[0] > xmax - 0.06 * span]
    ny = [v[1] for v in nose]
    dy = bore - (min(ny) + max(ny)) / 2.0
    vs = [(v[0], v[1] + dy, v[2]) for v in vs]

    fnorm = [normal(vs, f) for f in faces]
    at_vertex = {}
    for fi, f in enumerate(faces):
        for vi in f:
            at_vertex.setdefault(vi, []).append(fi)

    cos_crease = math.cos(math.radians(crease))
    out_v, out_i, seen = [], [], {}
    for fi, f in enumerate(faces):
        for vi in f:
            # every face at this vertex within the crease angle of this one
            acc = [0.0, 0.0, 0.0]
            group = []
            for fj in at_vertex[vi]:
                if sum(fnorm[fi][k] * fnorm[fj][k] for k in range(3)) >= cos_crease:
                    group.append(fj)
                    for k in range(3):
                        acc[k] += fnorm[fj][k]
            l = math.sqrt(sum(c * c for c in acc)) or 1e-9
            n = tuple(c / l for c in acc)
            key = (vi, tuple(sorted(group)))
            if key not in seen:
                seen[key] = len(out_v)
                out_v.append((vs[vi], n))
            out_i.append(seen[key])

    assert len(out_v) < 65536, f"{len(out_v)} vertices will not index in u16"

    lo = [min(v[0][i] for v in out_v) for i in range(3)]
    hi = [max(v[0][i] for v in out_v) for i in range(3)]
    sp = [max(hi[i] - lo[i], 1e-9) for i in range(3)]
    blob = bytearray()
    blob += struct.pack("<II6f", len(out_v), len(out_i), *lo, *sp)
    for p, n in out_v:
        q = [min(65535, max(0, round((p[i] - lo[i]) / sp[i] * 65535))) for i in range(3)]
        s = [min(127, max(-127, round(n[i] * 127))) for i in range(3)]
        # no texture coordinates in the source and none wanted: this one is
        # shaded from where it is rather than from a photograph
        blob += struct.pack("<4H4b2H", q[0], q[1], q[2], 0, s[0], s[1], s[2], 0, 0, 0)
    for i in out_i:
        blob += struct.pack("<H", i)
    open(dst, "wb").write(bytes(blob))
    print(f"{dst}: {len(out_v)} vertices, {len(out_i)//3} triangles, {len(blob)} bytes")
    print(f"  box {[round(c,3) for c in lo]} + {[round(c,3) for c in sp]}")


if __name__ == "__main__":
    a = sys.argv[1:]
    length = float(a[a.index("--length") + 1]) if "--length" in a else 1.0
    bore = float(a[a.index("--bore") + 1]) if "--bore" in a else 0.0
    main(a[0], a[1], length, bore)
