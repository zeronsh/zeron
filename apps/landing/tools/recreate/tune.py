"""Move each placed node by what makes it meet the reference: for every text
and icon, the whole-device-pixel shift (a few pixels at most) of the web render that
best matches the app's pixels in its box. Adds to offsets.json."""
import json, os, sys
import numpy as np
from PIL import Image
from lib import *
theme = P
web = np.asarray(Image.open(f"web-{theme}.png").convert("RGB")).astype(np.int16)
ref = REFS[theme]
offsets = json.load(open("offsets.json")) if os.path.exists("offsets.json") else {}
moved = 0
for item in json.load(open("layout.json")):
    x, y, w, h = item["box"]
    if y < 40: continue
    dxo, dyo = offsets.get(str(item["n"]), (0, 0))
    x0, y0, x1, y1 = int((x + dxo) * S) - 2, int((y + dyo) * S) - 2, int((x + dxo + w) * S) + 2, int((y + dyo + h) * S) + 2
    x1 = min(x1, 1305 * S)
    best = (1e18, 0, 0)
    for dy in range(-6, 7):
        for dx in range(-4, 5):
            a = web[y0:y1, x0:x1]
            b = ref[y0 + dy:y1 + dy, x0 + dx:x1 + dx]
            if a.shape != b.shape or a.size == 0: continue
            err = float(np.abs(a - b).sum()) * (1 + 0.004 * (abs(dx) + abs(dy)))
            if err < best[0]: best = (err, dx, dy)
    _, dx, dy = best
    if dx or dy:
        moved += 1
        offsets[str(item["n"])] = [dxo + dx / S, dyo + dy / S]
json.dump(offsets, open("offsets.json", "w"))
print("moved", moved)
