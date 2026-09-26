//! Linux desktop and daemon updates share immutable version directories.
//! Only `current` is replaced; the running executable and previous version
//! remain intact. Desktop updates restart an active user service as well.
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, ensure};
use sha2::{Digest, Sha256};

const BINARY_DIGEST: &str = ".zeron-update-sha256";
const ARCHIVE_DIGEST: &str = ".zeron-archive-sha256";

fn validate_version(version: &str) -> anyhow::Result<()> {
    ensure!(
        !version.is_empty()
            && version
                .split('.')
                .all(|part| { !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()) }),
        "invalid Linux release version"
    );
    Ok(())
}

fn digest(path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify(directory: &Path) -> anyhow::Result<()> {
    let binary = directory.join("zeron");
    ensure!(
        std::fs::symlink_metadata(&binary)?.file_type().is_file(),
        "update executable must be a regular file"
    );
    let expected = std::fs::read_to_string(directory.join(BINARY_DIGEST))?;
    ensure!(
        digest(&binary)? == expected.trim(),
        "staged update checksum mismatch"
    );
    Ok(())
}

async fn check_executable(directory: &Path, version: &str) -> anyhow::Result<()> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new(directory.join("zeron"))
            .arg("--version")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("updated executable version check timed out")??;
    ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == format!("zeron {version}"),
        "updated executable has the wrong version or cannot run"
    );
    Ok(())
}

/// Shared with the desktop installer. Fail promptly rather than blocking the
/// UI or a runtime worker behind a competing CLI/daemon update.
struct UpdateLock(File);

impl Drop for UpdateLock {
    fn drop(&mut self) {
        // Explicitly unlock: another thread may have forked a subprocess
        // between open and close, temporarily inheriting the file description.
        let _ = self.0.unlock();
    }
}

fn lock(app_root: &Path) -> anyhow::Result<UpdateLock> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(app_root.join(".update.lock"))?;
    file.try_lock()
        .context("another Zeron update is in progress")?;
    Ok(UpdateLock(file))
}

