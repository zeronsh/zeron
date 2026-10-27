"""scripts/ci/reuse.py and plan.py's reuse: a suite is skipped only when nothing it reads changed."""

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "ci"))

import plan  # noqa: E402
import reuse  # noqa: E402


def write(root, path, text):
    full = pathlib.Path(root, path)
    full.parent.mkdir(parents=True, exist_ok=True)
    full.write_text(text)


class FakeRepo:
    """A small workspace in a temporary git repository."""

    def __init__(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = self.dir.name
        write(self.root, "Cargo.toml", '[workspace]\nmembers = ["crates/a", "crates/b", "crates/c", "apps/app"]\n'
                                       '[workspace.package]\nversion = "1.0.0"\n'
                                       '[workspace.dependencies]\nzb = { path = "crates/b" }\n'
                                       'serde = { version = "1" }\n')
        write(self.root, "Cargo.lock", '[[package]]\nname = "za"\nversion = "1.0.0"\n\n'
                                       '[[package]]\nname = "serde"\nversion = "1.0.1"\n'
                                       'source = "registry+https://github.com/rust-lang/crates.io-index"\n')
        write(self.root, "rust-toolchain.toml", "[toolchain]\n")
        write(self.root, "crates/a/Cargo.toml", '[package]\nname = "za"\n[dependencies]\nzb.workspace = true\n')
        write(self.root, "crates/a/src/lib.rs", 'const ICONS: &str = include_str!("../../c/assets/icons.json");\n')
        write(self.root, "crates/b/Cargo.toml", '[package]\nname = "zb"\n[dev-dependencies]\n'
                                                'serde = "1"\n')
        write(self.root, "crates/b/src/lib.rs", "pub fn b() {}\n")
        write(self.root, "crates/c/Cargo.toml", '[package]\nname = "zc"\n')
        write(self.root, "crates/c/src/lib.rs", "pub fn c() {}\n")
        write(self.root, "crates/c/assets/icons.json", "{}\n")
        write(self.root, "apps/app/Cargo.toml", '[package]\nname = "app"\n[target.\'cfg(windows)\'.dependencies]\n'
                                               'renamed = { package = "zc", path = "../../crates/c" }\n')
        write(self.root, "apps/app/src/main.rs",
              'fn main() { let _ = env!("CARGO_MANIFEST_DIR").to_owned() + "../../scripts/fixture.txt"; '
              'let _guard = "../.."; }\n')
        write(self.root, "scripts/fixture.txt", "x\n")
        self.git("init", "-q")
        self.commit()

    def git(self, *args):
        subprocess.run(["git", "-C", self.root, *args], check=True, capture_output=True,
                       env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                            "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"})

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "c", "--allow-empty")

    def keys(self, packages):
        old = dict(reuse.SUITE_PACKAGES), dict(reuse.SUITE_INPUTS)
        reuse.SUITE_PACKAGES.clear()
        reuse.SUITE_PACKAGES.update(packages)
        reuse.SUITE_INPUTS.clear()
        reuse.SUITE_INPUTS.update({suite: [] for suite in packages})
        try:
            return reuse.suite_keys(list(packages), self.root)[:2]
        finally:
            reuse.SUITE_PACKAGES.clear()
            reuse.SUITE_PACKAGES.update(old[0])
            reuse.SUITE_INPUTS.clear()
            reuse.SUITE_INPUTS.update(old[1])


