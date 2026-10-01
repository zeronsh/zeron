#!/usr/bin/env python3
"""Measure the Android count badges on an emulator screenshot of `--es route badges` (see docs/android.md).

  measure-badges.py shot.png --dark --fits shapes.json     # badge alignment table
  measure-badges.py shot.png --tiles                       # harness-tile boxes (rows or grid)

Per badge: the shape's bounding box, the glyph ink box (threshold halfway between the fill and the label
colour), the ink centre's offset from the shape's optical centre (the fit computed from the polygon, dumped
by BadgeGeometryTest into --fits: `ZERON_BADGE_FITS=/tmp/fits.json ./gradlew :app:testDebugUnitTest --tests '*BadgeGeometryTest*'`) and from the bounding-box centre, the digit height and the ink-box area as
shares of the footprint and of the shape.
"""
import json, sys
from collections import deque
import numpy as np
from PIL import Image

COUNTS = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 20, 21, 67, 99, 100, 2**32 - 1]
SHAPES = {1: "Pill", 2: "Arch", 3: "Triangle", 4: "Diamond", 5: "Pentagon", 6: "Gem", 7: "Cookie7Sided", 8: "Clover8Leaf", 9: "PuffyDiamond"}


def shape_of(n):
    if n in SHAPES: return SHAPES[n]
    if 10 <= n <= 20: return "ClamShell"
    if 21 <= n <= 99: return "Puffy"
    return "Heart"


def components(mask, min_area):
    h, w = mask.shape
    seen = np.zeros_like(mask, bool)
    out = []
    for y in range(h):
        for x in range(w):
            if mask[y, x] and not seen[y, x]:
                q = deque([(y, x)]); seen[y, x] = True; pts = []
                while q:
                    cy, cx = q.popleft(); pts.append((cy, cx))
                    for dy in (-1, 0, 1):
                        for dx in (-1, 0, 1):
                            ny, nx = cy + dy, cx + dx
                            if 0 <= ny < h and 0 <= nx < w and mask[ny, nx] and not seen[ny, nx]:
                                seen[ny, nx] = True; q.append((ny, nx))
                if len(pts) >= min_area: out.append(pts)
    return out


def close(img, rgb, tol):
    return np.abs(img.astype(int) - np.array(rgb)).sum(axis=2) <= tol


def badges(img, fill, ink):
    comps = components(close(img, fill, 40), 300)
    res = []
    for pts in comps:
        ys = [p[0] for p in pts]; xs = [p[1] for p in pts]
        y0, y1, x0, x1 = min(ys), max(ys), min(xs), max(xs)
        crop = img[y0:y1 + 1, x0:x1 + 1].astype(float)
        solid = close(img[y0:y1 + 1, x0:x1 + 1], fill, 120)
        # Interior = everything not reachable from the crop border without crossing the fill.
        h, w = solid.shape
        outside = np.zeros((h, w), bool); q = deque()
        for yy in range(h):
            for xx in (0, w - 1):
                if not solid[yy, xx] and not outside[yy, xx]: outside[yy, xx] = True; q.append((yy, xx))
        for xx in range(w):
            for yy in (0, h - 1):
                if not solid[yy, xx] and not outside[yy, xx]: outside[yy, xx] = True; q.append((yy, xx))
        while q:
            cy, cx = q.popleft()
            for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                ny, nx = cy + dy, cx + dx
                if 0 <= ny < h and 0 <= nx < w and not solid[ny, nx] and not outside[ny, nx]:
                    outside[ny, nx] = True; q.append((ny, nx))
        interior = ~outside
        # Ink: closer to the label colour than to the fill.
        f = np.array(fill, float); k = np.array(ink, float)
        t = ((crop - f) @ (k - f)) / ((k - f) @ (k - f))
        inkm = interior & (t > 0.5)
        if inkm.sum() == 0: continue
        iy, ix = np.nonzero(inkm)
        res.append(dict(x0=x0, y0=y0, w=x1 - x0 + 1, h=y1 - y0 + 1, area=int(interior.sum()),
                        ix0=x0 + ix.min(), ix1=x0 + ix.max() + 1, iy0=y0 + iy.min(), iy1=y0 + iy.max() + 1))
    return res


