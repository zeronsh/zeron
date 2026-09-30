//! The old image's half of a handoff: decide, freeze, and `execve`.

use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::fds::set_inheritable;
use super::manifest::{
    HANDOFF_ATTEMPT_ENV, HANDOFF_FD_ENV, HANDOFF_ROLLED_BACK_ENV, MANIFEST_VERSION, Manifest,
};
use crate::EngineCore;

/// The new binary must answer its preflight within this window.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    /// Something running cannot be carried across right now. Nothing was
    /// touched; retry later.
    #[error("handoff deferred: {0}")]
    Busy(String),
    /// This engine cannot hand off at all (it serves no IPC listener).
    #[error("handoff unavailable: {0}")]
    Unsupported(String),
    /// The new binary is missing, not executable, or failed its preflight.
    #[error("preflight failed: {0}")]
    Preflight(String),
    /// A component could not be frozen. It was thawed; nothing was lost.
    #[error("could not freeze: {0}")]
    Freeze(String),
    /// `execve` failed. The freeze was undone: the engine is running as before.
    #[error("exec failed: {0}")]
    Exec(std::io::Error),
    /// Describing what the successor inherits failed (descriptors, manifest).
    /// Everything was thawed; not necessarily transient, so it is not retried
    /// as eagerly as a `Freeze`.
    #[error("could not prepare the handoff: {0}")]
    Prepare(String),
}

/// Environment variable that lifts the location rule of [`check_target`] (for
/// tests and unusual layouts). It is read from the ENGINE's own environment,
/// which no other local process controls.
pub const ANY_TARGET_ENV: &str = "ZERON_HANDOFF_ANY_EXE";

/// May `exe` become this engine? It replaces the process — inheriting the IPC
/// listener, every terminal and the agents' pipes — so it must be this
/// install's own binary, not whatever path a client names:
///
/// - an absolute path to a regular file owned by the engine's user and not
///   writable by group or others;
/// - the current install's binary (the `current` symlink of a managed install,
///   inside its `app_root`; the app bundle on macOS), or, for an unmanaged
///   build, the very path this engine runs from (a rebuilt binary).
pub fn check_target(exe: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if !exe.is_absolute() {
        return Err(format!("{} is not an absolute path", exe.display()));
    }
    let canonical = exe
        .canonicalize()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let meta = std::fs::metadata(&canonical).map_err(|e| format!("{}: {e}", exe.display()))?;
    // SAFETY: geteuid has no failure mode and touches no memory.
    let euid = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.uid() != euid || meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{} must be a regular file owned by the engine's user and not writable by others",
            exe.display()
        ));
    }
    if std::env::var_os(ANY_TARGET_ENV).is_some_and(|value| !value.is_empty()) {
        return Ok(());
    }
    let inside = |root: &Path| {
        root.canonicalize()
            .is_ok_and(|root| canonical.starts_with(root))
    };
    let allowed = match zeron_update::detect_install() {
        zeron_update::InstallKind::Managed { app_root } => inside(&app_root),
        zeron_update::InstallKind::MacApp { bundle } => inside(&bundle),
        _ => std::env::current_exe()
            .ok()
            .and_then(|own| own.canonicalize().ok())
            .is_some_and(|own| own == canonical),
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "{} is not this install's binary; a live handoff only goes to the installed build",
            exe.display()
        ))
    }
}

/// What `zeron handoff-preflight` prints: `handoff-ok <manifest version>` and one
/// `handoff-state <harness id> <version,...>` line per adoptable harness (the
/// state versions this build can read back; see
/// [`zeron_harness::adoptable_state_versions`]).
pub fn preflight_report() -> String {
    let mut report = format!("handoff-ok {MANIFEST_VERSION}\n");
    for (id, versions) in zeron_harness::adoptable_state_versions() {
        let name = serde_json::to_value(id)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned));
        if let Some(name) = name {
            let versions: Vec<String> = versions.iter().map(u32::to_string).collect();
            report.push_str(&format!("handoff-state {name} {}\n", versions.join(",")));
        }
    }
    report
}