class FingerprintTests(unittest.TestCase):
    def setUp(self):
        self.repo = FakeRepo()
        self.suites = {"a": ["za"], "b": ["zb"], "app": ["app"]}

    def tearDown(self):
        self.repo.dir.cleanup()

    def changed_after(self, path, text):
        before, _ = self.repo.keys(self.suites)
        write(self.repo.root, path, text)
        self.repo.commit()
        after, _ = self.repo.keys(self.suites)
        return {suite for suite in before if before[suite] != after[suite]}

    def test_dependency_closure(self):
        ws = reuse.Workspace(self.repo.root)
        self.assertEqual(ws.closure(["za"]), {"crates/a", "crates/b"})
        self.assertEqual(ws.closure(["app"]), {"apps/app", "crates/c"})  # renamed, target-specific
        self.assertEqual(ws.closure(["zb"]), {"crates/b"})  # a registry dev-dependency is not a package

    def test_files_named_from_outside_the_package_are_found(self):
        _, extra = self.repo.keys(self.suites)
        self.assertEqual(extra["a"], ["crates/c/assets/icons.json"])
        self.assertEqual(extra["app"], ["scripts/fixture.txt"])  # not "../.." (names no file)

    def test_a_dependency_change_reruns_its_dependents_only(self):
        self.assertEqual(self.changed_after("crates/b/src/lib.rs", "pub fn b() { }\n"), {"a", "b"})

    def test_a_leaf_change_reruns_only_its_suite(self):
        self.assertEqual(self.changed_after("crates/a/src/extra.rs", "\n"), {"a"})

    def test_a_named_file_counts_but_its_package_does_not(self):
        self.assertEqual(self.changed_after("crates/c/assets/icons.json", "[]\n"), {"a", "app"})
        self.assertEqual(self.changed_after("crates/c/src/other.rs", "\n"), {"app"})

    def test_the_lockfile_and_toolchain_rerun_everything(self):
        self.assertEqual(self.changed_after("Cargo.lock", "# lock 2\n"), {"a", "b", "app"})
        self.assertEqual(self.changed_after("rust-toolchain.toml", "[toolchain]\nchannel = \"1.99\"\n"),
                         {"a", "b", "app"})

    def test_a_version_bump_changes_no_fingerprint(self):
        cargo = pathlib.Path(self.repo.root, "Cargo.toml").read_text().replace('version = "1.0.0"', 'version = "1.0.1"')
        before, _ = self.repo.keys(self.suites)
        write(self.repo.root, "Cargo.toml", cargo)
        lock = pathlib.Path(self.repo.root, "Cargo.lock").read_text()
        write(self.repo.root, "Cargo.lock", lock.replace('name = "za"\nversion = "1.0.0"', 'name = "za"\nversion = "1.0.1"'))
        self.repo.commit()
        self.assertEqual(self.repo.keys(self.suites)[0], before)
        # A dependency's version is a real change.
        write(self.repo.root, "Cargo.lock", lock.replace('version = "1.0.1"\nsource', 'version = "1.0.2"\nsource'))
        self.repo.commit()
        self.assertNotEqual(self.repo.keys(self.suites)[0], before)

    def test_variants_are_part_of_the_key(self):
        with_browser = reuse.suite_keys(["core"], str(ROOT), variants={"core": "browser"})[0] \
            if (ROOT / "crates" / "ui" / "src").is_dir() else None
        if with_browser:
            plain = reuse.suite_keys(["core"], str(ROOT))[0]
            self.assertNotEqual(with_browser["core"], plain["core"])

    def test_the_planner_and_scheduler_are_no_input(self):
        self.assertEqual(self.changed_after("scripts/ci/plan.py", "# plan\n"), set())
        self.assertEqual(self.changed_after("scripts/ci/admit.py", "# admit\n"), set())
        self.assertEqual(self.changed_after(".github/workflows/ci.yml", "on: push\n"), {"a", "b", "app"})

    def test_ci_files_count_for_the_suites_that_run_them(self):
        keys = lambda: reuse.suite_keys(["core", "edge", "windows"], str(ROOT))[0]  # noqa: E731
        if not (ROOT / "crates" / "ui" / "src").is_dir():
            self.skipTest("Rust sources not checked out")
        self.assertIn("scripts/ci/nextest.toml", reuse.SUITE_INPUTS["core"])
        self.assertIn("scripts/ci/test-core.sh", reuse.SUITE_INPUTS["core"])
        self.assertNotIn("scripts/ci/nextest.toml", reuse.SUITE_INPUTS["edge"])
        self.assertIn(".github/actions/windows-ci-setup/**", reuse.SUITE_INPUTS["windows"])
        self.assertTrue(all(not reuse.matches(f"scripts/ci/{f}", reuse.COMMON)
                            for f in ("plan.py", "admit.py", "reuse.py", "emergency.py")))
        self.assertEqual(keys(), keys())

    def test_unrelated_files_change_nothing(self):
        self.assertEqual(self.changed_after("docs/notes.md", "hi\n"), set())

    def test_concat_paths_count(self):
        write(self.repo.root, "crates/b/src/fonts.rs",
              'const F: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../c/assets/icons.json");\n')
        self.repo.commit()
        _, extra = self.repo.keys(self.suites)
        self.assertIn("crates/c/assets/icons.json", extra["b"])

    def test_paths_the_scan_cannot_follow_make_the_suite_read_everything(self):
        write(self.repo.root, "crates/b/tests/walk.rs",
              'fn f() { let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(); }\n')
        self.repo.commit()
        self.assertIn("b", self.changed_after("docs/../scripts/other.txt", "y\n"))

    def test_keys_are_stable(self):
        self.assertEqual(self.repo.keys(self.suites), self.repo.keys(self.suites))


