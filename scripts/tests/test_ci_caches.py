"""scripts/ci/caches.py: the cache allowlist, the workflow lint that enforces it, and the janitor."""

import pathlib
import sys
import tempfile
import textwrap
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "ci"))

import caches  # noqa: E402
from caches import GB, MAIN, ZGX  # noqa: E402

RUST_CACHE = "Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2"
CACHE = "actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"
SAVE = "actions/cache/save@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"
RESTORE = "actions/cache/restore@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0"
NODE = "actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0"
ZIG = "mlugg/setup-zig@d1434d08867e3ee9daa34448df10607b98908d29 # v2.2.1"


class Workflows(unittest.TestCase):
    def test_the_repository_follows_the_cache_policy(self):
        errors = caches.lint(ROOT)
        self.assertEqual(errors, [], "\n" + "\n".join(errors))


def lint(workflows: dict[str, str], actions: dict[str, str] = None) -> list[str]:
    with tempfile.TemporaryDirectory() as root:
        for name, text in workflows.items():
            path = pathlib.Path(root, ".github", "workflows", name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(text), encoding="utf-8")
        for name, text in (actions or {}).items():
            path = pathlib.Path(root, ".github", "actions", name, "action.yml")
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(text), encoding="utf-8")
        return caches.lint(root)


def job(name: str, runner: str, steps: str, header: str = "") -> str:
    return header + f"""
on: push
jobs:
  {name}:
    runs-on: {runner}
    steps:
      - uses: actions/checkout@v4
""" + textwrap.indent(textwrap.dedent(steps).strip("\n"), " " * 6) + "\n"


