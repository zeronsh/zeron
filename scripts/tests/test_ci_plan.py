"""scripts/ci/plan.py: which suites a change selects, and the runners they reserve."""

import json
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "ci"))

import plan  # noqa: E402


def files(*paths, patch=""):
    return [{"filename": p, "patch": patch} for p in paths]


def selected(suites):
    return {name for name, on in suites.items() if on}


class PlanTests(unittest.TestCase):
    def test_docs_only_runs_nothing(self):
        self.assertEqual(selected(plan.plan(files("README.md", "docs/ci.md"), "fast")), set())

    def test_ui_change_on_a_pull_request_stays_on_linux(self):
        suites = plan.plan(files("crates/ui/src/shell.rs", patch="+let width = 3;"), "fast")
        self.assertEqual(selected(suites), {"core", "ui"})

    def test_ui_change_on_main_runs_every_platform_it_touches(self):
        suites = plan.plan(files("crates/ui/src/shell.rs", patch="+let width = 3;"), "full")
        self.assertEqual(selected(suites), {"core", "ui", "windows", "macos", "android"})

    def test_platform_code_selects_its_platform(self):
        windows = plan.plan(files("crates/ui/src/shell.rs", patch='+#[cfg(windows)]\n+fn f() {}'), "fast")
        self.assertTrue(windows["windows"])
        self.assertFalse(windows["macos"])
        macos = plan.plan(files("crates/ui/src/browser.rs", patch="+    let view: WKWebView = make();"), "fast")
        self.assertTrue(macos["macos"])
        self.assertFalse(macos["windows"])
        both = plan.plan(files("crates/term/src/pty.rs", patch='+#[cfg(target_os = "linux")]'), "fast")
        self.assertTrue(both["windows"] and both["macos"])
        unix = plan.plan(files("crates/term/src/pty.rs", patch='+    if cfg!(unix) {'), "fast")
        self.assertTrue(unix["windows"] and unix["macos"])
        objc = plan.plan(files("crates/notify/src/mac.rs", patch='+    let app: id = msg_send![class!(NSApp), sharedApplication];'), "fast")
        self.assertTrue(objc["macos"])

    def test_comments_and_window_identifiers_do_not_count(self):
        for patch in ("+    /// it has no page (Windows), the search finds none of it.",
                      "+        cx.refresh_windows();",
                      "+        .usage_windows",
                      "+    // content leaves the keyboard with the page. AppKit gives it back",
                      "+let x = 1; // as zui does on Windows"):
            suites = plan.plan(files("crates/ui/src/a.rs", patch=patch), "fast")
            self.assertFalse(suites["windows"] or suites["macos"], patch)

    def test_context_lines_do_not_count(self):
        patch = " #[cfg(windows)]\n fn f() {}\n+let x = 1;"
        self.assertFalse(plan.plan(files("crates/ui/src/a.rs", patch=patch), "fast")["windows"])

    def test_platform_file_names_count(self):
        suites = plan.plan(files("crates/ui/src/platform/windows.rs", patch="+let x = 1;"), "fast")
        self.assertTrue(suites["windows"])
        self.assertTrue(plan.plan(files("crates/ui/src/browser/macos.rs", patch="+let x = 1;"), "fast")["macos"])
        self.assertFalse(plan.plan(files("crates/ui/src/window_list.rs", patch="+let x = 1;"), "fast")["windows"])

    def test_rust_file_without_a_diff_is_treated_as_platform_specific(self):
        suites = plan.plan([{"filename": "crates/engine/src/huge.rs"}], "fast")
        self.assertTrue(suites["windows"] and suites["macos"])

    def test_lockfile_runs_every_rust_suite(self):
        suites = plan.plan(files("Cargo.lock"), "fast")
        self.assertEqual(selected(suites), {"core", "ui", "cursor", "windows", "macos", "ios", "android"})

    def test_mobile_core_selects_ios_and_android(self):
        suites = plan.plan(files("crates/sync/src/lib.rs", patch="+let x = 1;"), "fast")
        self.assertTrue(suites["ios"])
        self.assertFalse(suites["android"])  # the same Rust as the Linux suites test
        self.assertFalse(suites["windows"])
        self.assertTrue(plan.plan(files("crates/mobile/src/lib.rs", patch="+let x = 1;"), "fast")["android"])
        self.assertTrue(plan.plan(files("crates/sync/src/lib.rs", patch="+x"), "full")["android"])

    def test_renamed_from_counts(self):
        f = [{"filename": "docs/x.md", "previous_filename": "apps/ios/Zeron/X.swift", "patch": ""}]
        self.assertTrue(plan.plan(f, "fast")["ios"])

    def test_ci_changes_run_everything(self):
        daytime = set(plan.SUITE_JOBS) - set(plan.NIGHTLY_SUITES)
        self.assertEqual(selected(plan.plan(files("scripts/ci/admit.py"), "fast")), daytime)
        self.assertEqual(selected(plan.plan(None, "fast")), daytime)
        self.assertTrue(all(plan.plan(None, "full", nightly=True).values()))

    def test_voice_runs_only_nightly(self):
        change = files("crates/audio/src/aec.rs", patch="+let x = 1;")
        self.assertFalse(plan.plan(change, "fast")["voice"])
        self.assertFalse(plan.plan(change, "full")["voice"])
        self.assertTrue(plan.plan(change, "full", nightly=True)["voice"])

    def test_edge_and_installer(self):
        self.assertEqual(selected(plan.plan(files("edge/src/coordinator.ts"), "fast")), {"edge"})
        self.assertEqual(selected(plan.plan(files("edge/src/install.sh"), "fast")), {"edge", "installer"})
        self.assertTrue(plan.plan(files("edge/package-lock.json"), "fast")["macos"])

    def test_version_bumps_select_nothing(self):
        bump = [{"filename": "Cargo.toml", "patch": '@@ -37 +37 @@\n-version = "0.2.126"\n+version = "0.2.127"'},
                {"filename": "Cargo.lock", "patch": ' name = "zeron-ui"\n-version = "0.2.126"\n+version = "0.2.127"'}]
        self.assertEqual(selected(plan.plan(bump, "full")), set())
        dep = [{"filename": "Cargo.lock",
                "patch": '-version = "1.0.1"\n+version = "1.0.2"\n-checksum = "aa"\n+checksum = "bb"'}]
        self.assertTrue(plan.plan(dep, "fast")["core"])

    def test_frame_source_runs_when_the_gpui_pin_moves(self):
        moved = [{"filename": "Cargo.toml",
                  "patch": '-gpui = { git = "https://github.com/zeronsh/zui", rev = "a" }\n'
                           '+gpui = { git = "https://github.com/zeronsh/zui", rev = "b" }'}]
        self.assertTrue(plan.plan(moved, "fast")["frame_source"])
        other = [{"filename": "Cargo.toml", "patch": '+serde = "1"'}]
        self.assertFalse(plan.plan(other, "full")["frame_source"])
        self.assertTrue(plan.plan(files(".github/workflows/macos.yml"), "fast")["frame_source"])

    def test_jobs_and_steps_for_their_own_files(self):
        change = files("crates/ui/src/shell.rs", patch='+#[cfg(windows)]\n+fn f() {}')
        suites = plan.plan(change, "fast")
        off, flags = plan.gated(change, "fast", suites)
        self.assertEqual(off, {"windows": ["app"]})
        self.assertEqual(flags, {})
        browser = files("crates/ui/src/browser/view.rs", patch='+let a = 1;')
        off, flags = plan.gated(browser, "fast", plan.plan(browser, "fast"))
        self.assertEqual(flags, {})  # the Linux browser fixture runs nightly
        off, flags = plan.gated(browser, "fast", plan.plan(browser, "fast"), nightly=True)
        self.assertEqual(flags, {"ui": ["browser"]})
        app = files("crates/mobile/src/lib.rs", "apps/zeron/src/main.rs", patch="+x")
        off, flags = plan.gated(app, "fast", plan.plan(app, "fast"))
        self.assertNotIn("windows", off)
        self.assertNotIn("ios", off)
        sync = files("crates/sync/src/lib.rs", patch="+x")
        self.assertEqual(plan.gated(sync, "fast", plan.plan(sync, "fast"))[0], {"ios": ["ios-tests"]})
        full_off, full_flags = plan.gated(change, "full", plan.plan(change, "full"))
        self.assertEqual(full_off, {})
        self.assertEqual(full_flags, {"macos": ["fixtures"]})
        nightly = plan.gated(change, "full", plan.plan(change, "full", nightly=True), nightly=True)
        self.assertEqual(nightly[1], {"ui": ["browser"], "macos": ["fixtures"]})

    def test_reservation(self):
        suites = plan.plan(files("Cargo.lock"), "full", nightly=True)
        counts = plan.reservation(plan.jobs_to_run(suites), "pull_request", "refs/pull/1/merge")
        # result + core + ui + cursor + voice + ios-core + apk; voice runtime + windows x2; native + ios-tests
        self.assertEqual(counts, {"linux": 7, "windows": 3, "macos": 2})
        main = plan.reservation(plan.jobs_to_run(suites), "push", "refs/heads/main")
        self.assertEqual(main["linux"], 8)  # + android publish
        self.assertEqual(plan.format_reservation(counts), "linux=7 windows=3 macos=2")
        daytime = plan.reservation(plan.jobs_to_run(plan.plan(files("Cargo.lock"), "full")), "push",
                                   "refs/heads/main")
        self.assertEqual(daytime, {"linux": 7, "windows": 2, "macos": 2})  # no voice
        self.assertEqual(plan.reservation(plan.jobs_to_run({s: False for s in plan.SUITE_JOBS}), "push",
                                          "refs/heads/main"),
                         {"linux": 1, "windows": 0, "macos": 0})


