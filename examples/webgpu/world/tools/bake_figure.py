#!/usr/bin/env python3
"""Bakes a skinned character out of a glTF binary into the one file the page loads.

    python3 tools/bake_figure.py lib.glb figure.mesh

The source is Quaternius' Universal Animation Library (CC0), which carries one
mannequin on a fifty-three joint rig and forty-six clips. What comes out is the
mesh, the skeleton and the seven clips this valley has any use for.

Three observations about the source make the format small, and they are
checked rather than assumed:

  * no joint is ever scaled, so scale is not stored at all;
  * exactly one joint translates — the hips — so every other joint keeps its
    bind translation for the whole clip and a frame is a root position and a
    rotation per joint;
  * the clips are authored on a thirty-a-second grid already, so resampling
    onto one loses nothing.

Positions are unorm16 across the model's own bounding box and normals are
snorm8, which is what the rifle beside it does. Rotations are snorm16, which
is about a hundredth of a degree — far finer than a joint an arm long can show.

Layout, little-endian throughout:

    magic   "RILLFIG2"
    u32     joints, vertices, indices, clips
    u32     the joint a weapon hangs off
    f32[16] where a weapon sits on it, in that joint's own space
    f32[6]  bounding box: min xyz then max xyz
    joints  i32 parent, f32[3] bind translation, f32[4] bind rotation,
            f32[16] inverse bind matrix
    vertex  u16[4] position (the fourth is spare), i8[4] normal and material,
            u8[4] joint indices, u8[4] weights          -- twenty bytes
    u16     indices
    clip    u8 name length, name, u32 frames, f32 fps, u8 loops, u8 root joint,
            f32[3] per frame root translation,
            i16[4] per joint per frame rotation
"""

import json
import os
import struct
import sys

COMPONENT = {5120: ("b", 1), 5121: ("B", 1), 5122: ("h", 2), 5123: ("H", 2), 5125: ("I", 4), 5126: ("f", 4)}
COUNT = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}

# The hand a rifle goes in, and how big the rifle is meant to be. The grip is
# not guessed at: the forearm and the hand say which way the arm points in the
# bind pose, and the barrel is laid along that.
HAND = "DEF-hand.R"

# How long the rifle is, in metres, and it is measured against the person
# holding it rather than set by eye. A bolt-action rifle with a scope is about
# 1.1 m, which is a shade over six tenths of a 1.83 m mannequin — and the scale
# that gets it there is worked out from `rifle.mesh`'s own bounding box rather
# than written down, because a number written down beside a mesh is a number
# that goes wrong the first time the mesh is rebaked.
#
# `VM_SCALE` in `common.wgsl` is a different number for a different job: the
# viewmodel is placed in camera space and projected through its own frustum, so
# what looks right there says nothing about how long a rifle is in the valley.
# Taking it for one is what put a toothpick in everybody's hands.
RIFLE_METRES = 1.10
# Where along it the hand goes, as a fraction from the butt. A rifle is held at
# the small of the stock, which is behind the middle; the model's own origin is
# not, so the difference is what this shifts by.
GRIP_AT = 0.40
# The clip whose first frame the rifle is levelled against. See `main`.
GRIP_POSE = "Pistol_Idle_Loop"
# Which joints move with the arms rather than with the legs, so that somebody
# can walk with a rifle up in a library that has a walk and an armed idle and
# no armed walk. Everything descending from the spine — neck, head, both arms,
# every finger — plus the pelvis itself, and the pelvis is the interesting one.
#
# Its rotation is what pitches a running body forward, and the torso rides on
# it: masked from the spine alone, the rifle came out level and pointed twenty
# five degrees into the ground, because the whole upper body had been tipped
# by a joint underneath the mask. Its *children* stay behind — the legs hang
# off the pelvis rather than off the spine — so they still run, on an upright
# pelvis rather than a leaning one. The stride's own bob is a translation and
# is not masked at all, so the body still rises and falls with the footfalls.
#
# Which is why this goes into the file as a joint apiece rather than as one
# index and a subtree: the rule has an exception in it, and the place to know
# about the exception is here, where the rig is being read, and not on a page
# that has never heard of a pelvis.
UPPER = "DEF-spine.001"
PELVIS = "DEF-hips"
# Which way the mannequin faces in its own space. Its nose and its toes agree
# about this, and so does the π − yaw the page places it by.
FACES = (0.0, 0.0, 1.0)
GRIP_UP = 0.02

