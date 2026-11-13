//! Linux updates that can't strand a box.
//!
//! `app/current` is the only thing that decides which version boots, and it
//! moves exactly once: when the new version has proved it's ready. Until then
//! every start of the service runs the old version, so whatever goes wrong (the
//! new binary exits, hangs, the power goes) the next start lands on a binary
//! that works and knows how to put things back.
//!
//! The steps, each durable before the next, with `update-transaction.json` in
//! the data directory saying how far things got:
//!
//! 1. **Requested.** The running engine checks there's room, stages the release
//!    and records the update before it answers. Then it restarts as usual.
//! 2. The old version starts and takes the data directory before anything has
//!    opened it. It copies the databases ([`take_snapshot`]), records **Trial**,
//!    leaves a watchdog behind and replaces itself with the new binary: the same
//!    process, so the service manager sees nothing change.
//! 3. The new version starts, holds back work it couldn't take back, and proves
//!    it's ready (the engine's `update_trial`).
//! 4. It [commits](commit_trial): `current` moves, then **Updated** is recorded.
//!    A crash between the two is settled by the next start, which finds itself
//!    running as `current`.
//!
//! If the new version stops first, or the watchdog stops it at the deadline, the
//! service manager starts `current`: the old version, which finds **Trial** and
//! knows the trial failed. It records **Restoring**, puts the databases back,
//! records **RolledBack** and carries on. An interrupted restore is finished
//! before anything opens the databases.
//!
//! A reconnect can't tell an update from a rollback, so clients match the update
//! id from `ApplyUpdate` with [`UpdateReport`] in `UpdateStatus`.
//!
//! The other platforms keep the same rule with their own idea of `current`
//! ([`Layout`]). A macOS app's new bundle waits beside the installed one and
//! becomes it in one atomic swap; the engine replaces itself in place, so it is
//! the same process, in the same session, with the same access. Windows can't
//! replace a process, so the old one starts the new exe from beside the installed
//! one and stays on as the watchdog; moving the exe into place is the commit.
//! Neither needs the app to be open.
//!
//! Only the SQLite stores roll back. Files outside them (journals, settings,
//! attachments, the saved session) aren't copied, which is why the trial doesn't
//! touch chats' journals until it has committed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

const FILE: &str = "update-transaction.json";
/// Why a trial is ending, from whoever knows: the trial itself, or the watchdog
/// that's about to stop it.
const NOTE_FILE: &str = "update-trial-note";
const SNAPSHOTS: &str = "update-snapshots";
const SNAPSHOT_MANIFEST: &str = "files.json";

/// The trial's deadline before the databases' size is counted. A crash is seen
/// when it happens; this only bounds a new version that's stuck.
const BASE_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// Migrations scale with the database, and a missed deadline makes the next
/// attempt pay the same cost again.
const DEADLINE_PER_GIB: Duration = Duration::from_secs(60);
const MAX_DEADLINE: Duration = Duration::from_secs(60 * 60);
/// Overrides the deadline (seconds), for testing a stuck trial.
const DEADLINE_ENV: &str = "ZERON_UPDATE_TRIAL_SECS";

/// How much space to leave free on top of what an update needs.
const SPARE_BYTES: u64 = 256 * 1024 * 1024;
/// Unpacking a release takes about three times its download, on top of the
/// download itself.
const UNPACK_FACTOR: u64 = 4;
/// What to assume a release's download weighs when the feed won't say.
pub(crate) const UNKNOWN_ARTIFACT_BYTES: u64 = 300 * 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

// Only Unix runs the watchdog as its own process; on Windows the old version
// stays on as it.
#[cfg(unix)]
const WATCH_ID_ENV: &str = "ZERON_UPDATE_WATCH_ID";
#[cfg(unix)]
const WATCH_PID_ENV: &str = "ZERON_UPDATE_WATCH_PID";
#[cfg(unix)]
const WATCH_DATA_ENV: &str = "ZERON_UPDATE_WATCH_DATA";
#[cfg(unix)]
const WATCH_ARGS_ENV: &str = "ZERON_UPDATE_WATCH_ARGS";

/// The hidden subcommand that runs [`run_watchdog`].
pub const WATCHDOG_SUBCOMMAND: &str = "update-watchdog";

/// How far an update has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Recorded; the next start of the old version begins the trial.
    Requested,
    /// The new version has been started and hasn't committed.
    Trial,
    /// The trial failed and the databases are being put back. Nothing may open
    /// them until that's finished.
    Restoring,
    Updated,
    /// The new version ran and failed; the old one is back, databases and all.
    RolledBack,
    /// The new version never ran.
    Failed,
}

impl Phase {
    fn finished(self) -> bool {
        matches!(self, Self::Updated | Self::RolledBack | Self::Failed)
    }
}

/// How the engine gets started again after it stops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestartMode {
    /// A service manager restarts this unit.
    Service { unit: String },
    /// Nothing does (the desktop app's engine when the app is closed): the
    /// watchdog starts `current` itself.
    Relaunch,
}

/// What "the installed version" is on this device, and how it changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// `app_root/<version>` and a `current` symlink.
    Versioned { app_root: PathBuf },
    /// `bundle` is the installed app; `incoming` is the new one beside it.
    MacBundle { bundle: PathBuf, incoming: PathBuf },
    /// `directory/zeron.exe` is installed; `incoming` is the new exe, in a folder
    /// of its own inside `directory`.
    WindowsFolder { directory: PathBuf, incoming: PathBuf },
}

impl Layout {
    /// Where a release has to fit.
    fn root(&self) -> PathBuf {
        match self {
            Self::Versioned { app_root } => app_root.clone(),
            Self::MacBundle { bundle, .. } => {
                bundle.parent().unwrap_or(bundle).to_path_buf()
            }
            Self::WindowsFolder { directory, .. } => directory.clone(),
        }
    }

    /// The new version's executable called `name`.
    fn new_binary(&self, target: &str, name: &std::ffi::OsStr) -> PathBuf {
        match self {
            Self::Versioned { app_root } => app_root.join(target).join(name),
            Self::MacBundle { incoming, .. } => incoming.join("Contents/MacOS").join(name),
            Self::WindowsFolder { incoming, .. } => incoming.clone(),
        }
    }

    /// The installed executable called `name`.
    fn installed_binary(&self, name: &std::ffi::OsStr) -> PathBuf {
        match self {
            Self::Versioned { app_root } => app_root.join("current").join(name),
            Self::MacBundle { bundle, .. } => bundle.join("Contents/MacOS").join(name),
            Self::WindowsFolder { directory, .. } => directory.join("zeron.exe"),
        }
    }

    /// What the new version needs told, since it's running from beside the
    /// installation rather than from it.
    fn installed_at(&self) -> Option<&Path> {
        match self {
            Self::Versioned { .. } => None,
            Self::MacBundle { bundle, .. } => Some(bundle),
            Self::WindowsFolder { directory, .. } => Some(directory),
        }
    }

    /// The version this executable is, and the one that's installed. `None` when
    /// it isn't part of this installation (a build started by hand beside it).
    fn whoami(&self, transaction: &Transaction) -> Option<Whoami> {
        self.whoami_of(&std::env::current_exe().ok()?, transaction)
    }

    fn whoami_of(&self, exe: &Path, transaction: &Transaction) -> Option<Whoami> {
        let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
        let inside = |dir: &Path| {
            std::fs::canonicalize(dir).is_ok_and(|dir| exe.starts_with(dir))
        };
        match self {
            Self::Versioned { app_root } => {
                let current = std::fs::read_link(app_root.join("current")).ok()?;
                Some(Whoami {
                    me: version_dir(&exe)?,
                    current: current.file_name()?.to_str()?.to_owned(),
                })
            }
            Self::MacBundle { bundle, incoming } => {
                let current = crate::bundle_app_version(bundle)?;
                let me = if inside(incoming) {
                    transaction.target.clone()
                } else if inside(bundle) || became_the_bundle(&exe, bundle, incoming) {
                    current.clone()
                } else {
                    return None;
                };
                Some(Whoami { me, current })
            }
            Self::WindowsFolder { directory, incoming } => {
                #[cfg(windows)]
                let recorded = crate::windows::installed_version(directory);
                #[cfg(not(windows))]
                let recorded: Option<String> = None;
                let current = recorded.unwrap_or_else(|| transaction.from.clone());
                // The zgx app runs its engine from its version's folder beside
                // `zeron.exe` (crates/update `windows::version_files`): the new
                // version's while it's tried, the installed one's after.
                #[cfg(windows)]
                let files = |version: &str| inside(&crate::windows::version_files(directory, version));
                #[cfg(not(windows))]
                let files = |_: &str| false;
                let me = if inside(incoming) || files(&transaction.target) {
                    transaction.target.clone()
                } else if inside(&directory.join("zeron.exe")) || files(&current) {
                    current.clone()
                } else {
                    return None;
                };
                Some(Whoami { me, current })
            }
        }
    }

