#!/usr/bin/env python3
"""Writes ui/ds/orch_force_marks.luau: the providers' marks the Map's force
Graph draws in its nodes, from the SVGs the app already ships (the vendors'
own marks, ui/assets/icons), as canvas paths ready to draw:

  * every command made absolute, every curve a cubic (lines kept lines);
  * centred on their real visual bounds (the curves' own extremes, not
    their control points), so a mark sits in the middle of its node;
  * sized optically: the mark's outline (its convex hull) is given the area
    of a circle one unit across, so a starburst and a knot read the same
    size and a square-ish mark isn't heavier than a round one (never wider
    than MAX_SIDE).

Run after changing a mark there:

    python3 tools/force_marks.py
"""

import math
import pathlib
import re

here = pathlib.Path(__file__).resolve().parent.parent
icons = here / "ui" / "assets" / "icons"
out = here / "ui" / "ds" / "orch_force_marks.luau"

MARKS = [
    ("claude", "claude-mark.svg"),
    ("openai", "openai-mark.svg"),
    ("cursor", "cursor-mark.svg"),
    ("grok", "grok-mark.svg"),
    ("gemini", "registry/gemini.svg"),
    ("copilot", "registry/github-copilot-cli.svg"),
]
# The widest a mark may be, in units of the circle it's sized to.
MAX_SIDE = 1.08

NUM = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")
TOK = re.compile(r"[MmLlHhVvCcSsQqTtAaZz]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def arc_cubics(x1, y1, rx, ry, phi, fa, fs, x2, y2):
    """An SVG elliptical arc as cubics (each { c1x, c1y, c2x, c2y, x, y })."""
    if rx == 0 or ry == 0:
        return [(x1, y1, x2, y2, x2, y2)]
    rx, ry = abs(rx), abs(ry)
    cp, sp = math.cos(math.radians(phi)), math.sin(math.radians(phi))
    dx, dy = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = cp * dx + sp * dy, -sp * dx + cp * dy
    lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry)
    if lam > 1:
        s = math.sqrt(lam)
        rx, ry = rx * s, ry * s
    num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
    den = rx * rx * y1p * y1p + ry * ry * x1p * x1p
    co = math.sqrt(max(0.0, num / den)) if den else 0.0
    if fa == fs:
        co = -co
    cxp, cyp = co * rx * y1p / ry, -co * ry * x1p / rx
    cx = cp * cxp - sp * cyp + (x1 + x2) / 2
    cy = sp * cxp + cp * cyp + (y1 + y2) / 2

    def ang(ux, uy, vx, vy):
        a = math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        return a

    t1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dt = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not fs and dt > 0:
        dt -= 2 * math.pi
    elif fs and dt < 0:
        dt += 2 * math.pi
    n = max(1, math.ceil(abs(dt) / (math.pi / 2) - 1e-9))
    d = dt / n
    k = 4 / 3 * math.tan(d / 4)
    res = []

    def pt(t):
        ex, ey = rx * math.cos(t), ry * math.sin(t)
        return cp * ex - sp * ey + cx, sp * ex + cp * ey + cy

    def dv(t):
        ex, ey = -rx * math.sin(t), ry * math.cos(t)
        return cp * ex - sp * ey, sp * ex + cp * ey

    t = t1
    for _ in range(n):
        p0, p3 = pt(t), pt(t + d)
        d0, d3 = dv(t), dv(t + d)
        res.append((p0[0] + k * d0[0], p0[1] + k * d0[1], p3[0] - k * d3[0], p3[1] - k * d3[1], p3[0], p3[1]))
        t += d
    return res


