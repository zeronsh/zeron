#!/usr/bin/env python3
"""Exercise the packaged desktop installer without building or installing Zeron."""
import fcntl
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tarfile
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "dist/linux/install.sh"


class LinuxInstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / "home with spaces"
        self.package = self.root / "package"
        self.package.mkdir()
        shutil.copy(INSTALLER, self.package / "install.sh")
        (self.package / "zeron.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=Zeron\nExec=zeron %u\nTryExec=zeron\nIcon=zeron\n"
        )
        (self.package / "zeron.png").write_bytes(b"icon")
        self.binary = self.package / "zeron"
        self.binary.write_text("#!/bin/sh\necho 'zeron 1.2.3'\n")
        self.binary.chmod(0o755)
        self.app = self.home / ".zeron/app"
        self.command = self.home / ".local/bin/zeron"
        self.env = dict(os.environ, HOME=str(self.home), XDG_DATA_HOME=str(self.home / "xdg-data"))

    def install(self, success=True):
        result = subprocess.run(
            ["bash", str(self.package / "install.sh")], env=self.env,
            capture_output=True, text=True, timeout=10,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def test_fresh_install_and_repeat_use_versioned_layout_without_service(self):
        self.install()
        self.assertEqual(self.command.resolve(), self.app / "1.2.3/zeron")
        self.assertTrue((self.app / "1.2.3/.zeron-update-sha256").is_file())
        desktop = self.home / "xdg-data/applications/zeron.desktop"
        self.assertIn(f'Exec=/usr/bin/env "{self.command}" %u', desktop.read_text())
        self.assertFalse((self.home / ".config/systemd").exists())
        inode = self.command.stat().st_ino
        self.install()
        self.assertEqual(self.command.stat().st_ino, inode)

    def test_migrates_legacy_regular_binary(self):
        self.command.parent.mkdir(parents=True)
        self.command.write_text("old binary")
        with self.command.open() as old:
            self.install()
            self.assertEqual(old.read(), "old binary")
        self.assertTrue(self.command.is_symlink())
        self.assertEqual(self.command.resolve(), self.app / "1.2.3/zeron")

    def test_existing_headless_link_preserves_previous_binary(self):
        old = self.app / "1.0.0/zeron"
        old.parent.mkdir(parents=True)
        old.write_text("old binary")
        (self.app / "current").symlink_to(old.parent)
        self.command.parent.mkdir(parents=True)
        self.command.symlink_to(self.app / "current/zeron")
        self.install()
        self.assertEqual(old.read_text(), "old binary")
        self.assertEqual(self.command.resolve(), self.app / "1.2.3/zeron")

    def test_conflicting_version_and_concurrent_update_are_rejected(self):
        self.install()
        installed = self.command.read_bytes()
        self.binary.write_text("#!/bin/sh\necho 'zeron 1.2.3'\n# different build\n")
        self.assertIn("different contents", self.install(False).stderr)
        self.assertEqual(self.command.read_bytes(), installed)
        with (self.app / ".update.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertIn("in progress", self.install(False).stderr)

    @unittest.skipUnless(shutil.which("gio"), "GIO desktop launcher is unavailable")
    def test_desktop_launcher_handles_spaces_and_shell_characters(self):
        self.home = self.root / 'home \\ $ ` " % with spaces'
        self.app = self.home / ".zeron/app"
        self.command = self.home / ".local/bin/zeron"
        self.env.update(HOME=str(self.home), XDG_DATA_HOME=str(self.home / "xdg-data"))
        self.install()
        desktop = self.home / "xdg-data/applications/zeron.desktop"
        result = subprocess.run(
            ["gio", "launch", str(desktop)], env=self.env,
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "zeron 1.2.3")

    def headless_install(self):
        archive = self.root / "release.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(self.package, arcname="zeron-1.2.3-linux-test")
        commands = self.root / "commands"
        commands.mkdir(exist_ok=True)
        curl = commands / "curl"
        curl.write_text(
            '#!/bin/sh\ncase "$*" in\n'
            '  *latest.txt*) echo 1.2.3 ;;\n'
            '  *) while [ "$1" != -o ]; do shift; done; cp "$TEST_ARCHIVE" "$2" ;;\n'
            'esac\n'
        )
        curl.chmod(0o755)
        env = dict(self.env, PATH=f'{commands}:{os.environ["PATH"]}',
                   XDG_RUNTIME_DIR="", TEST_ARCHIVE=str(archive))
        return subprocess.run(
            ["sh", str(INSTALLER.parents[2] / "edge/src/install.sh")],
            env=env, capture_output=True, text=True, timeout=10,
        )

    def test_web_installer_shares_layout_and_lock(self):
        result = self.headless_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.command.resolve(), self.app / "1.2.3/zeron")
        self.assertFalse(any(self.app.glob(".install-*")))
        with (self.app / ".update.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.headless_install()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("in progress", result.stderr)

    def test_web_installer_rejects_incomplete_archive_without_publishing(self):
        self.binary.unlink()
        result = self.headless_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.app / "1.2.3").exists())
        self.assertFalse((self.app / "current").exists())
        self.assertFalse(any(self.app.glob(".install-*")))


if __name__ == "__main__":
    unittest.main()