class Lint(unittest.TestCase):
    """The lint catches each way a workflow could save a cache it shouldn't."""

    def one(self, steps: str, runner: str = "ubuntu-24.04", name: str = "build", header: str = "") -> list[str]:
        return lint({"w.yml": job(name, runner, steps, header)})

    def assertCaught(self, errors: list[str], *needles: str, count: int = 1):
        self.assertEqual(len(errors), count, errors)
        for error in errors:
            for needle in needles:
                self.assertIn(needle, error)

    def test_a_declared_cache_saved_from_its_ref_passes(self):
        self.assertEqual(self.one(f"""
            - uses: {RUST_CACHE}
              with:
                save-if: ${{{{ github.ref == 'refs/heads/main' }}}}
                cache-on-failure: true
            - name: Record the pass (CI skips this job while its inputs stay the same)
              if: inputs.pass-key != ''
              uses: {SAVE}
              with:
                path: rust-toolchain.toml
                key: ${{{{ inputs.pass-key }}}}-ios-core
            - uses: {RUST_CACHE}
              with:
                key: anything
                save-if: false
            - uses: {NODE}
              with:
                node-version: 24
            - uses: {ZIG}
              with:
                version: 0.16.0
                use-cache: false
        """, name="ios-core"), [])

    def test_rust_cache_without_save_if(self):
        self.assertCaught(self.one(f"""
            - uses: {RUST_CACHE}
              with:
                key: release
        """), "without `save-if`", "scripts/ci/caches.py")
        self.assertCaught(self.one(f"- uses: {RUST_CACHE}\n"), "without `save-if`")

    def test_rust_cache_with_another_condition(self):
        self.assertCaught(self.one(f"""
            - uses: {RUST_CACHE}
              with:
                save-if: ${{{{ github.ref != 'refs/heads/feature' }}}}
        """), "none of", "save-if: false")
        self.assertCaught(self.one(f"""
            - uses: {RUST_CACHE}
              with:
                save-if: ${{{{ github.ref == 'refs/heads/main' || github.event_name == 'push' }}}}
        """), "`||`")

    def test_an_undeclared_rust_cache(self):
        errors = self.one(f"""
            - uses: {RUST_CACHE}
              with:
                key: nightly
                save-if: ${{{{ github.ref == 'refs/heads/main' }}}}
        """, name="soak")
        self.assertCaught(errors, "v0-rust-nightly-soak-Linux-x64-", "no cache family declares",
                          "expected size and priority")

    def test_a_job_named_like_a_family_is_not_that_family(self):
        # voice's `windows-runtime` job: v0-rust-windows-runtime-..., not the shared `windows` cache.
        self.assertCaught(self.one(f"""
            - uses: {RUST_CACHE}
              with:
                save-if: ${{{{ github.ref == 'refs/heads/main' }}}}
        """, runner="windows-latest", name="windows-runtime"), "v0-rust-windows-runtime-Windows_NT-x64-")

    def test_a_family_saved_from_a_ref_it_does_not_allow(self):
        self.assertCaught(self.one(f"""
            - uses: {RUST_CACHE}
              with:
                key: zgx-tests
                save-if: ${{{{ github.ref == 'refs/heads/main' }}}}
        """, name="zgx-linux"), "zgx-tests-linux", "only refs/heads/zeron/zgx")

    def test_zgx_saves_through_save_caches(self):
        header = "env:\n  SAVE_CACHES: ${{ github.ref == 'refs/heads/zeron/zgx' }}\n"
        steps = f"""
            - uses: {RUST_CACHE}
              with:
                key: zgx-tests
                save-if: ${{{{ env.SAVE_CACHES == 'true' }}}}
            - id: zgx-cli
              uses: {RESTORE}
              with:
                path: ~/zgx-cli
                key: zgx-cli-${{{{ runner.os }}}}-${{{{ runner.arch }}}}-${{{{ steps.zgx-rev.outputs.rev }}}}
            - if: steps.zgx-cli.outputs.cache-hit != 'true' && env.SAVE_CACHES == 'true'
              uses: {SAVE}
              with:
                path: ~/zgx-cli
                key: zgx-cli-${{{{ runner.os }}}}-${{{{ runner.arch }}}}-${{{{ steps.zgx-rev.outputs.rev }}}}
        """
        self.assertEqual(self.one(steps, name="zgx-linux", header=header), [])
        self.assertCaught(self.one(steps, name="zgx-linux"), "SAVE_CACHES isn't defined", count=2)
        errors = self.one(steps, name="zgx-linux", header="env:\n  SAVE_CACHES: true\n")
        self.assertIn("SAVE_CACHES must be", errors[0])

    def test_actions_cache_is_refused(self):
        self.assertCaught(self.one(f"""
            - uses: {CACHE}
              with:
                path: ~/.cache/thing
                key: thing-${{{{ hashFiles('Cargo.lock') }}}}
        """), "actions/cache saves after every job", "actions/cache/restore")

    def test_cache_save_needs_a_ref_condition(self):
        self.assertCaught(self.one(f"""
            - uses: {SAVE}
              with:
                path: ~/apt-debs/*.deb
                key: apt-debs-v1-ui-browser-${{{{ hashFiles('scripts/ci/apt-install.sh') }}}}
        """), "no condition")
        self.assertCaught(self.one(f"""
            - if: steps.x.outputs.cache-hit != 'true'
              uses: {SAVE}
              with:
                path: ~/apt-debs/*.deb
                key: apt-debs-v1-ui-browser-${{{{ hashFiles('scripts/ci/apt-install.sh') }}}}
        """), "doesn't limit it to one ref")
        self.assertEqual(self.one(f"""
            - if: steps.x.outputs.cache-hit != 'true' && github.ref == 'refs/heads/main'
              uses: {SAVE}
              with:
                path: ~/apt-debs/*.deb
                key: apt-debs-v1-ui-browser-${{{{ hashFiles('scripts/ci/apt-install.sh') }}}}
        """), [])

    def test_a_pass_record_needs_no_condition_but_only_its_shape(self):
        self.assertCaught(self.one(f"""
            - if: inputs.pass-key != ''
              uses: {SAVE}
              with:
                path: target
                key: ${{{{ inputs.pass-key }}}}-tests
        """), "doesn't limit it to one ref")

    def test_setup_node_cache(self):
        self.assertCaught(self.one(f"""
            - uses: {NODE}
              with:
                node-version: 22
                cache: npm
                cache-dependency-path: edge/package-lock.json
        """), "`cache:`")
        self.assertCaught(self.one("""
            - uses: actions/setup-node@v5
              with:
                node-version: 22
        """), "package-manager-cache: false")

    def test_setup_zig_cache(self):
        self.assertCaught(self.one(f"""
            - uses: {ZIG}
              with:
                version: 0.16.0
        """, runner="macos-latest"), "use-cache: false")

    def test_a_composite_action_saves_for_the_callers_that_turn_it_on(self):
        action = f"""
            name: setup
            inputs:
              cache-name:
                required: true
              save-cache:
                default: 'false'
            runs:
              using: composite
              steps:
                - uses: {RUST_CACHE}
                  with:
                    prefix-key: ci-linux-v1
                    shared-key: ${{{{ inputs.cache-name }}}}
                    add-job-id-key: false
                    save-if: ${{{{ inputs.save-cache == 'true' && github.ref == 'refs/heads/main' }}}}
        """
        caller = """
            - uses: ./.github/actions/setup
              with:
                cache-name: {name}
                {save}
        """
        self.assertEqual(lint({"w.yml": job("t", "ubuntu-24.04", caller.format(name="ui", save="save-cache: 'true'"))},
                              {"setup": action}), [])
        self.assertEqual(lint({"w.yml": job("t", "ubuntu-24.04", caller.format(name="other", save=""))},
                              {"setup": action}), [])
        errors = lint({"w.yml": job("t", "ubuntu-24.04", caller.format(name="voice", save="save-cache: 'true'"))},
                      {"setup": action})
        self.assertCaught(errors, "ci-linux-v1-voice-Linux-x64-", "via .github/actions/setup/action.yml")

    def test_a_saving_job_on_an_unknown_runner(self):
        self.assertCaught(self.one(f"""
            - uses: {RUST_CACHE}
              with:
                save-if: ${{{{ github.ref == 'refs/heads/main' }}}}
        """, runner="${{ matrix.runner }}"), "can't tell which runner")

    def test_a_run_script_is_not_read_as_steps(self):
        self.assertEqual(self.one(f"""
            - run: |
                cat <<EOF
                - uses: {CACHE}
                EOF
        """), [])

    def test_a_missing_root_is_an_error_not_a_pass(self):
        with self.assertRaises(FileNotFoundError):
            caches.lint(pathlib.Path(tempfile.gettempdir(), "no-such-repository-here"))