/// What a successor said it can adopt.
#[derive(Debug, Default, Clone)]
pub struct SuccessorCaps {
    /// Harness id (its serialized name) -> the state versions it can read. A
    /// harness the successor does not list is unknown, not unsupported (an older
    /// build predates the report), and is not vetoed.
    states: std::collections::HashMap<String, Vec<u32>>,
}

impl SuccessorCaps {
    fn parse(stdout: &str) -> Self {
        let states = stdout
            .lines()
            .filter_map(|line| line.strip_prefix("handoff-state "))
            .filter_map(|rest| {
                let (id, versions) = rest.split_once(' ')?;
                let versions = versions
                    .trim()
                    .split(',')
                    .filter_map(|v| v.parse().ok())
                    .collect();
                Some((id.to_owned(), versions))
            })
            .collect();
        Self { states }
    }

    /// Why the successor could not adopt this exported run, if it cannot.
    fn unreadable(&self, chat_id: &str, harness: &zeron_harness::HarnessHandoff) -> Option<String> {
        let id = serde_json::to_value(harness.harness)
            .ok()?
            .as_str()?
            .to_owned();
        let supported = self.states.get(&id)?;
        (!supported.contains(&harness.state_version)).then(|| {
            format!(
                "the new binary cannot read the {id} state (version {}) of chat {chat_id}",
                harness.state_version
            )
        })
    }

    /// The first exported run the successor could not adopt, if any.
    fn unreadable_run(&self, runs: &[super::RunHandoff]) -> Option<String> {
        runs.iter()
            .find_map(|run| self.unreadable(&run.chat_id, &run.harness))
    }
}

/// `exe handoff-preflight` must succeed and print `handoff-ok <N>` with `N`
/// at least our manifest version, so the successor can read what we write.
pub async fn preflight(new_exe: &Path) -> Result<SuccessorCaps, String> {
    // A relative path resolves against the engine's cwd and a bare name is
    // searched on PATH — the checked file and the exec'd file could differ.
    if !new_exe.is_absolute() {
        return Err(format!("{} is not an absolute path", new_exe.display()));
    }
    let meta = std::fs::metadata(new_exe).map_err(|e| format!("{}: {e}", new_exe.display()))?;
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return Err(format!("{} is not an executable file", new_exe.display()));
    }
    let mut command = tokio::process::Command::new(new_exe);
    command
        .arg("handoff-preflight")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(PREFLIGHT_TIMEOUT, command.output())
        .await
        .map_err(|_| "the new binary did not answer in time".to_string())?
        .map_err(|e| format!("could not run {}: {e}", new_exe.display()))?;
    if !output.status.success() {
        return Err(format!("the new binary exited with {}", output.status));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let supported: u32 = stdout
        .lines()
        .find_map(|line| line.strip_prefix("handoff-ok "))
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| "the new binary does not support handoff".to_string())?;
    if supported < MANIFEST_VERSION {
        return Err(format!(
            "the new binary reads manifest version {supported}, this build writes {MANIFEST_VERSION}"
        ));
    }
    Ok(SuccessorCaps::parse(&stdout))
}

/// Replace this process with `exe`, same arguments, telling it where the
/// manifest is. Returns only on failure.
pub fn exec_into(exe: &Path, manifest_fd: RawFd, attempt: u32) -> std::io::Error {
    exec_with(exe, manifest_fd, attempt, None)
}

/// [`exec_into`], optionally telling the image that this is a rollback of
/// `rolled_back_from`'s failed adoption.
fn exec_with(
    exe: &Path,
    manifest_fd: RawFd,
    attempt: u32,
    rolled_back_from: Option<&str>,
) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(exe);
    command
        .args(std::env::args_os().skip(1))
        .env(HANDOFF_FD_ENV, manifest_fd.to_string())
        .env(HANDOFF_ATTEMPT_ENV, attempt.to_string());
    if let Some(version) = rolled_back_from {
        command.env(HANDOFF_ROLLED_BACK_ENV, version);
    }
    // The role was taken out of the environment at startup (agents must not
    // inherit it); the next image needs it back.
    if let Some(host) = zeron_update::engine_host() {
        command.env("ZERON_ENGINE_HOST", host);
    }
    command.exec()
}