class RealWorkspaceTests(unittest.TestCase):
    """The zeron workspace itself (skipped where the sources are not checked out)."""

    def setUp(self):
        if not (ROOT / "crates" / "ui" / "src").is_dir():
            self.skipTest("Rust sources not checked out")

    def test_core_does_not_read_the_ui(self):
        ws = reuse.Workspace(str(ROOT))
        core = ws.closure(reuse.SUITE_PACKAGES["core"])
        self.assertNotIn("crates/ui", core)
        self.assertIn("crates/ui", ws.closure(reuse.SUITE_PACKAGES["windows"]))

    def test_mobile_reads_the_ui_icons_and_fonts(self):
        _, extra, _ = reuse.suite_keys(["ios"], str(ROOT))
        self.assertIn("crates/ui/src/file-icons.json", extra["ios"])

    def test_every_rust_suite_has_packages_that_exist(self):
        ws = reuse.Workspace(str(ROOT))
        for suite, packages in reuse.SUITE_PACKAGES.items():
            for package in packages:
                self.assertIn(package, ws.dirs, (suite, package))
        self.assertEqual(set(reuse.SUITE_INPUTS), set(plan.SUITE_JOBS))


class LookupTests(unittest.TestCase):
    def test_only_trusted_refs_count(self):
        key = "ci-pass-v1-windows-abc"
        caches = {"actions_caches": [
            {"key": f"{key}-tests", "ref": "refs/heads/main"},
            {"key": f"{key}-app", "ref": "refs/pull/9/merge"},  # another pull request
            {"key": f"{key}-tests", "ref": "refs/pull/7/merge"},
            {"key": f"{key}0-app", "ref": "refs/heads/main"},  # a longer key with the same prefix
        ]}
        api = lambda path: caches  # noqa: E731
        self.assertEqual(reuse.passed_jobs(api, "o/r", key, {"refs/pull/7/merge", "refs/heads/main"}), {"tests"})
        self.assertEqual(reuse.passed_jobs(api, "o/r", key, set()), set())


class ReusePlanTests(unittest.TestCase):
    def test_whole_suites(self):
        suites = {s: s in ("core", "ios") for s in plan.SUITE_JOBS}
        skip = plan.reused_jobs(suites, {"core": {"core-tests"}, "ios": {"ios-core"}})
        self.assertEqual(skip, {"core": ["core-tests"]})  # iOS: the simulator tests never passed
        running = plan.jobs_to_run(suites, skip)
        self.assertEqual(running["core"], [])
        self.assertEqual([j for j, _ in running["ios"]], ["ios-core", "ios-tests"])

    def test_single_jobs_of_splittable_suites(self):
        suites = {s: s in ("windows", "voice") for s in plan.SUITE_JOBS}
        passed = {"windows": {"tests"}, "voice": {"offline-linux"}}
        skip = plan.reused_jobs(suites, passed)
        running = plan.jobs_to_run(suites, skip)
        self.assertEqual(running["windows"], [("app", "windows")])
        self.assertEqual(running["voice"], [("windows-runtime", "windows")])
        counts = plan.reservation(running, "pull_request", "refs/pull/1/merge")
        self.assertEqual(counts, {"linux": 1, "windows": 2, "macos": 0})

    def test_unselected_suites_are_not_reused(self):
        suites = {s: False for s in plan.SUITE_JOBS}
        self.assertEqual(plan.reused_jobs(suites, {"core": {"core-tests"}}), {})