    /// Make the new version the installed one, in one step that can't be half
    /// done. This is the commit.
    fn commit(&self, target: &str) -> anyhow::Result<()> {
        match self {
            Self::Versioned { app_root } => {
                crate::apply_headless(app_root, target)?;
                sync_dir(app_root).context("saving the new version as current")
            }
            Self::MacBundle { bundle, incoming } => crate::swap_mac_bundles(incoming, bundle),
            #[cfg(windows)]
            Self::WindowsFolder { directory, incoming } => {
                crate::windows::commit_trial(incoming, directory, target)
            }
            #[cfg(not(windows))]
            Self::WindowsFolder { .. } => bail!("not supported on this platform"),
        }
    }

    /// What's left once the update has settled either way. Nothing in use is
    /// deleted: a process whose image is gone is what AppleSystemPolicy rejects
    /// (#1037), and Windows refuses anyway. `abandoned` is the version an update
    /// gave up on, when it did.
    fn tidy(&self, abandoned: Option<&str>) {
        match self {
            Self::Versioned { app_root } => {
                crate::prune_installed_versions(app_root, crate::older_versions_kept(), abandoned);
                crate::strip_cloud_unused_installed(app_root);
            }
            Self::MacBundle { bundle, .. } => crate::prune_retired_mac_bundles(bundle),
            Self::WindowsFolder { incoming, .. } => {
                if let Some(folder) = incoming.parent() {
                    let _ = std::fs::remove_dir_all(folder);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    pub id: String,
    pub from: String,
    pub target: String,
    pub layout: Layout,
    pub phase: Phase,
    pub restart: RestartMode,
    pub requested_at: i64,
    pub deadline_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_started_at: Option<i64>,
    /// The process the trial runs in (it keeps the old version's process).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_pid: Option<u32>,
    /// Why it didn't work, in words for the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// Gone back because the service was stopped mid-trial (a sleep, a restart),
    /// not because the release failed: it's tried again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

/// An update's state, as `UpdateStatus` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    /// The id `ApplyUpdate` answered with.
    pub id: String,
    pub from_version: String,
    pub target_version: String,
    pub state: UpdateState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateState {
    InProgress,
    Updated,
    /// The new version ran and failed. The device is back on `fromVersion`.
    RolledBack,
    /// It never got as far as running the new version. Still on `fromVersion`.
    Failed,
    /// A newer engine's state this client doesn't know.
    #[serde(other)]
    Unknown,
}

impl Transaction {
    pub fn report(&self) -> UpdateReport {
        UpdateReport {
            id: self.id.clone(),
            from_version: self.from.clone(),
            target_version: self.target.clone(),
            state: match self.phase {
                Phase::Requested | Phase::Trial | Phase::Restoring => UpdateState::InProgress,
                Phase::Updated => UpdateState::Updated,
                Phase::RolledBack => UpdateState::RolledBack,
                Phase::Failed => UpdateState::Failed,
            },
            reason: self.reason.clone(),
            finished_at: self.finished_at,
        }
    }

    pub fn in_progress(&self) -> bool {
        !self.phase.finished()
    }

    /// The version this update gave up on, once it has. Not one it went back from
    /// only because it was stopped mid-trial: that's tried again, and its files
    /// spare the download.
    fn abandoned(&self) -> Option<&str> {
        (matches!(self.phase, Phase::RolledBack | Phase::Failed) && !self.interrupted)
            .then_some(self.target.as_str())
    }
}

// ---------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------

/// A crash right here, in a debug build, when the test rig asks for one. Release
/// builds don't have the knob.
fn crash_point(step: &str) {
    if cfg!(debug_assertions)
        && std::env::var("ZERON_UPDATE_CRASH_AT").is_ok_and(|at| at == step)
    {
        eprintln!("zeron-update: crashing at {step}, as asked");
        std::process::abort();
    }
}

/// Make a rename or removal in `dir` survive a power cut.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Replace `path` so that it holds either the old contents or the new, whatever
/// happens: same-directory rename with the file and directory synced.
fn write_durably(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!(".{name}.{}", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        rename_replacing(&temp, path)?;
        sync_dir(dir)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// On Windows a rename over a file fails ("Access is denied") while anything has
/// it open without delete sharing: antivirus, an indexer, a backup tool, or a
/// script reading the record. Those opens are brief, so wait them out.
fn rename_replacing(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempts = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(err)
                if cfg!(windows)
                    && err.kind() == std::io::ErrorKind::PermissionDenied
                    && attempts < 40 =>
            {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            result => return result,
        }
    }
}

/// The record. An unreadable one can't say what happened, so it's set aside and
/// the device boots what `current` says: stopping here would strand the box over
/// a file. Any snapshot stays put for someone to look at.
pub fn read(data_dir: &Path) -> Option<Transaction> {
    let path = data_dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            tracing::error!(error = %err, "couldn't read the update record");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(transaction) => Some(transaction),
        Err(err) => {
            tracing::error!(error = %err, "the update record is unreadable; setting it aside");
            let _ = std::fs::rename(&path, data_dir.join(format!("{FILE}.unreadable")));
            None
        }
    }
}

fn write(data_dir: &Path, transaction: &Transaction) -> anyhow::Result<()> {
    let json = serde_json::to_vec_pretty(transaction)?;
    write_durably(&data_dir.join(FILE), &json).context("saving the update record")
}

/// What `UpdateStatus` says about the latest update.
pub fn report(data_dir: &Path) -> Option<UpdateReport> {
    read(data_dir).map(|transaction| transaction.report())
}

fn finish(
    data_dir: &Path,
    mut transaction: Transaction,
    phase: Phase,
    reason: Option<String>,
) -> anyhow::Result<Transaction> {
    transaction.phase = phase;
    transaction.reason = reason;
    transaction.finished_at = Some(crate::now_ms());
    write(data_dir, &transaction)?;
    let _ = std::fs::remove_file(data_dir.join(NOTE_FILE));
    Ok(transaction)
}

/// Record a request that never reached the trial: the engine couldn't restart.
pub fn fail_request(data_dir: &Path, id: &str, reason: String) {
    let Some(transaction) = read(data_dir).filter(|t| t.id == id && t.phase == Phase::Requested)
    else {
        return;
    };
    if let Err(err) = finish(data_dir, transaction, Phase::Failed, Some(reason)) {
        tracing::error!(error = %err, "couldn't record the failed update");
    }
}

// ---------------------------------------------------------------------------
// Room
// ---------------------------------------------------------------------------

fn free_bytes(path: &Path) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated and `stats` is a valid out-pointer.
        if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(stats.f_bavail as u64 * stats.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available = 0u64;
        // SAFETY: `wide` is NUL-terminated; the out-pointer is valid and the
        // others may be null.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(available)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(std::io::Error::other("not supported on this platform"))
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB as f64)
}

/// `Err` says why not, in words for the person. Pure so it can be tested.
fn room_verdict(free: u64, needed: u64) -> Result<(), String> {
    if free >= needed {
        return Ok(());
    }
    Err(format!(
        "There isn't enough disk space to update safely: {} needed, {} free. Free up some space \
         and try again. Zeron is still running.",
        gib(needed),
        gib(free)
    ))
}

/// Refuse before anything stops: a full disk must cost an update, not an outage.
/// `download` is what the release will weigh in `app_root`'s filesystem, zero
/// once it's staged. A check that can't run refuses too: not knowing isn't room.
pub fn ensure_room(data_dir: &Path, app_root: &Path, download: u64) -> Result<(), String> {
    let cannot = |err: std::io::Error| {
        format!("Zeron couldn't check the free disk space ({err}), so it didn't update.")
    };
    let snapshot = store_bytes(data_dir);
    let snapshot_copy = snapshot + snapshot / 20;
    let needed_for_data = snapshot_copy + SPARE_BYTES;
    room_verdict(free_bytes(data_dir).map_err(cannot)?, needed_for_data)?;
    let release = download.saturating_mul(UNPACK_FACTOR);
    let shared = same_filesystem(data_dir, app_root);
    let needed_for_app = if shared {
        needed_for_data + release
    } else {
        release + SPARE_BYTES
    };
    room_verdict(free_bytes(app_root).map_err(cannot)?, needed_for_app)
}

fn same_filesystem(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (std::fs::metadata(left), std::fs::metadata(right)) {
            (Ok(left), Ok(right)) => left.dev() == right.dev(),
            _ => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        true
    }
}

// ---------------------------------------------------------------------------
// Database snapshot
// ---------------------------------------------------------------------------

fn is_store_file(name: &str) -> bool {
    let base = ["-wal", "-shm", "-journal"]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name);
    base.ends_with(".sqlite3") || base.ends_with(".sqlite")
}

/// Where the engine keeps its SQLite stores: the data directory itself, and each
/// profile's folder. Backups beside them (`docs.sqlite3.before-…`) aren't stores.
fn store_dirs(data_dir: &Path) -> Vec<PathBuf> {
    let subdirs = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut dirs = vec![data_dir.to_path_buf()];
    dirs.extend(subdirs(&data_dir.join("profiles")));
    for org in subdirs(&data_dir.join("orgs")) {
        dirs.extend(subdirs(&org));
    }
    dirs
}

/// The store files as they are now, relative to the data directory.
fn store_files(data_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in store_dirs(data_dir) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let is_file = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_file());
            if is_file
                && is_store_file(&entry.file_name().to_string_lossy())
                && let Ok(relative) = entry.path().strip_prefix(data_dir)
            {
                files.push(relative.to_path_buf());
            }
        }
    }
    files.sort();
    files
}

