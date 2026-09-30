//! zeron-update — release checking and self-update, shared by the engine (the
//! background checker + `ApplyUpdate`), the CLI (`zeron update`), and the UI
//! (its own report-only checker, the "Check for Updates…" menu item, the
//! sidebar update strip, and the desktop install paths).
//!
//! Release layout (see `.github/workflows/release.yml` and `edge/src/install.sh`):
//! artifacts live in the `comet-native-releases` R2 bucket, served pre-auth at
//! `{edge}/releases/*`. `manifest.json` carries the latest version plus a
//! sha256 per artifact; `latest.txt` (version only) remains as the fallback for
//! releases published before the manifest existed.
//!
//! Install kinds and their update paths:
//! - **Managed** (`~/.zeron/app/<ver>` + `current` symlink — the curl|sh
//!   installer and the Linux tarball's `install.sh`): download the headless
//!   tarball into a new versioned dir, flip the symlink, then restart the
//!   service (daemon) or relaunch (desktop). Same flow the installer script
//!   performs, natively.
//! - **MacApp** (running out of an app bundle): download the app tarball, swap the
//!   bundle directory, relaunch. Driven by the UI.
//! - **WindowsPortable** (the Windows installer or portable zip — both carry
//!   `zeron-update.json`): swap the executable in place. Driven by the UI.
//! - **Unmanaged** (source builds, hand-copied binaries): report only — the
//!   UI's advisory strip links to [`RELEASES_PAGE`].
//!
//! Desktop installs download as soon as a release is seen and install either
//! on "Restart to update" or when the app next quits, so an app nobody
//! restarts deliberately still lands on the new version at its next launch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, bail};
use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;

#[cfg(windows)]
pub mod windows;

/// The version compiled into this binary (the workspace version).
pub const fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Successful checks repeat this often. The feed is a sub-kilobyte,
/// edge-cached document, so hourly polling is free and bounds how long a
/// long-running app can sit on a stale release.
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Failed checks (offline boot, captive portal, transient edge error) back off
/// through these delays, then stay at the last one.
const CHECK_RETRY: [Duration; 4] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
];
/// Check deadlines are wall-clock times: a monotonic sleep stops counting
/// while the machine sleeps, so a laptop closed overnight used to push its
/// next check out by the whole night. The loop re-reads the clock this often.
const SCHEDULE_TICK: Duration = Duration::from_secs(60);
/// The engine's first check waits out engine boot (room joins, doc re-sync).
const ENGINE_INITIAL_DELAY: Duration = Duration::from_secs(20);
/// The desktop checker only needs the window on screen first.
const DESKTOP_INITIAL_DELAY: Duration = Duration::from_secs(2);
/// While an auto-apply is deferred behind active sessions, re-probe idleness
/// this often.
const IDLE_RECHECK: Duration = Duration::from_secs(5 * 60);
/// A staged binary must answer `--version` within this window.
const STAGED_VERSION_TIMEOUT: Duration = Duration::from_secs(30);
/// Release metadata is tiny. Bound both its buffered size and total transfer
/// time so a compromised or misconfigured public feed cannot hold a checker
/// forever or make every local install buffer an unbounded response.
const RELEASE_METADATA_MAX_BYTES: usize = 1024 * 1024;
const RELEASE_METADATA_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Release metadata
// ---------------------------------------------------------------------------

/// `{edge}/releases/manifest.json` — written by the release workflow.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// Artifact file name → metadata. Empty for pre-manifest releases resolved
    /// via `latest.txt` — downloads then skip checksum verification (with a log).
    #[serde(default)]
    pub files: BTreeMap<String, FileMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Artifact-name platform pair matching the packaging scripts. Unsupported
/// updater targets retain their real OS name rather than impersonating Linux.
pub fn platform_key() -> (&'static str, &'static str) {
    let os = std::env::consts::OS;
    let arch = match (os, std::env::consts::ARCH) {
        ("macos", "aarch64") => "arm64",
        (_, arch) => arch,
    };
    (os, arch)
}

fn managed_updates_supported(os: &str) -> bool {
    matches!(os, "linux" | "macos")
}

fn require_managed_update_platform() -> anyhow::Result<()> {
    if !managed_updates_supported(std::env::consts::OS) {
        bail!(
            "managed updates are not supported on {}",
            std::env::consts::OS
        );
    }
    Ok(())
}

fn require_mac_app_update_platform() -> anyhow::Result<()> {
    if std::env::consts::OS != "macos" {
        bail!(
            "macOS app updates are not supported on {}",
            std::env::consts::OS
        );
    }
    Ok(())
}

/// `zeron-<ver>-<os>-<arch>.tar.gz` — the headless/CLI tarball (Linux CI builds).
pub fn headless_artifact(version: &str) -> String {
    let (os, arch) = platform_key();
    format!("zeron-{version}-{os}-{arch}.tar.gz")
}

/// `zeron-<ver>-macos-<arch>-app.tar.gz` — the macOS app update payload.
pub fn mac_app_artifact(version: &str) -> String {
    let (_, arch) = platform_key();
    format!("zeron-{version}-macos-{arch}-app.tar.gz")
}

fn parse_version(v: &str) -> Option<Vec<u64>> {
    let nums: Vec<u64> = v
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    (!nums.is_empty()).then_some(nums)
}

/// Strictly-newer dotted-numeric compare (`0.1.10` > `0.1.9` > `0.1`).
/// Unparseable versions never count as newer — a garbage `latest.txt` must not
/// trigger an update loop.
pub fn version_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Fetch the newest release metadata: `manifest.json`, falling back to
/// `latest.txt` (version only, no checksums) for pre-manifest releases.
pub async fn fetch_latest(edge_url: &str) -> anyhow::Result<Manifest> {
    let base = release_base(edge_url)?;
    let client = http_client()?;
    let manifest_url = format!("{base}/manifest.json");
    match fetch_release_metadata(&client, &manifest_url).await {
        Ok(response) if response.status.is_success() => {
            let manifest: Manifest =
                serde_json::from_slice(&response.body).context("parsing manifest.json")?;
            if manifest.version.trim().is_empty() {
                bail!("manifest.json has an empty version");
            }
            return Ok(manifest);
        }
        Ok(response) => {
            tracing::debug!(status = %response.status, "manifest.json unavailable; trying latest.txt")
        }
        Err(err) => tracing::debug!(error = %err, "manifest.json fetch failed; trying latest.txt"),
    }
    let latest_url = format!("{base}/latest.txt");
    let response = fetch_release_metadata(&client, &latest_url)
        .await
        .context("fetching latest.txt")?;
    anyhow::ensure!(
        response.status.is_success(),
        "fetching latest.txt: HTTP {}",
        response.status
    );
    let version = std::str::from_utf8(&response.body)
        .context("reading latest.txt as UTF-8")?
        .trim()
        .to_string();
    if version.is_empty() {
        bail!("latest.txt is empty");
    }
    Ok(Manifest {
        version,
        files: BTreeMap::new(),
    })
}

#[derive(Debug)]
struct ReleaseMetadataResponse {
    status: reqwest::StatusCode,
    body: Vec<u8>,
}

async fn fetch_release_metadata(
    client: &reqwest::Client,
    url: &str,
) -> anyhow::Result<ReleaseMetadataResponse> {
    fetch_release_metadata_with_limits(
        client,
        url,
        RELEASE_METADATA_TIMEOUT,
        RELEASE_METADATA_MAX_BYTES,
    )
    .await
}

async fn fetch_release_metadata_with_limits(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
    max_bytes: usize,
) -> anyhow::Result<ReleaseMetadataResponse> {
    let request = async {
        let response = client
            .get(url)
            // Intermediaries must revalidate: the edge already bounds its own
            // cache to a minute, and a proxy holding an old manifest for
            // longer is exactly the "prompt appears late" failure.
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await
            .with_context(|| format!("fetching {url}"))?;
        let status = response.status();
        if !status.is_success() {
            return Ok(ReleaseMetadataResponse {
                status,
                body: Vec::new(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            bail!("release metadata exceeds {max_bytes} bytes");
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("reading release metadata")?;
            anyhow::ensure!(
                body.len().saturating_add(chunk.len()) <= max_bytes,
                "release metadata exceeds {max_bytes} bytes"
            );
            body.extend_from_slice(&chunk);
        }
        Ok(ReleaseMetadataResponse { status, body })
    };
    tokio::time::timeout(timeout, request)
        .await
        .with_context(|| format!("fetching {url} exceeded the metadata deadline"))?
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    http_client_with_timeouts(Duration::from_secs(15), Duration::from_secs(30))
}

fn http_client_with_timeouts(connect: Duration, read: Duration) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(connect)
        // Inactivity timeout, not a total download cap: slow progressing
        // updates remain viable on constrained links.
        .read_timeout(read)
        .user_agent(concat!("zeron/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many update redirects");
            }
            if attempt.previous().iter().any(|url| url.scheme() == "https")
                && attempt.url().scheme() != "https"
            {
                return attempt.error("update redirect would downgrade HTTPS");
            }
            attempt.follow()
        }))
        .build()
        .context("building http client")
}

fn validate_release_override(value: &str) -> anyhow::Result<String> {
    let url = reqwest::Url::parse(value.trim()).context("invalid update feed URL")?;
    anyhow::ensure!(
        url.scheme() == "https" && url.host_str().is_some(),
        "update feed must use HTTPS"
    );
    anyhow::ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "update feed must be a base URL without credentials, query, or fragment"
    );
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// The project's GitHub releases page — the advisory update strip opens this
/// for unmanaged installs (source builds, hand-copied binaries), where no
/// updater flow exists to drive.
pub const RELEASES_PAGE: &str = "https://github.com/zeronsh/zeron/releases";

