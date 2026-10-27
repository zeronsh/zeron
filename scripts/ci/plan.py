#!/usr/bin/env python3
"""Decide which CI suites a run needs and how many runners it reserves.

Run by the `plan` job of .github/workflows/ci.yml. Inputs come from the
environment (see main()); outputs are written to $GITHUB_OUTPUT:

  suites       JSON object, suite name -> true/false
  reservation  "linux=N windows=N macos=N", the runners the run will use. The
               admit job carries it in its name so that other runs' admission
               (scripts/ci/admit.py) can read it.
  tier         "fast" (pull requests) or "full" (main, merge queue, manual,
               `ci:full` label, CI_PR_TIER=full)
  keys         JSON object, suite -> cache key prefix under which its jobs record
               a pass (scripts/ci/reuse.py)
  skip         JSON object, suite -> jobs that already passed on the same
               inputs and are skipped (Windows and voice skip single jobs; the
               other suites only run or skip as a whole)
  flags        JSON object, suite -> optional steps to run (fast tier: only
               for their own files): ui "browser", macos "fixtures"
  pr           the pull request number (pull requests only), for the admit job name

Reuse. A selected suite whose inputs (scripts/ci/reuse.py) are unchanged since
its jobs passed, on this pull request or on main, is skipped, as are single
Windows and voice jobs that passed while the others did not.

Tiers. Every suite has a broad path list (what used to trigger its workflow).
On the full tier a suite runs when any changed file matches it. On the fast
tier the expensive platform suites (Windows, macOS native, voice platforms,
Android) only run when the change can plausibly be platform specific: their
own files, or Rust/script hunks that mention the platform (cfg(windows),
target_os, objc, WebKit...). Linux, iOS (already selective) and the cheap
suites run as on the full tier. Changes to CI itself run everything.

Nightly only. Some checks cost more runner time than the regressions they
catch on pull requests and main (NIGHTLY_SUITES, NIGHTLY_FLAGS): they run on
the nightly schedule, on manual runs and with the `ci:full` label.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import reuse  # noqa: E402
from reuse import matches  # noqa: E402

# Suite -> (job, runner) for each job it starts; the job name is also the name its
# pass is recorded under. Keep in step with the reusable workflows
# (scripts/tests/test_ci_plan.py checks).
SUITE_JOBS: dict[str, list[tuple[str, str]]] = {
    "core": [("core-tests", "linux")],  # ui-tests.yml
    "ui": [("ui-tests", "linux")],  # ui-tests.yml (UI + Linux browser)
    "cursor": [("cursor-compatibility", "linux")],  # cursor-compatibility.yml
    "edge": [("edge", "linux")],  # preview-tests.yml
    "installer": [("desktop-entry", "linux")],  # linux-installer.yml
    "voice": [("offline-linux", "linux"), ("windows-runtime", "windows")],  # voice-tests.yml
    "windows": [("tests", "windows"), ("app", "windows")],  # windows.yml
    "macos": [("macos-native", "macos")],  # macos.yml
    "frame_source": [("macos-frame-source", "macos")],  # macos.yml
    "ios": [("ios-core", "linux"), ("ios-tests", "macos")],  # macos.yml (+ android publish below)
    "android": [("apk", "linux")],  # android.yml (+ publish on main, added below)
}
# Run only nightly, manually or with `ci:full` (see the module docstring): voice
# (the AEC3 audio stack and the packaged Windows media runtime; no failures of
# their own in the two weeks measured, and the rest of voice is tested in
# core-tests, ios-core and the Windows tests) and the Linux native browser
# fixture (its failures were window-timing flakes and package downloads).
NIGHTLY_SUITES = ("voice",)
NIGHTLY_FLAGS = (("ui", "browser"),)
# Suites whose jobs are independent, so one can be skipped while another runs.
SPLITTABLE = ("voice", "windows")

CARGO = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]
CI_SELF = [
    ".github/workflows/ci.yml",
    "scripts/ci/plan.py",
    "scripts/ci/admit.py",
    "scripts/ci/reuse.py",
]

# The apps link the Rust mobile core and everything it depends on (iOS and Android).
# main() replaces this with zeron-mobile's dependency closure from the Cargo.toml
# files; this list is the fallback when they cannot be read.
MOBILE_CORE = [
    "crates/mobile/**", "crates/client/**", "crates/text/**", "crates/markdown/**",
    "crates/doc/**", "crates/sync/**", "crates/proto/**", "crates/rpc/**", "crates/syntax/**",
    "crates/orb/**", "crates/voice-session/**", "crates/visual/**", "crates/visual-gpu/**",
    "crates/term/**", "crates/term-link/**", "crates/veil/**", "crates/zatex/**", "crates/preview/**",
]

# Voice: the WebRTC audio stack (zeron-audio, which nothing else builds yet) and
# the packaged Windows media runtime. The rest of voice is tested where it is
# built: protocol and transport in core-tests, mobile voice in ios-core,
# voice-media in core-tests and the Windows tests.
VOICE_PATHS = ["crates/audio/**", "crates/voice-media/**", "dist/voice/**", "scripts/package-voice-runtime.py",
               "scripts/codex-voice-remote-smoke.py", "scripts/tests/*voice*", ".github/workflows/voice-tests.yml"]

BROAD: dict[str, list[str]] = {
    "core": CARGO + ["crates/**", "apps/zeron/**", "scripts/ci/**", ".github/actions/linux-ci/**",
                     ".github/workflows/ui-tests.yml"],
    "ui": CARGO + ["crates/**", "scripts/ci/**", ".github/actions/linux-ci/**",
                   "scripts/test-linux-browser.sh", "scripts/test-browser-storage.py",
                   ".github/workflows/ui-tests.yml"],
    "cursor": CARGO + ["crates/harness/**", "crates/doc/**", "crates/engine/**",
                       "scripts/*cursor-sdk*", ".github/workflows/cursor-*.yml"],
    "edge": ["edge/**", ".github/workflows/preview-tests.yml"],
    "installer": ["edge/src/install.sh", "scripts/package-linux.sh", "scripts/test-linux-desktop-entry.sh",
                  "scripts/ci/apt-install.sh", "dist/zeron.desktop", "dist/zeron.png",
                  ".github/workflows/linux-installer.yml"],
    "voice": CARGO + VOICE_PATHS,
    "windows": CARGO + ["crates/**", "apps/zeron/**", "dist/windows/**", "scripts/test-windows-*.ps1",
                        "scripts/package-windows.ps1", ".github/actions/windows-ci-setup/**",
                        ".github/workflows/windows.yml"],
    "macos": CARGO + ["crates/**", "apps/zeron/**", "edge/**", "scripts/run-macos-browser-fixture.sh",
                      "scripts/test-browser-storage.py", "scripts/ci/run-macos-fixture.sh",
                      "dist/macos/Info.plist", ".github/workflows/macos.yml"],
    # Tests the pinned GPUI checkout: only when the pin moves (see gpui_moved()).
    "frame_source": [".github/workflows/macos.yml"],
    "ios": CARGO + MOBILE_CORE + ["apps/ios/**", "scripts/ios/**", ".github/workflows/macos.yml"],
    "android": CARGO + ["apps/android/**", "crates/**", "scripts/android/**", "apps/ios/Zeron/Fonts/**",
                        "apps/ios/Zeron/Assets.xcassets/**", ".github/workflows/android.yml"],
}

# Fast tier: the files that always select the platform suite...
FAST_PATHS: dict[str, list[str]] = {
    "voice": VOICE_PATHS,
    "windows": CARGO + ["apps/zeron/**", "dist/windows/**", "scripts/test-windows-*.ps1",
                        "scripts/package-windows.ps1", ".github/actions/windows-ci-setup/**",
                        ".github/workflows/windows.yml", "crates/update/**"],
    "macos": CARGO + ["apps/zeron/**", "crates/update/**", "crates/preview/**", "crates/text/**",
                      "edge/package.json", "edge/package-lock.json", "scripts/run-macos-browser-fixture.sh",
                      "scripts/test-browser-storage.py", "scripts/ci/run-macos-fixture.sh",
                      "dist/macos/Info.plist", ".github/workflows/macos.yml"],
    # The Rust is the same code the Linux suites test (no Android cfg), so a pull
    # request builds the APK for the app, the FFI crate and dependency changes.
    "android": CARGO + ["apps/android/**", "scripts/android/**", "crates/mobile/**", "crates/mobile-bindgen/**",
                        "apps/ios/Zeron/Fonts/**", "apps/ios/Zeron/Assets.xcassets/**",
                        ".github/workflows/android.yml"],
}

# Fast tier: jobs and steps of a selected suite that only run for their own files.
# On the full tier everything runs.
JOB_PATHS: dict[tuple[str, str], list[str]] = {
    # Release build, packaging, installer and startup probes (`tests` already
    # compiles the app).
    ("windows", "app"): CARGO + ["apps/zeron/**", "dist/windows/**", "scripts/test-windows-*.ps1",
                                 "scripts/package-windows.ps1", ".github/actions/windows-ci-setup/**",
                                 ".github/workflows/windows.yml", "crates/update/**"],
    # The simulator unit tests (and the bindings they link): the app, the FFI
    # crate and the client it wraps; ios-core tests the rest of the mobile core.
    ("ios", "ios-tests"): CARGO + ["apps/ios/**", "scripts/ios/**", "crates/mobile/**", "crates/mobile-bindgen/**",
                                   "crates/client/**", ".github/workflows/macos.yml"],
}
# Fast tier: optional steps (flags passed to the suite workflow, and part of its fingerprint).
FLAG_PATHS: dict[tuple[str, str], list[str]] = {
    # Native browser fixture on X11/Wayland/OpenGL.
    ("ui", "browser"): CARGO + ["crates/ui/src/browser/**", "crates/ui/examples/browser-fixture*",
                                "scripts/test-linux-browser.sh", "scripts/test-browser-storage.py",
                                ".github/workflows/ui-tests.yml"],
    # WebKit browser fixture and the preview (HMR) fixture.
    ("macos", "fixtures"): CARGO + ["crates/ui/src/browser/**", "crates/ui/examples/browser-fixture*",
                                    "crates/ui/examples/preview-fixture*", "crates/preview/**", "edge/**",
                                    "scripts/run-macos-browser-fixture.sh", "scripts/test-browser-storage.py",
                                    "scripts/ci/run-macos-fixture.sh", ".github/workflows/macos.yml"],
}

# A version bump (workspace version in Cargo.toml and the workspace packages'
# versions in Cargo.lock) changes no code: it selects nothing and is left out of
# the fingerprints (reuse.py).
VERSION_LINE = re.compile(r'^\s*version\s*=\s*"[^"]*"\s*$')


# ...and, for files under crates/, platform markers in the file name or in the
# changed code (comments are ignored: "as on Windows" changes nothing). A
# platform conditional (target_os, cfg(unix), cfg(not(windows))...) counts for
# both, since it decides what each of them compiles; cfg(windows) only for Windows. A Rust file without a
# diff (too large, binary) counts too.
PLATFORM_CFG = r"target_os|target_family|target_vendor|cfg!?\(\s*(?:not\(\s*(?:unix|windows)|unix)\b"
PLATFORM_PATHS = {
    "windows": re.compile(r"(?:^|[/_.-])(?:windows|win32)(?:[/_.-]|$)", re.IGNORECASE),
    "macos": re.compile(r"(?:^|[/_.-])(?:macos|darwin|apple|coretext|webkit)(?:[/_.-]|$)", re.IGNORECASE),
}
PLATFORM_CODE = {
    "windows": re.compile(PLATFORM_CFG + r"|cfg!?\(\s*windows\b|\bwindows(?:_sys)?::|\bwin32|\bwinapi\b|\bmsvc\b|\bhwnd\b"
                          r"|direct3d|directwrite|\bdwrite|\bd3d1[12]|gpui_windows", re.IGNORECASE),
    "macos": re.compile(PLATFORM_CFG + r"|\bobjc2?\b|\bcocoa\b|\bappkit\b|core_?text|\bwebkit|wkwebview"
                        r"|\bns(?:view|window|string|app|event)\b|gpui_macos|\bcore_foundation", re.IGNORECASE),
}
COMMENT = re.compile(r"^\s*(?://|/\*|\*)")


PLATFORM_FAST_SUITES = ("voice", "windows", "macos", "android")


def changed_code(patch: str) -> str:
    """The added and removed lines of a diff, without comments."""
    lines = []
    for line in patch.splitlines():
        if line[:1] not in "+-" or line.startswith(("+++", "---")):
            continue
        code = line[1:]
        if COMMENT.match(code):
            continue
        lines.append(code.split("//", 1)[0])
    return "\n".join(lines)


def platform_specific(platform: str, files: list[dict]) -> bool:
    for f in files:
        path = f["filename"]
        if not path.startswith("crates/"):
            continue
        if PLATFORM_PATHS[platform].search(path):
            return True
        patch = f.get("patch")
        if patch is None:
            if path.endswith(".rs"):
                return True
            continue
        if PLATFORM_CODE[platform].search(changed_code(patch)):
            return True
    return False


def version_only(f: dict) -> bool:
    """A Cargo.toml / Cargo.lock change that only changes `version = "..."` lines."""
    if f["filename"] not in ("Cargo.toml", "Cargo.lock") or f.get("patch") is None:
        return False
    changed = [line[1:] for line in f["patch"].splitlines()
               if line[:1] in "+-" and not line.startswith(("+++", "---"))]
    return bool(changed) and all(VERSION_LINE.match(line) for line in changed)


def gpui_moved(files: list[dict]) -> bool:
    """Whether the pinned GPUI (zeronsh/zui) checkout changed."""
    for f in files:
        if f["filename"] not in ("Cargo.toml", "Cargo.lock"):
            continue
        patch = f.get("patch")
        if patch is None or re.search(r"^[+-].*(gpui|zui)", patch, re.MULTILINE):
            return True
    return False


def plan(files: list[dict] | None, tier: str, mobile: list[str] | None = None,
         nightly: bool = False) -> dict[str, bool]:
    """files: [{"filename", "previous_filename"?, "patch"?}], or None for "everything".

    `nightly`: the nightly schedule, a manual run or `ci:full`, which also run NIGHTLY_SUITES.
    """
    suites = select(files, tier, mobile)
    if not nightly:
        for suite in NIGHTLY_SUITES:
            suites[suite] = False
    return suites


def select(files: list[dict] | None, tier: str, mobile: list[str] | None) -> dict[str, bool]:
    if files is None:
        return {suite: True for suite in SUITE_JOBS}
    files = [f for f in files if not version_only(f)]
    paths = {f["filename"] for f in files} | {f["previous_filename"] for f in files if f.get("previous_filename")}
    if any(p in CI_SELF for p in paths):
        return {suite: True for suite in SUITE_JOBS}

    broad = dict(BROAD)
    if mobile is not None:
        broad["ios"] = CARGO + mobile + ["apps/ios/**", "scripts/ios/**", ".github/workflows/macos.yml"]
    suites = {suite: any(matches(p, broad[suite]) for p in paths) for suite in SUITE_JOBS}
    suites["frame_source"] = suites["frame_source"] or gpui_moved(files)
    if tier == "fast":
        for suite in PLATFORM_FAST_SUITES:
            if not suites[suite]:
                continue
            fast = any(matches(p, FAST_PATHS[suite]) for p in paths)
            if not fast and suite in PLATFORM_CODE:
                fast = platform_specific(suite, files)
            suites[suite] = fast
    return suites


def gated(files: list[dict] | None, tier: str, suites: dict[str, bool],
          nightly: bool = False) -> tuple[dict[str, list[str]], dict[str, list[str]]]:
    """Fast tier: (jobs not needed, flags on) per selected suite. The full tier runs
    everything but NIGHTLY_FLAGS, which only `nightly` runs."""
    paths = None if files is None else ({f["filename"] for f in files}
                                        | {f["previous_filename"] for f in files if f.get("previous_filename")})
    everything = tier != "fast" or paths is None or any(p in CI_SELF for p in paths)
    off: dict[str, list[str]] = {}
    flags: dict[str, list[str]] = {}
    for (suite, job), patterns in JOB_PATHS.items():
        if suites.get(suite) and not everything and not any(matches(p, patterns) for p in paths):
            off.setdefault(suite, []).append(job)
    for (suite, flag), patterns in FLAG_PATHS.items():
        if (suite, flag) in NIGHTLY_FLAGS and not nightly:
            continue
        if suites.get(suite) and (everything or any(matches(p, patterns) for p in paths)):
            flags.setdefault(suite, []).append(flag)
    return off, flags


def jobs_to_run(suites: dict[str, bool], skip: dict[str, list[str]] | None = None) -> dict[str, list[tuple[str, str]]]:
    skip = skip or {}
    return {suite: [job for job in SUITE_JOBS[suite] if job[0] not in skip.get(suite, ())]
            for suite, enabled in suites.items() if enabled}


def reservation(running: dict[str, list[tuple[str, str]]], event: str, ref: str) -> dict[str, int]:
    counts = {"linux": 1, "windows": 0, "macos": 0}  # 1: the final `CI result` job
    for jobs in running.values():
        for _, runner in jobs:
            counts[runner] += 1
    if running.get("android") and event == "push" and ref == "refs/heads/main":
        counts["linux"] += 1  # android.yml publish
    return counts


def reused_jobs(suites: dict[str, bool], passed: dict[str, set[str]],
                off: dict[str, list[str]] | None = None) -> dict[str, list[str]]:
    """Jobs to skip: all of a suite's (needed ones) when they all passed, or single ones of SPLITTABLE suites."""
    off = off or {}
    skip: dict[str, list[str]] = {}
    for suite, enabled in suites.items():
        if not enabled:
            continue
        jobs = [job for job, _ in SUITE_JOBS[suite] if job not in off.get(suite, ())]
        done = [job for job in jobs if job in passed.get(suite, set())]
        if len(done) == len(jobs) or (done and suite in SPLITTABLE):
            skip[suite] = done
    return skip


