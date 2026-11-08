//! A Cloud machine's disk (docs/design/cloud-device.md, "Disk"): what it
//! reports with each heartbeat, and the rebuildable caches it clears before a
//! sleep, so that a machine can still fit its size when it comes back.
//!
//! The cleanup deletes from an allow-list and nothing else: Cargo's own
//! `incremental` and `deps` under a directory Cargo marked as its target,
//! named package-manager download caches in the home directory, and Codex's
//! own downloads in the managed `CODEX_HOME` (every sleep: they cost every
//! wake, and Codex fetches them again itself). It never reads
//! a tracked file, untracked source, a login (`~/.claude`, whose
//! `.oauth_refresh.lock` a parked Claude child may hold) or any process. It is
//! bounded by a deadline, and whatever goes wrong the sleep carries on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const GIB: f64 = (1u64 << 30) as f64;

/// Mirrors `DISK_WARN` (edge/src/cloud/policy.ts).
const WARN_SHARE: f64 = 0.8;

/// A cleanup's whole budget: inside the 60 s the edge gives `zeron quiesce`.
pub const CLEANUP_BUDGET: Duration = Duration::from_secs(25);

/// How deep under the checkout a Cargo target directory is looked for.
const TARGET_SEARCH_DEPTH: usize = 4;

/// What a rename leaves until its deletion finishes (it may run out of time).
const REMOVING_SUFFIX: &str = ".zeron-removing";

/// Download caches, relative to the home directory: everything in them is
/// fetched again on demand, and none holds anything built here.
const PACKAGE_CACHES: &[&str] = &[
    ".cargo/registry/cache",
    ".cargo/registry/src",
    ".npm/_cacache",
    ".cache/pip",
    ".cache/uv",
    ".cache/yarn",
    ".cache/pnpm",
    ".local/share/pnpm/store",
    ".cache/go-build",
    ".bun/install/cache",
];

/// Codex's own downloads, relative to the managed `CODEX_HOME`
/// (`credentials::managed_codex_home`): the plugin catalog (about 60 MB), the
/// account's plugin bundles (about 30 MB) and the apps tool list. Every byte
/// on the disk is copied back in on every wake, beside the engine that's
/// starting; Codex fetches these again itself, in the background, once it
/// runs. None holds a login (`auth.json` sits beside them), a session, a
/// database or a setting.
const CODEX_CACHES: &[&str] = &["cache", "plugins/cache"];

/// Disk space, as `df` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// Disk space of the filesystem holding `path`.
#[cfg(unix)]
pub fn usage_of(path: &Path) -> Option<DiskUsage> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and outlives the call; `stat` is only
    // read after statvfs reports it filled.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    let unit = stat.f_frsize as u64;
    let total = (stat.f_blocks as u64).saturating_mul(unit);
    let free = (stat.f_bfree as u64).saturating_mul(unit);
    Some(DiskUsage {
        used_bytes: total.saturating_sub(free),
        total_bytes: total,
    })
}

#[cfg(not(unix))]
pub fn usage_of(_path: &Path) -> Option<DiskUsage> {
    None
}

/// This machine's disk: of its checkout when that exists yet, else its home.
pub fn machine_usage() -> Option<DiskUsage> {
    let checkout = crate::runner::session().and_then(|session| session.path.clone());
    checkout
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(home_dir)
        .and_then(|path| usage_of(&path))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// What this machine's size holds, as the edge last said (GiB, as `f64` bits;
/// 0 = not told yet).
static CAPACITY_GIB: AtomicU64 = AtomicU64::new(0);

/// The edge's answer to a heartbeat.
pub fn set_capacity_gib(capacity: f64) {
    if capacity.is_finite() && capacity > 0.0 {
        CAPACITY_GIB.store(capacity.to_bits(), Ordering::Relaxed);
    }
}

fn capacity_bytes() -> Option<f64> {
    let bits = CAPACITY_GIB.load(Ordering::Relaxed);
    (bits != 0).then(|| f64::from_bits(bits) * GIB)
}

/// At or past the level the edge warns at: of what the size holds when it has
/// said, else of the disk itself. A resumed disk reads low until it is fully
/// copied in, so right after a wake this says no, and only the always-safe
/// tier runs.
fn over_warn(usage: DiskUsage, capacity: Option<f64>) -> bool {
    let limit = capacity.unwrap_or(usage.total_bytes as f64);
    limit > 0.0 && usage.used_bytes as f64 >= limit * WARN_SHARE
}

/// What a cleanup did, for the log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub removed: Vec<PathBuf>,
    /// The budget ran out first.
    pub timed_out: bool,
}

