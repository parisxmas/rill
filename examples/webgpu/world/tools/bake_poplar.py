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
S = 2048
spray = Image.new("RGBA", (S, S), (0, 0, 0, 0))
random.seed(11)

# A leaf is five to ten centimetres and the card it lives on is most of a
# metre, so a leaf has to be about a tenth of the card — not a third, which is
# what the first spray had and which drew a tree hung with dinner plates. The
# density then has to come from the count, and the count is what a photograph
# of a branch actually has on it.
# A card is a piece of crown now, not a twig: a few hundred leaves scattered
# over it, thinning towards the edges so the card's own rectangle never shows.
# The thinning is the whole trick — a leaf that touches the border is a
# straight cut, and a straight cut is what gives cards away.
def put(i, cx, cy, scale_mul=1.0):
    leaf = leaves[i % len(leaves)]
    scale = 0.062 * scale_mul * S / max(leaf.size)
    l = leaf.resize((max(6, int(leaf.size[0] * scale)), max(6, int(leaf.size[1] * scale))),
                    Image.LANCZOS)
    # Turned every which way: two leaves at one angle read as a pattern, and a
    # pattern is what gives a texture away.
    l = l.rotate(random.uniform(0, 360), expand=True, resample=Image.BICUBIC)
    spray.alpha_composite(l, (max(0, min(S - l.size[0], int(cx - l.size[0] / 2))),
                              max(0, min(S - l.size[1], int(cy - l.size[1] / 2)))))

# Rejection sampling against a soft mask: densest down the middle and through
# the middle of the height, nothing at the corners.
kept = 0
tries = 0
while kept < 620 and tries < 40000:
    tries += 1
    x = random.random()
    y = random.random()
    edge = min(x, 1 - x, y, 1 - y) * 2.0            # 0 at a border, 1 at the middle
    across = 1.0 - abs(x - 0.5) * 1.5               # a little denser down the spine
    if random.random() > min(1.0, edge * 1.6) * max(0.35, across):
        continue
    put(kept, x * S, y * S, 0.8 + random.random() * 0.5)
    kept += 1
print("leaves:", kept)

spray.resize((512, 512), Image.LANCZOS).save("poplar-spray.png")
print("poplar-spray.png written")
