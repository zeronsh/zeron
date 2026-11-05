#!/usr/bin/env python3
"""Mirror the desktop's icons into the iOS asset catalog (vector SVG imagesets).

- Tool icons (crates/ui/assets/icons/<name>.svg) → ToolIcons/tool-<name>
  as template images (currentColor → black; the painter tints them).
- File icons (crates/ui/assets/file-icons/{files,folders}/*.svg) → FileIcons/
  fileicon-<dir>-<name>, polychrome, with a dark-appearance variant using the
  desktop's palette lift (file_icons.rs `dark_icon_svg`).

Re-run after the desktop icon sets change:  python3 scripts/ios/gen-icons.py
"""
import json, os, re, shutil, sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
UI = os.path.join(ROOT, "crates/ui/assets")
CAT = os.path.join(ROOT, "apps/ios/Zeron/Assets.xcassets")

TOOL_ICONS = ["terminal", "document", "document-add", "pen", "magnifer", "folder-with-files",
              "global", "checklist", "widget", "bot", "chat-round-line", "alt-arrow-down",
              "alt-arrow-right", "arrow-up-right", "git-branch"]

DARK_LIFT = [("#64748B", "#CBD5E1"), ("#71717A", "#D4D4D8"), ("#2563EB", "#60A5FA"),
             ("#EA580C", "#FB923C"), ("#16A34A", "#4ADE80"), ("#8B5CF6", "#A78BFA"),
             ("#A855F7", "#C084FC")]


def group(path):
    os.makedirs(path, exist_ok=True)
    with open(os.path.join(path, "Contents.json"), "w") as f:
        json.dump({"info": {"author": "xcode", "version": 1}, "properties": {"provides-namespace": False}}, f, indent=2)


def imageset(dir_, name, svgs, template):
    path = os.path.join(dir_, name + ".imageset")
    os.makedirs(path, exist_ok=True)
    images = []
    for appearance, svg in svgs:
        fname = f"{name}{'-dark' if appearance == 'dark' else ''}.svg"
        with open(os.path.join(path, fname), "w") as f:
            f.write(svg)
        entry = {"filename": fname, "idiom": "universal"}
        if appearance == "dark":
            entry["appearances"] = [{"appearance": "luminosity", "value": "dark"}]
        images.append(entry)
    props = {"preserves-vector-representation": True}
    if template:
        props["template-rendering-intent"] = "template"
    with open(os.path.join(path, "Contents.json"), "w") as f:
        json.dump({"images": images, "info": {"author": "xcode", "version": 1}, "properties": props}, f, indent=2)


def main():
    tools = os.path.join(CAT, "ToolIcons")
    files = os.path.join(CAT, "FileIcons")
    for d in (tools, files):
        shutil.rmtree(d, ignore_errors=True)
        group(d)
    for name in TOOL_ICONS:
        svg = open(os.path.join(UI, "icons", name + ".svg")).read().replace("currentColor", "#000000")
        imageset(tools, "tool-" + name, [("any", svg)], template=True)
    count = 0
    for sub in ("files", "folders"):
        base = os.path.join(UI, "file-icons", sub)
        for fn in sorted(os.listdir(base)):
            if not fn.endswith(".svg"):
                continue
            svg = open(os.path.join(base, fn)).read()
            dark = svg
            for a, b in DARK_LIFT:
                dark = dark.replace(a, b).replace(a.lower(), b)
            variants = [("any", svg)] + ([("dark", dark)] if dark != svg else [])
            imageset(files, f"fileicon-{sub}-{fn[:-4]}", variants, template=False)
            count += 1
    print(f"{len(TOOL_ICONS)} tool icons, {count} file icons")


if __name__ == "__main__":
    sys.exit(main())
