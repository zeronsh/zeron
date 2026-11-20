"""Render the recreation in Chrome at 2x and diff it against the native reference, region by region."""
import os, pathlib
ROOT = pathlib.Path(__file__).resolve().parents[3]
CHROME = os.environ.get("CHROME", "chromium")
import subprocess, sys, os
import numpy as np
from PIL import Image
from lib import *
theme = sys.argv[1] if len(sys.argv) > 1 else P
index = THEMES.index(theme)
FONTS = f"{ROOT}/apps/landing/src/assets/fonts"
page = f"""<!doctype html><meta charset=utf-8><style>
@font-face {{ font-family: "Geist"; src: url(file://{FONTS}/geist.woff2); font-weight: 100 900; }}
@font-face {{ font-family: "Geist Mono"; src: url(file://{FONTS}/geist-mono.woff2); font-weight: 100 900; }}
:root {{ --font-sans: "Geist"; --font-mono: "Geist Mono"; }}
html, body {{ margin: 0; background: rgb{GROUND[theme]}; }}
{open("zw.css").read()}
{open(f"{NAME}.icons.css").read()}
{open(f"{NAME}.vars.css").read()}
{open("session.extra.css").read() if os.path.exists("session.extra.css") else ""}
</style><div class="zw zw-{NAME} t{index}">{open(f"{NAME}.fragment.html").read()}</div>"""
open("test.html", "w").write(page)
subprocess.run([CHROME, "--headless=new", "--no-sandbox", "--disable-gpu", "--disable-lcd-text", "--hide-scrollbars", "--allow-file-access-from-files", "--force-device-scale-factor=2", "--window-size=1320,840", "--virtual-time-budget=3000", f"--screenshot={os.path.abspath('web-' + theme + '.png')}", "file://" + os.path.abspath("test.html")], capture_output=True)
web = np.asarray(Image.open(f"web-{theme}.png").convert("RGB")).astype(np.int16)
ref = REFS[theme]
REGIONS = {"window (below strip)": [0, 40, 1320, 800], "sidebar": [5, 42, 272, 793], "page": [281, 42, 515, 793], "transcript": [296, 56, 490, 600], "composer": [300, 740, 480, 90],
           "panel": [800, 42, 515, 793], "panel head": [800, 42, 515, 90], "changes list": [809, 131, 497, 584], "diff": [810, 276, 495, 240], "commit dock": [800, 730, 515, 100]}
for name, (x, y, w, h) in REGIONS.items():
    a = web[y * S:(y + h) * S, x * S:(x + w) * S]; b = ref[y * S:(y + h) * S, x * S:(x + w) * S]
    d = np.abs(a - b)
    print(f"{name:22} mae {d.mean():6.2f}   >8: {100 * (d.max(axis=2) > 8).mean():5.1f}%   >32: {100 * (d.max(axis=2) > 32).mean():5.1f}%")
if len(sys.argv) > 2:
    x, y, w, h = (int(v) for v in sys.argv[2].split(","))
    a = web[y * S:(y + h) * S, x * S:(x + w) * S]; b = ref[y * S:(y + h) * S, x * S:(x + w) * S]
    side = np.concatenate([b, a, np.clip(np.abs(a - b) * 4, 0, 255)], axis=1 if w < 700 else 0).astype(np.uint8)
    Image.fromarray(side).save("side.png"); print("side.png", side.shape)
