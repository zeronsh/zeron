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
  native_oses  JSON list, the voice platform checks to run
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
    "voice": [("offline-linux", "linux"), ("native-platform-check-macos-14", "macos"),
              ("native-platform-check-windows-latest", "windows")],  # voice-tests.yml
    "windows": [("tests", "windows"), ("app", "windows")],  # windows.yml
    "macos": [("macos-native", "macos")],  # macos.yml
    "frame_source": [("macos-frame-source", "macos")],  # macos.yml
    "ios": [("ios-core", "linux"), ("ios-tests", "macos")],  # macos.yml (+ android publish below)
    "android": [("apk", "linux")],  # android.yml (+ publish on main, added below)
}
# Suites whose jobs are independent, so one can be skipped while another runs.
SPLITTABLE = ("voice", "windows")
VOICE_NATIVE = "native-platform-check-"

CARGO = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]
CI_SELF = [
    ".github/workflows/ci.yml",
    "scripts/ci/plan.py",
    "scripts/ci/admit.py",
    "scripts/ci/reuse.py",
]

# The app links the Rust mobile core and everything it depends on (iOS and Android).
MOBILE_CORE = [
    "crates/mobile/**", "crates/client/**", "crates/text/**", "crates/markdown/**",
    "crates/doc/**", "crates/sync/**", "crates/proto/**", "crates/rpc/**", "crates/syntax/**",
]

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
    "voice": CARGO + ["crates/audio/**", "crates/voice/**", "crates/voice-media/**", "crates/voice-session/**",
                      "crates/orb/**", "crates/proto/src/voice/**", "crates/proto/src/voice.rs", "crates/rpc/**",
                      "crates/client/**", "crates/mobile/**", "crates/harness/**", "crates/engine/**",
                      "crates/ui/**", "crates/mcp/**", "apps/ios/Zeron/Voice/**",
                      "apps/ios/ZeronTests/RemoteVoiceLifecycleTests.swift", "dist/voice/**",
                      "scripts/package-voice-runtime.py", "scripts/codex-voice-remote-smoke.py",
                      "scripts/tests/*voice_remote*", "scripts/tests/test_voice_packaging.py",
                      ".github/workflows/voice-tests.yml"],
    "windows": CARGO + ["crates/**", "apps/zeron/**", "dist/windows/**", "scripts/test-windows-*.ps1",
                        "scripts/package-windows.ps1", ".github/actions/windows-ci-setup/**",
                        ".github/workflows/windows.yml"],
    "macos": CARGO + ["crates/**", "apps/zeron/**", "edge/**", "scripts/run-macos-browser-fixture.sh",
                      "scripts/test-browser-storage.py", "scripts/ci/run-macos-fixture.sh",
                      "dist/macos/Info.plist", ".github/workflows/macos.yml"],
    # Tests the pinned GPUI checkout, which only changes with the gpui rev.
    "frame_source": ["Cargo.toml", "Cargo.lock", ".github/workflows/macos.yml"],
    "ios": CARGO + MOBILE_CORE + ["apps/ios/**", "scripts/ios/**", ".github/workflows/macos.yml"],
    "android": CARGO + ["apps/android/**", "crates/**", "scripts/android/**", "apps/ios/Zeron/Fonts/**",
                        "apps/ios/Zeron/Assets.xcassets/**", ".github/workflows/android.yml"],
}

# Fast tier: the files that always select the platform suite...
FAST_PATHS: dict[str, list[str]] = {
    "voice": CARGO + ["crates/audio/**", "crates/voice/**", "crates/voice-media/**", "crates/voice-session/**",
                      "crates/orb/**", "crates/proto/src/voice/**", "crates/proto/src/voice.rs",
                      "apps/ios/Zeron/Voice/**", "dist/voice/**", "scripts/package-voice-runtime.py",
                      "scripts/codex-voice-remote-smoke.py", "scripts/tests/*voice*",
                      ".github/workflows/voice-tests.yml"],
    "windows": CARGO + ["apps/zeron/**", "dist/windows/**", "scripts/test-windows-*.ps1",
                        "scripts/package-windows.ps1", ".github/actions/windows-ci-setup/**",
                        ".github/workflows/windows.yml", "crates/update/**"],
    "macos": CARGO + ["apps/zeron/**", "crates/update/**", "crates/preview/**", "crates/text/**",
                      "edge/package.json", "edge/package-lock.json", "scripts/run-macos-browser-fixture.sh",
                      "scripts/test-browser-storage.py", "scripts/ci/run-macos-fixture.sh",
                      "dist/macos/Info.plist", ".github/workflows/macos.yml"],
    "android": CARGO + MOBILE_CORE + ["apps/android/**", "scripts/android/**", "apps/ios/Zeron/Fonts/**",
                                      "apps/ios/Zeron/Assets.xcassets/**", ".github/workflows/android.yml"],
}

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


