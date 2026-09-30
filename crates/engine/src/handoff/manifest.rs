//! The handoff manifest: what the old engine image gives the new one.

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::terminals::handoff::TerminalHandoff;

/// The manifest schema this build writes and can read. Additive changes keep
/// the number; a change an older reader cannot understand bumps it, and the
/// reader refuses rather than guess. A new field whose ABSENCE would change what
/// an older reader does with the rest (it would skip closing, locking or
/// validating something it cannot see) is not additive: bump the version.
pub const MANIFEST_VERSION: u32 = 1;
/// The fd number of the manifest, in the successor's environment.
pub const HANDOFF_FD_ENV: &str = "ZERON_HANDOFF_FD";
/// How many handoff/rollback execs led to this image (rollback-loop guard).
pub const HANDOFF_ATTEMPT_ENV: &str = "ZERON_HANDOFF_ATTEMPT";
/// Set only by a rollback: the version whose adoption failed. The image that
/// takes the handoff back uses it to not hand off to that same version again.
pub const HANDOFF_ROLLED_BACK_ENV: &str = "ZERON_HANDOFF_ROLLED_BACK_FROM";
/// A manifest is small; refuse to slurp anything absurd from a bad fd.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;

/// One running agent turn, owned by the successor after the handoff. The
/// contents are defined by the sessions engine; the manifest only carries them.
pub use crate::sessions::handoff::{PendingInputRecord, RunHandoff};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    /// The binary that wrote this manifest — the rollback target.
    pub from_exe: PathBuf,
    pub from_version: String,
    pub listener_fd: i32,
    pub lock_fd: i32,
    #[serde(default)]
    pub terminals: Vec<TerminalHandoff>,
    #[serde(default)]
    pub runs: Vec<RunHandoff>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("handoff manifest unreadable: {0}")]
    Unreadable(String),
    #[error("handoff manifest version {found} is newer than this build understands ({supported})")]
    Unsupported { found: u32, supported: u32 },
    #[error("handoff manifest corrupt: {0}")]
    Corrupt(String),
}

impl Manifest {
    /// Serialize into an unlinked anonymous file, rewound, whose fd survives
    /// `execve`. It holds secrets (agent server passwords), so it is never a
    /// named file.
    pub fn write_anon(&self) -> std::io::Result<OwnedFd> {
        let mut file = tempfile::tempfile()?;
        serde_json::to_writer(&mut file, self)?;
        file.flush()?;
        file.seek(SeekFrom::Start(0))?;
        let fd = OwnedFd::from(file);
        super::set_inheritable(fd.as_raw_fd(), true)?;
        Ok(fd)
    }

    /// Read a manifest from `fd` without consuming it: the offset is rewound,
    /// so a rollback can read the same fd again.
    pub fn read_fd(fd: RawFd) -> Result<Self, ManifestError> {
        let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if dup < 0 {
            return Err(ManifestError::Unreadable(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let mut file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(dup) });
        file.seek(SeekFrom::Start(0))
            .map_err(|e| ManifestError::Unreadable(e.to_string()))?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_MANIFEST_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| ManifestError::Unreadable(e.to_string()))?;
        let _ = file.seek(SeekFrom::Start(0));
        Self::parse(&bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| ManifestError::Corrupt(e.to_string()))?;
        // The version is checked before the typed parse: a newer writer may
        // have changed the shape of fields this build does not know.
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| ManifestError::Corrupt("no version".into()))?;
        if version > u64::from(MANIFEST_VERSION) {
            return Err(ManifestError::Unsupported {
                found: u32::try_from(version).unwrap_or(u32::MAX),
                supported: MANIFEST_VERSION,
            });
        }
        serde_json::from_value(value).map_err(|e| ManifestError::Corrupt(e.to_string()))
    }
}

/// A handoff this process was started to complete.
pub struct Adoption {
    pub manifest: Manifest,
    /// How many handoff/rollback execs led here (1 = first adoption).
    pub attempt: u32,
    /// The version that failed to adopt this handoff and handed it back (only
    /// on the image a rollback lands in).
    pub rolled_back_from: Option<String>,
    manifest_fd: RawFd,
    /// The inherited descriptors this adoption is responsible for — the
    /// numbers the manifest names that really are what it says (a listening
    /// socket, the lock file, pty masters, the manifest file), checked ONCE,
    /// before anything else in this process opens files. Everything that
    /// later flags or closes descriptors uses only this set: a stale or
    /// garbled manifest can never make it touch an fd that has since been
    /// reused for something else.
    originals: Vec<RawFd>,
}

