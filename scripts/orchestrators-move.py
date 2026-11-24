#!/usr/bin/env -S python3 -I
"""Move Orchestrators' UI out of apps/zgx/ui into its built-in plugin.

    python3 -I scripts/orchestrators-move.py [--plan] [--base REF]

Moves (`git mv`) the orchestrator-only Luau into
apps/zgx/plugins-builtin/orchestrators: its modules into app/ (keeping their
layout under ui/), its fixtures, worlds and Luau tests into tests/. Then it
rewrites requires so each still reaches what it did, flips the app's
stand-in (ui/builtin/orchestrators.luau) to `false`, and points the plugin's
index.luau parts at `./app/…`.

It finds the files by rules, not a list, so it can run on whatever tip it's
given, and again on its own result (nothing left to move, nothing to
rewrite). It prints what it computed. `--plan` prints it and changes nothing.

What moves:
  modules   screens/control/**, screens/orchestrator/**, ds/orch_*,
            account/{orchestration,orchestration_sample},
            screens/settings/orchestration, demos/orch*; and, only when
            nothing staying in ui/ requires them, ds/{hold,kept_scroll,stage},
            account/{rows,scratchpad}, screens/chat/card_hold
  fixtures  a file at ui/'s top level (a fixture, a world, a stub) that main
            (the base) doesn't have and that requires a moving module or a
            moving fixture, or that only moving tests and fixtures mount
  tests     a ui/tests/*.test.luau main doesn't have that mounts or requires
            a moving fixture or module, or is named as Orchestrators' own
            (control_*, orch_*, orchestrator_*, sidebar_orch*, sidebar_crew*,
            settings_orch*, progress_embed, crew_names, chat_card_hold): such
            a one may reach the plugin only through `plugins.module`, which
            only the plugin's test stage loads

How requires are written after the move:
  in app/   another moved module: relative (`./…`, `../…`; `@self/…` kept);
            the app's: `@app/…`, `@ds/…`, `@account/…`, `@demos/…`
  in tests/ a moved module: `@orchestrators/…` (the alias the test stage and
            tests/.luaurc give, naming app/); a moved fixture from a fixture:
            `./…`; the app's fixtures and modules: `@app/…` and the aliases

Outside the Luau: code, tests and CI (apps/zgx/src, apps/zgx/tests,
apps/zgx/tools, scripts/, .github/) may not name a moving file by its ui/
path (`ui/ds/orch_board.luau`, `join("ui/orch_push_fixture.luau")`), and a
Rust file naming a moving fixture alone must mount it through
`crate::fixture_tests` (src/fixture_tests.rs), which finds a fixture in ui/
or stages it from its plugin's tests/ as scripts/ci/test-zgx.sh does. Docs
and comments that do are printed as notes.

It fails, naming each, on anything that would break: a file staying in ui/
that requires (or names, as a module path or a mounted fixture) a moving
one; a main test or fixture that depends on orchestrator code; a moved
module that requires a fixture; a module path a moved file hands to the
app's code as text (it can't be made relative); code outside the Luau
naming a moving file as above; a require that doesn't resolve after the
move.
"""

from __future__ import annotations

import argparse
import json
import os
import posixpath
import re
import subprocess
import sys

UI = "apps/zgx/ui"
PLUGIN = "apps/zgx/plugins-builtin/orchestrators"
APP = f"{PLUGIN}/app"
TESTS = f"{PLUGIN}/tests"
STAND_IN = "builtin/orchestrators.luau"  # ui-relative
ALIAS = "orchestrators"  # tests' alias for app/
OWN = "@plugin"  # app/'s own top-level files, as virtual paths

# ui/.luaurc's aliases (checked against the file below).
ALIASES = {"ds": "ds", "screens": "screens", "demos": "demos", "account": "account"}