/// The newest release's page — the download destination offered when this
/// installation cannot replace itself.
pub const LATEST_RELEASE_PAGE: &str = "https://github.com/zeronsh/zeron/releases/latest";

fn release_base(edge_url: &str) -> anyhow::Result<String> {
    if let Ok(url) = std::env::var("ZERON_RELEASES_URL")
        && !url.trim().is_empty()
    {
        return validate_release_override(&url);
    }
    #[cfg(windows)]
    if let Some(url) = windows::release_url()? {
        return Ok(url.trim_end_matches('/').to_owned());
    }
    Ok(format!("{}/releases", edge_url.trim_end_matches('/')))
}

// ---------------------------------------------------------------------------
// Install-kind detection
// ---------------------------------------------------------------------------

/// How this binary was installed — decides the update path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// `~/.zeron/app/<ver>/zeron` behind the `current` symlink
    /// (curl|sh installer, the Linux tarball's `install.sh`, or a previous
    /// `zeron update`).
    Managed { app_root: PathBuf },
    /// Running out of a macOS `.app` bundle.
    MacApp { bundle: PathBuf },
    /// Windows installer or portable package with an explicit update-feed
    /// configuration.
    #[cfg(windows)]
    WindowsPortable { directory: PathBuf },
    /// Source build or hand-copied binary — updates are report-only.
    Unmanaged,
}

/// Why an installation that normally updates itself cannot right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateBlocker {
    /// macOS App Translocation ran the app from a randomized read-only path
    /// (launched from Downloads or a disk image without being moved first).
    Translocated,
    /// Running straight from the mounted disk image.
    DiskImage,
    /// The directory holding the installation is not writable by this user.
    NotWritable(PathBuf),
}

impl std::fmt::Display for UpdateBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Translocated | Self::DiskImage => f.write_str(
                "Zeron is running from a temporary, read-only location. Move Zeron to your \
                 Applications folder and reopen it to turn on updates.",
            ),
            Self::NotWritable(dir) => write!(
                f,
                "Zeron doesn't have permission to replace itself in {}.",
                dir.display()
            ),
        }
    }
}

impl InstallKind {
    /// Whether the desktop app downloads and installs updates itself.
    pub fn supports_desktop_update(&self) -> bool {
        match self {
            Self::MacApp { .. } => true,
            // The desktop path relaunches a GUI binary; on Linux that binary
            // is the managed install itself.
            Self::Managed { .. } => cfg!(target_os = "linux"),
            #[cfg(windows)]
            Self::WindowsPortable { .. } => true,
            Self::Unmanaged => false,
        }
    }

    /// Why this installation cannot replace itself, when it can't. Checked
    /// before downloading so the UI can explain the fix instead of failing
    /// after a wasted download with a raw rename error.
    pub fn desktop_update_blocker(&self) -> Option<UpdateBlocker> {
        match self {
            Self::MacApp { bundle } => mac_bundle_blocker(bundle, dir_writable),
            Self::Managed { app_root } => {
                (!dir_writable(app_root)).then(|| UpdateBlocker::NotWritable(app_root.clone()))
            }
            #[cfg(windows)]
            Self::WindowsPortable { directory } => {
                (!dir_writable(directory)).then(|| UpdateBlocker::NotWritable(directory.clone()))
            }
            Self::Unmanaged => None,
        }
    }

    pub async fn stage_desktop(
        &self,
        edge_url: &str,
        manifest: &Manifest,
        data_dir: &Path,
    ) -> anyhow::Result<PathBuf> {
        match self {
            Self::MacApp { .. } => stage_mac_app(edge_url, manifest, data_dir).await,
            Self::Managed { app_root } if self.supports_desktop_update() => {
                stage_headless(edge_url, manifest, app_root).await
            }
            #[cfg(windows)]
            Self::WindowsPortable { directory } => {
                windows::stage(edge_url, manifest, directory).await
            }
            _ => bail!("this installation does not support desktop updates"),
        }
    }

    /// Install a staged update. With `relaunch`, the new version starts once
    /// this process exits ("Restart to update"); without it the update takes
    /// effect at the next launch ("install on quit"). Either way the caller
    /// must quit after this succeeds.
    pub fn apply_desktop(&self, staged: &Path, relaunch: bool) -> anyhow::Result<()> {
        match self {
            Self::MacApp { bundle } => {
                apply_mac_app(staged, bundle)?;
                // The swap copied the staged bundle; the cache is spent.
                if let Some(version_dir) = staged.parent() {
                    let _ = std::fs::remove_dir_all(version_dir);
                }
                if relaunch {
                    relaunch_after_exit(Path::new("/usr/bin/open"), bundle);
                }
                Ok(())
            }
            Self::Managed { app_root } if self.supports_desktop_update() => {
                let version = staged
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("staged install has no version directory")?;
                apply_headless(app_root, version)?;
                if relaunch {
                    let binary = app_root.join("current").join("zeron");
                    relaunch_after_exit(&binary, Path::new(""));
                }
                Ok(())
            }
            #[cfg(windows)]
            Self::WindowsPortable { directory } => windows::apply(staged, directory, relaunch),
            _ => bail!("this installation does not support desktop updates"),
        }
    }
}

fn mac_bundle_blocker(bundle: &Path, writable: impl Fn(&Path) -> bool) -> Option<UpdateBlocker> {
    if bundle
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Some(UpdateBlocker::Translocated);
    }
    let parent = bundle.parent()?;
    if writable(parent) {
        return None;
    }
    if bundle.starts_with("/Volumes") {
        return Some(UpdateBlocker::DiskImage);
    }
    Some(UpdateBlocker::NotWritable(parent.to_path_buf()))
}

/// Probe by creating (and removing) a file: permission bits alone miss
/// read-only mounts, ACLs, and sandboxed locations.
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".zeron-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            true
        }
        // A leftover probe from a crashed run still proves writability.
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn detect_install() -> InstallKind {
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Unmanaged;
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    detect_install_from(&exe, home.as_deref())
}

fn detect_install_from(exe: &Path, home: Option<&Path>) -> InstallKind {
    detect_install_from_for_os(exe, home, std::env::consts::OS)
}

fn detect_install_from_for_os(exe: &Path, home: Option<&Path>, os: &str) -> InstallKind {
    #[cfg(windows)]
    if os == "windows" && windows::is_managed(exe) {
        return InstallKind::WindowsPortable {
            directory: exe.parent().unwrap().to_owned(),
        };
    }
    // Never interpret a coincidental Windows `%HOME%\.zeron\app` layout as
    // the Unix symlink-managed installation.
    if !managed_updates_supported(os) {
        return InstallKind::Unmanaged;
    }
    if let Some(home) = home {
        // `current_exe` resolves the `current` symlink to the versioned dir.
        let app_root = home.join(".zeron").join("app");
        if exe.starts_with(&app_root) {
            return InstallKind::Managed { app_root };
        }
    }
    for ancestor in exe.ancestors() {
        if ancestor.extension().is_some_and(|ext| ext == "app")
            && exe.starts_with(ancestor.join("Contents").join("MacOS"))
        {
            return InstallKind::MacApp {
                bundle: ancestor.to_path_buf(),
            };
        }
    }
    InstallKind::Unmanaged
}

/// The version currently installed on disk for `kind`, read without executing
/// anything. A long-running process compares it with its own version to notice
/// that something else (the desktop app, `zeron update`, a re-run installer)
/// has already replaced its binary.
pub fn installed_version(kind: &InstallKind) -> Option<String> {
    let version = match kind {
        InstallKind::Managed { app_root } => std::fs::read_link(app_root.join("current"))
            .ok()?
            .file_name()?
            .to_str()?
            .to_owned(),
        InstallKind::MacApp { bundle } => bundle_short_version(
            &std::fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?,
        )?,
        _ => return None,
    };
    parse_version(&version).map(|_| version)
}

/// `CFBundleShortVersionString` from an XML property list (the packaging
/// template; codesign leaves the format alone).
fn bundle_short_version(plist: &str) -> Option<String> {
    let after_key = plist
        .split("<key>CFBundleShortVersionString</key>")
        .nth(1)?;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    let end = value.find("</string>")?;
    Some(value[..end].trim().to_owned())
}

// ---------------------------------------------------------------------------
// Download + verify
// ---------------------------------------------------------------------------

/// Stream `{edge}/releases/<file>` to `dest`, verifying the manifest sha256 when
/// present. Writes through a `.partial` sidecar so an interrupted download never
/// leaves a plausible-looking artifact behind.
pub async fn download_release_file(
    edge_url: &str,
    manifest: &Manifest,
    file: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    let url = format!("{}/{file}", release_base(edge_url)?);
    let expected = manifest.files.get(file).and_then(|m| m.sha256.as_deref());
    if expected.is_none() {
        tracing::warn!(
            file,
            "no checksum in release metadata; skipping verification"
        );
    }
    let partial = dest.with_extension("partial");
    let resp = http_client()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    let mut out = tokio::fs::File::create(&partial)
        .await
        .with_context(|| format!("creating {}", partial.display()))?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading download stream")?;
        hasher.update(&chunk);
        out.write_all(&chunk).await.context("writing download")?;
    }
    out.flush().await.context("flushing download")?;
    drop(out);
    if let Some(expected) = expected {
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            tokio::fs::remove_file(&partial).await.ok();
            bail!("checksum mismatch for {file}: expected {expected}, got {actual}");
        }
    }
    tokio::fs::rename(&partial, dest)
        .await
        .with_context(|| format!("moving {} into place", dest.display()))?;
    Ok(())
}

