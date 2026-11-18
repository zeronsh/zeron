"""Build the session window's HTML from the app's layout tree and its two
native references (dark, light). Everything placed here is placed where the
app placed it; colours are the app's own pixels."""
import os, pathlib
ROOT = pathlib.Path(__file__).resolve().parents[3]
CHROME = os.environ.get("CHROME", "chromium")
import io, json, re, sys, html, urllib.parse
import numpy as np
import cairosvg
from PIL import Image
from lib import *

nodes = json.load(open("nodes.json"))
fits = json.load(open("fits.json"))
ICONS = json.load(open("icons.json"))
LANDING = f"{ROOT}/apps/landing"
for mark in ("claude", "openai", "grok", "cursor", "opencode"):
    ICONS["mark-" + mark] = open(f"{LANDING}/src/assets/agents/{mark}.svg").read()

STRIP = 40          # the title strip's height
LEFT_SHIFT = 78     # macOS: the strip's left controls start past the traffic lights
RIGHT_SHIFT = 104   # and its right controls reach the edge (no caption buttons)

def strip_x(x, y):
    """A strip node's place on macOS, or None for what macOS doesn't draw."""
    if y >= STRIP - 4: return x
    if x >= 1206: return None
    return x + (RIGHT_SHIFT if x >= 1000 else LEFT_SHIFT)

def every(fn):
    """What a function finds in each theme's capture."""
    return [fn(t) for t in THEMES]

def colour_vars(values, prefix="c"):
    """One colour a theme, as custom properties (--c0, --c1, …) the stylesheet picks from by the window's theme."""
    return ";".join(f"--{prefix}{i}:{v if isinstance(v, str) else hexc(v)}" for i, v in enumerate(values))

out = []   # (layer, html)
import os
OFFSETS = json.load(open("offsets.json")) if os.path.exists("offsets.json") else {}
layout = []
def placed(n, x, y, w, h):
    """Where a node goes: the app's box, moved by what the diff against the reference found (tune.py)."""
    dx, dy = OFFSETS.get(str(n), (0, 0))
    layout.append({"n": n, "box": [x, y, w, h]})
    return x + dx, y + dy

css_icons = {}

# ---------------------------------------------------------------- boxes
tree = json.load(open("tree.json"))
by_id = {n["id"]: n["box"] for n in tree if n.get("id")}
by_n = {n["n"]: n["box"] for n in tree}
_hidden = {int(m.group(1)) for m in re.finditer(r"@(\d+)[^\n]* hidden\s*$", open("tree.txt").read(), re.M)}
def box_of(role, text, where=lambda b: True):
    """The box of the visible node with this role and exact text."""
    for n in tree:
        if n.get("role") == role and n.get("text") == text and n["n"] not in _hidden and where(n["box"]):
            return n["box"]
    raise KeyError((role, text))
OPEN = next(n["text"] for n in tree if n.get("role") == "tab" and str(n.get("id", "")).startswith("session-tab-"))

emitted = []
ROOT_SEED = [2, 200]  # a point of the window that no surface covers

def emit_box(name, r):
    x, y, w, h = r["box"]
    nx = strip_x(x, y)
    if nx is None: return
    sx, sy = r["seed"]
    edge_at = (x + 0.25, y + h / 2)
    if GLASS:
        # Each theme's fill as what must be painted over what lies under it (the box it sits in, else the window).
        under = min((b for b in emitted if b["box"][0] <= sx <= b["box"][0] + b["box"][2] and b["box"][1] <= sy <= b["box"][1] + b["box"][3] and b["box"][2] * b["box"][3] > w * h), key=lambda b: b["box"][2] * b["box"][3], default={"seed": ROOT_SEED})
        fills = [css_rgba(*own(raw_at(t, sx, sy), raw_at(t, *under["seed"]))) for t in THEMES]
        edges = [css_rgba(*own(raw_at(t, *edge_at), raw_at(t, sx, sy))) for t in THEMES]
    else:
        fills = [at(t, sx, sy) for t in THEMES]
        edges = [at(t, *edge_at) for t in THEMES]
    emitted.append({"box": [x, y, w, h], "seed": [sx, sy]})
    style = f"left:{nx:g}px;top:{y:g}px;width:{w:g}px;height:{h:g}px;border-radius:{r['radius']:g}px;{colour_vars(fills, 'f')}"
    if r["edge"] is not None:
        style += ";" + colour_vars(edges, "e")
    out.append((-w * h, f'<i class="b{" e" if r["edge"] is not None else ""}" data-b="{name}" style="{style}"></i>'))
    print(f"{name:13} {x:7.1f},{y:6.1f} {w:6.1f}x{h:5.1f} r{r['radius']:g} fill {hexc(r['fill'])} edge {hexc(r['edge']) if r['edge'] is not None else '-'}", file=sys.stderr)

