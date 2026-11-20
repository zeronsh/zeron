//! Updates for Windows installs — the per-user installer
//! (`dist/windows/zeron.iss`) and the portable zip. Both place
//! `zeron-update.json` beside `zeron.exe`; source builds remain unmanaged.
use std::io::Read;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

const CONFIG: &str = "zeron-update.json";
/// The running image, moved aside during a swap. A running executable can be
/// renamed but not deleted, so the file survives until the process exits and
/// is removed by the relaunched instance (or the next update attempt).
const BACKUP: &str = "zeron.exe.old";
/// Deterministic name for the copy that becomes the next installation; a
/// crash between the two renames leaves at most this file behind.
const INCOMING: &str = ".zeron-update-incoming.exe";
/// The installed version, written by each update: the app's background
/// engine (`zeron.exe engine`) compares it with its own to restart into the
/// new image once idle.
const VERSION_FILE: &str = "zeron-version";
/// The installer's uninstall entry (`AppId` in `dist/windows/zeron.iss`, plus
/// Inno Setup's `_is1` suffix). Settings → Apps reads `DisplayVersion` here.
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{AD5DEC34-E254-467B-8F24-8127EBAF4DA6}_is1";
/// The zgx app's own files beside `zeron.exe`, by version:
/// `beta\<version>\ui`, `plugins-builtin` and `engine\zeron.exe` (a whole
/// Zeron executable run as `zeron.exe engine`), for a stable release as for a
/// beta. (Named for the betas that came first. Every beta in the field checks
/// a package for `beta\<version>` before it installs it, and the beta channel
/// carries stable releases too, so the one name stays.) Such an app can't
/// update by swapping one file, so it updates from the package
/// (`zeron-<version>-windows-<arch>.zip`); nothing already there is replaced,
/// which a running engine would prevent. The old app (one self-contained
/// `zeron.exe`) reads no such folder.
const BETA: &str = "beta";
/// Where a staged package unpacks, beside the staged `zeron.exe`.
const PACKAGE: &str = "package";
/// Held while one process completes an install ([`complete_install`]), so a
/// relaunched app and a restarting engine don't both unpack at once.
const COMPLETING: &str = ".zeron-completing.lock";

/// Set by an app whose `zeron.exe` needs its version's folder beside it (the
/// zgx app): its updates come as the package on every channel.
static PACKAGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// This app is a packaged one (see [`PACKAGED`]). Called first thing in `main`.
pub fn set_packaged() {
    PACKAGED.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn packaged() -> bool {
    PACKAGED.load(std::sync::atomic::Ordering::SeqCst)
}

/// `version`'s own files in the installation at `directory`.
pub fn version_files(directory: &Path, version: &str) -> PathBuf {
    directory.join(BETA).join(version)
}

#[derive(serde::Deserialize)]
struct Config {
    releases_url: String,
}

pub(super) fn is_managed(exe: &Path) -> bool {
    exe.file_name().is_some_and(|name| name == "zeron.exe")
        && exe.parent().is_some_and(|dir| dir.join(CONFIG).is_file())
}

/// The installation `exe` belongs to, if it's update-managed: the folder of
/// its `zeron.exe`, or for the zgx app's engine
/// (`<directory>\beta\<version>\engine\zeron.exe`), the folder its version's
/// files are in.
pub(super) fn managed_directory(exe: &Path) -> Option<PathBuf> {
    if is_managed(exe) {
        return exe.parent().map(Path::to_path_buf);
    }
    let engine = exe.parent()?;
    let folder = engine.parent()?.parent()?;
    if exe.file_name()? != "zeron.exe" || engine.file_name()? != "engine" || folder.file_name()? != BETA {
        return None;
    }
    let directory = folder.parent()?;
    is_managed(&directory.join("zeron.exe")).then(|| directory.to_path_buf())
}

pub(super) fn release_url() -> anyhow::Result<Option<String>> {
    // A new version on trial runs from beside the installation.
    let exe = match std::env::var_os(super::INSTALLED_AT_ENV) {
        Some(directory) => PathBuf::from(directory).join("zeron.exe"),
        None => match managed_directory(&std::env::current_exe()?) {
            Some(directory) => directory.join("zeron.exe"),
            None => return Ok(None),
        },
    };
    if !is_managed(&exe) {
        return Ok(None);
    }
    let config: Config = serde_json::from_slice(&std::fs::read(exe.with_file_name(CONFIG))?)
        .context("reading Windows update configuration")?;
    super::validate_release_override(&config.releases_url).map(Some)
}

pub fn artifact(version: &str) -> String {
    format!("zeron-{version}-windows-{}.exe", std::env::consts::ARCH)
}

/// A packaged app's update: the portable package.
fn package(version: &str) -> String {
    format!("zeron-{version}-windows-{}.zip", std::env::consts::ARCH)
}

/// `x.y.z`, or a beta's `x.y.z-beta.n`: digits and dots only, since it names
/// files and folders.
fn valid_version(version: &str) -> bool {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let (core, beta) = match version.split_once("-beta.") {
        Some((core, beta)) => (core, Some(beta)),
        None => (version, None),
    };
    core.split('.').all(digits) && beta.is_none_or(digits)
}

/// The manifest's mandatory checksum for `file`.
fn expected_checksum<'a>(manifest: &'a super::Manifest, file: &str) -> anyhow::Result<&'a str> {
    let expected = manifest
        .files
        .get(file)
        .and_then(|meta| meta.sha256.as_deref())
        .context("Windows updates require a SHA-256 checksum")?;
    ensure!(
        expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid SHA-256 checksum"
    );
    Ok(expected)
}