/// Before a sleep: clear caches in this machine's checkout and home. Never
/// fails, and returns by the deadline even if the filesystem doesn't.
pub async fn pre_sleep_cleanup(data_dir: &Path) {
    let Some(session) = crate::runner::session() else {
        return; // only ever on a Cloud machine
    };
    let checkout = session.path.as_ref().map(PathBuf::from);
    let home = home_dir();
    let codex_home = crate::credentials::managed_codex_home(data_dir);
    let app_root = data_dir.join("app");
    let probe = machine_usage;
    let cleaning = tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        // What's copied back in on every wake and never needed for one: a
        // machine installed before Cloud stopped unpacking these still has them.
        zeron_update::strip_cloud_unused_installed(&app_root);
        let mut report = clean_codex_caches(&codex_home, CLEANUP_BUDGET);
        let rest = clean(
            checkout.as_deref(),
            home.as_deref(),
            || probe().is_some_and(|usage| over_warn(usage, capacity_bytes())),
            CLEANUP_BUDGET.saturating_sub(started.elapsed()),
        );
        report.removed.extend(rest.removed);
        report.timed_out |= rest.timed_out;
        report
    });
    // A little past the budget: one stuck syscall can't hold up the sleep.
    match tokio::time::timeout(CLEANUP_BUDGET + Duration::from_secs(5), cleaning).await {
        Ok(Ok(report)) => {
            tracing::info!(
                removed = report.removed.len(),
                timed_out = report.timed_out,
                "cloud: cleared caches before sleeping"
            );
        }
        Ok(Err(err)) => tracing::warn!(error = %err, "cloud: the pre-sleep cleanup failed"),
        Err(_) => tracing::warn!("cloud: the pre-sleep cleanup ran out of time"),
    }
}

/// Codex's downloads under `codex_home` ([`CODEX_CACHES`]), every sleep, full
/// or not: they cost every wake. Renamed before they're deleted, like the rest.
pub fn clean_codex_caches(codex_home: &Path, budget: Duration) -> CleanupReport {
    let deadline = Instant::now() + budget;
    let mut report = CleanupReport::default();
    for cache in CODEX_CACHES {
        let renamed = codex_home.join(format!("{cache}{REMOVING_SUFFIX}"));
        if renamed.exists() {
            remove(&renamed, deadline, &mut report);
        }
        if report.timed_out {
            break;
        }
        remove(&codex_home.join(cache), deadline, &mut report);
    }
    report
}

/// The cleanup itself. `still_full`: whether the disk is still past the warning
/// level, asked again before each thing the second tier removes.
pub fn clean(
    checkout: Option<&Path>,
    home: Option<&Path>,
    still_full: impl Fn() -> bool,
    budget: Duration,
) -> CleanupReport {
    let deadline = Instant::now() + budget;
    let mut report = CleanupReport::default();
    let targets = checkout.map(cargo_targets).unwrap_or_default();

    // Whatever a cut-off cleanup left behind, then incremental caches, which
    // cost the least to lose.
    for target in &targets {
        for leftover in removing_leftovers(target) {
            remove(&leftover, deadline, &mut report);
        }
        for profile in target_profiles(target) {
            remove(&profile.join("incremental"), deadline, &mut report);
        }
    }
    if let Some(home) = home {
        for cache in PACKAGE_CACHES {
            let renamed = home.join(format!("{cache}{REMOVING_SUFFIX}"));
            if renamed.exists() {
                remove(&renamed, deadline, &mut report);
            }
        }
    }

    // Only if it's still over: compiled dependencies, then downloads.
    for target in &targets {
        for profile in target_profiles(target) {
            if !still_full() || report.timed_out {
                return report;
            }
            remove(&profile.join("deps"), deadline, &mut report);
        }
    }
    if let Some(home) = home {
        for cache in PACKAGE_CACHES {
            if !still_full() || report.timed_out {
                return report;
            }
            remove(&home.join(cache), deadline, &mut report);
        }
    }
    report
}

/// Remove `path` if it is a real directory. Renamed first, so a deletion the
/// deadline cuts off leaves no half-emptied cache behind for a build to trust.
fn remove(path: &Path, deadline: Instant, report: &mut CleanupReport) {
    if !is_real_dir(path) {
        return;
    }
    let doomed = if path
        .as_os_str()
        .to_string_lossy()
        .ends_with(REMOVING_SUFFIX)
    {
        path.to_path_buf()
    } else {
        let mut name = path.as_os_str().to_owned();
        name.push(REMOVING_SUFFIX);
        let doomed = PathBuf::from(name);
        if let Err(err) = std::fs::rename(path, &doomed) {
            tracing::warn!(path = %path.display(), error = %err, "cloud: couldn't clear a cache");
            return;
        }
        doomed
    };
    if remove_tree(&doomed, deadline) {
        report.removed.push(path.to_path_buf());
    } else {
        report.timed_out = true;
    }
}