pub async fn stage(
    edge_url: &str,
    manifest: &super::Manifest,
    app_root: &Path,
) -> anyhow::Result<PathBuf> {
    let version = &manifest.version;
    validate_version(version)?;
    let artifact = super::headless_artifact(version);
    let expected = manifest
        .files
        .get(&artifact)
        .and_then(|meta| meta.sha256.as_deref())
        .context("Linux updates require a SHA-256 checksum")?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid SHA-256 checksum"
    );
    let expected = expected.to_ascii_lowercase();
    std::fs::create_dir_all(app_root)?;
    let destination = app_root.join(version);
    if destination.exists() && verify_archive(&destination, &expected).is_ok() {
        check_executable(&destination, version).await?;
        return Ok(destination);
    }
    // Each attempt owns its staging directory, including simultaneous UI and
    // daemon downloads in the same process. Cancellation removes partial data.
    let temporary = tempfile::Builder::new()
        .prefix(".stage-")
        .tempdir_in(app_root)?;
    let archive = temporary.path().join(&artifact);
    super::download_release_file(edge_url, manifest, &artifact, &archive).await?;
    let unpacked = temporary.path().join("unpacked");
    std::fs::create_dir(&unpacked)?;
    let output = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpacked)
        .args([
            "--strip-components=1",
            "--no-same-owner",
            "--no-same-permissions",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("extracting Linux update")?;
    ensure!(
        output.status.success(),
        "extracting Linux update: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        std::fs::symlink_metadata(unpacked.join("zeron"))?
            .file_type()
            .is_file(),
        "update executable must be a regular file"
    );
    check_executable(&unpacked, version).await?;
    std::fs::write(
        unpacked.join(BINARY_DIGEST),
        digest(&unpacked.join("zeron"))?,
    )?;
    std::fs::write(unpacked.join(ARCHIVE_DIGEST), &expected)?;
    let _lock = lock(app_root)?;
    if destination.exists() {
        // A manual installation of this version has no archive receipt. Only
        // reuse it if its binary matches the freshly verified release exactly.
        ensure!(
            std::fs::symlink_metadata(destination.join("zeron"))?
                .file_type()
                .is_file()
                && digest(&destination.join("zeron"))? == digest(&unpacked.join("zeron"))?,
            "existing version does not match the downloaded update"
        );
        std::fs::copy(
            unpacked.join(BINARY_DIGEST),
            destination.join(BINARY_DIGEST),
        )?;
        std::fs::copy(
            unpacked.join(ARCHIVE_DIGEST),
            destination.join(ARCHIVE_DIGEST),
        )?;
    } else {
        std::fs::rename(&unpacked, &destination).context("publishing Linux update")?;
    }
    Ok(destination)
}

fn verify_archive(directory: &Path, expected: &str) -> anyhow::Result<()> {
    ensure!(
        std::fs::read_to_string(directory.join(ARCHIVE_DIGEST))?.trim() == expected,
        "existing version does not match the release checksum"
    );
    verify(directory)
}

fn point_current(app_root: &Path, target: &Path) -> anyhow::Result<()> {
    let temporary = tempfile::Builder::new()
        .prefix(".current-")
        .tempdir_in(app_root)?;
    let link = temporary.path().join("current");
    std::os::unix::fs::symlink(target, &link)?;
    std::fs::rename(link, app_root.join("current")).context("activating Linux update")
}

/// Stage paths must be direct versioned children of this installation. Keep
/// the previous directory for recovery and reject a late click that would
/// overwrite a newer version already installed by the daemon/CLI.
pub fn apply(staged: &Path, app_root: &Path, relaunch: bool) -> anyhow::Result<()> {
    activate(staged, app_root, || {
        if relaunch {
            restart_active_service()?;
            spawn_relauncher(&app_root.join("current/zeron"), std::process::id())?;
        }
        Ok(())
    })
}

fn activate(
    staged: &Path,
    app_root: &Path,
    after_swap: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let version = staged
        .file_name()
        .and_then(|name| name.to_str())
        .context("missing update version")?;
    validate_version(version)?;
    ensure!(
        staged.parent() == Some(app_root),
        "update is outside this installation"
    );
    let _lock = lock(app_root)?;
    verify(staged)?;
    let current = app_root.join("current");
    let previous = std::fs::read_link(&current).context("reading installed version")?;
    if let Some(installed) = previous.file_name().and_then(|name| name.to_str()) {
        ensure!(
            !super::version_newer(installed, version),
            "a newer version is already installed"
        );
    }
    point_current(app_root, staged)?;
    if let Err(error) = after_swap() {
        point_current(app_root, &previous).context("restoring previous Linux version")?;
        return Err(error);
    }
    Ok(())
}

fn restart_active_service() -> anyhow::Result<()> {
    restart_active_service_with(Path::new("systemctl"))
}

fn restart_active_service_with(systemctl: &Path) -> anyhow::Result<()> {
    // Desktop-only installs work without systemd. Never start an inactive
    // service: it could claim the IPC port of the app's embedded engine.
    let active = Command::new(systemctl)
        .args(["--user", "is-active", "--quiet", "zeron.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if active.is_ok_and(|status| status.success()) {
        let output = Command::new(systemctl)
            .args(["--user", "restart", "zeron.service"])
            .stdin(Stdio::null())
            .output()
            .context("restarting the Zeron background service")?;
        ensure!(
            output.status.success(),
            "restarting the Zeron background service: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn spawn_relauncher(executable: &Path, pid: u32) -> anyhow::Result<std::process::Child> {
    // Pass paths as arguments, never interpolate shell source. Wait for the
    // old process to release its engine lock and IPC listener before booting.
    Command::new("/bin/sh")
        .args([
            "-c",
            "while kill -0 \"$1\" 2>/dev/null; do sleep 0.1; done; exec \"$2\"",
            "zeron-relaunch",
        ])
        .arg(pid.to_string())
        .arg(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("starting Linux update relauncher")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn executable(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn version(root: &Path, name: &str) -> PathBuf {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        executable(&directory.join("zeron"), &format!("echo 'zeron {name}'"));
        std::fs::write(
            directory.join(BINARY_DIGEST),
            digest(&directory.join("zeron")).unwrap(),
        )
        .unwrap();
        directory
    }

    fn archive(version_name: &str) -> Vec<u8> {
        let temp = tempfile::tempdir().unwrap();
        version(temp.path(), version_name);
        let archive = temp.path().join("release.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(temp.path())
            .arg(version_name)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::read(archive).unwrap()
    }

    async fn feed(bytes: Vec<u8>, version: &str) -> (String, super::super::Manifest) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        tokio::spawn(async move {
            // One request per fixture: cached staging must not fetch again.
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(&bytes).await.unwrap();
        });
        let manifest = super::super::Manifest {
            version: version.to_owned(),
            files: [(
                super::super::headless_artifact(version),
                super::super::FileMeta { sha256: Some(hash) },
            )]
            .into(),
        };
        (format!("http://{address}"), manifest)
    }

    #[tokio::test]
    async fn download_verify_activate_and_reuse_without_network() {
        let root = tempfile::tempdir().unwrap();
        let old = version(root.path(), "1.0.0");
        point_current(root.path(), &old).unwrap();
        let (url, manifest) = feed(archive("1.1.0"), "1.1.0").await;
        let staged = stage(&url, &manifest, root.path()).await.unwrap();
        assert_eq!(
            std::fs::read_link(root.path().join("current")).unwrap(),
            old
        );
        let reused = stage("http://127.0.0.1:1", &manifest, root.path())
            .await
            .unwrap();
        assert_eq!(staged, reused);
        apply(&staged, root.path(), false).unwrap();
        assert_eq!(
            std::fs::read_link(root.path().join("current")).unwrap(),
            staged
        );
        assert!(old.join("zeron").is_file());
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
    }

    #[tokio::test]
    async fn bad_checksum_and_wrong_executable_preserve_current() {
        let root = tempfile::tempdir().unwrap();
        let old = version(root.path(), "1.0.0");
        point_current(root.path(), &old).unwrap();
        let (url, mut manifest) = feed(archive("1.1.0"), "1.1.0").await;
        manifest.files.values_mut().next().unwrap().sha256 = Some("0".repeat(64));
        assert!(
            stage(&url, &manifest, root.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
        let (url, manifest) = feed(archive("1.2.0"), "1.1.0").await;
        assert!(
            stage(&url, &manifest, root.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("wrong version")
        );
        assert!(!root.path().join("1.1.0").exists());
        assert_eq!(
            std::fs::read_link(root.path().join("current")).unwrap(),
            old
        );
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".stage-")
        }));
    }

    #[tokio::test]
    async fn invalid_metadata_is_rejected_before_side_effects() {
        let root = tempfile::tempdir().unwrap();
        for release in ["../1.0", "/tmp/1.0", "", "1..2", "1.2.0"] {
            let manifest = super::super::Manifest {
                version: release.into(),
                ..Default::default()
            };
            assert!(
                stage("http://127.0.0.1:1", &manifest, &root.path().join("app"))
                    .await
                    .is_err()
            );
        }
        assert!(!root.path().join("app").exists());
    }

    #[tokio::test]
    async fn interrupted_download_removes_partial_staging() {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10000\r\n\r\npartial")
                .await
                .unwrap();
        });
        let manifest = super::super::Manifest {
            version: "1.1.0".into(),
            files: [(
                super::super::headless_artifact("1.1.0"),
                super::super::FileMeta {
                    sha256: Some("0".repeat(64)),
                },
            )]
            .into(),
        };
        assert!(
            stage(&format!("http://{address}"), &manifest, root.path())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn manually_installed_version_is_verified_before_reuse() {
        let root = tempfile::tempdir().unwrap();
        let installed = version(root.path(), "1.1.0");
        let (url, manifest) = feed(archive("1.1.0"), "1.1.0").await;
        assert_eq!(
            stage(&url, &manifest, root.path()).await.unwrap(),
            installed
        );
        assert!(installed.join(ARCHIVE_DIGEST).is_file());
        executable(&installed.join("zeron"), "echo 'zeron 1.1.0'\n# changed");
        let (url, manifest) = feed(archive("1.1.0"), "1.1.0").await;
        assert!(
            stage(&url, &manifest, root.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("does not match")
        );
    }

    #[test]
    fn desktop_restarts_only_an_active_service_and_surfaces_failure() {
        let root = tempfile::tempdir().unwrap();
        let systemctl = root.path().join("systemctl");
        let marker = root.path().join("restarted");
        // Missing systemd and inactive services are normal desktop installs.
        restart_active_service_with(&systemctl).unwrap();
        executable(&systemctl, "exit 3");
        restart_active_service_with(&systemctl).unwrap();
        assert!(!marker.exists());
        executable(
            &systemctl,
            &format!("[ \"$2\" != restart ] || touch '{}'", marker.display()),
        );
        restart_active_service_with(&systemctl).unwrap();
        assert!(marker.is_file());
        executable(
            &systemctl,
            "[ \"$2\" != restart ] || { echo 'restart failed' >&2; exit 1; }",
        );
        assert!(
            restart_active_service_with(&systemctl)
                .unwrap_err()
                .to_string()
                .contains("restart failed")
        );
    }

    #[test]
    fn corrupted_stage_lock_contention_and_downgrade_leave_current_intact() {
        let root = tempfile::tempdir().unwrap();
        let old = version(root.path(), "1.0.0");
        let staged = version(root.path(), "1.1.0");
        point_current(root.path(), &old).unwrap();
        let held = lock(root.path()).unwrap();
        assert!(
            apply(&staged, root.path(), false)
                .unwrap_err()
                .to_string()
                .contains("another Zeron update")
        );
        drop(held);
        std::fs::write(staged.join("zeron"), "corrupt").unwrap();
        assert!(
            apply(&staged, root.path(), false)
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
        assert_eq!(
            std::fs::read_link(root.path().join("current")).unwrap(),
            old
        );
        let newer = version(root.path(), "2.0.0");
        point_current(root.path(), &newer).unwrap();
        assert!(
            apply(&old, root.path(), false)
                .unwrap_err()
                .to_string()
                .contains("newer version")
        );
        assert_eq!(
            std::fs::read_link(root.path().join("current")).unwrap(),
            newer
        );
    }

    #[test]
    fn failed_restart_rolls_back_and_paths_cannot_escape_installation() {
        let root = tempfile::tempdir().unwrap();
        let old = version(root.path(), "1.0.0");
        let staged = version(root.path(), "1.1.0");
        point_current(root.path(), &old).unwrap();
        let error = activate(&staged, root.path(), || anyhow::bail!("restart failed")).unwrap_err();
        assert!(error.to_string().contains("restart failed"));
        assert_eq!(
            std::fs::read_link(root.path().join("current")).unwrap(),
            old
        );
        let outside = tempfile::tempdir().unwrap();
        assert!(apply(&version(outside.path(), "1.2.0"), root.path(), false).is_err());
    }

    #[test]
    fn relaunch_waits_for_exit_and_preserves_paths_with_shell_characters() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("space ' dollar $ and `quote`");
        std::fs::create_dir(&directory).unwrap();
        let binary = directory.join("zeron");
        executable(&binary, "touch \"$(dirname \"$0\")/launched\"");
        let mut previous = Command::new("sleep").arg("30").spawn().unwrap();
        let mut next = spawn_relauncher(&binary, previous.id()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!directory.join("launched").exists());
        previous.kill().unwrap();
        previous.wait().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = next.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() > deadline {
                let _ = next.kill();
                panic!("relauncher did not finish");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(directory.join("launched").is_file());
    }
}