def main():
    a = sys.argv[1:]
    img = np.array(Image.open(a[0]).convert("RGB"))
    dark = "--dark" in a
    if "--tiles" in a:
        # Tiles: the harness mark's orange, grown to its 48dp tile by colour.
        density = float(a[a.index("--density") + 1]) if "--density" in a else 2.625
        mark = components(close(img, (0xD9, 0x77, 0x57), 90), 150)
        # Group fragments of one asterisk: sort by y, merge those closer than 20dp.
        boxes = []
        for pts in mark:
            ys = [p[0] for p in pts]; xs = [p[1] for p in pts]
            boxes.append([min(xs), min(ys), max(xs), max(ys)])
        merged = []
        for b in sorted(boxes, key=lambda b: (b[1], b[0])):
            for m in merged:
                if abs((b[0] + b[2]) / 2 - (m[0] + m[2]) / 2) < 20 * density and abs((b[1] + b[3]) / 2 - (m[1] + m[3]) / 2) < 20 * density:
                    m[0] = min(m[0], b[0]); m[1] = min(m[1], b[1]); m[2] = max(m[2], b[2]); m[3] = max(m[3], b[3]); break
            else:
                merged.append(b[:])
        print(f"{len(merged)} tiles")
        prev = None
        for m in merged:
            cx, cy = (m[0] + m[2]) // 2, (m[1] + m[3]) // 2
            tile = img[cy, int(cx - 18 * density)].astype(int); bg = img[cy, int(cx - 33 * density)].astype(int)
            tol = max(2, int(np.abs(tile - bg).sum() // 2))
            H, W = img.shape[:2]
            win = int(30 * density)
            x0w, x1w, y0w, y1w = max(0, cx - win), min(W, cx + win), max(0, cy - win), min(H, cy + win)
            mask = np.abs(img[y0w:y1w, x0w:x1w].astype(int) - tile).sum(axis=2) <= tol
            sx, sy = int(cx - 18 * density) - x0w, cy - y0w
            seen = np.zeros_like(mask); q = deque([(sy, sx)]); seen[sy, sx] = True; xs = []; ys = []
            while q:
                y, x = q.popleft(); xs.append(x); ys.append(y)
                for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    ny, nx = y + dy, x + dx
                    if 0 <= ny < mask.shape[0] and 0 <= nx < mask.shape[1] and mask[ny, nx] and not seen[ny, nx]:
                        seen[ny, nx] = True; q.append((ny, nx))
            bx0, bx1, by0, by1 = min(xs) + x0w, max(xs) + x0w + 1, min(ys) + y0w, max(ys) + y0w + 1
            step = "" if prev is None else f"  dy-from-previous {cy - prev}"
            print(f"mark centre ({cx},{cy})  tile box x {bx0}-{bx1} y {by0}-{by1}  = {bx1-bx0}x{by1-by0}px ({(bx1-bx0)/density:.1f}x{(by1-by0)/density:.1f}dp)  tile centre - mark centre ({(bx0+bx1)/2-cx:+.1f},{(by0+by1)/2-cy:+.1f}){step}")
            prev = cy
        return
    fill = (0x7C, 0x61, 0xDB) if dark else (0x5B, 0x43, 0xE8)
    ink = (255, 255, 255)
    fits = json.load(open(a[a.index("--fits") + 1])) if "--fits" in a else None
    found = sorted(badges(img, fill, ink), key=lambda b: (round(b["y0"] / 100), b["x0"]))
    print(f"{len(found)} badges")
    rows = []
    for n, b in zip(COUNTS, found):
        shape = shape_of(n)
        digits = len(str(n)) if n < 100 else 0
        F = max(b["w"], b["h"])
        bcx, bcy = b["x0"] + b["w"] / 2, b["y0"] + b["h"] / 2
        icx, icy = (b["ix0"] + b["ix1"]) / 2, (b["iy0"] + b["iy1"]) / 2
        iw, ih = b["ix1"] - b["ix0"], b["iy1"] - b["iy0"]
        r = dict(count=n if n < 2**32 - 1 else "max", shape=shape, F=F, bw=b["w"], bh=b["h"],
                 dxb=(icx - bcx) / F * 100, dyb=(icy - bcy) / F * 100, ih=ih / F * 100, iw=iw / F * 100,
                 share=iw * ih / b["area"] * 100)
        if fits:
            key = {1: "f1", 2: "f2", 0: "f3"}[digits]
            fx, fy, fh = fits[shape][key]
            r["dxo"] = (icx - (bcx + (fx - 0.5) * F)) / F * 100
            r["dyo"] = (icy - (bcy + (fy - 0.5) * F)) / F * 100
        rows.append(r)
    hdr = f"{'count':>5} {'shape':<13}{'F':>4}{'bbox':>9} {'dx_bb%':>7}{'dy_bb%':>7}{'dx_opt%':>8}{'dy_opt%':>8}{'inkH%F':>8}{'inkW%F':>8}{'ink/shape%':>11}"
    print(hdr)
    for r in rows:
        print(f"{r['count']:>5} {r['shape']:<13}{r['F']:>4}{str(r['bw'])+'x'+str(r['bh']):>9} {r['dxb']:7.1f}{r['dyb']:7.1f}{r.get('dxo', float('nan')):8.1f}{r.get('dyo', float('nan')):8.1f}{r['ih']:8.1f}{r['iw']:8.1f}{r['share']:11.1f}")
    if rows:
        sh = [r["share"] for r in rows]
        print(f"ink-box share of shape area: min {min(sh):.1f}%  max {max(sh):.1f}%")
        if fits:
            print(f"|dx_opt| max {max(abs(r['dxo']) for r in rows):.1f}%  |dy_opt| max {max(abs(r['dyo']) for r in rows):.1f}%")
    json.dump(rows, open("/tmp/badge-measure.json", "w"))


main()