CORE = [
    re.compile(r"^screens/control/.+\.luau$"),
    re.compile(r"^screens/orchestrator/.+\.luau$"),
    re.compile(r"^ds/orch_[^/]+\.luau$"),
    re.compile(r"^account/(orchestration|orchestration_sample)\.luau$"),
    re.compile(r"^screens/settings/orchestration\.luau$"),
    re.compile(r"^demos/orch[^/]*\.luau$"),
]
# Moved only when everything requiring them moves too.
VERIFY = [
    "ds/hold.luau",
    "ds/kept_scroll.luau",
    "ds/stage.luau",
    "account/rows.luau",
    "account/scratchpad.luau",
    "screens/chat/card_hold.luau",
]
# Test names the brief expects to move: printed against what's derived.
EXPECTED_TESTS = re.compile(
    r"^(control_|orch_|orchestrator_|sidebar_orch|sidebar_crew|settings_orch|"
    r"progress_embed|crew_names|chat_card_hold)"
)

PATHISH = re.compile(r"^(@[A-Za-z][\w-]*(/[\w./-]*)?|\.\.?/[\w./-]+)$")
REQUIRE_BEFORE = re.compile(r"require\s*(?:::\s*any\s*\)\s*)?\(\s*$|require\s*$")


def run(*args: str, check: bool = True) -> str:
    out = subprocess.run(args, capture_output=True, text=True)
    if check and out.returncode != 0:
        sys.exit(f"{' '.join(args)}: {out.stderr.strip()}")
    return out.stdout


# ---------------------------------------------------------------- lexing


def strings(code: str):
    """Each string literal's (start, end, quote, value): start/end span the
    literal's content (without quotes). Comments are skipped."""
    i, n = 0, len(code)
    while i < n:
        c = code[i]
        if c == "-" and code.startswith("--", i):
            m = re.match(r"--\[(=*)\[", code[i:])
            if m:
                close = "]" + m.group(1) + "]"
                j = code.find(close, i + m.end())
                i = n if j < 0 else j + len(close)
            else:
                j = code.find("\n", i)
                i = n if j < 0 else j
            continue
        if c == "[":
            m = re.match(r"\[(=*)\[", code[i:])
            if m:
                close = "]" + m.group(1) + "]"
                j = code.find(close, i + m.end())
                i = n if j < 0 else j + len(close)
                continue
        if c in "\"'`":
            j = i + 1
            while j < n and code[j] != c:
                if code[j] == "\\":
                    j += 1
                elif code[j] == "\n" and c != "`":
                    break
                j += 1
            if c != "`":
                yield (i + 1, j, c, code[i + 1 : j])
            i = j + 1
            continue
        i += 1


# ------------------------------------------------------------ the tree


class Tree:
    """The ui/ tree as it would be with nothing moved ("virtual" paths,
    ui-relative), and where each file is on disk now."""

    def __init__(self, root: str):
        self.root = root
        self.where: dict[str, str] = {}  # virtual -> repo path
        for path in run("git", "-C", root, "ls-files", "--cached", "--others",
                        "--exclude-standard", UI, PLUGIN).splitlines():
            if not path.endswith(".luau") or not os.path.exists(os.path.join(root, path)):
                continue
            v = self.virtual(path)
            if v is None:
                continue
            if v in self.where and self.where[v] != path:
                sys.exit(f"{v}: both {self.where[v]} and {path}")
            self.where[v] = path

    @staticmethod
    def virtual(path: str) -> str | None:
        if path.startswith(UI + "/"):
            return path[len(UI) + 1 :]
        if path.startswith(APP + "/"):
            rest = path[len(APP) + 1 :]
            # Its own top-level files (none of ui/'s): a folder ui/ hasn't.
            return rest if "/" in rest else f"{OWN}/{rest}"
        if path.startswith(TESTS + "/"):
            name = path[len(TESTS) + 1 :]
            if "/" in name:
                return None
            return f"tests/{name}" if name.endswith(".test.luau") else name
        return None

    def read(self, v: str) -> str:
        with open(os.path.join(self.root, self.where[v]), encoding="utf-8") as f:
            return f.read()

    def in_plugin(self, v: str) -> bool:
        return not self.where[v].startswith(UI + "/")

    def resolve(self, frm: str, s: str) -> str | None:
        """The virtual file `s` names, required from virtual `frm`."""
        here = posixpath.dirname(frm)
        if s.startswith("@"):
            alias, _, rest = s[1:].partition("/")
            if alias in ("app", "app-original", ALIAS):
                joined = rest
            elif alias == "self":
                joined = posixpath.join(here, rest)
            elif alias in ALIASES:
                joined = posixpath.join(ALIASES[alias], rest)
            else:
                return None
        elif s.startswith("./") or s.startswith("../"):
            base = here
            if posixpath.basename(frm) == "init.luau":
                base = posixpath.dirname(here)
            joined = posixpath.join(base, s)
        else:
            return None
        joined = posixpath.normpath(joined).lstrip("/")
        if joined.startswith(".."):
            return None
        for cand in (joined + "/init.luau", joined if joined.endswith(".luau") else joined + ".luau"):
            if cand in self.where:
                return cand
        return None


