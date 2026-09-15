//! Managed installs for ACP agents distributed as prebuilt zip archives
//! (the ACP registry's `binary` distribution), such as Google's Antigravity
//! server.
//!
//! Same contract as the npm installs in [`crate::adapter_install`]: the pinned
//! archive lands ONCE in `~/.zeron/adapters/<name>/<version>`, extraction runs
//! in a `.tmp-*` sibling that is renamed into place only after the entry
//! resolves and the marker is written, so a killed download never passes for
//! a working install.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::HarnessError;
use crate::adapter_install::{OK_MARKER, adapters_root, install_lock};

/// A pinned archive: where to fetch it and which file inside runs the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArchivePin {
    pub name: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    /// Path of the executable relative to the archive root.
    pub entry: &'static str,
}

/// multi-hundred-megabyte archives outlast any whole-request deadline on a
/// slow link, so only a stalled stream counts as a failure.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(600);

fn install_dir(pin: &ArchivePin) -> Option<PathBuf> {
    adapters_root().map(|root| root.join(pin.name).join(pin.version))
}

/// The entry of a COMPLETED install, `None` when absent.
pub(crate) fn installed_entry(pin: &ArchivePin) -> Option<PathBuf> {
    let dir = install_dir(pin)?;
    if !dir.join(OK_MARKER).exists() {
        return None;
    }
    let entry = dir.join(pin.entry);
    entry.exists().then_some(entry)
}

/// Where the entry lives once installed, whether or not it is yet.
pub(crate) fn entry_path(pin: &ArchivePin) -> Option<PathBuf> {
    install_dir(pin).map(|dir| dir.join(pin.entry))
}

