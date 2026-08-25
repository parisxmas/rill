# The meadow's flowers: four cells of the plant atlas, each a small clump of
# blossoms standing in its own blades.
#
# Both sources are CC0 from ambientCG — `FlowerSet001`, six chamomile heads,
# and `Foliage001`, nine grass blades. What is built here is not a photograph
# of a flower but a *plant*: blades first, blossoms on top of them, standing
# on the cell's floor the way everything in this atlas does. A blossom on its
# own reads as a sticker lying on the grass; a blossom with leaves under it
# reads as something growing.
#
# Four cells because a meadow is not one flower. The white is the source as it
# came; the others are the same heads with the petals turned — the yellow
# centre is left alone, which is what keeps them looking like flowers rather
# than like the same flower under coloured light.
#
#   python3 tools/bake_flowers.py FlowerSet001_1K-PNG_Color.png \
#                                 FlowerSet001_1K-PNG_Opacity.png \
#                                 Foliage001_1K-PNG_Color.png \
#                                 Foliage001_1K-PNG_Opacity.png
#
# Writes flowers-cells.png: four 256 cells in a row.
import sys, random, math
import numpy as np
from PIL import Image

def sheet(colour, opacity):
    c = Image.open(colour).convert("RGB")
    a = Image.open(opacity).convert("L")
    im = Image.new("RGBA", c.size)
    im.paste(c)
    im.putalpha(a)
    return im

def cut(im, cols, rows):
    W, H = im.size
    out = []
    for r in range(rows):
        for c in range(cols):
            cell = im.crop((c * W // cols, r * H // rows, (c + 1) * W // cols, (r + 1) * H // rows))
            box = cell.getbbox()
            if box:
                out.append(cell.crop(box))
    return out

blossoms = cut(sheet(sys.argv[1], sys.argv[2]), 3, 2)
blades = cut(sheet(sys.argv[3], sys.argv[4]), 9, 1)

# Turning the petals without touching the middle.
#
# A chamomile is a white ring round a yellow disc. Saturation tells the two
# apart — the petals have almost none and the disc has a great deal — so the
# tint is applied in proportion to how unsaturated a pixel is, and the disc
# comes through it unchanged.
def tinted(im, rgb):
    a = np.asarray(im).astype(np.float32)
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    mx = np.maximum(np.maximum(r, g), b)
    mn = np.minimum(np.minimum(r, g), b)
    sat = np.where(mx > 1, (mx - mn) / np.maximum(mx, 1), 0.0)
    k = np.clip(1.0 - sat * 2.2, 0.0, 1.0)[..., None]      # 1 on the petals, 0 on the disc
    want = np.array(rgb, dtype=np.float32)[None, None, :]
    a[..., :3] = a[..., :3] * (1 - k) + (a[..., :3] * want / 255.0) * k
    return Image.fromarray(np.clip(a, 0, 255).astype(np.uint8), "RGBA")

CELL = 512          # drawn large, reduced at the end
TINTS = [(255, 255, 255),      # white, as it came
         (255, 214, 92),       # a buttercup yellow
         (236, 132, 190),      # pink
         (150, 158, 246)]      # a pale blue

out = Image.new("RGBA", (CELL * 4, CELL), (0, 0, 0, 0))
random.seed(3)

def place(dst, src, cx, cy, height_frac, ang):
    scale = height_frac * CELL / src.size[1]
    s = src.resize((max(4, int(src.size[0] * scale)), max(4, int(src.size[1] * scale))), Image.LANCZOS)
    if ang:
        s = s.rotate(ang, expand=True, resample=Image.BICUBIC)
    dst.alpha_composite(s, (int(cx - s.size[0] / 2), int(cy - s.size[1])))

for ci, tint in enumerate(TINTS):
    cell = Image.new("RGBA", (CELL, CELL), (0, 0, 0, 0))
    # The blades, standing on the floor of the cell and leaning a little.
    for i in range(7):
        b = blades[(ci * 3 + i) % len(blades)]
        place(cell, b, CELL * (0.18 + 0.64 * random.random()), CELL * (1.0 - 0.02 * random.random()),
              0.42 + 0.34 * random.random(), random.uniform(-14, 14))
    # And the blossoms, on stems of their own: a short blade with a head at
    # the top of it, so nothing is floating.
    for i in range(4):
        stem = blades[(ci + i * 2) % len(blades)]
        h = 0.46 + 0.30 * random.random()
        x = CELL * (0.22 + 0.56 * random.random())
        place(cell, stem, x, CELL, h, random.uniform(-8, 8))
        head = tinted(blossoms[(ci * 2 + i) % len(blossoms)], tint)
        hs = 0.15 + 0.06 * random.random()
        place(cell, head, x + random.uniform(-0.03, 0.03) * CELL,
              CELL * (1.0 - h) + head.size[1] * hs * CELL / head.size[1] * 0.5,
              hs, random.uniform(-20, 20))
    out.alpha_composite(cell, (ci * CELL, 0))

out.resize((1024, 256), Image.LANCZOS).save("flowers-cells.png")
print("flowers-cells.png written: four 256 cells")