fn store_bytes(data_dir: &Path) -> u64 {
    store_files(data_dir)
        .iter()
        .filter_map(|file| std::fs::metadata(data_dir.join(file)).ok())
        .map(|meta| meta.len())
        .sum()
}

fn snapshot_dir(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join(SNAPSHOTS).join(id)
}

/// Copy file to file, flushed: on filesystems that can, `std::fs::copy` clones.
fn copy_synced(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to)?;
    std::fs::OpenOptions::new().write(true).open(to)?.sync_all()
}

/// Copy the stores, once per update. The caller holds the data directory and
/// nothing has opened it, so the files (a WAL included) are consistent as they
/// lie. A finished snapshot is never replaced: by the time this could run again
/// a trial may already have changed the live files.
pub(crate) fn take_snapshot(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let done = snapshot_dir(data_dir, id);
    if done.is_dir() {
        return Ok(());
    }
    let root = data_dir.join(SNAPSHOTS);
    std::fs::create_dir_all(&root)?;
    let staging = root.join(format!("{id}.partial"));
    let _ = std::fs::remove_dir_all(&staging);
    let copied = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(&staging)?;
        let files = store_files(data_dir);
        for (index, file) in files.iter().enumerate() {
            copy_synced(&data_dir.join(file), &staging.join("files").join(file))
                .with_context(|| format!("copying {}", file.display()))?;
            if index == 0 {
                crash_point("snapshot-partial");
            }
        }
        write_durably(
            &staging.join(SNAPSHOT_MANIFEST),
            &serde_json::to_vec(&files)?,
        )?;
        sync_dir(&staging)?;
        std::fs::rename(&staging, &done)?;
        sync_dir(&root)?;
        Ok(())
    })();
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    copied
}

/// Put the stores back as the snapshot has them, and remove any store file the
/// trial added (a new WAL, a new database). Each file is replaced in one step and
/// the whole thing can run again from the start, which is what an interrupted
/// restore does. A store reached through a symlink stays one.
fn restore_snapshot(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let snapshot = snapshot_dir(data_dir, id);
    let files: Vec<PathBuf> = serde_json::from_slice(
        &std::fs::read(snapshot.join(SNAPSHOT_MANIFEST)).context("reading the backup's list")?,
    )
    .context("reading the backup's list")?;
    for (index, file) in files.iter().enumerate() {
        let live = data_dir.join(file);
        let target = std::fs::canonicalize(&live).unwrap_or(live);
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let temp = target.with_file_name(format!(".{name}.restoring"));
        copy_synced(&snapshot.join("files").join(file), &temp)
            .with_context(|| format!("restoring {}", file.display()))?;
        std::fs::rename(&temp, &target)?;
        if let Some(parent) = target.parent() {
            sync_dir(parent)?;
        }
        if index == 0 {
            crash_point("restore-partial");
        }
    }
    for file in store_files(data_dir) {
        if !files.contains(&file) {
            let live = data_dir.join(&file);
            std::fs::remove_file(&live).with_context(|| format!("removing {}", file.display()))?;
            if let Some(parent) = live.parent() {
                sync_dir(parent)?;
            }
        }
    }
    Ok(())
}

/// Backups of finished updates, and any left half-made by a crash. Only called
/// once nothing is in progress.
fn discard_snapshots(data_dir: &Path) {
    let root = data_dir.join(SNAPSHOTS);
    if root.exists() && std::fs::remove_dir_all(&root).is_ok() {
        let _ = sync_dir(data_dir);
    }
}

// ---------------------------------------------------------------------------
// Requesting an update (the running engine)
// ---------------------------------------------------------------------------

/// How long the new version gets, by how much it may have to migrate.
fn trial_deadline(store_bytes: u64) -> Duration {
    trial_deadline_from(BASE_DEADLINE, store_bytes)
}

fn trial_deadline_from(base: Duration, store_bytes: u64) -> Duration {
    if let Some(secs) = std::env::var(DEADLINE_ENV)
        .ok()
        .and_then(|value| value.trim().parse().ok())
    {
        return Duration::from_secs(secs);
    }
    let gib = u32::try_from(store_bytes / GIB).unwrap_or(u32::MAX);
    (base + DEADLINE_PER_GIB.saturating_mul(gib)).min(MAX_DEADLINE)
}

pub(crate) fn new_id() -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(crate::now_ms().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let mut random = [0u8; 16];
    if let Ok(mut source) = std::fs::File::open("/dev/urandom") {
        use std::io::Read as _;
        let _ = source.read_exact(&mut random);
    }
    hasher.update(random);
    format!("{:x}", hasher.finalize())[..32].to_owned()
}

/// Record the update, durably, before anyone's told it was accepted. The next
/// start of `from` carries it out. Refuses if one is already under way.
pub fn request(
    data_dir: &Path,
    layout: Layout,
    from: &str,
    target: &str,
    restart: RestartMode,
) -> anyhow::Result<Transaction> {
    request_within(data_dir, layout, from, target, restart, BASE_DEADLINE)
}

/// [`request`], when something is waiting on this update: `base` is the trial's
/// deadline before the databases' size is counted, so a new version that hangs
/// gives way sooner.
pub fn request_within(
    data_dir: &Path,
    layout: Layout,
    from: &str,
    target: &str,
    restart: RestartMode,
    base: Duration,
) -> anyhow::Result<Transaction> {
    if read(data_dir).is_some_and(|existing| existing.in_progress()) {
        bail!("An update is already in progress on this device.");
    }
    if let Err(why) = ensure_room(data_dir, &layout.root(), 0) {
        bail!(why);
    }
    let transaction = Transaction {
        id: new_id(),
        from: from.to_owned(),
        target: target.to_owned(),
        layout,
        phase: Phase::Requested,
        restart,
        requested_at: crate::now_ms(),
        deadline_secs: trial_deadline_from(base, store_bytes(data_dir)).as_secs(),
        trial_started_at: None,
        trial_pid: None,
        reason: None,
        finished_at: None,
        interrupted: false,
    };
    write(data_dir, &transaction)?;
    crash_point("requested");
    Ok(transaction)
}

// ---------------------------------------------------------------------------
// Boot: what this start of the engine has to do first
// ---------------------------------------------------------------------------

/// An engine that was an earlier update's trial runs from that update's
/// incoming bundle (`.Zeron.app.new-…`, beside the installed one), and its
/// commit renamed that folder to the installed bundle: macOS goes on naming
/// the executable by the path it was started from, which no longer exists.
/// That engine *is* the installed version. Without this it counted as a build
/// started by hand beside the installation, so an update it was asked for
/// stopped it and nothing started the new version.
fn became_the_bundle(exe: &Path, bundle: &Path, incoming: &Path) -> bool {
    let (Some(parent), Some(name)) = (bundle.parent(), bundle.file_name()) else {
        return false;
    };
    let prefix = format!(".{}.new-", name.to_string_lossy());
    let parents = [
        parent.to_path_buf(),
        std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()),
    ];
    parents.iter().any(|parent| {
        let Ok(rest) = exe.strip_prefix(parent) else {
            return false;
        };
        let Some(folder) = rest.components().next() else {
            return false;
        };
        let earlier = parent.join(folder);
        folder.as_os_str().to_string_lossy().starts_with(&prefix)
            && earlier != incoming
            && rest.starts_with(Path::new(folder.as_os_str()).join("Contents/MacOS"))
            // Still there: an abandoned download, not the installed bundle.
            && !earlier.exists()
    })
}

