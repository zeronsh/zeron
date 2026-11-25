#!/usr/bin/env bash
# Main's tests, unedited: what proves that a branch with its new features
# off behaves as main does is main's own tests passing, so they must be
# main's. Every test file main (the base) has is diffed against HEAD:
#
#   crates/*/tests/**                      the crates' integration tests and
#                                          their fixtures
#   apps/zgx/ui/tests/*.test.luau          the Luau UI tests, and the files
#   apps/zgx/ui/*_fixture.luau, *_stub.luau  they mount
#   apps/zgx/src/**/*_tests.rs             zgx's Rust tests
#   apps/zgx/tests/**                      zgx's integration tests
#
# A changed or removed one fails, unless scripts/ci/main-parity-allow.txt
# names it (a path, then why: a general change main's test had to follow).
# An orchestrator-motivated edit never belongs there: revert it, or put the
# case in a test of its own.
#
# Inline `#[cfg(test)]` blocks of Rust files main has are diffed too, as
# warnings: they run in the same suites, but sit in files a feature edits.
#
#   scripts/ci/main-parity.sh [BASE]
#
# BASE: the argument, else $MAIN_PARITY_BASE, else the merge-base of HEAD
# with upstream/main (or origin/main). It's fetched if it isn't here.
# MAIN_PARITY_MERGED=1 (CI, where HEAD is GitHub's merge of the pull request
# into its branch): the merge's first parent instead, the very commit of the
# branch it was merged with. The event's base sha can be older than that
# (the branch moved on since), and main's own newer tests would then read as
# edits. A shallow checkout keeps no parents to walk, so they're read from
# the commit object itself.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd -P)"
cd "$root"
allow="scripts/ci/main-parity-allow.txt"

base="${1:-${MAIN_PARITY_BASE:-}}"
if [ -z "${1:-}" ] && [ "${MAIN_PARITY_MERGED:-}" = 1 ]; then
  parents="$(git cat-file -p HEAD | sed -n 's/^parent //p')"
  if [ "$(printf '%s\n' "$parents" | grep -c .)" = 2 ]; then
    base="$(printf '%s\n' "$parents" | head -1)"
  fi
fi
if [ -z "$base" ]; then
  for upstream in upstream/main origin/main; do
    if base="$(git merge-base HEAD "$upstream" 2>/dev/null)"; then
      break
    fi
    base=""
  done
fi
if [ -z "$base" ]; then
  echo "::error::main-parity: no base (pass one, or set MAIN_PARITY_BASE)" >&2
  exit 2
fi
if ! git cat-file -e "$base^{commit}" 2>/dev/null; then
  git fetch --no-tags --quiet --depth=1 origin "$base"
fi

exec python3 -I - "$base" "$allow" <<'PY'
import re
import subprocess
import sys

base, allow_path = sys.argv[1], sys.argv[2]


def git(*args: str) -> str:
    return subprocess.run(("git",) + args, check=True, capture_output=True, text=True).stdout


TESTS = re.compile(
    r"^(crates/[^/]+/tests/.+"
    r"|apps/zgx/ui/tests/[^/]+\.test\.luau"
    r"|apps/zgx/ui/[^/]+_(fixture|stub)\.luau"
    r"|apps/zgx/src/.+_tests\.rs"
    r"|apps/zgx/tests/.+)$"
)

at_base = [p for p in git("ls-tree", "-r", "--name-only", base).splitlines()]
tests = {p for p in at_base if TESTS.match(p)}

changed: dict[str, str] = {}
for line in git("diff", "--no-renames", "--name-status", base, "HEAD").splitlines():
    status, path = line.split("\t", 1)
    if path in tests and status[0] in "MDT":
        changed[path] = "removed" if status[0] == "D" else "changed"