class PlanMainTests(unittest.TestCase):
    """plan.py end to end on a fake repository and API."""

    def run_plan(self, event, payload, caches, labels=(), files=None, head_repo="o/r", extra_env=None):
        repo = FakeRepo()
        self.addCleanup(repo.dir.cleanup)
        out = pathlib.Path(repo.root, "out")
        event_path = pathlib.Path(repo.root, "event.json")
        event_path.write_text(json.dumps(payload))
        files = files or [{"filename": "crates/b/src/lib.rs", "patch": "+x"}]

        def fake_api(path):
            if "/actions/caches" in path:
                return {"actions_caches": caches}
            if path.endswith("/files?per_page=100&page=1"):
                return files
            if "/pulls/" in path:
                return {"number": 3, "changed_files": 1, "labels": [{"name": n} for n in labels],
                        "head": {"repo": {"full_name": head_repo} if head_repo else None}}
            return {"files": files}

        env = {"GITHUB_EVENT_NAME": event, "GITHUB_REPOSITORY": "o/r", "GITHUB_REF": "refs/pull/3/merge",
               "GITHUB_EVENT_PATH": str(event_path), "GITHUB_OUTPUT": str(out), "GITHUB_TOKEN": "t"}
        old_env, old_api, cwd = dict(os.environ), plan.api, os.getcwd()
        packages, inputs = dict(reuse.SUITE_PACKAGES), dict(reuse.SUITE_INPUTS)
        try:
            os.environ.pop("CI_FORK_REUSE", None)
            os.environ.update(env)
            os.environ.update(extra_env or {})
            os.environ.pop("GITHUB_STEP_SUMMARY", None)
            plan.api = fake_api
            reuse.SUITE_PACKAGES.clear()
            reuse.SUITE_PACKAGES.update({"core": ["zb"], "ui": ["za"]})
            os.chdir(repo.root)
            from contextlib import redirect_stdout
            from io import StringIO
            with redirect_stdout(StringIO()):
                plan.main()
        finally:
            os.chdir(cwd)
            plan.api = old_api
            os.environ.clear()
            os.environ.update(old_env)
            reuse.SUITE_PACKAGES.clear()
            reuse.SUITE_PACKAGES.update(packages)
            reuse.SUITE_INPUTS.clear()
            reuse.SUITE_INPUTS.update(inputs)
        return dict(line.split("=", 1) for line in out.read_text().splitlines())

    def test_a_passed_suite_is_skipped_and_its_runner_not_reserved(self):
        first = self.run_plan("pull_request", {"pull_request": {"number": 3}}, [])
        keys = json.loads(first["keys"])
        self.assertTrue(json.loads(first["suites"])["core"])
        caches = [{"key": keys["core"] + "-core-tests", "ref": "refs/pull/3/merge"}]
        second = self.run_plan("pull_request", {"pull_request": {"number": 3}}, caches)
        self.assertFalse(json.loads(second["suites"])["core"])
        self.assertTrue(json.loads(second["suites"])["ui"])
        self.assertEqual(json.loads(second["skip"]), {"core": ["core-tests"]})
        self.assertLess(int(second["reservation"].split()[0].split("=")[1]),
                        int(first["reservation"].split()[0].split("=")[1]))

    def test_forks_reuse_only_mains_passes(self):
        first = self.run_plan("pull_request", {"pull_request": {"number": 3}}, [], head_repo="someone/r")
        key = json.loads(first["keys"])["core"] + "-core-tests"
        own = [{"key": key, "ref": "refs/pull/3/merge"}]
        self.assertTrue(json.loads(self.run_plan("pull_request", {"pull_request": {"number": 3}}, own,
                                                 head_repo="someone/r")["suites"])["core"])
        self.assertTrue(json.loads(self.run_plan("pull_request", {"pull_request": {"number": 3}}, own,
                                                 head_repo=None)["suites"])["core"])  # deleted fork
        mains = [{"key": key, "ref": "refs/heads/main"}]
        self.assertFalse(json.loads(self.run_plan("pull_request", {"pull_request": {"number": 3}}, mains,
                                                  head_repo="someone/r")["suites"])["core"])
        # CI_FORK_REUSE=own, once the merge queue is required.
        self.assertFalse(json.loads(self.run_plan("pull_request", {"pull_request": {"number": 3}}, own,
                                                  head_repo="someone/r",
                                                  extra_env={"CI_FORK_REUSE": "own"})["suites"])["core"])

    def test_the_merge_queue_reuses_nothing(self):
        first = self.run_plan("pull_request", {"pull_request": {"number": 3}}, [])
        caches = [{"key": json.loads(first["keys"])["core"] + "-core-tests", "ref": "refs/heads/main"}]
        group = self.run_plan("merge_group", {"merge_group": {"base_sha": "a" * 40, "head_sha": "b" * 40}}, caches)
        self.assertTrue(json.loads(group["suites"])["core"])

    def test_a_build_file_no_fingerprint_covers_disables_reuse(self):
        first = self.run_plan("pull_request", {"pull_request": {"number": 3}}, [])
        caches = [{"key": json.loads(first["keys"])["core"] + "-core-tests", "ref": "refs/pull/3/merge"}]
        # crates/c is in no selected suite (core reads crates/b, ui crates/a and b).
        again = self.run_plan("pull_request", {"pull_request": {"number": 3}}, caches,
                              files=[{"filename": "crates/b/src/lib.rs", "patch": "+x"},
                                     {"filename": "crates/c/src/lib.rs", "patch": "+x"}])
        self.assertTrue(json.loads(again["suites"])["core"])

    def test_ci_full_and_main_do_not_trust_pull_request_passes(self):
        first = self.run_plan("pull_request", {"pull_request": {"number": 3}}, [])
        caches = [{"key": json.loads(first["keys"])["core"] + "-core-tests", "ref": "refs/pull/3/merge"}]
        full = self.run_plan("pull_request", {"pull_request": {"number": 3}}, caches, labels=["ci:full"])
        self.assertTrue(json.loads(full["suites"])["core"])
        main = self.run_plan("push", {"before": "a" * 40, "after": "b" * 40}, caches)
        self.assertTrue(json.loads(main["suites"])["core"])


if __name__ == "__main__":
    unittest.main()
