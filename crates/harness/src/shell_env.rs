//! Login-shell environment snapshot.
//!
//! GUI/service launches (Dock, Finder, launchd, systemd) never run the user's
//! shell init, so the daemon's own PATH misses everything the shell shapes:
//! nvm's shell function, fnm multishells, asdf/mise shims, custom npm
//! prefixes, nix profiles, `~/.zshrc` exports. The hardcoded known-location
//! lists in the resolvers cover the common managers, but the only fix that
//! works for *any* setup is asking the user's actual shell: spawn it once as
//! an interactive login shell, have it print its environment between markers,
//! and keep the PATH it reports. If `codex`/`claude` runs in their terminal,
//! it resolves here too. A few other variables the rc files commonly own
//! (corporate CA bundles, the SSH agent socket, the locale) are kept as well
//! and handed to agent children that lack them ([`child_env_defaults`]).
//!
//! The snapshot is captured once per process (cached, including a negative
//! result) and is defensive about hostile shell init:
//! - `-lic` first (interactive login — nvm and friends load in rc files),
//!   falling back to `-lc` if that produces nothing (some rc files hang or
//!   `exec` a multiplexer when interactive).
//! - The shell runs in its own session, without a controlling terminal: an
//!   interactive shell cannot take over the terminal zeron was started from.
//! - Output is read on a side thread into a shared buffer; the poll loop
//!   returns as soon as the end marker appears, so init that blocks *after*
//!   printing (or grandchildren inheriting the pipe) can't wedge us.
//! - A hard per-attempt timeout kills the shell, and the candidate shells
//!   ($SHELL, the passwd entry, the platform default) share one overall
//!   budget. macOS falls back to launchd's PATH when no shell answers.
//!
//! Set `ZERON_NO_LOGIN_SHELL=1` to disable the snapshot entirely.

use std::ffi::{OsStr, OsString};
use std::sync::OnceLock;

/// Variables besides PATH that user shell init commonly owns and agent
/// children need: TLS trust for corporate proxies (OpenSSL, Node, Python),
/// the SSH agent for git over SSH, the locale, and mise's record of the PATH
/// entries it added (it must travel with the shell's PATH, or `mise x` can
/// rebuild PATH behind a same-named wrapper that then re-runs itself).
#[cfg_attr(not(unix), allow(dead_code))]
const CAPTURED_VARS: [&str; 9] = [
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
    "SSH_AUTH_SOCK",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "__MISE_DIFF",
];

const LOCALE_VARS: [&str; 3] = ["LANG", "LC_ALL", "LC_CTYPE"];

#[derive(Debug, Default)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Snapshot {
    path: Option<OsString>,
    vars: Vec<(OsString, OsString)>,
}

static CACHE: OnceLock<Option<Snapshot>> = OnceLock::new();

#[cfg(unix)]
fn snapshot() -> Option<&'static Snapshot> {
    CACHE.get_or_init(unix::capture).as_ref()
}

#[cfg(not(unix))]
fn snapshot() -> Option<&'static Snapshot> {
    CACHE.get_or_init(|| None).as_ref()
}

/// The PATH the user's login shell reports, captured once and cached for the
/// life of the process. `None` when disabled, non-unix, no usable shell, or
/// the shell never produced a parseable snapshot.
pub fn login_shell_path() -> Option<&'static OsStr> {
    snapshot()?.path.as_deref()
}

/// The PATH the user's own sessions get, beyond this process's: the login
/// shell's on Unix; on Windows the persisted user + machine PATH, read fresh
/// so a toolchain installed after zeron started is found without a restart.
pub(crate) fn user_path() -> Option<OsString> {
    #[cfg(windows)]
    {
        crate::windows_process::registry_path()
    }
    #[cfg(not(windows))]
    {
        login_shell_path().map(OsString::from)
    }
}