def is_root_file(v: str) -> bool:
    return "/" not in v


def is_test(v: str) -> bool:
    return v.startswith("tests/") and v.endswith(".test.luau")


def is_module(v: str) -> bool:
    return not is_root_file(v) and not v.startswith("tests/") and not v.startswith("templates/")


# ----------------------------------------------------------- references


class Ref:
    __slots__ = ("start", "end", "text", "target", "kind")

    def __init__(self, start, end, text, target, kind):
        self.start, self.end, self.text, self.target, self.kind = start, end, text, target, kind


def refs(tree: Tree, v: str, code: str | None = None) -> list[Ref]:
    """What file `v` names: requires, module paths as text, mounted fixtures."""
    code = tree.read(v) if code is None else code
    out = []
    for start, end, _q, s in strings(code):
        if PATHISH.match(s):
            target = tree.resolve(v, s)
            if target is None:
                continue
            before = code[max(0, start - 40) : start - 1]
            kind = "require" if REQUIRE_BEFORE.search(before) else "text"
            out.append(Ref(start, end, s, target, kind))
        elif s.endswith(".luau") and "/" not in s and s in tree.where:
            out.append(Ref(start, end, s, s, "mount"))
    return out


# ----------------------------------------------------------- planning


def base_ref(root: str, given: str | None) -> str:
    if given:
        return given
    for upstream in ("upstream/main", "origin/main"):
        out = subprocess.run(["git", "-C", root, "merge-base", "HEAD", upstream],
                             capture_output=True, text=True)
        if out.returncode == 0:
            return out.stdout.strip()
    sys.exit("no base: pass --base REF")


def plan(tree: Tree, base: str):
    at_base = {
        p[len(UI) + 1 :]
        for p in run("git", "-C", tree.root, "ls-tree", "-r", "--name-only", base, UI).splitlines()
    }
    graph = {v: refs(tree, v) for v in tree.where}
    required_by: dict[str, set[str]] = {}
    for v, rs in graph.items():
        for r in rs:
            required_by.setdefault(r.target, set()).add(v)

    already = {v for v in tree.where if tree.in_plugin(v)}
    core = {v for v in tree.where if is_module(v) and any(p.match(v) for p in CORE)}
    core |= {v for v in already if is_module(v) and v not in VERIFY}
    verify = {v for v in VERIFY if v in tree.where}
    standin = {STAND_IN}

    def closure(modules: set[str]):
        fixtures: set[str] = {v for v in already if is_root_file(v)}
        changed = True
        while changed:
            changed = False
            for v in tree.where:
                if not is_root_file(v) or v in fixtures or v in at_base:
                    continue
                if any(r.target in modules or r.target in fixtures for r in graph[v]):
                    fixtures.add(v)
                    changed = True
        tests = {v for v in already if is_test(v)}
        for v in tree.where:
            if is_test(v) and v not in at_base and v not in tests:
                if any(r.target in modules or r.target in fixtures for r in graph[v]):
                    tests.add(v)
                # Named as Orchestrators' own: it may reach the plugin only
                # through `plugins.module("orchestrators")` (a command of a
                # fixture that stays), which only the plugin's stage loads.
                elif EXPECTED_TESTS.match(posixpath.basename(v)):
                    tests.add(v)
        # A fixture of the branch's that only moving tests and fixtures
        # mount: theirs, though it reaches the plugin by plugins.module.
        changed = True
        while changed:
            changed = False
            for v in tree.where:
                if not is_root_file(v) or v in fixtures or v in at_base:
                    continue
                users = required_by.get(v, set())
                if users and users <= (fixtures | tests):
                    fixtures.add(v)
                    changed = True
        return fixtures, tests

    accepted = set(verify)
    left: dict[str, list[str]] = {}
    while True:
        modules = core | accepted
        fixtures, tests = closure(modules)
        moving = modules | fixtures | tests
        dropped = None
        for v in sorted(accepted):
            if v in already:
                continue
            outside = sorted(u for u in required_by.get(v, ()) if u not in moving and u not in standin)
            if outside:
                left[v] = outside
                dropped = v
                break
        if dropped is None:
            break
        accepted.discard(dropped)

    modules = core | accepted
    fixtures, tests = closure(modules)
    moving = modules | fixtures | tests

    problems: list[str] = []
    for v in sorted(tree.where):
        if v in moving or v in standin:
            continue
        for r in graph[v]:
            if r.target in moving:
                why = "in main" if v in at_base else "new, stays in ui/"
                problems.append(f"{v} ({why}) {r.kind}s {r.text!r} -> {r.target}")
    for v in sorted(modules):
        for r in graph[v]:
            if is_root_file(r.target) and r.kind != "mount":
                problems.append(f"{v} (a moving module) {r.kind}s the fixture {r.target}")
    return modules, fixtures, tests, left, problems, graph, at_base