def known(name, box, radius, seed, edge=True):
    """A box the tree places: its fill from the reference, and its edge if its rim is another colour."""
    x, y, w, h = box
    fill = at(P, *seed).copy()
    ring, outside = at(P, x + 0.25, y + h / 2), at(P, x - 1.5, y + h / 2)
    has_edge = edge and not near(ring, fill, 6) and not near(ring, outside, 3)
    emit_box(name, {"box": list(box), "radius": radius, "fill": fill, "edge": ring.copy() if has_edge else None, "seed": list(seed)})

def found(name, seed, radii=None):
    """A box only the pixels place."""
    r = find_rect(seed)
    if radii: r = refine(r, radii)
    emit_box(name, r)
    return r

composer = by_id["chat-composer"]
known("sidebar", by_id["sidebar-content"], 12, (9, 400))
known("page", by_id["shell-page-card"], 12, (286, 300))
PANEL = any(n.get("id") == "panel-tab-chosen" and n["n"] not in _hidden and n["box"][2] > 0 for n in tree)  # the side panel, open
if PANEL:
    known("panel", by_id["panel"], 12, (804.5, 600))
    known("changes", [809, 131, 497, 584], 12, (815, 650))
    changes = {"box": [809, 131, 497, 584]}
    found("commit-field", (814, 757), (6, 8, 10))
    found("commit", (1240, 806), (5, 6, 7, 8))
known("tab", by_id["session-tab-chosen"], 8, (340, 21))
if PANEL: known("panel-tab", by_id["panel-tab-chosen"], 8, (900, 63))
row = box_of("button", OPEN, lambda b: b[0] < 280)
known("row", row, 8, (row[0] + 4, row[1] + 3))
ask = next(n["box"] for n in tree if n.get("role") == "text" and str(n.get("text", "")).startswith("Clients can send"))
found("bubble", (ask[0] - 8, ask[1] + 12), (12, 14, 16, 18, 20))
known("composer", [composer[0], composer[1] + 0.5, composer[2], 48], 24, (composer[0] + 190, composer[1] + 5))
attach = box_of("button", "Attach")
found("attach", (attach[0] + 3.5, attach[1] + 14), (14,))
send = box_of("button", "Send message")
found("send", (send[0] + 3.5, send[1] + 14), (14,))
if PANEL:
    known("hunk", [810, 276, 495, 24], 0, (1290, 288), edge=False)
    known("minus", [810, 366, 495, 22], 0, (813, 377), edge=False)
    known("plus", [810, 388, 495, 66], 0, (813, 410), edge=False)
    for name, seed in (("check-all", (824, 146)), ("check-1", (824, 255)), ("check-2", (824, 565))):
        r = find_rect(seed); r["radius"] = 4; emit_box(name, r)
    # Hairlines across the panel and its changes card.
    for name, box, seed in (("changes", changes["box"], (815, 650)), ("panel", by_id["panel"], (804.5, 600))):
        for y, h in hlines(box, seed):
            if name == "panel" and changes["box"][1] - 2 <= y <= changes["box"][1] + changes["box"][3] + 2: continue
            x, w = box[0] + 1, box[2] - 2
            out.append((0, f'<i class="b" data-b="line" style="left:{x:g}px;top:{y:g}px;width:{w:g}px;height:{h:g}px;{colour_vars(every(lambda t: at(t, x + w / 2 + 0.3, y + 0.2)), "f")}"></i>'))

# ---------------------------------------------------------------- icons
_raster = {}
def raster(name, w, h):
    key = (name, w, h)
    if key not in _raster:
        svg = ICONS[name].replace("currentColor", "#ffffff")
        try:
            png = cairosvg.svg2png(bytestring=svg.encode(), output_width=w, output_height=h)
            _raster[key] = np.asarray(Image.open(io.BytesIO(png)).convert("RGBA"))[:, :, 3].astype(np.float32) / 255
        except Exception:
            _raster[key] = np.zeros((h, w), np.float32)
    return _raster[key]