# What the valley does with a person, and the clip that says it. Everything
# else in the library is somebody else's game.
CLIPS = [
    # The armed idle rather than the empty-handed one, because somebody
    # standing still is who you look at, and they are holding a rifle.
    ("idle", "Pistol_Idle_Loop", True),
    ("walk", "Walk_Loop", True),
    ("run", "Sprint_Loop", True),
    ("crouch", "Crouch_Idle_Loop", True),
    ("fire", "Pistol_Shoot", False),
    ("hit", "Hit_Chest", False),
    ("die", "Death01", False),
]
FPS = 30.0


def load_glb(path):
    d = open(path, "rb").read()
    _, _, total = struct.unpack_from("<III", d, 0)
    off, js, bin_ = 12, None, b""
    while off < total:
        ln, ty = struct.unpack_from("<II", d, off)
        off += 8
        chunk = d[off:off + ln]
        off += ln
        if ty == 0x4E4F534A:
            js = json.loads(chunk)
        elif ty == 0x004E4942:
            bin_ = chunk
    return js, bin_


def accessor(js, bin_, index):
    acc = js["accessors"][index]
    fmt, size = COMPONENT[acc["componentType"]]
    n = COUNT[acc["type"]]
    view = js["bufferViews"][acc["bufferView"]]
    base = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    stride = view.get("byteStride") or n * size
    out = []
    for i in range(acc["count"]):
        vals = struct.unpack_from("<" + fmt * n, bin_, base + i * stride)
        out.append(vals[0] if n == 1 else vals)
    return out


def sample_at(times, values, t):
    """Linear between keys; quaternions take the short way round."""
    if t <= times[0]:
        return values[0]
    if t >= times[-1]:
        return values[-1]
    lo, hi = 0, len(times) - 1
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if times[mid] <= t:
            lo = mid
        else:
            hi = mid
    span = times[hi] - times[lo]
    f = 0.0 if span <= 0 else (t - times[lo]) / span
    a, b = values[lo], values[hi]
    if len(a) == 4:
        if sum(x * y for x, y in zip(a, b)) < 0:
            b = tuple(-y for y in b)
        out = tuple(x + (y - x) * f for x, y in zip(a, b))
        n = sum(v * v for v in out) ** 0.5 or 1.0
        return tuple(v / n for v in out)
    return tuple(x + (y - x) * f for x, y in zip(a, b))


def mat_mul(a, b):
    o = [0.0] * 16
    for c in range(4):
        for r in range(4):
            o[c * 4 + r] = (a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1]
                            + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3])
    return o


def mat_trs(t, q):
    x, y, z, w = q
    x2, y2, z2 = x + x, y + y, z + z
    xx, xy, xz = x * x2, x * y2, x * z2
    yy, yz, zz = y * y2, y * z2, z * z2
    wx, wy, wz = w * x2, w * y2, w * z2
    return [1 - (yy + zz), xy + wz, xz - wy, 0.0,
            xy - wz, 1 - (xx + zz), yz + wx, 0.0,
            xz + wy, yz - wx, 1 - (xx + yy), 0.0,
            t[0], t[1], t[2], 1.0]


def rigid_inverse(m):
    """The rig has no scale in it, so a transpose and a dot are the whole of it."""
    r = [m[0], m[4], m[8], 0.0, m[1], m[5], m[9], 0.0, m[2], m[6], m[10], 0.0]
    tx, ty, tz = m[12], m[13], m[14]
    return r + [-(m[0] * tx + m[1] * ty + m[2] * tz),
                -(m[4] * tx + m[5] * ty + m[6] * tz),
                -(m[8] * tx + m[9] * ty + m[10] * tz), 1.0]


def pose_world(js, bin_, at, parent, bind_t, bind_r, clip_name, t):
    """The skeleton at one moment of one clip, in world space.

    Rotations only: the root's translation moves the whole figure and the grip
    lives in the hand's own space, where it cancels.
    """
    anim = next(a for a in js["animations"] if a["name"] == clip_name)
    rot = {}
    for ch in anim["channels"]:
        node, path = ch["target"]["node"], ch["target"]["path"]
        if node in at and path == "rotation":
            s = anim["samplers"][ch["sampler"]]
            rot[at[node]] = (accessor(js, bin_, s["input"]), accessor(js, bin_, s["output"]))
    world = []
    for j in range(len(parent)):
        q = sample_at(*rot[j], t) if j in rot else bind_r[j]
        local = mat_trs(bind_t[j], q)
        world.append(local if parent[j] < 0 else mat_mul(world[parent[j]], local))
    return world