# ------------------------------------------------- outside the Luau

OUTSIDE = ["apps/zgx/src", "apps/zgx/tests", "apps/zgx/tools", "scripts", ".github"]
SELF = "scripts/orchestrators-move.py"
FIXTURE_HELPER = "fixture_tests::"
COMMENT = re.compile(r"^\s*(//|#|--|\*|/\*)")


def outside(root: str, moving: set[str]) -> tuple[list[str], list[str]]:
    """Code, tests and CI outside the Luau that name a moving file by its
    ui/ path, or (Rust) a moving fixture without the helper that finds it:
    problems; docs and comments that do: notes."""
    by_path = {f"ui/{v}": v for v in moving if not v.startswith(OWN + "/")}
    fixtures = {v for v in moving if is_root_file(v)}
    paths = re.compile("|".join(re.escape(p) for p in sorted(by_path, key=len, reverse=True)) or "$^")
    names = re.compile(r"(?<![\w/])(" + "|".join(re.escape(v) for v in sorted(fixtures)) + r")\b"
                       if fixtures else "$^")
    problems: list[str] = []
    notes: list[str] = []
    listed = run("git", "-C", root, "ls-files", "--cached", "--others", "--exclude-standard",
                 "--", *OUTSIDE, "apps/zgx/docs").splitlines()
    for rel in listed:
        if rel == SELF or not os.path.isfile(os.path.join(root, rel)):
            continue
        try:
            with open(os.path.join(root, rel), encoding="utf-8") as f:
                text = f.read()
        except (UnicodeDecodeError, OSError):
            continue
        doc = rel.endswith(".md") or rel.startswith("apps/zgx/docs/")
        helped = FIXTURE_HELPER in text or rel == "apps/zgx/src/fixture_tests.rs"
        for n, line in enumerate(text.splitlines(), 1):
            found = [m.group(0) for m in paths.finditer(line)]
            bare = [] if not rel.endswith(".rs") else [m.group(1) for m in names.finditer(line)]
            if not found and not bare:
                continue
            where = f"{rel}:{n}"
            if doc or COMMENT.match(line):
                for name in found:
                    notes.append(f"{where}: names {name} (moving), in a doc or comment")
                continue
            for name in found:
                problems.append(f"{where}: names {name}, which moves to {dest(by_path[name])}")
            if not helped:
                for name in bare:
                    problems.append(f"{where}: mounts {name}, which moves to {dest(name)}: "
                                    f"mount it through crate::{FIXTURE_HELPER.rstrip(':')}")
    return problems, notes


# ----------------------------------------------------------- rewriting