allowed: dict[str, str] = {}
bad_lines = []
try:
    with open(allow_path, encoding="utf-8") as f:
        for n, raw in enumerate(f, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split(None, 1)
            if len(parts) < 2:
                bad_lines.append(f"{allow_path}:{n}: a path needs its reason")
                continue
            allowed[parts[0]] = parts[1]
except FileNotFoundError:
    pass

failed = sorted(p for p in changed if p not in allowed)
stale = sorted(p for p in allowed if p not in changed)

print(f"main-parity: {len(tests)} of main's test files (base {base[:12]}), "
      f"{len(changed)} differ, {len(changed) - len(failed)} allowed")
for p in sorted(changed):
    if p in allowed:
        print(f"  allowed  {p}: {allowed[p]}")


# ------------------------------------------- inline #[cfg(test)] blocks

def cfg_test_blocks(code: str) -> dict[str, str]:
    """Each `#[cfg(test)]` item's text, by its name (`mod tests`, `fn x`)."""
    out: dict[str, str] = {}
    for m in re.finditer(r"#\[cfg\(test\)\]", code):
        i = m.end()
        head = re.match(r"\s*(?:(?:#\[[^\]]*\]|//[^\n]*)\s*)*(?:pub(?:\([^)]*\))?\s+)?"
                        r"(mod|fn|impl|struct|enum|const|static|use|async\s+fn|[a-z_]+!)\s*([^\s{;(<]*)", code[i:])
        end = item_end(code, i, named=head is not None)
        # A field or a statement under it: named by its own text.
        name = f"{head.group(1)} {head.group(2)}" if head else " ".join(code[i:end].split())
        key, k = name, 2
        while key in out:
            key, k = f"{name}#{k}", k + 1
        out[key] = code[m.start():end]
    return out


def item_end(code: str, i: int, named: bool = True) -> int:
    """Where the item starting at `i` ends: its closing brace at depth 0,
    or a `;` before any brace; a field or statement (`named` false) also at
    a `,` at depth 0 or its enclosing brace. Skips strings, chars and
    comments."""
    depth, n = 0, len(code)
    while i < n:
        c = code[i]
        if code.startswith("//", i):
            j = code.find("\n", i)
            i = n if j < 0 else j
            continue
        if code.startswith("/*", i):
            j = code.find("*/", i + 2)
            i = n if j < 0 else j + 2
            continue
        raw = re.match(r"b?r(#*)\"", code[i:])
        if raw and (i == 0 or not (code[i - 1].isalnum() or code[i - 1] == "_")):
            close = '"' + raw.group(1)
            j = code.find(close, i + raw.end())
            i = n if j < 0 else j + len(close)
            continue
        if c == '"':
            j = i + 1
            while j < n and code[j] != '"':
                j += 2 if code[j] == "\\" else 1
            i = j + 1
            continue
        if c == "'":
            ch = re.match(r"'(\\.[^']*|[^\\'])'", code[i:])
            if ch:
                i += ch.end()
                continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth < 0:
                return i
            if depth == 0 and named:
                return i + 1
        elif c in ";," and depth == 0 and (c == ";" or not named):
            return i + 1
        i += 1
    return n


def show(rev: str, path: str) -> str | None:
    out = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, text=True)
    return out.stdout if out.returncode == 0 else None


warnings = []
rust = [line.split("\t", 1) for line in
        git("diff", "--no-renames", "--name-status", base, "HEAD", "--", "*.rs").splitlines()]
for status, path in rust:
    if path in tests or status[0] not in "MD":
        continue
    old = show(base, path)
    if old is None or "#[cfg(test)]" not in old:
        continue
    new = show("HEAD", path) or ""
    before, after = cfg_test_blocks(old), cfg_test_blocks(new)
    for name in sorted(set(before) | set(after)):
        if name not in before:
            continue  # a new block: the branch's own tests
        if name not in after:
            what = name if len(name) < 80 else name[:77] + "..."
            warnings.append(f"{path}: #[cfg(test)] {what} removed or changed")
        elif before[name] != after[name]:
            a = set(before[name].splitlines())
            b = after[name].splitlines()
            gone = len([l for l in before[name].splitlines() if l not in set(b)])
            added = len([l for l in b if l not in a])
            warnings.append(f"{path}: #[cfg(test)] {name} changed (+{added} -{gone} lines)")

for w in warnings:
    print(f"::warning::main-parity: {w}")
for p in stale:
    print(f"::warning::main-parity: {allow_path} allows {p}, which is main's again: drop its line")
for line in bad_lines:
    print(f"::error::{line}")
if failed:
    print("::error::main-parity: main's tests edited on this branch, not in "
          f"{allow_path} (revert them, or move the case to a test of its own):")
    for p in failed:
        print(f"  {changed[p]}  {p}")
if failed or bad_lines:
    sys.exit(1)
print("main-parity: ok")
PY
