//! managed installs for acp agents distributed as prebuilt archives (the ACP
//! registry's `binary` distribution), such as Google's Antigravity server and
//! any registry agent a user adds. Zip, `.tar.gz`, `.tar.bz2` and bare
//! executables are understood.
//!
//! same contract as the npm installs in [`crate::adapter_install`]: the pinned
//! archive lands ONCE in `~/.zeron/adapters/<name>/<version>`, extraction runs
//! in a `.tmp-*` sibling that is renamed into place only after the entry
//! resolves and the marker is written, so a killed download never passes for
//! a working install.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use sha2::{Digest as _, Sha256, Sha512};
use tokio::io::AsyncWriteExt;

use crate::HarnessError;
use crate::adapter_install::{OK_MARKER, adapters_root, install_lock};

/// a pinned archive: where to fetch it and which file inside runs the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivePin {
    pub name: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    /// path of the executable relative to the archive root.
    pub entry: &'static str,
    pub sha512: &'static str,
}

/// multi-hundred-megabyte archives outlast any whole-request deadline on a
/// slow link, so only a stalled stream counts as a failure.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ARCHIVE_BYTES: u64 = 768 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 4_096;

/// an archive release whose integrity is proven after extraction (by a
/// vendor code signature) rather than by a digest pinned in zeron's source.
pub struct VerifiedRelease<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub url: &'a str,
    pub entry: &'a str,
    /// written as the install marker once verification passes.
    pub marker: &'a str,
}

/// a release from the ACP registry's `binary` distribution, checked against
/// the SHA-256 the registry publishes for it when it lists one.
pub struct RegistryRelease<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub url: &'a str,
    pub entry: &'a str,
    pub sha256: Option<&'a str>,
}

impl RegistryRelease<'_> {
    /// the digest when the registry lists one, else the url: a release the
    /// registry later republishes under the same version reinstalls.
    fn marker(&self) -> &str {
        self.sha256.unwrap_or(self.url)
    }
}

#[derive(Clone, Copy)]
enum Checksum<'a> {
    Sha512(&'a str),
    Sha256(&'a str),
}

struct Source<'a> {
    name: &'a str,
    version: &'a str,
    url: &'a str,
    entry: &'a str,
    checksum: Option<Checksum<'a>>,
    marker: &'a str,
}

impl<'a> From<&'a ArchivePin> for Source<'a> {
    fn from(pin: &'a ArchivePin) -> Self {
        Self {
            name: pin.name,
            version: pin.version,
            url: pin.url,
            entry: pin.entry,
            checksum: Some(Checksum::Sha512(pin.sha512)),
            marker: pin.sha512,
        }
    }
}

impl<'a> From<&'a VerifiedRelease<'a>> for Source<'a> {
    fn from(release: &'a VerifiedRelease<'a>) -> Self {
        Self {
            name: release.name,
            version: release.version,
            url: release.url,
            entry: release.entry,
            checksum: None,
            marker: release.marker,
        }
    }
}

impl<'a> From<&'a RegistryRelease<'a>> for Source<'a> {
    fn from(release: &'a RegistryRelease<'a>) -> Self {
        Self {
            name: release.name,
            version: release.version,
            url: release.url,
            entry: release.entry,
            checksum: release.sha256.map(Checksum::Sha256),
            marker: release.marker(),
        }
    }
}

fn install_dir(name: &str, version: &str) -> Option<PathBuf> {
    adapters_root().map(|root| root.join(name).join(version))
}

/// the entry of a completed install whose marker matches, `None` when absent.
pub fn installed_entry_with_marker(
    name: &str,
    version: &str,
    entry: &str,
    marker: &str,
) -> Option<PathBuf> {
    let dir = install_dir(name, version)?;
    if std::fs::read_to_string(dir.join(OK_MARKER)).ok()?.trim() != marker {
        return None;
    }
    let entry = dir.join(entry);
    entry.is_file().then_some(entry)
}

/// the entry of a completed install, `None` when absent.
pub fn installed_entry(pin: &ArchivePin) -> Option<PathBuf> {
    installed_entry_with_marker(pin.name, pin.version, pin.entry, pin.sha512)
}

