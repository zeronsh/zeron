#!/usr/bin/env python3
"""Isolated checks: no real systemctl, Crew, Boat, services, or credentials."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


bootstrap = load("configure-host")


class BootstrapChecks(unittest.TestCase):
    def setUp(self):
        self.addCleanup(os.umask, os.umask(0o077))
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name).resolve()
        self.binary = self.home / "releases" / "verified-version" / "comet"
        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(b"#!/bin/sh\nexit 0\n")
        self.binary.chmod(0o500)
        self.env = {
            "HOME": str(self.home), "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "BOAT_ID": "bx_jdyh7avc", "COMET_DEVBOX_PROVIDER": "boat",
            "COMET_DEVBOX_ID": "bx_jdyh7avc",
            "CREW_DEVBOX_NAME": "owner-ashler-dev", "CREW_BINARY": str(self.binary),
            "CREW_CHANNEL": "staging", "CREW_BINARY_SHA256": hashlib.sha256(self.binary.read_bytes()).hexdigest(),
        }
        self.addCleanup(patch.stopall)
        patch.dict(os.environ, self.env, clear=True).start()
        patch.object(bootstrap.sys, "platform", "linux").start()
        patch.object(bootstrap, "service_state", return_value=(None, False, 0)).start()
        patch.object(bootstrap, "port_busy", return_value=False).start()

    def configure(self):
        bootstrap.configure()

    def test_wrong_native_identity_and_digest_write_nothing(self):
        for key, bad in (("BOAT_ID", "bx_23456789"), ("CREW_BINARY_SHA256", "0" * 64),
                         ("COMET_DEVBOX_PROVIDER", "namespace")):
            with self.subTest(key=key), patch.dict(os.environ, {key: bad}):
                with self.assertRaises(bootstrap.Refusal):
                    self.configure()
                self.assertFalse((self.home / ".config").exists())

    def test_configure_is_private_repeatable_and_never_starts_or_authenticates(self):
        with patch.object(bootstrap, "crew", side_effect=AssertionError("must not invoke Crew")):
            self.configure()
            self.configure()
        config = self.home / ".config/crew-devbox/staging.env"
        self.assertEqual(config.stat().st_mode & 0o777, 0o600)
        self.assertNotIn("BOAT_ID=", config.read_text())
        self.assertFalse((self.home / ".crew-staging").exists())
        with patch.dict(os.environ, {"CREW_EXISTING_DEVICE_ID": "other-device"}):
            with self.assertRaises(bootstrap.Refusal):
                self.configure()

    # Crew's device picker truncates long names, so `<Owner> · Devbox · <machine>` rows
    # all read the same; the provider ID is what tells two devboxes apart.
    def test_devices_are_named_after_their_provider_id(self):
        self.configure()
        self.assertEqual(bootstrap.load_config()["ZERON_DEVICE_NAME"], "devbox-jdyh7avc")

    def test_namespace_devices_are_named_after_their_instance_id(self):
        is_dir = Path.is_dir
        with patch.dict(os.environ, {"BOAT_ID": "", "COMET_DEVBOX_PROVIDER": "namespace", "COMET_DEVBOX_ID": "abcdefgh23456"}), \
             patch.object(Path, "is_dir", autospec=True, side_effect=lambda path: str(path) == "/.namespace/tasks" or is_dir(path)):
            self.configure()
            self.assertEqual(bootstrap.load_config()["ZERON_DEVICE_NAME"], "devbox-abcdefgh23456")

    # Crew owns a registered device's name, so a box set up under the old naming must
    # still re-run setup instead of refusing because the default name changed.
    def test_rerun_keeps_the_name_an_existing_binding_was_set_up_with(self):
        with patch.object(bootstrap, "device_name", return_value="Real Owner · Devbox · owner-ashler-dev"):
            self.configure()
        self.configure()
        self.assertEqual(bootstrap.load_config()["ZERON_DEVICE_NAME"], "Real Owner · Devbox · owner-ashler-dev")


    def test_namespace_does_not_install_a_boat_unit(self):
        is_dir = Path.is_dir
        with patch.dict(os.environ, {"BOAT_ID": "", "COMET_DEVBOX_PROVIDER": "namespace", "COMET_DEVBOX_ID": "abcdefgh23456"}), \
             patch.object(Path, "is_dir", autospec=True, side_effect=lambda path: str(path) == "/.namespace/tasks" or is_dir(path)):
            self.configure()
            self.assertFalse(bootstrap.unit_path().exists())

    def test_child_environment_drops_parent_authority(self):
        self.configure()
        config = bootstrap.load_config()
        with patch.dict(os.environ, {"COMET_EDGE_TOKEN": "parent-secret", "ZERON_EDGE_URL": "https://wrong.example"}):
            self.assertNotIn("COMET_EDGE_TOKEN", bootstrap.environment(config))
            self.assertNotIn("ZERON_EDGE_URL", bootstrap.environment(config))


    def existing_profile(self):
        profile = self.home / ".crew-staging"
        profile.mkdir(mode=0o700)
        (profile / "device-id").write_text("d861aa09-a7e5-4853-9ec2-053ff8191b1b\n")
        (profile / "crew-runtime.json").write_text(json.dumps({
            "edgeUrl": bootstrap.coordinates()["COMET_EDGE_URL"],
            "scaffoldUrl": bootstrap.coordinates()["COMET_SCAFFOLD_URL"],
            "projectScope": "ashler-staging", "deploymentId": "ashler-staging",
            "ipcPort": 27957,
        }))
        (profile / "saved-auth").write_text("synthetic-private-auth")
        return profile

    def test_explicit_existing_device_adoption_preserves_profile_and_auth(self):
        profile = self.existing_profile()
        before = {path.name: path.read_bytes() for path in profile.iterdir()}
        with patch.dict(os.environ, {"CREW_EXISTING_DEVICE_ID": "d861aa09-a7e5-4853-9ec2-053ff8191b1b"}):
            self.configure()
            self.configure()
            bootstrap.load_config()
        self.assertEqual(before, {path.name: path.read_bytes() for path in profile.iterdir()})
        (profile / "device-id").write_text("other-device")
        with self.assertRaises(bootstrap.Refusal):
            bootstrap.load_config()

    def test_existing_device_or_runtime_mismatch_writes_nothing(self):
        profile = self.existing_profile()
        with patch.dict(os.environ, {"CREW_EXISTING_DEVICE_ID": "other-device"}):
            with self.assertRaises(bootstrap.Refusal):
                self.configure()
        with patch.dict(os.environ, {"CREW_EXISTING_DEVICE_ID": "d861aa09-a7e5-4853-9ec2-053ff8191b1b"}):
            runtime = profile / "crew-runtime.json"
            settings = json.loads(runtime.read_text())
            settings["deploymentId"] = "other-deployment"
            runtime.write_text(json.dumps(settings))
            with self.assertRaises(bootstrap.Refusal):
                self.configure()
        self.assertFalse((self.home / ".config").exists())


    def test_digest_guard_is_private_and_rechecks_automatic_starts(self):
        self.configure()
        guard = bootstrap.guard_path()
        self.assertEqual(guard.stat().st_mode & 0o777, 0o600)
        with patch.object(sys, "argv", ["bootstrap.py", "verify"]):
            bootstrap.main()
            self.binary.chmod(0o700)
            self.binary.write_bytes(b"#!/bin/sh\nexit 1\n")
            with self.assertRaises(bootstrap.Refusal):
                bootstrap.main()


    def test_control_failure_does_not_disclose_command_arguments(self):
        process = unittest.mock.Mock(pid=123)
        process.stdout.fileno.return_value = 10
        exited = SimpleNamespace(si_status=1, si_code=os.CLD_EXITED if hasattr(os, "CLD_EXITED") else 1)
        with patch.object(bootstrap.subprocess, "Popen", return_value=process), \
             patch.object(bootstrap.os, "set_blocking"), patch.object(bootstrap.os, "read", return_value=b""), \
             patch.object(bootstrap.os, "waitid", return_value=exited, create=True), \
             patch.object(bootstrap.os, "P_PID", 1, create=True), \
             patch.object(bootstrap.os, "WEXITED", 4, create=True), \
             patch.object(bootstrap.os, "WNOHANG", 1, create=True), \
             patch.object(bootstrap.os, "WNOWAIT", 0x1000000, create=True), \
             patch.object(bootstrap.os, "CLD_EXITED", exited.si_code, create=True), \
             patch.object(bootstrap.os, "killpg"):
            with self.assertRaises(bootstrap.Refusal) as failure:
                bootstrap.command(["synthetic-cli", "--token", "synthetic-private-secret"])
        self.assertNotIn("synthetic-private-secret", str(failure.exception))

    def test_permissive_umask_still_creates_private_ancestors_and_is_restored(self):
        previous_umask = os.umask(0o002)
        try:
            self.configure()
            self.configure()
            self.assertEqual(os.umask(0o002), 0o002)
            for relative in (".config", ".config/crew-devbox", ".local", ".local/bin",
                             ".local/share", ".local/share/crew-devbox"):
                self.assertEqual((self.home / relative).stat().st_mode & 0o777, 0o700)
        finally:
            os.umask(previous_umask)

    @unittest.skipUnless(sys.platform == "linux" and hasattr(os, "waitid"), "Requires real Linux native children")
    def test_native_control_and_installed_wrapper_create_private_profile_under_umask002(self):
        self.binary.chmod(0o700)
        self.binary.write_bytes(b'#!/bin/sh\nmkdir -p "$COMET_DATA_DIR"\nprintf synthetic > "$COMET_DATA_DIR/fake-credential"\n')
        self.binary.chmod(0o500)
        digest = hashlib.sha256(self.binary.read_bytes()).hexdigest()
        with patch.dict(os.environ, {"CREW_BINARY_SHA256": digest}):
            self.configure()
            config = bootstrap.load_config()
            profile = self.home / ".crew-staging"
            credential = profile / "fake-credential"
            previous_umask = os.umask(0o002)
            try:
                for entry in ("control", "installed-wrapper"):
                    with self.subTest(entry=entry):
                        try:
                            if entry == "control":
                                bootstrap.crew(config, ("status",))
                            else:
                                subprocess.run([str(self.home / ".local/bin/crew-devbox-staging"), "login"],
                                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                               stderr=subprocess.PIPE, check=True, timeout=10)
                            self.assertEqual(profile.stat().st_mode & 0o777, 0o700)
                            self.assertEqual(credential.stat().st_mode & 0o777, 0o600)
                            bootstrap.load_config()  # The next real launch must accept the created profile.
                            self.assertEqual(os.umask(0o002), 0o002)
                        finally:
                            if credential.exists():
                                credential.unlink()
                            if profile.exists():
                                profile.rmdir()
            finally:
                os.umask(previous_umask)

    def test_unrelated_shared_library_directory_is_preserved_while_configuring(self):
        local = self.home / ".local"
        local.mkdir(mode=0o755)
        local.chmod(0o755)
        for name in ("bin", "share"):
            (local / name).mkdir(mode=0o755)
            (local / name).chmod(0o755)
        unrelated = local / "lib"
        unrelated.mkdir()
        unrelated.chmod(0o775)
        sentinel = unrelated / "user-data"
        sentinel.write_text("preserve unrelated library data")
        self.configure()
        self.configure()
        self.assertEqual(unrelated.stat().st_mode & 0o777, 0o775)
        self.assertEqual(sentinel.read_text(), "preserve unrelated library data")
        self.assertEqual(set(unrelated.iterdir()), {sentinel})
        for directory in (local, local / "bin", local / "share"):
            self.assertEqual(directory.stat().st_mode & 0o777, 0o755)

    def test_existing_shared_writable_directory_is_refused_without_chmod(self):
        existing = self.home / ".local"
        existing.mkdir()
        existing.chmod(0o775)
        sentinel = existing / "user-data"
        sentinel.write_text("preserve me")
        with self.assertRaises(bootstrap.Refusal):
            self.configure()
        self.assertEqual(existing.stat().st_mode & 0o777, 0o775)
        self.assertEqual(sentinel.read_text(), "preserve me")
        self.assertFalse((self.home / ".config").exists())

    def test_binary_with_shared_writable_ancestor_is_refused_without_chmod(self):
        self.binary.parent.chmod(0o775)
        with self.assertRaises(bootstrap.Refusal):
            self.configure()
        self.assertEqual(self.binary.parent.stat().st_mode & 0o777, 0o775)
        self.assertEqual(hashlib.sha256(self.binary.read_bytes()).hexdigest(), self.env["CREW_BINARY_SHA256"])
        self.assertFalse((self.home / ".config").exists())

    def test_existing_unmanaged_profile_or_occupied_port_is_preserved(self):
        profile = self.home / ".crew-staging"
        profile.mkdir()
        sentinel = profile / "existing-data"
        sentinel.write_text("keep me")
        with self.assertRaises(bootstrap.Refusal):
            self.configure()
        self.assertEqual(sentinel.read_text(), "keep me")
        sentinel.unlink()
        profile.rmdir()
        with patch.object(bootstrap, "port_busy", return_value=True):
            with self.assertRaises(bootstrap.Refusal):
                self.configure()
        self.assertFalse((self.home / ".config").exists())

    def test_image_digest_requires_exact_length_across_buffer_boundary(self):
        path = self.home / "checksum-image"
        digest = hashlib.sha256()
        chunk = b"x" * 65536
        with path.open("wb") as image:
            for _ in range(17):
                image.write(chunk)
                digest.update(chunk)
        size = path.stat().st_size
        for expected_size in (size - 1, size, size + 1):
            with self.subTest(expected_size=expected_size), path.open("rb") as image:
                self.assertEqual(bootstrap.image_digest(image, expected_size),
                                 digest.hexdigest() if expected_size == size else None)
        path.write_bytes(b"")
        with path.open("rb") as image:
            self.assertEqual(bootstrap.image_digest(image, 0), hashlib.sha256(b"").hexdigest())

    @unittest.skipUnless(sys.platform == "linux", "Requires real Linux process images and sockets")
    def test_listener_requires_pinned_process_image_and_owned_socket_not_matching_path(self):
        self.binary.chmod(0o700)
        shutil.copyfile("/bin/sleep", self.binary)
        with self.binary.open("r+b") as image:
            self.assertEqual(image.read(4), b"\x7fELF")
            image.seek(0, os.SEEK_END)
            image.write(b"A")  # ELF ignores appended data; pin it to exercise same-size digest rejection.
        self.binary.chmod(0o500)
        digest = hashlib.sha256()
        with self.binary.open("rb") as image:
            for chunk in iter(lambda: image.read(1024 * 1024), b""):
                digest.update(chunk)
        with patch.dict(os.environ, {"CREW_BINARY_SHA256": digest.hexdigest()}):
            self.configure()
            config = bootstrap.load_config()
        backing = self.home / "provider-backing-image"
        shutil.copyfile(self.binary, backing)
        backing.chmod(0o500)
        different = self.home / "different-image"
        shutil.copyfile(self.binary, different)
        with different.open("r+b") as image:
            image.seek(-1, os.SEEK_END)
            image.write(b"B")
        different.chmod(0o500)
        self.assertEqual(different.stat().st_size, self.binary.stat().st_size)
        self.assertFalse(backing.samefile(self.binary))
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            with patch.dict(config, {"COMET_IPC_PORT": str(listener.getsockname()[1])}):
                for image, inherit_socket, expected in (
                    (self.binary, True, True), (backing, True, True),
                    (different, True, False), (backing, False, False),
                ):
                    with self.subTest(image=image.name, inherit_socket=inherit_socket):
                        child = subprocess.Popen(
                            ["sleep", "60"], executable=str(image),
                            pass_fds=(listener.fileno(),) if inherit_socket else (),
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                        )
                        try:
                            self.assertIsNone(child.poll())
                            self.assertTrue(Path(f"/proc/{child.pid}/exe").samefile(image))
                            self.assertEqual(bootstrap.owns_listener(child.pid, config), expected)
                        finally:
                            child.kill()
                            child.wait(timeout=5)
                self.assertFalse(bootstrap.owns_listener(child.pid, config))
                self.assertFalse(bootstrap.owns_listener(0, config))

    def test_signed_out_status_does_not_install_start_or_import(self):
        self.configure()
        with patch.object(bootstrap, "crew", return_value="Mode:     local only\nAuth:     signed out\n"), \
             patch.object(bootstrap, "systemctl") as systemctl:
            with self.assertRaises(bootstrap.Refusal):
                bootstrap.start()
        systemctl.assert_not_called()


    def test_resume_checks_native_identity_before_any_command(self):
        self.configure()
        with patch.dict(os.environ, {"BOAT_ID": "bx_23456789"}), \
             patch.object(bootstrap, "crew") as cli, patch.object(bootstrap, "systemctl") as systemctl:
            with self.assertRaises(bootstrap.Refusal):
                bootstrap.start()
        cli.assert_not_called()
        systemctl.assert_not_called()

    def test_foreign_unit_or_override_is_not_adopted(self):
        self.configure()
        unit = self.home / ".config/systemd/user/crew-devbox-staging.service"
        unit.parent.mkdir(parents=True, exist_ok=True)
        unit.write_text("[Service]\nExecStart=/unrelated/service\n")
        with patch.object(bootstrap, "service_state", return_value=(unit, False, 0)), \
             patch.object(bootstrap, "crew") as cli:
            with self.assertRaises(bootstrap.Refusal):
                bootstrap.start()
        cli.assert_not_called()


if __name__ == "__main__":
    unittest.main()