def rifle_box(path):
    """The rifle's own origin and size, out of the header of `rifle.mesh`.

    Six floats after the two counts: the minimum corner, then the size — which
    is what the viewmodel's decode says they are, `min + q * size`.
    """
    with open(path, "rb") as f:
        head = f.read(32)
    return struct.unpack_from("<6f", head, 8)


def grip_matrix(world, hand, box):
    """Where a rifle sits in the hand's own space.

    The model is built with +x down the barrel, +y up and +z out to the right,
    which `common.wgsl` says and the viewmodel relies on. So in the pose this
    is worked out against: point +x where the figure faces, take up for +y as
    far as it is perpendicular, and let +z fall out of the two. Then take the
    whole frame back into the hand's space, which is where it has to live to
    travel with it.

    The barrel goes where the *figure* points and not where the forearm does.
    Those differ by ten degrees in the armed idle — an arm holding something is
    not a rod aimed at what it is holding — and ten degrees is the difference
    between a rifle that is aimed at what its owner is aimed at and one that is
    not. Everywhere but this pose it follows the wrist, which is right: that is
    what a wrist does.
    """
    h = world[hand]
    ax = list(FACES)
    up = [0.0, 1.0, 0.0]
    az = [ax[1] * up[2] - ax[2] * up[1], ax[2] * up[0] - ax[0] * up[2], ax[0] * up[1] - ax[1] * up[0]]
    n = sum(v * v for v in az) ** 0.5 or 1.0
    az = [v / n for v in az]
    ay = [az[1] * ax[2] - az[2] * ax[1], az[2] * ax[0] - az[0] * ax[2], az[0] * ax[1] - az[1] * ax[0]]
    s = RIFLE_METRES / box[3]
    # The origin sits wherever the modeller left it along the barrel; the hand
    # wants to be at the small of the stock. The gap between the two is what
    # the rifle is slid by, so that changing where a hand grips is one number
    # and not a fiddle.
    origin_at = -box[0] / box[3]
    at = [h[12 + c] + ax[c] * (origin_at - GRIP_AT) * RIFLE_METRES + ay[c] * GRIP_UP for c in range(3)]
    want = [ax[0] * s, ax[1] * s, ax[2] * s, 0.0,
            ay[0] * s, ay[1] * s, ay[2] * s, 0.0,
            az[0] * s, az[1] * s, az[2] * s, 0.0,
            at[0], at[1], at[2], 1.0]
    return mat_mul(rigid_inverse(h), want)


