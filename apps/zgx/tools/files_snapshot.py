#!/usr/bin/env python3
"""The Files demo's checkout: this worktree's files as git lists them, their
status, the sizes of images, and the text of a selection of files, written
as a Luau module (ui/demos/files_snapshot.luau).

A script reads only files the user picks, so the gallery can't walk the
checkout itself; this takes it once. Run it again to refresh:

    python3 apps/zgx/tools/files_snapshot.py
"""

import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ZGX = os.path.dirname(HERE)
ROOT = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], cwd=ZGX, text=True).strip()
OUT = os.path.join(ZGX, "ui", "demos", "files_snapshot.luau")
# Where the demo says the checkout lives: no one's real home folder.
SHOWN_ROOT = "~/code/zeron"

# Whose text comes along. Every file is listed (path, size, status), but only
# some carry their text, so the demo stays small: the files the page is built
# around (CURATED, whatever their size up to TEXT_MAX), then a sample spread
# evenly over the rest of TEXT_GLOBS's files, each at most SAMPLE_MAX, until
# TEXT_COUNT files or TEXT_BUDGET bytes. Any other file opens to "not in this
# demo" (ui/demos/files.luau).
CURATED = [
    "README.md",
    "Cargo.toml",
    ".gitignore",
    "apps/zgx/README.md",
    "apps/zgx/Cargo.toml",
    # One big file to open: Zeron's preview.rs, 5,000 lines.
    "crates/ui/src/files/preview.rs",
    "crates/ui/src/files/tree.rs",
    "apps/zgx/src/files.rs",
    "apps/zgx/ui/demos/files.luau",
    "apps/zgx/ui/ds/file_tree.luau",
    "apps/zgx/ui/ds/file_viewer.luau",
    "apps/zgx/ui/ds/files_pane.luau",
    "apps/zgx/tools/files_snapshot.py",
]
TEXT_GLOBS = [
    r"^apps/zgx/(README\.md|Cargo\.toml|src/.*\.rs|ui/.*\.(luau|json|md)|ui/\.luaurc|tools/.*\.py)$",
    r"^crates/ui/src/files/(tree|git_status|preview|rename|search|drag)\.rs$",
    r"^[^/]+\.(md|toml)$",
    r"^\.gitignore$",
    r"^docs/[^/]+\.md$",
    r"^edge/[^/]+\.(json|toml|md)$",
    r"^scripts/[^/]+\.sh$",
]
# Docs that stay out of the demo: listed in the tree, their text not embedded.
TEXT_LEFT_OUT = {
    "docs/PARITY.md",
    "docs/chat2-sync.md",
    "docs/performance-workspace-search.md",
    "docs/mobile-polish.md",
}
# Security-sensitive sources never carry their text: plugin and template
# consent (the files src/plugins/grants.rs lets decide or sign), and any file
# naming what it guards, so the snapshot never holds that code either.
GUARDED_NAMES = {
    "consent.rs",
    "consent.luau",
    "consent_fixture.luau",
    "plugins_consent.test.luau",
    "grants.rs",
    "template_consent.rs",
    "template_consent.luau",
    "template_consent_fixture.luau",
    "templates_consent.test.luau",
}
GUARDED_WORDS = ("DecidePlugin", "SignPlugin", "plugins.decide", "plugins.sign", "DecideTemplate", "templates.decide")
TEXT_MAX = 256 * 1024
SAMPLE_MAX = 24 * 1024
TEXT_COUNT = 150
TEXT_BUDGET = 2 * 1024 * 1024
IMAGE_EXT = (".png", ".jpg", ".jpeg")


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True)