def parse(d):
    """Path data → subpaths, each [("M", x, y), ("L", x, y) | ("C", 6 numbers)…, ("Z",)]."""
    toks = TOK.findall(d)
    i = 0
    cmd = None
    x = y = sx = sy = 0.0
    lc = None  # last cubic's second control (for S)
    lq = None  # last quad's control (for T)
    out = []

    def num():
        nonlocal i
        v = float(toks[i])
        i += 1
        return v

    def flag():
        # (Arc flags may be packed: "01" is two flags.)
        nonlocal i
        t = toks[i]
        if len(t) > 1 and t[0] in "01" and not t.startswith(("0.", "1.")):
            toks[i] = t[1:]
            return int(t[0])
        i += 1
        return int(float(t))

    while i < len(toks):
        if re.fullmatch(r"[A-Za-z]", toks[i]):
            cmd = toks[i]
            i += 1
            if cmd in "Zz":
                out.append(("Z",))
                x, y = sx, sy
                lc = lq = None
                continue
        rel = cmd.islower()
        c = cmd.upper()
        ox, oy = (x, y) if rel else (0.0, 0.0)
        if c == "M":
            x, y = num() + ox, num() + oy
            sx, sy = x, y
            out.append(("M", x, y))
            cmd = "l" if rel else "L"
            lc = lq = None
        elif c == "L":
            x, y = num() + ox, num() + oy
            out.append(("L", x, y))
            lc = lq = None
        elif c == "H":
            x = num() + ox
            out.append(("L", x, y))
            lc = lq = None
        elif c == "V":
            y = num() + oy
            out.append(("L", x, y))
            lc = lq = None
        elif c == "C":
            a, b, e, f, g, h = num() + ox, num() + oy, num() + ox, num() + oy, num() + ox, num() + oy
            out.append(("C", a, b, e, f, g, h))
            x, y, lc, lq = g, h, (e, f), None
        elif c == "S":
            a, b = (2 * x - lc[0], 2 * y - lc[1]) if lc else (x, y)
            e, f, g, h = num() + ox, num() + oy, num() + ox, num() + oy
            out.append(("C", a, b, e, f, g, h))
            x, y, lc, lq = g, h, (e, f), None
        elif c in "QT":
            if c == "Q":
                qx, qy = num() + ox, num() + oy
            else:
                qx, qy = (2 * x - lq[0], 2 * y - lq[1]) if lq else (x, y)
            g, h = num() + ox, num() + oy
            out.append(("C", x + 2 / 3 * (qx - x), y + 2 / 3 * (qy - y), g + 2 / 3 * (qx - g), h + 2 / 3 * (qy - h), g, h))
            x, y, lq, lc = g, h, (qx, qy), None
        elif c == "A":
            rx, ry, phi = num(), num(), num()
            fa, fs = flag(), flag()
            g, h = num() + ox, num() + oy
            for cu in arc_cubics(x, y, rx, ry, phi, fa, fs, g, h):
                out.append(("C",) + cu)
            x, y = g, h
            lc = lq = None
        else:
            raise ValueError(f"unknown command {cmd}")
    return out


def flatten(cmds, steps=48):
    """The outline's points: lines' ends, curves sampled finely."""
    pts = []
    x = y = 0.0
    for c in cmds:
        if c[0] in "ML":
            x, y = c[1], c[2]
            pts.append((x, y))
        elif c[0] == "C":
            a, b, e, f, g, h = c[1:]
            for k in range(1, steps + 1):
                t = k / steps
                mt = 1 - t
                pts.append((mt**3 * x + 3 * mt * mt * t * a + 3 * mt * t * t * e + t**3 * g, mt**3 * y + 3 * mt * mt * t * b + 3 * mt * t * t * f + t**3 * h))
            x, y = g, h
    return pts


def hull_area(pts):
    p = sorted(set(pts))
    if len(p) < 3:
        return 0.0

    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lo, hi = [], []
    for q in p:
        while len(lo) >= 2 and cross(lo[-2], lo[-1], q) <= 0:
            lo.pop()
        lo.append(q)
    for q in reversed(p):
        while len(hi) >= 2 and cross(hi[-2], hi[-1], q) <= 0:
            hi.pop()
        hi.append(q)
    h = lo[:-1] + hi[:-1]
    return abs(sum(h[i][0] * h[(i + 1) % len(h)][1] - h[(i + 1) % len(h)][0] * h[i][1] for i in range(len(h)))) / 2