/// The staged executable must run and be the release the manifest promised.
async fn verify_staged_version(staged: &Path, version: &str) -> anyhow::Result<()> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new(staged)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("staged executable version check timed out")??;
    ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == format!("zeron {version}"),
        "staged executable has the wrong version or cannot run"
    );
    Ok(())
}

/// Stage a release from its package: the executable where the single-file
/// path leaves one (its checksum beside it), the rest in [`PACKAGE`].
/// From the feed at `base` (a channel's, or one release's own).
async fn stage_package(
    base: &str,
    manifest: &super::Manifest,
    directory: &Path,
) -> anyhow::Result<PathBuf> {
    let file = package(&manifest.version);
    expected_checksum(manifest, &file)?;
    let temporary = tempfile::Builder::new()
        .prefix(".zeron-update-")
        .tempdir_in(directory)?;
    let archive = temporary.path().join(&file);
    super::download_release_file_from(base, manifest, &file, &archive).await?;
    let unpacked = temporary.path().join(PACKAGE);
    std::fs::create_dir(&unpacked)?;
    // Windows' own tar (since Windows 10 1803), by its full path.
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let output = tokio::process::Command::new(system.join(r"System32\tar.exe"))
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpacked)
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .output()
        .await
        .context("unpacking the update")?;
    ensure!(
        output.status.success(),
        "unpacking the update failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    std::fs::remove_file(&archive)?;
    // The zgx app's package has its version's folder; the old app's (a
    // release going back to it) has no such folder at all.
    ensure!(
        files_complete(&version_files(&unpacked, &manifest.version)) || !unpacked.join(BETA).exists(),
        "the package has no {BETA}\\{}\\engine\\zeron.exe",
        manifest.version
    );
    let staged = temporary.path().join("zeron.exe");
    std::fs::rename(unpacked.join("zeron.exe"), &staged).context("the package has no zeron.exe")?;
    std::fs::write(temporary.path().join("sha256"), digest(&staged)?)?;
    std::fs::write(temporary.path().join("version"), &manifest.version)?;
    verify_staged_version(&staged, &manifest.version).await?;
    let _ = temporary.keep();
    Ok(staged)
}

/// A staged package's own folder into place: under its version, so it
/// replaces nothing in use. (Left behind if the swap then fails; the next try
/// replaces it, and the next launch removes it.) A version that's already
/// whole there stays as it is: its engine may be running.
fn install_version_files(staged: &Path, directory: &Path) -> anyhow::Result<()> {
    let incoming = staged.with_file_name(PACKAGE).join(BETA);
    if !incoming.is_dir() {
        return Ok(());
    }
    let installed = directory.join(BETA);
    std::fs::create_dir_all(&installed)?;
    for entry in std::fs::read_dir(&incoming)? {
        let entry = entry?;
        let target = installed.join(entry.file_name());
        if target.exists() {
            if files_complete(&target) {
                continue;
            }
            ensure!(remove_whole(&target), "{} is in use and incomplete", target.display());
        }
        std::fs::rename(entry.path(), &target)
            .with_context(|| format!("moving {} into place", target.display()))?;
    }
    Ok(())
}

/// A version's folder has all it runs on: its engine and its scripts. (An
/// engine alone can be what a cleanup couldn't delete while it ran.)
fn files_complete(files: &Path) -> bool {
    files.join("engine").join("zeron.exe").is_file() && files.join("ui").join("app.luau").is_file()
}

/// Delete `dir` whole or not at all: it's first moved aside, which Windows
/// refuses while anything inside is in use (a running engine), so a folder
/// in use is never left half deleted. Whether it went.
fn remove_whole(dir: &Path) -> bool {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return false;
    };
    let aside = parent.join(format!(".{}.removing-{}", name.to_string_lossy(), std::process::id()));
    if std::fs::rename(dir, &aside).is_err() {
        return false;
    }
    let _ = std::fs::remove_dir_all(&aside);
    true
}

/// An installation the old app's updater moved to the zgx app still names
/// that app's feed, the GitHub release marked latest, which has no beta
/// channel beside it: from its first launch as the zgx app it follows the
/// edge's (stable at its root, beta in `beta/`), as a zgx package names. A
/// feed someone set themselves stays.
fn leave_github_feed(directory: &Path) {
    let config = directory.join(CONFIG);
    let on_github = std::fs::read(&config)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).ok())
        .is_some_and(|c| c.releases_url.trim().trim_end_matches('/') == GITHUB_LATEST);
    if !on_github {
        return;
    }
    let new = directory.join(format!(".{CONFIG}.new"));
    let json = serde_json::json!({ "releases_url": EDGE_FEED }).to_string();
    if std::fs::write(&new, json).and_then(|()| std::fs::rename(&new, &config)).is_err() {
        let _ = std::fs::remove_file(&new);
    }
}

/// Remove every version's folder but `keep`'s, best-effort: an engine that's
/// still running keeps its own executable until it stops.
fn remove_version_files(directory: &Path, keep: Option<&str>) {
    let installed = directory.join(BETA);
    let Ok(entries) = std::fs::read_dir(&installed) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            // Moved aside by a removal that didn't finish: nothing runs from it.
            let _ = std::fs::remove_dir_all(entry.path());
        } else if keep.is_none_or(|keep| name != keep) {
            remove_whole(&entry.path());
        }
    }
    let _ = std::fs::remove_dir(&installed);
}