class Families(unittest.TestCase):
    LIVE = {
        "v0-rust-windows-Windows_NT-x64-15d0a0c1-9f8dbf8f": "windows",
        "ci-linux-v1-ui-Linux-x64-61726f36-39c2c8be": "linux-ui",
        "ci-linux-v1-core-Linux-x64-61726f36-39c2c8be": "linux-core",
        "v0-rust-zgx-tests-zgx-macos-Darwin-arm64-3b5582f9-f54c6c10": "zgx-tests-macos",
        "v0-rust-zgx-tests-zgx-linux-Linux-x64-61726f36-39c2c8be": "zgx-tests-linux",
        "v0-rust-ios-tests-Darwin-arm64-3b5582f9-f54c6c10": "ios-tests",
        "v0-rust-macos-native-Darwin-arm64-3b5582f9-f54c6c10": "macos-native",
        "v0-rust-ios-core-Linux-x64-61726f36-39c2c8be": "ios-core",
        "apt-debs-v1-ui-browser-c0ae4a0d3c4cd45a0715731f43ad184f7ec2360315469c0c0d55ef9a49e61469": "apt-ui-browser",
        "zgx-ghostty-7103ce606029f24528a318ebff8b5696e2b54a814fec2241227a0f45b02e42d6": "zgx-ghostty",
        "zgx-cli-macOS-ARM64-7cd25fc0b1a2c3d4e5f60718293a4b5c6d7e8f90": "zgx-cli-macos",
        "zgx-cli-Linux-X64-7cd25fc0b1a2c3d4e5f60718293a4b5c6d7e8f90": "zgx-cli-linux",
        "ci-pass-v1-frame_source-be6cb1245beb24194f35b7c0c06e665d41310905-macos-frame-source": "pass-records",
        # Dropped: undeclared.
        "v0-rust-windows-runtime-Windows_NT-x64-15d0a0c1-9f8dbf8f": None,
        "ci-linux-v1-voice-Linux-x64-61726f36-39c2c8be": None,
        "v0-rust-macos-frame-source-Darwin-arm64-3b5582f9-f54c6c10": None,
        "v0-rust-android-apk-Linux-x64-61726f36-39c2c8be": None,
        "v0-rust-ios-core-upload-Darwin-arm64-3b5582f9-f54c6c10": None,
        "v0-rust-zgx-beta-macos-Darwin-arm64-30c62bd1-f3a6afd9": None,
        "apt-debs-v1-voice-c0ae4a0d3c4cd45a0715731f43ad184f7ec2360315469c0c0d55ef9a49e61469": None,
        "node-cache-Linux-x64-npm-11b6cfc5cedf9b91489e2429db7ae6e606caa1f3f83dd9409fffbf797655e889": None,
        "setup-zig-tarball-zig-aarch64-macos-0.16.0": None,
    }

    def test_keys_belong_to_their_families(self):
        for key, name in self.LIVE.items():
            family = caches.family_of(key)
            self.assertEqual(family.name if family else None, name, key)

    def test_ranks_follow_the_order_and_names_are_unique(self):
        self.assertEqual([f.rank for f in caches.FAMILIES], list(range(1, len(caches.FAMILIES) + 1)))
        names = [f.name for f in caches.ALL_FAMILIES]
        self.assertEqual(len(names), len(set(names)))

    def test_no_key_belongs_to_two_families(self):
        for key in self.LIVE:
            self.assertLessEqual(sum(f.matches(key) for f in caches.ALL_FAMILIES), 1, key)

    def test_the_declared_families_fit_the_budget(self):
        # A new family needs room: drop or shrink another, or the janitor drops the lowest priority.
        total = sum(f.size_gb for f in caches.FAMILIES) * GB
        self.assertLessEqual(total, caches.BUDGET, "the declared families' sizes exceed BUDGET")
        self.assertLess(caches.BUDGET, 10 * GB)

    def test_every_family_names_who_may_save_it(self):
        for f in caches.FAMILIES:
            self.assertTrue(f.refs and all(r in (MAIN, ZGX) for r in f.refs), f.name)
            self.assertGreater(f.size_gb, 0, f.name)
        self.assertIsNone(caches.PASS.refs)


