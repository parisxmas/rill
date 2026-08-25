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

# A leaf is five to ten centimetres and the card it lives on is most of a
# metre, so a leaf has to be about a tenth of the card — not a third, which is
# what the first spray had and which drew a tree hung with dinner plates. The
# density then has to come from the count, and the count is what a photograph
# of a branch actually has on it.
def put(i, t, side, scale_mul=1.0):
    leaf = leaves[i % len(leaves)]
    scale = (0.15 - 0.05 * t) * scale_mul * S / max(leaf.size)
    l = leaf.resize((max(8, int(leaf.size[0] * scale)), max(8, int(leaf.size[1] * scale))),
                    Image.LANCZOS)
    # Hanging, turned, and never twice the same: two leaves at one angle read
    # as a pattern, and a pattern is what gives a texture away.
    ang = side * (48 + random.uniform(-40, 40)) + 180 + random.uniform(-25, 25)
    l = l.rotate(ang, expand=True, resample=Image.BICUBIC)
    cx = S * 0.5 + side * (0.04 + 0.26 * (1 - t)) * S + random.uniform(-0.10 * S, 0.10 * S)
    cy = S * (0.05 + 0.88 * t) + random.uniform(-0.05 * S, 0.05 * S)
    spray.alpha_composite(l, (max(0, min(S - l.size[0], int(cx - l.size[0] / 2))),
                              max(0, min(S - l.size[1], int(cy - l.size[1] / 2)))))

# Something over a hundred leaves to a card. That is not extravagance: at a
# tenth of the card each, fewer than this is a branch you can see daylight
# through from every angle, and a poplar in leaf is not a see-through thing.
n = 42
for i in range(n):
    t = i / (n - 1)
    put(i, t, -1)
    put(i + 3, t, 1)
    put(i + 5, t, -1, 0.86)
    put(i + 1, t, 1, 0.86)
# And a run down the middle, so the card has no hole in it.
for i in range(30):
    put(i + 1, 0.03 + i * 0.032, 0, 0.88)

spray.resize((256, 256), Image.LANCZOS).save("poplar-spray.png")
print("poplar-spray.png written")
