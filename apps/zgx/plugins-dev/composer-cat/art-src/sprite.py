#!/usr/bin/env python3
"""The composer cat's sprite, placed by hand: a 28x24 grid, one character a
pixel, frames as small changes to the idle frame. Writes art/cat-<frame>.png
(1x, palette colours, alpha 0 or 255: no smoothing anywhere).

    python3 sprite.py [preview.png]
"""
import sys
from pathlib import Path
from PIL import Image

PALETTE = {
    ".": None,
    "o": (27, 26, 31),     # outline
    "b": (74, 72, 84),     # body
    "h": (99, 96, 110),    # light on the left
    "d": (58, 56, 66),     # shade on the right
    "w": (182, 177, 194),  # muzzle, chest, paws
    "i": (125, 106, 128),  # inside the ears
    "e": (215, 226, 125),  # eyes
    "n": (224, 154, 166),  # nose
}

IDLE = [
    "............................",
    "....o........o..............",
    "...obo......obo.............",
    "...oibo....obio.............",
    "..obiiboooobiibo............",
    "..obbbbbbbbbbbbo............",
    ".ohbbbbbbbbbbbbdo...........",
    ".ohbbeobbbboebbdo...........",
    ".ohbbeobbbboebbdo...........",
    ".ohbbbwwnnwwbbbdo...........",
    "..obbbwwwwwwbbdo............",
    "...oobbbbbbbboo.............",
    "...ohbbbbbbbbdo.............",
    "..ohbbbwwwwbbbdo............",
    "..ohbbwwwwwwbbdo............",
    ".ohbbbbwwwwwwbbdo...........",
    ".ohbbbbbwwwwbbbdo...........",
    "ohbbbbbbbwwbbbbbdo..........",
    "ohbbbbbbbbbbbbbbdo..........",
    "ohbbbbbbbbbbbbbbdo..........",
    "ohbbwwwwbbwwwwbbdo..........",
    "oobbwwwwoowwwwbboo..........",
    ".oooooooooooooooo...........",
    "............................",
]

# The tail, from behind the right haunch, curling up: each frame's cells
# (x, y, colour) drawn over the body's right (behind the haunch's outline).
def tail(path):
    """A tail two pixels thick along `path` (each point and the one to its
    right), outlined, its last two points the pale tip."""
    body = []
    for (x, y) in path:
        body += [(x, y), (x + 1, y)]
    cells = {}
    for (x, y) in body:
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                cells.setdefault((x + dx, y + dy), "o")
    for (x, y) in body:
        cells[(x, y)] = "b"
    for (x, y) in path[-2:]:
        cells[(x, y)] = cells[(x + 1, y)] = "w"
    return cells

TAILS = {
    # at rest: out from the haunch and up in a curve
    "rest": [(18, 20), (19, 19), (20, 18), (21, 17), (21, 16), (22, 15), (22, 14), (22, 13), (21, 12), (21, 11), (20, 10)],
    # a flick: the tip swings out, then back over
    "out": [(18, 20), (19, 19), (20, 18), (21, 17), (22, 16), (23, 15), (24, 14), (24, 13), (25, 12), (25, 11), (24, 10)],
    "over": [(18, 20), (19, 19), (20, 18), (20, 17), (20, 16), (20, 15), (19, 14), (19, 13), (18, 12), (18, 11), (19, 10)],
}

def frame(eyes="open", tail_at="rest", ear="up"):
    g = [list(r) for r in IDLE]
    assert all(len(r) == 28 for r in g) and len(g) == 24
    # The tail behind the body: only where nothing of the body is.
    for (x, y), c in tail(TAILS[tail_at]).items():
        if 0 <= x < 28 and 0 <= y < 24 and g[y][x] == ".":
            g[y][x] = c
    if eyes == "shut":
        for y in (7, 8):
            for x in (5, 6, 11, 12):
                g[y][x] = "b"
        for x in (5, 6, 11, 12):
            g[8][x] = "o"
    if ear == "twitch":
        # the right ear flicks flat, out to the side
        for x in (12, 13, 14):
            g[1][x] = "."
        for x, c in zip(range(11, 16), ".oooo"):
            g[2][x] = c
        for x, c in zip(range(11, 17), "obibbo"):
            g[3][x] = c
    return g

FRAMES = {
    "idle": frame(),
    "blink": frame(eyes="shut"),
    "tail-out": frame(tail_at="out"),
    "tail-over": frame(tail_at="over"),
    "ear": frame(ear="twitch"),
}

def image(g):
    im = Image.new("RGBA", (28, 24), (0, 0, 0, 0))
    for y, row in enumerate(g):
        for x, c in enumerate(row):
            col = PALETTE[c]
            if col:
                im.putpixel((x, y), col + (255,))
    return im

art = Path(__file__).resolve().parent.parent / "art"
art.mkdir(exist_ok=True)
images = {name: image(g) for name, g in FRAMES.items()}
for name, im in images.items():
    im.save(art / f"cat-{name}.png")
if len(sys.argv) > 1:
    k = 10
    sheet = Image.new("RGBA", ((28 * k + 20) * len(images) + 20, 24 * k + 40), (40, 40, 46, 255))
    for i, (name, im) in enumerate(images.items()):
        sheet.paste(im.resize((28 * k, 24 * k), Image.NEAREST), (20 + i * (28 * k + 20), 20), im.resize((28 * k, 24 * k), Image.NEAREST))
    sheet.save(sys.argv[1])