/// Download next to the installation, verifying its mandatory checksum.
/// Each attempt has its own directory, so failed or concurrent downloads cannot
/// leave a reusable, partially staged executable.
pub async fn stage(
    edge_url: &str,
    manifest: &super::Manifest,
    directory: &Path,
) -> anyhow::Result<PathBuf> {
    ensure!(
        valid_version(&manifest.version),
        "invalid Windows release version"
    );
    // The zgx app (this one, or the one whose engine this is: its version's
    // folder is there) updates from the package, on every channel.
    if manifest.channel == super::Channel::Beta
        || packaged()
        || version_files(directory, super::app_version()).is_dir()
    {
        let base = manifest.channel.base(&super::release_base(edge_url)?);
        return stage_package(&base, manifest, directory).await;
    }
    let file = artifact(&manifest.version);
    let expected = expected_checksum(manifest, &file)?;
    let temporary = tempfile::Builder::new()
        .prefix(".zeron-update-")
        .tempdir_in(directory)?;
    let staged = temporary.path().join("zeron.exe");
    super::download_release_file(edge_url, manifest, &file, &staged).await?;
    std::fs::write(temporary.path().join("sha256"), expected)?;
    std::fs::write(temporary.path().join("version"), &manifest.version)?;
    verify(&staged)?;
    verify_staged_version(&staged, &manifest.version).await?;
    let _ = temporary.keep();
    Ok(staged)
}

/// What [`complete_install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    /// The version's folder was there already (another process finished it).
    Complete,
    /// The version's folder is in place now, from its own package.
    Installed,
    /// The feed had moved on: the newer release replaced `zeron.exe` and its
    /// folder is in place. Run the installed `zeron.exe` instead of this one.
    Updated(String),
}

/// The installation a packaged app at `exe` must complete before it runs:
/// its directory, when `exe` is an update-managed `zeron.exe` whose own
/// version's folder isn't beside it.
///
/// That's how an updater that only swaps the one file (every Zeron before
/// the zgx app, and the zgx app's own single-file artifact,
/// [`artifact`]) leaves a zgx app: the executable alone. And how an update's
/// trial starts one: staged on its own, run in place of the installation.
pub fn incomplete_install(exe: &Path, version: &str) -> Option<PathBuf> {
    let directory = install_directory(exe)?;
    (is_managed(&directory.join("zeron.exe"))
        && !files_complete(&version_files(&directory, version)))
    .then_some(directory)
}

/// The installation the executable at `exe` belongs to: its folder, or, for a
/// new version on trial (started from where the update staged it), the one
/// it's going to become ([`super::INSTALLED_AT_ENV`]).
pub fn install_directory(exe: &Path) -> Option<PathBuf> {
    match std::env::var_os(super::INSTALLED_AT_ENV) {
        Some(directory) => Some(PathBuf::from(directory)),
        None => exe.parent().map(Path::to_path_buf),
    }
}

/// Bring in this version's folder from the release feed (the one
/// `zeron-update.json` names), as an update would: the package, its
/// checksum from the feed's manifest, unpacked beside the installation.
/// When the feed has a newer release by now, that one is installed instead,
/// `zeron.exe` with it. Nothing already installed is removed or replaced
/// unless the whole new version is in place, so a failure leaves things as
/// they were and the next start tries again.
pub async fn complete_install(
    edge_url: &str,
    directory: &Path,
    version: &str,
) -> anyhow::Result<Completion> {
    ensure!(valid_version(version), "invalid Windows release version");
    let _lock = lock_completion(directory).await?;
    if files_complete(&version_files(directory, version)) {
        return Ok(Completion::Complete);
    }
    tracing::info!(version, "completing the install: this version's files aren't here");
    // A trial started from where an update staged it has its package beside
    // it, verified when it was staged: no second download.
    if let Ok(exe) = std::env::current_exe()
        && files_complete(&version_files(&exe.with_file_name(PACKAGE), version))
    {
        install_version_files(&exe, directory)?;
        return Ok(Completion::Installed);
    }
    // This version's own package first, wherever the feed keeps one release's
    // files: what `latest` says can be older (a release withdrawn) or newer.
    match pinned_release(edge_url, version).await {
        Some((base, manifest)) => match stage_package(&base, &manifest, directory).await {
            Ok(staged) => {
                let result = install_version_files(&staged, directory);
                if let Some(stage) = staged.parent() {
                    let _ = std::fs::remove_dir_all(stage);
                }
                result?;
                return Ok(Completion::Installed);
            }
            Err(err) => tracing::warn!(error = %format!("{err:#}"), "this version's own package didn't install; trying the latest"),
        },
        None => tracing::info!(version, "the feed keeps no package of this version apart; trying the latest"),
    }
    let manifest = super::fetch_latest_on(edge_url, super::Channel::of_version(version))
        .await
        .with_context(|| {
            format!("reading the release feed (neither this version's own manifest nor the latest one could be read)")
        })?;
    let latest = manifest.version.trim().to_owned();
    if latest != version && !super::version_newer(&latest, version) {
        bail!("the release feed offers Zeron {latest}, which is older than this Zeron {version}");
    }
    let base = manifest.channel.base(&super::release_base(edge_url)?);
    let staged = stage_package(&base, &manifest, directory).await?;
    // A trial (or anything not run as the installed `zeron.exe`) can't
    // replace the installation; it installs only its own version's files.
    let installed = directory.join("zeron.exe");
    let is_installed = std::env::current_exe()
        .and_then(|exe| Ok(exe.canonicalize()? == installed.canonicalize()?))
        .unwrap_or(false);
    if latest != version && !is_installed {
        if let Some(stage) = staged.parent() {
            let _ = std::fs::remove_dir_all(stage);
        }
        bail!("the release feed has moved on to Zeron {latest}; this Zeron {version} can't be completed from it");
    }
    if latest == version {
        let result = install_version_files(&staged, directory);
        if let Some(stage) = staged.parent() {
            let _ = std::fs::remove_dir_all(stage);
        }
        result?;
        return Ok(Completion::Installed);
    }
    // (The lock is this call's already.)
    apply_locked(&staged, directory, false)?;
    Ok(Completion::Updated(latest))
}