/// A path that starts this executable: its own, or — gone because an earlier
/// update's commit renamed the folder it ran from ([`became_the_bundle`]) —
/// the same executable in the installation. The watchdog is started through a
/// shell that can't report a missing program, so a vanished path meant a
/// trial with no watchdog at all.
#[cfg(unix)]
fn runnable_self(layout: &Layout, exe: PathBuf) -> PathBuf {
    if exe.is_file() {
        return exe;
    }
    match exe.file_name() {
        Some(name) => layout.installed_binary(name),
        None => exe,
    }
}

/// Whether [`boot`], run by this process once it has stopped, would start the
/// update recorded in `data_dir` as a trial, rather than leave the record for
/// another start. A background engine nothing restarts asks this before it
/// stops for an update: stopping when it wouldn't strands the device.
pub fn starts_here(data_dir: &Path) -> bool {
    std::env::current_exe().is_ok_and(|exe| starts_from(data_dir, &exe))
}

/// [`starts_here`], for the executable at `exe`.
pub fn starts_from(data_dir: &Path, exe: &Path) -> bool {
    step_from(data_dir, exe) == Some(Step::StartTrial)
}

/// A newer version was installed by other means (the app updated itself)
/// since the update recorded in `data_dir` was asked for, as seen from the
/// executable at `exe`. The next start's [`boot`] settles that record, so a
/// restart to follow the newer version needn't wait for it.
pub fn superseded_from(data_dir: &Path, exe: &Path) -> bool {
    step_from(data_dir, exe) == Some(Step::Superseded)
}

fn step_from(data_dir: &Path, exe: &Path) -> Option<Step> {
    let transaction = read(data_dir)?;
    let who = transaction.layout.whoami_of(exe, &transaction)?;
    Some(step_for(&transaction, &who))
}

/// The version folder this executable runs from, and the one `current` names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Whoami {
    me: String,
    current: String,
}

fn version_dir(path: &Path) -> Option<String> {
    Some(path.parent()?.file_name()?.to_str()?.to_owned())
}

/// What to do about a record, given who's starting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Nothing outstanding.
    Proceed,
    /// The old version's turn: back up and start the new one.
    StartTrial,
    /// The new version, not yet committed: run as the trial.
    RunTrial,
    /// The old version with a trial behind it: put things back.
    Rollback,
    /// The new version as `current`: the commit happened.
    SettleCommitted,
    /// Some third version was installed by other means. Its data is its own
    /// business now, and restoring could lose what it wrote.
    Superseded,
    /// The new version, mid-restore: the old one has to finish first.
    WaitForRestore,
}

fn step_for(transaction: &Transaction, who: &Whoami) -> Step {
    let Whoami { me, current } = who;
    let (from, target) = (&transaction.from, &transaction.target);
    match transaction.phase {
        Phase::Updated | Phase::RolledBack | Phase::Failed => Step::Proceed,
        Phase::Requested if me == from && current == from => Step::StartTrial,
        // Installed by other means after the request; no trial ever started.
        Phase::Requested if me == target && current == target => Step::SettleCommitted,
        Phase::Requested => Step::Superseded,
        Phase::Trial if me == target && current == from => Step::RunTrial,
        Phase::Trial if me == target && current == target => Step::SettleCommitted,
        Phase::Trial if me == from && current == from => Step::Rollback,
        Phase::Trial => Step::Superseded,
        Phase::Restoring if me == from && current == from => Step::Rollback,
        Phase::Restoring if me == target => Step::WaitForRestore,
        Phase::Restoring => Step::Superseded,
    }
}

/// This process's trial, between [`boot`] and [`commit_trial`].
#[derive(Debug, Clone)]
pub struct Trial {
    pub id: String,
    pub target: String,
    data_dir: PathBuf,
    layout: Layout,
    deadline: Instant,
}

impl Trial {
    /// How long is left before the watchdog stops this process.
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

static TRIAL: Mutex<Option<Trial>> = Mutex::new(None);

fn lock_trial() -> std::sync::MutexGuard<'static, Option<Trial>> {
    TRIAL.lock().unwrap_or_else(|err| err.into_inner())
}

/// This process is a new version that hasn't committed: nothing it can't take
/// back (resuming chats, starting queued work) may begin.
pub fn in_trial() -> bool {
    lock_trial().is_some()
}

pub fn current_trial() -> Option<Trial> {
    lock_trial().clone()
}

/// Why this trial is giving up, for the record the old version will write.
pub fn note_trial_failure(data_dir: &Path, why: &str) {
    let _ = write_durably(&data_dir.join(NOTE_FILE), why.as_bytes());
}

/// How an interrupted trial's note starts ([`note_trial_interrupted`]).
const INTERRUPTED_NOTE: &str = "interrupted:";

/// This process, on trial, is being stopped from outside (SIGTERM: the machine
/// going to sleep, the service restarting, a person). That says nothing about the
/// release: the old version still goes back, since nothing committed, but says
/// why, and the release is tried again. Before this, such a stop read as "the new
/// version stopped before it was ready", and a wake never tried the release again.
pub fn note_trial_interrupted() {
    if let Some(trial) = current_trial() {
        note_trial_failure(&trial.data_dir, INTERRUPTED_NOTE);
    }
}

/// This process, on trial, panicked: what it said, for the record (the log alone
/// may be gone by the time anyone looks).
pub fn note_trial_panicked(message: &str) {
    if let Some(trial) = current_trial() {
        let message = message.trim();
        let message: String = message.chars().take(300).collect();
        note_trial_failure(
            &trial.data_dir,
            &format!("The new version crashed while starting ({message})."),
        );
    }
}

/// The new version has proved itself: move `current`, which is the commit, then
/// record it. A crash in between is settled by the next start.
pub fn commit_trial() -> anyhow::Result<()> {
    let Some(trial) = current_trial() else {
        return Ok(());
    };
    crash_point("trial-before-commit");
    trial.layout.commit(&trial.target)?;
    crash_point("commit-flipped");
    settle_committed(&trial.data_dir, &trial.id)?;
    *lock_trial() = None;
    Ok(())
}

fn settle_committed(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    let Some(transaction) = read(data_dir).filter(|t| t.id == id && t.in_progress()) else {
        return Ok(());
    };
    let layout = transaction.layout.clone();
    finish(data_dir, transaction, Phase::Updated, None)?;
    crash_point("commit-recorded");
    discard_snapshots(data_dir);
    layout.tidy(None);
    Ok(())
}

/// Tidy up after the latest update again. At the commit itself the old version's
/// watchdog is still on its way out, and it counts as something running from the
/// old bundle.
pub fn tidy_latest(data_dir: &Path) {
    if let Some(latest) = read(data_dir).filter(|update| !update.in_progress()) {
        latest.layout.tidy(latest.abandoned());
    }
}

/// Options for [`boot`].
pub struct BootOptions {
    /// Arguments that make this executable [`run_watchdog`].
    pub watchdog_args: Vec<OsString>,
    /// Arguments to start this engine again with.
    pub engine_args: Vec<OsString>,
}

impl BootOptions {
    pub fn for_this_process() -> Self {
        Self {
            watchdog_args: vec![WATCHDOG_SUBCOMMAND.into()],
            engine_args: std::env::args_os().skip(1).collect(),
        }
    }
}

