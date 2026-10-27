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

Tiers. Every suite has a broad path list (what used to trigger its workflow).
On the full tier a suite runs when any changed file matches it. On the fast
tier the expensive platform suites (Windows, macOS native, voice platforms,
Android) only run when the change can plausibly be platform specific: their
own files, or Rust/script hunks that mention the platform (cfg(windows),
target_os, objc, WebKit...). Linux, iOS (already selective) and the cheap
suites run as on the full tier. Changes to CI itself run everything.
"""

from __future__ import annotations

import fnmatch
import json
import os
import re
import sys
import urllib.request

# Suite -> the runner OS of each job it starts. Keep in step with the reusable
# workflows (scripts/tests/test_ci_plan.py checks the job counts).
SUITE_JOBS: dict[str, list[str]] = {
    "core": ["linux"],  # ui-tests.yml core-tests
    "ui": ["linux"],  # ui-tests.yml ui-tests (UI + Linux browser)
    "cursor": ["linux"],  # cursor-compatibility.yml
    "edge": ["linux"],  # preview-tests.yml
    "installer": ["linux"],  # linux-installer.yml
    "voice": ["linux", "macos", "windows"],  # voice-tests.yml
    "windows": ["windows", "windows"],  # windows.yml tests + app
    "macos": ["macos"],  # macos.yml macos-native
    "frame_source": ["macos"],  # macos.yml macos-frame-source
    "ios": ["linux", "macos"],  # macos.yml ios-core + ios-tests
    "android": ["linux"],  # android.yml apk (+ publish on main, added below)
}

CARGO = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"]
CI_SELF = [
    ".github/workflows/ci.yml",
    "scripts/ci/plan.py",
    "scripts/ci/admit.py",
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


def matches(path: str, patterns: list[str]) -> bool:
    for pattern in patterns:
        if pattern.endswith("/**"):
            if path.startswith(pattern[:-2]):
                return True
        elif fnmatch.fnmatchcase(path, pattern):
            return True
    return False


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


def reservation(suites: dict[str, bool], event: str, ref: str) -> dict[str, int]:
    counts = {"linux": 1, "windows": 0, "macos": 0}  # 1: the final `CI result` job
    for suite, enabled in suites.items():
        if enabled:
            for runner in SUITE_JOBS[suite]:
                counts[runner] += 1
    if suites.get("android") and event == "push" and ref == "refs/heads/main":
        counts["linux"] += 1  # android.yml publish
    return counts


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
    try:
        if event == "pull_request":
            # The pull request as it is now (not as in the event), so adding `ci:full`
            # and re-running works.
            pr = api(f"/repos/{repo}/pulls/{payload['pull_request']['number']}")
            labels = {label["name"] for label in pr.get("labels", [])}
            if "ci:full" in labels:
                why = "label ci:full"
            elif os.environ.get("CI_PR_TIER", "fast") == "full":
                why = "CI_PR_TIER=full"
            else:
                tier = "fast"
            files = pull_request_files(repo, pr["number"], pr.get("changed_files", 0))
        elif event == "push":
            files = compare_files(repo, payload.get("before", ""), payload.get("after", ""))
        elif event == "merge_group":
            group = payload["merge_group"]
            files = compare_files(repo, group["base_sha"], group["head_sha"])
        # workflow_dispatch and anything else: everything.
    except (OSError, KeyError, ValueError) as error:  # URLError, HTTPError, timeouts, bad JSON
        print(f"::warning::could not list changed files ({error}); running every suite")
        files = None

    suites = plan(files, tier)
    counts = reservation(suites, event, ref)
    out = {
        "suites": json.dumps(suites, separators=(",", ":")),
        "reservation": format_reservation(counts),
        "tier": tier,
    }
    with open(os.environ["GITHUB_OUTPUT"], "a") as f:
        for key, value in out.items():
            f.write(f"{key}={value}\n")

    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    lines = [f"### CI plan: {tier} tier{f' ({why})' if why else ''}", "",
             f"Changed files: {'all (not listed)' if files is None else len(files)}", "",
             "| suite | runs |", "|---|---|"]
    lines += [f"| {suite} | {'yes' if enabled else '–'} |" for suite, enabled in suites.items()]
    lines += ["", f"Reserved runners: `{out['reservation']}`"]
    if tier == "fast":
        lines += ["", "Windows, macOS, voice and Android run on pull requests only when the change looks "
                  "platform specific. Add the `ci:full` label and re-run for everything."]
    text = "\n".join(lines)
    print(text)
    if summary:
        with open(summary, "a") as f:
            f.write(text + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
