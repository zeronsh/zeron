#!/usr/bin/env python3
"""Zeron Preview's icon: Zeron's (dist/macos/icon-1024.png) with an amber
PREVIEW band across its foot, so the two apps never look alike in the Dock,
Finder or Launchpad. Writes dist/macos/icon-preview-1024.png, which
scripts/package-zgx-preview.sh turns into the bundle's icns (committed, as
the dmg backgrounds are: CI needs no Pillow).

    python3 -m pip install --user pillow   # once
    python3 scripts/preview-icon.py
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "dist/macos/icon-1024.png"
OUT = ROOT / "dist/macos/icon-preview-1024.png"
FONT = ROOT / "apps/zgx/fonts/Geist-Bold.ttf"

BAND = (255, 176, 32, 255)  # amber, far from the icon's violet
INK = (28, 18, 4, 255)
LABEL = "PREVIEW"


def main() -> None:
    icon = Image.open(SOURCE).convert("RGBA")
    width, height = icon.size
    alpha = icon.getchannel("A")
    # The squircle itself (the baked shadow is only partly opaque).
    body = alpha.point(lambda a: 255 if a >= 250 else 0)
    left, top, right, bottom = body.getbbox()
    side = bottom - top

    # A band over the bottom fifth of the squircle, clipped to it.
    band_top = bottom - round(side * 0.22)
    band = Image.new("RGBA", icon.size, (0, 0, 0, 0))
    ImageDraw.Draw(band).rectangle((left, band_top, right, bottom), fill=BAND)
    band.putalpha(ImageChops.multiply(band.getchannel("A"), body))
    # A hairline of shade above it, so it reads as a band, not a cut.
    shade = Image.new("RGBA", icon.size, (0, 0, 0, 0))
    ImageDraw.Draw(shade).rectangle((left, band_top - 6, right, band_top), fill=(0, 0, 0, 90))
    shade.putalpha(ImageChops.multiply(shade.getchannel("A"), body))

    out = Image.alpha_composite(icon, shade)
    out = Image.alpha_composite(out, band)

    draw = ImageDraw.Draw(out)
    size = round(side * 0.13)
    font = ImageFont.truetype(str(FONT), size)
    # Letter-spaced, centred in the band.
    tracking = round(size * 0.12)
    widths = [draw.textlength(ch, font=font) for ch in LABEL]
    total = sum(widths) + tracking * (len(LABEL) - 1)
    ascent, descent = font.getmetrics()
    x = (left + right) / 2 - total / 2
    cap = font.getbbox("P")
    y = (band_top + bottom) / 2 - (cap[1] + cap[3]) / 2
    for ch, w in zip(LABEL, widths):
        draw.text((x, y), ch, font=font, fill=INK)
        x += w + tracking

    out.save(OUT, optimize=True)
    print(f"wrote {OUT.relative_to(ROOT)} ({width}x{height})")


if __name__ == "__main__":
    main()
