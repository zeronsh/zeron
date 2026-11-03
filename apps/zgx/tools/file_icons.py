#!/usr/bin/env python3
"""File icons for the design system: Zeron's Symbols set (crates/ui/assets/
file-icons, miguelsolorio/vscode-symbols, MIT) copied under ui/assets/
icons/files/, each twice (as drawn for light, and with its darker accents
lifted for dark, as Zeron's `dark_icon_svg` does), and the theme's lookup
tables written as ui/ds/file_icons_map.luau. Run it again when Zeron's set
changes:

    python3 apps/zgx/tools/file_icons.py
"""

import json
import os
import shutil
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
ZGX = os.path.dirname(HERE)
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], cwd=ZGX, text=True).strip()
SRC = os.path.join(ROOT, "crates", "ui", "assets", "file-icons")
MANIFEST = os.path.join(ROOT, "crates", "ui", "src", "file-icons.json")
OUT_ASSETS = os.path.join(ZGX, "ui", "assets", "icons", "files")
OUT_MAP = os.path.join(ZGX, "ui", "ds", "file_icons_map.luau")

# Zeron's lift for dark (file_icons.rs `dark_icon_svg`).
LIFT = [
    ("#64748B", "#CBD5E1"),
    ("#71717A", "#D4D4D8"),
    ("#2563EB", "#60A5FA"),
    ("#EA580C", "#FB923C"),
    ("#16A34A", "#4ADE80"),
    ("#8B5CF6", "#A78BFA"),
    ("#A855F7", "#C084FC"),
]


def lift(svg):
    for a, b in LIFT:
        svg = svg.replace(a, b).replace(a.lower(), b)
    return svg


def luau_table(name, mapping):
    lines = [f"\t{name} = {{"]
    for key in sorted(mapping):
        lines.append(f'\t\t[{json.dumps(key.lower())}] = "{mapping[key]}",')
    lines.append("\t},")
    return "\n".join(lines)


def main():
    with open(MANIFEST) as f:
        theme = json.load(f)
    if os.path.isdir(OUT_ASSETS):
        shutil.rmtree(OUT_ASSETS)
    ids = {}
    for icon_id, d in theme["iconDefinitions"].items():
        rel = d["iconPath"].removeprefix("./icons/")
        src = os.path.join(SRC, rel)
        if not os.path.isfile(src):
            continue
        with open(src) as f:
            svg = f.read()
        for mode, text in (("light", svg), ("dark", lift(svg))):
            dst = os.path.join(OUT_ASSETS, mode, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            with open(dst, "w") as f:
                f.write(text)
        ids[icon_id] = rel.removesuffix(".svg")
    shutil.copy(os.path.join(SRC, "LICENSE.symbols"), os.path.join(OUT_ASSETS, "LICENSE.symbols"))

    def known(table):
        return {k: ids[v] for k, v in table.items() if v in ids}

    with open(OUT_MAP, "w") as f:
        f.write("-- Written by tools/file_icons.py from Zeron's Symbols theme\n")
        f.write("-- (crates/ui/src/file-icons.json). Don't edit; run the script again.\n")
        f.write("-- Each name is an icon's path under assets/icons/files/<light|dark>/.\n\n")
        f.write("return {\n")
        f.write(f'\tfile = "{ids[theme["file"]]}",\n')
        f.write(f'\tfolder = "{ids[theme["folder"]]}",\n')
        f.write(f'\troot = "{ids[theme["rootFolder"]]}",\n')
        f.write(luau_table("names", known(theme["fileNames"])) + "\n")
        f.write(luau_table("extensions", known(theme["fileExtensions"])) + "\n")
        f.write(luau_table("folders", known(theme["folderNames"])) + "\n")
        f.write(luau_table("languages", known(theme["languageIds"])) + "\n")
        f.write("}\n")
    print(f"{len(ids)} icons -> {os.path.relpath(OUT_ASSETS, ROOT)}, map -> {os.path.relpath(OUT_MAP, ROOT)}")


if __name__ == "__main__":
    main()
