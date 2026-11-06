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
//!   tarball into a new versioned dir, then restart into a trial of it that
//!   moves the symlink only once it has proved it's ready ([`transaction`]).
//! - **MacApp** (running out of an app bundle): download the app tarball, swap the
//!   bundle directory, relaunch, driven by the UI; or, asked from another device,
//!   the engine's own trial of it ([`transaction`]).
//! - **WindowsPortable** (the Windows installer or portable zip — both carry
//!   `zeron-update.json`): swap the executable in place, the same two ways.
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

pub mod channel;
pub mod desktop;
pub mod transaction;
pub use channel::Channel;
pub use transaction::{UpdateReport, UpdateState};

/// The version compiled into this binary: the workspace's, or a beta's
/// (`ZERON_ENGINE_VERSION=0.2.137-beta.3` at build time). Betas of one release
/// share a workspace version, and a running engine only knows a bundle replaced
/// it by this being older than the bundle's.
pub const fn current_version() -> &'static str {
    match option_env!("ZERON_ENGINE_VERSION") {
        Some(version) => version,
        None => env!("CARGO_PKG_VERSION"),
    }
}

static APP_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The app's own version when it isn't this crate's: a beta interface built
/// apart from the workspace (`0.2.136-beta.1`). Set once, before the desktop
/// checker starts.
pub fn set_app_version(version: &str) {
    let _ = APP_VERSION.set(version.trim().to_owned());
}

/// The version the desktop app compares releases with: [`set_app_version`]'s,
/// or [`current_version`].
pub fn app_version() -> &'static str {
    APP_VERSION.get().map(String::as_str).unwrap_or(current_version())
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
    /// The engine inside the release's app, when it isn't `version`'s own (a
    /// beta app carries the stable engine).
    #[serde(default, rename = "engineVersion", skip_serializing_if = "Option::is_none")]
    pub engine_version: Option<String>,
    /// The oldest macOS the release's app runs on, when it's newer than the
    /// stable app's (beta: [`channel::BETA_MINIMUM_MACOS`]).
    #[serde(default, rename = "minimumMacOS", skip_serializing_if = "Option::is_none")]
    pub minimum_macos: Option<String>,
    /// The channel this manifest was fetched from: where its files are.
    #[serde(skip)]
    pub channel: Channel,
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

/// A release version: dotted numbers and, for a prerelease, its semver
/// identifiers (`0.2.136-beta.2` → `[0, 2, 136]`, `["beta", "2"]`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    core: Vec<u64>,
    pre: Vec<PreId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PreId {
    // Numeric identifiers sort below alphanumeric ones (semver 11.4.3).
    Num(u64),
    Text(String),
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                // A release outranks its prereleases.
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Identifier by identifier; a shorter prefix is lower.
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Dotted numbers with an optional leading `v` and an optional `-prerelease`
/// (`0.2.136-beta.1`). Build metadata (`+…`) and anything else is rejected.
fn parse_version(v: &str) -> Option<Version> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (v, None),
    };
    let core: Vec<u64> = core
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    if core.is_empty() {
        return None;
    }
    let pre = match pre {
        None => Vec::new(),
        Some(pre) => pre
            .split('.')
            .map(|id| {
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    None
                } else if id.chars().all(|c| c.is_ascii_digit()) {
                    id.parse().ok().map(PreId::Num)
                } else {
                    Some(PreId::Text(id.to_ascii_lowercase()))
                }
            })
            .collect::<Option<_>>()?,
    };
    Some(Version { core, pre })
}

/// Strictly-newer compare (`0.1.10` > `0.1.9` > `0.1`), semver's for
/// prereleases: `0.2.135` < `0.2.136-beta.1` < `0.2.136-beta.2` < `0.2.136`.
/// Unparseable versions never count as newer — a garbage `latest.txt` must not
/// trigger an update loop.
pub fn version_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// What a release means for the build that's running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Nothing to install.
    Current,
    /// A newer release on the build's own channel (or a newer beta for a
    /// stable build that chose beta).
    Newer,
    /// The chosen channel's release replaces this build of the other
    /// channel even though it isn't newer: leaving beta for stable.
    Switch,
    /// Not installable here, and why (in words for the person).
    Held(String),
}

impl Offer {
    pub fn installs(&self) -> bool {
        matches!(self, Self::Newer | Self::Switch)
    }
}

/// What `manifest` (the `selected` channel's) offers a build of `running`
/// whose bundle carries the engine `running_engine`, on macOS `macos`.
///
/// - Its channel's release installs when newer.
/// - Choosing beta on a stable build installs a beta only when it's newer:
///   a stable that moved past the last beta waits for the next one.
/// - Choosing stable on a beta build installs stable even when older, unless
///   stable's engine is older than the one the beta brought (it would run on
///   data a newer engine wrote): then it waits until stable catches up.
/// - A release needing a newer macOS than this one is held.
pub fn offer(
    manifest: &Manifest,
    running: &str,
    selected: Channel,
    running_engine: Option<&str>,
    macos: Option<&str>,
) -> Offer {
    let latest = manifest.version.as_str();
    if parse_version(latest).is_none() || latest.trim() == running.trim() {
        return Offer::Current;
    }
    let leaving_beta = Channel::of_version(running) == Channel::Beta && selected == Channel::Stable;
    if !leaving_beta && !version_newer(latest, running) {
        return Offer::Current;
    }
    if let (Some(minimum), Some(system)) = (manifest.minimum_macos.as_deref(), macos)
        && !channel::macos_at_least(system, minimum)
    {
        return Offer::Held(format!(
            "Zeron {latest} needs macOS {minimum} or later. This Mac has macOS {system}."
        ));
    }
    if !leaving_beta {
        return Offer::Newer;
    }
    let target_engine = manifest.engine_version.as_deref().unwrap_or(latest);
    if let Some(engine) = running_engine
        && version_newer(engine, target_engine)
    {
        return Offer::Held(format!(
            "This beta's engine ({engine}) is newer than Zeron {latest}'s. You'll move to Stable once it catches up."
        ));
    }
    if version_newer(latest, running) {
        Offer::Newer
    } else {
        Offer::Switch
    }
}

/// Fetch the newest release metadata: `manifest.json`, falling back to
/// `latest.txt` (version only, no checksums) for pre-manifest releases.
pub async fn fetch_latest(edge_url: &str) -> anyhow::Result<Manifest> {
    fetch_latest_on(edge_url, channel::selected()).await
}

/// [`fetch_latest`] from `channel`'s feed.
pub async fn fetch_latest_on(edge_url: &str, channel: Channel) -> anyhow::Result<Manifest> {
    let mut manifest = fetch_latest_from(&channel.base(&release_base(edge_url)?)).await?;
    manifest.channel = channel;
    Ok(manifest)
}

