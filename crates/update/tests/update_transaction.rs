//! An update's failure paths, with real processes: this test binary, copied
//! into a real `app/<version>/` layout, plays both the old and the new engine. It
//! really replaces itself with the new binary, really gets killed by the watchdog,
//! and really crashes (`abort`) at each step. The test plays the service manager,
//! starting `app/current/zeron` until the update has settled.
//!
//! The databases here are plain files, since only their bytes matter to the backup.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use zeron_update::transaction::{self, BootOptions, Layout, Phase, RestartMode};

const DATA_ENV: &str = "FAKE_ENGINE_DATA";
const ORIGINAL: &str = "original";
const MIGRATED: &str = "migrated by the new version";

fn this_version() -> String {
    let exe = std::env::current_exe().unwrap();
    let exe = std::fs::canonicalize(exe).unwrap();
    let dir = exe.parent().unwrap();
    // On Windows: a trial is told where it's installed; once there, a file says.
    #[cfg(windows)]
    {
        if std::env::var_os(zeron_update::INSTALLED_AT_ENV).is_some() {
            return "2.0.0".into();
        }
        std::fs::read_to_string(dir.join("zeron-version")).unwrap_or_else(|_| "1.0.0".into())
    }
    // A macOS bundle says in its Info.plist; a versioned folder is its name.
    #[cfg(unix)]
    {
        if dir.ends_with("Contents/MacOS") {
            return zeron_update::bundle_app_version(dir.parent().unwrap().parent().unwrap())
                .unwrap();
        }
        dir.file_name().unwrap().to_str().unwrap().to_owned()
    }
}

fn docs(data: &Path) -> PathBuf {
    data.join("orgs/org/user/docs.sqlite3")
}

fn migrate(data: &Path) {
    std::fs::write(docs(data), MIGRATED).unwrap();
    std::fs::write(docs(data).with_file_name("docs.sqlite3-wal"), "a new wal").unwrap();
}

/// The engine: settle what's outstanding; on trial, do what this version's
/// `behavior` file says; otherwise note that it's up, as a normal start would be.
#[test]
fn fake_engine() {
    let Some(data) = std::env::var_os(DATA_ENV).map(PathBuf::from) else {
        return;
    };
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    // Before anything can move this executable (a commit does).
    let version = this_version();
    let options = BootOptions {
        watchdog_args: ["--exact", "fake_watchdog", "--test-threads=1", "--nocapture"]
            .map(Into::into)
            .to_vec(),
        engine_args: std::env::args_os().skip(1).collect(),
    };
    let booted = transaction::boot(&data, &options, || -> anyhow::Result<()> { Ok(()) });
    if let Err(err) = booted {
        eprintln!("boot refused: {err:#}");
        std::process::exit(4);
    }
    if transaction::in_trial() {
        // Beside the exe, or for Windows beside the exe's folder (the commit moves
        // the exe away from it).
        let behavior = std::fs::read_to_string(exe.with_file_name("behavior")).unwrap();
        match behavior.trim() {
            "ok" => {
                migrate(&data);
                transaction::commit_trial().unwrap();
            }
            "exit-1" => std::process::exit(1),
            "exit-0" => std::process::exit(0),
            "migrate-then-exit" => {
                migrate(&data);
                std::process::exit(1);
            }
            "gives-up" => {
                migrate(&data);
                transaction::note_trial_failure(&data, "It couldn't reach anything.");
                std::process::exit(75);
            }
            "hang" => {
                migrate(&data);
                // Long past any deadline here, but finite: if the watchdog ever
                // stops working this fails rather than holding the suite up.
                std::thread::sleep(Duration::from_secs(45));
                std::process::exit(0);
            }
            other => panic!("unknown behavior {other}"),
        }
    }
    use std::io::Write as _;
    let mut ran = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data.join("ran"))
        .unwrap();
    writeln!(ran, "{version}").unwrap();
    std::process::exit(0);
}

#[test]
fn fake_watchdog() {
    if std::env::var_os("ZERON_UPDATE_WATCH_ID").is_none() {
        return;
    }
    transaction::run_watchdog().unwrap();
    std::process::exit(0);
}

struct Rig {
    _dir: tempfile::TempDir,
    data: PathBuf,
    app: PathBuf,
    /// A macOS app install instead of a versioned one.
    mac: Option<MacRig>,
    /// A Windows install: its folder and the new exe's.
    windows: Option<(PathBuf, PathBuf)>,
}

