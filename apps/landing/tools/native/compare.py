#!/usr/bin/env python3
"""Pixel-diff the web hero against the app's own render.

usage: compare.py <native dir> <web dir> <out dir> [names] [--region name=x,y,w,h ...]
Regions are in points (the pictures are 2x). Prints, per picture and region:
the mean absolute error per channel (of 255), the share of pixels off by
more than 8 and by more than 32 (any channel), and PSNR; writes the
native | web | difference x4 side by side, and the difference alone.
"""
import math
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageStat

args = sys.argv[1:]
native, web, out = Path(args[0]), Path(args[1]), Path(args[2])
names = args[3].split(",") if len(args) > 3 and not args[3].startswith("--") else ["rest"]
# WIDTH=<points>: the pictures' width in points (a card's 560; the window's 1176).
WIDTH = float(__import__("os").environ.get("WIDTH", "1176"))
regions = {"window": (0, 0, WIDTH, 768 if WIDTH == 1176 else 388)}
if "--region" in args:
    for spec in args[args.index("--region") + 1:]:
        if spec.startswith("--"):
            break
        k, v = spec.split("=")
        regions[k] = tuple(float(x) for x in v.split(","))
out.mkdir(parents=True, exist_ok=True)


def stats(a, b):
    d = ImageChops.difference(a, b)
    mean = sum(ImageStat.Stat(d).mean[:3]) / 3
    rms = math.sqrt(sum(v * v for v in ImageStat.Stat(d).rms[:3]) / 3)
    psnr = 99.0 if rms == 0 else 20 * math.log10(255 / rms)
    # the largest channel difference per pixel
    r, g, bb = d.split()[:3]
    peak = ImageChops.lighter(ImageChops.lighter(r, g), bb)
    hist = peak.histogram()
    n = sum(hist)
    over8 = sum(hist[9:]) / n
    over32 = sum(hist[33:]) / n
    return mean, over8, over32, psnr, d


for name in names:
    a = Image.open(native / f"{name}.png").convert("RGB")
    b = Image.open(web / f"{name}.png").convert("RGB")
    if a.size != b.size:
        print(f"{name}: sizes differ {a.size} vs {b.size}")
        b = b.resize(a.size)
    sx = a.size[0] / WIDTH
    for rname, (x, y, w, h) in regions.items():
        box = tuple(int(round(v * sx)) for v in (x, y, x + w, y + h))
        mean, over8, over32, psnr, d = stats(a.crop(box), b.crop(box))
        print(f"{name:6} {rname:10} MAE {mean:5.2f}/255  >8: {over8 * 100:5.1f}%  >32: {over32 * 100:5.1f}%  PSNR {psnr:5.1f} dB")
    _, _, _, _, d = stats(a, b)
    amp = d.point(lambda v: min(255, v * 4))
    amp.save(out / f"{name}-diff.png")
    side = Image.new("RGB", (a.size[0] * 3, a.size[1]))
    side.paste(a, (0, 0))
    side.paste(b, (a.size[0], 0))
    side.paste(amp, (a.size[0] * 2, 0))
    side.save(out / f"{name}-side.png")
