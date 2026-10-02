#!/usr/bin/env python3
"""Convert Zeron's line icons (desktop crates/ui/assets/icons, iOS tab and
tool icons) into Android VectorDrawables, so every platform draws the same
glyphs. Handles the subset those icons use: <g>/<path>/<circle>/<rect>,
inherited fill/stroke attributes, caps, joins and even-odd fills.

    scripts/android/svg2vd.py <out_res_dir>
"""
import glob
import os
import re
import sys
import xml.etree.ElementTree as ET

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
SOURCES = [
    ("", os.path.join(ROOT, "crates/ui/assets/icons/*.svg")),
    ("", os.path.join(ROOT, "apps/ios/Zeron/Assets.xcassets/TabIcons/*/*.svg")),
    ("", os.path.join(ROOT, "apps/ios/Zeron/Assets.xcassets/ToolIcons/*/*.svg")),
]
# Brand marks are drawn from path data at runtime (their own colors).
SKIP = re.compile(r"-mark$|^zeron-logo$")
INHERITED = ("fill", "stroke", "stroke-width", "stroke-linecap", "stroke-linejoin",
             "fill-rule", "fill-opacity", "stroke-opacity", "opacity")


def tag(el):
    return el.tag.split("}")[-1]


def color(v):
    if v is None or v == "none":
        return None
    # Everything is a single-ink glyph tinted by the caller.
    return "#FF000000"


def num(v, default=0.0):
    try:
        return float(re.sub(r"[a-z%]+$", "", v)) if v is not None else default
    except ValueError:
        return default


def f(x):
    return ("%.4f" % x).rstrip("0").rstrip(".")


def circle_path(cx, cy, r):
    return f"M{f(cx - r)},{f(cy)}a{f(r)},{f(r)} 0 1,0 {f(2 * r)},0a{f(r)},{f(r)} 0 1,0 {f(-2 * r)},0Z"


def rect_path(x, y, w, h, rx, ry):
    if rx <= 0 and ry <= 0:
        return f"M{f(x)},{f(y)}h{f(w)}v{f(h)}h{f(-w)}Z"
    rx = min(rx or ry, w / 2)
    ry = min(ry or rx, h / 2)
    return (f"M{f(x + rx)},{f(y)}h{f(w - 2 * rx)}a{f(rx)},{f(ry)} 0 0 1 {f(rx)},{f(ry)}"
            f"v{f(h - 2 * ry)}a{f(rx)},{f(ry)} 0 0 1 {f(-rx)},{f(ry)}h{f(-(w - 2 * rx))}"
            f"a{f(rx)},{f(ry)} 0 0 1 {f(-rx)},{f(-ry)}v{f(-(h - 2 * ry))}a{f(rx)},{f(ry)} 0 0 1 {f(rx)},{f(-ry)}Z")


def style_attrs(el):
    out = {k: el.get(k) for k in INHERITED if el.get(k) is not None}
    for decl in (el.get("style") or "").split(";"):
        if ":" in decl:
            k, v = decl.split(":", 1)
            if k.strip() in INHERITED:
                out[k.strip()] = v.strip()
    return out


def walk(el, inherited, paths):
    attrs = dict(inherited)
    attrs.update(style_attrs(el))
    t = tag(el)
    d = None
    if t == "path":
        d = el.get("d")
    elif t == "circle":
        d = circle_path(num(el.get("cx")), num(el.get("cy")), num(el.get("r")))
    elif t == "rect":
        d = rect_path(num(el.get("x")), num(el.get("y")), num(el.get("width")), num(el.get("height")),
                      num(el.get("rx")), num(el.get("ry")))
    if d:
        paths.append((d, attrs))
    if t in ("svg", "g"):
        for child in el:
            walk(child, attrs, paths)


def convert(svg_path):
    root = ET.parse(svg_path).getroot()
    vb = [float(v) for v in re.split(r"[ ,]+", (root.get("viewBox") or "0 0 24 24").strip())]
    paths = []
    walk(root, {"fill": "#000"}, paths)
    lines = [
        '<vector xmlns:android="http://schemas.android.com/apk/res/android"',
        '    android:width="24dp" android:height="24dp"',
        f'    android:viewportWidth="{f(vb[2])}" android:viewportHeight="{f(vb[3])}">',
    ]
    if vb[0] or vb[1]:
        lines.append(f'  <group android:translateX="{f(-vb[0])}" android:translateY="{f(-vb[1])}">')
    for d, a in paths:
        attrs = [f'android:pathData="{d}"']
        fill = color(a.get("fill"))
        stroke = color(a.get("stroke"))
        if fill:
            attrs.append(f'android:fillColor="{fill}"')
            alpha = num(a.get("fill-opacity"), 1.0) * num(a.get("opacity"), 1.0)
            if alpha < 1:
                attrs.append(f'android:fillAlpha="{f(alpha)}"')
            if a.get("fill-rule") == "evenodd":
                attrs.append('android:fillType="evenOdd"')
        if stroke:
            attrs.append(f'android:strokeColor="{stroke}"')
            attrs.append(f'android:strokeWidth="{f(num(a.get("stroke-width"), 1.0))}"')
            cap = a.get("stroke-linecap")
            if cap in ("round", "square", "butt"):
                attrs.append(f'android:strokeLineCap="{cap}"')
            join = a.get("stroke-linejoin")
            if join in ("round", "bevel", "miter"):
                attrs.append(f'android:strokeLineJoin="{join}"')
            alpha = num(a.get("stroke-opacity"), 1.0) * num(a.get("opacity"), 1.0)
            if alpha < 1:
                attrs.append(f'android:strokeAlpha="{f(alpha)}"')
        if not fill and not stroke:
            continue
        lines.append("  <path " + "\n      ".join(attrs) + " />")
    if vb[0] or vb[1]:
        lines.append("  </group>")
    lines.append("</vector>")
    return "\n".join(lines) + "\n"


def main():
    out = os.path.join(sys.argv[1], "drawable")
    os.makedirs(out, exist_ok=True)
    count = 0
    for _, pattern in SOURCES:
        for svg in sorted(glob.glob(pattern)):
            name = os.path.splitext(os.path.basename(svg))[0]
            if SKIP.search(name):
                continue
            res = "zi_" + re.sub(r"[^a-z0-9]+", "_", name.lower())
            with open(os.path.join(out, res + ".xml"), "w") as fh:
                fh.write(convert(svg))
            count += 1
    print(f"vector icons: {count}")


if __name__ == "__main__":
    main()
