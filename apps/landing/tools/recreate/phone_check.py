"""Render the phone recreation at 3x and diff it against the iOS app's screenshot.
phone_check.py [--tune]   prints, per element, the shift (in points) that best meets the reference."""
import os, re, subprocess, sys, pathlib
import numpy as np
from PIL import Image
ROOT = pathlib.Path(__file__).resolve().parents[3]
CHROME = os.environ.get("CHROME", "chromium")
here = pathlib.Path(os.environ.get("PHONE_SRC", ROOT / "apps/landing/src/components/app"))
FONTS = ROOT / "apps/landing/src/assets/fonts"
css = (here / "phone.css").read_text().replace("../../assets", f"file://{ROOT}/apps/landing/src/assets")
frag = (here / "phone.fragment.html").read_text()
page = f"""<!doctype html><meta charset=utf-8><style>
@font-face {{ font-family: "Geist"; src: url(file://{FONTS}/geist.woff2); font-weight: 100 900; }}
:root {{ --font-sans: "Geist", system-ui, sans-serif; }} html, body {{ margin: 0; background: #060606; }}
{css}</style><div class="zp">{frag}</div>"""
open("phone-test.html", "w").write(page)
subprocess.run([CHROME, "--headless=new", "--no-sandbox", "--disable-gpu", "--disable-lcd-text", "--hide-scrollbars", "--allow-file-access-from-files", "--force-device-scale-factor=3", "--window-size=402,874", "--virtual-time-budget=3000", "--screenshot=" + os.path.abspath("phone-web.png"), "file://" + os.path.abspath("phone-test.html")], capture_output=True)
web = np.asarray(Image.open("phone-web.png").convert("RGB")).astype(np.int16)
ref = np.asarray(Image.open(os.environ.get("PHONE_REF", ROOT / "docs/screenshots/message-captions/ios-dark.png")).convert("RGB")).astype(np.int16)
S = 3
d = np.abs(web - ref)
for name, (y0, y1) in {"screen": (0, 874), "header": (56, 112), "conversation": (120, 480), "composer": (776, 838)}.items():
    r = d[y0 * S:y1 * S]
    print(f"{name:13} mae {r.mean():5.2f}  >32: {100 * (r.max(axis=2) > 32).mean():4.1f}%")
Image.fromarray(np.concatenate([ref, web, np.clip(d * 4, 0, 255)], axis=1).astype(np.uint8)).save("phone-side.png")
if "--tune" in sys.argv:
    for m in re.finditer(r'data-n="([\w-]+)"[^>]*style="left:([\d.]+)px;top:([\d.]+)px(?:;width:([\d.]+)px)?(?:;height:([\d.]+)px)?', frag):
        name, x, y = m.group(1), float(m.group(2)), float(m.group(3))
        w = float(m.group(4) or 300); h = float(m.group(5) or 26)
        x0, y0, x1, y1 = int(x * S) - 3, int(y * S) - 3, min(int((x + w) * S) + 3, 1206 - 12), int((y + h) * S) + 3
        best = (1e18, 0, 0)
        for dy in range(-12, 13):
            for dx in range(-12, 13):
                a = web[y0:y1, x0:x1]; b = ref[y0 + dy:y1 + dy, x0 + dx:x1 + dx]
                if a.shape != b.shape or not a.size: continue
                e = float(np.abs(a - b).sum()) * (1 + 0.003 * (abs(dx) + abs(dy)))
                if e < best[0]: best = (e, dx, dy)
        if best[1] or best[2]: print(f"  {name:12} move {best[1] / S:+.2f}, {best[2] / S:+.2f}")
