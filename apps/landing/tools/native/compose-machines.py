#!/usr/bin/env python3
"""The machines card's native reference at each pointer pose.

Popups open only while the fixture window is frontmost, which it is just
after launch, not after the waits a pose takes. What they cover, and the
place chip washed while they're open, don't depend on the pose (they're
opaque, over the composer's tray). So: one pass right at launch with the
picker open (`open/`: the window, rest.png, and each popup alone with its
box), and the poses with nothing open (`poses/`); each pose gets the open
chip's pixels from `open/` and the popups laid over it, alpha and all, as
reference.py's POPUPS lays them.

usage: compose-machines.py <open dir> <poses dir> <out dir> [names]
"""
import re
import sys
from pathlib import Path
from PIL import Image

CHIP = (367, 422, 194, 24)
opened, poses, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
names = sys.argv[4].split(",") if len(sys.argv) > 4 else ["rest"]
out.mkdir(parents=True, exist_ok=True)
open_window = Image.open(opened / "rest-open.png").convert("RGBA")
k = open_window.size[0] / 1176
popups = []
for m in re.finditer(r"popup @(\d+) (-?[\d.]+),(-?[\d.]+) ", ((opened / "open-windows.txt") if (opened / "open-windows.txt").exists() else (opened / "rest-windows.txt")).read_text()):
    popups.append((Image.open(opened / f"rest-popup-{m.group(1)}.png").convert("RGBA"), float(m.group(2)), float(m.group(3))))
assert len(popups) == 2, f"expected both popups, got {len(popups)}"
x, y, w, h = CHIP
box = tuple(round(v * k) for v in (x, y, x + w, y + h))
chip = open_window.crop(box)
for n in names:
    im = Image.open(poses / f"{n}.png").convert("RGBA")
    im.paste(chip, box[:2])
    for pop, px, py in popups:
        im.alpha_composite(pop, (round(px * k), round(py * k)))
    im.save(out / f"{n}.png")
    print(n)