/// A successor that cannot finish booting hands the same manifest back to
/// the image that wrote it. Loop guard: after this many handoff/rollback
/// execs, stop and let the service manager restart the engine (the ordinary
/// crash-recovery path).
const MAX_HANDOFF_ATTEMPTS: u32 = 3;

/// Hand a failed adoption back to the predecessor binary. Returns only if it
/// could not (or should not): the caller then exits with its error.
///
/// The originals of everything inherited are still open and inheritable — the
/// failed image worked on close-on-exec duplicates, which the exec closes —
/// and the adopted lock never unlocked, so the predecessor adopts the very
/// same manifest again.
pub fn roll_back(adoption: super::manifest::Adoption, error: &anyhow::Error) {
    if adoption.attempt >= MAX_HANDOFF_ATTEMPTS {
        tracing::error!(%error, attempt = adoption.attempt, "handoff adoption failed and the rollback limit is reached");
        return;
    }
    let from = adoption.manifest.from_exe.clone();
    tracing::error!(%error, from = %from.display(), "handoff adoption failed; handing back to the previous binary");
    adoption.release_for_rollback();
    // Say which version failed, so the predecessor does not hand off to it
    // again (its updater would otherwise find the same newer install and loop).
    let exec_error = exec_with(
        &from,
        adoption.manifest_fd(),
        adoption.attempt + 1,
        Some(zeron_update::current_version()),
    );
    tracing::error!(error = %exec_error, "could not hand back to the previous binary");
}

/// Where a failed successor hands the handoff back to.
///
/// The running binary's own path, except on macOS: there an update replaces the
/// whole app bundle, so by the time the successor fails, that path holds the
/// NEW binary and a "rollback" would exec the very build that just failed. A
/// private copy of the running binary, named by version, keeps the way back.
///
/// Call it at BOOT, before anything can swap the bundle: the copy must be of
/// the binary this process is really running, which only holds until an update
/// replaces the file at `current_exe()`.
pub fn rollback_exe(data_dir: &Path) -> PathBuf {
    let current = std::env::current_exe().unwrap_or_default();
    if !cfg!(target_os = "macos") {
        return current;
    }
    let dir = rollback_dir(data_dir);
    let copy = dir.join(format!("zeron-{}", zeron_update::current_version()));
    let made = (|| -> std::io::Result<()> {
        // A copy left by an earlier boot of this same version is reusable
        // (the version names the binary); a different size is not this build.
        if std::fs::metadata(&copy).map(|m| m.len()).ok()
            == std::fs::metadata(&current).map(|m| m.len()).ok()
            && copy.exists()
        {
            return Ok(());
        }
        std::fs::create_dir_all(&dir)?;
        let partial = dir.join(format!(".zeron-{}.partial", std::process::id()));
        std::fs::copy(&current, &partial)?;
        std::fs::rename(&partial, &copy)
    })();
    match made {
        Ok(()) => copy,
        Err(error) => {
            tracing::warn!(%error, "could not keep a copy of the running binary for a rollback; using its own path");
            current
        }
    }
}

fn rollback_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("handoff-rollback")
}