def main(src, dst):
    js, bin_ = load_glb(src)
    skin = js["skins"][0]
    joints = skin["joints"]
    at = {node: i for i, node in enumerate(joints)}
    inv = accessor(js, bin_, skin["inverseBindMatrices"])

    parent = [-1] * len(joints)
    for n, node in enumerate(js["nodes"]):
        for child in node.get("children", []):
            if child in at and n in at:
                parent[at[child]] = at[n]

    bind_t, bind_r = [], []
    for node in joints:
        n = js["nodes"][node]
        bind_t.append(tuple(n.get("translation", (0.0, 0.0, 0.0))))
        bind_r.append(tuple(n.get("rotation", (0.0, 0.0, 0.0, 1.0))))

    # ---- the mesh, both primitives merged into one buffer -------------------
    verts, tris = [], []
    for prim in js["meshes"][0]["primitives"]:
        a = prim["attributes"]
        pos = accessor(js, bin_, a["POSITION"])
        nrm = accessor(js, bin_, a["NORMAL"])
        jnt = accessor(js, bin_, a["JOINTS_0"])
        wgt = accessor(js, bin_, a["WEIGHTS_0"])
        base = len(verts)
        mat = prim.get("material", 0)
        for i in range(len(pos)):
            verts.append((pos[i], nrm[i], jnt[i], wgt[i], mat))
        tris += [base + i for i in accessor(js, bin_, prim["indices"])]
    assert len(verts) < 65536, "the indices are sixteen bit"

    lo = [min(v[0][c] for v in verts) for c in range(3)]
    hi = [max(v[0][c] for v in verts) for c in range(3)]
    span = [max(hi[c] - lo[c], 1e-6) for c in range(3)]

    names = [js["nodes"][n].get("name", "") for n in joints]
    hand = names.index(HAND)
    # The rifle is next to the figure it goes in, and is read for its size
    # rather than believed about it.
    box = rifle_box(os.path.join(os.path.dirname(os.path.abspath(dst)), "rifle.mesh"))
    # The pose the rifle is levelled in — not the bind pose, which is the one
    # place in the whole library nobody is holding anything. A bind pose has
    # the palms turned down; every armed pose in it turns the wrist ninety
    # degrees to grip, and a rifle levelled against the bind pose therefore
    # spends the entire game lying on its side. Levelled against the pose
    # somebody actually stands in, it is level when it is being aimed and rolls
    # with the wrist everywhere else, which is what a wrist is for.
    armed = pose_world(js, bin_, at, parent, bind_t, bind_r, GRIP_POSE, 0.0)
    grip = grip_matrix(armed, hand, box)

    out = bytearray()
    # The mask, one byte a joint: the spine's subtree, and the pelvis on its own.
    spine = names.index(UPPER)
    upper = [0] * len(joints)
    for j in range(len(joints)):
        upper[j] = 1 if j == spine or (parent[j] >= 0 and upper[parent[j]]) else 0
    upper[names.index(PELVIS)] = 1

    out += b"RILLFIG4"
    out += struct.pack("<IIII", len(joints), len(verts), len(tris), len(CLIPS))
    out += struct.pack("<I", hand)
    out += struct.pack("<16f", *grip)
    out += struct.pack("<6f", *lo, *hi)
    out += bytes(upper) + b"\0" * (-len(upper) % 4)
    for i in range(len(joints)):
        out += struct.pack("<i", parent[i])
        out += struct.pack("<3f", *bind_t[i])
        out += struct.pack("<4f", *bind_r[i])
        out += struct.pack("<16f", *inv[i])

    for pos, nrm, jnt, wgt, mat in verts:
        q = [int(round((pos[c] - lo[c]) / span[c] * 65535)) for c in range(3)]
        out += struct.pack("<4H", q[0], q[1], q[2], 0)
        nq = [max(-127, min(127, int(round(nrm[c] * 127)))) for c in range(3)]
        out += struct.pack("<4b", nq[0], nq[1], nq[2], mat)
        out += struct.pack("<4B", *[min(255, j) for j in jnt])
        total = sum(wgt) or 1.0
        w = [int(round(x / total * 255)) for x in wgt]
        w[0] += 255 - sum(w)                     # they have to add up to one
        out += struct.pack("<4B", *[max(0, min(255, x)) for x in w])
    out += struct.pack(f"<{len(tris)}H", *tris)

    for name, clip_name, loops in CLIPS:
        anim = next(a for a in js["animations"] if a["name"] == clip_name)
        rot, trans = {}, {}
        duration = 0.0
        for ch in anim["channels"]:
            node, path = ch["target"]["node"], ch["target"]["path"]
            if node not in at:
                continue
            s = anim["samplers"][ch["sampler"]]
            times = accessor(js, bin_, s["input"])
            values = accessor(js, bin_, s["output"])
            duration = max(duration, times[-1])
            if path == "rotation":
                rot[at[node]] = (times, values)
            elif path == "translation":
                moves = max(max(abs(v[c] - values[0][c]) for v in values) for c in range(3))
                if moves > 1e-4:
                    trans[at[node]] = (times, values)
            elif path == "scale":
                worst = max(max(abs(v[c] - 1.0) for v in values) for c in range(3))
                assert worst < 1e-3, f"{clip_name} scales a joint, which this format does not carry"
        assert len(trans) <= 1, f"{clip_name} translates {len(trans)} joints, not one"
        root = next(iter(trans), 0)
        frames = max(2, int(round(duration * FPS)) + 1)

        out += struct.pack("<B", len(name)) + name.encode()
        out += struct.pack("<IfBB", frames, FPS, 1 if loops else 0, root)
        for f in range(frames):
            t = f / FPS
            if root in trans:
                out += struct.pack("<3f", *sample_at(*trans[root], t))
            else:
                out += struct.pack("<3f", *bind_t[root])
        for f in range(frames):
            t = f / FPS
            for j in range(len(joints)):
                q = sample_at(*rot[j], t) if j in rot else bind_r[j]
                out += struct.pack("<4h", *[max(-32767, min(32767, int(round(v * 32767)))) for v in q])

    open(dst, "wb").write(out)
    print(f"{dst}: {len(out) / 1024:.0f} KB — {len(joints)} joints, {len(verts)} vertices, "
          f"{len(tris) // 3} triangles, {len(CLIPS)} clips, weapon on {names[hand]}, "
          f"{sum(upper)} joints on the upper body")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
