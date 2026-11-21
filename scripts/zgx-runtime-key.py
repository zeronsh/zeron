#!/usr/bin/env python3
"""The key of Zeron's binary as zgx's runtime (scripts/package-*.sh with
ZGX_BUNDLE=1): a SHA-256 over everything it's built from that git tracks
(every file but the scripts, builtin plugins and native modules it ships as
files), the compiler, and how it's built. A binary built for a key is kept in
zgx's cache and taken from there while the key stays the same.

usage: zgx-runtime-key.py <repository root> <how it's built: "release beta">"""
import hashlib
import os
import subprocess
import sys

root, how = sys.argv[1], sys.argv[2]
files = subprocess.run(["git", "ls-files", "-z"], cwd=root, capture_output=True, check=True).stdout.split(b"\0")
shipped = (b"apps/zgx/ui/", b"apps/zgx/plugins-builtin/", b"apps/zgx/modules/")
hasher = hashlib.sha256()
hasher.update(subprocess.run(["rustc", "-vV"], cwd=os.path.join(root, "apps/zgx"), capture_output=True,
                             check=True).stdout)
hasher.update(how.encode() + b"\0")
for name in sorted(files):
    if not name or name.startswith(shipped):
        continue
    try:
        with open(os.path.join(root, name.decode()), "rb") as file:
            data = file.read()
    except (FileNotFoundError, IsADirectoryError):
        data = b"<none>"
    hasher.update(name + b"\0" + len(data).to_bytes(8, "little") + data)
print(hasher.hexdigest()[:32])