/// Delete a directory tree, never following a symlink out of it. `false` once
/// the deadline passes.
fn remove_tree(path: &Path, deadline: Instant) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return true; // gone, or unreadable: nothing more to do here
    };
    for entry in entries.flatten() {
        if Instant::now() >= deadline {
            return false;
        }
        let child = entry.path();
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_dir {
            if !remove_tree(&child, deadline) {
                return false;
            }
        } else if let Err(err) = std::fs::remove_file(&child) {
            tracing::debug!(path = %child.display(), error = %err, "cloud: couldn't remove a cached file");
        }
    }
    let _ = std::fs::remove_dir(path);
    true
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Directories Cargo made as its build output under `checkout`: named `target`,
/// marked as Cargo's, and with nothing in them that git tracks. Never a
/// `target` someone else's tool or the project itself keeps.
fn cargo_targets(checkout: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut frontier = vec![(checkout.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = frontier.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" {
                continue;
            }
            let path = entry.path();
            if name == "target" && is_cargo_target(&path) {
                if !git_tracks_anything_in(checkout, &path) {
                    found.push(path);
                }
                continue;
            }
            if depth + 1 < TARGET_SEARCH_DEPTH {
                frontier.push((path, depth + 1));
            }
        }
    }
    found
}

/// Cargo writes both markers into every target directory it makes.
fn is_cargo_target(path: &Path) -> bool {
    path.join(".rustc_info.json").is_file()
        && std::fs::read_to_string(path.join("CACHEDIR.TAG"))
            .is_ok_and(|tag| tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55"))
}

/// When in doubt (no repository, git failing): yes.
fn git_tracks_anything_in(checkout: &Path, dir: &Path) -> bool {
    let listed = std::process::Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["ls-files", "--"])
        .arg(dir)
        .output();
    match listed {
        Ok(out) if out.status.success() => !out.stdout.is_empty(),
        _ => true,
    }
}

/// `target/<profile>` and `target/<triple>/<profile>`: every directory Cargo
/// builds into. A profile holds the `.cargo-lock` Cargo builds under.
fn target_profiles(target: &Path) -> Vec<PathBuf> {
    let mut profiles = Vec::new();
    for child in real_child_dirs(target) {
        if child.join(".cargo-lock").is_file() {
            profiles.push(child);
        } else {
            profiles.extend(
                real_child_dirs(&child)
                    .into_iter()
                    .filter(|grandchild| grandchild.join(".cargo-lock").is_file()),
            );
        }
    }
    profiles
}

