#!/usr/bin/env python3
"""A card's native reference: the app's window pictures cropped to the card.

usage: crop.py <native dir> <out dir> <card> [names]
The crops (points of the 1176×768 window) are the cards' (public/hero/cards.js).
"""
import sys
from pathlib import Path
from PIL import Image

CROPS = {"cat": (436, 150, 560, 388), "machines": (356, 360, 560, 388), "visuals": (308, 110, 560, 388)}
src, out, card = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
names = sys.argv[4].split(",") if len(sys.argv) > 4 else [p.stem for p in src.glob("*.png") if "-" not in p.stem]
out.mkdir(parents=True, exist_ok=True)
x, y, w, h = CROPS[card]
for n in names:
    im = Image.open(src / f"{n}.png")
    k = im.size[0] / 1176
    im.crop((round(x * k), round(y * k), round((x + w) * k), round((y + h) * k))).save(out / f"{n}.png")
    print(n)