ARTWORK = {name: open(f"../artwork/{name}.svg").read() for name in ("api", "web", "uptime", "docs")}
def artwork(box):
    """The project whose artwork fills this tile, by its ground colour; None for a glyph."""
    c = crop(P, box).reshape(-1, 3)
    vals, counts = np.unique(c, axis=0, return_counts=True)
    ground = vals[counts.argmax()]
    if np.ptp(ground) < 40: return None
    best = min(ARTWORK, key=lambda name: np.abs(np.array([int(re.search(r'fill="#([0-9a-f]{6})"', ARTWORK[name]).group(1)[i:i + 2], 16) for i in (0, 2, 4)]) - ground).sum())
    return best

def match_icon(box):
    c = crop(P, box).astype(np.float32)
    ground, ink = ink_of(P, box)
    span = np.abs(ink - ground).sum() or 1
    shape = np.clip(np.abs(c - ground).sum(axis=2) / span, 0, 1)
    best = (-1, None)
    for name in ICONS:
        r = raster(name, shape.shape[1], shape.shape[0])
        if r.shape != shape.shape or r.sum() == 0: continue
        score = float((r * shape).sum() / (np.sqrt((r * r).sum() * (shape * shape).sum()) + 1e-6))
        if score > best[0]: best = (score, name)
    return best

for n in nodes:
    if n["kind"] != "img": continue
    x, y, w, h = n["box"]
    nx = strip_x(x, y)
    if nx is None: continue
    project = artwork(n["box"])
    if project:
        nx, y = placed(n["n"], nx, y, w, h)
        uri = "data:image/svg+xml," + urllib.parse.quote(ARTWORK[project])
        out.append((1, f'<img alt="" src="{uri}" style="left:{nx:g}px;top:{y:g}px;width:{w:g}px;height:{h:g}px;border-radius:{4 if w < 20 else 4.5:g}px">'))
        print(f"artwork @{n['n']} {x},{y} {w}x{h} -> {project}", file=sys.stderr)
        continue
    score, name = match_icon(n["box"])
    inks = every(lambda t: ink_of(t, n["box"])[1])
    print(f"icon @{n['n']:5} {x:6},{y:5} {w}x{h} -> {name} ({score:.2f}) {hexc(inks[0])}", file=sys.stderr)
    if score < 0.55: continue
    css_icons[name] = ICONS[name]
    nx, y = placed(n["n"], nx, y, w, h)
    out.append((1, f'<i class="i i-{name}" style="left:{nx:g}px;top:{y:g}px;width:{w:g}px;height:{h:g}px;{colour_vars(inks)}"></i>'))

# ---------------------------------------------------------------- text
# Each theme's primary text colour (a label that wears it), and the opacities its other tones are.
PRIMARY = {t: ink_of(t, box_of("text", "New chat"))[1] for t in THEMES}
ALPHAS = {t: (1, 0.6, 0.42, 0.28) for t in THEMES}

def text_colour(theme, box):
    """A node's ink: one of the theme's four text tones where it is grey, as sampled where it has a hue."""
    ground, ink = ink_of(theme, box)
    d = (ink - ground).astype(float)
    full = (PRIMARY[theme] - ground).astype(float)
    if np.ptp(d / np.where(np.abs(full) < 1, 1, full)) < 0.12 and np.abs(full).sum() > 30:
        est = float((d / np.where(np.abs(full) < 1, 1, full)).mean())
        alpha = min((a for a in ALPHAS[theme] if a >= est - 0.06), default=1)
        return ground + full * alpha
    return ink

from PIL import ImageFont
_fonts = {}
def measure(text, fam, weight, size):
    name = {("G", 400): "Geist.ttf", ("G", 500): "Geist-Medium.ttf", ("G", 600): "Geist-SemiBold.ttf", ("M", 400): "GeistMono.ttf", ("M", 500): "GeistMono-Medium.ttf"}[(fam, weight)]
    key = (name, size)
    if key not in _fonts: _fonts[key] = ImageFont.truetype(f"{ROOT}/apps/zgx/fonts/{name}", size * 8)
    return _fonts[key].getlength(text) / 8