/// the marker of every completed-looking version directory under `name`.
pub(crate) fn installed_versions(name: &str) -> Vec<(String, String)> {
    let Some(root) = adapters_root().map(|root| root.join(name)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let version = entry.file_name().into_string().ok()?;
            let marker = std::fs::read_to_string(entry.path().join(OK_MARKER)).ok()?;
            Some((version, marker.trim().to_owned()))
        })
        .collect()
}

/// the entry of a completed registry install, `None` when absent.
pub fn installed_registry_entry(release: &RegistryRelease<'_>) -> Option<PathBuf> {
    installed_entry_with_marker(
        release.name,
        release.version,
        release.entry,
        release.marker(),
    )
}

/// install a registry release (any archive kind it ships).
pub async fn install_registry(
    release: &RegistryRelease<'_>,
    display_name: &str,
) -> Result<PathBuf, HarnessError> {
    install(Source::from(release), display_name, |_| async { Ok(()) }).await
}

/// delete every installed version of `name`.
pub fn remove_installs(name: &str) {
    if let Some(dir) = adapters_root().map(|root| root.join(name)) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub async fn ensure_installed(
    pin: ArchivePin,
    display_name: &str,
) -> Result<PathBuf, HarnessError> {
    install(Source::from(&pin), display_name, |_| async { Ok(()) }).await
}

/// install a release that has no pinned digest. `verify` inspects the
/// extracted tree before it is committed; any error discards the download.
pub async fn install_verified<F, Fut>(
    release: &VerifiedRelease<'_>,
    display_name: &str,
    verify: F,
) -> Result<PathBuf, HarnessError>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<(), HarnessError>>,
{
    install(Source::from(release), display_name, verify).await
}

async fn install<F, Fut>(
    source: Source<'_>,
    display_name: &str,
    verify: F,
) -> Result<PathBuf, HarnessError>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<(), HarnessError>>,
{
    let installed =
        || installed_entry_with_marker(source.name, source.version, source.entry, source.marker);
    if let Some(entry) = installed() {
        return Ok(entry);
    }
    let _guard = install_lock().lock().await;
    if let Some(entry) = installed() {
        return Ok(entry);
    }

    let root = adapters_root().ok_or_else(|| {
        HarnessError::Install("cannot locate an adapters directory (HOME is unset)".into())
    })?;
    let final_dir = install_dir(source.name, source.version).expect("root resolved");
    let tmp_dir = root.join(format!(
        ".tmp-{}-{}-{}",
        source.name,
        source.version,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir)?;
    tracing::info!(
        target: "zeron_harness::adapter_install",
        url = source.url,
        dir = %tmp_dir.display(),
        "installing {display_name} ACP server"
    );
    let prepared = match download_and_extract(&source, &tmp_dir, display_name).await {
        Ok(()) => verify(tmp_dir.clone()).await,
        Err(error) => Err(error),
    };
    if let Err(error) = prepared {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(error);
    }
    std::fs::write(tmp_dir.join(OK_MARKER), format!("{}\n", source.marker))?;
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(&tmp_dir, &final_dir).is_err() {
        if installed().is_none() {
            let _ = std::fs::remove_dir_all(&final_dir);
            std::fs::rename(&tmp_dir, &final_dir)?;
        } else {
            let _ = std::fs::remove_dir_all(&tmp_dir);
        }
    }
    installed().ok_or_else(|| {
        HarnessError::Install(format!(
            "install of the {display_name} ACP server {} finished but {} did not resolve",
            source.version, source.entry
        ))
    })
}

async fn download_and_extract(
    source: &Source<'_>,
    dir: &Path,
    display_name: &str,
) -> Result<(), HarnessError> {
    let kind = ArchiveKind::of(source.url);
    let entry = dir.join(source.entry);
    if kind == ArchiveKind::Executable {
        if let Some(parent) = entry.parent() {
            std::fs::create_dir_all(parent)?;
        }
        download(source, &entry, display_name).await?;
    } else {
        let archive = dir.join(kind.download_name());
        download(source, &archive, display_name).await?;
        match kind {
            ArchiveKind::Zip => extract_zip(&archive, dir).await?,
            _ => extract_tar(&archive, dir, kind).await?,
        }
        let _ = std::fs::remove_file(&archive);
    }
    if !entry.is_file() {
        return Err(HarnessError::Install(format!(
            "the {display_name} archive ({}) has no {}",
            source.url, source.entry
        )));
    }
    mark_executables(dir)?;
    mark_executable(&entry)?;
    Ok(())
}

/// what a release url downloads, judged by its file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    TarGz,
    TarBz2,
    Tar,
    /// a bare executable, saved as the entry itself.
    Executable,
}

impl ArchiveKind {
    fn of(url: &str) -> Self {
        let name = url
            .split(['?', '#'])
            .next()
            .unwrap_or(url)
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name.ends_with(".zip") {
            Self::Zip
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Self::TarGz
        } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") {
            Self::TarBz2
        } else if name.ends_with(".tar") {
            Self::Tar
        } else {
            Self::Executable
        }
    }

    fn download_name(self) -> &'static str {
        match self {
            Self::Zip => "download.zip",
            Self::TarGz => "download.tar.gz",
            Self::TarBz2 => "download.tar.bz2",
            Self::Tar | Self::Executable => "download.tar",
        }
    }
}