def dest(v: str) -> str:
    if v.startswith(OWN + "/"):
        return f"{APP}/{v[len(OWN) + 1:]}"
    if is_test(v):
        return f"{TESTS}/{v[len('tests/'):]}"
    if is_root_file(v):
        return f"{TESTS}/{v}"
    return f"{APP}/{v}"


def module_name(v: str) -> str:
    v = v[: -len(".luau")]
    return v[: -len("/init")] if v.endswith("/init") else v


def app_form(v: str) -> str:
    name = module_name(v)
    head, _, rest = name.partition("/")
    for alias, folder in ALIASES.items():
        if folder == head and alias != "screens":
            return f"@{alias}/{rest}" if rest else f"@{alias}"
    return f"@app/{name}"


def relative(frm: str, v: str) -> str:
    here = posixpath.dirname(frm)
    if posixpath.basename(frm) == "init.luau":
        here = posixpath.dirname(here)
    rel = posixpath.relpath(module_name(v), here or ".")
    return rel if rel.startswith("../") else "./" + rel


def rewrite(tree: Tree, v: str, moving: set[str], problems: list[str]) -> str | None:
    """File `v`'s text with its requires written for where it now is, or
    None when nothing changes."""
    code = tree.read(v)
    edits = []
    in_tests = is_root_file(v) or v.startswith("tests/")
    for r in refs(tree, v, code):
        t, s = r.target, r.text
        if r.kind == "mount":
            if in_tests and is_test(v):
                continue  # fixtures sit beside app.luau in the stage
            continue
        absolute = s.startswith("@") and not s.startswith("@self")
        if not in_tests:
            if t in moving:
                if is_root_file(t) or t.startswith("tests/"):
                    problems.append(f"{v} requires the moved fixture {t}")
                    continue
                if s.startswith("@self") or not absolute:
                    continue  # already relative, and app/ keeps ui/'s layout
                new = relative(v, t)
            else:
                if absolute and not s.startswith(f"@{ALIAS}"):
                    continue
                new = app_form(t)
            if r.kind == "text":
                problems.append(f"{v} hands the module path {s!r} ({t}) to the app as text: "
                                f"it can't be made relative there")
                continue
        else:
            if t in moving and is_module(t):
                new = f"@{ALIAS}/{module_name(t)}"
            elif t in moving:
                if is_test(v):
                    problems.append(f"{v} requires the moved fixture {t}: a test can't reach "
                                    f"it by one path in tests/ and in the stage alike")
                    continue
                new = relative(v, t)
            elif is_root_file(t):
                new = f"@app/{module_name(t)}"
            else:
                if absolute and not s.startswith(f"@{ALIAS}"):
                    continue
                new = app_form(t)
        if new != s:
            edits.append((r.start, r.end, new))
    if not edits:
        return None
    for start, end, new in sorted(edits, reverse=True):
        code = code[:start] + new + code[end:]
    return code


STAND_IN_TEXT = """\
-- Orchestrators' app-side module as the app has it with the plugin off:
-- nothing. The built-in plugin (plugins-builtin/orchestrators) puts its own
-- in this one's place while it's on (`ui.override`, a bundled built-in's
-- trust: docs/plugins.md, "Zeron's own plugins"). Reach it through
-- `require("@account/plugins").module("orchestrators")`, never directly.
return false
"""

PART = re.compile(r'^(\s*[A-Za-z_][\w]*\s*=\s*")([^"]+)(",)', re.M)


def rewrite_index(tree: Tree, code: str, moving: set[str], notes: list[str]) -> str:
    def part(m: re.Match) -> str:
        s = m.group(2)
        if s.startswith("./"):
            return m.group(0)
        t = tree.resolve("index.luau", s) if s.startswith("@") else None
        if t is None:
            notes.append(f"index.luau: part {s!r} names no module")
            return m.group(0)
        if t not in moving:
            notes.append(f"index.luau: part {s!r} stays the app's ({t})")
            return m.group(0)
        return m.group(1) + "./app/" + module_name(t) + m.group(3)

    code = PART.sub(part, code)
    # Its header, once every part is app/'s.
    if all(m.group(2).startswith("./app/") for m in PART.finditer(code)):
        for before, after in INDEX_HEADER:
            code = code.replace(before, after)
    return code


