#!/usr/bin/env python3
"""The three weapon icons, out of game-icons.net and into one strip.

Rifle and AK by Skoll, the launcher by Delapouite, all from game-icons.net
under CC BY 3.0 — the credit is in host.js where the strip is bound and in
weapon-icons.license.txt beside the image.

Each file carries a black square as a backdrop, which is not part of the
drawing: taken out, what is left is a white silhouette on nothing, and white
on nothing is what the HUD wants — it tints its own colours and a baked-in
black would be a black rectangle behind every icon.

    python3 tools/bake_icons.py        # writes weapon-icons.png, 576x192
"""
import re
import subprocess
import tempfile
from pathlib import Path
from PIL import Image

BASE = "https://raw.githubusercontent.com/game-icons/icons/master"
ICONS = [
    ("skoll/winchester-rifle", "rifle"),
    ("skoll/ak47", "ak"),
    ("delapouite/missile-launcher", "launcher"),
]
CELL = 192

here = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    cells = []
    for path, name in ICONS:
        svg = tmp / f"{name}.svg"
        subprocess.run(["curl", "-sS", "-o", str(svg), f"{BASE}/{path}.svg"], check=True)
        text = svg.read_text()
        text = re.sub(r'<path d="M0 0h512v512H0z"\s*/>', "", text, count=1)
        svg.write_text(text)
        png = tmp / f"{name}.png"
        subprocess.run(["rsvg-convert", "-w", str(CELL), "-h", str(CELL),
                        str(svg), "-o", str(png)], check=True)
        cells.append(Image.open(png).convert("RGBA"))

    sheet = Image.new("RGBA", (CELL * len(cells), CELL), (255, 255, 255, 0))
    for i, im in enumerate(cells):
        sheet.paste(im, (i * CELL, 0), im)
    sheet.save(here / "weapon-icons.png")
    print(f"weapon-icons.png written: {sheet.width}x{sheet.height}")
