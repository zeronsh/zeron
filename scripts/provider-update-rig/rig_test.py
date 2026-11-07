#!/usr/bin/env python3
"""The rig's own promises: python3 scripts/provider-update-rig/rig_test.py"""
import os, sys, tempfile, unittest

os.environ.setdefault("RIG_DIR", tempfile.gettempdir())
os.environ.setdefault("RIGCLIENT", "/nonexistent/rigclient")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rig


class RigIsolation(unittest.TestCase):
    def test_every_other_provider_is_pinned_inside_the_run(self):
        env = rig.engine_env("/run/home", "/run/data", 1234, "/usr/bin", "pi",
                             {"PI_EXECUTABLE": "/run/home/pi"}, "/run")
        self.assertEqual(env["PI_EXECUTABLE"], "/run/home/pi")
        self.assertEqual(sorted(rig.PINS), sorted(rig.ALL))
        for harness, var in rig.PINS.items():
            if harness != "pi":
                self.assertTrue(env[var].startswith("/run/not-installed/"), var)

    def test_a_target_found_by_discovery_is_not_pinned_away(self):
        env = rig.engine_env("/run/home", "/run/data", 1234, "/usr/bin", "cursor", {}, "/run")
        self.assertNotIn(rig.PINS["cursor"], env)
        self.assertIn(rig.PINS["claude-code"], env)

    def test_pnpm_lives_where_pnpm_setup_puts_it(self):
        self.assertEqual(rig.pnpm_home_under("linux"), ".local/share/pnpm")
        self.assertEqual(rig.pnpm_home_under("darwin"), "Library/pnpm")

    def test_the_system_package_row_never_applies(self):
        with tempfile.TemporaryDirectory() as home:
            self.assertTrue(rig.system_git(home)[4].get("check_only"))


if __name__ == "__main__":
    unittest.main()