NATURAL = {14: 10, 16: 11, 17: 12, 19: 13, 20: 14, 21: 15, 22: 15, 24: 17}
FORCE = {"+3": ("M", 400, 11), "+9": ("M", 400, 11), "−0": ("M", 400, 11), "−1": ("M", 400, 11), "+12": ("M", 400, 11),
         "Changes": ("G", 500, 12), "M": ("M", 500, 12), "0/50": ("G", 400, 10)}
COLUMN = next(n["box"][2] for n in tree if n.get("role") == "article" and n.get("text") == "Reply")  # the reply's column
RICH = {
    "Clients can send": ("bubble", "Clients can send about twice the limit if they time it around a window boundary. Find out why and fix it."),
    "Found it.": ("column", "Found it. <code>estimate()</code> weights the previous window by <code>Math.floor(1 - into)</code>, and that is <code>0</code> the moment a new window starts. One millisecond after the boundary the last window's requests stop counting, so a client that fills a window right before it ends can fill the next one right after."),
    "Fixed in": ("column", "Fixed in <code>src/window.ts</code>: the previous window now counts for the share of it that still overlaps, so the limit holds across a boundary."),
    "test/window.test.ts": ("item", "<code>test/window.test.ts</code> fills a window in its last 10 ms, then checks that only one more request gets through after it rolls"),
    "This matches": ("column", "This matches the October incident: a client at the limit got about twice its rate for as long as it kept landing on the boundary. Nothing is committed yet."),
}
CODE_X = 886
ADVANCE = 7.2  # Geist Mono at 12: 0.6 em

def code_line(n, px, py):
    """A line of code: its characters grouped into runs of one ink (the app's syntax colours), sampled cell by cell."""
    x, y, w, h = n["box"]
    text = n["text"]
    def ink(theme, cx):
        cell = crop(theme, [cx, y + 1, ADVANCE, h - 2]).reshape(-1, 3)
        ground = at(theme, cx + 0.1, y + 0.6)
        return ground, cell[np.abs(cell - ground).sum(axis=1).argmax()]
    def kind(c, g):
        """A hue, or for greys how strong: text 0.9, operators 0.66, punctuation and comments about 0.5."""
        v = (c - g).astype(float)
        if np.ptp(v) > 14: return tuple(np.round(v / (np.abs(v).max() or 1) * 5).astype(int))
        strength = float(v.mean()) / float((PRIMARY[P] - g).mean())
        return "text" if strength > 0.76 else "operator" if strength > 0.57 else "quiet"
    spans, gap = [], ""
    for i, ch in enumerate(text):
        cx = x + i * ADVANCE
        if cx + ADVANCE > 1305 + ADVANCE: break
        if ch == " ":
            gap += ch
            continue
        seen = every(lambda t: ink(t, cx))  # (ground, ink) a theme
        k = kind(seen[0][1], seen[0][0])
        if spans and spans[-1]["k"] == k and "`" not in ch:
            s = spans[-1]
            s["t"] += gap + ch
            # (A thin glyph's best pixel is dimmed by coverage: each theme keeps the strongest ink the run shows.)
            s["seen"] = [new if np.abs(new[1] - new[0]).sum() > np.abs(old[1] - old[0]).sum() else old for new, old in zip(seen, s["seen"])]
        else:
            spans.append({"t": gap + ch, "k": k, "seen": seen})
        gap = ""
    body = "".join(f'<b style="{colour_vars([ink_ for _, ink_ in s["seen"]])}">{html.escape(s["t"])}</b>' for s in spans)
    return f'<span class="t m" style="left:{px:g}px;top:{py:g}px;height:{h}px;line-height:{h}px;font-size:12px;width:{min(w, 1305 - x):g}px;white-space:pre;overflow:hidden">{body}</span>'