pub(crate) async fn ensure_installed(
    pin: ArchivePin,
    display_name: &str,
) -> Result<PathBuf, HarnessError> {
    if let Some(entry) = installed_entry(&pin) {
        return Ok(entry);
    }
    let _guard = install_lock().lock().await;
    if let Some(entry) = installed_entry(&pin) {
        return Ok(entry);
    }

    let root = adapters_root().ok_or_else(|| {
        HarnessError::Install("cannot locate an adapters directory (HOME is unset)".into())
    })?;
    let final_dir = install_dir(&pin).expect("root resolved");
    let tmp_dir = root.join(format!(
        ".tmp-{}-{}-{}",
        pin.name,
        pin.version,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)?;
    tracing::info!(
        target: "zeron_harness::adapter_install",
        url = pin.url,
        dir = %tmp_dir.display(),
        "installing {display_name} ACP server"
    );
    if let Err(e) = download_and_extract(&pin, &tmp_dir, display_name).await {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }
    std::fs::write(tmp_dir.join(OK_MARKER), pin.version)?;
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(&tmp_dir, &final_dir).is_err() {
        if installed_entry(&pin).is_none() {
            let _ = std::fs::remove_dir_all(&final_dir);
            std::fs::rename(&tmp_dir, &final_dir)?;
        } else {
            let _ = std::fs::remove_dir_all(&tmp_dir);
        }
    }
    installed_entry(&pin).ok_or_else(|| {
        HarnessError::Install(format!(
            "install of the {display_name} ACP server {} finished but {} did not resolve",
            pin.version, pin.entry
        ))
    })
}

async fn download_and_extract(
    pin: &ArchivePin,
    dir: &Path,
    display_name: &str,
) -> Result<(), HarnessError> {
    let archive = dir.join("download.zip");
    download(pin.url, &archive, display_name).await?;
    extract_zip(&archive, dir).await?;
    let _ = std::fs::remove_file(&archive);
    let entry = dir.join(pin.entry);
    if !entry.exists() {
        return Err(HarnessError::Install(format!(
            "the {display_name} archive ({}) has no {}",
            pin.url, pin.entry
        )));
    }
    mark_executables(dir)?;
    Ok(())
}

async fn download(url: &str, dest: &Path, display_name: &str) -> Result<(), HarnessError> {
    let failed = |detail: String| {
        HarnessError::Install(format!(
            "download of the {display_name} ACP server ({url}) failed: {detail}"
        ))
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| failed(e.to_string()))?;
    let mut body = response.bytes_stream();
    let mut file = tokio::fs::File::create(dest).await?;
    loop {
        let chunk = tokio::time::timeout(STALL_TIMEOUT, body.next())
            .await
            .map_err(|_| {
                failed(format!(
                    "no data for {}s — check your network",
                    STALL_TIMEOUT.as_secs()
                ))
            })?;
        match chunk {
            Some(Ok(bytes)) => file.write_all(&bytes).await?,
            Some(Err(e)) => return Err(failed(e.to_string())),
            None => break,
        }
    }
    file.flush().await?;
    Ok(())
}

/// the system unpacker keeps the crate free of a zip dependency: `unzip`
/// ships with macOS and mainstream linux distros, and windows 10+ bundles a
/// bsdtar that reads zip archives.
pub(crate) async fn extract_zip(archive: &Path, dest: &Path) -> Result<(), HarnessError> {
    let mut cmd = if cfg!(windows) {
        let mut cmd = tokio::process::Command::new("tar");
        cmd.arg("-xf").arg(archive).arg("-C").arg(dest);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("unzip");
        cmd.args(["-q", "-o"]).arg(archive).arg("-d").arg(dest);
        cmd
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let unpacker = if cfg!(windows) { "tar" } else { "unzip" };
    let output = match tokio::time::timeout(EXTRACT_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(HarnessError::NotInstalled(format!(
                "{unpacker} (required to unpack a managed ACP server archive)"
            )));
        }
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => {
            return Err(HarnessError::Install(format!(
                "{unpacker} did not finish within {} minutes",
                EXTRACT_TIMEOUT.as_secs() / 60
            )));
        }
    };
    if output.status.success() {
        return Ok(());
    }
    Err(HarnessError::Install(format!(
        "{unpacker} failed ({}): {}",
        crate::describe_exit(Some(output.status)),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// archives built off-unix can lose the executable bit, and the entry spawns
/// sibling helper binaries of its own.
#[cfg(unix)]
fn mark_executables(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let mut permissions = entry.metadata()?.permissions();
            permissions.set_mode(permissions.mode() | 0o755);
            std::fs::set_permissions(entry.path(), permissions)?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn mark_executables(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn extracts_zip_and_marks_files_executable() {
        use std::os::unix::fs::PermissionsExt;
        if crate::acp::find_on_paths("zip", Vec::new()).is_none() {
            return;
        }
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("server.par"), "#!/bin/sh\n").unwrap();
        std::fs::write(src.path().join("helper"), "bin").unwrap();
        let archive = src.path().join("bundle.zip");
        let zipped = std::process::Command::new("zip")
            .args(["-q", "bundle.zip", "server.par", "helper"])
            .current_dir(src.path())
            .status()
            .unwrap();
        assert!(zipped.success());

        let dest = tempfile::tempdir().unwrap();
        extract_zip(&archive, dest.path()).await.unwrap();
        mark_executables(dest.path()).unwrap();
        for name in ["server.par", "helper"] {
            let mode = std::fs::metadata(dest.path().join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "{name} is executable");
        }
    }

    #[tokio::test]
    async fn corrupt_archive_reports_the_unpacker_failure() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("broken.zip");
        std::fs::write(&archive, "not a zip").unwrap();
        let error = extract_zip(&archive, dir.path()).await.unwrap_err();
        assert!(
            matches!(error, HarnessError::Install(ref m) if m.contains("failed")),
            "{error}"
        );
    }

    #[test]
    fn entry_resolves_only_after_a_completed_install() {
        let pin = ArchivePin {
            name: "never-installed-acp",
            version: "0.0.0-test",
            url: "https://example.invalid/x.zip",
            entry: "server",
        };
        assert!(installed_entry(&pin).is_none());
        if std::env::var_os("HOME").is_some() || std::env::var_os("ZERON_ADAPTERS_DIR").is_some() {
            let expected = entry_path(&pin).unwrap();
            assert!(expected.ends_with("never-installed-acp/0.0.0-test/server"));
        }
    }
}