async fn extract_tar(archive: &Path, dest: &Path, kind: ArchiveKind) -> Result<(), HarnessError> {
    let archive = archive.to_owned();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || {
        let file = std::io::BufReader::new(std::fs::File::open(&archive)?);
        let reader: Box<dyn std::io::Read> = match kind {
            ArchiveKind::TarGz => Box::new(flate2::read::GzDecoder::new(file)),
            ArchiveKind::TarBz2 => Box::new(bzip2::read::BzDecoder::new(file)),
            _ => Box::new(file),
        };
        extract_tar_blocking(reader, &dest)
    })
    .await
    .map_err(|error| HarnessError::Install(format!("archive extraction task failed: {error}")))?
}

fn extract_tar_blocking(reader: impl std::io::Read, dest: &Path) -> Result<(), HarnessError> {
    let invalid =
        |error: std::io::Error| HarnessError::Install(format!("invalid tar archive: {error}"));
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(true);
    let mut extracted = 0_u64;
    for (index, entry) in archive.entries().map_err(invalid)?.enumerate() {
        if index >= MAX_ARCHIVE_ENTRIES {
            return Err(HarnessError::Install(format!(
                "archive exceeds the {MAX_ARCHIVE_ENTRIES} entry limit"
            )));
        }
        let mut entry = entry.map_err(invalid)?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            return Err(HarnessError::Install(format!(
                "archive entry {} is a hard link or special file",
                entry.path().map_err(invalid)?.display()
            )));
        }
        extracted = extracted.saturating_add(entry.size());
        if extracted > MAX_EXTRACTED_BYTES {
            return Err(HarnessError::Install(format!(
                "archive expands beyond the {} MiB limit",
                MAX_EXTRACTED_BYTES / (1024 * 1024)
            )));
        }
        // `unpack_in` refuses paths that would land outside `dest` (absolute
        // names, `..`, writes through an earlier symlink).
        if !entry.unpack_in(dest).map_err(invalid)? {
            return Err(HarnessError::Install(
                "archive contains an unsafe entry path".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn mark_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    std::fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn mark_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

async fn download(
    source: &Source<'_>,
    dest: &Path,
    display_name: &str,
) -> Result<(), HarnessError> {
    let url = source.url;
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
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        return Err(failed(format!(
            "server reported an archive larger than the {} MiB limit",
            MAX_ARCHIVE_BYTES / (1024 * 1024)
        )));
    }
    let mut body = response.bytes_stream();
    let mut file = tokio::fs::File::create(dest).await?;
    let mut received = 0_u64;
    let mut sha512 = Sha512::new();
    let mut sha256 = Sha256::new();
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
            Some(Ok(bytes)) => {
                received = received.saturating_add(bytes.len() as u64);
                if received > MAX_ARCHIVE_BYTES {
                    return Err(failed(format!(
                        "archive exceeded the {} MiB limit",
                        MAX_ARCHIVE_BYTES / (1024 * 1024)
                    )));
                }
                match source.checksum {
                    Some(Checksum::Sha512(_)) => sha512.update(&bytes),
                    Some(Checksum::Sha256(_)) => sha256.update(&bytes),
                    None => {}
                }
                file.write_all(&bytes).await?;
            }
            Some(Err(e)) => return Err(failed(e.to_string())),
            None => break,
        }
    }
    file.flush().await?;
    let (algorithm, expected, actual) = match source.checksum {
        Some(Checksum::Sha512(expected)) => {
            ("SHA-512", expected, format!("{:x}", sha512.finalize()))
        }
        Some(Checksum::Sha256(expected)) => {
            ("SHA-256", expected, format!("{:x}", sha256.finalize()))
        }
        None => return Ok(()),
    };
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(failed(format!(
            "{algorithm} mismatch (expected {expected}, got {actual})"
        )));
    }
    Ok(())
}