def ink(parts, x0, y0, side, px=400):
    """The mark's inked share of its unit bounds (each part by its fill rule:
    even-odd as its subpaths' alternation, nonzero as their union)."""
    from PIL import Image, ImageChops, ImageDraw

    total = Image.new("1", (px, px), 0)
    for rule, cmds in parts:
        part = Image.new("1", (px, px), 0)
        sub = []
        for c in cmds + [("M", 0, 0)]:
            if c[0] == "M" and sub:
                one = Image.new("1", (px, px), 0)
                pts = [((x - x0) / side * px, (y - y0) / side * px) for x, y in flatten(sub)]
                if len(pts) >= 3:
                    ImageDraw.Draw(one).polygon(pts, fill=1)
                part = ImageChops.logical_xor(part, one) if rule == "evenodd" else ImageChops.logical_or(part, one)
                sub = []
            sub.append(c)
        total = ImageChops.logical_or(total, part)
    return total.convert("L").histogram()[255] / (px * px)


# A typical mark's ink in its outline, and how far a mark's own sways its
# size from that.
INK_REF = 0.55
INK_EASE = 0.2


def fmt(v):
    s = f"{v:.4f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


lines = [
    "-- Generated by tools/force_marks.py from the vendors' marks in",
    "-- ui/assets/icons (claude-mark, openai-mark, cursor-mark, grok-mark,",
    "-- registry/gemini, registry/github-copilot-cli): don't edit.",
    "--",
    "-- Each mark's parts (an SVG path each, with its fill rule) as commands,",
    "-- every one absolute and every curve a cubic: 1 move (x, y), 2 line",
    "-- (x, y), 3 cubic (c1x, c1y, c2x, c2y, x, y), 4 close. Centred on the",
    "-- mark's visual bounds (0, 0 their middle) and sized optically: its",
    "-- outline has the area of a circle 1 across (`side`: its bounds' widest).",
    "",
    "return {",
]
for name, rel in MARKS:
    text = (icons / rel).read_text()
    parts = []
    for m in re.finditer(r"<path\b([^>]*)>", text):
        attrs = m.group(1)
        d = re.search(r'\bd="([^"]*)"', attrs).group(1)
        rule = "evenodd" if 'fill-rule="evenodd"' in attrs else "nonzero"
        parts.append((rule, parse(d)))
    pts = [p for _, cmds in parts for p in flatten(cmds)]
    x0, x1 = min(p[0] for p in pts), max(p[0] for p in pts)
    y0, y1 = min(p[1] for p in pts), max(p[1] for p in pts)
    side = max(x1 - x0, y1 - y0)
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    # (Unit bounds first, then the optical size: the hull's area a circle's,
    # eased by how much of it is ink: a solid mark reads bigger than an
    # open one of the same outline.)
    area = hull_area([((px - cx) / side, (py - cy) / side) for px, py in pts])
    density = ink(parts, x0, y0, side) / area
    k = min(math.sqrt((math.pi / 4) / area) * (INK_REF / density) ** INK_EASE, MAX_SIDE) / side
    lines.append(f"\t{name} = {{")
    lines.append(f"\t\tside = {fmt(side * k)},")
    lines.append("\t\tparts = {")
    for rule, cmds in parts:
        nums = []
        for c in cmds:
            if c[0] == "M":
                nums += ["1", fmt((c[1] - cx) * k), fmt((c[2] - cy) * k)]
            elif c[0] == "L":
                nums += ["2", fmt((c[1] - cx) * k), fmt((c[2] - cy) * k)]
            elif c[0] == "C":
                nums += ["3"] + [fmt((v - (cx if j % 2 == 0 else cy)) * k) for j, v in enumerate(c[1:])]
            else:
                nums.append("4")
        lines.append(f'\t\t\t{{ rule = "{rule}", cmds = {{ {", ".join(nums)} }} }},')
    lines.append("\t\t},")
    lines.append("\t},")
    print(f"{name}: side {side * k:.3f} unit hull {area:.3f} ink {density:.2f}")
lines.append("}")
out.write_text("\n".join(lines) + "\n")
print(out)