/// The descriptors `manifest` names that are verifiably what it claims.
fn validated_originals(manifest: &Manifest, manifest_fd: RawFd) -> Vec<RawFd> {
    use super::fds::{is_listening_socket, is_regular_file};
    use crate::terminals::pty_unix::is_pty_master;
    let mut fds = std::collections::BTreeSet::new();
    let candidates = [
        (
            manifest.listener_fd,
            is_listening_socket(manifest.listener_fd),
        ),
        (manifest.lock_fd, is_regular_file(manifest.lock_fd)),
        (manifest_fd, is_regular_file(manifest_fd)),
    ];
    for (fd, valid) in candidates {
        if valid {
            fds.insert(fd);
        } else {
            tracing::warn!(
                fd,
                "the handoff manifest names a descriptor that is not what it says"
            );
        }
    }
    // Agent pipes: stdin/stdout/stderr of each adopted run (pipes or, for
    // some harnesses, sockets). A descriptor of any other kind is not ours.
    for fd in manifest.runs.iter().flat_map(|run| {
        run.fds()
            .into_iter()
            .filter(|fd| !run.harness.extra_fds.contains(fd))
    }) {
        if super::fds::is_pipe_like(fd) {
            fds.insert(fd);
        } else {
            tracing::warn!(
                fd,
                "the handoff manifest names a run descriptor that is not a pipe"
            );
        }
    }
    // Descriptors a harness keeps for itself (Cursor's store lease): pipes or
    // regular files, nothing else.
    for fd in manifest
        .runs
        .iter()
        .flat_map(|run| run.harness.extra_fds.iter().copied())
    {
        if super::fds::is_pipe_like(fd) || super::fds::is_regular_file(fd) {
            fds.insert(fd);
        } else {
            tracing::warn!(
                fd,
                "the handoff manifest names an extra run descriptor that is not a pipe or a file"
            );
        }
    }
    for terminal in manifest
        .terminals
        .iter()
        .filter(|terminal| !terminal.exited)
    {
        if is_pty_master(terminal.master_fd) {
            fds.insert(terminal.master_fd);
        } else {
            tracing::warn!(
                terminal = %terminal.id,
                fd = terminal.master_fd,
                "the handoff manifest names a descriptor that is not a pty master"
            );
        }
    }
    fds.into_iter().collect()
}

impl Adoption {
    /// Read the manifest at `manifest_fd` and validate what it names.
    pub fn from_manifest_fd(manifest_fd: RawFd, attempt: u32) -> Result<Self, ManifestError> {
        let manifest = Manifest::read_fd(manifest_fd)?;
        let originals = validated_originals(&manifest, manifest_fd);
        Ok(Self {
            manifest,
            attempt,
            rolled_back_from: None,
            manifest_fd,
            originals,
        })
    }

    pub fn manifest_fd(&self) -> RawFd {
        self.manifest_fd
    }

    fn originals(&self) -> &[RawFd] {
        &self.originals
    }

    /// The predecessor made its descriptors inheritable for the exec. Until
    /// the adoption commits, every child this image spawns (git, harness
    /// processes) would inherit them too, so make them close-on-exec again —
    /// [`Self::release_for_rollback`] undoes this if the boot fails.
    pub fn secure_originals(&self) {
        for &fd in self.originals() {
            let _ = super::set_inheritable(fd, false);
        }
    }

    /// The boot failed and the predecessor is about to be exec'd again: make
    /// what it needs to adopt inheritable once more.
    pub fn release_for_rollback(&self) {
        for &fd in self.originals() {
            let _ = super::set_inheritable(fd, true);
        }
    }

    /// Adoption succeeded. This image works on duplicates of everything it
    /// inherited (so a failed boot could hand the originals back); now close
    /// the originals — the listener, the lock file, every terminal master,
    /// every agent pipe — and the manifest itself, so nothing stale is left for children to
    /// inherit or for a later crash to mistake for a handoff.
    pub fn commit(self) {
        for &fd in self.originals() {
            unsafe { libc::close(fd) };
        }
    }
}