INDEX_HEADER = [
    ("-- (init.luau). The same parts as the app's own stand-in\n"
     "-- (ui/builtin/orchestrators.luau) while their modules are still in ui/.\n",
     "-- (init.luau). With the plugin off, the app has its own stand-in\n"
     "-- (ui/builtin/orchestrators.luau), which is `false`.\n"),
    ("-- One part a line (`name = \"module\",`): moving the modules into app/\n"
     "-- rewrites these paths to `./app/…`.\n",
     "-- One part a line (`name = \"module\",`), each a module in app/.\n"),
]


# The tests/ folder's .luaurc, for `zgx check` alone: luau-lsp resolves the
# tests' `@app/…` and the ui aliases against ui/, and `@orchestrators/…`
# against app/. zgx's own runs never read it (the test stage gives the
# alias; scripts/ci/test-zgx.sh).
def plugin_luaurc() -> str:
    ui = "../../../ui"
    aliases = {"app": ui, ALIAS: "../app"}
    aliases.update({a: f"{ui}/{d}" for a, d in ALIASES.items()})
    return json.dumps({"languageMode": "strict", "aliases": aliases}, indent="\t") + "\n"


def app_luaurc() -> str:
    ui = "../../../ui"
    aliases = {"app": ui}
    aliases.update({a: f"{ui}/{d}" for a, d in ALIASES.items()})
    return json.dumps({"languageMode": "strict", "aliases": aliases}, indent="\t") + "\n"


# ----------------------------------------------------------- main


def show(title: str, items) -> None:
    items = sorted(items)
    print(f"{title} ({len(items)}):")
    for item in items:
        print(f"  {item}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--plan", action="store_true", help="print what it would do, change nothing")
    ap.add_argument("--base", help="main's commit (default: merge-base with upstream/main)")
    args = ap.parse_args()
    root = run("git", "rev-parse", "--show-toplevel").strip()
    os.chdir(root)

    with open(f"{UI}/.luaurc", encoding="utf-8") as f:
        have = json.load(f).get("aliases", {})
    for alias, folder in ALIASES.items():
        if have.get(alias, "").strip("./") != folder:
            sys.exit(f"{UI}/.luaurc: alias {alias} isn't ./{folder} any more; update ALIASES")

    base = base_ref(root, args.base)
    tree = Tree(root)
    modules, fixtures, tests, left, problems, graph, at_base = plan(tree, base)
    moving = modules | fixtures | tests

    print(f"base {base[:12]}")
    show("modules -> app/", modules)
    show("fixtures and worlds -> tests/", fixtures)
    show("tests -> tests/", tests)
    for v, by in sorted(left.items()):
        print(f"stays in ui/ (required from outside orchestrator code): {v}")
        for u in by:
            print(f"    by {u}")
    names = {posixpath.basename(t)[: -len('.test.luau')] for t in tests}
    expected = {posixpath.basename(v)[: -len('.test.luau')]
                for v in tree.where if is_test(v) and EXPECTED_TESTS.match(posixpath.basename(v))}
    expected |= {n for n in names if EXPECTED_TESTS.match(n)}
    if names - expected:
        show("note: moving tests with other names (they mount orchestrator fixtures)", names - expected)
    # Modules new on the branch that only moving code requires: candidates
    # for the rules, not moved by them.
    lonely = []
    for v in tree.where:
        if v in moving or not is_module(v) or v.startswith(f"{STAND_IN}") or f"{v}" in at_base:
            continue
        users = {u for u, rs in graph.items() for r in rs if r.target == v}
        if users and users <= moving:
            lonely.append(v)
    if lonely:
        show("note: new modules only moving code requires (left in ui/)", lonely)

    if problems:
        show("FAIL: general code would reach moved files", problems)
        return 1
    elsewhere, notes = outside(root, moving)
    for n in notes:
        print(f"note: {n}")
    if elsewhere:
        show("FAIL: code, tests or CI outside the Luau name moving files", elsewhere)
        return 1

    # Each file's new text, computed before anything moves.
    texts: dict[str, str] = {}
    for v in sorted(moving):
        new = rewrite(tree, v, moving, problems)
        if new is not None:
            texts[v] = new
    if problems:
        show("FAIL: requires that can't be written for the plugin", problems)
        return 1
    show("requires rewritten in", texts)

    notes: list[str] = []
    index_path = f"{PLUGIN}/index.luau"
    with open(index_path, encoding="utf-8") as f:
        index = f.read()
    new_index = rewrite_index(tree, index, moving, notes)
    for n in notes:
        print(f"note: {n}")

    if args.plan:
        print("--plan: nothing changed")
        return 0

    touched = []
    for v in sorted(moving):
        src, dst = tree.where[v], dest(v)
        if src != dst:
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            run("git", "mv", src, dst)
        if v in texts:
            with open(dst, "w", encoding="utf-8") as f:
                f.write(texts[v])
        touched.append(dst)
    if new_index != index:
        with open(index_path, "w", encoding="utf-8") as f:
            f.write(new_index)
        touched.append(index_path)
    stand_in = f"{UI}/{STAND_IN}"
    with open(stand_in, encoding="utf-8") as f:
        if f.read() != STAND_IN_TEXT:
            with open(stand_in, "w", encoding="utf-8") as g:
                g.write(STAND_IN_TEXT)
    touched.append(stand_in)
    for path, text in ((f"{TESTS}/.luaurc", plugin_luaurc()), (f"{APP}/.luaurc", app_luaurc())):
        old = open(path, encoding="utf-8").read() if os.path.exists(path) else None
        if old != text:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
        touched.append(path)
    run("git", "add", "--", *touched)

    # What it left: every require of a plugin file resolves, on disk, as
    # zgx resolves it (app/ beside ui/, tests/ in the stage), and nothing in
    # ui/ names a plugin file.
    after = Tree(root)
    broken = verify(after)
    if broken:
        show("FAIL: after the move", broken)
        return 1
    print("moved; requires resolve")
    return 0