def format_reservation(counts: dict[str, int]) -> str:
    return " ".join(f"{k}={counts[k]}" for k in ("linux", "windows", "macos"))


# GitHub API --------------------------------------------------------------------------------------

def api(path: str):
    url = os.environ.get("GITHUB_API_URL", "https://api.github.com") + path
    request = urllib.request.Request(url, headers={
        "Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    })
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def pull_request_files(repo: str, number: int, expected: int) -> list[dict] | None:
    files: list[dict] = []
    page = 1
    while True:
        batch = api(f"/repos/{repo}/pulls/{number}/files?per_page=100&page={page}")
        files.extend(batch)
        if len(batch) < 100:
            break
        page += 1
    # The API stops at 3000 files: run everything rather than guess.
    return files if len(files) >= expected else None


def compare_files(repo: str, base: str, head: str) -> list[dict] | None:
    if not base or set(base) == {"0"}:
        return None
    data = api(f"/repos/{repo}/compare/{base}...{head}")
    files = data.get("files")
    # compare lists at most 300 files.
    if files is None or len(files) >= 300:
        return None
    return files


def main() -> int:
    event = os.environ["GITHUB_EVENT_NAME"]
    repo = os.environ["GITHUB_REPOSITORY"]
    ref = os.environ.get("GITHUB_REF", "")
    with open(os.environ["GITHUB_EVENT_PATH"]) as f:
        payload = json.load(f)

    tier = "full"
    # The nightly schedule and manual runs (and `ci:full`, below) also run the nightly-only checks.
    nightly = event in ("schedule", "workflow_dispatch")
    files: list[dict] | None = None
    why = ""
    # Refs whose recorded passes this run may reuse (none: run everything selected).
    trusted: set[str] = set()
    try:
        if event == "pull_request":
            # The pull request as it is now (not as in the event), so adding `ci:full`
            # and re-running works.
            pr = api(f"/repos/{repo}/pulls/{payload['pull_request']['number']}")
            labels = {label["name"] for label in pr.get("labels", [])}
            if "ci:full" in labels:
                why = "label ci:full: every suite, nothing reused"
                nightly = True
            else:
                # A fork's earlier runs could have recorded a pass for any fingerprint
                # (its own edited workflows write the cache), including the one of the
                # commit it asks to merge: forks reuse main's passes only, until the
                # merge queue (which reuses nothing) is the required gate and
                # CI_FORK_REUSE=own turns this back on.
                head_repo = (pr.get("head", {}).get("repo") or {}).get("full_name")
                fork = head_repo != repo
                own = not fork or os.environ.get("CI_FORK_REUSE", "") == "own"
                trusted = {ref, "refs/heads/main"} if own else {"refs/heads/main"}
                if os.environ.get("CI_PR_TIER", "fast") == "full":
                    why = "CI_PR_TIER=full"
                else:
                    tier = "fast"
            files = pull_request_files(repo, pr["number"], pr.get("changed_files", 0))
        elif event == "push":
            trusted = {"refs/heads/main"}
            files = compare_files(repo, payload.get("before", ""), payload.get("after", ""))
        elif event == "merge_group":
            # The last gate before main: run everything selected, reuse nothing.
            group = payload["merge_group"]
            files = compare_files(repo, group["base_sha"], group["head_sha"])
        # workflow_dispatch and anything else: everything, nothing reused.
    except (OSError, KeyError, ValueError) as error:  # URLError, HTTPError, timeouts, bad JSON
        print(f"::warning::could not list changed files ({error}); running every suite")
        files = None

    mobile = None
    try:
        mobile = [d + "/**" for d in sorted(reuse.Workspace(".").closure(["zeron-mobile"]))] + ["crates/mobile-bindgen/**"]
    except (OSError, ValueError, KeyError) as error:
        print(f"::warning::could not read the workspace ({error}); using the static mobile core list")
    selected = plan(files, tier, mobile, nightly)
    off, flags = gated(files, tier, selected, nightly)
    # What a job does depends on its gated jobs and flags too: they are part of the fingerprint.
    variants = {suite: " ".join(f"-{j}" for j in off.get(suite, [])) + " " + " ".join(flags.get(suite, []))
                for suite, on in selected.items() if on}
    keys: dict[str, str] = {}
    extra: dict[str, list[str]] = {}
    passed: dict[str, set[str]] = {}
    try:
        keys, extra, inputs = reuse.suite_keys([suite for suite, on in selected.items() if on], variants=variants)
        uncovered = sorted(f["filename"] for f in files or []
                           if matches(f["filename"], reuse.BUILD_PATHS) and not reuse.covered(f["filename"], inputs))
        if trusted and uncovered:
            print(f"::notice::nothing reused: no suite fingerprint covers {', '.join(uncovered[:5])}")
            why = (why + "; " if why else "") + "nothing reused (changed build files outside every fingerprint)"
            trusted = set()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"::warning::could not fingerprint the suites ({error}); nothing is reused or recorded")
    if trusted:
        for suite, key in keys.items():
            try:
                passed[suite] = reuse.passed_jobs(api, repo, key, trusted)
            except (OSError, KeyError, ValueError) as error:
                print(f"::warning::could not look up earlier passes of {suite} ({error})")
    reused = reused_jobs(selected, passed, off)
    skip = {suite: sorted(set(off.get(suite, [])) | set(reused.get(suite, [])))
            for suite in set(off) | set(reused)}
    running = jobs_to_run(selected, skip)
    suites = {suite: bool(running.get(suite)) for suite in selected}
    counts = reservation(running, event, ref)
    out = {
        "suites": json.dumps(suites, separators=(",", ":")),
        "reservation": format_reservation(counts),
        "tier": tier,
        "keys": json.dumps(keys, separators=(",", ":")),
        "skip": json.dumps(skip, separators=(",", ":")),
        "flags": json.dumps(flags, separators=(",", ":")),
        # The admit job names its pull request, for ci:priority and ci:lock (scripts/ci/admit.py).
        "pr": str(payload["pull_request"]["number"]) if event == "pull_request" else "",
    }
    with open(os.environ["GITHUB_OUTPUT"], "a") as f:
        for key, value in out.items():
            f.write(f"{key}={value}\n")

    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    lines = [f"### CI plan: {tier} tier{f' ({why})' if why else ''}", "",
             f"Changed files: {'all (not listed)' if files is None else len(files)}", "",
             "| suite | runs | |", "|---|---|---|"]
    for suite, on in selected.items():
        if not on:
            lines.append(f"| {suite} | – | not affected |")
            continue
        notes = []
        if reused.get(suite):
            notes.append(f"reused (passed on the same inputs): {', '.join(reused[suite])}")
        if off.get(suite):
            notes.append(f"not needed for this change: {', '.join(off[suite])}")
        if flags.get(suite):
            notes.append(f"with {', '.join(flags[suite])}")
        state = "yes" if running.get(suite) else "reused" if reused.get(suite) else "–"
        lines.append(f"| {suite} | {state} | {'; '.join(notes)} |")
    lines += ["", f"Reserved runners: `{out['reservation']}`"]
    if tier == "fast":
        lines += ["", "Windows, macOS and Android run on pull requests only when the change looks "
                  "platform specific. Add the `ci:full` label and re-run for everything."]
    if not nightly:
        lines += ["", "Nightly only (or with `ci:full`): voice and the Linux native browser fixture."]
    files_read = {path for paths in extra.values() for path in paths}
    if files_read:
        lines += ["", "Files outside their packages that Rust suites read (part of their fingerprints): "
                  + ", ".join(f"`{p}`" for p in sorted(files_read))]
    text = "\n".join(lines)
    print(text)
    if summary:
        with open(summary, "a") as f:
            f.write(text + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
