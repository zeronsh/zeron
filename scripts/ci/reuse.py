"""Skip CI jobs that already passed on exactly the same inputs.

A job's result depends on the files it builds and reads, not on which commit
they came from. plan.py fingerprints every suite's inputs in the tree being
tested (for a pull request, the merge with the current main) and asks the
Actions cache whether each of the suite's jobs already passed with that
fingerprint. Jobs record a pass as their last step (a tiny cache entry keyed
`<suite key>-<job>`); see the "Record the pass" steps in the suite workflows.

A suite's inputs:

  - COMMON: the workspace manifest and lockfile, the toolchain and ci.yml;
  - SUITE_INPUTS: its workflow, the CI scripts and actions it runs (not the
    planner and scheduler), and the non-Rust files it uses;
  - for Rust suites, the directories of the workspace packages it tests and of
    every workspace package those depend on (Cargo.toml path dependencies, of
    every kind), plus any file outside those directories that their Rust
    sources name with a relative path ("../ui/src/file-icons.json",
    CARGO_MANIFEST_DIR.join("../harness/tests/fixtures")), found by scanning
    the sources on every run.

Git object ids identify the contents, so the fingerprint is the hash of the
sorted (path, object id) list. The ids come from `git ls-tree` on the checked
out commit: no file needs to be read except the Rust sources that are scanned.

Which passes count: a pull request trusts its own earlier runs and main's (a
fork only main's, unless CI_FORK_REUSE=own: its earlier runs could have
forged a pass for the fingerprint of the commit it asks to merge); main
trusts only main's; the merge queue, the last gate before main, reuses
nothing. If a pull request changes a Rust build file no selected suite's
fingerprint covers, nothing is reused either (fail closed).
The `ci:full` label and manual runs reuse nothing.

Two refinements: a suite's gated jobs and optional steps (plan.py's JOB_PATHS
and FLAG_PATHS) are part of its fingerprint, since they change what its jobs
check; and the workspace version (Cargo.toml) and the workspace packages'
versions (Cargo.lock) are left out, since a version bump changes no code.
"""

from __future__ import annotations

import fnmatch
import hashlib
import os
import posixpath
import re
import subprocess
import tomllib

VERSION = "v1"

