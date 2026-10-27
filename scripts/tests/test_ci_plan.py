"""scripts/ci/plan.py: which suites a change selects, and the runners they reserve."""

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
        self.assertEqual(selected(suites), {"core", "ui", "voice", "windows", "macos", "android"})

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
        self.assertEqual(selected(suites), {"core", "ui", "cursor", "voice", "windows", "macos",
                                            "frame_source", "ios", "android"})

    def test_mobile_core_selects_ios_and_android(self):
        suites = plan.plan(files("crates/sync/src/lib.rs", patch="+let x = 1;"), "fast")
        self.assertTrue(suites["ios"] and suites["android"])
        self.assertFalse(suites["windows"])

    def test_renamed_from_counts(self):
        f = [{"filename": "docs/x.md", "previous_filename": "apps/ios/Zeron/X.swift", "patch": ""}]
        self.assertTrue(plan.plan(f, "fast")["ios"])

    def test_ci_changes_run_everything(self):
        self.assertTrue(all(plan.plan(files("scripts/ci/admit.py"), "fast").values()))
        self.assertTrue(all(plan.plan(None, "fast").values()))

    def test_edge_and_installer(self):
        self.assertEqual(selected(plan.plan(files("edge/src/coordinator.ts"), "fast")), {"edge"})
        self.assertEqual(selected(plan.plan(files("edge/src/install.sh"), "fast")), {"edge", "installer"})
        self.assertTrue(plan.plan(files("edge/package-lock.json"), "fast")["macos"])

    def test_reservation(self):
        suites = plan.plan(files("Cargo.lock"), "full")
        counts = plan.reservation(suites, "pull_request", "refs/pull/1/merge")
        # result + core + ui + cursor + voice + ios-core + apk; voice + windows x2; voice + native + frame + ios
        self.assertEqual(counts, {"linux": 7, "windows": 3, "macos": 4})
        main = plan.reservation(suites, "push", "refs/heads/main")
        self.assertEqual(main["linux"], 8)  # + android publish
        self.assertEqual(plan.format_reservation(counts), "linux=7 windows=3 macos=4")
        self.assertEqual(plan.reservation({s: False for s in plan.SUITE_JOBS}, "push", "refs/heads/main"),
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
        "voice": ("voice-tests.yml", ["offline-linux", "native-platform-check"]),
        "windows": ("windows.yml", ["tests", "app"]),
        "macos": ("macos.yml", ["macos-native"]),
        "frame_source": ("macos.yml", ["macos-frame-source"]),
        "ios": ("macos.yml", ["ios-core", "ios-tests"]),
        "android": ("android.yml", ["apk"]),
    }

    def test_job_runners_match(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")
        workflows = ROOT / ".github" / "workflows"
        for suite, (workflow, jobs) in self.SUITES.items():
            spec = yaml.safe_load((workflows / workflow).read_text())["jobs"]
            runners = []
            for job in jobs:
                runs_on = spec[job]["runs-on"]
                matrix = spec[job].get("strategy", {}).get("matrix", {}).get("os")
                for label in (matrix if "matrix.os" in runs_on else [runs_on]):
                    runners.append("windows" if "windows" in label else "macos" if "macos" in label else "linux")
            self.assertEqual(sorted(runners), sorted(plan.SUITE_JOBS[suite]), suite)

    def test_every_suite_is_called(self):
        try:
            import yaml
        except ImportError:
            self.skipTest("PyYAML not installed")
        ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        for suite in plan.SUITE_JOBS:
            self.assertIn(f"suites).{suite}", ci, suite)


if __name__ == "__main__":
    unittest.main()