def plan(files: list[dict] | None, tier: str) -> dict[str, bool]:
    """files: [{"filename", "previous_filename"?, "patch"?}], or None for "everything"."""
    if files is None:
        return {suite: True for suite in SUITE_JOBS}
    paths = {f["filename"] for f in files} | {f["previous_filename"] for f in files if f.get("previous_filename")}
    if any(p in CI_SELF for p in paths):
        return {suite: True for suite in SUITE_JOBS}

    suites = {suite: any(matches(p, BROAD[suite]) for p in paths) for suite in SUITE_JOBS}
    if tier == "fast":
        for suite in PLATFORM_FAST_SUITES:
            if not suites[suite]:
                continue
            fast = any(matches(p, FAST_PATHS[suite]) for p in paths)
            if not fast and suite in PLATFORM_CODE:
                fast = platform_specific(suite, files)
            suites[suite] = fast
    return suites


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


def reused_jobs(suites: dict[str, bool], passed: dict[str, set[str]]) -> dict[str, list[str]]:
    """Jobs to skip: all of a suite's when they all passed, or single ones of SPLITTABLE suites."""
    skip: dict[str, list[str]] = {}
    for suite, enabled in suites.items():
        if not enabled:
            continue
        jobs = [job for job, _ in SUITE_JOBS[suite]]
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
            else:
                trusted = {ref, "refs/heads/main"}
                if os.environ.get("CI_PR_TIER", "fast") == "full":
                    why = "CI_PR_TIER=full"
                else:
                    tier = "fast"
            files = pull_request_files(repo, pr["number"], pr.get("changed_files", 0))
        elif event == "push":
            trusted = {"refs/heads/main"}
            files = compare_files(repo, payload.get("before", ""), payload.get("after", ""))
        elif event == "merge_group":
            trusted = {"refs/heads/main"}
            group = payload["merge_group"]
            files = compare_files(repo, group["base_sha"], group["head_sha"])
        # workflow_dispatch and anything else: everything, nothing reused.
    except (OSError, KeyError, ValueError) as error:  # URLError, HTTPError, timeouts, bad JSON
        print(f"::warning::could not list changed files ({error}); running every suite")
        files = None

    selected = plan(files, tier)
    keys: dict[str, str] = {}
    extra: dict[str, list[str]] = {}
    passed: dict[str, set[str]] = {}
    try:
        keys, extra = reuse.suite_keys([suite for suite, on in selected.items() if on])
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"::warning::could not fingerprint the suites ({error}); nothing is reused or recorded")
    if trusted:
        for suite, key in keys.items():
            try:
                passed[suite] = reuse.passed_jobs(api, repo, key, trusted)
            except (OSError, KeyError, ValueError) as error:
                print(f"::warning::could not look up earlier passes of {suite} ({error})")
    skip = reused_jobs(selected, passed)
    running = jobs_to_run(selected, skip)
    suites = {suite: bool(running.get(suite)) for suite in selected}
    native_oses = [job[len(VOICE_NATIVE):] for job, _ in running.get("voice", []) if job.startswith(VOICE_NATIVE)]
    counts = reservation(running, event, ref)
    out = {
        "suites": json.dumps(suites, separators=(",", ":")),
        "reservation": format_reservation(counts),
        "tier": tier,
        "keys": json.dumps(keys, separators=(",", ":")),
        "skip": json.dumps(skip, separators=(",", ":")),
        "native_oses": json.dumps(native_oses, separators=(",", ":")),
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
        done = skip.get(suite, [])
        if not running.get(suite):
            note = "reused: passed before on the same inputs"
        elif done:
            note = f"reused {', '.join(done)}"
        else:
            note = ""
        lines.append(f"| {suite} | {'yes' if running.get(suite) else 'reused'} | {note} |")
    lines += ["", f"Reserved runners: `{out['reservation']}`"]
    if tier == "fast":
        lines += ["", "Windows, macOS, voice and Android run on pull requests only when the change looks "
                  "platform specific. Add the `ci:full` label and re-run for everything."]
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
