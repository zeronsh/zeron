"""Shared by the recreation tools: the two native references, sampling,
and finding the shapes the app drew (filled boxes, their corners and edges)."""
import io, json, math
import numpy as np
from PIL import Image

import os
W, H, S = 1320, 840, 2
# The themes captured (ref-<theme>.png), the first the one shapes are read from;
# NAME, what is being built; GLASS, whether its surfaces keep the alpha the app drew.
THEMES = os.environ.get("THEMES", "dark,light").split(",")
P = THEMES[0]
NAME = os.environ.get("NAME", "session")
GLASS = os.environ.get("GLASS") == "1"

def raw(path):
    """A capture as the app drew it: colour (0..255) and alpha (0..1), un-premultiplied."""
    a = np.asarray(Image.open(path).convert("RGBA")).astype(np.float64)
    return a[:, :, :3], a[:, :, 3] / 255

RAW = {t: raw(f"ref-{t}.png") for t in THEMES}
# What a capture is laid on to be read: the window's own colour, opaque (the app's opaque look).
GROUND = {"dark": (14, 14, 16), "light": (238, 238, 240)}
for t in THEMES:
    GROUND.setdefault(t, tuple(int(round(v)) for v in RAW[t][0][400, 4]))

def load(path, ground):
    im = Image.open(path).convert("RGBA")
    return np.asarray(Image.alpha_composite(Image.new("RGBA", im.size, tuple(ground) + (255,)), im).convert("RGB")).astype(np.int16)

REFS = {t: load(f"ref-{t}.png", GROUND[t]) for t in THEMES}

def raw_at(theme, x, y):
    rgb, alpha = RAW[theme]
    i, j = min(max(int(y * S), 0), rgb.shape[0] - 1), min(max(int(x * S), 0), rgb.shape[1] - 1)
    return rgb[i, j], float(alpha[i, j])

def own(target, parent):
    """The colour and alpha a surface must be painted in to show as `target` over `parent`
    (each a colour and an alpha): source-over, undone. Where the alpha it needs is too
    little to carry the colour, the least alpha that can."""
    (ct, at_), (cp, ap) = target, parent
    if ap >= 0.999: return ct, 1.0
    least = min(max((at_ - ap) / (1 - ap), 0.0), 1.0)
    if least < 0.002 and np.abs(ct * at_ - cp * ap).max() < 1.0: return ct, 0.0
    for a in [least] + [v / 200 for v in range(1, 201)]:
        if a < least or a <= 0: continue
        c = (ct * at_ - cp * ap * (1 - a)) / a
        if (c >= -1.5).all() and (c <= 256.5).all(): return np.clip(c, 0, 255), a
    return ct, 1.0

def css_rgba(colour, alpha):
    r, g, b = (int(round(float(v))) for v in colour)
    return hexc(colour) if alpha >= 0.999 else f"rgb({r} {g} {b} / {alpha:.3f})"

def at(theme, x, y):
    a = REFS[theme]
    return a[min(max(int(y * S), 0), a.shape[0] - 1), min(max(int(x * S), 0), a.shape[1] - 1)]

def crop(theme, box):
    x, y, w, h = box
    return REFS[theme][int(round(y * S)):int(round((y + h) * S)), int(round(x * S)):int(round((x + w) * S))]

def hexc(c):
    return "#%02x%02x%02x" % tuple(int(min(max(round(float(v)), 0), 255)) for v in c)

def near(a, b, tol=7):
    return int(np.abs(np.asarray(a) - np.asarray(b)).sum()) <= tol

def ink_of(theme, box):
    """The ground under a node (the commonest colour of its box) and its ink (the pixel farthest from it)."""
    p = crop(theme, box).reshape(-1, 3)
    vals, counts = np.unique(p, axis=0, return_counts=True)
    ground = vals[counts.argmax()]
    return ground, p[np.abs(p - ground).sum(axis=1).argmax()]

def ink_span(theme, box, gap=None):
    """Where the ink lies across a node's box, in points from its left edge: (first, last). Rows that are
    ink all the way across (a hairline through the box) don't count. With `gap`, the ink ends at the
    first empty stretch that wide (a title cut short before what follows it on its row)."""
    c = crop(theme, box)
    ground, _ = ink_of(theme, box)
    on = np.abs(c - ground).sum(axis=2) > 24
    on = on[on.mean(axis=1) < 0.8]
    cols = np.where(on.any(axis=0))[0]
    if not len(cols): return (0, box[2])
    first, last = cols[0], cols[-1]
    if gap:
        for a, b in zip(cols, cols[1:]):
            if b - a > gap * S:
                last = a
                break
    return (first / S, (last + 1) / S)