/// Delete kept rollback binaries once an adoption has committed: the images
/// they were for are gone. The running binary itself is left alone (a
/// rolled-back image runs from one).
pub fn prune_rollback_copies(data_dir: &Path) {
    let running = std::env::current_exe().ok();
    let Ok(entries) = std::fs::read_dir(rollback_dir(data_dir)) else {
        return;
    };
    for entry in entries.flatten() {
        if running.as_deref() != Some(entry.path().as_path()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Clears `handoff_running` however the handoff ends.
struct RunningGuard<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Descriptors made inheritable for the exec. Restored to close-on-exec if the
/// handoff does not end in one.
struct Inheritable(Vec<RawFd>);

impl Inheritable {
    fn add(&mut self, fd: RawFd) -> std::io::Result<()> {
        set_inheritable(fd, true)?;
        self.0.push(fd);
        Ok(())
    }
}

impl Drop for Inheritable {
    fn drop(&mut self) {
        for &fd in &self.0 {
            let _ = set_inheritable(fd, false);
        }
    }
}

impl EngineCore {
    /// Record the IPC listener's descriptor: the successor inherits it, so
    /// clients queue in its backlog during the gap instead of being refused.
    pub fn set_handoff_listener(&self, fd: RawFd) {
        self.handoff_listener_fd.store(fd, Ordering::SeqCst);
    }

    /// Why a handoff cannot proceed right now, one entry per condition. Every
    /// one is state that lives only in this process.
    pub fn handoff_vetoes(&self) -> Vec<String> {
        let mut vetoes = Vec::new();
        if self.agent_accounts.login_in_flight() {
            vetoes.push("an agent sign-in is in progress".into());
        }
        if self.harness_updates.any_active() {
            vetoes.push("a harness update is running".into());
        }
        if self.previews.has_active_tunnels() {
            vetoes.push("a login callback tunnel is open".into());
        }
        vetoes.extend(self.sessions.unfreezable_runs());
        vetoes
    }

    /// Hand this engine to `new_exe` in place. Returns only on failure — on
    /// success the process image is replaced — and on every failure the
    /// engine is exactly as it was.
    ///
    /// Everything before the `execve` is reversible: components are frozen
    /// (each can thaw), and the only irreversible step is the exec itself.
    /// Sockets, threads and tasks that are not deliberately carried simply
    /// end with the old image, which is why the edge relay needs no teardown.
    pub async fn handoff(&self, new_exe: &Path) -> HandoffError {
        // The caller may be any local process (the RPC is on the loopback
        // socket): the binary that becomes this engine must be one this
        // install put there.
        if let Err(reason) = check_target(new_exe) {
            return HandoffError::Preflight(reason);
        }
        self.handoff_with(new_exe, exec_into).await
    }

    /// [`Self::handoff`] with the exec step injectable, so the whole freeze /
    /// prepare / thaw sequence is testable without replacing the test process.
    pub(crate) async fn handoff_with(
        &self,
        new_exe: &Path,
        exec: impl FnOnce(&Path, RawFd, u32) -> std::io::Error,
    ) -> HandoffError {
        if self.handoff_running.swap(true, Ordering::SeqCst) {
            return HandoffError::Busy("a handoff is already in progress".into());
        }
        let _running = RunningGuard(&self.handoff_running);

        let listener_fd = self.handoff_listener_fd.load(Ordering::SeqCst);
        if listener_fd < 0 {
            return HandoffError::Unsupported("this engine does not serve IPC".into());
        }
        let vetoes = self.handoff_vetoes();
        if !vetoes.is_empty() {
            return HandoffError::Busy(vetoes.join("; "));
        }
        let successor = match preflight(new_exe).await {
            Ok(caps) => caps,
            Err(reason) => return HandoffError::Preflight(reason),
        };

        // ── Phase A: freeze. Every step below can be undone. ──
        // A WorkOS refresh token is single-use: never exec mid-rotation.
        let auth = self.auth();
        let _no_token_rotation = auth.quiesce_refresh().await;
        // Nothing may start a run (or release a queued row into one) while we
        // freeze and exec; the pause remembers which queues it flipped.
        let queues = self.doc_host.pause_queues_for_handoff();
        let resume_queues =
            |core: &EngineCore, queues| core.doc_host.resume_queues_after_handoff(queues);

        let runs = match self.sessions.freeze_runs().await {
            Ok(runs) => runs,
            Err(err) => {
                resume_queues(self, queues);
                return HandoffError::Busy(err.to_string());
            }
        };
        // A run whose exported state the successor cannot read would be stopped
        // and recovered from its journal (losing the turn in flight): refuse the
        // handoff instead, and never kill work for an update.
        if let Some(reason) = successor.unreadable_run(&runs.handoffs()) {
            runs.thaw();
            resume_queues(self, queues);
            return HandoffError::Preflight(reason);
        }
        let terminals = match self.terminals.freeze().await {
            Ok(terminals) => terminals,
            Err(err) => {
                runs.thaw();
                resume_queues(self, queues);
                return HandoffError::Freeze(err.to_string());
            }
        };

        // ── Phase B: describe what the successor inherits, then exec. ──
        // Persist every open doc BEFORE making anything inheritable: a flush
        // can take a while, and any child the engine spawns while its fds are
        // inheritable (git, a harness process) would carry them off and could
        // pin the lock or the listener after this engine is gone. (Doc
        // workers are deliberately NOT shut down — that cannot be undone — so
        // a write landing after the flush and before the exec is at most as
        // exposed as a crash today, which loses up to the persistence
        // debounce.)
        self.doc_host.flush_all();
        let mut inherit = Inheritable(Vec::new());
        let manifest = Manifest {
            version: MANIFEST_VERSION,
            from_exe: self
                .rollback_exe
                .get()
                .cloned()
                .unwrap_or_else(|| std::env::current_exe().unwrap_or_default()),
            from_version: zeron_update::current_version().to_string(),
            listener_fd,
            lock_fd: self._instance_lock.raw_fd(),
            terminals: terminals.handoffs().to_vec(),
            runs: runs.handoffs(),
        };
        let prepared = (|| -> std::io::Result<std::os::fd::OwnedFd> {
            inherit.add(manifest.listener_fd)?;
            inherit.add(manifest.lock_fd)?;
            terminals.make_inheritable()?;
            runs.make_inheritable()?;
            manifest.write_anon()
        })();
        let manifest_fd = match prepared {
            Ok(fd) => fd,
            Err(err) => {
                drop(inherit);
                terminals.thaw();
                runs.thaw();
                resume_queues(self, queues);
                return HandoffError::Prepare(err.to_string());
            }
        };
        // A committed engine starts the failure count afresh: `attempt` only
        // counts consecutive handoff/rollback execs, and lives in the image
        // being adopted (see `roll_back`), never in the engine.
        let err = exec(new_exe, manifest_fd.as_raw_fd(), 1);

        // The exec failed: the engine is still this image. Put everything back.
        drop(manifest_fd);
        drop(inherit);
        terminals.thaw();
        runs.thaw();
        resume_queues(self, queues);
        HandoffError::Exec(err)
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn a_successor_that_cannot_read_a_runs_state_vetoes_the_handoff_but_unknown_is_fine() {
        let report = preflight_report();
        assert!(report.starts_with(&format!("handoff-ok {MANIFEST_VERSION}\n")));
        assert!(report.contains("handoff-state claude-code 1"), "{report}");
        let caps = SuccessorCaps::parse(
            "handoff-ok 1\nhandoff-state claude-code 1,2\nhandoff-state codex 3\n",
        );
        let run = |harness, version| zeron_harness::HarnessHandoff {
            harness,
            state_version: version,
            pid: 4242,
            stdin_fd: 5,
            stdout_fd: 6,
            stderr_fd: None,
            extra_fds: Vec::new(),
            stdout_leftover: Vec::new(),
            stderr_tail: Vec::new(),
            state: serde_json::json!({}),
            undrained_steers: Vec::new(),
        };
        // Listed and readable.
        assert!(
            caps.unreadable("c1", &run(HarnessId::ClaudeCode, 2))
                .is_none()
        );
        // Listed, but not this version: veto with the reason.
        let reason = caps.unreadable("c1", &run(HarnessId::Codex, 1)).unwrap();
        assert!(
            reason.contains("codex") && reason.contains("version 1"),
            "{reason}"
        );
        // Not listed at all (an older build predates the report): not vetoed.
        assert!(caps.unreadable("c1", &run(HarnessId::Cursor, 9)).is_none());
    }

    #[test]
    fn only_this_installs_own_binary_may_become_the_engine() {
        use std::os::unix::fs::PermissionsExt;
        // (Assumes the override is not set in the test environment.)
        assert!(std::env::var_os(ANY_TARGET_ENV).is_none());
        let own = std::env::current_exe().unwrap();
        assert!(check_target(&own).is_ok(), "the running binary itself");
        assert!(
            check_target(Path::new("relative/zeron"))
                .unwrap_err()
                .contains("absolute")
        );
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("zeron-other");
        std::fs::copy(&own, &other).unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A perfectly good executable somewhere else is not this install.
        assert!(
            check_target(&other)
                .unwrap_err()
                .contains("not this install's binary")
        );
        // Writable by others: refused before anything else.
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            check_target(&other)
                .unwrap_err()
                .contains("not writable by others")
        );
        assert!(check_target(dir.path()).is_err(), "a directory");
        assert!(check_target(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn a_rollback_goes_to_the_running_binary_except_on_macos_where_it_is_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let exe = std::env::current_exe().unwrap();
        let target = rollback_exe(dir.path());
        if cfg!(target_os = "macos") {
            assert!(target.starts_with(rollback_dir(dir.path())));
            assert_eq!(
                std::fs::metadata(&target).unwrap().len(),
                std::fs::metadata(&exe).unwrap().len(),
                "a copy of the running binary"
            );
            assert_eq!(rollback_exe(dir.path()), target, "reused, not re-copied");
        } else {
            assert_eq!(target, exe);
            assert!(!rollback_dir(dir.path()).exists());
        }
    }

    #[test]
    fn kept_rollback_binaries_are_pruned_after_a_commit_except_the_running_one() {
        let dir = tempfile::tempdir().unwrap();
        let kept = rollback_dir(dir.path());
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(kept.join("zeron-0.2.98"), b"old").unwrap();
        std::fs::write(kept.join("zeron-0.2.99"), b"older").unwrap();
        prune_rollback_copies(dir.path());
        assert_eq!(std::fs::read_dir(&kept).unwrap().count(), 0);
        // Nothing to prune is not an error.
        prune_rollback_copies(&dir.path().join("missing"));
    }
    use crate::handoff::{is_cloexec, read_adoption_from_env};
    use crate::{EngineProfile, HarnessId, default_registry};

    struct Rig {
        _dir: TempDir,
        core: EngineCore,
        _listener: std::net::TcpListener,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let core = EngineCore::assemble_with_profile(
            EngineProfile::local(dir.path()).unwrap(),
            Arc::new(default_registry()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        core.set_handoff_listener(listener.as_raw_fd());
        Rig {
            _dir: dir,
            core,
            _listener: listener,
        }
    }

    /// An executable that answers the preflight like a real new binary would.
    fn fake_binary(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("zeron-next");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn good_binary(dir: &Path) -> std::path::PathBuf {
        fake_binary(dir, &format!("echo handoff-ok {MANIFEST_VERSION}"))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_engine_that_serves_no_ipc_cannot_hand_off() {
        let dir = tempfile::tempdir().unwrap();
        let core = EngineCore::assemble_with_profile(
            EngineProfile::local(dir.path()).unwrap(),
            Arc::new(default_registry()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let new_exe = good_binary(dir.path());
        let outcome = core.handoff_with(&new_exe, |_, _, _| unreachable!()).await;
        assert!(matches!(outcome, HandoffError::Unsupported(_)), "{outcome}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_preflight_touches_nothing() {
        let rig = rig();
        let terminal = rig
            .core
            .terminals
            .open_with_shell("/tmp", 80, 24, Some("/bin/sh"))
            .unwrap();
        for body in [
            "echo hello",        // does not speak handoff
            "echo handoff-ok 0", // reads a manifest version older than ours
            "exit 3",            // fails
            "echo handoff-ok not-a-number",
        ] {
            let new_exe = fake_binary(rig._dir.path(), body);
            let outcome = rig
                .core
                .handoff_with(&new_exe, |_, _, _| unreachable!("must not exec"))
                .await;
            assert!(
                matches!(outcome, HandoffError::Preflight(_)),
                "{body}: {outcome}"
            );
        }
        let missing = rig._dir.path().join("does-not-exist");
        assert!(matches!(
            rig.core
                .handoff_with(&missing, |_, _, _| unreachable!())
                .await,
            HandoffError::Preflight(_)
        ));
        let not_executable = rig._dir.path().join("plain-file");
        std::fs::write(&not_executable, "x").unwrap();
        assert!(matches!(
            rig.core
                .handoff_with(&not_executable, |_, _, _| unreachable!())
                .await,
            HandoffError::Preflight(_)
        ));
        // Nothing was frozen: the terminal still works and new ones open.
        rig.core
            .terminals
            .write_bytes(&terminal.id, b"echo fine\n")
            .unwrap();
        let second = rig
            .core
            .terminals
            .open_with_shell("/tmp", 80, 24, Some("/bin/sh"))
            .unwrap();
        rig.core.terminals.close(&second.id).unwrap();
        rig.core.terminals.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_exec_gets_the_manifest_and_a_failed_exec_puts_everything_back() {
        let rig = rig();
        let terminal = rig
            .core
            .terminals
            .open_with_shell("/tmp", 80, 24, Some("/bin/sh"))
            .unwrap();
        let new_exe = good_binary(rig._dir.path());
        let listener_fd = rig._listener.as_raw_fd();
        let lock_fd = rig.core._instance_lock.raw_fd();
        assert!(is_cloexec(listener_fd) && is_cloexec(lock_fd));

        let seen = std::cell::RefCell::new(None);
        let outcome = rig
            .core
            .handoff_with(&new_exe, |exe, manifest_fd, attempt| {
                assert_eq!(exe, new_exe);
                assert_eq!(attempt, 1, "a fresh engine's first handoff is attempt 1");
                // What the successor would inherit, checked at the last moment.
                assert!(!is_cloexec(listener_fd), "the IPC listener is carried");
                assert!(!is_cloexec(lock_fd), "the instance lock is carried");
                assert!(!is_cloexec(manifest_fd), "the manifest is carried");
                let manifest = Manifest::read_fd(manifest_fd).unwrap();
                assert_eq!(manifest.version, MANIFEST_VERSION);
                assert_eq!(manifest.listener_fd, listener_fd);
                assert_eq!(manifest.lock_fd, lock_fd);
                assert_eq!(manifest.terminals.len(), 1);
                assert_eq!(manifest.terminals[0].id, terminal.id);
                assert!(
                    !is_cloexec(manifest.terminals[0].master_fd),
                    "masters are carried"
                );
                assert_eq!(manifest.from_version, zeron_update::current_version());
                *seen.borrow_mut() = Some(manifest.terminals[0].master_fd);
                std::io::Error::other("simulated exec failure")
            })
            .await;
        assert!(matches!(outcome, HandoffError::Exec(_)), "{outcome}");

        // The engine is exactly as before.
        let master_fd = seen.borrow().expect("the exec closure ran");
        assert!(is_cloexec(listener_fd), "listener is close-on-exec again");
        assert!(is_cloexec(lock_fd), "lock is close-on-exec again");
        assert!(is_cloexec(master_fd), "master is close-on-exec again");
        assert!(!rig.core.handoff_running.load(Ordering::SeqCst));
        rig.core
            .terminals
            .write_bytes(&terminal.id, b"echo alive-$((1+1))\n")
            .expect("the terminal works after a failed handoff");
        let second = rig
            .core
            .terminals
            .open_with_shell("/tmp", 80, 24, Some("/bin/sh"))
            .expect("terminals can be opened again");
        rig.core.terminals.close(&second.id).unwrap();
        // Token refreshes resume.
        let auth = rig.core.auth();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), auth.quiesce_refresh())
                .await
                .is_ok(),
            "the refresh gate was released"
        );
        rig.core.terminals.shutdown();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_handoff_while_one_runs_is_refused() {
        let rig = rig();
        // A slow preflight keeps the first handoff in flight.
        let slow = fake_binary(
            rig._dir.path(),
            &format!("sleep 1; echo handoff-ok {MANIFEST_VERSION}"),
        );
        let (first, second) = tokio::join!(
            rig.core
                .handoff_with(&slow, |_, _, _| std::io::Error::other("stop")),
            async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                rig.core
                    .handoff_with(&slow, |_, _, _| unreachable!("the second must not exec"))
                    .await
            }
        );
        assert!(
            matches!(second, HandoffError::Busy(ref why) if why.contains("already")),
            "{second}"
        );
        assert!(matches!(first, HandoffError::Exec(_)), "{first}");
        rig.core.terminals.shutdown();
    }

    #[test]
    fn no_adoption_without_the_environment() {
        // The variable is only ever set by a predecessor's exec.
        if std::env::var_os(crate::handoff::HANDOFF_FD_ENV).is_none() {
            assert!(read_adoption_from_env().unwrap().is_none());
        }
    }
}