pub(crate) async fn extract_zip(archive: &Path, dest: &Path) -> Result<(), HarnessError> {
    let archive = archive.to_owned();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || extract_zip_blocking(&archive, &dest))
        .await
        .map_err(|error| {
            HarnessError::Install(format!("archive extraction task failed: {error}"))
        })?
}

fn archive_entry_path(
    dest: &Path,
    enclosed_name: Option<&Path>,
    unix_mode: Option<u32>,
) -> Result<PathBuf, HarnessError> {
    let name = enclosed_name
        .ok_or_else(|| HarnessError::Install("archive contains an unsafe entry path".into()))?;
    if let Some(mode) = unix_mode {
        let kind = mode & 0o170000;
        if !matches!(kind, 0 | 0o040000 | 0o100000) {
            return Err(HarnessError::Install(format!(
                "archive entry {} is a link or special file",
                name.display()
            )));
        }
    }
    Ok(dest.join(name))
}

fn extract_zip_blocking(archive_path: &Path, dest: &Path) -> Result<(), HarnessError> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| HarnessError::Install(format!("invalid zip archive: {error}")))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(HarnessError::Install(format!(
            "archive has {} entries, exceeding the {MAX_ARCHIVE_ENTRIES} entry limit",
            archive.len()
        )));
    }
    let mut extracted = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            HarnessError::Install(format!("could not read archive entry {index}: {error}"))
        })?;
        extracted = extracted.checked_add(entry.size()).ok_or_else(|| {
            HarnessError::Install("archive's extracted size overflows its limit".into())
        })?;
        if extracted > MAX_EXTRACTED_BYTES {
            return Err(HarnessError::Install(format!(
                "archive expands beyond the {} MiB limit",
                MAX_EXTRACTED_BYTES / (1024 * 1024)
            )));
        }
        let enclosed_name = entry.enclosed_name();
        let output = archive_entry_path(dest, enclosed_name.as_deref(), entry.unix_mode())?;
        if output == archive_path {
            return Err(HarnessError::Install(
                "archive attempts to overwrite its download file".into(),
            ));
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output_file = std::fs::File::create(&output)?;
        let copied = std::io::copy(&mut entry, &mut output_file)?;
        if copied != entry.size() {
            return Err(HarnessError::Install(format!(
                "archive entry {} extracted {copied} bytes but declared {}",
                output.display(),
                entry.size()
            )));
        }
        output_file.flush()?;
    }
    Ok(())
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
    async fn corrupt_archive_reports_the_parser_failure() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("broken.zip");
        std::fs::write(&archive, "not a zip").unwrap();
        let error = extract_zip(&archive, dir.path()).await.unwrap_err();
        assert!(
            matches!(error, HarnessError::Install(ref m) if m.contains("invalid zip archive")),
            "{error}"
        );
    }

    #[test]
    fn archive_entries_reject_traversal_links_and_special_files() {
        let dest = Path::new("/safe/root");
        assert!(archive_entry_path(dest, None, Some(0o100644)).is_err());
        assert!(archive_entry_path(dest, Some(Path::new("link")), Some(0o120777)).is_err());
        assert!(archive_entry_path(dest, Some(Path::new("device")), Some(0o020666)).is_err());
        assert_eq!(
            archive_entry_path(dest, Some(Path::new("bin/server")), Some(0o100755)).unwrap(),
            dest.join("bin/server")
        );
    }

    /// runs in a child process, since installs resolve through the
    /// process-wide `ZERON_ADAPTERS_DIR`.
    #[test]
    fn verified_installs_commit_only_after_verification_passes() {
        if std::env::var_os("ZERON_TEST_VERIFIED_INSTALL").is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "archive_install::tests::verified_installs_commit_only_after_verification_passes",
                    "--nocapture",
                ])
                .env("ZERON_ADAPTERS_DIR", root.path())
                .env("ZERON_TEST_VERIFIED_INSTALL", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                zip.start_file("server", zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"#!/bin/sh\n").unwrap();
                let bytes = zip.finish().unwrap().into_inner();
                let url = serve(bytes).await;
                let release = VerifiedRelease {
                    name: "verified-acp",
                    version: "2.0.0",
                    url: &url,
                    entry: "server",
                    marker: "vendor-signature",
                };

                let rejected = install_verified(&release, "Test", |dir| async move {
                    assert!(dir.join("server").is_file());
                    Err(HarnessError::Install("not signed".into()))
                })
                .await
                .unwrap_err();
                assert!(rejected.to_string().contains("not signed"), "{rejected}");
                let root = adapters_root().unwrap();
                assert!(!root.join("verified-acp").exists());
                assert!(
                    std::fs::read_dir(&root)
                        .unwrap()
                        .flatten()
                        .all(|entry| !entry.file_name().to_string_lossy().starts_with(".tmp-")),
                    "the rejected download is discarded"
                );

                let entry = install_verified(&release, "Test", |_| async { Ok(()) })
                    .await
                    .unwrap();
                assert_eq!(entry, root.join("verified-acp/2.0.0/server"));
                assert_eq!(
                    installed_versions("verified-acp"),
                    vec![("2.0.0".to_owned(), "vendor-signature".to_owned())]
                );
                assert_eq!(
                    installed_entry_with_marker("verified-acp", "2.0.0", "server", "other"),
                    None
                );
            });
    }

    /// answer every request with `body`, returning the url to fetch it.
    async fn serve(body: Vec<u8>) -> String {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/archive.zip", listener.local_addr().unwrap());
        let body = std::sync::Arc::new(body);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    let mut headers = Vec::new();
                    let mut buf = [0; 4096];
                    while !headers.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => headers.extend_from_slice(&buf[..read]),
                        }
                    }
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        url
    }

    #[test]
    fn entry_resolves_only_after_a_completed_install() {
        let pin = ArchivePin {
            name: "never-installed-acp",
            version: "0.0.0-test",
            url: "https://example.invalid/x.zip",
            entry: "server",
            sha512: "test-digest",
        };
        assert!(installed_entry(&pin).is_none());
        if std::env::var_os("HOME").is_some() || std::env::var_os("ZERON_ADAPTERS_DIR").is_some() {
            let expected = install_dir(pin.name, pin.version).unwrap().join(pin.entry);
            assert!(expected.ends_with("never-installed-acp/0.0.0-test/server"));
        }
    }

    fn tar_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            // Raw name bytes: `set_path` itself refuses `..`.
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn archive_kind_follows_the_file_name() {
        for (url, kind) in [
            ("https://x/a.zip", ArchiveKind::Zip),
            ("https://x/a.tar.gz?sig=1", ArchiveKind::TarGz),
            ("https://x/a.tgz", ArchiveKind::TarGz),
            ("https://x/a.TAR.BZ2", ArchiveKind::TarBz2),
            ("https://x/a.tar", ArchiveKind::Tar),
            ("https://x/sigit-linux-amd64", ArchiveKind::Executable),
            ("https://x/agent.exe", ArchiveKind::Executable),
        ] {
            assert_eq!(ArchiveKind::of(url), kind, "{url}");
        }
    }

    #[test]
    fn extracts_gzip_and_bzip2_tarballs() {
        use std::io::Write as _;
        let tarball = tar_with(&[("pkg/agent", b"#!/bin/sh\n")]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tarball).unwrap();
        let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        bz.write_all(&tarball).unwrap();
        for (bytes, kind) in [
            (gz.finish().unwrap(), ArchiveKind::TarGz),
            (bz.finish().unwrap(), ArchiveKind::TarBz2),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let reader: Box<dyn std::io::Read> = match kind {
                ArchiveKind::TarGz => Box::new(flate2::read::GzDecoder::new(&bytes[..])),
                _ => Box::new(bzip2::read::BzDecoder::new(&bytes[..])),
            };
            extract_tar_blocking(reader, dir.path()).unwrap();
            assert_eq!(
                std::fs::read(dir.path().join("pkg/agent")).unwrap(),
                b"#!/bin/sh\n"
            );
        }
    }

    #[test]
    fn tar_entries_cannot_escape_the_install_dir() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("install");
        std::fs::create_dir(&dest).unwrap();
        let tarball = tar_with(&[("../escaped", b"x")]);
        assert!(extract_tar_blocking(&tarball[..], &dest).is_err());
        assert!(!root.path().join("escaped").exists());
    }
}