/// Run before anything opens the data directory. Settles whatever an earlier
/// start left, and, when it's this start's turn to begin a trial, replaces this
/// process with the new version and doesn't return.
///
/// `hold_data_dir` takes the data directory away from any predecessor for as
/// long as the guard it returns lives. An error means the engine must not start:
/// the databases may be half restored. A service manager will try again.
pub fn boot<G>(
    data_dir: &Path,
    options: &BootOptions,
    hold_data_dir: impl Fn() -> anyhow::Result<G>,
) -> anyhow::Result<()> {
    let Some(transaction) = read(data_dir) else {
        return Ok(());
    };
    if !transaction.in_progress() {
        // Nothing's in progress, so no backup is needed any more, including
        // one a crash left behind. Cheap when there's none.
        discard_snapshots(data_dir);
        transaction.layout.tidy(transaction.abandoned());
        return Ok(());
    }
    let Some(who) = transaction.layout.whoami(&transaction) else {
        // Not running from that installation (a build started by hand beside
        // it): leave the record for the installation to settle.
        return Ok(());
    };
    match step_for(&transaction, &who) {
        Step::Proceed => Ok(()),
        step @ (Step::StartTrial | Step::Rollback) => {
            if step == Step::Rollback && still_being_tried(&transaction) {
                bail!("the new version is still being tried");
            }
            // Two can get here at once: the desktop app sees its engine's port go
            // quiet while that engine is on its way out. Whoever takes the data
            // directory first acts; the other finds it already done.
            let guard = hold_data_dir()?;
            let Some(transaction) = read(data_dir).filter(|t| t.in_progress()) else {
                return Ok(());
            };
            match step_for(&transaction, &who) {
                Step::StartTrial => start_trial(data_dir, options, transaction, guard),
                Step::Rollback if still_being_tried(&transaction) => {
                    bail!("the new version is still being tried")
                }
                Step::Rollback => roll_back(data_dir, transaction),
                _ => bail!("another start is already dealing with this update"),
            }
        }
        Step::RunTrial => {
            tracing::info!(id = %transaction.id, version = %transaction.target, "running as an update trial");
            let started = transaction.trial_started_at.unwrap_or(transaction.requested_at);
            let elapsed = (crate::now_ms() - started).max(0) as u64;
            let deadline = Duration::from_secs(transaction.deadline_secs)
                .saturating_sub(Duration::from_millis(elapsed));
            *lock_trial() = Some(Trial {
                id: transaction.id,
                target: transaction.target,
                data_dir: data_dir.to_path_buf(),
                layout: transaction.layout,
                deadline: Instant::now() + deadline,
            });
            Ok(())
        }
        Step::SettleCommitted => {
            tracing::info!(id = %transaction.id, "update committed");
            settle_committed(data_dir, &transaction.id)
        }
        Step::Superseded => {
            tracing::warn!(
                id = %transaction.id,
                running = %who.me,
                "another version was installed during the update; leaving its data alone"
            );
            let reason = format!(
                "Zeron {} was installed while this update was running, so the update stopped there.",
                who.me
            );
            let done = finish(data_dir, transaction, Phase::Failed, Some(reason))?;
            discard_snapshots(data_dir);
            done.layout.tidy(done.abandoned());
            Ok(())
        }
        Step::WaitForRestore => {
            bail!("the previous version has to finish restoring its data before this one can start")
        }
    }
}

/// Started while the trial is still running (the desktop app seeing its engine's
/// port go quiet, in the moment the data directory is between owners): not a
/// failed trial. Past the deadline it can't be that, so a reused pid can't hold
/// this up for ever.
fn still_being_tried(transaction: &Transaction) -> bool {
    transaction.phase == Phase::Trial && trial_still_running(transaction)
}

fn trial_still_running(transaction: &Transaction) -> bool {
    let Some(pid) = transaction.trial_pid.filter(|pid| *pid != std::process::id()) else {
        return false;
    };
    let started = transaction.trial_started_at.unwrap_or(transaction.requested_at);
    let elapsed = (crate::now_ms() - started).max(0) as u64 / 1000;
    process_running(pid) && elapsed < transaction.deadline_secs
}

/// Back up, leave a watchdog, become the new version.
fn start_trial<G>(
    data_dir: &Path,
    options: &BootOptions,
    mut transaction: Transaction,
    guard: G,
) -> anyhow::Result<()> {
    let give_up = |transaction: Transaction, why: String, cause: String| {
        tracing::error!(%cause, "update stopped before the new version ran");
        let done = finish(data_dir, transaction, Phase::Failed, Some(why))?;
        discard_snapshots(data_dir);
        done.layout.tidy(done.abandoned());
        Ok(())
    };
    let exe_name = std::env::current_exe()?
        .file_name()
        .context("this executable has no name")?
        .to_owned();
    let new_binary = transaction.layout.new_binary(&transaction.target, &exe_name);
    if !new_binary.is_file() {
        let why = format!(
            "Zeron {}'s files are missing, so it stayed on {}.",
            transaction.target, transaction.from
        );
        return give_up(transaction, why, new_binary.display().to_string());
    }
    if let Err(err) = take_snapshot(data_dir, &transaction.id) {
        let why = format!(
            "Zeron couldn't back up its data before updating, so it stayed on {}.",
            transaction.from
        );
        return give_up(transaction, why, format!("{err:#}"));
    }
    crash_point("snapshot-done");
    transaction.phase = Phase::Trial;
    transaction.trial_started_at = Some(crate::now_ms());
    transaction.trial_pid = Some(std::process::id());
    write(data_dir, &transaction)?;
    crash_point("trial-marked");
    if let Err(err) = prepare_watchdog(data_dir, options, &transaction) {
        // Without a watchdog a stuck new version would never be stopped. Nothing
        // has touched the data yet, so there's nothing to restore.
        let why = format!(
            "Zeron couldn't watch over the update, so it stayed on {}.",
            transaction.from
        );
        return give_up(transaction, why, format!("{err:#}"));
    }
    crash_point("watchdog-started");
    drop(guard);
    tracing::info!(version = %transaction.target, "starting the new version as a trial");
    let err = hand_over(data_dir, options, &mut transaction, &new_binary);
    // Only reached if the new binary couldn't be started at all.
    let why = format!(
        "Zeron {} couldn't start here, so it stayed on {}.",
        transaction.target, transaction.from
    );
    give_up(transaction, why, err.to_string())
}

/// Become the new version: the same process, so that a service manager, and
/// macOS's idea of who this is, see nothing change. Returns only on failure.
#[cfg(unix)]
fn hand_over(
    _data_dir: &Path,
    options: &BootOptions,
    transaction: &mut Transaction,
    binary: &Path,
) -> std::io::Error {
    use std::os::unix::process::CommandExt as _;
    let mut command = std::process::Command::new(binary);
    command.args(&options.engine_args);
    if let Some(installed) = transaction.layout.installed_at() {
        command.env(crate::INSTALLED_AT_ENV, installed);
    }
    command.exec()
}

/// Windows can't replace a process: start the new version, and stay on as its
/// watchdog until it's settled. Returns only on failure to start it.
#[cfg(windows)]
fn hand_over(
    data_dir: &Path,
    options: &BootOptions,
    transaction: &mut Transaction,
    binary: &Path,
) -> std::io::Error {
    let mut command = std::process::Command::new(binary);
    command.args(&options.engine_args);
    if let Some(installed) = transaction.layout.installed_at() {
        command.env(crate::INSTALLED_AT_ENV, installed);
    }
    let child = match spawn_detached(&mut command) {
        Ok(child) => child,
        Err(err) => return err,
    };
    transaction.trial_pid = Some(child.id());
    if let Err(err) = write(data_dir, transaction) {
        tracing::error!(error = %format!("{err:#}"), "couldn't record the trial's process");
    }
    let args = options
        .engine_args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let exe_name = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(ToOwned::to_owned))
        .unwrap_or_default();
    watch(data_dir, &transaction.id, child.id(), &args, &exe_name);
    std::process::exit(0)
}

#[cfg(not(any(unix, windows)))]
fn hand_over(
    _data_dir: &Path,
    _options: &BootOptions,
    _transaction: &mut Transaction,
    _binary: &Path,
) -> std::io::Error {
    std::io::Error::other("not supported on this platform")
}