/// The GitHub release marked latest, the feed the old app's Windows installs name.
const GITHUB_LATEST: &str = "https://github.com/zeronsh/zeron/releases/latest/download";
/// The edge's feed, the zgx app's: stable at its root, the beta channel in `beta/`.
const EDGE_FEED: &str = "https://zeron.sh/releases";

/// Where the feed keeps `version`'s own files, and its manifest: a GitHub
/// release of its own (`releases/download/v<version>`, whose `manifest.json`
/// is that release's), or on the edge, the files of every release side by
/// side with `manifest-<version>.json` (release.yml). `None` when there's
/// none to be had.
async fn pinned_release(edge_url: &str, version: &str) -> Option<(String, super::Manifest)> {
    let channel = super::Channel::of_version(version);
    let feed = super::release_base(edge_url).ok()?;
    let (base, manifest_url) = match feed.strip_suffix("/releases/latest/download") {
        Some(repository) if channel == super::Channel::Stable => {
            let base = format!("{repository}/releases/download/v{version}");
            let url = format!("{base}/manifest.json");
            (base, url)
        }
        _ => {
            let base = channel.base(&feed);
            let url = format!("{base}/manifest-{version}.json");
            (base, url)
        }
    };
    let client = super::http_client().ok()?;
    let response = super::fetch_release_metadata(&client, &manifest_url).await.ok()?;
    if !response.status.is_success() {
        return None;
    }
    let mut manifest: super::Manifest = serde_json::from_slice(&response.body).ok()?;
    if manifest.version.trim() != version {
        return None;
    }
    manifest.channel = channel;
    Some((base, manifest))
}

/// The completion lock if no one holds it, without waiting.
fn try_lock_completion(directory: &Path) -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(directory.join(COMPLETING))
        .ok()
}

/// [`try_lock_completion`], waiting up to `wait` for a holder (a cleanup is
/// brief; a completion can take a download) to let go.
fn lock_completion_blocking(directory: &Path, wait: std::time::Duration) -> anyhow::Result<std::fs::File> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if let Some(file) = try_lock_completion(directory) {
            return Ok(file);
        }
        ensure!(
            std::time::Instant::now() < deadline,
            "another start of Zeron is completing this installation; try again in a moment"
        );
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Exclusive use of the completion lock, waiting up to ten minutes for
/// another process that holds it (a download on a slow line).
async fn lock_completion(directory: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    let path = directory.join(COMPLETING);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&path)
        {
            Ok(file) => return Ok(file),
            // ERROR_SHARING_VIOLATION: someone else is completing it.
            Err(err) if err.raw_os_error() == Some(32) && std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(err) => return Err(err).with_context(|| format!("locking {}", path.display())),
        }
    }
}

fn verify(staged: &Path) -> anyhow::Result<()> {
    let expected = std::fs::read_to_string(staged.with_file_name("sha256"))?;
    verify_digest(staged, expected.trim())
}

fn verify_digest(path: &Path, expected: &str) -> anyhow::Result<()> {
    ensure!(
        digest(path)?.eq_ignore_ascii_case(expected),
        "staged update checksum mismatch"
    );
    Ok(())
}