class WorkflowConsistencyTests(unittest.TestCase):
    """SUITE_JOBS must match the jobs the reusable workflows start."""

    # suite -> (workflow, jobs of that workflow the suite turns on)
    SUITES = {
        "core": ("ui-tests.yml", ["core-tests"]),
        "ui": ("ui-tests.yml", ["ui-tests"]),
        "cursor": ("cursor-compatibility.yml", ["cursor-compatibility"]),
        "edge": ("preview-tests.yml", ["edge"]),
        "installer": ("linux-installer.yml", ["desktop-entry"]),
        "voice": ("voice-tests.yml", ["offline-linux", "windows-runtime"]),
        "windows": ("windows.yml", ["tests", "app"]),
        "macos": ("macos.yml", ["macos-native"]),
        "frame_source": ("macos.yml", ["macos-frame-source"]),
        "ios": ("macos.yml", ["ios-core", "ios-tests"]),
        "android": ("android.yml", ["apk"]),
    }

    def test_job_runners_match(self):
        """Each suite's jobs and runners, and the name each records its pass under."""
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")
        workflows = ROOT / ".github" / "workflows"
        for suite, (workflow, jobs) in self.SUITES.items():
            text = (workflows / workflow).read_text()
            spec = yaml.safe_load(text)
            runners = []
            for job in jobs:
                runs_on = spec["jobs"][job]["runs-on"]
                if "matrix.os" in runs_on:
                    default = spec.get("on", spec.get(True))["workflow_call"]["inputs"]["native-oses"]["default"]
                    labels = [(f"{job}-{os}", os) for os in json.loads(default)]
                else:
                    labels = [(job, runs_on)]
                for marker, label in labels:
                    runner = "windows" if "windows" in label else "macos" if "macos" in label else "linux"
                    runners.append((marker, runner))
                    recorded = marker if "matrix.os" not in runs_on else f"{job}-${{{{ matrix.os }}}}"
                    self.assertIn(f"key: ${{{{ inputs.pass-key }}}}-{recorded}", text, (suite, marker))
            self.assertEqual(sorted(runners), sorted(plan.SUITE_JOBS[suite]), suite)

    def test_every_suite_is_called(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")
        ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        for suite in plan.SUITE_JOBS:
            self.assertIn(f"suites).{suite}", ci, suite)
            self.assertIn(f"keys).{suite} ||", ci, suite)


if __name__ == "__main__":
    unittest.main()