struct MacRig {
    bundle: PathBuf,
    incoming: PathBuf,
}

fn write_bundle(bundle: &Path, version: &str, behavior: &str) {
    let dir = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().unwrap();
    std::fs::copy(&exe, dir.join("zeron")).unwrap();
    std::fs::write(dir.join("behavior"), behavior).unwrap();
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        format!("<plist><dict><key>CFBundleShortVersionString</key>\n<string>{version}</string></dict></plist>"),
    )
    .unwrap();
}

impl Rig {
    fn new(behavior: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let data = root.join(".zeron");
        let app = data.join("app");
        let rig = Self { _dir: dir, data, app, mac: None, windows: None };
        std::fs::create_dir_all(docs(&rig.data).parent().unwrap()).unwrap();
        std::fs::write(docs(&rig.data), ORIGINAL).unwrap();
        std::fs::write(docs(&rig.data).with_file_name("docs.sqlite3-wal"), "the wal").unwrap();
        #[cfg(unix)]
        {
            rig.install("1.0.0", "ok");
            rig.install("2.0.0", behavior);
            rig.point_current_at("1.0.0");
        }
        #[cfg(windows)]
        let _ = behavior;
        rig
    }

    /// The same, as a Windows install: `zeron.exe`, and the new one in a folder of
    /// its own inside the installation's.
    #[cfg(windows)]
    fn new_windows(behavior: &str) -> Self {
        let mut rig = Self::new(behavior);
        let directory = rig.data.parent().unwrap().join("Zeron");
        let incoming = directory.join(".zeron-update-test");
        std::fs::create_dir_all(&incoming).unwrap();
        let exe = std::env::current_exe().unwrap();
        std::fs::copy(&exe, directory.join("zeron.exe")).unwrap();
        std::fs::copy(&exe, incoming.join("zeron.exe")).unwrap();
        std::fs::write(directory.join("behavior"), "ok").unwrap();
        std::fs::write(incoming.join("behavior"), behavior).unwrap();
        rig.windows = Some((directory, incoming.join("zeron.exe")));
        rig
    }

    /// The same, as a macOS app: the installed bundle, and the new one beside it.
    fn new_mac(behavior: &str) -> Self {
        let mut rig = Self::new(behavior);
        let applications = rig.data.parent().unwrap().join("Applications");
        let mac = MacRig {
            bundle: applications.join("Zeron.app"),
            incoming: applications.join(".Zeron.app.new-test"),
        };
        write_bundle(&mac.bundle, "1.0.0", "ok");
        write_bundle(&mac.incoming, "2.0.0", behavior);
        rig.mac = Some(mac);
        rig
    }

    fn layout(&self) -> Layout {
        if let Some((directory, incoming)) = &self.windows {
            return Layout::WindowsFolder {
                directory: directory.clone(),
                incoming: incoming.clone(),
            };
        }
        match &self.mac {
            Some(mac) => Layout::MacBundle {
                bundle: mac.bundle.clone(),
                incoming: mac.incoming.clone(),
            },
            None => Layout::Versioned { app_root: self.app.clone() },
        }
    }

    #[cfg(unix)]
    fn install(&self, version: &str, behavior: &str) {
        let dir = self.app.join(version);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap();
        // A copy, not a hard link: which name a process's image goes by decides
        // things here, and links to one file across installs muddle that.
        std::fs::copy(&exe, dir.join("zeron")).unwrap();
        std::fs::write(dir.join("behavior"), behavior).unwrap();
    }

    #[cfg(unix)]
    fn point_current_at(&self, version: &str) {
        let _ = std::fs::remove_file(self.app.join("current"));
        std::os::unix::fs::symlink(self.app.join(version), self.app.join("current")).unwrap();
    }