/// Read the handoff this process was exec'd to complete, if any.
///
/// Does not clear the environment variables (`std::env::remove_var` is unsafe
/// once threads exist): the binary's `main` clears [`HANDOFF_FD_ENV`] and
/// [`HANDOFF_ATTEMPT_ENV`] and [`HANDOFF_ROLLED_BACK_ENV`] itself, single-threaded, right after calling this,
/// so agents and other children never inherit them.
pub fn read_adoption_from_env() -> Result<Option<Adoption>, ManifestError> {
    let Some(raw) = std::env::var_os(HANDOFF_FD_ENV) else {
        return Ok(None);
    };
    let fd: RawFd = raw
        .to_str()
        .and_then(|value| value.trim().parse().ok())
        .ok_or_else(|| ManifestError::Unreadable(format!("{HANDOFF_FD_ENV} is not a number")))?;
    if fd < 3 {
        return Err(ManifestError::Unreadable(format!(
            "{HANDOFF_FD_ENV}={fd} is a standard descriptor"
        )));
    }
    let attempt = std::env::var(HANDOFF_ATTEMPT_ENV)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(1);
    let mut adoption = Adoption::from_manifest_fd(fd, attempt)?;
    adoption.rolled_back_from = std::env::var(HANDOFF_ROLLED_BACK_ENV)
        .ok()
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty());
    Ok(Some(adoption))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            version: MANIFEST_VERSION,
            from_exe: PathBuf::from("/opt/zeron/0.2.99/zeron"),
            from_version: "0.2.99".into(),
            listener_fd: 7,
            lock_fd: 8,
            terminals: Vec::new(),
            runs: Vec::new(),
        }
    }

    fn anon_with(bytes: &[u8]) -> OwnedFd {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        OwnedFd::from(file)
    }

    #[test]
    fn manifest_round_trips_through_an_anonymous_file_and_can_be_read_twice() {
        let fd = sample().write_anon().unwrap();
        assert!(
            !super::super::is_cloexec(fd.as_raw_fd()),
            "must survive exec"
        );
        for _ in 0..2 {
            let back = Manifest::read_fd(fd.as_raw_fd()).unwrap();
            assert_eq!(back.version, MANIFEST_VERSION);
            assert_eq!(back.listener_fd, 7);
            assert_eq!(back.lock_fd, 8);
            assert_eq!(back.runs, sample().runs);
        }
    }

    #[test]
    fn a_newer_manifest_is_refused_not_guessed_at() {
        // Even when the newer writer changed the shape of a field.
        let fd = anon_with(br#"{"version":999,"fromExe":42,"listenerFd":"three"}"#);
        assert!(matches!(
            Manifest::read_fd(fd.as_raw_fd()),
            Err(ManifestError::Unsupported {
                found: 999,
                supported: 1
            })
        ));
    }

    #[test]
    fn garbage_and_truncated_manifests_are_errors_not_panics() {
        for bytes in [
            &b""[..],
            b"{",
            b"\xff\xfe",
            b"null",
            b"[]",
            br#"{"version":1}"#,
            br#"{"version":"1"}"#,
            br#"{"version":1,"fromExe":"/x","fromVersion":"1","listenerFd":3}"#,
        ] {
            let fd = anon_with(bytes);
            assert!(
                matches!(
                    Manifest::read_fd(fd.as_raw_fd()),
                    Err(ManifestError::Corrupt(_)) | Err(ManifestError::Unreadable(_))
                ),
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn unknown_fields_are_ignored_and_optional_lists_default() {
        let fd = anon_with(
            br#"{"version":1,"fromExe":"/x","fromVersion":"1","listenerFd":3,"lockFd":4,"futureThing":{"a":1}}"#,
        );
        let manifest = Manifest::read_fd(fd.as_raw_fd()).unwrap();
        assert!(manifest.terminals.is_empty() && manifest.runs.is_empty());
    }

    #[test]
    fn only_verified_descriptors_are_ever_flagged_or_closed() {
        use std::os::fd::IntoRawFd;
        // What a stale or garbled manifest could name: descriptors that are
        // open in this process but are NOT a listener / lock file / pty master.
        let unrelated_file = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let unrelated_socket = socket.into_raw_fd();
        let mut manifest = sample();
        manifest.listener_fd = unrelated_file; // not a listening socket
        manifest.lock_fd = unrelated_socket; // not a regular file
        manifest.terminals = vec![crate::terminals::handoff::TerminalHandoff {
            id: "t".into(),
            cwd: "/".into(),
            shell: "sh".into(),
            pid: 1234,
            master_fd: unrelated_file,
            seq: 0,
            replay: Vec::new(),
            exited: false,
            idle_ms: 0,
            script: None,
        }];
        let fd = manifest.write_anon().unwrap();
        let adoption = Adoption::from_manifest_fd(fd.as_raw_fd(), 1).unwrap();
        // Only the manifest file itself is verifiable here.
        assert_eq!(adoption.originals(), &[fd.as_raw_fd()]);
        let cloexec = |fd: RawFd| unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC != 0;
        let before = (cloexec(unrelated_file), cloexec(unrelated_socket));
        adoption.secure_originals();
        adoption.release_for_rollback();
        assert_eq!(
            (cloexec(unrelated_file), cloexec(unrelated_socket)),
            before,
            "unrelated descriptors keep their flags"
        );
        let manifest_fd = fd.into_raw_fd();
        assert_eq!(adoption.manifest_fd(), manifest_fd);
        // Identify the file by (device, inode): another test thread may reuse
        // the descriptor NUMBER the moment it is closed.
        let identity = |fd: RawFd| {
            let mut stat: libc::stat = unsafe { std::mem::zeroed() };
            (unsafe { libc::fstat(fd, &mut stat) } == 0).then_some((stat.st_dev, stat.st_ino))
        };
        let manifest_identity = identity(manifest_fd).expect("the manifest fd is open");
        adoption.commit();
        assert_ne!(
            identity(manifest_fd),
            Some(manifest_identity),
            "the manifest itself is closed at commit"
        );
        for still_open in [unrelated_file, unrelated_socket] {
            assert!(
                unsafe { libc::fcntl(still_open, libc::F_GETFD) } >= 0,
                "an unrelated descriptor {still_open} must survive the commit"
            );
            unsafe { libc::close(still_open) };
        }
    }

    #[test]
    fn reading_a_bad_fd_is_unreadable() {
        assert!(matches!(
            Manifest::read_fd(9999),
            Err(ManifestError::Unreadable(_))
        ));
    }
}