fn run(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Run a freshly unpacked binary's `--version` before it can replace anything:
/// proves the payload is complete, executable on this machine, and the release
/// the manifest promised.
async fn verify_staged_binary(binary: &Path, version: &str) -> anyhow::Result<()> {
    let output = tokio::time::timeout(
        STAGED_VERSION_TIMEOUT,
        tokio::process::Command::new(binary)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("staged binary version check timed out")?
    .with_context(|| format!("running {} --version", binary.display()))?;
    let reported = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    anyhow::ensure!(
        output.status.success() && reported == format!("zeron {version}"),
        "staged binary reported {reported:?} (exit {}), expected \"zeron {version}\"",
        output.status
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Managed (symlink) installs — the daemon/VPS path and Linux desktop
// ---------------------------------------------------------------------------

/// Download + unpack the headless tarball into `app_root/<ver>` (idempotent —
/// an already-staged version is reused). Returns the versioned dir.
pub async fn stage_headless(
    edge_url: &str,
    manifest: &Manifest,
    app_root: &Path,
) -> anyhow::Result<PathBuf> {
    // Reject unsupported targets before creating a stage or making a request.
    require_managed_update_platform()?;
    let version = &manifest.version;
    anyhow::ensure!(
        parse_version(version).is_some(),
        "invalid release version {version:?}"
    );
    let dest = app_root.join(version);
    if dest.join("zeron").exists() {
        return Ok(dest);
    }
    let file = headless_artifact(version);
    let stage = app_root.join(format!(".stage-{version}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).with_context(|| format!("creating {}", stage.display()))?;
    let result = async {
        let tarball = stage.join(&file);
        download_release_file(edge_url, manifest, &file, &tarball).await?;
        let unpacked = stage.join("unpacked");
        std::fs::create_dir_all(&unpacked)?;
        // Tarball root is the versioned stage dir (see scripts/package-linux.sh);
        // strip it exactly as install.sh does.
        run(
            "tar",
            &[
                "-xzf",
                &tarball.to_string_lossy(),
                "-C",
                &unpacked.to_string_lossy(),
                "--strip-components=1",
            ],
        )?;
        if !unpacked.join("zeron").is_file() {
            bail!("tarball {file} did not contain a zeron binary");
        }
        verify_staged_binary(&unpacked.join("zeron"), version).await?;
        match std::fs::rename(&unpacked, &dest) {
            Ok(()) => {}
            // Lost a race with another stager — the staged copy is equivalent.
            Err(_) if dest.join("zeron").exists() => {}
            Err(err) => {
                return Err(err).with_context(|| format!("moving {} into place", dest.display()));
            }
        }
        Ok(dest.clone())
    }
    .await;
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// Atomically repoint `app_root/current` at `app_root/<ver>` (symlink to a temp
/// name, then rename over — never a window with no `current`).
pub fn apply_headless(app_root: &Path, version: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let target = app_root.join(version);
        if !target.join("zeron").exists() {
            bail!("{} is not a staged install", target.display());
        }
        let tmp = app_root.join(format!(".current-{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&target, &tmp).context("creating current symlink")?;
        std::fs::rename(&tmp, app_root.join("current")).context("swapping current symlink")?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (app_root, version);
        require_managed_update_platform()?;
        unreachable!("supported managed-update platforms are Unix")
    }
}

/// Whether this process is the engine service [`restart_service`] manages:
/// systemd places it in the `zeron.service` cgroup; launchd names the job in
/// `XPC_SERVICE_NAME`.
pub fn running_as_installed_service() -> bool {
    if cfg!(target_os = "macos") {
        std::env::var("XPC_SERVICE_NAME").is_ok_and(|label| label == "sh.zeron.app")
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/self/cgroup")
            .is_ok_and(|cgroups| in_zeron_service_cgroup(&cgroups))
    } else {
        false
    }
}

fn in_zeron_service_cgroup(cgroups: &str) -> bool {
    cgroups
        .lines()
        .filter_map(|line| line.rsplit(':').next())
        .any(|path| path.split('/').any(|part| part == "zeron.service"))
}

/// Restart the installed engine service (the same units `zeron daemon` and the
/// curl|sh installer manage). Called after a symlink swap so the running daemon
/// picks up the new binary. Only queues the restart: the caller may be the
/// service itself, which must stay responsive to the stop signal that follows.
pub fn restart_service() -> anyhow::Result<()> {
    require_managed_update_platform()?;
    if cfg!(target_os = "macos") {
        let output = std::process::Command::new("id").arg("-u").output()?;
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        run(
            "launchctl",
            &["kickstart", "-k", &format!("gui/{uid}/sh.zeron.app")],
        )
    } else {
        run(
            "systemctl",
            &["--user", "--no-block", "restart", "zeron.service"],
        )
    }
}

// ---------------------------------------------------------------------------
// macOS app-bundle installs — the desktop path
// ---------------------------------------------------------------------------

/// Download + unpack the app tarball into `{data_dir}/updates/<ver>/Zeron.app`
/// (idempotent). The bundle is unpacked beside its final name and renamed in
/// only after its binary answers with the expected version, so an interrupted
/// unpack can never be mistaken for a staged update. Returns the staged bundle.
pub async fn stage_mac_app(
    edge_url: &str,
    manifest: &Manifest,
    data_dir: &Path,
) -> anyhow::Result<PathBuf> {
    // Reject unsupported targets before creating a stage or making a request.
    require_mac_app_update_platform()?;
    let version = &manifest.version;
    anyhow::ensure!(
        parse_version(version).is_some(),
        "invalid release version {version:?}"
    );
    let updates = data_dir.join("updates");
    let dir = updates.join(version);
    let staged = dir.join("Zeron.app");
    let staged_binary = staged.join("Contents/MacOS/zeron");
    if staged_binary.exists() && verify_staged_binary(&staged_binary, version).await.is_ok() {
        return Ok(staged);
    }
    prune_stale_stages(&updates, version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = mac_app_artifact(version);
    let tarball = dir.join(&file);
    download_release_file(edge_url, manifest, &file, &tarball).await?;
    let unpack = dir.join(".unpack");
    std::fs::create_dir_all(&unpack)?;
    let unpacked = run(
        "tar",
        &[
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &unpack.to_string_lossy(),
        ],
    )
    .map(|()| unpack.join("Zeron.app"));
    std::fs::remove_file(&tarball).ok();
    let unpacked = unpacked?;
    let unpacked_binary = unpacked.join("Contents/MacOS/zeron");
    if !unpacked_binary.exists() {
        let _ = std::fs::remove_dir_all(&dir);
        bail!("app tarball {file} did not contain Zeron.app");
    }
    if let Err(err) = verify_staged_binary(&unpacked_binary, version).await {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(err);
    }
    std::fs::rename(&unpacked, &staged)
        .with_context(|| format!("moving {} into place", staged.display()))?;
    let _ = std::fs::remove_dir_all(&unpack);
    Ok(staged)
}

/// Staged bundles of other versions are dead weight (each is a whole app):
/// drop them whenever a new version stages.
fn prune_stale_stages(updates: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(updates) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != keep {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Swap the installed bundle for the staged one: `ditto` the staged copy next to
/// the target (metadata-preserving, cross-volume safe), then two renames — the
/// old bundle is restored if the second rename fails.
pub fn apply_mac_app(staged: &Path, bundle: &Path) -> anyhow::Result<()> {
    require_mac_app_update_platform()?;
    let parent = bundle
        .parent()
        .context("app bundle has no parent directory")?;
    let name = bundle
        .file_name()
        .context("app bundle has no name")?
        .to_string_lossy();
    let pid = std::process::id();
    let fresh = parent.join(format!(".{name}.new-{pid}"));
    let old = parent.join(format!(".{name}.old-{pid}"));
    let _ = std::fs::remove_dir_all(&fresh);
    if let Err(err) = run(
        "ditto",
        &[&staged.to_string_lossy(), &fresh.to_string_lossy()],
    ) {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(bundle, &old) {
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(err).context("moving the current app aside");
    }
    if let Err(err) = std::fs::rename(&fresh, bundle) {
        let _ = std::fs::rename(&old, bundle);
        let _ = std::fs::remove_dir_all(&fresh);
        return Err(err).context("installing the new app bundle");
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Detached relauncher: waits for THIS process to exit, then runs `program`
/// (with `argument` unless empty). Launching before exit would race the
/// single-instance engine lock and the IPC port. Paths travel as positional
/// parameters, never spliced into the script. The caller quits after this.
pub fn relaunch_after_exit(program: &Path, argument: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let pid = std::process::id().to_string();
        let script = r#"while /bin/kill -0 "$1" 2>/dev/null; do sleep 0.2; done
if [ -n "$3" ]; then exec "$2" "$3"; else exec "$2"; fi"#;
        let mut command = std::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .arg("zeron-relaunch")
            .arg(&pid)
            .arg(program)
            .arg(argument)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        if let Err(err) = command.spawn() {
            tracing::error!(error = %err, "failed to spawn the relauncher");
        }
    }
    #[cfg(not(unix))]
    let _ = (program, argument);
}

// ---------------------------------------------------------------------------
// Background checker
// ---------------------------------------------------------------------------

/// What the checker reports (the engine's `UpdateStatus` stream, the desktop
/// strip). Version facts only — download/apply progress is owned by whoever
/// drives the update (UI or CLI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub update_available: bool,
    /// Epoch ms of the last successful check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UpdateStatus {
    fn initial() -> Self {
        Self {
            current_version: current_version().to_string(),
            latest_version: None,
            update_available: false,
            checked_at: None,
            error: None,
        }
    }
}

/// Whether an engine watches for a newer installed binary at all: the
/// installed service always does (it can restart into it); any other engine
/// only when it can hand itself over in place.
fn watches_for_newer_install(is_installed_service: bool, can_hand_off: bool) -> bool {
    is_installed_service || can_hand_off
}

/// Whether an engine applies updates itself. `ZERON_AUTO_UPDATE=1|true|yes`
/// always opts in (an engine without a live handoff then restarts at a quiet
/// moment, as before) and `0|false|no` keeps it report-only. Unset means on
/// ONLY for an engine that can hand itself over in place (`can_hand_off`; see
/// [`HandoffHook`]): there an update disturbs nothing, so it needs no opt-in.
/// An engine without a hook (the desktop app's in-process engine) must never
/// install and restart on its own by default. An engine host the desktop app
/// started (`ZERON_ENGINE_HOST=app`) leaves downloading and installing to the app.
pub fn engine_auto_update_enabled(can_hand_off: bool) -> bool {
    engine_auto_update_from(
        std::env::var("ZERON_AUTO_UPDATE").ok().as_deref(),
        engine_host().as_deref(),
        can_hand_off,
    )
}

static ENGINE_HOST: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Read `ZERON_ENGINE_HOST` (set by an app that started this process as its
/// engine host) into the process and REMOVE it from the environment, so the
/// agents and terminals this process spawns never inherit it and mistake
/// themselves for an app-hosted engine. `keep` says whether this process is
/// such a host at all. Call once, first thing in `main`, while single-threaded.
pub fn capture_engine_host_env(keep: bool) {
    let value = std::env::var("ZERON_ENGINE_HOST").ok();
    // SAFETY: the caller runs this before any thread exists.
    unsafe { std::env::remove_var("ZERON_ENGINE_HOST") };
    let _ = ENGINE_HOST.set(if keep { value } else { None });
}

/// The engine host role captured by [`capture_engine_host_env`] (`Some("app")`
/// for an app's own host); the environment itself when nothing captured it
/// (tests, embedders).
pub fn engine_host() -> Option<String> {
    match ENGINE_HOST.get() {
        Some(value) => value.clone(),
        None => std::env::var("ZERON_ENGINE_HOST").ok(),
    }
}

fn engine_auto_update_from(
    auto_update: Option<&str>,
    engine_host: Option<&str>,
    can_hand_off: bool,
) -> bool {
    if engine_host.is_some_and(|host| host.trim() == "app") {
        return false;
    }
    match auto_update.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes") => true,
        Some(value) if matches!(value.as_str(), "0" | "false" | "no") => false,
        _ => can_hand_off,
    }
}

/// `ZERON_AUTO_UPDATE=0|false|no` — the desktop app then only reports: no
/// background download and no install on quit. Unset means on.
pub fn desktop_auto_update_enabled() -> bool {
    std::env::var("ZERON_AUTO_UPDATE")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
        .unwrap_or(true)
}

/// What a handoff attempt reports when it did NOT happen (on success the
/// engine's process image is replaced, so nothing comes back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffOutcome {
    /// Something running cannot be carried across right now; try again later.
    /// Nothing was touched.
    Busy(String),
    /// The handoff failed. The engine is unchanged and keeps running.
    Failed(String),
}

/// Replaces the running engine, in place, with the binary at the given path
/// (wired by the engine to `EngineCore::handoff`). With it set, an installed
/// update is picked up by a handoff that keeps agents and terminals running,
/// and the service is never restarted for it — the restart stays only as the
/// fallback when a handoff itself fails.
pub type HandoffHook =
    Arc<dyn Fn(PathBuf) -> futures::future::BoxFuture<'static, HandoffOutcome> + Send + Sync>;

/// The path to exec for the newest installed version of this binary: the
/// `current` symlink of a managed install (so a later update is picked up),
/// the executable inside the app bundle on macOS, otherwise this executable.
pub fn stable_exe() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("resolving the zeron executable path")?;
    Ok(stable_exe_for(&detect_install(), &exe))
}

pub fn stable_exe_for(kind: &InstallKind, exe: &Path) -> PathBuf {
    match kind {
        InstallKind::Managed { app_root } => app_root.join("current").join("zeron"),
        InstallKind::MacApp { bundle } => bundle.join("Contents/MacOS/zeron"),
        _ => exe.to_path_buf(),
    }
}

/// "Nothing would be interrupted by a restart right now" — wired by the engine
/// to its live-run and open-terminal registries. `None` = no gate.
pub type QuiescentCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// Who runs the checker — decides what it may do beyond reporting.
enum Role {
    /// The engine: managed installs with `ZERON_AUTO_UPDATE` apply themselves,
    /// and a service daemon restarts into a binary someone else installed.
    Engine { quiescent: Option<QuiescentCheck> },
    /// The desktop app: report only — downloading and installing are UI
    /// decisions (see `crates/ui/src/app_update.rs`).
    Desktop,
}

/// Background release checker: polls `{edge}/releases` hourly on a wall-clock
/// schedule (so sleep/wake cannot stretch it), backs off on failure, and
/// publishes [`UpdateStatus`] over a watch channel. In the engine, managed
/// installs with `ZERON_AUTO_UPDATE` set stage + apply + service restart on
/// their own — but only in a quiet window: while `quiescent` reports activity,
/// the apply defers and re-probes every [`IDLE_RECHECK`].
#[derive(Clone)]
pub struct Updater {
    edge_url: String,
    role: Arc<Role>,
    status_tx: Arc<watch::Sender<UpdateStatus>>,
    /// Wakes the loop; `forced` says whether the wake demands a check or only
    /// a fresh look at the wall clock.
    wake_tx: Arc<watch::Sender<u64>>,
    forced: Arc<AtomicBool>,
    /// Set by `zeron headless`: this process watches for a newer installed
    /// binary and moves onto it when it can (by handoff, else — for the
    /// installed service only — by restart at a quiet moment).
    service: Arc<AtomicBool>,
    /// How to replace this engine in place; set by the engine once it can.
    handoff: Arc<std::sync::OnceLock<HandoffHook>>,
    /// An installed version this engine must not hand off to: a rollback
    /// brought us back from it, so it cannot adopt this engine's state.
    handoff_blocked: Arc<std::sync::OnceLock<String>>,
    /// Flips to true exactly once; the check loop selects against it so
    /// cancellation lands at any await point (no tokio-util in this crate).
    shutdown_tx: Arc<watch::Sender<bool>>,
    check_task: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl Updater {
    /// Spawn the engine's check loop (must run on a tokio runtime).
    pub fn spawn(edge_url: String, quiescent: Option<QuiescentCheck>) -> Self {
        Self::spawn_role(edge_url, Role::Engine { quiescent })
    }

    /// Spawn the desktop app's report-only check loop (must run on a tokio
    /// runtime).
    pub fn spawn_desktop(edge_url: String) -> Self {
        Self::spawn_role(edge_url, Role::Desktop)
    }

    fn spawn_role(edge_url: String, role: Role) -> Self {
        let initial_delay = match role {
            Role::Engine { .. } => ENGINE_INITIAL_DELAY,
            Role::Desktop => DESKTOP_INITIAL_DELAY,
        };
        let (status_tx, _) = watch::channel(UpdateStatus::initial());
        let (wake_tx, _) = watch::channel(0);
        let (shutdown_tx, _) = watch::channel(false);
        let updater = Self {
            edge_url,
            role: Arc::new(role),
            status_tx: Arc::new(status_tx),
            wake_tx: Arc::new(wake_tx),
            forced: Arc::new(AtomicBool::new(false)),
            service: Arc::new(AtomicBool::new(false)),
            handoff: Arc::new(std::sync::OnceLock::new()),
            handoff_blocked: Arc::new(std::sync::OnceLock::new()),
            shutdown_tx: Arc::new(shutdown_tx),
            check_task: Arc::new(std::sync::Mutex::new(None)),
        };
        // Subscribe before spawning so an immediate `check_now` cannot land
        // before the background task first polls and be lost behind the
        // initial delay.
        let wakes = updater.wake_tx.subscribe();
        let for_loop = updater.clone();
        let task = tokio::spawn(async move { for_loop.check_loop(wakes, initial_delay).await });
        *updater.check_task.lock().unwrap() = Some(task);
        updater
    }

    /// Stop the check loop and wait for it to exit — a replaced runtime must
    /// not keep polling `{edge}/releases` (or auto-applying) in the background.
    /// Idempotent, and callable from any clone.
    pub async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        let task = self
            .check_task
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    pub fn watch(&self) -> watch::Receiver<UpdateStatus> {
        self.status_tx.subscribe()
    }

    /// Check immediately, for example when authentication recovers after the
    /// process started offline.
    pub fn check_now(&self) {
        self.forced.store(true, Ordering::SeqCst);
        self.wake_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    /// Re-read the wall clock now (window activation, system wake): checks
    /// only if one is due, so frequent pokes cost nothing.
    pub fn poke(&self) {
        self.wake_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    /// How this engine replaces itself in place (see [`HandoffHook`]). Set
    /// once, before [`Self::restart_when_superseded`].
    pub fn set_handoff(&self, hook: HandoffHook) {
        let _ = self.handoff.set(hook);
    }

    /// Never hand off to `version` (a live handoff to it just failed and was
    /// rolled back). A newer version, or an engine restart, is tried normally;
    /// the restart fallback for the installed service still applies.
    pub fn skip_handoff_for(&self, version: &str) {
        let _ = self.handoff_blocked.set(version.to_owned());
    }

    /// Called by `zeron headless`: when something installs a newer binary
    /// (the desktop app, `zeron update`, this checker itself), move onto it.
    /// With a handoff hook that is a live handoff, retried each minute while
    /// work in flight cannot be carried across, and it works for any engine
    /// that can exec the installed binary. Without one, only the installed
    /// engine service restarts into it at a quiet moment; a hand-started
    /// `zeron headless` is left alone — restarting the service unit would
    /// start a second engine beside it.
    pub fn restart_when_superseded(&self) {
        if !watches_for_newer_install(running_as_installed_service(), self.handoff.get().is_some())
        {
            return;
        }
        self.service.store(true, Ordering::SeqCst);
        self.poke();
    }

    /// One user-requested check, awaited: publishes the result like a
    /// scheduled check and returns it.
    pub async fn check(&self) -> anyhow::Result<UpdateStatus> {
        match fetch_latest(&self.edge_url).await {
            Ok(manifest) => {
                self.publish(manifest);
                Ok(self.status_tx.borrow().clone())
            }
            Err(err) => {
                self.status_tx
                    .send_modify(|s| s.error = Some(format!("{err:#}")));
                Err(err)
            }
        }
    }

    fn quiescent_now(&self) -> bool {
        match &*self.role {
            Role::Engine { quiescent } => quiescent.as_ref().is_none_or(|check| check()),
            Role::Desktop => true,
        }
    }

    async fn check_loop(&self, mut wakes: watch::Receiver<u64>, initial_delay: Duration) {
        let mut shutdown = self.shutdown_tx.subscribe();
        // Shutdown must cut the loop at ANY await point — including mid
        // `check_once()` / `auto_apply_when_idle()` HTTP — so the whole body
        // races the flag rather than checking it between iterations.
        tokio::select! {
            _ = shutdown.wait_for(|stop| *stop) => {}
            _ = async {
                tokio::select! {
                    _ = tokio::time::sleep(initial_delay) => {}
                    _ = wakes.changed() => {}
                }
                let mut schedule = Schedule::default();
                let mut superseded = SupersededRestart::default();
                loop {
                    let forced = self.forced.swap(false, Ordering::SeqCst);
                    if forced || schedule.due(SystemTime::now()) {
                        let ok = self.check_once().await;
                        schedule.record(SystemTime::now(), ok);
                        if ok
                            && self.status_tx.borrow().update_available
                            && matches!(*self.role, Role::Engine { .. })
                            && engine_auto_update_enabled(self.handoff.get().is_some())
                            && let InstallKind::Managed { .. } = detect_install()
                        {
                            self.auto_apply_when_idle().await;
                        }
                    }
                    if self.service.load(Ordering::SeqCst) {
                        superseded
                            .poll(
                                || self.quiescent_now(),
                                self.handoff.get(),
                                self.handoff_blocked.get().map(String::as_str),
                            )
                            .await;
                    }
                    let wait = schedule.wait(SystemTime::now());
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = wakes.changed() => {}
                    }
                }
            } => {}
        }
    }

    /// Sessions must never die to an update: pre-stage the download now
    /// (harmless while busy), wait for a quiet window (no live runs, no open
    /// terminals), then apply — which re-fetches the manifest (so a long defer
    /// lands on whatever is newest) and reuses the staged dir, keeping the
    /// idle→restart gap to well under a second.
    async fn auto_apply_when_idle(&self) {
        if self.handoff.get().is_some() {
            // With a live handoff an update needs no quiet window: staging and
            // flipping the symlink disturb nothing, and the handoff that
            // follows (driven by the superseded watch `apply_inner` wakes)
            // carries running work across instead of waiting for it to end.
            match self.apply_inner(false).await {
                Ok(Some(version)) => {
                    tracing::info!(%version, "auto-update installed; handing the engine over")
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(error = %err, "auto-update failed"),
            }
            return;
        }
        if let InstallKind::Managed { app_root } = detect_install() {
            match fetch_latest(&self.edge_url).await {
                Ok(manifest) if version_newer(&manifest.version, current_version()) => {
                    if let Err(err) = stage_headless(&self.edge_url, &manifest, &app_root).await {
                        tracing::warn!(error = %err, "auto-update staging failed");
                        return;
                    }
                }
                Ok(_) => return,
                Err(err) => {
                    tracing::warn!(error = %err, "auto-update staging fetch failed");
                    return;
                }
            }
        }
        let mut deferred = false;
        loop {
            while !self.quiescent_now() {
                if !deferred {
                    deferred = true;
                    tracing::info!("auto-update deferred: sessions or terminals active");
                }
                tokio::time::sleep(IDLE_RECHECK).await;
            }
            // `apply_inner` fetches and stages again so a long defer lands on
            // the newest release. Re-check quiescence immediately before the
            // symlink swap: work can begin while that network/disk I/O runs.
            match self.apply_inner(true).await {
                Ok(Some(version)) => {
                    tracing::info!(%version, "auto-update applied; service restarting");
                    return;
                }
                Ok(None) => {
                    if !deferred {
                        deferred = true;
                        tracing::info!(
                            "auto-update deferred: activity began while staging the update"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "auto-update failed");
                    return;
                }
            }
        }
    }

    fn publish(&self, manifest: Manifest) {
        let status = UpdateStatus {
            current_version: current_version().to_string(),
            update_available: version_newer(&manifest.version, current_version()),
            latest_version: Some(manifest.version),
            checked_at: Some(now_ms()),
            error: None,
        };
        if status.update_available {
            tracing::info!(
                latest = status.latest_version.as_deref().unwrap_or(""),
                current = %status.current_version,
                "update available"
            );
        }
        self.status_tx.send_replace(status);
    }

    /// One check; returns false on fetch failure (retry sooner).
    async fn check_once(&self) -> bool {
        match fetch_latest(&self.edge_url).await {
            Ok(manifest) => {
                self.publish(manifest);
                true
            }
            Err(err) => {
                tracing::debug!(error = %err, "update check failed");
                self.status_tx
                    .send_modify(|s| s.error = Some(format!("{err:#}")));
                false
            }
        }
    }

    /// Stage + apply the newest release on THIS device (managed installs only),
    /// then restart the service after a short delay so the caller's RPC reply
    /// flushes before systemd/launchd kills this process.
    pub async fn apply(&self) -> anyhow::Result<String> {
        self.apply_inner(false)
            .await?
            .ok_or_else(|| anyhow::anyhow!("update became busy before apply"))
    }

    /// `require_quiescent` is used by automatic updates. It deliberately checks
    /// after all network and staging I/O and directly before the destructive
    /// swap/restart boundary; `None` tells the caller to wait and try again.
    async fn apply_inner(&self, require_quiescent: bool) -> anyhow::Result<Option<String>> {
        let InstallKind::Managed { app_root } = detect_install() else {
            bail!(
                "this install is not update-managed — the desktop app updates from its UI; \
                 source builds update via git"
            );
        };
        let manifest = fetch_latest(&self.edge_url).await?;
        if !version_newer(&manifest.version, current_version()) {
            bail!("already up to date ({})", current_version());
        }
        stage_headless(&self.edge_url, &manifest, &app_root).await?;
        if require_quiescent && !self.quiescent_now() {
            return Ok(None);
        }
        apply_headless(&app_root, &manifest.version)?;
        if self.handoff.get().is_some() {
            // Installed. The superseded watch, woken now, hands this engine
            // over in place — no service restart, nothing running is touched.
            self.service.store(true, Ordering::SeqCst);
            self.poke();
        } else {
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(800)).await;
                if let Err(err) = restart_service() {
                    tracing::warn!(error = %err, "service restart failed — restart the engine to finish the update");
                }
            });
        }
        Ok(Some(manifest.version))
    }
}

/// Wall-clock check deadlines with failure backoff.
#[derive(Debug, Default)]
struct Schedule {
    /// `None` until the first check: due immediately.
    next_due: Option<SystemTime>,
    failures: usize,
}

impl Schedule {
    fn due(&self, now: SystemTime) -> bool {
        match self.next_due {
            None => true,
            Some(due) => match due.duration_since(now) {
                Err(_) => true,
                // A deadline further out than any interval means the clock
                // jumped backwards; don't wait for it to catch up.
                Ok(remaining) => remaining.is_zero() || remaining > CHECK_INTERVAL,
            },
        }
    }

    fn record(&mut self, now: SystemTime, ok: bool) {
        let delay = if ok {
            self.failures = 0;
            CHECK_INTERVAL
        } else {
            let delay = CHECK_RETRY[self.failures.min(CHECK_RETRY.len() - 1)];
            self.failures = self.failures.saturating_add(1);
            delay
        };
        self.next_due = Some(now + delay);
    }

    /// Monotonic sleep until the next look at the clock.
    fn wait(&self, now: SystemTime) -> Duration {
        self.next_due
            .and_then(|due| due.duration_since(now).ok())
            .unwrap_or(Duration::ZERO)
            .clamp(Duration::from_secs(1), SCHEDULE_TICK)
    }
}

/// Moves a running engine onto a newer binary that is already installed (the
/// desktop app swapped the bundle, `zeron update` flipped the symlink, the
/// checker itself applied it).
///
/// With a handoff hook it hands the engine over in place; a `Busy` answer (a
/// turn is running, a sign-in is open, …) is simply retried on the next poll,
/// because work in flight can become carryable, or finish, without anyone
/// restarting anything. A handoff that FAILS backs off, and only then does
/// the older behaviour apply: the installed engine *service* restarts at a
/// quiet moment, at most once per installed version so a broken service
/// manager cannot turn into a restart loop. A hand-started engine is never
/// restarted (that would start a second one beside it).
#[derive(Debug, Default)]
struct SupersededRestart {
    attempted: Option<String>,
    handoff_deferred_logged: bool,
    restart_deferred_logged: bool,
    /// The installed version the last failed handoff was for, and when to try
    /// again. A NEWER install is a different build: it is tried at once.
    handoff_failed_for: Option<String>,
    handoff_retry_at: Option<std::time::Instant>,
}

/// After a failed handoff, wait this long before trying again.
const HANDOFF_RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10 * 60);

impl SupersededRestart {
    async fn poll(
        &mut self,
        quiescent: impl Fn() -> bool,
        handoff: Option<&HandoffHook>,
        handoff_blocked: Option<&str>,
    ) {
        let kind = detect_install();
        let Some(installed) = installed_version(&kind) else {
            return;
        };
        let exe = stable_exe_for(&kind, &std::env::current_exe().unwrap_or_default());
        self.poll_installed(
            &installed,
            current_version(),
            &exe,
            quiescent,
            handoff,
            handoff_blocked,
            running_as_installed_service(),
            restart_service,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn poll_installed(
        &mut self,
        installed: &str,
        running: &str,
        exe: &Path,
        quiescent: impl Fn() -> bool,
        handoff: Option<&HandoffHook>,
        handoff_blocked: Option<&str>,
        may_restart: bool,
        restart: impl FnOnce() -> anyhow::Result<()>,
    ) {
        if !version_newer(installed, running) {
            return;
        }
        if self.handoff_failed_for.as_deref() != Some(installed) {
            self.handoff_failed_for = None;
            self.handoff_retry_at = None;
        }
        if let Some(hook) = handoff
            && handoff_blocked != Some(installed)
            && self
                .handoff_retry_at
                .is_none_or(|at| std::time::Instant::now() >= at)
        {
            match hook(exe.to_path_buf()).await {
                HandoffOutcome::Busy(reason) => {
                    if !std::mem::replace(&mut self.handoff_deferred_logged, true) {
                        tracing::info!(installed, running, %reason, "newer version installed; live handoff deferred");
                    }
                    return;
                }
                HandoffOutcome::Failed(reason) => {
                    if self.handoff_failed_for.as_deref() != Some(installed) {
                        tracing::warn!(installed, running, %reason, "live handoff failed; falling back to a restart at a quiet moment");
                        self.handoff_failed_for = Some(installed.to_owned());
                    }
                    self.handoff_retry_at =
                        Some(std::time::Instant::now() + HANDOFF_RETRY_AFTER_FAILURE);
                }
            }
        }
        if !may_restart || self.attempted.as_deref() == Some(installed) {
            return;
        }
        if !quiescent() {
            if !self.restart_deferred_logged {
                self.restart_deferred_logged = true;
                tracing::info!(
                    installed,
                    running,
                    "newer version installed; restart deferred: sessions or terminals active"
                );
            }
            return;
        }
        self.attempted = Some(installed.to_owned());
        tracing::info!(
            installed,
            running,
            "newer version installed; restarting service"
        );
        if let Err(err) = restart() {
            tracing::warn!(error = %err, "service restart failed — restart the engine to finish the update");
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stalled_update_headers_and_body_time_out_but_progressing_body_survives() {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for stall_body in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                socket.read(&mut request).await.unwrap();
                if stall_body {
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                        .await
                        .unwrap();
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            });
            let client =
                http_client_with_timeouts(Duration::from_millis(200), Duration::from_millis(100))
                    .unwrap();
            let request = async {
                client
                    .get(format!("http://{address}"))
                    .send()
                    .await?
                    .bytes()
                    .await
            };
            let error = tokio::time::timeout(Duration::from_secs(1), request)
                .await
                .expect("bounded read")
                .unwrap_err();
            assert!(error.is_timeout());
            server.abort();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n")
                .await
                .unwrap();
            for _ in 0..10 {
                socket.write_all(b"x").await.unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        });
        let client =
            http_client_with_timeouts(Duration::from_secs(1), Duration::from_millis(200)).unwrap();
        assert_eq!(
            client
                .get(format!("http://{address}"))
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .len(),
            10
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn release_metadata_has_size_and_total_time_limits() {
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Reject an advertised oversized body before buffering it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\n01234567890")
                .await
                .unwrap();
        });
        let client =
            http_client_with_timeouts(Duration::from_secs(1), Duration::from_secs(1)).unwrap();
        let error = fetch_release_metadata_with_limits(
            &client,
            &format!("http://{address}"),
            Duration::from_secs(1),
            10,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 10 bytes"));
        server.await.unwrap();

        // Enforce the same cap when Content-Length is absent.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n01234567890")
                .await
                .unwrap();
        });
        let error = fetch_release_metadata_with_limits(
            &client,
            &format!("http://{address}"),
            Duration::from_secs(1),
            10,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 10 bytes"));
        server.await.unwrap();

        // Progressing bytes stay below the client's inactivity timeout but
        // must still obey the metadata operation's total deadline.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n")
                .await
                .unwrap();
            for _ in 0..10 {
                if socket.write_all(b"x").await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        });
        let error = fetch_release_metadata_with_limits(
            &client,
            &format!("http://{address}"),
            Duration::from_millis(80),
            100,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("metadata deadline"));
        server.abort();
    }

    #[tokio::test]
    async fn stalled_update_tls_handshake_has_a_connect_deadline() {
        use std::time::Duration;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let client =
            http_client_with_timeouts(Duration::from_millis(100), Duration::from_secs(30)).unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            client.get(format!("https://{address}")).send(),
        )
        .await
        .expect("bounded TLS handshake")
        .unwrap_err();
        assert!(error.is_timeout());
        server.abort();
    }

    #[test]
    fn update_feed_override_requires_an_https_base_url() {
        assert_eq!(
            validate_release_override(" https://example.com/releases/ ").unwrap(),
            "https://example.com/releases"
        );
        for url in [
            "http://example.com/releases",
            "file:///tmp/update",
            "https://user:password@example.com",
            "https://example.com?feed=x",
            "https://example.com/#fragment",
        ] {
            assert!(validate_release_override(url).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn version_compare() {
        assert!(version_newer("0.1.1", "0.1.0"));
        assert!(version_newer("0.2.0", "0.1.9"));
        assert!(version_newer("0.1.10", "0.1.9"));
        assert!(version_newer("v0.1.1", "0.1.0"));
        assert!(version_newer("0.1.0.1", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.1.1"));
        // Garbage never counts as newer.
        assert!(!version_newer("", "0.1.0"));
        assert!(!version_newer("nightly", "0.1.0"));
    }

    #[test]
    fn install_kind_detection() {
        assert_eq!(
            detect_install_from_for_os(
                Path::new("/home/u/.zeron/app/0.1.1/zeron"),
                Some(Path::new("/home/u")),
                "linux",
            ),
            InstallKind::Managed {
                app_root: PathBuf::from("/home/u/.zeron/app")
            }
        );
        assert_eq!(
            detect_install_from_for_os(
                Path::new("/Applications/Zeron.app/Contents/MacOS/zeron"),
                Some(Path::new("/Users/u")),
                "macos",
            ),
            InstallKind::MacApp {
                bundle: PathBuf::from("/Applications/Zeron.app")
            }
        );
        // A path merely containing `.app` without the bundle layout is not a bundle.
        assert_eq!(
            detect_install_from_for_os(Path::new("/tmp/foo.app/zeron"), None, "macos"),
            InstallKind::Unmanaged
        );
        assert_eq!(
            detect_install_from_for_os(
                Path::new("/src/target/release/zeron"),
                Some(Path::new("/home/u")),
                "linux",
            ),
            InstallKind::Unmanaged
        );
    }

    #[test]
    fn artifact_names_match_packaging() {
        let (os, arch) = platform_key();
        assert!(headless_artifact("0.2.0").starts_with("zeron-0.2.0-"));
        assert_eq!(
            headless_artifact("0.2.0"),
            format!("zeron-0.2.0-{os}-{arch}.tar.gz")
        );
        assert!(mac_app_artifact("0.2.0").ends_with("-app.tar.gz"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_is_not_misclassified_as_linux() {
        assert_eq!(platform_key().0, "windows");
    }

    #[cfg(windows)]
    #[test]
    fn windows_install_is_always_unmanaged() {
        assert_eq!(
            detect_install_from(
                Path::new(r"C:\Users\u\.zeron\app\0.2.0\zeron.exe"),
                Some(Path::new(r"C:\Users\u")),
            ),
            InstallKind::Unmanaged
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_rejects_platform_specific_updates_before_side_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            version: "9.9.9".into(),
            files: BTreeMap::new(),
        };
        let app_root = tmp.path().join("app");
        let data_dir = tmp.path().join("data");

        let managed_err = stage_headless("http://127.0.0.1:1", &manifest, &app_root)
            .await
            .unwrap_err();
        assert!(managed_err.to_string().contains("not supported on windows"));
        assert!(!app_root.exists(), "managed staging must not touch disk");

        let mac_err = stage_mac_app("http://127.0.0.1:1", &manifest, &data_dir)
            .await
            .unwrap_err();
        assert!(mac_err.to_string().contains("not supported on windows"));
        assert!(!data_dir.exists(), "macOS staging must not touch disk");

        assert!(
            apply_headless(&app_root, &manifest.version)
                .unwrap_err()
                .to_string()
                .contains("not supported on windows")
        );
        assert!(
            apply_mac_app(&data_dir.join("Zeron.app"), &data_dir.join("Installed.app"))
                .unwrap_err()
                .to_string()
                .contains("not supported on windows")
        );
        assert!(
            restart_service()
                .unwrap_err()
                .to_string()
                .contains("not supported on windows")
        );
    }

    #[test]
    fn manifest_parses_with_and_without_files() {
        let full: Manifest = serde_json::from_str(
            r#"{"version":"0.1.1","files":{"zeron-0.1.1-linux-x86_64.tar.gz":{"sha256":"abc"}}}"#,
        )
        .unwrap();
        assert_eq!(full.version, "0.1.1");
        assert_eq!(
            full.files["zeron-0.1.1-linux-x86_64.tar.gz"]
                .sha256
                .as_deref(),
            Some("abc")
        );
        let bare: Manifest = serde_json::from_str(r#"{"version":"0.1.1"}"#).unwrap();
        assert!(bare.files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn headless_symlink_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("app");
        for ver in ["0.1.0", "0.1.1"] {
            std::fs::create_dir_all(app_root.join(ver)).unwrap();
            std::fs::write(app_root.join(ver).join("zeron"), ver).unwrap();
        }
        apply_headless(&app_root, "0.1.0").unwrap();
        assert_eq!(
            std::fs::read_link(app_root.join("current")).unwrap(),
            app_root.join("0.1.0")
        );
        // Swap over an existing symlink.
        apply_headless(&app_root, "0.1.1").unwrap();
        assert_eq!(
            std::fs::read_link(app_root.join("current")).unwrap(),
            app_root.join("0.1.1")
        );
        // Unstaged version refuses.
        assert!(apply_headless(&app_root, "0.2.0").is_err());
    }
    #[test]
    fn schedule_is_wall_clock_with_failure_backoff() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut schedule = Schedule::default();
        assert!(schedule.due(start), "the first check is due immediately");

        schedule.record(start, true);
        assert!(!schedule.due(start + Duration::from_secs(59 * 60)));
        // A machine that slept through the deadline is due the moment it
        // looks at the clock again — no monotonic remainder to wait out.
        assert!(schedule.due(start + Duration::from_secs(9 * 60 * 60)));
        assert_eq!(schedule.wait(start), SCHEDULE_TICK);
        // A clock that jumped backwards does not postpone checks.
        assert!(schedule.due(start - Duration::from_secs(24 * 60 * 60)));

        let mut backoff = Vec::new();
        for _ in 0..6 {
            schedule.record(start, false);
            backoff.push(schedule.next_due.unwrap().duration_since(start).unwrap());
        }
        assert_eq!(
            backoff,
            [
                CHECK_RETRY[0],
                CHECK_RETRY[1],
                CHECK_RETRY[2],
                CHECK_RETRY[3],
                CHECK_RETRY[3],
                CHECK_RETRY[3]
            ]
        );
        schedule.record(start, true);
        assert_eq!(schedule.failures, 0);
    }

    /// Drive `poll_installed` with the given knobs, counting restarts and hook calls.
    struct Rig {
        restarts: std::cell::Cell<u32>,
        hook_calls: Arc<std::sync::atomic::AtomicU32>,
        seen_exe: Arc<std::sync::Mutex<Vec<PathBuf>>>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                restarts: std::cell::Cell::new(0),
                hook_calls: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                seen_exe: Arc::default(),
            }
        }

        fn hook(&self, outcome: HandoffOutcome) -> HandoffHook {
            let calls = self.hook_calls.clone();
            let seen = self.seen_exe.clone();
            Arc::new(move |exe| {
                calls.fetch_add(1, Ordering::SeqCst);
                seen.lock().unwrap().push(exe);
                let outcome = outcome.clone();
                Box::pin(async move { outcome })
            })
        }

        fn calls(&self) -> u32 {
            self.hook_calls.load(Ordering::SeqCst)
        }

        async fn poll(
            &self,
            state: &mut SupersededRestart,
            installed: &str,
            quiet: bool,
            hook: Option<&HandoffHook>,
            may_restart: bool,
        ) {
            self.poll_blocked(state, installed, quiet, hook, None, may_restart)
                .await;
        }

        async fn poll_blocked(
            &self,
            state: &mut SupersededRestart,
            installed: &str,
            quiet: bool,
            hook: Option<&HandoffHook>,
            blocked: Option<&str>,
            may_restart: bool,
        ) {
            state
                .poll_installed(
                    installed,
                    "0.2.0",
                    Path::new("/app/current/zeron"),
                    || quiet,
                    hook,
                    blocked,
                    may_restart,
                    || {
                        self.restarts.set(self.restarts.get() + 1);
                        Ok(())
                    },
                )
                .await;
        }
    }

    #[tokio::test]
    async fn without_a_hook_the_installed_service_restarts_once_when_quiet() {
        let rig = Rig::new();
        let mut state = SupersededRestart::default();
        // Same or older installed versions never restart.
        rig.poll(&mut state, "0.2.0", true, None, true).await;
        rig.poll(&mut state, "0.1.9", true, None, true).await;
        assert_eq!(rig.restarts.get(), 0);
        // Busy: deferred, not attempted.
        rig.poll(&mut state, "0.2.1", false, None, true).await;
        assert_eq!(rig.restarts.get(), 0);
        // Quiet: exactly one attempt per installed version.
        rig.poll(&mut state, "0.2.1", true, None, true).await;
        rig.poll(&mut state, "0.2.1", true, None, true).await;
        assert_eq!(rig.restarts.get(), 1);
        rig.poll(&mut state, "0.2.2", true, None, true).await;
        assert_eq!(rig.restarts.get(), 2);
    }

    #[tokio::test]
    async fn a_hand_started_engine_is_never_restarted() {
        let rig = Rig::new();
        let mut state = SupersededRestart::default();
        rig.poll(&mut state, "0.2.1", true, None, false).await;
        assert_eq!(
            rig.restarts.get(),
            0,
            "a restart would start a second engine"
        );
    }

    #[tokio::test]
    async fn a_busy_handoff_is_retried_every_poll_and_never_becomes_a_restart() {
        let rig = Rig::new();
        let hook = rig.hook(HandoffOutcome::Busy("an agent turn is running".into()));
        let mut state = SupersededRestart::default();
        for _ in 0..5 {
            // Even at a "quiet" moment by the old definition: a restart would
            // kill the very work the handoff is waiting to carry across.
            rig.poll(&mut state, "0.2.1", true, Some(&hook), true).await;
        }
        assert_eq!(rig.calls(), 5, "retried on every poll");
        assert_eq!(rig.restarts.get(), 0);
        assert!(
            rig.seen_exe
                .lock()
                .unwrap()
                .iter()
                .all(|exe| exe == Path::new("/app/current/zeron")),
            "the hook is told which binary to exec"
        );
    }

    #[tokio::test]
    async fn a_failed_handoff_backs_off_and_falls_back_to_a_restart_only_when_quiet() {
        let rig = Rig::new();
        let hook = rig.hook(HandoffOutcome::Failed("preflight failed".into()));
        let mut state = SupersededRestart::default();
        // Not quiet: the fallback defers (nothing is restarted under running work).
        rig.poll(&mut state, "0.2.1", false, Some(&hook), true)
            .await;
        assert_eq!((rig.calls(), rig.restarts.get()), (1, 0));
        // Quiet: one restart. The hook is not hammered again inside the backoff.
        rig.poll(&mut state, "0.2.1", true, Some(&hook), true).await;
        rig.poll(&mut state, "0.2.1", true, Some(&hook), true).await;
        assert_eq!(rig.calls(), 1, "the failure backs off");
        assert_eq!(
            rig.restarts.get(),
            1,
            "at most one restart per installed version"
        );
    }

    #[tokio::test]
    async fn a_failed_handoff_is_retried_after_the_backoff_and_a_newer_install_at_once() {
        let rig = Rig::new();
        let hook = rig.hook(HandoffOutcome::Failed("boom".into()));
        let mut state = SupersededRestart::default();
        rig.poll(&mut state, "0.2.1", false, Some(&hook), false)
            .await;
        rig.poll(&mut state, "0.2.1", false, Some(&hook), false)
            .await;
        assert_eq!(rig.calls(), 1, "inside the backoff the hook is left alone");
        // A newer build appearing mid-backoff is a different build: tried now.
        rig.poll(&mut state, "0.2.2", false, Some(&hook), false)
            .await;
        assert_eq!(rig.calls(), 2);
        // The backoff itself expires.
        state.handoff_retry_at = Some(std::time::Instant::now() - Duration::from_secs(1));
        rig.poll(&mut state, "0.2.2", false, Some(&hook), false)
            .await;
        assert_eq!(rig.calls(), 3);
    }

    #[tokio::test]
    async fn deferring_a_handoff_does_not_silence_the_restart_deferral_log() {
        let rig = Rig::new();
        let busy = rig.hook(HandoffOutcome::Busy("a turn".into()));
        let mut state = SupersededRestart::default();
        rig.poll(&mut state, "0.2.1", false, Some(&busy), true)
            .await;
        assert!(state.handoff_deferred_logged);
        assert!(!state.restart_deferred_logged);
    }

    #[tokio::test]
    async fn a_failed_handoff_never_restarts_an_engine_that_is_not_the_service() {
        let rig = Rig::new();
        let hook = rig.hook(HandoffOutcome::Failed("exec failed".into()));
        let mut state = SupersededRestart::default();
        rig.poll(&mut state, "0.2.1", true, Some(&hook), false)
            .await;
        assert_eq!((rig.calls(), rig.restarts.get()), (1, 0));
    }

    #[tokio::test]
    async fn a_version_that_failed_to_adopt_is_never_handed_off_to_again() {
        let rig = Rig::new();
        let hook = rig.hook(HandoffOutcome::Busy("never reached".into()));
        let mut state = SupersededRestart::default();
        // A rollback brought us back from 0.2.1: the same install is still
        // newer than we are, but handing off to it would loop.
        for _ in 0..3 {
            rig.poll_blocked(&mut state, "0.2.1", true, Some(&hook), Some("0.2.1"), false)
                .await;
        }
        assert_eq!(rig.calls(), 0, "the failed version is not tried again");
        // A newer install is a different build and is handed off to normally.
        rig.poll_blocked(&mut state, "0.2.2", true, Some(&hook), Some("0.2.1"), false)
            .await;
        assert_eq!(rig.calls(), 1);
    }

    #[tokio::test]
    async fn a_blocked_version_still_gets_the_quiet_service_restart() {
        let rig = Rig::new();
        let hook = rig.hook(HandoffOutcome::Busy("never reached".into()));
        let mut state = SupersededRestart::default();
        rig.poll_blocked(&mut state, "0.2.1", false, Some(&hook), Some("0.2.1"), true)
            .await;
        assert_eq!(rig.restarts.get(), 0, "never under running work");
        rig.poll_blocked(&mut state, "0.2.1", true, Some(&hook), Some("0.2.1"), true)
            .await;
        assert_eq!((rig.calls(), rig.restarts.get()), (0, 1));
    }

    #[test]
    fn engines_apply_updates_by_default_only_when_they_can_hand_off() {
        // Unset: on for an engine with a live handoff, off for one without
        // (the desktop app's in-process engine must not install and restart).
        assert!(engine_auto_update_from(None, None, true));
        assert!(!engine_auto_update_from(None, None, false));
        // An explicit opt-in still works for any engine (the old behaviour).
        for on in ["1", "true", "yes", " YES "] {
            assert!(engine_auto_update_from(Some(on), None, false), "{on}");
            assert!(engine_auto_update_from(Some(on), None, true), "{on}");
        }
        for off in ["0", "false", "no", " NO ", "False"] {
            assert!(!engine_auto_update_from(Some(off), None, true), "{off}");
        }
        // Unrecognised values are not an opt-in.
        assert!(!engine_auto_update_from(Some("maybe"), None, false));
        // The desktop app drives updates for the host it started.
        assert!(!engine_auto_update_from(None, Some("app"), true));
        assert!(!engine_auto_update_from(Some("1"), Some("app"), true));
        assert!(engine_auto_update_from(None, Some("other"), true));
    }

    #[test]
    fn the_stable_exe_is_the_current_symlink_or_the_bundle_binary() {
        let exe = Path::new("/home/u/.zeron/app/0.2.0/zeron");
        assert_eq!(
            stable_exe_for(
                &InstallKind::Managed {
                    app_root: PathBuf::from("/home/u/.zeron/app")
                },
                exe
            ),
            Path::new("/home/u/.zeron/app/current/zeron")
        );
        assert_eq!(
            stable_exe_for(
                &InstallKind::MacApp {
                    bundle: PathBuf::from("/Applications/Zeron.app")
                },
                exe
            ),
            Path::new("/Applications/Zeron.app/Contents/MacOS/zeron")
        );
        assert_eq!(stable_exe_for(&InstallKind::Unmanaged, exe), exe);
    }

    #[test]
    fn a_handoff_hook_makes_any_engine_watch_for_a_newer_install() {
        assert!(
            watches_for_newer_install(true, false),
            "the service always did"
        );
        assert!(
            watches_for_newer_install(false, true),
            "a hand-started engine with a hook"
        );
        assert!(
            !watches_for_newer_install(false, false),
            "left alone, as before"
        );
    }

    #[test]
    fn systemd_service_cgroup_is_recognized() {
        assert!(in_zeron_service_cgroup(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/zeron.service\n"
        ));
        assert!(in_zeron_service_cgroup(
            "12:pids:/user.slice/user@1000.service/zeron.service\n1:name=systemd:/x\n"
        ));
        assert!(!in_zeron_service_cgroup(
            "0::/user.slice/user-1000.slice/session-3.scope\n"
        ));
        assert!(!in_zeron_service_cgroup(
            "0::/user.slice/user@1000.service/app.slice/zeron.service.d\n"
        ));
    }

    #[test]
    fn bundle_version_reads_the_packaged_plist() {
        let template = include_str!("../../../dist/macos/Info.plist");
        let plist = template.replace("__VERSION__", "0.3.1");
        assert_eq!(bundle_short_version(&plist).as_deref(), Some("0.3.1"));
        assert_eq!(bundle_short_version("<plist></plist>"), None);
    }

    #[cfg(unix)]
    #[test]
    fn installed_version_follows_the_current_symlink_and_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("app");
        std::fs::create_dir_all(app_root.join("0.4.0")).unwrap();
        std::fs::write(app_root.join("0.4.0").join("zeron"), "").unwrap();
        let managed = InstallKind::Managed {
            app_root: app_root.clone(),
        };
        assert_eq!(installed_version(&managed), None, "no current link yet");
        apply_headless(&app_root, "0.4.0").unwrap();
        assert_eq!(installed_version(&managed).as_deref(), Some("0.4.0"));

        let bundle = tmp.path().join("Zeron.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            include_str!("../../../dist/macos/Info.plist").replace("__VERSION__", "0.4.1"),
        )
        .unwrap();
        assert_eq!(
            installed_version(&InstallKind::MacApp { bundle }).as_deref(),
            Some("0.4.1")
        );
        assert_eq!(installed_version(&InstallKind::Unmanaged), None);
    }

    #[test]
    fn mac_bundles_that_cannot_replace_themselves_are_explained() {
        let writable = |_: &Path| true;
        let read_only = |_: &Path| false;
        assert_eq!(
            mac_bundle_blocker(Path::new("/Applications/Zeron.app"), writable),
            None
        );
        assert_eq!(
            mac_bundle_blocker(
                Path::new("/private/var/folders/x/T/AppTranslocation/ABC/d/Zeron.app"),
                writable
            ),
            Some(UpdateBlocker::Translocated)
        );
        assert_eq!(
            mac_bundle_blocker(Path::new("/Volumes/Zeron/Zeron.app"), read_only),
            Some(UpdateBlocker::DiskImage)
        );
        // An external drive the user can write to updates in place.
        assert_eq!(
            mac_bundle_blocker(Path::new("/Volumes/Work/Zeron.app"), writable),
            None
        );
        assert_eq!(
            mac_bundle_blocker(Path::new("/Applications/Zeron.app"), read_only),
            Some(UpdateBlocker::NotWritable(PathBuf::from("/Applications")))
        );
    }

    #[test]
    fn writability_probe_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(dir_writable(tmp.path()));
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
        assert!(!dir_writable(&tmp.path().join("missing")));
    }

    /// Serves `body` for every request until aborted.
    async fn serve_forever(body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut request = [0; 4096];
                    let _ = socket.read(&mut request).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        (base, server)
    }

    #[tokio::test]
    async fn desktop_checker_reports_a_newer_release_on_demand() {
        let (base, server) = serve_forever(br#"{"version":"999.0.0","files":{}}"#.to_vec()).await;
        let updater = Updater::spawn_desktop(base);
        let status = updater.check().await.unwrap();
        assert!(status.update_available);
        assert_eq!(status.latest_version.as_deref(), Some("999.0.0"));
        assert_eq!(status.current_version, current_version());
        assert!(status.checked_at.is_some());
        assert_eq!(*updater.watch().borrow(), status);
        updater.shutdown().await;
        server.abort();
    }

    /// A headless tarball whose `zeron` reports `reported` from `--version`.
    #[cfg(unix)]
    fn fake_headless_tarball(dir: &Path, reported: &str) -> Vec<u8> {
        use std::os::unix::fs::PermissionsExt as _;
        let root = dir.join("zeron-pkg");
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("zeron");
        std::fs::write(&binary, format!("#!/bin/sh\necho \"zeron {reported}\"\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tarball = dir.join("pkg.tar.gz");
        run(
            "tar",
            &[
                "-czf",
                &tarball.to_string_lossy(),
                "-C",
                &dir.to_string_lossy(),
                "zeron-pkg",
            ],
        )
        .unwrap();
        std::fs::read(tarball).unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn headless_staging_rejects_a_binary_that_is_not_the_promised_release() {
        let tmp = tempfile::tempdir().unwrap();
        let app_root = tmp.path().join("app");
        std::fs::create_dir_all(&app_root).unwrap();
        let manifest = |bytes: &[u8]| Manifest {
            version: "9.9.9".into(),
            files: [(
                headless_artifact("9.9.9"),
                FileMeta {
                    sha256: Some(format!("{:x}", Sha256::digest(bytes))),
                },
            )]
            .into(),
        };

        let wrong = fake_headless_tarball(&tmp.path().join("wrong"), "1.0.0");
        let (base, server) = serve_forever(wrong.clone()).await;
        let error = stage_headless(&base, &manifest(&wrong), &app_root)
            .await
            .unwrap_err();
        server.abort();
        assert!(
            format!("{error:#}").contains("expected \"zeron 9.9.9\""),
            "unexpected error: {error:#}"
        );
        assert!(!app_root.join("9.9.9").exists());
        assert_eq!(std::fs::read_dir(&app_root).unwrap().count(), 0);

        let right = fake_headless_tarball(&tmp.path().join("right"), "9.9.9");
        let (base, server) = serve_forever(right.clone()).await;
        let staged = stage_headless(&base, &manifest(&right), &app_root)
            .await
            .unwrap();
        server.abort();
        assert_eq!(staged, app_root.join("9.9.9"));
        assert!(staged.join("zeron").is_file());
    }
}