fn roll_back(data_dir: &Path, mut transaction: Transaction) -> anyhow::Result<()> {
    let note = std::fs::read_to_string(data_dir.join(NOTE_FILE))
        .ok()
        .filter(|note| !note.trim().is_empty());
    // A restore that was itself interrupted keeps what the first pass decided.
    if note.as_deref().is_some_and(|note| note.starts_with(INTERRUPTED_NOTE)) {
        transaction.interrupted = true;
    }
    let why = if transaction.interrupted {
        "Zeron was stopped while it was trying the new version (the machine went to sleep, or \
         the service restarted)."
            .to_owned()
    } else {
        note.or_else(|| transaction.reason.clone())
            .unwrap_or_else(|| "The new version stopped before it was ready.".to_owned())
    };
    tracing::warn!(id = %transaction.id, %why, "update failed; going back");
    if !snapshot_dir(data_dir, &transaction.id).is_dir() {
        // Trial is only recorded once the snapshot is done, so this is someone's
        // cleanup, not a state this code leaves. Refusing would strand the box.
        tracing::error!("the update's backup is gone; carrying on with the data as it is");
    } else {
        transaction.phase = Phase::Restoring;
        transaction.reason = Some(why.clone());
        write(data_dir, &transaction)?;
        crash_point("restore-marked");
        restore_snapshot(data_dir, &transaction.id).context("restoring the databases")?;
        crash_point("restored");
    }
    let from = transaction.from.clone();
    let target = transaction.target.clone();
    let outcome = if transaction.interrupted {
        format!("{why} It's on {from} for now and will try {target} again.")
    } else {
        format!("{why} Zeron went back to {from} instead of {target}.")
    };
    let done = finish(data_dir, transaction, Phase::RolledBack, Some(outcome))?;
    crash_point("rollback-recorded");
    discard_snapshots(data_dir);
    done.layout.tidy(done.abandoned());
    Ok(())
}

// ---------------------------------------------------------------------------
// The watchdog
// ---------------------------------------------------------------------------

/// Start the watchdog from the old version's executable. It's a sibling rather
/// than a child: the new version takes over this process and would never collect
/// it, so it's left to the service manager to reap.
#[cfg(unix)]
fn prepare_watchdog(
    data_dir: &Path,
    options: &BootOptions,
    transaction: &Transaction,
) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt as _;
    let exe = runnable_self(&transaction.layout, std::env::current_exe()?);
    let args: Vec<String> = options
        .engine_args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let mut command = std::process::Command::new("/bin/sh");
    // A new version that exits in milliseconds has the service manager stopping
    // everything in the unit before the watchdog can set up its handler. Held
    // blocked from here, the signal waits for it rather than killing it. Blocked
    // signals survive fork and exec; `run_watchdog` unblocks it.
    // SAFETY: sigemptyset, sigaddset and sigprocmask are async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            let mut term: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut term);
            libc::sigaddset(&mut term, libc::SIGTERM);
            if libc::sigprocmask(libc::SIG_BLOCK, &term, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let status = command
        .arg("-c")
        .arg(r#""$@" </dev/null & exit 0"#)
        .arg("zeron-update-watch")
        .arg(&exe)
        .args(&options.watchdog_args)
        .env(WATCH_ID_ENV, &transaction.id)
        .env(WATCH_PID_ENV, std::process::id().to_string())
        .env(WATCH_DATA_ENV, data_dir)
        .env(WATCH_ARGS_ENV, serde_json::to_string(&args)?)
        .stdin(std::process::Stdio::null())
        .status()
        .context("starting the update watchdog")?;
    anyhow::ensure!(status.success(), "the update watchdog didn't start: {status}");
    Ok(())
}

/// On Windows this process stays on as the watchdog (`hand_over`).
#[cfg(not(unix))]
fn prepare_watchdog(
    _data_dir: &Path,
    _options: &BootOptions,
    _transaction: &Transaction,
) -> anyhow::Result<()> {
    Ok(())
}

/// The hidden subcommand's entry: keep watching the new version.
#[cfg(unix)]
pub fn run_watchdog() -> anyhow::Result<()> {
    extern "C" fn on_term(_: libc::c_int) {
        TERMINATED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    // SAFETY: the handler only stores to an atomic; the rest are plain calls on
    // a local set.
    unsafe {
        libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
        // Blocked by whoever started this (`prepare_watchdog`), so that one sent
        // early waited for the handler above.
        let mut term: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut term);
        libc::sigaddset(&mut term, libc::SIGTERM);
        libc::sigprocmask(libc::SIG_UNBLOCK, &term, std::ptr::null_mut());
    }
    let id = std::env::var(WATCH_ID_ENV).context("no update to watch")?;
    let pid: u32 = std::env::var(WATCH_PID_ENV)
        .context("no process to watch")?
        .parse()
        .context("no process to watch")?;
    let data_dir = PathBuf::from(std::env::var_os(WATCH_DATA_ENV).context("no data directory")?);
    let engine_args: Vec<String> =
        serde_json::from_str(&std::env::var(WATCH_ARGS_ENV).unwrap_or_else(|_| "[]".into()))
            .unwrap_or_default();
    let exe_name = std::env::current_exe()?
        .file_name()
        .context("this executable has no name")?
        .to_owned();
    watch(&data_dir, &id, pid, &engine_args, &exe_name);
    Ok(())
}

#[cfg(not(unix))]
pub fn run_watchdog() -> anyhow::Result<()> {
    bail!("not used on this platform")
}

static TERMINATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Until the trial settles or the process is gone: stop it at the deadline, and
/// see to it that the old version gets started once it's gone.
fn watch(data_dir: &Path, id: &str, pid: u32, engine_args: &[String], exe_name: &std::ffi::OsStr) {
    use std::sync::atomic::Ordering;
    const TICK: Duration = Duration::from_millis(250);
    /// How long a dead trial's service manager gets to act on its own.
    const MANAGER_GRACE: Duration = Duration::from_secs(15);
    let mut gone_since: Option<Instant> = None;
    loop {
        let Some(transaction) = read(data_dir).filter(|t| t.id == id && t.phase == Phase::Trial)
        else {
            return;
        };
        let alive = process_running(pid);
        let terminated = TERMINATED.load(Ordering::SeqCst);
        if terminated && alive {
            // Someone stopped the service, trial and all: that's theirs to
            // decide. Its next start finds the trial unfinished and goes back.
            return;
        }
        if alive {
            let started = transaction.trial_started_at.unwrap_or(transaction.requested_at);
            let elapsed = (crate::now_ms() - started).max(0) as u64 / 1000;
            if elapsed >= transaction.deadline_secs {
                let minutes = (transaction.deadline_secs / 60).max(1);
                note_trial_failure(
                    data_dir,
                    &format!("The new version didn't finish starting within {minutes} min."),
                );
                tracing::error!(pid, "update trial missed its deadline; stopping it");
                eprintln!("zeron-update: the update trial missed its deadline; stopping it");
                kill_process(pid);
            }
        } else {
            let since = *gone_since.get_or_insert_with(Instant::now);
            match &transaction.restart {
                // A restart-on-failure unit stays down after a clean exit, and
                // stopping it is what brings us this SIGTERM. Ask for a start,
                // which joins any restart already on its way, unless a person's
                // stop is why the trial is gone: its SIGTERM can kill a stuck
                // trial before this loop looks.
                RestartMode::Service { unit } => {
                    if terminated || since.elapsed() >= MANAGER_GRACE {
                        if !unit_has_job(unit) {
                            start_unit(unit);
                        }
                        return;
                    }
                }
                RestartMode::Relaunch => {
                    if since.elapsed() >= Duration::from_secs(1) {
                        relaunch_installed(&transaction.layout, exe_name, engine_args);
                        return;
                    }
                }
            }
        }
        std::thread::sleep(TICK);
    }
}

#[cfg(unix)]
fn process_running(pid: u32) -> bool {
    let pid = pid as libc::pid_t;
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    // A zombie has stopped; only its parent hasn't noticed yet.
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some(state) = stat.rsplit(") ").next().and_then(|rest| rest.chars().next())
    {
        return state != 'Z';
    }
    #[cfg(target_os = "macos")]
    {
        const SZOMB: u32 = 5;
        // SAFETY: an all-zero proc_bsdinfo is valid, and the call writes at most
        // the size it's given.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                // Non-zero: answer for a zombie too.
                1,
                (&raw mut info).cast(),
                size,
            )
        };
        if read == size {
            return info.pbi_status != SZOMB;
        }
    }
    true
}

#[cfg(windows)]
fn process_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the handle is checked before use and closed once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        read != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(any(unix, windows)))]
fn process_running(_pid: u32) -> bool {
    false
}