COMMON = [
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/**", ".github/workflows/ci.yml",
]
# The CI files a suite runs. plan.py, admit.py, reuse.py and emergency.py decide
# which suites run and when, not whether their tests pass, so they are no input
# (a change to what a fingerprint means bumps VERSION instead).
LINUX_SETUP = [".github/actions/linux-ci/**", "scripts/ci/apt-install.sh", "scripts/ci/install-nextest.sh"]
NEXTEST = ["scripts/ci/nextest.toml"]

SUITE_INPUTS: dict[str, list[str]] = {
    "core": [".github/workflows/ui-tests.yml", "scripts/ci/test-core.sh"] + LINUX_SETUP + NEXTEST,
    "ui": [".github/workflows/ui-tests.yml", "scripts/test-linux-browser.sh", "scripts/test-browser-storage.py"]
          + LINUX_SETUP + NEXTEST,
    "cursor": [".github/workflows/cursor-compatibility.yml", "scripts/*cursor-sdk*", "crates/harness/src/cursor/**"],
    "edge": [".github/workflows/preview-tests.yml", "edge/**"],
    "installer": [".github/workflows/linux-installer.yml", "edge/src/install.sh", "scripts/package-linux.sh",
                  "scripts/test-linux-desktop-entry.sh", "dist/zeron.desktop", "dist/zeron.png",
                  "scripts/ci/apt-install.sh"],
    "voice": [".github/workflows/voice-tests.yml", "dist/voice/**", "scripts/package-voice-runtime.py",
              "scripts/codex-voice-remote-smoke.py", "scripts/tests/*voice*"] + LINUX_SETUP + NEXTEST,
    "windows": [".github/workflows/windows.yml", "dist/windows/**", "scripts/test-windows-*.ps1",
                "scripts/package-windows.ps1", ".github/actions/windows-ci-setup/**"] + NEXTEST,
    "macos": [".github/workflows/macos.yml", "edge/**", "scripts/run-macos-browser-fixture.sh",
              "scripts/test-browser-storage.py", "dist/macos/**", "scripts/ci/run-macos-fixture.sh"] + NEXTEST,
    "frame_source": [".github/workflows/macos.yml"],
    "ios": [".github/workflows/macos.yml", "apps/ios/**", "scripts/ios/**"],
    "android": [".github/workflows/android.yml", "apps/android/**", "scripts/android/**",
                "apps/ios/Zeron/Fonts/**", "apps/ios/Zeron/Assets.xcassets/**"],
}

# The workspace packages each suite builds and tests (their dependencies are added).
SUITE_PACKAGES: dict[str, list[str]] = {
    "core": ["zeron-harness", "zeron-engine", "zeron-sync", "zeron-update", "zeron-doc", "zeron-preview",
             "zeron-mcp", "zeron-proto", "zeron-rpc", "zeron-orb", "zeron-voice-session", "zeron-voice-media"],
    "ui": ["zeron-ui"],
    "voice": ["zeron-audio", "zeron-voice-media"],
    "windows": ["zeron-ui", "zeron-harness", "zeron", "zeron-update", "zeron-proto", "zeron-engine",
                "zeron-voice-media"],
    "macos": ["zeron-update", "zeron-preview", "zeron-ui", "zeron-text", "zeron"],
    "ios": ["zeron-mobile", "zeron-text", "zeron-markdown", "zeron-client"],
    "android": ["zeron-mobile"],
}

# A ../ path that names something (a bare "../.." names no file); "/../x" as in
# concat!(env!("CARGO_MANIFEST_DIR"), "/../x") too.
RELATIVE_PATH = re.compile(r'"/?((?:\.\./)+[^"\\\n{}./][^"\\\n{}]*)"')
# Paths the scan cannot follow: the package directory's parent reached in code.
# A suite whose sources do this reads the whole workspace (fail closed).
UNFOLLOWABLE = re.compile(r'CARGO_MANIFEST_DIR[^;]{0,200}?(?:\.parent\(\)|\.ancestors\(\)|join\(\s*"\.\.")')
EVERYTHING = ["crates/**", "apps/**", "scripts/**", "dist/**", "edge/**"]
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def matches(path: str, patterns: list[str]) -> bool:
    for pattern in patterns:
        if pattern.endswith("/**"):
            if path.startswith(pattern[:-2]):
                return True
        elif fnmatch.fnmatchcase(path, pattern):
            return True
    return False


def tree_index(root: str = ".") -> dict[str, str]:
    """path -> git object id for every file of HEAD (submodules: their commit)."""
    out = subprocess.run(["git", "-C", root, "ls-tree", "-r", "-z", "--full-tree", "HEAD"],
                         check=True, capture_output=True).stdout.decode()
    index = {}
    for entry in out.split("\0"):
        if entry:
            meta, path = entry.split("\t", 1)
            index[path] = meta.split()[2]
    return index


class Workspace:
    """Workspace packages and their path dependencies, from the Cargo.toml files."""

    def __init__(self, root: str = "."):
        self.root = root
        with open(os.path.join(root, "Cargo.toml"), "rb") as f:
            manifest = tomllib.load(f)
        workspace = manifest["workspace"]
        shared = workspace.get("dependencies", {})
        self.dirs: dict[str, str] = {}  # package -> directory
        manifests: dict[str, dict] = {}
        for member in workspace["members"]:
            with open(os.path.join(root, member, "Cargo.toml"), "rb") as f:
                data = tomllib.load(f)
            self.dirs[data["package"]["name"]] = member
            manifests[member] = data
        by_dir = {d: name for name, d in self.dirs.items()}
        self.deps: dict[str, set[str]] = {}
        for member, data in manifests.items():
            tables = [data.get(t, {}) for t in DEPENDENCY_TABLES]
            for target in data.get("target", {}).values():
                tables += [target.get(t, {}) for t in DEPENDENCY_TABLES]
            found = set()
            for table in tables:
                for key, spec in table.items():
                    spec = spec if isinstance(spec, dict) else {}
                    name = spec.get("package", key)
                    path = spec.get("path")
                    if spec.get("workspace"):
                        shared_spec = shared.get(key, {})
                        if isinstance(shared_spec, dict):
                            name = shared_spec.get("package", key)
                            path = shared_spec.get("path")
                    if path is not None:
                        target_dir = posixpath.normpath(posixpath.join(member, path)) if not spec.get(
                            "workspace") else posixpath.normpath(path)
                        if target_dir in by_dir:
                            found.add(by_dir[target_dir])
                    elif name in self.dirs:
                        found.add(name)
            self.deps[by_dir[member]] = found

    def closure(self, packages: list[str]) -> set[str]:
        """The directories of `packages` and of everything they depend on."""
        seen: set[str] = set()
        stack = [p for p in packages if p in self.dirs]
        while stack:
            package = stack.pop()
            if package in seen:
                continue
            seen.add(package)
            stack.extend(self.deps.get(package, ()))
        return {self.dirs[p] for p in seen}

    def references(self, dirs: set[str], index: dict[str, str]) -> set[str]:
        """Paths outside `dirs` that Rust sources in `dirs` name relatively."""
        found: set[str] = set()
        for top in dirs:
            for target in self._named_paths(top, index):
                if target == "*" or not any(target == d or target.startswith(d + "/") for d in dirs):
                    found.add(target)
        return found

    def _named_paths(self, top: str, index: dict[str, str]) -> set[str]:
        """Every existing path a Rust file under `top` names with a ../ path."""
        cache = self.__dict__.setdefault("_named", {})
        if top in cache:
            return cache[top]
        named: set[str] = set()
        for walk_dir, _, names in os.walk(os.path.join(self.root, top)):
            rel_dir = posixpath.relpath(walk_dir.replace(os.sep, "/"), self.root.replace(os.sep, "/"))
            for name in names:
                if not name.endswith(".rs"):
                    continue
                with open(os.path.join(walk_dir, name), encoding="utf-8", errors="replace") as f:
                    text = f.read()
                if UNFOLLOWABLE.search(text):
                    named.add("*")
                for literal in RELATIVE_PATH.findall(text):
                    # include_str!/include_bytes! resolve from the file, CARGO_MANIFEST_DIR from the package.
                    for base in (rel_dir, top):
                        target = posixpath.normpath(posixpath.join(base, literal))
                        if not target.startswith("..") and exists(target, index):
                            named.add(target)
                            break
        cache[top] = named
        return named


def exists(path: str, index: dict[str, str]) -> bool:
    if path in index:
        return True
    prefix = path.rstrip("/") + "/"
    return any(p.startswith(prefix) for p in index)


VERSION_KEY = re.compile(r'^version\s*=\s*"[^"]*"\s*$', re.MULTILINE)


def without_versions(path: str, root: str) -> str | None:
    """Cargo.toml without the workspace version, Cargo.lock without the workspace
    packages' versions (a version bump changes no code); None for other files."""
    if path not in ("Cargo.toml", "Cargo.lock"):
        return None
    with open(os.path.join(root, path), encoding="utf-8") as f:
        text = f.read()
    if path == "Cargo.toml":
        # Only [workspace.package] has a bare `version = "..."` line; dependencies use inline tables.
        return hashlib.sha256(VERSION_KEY.sub("", text).encode()).hexdigest()
    blocks = text.split("\n\n")
    # Workspace packages are the [[package]] entries without a `source`.
    kept = [VERSION_KEY.sub("", b) if "[[package]]" in b and "\nsource = " not in b else b for b in blocks]
    return hashlib.sha256("\n\n".join(kept).encode()).hexdigest()


def fingerprint(suite: str, index: dict[str, str], patterns: list[str], dirs: set[str],
                variant: str = "", root: str | None = None) -> str:
    """Hash of every (path, object id) a suite depends on. `dirs` are file or directory paths."""
    prefixes = tuple(d.rstrip("/") + "/" for d in dirs)
    lines = []
    for path, oid in index.items():
        if path in dirs or path.startswith(prefixes) or matches(path, patterns):
            if root is not None:
                oid = without_versions(path, root) or oid
            lines.append(f"{path} {oid}")
    lines.sort()
    digest = hashlib.sha256(f"{VERSION}\n{suite}\n{variant.strip()}\n".encode())
    digest.update("\n".join(lines).encode())
    return digest.hexdigest()[:40]


def suite_keys(suites: list[str], root: str = ".", variants: dict[str, str] | None = None) -> tuple[dict[str, str], dict[str, list[str]],
                                                             dict[str, tuple[list[str], set[str]]]]:
    """Per suite: cache key prefix, the paths outside its packages it reads, and its inputs."""
    index = tree_index(root)
    workspace = Workspace(root)
    keys: dict[str, str] = {}
    extra: dict[str, list[str]] = {}
    inputs: dict[str, tuple[list[str], set[str]]] = {}
    for suite in suites:
        patterns = COMMON + SUITE_INPUTS[suite]
        dirs: set[str] = set()
        if suite in SUITE_PACKAGES:
            dirs = workspace.closure(SUITE_PACKAGES[suite])
            refs = workspace.references(dirs, index)
            extra[suite] = sorted(r if r != "*" else "the whole workspace (a path built from "
                                  "CARGO_MANIFEST_DIR's parent)" for r in refs)
            if "*" in refs:
                patterns = patterns + EVERYTHING
                refs.discard("*")
            dirs |= refs
        variant = (variants or {}).get(suite, "")
        keys[suite] = f"ci-pass-{VERSION}-{suite}-{fingerprint(suite, index, patterns, dirs, variant, root)}"
        inputs[suite] = (patterns, dirs)
    return keys, extra, inputs


def covered(path: str, inputs: dict[str, tuple[list[str], set[str]]]) -> bool:
    """Whether some suite's fingerprint includes `path`."""
    for patterns, dirs in inputs.values():
        if path in dirs or any(path.startswith(d.rstrip("/") + "/") for d in dirs) or matches(path, patterns):
            return True
    return False


# Changed files here that no fingerprint covers mean the inputs are not understood:
# nothing is reused (fail closed).
BUILD_PATHS = ["crates/**", "apps/zeron/**", "Cargo.toml", "Cargo.lock", ".cargo/**", "rust-toolchain.toml"]


def passed_jobs(api, repo: str, key: str, trusted_refs: set[str]) -> set[str]:
    """Jobs recorded as passed under `key` on a trusted ref."""
    data = api(f"/repos/{repo}/actions/caches?key={key}-&per_page=100")
    jobs = set()
    for cache in data.get("actions_caches", []):
        if cache.get("ref") in trusted_refs and cache["key"].startswith(key + "-"):
            jobs.add(cache["key"][len(key) + 1:])
    return jobs
