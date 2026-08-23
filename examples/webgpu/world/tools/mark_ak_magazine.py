#!/usr/bin/env python3
"""Which of the AK's vertices are its magazine, written into the mesh.

    python3 tools/mark_ak_magazine.py ak47.mesh

The rifle carries a flag like this from its own baker: the spare fourth
component of the quantized position says whether a vertex belongs to the bolt,
to the round, or to the rifle proper, and `viewmodel.wgsl` moves the bolt by
reading it. The AK needs the same thing for its magazine and cannot get it the
same way — `bake_obj.py` builds it from a raw OBJ that is one unnamed lump of
geometry, and the OBJ is not in the tree any more. So the flag is worked out
from the baked mesh instead, and written back into it.

**Why not a region test in the shader.** Because it would tear the model. The
magazine's two side plates are single connected pieces that run from the floor
of the magazine up *inside* the receiver, so any cut through the middle of the
gun leaves half a plate hanging in the magwell while the other half falls away.
A flag decided per connected piece cannot do that: a piece is either the
magazine or it is not.

**How a piece is decided.** The baker splits vertices wherever two faces meet
at more than forty degrees, which shatters the model into a thousand-odd
pieces — so a piece here is small and local, and a box test on the *whole* of
one is safe where the same test per vertex is not. A piece is the magazine when
all of it lies in the slab the magazine occupies and some of it hangs below the
trigger guard. That is 102 pieces and 4,359 vertices, and drawing them on their
own gives the banana every AK has.

Idempotent: run it twice and the second run finds the flag already set and
writes the same file.
"""
import struct
import sys
from collections import deque

# The magazine's own slab, in the model's units. `x` runs down the barrel, `y`
# is up and `z` across; the numbers are read off the mesh, not guessed.
X_LO, X_HI = -0.070, 0.085     # from the front of the well to the mag's belly
Z_LO, Z_HI = -0.027, 0.003     # the magazine is narrower than the receiver
BELOW = -0.045                 # and it is the only thing that hangs this low


def read(path):
    d = open(path, "rb").read()
    nv, ni = struct.unpack("<II", d[:8])
    lo = struct.unpack("<3f", d[8:20])
    sp = struct.unpack("<3f", d[20:32])
    pts, off = [], 32
    for _ in range(nv):
        q = struct.unpack("<4H", d[off:off + 8])
        off += 16
        pts.append(tuple(lo[k] + q[k] / 65535.0 * sp[k] for k in range(3)))
    idx = struct.unpack("<%dH" % ni, d[32 + nv * 16: 32 + nv * 16 + ni * 2])
    return d, nv, ni, pts, idx


def pieces(nv, ni, idx):
    """Every connected run of triangles, as a list of vertex indices."""
    adj = [[] for _ in range(nv)]
    for t in range(0, ni, 3):
        a, b, c = idx[t], idx[t + 1], idx[t + 2]
        adj[a] += [b, c]
        adj[b] += [a, c]
        adj[c] += [a, b]
    seen = [False] * nv
    out = []
    for s in range(nv):
        if seen[s] or not adj[s]:
            continue
        q, mem = deque([s]), []
        seen[s] = True
        while q:
            v = q.popleft()
            mem.append(v)
            for w in adj[v]:
                if not seen[w]:
                    seen[w] = True
                    q.append(w)
        out.append(mem)
    return out


def main(path):
    d, nv, ni, pts, idx = read(path)
    flagged, kept = set(), 0
    for m in pieces(nv, ni, idx):
        xs = [pts[v][0] for v in m]
        ys = [pts[v][1] for v in m]
        zs = [pts[v][2] for v in m]
        if (min(xs) > X_LO and max(xs) < X_HI
                and min(zs) > Z_LO and max(zs) < Z_HI
                and min(ys) < BELOW):
            flagged.update(m)
            kept += 1
    out = bytearray(d)
    for v in range(nv):
        at = 32 + v * 16 + 6            # the fourth of the four unorm16 positions
        struct.pack_into("<H", out, at, 65535 if v in flagged else 0)
    open(path, "wb").write(bytes(out))
    ys = [pts[v][1] for v in flagged]
    xs = [pts[v][0] for v in flagged]
    print("%s: %d of %d vertices flagged as the magazine, over %d pieces"
          % (path, len(flagged), nv, kept))
    print("  they span x %+.3f..%+.3f  y %+.3f..%+.3f"
          % (min(xs), max(xs), min(ys), max(ys)))


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "ak47.mesh")
