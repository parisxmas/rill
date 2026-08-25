# The poplar's leaves, cut out of a photograph and arranged into a branch.
#
# The source is ambientCG's LeafSet004 (CC0): six heart-shaped leaves on a
# sheet with a matching opacity map. Poplar leaves are broadly heart-shaped
# with a long petiole, which is what that set happens to be — near enough
# that nobody who has stood under one would argue.
#
# What comes out is the right-hand cell of `pine-twig.png`, the two-cell
# foliage atlas the trees share: needles on the left, this on the right. A
# spray rather than one leaf, because a crown built from single leaves is a
# crown you can count the leaves of.
#
#   python3 tools/bake_poplar.py LeafSet004_1K-PNG_Color.png \
#                                LeafSet004_1K-PNG_Opacity.png
#
# It writes poplar-spray.png beside itself; `pine-twig.png` is then the two
# cells pasted side by side at 256 each.
import sys, math, random
from PIL import Image

colour, opacity = sys.argv[1], sys.argv[2]
c = Image.open(colour).convert("RGB")
a = Image.open(opacity).convert("L")
sheet = Image.new("RGBA", c.size)
sheet.paste(c)
sheet.putalpha(a)

# Six leaves in two rows of three, each trimmed to its own ink.
W, H = c.size
leaves = []
for r in range(2):
    for col in range(3):
        cell = sheet.crop((col * W // 3, r * H // 2, (col + 1) * W // 3, (r + 1) * H // 2))
        box = cell.getbbox()
        if box:
            leaves.append(cell.crop(box))

# Drawn at four times the size and reduced at the end, which is the cheapest
# antialiasing there is.
S = 1024
spray = Image.new("RGBA", (S, S), (0, 0, 0, 0))
random.seed(11)

def put(i, t, side, scale_mul=1.0):
    leaf = leaves[i % len(leaves)]
    scale = (0.34 - 0.13 * t) * scale_mul * S / max(leaf.size)
    l = leaf.resize((max(8, int(leaf.size[0] * scale)), max(8, int(leaf.size[1] * scale))),
                    Image.LANCZOS)
    # Hanging, turned, and never twice the same: two leaves at one angle read
    # as a pattern, and a pattern is what gives a texture away.
    ang = side * (48 + random.uniform(-40, 40)) + 180 + random.uniform(-25, 25)
    l = l.rotate(ang, expand=True, resample=Image.BICUBIC)
    cx = S * 0.5 + side * (0.05 + 0.17 * (1 - t)) * S + random.uniform(-0.07 * S, 0.07 * S)
    cy = S * (0.08 + 0.80 * t) + random.uniform(-0.04 * S, 0.04 * S)
    spray.alpha_composite(l, (max(0, min(S - l.size[0], int(cx - l.size[0] / 2))),
                              max(0, min(S - l.size[1], int(cy - l.size[1] / 2)))))

n = 11
for i in range(n):
    t = i / (n - 1)
    put(i, t, -1)
    put(i + 3, t, 1)
# And a few down the middle, smaller, so the card has no hole in it.
for i in range(7):
    put(i + 1, 0.12 + i * 0.13, 0, 0.82)

spray.resize((256, 256), Image.LANCZOS).save("poplar-spray.png")
print("poplar-spray.png written")