/// Not a polite stop: it's stuck, and a clean exit wouldn't count as a failure
/// to a service manager that restarts on failure.
#[cfg(unix)]
fn kill_process(pid: u32) {
    // SAFETY: plain syscall on a pid we were given.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: the handle is checked before use and closed once.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn kill_process(_pid: u32) {}

/// Whether something asked the service manager to act on this unit: a stop or
/// restart is a queued job, a unit stopping because its process exited isn't.
fn unit_has_job(unit: &str) -> bool {
    crate::systemctl_query(unit)
        .args(["show", unit, "-p", "Job", "--value"])
        .output()
        .is_ok_and(|output| {
            let job = String::from_utf8_lossy(&output.stdout);
            !matches!(job.trim(), "" | "0")
        })
}

fn start_unit(unit: &str) {
    let started = crate::systemctl_action(unit)
        .args(["--no-block", "start", unit])
        .status();
    if !started.is_ok_and(|status| status.success()) {
        eprintln!("zeron-update: couldn't ask the service manager to start {unit}");
    }
}

/// Start the old version, which will put things back. It may lose a race for the
/// data directory with the desktop app starting it too, which is just as good.
fn relaunch_installed(layout: &Layout, exe_name: &std::ffi::OsStr, engine_args: &[String]) {
    let mut command = std::process::Command::new(layout.installed_binary(exe_name));
    command.args(engine_args).stdin(std::process::Stdio::null());
    if let Err(err) = spawn_detached(&mut command) {
        eprintln!("zeron-update: couldn't start the previous version: {err}");
    }
}

/// Started to outlive whoever started it.
#[cfg(unix)]
fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0).spawn()
}