async fn fetch_latest_from(base: &str) -> anyhow::Result<Manifest> {
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
        ..Manifest::default()
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
    // A test install's local feed, in a debug build only: release builds can't be
    // pointed anywhere but HTTPS.
    let local_test_feed =
        cfg!(debug_assertions) && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    anyhow::ensure!(
        local_test_feed || (url.scheme() == "https" && url.host_str().is_some()),
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
        // The engine is trying a new version out (asked from another device), and
        // going back from that needs the installed app to stay as it was.
        if channel::data_dir()
            .and_then(transaction::read)
            .is_some_and(|update| update.in_progress())
        {
            bail!("An update is already in progress on this device.");
        }
        match self {
            Self::MacApp { bundle } => {
                apply_mac_app(staged, bundle)?;
                // The swap copied the staged bundle; the cache is spent.
                if let Some(version_dir) = staged.parent()
                    && !process_runs_from(version_dir)
                {
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

/// How this copy of Zeron is installed, as `UpdateStatus` reports it. A client
/// shows what the device can do from this and `can_apply_remotely`, rather
/// than guessing from its platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallMethod {
    /// Installed by the Linux installer and kept running by the service manager.
    LinuxService,
    /// Installed on Linux and run by the desktop app.
    LinuxApp,
    /// Installed by the Linux installer, but started by hand: nothing would
    /// start it again after an update.
    StartedByHand,
    /// The macOS app.
    MacApp,
    /// The Windows installer or portable package.
    WindowsApp,
    /// Built from source or copied by hand.
    Unmanaged,
    /// A newer engine's method this client doesn't know.
    #[serde(other)]
    Unknown,
}

/// What a device can do about its own updates, for `UpdateStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAbility {
    pub install: InstallMethod,
    /// `ApplyUpdate` works from another device.
    pub can_apply_remotely: bool,
    /// Why it doesn't, in words for the person.
    pub cannot_apply_reason: Option<String>,
    /// What to run on the device instead, when there's a command for it.
    pub manual_command: Option<String>,
}

/// The one command that installs Zeron on Linux.
const LINUX_INSTALL_COMMAND: &str = "curl -fsSL https://zeron.sh/install.sh | sh";

impl UpdateAbility {
    fn refused(
        install: InstallMethod,
        reason: impl Into<String>,
        manual_command: Option<&str>,
    ) -> Self {
        Self {
            install,
            can_apply_remotely: false,
            cannot_apply_reason: Some(reason.into()),
            manual_command: manual_command.map(str::to_owned),
        }
    }

    /// `supervised`: something starts this engine again once it stops (see
    /// [`running_as_installed_service`]). `systemd`: that something is the
    /// service manager rather than the desktop app. `blocker`: why the
    /// installation can't replace itself right now.
    fn decide(
        kind: &InstallKind,
        os: &str,
        supervised: bool,
        systemd: bool,
        blocker: Option<UpdateBlocker>,
    ) -> Self {
        match kind {
            InstallKind::Managed { .. } if os != "linux" => Self::refused(
                InstallMethod::Unmanaged,
                "Zeron on a Mac updates through the Zeron app. Install the app on this Mac.",
                None,
            ),
            InstallKind::Managed { .. } if !supervised => Self::refused(
                InstallMethod::StartedByHand,
                "Zeron was started by hand on this device, so nothing would start it again \
                 after an update. Update it on the device, then start it again.",
                Some("zeron update"),
            ),
            InstallKind::Managed { .. } => {
                let install = if systemd {
                    InstallMethod::LinuxService
                } else {
                    InstallMethod::LinuxApp
                };
                match blocker {
                    Some(blocker) => Self::refused(install, blocker.to_string(), None),
                    None => Self {
                        install,
                        can_apply_remotely: true,
                        cannot_apply_reason: None,
                        manual_command: None,
                    },
                }
            }
            // The app's background engine can carry out an update itself. One
            // run any other way has nothing to bring it back.
            InstallKind::MacApp { .. } => Self::desktop(
                InstallMethod::MacApp,
                "This Zeron wasn't started by the app, so nothing would bring it back after an \
                 update. Update it from the Zeron app on that Mac.",
                supervised,
                blocker,
            ),
            #[cfg(windows)]
            InstallKind::WindowsPortable { .. } => Self::desktop(
                InstallMethod::WindowsApp,
                "This Zeron wasn't started by the app, so nothing would bring it back after an \
                 update. Update it from the Zeron app on that PC.",
                supervised,
                blocker,
            ),
            InstallKind::Unmanaged => Self::refused(
                InstallMethod::Unmanaged,
                "This copy of Zeron wasn't installed by the installer, so it can't replace itself.",
                (os == "linux").then_some(LINUX_INSTALL_COMMAND),
            ),
        }
    }

    fn desktop(
        install: InstallMethod,
        unsupervised: &str,
        supervised: bool,
        blocker: Option<UpdateBlocker>,
    ) -> Self {
        match blocker {
            _ if !supervised => Self::refused(install, unsupervised, None),
            Some(blocker) => Self::refused(install, blocker.to_string(), None),
            None => Self {
                install,
                can_apply_remotely: true,
                cannot_apply_reason: None,
                manual_command: None,
            },
        }
    }

    /// This process's: how it's installed, and whether it can update from afar.
    pub fn current() -> Self {
        let kind = detect_install();
        Self::decide(
            &kind,
            std::env::consts::OS,
            running_as_installed_service(),
            MANAGED_RESTART.get().is_none(),
            kind.desktop_update_blocker(),
        )
    }
}

/// Set on a new version on trial: the installation it's running in place of
/// (macOS: the app bundle; Windows: the folder).
pub const INSTALLED_AT_ENV: &str = "ZERON_INSTALLED_AT";

/// Where this process was installed from, resolved once: an update moves
/// the running image aside (Windows renames it to `zeron.exe.old`), and a
/// long-running engine must still recognise the installation it belongs to.
pub fn detect_install() -> InstallKind {
    static INSTALL: std::sync::OnceLock<InstallKind> = std::sync::OnceLock::new();
    INSTALL
        .get_or_init(|| {
            // A new version on trial runs from beside the installation it's
            // going to become.
            if let Some(installed) = std::env::var_os(INSTALLED_AT_ENV).map(PathBuf::from) {
                #[cfg(windows)]
                return InstallKind::WindowsPortable { directory: installed };
                #[cfg(not(windows))]
                return InstallKind::MacApp { bundle: installed };
            }
            let Ok(exe) = std::env::current_exe() else {
                return InstallKind::Unmanaged;
            };
            let home = std::env::var_os("HOME").map(PathBuf::from);
            detect_install_from(&exe, home.as_deref())
        })
        .clone()
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
        InstallKind::MacApp { bundle } => bundle_engine_version(bundle)?,
        #[cfg(windows)]
        InstallKind::WindowsPortable { directory } => windows::installed_version(directory)?,
        _ => return None,
    };
    parse_version(&version).map(|_| version)
}

/// The engine a macOS app bundle carries: its Info.plist's `ZeronEngineVersion`
/// (packaging writes it; a beta app's engine is stable's, older than the app),
/// else `CFBundleShortVersionString` (bundles from before the key, whose
/// engine was always the app's own version). An engine compares this with
/// its own to restart into a newer one; comparing the app's version would
/// restart a beta's engine forever.
pub fn bundle_engine_version(bundle: &Path) -> Option<String> {
    let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?;
    plist_string(&plist, "ZeronEngineVersion").or_else(|| plist_string(&plist, "CFBundleShortVersionString"))
}

/// The app's own version in a macOS bundle (a beta's isn't its engine's). A
/// beta says it in `ZeronVersion`: its short version has to stay numeric, so
/// that alone would read `0.2.137-beta.3` as the stable `0.2.137`.
pub fn bundle_app_version(bundle: &Path) -> Option<String> {
    let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).ok()?;
    plist_string(&plist, "ZeronVersion").or_else(|| plist_string(&plist, "CFBundleShortVersionString"))
}

/// A string value from an XML property list (the packaging template;
/// codesign leaves the format alone).
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let after_key = plist.split(&format!("<key>{key}</key>")).nth(1)?;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    let end = value.find("</string>")?;
    Some(value[..end].trim().to_owned()).filter(|v| !v.is_empty())
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
    // From the feed the manifest came from: a beta's files are beta's.
    let url = format!("{}/{file}", manifest.channel.base(&release_base(edge_url)?));
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

type ManagedRestart = Box<dyn Fn() -> anyhow::Result<()> + Send + Sync>;

static MANAGED_RESTART: std::sync::OnceLock<ManagedRestart> = std::sync::OnceLock::new();

/// This process is the desktop app's background engine, which no service
/// manager supervises: `restart` relaunches it (the replacement waits for
/// this one to let go of the data directory) and stops this one. With it set,
/// [`Updater::restart_when_superseded`] restarts into a newer installed
/// version once nothing is running, exactly as for an installed service.
pub fn set_managed_restart(restart: impl Fn() -> anyhow::Result<()> + Send + Sync + 'static) {
    let _ = MANAGED_RESTART.set(Box::new(restart));
}

type ManagedRestartReady = Box<dyn Fn() -> bool + Send + Sync>;
static MANAGED_RESTART_READY: std::sync::OnceLock<ManagedRestartReady> = std::sync::OnceLock::new();

/// When the background engine may restart into a newer installed version,
/// on top of nothing running (e.g. only while the app that starts it runs).
/// Checked on every pass, so a restart it holds back happens on a later one.
pub fn set_managed_restart_ready(ready: impl Fn() -> bool + Send + Sync + 'static) {
    let _ = MANAGED_RESTART_READY.set(Box::new(ready));
}

fn managed_restart_ready() -> bool {
    MANAGED_RESTART_READY.get().is_none_or(|ready| ready())
}

/// Whether this process is the engine service [`restart_service`] manages:
/// the desktop app's background engine ([`set_managed_restart`]); systemd
/// places a service in the `zeron.service` cgroup; launchd names the job in
/// `XPC_SERVICE_NAME`.
pub fn running_as_installed_service() -> bool {
    if MANAGED_RESTART.get().is_some() {
        return true;
    }
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
    in_service_cgroup(cgroups, &service_unit())
}

fn in_service_cgroup(cgroups: &str, unit: &str) -> bool {
    cgroups
        .lines()
        .filter_map(|line| line.rsplit(':').next())
        .any(|path| path.split('/').any(|part| part == unit))
}

/// The systemd unit this engine runs as. `ZERON_SERVICE_UNIT` names another, so
/// a test install beside a real one can never restart the real one.
pub fn service_unit() -> String {
    std::env::var("ZERON_SERVICE_UNIT")
        .ok()
        .filter(|unit| !unit.trim().is_empty())
        .unwrap_or_else(|| "zeron.service".to_owned())
}

/// Restart the installed engine service (the same units `zeron daemon` and the
/// curl|sh installer manage). Called after a symlink swap so the running daemon
/// picks up the new binary. Only queues the restart: the caller may be the
/// service itself, which must stay responsive to the stop signal that follows.
pub fn restart_service() -> anyhow::Result<()> {
    if let Some(restart) = MANAGED_RESTART.get() {
        return restart();
    }
    require_managed_update_platform()?;
    if cfg!(target_os = "macos") {
        let output = std::process::Command::new("id").arg("-u").output()?;
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        run(
            "launchctl",
            &["kickstart", "-k", &format!("gui/{uid}/sh.zeron.app")],
        )
    } else {
        ask_systemd_to_restart(&service_unit())
    }
}

/// Called from inside the unit, `systemctl` is in the cgroup being stopped and can
/// be killed by the very restart it asked for. Killed isn't refused: only an exit
/// status says it didn't work.
fn ask_systemd_to_restart(unit: &str) -> anyhow::Result<()> {
    let output = std::process::Command::new("systemctl")
        .args(["--user", "--no-block", "restart", unit])
        .output()
        .context("running systemctl")?;
    restart_outcome(output.status.code(), &output.stderr)
}

fn restart_outcome(code: Option<i32>, stderr: &[u8]) -> anyhow::Result<()> {
    match code {
        None | Some(0) => Ok(()),
        Some(code) => bail!(
            "systemctl restart failed ({code}): {}",
            String::from_utf8_lossy(stderr).trim()
        ),
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
        if entry.file_name() != keep && !process_runs_from(&entry.path()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether any process runs an executable from under `dir`. A bundle that's still
/// in use mustn't be deleted: with its image gone, AppleSystemPolicy rejects every
/// process it forks, thousands of times a minute (#1037). Can't tell: yes.
pub fn process_runs_from(dir: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let mut pids = vec![0 as libc::pid_t; 8192];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer is `bytes` long; the call writes at most that.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if count <= 0 {
            return true;
        }
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        pids.iter().take(count as usize).filter(|pid| **pid > 0).any(|&pid| {
            // SAFETY: `path` is PROC_PIDPATHINFO_MAXSIZE bytes, as the call needs.
            let len =
                unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
            len > 0 && Path::new(std::ffi::OsStr::from_bytes(&path[..len as usize])).starts_with(&dir)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dir;
        false
    }
}

/// Bundles an update set aside (`.Zeron.app.old-…`) or left half-installed
/// (`.Zeron.app.new-…`), once nothing runs from them.
pub fn prune_retired_mac_bundles(bundle: &Path) {
    let (Some(parent), Some(name)) = (bundle.parent(), bundle.file_name()) else {
        return;
    };
    let name = name.to_string_lossy();
    let (old, new) = (format!(".{name}.old-"), format!(".{name}.new-"));
    // An update waiting to be tried needs its copy.
    let waiting = channel::data_dir()
        .and_then(transaction::read)
        .filter(|update| update.in_progress())
        .and_then(|update| match update.layout {
            transaction::Layout::MacBundle { incoming, .. } => Some(incoming),
            _ => None,
        });
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if (file_name.starts_with(&old) || file_name.starts_with(&new))
            && waiting.as_deref() != Some(entry.path().as_path())
            && !process_runs_from(&entry.path())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Exchange two bundles in one step: `incoming` becomes `bundle` and the old one
/// is left under the retired name. Nothing ever finds `bundle` missing.
pub(crate) fn swap_mac_bundles(incoming: &Path, bundle: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let c = |path: &Path| std::ffi::CString::new(path.as_os_str().as_bytes());
        let (from, to) = (c(incoming)?, c(bundle)?);
        // SAFETY: both are NUL-terminated paths.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) } != 0 {
            return Err(std::io::Error::last_os_error()).context("swapping in the new app");
        }
        let parent = bundle.parent().context("app bundle has no parent directory")?;
        let name = bundle.file_name().context("app bundle has no name")?.to_string_lossy();
        let retired = parent.join(format!(".{name}.old-{}", std::process::id()));
        // If this is lost to a crash, it's tidied up under the name it has.
        let _ = std::fs::rename(incoming, &retired);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (incoming, bundle);
        bail!("macOS app updates are not supported on {}", std::env::consts::OS)
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
    // Not deleted: the background engine, this very app, or an update's trial may
    // still be running from it. Gone once nothing is (#1037).
    prune_retired_mac_bundles(bundle);
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
    /// The channel the check followed (`stable` | `beta`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// `latest_version` exists but can't be installed here, and why (an
    /// older macOS; a beta's engine newer than stable's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<String>,
    /// Installing `latest_version` leaves this build's channel and may go
    /// back a version (beta → stable).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub switch: bool,
    /// How this copy is installed. Absent from older engines and from the
    /// desktop app's own checker: unknown, so don't guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<InstallMethod>,
    /// Whether another device can install the update (`ApplyUpdate`). Absent
    /// from older engines: they don't say. An older engine refuses
    /// `ApplyUpdate` itself when it can't, so treat absent as "try, and show
    /// its refusal", never as a promise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_apply_remotely: Option<bool>,
    /// Why it can't, in words for the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cannot_apply_reason: Option<String>,
    /// What to run on the device instead, when there's a command for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_command: Option<String>,
    /// The latest update asked of this engine and how it went. Match its `id`
    /// with `ApplyUpdate`'s `updateId`: reconnecting doesn't say whether the
    /// update worked or went back. Absent from older engines, which only have
    /// `currentVersion` to go by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateReport>,
    /// On a macOS app install, the installed app's version, which is what
    /// `latestVersion` is compared with. It isn't `currentVersion` on a beta,
    /// whose engine is stable's. Absent elsewhere and from older engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

impl UpdateStatus {
    fn initial(version: &str) -> Self {
        Self {
            current_version: version.to_string(),
            latest_version: None,
            update_available: false,
            checked_at: None,
            error: None,
            channel: None,
            held: None,
            switch: false,
            install: None,
            can_apply_remotely: None,
            cannot_apply_reason: None,
            manual_command: None,
            update: None,
            app_version: None,
        }
    }

    fn with_ability(mut self, ability: UpdateAbility) -> Self {
        self.install = Some(ability.install);
        self.can_apply_remotely = Some(ability.can_apply_remotely);
        self.cannot_apply_reason = ability.cannot_apply_reason;
        self.manual_command = ability.manual_command;
        self
    }
}

/// `ZERON_AUTO_UPDATE=1|true|yes` — headless daemons apply updates themselves.
fn auto_update_enabled() -> bool {
    std::env::var("ZERON_AUTO_UPDATE")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// `ZERON_AUTO_UPDATE=0|false|no` — the desktop app then only reports: no
/// background download and no install on quit. Unset means on.
pub fn desktop_auto_update_enabled() -> bool {
    std::env::var("ZERON_AUTO_UPDATE")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
        .unwrap_or(true)
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
    /// The version releases are compared with: the engine's own, or the app's
    /// ([`app_version`]).
    version: Arc<str>,
    status_tx: Arc<watch::Sender<UpdateStatus>>,
    /// Wakes the loop; `forced` says whether the wake demands a check or only
    /// a fresh look at the wall clock.
    wake_tx: Arc<watch::Sender<u64>>,
    forced: Arc<AtomicBool>,
    /// Set by `zeron headless`: this process is the installed service and may
    /// restart itself into a newer installed binary.
    service: Arc<AtomicBool>,
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
        let version: Arc<str> = match role {
            Role::Engine { .. } => current_version().into(),
            Role::Desktop => app_version().into(),
        };
        let initial = UpdateStatus::initial(&version);
        let initial = match role {
            Role::Engine { .. } => UpdateStatus {
                update: channel::data_dir().and_then(transaction::report),
                app_version: installed_app_version(),
                ..initial.with_ability(UpdateAbility::current())
            },
            Role::Desktop => initial,
        };
        let (status_tx, _) = watch::channel(initial);
        let (wake_tx, _) = watch::channel(0);
        let (shutdown_tx, _) = watch::channel(false);
        let updater = Self {
            edge_url,
            role: Arc::new(role),
            version,
            status_tx: Arc::new(status_tx),
            wake_tx: Arc::new(wake_tx),
            forced: Arc::new(AtomicBool::new(false)),
            service: Arc::new(AtomicBool::new(false)),
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

    /// Called by `zeron headless`: when this process is the installed engine
    /// service and something else installs a newer binary, restart into it at
    /// the next quiet moment. A hand-started `zeron headless` is left alone —
    /// restarting the service unit would start a second engine beside it.
    pub fn restart_when_superseded(&self) {
        if !running_as_installed_service() {
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
                            && auto_update_enabled()
                            && let InstallKind::Managed { .. } = detect_install()
                            && UpdateAbility::current().can_apply_remotely
                            && !self.went_back_from_latest()
                        {
                            self.auto_apply_when_idle().await;
                        }
                    }
                    if self.service.load(Ordering::SeqCst) {
                        superseded.poll(|| self.quiescent_now());
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
                Ok(Some(applied)) => {
                    tracing::info!(version = %applied.version, "auto-update accepted; service restarting");
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
        // A macOS app's bundle carries the engine a switch must not go below, and
        // its own version is what releases are compared with: a beta's engine is
        // stable's, so by the engine's version a newer beta looks like nothing new.
        let bundle = match detect_install() {
            InstallKind::MacApp { bundle } => Some(bundle),
            _ => None,
        };
        let running_engine = bundle.as_deref().and_then(bundle_engine_version);
        let app_version = bundle.as_deref().and_then(bundle_app_version);
        let running = match &*self.role {
            Role::Engine { .. } => app_version.as_deref().unwrap_or(&self.version),
            Role::Desktop => &self.version,
        };
        let offer = offer(
            &manifest,
            running,
            manifest.channel,
            running_engine.as_deref(),
            channel::macos_version(),
        );
        let status = UpdateStatus {
            current_version: self.version.to_string(),
            update_available: offer.installs(),
            latest_version: Some(manifest.version.clone()),
            checked_at: Some(now_ms()),
            error: None,
            channel: Some(manifest.channel.as_str().to_owned()),
            held: match &offer {
                Offer::Held(why) => Some(why.clone()),
                _ => None,
            },
            switch: offer == Offer::Switch,
            install: None,
            can_apply_remotely: None,
            cannot_apply_reason: None,
            manual_command: None,
            update: None,
            app_version: None,
        };
        let status = match &*self.role {
            Role::Engine { .. } => UpdateStatus {
                update: channel::data_dir().and_then(transaction::report),
                app_version,
                ..status.with_ability(UpdateAbility::current())
            },
            Role::Desktop => status,
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
        if let InstallKind::MacApp { bundle } = detect_install() {
            prune_retired_mac_bundles(&bundle);
        }
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

    /// The latest release is one this device already tried and went back from.
    /// Trying again on a timer would cut running chats off every hour for the
    /// same result; a person asking still can.
    fn went_back_from_latest(&self) -> bool {
        let latest = self.status_tx.borrow().latest_version.clone();
        channel::data_dir()
            .and_then(transaction::read)
            .is_some_and(|last| {
                last.phase == transaction::Phase::RolledBack
                    && latest.as_deref() == Some(last.target.as_str())
            })
    }

    /// Publish the update's latest state, after something settled it.
    pub fn refresh_update_report(&self) {
        let update = channel::data_dir().and_then(transaction::report);
        let app_version = installed_app_version();
        self.status_tx.send_modify(|status| {
            status.update = update;
            status.current_version = self.version.to_string();
            status.app_version = app_version;
        });
    }

    /// Stage the newest release on THIS device and restart into a trial of it.
    /// The answer goes out first: the restart waits a moment so that it can.
    /// What became of the update is `UpdateStatus`'s `update`, under the id
    /// answered here.
    pub async fn apply(&self) -> anyhow::Result<AppliedUpdate> {
        self.apply_inner(false)
            .await?
            .ok_or_else(|| anyhow::anyhow!("update became busy before apply"))
    }

    /// `require_quiescent` is used by automatic updates. It deliberately checks
    /// after all network and staging I/O and directly before the destructive
    /// swap/restart boundary; `None` tells the caller to wait and try again.
    async fn apply_inner(&self, require_quiescent: bool) -> anyhow::Result<Option<AppliedUpdate>> {
        let ability = UpdateAbility::current();
        if !ability.can_apply_remotely {
            bail!("{}", ability.cannot_apply_reason.unwrap_or_default());
        }
        let data_dir = channel::data_dir()
            .context("this engine has no data directory")?
            .to_path_buf();
        if transaction::read(&data_dir).is_some_and(|last| last.in_progress()) {
            bail!("An update is already in progress on this device.");
        }
        let manifest = fetch_latest(&self.edge_url).await?;
        let Some(prepared) =
            prepare_this_install(&self.edge_url, &manifest, &data_dir).await?
        else {
            bail!("already up to date ({})", current_version());
        };
        if require_quiescent && !self.quiescent_now() {
            return Ok(None);
        }
        // Already on disk as `current`: there's nothing to try out, only to
        // restart into.
        let update_id = match prepared.layout {
            None => None,
            Some(layout) => {
                let requested = transaction::request(
                    &data_dir,
                    layout,
                    &prepared.from,
                    &manifest.version,
                    if MANAGED_RESTART.get().is_some() {
                        transaction::RestartMode::Relaunch
                    } else {
                        transaction::RestartMode::Service { unit: service_unit() }
                    },
                )?;
                self.refresh_update_report();
                Some(requested.id)
            }
        };
        let updater = self.clone();
        let restarting = update_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(800)).await;
            if let Err(err) = restart_service() {
                tracing::warn!(error = %err, "service restart failed — restart the engine to finish the update");
                if let (Some(id), Some(data_dir)) = (&restarting, channel::data_dir()) {
                    transaction::fail_request(
                        data_dir,
                        id,
                        "Zeron couldn't restart to install the update, so nothing changed.".into(),
                    );
                    updater.refresh_update_report();
                }
            }
        });
        Ok(Some(AppliedUpdate {
            version: manifest.version,
            update_id,
        }))
    }
}

/// The installed macOS app's own version.
fn installed_app_version() -> Option<String> {
    match detect_install() {
        InstallKind::MacApp { bundle } => bundle_app_version(&bundle),
        _ => None,
    }
}

/// A release ready to try.
struct PreparedInstall {
    from: String,
    /// `None`: it's already what's installed.
    layout: Option<transaction::Layout>,
}

/// Whatever this kind of install needs done before anything stops. `None`: nothing
/// newer to install. A release that isn't installable here is an error, in words
/// for the person.
async fn prepare_this_install(
    edge_url: &str,
    manifest: &Manifest,
    data_dir: &Path,
) -> anyhow::Result<Option<PreparedInstall>> {
    match detect_install() {
        InstallKind::Managed { app_root } => {
            if !version_newer(&manifest.version, current_version()) {
                return Ok(None);
            }
            prepare_install(edge_url, manifest, &app_root, data_dir).await?;
            let from = installed_version(&InstallKind::Managed {
                app_root: app_root.clone(),
            })
            .unwrap_or_else(|| current_version().to_owned());
            Ok(Some(PreparedInstall {
                layout: (from != manifest.version)
                    .then_some(transaction::Layout::Versioned { app_root }),
                from,
            }))
        }
        InstallKind::MacApp { bundle } => {
            // The engine's version isn't the app's (a beta carries stable's).
            let from = bundle_app_version(&bundle).context("reading the installed app's version")?;
            match offer(
                manifest,
                &from,
                manifest.channel,
                bundle_engine_version(&bundle).as_deref(),
                channel::macos_version(),
            ) {
                Offer::Newer | Offer::Switch => {}
                Offer::Held(why) => bail!(why),
                Offer::Current => return Ok(None),
            }
            let parent = bundle.parent().context("app bundle has no parent directory")?;
            let download = artifact_size(edge_url, manifest, &mac_app_artifact(&manifest.version))
                .await
                .unwrap_or(transaction::UNKNOWN_ARTIFACT_BYTES);
            // A staged copy and the one beside the installed app.
            if let Err(why) = transaction::ensure_room(data_dir, parent, download.saturating_mul(2)) {
                bail!(why);
            }
            let staged = stage_mac_app(edge_url, manifest, data_dir).await?;
            let name = bundle.file_name().context("app bundle has no name")?.to_string_lossy();
            let incoming = parent.join(format!(".{name}.new-{}", transaction::new_id()));
            // Everything that can fail about installing it happens here, with
            // nothing stopped: it's a plain copy, and then one atomic swap.
            if let Err(err) = run("ditto", &[&staged.to_string_lossy(), &incoming.to_string_lossy()]) {
                let _ = std::fs::remove_dir_all(&incoming);
                return Err(err).context("copying the new app into place");
            }
            if let Some(version_dir) = staged.parent()
                && !process_runs_from(version_dir)
            {
                let _ = std::fs::remove_dir_all(version_dir);
            }
            Ok(Some(PreparedInstall {
                from,
                layout: Some(transaction::Layout::MacBundle { bundle, incoming }),
            }))
        }
        #[cfg(windows)]
        InstallKind::WindowsPortable { directory } => {
            let from = windows::installed_version(&directory)
                .unwrap_or_else(|| current_version().to_owned());
            if !version_newer(&manifest.version, &from) {
                return Ok(None);
            }
            let download = artifact_size(edge_url, manifest, &windows::artifact(&manifest.version))
                .await
                .unwrap_or(transaction::UNKNOWN_ARTIFACT_BYTES);
            if let Err(why) = transaction::ensure_room(data_dir, &directory, download) {
                bail!(why);
            }
            let incoming = windows::stage(edge_url, manifest, &directory).await?;
            Ok(Some(PreparedInstall {
                from,
                layout: Some(transaction::Layout::WindowsFolder { directory, incoming }),
            }))
        }
        InstallKind::Unmanaged => bail!("this install doesn't update itself"),
    }
}

/// What `ApplyUpdate` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedUpdate {
    pub version: String,
    /// `UpdateStatus`'s `update.id`, to find out how it went. Absent when the
    /// version was already installed and only needed a restart.
    pub update_id: Option<String>,
}

/// Everything before an update has to stop anything: refuse if there's no room,
/// then download, verify and unpack the release beside the running one.
pub async fn prepare_install(
    edge_url: &str,
    manifest: &Manifest,
    app_root: &Path,
    data_dir: &Path,
) -> anyhow::Result<()> {
    let download = if app_root.join(&manifest.version).join("zeron").exists() {
        0
    } else {
        artifact_size(edge_url, manifest, &headless_artifact(&manifest.version))
            .await
            .unwrap_or(transaction::UNKNOWN_ARTIFACT_BYTES)
    };
    if let Err(why) = transaction::ensure_room(data_dir, app_root, download) {
        bail!(why);
    }
    stage_headless(edge_url, manifest, app_root).await?;
    Ok(())
}

/// What the feed says the release's download weighs, if it says.
async fn artifact_size(edge_url: &str, manifest: &Manifest, file: &str) -> Option<u64> {
    let base = manifest.channel.base(&release_base(edge_url).ok()?);
    let url = format!("{base}/{file}");
    let response = http_client().ok()?.head(&url).send().await.ok()?;
    response.status().is_success().then(|| response.content_length())?
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

/// Restarts a service daemon into a newer binary that is already installed
/// (the desktop app swapped the bundle, `zeron update` flipped the symlink).
/// Waits for quiescence, and tries at most once per installed version so a
/// broken service manager cannot turn into a restart loop.
#[derive(Debug, Default)]
struct SupersededRestart {
    attempted: Option<String>,
    deferred_logged: bool,
}

impl SupersededRestart {
    fn poll(&mut self, quiescent: impl Fn() -> bool) {
        let Some(installed) = installed_version(&detect_install()) else {
            return;
        };
        self.poll_installed(
            &installed,
            current_version(),
            || quiescent() && managed_restart_ready(),
            restart_service,
        );
    }

    fn poll_installed(
        &mut self,
        installed: &str,
        running: &str,
        quiescent: impl Fn() -> bool,
        restart: impl FnOnce() -> anyhow::Result<()>,
    ) {
        if !version_newer(installed, running) || self.attempted.as_deref() == Some(installed) {
            return;
        }
        if !quiescent() {
            if !self.deferred_logged {
                self.deferred_logged = true;
                tracing::info!(
                    installed,
                    running,
                    "newer version installed; restart deferred: sessions or terminals active, or waiting for the app"
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

    fn managed() -> InstallKind {
        InstallKind::Managed {
            app_root: PathBuf::from("/home/me/.zeron/app"),
        }
    }

    /// A test install beside a real one names its own unit, and then the real
    /// one's cgroup isn't its own.
    #[test]
    fn the_service_is_recognised_by_its_own_unit_only() {
        let cgroups = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/zeron.service\n";
        assert!(in_service_cgroup(cgroups, "zeron.service"));
        assert!(!in_service_cgroup(cgroups, "zeron-test.service"));
    }

    /// Inside the unit, the restart kills the `systemctl` that asked for it.
    #[test]
    fn a_restart_that_kills_its_own_systemctl_still_counts() {
        assert!(restart_outcome(None, b"").is_ok());
        assert!(restart_outcome(Some(0), b"").is_ok());
        let refused = restart_outcome(Some(1), b"Unit not found.").unwrap_err();
        assert!(refused.to_string().contains("Unit not found."), "{refused}");
    }

    #[test]
    fn a_linux_service_can_update_from_afar() {
        let ability = UpdateAbility::decide(&managed(), "linux", true, true, None);
        assert_eq!(ability.install, InstallMethod::LinuxService);
        assert!(ability.can_apply_remotely);
        assert_eq!(ability.cannot_apply_reason, None);
        assert_eq!(ability.manual_command, None);
        let app = UpdateAbility::decide(&managed(), "linux", true, false, None);
        assert_eq!(app.install, InstallMethod::LinuxApp);
        assert!(app.can_apply_remotely);
    }

    #[test]
    fn what_cant_update_from_afar_says_why_and_what_to_run() {
        let by_hand = UpdateAbility::decide(&managed(), "linux", false, false, None);
        assert_eq!(by_hand.install, InstallMethod::StartedByHand);
        assert!(!by_hand.can_apply_remotely);
        assert_eq!(by_hand.manual_command.as_deref(), Some("zeron update"));

        let read_only = UpdateAbility::decide(
            &managed(),
            "linux",
            true,
            true,
            Some(UpdateBlocker::NotWritable(PathBuf::from("/opt/zeron"))),
        );
        assert_eq!(read_only.install, InstallMethod::LinuxService);
        assert!(!read_only.can_apply_remotely);
        assert!(read_only.cannot_apply_reason.unwrap().contains("/opt/zeron"));

        let bundle = InstallKind::MacApp {
            bundle: PathBuf::from("/Applications/Zeron.app"),
        };
        let mac = UpdateAbility::decide(&bundle, "macos", true, false, None);
        assert_eq!(mac.install, InstallMethod::MacApp);
        assert!(mac.can_apply_remotely, "the app's background engine can");
        let mac_by_hand = UpdateAbility::decide(&bundle, "macos", false, false, None);
        assert!(!mac_by_hand.can_apply_remotely);
        assert!(mac_by_hand.cannot_apply_reason.unwrap().contains("Mac"));
        let mac_translocated = UpdateAbility::decide(
            &bundle,
            "macos",
            true,
            false,
            Some(UpdateBlocker::Translocated),
        );
        assert!(!mac_translocated.can_apply_remotely);
        assert!(mac_translocated.cannot_apply_reason.unwrap().contains("Applications"));

        // No macOS headless release exists, whatever the old layout says.
        let old_mac_service = UpdateAbility::decide(&managed(), "macos", true, true, None);
        assert_eq!(old_mac_service.install, InstallMethod::Unmanaged);
        assert!(!old_mac_service.can_apply_remotely);

        let source = UpdateAbility::decide(&InstallKind::Unmanaged, "linux", false, false, None);
        assert_eq!(source.install, InstallMethod::Unmanaged);
        assert_eq!(source.manual_command.as_deref(), Some(LINUX_INSTALL_COMMAND));
        let mac_source = UpdateAbility::decide(&InstallKind::Unmanaged, "macos", false, false, None);
        assert_eq!(mac_source.manual_command, None);
    }

    /// An older engine's report has none of the fields; a newer engine's
    /// method this build doesn't know is `Unknown`, not a parse failure that
    /// would lose the whole report.
    #[test]
    fn older_and_newer_engines_reports_still_read() {
        let older: UpdateStatus = serde_json::from_value(serde_json::json!({
            "currentVersion": "0.2.100",
            "latestVersion": "0.2.136",
            "updateAvailable": true,
        }))
        .unwrap();
        assert_eq!(older.install, None);
        assert_eq!(older.can_apply_remotely, None);
        assert_eq!(older.cannot_apply_reason, None);
        assert_eq!(older.manual_command, None);

        let newer: UpdateStatus = serde_json::from_value(serde_json::json!({
            "currentVersion": "0.2.200",
            "install": "somethingNew",
            "canApplyRemotely": false,
        }))
        .unwrap();
        assert_eq!(newer.install, Some(InstallMethod::Unknown));
        assert_eq!(newer.can_apply_remotely, Some(false));
    }

    #[test]
    fn the_report_names_its_fields_for_clients() {
        let status = UpdateStatus::initial("1.0.0").with_ability(UpdateAbility::decide(
            &managed(),
            "linux",
            false,
            false,
            None,
        ));
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(value["install"], "startedByHand");
        assert_eq!(value["canApplyRemotely"], false);
        assert_eq!(value["manualCommand"], "zeron update");
        assert!(value["cannotApplyReason"].is_string());
    }

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
            ..Manifest::default()
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

    #[test]
    fn superseded_service_restarts_once_when_quiet() {
        let restarts = std::cell::Cell::new(0);
        let restart = || {
            restarts.set(restarts.get() + 1);
            Ok(())
        };
        let mut superseded = SupersededRestart::default();
        // Same or older installed versions never restart.
        superseded.poll_installed("0.2.0", "0.2.0", || true, restart);
        superseded.poll_installed("0.1.9", "0.2.0", || true, restart);
        assert_eq!(restarts.get(), 0);
        // Busy: deferred, not attempted.
        superseded.poll_installed("0.2.1", "0.2.0", || false, restart);
        assert_eq!(restarts.get(), 0);
        // Quiet: exactly one attempt per installed version.
        superseded.poll_installed("0.2.1", "0.2.0", || true, restart);
        superseded.poll_installed("0.2.1", "0.2.0", || true, restart);
        assert_eq!(restarts.get(), 1);
        superseded.poll_installed("0.2.2", "0.2.0", || true, restart);
        assert_eq!(restarts.get(), 2);
    }

    #[test]
    fn an_engine_restarts_into_the_next_beta_and_the_stable_it_becomes() {
        let attempts = |installed: &str, running: &str| {
            let restarts = std::cell::Cell::new(0);
            SupersededRestart::default().poll_installed(installed, running, || true, || {
                restarts.set(restarts.get() + 1);
                Ok(())
            });
            restarts.get()
        };
        assert_eq!(attempts("0.2.137-beta.19", "0.2.137-beta.18"), 1);
        assert_eq!(attempts("0.2.137", "0.2.137-beta.19"), 1);
        assert_eq!(attempts("0.2.137-beta.18", "0.2.137-beta.18"), 0);
        // (What a numeric version on both would miss.)
        assert_eq!(attempts("0.2.137", "0.2.137"), 0);
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
        assert_eq!(plist_string(&plist, "CFBundleShortVersionString").as_deref(), Some("0.3.1"));
        // Packaging names the engine it bundles.
        assert_eq!(plist_string(&plist, "ZeronEngineVersion").as_deref(), Some("0.3.1"));
        assert_eq!(plist_string("<plist></plist>", "CFBundleShortVersionString"), None);
    }

    #[test]
    fn a_bundle_s_engine_version_is_its_key_else_the_app_s() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().join("Zeron.app");
        std::fs::create_dir_all(bundle.join("Contents")).unwrap();
        let write = |body: &str| std::fs::write(bundle.join("Contents/Info.plist"), body).unwrap();
        // A beta: the app is 0.2.136-beta.1, its engine stable's 0.2.135. An
        // engine weighing the app's version would restart forever.
        write(
            "<plist><dict><key>CFBundleShortVersionString</key><string>0.2.136</string>\
             <key>ZeronEngineVersion</key><string>0.2.135</string></dict></plist>",
        );
        assert_eq!(bundle_engine_version(&bundle).as_deref(), Some("0.2.135"));
        let mac = InstallKind::MacApp { bundle: bundle.clone() };
        assert_eq!(installed_version(&mac).as_deref(), Some("0.2.135"));
        // A bundle from before the key.
        write("<plist><dict><key>CFBundleShortVersionString</key><string>0.2.134</string></dict></plist>");
        assert_eq!(bundle_engine_version(&bundle).as_deref(), Some("0.2.134"));
    }

    #[test]
    fn prereleases_sort_as_semver_says() {
        let ordered = ["0.2.134", "0.2.135", "0.2.136-beta.1", "0.2.136-beta.2", "0.2.136-beta.10", "0.2.136", "0.2.137-beta.1"];
        for pair in ordered.windows(2) {
            assert!(version_newer(pair[1], pair[0]), "{} > {}", pair[1], pair[0]);
            assert!(!version_newer(pair[0], pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert!(!version_newer("0.2.136-beta.1", "0.2.136-beta.1"));
        assert!(version_newer("v0.2.136-beta.1", "0.2.135"));
        // Garbage never counts as newer.
        for bad in ["0.2.136-", "0.2.136-beta..1", "0.2.136+build", "beta", ""] {
            assert!(!version_newer(bad, "0.0.1"), "{bad}");
            assert!(parse_version(bad).is_none(), "{bad}");
        }
        // The numeric compare is unchanged.
        assert!(version_newer("0.1.10", "0.1.9") && version_newer("0.1.0", "0.1"));
    }

    fn manifest_of(version: &str, channel: Channel) -> Manifest {
        Manifest {
            version: version.into(),
            channel,
            ..Manifest::default()
        }
    }

    #[test]
    fn a_build_takes_newer_releases_of_its_channel() {
        let stable = manifest_of("0.2.136", Channel::Stable);
        assert_eq!(offer(&stable, "0.2.135", Channel::Stable, None, None), Offer::Newer);
        assert_eq!(offer(&stable, "0.2.136", Channel::Stable, None, None), Offer::Current);
        assert_eq!(offer(&stable, "0.2.137", Channel::Stable, None, None), Offer::Current);
        let beta = manifest_of("0.2.136-beta.2", Channel::Beta);
        assert_eq!(offer(&beta, "0.2.136-beta.1", Channel::Beta, Some("0.2.135"), None), Offer::Newer);
        assert_eq!(offer(&beta, "0.2.136-beta.2", Channel::Beta, Some("0.2.135"), None), Offer::Current);
    }

    #[test]
    fn choosing_beta_on_stable_takes_only_a_newer_beta() {
        let beta = manifest_of("0.2.136-beta.1", Channel::Beta);
        assert_eq!(offer(&beta, "0.2.135", Channel::Beta, None, None), Offer::Newer);
        // Stable moved past the last beta: wait for the next one.
        assert_eq!(offer(&beta, "0.2.136", Channel::Beta, None, None), Offer::Current);
    }

    #[test]
    fn leaving_beta_installs_stable_even_when_older() {
        let stable = manifest_of("0.2.135", Channel::Stable);
        assert_eq!(offer(&stable, "0.2.136-beta.2", Channel::Stable, Some("0.2.135"), None), Offer::Switch);
        // A stable that caught up is simply newer.
        let caught_up = manifest_of("0.2.136", Channel::Stable);
        assert_eq!(offer(&caught_up, "0.2.136-beta.2", Channel::Stable, Some("0.2.135"), None), Offer::Newer);
    }

    #[test]
    fn leaving_beta_waits_when_stable_s_engine_is_older_than_the_beta_s() {
        let stable = manifest_of("0.2.135", Channel::Stable);
        let held = offer(&stable, "0.2.136-beta.3", Channel::Stable, Some("0.2.136"), None);
        assert!(matches!(&held, Offer::Held(why) if why.contains("catches up")), "{held:?}");
        // A manifest naming its engine is weighed by that.
        let named = Manifest { engine_version: Some("0.2.136".into()), ..stable };
        assert_eq!(offer(&named, "0.2.136-beta.3", Channel::Stable, Some("0.2.136"), None), Offer::Switch);
    }

    #[test]
    fn a_release_for_a_newer_macos_is_held() {
        let beta = Manifest {
            minimum_macos: Some("14.0".into()),
            ..manifest_of("0.2.136-beta.1", Channel::Beta)
        };
        let held = offer(&beta, "0.2.135", Channel::Beta, None, Some("13.6"));
        assert!(matches!(&held, Offer::Held(why) if why.contains("macOS 14.0")), "{held:?}");
        assert_eq!(offer(&beta, "0.2.135", Channel::Beta, None, Some("14.1")), Offer::Newer);
        assert!(!held.installs());
    }

    #[test]
    fn a_beta_manifest_reads_its_extra_fields() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"version":"0.2.136-beta.1","engineVersion":"0.2.135","minimumMacOS":"14.0","files":{}}"#,
        )
        .unwrap();
        assert_eq!(manifest.engine_version.as_deref(), Some("0.2.135"));
        assert_eq!(manifest.minimum_macos.as_deref(), Some("14.0"));
        // A stable manifest has neither.
        let stable: Manifest = serde_json::from_str(r#"{"version":"0.2.135","files":{}}"#).unwrap();
        assert_eq!((stable.engine_version, stable.minimum_macos), (None, None));
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
            ..Manifest::default()
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

    #[test]
    fn a_beta_bundle_says_its_beta_version_not_its_numeric_short_one() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        // As scripts/package-zgx-beta.sh writes it.
        std::fs::write(
            contents.join("Info.plist"),
            "<plist><dict><key>CFBundleShortVersionString</key><string>0.2.137</string>\
             <key>ZeronEngineVersion</key><string>0.2.136</string>\
             <key>ZeronVersion</key><string>0.2.137-beta.16</string></dict></plist>",
        )
        .unwrap();
        assert_eq!(bundle_app_version(dir.path()).as_deref(), Some("0.2.137-beta.16"));
        // A stable bundle has no `ZeronVersion`.
        std::fs::write(
            contents.join("Info.plist"),
            "<plist><dict><key>CFBundleShortVersionString</key><string>0.2.136</string></dict></plist>",
        )
        .unwrap();
        assert_eq!(bundle_app_version(dir.path()).as_deref(), Some("0.2.136"));
    }
}