    fn request(&self, restart: RestartMode, deadline_secs: u64) -> String {
        let requested =
            transaction::request(&self.data, self.layout(), "1.0.0", "2.0.0", restart).unwrap();
        // The real deadline is minutes; these tests have seconds.
        let mut record = requested.clone();
        record.deadline_secs = deadline_secs;
        std::fs::write(
            self.data.join("update-transaction.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        requested.id
    }

    fn service() -> RestartMode {
        RestartMode::Service { unit: "no-such-unit-for-this-test.service".into() }
    }

    /// One start of the service, as a service manager makes it: whatever
    /// `current` is at that moment.
    fn start(&self, crash_at: Option<&str>) -> std::process::ExitStatus {
        let installed = match (&self.mac, &self.windows) {
            (Some(mac), _) => mac.bundle.join("Contents/MacOS/zeron"),
            (_, Some((directory, _))) => directory.join("zeron.exe"),
            _ => self.app.join("current/zeron"),
        };
        let mut command = Command::new(installed);
        command
            .args(["--exact", "fake_engine", "--test-threads=1", "--nocapture"])
            .env(DATA_ENV, &self.data)
            .env_remove("ZERON_UPDATE_CRASH_AT")
            .stdout(std::process::Stdio::null());
        if let Some(step) = crash_at {
            command.env("ZERON_UPDATE_CRASH_AT", step);
        }
        let status = command.status().unwrap();
        eprintln!("start ({crash_at:?}) ended {status:?}, phase {:?}", transaction::read(&self.data).map(|t| t.phase));
        status
    }

    /// Keep restarting, as a service manager would, until the update is settled
    /// and a normal start has happened.
    fn run_until_settled(&self) {
        for _ in 0..8 {
            self.start(None);
            if self.record().phase_finished() && self.last_ran().is_some() {
                return;
            }
        }
        panic!("never settled: {:?} ran {:?}", self.record().phase, self.last_ran());
    }

    fn record(&self) -> transaction::Transaction {
        transaction::read(&self.data).expect("a record")
    }

    fn current(&self) -> String {
        if let Some((directory, _)) = &self.windows {
            return std::fs::read_to_string(directory.join("zeron-version"))
                .unwrap_or_else(|_| "1.0.0".into());
        }
        if let Some(mac) = &self.mac {
            return zeron_update::bundle_app_version(&mac.bundle).unwrap();
        }
        let target = std::fs::read_link(self.app.join("current")).unwrap();
        target.file_name().unwrap().to_str().unwrap().to_owned()
    }

    fn last_ran(&self) -> Option<String> {
        std::fs::read_to_string(self.data.join("ran"))
            .ok()?
            .lines()
            .last()
            .map(str::to_owned)
    }

    fn database(&self) -> String {
        std::fs::read_to_string(docs(&self.data)).unwrap()
    }

    #[track_caller]
    fn assert_updated(&self, id: &str) {
        let record = self.record();
        assert_eq!(record.id, id);
        assert_eq!(record.phase, Phase::Updated, "{:?}", record.reason);
        assert_eq!(self.current(), "2.0.0");
        assert_eq!(self.last_ran().as_deref(), Some("2.0.0"));
        assert_eq!(self.database(), MIGRATED);
        assert!(!self.data.join("update-snapshots").exists(), "backup left behind");
        self.assert_no_bundles_left();
    }

    /// Set aside or half-installed bundles go once nothing runs from them.
    #[track_caller]
    fn assert_no_bundles_left(&self) {
        let Some(mac) = &self.mac else { return };
        let left = || -> Vec<String> {
            std::fs::read_dir(mac.bundle.parent().unwrap())
                .unwrap()
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != "Zeron.app")
                .collect()
        };
        // What the engine does a moment after committing: at the commit itself
        // the old version's watchdog is still on its way out of the old bundle.
        let waited = Instant::now();
        while !left().is_empty() && waited.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(100));
            transaction::tidy_latest(&self.data);
        }
        assert!(left().is_empty(), "left behind: {:?}", left());
    }

    #[track_caller]
    fn assert_back_on_the_old_version(&self, id: &str, phase: Phase) {
        let record = self.record();
        assert_eq!(record.id, id);
        assert_eq!(record.phase, phase, "{:?}", record.reason);
        assert!(record.reason.is_some(), "no reason given");
        assert_eq!(self.current(), "1.0.0");
        assert_eq!(self.last_ran().as_deref(), Some("1.0.0"));
        assert_eq!(self.database(), ORIGINAL);
        assert_eq!(
            std::fs::read_to_string(docs(&self.data).with_file_name("docs.sqlite3-wal")).unwrap(),
            "the wal"
        );
        assert!(!self.data.join("update-snapshots").exists(), "backup left behind");
        self.assert_no_bundles_left();
    }
}

trait Finished {
    fn phase_finished(&self) -> bool;
}

impl Finished for transaction::Transaction {
    fn phase_finished(&self) -> bool {
        !self.in_progress()
    }
}

#[cfg(unix)]
mod versioned {
    use super::*;

    #[test]
    fn a_good_release_commits_and_keeps_its_migration() {
        let rig = Rig::new("ok");
        let id = rig.request(Rig::service(), 60);
        rig.run_until_settled();
        rig.assert_updated(&id);
        assert_eq!(rig.record().report().state, zeron_update::UpdateState::Updated);
    }