for n in nodes:
    if n["kind"] != "text": continue
    x, y, w, h = n["box"]
    nx = strip_x(x, y)
    if nx is None: continue
    text = n["text"]
    inks = every(lambda t: text_colour(t, n["box"]))
    mac = {"Ctrl+N": "⌘N", "Ctrl+K": "⌘K"}.get(text)
    rich = next((k for k in RICH if text.startswith(k) and h > 24), None)
    if rich:
        kind_, body = RICH[rich]
        # As wide as the app set it: the message in its bubble, a list item a marker in from the column, the rest the column.
        width = {"bubble": w + 2, "item": COLUMN - 24, "column": COLUMN}[kind_]
        x, y = placed(n["n"], x, y, w, h)
        out.append((2, f'<p class="t p" style="left:{x:g}px;top:{y:g}px;width:{width:g}px;{colour_vars(inks)}">{body}</p>'))
        continue
    if PANEL and x >= CODE_X and 290 < y < 520 and h == 17:
        out.append((2, code_line(n, *placed(n["n"], x, y, min(w, 1305 - x), h))))
        continue
    err, fam, weight, size, measured = fits[str(n["n"])][0]
    align = "left"
    if text in FORCE:
        fam, weight, size = FORCE[text]
    if err > 2 or (text in FORCE and abs(measured - w) > 2) or len(text) <= 2:
        # A box of a set width: the size its line height gives, the text where its ink lies.
        size = NATURAL.get(h, size)
        first, last = ink_span(P, n["box"])
        if h not in NATURAL or len(text) > 8:
            size = min(range(10, 18), key=lambda s: abs(measure(text, "G", 400, s) - (last - first) - 1))
        if text not in FORCE:
            fam, weight = ("M", 400) if re.fullmatch(r"[\d+\-−]+", text) else ("G", 600 if re.fullmatch(r"[A-Z]", text) else 400)
        first, last = ink_span(P, n["box"])
        if w - (last - first) > 3:
            align = "left" if first < 2.5 else "right" if w - last < 2.5 else "center"
    nx, y = placed(n["n"], nx, y, w, h)
    fixed = err > 2
    first, last = ink_span(P, n["box"], gap=7)
    if align == "left" and not fixed and fam == "G" and (x < 280 or y < STRIP) and measured - last > 5 and len(text) > 12:
        # (FadeText: the row ends before the words do.)
        w = round(last + 1)
        style_fade = f"overflow:hidden;-webkit-mask-image:linear-gradient(90deg,#000 {w - 20}px,transparent {w}px);mask-image:linear-gradient(90deg,#000 {w - 20}px,transparent {w}px);"
    else:
        style_fade = ""
    if mac: text, align = mac, "right"
    style = style_fade + f"left:{nx:g}px;top:{y:g}px;width:{w:g}px;height:{h}px;line-height:{h}px;font-size:{size}px;font-weight:{weight};text-align:{align};{colour_vars(inks)}"
    out.append((2, f'<span class="t{" m" if fam == "M" else ""}" style="{style}">{html.escape(text)}</span>'))

# ---------------------------------------------------------------- the sidebar's branches, and work under way
headers = [n for n in tree if n.get("role") == "button" and n["n"] not in _hidden and n["box"][0] == 13 and n["box"][2] == 256 and n["box"][3] == 30 and n["box"][1] > 170]
groups, current = [], None
for n in headers:
    # (A project's row holds an 18-point tile; a session's a 14-point mark.)
    i = tree.index(n)
    tile = next((m for m in tree[i + 1:i + 6] if m.get("role") == "img"), None)
    if tile and tile["box"][2] == 18:
        current = []; groups.append(current)
    elif current is not None:
        current.append(n["box"][1] + 15)
def alpha_ink(theme, x, y, gx, gy):
    """A hairline's ink as white or black at an opacity, from one pixel of it and one of its ground."""
    line, ground = at(theme, x, y).astype(float), at(theme, gx, gy).astype(float)
    target = 255.0 if line.mean() > ground.mean() else 0.0
    return f"rgb({int(target)} {int(target)} {int(target)} / {abs(line.mean() - ground.mean()) / max(abs(target - ground.mean()), 1):.3f})"
paths = []
for centres in groups:
    if not centres: continue
    top = centres[0] - 15
    paths.append(f"M28 {top}V{centres[-1] - 5}" + "".join(f"M28 {c - 5}q0 5 5 5h5" for c in centres))
if paths:
    probe = groups[0][0] + 20 if len(groups[0]) > 1 else groups[0][0] - 12
    inks = every(lambda t: alpha_ink(t, 28, probe, 32, probe))
    out.append((1, f'<svg class="branches" viewBox="0 0 277 840" style="left:0;top:0;width:277px;height:840px;{colour_vars(inks)}"><path d="{" ".join(paths)}"/></svg>'))
    print("branches", len(paths), inks, file=sys.stderr)
for n in tree:
    if n.get("role") == "group" and n.get("text") == "Working" and n["n"] not in _hidden and n["box"][2] == 16:
        x, y, w, h = n["box"]
        # (The theme's accent: #9277b0 dark, #7a55c4 light.)
        out.append((1, f'<svg class="working" viewBox="0 0 16 16" style="left:{x}px;top:{y}px;width:16px;height:16px;{colour_vars(every(lambda t: ink_of(t, n["box"])[1]))}"><circle cx="8" cy="8" r="5.7" pathLength="80"/></svg>'))

