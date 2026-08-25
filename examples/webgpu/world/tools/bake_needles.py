# The pine's foliage cell, made denser so the tree can be made of fewer cards.
#
# The source is the needle spray that has always been in `pine-twig.png` —
# Poly Haven's `pine_tree_01`, CC0. What this does is the same thing
# `bake_poplar.py` does for the poplar: scatter many copies of it, turned and
# scaled, thinning towards the edges so a card's own rectangle never shows,
# and hand back a cell that carries a branch's worth of needles instead of one
# twig's. A leaf costs pixels; a card costs triangles.
#
#   python3 tools/bake_needles.py old-needle-cell.png
#
# Writes needle-mass.png at 512, which is the left half of the two-cell
# foliage atlas.
import sys, random
from PIL import Image

twig = Image.open(sys.argv[1]).convert("RGBA")
box = twig.getbbox()
if box:
    twig = twig.crop(box)

S = 2048
out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
random.seed(5)

# All of them pointing the same way, which is the whole difference between a
# branch and a bush.
#
# The first attempt scattered them at every angle, the way the poplar's leaves
# are scattered — and a poplar card is a piece of crown, where a pine card is
# one branch reaching out of the trunk. Turned every which way it stopped
# being a branch: what came out was a cloud of loose twigs with no direction
# in it, and once the cards were made big enough to be worth having, you could
# see every twig sitting on its own.
#
# So they all run stem-to-tip up the cell, fanned by a few degrees each, wide
# at the stem end and closing towards the tip — which is what a branch does.
def put(cx, cy, scale_mul, ang):
    scale = 0.62 * scale_mul * S / max(twig.size)
    t = twig.resize((max(8, int(twig.size[0] * scale)), max(8, int(twig.size[1] * scale))),
                    Image.LANCZOS).rotate(ang, expand=True, resample=Image.BICUBIC)
    out.alpha_composite(t, (max(0, min(S - t.size[0], int(cx - t.size[0] / 2))),
                            max(0, min(S - t.size[1], int(cy - t.size[1] / 2)))))

# Down the middle first, then out to the sides: the ones at the edge are
# shorter and lean further out, so the card's outline is a fan rather than a
# rectangle of needles.
lanes = [(0.50, 0.00, 1.00), (0.34, -13.0, 0.88), (0.66, 13.0, 0.88),
         (0.22, -24.0, 0.72), (0.78, 24.0, 0.72),
         (0.13, -34.0, 0.58), (0.87, 34.0, 0.58)]
for (lx, la, lm) in lanes:
    for k in range(3):
        # Three up each lane, overlapping, the lowest one largest.
        t = 0.30 + k * 0.26
        put(lx * S + random.uniform(-0.02, 0.02) * S,
            (1.0 - t) * S + random.uniform(-0.03, 0.03) * S,
            lm * (1.05 - 0.18 * k),
            la + random.uniform(-7, 7))

out.resize((512, 512), Image.LANCZOS).save("needle-mass.png")
print("needle-mass.png written")
