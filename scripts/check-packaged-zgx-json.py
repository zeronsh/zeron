#!/usr/bin/env python3
"""Fails if a packaged Zeron's zgx.json lost, or changed, anything the
interface's own (apps/zgx/ui/zgx.json) says: the packaging makes the file it
ships, so what it forgets to carry is gone from the app (its windows, say,
without which it doesn't open).

  check-packaged-zgx-json.py <apps/zgx/ui/zgx.json> <the package's ui/zgx.json>

`packages` and `modules` aren't compared: `zgx build` takes both out, the
bundle's folder and manifest having them.
"""

import json
import sys

CARRIED_BY_THE_BUNDLE = ("packages", "modules")


def lost(source, packaged, at=""):
    """What `source` says that `packaged` doesn't, each by its path."""
    if not isinstance(source, dict) or not isinstance(packaged, dict):
        return [] if source == packaged else [f"{at}: {packaged!r}, not {source!r}"]
    found = []
    for key, value in source.items():
        path = f"{at}.{key}" if at else key
        if not at and key in CARRIED_BY_THE_BUNDLE:
            continue
        if key not in packaged:
            found.append(f"{path}: not there")
        else:
            found.extend(lost(value, packaged[key], path))
    return found


def main() -> int:
    source_path, packaged_path = sys.argv[1:]
    with open(source_path) as file:
        source = json.load(file)
    try:
        with open(packaged_path) as file:
            packaged = json.load(file)
    except FileNotFoundError:
        print(f"{packaged_path}: not there", file=sys.stderr)
        return 1
    found = lost(source, packaged)
    for what in found:
        print(f"{packaged_path} lost what {source_path} says: {what}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