fn real_child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// What an earlier, cut-off cleanup renamed but didn't finish deleting.
fn removing_leftovers(target: &Path) -> Vec<PathBuf> {
    target_profiles(target)
        .iter()
        .flat_map(|profile| real_child_dirs(profile))
        .filter(|dir| dir.to_string_lossy().ends_with(REMOVING_SUFFIX))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A repository with a Cargo target, tracked source, untracked source, and
    /// the things beside them that must survive.
    fn machine() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("home/user/app");
        let home = root.path().join("home/user");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        write(&repo.join(".gitignore"), "target/\n");
        write(&repo.join("src/main.rs"), "fn main() {}\n");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "init"]);
        write(&repo.join("src/untracked.rs"), "// mine\n");
        cargo_target(&repo.join("target"));
        write(&home.join(".claude/.credentials.json"), "{}");
        write(&home.join(".claude/.oauth_refresh.lock/x"), "");
        write(&home.join(".npm/_cacache/a"), "cache");
        write(&home.join(".cargo/registry/cache/crate"), "cache");
        write(&home.join(".cargo/bin/tool"), "keep");
        write(&home.join(".cache/other-tool/state"), "keep");
        (root, repo, home)
    }

    fn cargo_target(target: &Path) {
        write(&target.join(".rustc_info.json"), "{}");
        write(
            &target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n# cargo\n",
        );
        for profile in ["debug", "x86_64-unknown-linux-gnu/release"] {
            let profile = target.join(profile);
            write(&profile.join(".cargo-lock"), "");
            write(&profile.join("incremental/app-1/s.bin"), "cache");
            write(&profile.join("deps/libapp.rlib"), "cache");
            write(&profile.join("app"), "binary");
        }
    }

    fn everything_kept(repo: &Path, home: &Path) {
        for kept in [
            repo.join("src/main.rs"),
            repo.join("src/untracked.rs"),
            repo.join(".git/HEAD"),
            repo.join("target/debug/app"),
            repo.join("target/debug/.cargo-lock"),
            repo.join("target/.rustc_info.json"),
            home.join(".claude/.credentials.json"),
            home.join(".claude/.oauth_refresh.lock/x"),
            home.join(".cargo/bin/tool"),
            home.join(".cache/other-tool/state"),
        ] {
            assert!(kept.exists(), "{} must survive", kept.display());
        }
    }

    /// Codex's downloads go every sleep; its login, sessions, databases and
    /// settings never do.
    #[test]
    fn codex_s_downloads_go_and_its_login_and_state_stay() {
        let root = tempfile::tempdir().unwrap();
        let codex = root.path().join(".zeron/cloud/codex-home");
        write(&codex.join("cache/remote_plugin_catalog/c981.json"), "60 MB");
        write(&codex.join("cache/codex_apps_tools/6b7d.json"), "tools");
        write(&codex.join("plugins/cache/openai-curated-remote/t/0.1.1/skills/x/a.pptx"), "bundle");
        // A cleanup a deadline cut off.
        write(&codex.join("cache.zeron-removing/old.json"), "half gone");
        let kept = [
            "auth.json",
            "config.toml",
            "installation_id",
            "models_cache.json",
            "state_5.sqlite",
            "state_5.sqlite-wal",
            "sessions/2026/11/07/rollout.jsonl",
            "skills/mine/SKILL.md",
            "plugins/installed.json",
        ];
        for file in kept {
            write(&codex.join(file), "keep");
        }

        let report = clean_codex_caches(&codex, CLEANUP_BUDGET);

        assert!(!report.timed_out);
        for gone in ["cache", "plugins/cache", "cache.zeron-removing"] {
            assert!(!codex.join(gone).exists(), "{gone} must go");
        }
        for file in kept {
            assert!(codex.join(file).exists(), "{file} must survive");
        }
        // Nothing there: nothing to do, and no harm.
        assert!(clean_codex_caches(&root.path().join("nowhere"), CLEANUP_BUDGET).removed.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_cache_that_is_a_symlink_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        write(&elsewhere.join("precious"), "keep");
        let codex = root.path().join("codex-home");
        std::fs::create_dir_all(&codex).unwrap();
        std::os::unix::fs::symlink(&elsewhere, codex.join("cache")).unwrap();
        clean_codex_caches(&codex, CLEANUP_BUDGET);
        assert!(elsewhere.join("precious").exists());
    }

    #[test]
    fn a_machine_with_room_only_loses_incremental_caches() {
        let (_root, repo, home) = machine();
        let report = clean(Some(&repo), Some(&home), || false, CLEANUP_BUDGET);
        assert!(!report.timed_out);
        assert!(!repo.join("target/debug/incremental").exists());
        assert!(
            !repo
                .join("target/x86_64-unknown-linux-gnu/release/incremental")
                .exists()
        );
        assert!(repo.join("target/debug/deps/libapp.rlib").exists());
        assert!(home.join(".npm/_cacache/a").exists());
        everything_kept(&repo, &home);
    }

    #[test]
    fn a_full_machine_loses_compiled_deps_then_downloads() {
        let (_root, repo, home) = machine();
        clean(Some(&repo), Some(&home), || true, CLEANUP_BUDGET);
        assert!(!repo.join("target/debug/deps").exists());
        assert!(!home.join(".npm/_cacache").exists());
        assert!(!home.join(".cargo/registry/cache").exists());
        everything_kept(&repo, &home);
    }

    #[test]
    fn it_stops_as_soon_as_the_disk_is_back_under_the_warning_level() {
        let (_root, repo, home) = machine();
        // Full for the first check only: one profile's deps, and nothing more.
        let checks = Cell::new(0);
        clean(
            Some(&repo),
            Some(&home),
            || {
                checks.set(checks.get() + 1);
                checks.get() == 1
            },
            CLEANUP_BUDGET,
        );
        let gone = [
            repo.join("target/debug/deps"),
            repo.join("target/x86_64-unknown-linux-gnu/release/deps"),
        ]
        .iter()
        .filter(|path| !path.exists())
        .count();
        assert_eq!(gone, 1);
        assert!(home.join(".npm/_cacache/a").exists());
    }

    #[test]
    fn a_target_cargo_did_not_make_is_never_touched() {
        let (_root, repo, home) = machine();
        // A project's own directory that merely has the name.
        write(&repo.join("web/target/debug/deps/data.json"), "mine");
        write(&repo.join("web/target/debug/incremental/plan.md"), "mine");
        write(&repo.join("web/target/debug/.cargo-lock"), "");
        clean(Some(&repo), Some(&home), || true, CLEANUP_BUDGET);
        assert!(repo.join("web/target/debug/deps/data.json").exists());
        assert!(repo.join("web/target/debug/incremental/plan.md").exists());
    }

    #[test]
    fn a_target_with_tracked_files_is_never_touched() {
        let (_root, repo, home) = machine();
        cargo_target(&repo.join("vendored/target"));
        write(&repo.join("vendored/.gitignore"), "");
        git(
            &repo,
            &["add", "-f", "vendored/target/debug/deps/libapp.rlib"],
        );
        git(&repo, &["commit", "-qm", "vendor it"]);
        clean(Some(&repo), Some(&home), || true, CLEANUP_BUDGET);
        assert!(repo.join("vendored/target/debug/deps/libapp.rlib").exists());
        assert!(
            repo.join("vendored/target/debug/incremental/app-1/s.bin")
                .exists()
        );
        assert!(!repo.join("target/debug/deps").exists());
    }

    #[test]
    fn outside_a_repository_nothing_in_the_checkout_is_touched() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("folder");
        cargo_target(&folder.join("target"));
        clean(Some(&folder), None, || true, CLEANUP_BUDGET);
        assert!(folder.join("target/debug/incremental/app-1/s.bin").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_never_followed_out_of_a_cache() {
        let (root, repo, home) = machine();
        let precious = root.path().join("precious");
        write(&precious.join("file"), "mine");
        std::os::unix::fs::symlink(&precious, repo.join("target/debug/incremental/link")).unwrap();
        // And a cache that is itself a link is left alone.
        let outside = root.path().join("outside");
        write(&outside.join("file"), "mine");
        std::fs::remove_dir_all(repo.join("target/debug/deps")).unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("target/debug/deps")).unwrap();
        clean(Some(&repo), Some(&home), || true, CLEANUP_BUDGET);
        assert!(precious.join("file").exists());
        assert!(outside.join("file").exists());
        assert!(!repo.join("target/debug/incremental").exists());
    }

    #[test]
    fn running_out_of_time_stops_it_and_a_later_cleanup_finishes() {
        let (_root, repo, home) = machine();
        for n in 0..50 {
            write(&repo.join(format!("target/debug/incremental/f{n}")), "x");
        }
        let report = clean(Some(&repo), Some(&home), || true, Duration::ZERO);
        assert!(report.timed_out);
        // What it started is out of a build's way, whole or not.
        assert!(!repo.join("target/debug/incremental").exists());
        everything_kept(&repo, &home);
        // The next sleep's cleanup clears what it left.
        let report = clean(Some(&repo), Some(&home), || false, CLEANUP_BUDGET);
        assert!(!report.timed_out);
        let leftovers = real_child_dirs(&repo.join("target/debug"));
        assert!(
            leftovers
                .iter()
                .all(|dir| !dir.to_string_lossy().ends_with(REMOVING_SUFFIX)),
            "{leftovers:?}"
        );
    }

    #[test]
    fn nothing_to_clean_is_not_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let report = clean(
            Some(&root.path().join("missing")),
            None,
            || true,
            CLEANUP_BUDGET,
        );
        assert_eq!(report, CleanupReport::default());
        clean(None, None, || true, CLEANUP_BUDGET);
    }

    #[test]
    fn the_warning_level_is_of_what_the_size_holds_when_known() {
        let usage = DiskUsage {
            used_bytes: 41 * (1u64 << 30),
            total_bytes: 200 * (1u64 << 30),
        };
        // 41 of the 200 the disk has, but 80% of the 51 the size holds.
        assert!(over_warn(usage, Some(51.0 * GIB)));
        assert!(!over_warn(usage, None));
        assert!(!over_warn(usage, Some(111.8 * GIB)));
    }

    #[cfg(unix)]
    #[test]
    fn it_reads_the_disk() {
        let usage = usage_of(Path::new("/")).unwrap();
        assert!(usage.total_bytes > 0);
        assert!(usage.used_bytes <= usage.total_bytes);
        assert!(usage_of(Path::new("/no/such/path")).is_none());
    }
}