    #[test]
    fn a_new_binary_that_exits_at_once_goes_back() {
        let rig = Rig::new("exit-1");
        let id = rig.request(Rig::service(), 60);
        rig.run_until_settled();
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
        assert_eq!(rig.record().report().state, zeron_update::UpdateState::RolledBack);
    }

    /// A clean exit is still a failed trial, whatever a service manager makes of it.
    #[test]
    fn a_new_binary_that_exits_cleanly_goes_back() {
        let rig = Rig::new("exit-0");
        let id = rig.request(Rig::service(), 60);
        rig.run_until_settled();
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
    }

    #[test]
    fn a_migration_is_undone_when_the_new_binary_then_dies() {
        let rig = Rig::new("migrate-then-exit");
        let id = rig.request(Rig::service(), 60);
        let first = rig.start(None);
        assert_eq!(first.code(), Some(1));
        assert_eq!(rig.database(), MIGRATED, "the trial should have got that far");
        rig.run_until_settled();
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
    }

    #[test]
    fn a_trials_own_reason_reaches_the_record() {
        let rig = Rig::new("gives-up");
        let id = rig.request(Rig::service(), 60);
        rig.run_until_settled();
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
        let reason = rig.record().reason.unwrap();
        assert!(reason.contains("It couldn't reach anything."), "{reason}");
        assert!(reason.contains("went back to 1.0.0"), "{reason}");
    }

    /// The hang: only the watchdog can end it. It's a sibling of the real process,
    /// from the old version, and it kills the new one at the deadline.
    #[test]
    fn a_new_binary_that_hangs_is_stopped_at_the_deadline_and_goes_back() {
        let rig = Rig::new("hang");
        let id = rig.request(Rig::service(), 2);
        let started = Instant::now();
        let hung = rig.start(None);
        let waited = started.elapsed();
        use std::os::unix::process::ExitStatusExt as _;
        assert_eq!(hung.signal(), Some(libc_sigkill()), "{hung:?}");
        assert!(waited >= Duration::from_secs(2), "killed too soon: {waited:?}");
        assert!(waited < Duration::from_secs(20), "killed too late: {waited:?}");
        rig.run_until_settled();
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
        let reason = rig.record().reason.unwrap();
        assert!(reason.contains("didn't finish starting"), "{reason}");
    }

    /// The desktop app starts its engine when the port goes quiet. Doing that during
    /// a trial must not be taken for the trial having failed.
    #[test]
    fn a_second_start_during_a_trial_leaves_the_trial_alone() {
        let rig = Rig::new("hang");
        // Long enough that a slow machine's second start still lands inside it.
        let id = rig.request(Rig::service(), 8);
        std::thread::scope(|scope| {
            let trial = scope.spawn(|| rig.start(None));
            let waited = Instant::now();
            while rig.record().phase != Phase::Trial || rig.database() != MIGRATED {
                assert!(waited.elapsed() < Duration::from_secs(10), "the trial never started");
                std::thread::sleep(Duration::from_millis(20));
            }
            let second = rig.start(None);
            assert_eq!(second.code(), Some(4), "{second:?}");
            assert_eq!(rig.record().phase, Phase::Trial);
            assert_eq!(rig.database(), MIGRATED, "it must not have restored under the trial");
            let _ = trial.join();
        });
        rig.run_until_settled();
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
    }

    fn libc_sigkill() -> i32 {
        9
    }

    /// With nothing to restart it (the desktop app closed), the watchdog itself
    /// starts the old version. This test starts nothing after the first start.
    #[test]
    fn with_no_service_manager_the_watchdog_brings_the_old_version_back() {
        for (behavior, deadline) in [("exit-1", 60), ("exit-0", 60), ("hang", 2)] {
            let rig = Rig::new(behavior);
            let id = rig.request(RestartMode::Relaunch, deadline);
            rig.start(None);
            let waited = Instant::now();
            while rig.last_ran().is_none() {
                assert!(waited.elapsed() < Duration::from_secs(30), "{behavior}: nothing came back");
                std::thread::sleep(Duration::from_millis(100));
            }
            rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
        }
    }