/// Variables an agent child should get because our own environment lacks
/// them: the login shell's values for [`CAPTURED_VARS`], and on macOS a UTF-8
/// `LC_CTYPE` when a Dock launch left no locale at all (Node and Python
/// otherwise fall back to ASCII for file names and output).
pub(crate) fn child_env_defaults() -> Vec<(OsString, OsString)> {
    let captured = snapshot().map(|s| s.vars.as_slice()).unwrap_or_default();
    defaults_for(
        captured,
        |key| std::env::var_os(key).is_some_and(|v| !v.is_empty()),
        cfg!(target_os = "macos"),
    )
}

fn defaults_for(
    captured: &[(OsString, OsString)],
    present: impl Fn(&OsStr) -> bool,
    macos: bool,
) -> Vec<(OsString, OsString)> {
    let mut defaults: Vec<(OsString, OsString)> = captured
        .iter()
        .filter(|(key, value)| !value.is_empty() && !present(key))
        .cloned()
        .collect();
    let has_locale =
        |key: &str| present(OsStr::new(key)) || defaults.iter().any(|(k, _)| k.as_os_str() == key);
    if macos && !LOCALE_VARS.iter().any(|key| has_locale(key)) {
        defaults.push(("LC_CTYPE".into(), "en_US.UTF-8".into()));
    }
    defaults
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
    use super::{CAPTURED_VARS, Snapshot};
    use std::ffi::OsString;
    use std::io::Read;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const BEGIN_MARKER: &str = "__ZERON_SHELL_ENV_BEGIN__";
    const END_MARKER: &str = "__ZERON_SHELL_ENV_END__";
    /// Enough for any sane environment; a runaway rc file can't OOM us.
    const MAX_OUTPUT: usize = 2 * 1024 * 1024;
    const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);
    /// All candidate shells together: the first resolve may block on this.
    const TOTAL_BUDGET: Duration = Duration::from_secs(10);
    /// After the shell exits, wait this long for the pipe to flush.
    const EXIT_FLUSH_GRACE: Duration = Duration::from_millis(250);

    pub(super) fn capture() -> Option<Snapshot> {
        if std::env::var_os("ZERON_NO_LOGIN_SHELL").is_some_and(|v| !v.is_empty()) {
            return None;
        }
        let deadline = Instant::now() + TOTAL_BUDGET;
        for shell in user_shells() {
            if Instant::now() >= deadline {
                break;
            }
            if let Some(snapshot) = snapshot_shell(&shell, ATTEMPT_TIMEOUT, deadline) {
                return Some(snapshot);
            }
            tracing::debug!(shell = %shell.display(), "login shell produced no environment");
        }
        #[cfg(target_os = "macos")]
        if let Some(path) = launchctl_path() {
            return Some(Snapshot {
                path: Some(path),
                vars: Vec::new(),
            });
        }
        None
    }

    /// The user's shells, most specific first: `$SHELL`, then the passwd
    /// entry, then well-known defaults. Non-executables and nologin shells
    /// are skipped; each is tried until one answers.
    fn user_shells() -> Vec<PathBuf> {
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
        let mut seen = std::collections::HashSet::new();
        candidates
            .into_iter()
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string());
                let blocked = matches!(name.as_deref(), Some("nologin" | "false") | None);
                !blocked && is_executable(p) && seen.insert(p.clone())
            })
            .collect()
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

    fn shell_name(shell: &Path) -> String {
        shell
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// Flag sets to try, most-loaded first. csh/tcsh reject `-l` combined with
    /// `-c`; fish runs config.fish for every invocation, so `-l` alone loads
    /// everything without interactive-mode side effects.
    fn attempt_flag_sets(shell: &Path) -> Vec<Vec<&'static str>> {
        match shell_name(shell).as_str() {
            "csh" | "tcsh" => vec![vec!["-c"]],
            "fish" => vec![vec!["-l", "-c"], vec!["-c"]],
            _ => vec![vec!["-l", "-i", "-c"], vec!["-l", "-c"]],
        }
    }

    /// The probe command in the shell's own language. Nushell discards the
    /// values of non-final statements (so `echo` prints nothing) and has no
    /// `env` builtin: print the markers and run the external `env`.
    fn script(shell: &Path) -> String {
        if shell_name(shell) == "nu" {
            format!("print '{BEGIN_MARKER}'; ^env; print '{END_MARKER}'")
        } else {
            format!("echo {BEGIN_MARKER}; env; echo {END_MARKER}")
        }
    }

    /// Run the probe per flag set until one yields a parseable PATH. Each
    /// attempt gets `timeout`, cut short by the shared `deadline`.
    fn snapshot_shell(shell: &Path, timeout: Duration, deadline: Instant) -> Option<Snapshot> {
        let script = script(shell);
        for flags in attempt_flag_sets(shell) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let output = run_and_capture(shell, &flags, &script, timeout.min(remaining));
            if let Some(snapshot) = parse_snapshot(&output) {
                return Some(snapshot);
            }
        }
        None
    }

    #[cfg(test)]
    fn snapshot_path(shell: &Path, timeout: Duration) -> Option<OsString> {
        snapshot_shell(shell, timeout, Instant::now() + timeout * 4)?.path
    }

    /// Spawn the shell and collect stdout until the end marker appears, the
    /// child exits (plus a flush grace), or the timeout kills it. The reader
    /// lives on its own thread appending into a shared buffer, so a shell that
    /// blocks after printing — or a grandchild that inherits the pipe and
    /// never closes it — can't hang us on EOF.
    fn run_and_capture(shell: &Path, flags: &[&str], script: &str, timeout: Duration) -> Vec<u8> {
        let mut cmd = std::process::Command::new(shell);
        cmd.args(flags)
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Let rc files detect (and skip work for) this probe, mirroring
            // VSCODE_RESOLVING_ENVIRONMENT; TERM=dumb quiets fancy prompts.
            .env("ZERON_RESOLVING_ENVIRONMENT", "1")
            .env("TERM", "dumb");
        // SAFETY: setsid is async-signal-safe. A new session has no
        // controlling terminal, so an interactive shell's job control cannot
        // take the terminal's foreground from the zeron that launched it.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let Ok(mut child) = cmd.spawn() else {
            return Vec::new();
        };
        // The shell leads its own session and process group.
        let kill = |child: &mut std::process::Child| {
            // SAFETY: the group is the probe's own fresh session.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.kill();
            let _ = child.wait();
        };
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        if let Some(mut stdout) = child.stdout.take() {
            let buf = Arc::clone(&buf);
            let _ = std::thread::Builder::new()
                .name("zeron-shell-env-read".into())
                .spawn(move || {
                    let mut chunk = [0u8; 8192];
                    loop {
                        match stdout.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                let mut b = buf
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if b.len() >= MAX_OUTPUT {
                                    break;
                                }
                                b.extend_from_slice(&chunk[..n]);
                            }
                        }
                    }
                });
        }
        let deadline = Instant::now() + timeout;
        let mut exited_at: Option<Instant> = None;
        let mut scanned = 0usize;
        loop {
            {
                let b = buf
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Only scan the unscanned tail (minus marker-length overlap).
                let from = scanned.saturating_sub(END_MARKER.len());
                if find_subslice(&b[from..], END_MARKER.as_bytes()).is_some() {
                    break;
                }
                scanned = b.len();
            }
            match exited_at {
                Some(at) if at.elapsed() >= EXIT_FLUSH_GRACE => break,
                Some(_) => {}
                None => match child.try_wait() {
                    Ok(Some(_)) => exited_at = Some(Instant::now()),
                    Ok(None) if Instant::now() >= deadline => {
                        kill(&mut child);
                        break;
                    }
                    Ok(None) => {}
                    Err(_) => break,
                },
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if exited_at.is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let b = buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        b.clone()
    }

    /// launchd's own PATH (`launchctl setenv` / a LaunchAgent): what a Dock
    /// launch would have inherited had the user configured one.
    #[cfg(target_os = "macos")]
    fn launchctl_path() -> Option<OsString> {
        let mut child = std::process::Command::new("/bin/launchctl")
            .args(["getenv", "PATH"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().ok()?.is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut out = Vec::new();
        child.stdout.take()?.read_to_end(&mut out).ok()?;
        let trimmed = out.trim_ascii();
        (!trimmed.is_empty()).then(|| OsString::from_vec(trimmed.to_vec()))
    }

    /// Extract PATH and [`CAPTURED_VARS`] from the `env` dump between the
    /// LAST begin marker and the first end marker after it (rc noise printed
    /// before our command — or a marker echoed by init itself — lands before
    /// the real one). No PATH means the attempt failed.
    fn parse_snapshot(output: &[u8]) -> Option<Snapshot> {
        let begin = rfind_subslice(output, BEGIN_MARKER.as_bytes())?;
        let after = &output[begin + BEGIN_MARKER.len()..];
        let end = find_subslice(after, END_MARKER.as_bytes())?;
        let mut snapshot = Snapshot::default();
        for line in after[..end].split(|b| *b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(eq) = line.iter().position(|b| *b == b'=') else {
                continue;
            };
            let (key, value) = (&line[..eq], &line[eq + 1..]);
            if value.is_empty() {
                continue;
            }
            if key == b"PATH" {
                snapshot
                    .path
                    .get_or_insert_with(|| OsString::from_vec(value.to_vec()));
            } else if CAPTURED_VARS.iter().any(|name| name.as_bytes() == key)
                && !snapshot.vars.iter().any(|(k, _)| k.as_bytes() == key)
            {
                snapshot.vars.push((
                    OsString::from_vec(key.to_vec()),
                    OsString::from_vec(value.to_vec()),
                ));
            }
        }
        snapshot.path.is_some().then_some(snapshot)
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

        #[test]
        fn parses_path_between_markers() {
            let output = format!(
                "rc noise\n{BEGIN_MARKER}\nHOME=/home/u\nPATH=/custom/bin:/usr/bin\nX=y\n{END_MARKER}\ntrailing"
            );
            let path = parse_snapshot(output.as_bytes()).unwrap().path.unwrap();
            assert_eq!(path, OsString::from("/custom/bin:/usr/bin"));
        }

        #[test]
        fn captures_tls_ssh_and_locale_variables_but_nothing_else() {
            let output = format!(
                "{BEGIN_MARKER}\nPATH=/bin\nSSL_CERT_FILE=/etc/corp.pem\nNODE_EXTRA_CA_CERTS=/etc/corp.pem\n\
                 SSH_AUTH_SOCK=/tmp/agent.sock\nLANG=de_DE.UTF-8\nSECRET_TOKEN=nope\nSSL_CERT_DIR=\n{END_MARKER}\n"
            );
            let snapshot = parse_snapshot(output.as_bytes()).unwrap();
            let keys: Vec<_> = snapshot
                .vars
                .iter()
                .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
                .collect();
            assert_eq!(
                keys,
                [
                    "SSL_CERT_FILE=/etc/corp.pem",
                    "NODE_EXTRA_CA_CERTS=/etc/corp.pem",
                    "SSH_AUTH_SOCK=/tmp/agent.sock",
                    "LANG=de_DE.UTF-8",
                ]
            );
            // Without a PATH the attempt failed, whatever else it printed.
            let output = format!("{BEGIN_MARKER}\nLANG=C\n{END_MARKER}\n");
            assert!(parse_snapshot(output.as_bytes()).is_none());
        }

        #[test]
        fn ignores_marker_echoed_by_init() {
            // rc noise that happens to contain the begin marker but no PATH
            // after it must not shadow the real snapshot.
            let output =
                format!("{BEGIN_MARKER}\ngarbage\n{BEGIN_MARKER}\nPATH=/real/bin\n{END_MARKER}\n");
            let path = parse_snapshot(output.as_bytes()).unwrap().path.unwrap();
            assert_eq!(path, OsString::from("/real/bin"));
        }

        #[test]
        fn nushell_gets_a_script_it_prints() {
            assert!(script(Path::new("/opt/homebrew/bin/nu")).starts_with("print '"));
            assert!(script(Path::new("/opt/homebrew/bin/nu")).contains("^env"));
            assert!(script(Path::new("/bin/zsh")).starts_with("echo "));
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
        fn probe_runs_in_its_own_session() {
            let dir = tempfile::tempdir().unwrap();
            // Report the shell's session id in place of a PATH entry.
            let shell = fake_shell(
                dir.path(),
                &format!(
                    "#!/bin/sh\nsid=$(python3 -c 'import os; print(os.getsid(0))')\n\
                     echo {BEGIN_MARKER}; echo \"PATH=/sid/$sid\"; echo {END_MARKER}\n"
                ),
            );
            let path = snapshot_path(&shell, Duration::from_secs(10)).unwrap();
            let sid: i32 = path
                .to_string_lossy()
                .strip_prefix("/sid/")
                .and_then(|rest| rest.split(':').next())
                .and_then(|sid| sid.parse().ok())
                .expect("session id reported");
            // SAFETY: getsid(0) only reads our own session id.
            assert_ne!(sid, unsafe { libc::getsid(0) });
        }

        #[test]
        fn falls_back_when_interactive_attempt_hangs() {
            let dir = tempfile::tempdir().unwrap();
            // Simulates rc files that wedge only in interactive mode (`exec
            // tmux` and friends): sleep forever when -i is present.
            let shell = fake_shell(
                dir.path(),
                &format!(
                    "#!/bin/sh\ncase \" $* \" in *\" -i \"*) sleep 60;; esac\nPATH=\"/zeron-test/fallback/bin:/usr/bin:/bin\"; export PATH\n{RUN_PAYLOAD}"
                ),
            );
            let start = Instant::now();
            // Generous enough for the fallback attempt's exec of a freshly
            // written script: under a full parallel test run that alone has
            // taken ~2s on macOS.
            let path = snapshot_path(&shell, Duration::from_secs(3)).unwrap();
            assert!(
                path.to_string_lossy()
                    .starts_with("/zeron-test/fallback/bin"),
                "got: {}",
                path.to_string_lossy()
            );
            // First attempt burned ~3s then was killed; the whole resolve must
            // not have waited out the sleep.
            assert!(start.elapsed() < Duration::from_secs(15));
        }

        #[test]
        fn gives_up_on_a_shell_that_never_answers() {
            let dir = tempfile::tempdir().unwrap();
            let shell = fake_shell(dir.path(), "#!/bin/sh\nsleep 60\n");
            let start = Instant::now();
            assert!(snapshot_path(&shell, Duration::from_millis(300)).is_none());
            assert!(start.elapsed() < Duration::from_secs(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(values: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        values
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    #[test]
    fn captured_variables_fill_only_what_our_environment_lacks() {
        let captured = pairs(&[
            ("SSL_CERT_FILE", "/etc/corp.pem"),
            ("SSH_AUTH_SOCK", "/tmp/shell-agent"),
        ]);
        let ours = |key: &OsStr| key == "SSH_AUTH_SOCK" || key == "LANG";
        assert_eq!(
            defaults_for(&captured, ours, true),
            pairs(&[("SSL_CERT_FILE", "/etc/corp.pem")])
        );
    }

    #[test]
    fn macos_gets_a_utf8_ctype_only_without_any_locale() {
        assert_eq!(
            defaults_for(&[], |_| false, true),
            pairs(&[("LC_CTYPE", "en_US.UTF-8")])
        );
        // The shell's own locale wins over the fallback.
        assert_eq!(
            defaults_for(&pairs(&[("LANG", "fr_FR.UTF-8")]), |_| false, true),
            pairs(&[("LANG", "fr_FR.UTF-8")])
        );
        assert!(defaults_for(&[], |key| key == "LC_ALL", true).is_empty());
        assert!(defaults_for(&[], |_| false, false).is_empty());
    }
}
