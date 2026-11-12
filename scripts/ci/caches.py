#!/usr/bin/env python3
"""The repository's Actions cache: which caches may exist, who saves them, and
the janitor that removes everything else.

GitHub gives the repository 10 GB of Actions cache and evicts the least
recently used entries when it's full. One stray saver (a tag's release build,
a pull request's npm cache) is enough to push out a cache main's CI restores
on every run. So every cache is declared here, as a family:

  - `pattern`: a regular expression its keys match, whole;
  - `refs`: the refs allowed to save it (main, or zeron/zgx for zgx's suite);
  - `rank`: its priority, 1 the highest (minutes saved per month per GB);
  - `size_gb`: its expected size, one generation.

Two things hold the line:

  - scripts/tests/test_ci_caches.py lints the workflows and actions with
    `lint()`: every step that can save a cache saves a declared family, only
    from the refs that family names, and nothing saves by default (no
    actions/cache@, no setup-node `cache:`, no setup-zig cache);
  - .github/workflows/ci-caches.yml runs `janitor` every 6 hours, which
    deletes entries no family declares, entries saved from refs their family
    doesn't allow, older generations, and, while the rest is over BUDGET, the
    lowest-priority families (`select()`).

Pass records (scripts/ci/reuse.py, `ci-pass-...`) are a few hundred bytes each
and may be saved from any ref, pull requests included: CI skips what already
passed on the same inputs. They never count against the budget, and the
janitor keeps only those of refs some run trusts (PASS_REFS): main, zeron/zgx
and pull requests' merge refs.

Usage:
  caches.py janitor [--dry-run] [--repo OWNER/REPO] [--from-json FILE]
  caches.py lint [ROOT]
  caches.py families
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field

MAIN = "refs/heads/main"
ZGX = "refs/heads/zeron/zgx"

GB = 10**9
# GitHub's 10 GB, less room for a new generation being saved before the janitor
# removes the one it replaces. The families below add up to about 9.4 GB.
BUDGET = int(9.5 * GB)

HEX = "[0-9a-f]+"
# rust-cache's key: <prefix-key, v0-rust>-<shared-key, or key and job id>-<os>-<arch>-<environment hash>-<lockfile hash>.
# The os and arch are Node's os.type() and os.arch().
LINUX, LINUX_ARM, MACOS, WINDOWS = "Linux-x64", "Linux-arm64", "Darwin-arm64", "Windows_NT-x64"


def rust(name: str, platform: str) -> str:
    return rf"{re.escape(name)}-{re.escape(platform)}-{HEX}-{HEX}"


@dataclass(frozen=True)
class Family:
    name: str
    pattern: str
    refs: tuple[str, ...] | None  # None: any ref (see ref_pattern)
    rank: int  # 1 is kept first; 0 never counts against the budget
    size_gb: float
    saved_by: str
    value: int | None = None  # minutes saved per month per GB, as measured
    max_entry_bytes: int | None = None
    ref_pattern: str | None = None  # refs None: only refs matching this, whole (None: any)
    regex: re.Pattern = field(init=False, repr=False, compare=False)

    def __post_init__(self):
        object.__setattr__(self, "regex", re.compile(self.pattern))

    def matches(self, key: str) -> bool:
        return self.regex.fullmatch(key) is not None

    def allows(self, ref: str) -> bool:
        if self.refs is None:
            return self.ref_pattern is None or re.fullmatch(self.ref_pattern, ref) is not None
        return ref in self.refs


# The refs whose pass records some run trusts (plan.py): main, zeron/zgx (the zgx
# interface's integration branch) and a pull request's merge ref (its own later
# runs). Saved from anywhere else (a run by hand on a side branch, the merge
# queue, a tag), nobody reads them. A fork's pull request's records can't be
# told from a same-repository one's by ref, so every pull request's stay; none
# count against the budget, so they can't push out a cache.
PASS_REFS = rf"{re.escape(MAIN)}|{re.escape(ZGX)}|refs/pull/\d+/merge"

PASS = Family(
    "pass-records", r"ci-pass-v\d+-[a-z0-9_-]+-[0-9a-f]{40}-[a-z0-9-]+", None, 0, 0.0,
    "every CI job's last step (scripts/ci/reuse.py), from any ref", max_entry_bytes=64 * 1024,
    ref_pattern=PASS_REFS)

# In priority order. Keep `rank` in step with the order (test_ci_caches.py checks).
FAMILIES: tuple[Family, ...] = (
    Family("windows", rust("v0-rust-windows", WINDOWS), (MAIN,), 1, 1.25,
           "windows.yml `tests` (.github/actions/windows-ci-setup, shared-key windows)", 7640),
    Family("linux-ui", rust("ci-linux-v1-ui", LINUX), (MAIN,), 2, 1.15,
           "ui-tests.yml `ui-tests` (.github/actions/linux-ci)", 7200),
    Family("linux-core", rust("ci-linux-v1-core", LINUX), (MAIN,), 3, 0.68,
           "ui-tests.yml `core-tests` (.github/actions/linux-ci)", 6350),
    Family("zgx-tests-macos", rust("v0-rust-zgx-tests-zgx-macos", MACOS), (ZGX,), 4, 1.4,
           "zgx-tests.yml `zgx-macos`, on zeron/zgx"),
    Family("zgx-tests-linux", rust("v0-rust-zgx-tests-zgx-linux", LINUX), (ZGX,), 5, 0.8,
           "zgx-tests.yml `zgx-linux`, on zeron/zgx"),
    Family("ios-tests", rust("v0-rust-ios-tests", MACOS), (MAIN,), 6, 0.76,
           "macos.yml `ios-tests`", 3840),
    Family("macos-native", rust("v0-rust-macos-native", MACOS), (MAIN,), 7, 1.79,
           "macos.yml `macos-native`", 3170),
    Family("ios-core", rust("v0-rust-ios-core", LINUX), (MAIN,), 8, 0.81,
           "macos.yml `ios-core` (on Linux)", 2950),
    Family("apt-ui-browser", r"apt-debs-v1-ui-browser-[0-9a-f]{64}", (MAIN,), 9, 0.26,
           "ui-tests.yml `ui-tests` (.github/actions/linux-ci, save-apt-cache)"),
    Family("zgx-ghostty", r"zgx-ghostty-[0-9a-f]{64}", (ZGX,), 10, 0.49,
           "zgx-tests.yml `zgx-macos`, on zeron/zgx"),
    Family("zgx-cli-macos", rf"zgx-cli-macOS-ARM64-{HEX}", (ZGX,), 11, 0.02,
           "zgx-tests.yml `zgx-macos`, on zeron/zgx"),
    Family("zgx-cli-linux", rf"zgx-cli-Linux-X64-{HEX}", (ZGX,), 12, 0.02,
           "zgx-tests.yml `zgx-linux`, on zeron/zgx"),
)

ALL_FAMILIES = (PASS,) + FAMILIES

DECLARE = ("Declare its family in scripts/ci/caches.py (key pattern, the refs that may save it, "
           "expected size and priority), or don't save it: `save-if: false` for rust-cache, "
           "actions/cache/restore alone otherwise. See docs/reference/ci.md#caches.")


def family_of(key: str, size: int = 0) -> Family | None:
    for family in ALL_FAMILIES:
        if family.matches(key):
            if family.max_entry_bytes is not None and size > family.max_entry_bytes:
                return None
            return family
    return None


# --- The janitor ---------------------------------------------------------------------------------

@dataclass
class Entry:
    id: int
    key: str
    ref: str
    size: int
    created: str
    accessed: str = ""


def entry(raw: dict) -> Entry:
    """A cache entry from the REST API (snake case) or `gh cache list --json` (camel case)."""
    return Entry(
        id=int(raw.get("id") or 0),
        key=raw["key"],
        ref=raw.get("ref", ""),
        size=int(raw.get("size_in_bytes", raw.get("sizeInBytes", 0)) or 0),
        created=raw.get("created_at") or raw.get("createdAt") or "",
        accessed=raw.get("last_accessed_at") or raw.get("lastAccessedAt") or "",
    )


@dataclass
class Selection:
    keep: list[tuple[Entry, Family]]
    delete: list[tuple[Entry, str]]  # (entry, reason)

    @property
    def kept_bytes(self) -> int:
        return sum(e.size for e, _ in self.keep)

    @property
    def deleted_bytes(self) -> int:
        return sum(e.size for e, _ in self.delete)


def select(entries: list[Entry], families: tuple[Family, ...] = ALL_FAMILIES,
           budget: int = BUDGET) -> Selection:
    """What to keep and what to delete:

    (a) entries no family declares;
    (b) entries saved from a ref their family doesn't allow (pass records: from a ref no run
        trusts, PASS_REFS);
    (c) older generations: only the newest entry of a family per ref stays (rust-cache's key
        changes with the lockfile, so main's previous cache lingers until evicted);
    (d) while the rest exceeds `budget`, whole families, lowest priority first. Pass records
        (rank 0) don't count: any pull request, a fork's too, can save any number of
        them, and they must not push out a cache.
    """
    delete: list[tuple[Entry, str]] = []
    candidates: list[tuple[Entry, Family]] = []
    for e in entries:
        family = next((f for f in families if f.matches(e.key)
                       and (f.max_entry_bytes is None or e.size <= f.max_entry_bytes)), None)
        if family is None:
            delete.append((e, "undeclared: no family in scripts/ci/caches.py"))
        elif not family.allows(e.ref):
            delete.append((e, f"{family.name} saved from {e.ref or '(no ref)'}, which may not save it"))
        else:
            candidates.append((e, family))

    keep: list[tuple[Entry, Family]] = []
    newest: dict[tuple[str, str], Entry] = {}
    for e, family in sorted(candidates, key=lambda c: (c[0].created, c[0].id), reverse=True):
        if family.rank == 0:
            keep.append((e, family))  # pass records: each is a different suite and fingerprint
            continue
        slot = (family.name, e.ref)
        if slot in newest:
            delete.append((e, f"older {family.name} generation (newer: {newest[slot].key})"))
        else:
            newest[slot] = e
            keep.append((e, family))

    total = sum(e.size for e, f in keep if f.rank > 0)
    for family in sorted({f for _, f in keep if f.rank > 0}, key=lambda f: f.rank, reverse=True):
        if total <= budget:
            break
        dropped = [(e, f) for e, f in keep if f is family]
        for e, _ in dropped:
            delete.append((e, f"over the {budget / GB:.1f} GB budget: {family.name} has priority {family.rank}"))
            total -= e.size
        keep = [(e, f) for e, f in keep if f is not family]

    keep.sort(key=lambda c: (c[1].rank, c[0].ref, c[0].key))
    return Selection(keep, delete)


def size(n: int) -> str:
    return f"{n / GB:.2f} GB" if n >= GB else f"{n / 10**6:.2f} MB" if n >= 10**4 else f"{n} B"


def summary(sel: Selection, dry_run: bool, failed: list[Entry] = ()) -> str:
    lines = [
        "## Actions caches" + (" (dry run: nothing deleted)" if dry_run else ""),
        "",
        f"Kept {len(sel.keep)} entries, {size(sel.kept_bytes)} of the {BUDGET / GB:.1f} GB budget. "
        f"{'Would delete' if dry_run else 'Deleted'} {len(sel.delete)}, {size(sel.deleted_bytes)}.",
        "",
    ]
    if sel.delete:
        lines += ["### " + ("To delete" if dry_run else "Deleted"), "",
                  "| Key | Ref | Size | Why |", "|---|---|---|---|"]
        lines += [f"| `{e.key}` | `{e.ref}` | {size(e.size)} | {why} |"
                  for e, why in sorted(sel.delete, key=lambda d: -d[0].size)]
        lines.append("")
    if failed:
        lines += ["Could not delete: " + ", ".join(f"`{e.key}`" for e in failed), ""]
    lines += ["### Kept", "", "| Family | Priority | Key | Ref | Size |", "|---|---|---|---|---|"]
    lines += [f"| {f.name} | {f.rank} | `{e.key}` | `{e.ref}` | {size(e.size)} |" for e, f in sel.keep if f.rank]
    passes = [e for e, f in sel.keep if not f.rank]
    if passes:
        refs = len({e.ref for e in passes})
        lines.append(f"| {PASS.name} | - | {len(passes)} entries | {refs} refs | {size(sum(e.size for e in passes))} |")
    return "\n".join(lines) + "\n"


class Api:
    def __init__(self, repo: str, token: str):
        self.repo, self.token = repo, token

    def request(self, method: str, path: str):
        req = urllib.request.Request(f"https://api.github.com/repos/{self.repo}{path}", method=method, headers={
            "Authorization": f"Bearer {self.token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "ci-caches",
        })
        with urllib.request.urlopen(req, timeout=60) as response:
            body = response.read()
            return json.loads(body) if body else None

    def caches(self) -> list[dict]:
        found: list[dict] = []
        page = 1
        while True:
            data = self.request("GET", f"/actions/caches?per_page=100&page={page}")
            found.extend(data["actions_caches"])
            if len(data["actions_caches"]) < 100:
                return found
            page += 1

    def delete(self, cache_id: int) -> bool:
        try:
            self.request("DELETE", f"/actions/caches/{cache_id}")
        except urllib.error.HTTPError as error:
            if error.code != 404:  # already gone
                print(f"::warning::deleting cache {cache_id}: HTTP {error.code}", file=sys.stderr)
                return False
        return True


def janitor(args) -> int:
    repo = args.repo or os.environ.get("GITHUB_REPOSITORY", "zeronsh/zeron")
    if args.from_json:
        raw = json.loads(pathlib.Path(args.from_json).read_text(encoding="utf-8"))
        raw = raw.get("actions_caches", raw) if isinstance(raw, dict) else raw
        api = None
    else:
        token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
        if not token:
            print("GITHUB_TOKEN is not set", file=sys.stderr)
            return 2
        api = Api(repo, token)
        raw = api.caches()
    sel = select([entry(r) for r in raw])
    dry_run = args.dry_run or api is None
    failed = []
    if not dry_run:
        for e, why in sel.delete:
            print(f"deleting {e.key} ({e.ref}, {size(e.size)}): {why}")
            if not api.delete(e.id):
                failed.append(e)
    text = summary(sel, dry_run, failed)
    print(text)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as out:
            out.write(text)
    return 1 if failed else 0


# --- The lint ------------------------------------------------------------------------------------
#
# A small reader for the workflows' YAML: steps by indentation, their `uses`, `if` and `with`
# values (block scalars folded), each with its job and that job's runner. No PyYAML: CI's plan
# job runs this with the runner's bare python3.

ITEM = re.compile(r"^(\s*)-\s+(\S.*)$")
KEY = re.compile(r"^(\s*)([\w.-]+):(?:\s+(.*?))?\s*$")
BLOCK = re.compile(r"^[|>][-+]?\d*$")


@dataclass
class Step:
    path: str
    line: int
    job: str | None
    runs_on: str | None
    uses: str
    uses_raw: str  # with its comment (`# v4.4.0`)
    keys: dict[str, str]
    with_: dict[str, str]

    @property
    def where(self) -> str:
        return f"{self.path}:{self.line}" + (f" (job {self.job})" if self.job else "")


def scalar(value: str | None) -> str:
    value = (value or "").strip()
    if value[:1] in ("'", '"'):
        quote = value[0]
        end = value.find(quote, 1)
        return value[1:end] if end > 0 else value[1:]
    return re.split(r"\s+#", value, maxsplit=1)[0].strip()


def indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def meaningful(line: str) -> bool:
    stripped = line.strip()
    return bool(stripped) and not stripped.startswith("#")


def read_mapping(lines: list[str], start: int, end: int, at: int) -> dict[str, str]:
    """Keys at indentation `at` in lines[start:end], block scalars joined."""
    found: dict[str, str] = {}
    i = start
    while i < end:
        line = lines[i]
        m = KEY.match(line)
        if m and indent(line) == at and meaningful(line):
            name, value = m.group(2), (m.group(3) or "")
            j = i + 1
            while j < end and (not meaningful(lines[j]) or indent(lines[j]) > at):
                j += 1
            if BLOCK.match(value.strip()):
                body = [lines[k].strip() for k in range(i + 1, j) if lines[k].strip()]
                found[name] = ("\n" if value.startswith("|") else " ").join(body)
            elif value:
                found[name] = scalar(value)
            else:
                found[name] = ""
            i = j
        else:
            i += 1
    return found


def scalar_lines(lines: list[str]) -> set[int]:
    """Lines inside block scalars (a `run: |` script), never read as YAML."""
    inside: set[int] = set()
    i = 0
    while i < len(lines):
        m = KEY.match(lines[i]) or None
        value = m.group(3) if m else None
        if value is None:
            item = ITEM.match(lines[i])
            if item:
                inner = KEY.match(" " * (len(item.group(1)) + 2) + item.group(2))
                value = inner.group(3) if inner else None
        if value is not None and BLOCK.match(value.strip()):
            base = indent(lines[i]) + (2 if ITEM.match(lines[i]) else 0)
            j = i + 1
            while j < len(lines) and (not lines[j].strip() or indent(lines[j]) > base):
                inside.add(j)
                j += 1
            i = j
        else:
            i += 1
    return inside


def parse_steps(path: str, text: str) -> list[Step]:
    lines = text.splitlines()
    skip = scalar_lines(lines)
    # Jobs (a workflow's `jobs:` children) and their runners.
    jobs: list[tuple[int, str]] = []
    runs_on: dict[str, str] = {}
    in_jobs = False
    for n, line in enumerate(lines):
        if n in skip or not meaningful(line):
            continue
        if indent(line) == 0:
            in_jobs = line.startswith("jobs:")
            continue
        m = KEY.match(line)
        if in_jobs and m and indent(line) == 2:
            jobs.append((n, m.group(2)))
        elif in_jobs and m and indent(line) == 4 and m.group(2) == "runs-on" and jobs:
            runs_on[jobs[-1][1]] = scalar(m.group(3))

    def job_at(n: int) -> str | None:
        current = None
        for start, name in jobs:
            if start < n:
                current = name
        return current

    steps = []
    for n, line in enumerate(lines):
        if n in skip:
            continue
        m = ITEM.match(line)
        if not m:
            continue
        at = len(m.group(1)) + 2
        end = n + 1
        while end < len(lines) and (end in skip or not meaningful(lines[end]) or indent(lines[end]) >= at):
            end += 1
        block = [" " * at + m.group(2)] + lines[n + 1:end]
        keys = read_mapping(block, 0, len(block), at)
        if "uses" not in keys:
            continue
        with_: dict[str, str] = {}
        for k, l in enumerate(block):
            km = KEY.match(l)
            if km and indent(l) == at and km.group(2) == "with":
                stop = k + 1
                while stop < len(block) and (not meaningful(block[stop]) or indent(block[stop]) > at):
                    stop += 1
                inner = next((indent(block[s]) for s in range(k + 1, stop) if meaningful(block[s])), at + 2)
                with_ = read_mapping(block, k + 1, stop, inner)
        job = job_at(n)
        raw = next((u.group(1) for u in (re.search(r"\buses:\s*(.+?)\s*$", l) for l in block) if u), keys["uses"])
        steps.append(Step(path, n + 1, job, runs_on.get(job) if job else None, keys["uses"], raw, keys, with_))
    return steps


def action_inputs(text: str) -> dict[str, str]:
    """A composite action's inputs and their defaults."""
    lines = text.splitlines()
    defaults: dict[str, str] = {}
    in_inputs, current = False, None
    for line in lines:
        if not meaningful(line):
            continue
        if indent(line) == 0:
            in_inputs = line.startswith("inputs:")
            continue
        m = KEY.match(line)
        if in_inputs and m and indent(line) == 2:
            current = m.group(2)
            defaults[current] = ""
        elif in_inputs and m and indent(line) == 4 and m.group(2) == "default" and current:
            defaults[current] = scalar(m.group(3))
    return defaults


def platform(runs_on: str | None) -> str | None:
    """Node's os.type()-os.arch() on a hosted runner label (rust-cache's key)."""
    label = (runs_on or "").lower()
    if not label or "${{" in label:
        return None
    if "windows" in label:
        return WINDOWS
    if "macos" in label:
        return "Darwin-x64" if re.search(r"macos-13|intel|large", label) else MACOS
    if "ubuntu" in label or "linux" in label:
        return LINUX_ARM if "arm" in label else LINUX
    return None


def runner_context(plat: str) -> dict[str, str]:
    os_name, arch = plat.split("-")
    return {"runner.os": {"Linux": "Linux", "Darwin": "macOS", "Windows_NT": "Windows"}[os_name],
            "runner.arch": {"x64": "X64", "arm64": "ARM64"}[arch]}


def expand(template: str, inputs: dict[str, str], plat: str) -> str:
    """A key with its expressions replaced by values of the shape they'd have."""
    context = runner_context(plat)

    def value(m: re.Match) -> str:
        expr = m.group(1).strip()
        if expr.startswith("hashFiles("):
            return "0" * 64
        if expr in context:
            return context[expr]
        if expr.startswith("inputs."):
            return inputs.get(expr[len("inputs."):], "")
        return "0" * 40  # a step's output (a git revision) or anything else

    return re.sub(r"\$\{\{(.*?)\}\}", value, template)


def rust_cache_key(with_: dict[str, str], job: str | None, inputs: dict[str, str], plat: str) -> str:
    """rust-cache's key, with placeholder hashes."""
    w = {k: expand(v, inputs, plat) for k, v in with_.items()}
    key = w.get("prefix-key") or "v0-rust"
    if w.get("shared-key"):
        key += f"-{w['shared-key']}"
    else:
        if w.get("key"):
            key += f"-{w['key']}"
        if job and w.get("add-job-id-key", "true").lower() == "true":
            key += f"-{job}"
    return f"{key}-{plat}-00000000-00000000"


def unwrap(expr: str) -> str:
    expr = " ".join(expr.split())
    m = re.fullmatch(r"\$\{\{\s*(.*?)\s*\}\}", expr)
    return m.group(1) if m else expr


# The conditions that let a step save, and the ref each one saves from. Anything else is refused.
REF_GATES = {
    "github.ref == 'refs/heads/main'": MAIN,
    "env.SAVE_CACHES == 'true'": "env.SAVE_CACHES",
}
# SAVE_CACHES (zgx-tests.yml on zeron/zgx): the ref it names. (`&& inputs.integration-trusted`:
# only while zeron/zgx is protected, plan.py's integration_trusted; it narrows, never widens.)
SAVE_CACHES_VALUES = {
    "github.ref == 'refs/heads/zeron/zgx'": ZGX,
    "github.ref == 'refs/heads/zeron/zgx' && inputs.integration-trusted": ZGX,
    "github.ref == 'refs/heads/main'": MAIN,
}
TOGGLE = re.compile(r"^inputs\.([\w-]+) == 'true'$")
# rust-cache's save-if, whole: false; main only; zgx's SAVE_CACHES; and, in a composite action,
# an input (save-rust-cache) and main.
SAVE_IF_FORMS = (
    "false",
    "${{ github.ref == 'refs/heads/main' }}",
    "${{ env.SAVE_CACHES == 'true' }}",
    "${{ inputs.<name> == 'true' && github.ref == 'refs/heads/main' }} (composite actions)",
)


@dataclass
class Gate:
    ref: str | None  # None: never saves
    toggle: str | None = None  # a composite action's input that turns it on
    error: str | None = None


def gate(expr: str | None, composite: bool, env_saves: str | None, step_if: bool) -> Gate:
    """The ref a save condition (rust-cache's save-if, or actions/cache/save's if) saves from."""
    if expr is None:
        return Gate(None, error="no condition: it saves from every ref, pull requests and tags included")
    inner = unwrap(expr)
    if inner == "false" and not step_if:
        return Gate(None)
    if "||" in inner:
        return Gate(None, error=f"`{expr}`: an `||` can open it to other refs")
    conjuncts = [c.strip() for c in inner.split("&&")]
    refs = [REF_GATES[c] for c in conjuncts if c in REF_GATES]
    toggles = [TOGGLE.match(c).group(1) for c in conjuncts if TOGGLE.match(c)]
    if not step_if:
        # rust-cache's save-if: one of the forms, whole.
        allowed = {"github.ref == 'refs/heads/main'", "env.SAVE_CACHES == 'true'"}
        if not (len(conjuncts) == 1 and conjuncts[0] in allowed) and not (
                composite and len(conjuncts) == 2 and toggles and refs == [MAIN]):
            return Gate(None, error=f"save-if `{expr}` is none of: " + "; ".join(SAVE_IF_FORMS))
    if len(refs) != 1:
        return Gate(None, error=f"`{expr}` doesn't limit it to one ref: add `github.ref == 'refs/heads/main'`"
                                " (or zgx's `env.SAVE_CACHES == 'true'`)")
    ref = refs[0]
    if ref == "env.SAVE_CACHES":
        if env_saves is None:
            return Gate(None, error="env.SAVE_CACHES isn't defined as "
                                    "`${{ github.ref == 'refs/heads/zeron/zgx' }}` in this workflow")
        ref = env_saves
    if toggles and not composite:
        toggles = []
    return Gate(ref, toggles[0] if toggles else None)


def save_caches_env(path: str, text: str, errors: list[str]) -> str | None:
    values = set()
    for n, line in enumerate(text.splitlines(), 1):
        m = re.match(r"^\s*SAVE_CACHES:\s*(.+?)\s*$", line)
        if m:
            value = unwrap(scalar(m.group(1)) if not m.group(1).startswith("${{") else m.group(1))
            if value not in SAVE_CACHES_VALUES:
                errors.append(f"{path}:{n}: SAVE_CACHES must be `${{{{ github.ref == 'refs/heads/zeron/zgx' }}}}` "
                              f"(or main's), not `{m.group(1)}`")
            else:
                values.add(SAVE_CACHES_VALUES[value])
    return values.pop() if len(values) == 1 else None


def is_pass_record(step: Step) -> bool:
    return (step.with_.get("path", "").strip() == "rust-toolchain.toml"
            and re.fullmatch(r"\$\{\{\s*inputs\.pass-key\s*\}\}-[a-z0-9-]+", step.with_.get("key", "")) is not None)


def action_name(uses: str) -> str:
    return re.split(r"\s+#", uses, maxsplit=1)[0].strip()


@dataclass
class Saver:
    """A step that saves a cache: its key template and the condition it saves under."""
    step: Step
    kind: str  # rust-cache | cache/save
    gate: Gate


def check_key(key: str, ref: str, where: str, what: str, errors: list[str]):
    family = family_of(key)
    if family is None or family is PASS:
        errors.append(f"{where}: {what} saves `{key}`, which no cache family declares. {DECLARE}")
    elif not family.allows(ref):
        errors.append(f"{where}: {what} saves {family.name} (`{key}`) from {ref}, but only "
                      f"{', '.join(family.refs)} may save it. {DECLARE}")


def lint_file(path: str, text: str, composite: bool, errors: list[str]) -> list[Saver]:
    """Checks one workflow or action; returns its saving steps (for an action's callers)."""
    env_saves = save_caches_env(path, text, errors)
    savers = []
    for step in parse_steps(path, text):
        name = action_name(step.uses)
        version = re.search(r"(?:@v|#\s*v)(\d+)", step.uses_raw)
        major = int(version.group(1)) if version else None
        if re.fullmatch(r"actions/cache@\S+", name):
            errors.append(f"{step.where}: actions/cache saves after every job, from every ref. Use "
                          "actions/cache/restore, and actions/cache/save with an `if:` that limits it to the "
                          f"refs its family allows. {DECLARE}")
        elif name.startswith("actions/cache/save@"):
            if is_pass_record(step):
                continue
            g = gate(step.keys.get("if"), composite, env_saves, step_if=True)
            if g.error:
                errors.append(f"{step.where}: actions/cache/save {g.error}. {DECLARE}")
            else:
                savers.append(Saver(step, "cache/save", g))
        elif name.startswith("Swatinem/rust-cache@"):
            if "save-if" not in step.with_:
                errors.append(f"{step.where}: rust-cache without `save-if` saves from every ref (pull requests, "
                              f"tags). {DECLARE}")
                continue
            g = gate(step.with_["save-if"], composite, env_saves, step_if=False)
            if g.error:
                errors.append(f"{step.where}: {g.error}. {DECLARE}")
            elif g.ref is not None:
                savers.append(Saver(step, "rust-cache", g))
        elif re.match(r"actions/setup-(node|python|java|go|dotnet)@", name):
            tool = name.split("@")[0]
            if "cache" in step.with_ and step.with_["cache"].lower() not in ("false", ""):
                errors.append(f"{step.where}: {tool}'s `cache:` saves its own cache from every ref. Remove it. "
                              f"{DECLARE}")
            if tool == "actions/setup-go" and step.with_.get("cache", "").lower() != "false":
                errors.append(f"{step.where}: actions/setup-go caches by default: add `cache: false`.")
            if tool == "actions/setup-node" and (major is None or major >= 5) \
                    and step.with_.get("package-manager-cache", "").lower() != "false":
                errors.append(f"{step.where}: actions/setup-node v5+ caches npm by default: add "
                              "`package-manager-cache: false`.")
        elif name.startswith("mlugg/setup-zig@"):
            if step.with_.get("use-cache", "").lower() != "false":
                errors.append(f"{step.where}: mlugg/setup-zig saves its own cache from every ref: add "
                              f"`use-cache: false`. {DECLARE}")
        if not composite:
            for saver in savers:
                if saver.step is step:
                    check_saver(saver, step.job, step.runs_on, {}, errors)
    return savers


def check_saver(saver: Saver, job: str | None, runs_on: str | None, inputs: dict[str, str],
                errors: list[str], where: str | None = None):
    where = where or saver.step.where
    plat = platform(runs_on)
    if plat is None:
        errors.append(f"{where}: can't tell which runner `runs-on: {runs_on}` is, so not which cache this "
                      "saves: give the saving job a fixed runner label.")
        return
    if saver.kind == "rust-cache":
        key = rust_cache_key(saver.step.with_, job, inputs, plat)
    else:
        key = expand(saver.step.with_.get("key", ""), inputs, plat)
    check_key(key, saver.gate.ref, where, saver.kind, errors)


def lint(root: str | os.PathLike) -> list[str]:
    """Every cache policy violation in the workflows and composite actions under `root`."""
    root = pathlib.Path(root)
    if not (root / ".github" / "workflows").is_dir():
        raise FileNotFoundError(f"no .github/workflows under {root}")
    errors: list[str] = []
    actions: dict[str, tuple[dict[str, str], list[Saver]]] = {}
    for action in sorted((root / ".github" / "actions").glob("*/action.y*ml")):
        rel = action.relative_to(root).as_posix()
        text = action.read_text(encoding="utf-8")
        actions[f"./.github/actions/{action.parent.name}"] = (action_inputs(text), lint_file(rel, text, True, errors))
    for workflow in sorted((root / ".github" / "workflows").glob("*.y*ml")):
        rel = workflow.relative_to(root).as_posix()
        text = workflow.read_text(encoding="utf-8")
        lint_file(rel, text, False, errors)
        for step in parse_steps(rel, text):
            name = action_name(step.uses)
            if name not in actions:
                continue
            defaults, savers = actions[name]
            inputs = {**defaults, **step.with_}
            for saver in savers:
                if saver.gate.toggle is not None and inputs.get(saver.gate.toggle, "").strip().lower() in ("", "false"):
                    continue  # this caller leaves it off
                check_saver(saver, step.job, step.runs_on, inputs, errors,
                            where=f"{step.where} via {saver.step.where}")
    return errors


# --- Command line --------------------------------------------------------------------------------

def families_table() -> str:
    rows = ["| Priority | Family | Saved from | Size | Value | Saved by |", "|---|---|---|---|---|---|"]
    for f in FAMILIES:
        refs = ", ".join(r.removeprefix("refs/heads/") for r in f.refs) if f.refs else "any"
        rows.append(f"| {f.rank} | {f.name} | {refs} | {f.size_gb:.2f} GB | {f.value or ''} | {f.saved_by} |")
    total = sum(f.size_gb for f in FAMILIES)
    rows.append(f"| | total | | {total:.2f} GB (budget {BUDGET / GB:.1f}) | | |")
    return "\n".join(rows)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    j = sub.add_parser("janitor", help="delete the caches the policy doesn't keep")
    j.add_argument("--dry-run", action="store_true", help="list what would be deleted, delete nothing")
    j.add_argument("--repo", help="owner/repo (GITHUB_REPOSITORY)")
    j.add_argument("--from-json", help="read the cache list from a file (the API's, or `gh cache list --json "
                                       "id,key,ref,sizeInBytes,createdAt,lastAccessedAt`); implies --dry-run")
    l = sub.add_parser("lint", help="check the workflows against the policy")
    l.add_argument("root", nargs="?", default=str(pathlib.Path(__file__).resolve().parents[2]))
    sub.add_parser("families", help="print the declared families")
    args = parser.parse_args(argv)
    if args.command == "janitor":
        return janitor(args)
    if args.command == "lint":
        errors = lint(args.root)
        print("\n".join(errors) or "No cache policy violations.")
        return 1 if errors else 0
    print(families_table())
    return 0


if __name__ == "__main__":
    sys.exit(main())