    /// A crash at each step of a good update. Up to the point where the new version
    /// has been started, starting again simply carries on; once it has, the old
    /// version can't know how far it got and goes back; from the moment `current`
    /// moves, it's done.
    #[test]
    fn a_crash_at_any_step_of_a_good_update_lands_somewhere_whole() {
        enum Lands {
            Updated,
            Back,
        }
        for (step, lands) in [
            ("snapshot-partial", Lands::Updated),
            ("snapshot-done", Lands::Updated),
            ("trial-marked", Lands::Back),
            ("watchdog-started", Lands::Back),
            ("trial-before-commit", Lands::Back),
            ("commit-flipped", Lands::Updated),
            ("commit-recorded", Lands::Updated),
        ] {
            let rig = Rig::new("ok");
            let id = rig.request(Rig::service(), 60);
            let crashed = rig.start(Some(step));
            assert!(!crashed.success(), "{step}: it should have crashed");
            rig.run_until_settled();
            match lands {
                Lands::Updated => rig.assert_updated(&id),
                Lands::Back => rig.assert_back_on_the_old_version(&id, Phase::RolledBack),
            }
        }
    }

    /// A crash at each step of going back: the restore finishes before anything
    /// opens the databases, and the record ends up true.
    #[test]
    fn a_crash_at_any_step_of_going_back_still_gets_back() {
        for step in ["restore-marked", "restore-partial", "restored", "rollback-recorded"] {
            let rig = Rig::new("migrate-then-exit");
            let id = rig.request(Rig::service(), 60);
            assert_eq!(rig.start(None).code(), Some(1));
            let crashed = rig.start(Some(step));
            assert!(!crashed.success(), "{step}: it should have crashed");
            if step != "rollback-recorded" {
                assert_eq!(rig.record().phase, Phase::Restoring, "{step}");
            }
            rig.run_until_settled();
            rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
        }
    }

    /// `current` is what boots, so the new version can't be run by mistake while the
    /// old one has a restore to finish.
    #[test]
    fn the_new_version_refuses_to_start_during_a_restore() {
        let rig = Rig::new("migrate-then-exit");
        rig.request(Rig::service(), 60);
        rig.start(None);
        rig.start(Some("restore-partial"));
        assert_eq!(rig.record().phase, Phase::Restoring);
        rig.point_current_at("2.0.0");
        let refused = rig.start(None);
        assert_eq!(refused.code(), Some(4));
        rig.point_current_at("1.0.0");
        rig.run_until_settled();
        assert_eq!(rig.database(), ORIGINAL);
    }

    #[test]
    fn release_files_gone_missing_leave_it_on_the_old_version_untouched() {
        let rig = Rig::new("ok");
        let id = rig.request(Rig::service(), 60);
        std::fs::remove_dir_all(rig.app.join("2.0.0")).unwrap();
        rig.run_until_settled();
        rig.assert_back_on_the_old_version(&id, Phase::Failed);
        assert_eq!(rig.record().report().state, zeron_update::UpdateState::Failed);
    }

    /// A binary that can't even be started: it never ran, so nothing was restored
    /// and there's no second start to wait for.
    #[test]
    fn a_new_binary_that_cannot_be_started_leaves_it_on_the_old_version() {
        use std::os::unix::fs::PermissionsExt as _;
        let rig = Rig::new("ok");
        let id = rig.request(Rig::service(), 60);
        let broken = rig.app.join("2.0.0/zeron");
        std::fs::remove_file(&broken).unwrap();
        std::fs::write(&broken, "not a program").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o644)).unwrap();
        let only = rig.start(None);
        assert!(only.success(), "{only:?}");
        rig.assert_back_on_the_old_version(&id, Phase::Failed);
    }

    /// Someone ran the installer mid-update: that version owns the data now, and
    /// restoring over it could lose what it wrote.
    #[test]
    fn a_third_version_installed_meanwhile_is_left_alone() {
        let rig = Rig::new("migrate-then-exit");
        rig.install("3.0.0", "ok");
        let id = rig.request(Rig::service(), 60);
        rig.start(None);
        rig.point_current_at("3.0.0");
        rig.run_until_settled();
        let record = rig.record();
        assert_eq!(record.id, id);
        assert_eq!(record.phase, Phase::Failed);
        assert_eq!(rig.last_ran().as_deref(), Some("3.0.0"));
        assert_eq!(rig.database(), MIGRATED, "it must not restore over another version's data");
    }

}