# ---------------------------------------------------------------- avatars
import base64
PALETTE = {hue: dict(re.findall(r'\["(\d+)"\] = "(#[0-9a-f]{6})"', body)) for hue, body in re.findall(r"^\t(\w+) = \{(.*)\},?$", open(f"{ROOT}/apps/zgx/ui/ds/palette.luau").read(), re.M)}
WHEEL = ["red", "orange", "amber", "yellow", "lime", "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia", "pink", "rose"]

def avatar(seed, size, scale=3):
    """ds/avatar.luau's waves for a seed, as a picture."""
    state = 2166136261
    for ch in seed.encode():
        state ^= ch
        state = ((state << 24) + state * 403) % 4294967296
    def nxt():
        nonlocal state
        state = (state * 1664525 + 1013904223) % 4294967296
        return state / 4294967296
    base = int(nxt() * len(WHEEL))
    cols = [np.array([int(PALETTE[WHEEL[(base + off) % len(WHEEL)]][step][i:i + 2], 16) for i in (1, 3, 5)], float) for off, step in ((0, "300"), (1, "500"), (-1, "600"))]
    P = [nxt(), nxt(), nxt(), nxt()]
    n = size * scale
    py, px = (np.mgrid[0:n, 0:n] + 0.5) / scale
    ux, uy = px / size - 0.5, py / size - 0.5
    tau = 2 * np.pi
    ang = P[0] * tau
    rx, ry = np.cos(ang) * ux - np.sin(ang) * uy, np.sin(ang) * ux + np.cos(ang) * uy
    k = 2.2 + P[1] * 2.4
    sm = lambda lo, hi, v: (lambda t: t * t * (3 - 2 * t))(np.clip((v - lo) / (hi - lo), 0, 1))
    t1 = 0.5 + 0.5 * np.sin(rx * k + P[2] * tau + 1.6 * np.sin(ry * k * 1.3 + P[3] * tau))
    t2 = 0.5 + 0.5 * np.sin(ry * k * 0.9 - P[3] * tau + 1.3 * np.sin(rx * k * 1.1 + P[2] * tau))
    m1, m2 = sm(0.15, 0.85, t1)[..., None], (0.65 * sm(0.3, 0.95, t2))[..., None]
    v = cols[0] + (cols[1] - cols[0]) * m1
    rgb = v + (cols[2] - v) * m2
    d = np.hypot(px - size / 2, py - size / 2) - size / 2
    alpha = np.clip(0.5 - d * scale, 0, 1)[..., None] * 255
    buf = io.BytesIO()
    Image.fromarray(np.concatenate([rgb, alpha], axis=2).round().astype(np.uint8), "RGBA").save(buf, "PNG", optimize=True)
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode(), rgb.reshape(-1, 3).mean(axis=0)

for box, seeds in [(box_of("group", "Local"), ["Local"])] + ([(box_of("group", "Mara Okafor"), ["git-author-Mara Okafor"])] if PANEL else []):
    x, y, w, h = box
    seen = crop(P, [x + w * 0.25, y + h * 0.25, w * 0.5, h * 0.5]).reshape(-1, 3).mean(axis=0)
    uri, _ = avatar(seeds[0], w)
    out.append((2, f'<img alt="" src="{uri}" style="left:{x:g}px;top:{y:g}px;width:{w:g}px;height:{h:g}px">'))
    print("avatar", box, [f"{s}: {float(np.abs(avatar(s, w)[1] - seen).sum()):.0f}" for s in seeds], file=sys.stderr)

# The Commit button's word (the tree names the button, not its text).
if PANEL:
    cx, cy, cw, ch = box_of("button", "Commit")
    inks = every(lambda t: ink_of(t, [cx + 6, cy + 4, cw - 12, ch - 8])[1])
    out.append((2, f'<span class="t" style="left:{cx:g}px;top:{cy:g}px;width:{cw:g}px;height:{ch}px;line-height:{ch}px;font-size:12px;font-weight:500;text-align:center;{colour_vars(inks)}">Commit</span>'))