fn digest(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Replace the running executable with the staged update.
///
/// A hand-rolled swap replaces `self-replace` here: that crate renames the
/// running executable away and schedules its deletion *before* copying the
/// replacement back, so a failed copy (disk full, antivirus lock) left no
/// launchable installation. This swap copies first, moves the running image
/// aside only when the replacement is verified in place, and restores the
/// previous executable on every failure after that move. The relaunch cleanup
/// and [`Self::recover_from_backup`] cover the leftovers of a hard crash
/// between the two renames.
pub fn apply(staged: &Path, directory: &Path, relaunch: bool) -> anyhow::Result<()> {
    // From the new version's folder going in to the swap, no start's cleanup
    // may take that folder for an old one (nothing runs from it yet).
    let _lock = lock_completion_blocking(directory, std::time::Duration::from_secs(30))?;
    apply_locked(staged, directory, relaunch)
}

/// [`apply`], the completion lock held already.
fn apply_locked(staged: &Path, directory: &Path, relaunch: bool) -> anyhow::Result<()> {
    let installed = directory.join("zeron.exe");
    ensure!(
        std::env::current_exe()?.canonicalize()? == installed.canonicalize()?,
        "update must run from its installation"
    );
    verify(staged)?;
    recover_from_backup(&installed)?;
    let version = std::fs::read_to_string(staged.with_file_name("version")).ok();
    let version = version.as_deref().map(str::trim);
    install_version_files(staged, directory)?;
    swap_into_place(staged, &installed)?;
    // The old app (a single executable) reads no folder; the zgx app only
    // its own version's. Others' go, except while an engine still runs them.
    let packaged_update = staged.with_file_name(PACKAGE).join(BETA).is_dir();
    remove_version_files(directory, version.filter(|_| packaged_update));
    if let Some(version) = version {
        refresh_installer_version(directory, version);
        let _ = std::fs::write(directory.join(VERSION_FILE), version);
    }
    if relaunch {
        std::process::Command::new(&installed)
            .args(["--wait-for-exit", &std::process::id().to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("relaunching updated application")?;
    }
    let _ = std::fs::remove_file(staged);
    let _ = std::fs::remove_file(staged.with_file_name("sha256"));
    let _ = std::fs::remove_file(staged.with_file_name("version"));
    let _ = std::fs::remove_dir_all(staged.with_file_name(PACKAGE));
    if let Some(parent) = staged.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

/// Make a new version that's proved itself the installed one, from the exe it's
/// running as. A running image can be renamed, so it simply moves into place; the
/// old one moves aside first and goes on running until it exits.
pub(super) fn commit_trial(incoming: &Path, directory: &Path, version: &str) -> anyhow::Result<()> {
    let installed = directory.join("zeron.exe");
    recover_from_backup(&installed)?;
    let backup = directory.join(BACKUP);
    std::fs::rename(&installed, &backup).context("moving the old executable aside")?;
    if let Err(err) = std::fs::rename(incoming, &installed) {
        std::fs::rename(&backup, &installed)
            .context("restoring the previous executable after a failed update")?;
        return Err(err).context("installing the updated executable");
    }
    refresh_installer_version(directory, version);
    std::fs::write(directory.join(VERSION_FILE), version).context("recording the installed version")?;
    Ok(())
}

/// Settle leftovers from a previous swap: restore the backup when the
/// installation itself is missing (a crash between the two renames), or drop
/// the backup when the installation is already in place.
fn recover_from_backup(installed: &Path) -> anyhow::Result<()> {
    let Some(directory) = installed.parent() else {
        return Ok(());
    };
    let backup = directory.join(BACKUP);
    if !backup.exists() {
        return Ok(());
    }
    if installed.exists() {
        let _ = std::fs::remove_file(&backup);
        return Ok(());
    }
    std::fs::rename(&backup, installed)
        .context("restoring the installation from its pre-update backup")
}

/// Move `staged` onto `installed` through a verified copy, keeping the old
/// image as a recoverable backup until the new one is in place.
fn swap_into_place(staged: &Path, installed: &Path) -> anyhow::Result<()> {
    let directory = installed
        .parent()
        .context("installation has no parent directory")?;
    let backup = directory.join(BACKUP);
    let incoming = directory.join(INCOMING);
    let expected = std::fs::read_to_string(staged.with_file_name("sha256"))
        .context("reading the staged checksum")?;
    let expected = expected.trim().to_owned();

    // Copy and re-verify before touching the installation: an error here, or
    // a corrupt copy, leaves the launch path alone.
    if let Err(err) = std::fs::copy(staged, &incoming)
        .and_then(|_| verify_digest(&incoming, &expected).map_err(std::io::Error::other))
    {
        let _ = std::fs::remove_file(&incoming);
        return Err(err).context("copying the staged update into the installation directory");
    }

    // The running image can be renamed but not deleted; move it aside only
    // now that the replacement is verified next to it.
    if let Err(err) = std::fs::rename(installed, &backup) {
        let _ = std::fs::remove_file(&incoming);
        return Err(err).context("moving the running executable aside for replacement");
    }
    if let Err(err) = std::fs::rename(&incoming, installed) {
        // The installation must not be left missing: restore the backup or
        // fail loudly about both errors.
        std::fs::rename(&backup, installed)
            .context("restoring the previous executable after a failed update")?;
        return Err(err).context("installing the updated executable");
    }
    Ok(())
}

/// The version the last update installed in `directory`, if one recorded it.
pub(super) fn installed_version(directory: &Path) -> Option<String> {
    std::fs::read_to_string(directory.join(VERSION_FILE))
        .ok()
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
}

/// Called by the newly installed desktop before it opens the engine profile.
/// The old image, renamed aside by the update, can only be deleted after its
/// process exits — this is that moment.
pub fn wait_for_exit(pid: u32) -> anyhow::Result<()> {
    ensure!(pid != std::process::id(), "cannot wait for own process");
    let result = wait_for_exit_impl(pid);
    let _ = remove_own_backup();
    result
}

fn wait_for_exit_impl(pid: u32) -> anyhow::Result<()> {
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return Ok(()); // The previous instance already exited.
        }
        return Err(error).context("opening previous instance");
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    if unsafe { WaitForSingleObject(process.as_raw_handle(), 60000) } != WAIT_OBJECT_0 {
        bail!("previous instance did not exit within 60 seconds");
    }
    Ok(())
}

/// Startup cleanup for a normal launch: the image an earlier update renamed
/// aside can be deleted once no process runs it. Best-effort — a previous
/// instance still exiting keeps it until the next launch.
pub fn cleanup_previous_image() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if is_managed(&exe) {
        let _ = remove_own_backup();
        // Not while another start completes this installation: a folder it
        // just put in place may not be running anything yet.
        let Some(_lock) = exe.parent().and_then(try_lock_completion) else {
            return;
        };
        if let Some(directory) = exe.parent() {
            let own = super::app_version();
            let beta = super::Channel::of_version(own) == super::Channel::Beta;
            remove_version_files(directory, (beta || packaged()).then_some(own));
            if packaged() {
                leave_github_feed(directory);
            }
        }
    }
}

/// Keep the installer's uninstall entry truthful after an in-app update of
/// the installation in `directory`. Only the installer's own installation
/// may: the entry is per user, not per folder, so a portable copy (or a
/// test copy) updating elsewhere on the same PC must leave the installed
/// one's version alone. A portable-only PC has no entry, and the open fails.
fn refresh_installer_version(directory: &Path, version: &str) {
    refresh_installer_version_at(UNINSTALL_KEY, directory, version);
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// [`refresh_installer_version`] under `subkey` of HKEY_CURRENT_USER.
fn refresh_installer_version_at(subkey: &str, directory: &Path, version: &str) {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey,
        RegOpenKeyExW, RegSetValueExW,
    };
    let path = wide(subkey);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: a NUL-terminated path and an out pointer to a key closed below.
    let opened = unsafe {
        RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_QUERY_VALUE | KEY_SET_VALUE, &mut key)
    };
    if opened != ERROR_SUCCESS {
        return;
    }
    let location = read_string(key, "InstallLocation");
    if !installer_owns(location.as_deref(), directory) {
        tracing::info!(
            directory = %directory.display(),
            installer = location.as_deref().unwrap_or(""),
            "not the installer's installation; its DisplayVersion is left as it is"
        );
        // SAFETY: the key opened above.
        unsafe { RegCloseKey(key) };
        return;
    }
    let name = wide("DisplayVersion");
    let data = wide(version);
    // SAFETY: `data` is a NUL-terminated UTF-16 string of the length given.
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * std::mem::size_of::<u16>()) as u32,
        )
    };
    // SAFETY: the key opened above.
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS {
        tracing::warn!(status, "could not update the installer's DisplayVersion");
    }
}

