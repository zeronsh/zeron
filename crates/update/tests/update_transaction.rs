//! An update's failure paths, with real processes: this test binary, hard-linked
//! into a real `app/<version>/` layout, plays both the old and the new engine. It
//! really replaces itself with the new binary, really gets killed by the watchdog,
//! and really crashes (`abort`) at each step. The test plays the service manager,
//! starting `app/current/zeron` until the update has settled.
//!
//! The databases here are plain files, since only their bytes matter to the backup.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use zeron_update::transaction::{self, BootOptions, Phase, RestartMode};

const DATA_ENV: &str = "FAKE_ENGINE_DATA";
const ORIGINAL: &str = "original";
const MIGRATED: &str = "migrated by the new version";

fn this_version() -> String {
    let exe = std::env::current_exe().unwrap();
    let exe = std::fs::canonicalize(exe).unwrap();
    exe.parent().unwrap().file_name().unwrap().to_str().unwrap().to_owned()
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
    writeln!(ran, "{}", this_version()).unwrap();
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
}

impl Rig {
    fn new(behavior: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let data = root.join(".zeron");
        let app = data.join("app");
        let rig = Self { _dir: dir, data, app };
        std::fs::create_dir_all(docs(&rig.data).parent().unwrap()).unwrap();
        std::fs::write(docs(&rig.data), ORIGINAL).unwrap();
        std::fs::write(docs(&rig.data).with_file_name("docs.sqlite3-wal"), "the wal").unwrap();
        rig.install("1.0.0", "ok");
        rig.install("2.0.0", behavior);
        rig.point_current_at("1.0.0");
        rig
    }

    fn install(&self, version: &str, behavior: &str) {
        let dir = self.app.join(version);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap();
        if std::fs::hard_link(&exe, dir.join("zeron")).is_err() {
            std::fs::copy(&exe, dir.join("zeron")).unwrap();
        }
        std::fs::write(dir.join("behavior"), behavior).unwrap();
    }

    fn point_current_at(&self, version: &str) {
        let _ = std::fs::remove_file(self.app.join("current"));
        std::os::unix::fs::symlink(self.app.join(version), self.app.join("current")).unwrap();
    }

    fn request(&self, restart: RestartMode, deadline_secs: u64) -> String {
        let requested = transaction::request(&self.data, &self.app, "1.0.0", "2.0.0", restart).unwrap();
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
        let mut command = Command::new(self.app.join("current/zeron"));
        command
            .args(["--exact", "fake_engine", "--test-threads=1", "--nocapture"])
            .env(DATA_ENV, &self.data)
            .env_remove("ZERON_UPDATE_CRASH_AT")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(step) = crash_at {
            command.env("ZERON_UPDATE_CRASH_AT", step);
        }
        command.status().unwrap()
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
    let id = rig.request(Rig::service(), 3);
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