def main():
    tracked = [p for p in git("ls-files", "-z").split("\0") if p]
    untracked = [p for p in git("ls-files", "-z", "--others", "--exclude-standard").split("\0") if p]
    status = {}
    for line in git("status", "--porcelain=v1", "-uall", "-z").split("\0"):
        if len(line) < 4:
            continue
        xy, path = line[:2], line[3:]
        if xy == "??":
            status[path] = "untracked"
        elif "U" in xy or xy in ("AA", "DD"):
            status[path] = "conflict"
        elif "D" in xy:
            status[path] = "deleted"
        elif "A" in xy:
            status[path] = "added"
        elif "R" in xy:
            status[path] = "renamed"
        elif "M" in xy:
            status[path] = "modified"
    # What git ignores, as folders with what's directly in them.
    ignored = []
    for line in git("status", "--porcelain=v1", "--ignored", "-z").split("\0"):
        if line.startswith("!! "):
            path = line[3:]
            full = os.path.join(ROOT, path)
            if path.endswith("/") and os.path.isdir(full):
                for name in sorted(os.listdir(full))[:40]:
                    child = os.path.join(full, name)
                    suffix = "/" if os.path.isdir(child) else ""
                    ignored.append(path + name + suffix)
            else:
                ignored.append(path)

    rows = []
    texts = {}
    images = {}
    seen = set()
    candidates = []
    for path in sorted(set(tracked) | set(untracked)):
        full = os.path.join(ROOT, path)
        if not os.path.isfile(full) or path in seen:
            continue
        seen.add(path)
        size = os.path.getsize(full)
        rows.append(f"{path}\t{size}\t{status.get(path, '')}")
        if path not in TEXT_LEFT_OUT and path not in CURATED and size <= SAMPLE_MAX and any(re.match(g, path) for g in TEXT_GLOBS):
            candidates.append(path)
        if path.lower().endswith(IMAGE_EXT):
            out = subprocess.run(["sips", "-g", "pixelWidth", "-g", "pixelHeight", full], capture_output=True, text=True).stdout
            w = re.search(r"pixelWidth: (\d+)", out)
            h = re.search(r"pixelHeight: (\d+)", out)
            if w and h:
                images[path] = (int(w.group(1)), int(h.group(1)))
    # The curated files, then an even spread over the candidates.
    budget = TEXT_BUDGET

    def take(path, cap):
        nonlocal budget
        full = os.path.join(ROOT, path)
        if os.path.basename(path) in GUARDED_NAMES:
            return
        if not os.path.isfile(full) or os.path.getsize(full) > min(cap, budget):
            return
        try:
            with open(full, encoding="utf-8") as f:
                text = f.read()
        except UnicodeDecodeError:
            return
        if any(w in text for w in GUARDED_WORDS):
            return
        texts[path] = text
        budget -= os.path.getsize(full)

    for path in CURATED:
        take(path, TEXT_MAX)
    room = TEXT_COUNT - len(texts)
    if room > 0 and candidates:
        step = max(1, len(candidates) / room)
        picks = sorted({candidates[int(i * step)] for i in range(room) if int(i * step) < len(candidates)})
        for path in picks:
            if len(texts) >= TEXT_COUNT:
                break
            take(path, SAMPLE_MAX)
    for path in ignored:
        full = os.path.join(ROOT, path)
        size = os.path.getsize(full) if os.path.isfile(full) else 0
        rows.append(f"{path}\t{size}\tignored")

    def long_string(s):
        level = 0
        while f"]{'=' * level}]" in s:
            level += 1
        eq = "=" * level
        # A leading newline is dropped by Luau's long strings: keep it.
        return f"[{eq}[\n{s}]{eq}]"

    with open(OUT, "w", encoding="utf-8") as f:
        f.write("-- Written by tools/files_snapshot.py: this checkout's files, as the Files\n")
        f.write("-- demo shows them. Don't edit; run the script again.\n\n")
        f.write("return {\n")
        # A neutral root, not this machine's: the demo only shows it.
        f.write(f'\troot = "{SHOWN_ROOT}",\n')
        f.write(f'\tname = "{os.path.basename(SHOWN_ROOT)}",\n')
        f.write("\t-- A line a file: its path, its size in bytes, its git status.\n")
        f.write(f"\tfiles = {long_string(chr(10).join(rows))},\n")
        f.write("\timages = {\n")
        for path, (w, h) in sorted(images.items()):
            f.write(f'\t\t["{path}"] = {{ {w}, {h} }},\n')
        f.write("\t},\n")
        f.write("\ttexts = {\n")
        for path, text in sorted(texts.items()):
            f.write(f'\t\t["{path}"] = {long_string(text)},\n')
        f.write("\t},\n")
        f.write("}\n")
    print(f"{len(rows)} files, {len(texts)} texts, {len(images)} images -> {os.path.relpath(OUT, ROOT)} ({os.path.getsize(OUT) // 1024} KB)")


if __name__ == "__main__":
    sys.exit(main())
