//! Capability gating for the Claude Code CLI's undocumented launch flags.
//!
//! `--thinking-display` is real but absent from `claude --help`, and an older
//! CLI that predates it rejects it in commander's argument parser before the
//! run produces a single frame: `error: unknown option '--thinking-display'`.
//! The user sees that as `claude exited unexpectedly (still running)` naming a
//! flag they never typed, and the resolved CLI — which is usually an older
//! install shadowing a newer one — is nowhere in the message (#477).
//!
//! Neither `--help` (does not list it) nor `--version` (short-circuits
//! argument validation in commander) can tell the two CLIs apart, so the only
//! reliable probe is a real invocation: the launch flags we pass
//! unconditionally plus the flag under test, with stdin at EOF. A CLI that
//! knows the flag parses it and then fails — or exits — for some unrelated
//! reason; one that does not exits with `unknown option '<flag>'`.
//!
//! `--permission-prompt-tool` is deliberately left out of the probe: it is a
//! second undocumented flag with the same failure mode, and letting it decide
//! this one would gate `--thinking-display` on an unrelated capability. Gate
//! it the same way once its own version floor is known.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};

use crate::process::{Command, Stdio};

/// The flag and its value, as `ClaudeHarness::build_command` passes them.
const THINKING_DISPLAY: [&str; 2] = ["--thinking-display", "summarized"];

/// The unconditional half of the launch flags: everything a run passes before
/// the flags under test. Parsing these is what puts commander in a state where
/// it can reject an unknown option, so the probe and a run agree on the parse.
const BASE_ARGS: [&str; 8] = [
    "--print",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--replay-user-messages",
];

/// A full CLI start is bounded like every other probe in this crate (see
/// [`crate::executable::binary_version`]): a wedged install must cost one
/// timeout, not a hung run.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Far more than an argument-parser complaint — commander writes it as its
/// first line, so the cap keeps a chatty install from growing what the probe
/// reads without ever truncating the answer.
const PROBE_OUTPUT_CAP: usize = 64 * 1024;

fn cache() -> &'static Mutex<HashMap<PathBuf, bool>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

async fn locked() -> MutexGuard<'static, HashMap<PathBuf, bool>> {
    cache().lock().await
}

fn key_for(exe: &Path) -> PathBuf {
    exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf())
}

/// Does the resolved CLI accept `--thinking-display summarized`?
///
/// Probed once per executable and cached: the answer cannot change without a
/// CLI upgrade. A probe that cannot answer — spawn failure, timeout — assumes
/// support, so the worst case is today's behaviour rather than a silently
/// dropped thinking summary. One probe per executable at most, but the lock
/// spans it, so a wedged install can hold up the first runs behind it for
/// [`PROBE_TIMEOUT`].
pub(super) async fn supports_thinking_display(exe: &Path) -> bool {
    let key = key_for(exe);
    if let Some(supported) = locked().await.get(&key) {
        return *supported;
    }
    // Hold the lock across the probe so concurrent first runs coalesce into
    // one child instead of one per caller.
    let mut entries = locked().await;
    if let Some(supported) = entries.get(&key) {
        return *supported;
    }
    let supported = probe(exe).await;
    entries.insert(key, supported);
    supported
}

/// Does the CLI's argument parser accept [`THINKING_DISPLAY`]?
async fn probe(exe: &Path) -> bool {
    match tokio::time::timeout(PROBE_TIMEOUT, spawn(exe)).await {
        Ok(Ok(output)) => !rejects_thinking_display(&output.stdout, &output.stderr),
        Ok(Err(error)) => {
            tracing::debug!(
                target: "zeron_harness::claude",
                %error,
                path = %exe.display(),
                "thinking-display probe could not start; assuming support"
            );
            true
        }
        Err(_) => {
            tracing::debug!(
                target: "zeron_harness::claude",
                path = %exe.display(),
                "thinking-display probe timed out; assuming support"
            );
            true
        }
    }
}

/// stdin is closed so the CLI cannot wait for a prompt that never comes. Both
/// streams are read: commander writes the complaint to stderr, and some builds
/// print usage on stdout.
async fn spawn(exe: &Path) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new(exe);
    crate::compose_child_path(&mut cmd, exe);
    cmd.args(BASE_ARGS).args(THINKING_DISPLAY);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd.output().await
}

/// `error: unknown option '--thinking-display'` — commander's wording. Any
/// other outcome (success, a usage error about something else, a crash) counts
/// as "the flag parsed", so a CLI that fails for unrelated reasons keeps the
/// flag.
fn rejects_thinking_display(stdout: &[u8], stderr: &[u8]) -> bool {
    [stdout, stderr].into_iter().any(|stream| {
        let text = String::from_utf8_lossy(&stream[..stream.len().min(PROBE_OUTPUT_CAP)]);
        text.lines().any(|line| {
            let line = line.trim().to_ascii_lowercase();
            line.contains("unknown option") && line.contains(THINKING_DISPLAY[0])
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejects(stdout: &[u8], stderr: &[u8]) -> bool {
        rejects_thinking_display(stdout, stderr)
    }

    #[test]
    fn a_commander_complaint_names_the_flag_as_the_rejection() {
        assert!(rejects(
            b"",
            b"error: unknown option '--thinking-display'\n"
        ));
        // Output format varies; the wording is what identifies the rejection.
        assert!(rejects(b"", b"error: unknown option '--thinking-display'"));
        assert!(rejects(
            b"",
            b"error: unknown option '--thinking-display'\r\n"
        ));
        assert!(rejects(b"", b"Error: Unknown option '--thinking-display'"));
    }

    #[test]
    fn any_other_exit_reads_as_support() {
        // A CLI that knows the flag parses it, then complains about the empty
        // prompt instead — still support.
        assert!(!rejects(
            b"",
            b"error: input stream ended before a message\n"
        ));
        // A different unknown option must not gate this one.
        assert!(!rejects(
            b"",
            b"error: unknown option '--permission-prompt-tool'\n"
        ));
        // A usage dump on stdout.
        assert!(!rejects(b"Usage: claude [options]\n", b""));
        assert!(!rejects(b"", b""));
    }

    #[test]
    fn a_complaint_inside_the_read_cap_is_found_whatever_precedes_it() {
        let complaint = "error: unknown option '--thinking-display'\n";
        // Chatty output ahead of the complaint must not hide it.
        let filler = "x".repeat(PROBE_OUTPUT_CAP - complaint.len());
        assert!(rejects(format!("{filler}{complaint}").as_bytes(), b""));
        // And the complaint alone on stderr is enough.
        assert!(rejects(b"", complaint.as_bytes()));
    }
}
