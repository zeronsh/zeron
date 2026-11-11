#!/usr/bin/env python3
"""The visuals card's native reference: the app's own two pictures composed.

A session page has no wallpaper, so the card is a cut-out of the session
page (its region PIECE, as the app draws it) laid on the app's new chat over
Aurora (its wallpaper, the same window), rounded as the page card is (12)
and shadowed as the composer is ("0 12 32 #00000066": zgx's box shadow, the
box offset, 0.5·erfc(d/(σ√2)), σ = 16, its erf approximated as draw.wgsl
does). public/hero/cards.js draws the same frame.

usage: compose-visuals.py <ground dir> <session.png> <out dir> [names]
  the ground's pictures by pose (rest, tl, …), the session page at rest
  (nothing in it moves), written cropped to the card.
"""
import math
import sys
from pathlib import Path
from PIL import Image

CROP = (308, 110, 560, 388)
PIECE = (336, 128, 504, 352)
RADIUS = 12
SHADOW = (0, 12, 32, 0.4)

ground_dir, session, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
names = sys.argv[4].split(",") if len(sys.argv) > 4 else ["rest"]
out.mkdir(parents=True, exist_ok=True)
page = Image.open(session).convert("RGB")
k = page.size[0] / 1176


def erf_approx(x):
    x2 = x * x
    return math.copysign(math.sqrt(1 - math.exp(-x2 * (1.27323954 + 0.147 * x2) / (1 + 0.147 * x2))), x)


def sdf(px, py, box, r):
    x, y, w, h = box
    hw, hh = w / 2, h / 2
    qx = abs(px - (x + hw)) - (hw - r)
    qy = abs(py - (y + hh)) - (hh - r)
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


cx, cy, cw, ch = CROP
W, H = round(cw * k), round(ch * k)
# The frame's cover per pixel, worked out once: the shadow's alpha and the
# piece's coverage.
dx, dy, blur, alpha = SHADOW
sigma = blur / 2
shadow_box = (PIECE[0] + dx, PIECE[1] + dy, PIECE[2], PIECE[3])
frame = []
for j in range(H):
    py = cy + (j + 0.5) / k
    row = []
    for i in range(W):
        px = cx + (i + 0.5) / k
        d = sdf(px, py, shadow_box, RADIUS)
        a = alpha * (0.5 - 0.5 * erf_approx(d / (sigma * math.sqrt(2))))
        cover = min(max(0.5 - sdf(px, py, PIECE, RADIUS) * k, 0), 1)
        row.append((a, cover))
    frame.append(row)

for name in names:
    ground = Image.open(ground_dir / f"{name}.png").convert("RGB")
    g = ground.crop((round(cx * k), round(cy * k), round((cx + cw) * k), round((cy + ch) * k)))
    p = page.crop((round(cx * k), round(cy * k), round((cx + cw) * k), round((cy + ch) * k)))
    gp, pp = g.load(), p.load()
    res = Image.new("RGB", (W, H))
    rp = res.load()
    for j in range(H):
        for i in range(W):
            a, cover = frame[j][i]
            c = gp[i, j]
            c = tuple(v * (1 - a) for v in c)
            q = pp[i, j]
            rp[i, j] = tuple(round(c[n] * (1 - cover) + q[n] * cover) for n in range(3))
    res.save(out / f"{name}.png")
    print(name)
