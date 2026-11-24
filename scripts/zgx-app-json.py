#!/usr/bin/env python3
"""The zgx.json a packaged Zeron carries: its interface's (apps/zgx/ui/zgx.json:
its windows), with the app's id and version and its native modules, as
scripts/package-zgx-macos.sh and scripts/package-linux.sh hand to `zgx build`.

  zgx-app-json.py <ui zgx.json, or none> <version> <module>=<path>... <out>
"""

import json
import os
import sys


def main() -> None:
    *given, out = sys.argv[1:]
    ui, version, *modules = given
    config = {}
    if os.path.exists(ui):
        with open(ui) as file:
            config = json.load(file)
    for key in ("app_id", "version", "modules"):
        if key in config:
            sys.exit(f"{ui}: `{key}` is the packaging script's to say")
    config["app_id"] = "zeron"
    config["version"] = version
    config["modules"] = dict(module.split("=", 1) for module in modules)
    with open(out, "w") as file:
        json.dump(config, file, indent=2)
        file.write("\n")


if __name__ == "__main__":
    main()