def e(key, ref=MAIN, size=1, created="2026-11-11T10:00:00Z", id=None):
    e.n = getattr(e, "n", 0) + 1
    return caches.Entry(id or e.n, key, ref, size, created)


H = "61726f36-39c2c8be"
PASS_KEY = "ci-pass-v1-core-95591ce05b90a5069117b287b82cad3c5d7111cc-core-tests"


class Janitor(unittest.TestCase):
    def deleted(self, sel):
        return {(d.key, d.ref) for d, _ in sel.delete}

    def kept(self, sel):
        return {(k.key, k.ref) for k, _ in sel.keep}

    def test_undeclared_entries_go(self):
        sel = caches.select([e("node-cache-Linux-x64-npm-abc"), e(f"ci-linux-v1-voice-Linux-x64-{H}"),
                             e(f"ci-linux-v1-core-Linux-x64-{H}")])
        self.assertEqual(self.deleted(sel), {("node-cache-Linux-x64-npm-abc", MAIN),
                                             (f"ci-linux-v1-voice-Linux-x64-{H}", MAIN)})
        self.assertIn("undeclared", sel.delete[0][1])

    def test_entries_from_refs_their_family_does_not_allow_go(self):
        ghostty = "zgx-ghostty-" + "7" * 64
        tag = "refs/heads/refs/tags/beta-v0.2.139-beta.7"
        sel = caches.select([
            e(f"ci-linux-v1-core-Linux-x64-{H}", ref="refs/pull/1219/merge"),
            e(ghostty, ref=tag),
            e(ghostty, ref="refs/pull/7/merge"),
            e(ghostty, ref=ZGX),
            e(ghostty, ref=MAIN),
            e(f"v0-rust-zgx-tests-zgx-linux-Linux-x64-{H}", ref="refs/heads/zeron/linux-headless-fixes"),
            e(PASS_KEY, ref="refs/pull/1219/merge"),
        ])
        self.assertEqual(self.kept(sel), {(ghostty, ZGX), (PASS_KEY, "refs/pull/1219/merge")})
        self.assertTrue(all("may not save" in why for _, why in sel.delete))

    def test_pass_records_stay_only_where_a_run_trusts_them(self):
        trusted = (MAIN, ZGX, "refs/pull/1219/merge", "refs/pull/7/merge")
        never = ("refs/heads/gh-readonly-queue/main/pr-1-abc",  # the merge queue reuses nothing
                 "refs/heads/zeron/some-branch",  # a run by hand
                 "refs/tags/v0.2.140", "refs/pull/7/head", "refs/heads/main-copy", "")
        sel = caches.select([e(PASS_KEY, ref=r) for r in trusted + never])
        self.assertEqual(self.kept(sel), {(PASS_KEY, r) for r in trusted})
        self.assertEqual(self.deleted(sel), {(PASS_KEY, r) for r in never})

    def test_only_the_newest_generation_per_family_and_ref_stays(self):
        old = e(f"v0-rust-windows-Windows_NT-x64-15d0a0c1-00000001", created="2026-11-10T09:00:00Z")
        new = e(f"v0-rust-windows-Windows_NT-x64-15d0a0c1-00000002", created="2026-11-11T09:00:00Z")
        cli_old = e("zgx-cli-Linux-X64-aaaa", ref=ZGX, created="2026-11-01T00:00:00Z")
        cli_new = e("zgx-cli-Linux-X64-bbbb", ref=ZGX, created="2026-11-02T00:00:00Z")
        cli_mac = e("zgx-cli-macOS-ARM64-aaaa", ref=ZGX, created="2026-11-01T00:00:00Z")
        passes = [e(PASS_KEY.replace("core-tests", job), created=f"2026-11-0{i + 1}T00:00:00Z")
                  for i, job in enumerate(("a", "b", "c"))]
        sel = caches.select([old, new, cli_old, cli_new, cli_mac] + passes)
        self.assertEqual(self.deleted(sel), {(old.key, MAIN), (cli_old.key, ZGX)})
        self.assertIn("older windows generation", dict((d.key, w) for d, w in sel.delete)[old.key])

    def test_over_budget_the_lowest_priority_families_go_whole(self):
        entries = [
            e(f"v0-rust-windows-Windows_NT-x64-{H}", size=4 * GB),                        # 1
            e(f"ci-linux-v1-core-Linux-x64-{H}", size=2 * GB),                            # 3
            e(f"v0-rust-zgx-tests-zgx-linux-Linux-x64-{H}", ref=ZGX, size=2 * GB),        # 5
            e(f"v0-rust-ios-core-Linux-x64-{H}", size=1 * GB),                            # 8
            e("zgx-ghostty-" + "1" * 64, ref=ZGX, size=1 * GB),                           # 10
            e(PASS_KEY, size=200),
        ]
        sel = caches.select(entries, budget=7 * GB)
        self.assertEqual({d.key for d, _ in sel.delete},
                         {f"v0-rust-ios-core-Linux-x64-{H}", "zgx-ghostty-" + "1" * 64,
                          f"v0-rust-zgx-tests-zgx-linux-Linux-x64-{H}"})
        self.assertLessEqual(sel.kept_bytes, 7 * GB)
        self.assertIn((PASS_KEY, MAIN), self.kept(sel))
        self.assertTrue(all("budget" in why for _, why in sel.delete))

    def test_pass_records_do_not_count_against_the_budget(self):
        # Pull requests (a fork's too) can save any number of them: they never push out a cache.
        windows = e(f"v0-rust-windows-Windows_NT-x64-{H}", size=GB)
        passes = [e(PASS_KEY.replace("core-tests", f"job{n}"), ref=f"refs/pull/{n}/merge", size=60 * 1024)
                  for n in range(1, 200)]
        sel = caches.select([windows] + passes, budget=GB + 10**6)
        self.assertEqual(sel.delete, [])
        self.assertEqual(len(sel.keep), 200)

    def test_within_budget_nothing_declared_goes(self):
        entries = [e(f"v0-rust-windows-Windows_NT-x64-{H}", size=GB), e(f"ci-linux-v1-ui-Linux-x64-{H}", size=GB)]
        self.assertEqual(caches.select(entries).delete, [])

    def test_a_large_entry_named_like_a_pass_record_goes(self):
        sel = caches.select([e(PASS_KEY, ref="refs/pull/3/merge", size=500 * 10**6)])
        self.assertEqual(len(sel.delete), 1)

    def test_entries_from_the_api_and_from_gh(self):
        api = caches.entry({"id": 7, "key": "k", "ref": MAIN, "size_in_bytes": 5,
                            "created_at": "2026-11-11T00:00:00Z", "last_accessed_at": "2026-11-11T01:00:00Z"})
        gh = caches.entry({"id": 7, "key": "k", "ref": MAIN, "sizeInBytes": 5,
                           "createdAt": "2026-11-11T00:00:00Z", "lastAccessedAt": "2026-11-11T01:00:00Z"})
        self.assertEqual(api, gh)

    def test_the_summary_lists_both_sides_with_sizes(self):
        sel = caches.select([e("node-cache-x", size=274 * 10**6), e(f"ci-linux-v1-ui-Linux-x64-{H}", size=GB),
                             e(PASS_KEY, size=240)])
        text = caches.summary(sel, dry_run=True)
        self.assertIn("dry run", text)
        self.assertIn("`node-cache-x`", text)
        self.assertIn("274.00 MB", text)
        self.assertIn("linux-ui", text)
        self.assertIn("pass-records | - | 1 entries", text)


if __name__ == "__main__":
    unittest.main()
