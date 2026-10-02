//! Login-shell PATH snapshot.
//!
//! GUI/service launches (Dock, Finder, launchd, systemd) never run the user's
//! shell init, so the daemon's own PATH misses everything the shell shapes:
//! nvm's shell function, fnm multishells, asdf/mise shims, custom npm
//! prefixes, nix profiles, `~/.zshrc` exports. The hardcoded known-location
//! lists in the resolvers cover the common managers, but the only fix that
//! works for *any* setup is asking the user's actual shell: spawn it once as
//! an interactive login shell, have it print its environment between markers,
//! and keep the PATH it reports. If `codex`/`claude` runs in their terminal,
//! it resolves here too.
//!
//! The snapshot is captured once per process (cached, including a negative
//! result) and is defensive about hostile shell init:
//! - `-lic` first (interactive login — nvm and friends load in rc files),
//!   falling back to `-lc` if that produces nothing (some rc files hang or
//!   `exec` a multiplexer when interactive).
//! - Nonblocking stdout reads stop at the end marker, EOF, output limit, or
//!   deadline, even when descendants keep the pipe open.
//! - Every attempt owns a process group, killed on every exit path before
//!   reaping the shell. Descendants that stay in that group are killed too.
//! - An inherited `ZERON_RESOLVING_ENVIRONMENT` suppresses nested snapshots.
//!
//! Set `ZERON_NO_LOGIN_SHELL=1` to disable the snapshot entirely.

use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;

static CACHE: OnceLock<Option<OsString>> = OnceLock::new();