/// A string value of `key`, if it has one.
fn read_string(key: windows_sys::Win32::System::Registry::HKEY, name: &str) -> Option<String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let name = wide(name);
    let mut bytes: u32 = 0;
    // SAFETY: asks the value's size only (no buffer).
    let sized = unsafe {
        RegGetValueW(key, std::ptr::null(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut bytes)
    };
    if sized != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buffer` holds `bytes` bytes.
    let read = unsafe {
        RegGetValueW(key, std::ptr::null(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut bytes)
    };
    if read != ERROR_SUCCESS {
        return None;
    }
    let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// Whether the installer's recorded `InstallLocation` is `directory`: the
/// same folder however it's spelt (case, a trailing separator, `/` or `\`,
/// or a path that resolves to it). No location: not the installer's.
fn installer_owns(location: Option<&str>, directory: &Path) -> bool {
    let Some(location) = location.map(str::trim).filter(|l| !l.is_empty()) else {
        return false;
    };
    let spelt = |path: &Path| {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let text = path.to_string_lossy().replace('/', "\\");
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned();
        text.trim_end_matches('\\').to_lowercase()
    };
    spelt(Path::new(location)) == spelt(directory)
}

fn remove_own_backup() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let Some(directory) = exe.parent() else {
        return Ok(());
    };
    std::fs::remove_file(directory.join(BACKUP))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An open handle sharing read/write but NOT delete: `std::fs::copy` can
    /// still write through it, while the final rename (which needs delete
    /// access) fails with a sharing violation — a deterministic stand-in for
    /// antivirus or file-sharing errors after the old image has moved.
    struct NoDeleteHandle(*mut core::ffi::c_void);
    impl NoDeleteHandle {
        fn open(path: &Path) -> Self {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Foundation::GENERIC_READ;
            use windows_sys::Win32::Storage::FileSystem::{
                CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
            };
            let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            wide.push(0);
            let raw = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            assert!(
                raw != -1isize as _,
                "CreateFileW failed: {}",
                std::io::Error::last_os_error()
            );
            Self(raw)
        }
    }
    impl Drop for NoDeleteHandle {
        fn drop(&mut self) {
            use windows_sys::Win32::Foundation::CloseHandle;
            unsafe { CloseHandle(self.0) };
        }
    }

    /// A staged update layout: `<dir>/zeron.exe` plus its `sha256` sidecar.
    fn staged_with(dir: &Path, bytes: &[u8]) -> PathBuf {
        let staged = dir.join("zeron.exe");
        std::fs::write(&staged, bytes).unwrap();
        std::fs::write(dir.join("sha256"), format!("{:x}", Sha256::digest(bytes))).unwrap();
        staged
    }

    #[test]
    fn only_the_installers_own_folder_is_its_installation() {
        let install = tempfile::tempdir().unwrap();
        let dir = install.path();
        let text = dir.to_string_lossy().into_owned();
        // As Inno Setup records it: a trailing backslash.
        assert!(installer_owns(Some(&format!("{text}\\")), dir));
        assert!(installer_owns(Some(&text.to_uppercase()), dir));
        assert!(installer_owns(Some(&text.replace('\\', "/")), dir));
        assert!(installer_owns(Some(&format!("  {text}  ")), dir));
        let other = tempfile::tempdir().unwrap();
        assert!(!installer_owns(Some(&other.path().to_string_lossy()), dir));
        // A folder inside the installation is not the installation.
        let inner = dir.join("beta");
        std::fs::create_dir(&inner).unwrap();
        assert!(!installer_owns(Some(&text), &inner));
        assert!(!installer_owns(None, dir));
        assert!(!installer_owns(Some(""), dir));
    }

    /// The real registry path against a volatile key of the test's own: a
    /// copy elsewhere leaves the entry's version alone; the installer's own
    /// folder updates it; an entry without a location is never written.
    #[test]
    fn a_copy_elsewhere_leaves_the_installers_version_alone() {
        use windows_sys::Win32::Foundation::ERROR_SUCCESS;
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_VOLATILE, REG_SZ, RegCloseKey,
            RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegSetValueExW,
        };
        let subkey = format!(r"Software\zeron-test-uninstall-{}", std::process::id());
        let path = wide(&subkey);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: a volatile key of this test's own, deleted below.
        let made = unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, std::ptr::null(), REG_OPTION_VOLATILE, KEY_ALL_ACCESS, std::ptr::null(), &mut key, std::ptr::null_mut())
        };
        assert_eq!(made, ERROR_SUCCESS);
        let set = |name: &str, value: &str| {
            let (name, data) = (wide(name), wide(value));
            // SAFETY: NUL-terminated strings of the lengths given.
            let status = unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
            assert_eq!(status, ERROR_SUCCESS);
        };
        let installed = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        set("InstallLocation", &format!("{}\\", installed.path().display()));
        set("DisplayVersion", "0.2.139-beta.6");

        refresh_installer_version_at(&subkey, elsewhere.path(), "0.2.140");
        assert_eq!(read_string(key, "DisplayVersion").as_deref(), Some("0.2.139-beta.6"));

        refresh_installer_version_at(&subkey, installed.path(), "0.2.140");
        assert_eq!(read_string(key, "DisplayVersion").as_deref(), Some("0.2.140"));

        let name = wide("InstallLocation");
        // SAFETY: our key; a NUL-terminated name.
        assert_eq!(unsafe { RegDeleteValueW(key, name.as_ptr()) }, ERROR_SUCCESS);
        refresh_installer_version_at(&subkey, installed.path(), "0.2.141");
        assert_eq!(read_string(key, "DisplayVersion").as_deref(), Some("0.2.140"));

        // SAFETY: our key, then the tree under our own name.
        unsafe {
            RegCloseKey(key);
            RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr());
        }
    }

    #[test]
    fn portable_install_requires_explicit_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("zeron.exe");
        assert!(!is_managed(&exe));
        std::fs::write(dir.path().join(CONFIG), "{}").unwrap();
        assert!(is_managed(&exe));
        assert!(!is_managed(&dir.path().join("another.exe")));
    }

    #[test]
    fn a_beta_s_version_names_files_and_nothing_else_does() {
        assert!(valid_version("0.2.137"));
        assert!(valid_version("0.2.137-beta.1"));
        for bad in ["", "0.2.", "0.2.137-beta.", "0.2.137-rc.1", r"..\..\evil", "0.2.137-beta.1/x"] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn a_beta_s_files_go_in_by_version_and_old_ones_leave() {
        let install = tempfile::tempdir().unwrap();
        let old = install.path().join(BETA).join("0.2.137-beta.1");
        std::fs::create_dir_all(old.join("ui")).unwrap();
        let stage = tempfile::tempdir().unwrap();
        let staged = stage.path().join("zeron.exe");
        let incoming = stage.path().join(PACKAGE).join(BETA).join("0.2.137-beta.2");
        std::fs::create_dir_all(incoming.join("ui")).unwrap();
        std::fs::write(incoming.join("ui").join("app.luau"), "").unwrap();
        install_version_files(&staged, install.path()).unwrap();
        let new = install.path().join(BETA).join("0.2.137-beta.2");
        assert!(new.join("ui").join("app.luau").is_file());
        assert!(old.is_dir(), "the running version's stays until the next launch");
        remove_version_files(install.path(), Some("0.2.137-beta.2"));
        assert!(!old.exists());
        assert!(new.is_dir());
        // The old app's update brings no package, and there's nothing to move.
        let plain = tempfile::tempdir().unwrap();
        install_version_files(&plain.path().join("zeron.exe"), install.path()).unwrap();
        remove_version_files(install.path(), None);
        assert!(!install.path().join(BETA).exists());
    }

    #[test]
    fn a_stable_release_s_files_replace_a_beta_s() {
        let install = tempfile::tempdir().unwrap();
        let beta = version_files(install.path(), "0.2.139-beta.6");
        std::fs::create_dir_all(beta.join("ui")).unwrap();
        let stage = tempfile::tempdir().unwrap();
        let staged = stage.path().join("zeron.exe");
        let incoming = stage.path().join(PACKAGE).join(BETA).join("0.2.139");
        std::fs::create_dir_all(incoming.join("engine")).unwrap();
        std::fs::create_dir_all(incoming.join("ui")).unwrap();
        std::fs::write(incoming.join("engine").join("zeron.exe"), "").unwrap();
        std::fs::write(incoming.join("ui").join("app.luau"), "").unwrap();
        install_version_files(&staged, install.path()).unwrap();
        let stable = version_files(install.path(), "0.2.139");
        assert!(files_complete(&stable));
        remove_version_files(install.path(), Some("0.2.139"));
        assert!(!beta.exists());
        assert!(stable.is_dir());
        // Leftovers of a removal that didn't finish go too.
        let leftover = install.path().join(BETA).join(".0.2.138.removing-1");
        std::fs::create_dir_all(&leftover).unwrap();
        remove_version_files(install.path(), Some("0.2.139"));
        assert!(!leftover.exists());
    }

    /// A version folder whose engine runs can't be moved, so it isn't touched:
    /// never left with its engine and without its scripts.
    #[test]
    fn a_version_in_use_is_kept_whole() {
        let install = tempfile::tempdir().unwrap();
        let running = version_files(install.path(), "0.2.140");
        std::fs::create_dir_all(running.join("engine")).unwrap();
        std::fs::create_dir_all(running.join("ui")).unwrap();
        std::fs::write(running.join("ui").join("app.luau"), "").unwrap();
        let engine = running.join("engine").join("zeron.exe");
        std::fs::write(&engine, "").unwrap();
        let lock = NoDeleteHandle::open(&engine);
        remove_version_files(install.path(), Some("0.2.139"));
        assert!(running.join("ui").join("app.luau").is_file(), "its scripts stay");
        drop(lock);
        remove_version_files(install.path(), Some("0.2.139"));
        assert!(!running.exists());
    }

    #[test]
    fn only_the_old_app_s_github_feed_moves_to_the_edge() {
        let install = tempfile::tempdir().unwrap();
        let config = install.path().join(CONFIG);
        std::fs::write(&config, format!(r#"{{"releases_url":"{GITHUB_LATEST}/"}}"#)).unwrap();
        leave_github_feed(install.path());
        assert!(std::fs::read_to_string(&config).unwrap().contains(EDGE_FEED));
        std::fs::write(&config, r#"{"releases_url":"https://example.com/feed"}"#).unwrap();
        leave_github_feed(install.path());
        assert!(std::fs::read_to_string(&config).unwrap().contains("example.com"));
    }

    #[test]
    fn only_a_managed_install_without_its_files_is_incomplete() {
        let install = tempfile::tempdir().unwrap();
        let exe = install.path().join("zeron.exe");
        // Not update-managed: a source build, left alone.
        assert_eq!(incomplete_install(&exe, "0.2.139"), None);
        std::fs::write(install.path().join(CONFIG), "{}").unwrap();
        assert_eq!(incomplete_install(&exe, "0.2.139").as_deref(), Some(install.path()));
        // A folder without its engine, or its engine alone, is incomplete.
        let files = version_files(install.path(), "0.2.139");
        std::fs::create_dir_all(files.join("ui")).unwrap();
        assert!(incomplete_install(&exe, "0.2.139").is_some());
        std::fs::create_dir_all(files.join("engine")).unwrap();
        std::fs::write(files.join("engine").join("zeron.exe"), "").unwrap();
        assert!(incomplete_install(&exe, "0.2.139").is_some());
        std::fs::write(files.join("ui").join("app.luau"), "").unwrap();
        assert_eq!(incomplete_install(&exe, "0.2.139"), None);
        // Another version's files don't count.
        assert!(incomplete_install(&exe, "0.2.140").is_some());
    }

    #[test]
    fn changed_staging_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("zeron.exe");
        std::fs::write(&exe, b"original").unwrap();
        std::fs::write(
            dir.path().join("sha256"),
            format!("{:x}", Sha256::digest(b"original")),
        )
        .unwrap();
        verify(&exe).unwrap();
        std::fs::write(&exe, b"changed").unwrap();
        assert!(verify(&exe).is_err());
    }

    #[test]
    fn failed_swap_after_old_image_moved_restores_the_installation() {
        let install = tempfile::tempdir().unwrap();
        let installed = install.path().join("zeron.exe");
        std::fs::write(&installed, b"old image").unwrap();
        let stage = tempfile::tempdir().unwrap();
        let staged = staged_with(stage.path(), b"new image");

        // The incoming file exists (so the handle can bind to it) and is held
        // open without delete sharing: the copy succeeds, the swap's final
        // rename fails after the running image has already been moved aside.
        let incoming = install.path().join(INCOMING);
        std::fs::write(&incoming, b"placeholder").unwrap();
        let lock = NoDeleteHandle::open(&incoming);

        let error = swap_into_place(&staged, &installed).unwrap_err();
        drop(lock);
        assert!(
            error
                .to_string()
                .contains("installing the updated executable"),
            "unexpected error: {error:#}"
        );
        // The launchable installation is the old executable again, and no
        // backup is left dangling.
        assert_eq!(std::fs::read(&installed).unwrap(), b"old image");
        assert!(!install.path().join(BACKUP).exists());

        // With the lock released the same swap succeeds end to end.
        swap_into_place(&staged, &installed).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"new image");
        assert!(!incoming.exists());
        assert!(install.path().join(BACKUP).exists());
    }

    #[test]
    fn corrupt_copy_is_rejected_before_the_installation_moves() {
        let install = tempfile::tempdir().unwrap();
        let installed = install.path().join("zeron.exe");
        std::fs::write(&installed, b"old image").unwrap();
        let stage = tempfile::tempdir().unwrap();
        let staged = staged_with(stage.path(), b"new image");
        // Corrupt the staged bytes after the sidecar was written.
        std::fs::write(&staged, b"tampered").unwrap();

        let error = swap_into_place(&staged, &installed).unwrap_err();
        assert!(
            error.to_string().contains("copying the staged update"),
            "unexpected error: {error:#}"
        );
        assert_eq!(std::fs::read(&installed).unwrap(), b"old image");
        assert!(!install.path().join(BACKUP).exists());
    }

    #[test]
    fn backup_recovers_a_missing_installation_and_clears_when_intact() {
        let install = tempfile::tempdir().unwrap();
        let installed = install.path().join("zeron.exe");
        let backup = install.path().join(BACKUP);
        std::fs::write(&backup, b"survivor").unwrap();

        // Installation missing: the backup is promoted back into place.
        recover_from_backup(&installed).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"survivor");
        assert!(!backup.exists());

        // Installation present: a stale backup is dropped, not applied.
        std::fs::write(&backup, b"stale").unwrap();
        recover_from_backup(&installed).unwrap();
        assert_eq!(std::fs::read(&installed).unwrap(), b"survivor");
        assert!(!backup.exists());
    }

    #[tokio::test]
    async fn missing_checksum_is_rejected_before_download() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = super::super::Manifest {
            version: "1.2.3".into(),
            ..Default::default()
        };
        assert!(
            stage("http://127.0.0.1:1", &manifest, dir.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn corrupt_download_preserves_install_and_removes_staging() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\ncorrupt",
                )
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("zeron.exe");
        std::fs::write(&installed, b"existing installation").unwrap();
        let manifest = super::super::Manifest {
            version: "1.2.3".into(),
            files: [(
                artifact("1.2.3"),
                super::super::FileMeta {
                    sha256: Some(format!("{:x}", Sha256::digest(b"expected"))),
                },
            )]
            .into(),
            ..Default::default()
        };
        let error = stage(&base, &manifest, dir.path()).await.unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("checksum mismatch"));
        assert_eq!(std::fs::read(&installed).unwrap(), b"existing installation");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