#[cfg(windows)]
fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt as _;
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Out of the app's job too, when the job allows it (as the engine's own start).
    let detached = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    command.creation_flags(detached | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        Ok(child) => Ok(child),
        Err(_) => {
            command.creation_flags(detached);
            command.spawn()
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn spawn_detached(command: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(me: &str, current: &str) -> Whoami {
        Whoami {
            me: me.into(),
            current: current.into(),
        }
    }

    fn transaction(phase: Phase) -> Transaction {
        Transaction {
            id: "abc".into(),
            from: "1.0.0".into(),
            target: "2.0.0".into(),
            layout: Layout::Versioned { app_root: PathBuf::from("/nowhere") },
            phase,
            restart: RestartMode::Relaunch,
            requested_at: 1,
            deadline_secs: 60,
            trial_started_at: None,
            trial_pid: None,
            reason: None,
            finished_at: None,
            interrupted: false,
        }
    }

    /// `current` only moves at the commit, so who's starting says what happened.
    #[test]
    fn who_is_starting_says_what_happened() {
        use Phase::*;
        let (old, new, other) = ("1.0.0", "2.0.0", "3.0.0");
        for (phase, me, current, expected) in [
            (Requested, old, old, Step::StartTrial),
            (Requested, new, new, Step::SettleCommitted),
            (Requested, other, other, Step::Superseded),
            (Trial, new, old, Step::RunTrial),
            (Trial, new, new, Step::SettleCommitted),
            (Trial, old, old, Step::Rollback),
            (Trial, other, other, Step::Superseded),
            (Restoring, old, old, Step::Rollback),
            (Restoring, new, old, Step::WaitForRestore),
            (Restoring, other, other, Step::Superseded),
            (Updated, new, new, Step::Proceed),
            (RolledBack, old, old, Step::Proceed),
            (Failed, old, old, Step::Proceed),
        ] {
            assert_eq!(
                step_for(&transaction(phase), &who(me, current)),
                expected,
                "{phase:?} me={me} current={current}"
            );
        }
    }

    fn data_dir_with_stores() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("orgs/org_1/user_1");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(dir.path().join("profiles/local")).unwrap();
        std::fs::create_dir_all(dir.path().join("worktrees/repo")).unwrap();
        std::fs::write(profile.join("docs.sqlite3"), "old docs").unwrap();
        std::fs::write(profile.join("docs.sqlite3-wal"), "old wal").unwrap();
        std::fs::write(profile.join("docs.sqlite3.before-cleanup"), "a backup").unwrap();
        std::fs::write(dir.path().join("profiles/local/docs.sqlite3"), "local").unwrap();
        std::fs::write(dir.path().join("pull-requests.sqlite3"), "prs").unwrap();
        std::fs::write(dir.path().join("worktrees/repo/app.sqlite3"), "theirs").unwrap();
        dir
    }

    /// A reader that doesn't share delete access (as antivirus may) only delays
    /// the record's replacement.
    #[cfg(windows)]
    #[test]
    fn a_brief_reader_does_not_fail_the_record() {
        use std::os::windows::fs::OpenOptionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, "old").unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ only
            .open(&path)
            .unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(reader);
        });
        write_durably(&path, b"new").unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn only_the_engines_stores_are_backed_up() {
        let dir = data_dir_with_stores();
        let mut files: Vec<String> = store_files(dir.path())
            .iter()
            .map(|file| file.display().to_string().replace('\\', "/"))
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "orgs/org_1/user_1/docs.sqlite3",
                "orgs/org_1/user_1/docs.sqlite3-wal",
                "profiles/local/docs.sqlite3",
                "pull-requests.sqlite3",
            ]
        );
    }

    #[test]
    fn a_restore_undoes_everything_a_trial_did_to_the_stores() {
        let dir = data_dir_with_stores();
        let profile = dir.path().join("orgs/org_1/user_1");
        take_snapshot(dir.path(), "abc").unwrap();

        std::fs::write(profile.join("docs.sqlite3"), "migrated").unwrap();
        std::fs::remove_file(profile.join("docs.sqlite3-wal")).unwrap();
        std::fs::write(profile.join("docs.sqlite3-shm"), "a new shm").unwrap();
        std::fs::write(dir.path().join("brand-new.sqlite3"), "a new store").unwrap();

        // Running it twice is what a crash mid-restore does.
        restore_snapshot(dir.path(), "abc").unwrap();
        restore_snapshot(dir.path(), "abc").unwrap();

        assert_eq!(std::fs::read_to_string(profile.join("docs.sqlite3")).unwrap(), "old docs");
        assert_eq!(std::fs::read_to_string(profile.join("docs.sqlite3-wal")).unwrap(), "old wal");
        assert!(!profile.join("docs.sqlite3-shm").exists());
        assert!(!dir.path().join("brand-new.sqlite3").exists());
        // Files that aren't stores were never part of it.
        assert_eq!(
            std::fs::read_to_string(profile.join("docs.sqlite3.before-cleanup")).unwrap(),
            "a backup"
        );
    }

    /// A retry after a crash must not back up what the trial already changed.
    #[test]
    fn a_finished_snapshot_is_never_replaced() {
        let dir = data_dir_with_stores();
        take_snapshot(dir.path(), "abc").unwrap();
        let docs = dir.path().join("pull-requests.sqlite3");
        std::fs::write(&docs, "changed by the trial").unwrap();
        take_snapshot(dir.path(), "abc").unwrap();
        restore_snapshot(dir.path(), "abc").unwrap();
        assert_eq!(std::fs::read_to_string(&docs).unwrap(), "prs");
    }

    #[cfg(unix)]
    #[test]
    fn a_store_behind_a_symlink_stays_a_symlink() {
        let dir = data_dir_with_stores();
        let elsewhere = tempfile::tempdir().unwrap();
        let real = elsewhere.path().join("real.sqlite3");
        std::fs::write(&real, "before").unwrap();
        let link = dir.path().join("linked.sqlite3");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        take_snapshot(dir.path(), "abc").unwrap();
        std::fs::write(&real, "after").unwrap();
        restore_snapshot(dir.path(), "abc").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "before");
    }

    #[test]
    fn too_little_room_is_refused_in_plain_words() {
        assert_eq!(room_verdict(10, 10), Ok(()));
        let why = room_verdict(GIB, 3 * GIB).unwrap_err();
        assert!(why.contains("3.0 GB needed"), "{why}");
        assert!(why.contains("1.0 GB free"), "{why}");
        assert!(why.contains("still running"), "{why}");
    }

    /// Against the real free space, so the numbers only have to be bigger than
    /// any disk.
    #[cfg(unix)]
    #[test]
    fn room_counts_the_download_and_the_databases() {
        let dir = data_dir_with_stores();
        let app_root = dir.path().join("app");
        std::fs::create_dir_all(&app_root).unwrap();
        ensure_room(dir.path(), &app_root, 0).unwrap();
        let why = ensure_room(dir.path(), &app_root, u64::MAX / 8).unwrap_err();
        assert!(why.contains("disk space"), "{why}");
    }

    #[test]
    fn the_deadline_grows_with_the_data_and_has_a_ceiling() {
        if std::env::var_os(DEADLINE_ENV).is_some() {
            return;
        }
        assert_eq!(trial_deadline(0), BASE_DEADLINE);
        assert_eq!(trial_deadline(5 * GIB), BASE_DEADLINE + 5 * DEADLINE_PER_GIB);
        assert_eq!(trial_deadline(u64::MAX), MAX_DEADLINE);
    }

    #[test]
    fn the_record_survives_a_round_trip_and_a_bad_one_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()), None);
        let record = transaction(Phase::Trial);
        write(dir.path(), &record).unwrap();
        assert_eq!(read(dir.path()), Some(record));

        std::fs::write(dir.path().join(FILE), "not json").unwrap();
        assert_eq!(read(dir.path()), None);
        assert!(!dir.path().join(FILE).exists());
        assert!(dir.path().join(format!("{FILE}.unreadable")).exists());
    }

    #[test]
    fn only_one_update_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let app_root = dir.path().join("app");
        std::fs::create_dir_all(&app_root).unwrap();
        let first = request(dir.path(), Layout::Versioned { app_root: app_root.clone() }, "1.0.0", "2.0.0", RestartMode::Relaunch).unwrap();
        let again = request(dir.path(), Layout::Versioned { app_root: app_root.clone() }, "1.0.0", "2.0.0", RestartMode::Relaunch);
        assert!(again.unwrap_err().to_string().contains("already in progress"));

        fail_request(dir.path(), &first.id, "couldn't restart".into());
        let failed = read(dir.path()).unwrap();
        assert_eq!(failed.phase, Phase::Failed);
        assert_eq!(failed.report().state, UpdateState::Failed);
        let next = request(dir.path(), Layout::Versioned { app_root: app_root.clone() }, "1.0.0", "2.0.0", RestartMode::Relaunch).unwrap();
        assert_ne!(next.id, first.id);
    }

    /// A trial that died under a parent that hasn't noticed still has a pid, and
    /// must not count as running.
    #[cfg(unix)]
    #[test]
    fn a_zombie_doesnt_count_as_running() {
        let mut child = std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        let waited = std::time::Instant::now();
        while process_running(pid) {
            assert!(waited.elapsed() < Duration::from_secs(5), "still counted as running");
            std::thread::sleep(Duration::from_millis(20));
        }
        child.wait().unwrap();
    }

    #[test]
    fn the_report_names_its_states_for_clients() {
        let report = transaction(Phase::Restoring).report();
        assert_eq!(report.state, UpdateState::InProgress);
        let value = serde_json::to_value(transaction(Phase::RolledBack).report()).unwrap();
        assert_eq!(value["state"], "rolledBack");
        assert_eq!(value["fromVersion"], "1.0.0");
        assert_eq!(value["targetVersion"], "2.0.0");
        let unknown: UpdateReport = serde_json::from_value(serde_json::json!({
            "id": "x", "fromVersion": "1", "targetVersion": "2", "state": "somethingNew",
        }))
        .unwrap();
        assert_eq!(unknown.state, UpdateState::Unknown);
    }

    /// `/Applications`-like folder with the installed `Zeron.app` at
    /// `version`, and an update to 2.0.0 requested into `.Zeron.app.new-next`.
    fn mac_install(version: &str) -> (tempfile::TempDir, Transaction) {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("Zeron.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            format!("<plist><key>ZeronVersion</key><string>{version}</string></plist>"),
        )
        .unwrap();
        std::fs::write(bundle.join("Contents/MacOS/Zeron Engine"), "").unwrap();
        let incoming = root.path().join(".Zeron.app.new-next");
        std::fs::create_dir_all(incoming.join("Contents/MacOS")).unwrap();
        let mut update = transaction(Phase::Requested);
        update.layout = Layout::MacBundle { bundle, incoming };
        (root, update)
    }

    /// m1-laptop, Nov 11: the engine had been the trial of the update before
    /// (started from `.Zeron.app.new-<id>`, renamed to `Zeron.app` at its
    /// commit) and was still named by that path. Asked for the next update,
    /// it didn't recognise its own installation, stopped, and never came back.
    #[test]
    fn an_engine_from_an_earlier_update_is_the_installed_version() {
        let (root, update) = mac_install("1.0.0");
        let installed = root.path().join("Zeron.app/Contents/MacOS/Zeron Engine");
        let earlier = root.path().join(".Zeron.app.new-earlier/Contents/MacOS/Zeron Engine");
        for exe in [&installed, &earlier] {
            let who = update.layout.whoami_of(exe, &update);
            assert_eq!(who, Some(self::who("1.0.0", "1.0.0")), "{}", exe.display());
            assert_eq!(step_for(&update, &who.unwrap()), Step::StartTrial);
        }
        // The new version, on trial from the incoming bundle.
        let trial = root.path().join(".Zeron.app.new-next/Contents/MacOS/Zeron Engine");
        std::fs::write(&trial, "").unwrap();
        assert_eq!(update.layout.whoami_of(&trial, &update), Some(who("2.0.0", "1.0.0")));
    }

    /// The zgx app on Windows runs its engine from its version's folder
    /// (`app\<version>\engine\zeron.exe`): there it's the new version on
    /// trial, or the installed one asked for the next update.
    #[cfg(windows)]
    #[test]
    fn a_zgx_engine_in_its_version_s_folder_is_part_of_the_installation() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().to_path_buf();
        std::fs::write(directory.join("zeron.exe"), "").unwrap();
        std::fs::write(directory.join("zeron-version"), "1.0.0").unwrap();
        let incoming = directory.join(".zeron-update-x").join("zeron.exe");
        std::fs::create_dir_all(incoming.parent().unwrap()).unwrap();
        std::fs::write(&incoming, "").unwrap();
        let engine = |version: &str| {
            let exe = crate::windows::version_files(&directory, version).join("engine").join("zeron.exe");
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, "").unwrap();
            exe
        };
        let mut update = transaction(Phase::Requested);
        update.layout = Layout::WindowsFolder { directory: directory.clone(), incoming: incoming.clone() };
        assert_eq!(update.layout.whoami_of(&engine("1.0.0"), &update), Some(who("1.0.0", "1.0.0")));
        assert_eq!(update.layout.whoami_of(&directory.join("zeron.exe"), &update), Some(who("1.0.0", "1.0.0")));
        assert_eq!(update.layout.whoami_of(&engine("2.0.0"), &update), Some(who("2.0.0", "1.0.0")));
        assert_eq!(update.layout.whoami_of(&incoming, &update), Some(who("2.0.0", "1.0.0")));
        assert_eq!(update.layout.whoami_of(&engine("3.0.0"), &update), None);
    }

    /// What isn't the installation stays out of its update: a build beside
    /// it, an incoming bundle that's still there (an abandoned download), and
    /// a folder that only looks like one.
    #[test]
    fn only_the_installation_is_recognised() {
        let (root, update) = mac_install("1.0.0");
        let abandoned = root.path().join(".Zeron.app.new-abandoned");
        std::fs::create_dir_all(abandoned.join("Contents/MacOS")).unwrap();
        for exe in [
            root.path().join("build/Zeron Engine"),
            abandoned.join("Contents/MacOS/Zeron Engine"),
            root.path().join(".Other.app.new-x/Contents/MacOS/Zeron Engine"),
            root.path().join(".Zeron.app.new-x/Zeron Engine"),
        ] {
            assert_eq!(update.layout.whoami_of(&exe, &update), None, "{}", exe.display());
        }
    }

    /// The question a background engine asks before it stops for an update:
    /// would it start the trial itself once stopped.
    #[test]
    fn an_engine_knows_whether_it_carries_out_the_update() {
        let (root, update) = mac_install("1.0.0");
        let data = tempfile::tempdir().unwrap();
        let earlier = root.path().join(".Zeron.app.new-earlier/Contents/MacOS/Zeron Engine");
        let beside = root.path().join("build/Zeron Engine");
        assert!(!starts_from(data.path(), &earlier), "no update requested");
        write(data.path(), &update).unwrap();
        assert!(starts_from(data.path(), &earlier));
        assert!(!starts_from(data.path(), &beside), "a build beside the app leaves it alone");
        assert!(!superseded_from(data.path(), &earlier));
        // The app installed 3.0.0 by itself meanwhile.
        let Layout::MacBundle { bundle, .. } = &update.layout else { unreachable!() };
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            "<plist><key>ZeronVersion</key><string>3.0.0</string></plist>",
        )
        .unwrap();
        assert!(!starts_from(data.path(), &earlier));
        assert!(superseded_from(data.path(), &earlier));
        assert!(!superseded_from(data.path(), &beside));
        let mut done = update.clone();
        done.phase = Phase::Failed;
        write(data.path(), &done).unwrap();
        assert!(!starts_from(data.path(), &earlier), "nothing in progress");
    }
}
