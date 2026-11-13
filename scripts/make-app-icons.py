#!/usr/bin/env python3
"""Make every platform's app icon from the art in dist/icon.

    scripts/make-app-icons.py     (needs rsvg-convert, Pillow and Xcode 26+)

dist/icon holds the two finished macOS icons (Default and Dark) and the mark
alone in ink and ivory. Where a platform can follow the system it gets both
looks; where it can't (Windows, Linux, macOS before 26) it gets Dark, which
reads on any taskbar. It also writes the logo the apps draw themselves, the
mark alone, as each one's zeron-logo.svg.

The results are committed, so packaging needs none of this. Run it when the art
changes. dist/macos/Zeron.icon/icon.json and the Android colours are written by
hand; icon.json is tuned to render like the two finished icons. Android is flat
on purpose: ink and ivory, with the mark and tile as separate layers.
"""

import io
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
ART = ROOT / "dist/icon"
DOCUMENT = ROOT / "dist/macos/Zeron.icon"
ICTOOL = "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"

INK = "#141418"
IVORY = "#F4F1EA"
MARK = re.search(r' d="([^"]+)"', (ART / "zeron-mark-solid-ink.svg").read_text()).group(1)

# The mark on its 1024 canvas is 700 wide. For a small space, a browser tab or a
# logo in an app, it's framed square with the canvas's margin taken off: every
# caller draws it in a square box, and some stretch what doesn't fit.
TIGHT_AT, TIGHT = 122, 780
LOGO = re.sub(
    r"(-?\d+\.\d+),(-?\d+\.\d+)",
    lambda m: f"{float(m[1]) - TIGHT_AT:.2f},{float(m[2]) - TIGHT_AT:.2f}",
    MARK,
)

CORNER_OVER_TILE = 185.4 / 824

# The mark's squares, by row, and where the finished icon has a square in each.
GRID = ("11101", "11011", "10111")
ROW_CENTRES = (370, 512, 655)
FIRST_SQUARE_X, MARK_LEFT, MARK_TOP, MARK_BOTTOM = 225, 162, 306, 718

# icon.json's gradients, for what's behind iOS's corners.
LIGHT_TILE = ((252, 252, 249), (233, 229, 222))
DARK_TILE = ((37, 37, 43), (9, 9, 12))


def rsvg(svg, size):
    png = subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size)],
        input=svg.encode(),
        capture_output=True,
        check=True,
    ).stdout
    return Image.open(io.BytesIO(png)).convert("RGBA")


def logo_svg(fill):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {TIGHT} {TIGHT}">'
        f'<path fill="{fill}" d="{LOGO}"/></svg>\n'
    )


def inset(icon, size, tile):
    """The finished icon at `tile` pixels, centred on a clear `size` canvas."""
    canvas = Image.new("RGBA", (size, size))
    scaled = icon.resize((tile, tile), Image.LANCZOS)
    at = (size - tile) // 2
    canvas.alpha_composite(scaled, (at, at))
    return canvas


def macos_before_26(dark):
    # Apple's grid: an 824 tile on 1024, shadow baked in. The dmg shows it too.
    shadow = rsvg(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
        '<filter id="s" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="14"/></filter>'
        f'<rect x="100" y="112" width="824" height="824" rx="{824 * CORNER_OVER_TILE}" '
        'fill="#000" fill-opacity=".35" filter="url(#s)"/></svg>',
        1024,
    )
    shadow.alpha_composite(inset(dark, 1024, 824))
    return shadow


def snapped(dark, size, tile, cell):
    """A frame too small for the art: the same tile, with its squares redrawn
    on whole pixels. Scaled down, the 1 pixel gaps between them go grey."""
    frame = inset(dark, size, tile)
    at = (size - tile) // 2
    scale = tile / dark.width
    # Clear the mark from the tile, keeping the tile's gradient behind it.
    left = max(at + int(MARK_LEFT * scale) - 2, 1)
    for y in range(at + int(MARK_TOP * scale) - 1, at + int(MARK_BOTTOM * scale) + 2):
        colour = frame.getpixel((left, y))
        for x in range(left, size - left):
            frame.putpixel((x, y), colour)
    pitch = cell + 1
    x0 = (size - (len(GRID[0]) * pitch - 1)) // 2
    y0 = (size - (len(GRID) * pitch - 1)) // 2
    squares = "".join(
        f'<rect x="{x0 + col * pitch}" y="{y0 + r * pitch}" width="{cell}" height="{cell}" rx="{cell * 0.25}" '
        f'fill="#{"%02x%02x%02x" % dark.getpixel((FIRST_SQUARE_X, ROW_CENTRES[r]))[:3]}"/>'
        for r, row in enumerate(GRID)
        for col, on in enumerate(row)
        if on == "1"
    )
    frame.alpha_composite(
        rsvg(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}">{squares}</svg>', size)
    )
    return frame


def windows_ico(dark, path):
    # Size → the square's side in whole pixels; larger frames scale the art.
    small = {16: 2, 24: 3, 32: 4, 48: 6}
    frames = []
    for size in (16, 24, 32, 48, 64, 128, 256):
        tile = size - 2 * round(size / 32)
        frames.append(snapped(dark, size, tile, small[size]) if size in small else inset(dark, size, tile))
    frames[-1].save(
        path,
        format="ICO",
        sizes=[f.size for f in frames],
        append_images=frames[:-1],
        bitmap_format="png",
    )