/// Windows: no replacing a process, so the old one starts the new exe and stays on
/// as the watchdog (the test plays the app, which starts the engine again). A
/// running exe can be renamed, and that's the commit.
#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn a_good_release_moves_into_place_and_keeps_its_migration() {
        let rig = Rig::new_windows("ok");
        let id = rig.request(RestartMode::Relaunch, 60);
        rig.run_until_settled();
        wait_for_ran(&rig, "2.0.0");
        rig.assert_updated(&id);
    }

    #[test]
    fn a_bad_release_goes_back() {
        for behavior in ["exit-1", "exit-0", "migrate-then-exit"] {
            let rig = Rig::new_windows(behavior);
            let id = rig.request(RestartMode::Relaunch, 60);
            rig.start(None);
            wait_for_ran(&rig, "1.0.0");
            rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
            assert!(!rig.windows.as_ref().unwrap().1.exists(), "{behavior}: new exe left behind");
        }
    }

    #[test]
    fn a_hung_release_is_stopped_and_the_old_version_comes_back() {
        let rig = Rig::new_windows("hang");
        let id = rig.request(RestartMode::Relaunch, 2);
        rig.start(None);
        wait_for_ran(&rig, "1.0.0");
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
        assert!(rig.record().reason.unwrap().contains("didn't finish starting"));
    }

    fn wait_for_ran(rig: &Rig, version: &str) {
        let waited = Instant::now();
        while rig.last_ran().as_deref() != Some(version) {
            assert!(
                waited.elapsed() < Duration::from_secs(60),
                "{version} never came back: {:?}",
                rig.record()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A macOS app: the same rule with the bundle in `current`'s place. The bundle
/// only changes at the commit, in one atomic swap.
#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    #[test]
    fn a_good_release_swaps_in_and_keeps_its_migration() {
        let rig = Rig::new_mac("ok");
        let id = rig.request(Rig::service(), 60);
        rig.run_until_settled();
        rig.assert_updated(&id);
    }

    #[test]
    fn a_bad_release_leaves_the_installed_app_alone() {
        for behavior in ["exit-1", "exit-0", "migrate-then-exit"] {
            let rig = Rig::new_mac(behavior);
            let id = rig.request(Rig::service(), 60);
            rig.run_until_settled();
            rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
        }
    }

    #[test]
    fn a_hung_release_is_stopped_and_the_installed_app_comes_back() {
        let rig = Rig::new_mac("hang");
        let id = rig.request(RestartMode::Relaunch, 2);
        rig.start(None);
        let waited = Instant::now();
        while rig.last_ran().is_none() {
            assert!(waited.elapsed() < Duration::from_secs(30), "nothing came back");
            std::thread::sleep(Duration::from_millis(100));
        }
        rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
    }

    #[test]
    fn a_crash_at_any_step_lands_somewhere_whole() {
        for (step, updated) in [
            ("snapshot-partial", true),
            ("snapshot-done", true),
            ("trial-marked", false),
            ("watchdog-started", false),
            ("trial-before-commit", false),
            ("commit-flipped", true),
            ("commit-recorded", true),
        ] {
            let rig = Rig::new_mac("ok");
            let id = rig.request(Rig::service(), 60);
            let crashed = rig.start(Some(step));
            assert!(!crashed.success(), "{step}: it should have crashed");
            rig.run_until_settled();
            if updated {
                rig.assert_updated(&id);
            } else {
                rig.assert_back_on_the_old_version(&id, Phase::RolledBack);
            }
        }
    }

    /// #1037: a bundle something runs from isn't deleted, however finished with
    /// it the update is, and goes once nothing does.
    #[test]
    fn a_bundle_in_use_is_kept_until_nothing_runs_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let bundle = root.join("Zeron.app");
        let retired = root.join(".Zeron.app.old-123");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(retired.join("Contents/MacOS")).unwrap();
        let sleeper = retired.join("Contents/MacOS/sleeper");
        let source = root.join("sleep.c");
        std::fs::write(&source, "#include <unistd.h>\nint main(void){sleep(30);return 0;}\n").unwrap();
        let compiled = Command::new("cc").arg("-o").arg(&sleeper).arg(&source).status();
        if !compiled.is_ok_and(|status| status.success()) {
            eprintln!("no C compiler: skipping");
            return;
        }
        let mut running = Command::new(&sleeper).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(zeron_update::process_runs_from(&retired));
        zeron_update::prune_retired_mac_bundles(&bundle);
        assert!(retired.exists(), "deleted from under a running process");

        running.kill().unwrap();
        running.wait().unwrap();
        assert!(!zeron_update::process_runs_from(&retired));
        zeron_update::prune_retired_mac_bundles(&bundle);
        assert!(!retired.exists(), "left behind once nothing ran from it");
        assert!(bundle.exists());
    }
}
