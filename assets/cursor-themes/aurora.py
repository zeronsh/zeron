#!/usr/bin/env python3
"""Build the Zeron "Aurora" agent cursor theme.

Aurora is a soft glass pointer: a translucent aurora-teal body with a bright
inner highlight, a crisp white rim, and a wide bloom that shifts violet to cyan
across its glow layers. Every action keeps the pointer and its semantic mark in
one shared floating transform, exactly like the built-in `cua.default` theme,
so the two read as a single object.

The theme obeys profile v2: static path geometry, animation only on the layer
transform (position, scale, rotation, opacity), 128x128 at 30 fps, at most 120
frames per action, 1,000 frames across the theme.

Usage:
    python3 aurora.py --output aurora.lottie
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Sequence
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

CANVAS = 128
FPS = 30

THEME_ID = "sh.zeron.cursor.aurora"
THEME_NAME = "Aurora"
THEME_VERSION = "1.0.0"
THEME_AUTHOR = "Zeron"
THEME_LICENSE = "MIT"

# The primary body color. The overlay tints the pointer per session for the
# built-in theme; a custom theme keeps its authored colors, which is why the
# authoring colors here have to read well on light and dark backgrounds alike.
AURORA = [0.243, 0.855, 0.784, 1.0]  # #3EDAA8-ish teal
AURORA_DEEP = [0.106, 0.478, 0.612, 1.0]  # #1B7A9C deep teal
VIOLET = [0.561, 0.365, 0.941, 1.0]  # #8F5DF0
CYAN = [0.302, 0.812, 0.949, 1.0]  # #4DCFF2
WHITE = [1.0, 1.0, 1.0, 1.0]
TRANSPARENT = [0.0, 0.0, 0.0, 0.0]

Point = tuple[float, float]
PathPoint = tuple[Point, Point, Point]


def static(value: Any) -> dict[str, Any]:
    return {"a": 0, "k": value}


def _value(value: Any) -> list[float]:
    if isinstance(value, (int, float)):
        return [value]
    return list(value)


def animated(values: Sequence[tuple[int, Any]], *, linear: bool = False) -> dict[str, Any]:
    frames: list[dict[str, Any]] = []
    for index, (frame, value) in enumerate(values):
        item: dict[str, Any] = {"t": frame, "s": _value(value)}
        if index + 1 < len(values):
            item["e"] = _value(values[index + 1][1])
            if linear:
                item["i"] = {"x": [0.833], "y": [0.833]}
                item["o"] = {"x": [0.167], "y": [0.167]}
            else:
                item["i"] = {"x": [0.25], "y": [1.0]}
                item["o"] = {"x": [0.2], "y": [0.75]}
        frames.append(item)
    return {"a": 1, "k": frames}


def path_shape(
    points: Sequence[PathPoint], *, closed: bool = False, name: str = "Path"
) -> dict[str, Any]:
    return {
        "ty": "sh",
        "nm": name,
        "ks": {
            "a": 0,
            "k": {
                "i": [[p[1][0], p[1][1]] for p in points],
                "o": [[p[2][0], p[2][1]] for p in points],
                "v": [[p[0][0], p[0][1]] for p in points],
                "c": closed,
            },
        },
    }


def points(
    coordinates: Sequence[Point], *, closed: bool = False, name: str = "Path"
) -> dict[str, Any]:
    return path_shape(
        [((x, y), (0, 0), (0, 0)) for x, y in coordinates], closed=closed, name=name
    )


def ellipse(x: float, y: float, width: float, height: float, *, name: str) -> dict[str, Any]:
    return {"ty": "el", "nm": name, "p": static([x, y]), "s": static([width, height]), "d": 1}


def rectangle(
    x: float, y: float, width: float, height: float, *, roundness: float = 0, name: str
) -> dict[str, Any]:
    return {
        "ty": "rc",
        "nm": name,
        "p": static([x, y]),
        "s": static([width, height]),
        "r": static(roundness),
        "d": 1,
    }


def stroke(color: list[float], width: float, opacity: float = 100) -> dict[str, Any]:
    return {
        "ty": "st",
        "c": static(color),
        "o": static(opacity),
        "w": static(width),
        "lc": 2,
        "lj": 2,
        "ml": 4,
    }


def fill(color: list[float], opacity: float = 100) -> dict[str, Any]:
    return {"ty": "fl", "c": static(color), "o": static(opacity), "r": 1}


def shape_transform() -> dict[str, Any]:
    return {
        "ty": "tr",
        "p": static([0, 0]),
        "a": static([0, 0]),
        "s": static([100, 100]),
        "r": static(0),
        "o": static(100),
        "sk": static(0),
        "sa": static(0),
    }


def layer_transform(
    *,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
    rotation: dict[str, Any] | None = None,
    opacity: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "o": opacity or static(100),
        "r": rotation or static(0),
        "p": position or static([64, 64]),
        "a": static([64, 64]),
        "s": scale or static([100, 100]),
    }


def shape_layer(
    index: int,
    name: str,
    shapes: Sequence[dict[str, Any]],
    frames: int,
    *,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
    rotation: dict[str, Any] | None = None,
    opacity: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return {
        "ddd": 0,
        "ind": index,
        "ty": 4,
        "nm": name,
        "sr": 1,
        "ks": layer_transform(
            position=position, scale=scale, rotation=rotation, opacity=opacity
        ),
        "ao": 0,
        "shapes": [*shapes, shape_transform()],
        "ip": 0,
        "op": frames,
        "st": 0,
        "bm": 0,
    }


# The Aurora pointer: slightly softer shoulders than the built-in arrow, with a
# tapered tail so the bloom reads as a comet rather than a uniform outline.
CURSOR_PATH = path_shape(
    [
        ((55, 28), (0, 0), (-8, -2)),
        ((42, 40), (-1, -9), (0, 0)),
        ((61, 100), (0, 0), (3, 9)),
        ((75, 101), (-4, 7), (0, 0)),
        ((85, 80), (0, 0), (2, -4)),
        ((95, 71), (-4, 2), (0, 0)),
        ((109, 63), (0, 0), (7, -4)),
        ((108, 49), (7, 3), (0, 0)),
    ],
    closed=True,
    name="Aurora arrow",
)


# The glow ladder: wide and vivid far out, tight and bright near the body. The
# outermost rings lean violet, then indigo and cyan, so the bloom reads aurora
# rather than a single flat halo. Opacities stay high enough that the colored
# rings survive over both light and dark backgrounds.
CURSOR_GLOW = [
    (52, 4.2, VIOLET),
    (44, 5.2, VIOLET),
    (37, 6.2, [0.42, 0.58, 0.96, 1.0]),
    (30, 7.2, CYAN),
    (24, 8.2, CYAN),
    (19, 9.2, AURORA),
    (14, 10.0, AURORA),
    (10, 10.0, AURORA),
    (6, 10.0, AURORA),
]


def cursor_layers(
    frames: int,
    *,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    layers: list[dict[str, Any]] = []
    index = 1
    for width, opacity, color in CURSOR_GLOW:
        layers.append(
            shape_layer(
                index,
                f"Aurora glow {width}",
                [CURSOR_PATH, stroke(color, width, opacity)],
                frames,
                position=position,
                scale=scale,
            )
        )
        index += 1
    # The body is a glass lens: a deep teal base, an aurora tint over it, a
    # darker inner line for thickness, and a crisp white rim on the outside.
    layers.append(
        shape_layer(
            index,
            "Aurora body",
            [CURSOR_PATH, fill(AURORA_DEEP)],
            frames,
            position=position,
            scale=scale,
        )
    )
    index += 1
    layers.append(
        shape_layer(
            index,
            "Aurora tint",
            [CURSOR_PATH, fill(AURORA, 68)],
            frames,
            position=position,
            scale=scale,
        )
    )
    index += 1
    layers.append(
        shape_layer(
            index,
            "Aurora inner edge",
            [CURSOR_PATH, stroke(AURORA_DEEP, 2.2, 85)],
            frames,
            position=position,
            scale=scale,
        )
    )
    index += 1
    layers.append(
        shape_layer(
            index,
            "Aurora rim",
            [CURSOR_PATH, stroke(WHITE, 4.2)],
            frames,
            position=position,
            scale=scale,
        )
    )
    return layers


def cue_layers(
    start_index: int,
    name: str,
    geometry: Sequence[dict[str, Any]],
    frames: int,
    *,
    width: float = 4,
    position: dict[str, Any] | None = None,
    scale: dict[str, Any] | None = None,
    rotation: dict[str, Any] | None = None,
    opacity: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    layers: list[dict[str, Any]] = []
    index = start_index
    for expansion, glow_opacity in [(15, 1.8), (10, 2.6), (6, 3.6), (3, 5.0)]:
        layers.append(
            shape_layer(
                index,
                f"{name} glow {expansion}",
                [*geometry, stroke(AURORA, width + expansion, glow_opacity)],
                frames,
                position=position,
                scale=scale,
                rotation=rotation,
                opacity=opacity,
            )
        )
        index += 1
    layers.append(
        shape_layer(
            index,
            f"{name} rim",
            [*geometry, stroke(WHITE, width + 1.5)],
            frames,
            position=position,
            scale=scale,
            rotation=rotation,
            opacity=opacity,
        )
    )
    index += 1
    layers.append(
        shape_layer(
            index,
            f"{name} color",
            [*geometry, stroke(AURORA, max(width - 1, 1.5))],
            frames,
            position=position,
            scale=scale,
            rotation=rotation,
            opacity=opacity,
        )
    )
    return layers


def lottie(name: str, frames: int, layers: Sequence[dict[str, Any]]) -> dict[str, Any]:
    return {
        "v": "5.12.2",
        "fr": FPS,
        "ip": 0,
        "op": frames,
        "w": CANVAS,
        "h": CANVAS,
        "nm": name,
        "ddd": 0,
        "assets": [],
        "layers": list(reversed(layers)),
        "markers": [],
    }


def action_idle() -> dict[str, Any]:
    return lottie("action_idle", 1, cursor_layers(1))


def action_observe() -> dict[str, Any]:
    frames = 48
    geometry = [
        path_shape([((36, 26), (0, 0), (-13, 1)), ((16, 50), (0, -13), (0, 0))], name="Observe inner"),
        path_shape([((40, 16), (0, 0), (-22, 0)), ((6, 52), (0, -21), (0, 0))], name="Observe outer"),
    ]
    cue = cue_layers(
        20,
        "Observe",
        geometry,
        frames,
        position=static([72, 54]),
        scale=animated([(0, [86, 86]), (24, [100, 100]), (48, [110, 110])]),
        opacity=animated([(0, 0), (7, 100), (34, 100), (44, 0), (48, 0)]),
    )
    return lottie("action_observe", frames, [*cursor_layers(frames), *cue])


def action_click() -> dict[str, Any]:
    frames = 20
    geometry = [
        points([(34, 18), (33, 9)], name="Click upper"),
        points([(26, 24), (18, 18)], name="Click middle"),
        points([(24, 34), (14, 34)], name="Click lower"),
    ]
    cursor_scale = animated([(0, [100, 100]), (7, [92, 92]), (12, [104, 104]), (20, [100, 100])])
    cue = cue_layers(
        20,
        "Click",
        geometry,
        frames,
        position=static([74, 68]),
        scale=animated([(0, [108, 108]), (8, [132, 132]), (20, [156, 156])]),
        opacity=animated([(0, 0), (4, 100), (11, 100), (20, 0)]),
    )
    return lottie("action_click", frames, [*cursor_layers(frames, scale=cursor_scale), *cue])


def action_drag() -> dict[str, Any]:
    frames = 48
    movement = animated([(0, [64, 64]), (8, [64, 64]), (30, [72, 67]), (40, [72, 67]), (48, [64, 64])])
    geometry = [
        points([(27, 37), (14, 34)], name="Drag upper"),
        points([(25, 48), (10, 45)], name="Drag lower"),
    ]
    cue = cue_layers(
        20,
        "Drag",
        geometry,
        frames,
        position=movement,
        opacity=animated([(0, 18), (15, 100), (36, 100), (48, 18)]),
    )
    return lottie(
        "action_drag", frames, [*cursor_layers(frames, position=movement), *cue]
    )


def action_scroll() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(22, 30), (31, 20), (40, 30)], name="Scroll up"),
        points([(22, 50), (31, 60), (40, 50)], name="Scroll down"),
    ]
    cue = cue_layers(
        20,
        "Scroll",
        geometry,
        frames,
        position=animated([(0, [59, 69]), (24, [59, 60]), (48, [59, 69])]),
        opacity=animated([(0, 40), (24, 100), (48, 40)]),
    )
    return lottie("action_scroll", frames, [*cursor_layers(frames), *cue])


def action_text() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(31, 21), (31, 59)], name="Text stem"),
        points([(23, 21), (39, 21)], name="Text top"),
        points([(23, 59), (39, 59)], name="Text bottom"),
    ]
    cue = cue_layers(
        20,
        "Text",
        geometry,
        frames,
        position=static([60, 64]),
        opacity=animated(
            [(0, 100), (16, 100), (17, 16), (30, 16), (31, 100), (48, 100)], linear=True
        ),
    )
    return lottie("action_text", frames, [*cursor_layers(frames), *cue])


def action_key() -> dict[str, Any]:
    frames = 48
    geometry = [
        rectangle(27, 39, 29, 29, roundness=7, name="Key cap"),
        points([(22, 32), (22, 46)], name="Key stem"),
        points([(23, 39), (33, 31)], name="Key upper"),
        points([(24, 39), (34, 47)], name="Key lower"),
    ]
    cue = cue_layers(
        20,
        "Key",
        geometry,
        frames,
        width=3.5,
        position=animated([(0, [55, 64]), (8, [55, 68]), (17, [55, 62]), (26, [55, 64]), (48, [55, 64])]),
        scale=animated([(0, [100, 100]), (8, [91, 91]), (17, [104, 104]), (26, [100, 100]), (48, [100, 100])]),
        opacity=animated([(0, 0), (4, 100), (30, 100), (42, 0), (48, 0)]),
    )
    return lottie("action_key", frames, [*cursor_layers(frames), *cue])


def action_navigate() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(14, 28), (25, 40), (14, 52)], name="Navigate first"),
        points([(29, 28), (40, 40), (29, 52)], name="Navigate second"),
    ]
    cue = cue_layers(
        20,
        "Navigate",
        geometry,
        frames,
        position=animated([(0, [53, 64]), (18, [59, 64]), (30, [64, 64]), (48, [64, 64])]),
        opacity=animated([(0, 0), (8, 100), (28, 100), (42, 0), (48, 0)]),
    )
    return lottie("action_navigate", frames, [*cursor_layers(frames), *cue])


def action_app() -> dict[str, Any]:
    frames = 48
    geometry = [
        rectangle(x, y, 11, 11, roundness=2.5, name=f"App {x} {y}")
        for x, y in [(17, 30), (34, 30), (17, 48), (34, 48)]
    ]
    cue = cue_layers(
        20,
        "App",
        geometry,
        frames,
        width=3.5,
        position=static([59, 64]),
        scale=animated([(0, [18, 18]), (14, [112, 112]), (24, [100, 100]), (48, [100, 100])]),
        opacity=animated([(0, 0), (8, 100), (34, 100), (44, 0), (48, 0)]),
    )
    return lottie("action_app", frames, [*cursor_layers(frames), *cue])


def action_transfer() -> dict[str, Any]:
    frames = 48
    geometry = [
        points([(22, 51), (22, 19), (14, 27), (22, 19), (30, 27)], name="Transfer up"),
        points([(38, 27), (38, 59), (30, 51), (38, 59), (46, 51)], name="Transfer down"),
    ]
    cue = cue_layers(
        20,
        "Transfer",
        geometry,
        frames,
        position=animated([(0, [55, 71]), (24, [55, 58]), (48, [55, 71])]),
        opacity=animated([(0, 36), (24, 100), (48, 36)]),
    )
    return lottie("action_transfer", frames, [*cursor_layers(frames), *cue])


def action_record() -> dict[str, Any]:
    frames = 48
    ring = cue_layers(
        20,
        "Record ring",
        [ellipse(29, 39, 36, 36, name="Record ring")],
        frames,
        position=static([52, 64]),
    )
    dot_scale = animated([(0, [70, 70]), (24, [116, 116]), (48, [70, 70])])
    dot_opacity = animated([(0, 40), (24, 100), (48, 40)])
    dot = [
        shape_layer(
            30,
            "Record dot",
            [ellipse(29, 39, 11, 11, name="Record dot"), fill(AURORA), stroke(WHITE, 3)],
            frames,
            position=static([52, 64]),
            scale=dot_scale,
            opacity=dot_opacity,
        )
    ]
    return lottie("action_record", frames, [*cursor_layers(frames), *ring, *dot])


def action_system() -> dict[str, Any]:
    frames = 48
    geometry = [
        ellipse(29, 39, 24, 24, name="System outer"),
        ellipse(29, 39, 8, 8, name="System inner"),
        *[
            points(coordinates, name=f"System tooth {index}")
            for index, coordinates in enumerate(
                [
                    [(29, 19), (29, 25)],
                    [(29, 53), (29, 59)],
                    [(9, 39), (15, 39)],
                    [(43, 39), (49, 39)],
                    [(15, 25), (19, 29)],
                    [(39, 49), (43, 53)],
                    [(15, 53), (19, 49)],
                    [(39, 29), (43, 25)],
                ]
            )
        ],
    ]
    cue = cue_layers(
        20,
        "System",
        geometry,
        frames,
        width=3.5,
        position=static([49, 64]),
        rotation=animated([(0, -20), (22, 55), (34, 0), (48, 0)]),
        opacity=animated([(0, 0), (6, 100), (34, 100), (44, 0), (48, 0)]),
    )
    return lottie("action_system", frames, [*cursor_layers(frames), *cue])


ACTION_BUILDERS = {
    "idle": action_idle,
    "observe": action_observe,
    "click": action_click,
    "drag": action_drag,
    "scroll": action_scroll,
    "text": action_text,
    "key": action_key,
    "navigate": action_navigate,
    "app": action_app,
    "transfer": action_transfer,
    "record": action_record,
    "system": action_system,
}

# The frame each action rests on when reduced motion is active: a pose where the
# pointer and its mark are both fully drawn and readable standing still.
STILL_FRAMES = {
    "idle": 0,
    "observe": 20,
    "click": 8,
    "drag": 22,
    "scroll": 24,
    "text": 8,
    "key": 8,
    "navigate": 14,
    "app": 18,
    "transfer": 22,
    "record": 22,
    "system": 18,
}


def all_animations() -> dict[str, dict[str, Any]]:
    return {f"action_{name}": build() for name, build in ACTION_BUILDERS.items()}


def semantic_manifest() -> dict[str, Any]:
    return {
        "schema": "cua.cursor-theme/2",
        "id": THEME_ID,
        "name": THEME_NAME,
        "version": THEME_VERSION,
        "author": THEME_AUTHOR,
        "license": THEME_LICENSE,
        "compatibility": {"profile": "cua-driver-actions-v2", "semantics": 2},
        "canvas": {"width": CANVAS, "height": CANVAS, "fps": FPS},
        "hotspot": {"x": 55, "y": 28},
        "actions": {
            name: {"animation": f"action_{name}", "still_frame": STILL_FRAMES[name]}
            for name in ACTION_BUILDERS
        },
    }


def encoded_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def write_entry(archive: ZipFile, name: str, data: bytes) -> None:
    info = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    archive.writestr(info, data, compresslevel=9)


def build(output: Path) -> None:
    animations = all_animations()
    standard_manifest = {
        "version": "2",
        "generator": "Zeron Aurora cursor generator",
        "animations": [{"id": aid, "name": aid} for aid in animations],
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    with ZipFile(output, "w") as archive:
        write_entry(archive, "manifest.json", encoded_json(standard_manifest))
        write_entry(archive, "cua/theme.json", encoded_json(semantic_manifest()))
        for aid, animation in animations.items():
            write_entry(archive, f"a/{aid}.json", encoded_json(animation))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=Path(__file__).with_name("aurora.lottie"))
    args = parser.parse_args()
    build(args.output.resolve())
    print(args.output.resolve())


if __name__ == "__main__":
    main()