def ios_render(rendition, tile, work):
    """iOS draws its own corners, so its icons are square and opaque: Apple's
    render, with the tile's gradient carried out behind its rounded corners."""
    out = Path(work) / f"{rendition}.png"
    subprocess.run(
        [ICTOOL, str(DOCUMENT), "--export-image", "--output-file", str(out), "--platform", "iOS",
         "--rendition", rendition, "--width", "1024", "--height", "1024", "--scale", "1"],
        capture_output=True,
        check=True,
    )
    top, bottom = tile
    ramp = Image.new("RGBA", (1, 1024))
    for y in range(1024):
        ramp.putpixel((0, y), tuple(round(a + (b - a) * y / 1023) for a, b in zip(top, bottom)) + (255,))
    square = ramp.resize((1024, 1024))
    square.alpha_composite(Image.open(out).convert("RGBA"))
    # The render's edge has a light rim baked in, and iOS draws its own. A little
    # larger puts this one outside the system's mask.
    larger = square.resize((1075, 1075), Image.LANCZOS)
    return larger.crop((25, 25, 1049, 1049)).convert("RGB")


def adaptive_favicon():
    """A tab icon that follows the system: ink on a light tab, ivory on a dark one."""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {TIGHT} {TIGHT}">\n'
        "<style>\n"
        f".mark{{fill:{INK}}}\n"
        f"@media (prefers-color-scheme:dark){{.mark{{fill:{IVORY}}}}}\n"
        "</style>\n"
        f'<path class="mark" d="{LOGO}"/>\n'
        "</svg>\n"
    )


def android_foreground():
    """The adaptive icon's foreground, flat, its colour from values/ and
    values-night/. The mark takes the same share of the 72dp a launcher shows as
    of the 1024 canvas, and so stays well inside the 66dp safe circle."""
    scale = round(72 / 1024, 5)
    at = round((108 - 1024 * scale) / 2, 4)
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        "<!-- Written by scripts/make-app-icons.py. -->\n"
        '<vector xmlns:android="http://schemas.android.com/apk/res/android"\n'
        '    android:width="108dp"\n'
        '    android:height="108dp"\n'
        '    android:viewportWidth="108"\n'
        '    android:viewportHeight="108">\n'
        f'    <group android:translateX="{at}" android:translateY="{at}" android:scaleX="{scale}" android:scaleY="{scale}">\n'
        "        <path\n"
        '            android:fillColor="@color/icon_foreground"\n'
        f'            android:pathData="{MARK}" />\n'
        "    </group>\n"
        "</vector>\n"
    )


def main():
    dark = Image.open(ART / "zeron-solid-macos-Dark.png").convert("RGBA")

    # macOS.
    (DOCUMENT / "Assets").mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ART / "zeron-mark-solid-ink.svg", DOCUMENT / "Assets/mark.svg")
    macos_before_26(dark).save(ROOT / "dist/macos/icon-1024.png")

    # Linux and Windows.
    inset(dark, 1024, 896).save(ROOT / "dist/zeron.png")
    windows_ico(dark, ROOT / "dist/windows/zeron.ico")
    (ROOT / "dist/zeron.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024" fill="none">\n'
        f'<rect x="100" y="100" width="824" height="824" rx="{824 * CORNER_OVER_TILE}" fill="{INK}"/>\n'
        f'<path fill="{IVORY}" transform="translate(512 512) scale(.8047) translate(-512 -512)" d="{MARK}"/>\n'
        "</svg>\n"
    )

    # iOS. Tinted is grayscale on black, which the system colours itself.
    ios = ROOT / "apps/ios/Zeron/Assets.xcassets/AppIcon.appiconset"
    with tempfile.TemporaryDirectory() as work:
        light_icon = ios_render("Default", LIGHT_TILE, work)
        dark_icon = ios_render("Dark", DARK_TILE, work)
    light_icon.save(ios / "AppIcon1024.png")
    dark_icon.save(ios / "AppIcon1024-dark.png")
    tinted = rsvg(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
        f'<rect width="1024" height="1024" fill="#000"/><path fill="#fff" d="{MARK}"/></svg>',
        1024,
    )
    tinted.convert("RGB").save(ios / "AppIcon1024-tinted.png")

    # Android.
    (ROOT / "apps/android/app/src/main/res/drawable/ic_launcher_foreground.xml").write_text(android_foreground())

    # The website. Tabs take favicon.svg, and the PNGs are for browsers that
    # can't. Its pages are dark whatever the system's look, so their logos stay
    # ivory: ink would vanish. The home-screen icon can't follow the system.
    site = ROOT / "apps/landing/public"
    (site / "favicon.svg").write_text(adaptive_favicon())
    (site / "assets/icons/zeron-mark-solid-ivory.svg").write_text(logo_svg(IVORY))
    rsvg(logo_svg(INK), 128).save(site / "assets/zeron-favicon-light.png")
    rsvg(logo_svg(IVORY), 128).save(site / "assets/zeron-favicon-dark.png")
    dark_icon.resize((180, 180), Image.LANCZOS).save(site / "assets/zeron-app-icon.png")

    # The logo the apps draw themselves. Desktop and zgx tint it, iOS and Android
    # take its shape only. After changing it, run apps/zgx/tools/icon_svgs.py.
    (ROOT / "crates/ui/assets/icons/zeron-logo.svg").write_text(logo_svg("currentColor"))
    (ROOT / "apps/zgx/ui/assets/icons/ui/zeron-logo.svg").write_text(logo_svg("currentColor"))
    (ROOT / "apps/ios/Zeron/Assets.xcassets/zeron-logo.imageset/zeron-logo.svg").write_text(logo_svg("#000000"))
    (ROOT / "apps/android/icons/zeron.svg").write_text(logo_svg("#000000"))

    # zgx's sample project pictures: Zeron's, and the iOS app's.
    favicons = ROOT / "apps/zgx/ui/assets/favicons"
    inset(dark, 64, 60).save(favicons / "zeron.png")
    light_icon.resize((64, 64), Image.LANCZOS).save(favicons / "ios.png")


if __name__ == "__main__":
    main()