/// The PATH the user's login shell reports, captured once and cached for the
/// life of the process. `None` when disabled, non-unix, no usable shell, or
/// the shell never produced a parseable snapshot.
pub fn login_shell_path() -> Option<&'static OsStr> {
    #[cfg(unix)]
    {
        CACHE.get_or_init(unix::capture).as_deref()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Kick off the snapshot on a background thread so the first harness resolve
/// doesn't pay the shell-startup latency inline. Call at daemon startup.
pub fn prewarm() {
    #[cfg(unix)]
    {
        let _ = std::thread::Builder::new()
            .name("zeron-shell-env".into())
            .spawn(|| {
                let _ = login_shell_path();
            });
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsString;
    use std::io::{self, Read};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    const BEGIN_MARKER: &str = "__ZERON_SHELL_ENV_BEGIN__";
    const END_MARKER: &str = "__ZERON_SHELL_ENV_END__";
    /// Bound captured bytes; this is not a subprocess memory limit.
    const MAX_OUTPUT: usize = 2 * 1024 * 1024;
    const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);

    pub(super) fn capture() -> Option<OsString> {
        // OnceLock only protects this process. A program launched by shell
        // init can load this module with a fresh cache and start another probe.
        if ["ZERON_NO_LOGIN_SHELL", "ZERON_RESOLVING_ENVIRONMENT"]
            .iter()
            .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
        {
            return None;
        }
        let shell = user_shell()?;
        snapshot_path(&shell, ATTEMPT_TIMEOUT)
    }

    /// The user's shell: `$SHELL`, then the passwd entry, then well-known
    /// defaults. Non-executables and nologin shells are skipped.
    fn user_shell() -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(s) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
            candidates.push(PathBuf::from(s));
        }
        // systemd/launchd services often start without SHELL — passwd has it.
        if let Some(p) = passwd_shell() {
            candidates.push(p);
        }
        candidates.push(PathBuf::from("/bin/zsh"));
        candidates.push(PathBuf::from("/bin/bash"));
        candidates.push(PathBuf::from("/bin/sh"));
        candidates.into_iter().find(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string());
            let blocked = matches!(name.as_deref(), Some("nologin" | "false") | None);
            !blocked && is_executable(p)
        })
    }

    fn passwd_shell() -> Option<PathBuf> {
        // SAFETY: getpwuid's static buffer is only read here, and callers are
        // serialized through the OnceLock init above.
        unsafe {
            let pw = libc::getpwuid(libc::getuid());
            if pw.is_null() || (*pw).pw_shell.is_null() {
                return None;
            }
            let shell = std::ffi::CStr::from_ptr((*pw).pw_shell);
            (!shell.to_bytes().is_empty())
                .then(|| PathBuf::from(std::ffi::OsStr::from_bytes(shell.to_bytes())))
        }
    }

    fn is_executable(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }

    /// Flag sets to try, most-loaded first. csh/tcsh reject `-l` combined with
    /// `-c`; fish runs config.fish for every invocation, so `-l` alone loads
    /// everything without interactive-mode side effects.
    fn attempt_flag_sets(shell: &Path) -> Vec<Vec<&'static str>> {
        let name = shell
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        match name.as_str() {
            "csh" | "tcsh" => vec![vec!["-c"]],
            "fish" => vec![vec!["-l", "-c"], vec!["-c"]],
            _ => vec![vec!["-l", "-i", "-c"], vec!["-l", "-c"]],
        }
    }

    /// Run `<shell> <flags> 'echo BEGIN; env; echo END'` per flag set until one
    /// yields a parseable PATH.
    pub(super) fn snapshot_path(shell: &Path, timeout: Duration) -> Option<OsString> {
        let script = format!("echo {BEGIN_MARKER}; env; echo {END_MARKER}");
        for flags in attempt_flag_sets(shell) {
            let output = run_and_capture(shell, &flags, &script, timeout);
            if let Some(path) = parse_snapshot_path(&output) {
                return Some(path);
            }
        }
        None
    }

    /// Own the group until cleanup. Never reap the leader before signaling:
    /// retaining its PID prevents it from being reused for an unrelated group.
    struct Probe(Child);

    impl Drop for Probe {
        fn drop(&mut self) {
            // SAFETY: setsid created a group whose positive ID is
            // this still-unreaped child's PID, never the caller's group.
            // SIGKILL also handles shell init that ignores SIGTERM.
            unsafe { libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }

    /// Read without a worker thread: even a descendant that leaves the group
    /// and retains stdout cannot keep a blocked reader alive after the deadline.
    fn run_and_capture(shell: &Path, flags: &[&str], script: &str, timeout: Duration) -> Vec<u8> {
        let mut cmd = std::process::Command::new(shell);
        cmd.args(flags)
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Let rc files and nested Zeron processes skip this probe.
            .env("ZERON_RESOLVING_ENVIRONMENT", "1")
            .env("TERM", "dumb");
        // Start a session as well as a process group, so interactive shells
        // cannot use the caller's controlling terminal to enable job control.
        // SAFETY: setsid is async-signal-safe; no allocation or locks in the
        // child hook. A setup failure makes spawn fail before running init.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let Ok(child) = cmd.spawn() else {
            return Vec::new();
        };
        let mut probe = Probe(child);
        let Some(mut stdout) = probe.0.stdout.take() else {
            return Vec::new();
        };
        // SAFETY: stdout owns this live descriptor. Preserve the existing
        // flags and make only the pipe's read end nonblocking.
        let fd = stdout.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Vec::new();
        }
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        let mut chunk = [0u8; 8192];
        while Instant::now() < deadline && output.len() < MAX_OUTPUT {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let from = output.len().saturating_sub(END_MARKER.len());
                    let n = n.min(MAX_OUTPUT - output.len());
                    output.extend_from_slice(&chunk[..n]);
                    if find_subslice(&output[from..], END_MARKER.as_bytes()).is_some() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(
                        Duration::from_millis(20)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(_) => break,
            }
        }
        // Drop closes stdout and kills the group on success, EOF, timeout,
        // output limit, read/setup failure, and unwinding. No early try_wait:
        // an exited shell with inherited writers uses the same hard deadline.
        output
    }

    /// Extract PATH from the `env` dump between the LAST begin marker and the
    /// first end marker after it (rc noise printed before our command — or a
    /// marker echoed by init itself — lands before the real one).
    fn parse_snapshot_path(output: &[u8]) -> Option<OsString> {
        let begin = rfind_subslice(output, BEGIN_MARKER.as_bytes())?;
        let after = &output[begin + BEGIN_MARKER.len()..];
        let end = find_subslice(after, END_MARKER.as_bytes())?;
        for line in after[..end].split(|b| *b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(value) = line.strip_prefix(b"PATH=")
                && !value.is_empty()
            {
                return Some(OsString::from_vec(value.to_vec()));
            }
        }
        None
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn rfind_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .rposition(|window| window == needle)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::ffi::OsStr;
        use std::os::unix::fs::PermissionsExt;

        fn fake_shell(dir: &Path, body: &str) -> PathBuf {
            let path = dir.join("fake-shell");
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        /// A fake $SHELL skeleton: consume flags, exec the `-c` payload.
        const RUN_PAYLOAD: &str = r#"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-c" ]; then shift; exec /bin/sh -c "$1"; fi
  shift
done
exit 1
"#;

        /// Invoked only by the bounded shell fixtures below, in a fresh test
        /// process. The socket is both a death witness and a cleanup channel:
        /// dropping the listener/accepted stream on assertion failure releases
        /// the child, even if group cleanup regresses. The timeout is a backup.
        #[test]
        fn descendant_fixture() {
            use std::io::Write;
            use std::os::unix::net::UnixStream;

            let Some(socket) = std::env::var_os("ZERON_TEST_DESCENDANT_SOCKET") else {
                return;
            };
            if std::env::var_os("ZERON_TEST_DESCENDANT_ESCAPE").as_deref() == Some(OsStr::new("1"))
            {
                // SAFETY: this isolated fixture intentionally leaves the probe
                // group to test descriptor handling, not group containment.
                assert_ne!(unsafe { libc::setsid() }, -1);
            }
            let mut stream = UnixStream::connect(socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            // SAFETY: getpgrp only reads the current process group ID.
            stream
                .write_all(&unsafe { libc::getpgrp() }.to_ne_bytes())
                .unwrap();
            std::fs::write(
                std::env::var_os("ZERON_TEST_DESCENDANT_READY").unwrap(),
                b"ready",
            )
            .unwrap();
            let _ = stream.read(&mut [0]);
        }

        fn shell_quote(value: &OsStr) -> String {
            format!("'{}'", value.to_str().unwrap().replace('\'', "'\"'\"'"))
        }

        #[derive(Clone, Copy)]
        enum DescendantMode {
            InheritedStdout,
            ClosedStdout,
            EscapedGroup,
        }

        fn check_descendant_cleanup(action: &str, mode: DescendantMode) -> Vec<u8> {
            use std::io::Write;
            use std::os::unix::net::UnixListener;

            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("control");
            let ready = dir.path().join("ready");
            let pid_file = dir.path().join("pid");
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let fixture = format!(
                "ZERON_TEST_DESCENDANT_ESCAPE={} ZERON_TEST_DESCENDANT_SOCKET={} ZERON_TEST_DESCENDANT_READY={} {} --exact shell_env::unix::tests::descendant_fixture --nocapture; :",
                if matches!(mode, DescendantMode::EscapedGroup) {
                    "1"
                } else {
                    "0"
                },
                shell_quote(socket.as_os_str()),
                shell_quote(ready.as_os_str()),
                shell_quote(std::env::current_exe().unwrap().as_os_str()),
            );
            // Two generations: probe shell -> wrapper shell -> Rust fixture.
            // No recursive spawning; all waits have a strict upper bound.
            let script = format!(
                "echo $$ > {pid}; /bin/sh -c {fixture} {redirect} &\n\
                 i=0; while [ ! -f {ready} ] && [ \"$i\" -lt 100 ]; do i=$((i+1)); sleep 0.01; done\n{action}",
                pid = shell_quote(pid_file.as_os_str()),
                fixture = shell_quote(OsStr::new(&fixture)),
                ready = shell_quote(ready.as_os_str()),
                redirect = if matches!(mode, DescendantMode::ClosedStdout) {
                    ">/dev/null"
                } else {
                    ""
                },
            );
            let start = Instant::now();
            let output = run_and_capture(
                Path::new("/bin/sh"),
                &["-c"],
                &script,
                Duration::from_secs(2),
            );
            assert!(start.elapsed() < Duration::from_secs(4));
            let (mut stream, _) = listener
                .accept()
                .expect("descendant connected before cleanup");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut pgid = [0; std::mem::size_of::<libc::pid_t>()];
            stream.read_exact(&mut pgid).unwrap();
            let pgid = libc::pid_t::from_ne_bytes(pgid);
            let pid: libc::pid_t = std::fs::read_to_string(pid_file)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            if matches!(mode, DescendantMode::EscapedGroup) {
                assert_ne!(pgid, pid);
                stream
                    .set_read_timeout(Some(Duration::from_millis(100)))
                    .unwrap();
                let err = stream
                    .read(&mut [0])
                    .expect_err("escaped fixture is still alive");
                assert!(matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ));
                // Explicitly release the escaped process. Drop closes this
                // channel on failure too; no PID-based emergency signals.
                stream.write_all(b"x").unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
            } else {
                assert_eq!(pgid, pid, "descendants share the probe's group");
            }
            // SAFETY: getpgrp only reads the test runner's group ID.
            assert_ne!(pgid, unsafe { libc::getpgrp() });
            assert_eq!(
                stream.read(&mut [0]).unwrap(),
                0,
                "descendant must have exited"
            );
            // SAFETY: waitpid only checks this exact PID and never blocks.
            // Probe must already have reaped its direct child.
            assert_eq!(
                unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
                -1
            );
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            output
        }

        #[test]
        fn kills_descendants_on_timeout() {
            check_descendant_cleanup("wait", DescendantMode::InheritedStdout);
        }

        #[test]
        fn kills_descendants_after_end_marker() {
            let output = check_descendant_cleanup(
                &format!("echo {BEGIN_MARKER}; echo PATH=/probe/bin; echo {END_MARKER}; wait"),
                DescendantMode::InheritedStdout,
            );
            assert_eq!(
                parse_snapshot_path(&output),
                Some(OsString::from("/probe/bin"))
            );
        }

        #[test]
        fn kills_descendants_after_shell_exit_with_inherited_stdout() {
            check_descendant_cleanup("exit 0", DescendantMode::InheritedStdout);
        }

        #[test]
        fn kills_descendants_on_eof() {
            check_descendant_cleanup("exit 0", DescendantMode::ClosedStdout);
        }

        #[test]
        fn returns_when_an_escaped_descendant_keeps_stdout_open() {
            check_descendant_cleanup("exit 0", DescendantMode::EscapedGroup);
        }

        #[test]
        fn kills_descendants_at_output_limit() {
            let output = check_descendant_cleanup(
                "dd if=/dev/zero bs=8192 count=257 2>/dev/null; wait",
                DescendantMode::InheritedStdout,
            );
            assert_eq!(output.len(), MAX_OUTPUT);
        }

        #[test]
        fn parses_path_between_markers() {
            let output = format!(
                "rc noise\n{BEGIN_MARKER}\nHOME=/home/u\nPATH=/custom/bin:/usr/bin\nX=y\n{END_MARKER}\ntrailing"
            );
            let path = parse_snapshot_path(output.as_bytes()).unwrap();
            assert_eq!(path, OsString::from("/custom/bin:/usr/bin"));
        }

        #[test]
        fn ignores_marker_echoed_by_init() {
            // rc noise that happens to contain the begin marker but no PATH
            // after it must not shadow the real snapshot.
            let output =
                format!("{BEGIN_MARKER}\ngarbage\n{BEGIN_MARKER}\nPATH=/real/bin\n{END_MARKER}\n");
            let path = parse_snapshot_path(output.as_bytes()).unwrap();
            assert_eq!(path, OsString::from("/real/bin"));
        }

        #[test]
        fn snapshots_path_from_fake_shell() {
            let dir = tempfile::tempdir().unwrap();
            let shell = fake_shell(
                dir.path(),
                &format!(
                    "#!/bin/sh\nPATH=\"/zeron-test/custom/bin:/usr/bin:/bin\"; export PATH\n{RUN_PAYLOAD}"
                ),
            );
            let path = snapshot_path(&shell, Duration::from_secs(10)).unwrap();
            let path = path.to_string_lossy();
            assert!(path.starts_with("/zeron-test/custom/bin:"), "got: {path}");
        }

        #[test]
        fn falls_back_when_interactive_attempt_hangs() {
            let dir = tempfile::tempdir().unwrap();
            // Simulates rc files that wedge only in interactive mode (`exec
            // tmux` and friends): replace the probe with a bounded sleep
            // when -i is present.
            let shell = fake_shell(
                dir.path(),
                &format!(
                    "#!/bin/sh\ncase \" $* \" in *\" -i \"*) exec sleep 2;; esac\nPATH=\"/zeron-test/fallback/bin:/usr/bin:/bin\"; export PATH\n{RUN_PAYLOAD}"
                ),
            );
            let start = Instant::now();
            let path = snapshot_path(&shell, Duration::from_millis(400)).unwrap();
            assert!(
                path.to_string_lossy()
                    .starts_with("/zeron-test/fallback/bin"),
                "got: {}",
                path.to_string_lossy()
            );
            // First attempt burned ~400ms then was killed; the whole resolve
            // must not have waited out the sleep.
            assert!(start.elapsed() < Duration::from_secs(5));
        }

        #[test]
        fn gives_up_on_a_shell_that_never_answers() {
            let dir = tempfile::tempdir().unwrap();
            let shell = fake_shell(dir.path(), "#!/bin/sh\nexec sleep 2\n");
            let start = Instant::now();
            assert!(snapshot_path(&shell, Duration::from_millis(300)).is_none());
            assert!(start.elapsed() < Duration::from_secs(5));
        }
    }
}