# ---------------------------------------------------------------- bullets, lights
for item in (n for n in tree if n.get("role") == "listitem" and n["n"] not in _hidden):
    box = [item["box"][0], item["box"][1], 24, 24]  # the marker's place, before the item's words
    c = crop(P, box); ground, ink = ink_of(P, box)
    ys, xs = np.where(np.abs(c - ground).sum(axis=2) > 30)
    bx, by, bw = box[0] + xs.min() / S, box[1] + ys.min() / S, (xs.max() - xs.min() + 1) / S
    out.append((2, f'<i class="dot" style="left:{bx:g}px;top:{by:g}px;width:{bw:g}px;height:{bw:g}px;{colour_vars(every(lambda t: ink_of(t, box)[1]), "f")}"></i>'))
for i, (fill, edge) in enumerate([("#ff5f57", "#e0443e"), ("#febc2e", "#dea123"), ("#28c840", "#1aab29")]):
    out.append((3, f'<i class="tl" style="left:{14 + i * 23}px;top:13px;background:{fill};box-shadow:inset 0 0 0 .5px {edge}"></i>'))

BAR_INK = every(lambda t: ink_of(t, box_of("button", "Toggle sidebar"))[1])  # the title strip's glyphs
def side_glyph(x, y, side, opened):
    bar = 1.1667 + (2.5 if opened else 0)
    bx = 4.3333 if side == "left" else 16 - 4.3333 - bar
    svg = f'<svg viewBox="0 0 16 16" fill="none"><rect x="2.1" y="3.1" width="11.8" height="9.8" rx="2.6" stroke="currentColor" stroke-width="1.2"/><rect x="{bx:.3f}" y="5" width="{bar:.3f}" height="6" rx=".58" fill="currentColor"/></svg>'
    return f'<i class="g" style="left:{x}px;top:{y}px;width:16px;height:16px;{colour_vars(BAR_INK)}">{svg}</i>'
out.append((3, side_glyph(92, 12, "left", True)))
out.append((3, side_glyph(1320 - 30, 12, "right", PANEL)))
out.sort(key=lambda o: o[0])
masks = "\n".join(f'.zw .i-{name}{{--m:url("data:image/svg+xml,{urllib.parse.quote(re.sub(chr(10), " ", svg).replace("currentColor", "#000"))}")}}' for name, svg in sorted(css_icons.items()))
fragment = "\n".join(o[1] for o in out)
open(f"{NAME}.fragment.html", "w").write(fragment)
json.dump(layout, open("layout.json", "w"))
open(f"{NAME}.icons.css", "w").write(masks)

# ---------------------------------------------------------------- each theme's own values
def chip(theme):
    """Inline code in a theme: its wash and its ink, read from the reply's first line."""
    box = next(n["box"] for n in tree if n.get("role") == "text" and str(n.get("text", "")).startswith("Found it."))
    band = crop(theme, [box[0], box[1], box[2], 24])
    vals, counts = np.unique(band.reshape(-1, 3), axis=0, return_counts=True)
    wash = vals[counts.argsort()[::-1][1]]
    on = np.abs(band - wash).sum(axis=2) <= 4
    cols = np.where(on.sum(axis=0) >= 8)[0]
    # (The first chip alone: its columns up to the first gap wider than a glyph.)
    end = next((a for a, b in zip(cols, cols[1:]) if b - a > 14 * S), cols[-1])
    inside = band[:, cols[0]:end + 1].reshape(-1, 3)
    return wash, inside[np.abs(inside - wash).sum(axis=1).argmax()]
rules = []
for i, t in enumerate(THEMES):
    window = css_rgba(*raw_at(t, *ROOT_SEED)) if GLASS else hexc(GROUND[t])
    wash, ink = chip(t)
    rules.append(f".zw-{NAME}.t{i}{{--zw-window:{window};--zw-code:{hexc(ink)};--zw-wash:{hexc(wash)};--zw-text:{hexc(PRIMARY[t])}}}")
    print(t, rules[-1], file=sys.stderr)
open(f"{NAME}.vars.css", "w").write("\n".join(rules) + "\n")
# What a page needs to offer the themes: each one's name here, and the three colours that stand for it.
json.dump([{"id": t, "window": hexc(GROUND[t]), "accent": hexc(chip(t)[1]), "text": hexc(PRIMARY[t])} for t in THEMES], open(f"{NAME}.themes.json", "w"), indent=1)
print(len(out), "elements,", len(css_icons), "icons", file=sys.stderr)