def find_rect(seed, theme=P, tol=7):
    """The filled box around a point: its bounds in points, corner radius, fill, and 1-point edge if it has one.
    Text and icons inside block single scan lines, so each side is the farthest any line reaches."""
    a = REFS[theme]
    sx, sy = int(seed[0] * S), int(seed[1] * S)
    fill = a[sy, sx].copy()
    same = np.abs(a - fill).sum(axis=2) <= tol
    def reach(line, start):
        """How far a line of the mask runs each way from a start index (inclusive bounds)."""
        before = np.where(~line[:start][::-1])[0]
        after = np.where(~line[start + 1:])[0]
        return start - (before[0] if len(before) else start), start + (after[0] if len(after) else len(line) - 1 - start)
    t, b = reach(same[:, sx], sy)
    l, r = sx, sx
    for _ in range(3):
        spans = [reach(same[y], sx) for y in range(t, b + 1)]
        l, r = min(s[0] for s in spans), max(s[1] for s in spans)
        cols = [reach(same[:, x], sy) for x in range(l, r + 1)]
        t, b = min(c[0] for c in cols), max(c[1] for c in cols)
    # The corner: the radius whose quarter circle best covers what the top-left block shows.
    n = int(min(26 * S, (r - l + 1) // 2, (b - t + 1) // 2))
    radius = 0.0
    if n >= 2:
        block = same[t:t + n, l:l + n]
        yy, xx = np.mgrid[0:n, 0:n] + 0.5
        best = (1e9, 0.0)
        for R in [v * S for v in (0, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 22, 24)] + [n]:
            inside = ((xx >= R) | (yy >= R) | ((xx - R) ** 2 + (yy - R) ** 2 <= R * R))
            miss = float((inside != block)[: int(R) + 2, : int(R) + 2].mean()) if R else float((~block[:4, :4]).mean())
            if miss < best[0] - 1e-9: best = (miss, R / S)
        radius = best[1]
    box = [l / S, t / S, (r - l + 1) / S, (b - t + 1) / S]
    # An edge: a ring a point wide of one other colour just outside the fill, on rows that reach the left side.
    edge = None
    rows = [y for y in range(t, b + 1) if same[y, l]][:: max((b - t) // 7, 1)]
    if l - S - 3 >= 0 and len(rows) >= 3:
        rings = [a[y, l - 1] for y in rows]
        beyond = [a[y, l - S - 2] for y in rows]
        if all(near(q, rings[0], 5) for q in rings) and not near(rings[0], fill, tol) and not near(rings[0], beyond[0], 4):
            edge = rings[0].copy()
            box = [box[0] - 1, box[1] - 1, box[2] + 2, box[3] + 2]
            radius = radius + 1 if radius else 0
    return {"box": box, "radius": radius, "fill": fill, "edge": edge, "seed": list(seed)}

def sample_rect(rect, theme):
    """The same box's fill and edge in another reference."""
    fill = at(theme, *rect["seed"])
    edge = None
    if rect["edge"] is not None:
        x, y, w, h = rect["box"]
        edge = at(theme, x + 0.25, y + h / 2)
    return fill, edge

def cover(box, radius, x0, y0, nx, ny):
    """A rounded box's coverage of each device pixel in a patch (the app's antialiasing: half a pixel either side of the edge)."""
    x, y, w, h = (v * S for v in box)
    r = min(radius * S, w / 2, h / 2)
    px, py = x0 + np.arange(nx) + 0.5, y0 + np.arange(ny) + 0.5
    qx = np.abs(px[None, :] - (x + w / 2)) - (w / 2 - r)
    qy = np.abs(py[:, None] - (y + h / 2)) - (h / 2 - r)
    d = np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - r
    return np.clip(0.5 - d, 0, 1)

def refine(rect, radii, theme=P):
    """Settle a small box to the half point and its radius: the shape whose painted rim best matches the reference's."""
    a = REFS[theme]
    box, fill = list(rect["box"]), rect["fill"].astype(float)
    m = 3
    x0, y0 = int((box[0] - m) * S), int((box[1] - m) * S)
    nx, ny = int((box[2] + 2 * m) * S), int((box[3] + 2 * m) * S)
    patch = a[y0:y0 + ny, x0:x0 + nx].astype(float)
    ground = np.median(np.concatenate([patch[0], patch[-1], patch[:, 0], patch[:, -1]]), axis=0)
    def err(b, r):
        c = cover(b, r, x0, y0, nx, ny)
        grown = cover([b[0] - 2, b[1] - 2, b[2] + 4, b[3] + 4], r + 2, x0, y0, nx, ny)
        shrunk = cover([b[0] + 2.5, b[1] + 2.5, max(b[2] - 5, 0), max(b[3] - 5, 0)], max(r - 2.5, 0), x0, y0, nx, ny)
        rim = (grown - shrunk) > 0.5
        pred = ground + (fill - ground) * c[:, :, None]
        return float(np.abs(pred - patch).sum(axis=2)[rim].mean())
    best = (err(box, rect["radius"]), box, rect["radius"])
    for _ in range(2):
        for axis in (0, 1):
            for d0 in (-1, -0.5, 0, 0.5, 1):
                for d1 in (-1, -0.5, 0, 0.5, 1):
                    for r in radii:
                        b = list(best[1])
                        b[axis] += d0; b[axis + 2] += d1 - d0
                        e = err(b, r)
                        if e < best[0] - 1e-6: best = (e, b, r)
    rect["box"], rect["radius"] = best[1], best[2]
    return rect

def hlines(box, seed, theme=P):
    """Hairlines across a box: rows that are one other colour nearly all the way across, a pixel or two tall."""
    a = REFS[theme]
    x, y, w, h = box
    fill = at(theme, *seed)
    block = a[int(y * S):int((y + h) * S), int((x + 1) * S):int((x + w - 1) * S)]
    off = (np.abs(block - fill).sum(axis=2) > 5).mean(axis=1)
    lines, r = [], 0
    while r < len(off):
        if off[r] > 0.93:
            e = r
            while e + 1 < len(off) and off[e + 1] > 0.93: e += 1
            if e - r + 1 <= 2 and (r < 3 or off[r - 3] < 0.6) and (e + 3 >= len(off) or off[e + 3] < 0.6):
                lines.append((y + r / S, (e - r + 1) / S))
            r = e + 1
        else:
            r += 1
    return lines