def verify(tree: Tree) -> list[str]:
    out = []
    plugin = {v for v in tree.where if tree.in_plugin(v)}
    for v in sorted(tree.where):
        rs = refs(tree, v)
        for r in rs:
            if v in plugin:
                path = tree.where[v]
                s = r.text
                if r.kind == "mount":
                    continue
                if path.startswith(APP + "/") and s.startswith(f"@{ALIAS}"):
                    out.append(f"{path}: {s!r}: app/ has no @{ALIAS} alias at run time")
                if path.startswith(TESTS + "/") and (s.startswith("./") or s.startswith("../")) \
                        and is_module(r.target):
                    out.append(f"{path}: {s!r}: relative from tests/ to a module")
                if path.startswith(APP + "/") and is_root_file(r.target):
                    out.append(f"{path}: {s!r}: a module requiring a fixture")
                # On disk: a relative require from app/ must land in the plugin.
                if path.startswith(APP + "/") and (s.startswith("./") or s.startswith("../") or s.startswith("@self")):
                    if not tree.in_plugin(r.target):
                        out.append(f"{path}: {s!r} -> {r.target}, which is the app's")
                if not s.startswith("@") or s.startswith("@self"):
                    pass
                elif tree.in_plugin(r.target) and not s.startswith(f"@{ALIAS}"):
                    out.append(f"{path}: {s!r} -> {r.target}, a plugin file by the app's path")
            elif v != STAND_IN and r.target in plugin:
                out.append(f"{tree.where[v]}: {r.kind} {r.text!r} -> {tree.where[r.target]}")
    # Unresolved requires in plugin files (anything path-like that names nothing).
    for v in sorted(plugin):
        code = tree.read(v)
        for start, _e, _q, s in strings(code):
            if PATHISH.match(s) and REQUIRE_BEFORE.search(code[max(0, start - 40) : start - 1]):
                if tree.resolve(v, s) is None:
                    out.append(f"{tree.where[v]}: require({s!r}) names no file")
    return out


if __name__ == "__main__":
    sys.exit(main())
