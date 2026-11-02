#!/usr/bin/env python3
"""Isolated checks for build-crew.sh: stub cargo and dpkg, no real build, apt or Boat."""
import hashlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("build-crew.sh")
COMMIT = "0123456789abcdef0123456789abcdef01234567"
# Stub cargo: record where the build ran and write runnable executables, like the real one.
CARGO = """#!/bin/sh
printf '%s\\n' "$CARGO_HOME" "$CARGO_TARGET_DIR" "$PWD" "$COMET_DEFAULT_ENVIRONMENT" > "$CARGO_LOG"
mkdir -p "$CARGO_TARGET_DIR/release"
printf '#!/bin/sh\\necho comet 9.9.9\\n' > "$CARGO_TARGET_DIR/release/zeron"
cp "$CARGO_TARGET_DIR/release/zeron" "$CARGO_TARGET_DIR/release/zeron-engine"
chmod +x "$CARGO_TARGET_DIR/release/zeron" "$CARGO_TARGET_DIR/release/zeron-engine"
"""


@unittest.skipUnless(sys.platform == "linux", "build-crew.sh targets Linux devboxes")
class BuildCrewChecks(unittest.TestCase):
    def setUp(self):
        self.home = Path(tempfile.mkdtemp())
        self.addCleanup(subprocess.run, ["rm", "-rf", str(self.home)])
        stubs = self.home / "stubs"
        stubs.mkdir()
        (stubs / "cargo").write_text(CARGO)
        (stubs / "dpkg-query").write_text("#!/bin/sh\nprintf 'install ok installed'\n")
        for stub in stubs.iterdir():
            stub.chmod(0o755)
        self.cache = self.home / ".cache" / "crew-build"
        self.cache.mkdir(parents=True)
        self.cargo_log = self.home / "cargo.log"
        self.env = {"HOME": str(self.home), "PATH": f"{stubs}:/usr/bin:/bin", "CARGO_LOG": str(self.cargo_log),
                    "CREW_DEVBOX_SETUP_PROTECTED": "1"}

    def archive(self, path, prefix="Ashler-AI-comet-0123456/"):
        data = io.BytesIO()
        with tarfile.open(fileobj=data, mode="w:gz") as tar:
            manifest = b'[workspace]\n\n[workspace.package]\nversion = "9.9.9"\n'
            info = tarfile.TarInfo(prefix + "Cargo.toml")
            info.size = len(manifest)
            tar.addfile(info, io.BytesIO(manifest))
        path.write_bytes(data.getvalue())
        return hashlib.sha256(data.getvalue()).hexdigest()

    def run_script(self, *args):
        return subprocess.run(["bash", str(SCRIPT), *args], env=self.env, capture_output=True, text=True)

    def test_builds_in_the_cache_and_installs_by_commit_and_digest(self):
        archive = self.cache / f"comet-{COMMIT}.tar.gz"
        result = self.run_script(str(archive), self.archive(archive), COMMIT)
        self.assertEqual(result.returncode, 0, result.stderr)
        cargo_home, target, build_dir, environment = self.cargo_log.read_text().split("\n")[:4]
        for path in (cargo_home, target, build_dir):
            self.assertTrue(path.startswith(str(self.cache) + "/"), path)
        self.assertEqual(build_dir, f"{self.cache}/src-{COMMIT}")
        self.assertEqual(environment, "production")
        lines = dict(line.split("=", 1) for line in result.stdout.split())
        digest = hashlib.sha256(Path(target, "release", "zeron").read_bytes()).hexdigest()
        self.assertEqual(lines["CREW_BINARY_SHA256"], digest)
        installed = Path(lines["CREW_BINARY"])
        self.assertEqual(installed, self.home / ".local/lib/crew" / f"9.9.9-src-{COMMIT[:12]}-{digest}" / "comet")
        self.assertEqual(hashlib.sha256(installed.read_bytes()).hexdigest(), digest)
        self.assertEqual(installed.parent.stat().st_mode & 0o777, 0o755)
        self.assertTrue((installed.parent / "zeron-engine").is_file())

    def test_refuses_a_source_archive_outside_the_cache(self):
        archive = self.home / f"comet-{COMMIT}.tar.gz"
        result = self.run_script(str(archive), self.archive(archive), COMMIT)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("outside the snapshot", result.stderr)
        self.assertFalse(self.cargo_log.exists())

    def test_refuses_an_archive_whose_digest_differs_before_unpacking(self):
        archive = self.cache / f"comet-{COMMIT}.tar.gz"
        self.archive(archive)
        result = self.run_script(str(archive), "0" * 64, COMMIT)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("digest does not match", result.stderr)
        self.assertFalse((self.cache / f"src-{COMMIT}").exists())
        self.assertFalse(self.cargo_log.exists())


if __name__ == "__main__":
    unittest.main()
