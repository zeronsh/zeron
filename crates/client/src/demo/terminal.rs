//! The demo host's terminals: a scripted zsh that echoes what's typed, keeps
//! a line editor (backspace, ^C, ^U, ^L, history on ↑/↓) and answers a few
//! commands with realistic, colored output — streamed line by line like a
//! real build. Output carries sequence numbers and a replay window, so the
//! client's resume-after-drop path runs exactly as against an engine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::error::{ClientError, Result};
use crate::live::b64;
use crate::lock;

const PROMPT_DIR: &str = "\x1b[1;38;5;141m~/zeron\x1b[0m";
const PROMPT_BRANCH: &str = " \x1b[2mon\x1b[0m \x1b[38;5;212m veil-fade\x1b[0m";

fn prompt() -> String {
    format!("\r\n{PROMPT_DIR}{PROMPT_BRANCH}\r\n\x1b[38;5;141m❯\x1b[0m ")
}

struct Shell {
    seq: u64,
    replay: Vec<Value>,
    viewers: Vec<mpsc::Sender<Value>>,
    line: String,
    history: Vec<String>,
    history_at: Option<usize>,
    /// Bytes of an escape sequence still arriving.
    pending_escape: Vec<u8>,
    exited: bool,
    /// While a write is processed, its echo gathers here and goes out as
    /// one chunk (a PTY's read returns what's there, not a byte at a time).
    batch: Option<String>,
}

impl Shell {
    fn emit(&mut self, event: Value) {
        self.replay.push(event.clone());
        // A bounded window, like the engine's.
        if self.replay.len() > 4000 {
            self.replay.drain(..1000);
        }
        self.viewers.retain(|tx| tx.try_send(event.clone()).is_ok());
    }

    fn output(&mut self, text: &str) {
        if let Some(batch) = self.batch.as_mut() {
            batch.push_str(text);
            return;
        }
        self.seq += 1;
        let event =
            json!({ "type": "data", "seq": self.seq, "data": b64::encode(text.as_bytes()) });
        self.emit(event);
    }

    /// Send what a write's processing echoed, as one chunk.
    fn flush(&mut self) {
        if let Some(text) = self.batch.take().filter(|t| !t.is_empty()) {
            self.output(&text);
        }
    }

    fn exit(&mut self, code: i32) {
        self.flush();
        self.seq += 1;
        self.exited = true;
        let event = json!({ "type": "exit", "seq": self.seq, "exitCode": code });
        self.emit(event);
    }

    /// Replace the line being edited (history recall).
    fn set_line(&mut self, line: String) {
        let erase = "\x08 \x08".repeat(self.line.chars().count());
        self.output(&format!("{erase}{line}"));
        self.line = line;
    }
}

#[derive(Default)]
pub(crate) struct DemoTerminals {
    shells: Mutex<HashMap<String, Arc<Mutex<Shell>>>>,
}

impl DemoTerminals {
    pub(crate) fn open(&self, cwd: &str) -> Value {
        let id = crate::new_id();
        let mut shell = Shell {
            seq: 0,
            replay: Vec::new(),
            viewers: Vec::new(),
            line: String::new(),
            history: vec!["git status".into(), "cargo test -p zeron-veil".into()],
            history_at: None,
            pending_escape: Vec::new(),
            exited: false,
            batch: None,
        };
        shell.output(&format!(
            "\x1b[2mLast login: {} on ttys004\x1b[0m{}",
            chrono::Local::now().format("%a %b %e %H:%M"),
            prompt()
        ));
        lock(&self.shells).insert(id.clone(), Arc::new(Mutex::new(shell)));
        json!({ "id": id, "cwd": cwd, "shell": "zsh", "hostReplies": true })
    }

    fn shell(&self, id: &str) -> Result<Arc<Mutex<Shell>>> {
        lock(&self.shells)
            .get(id)
            .cloned()
            .ok_or_else(|| ClientError::HostError("SubscribeTerminal: Terminal not found".into()))
    }

    pub(crate) fn subscribe(&self, id: &str, after_seq: u64) -> Result<mpsc::Receiver<Value>> {
        let shell = self.shell(id)?;
        let mut shell = lock(&shell);
        let (tx, rx) = mpsc::channel(4096);
        for event in shell
            .replay
            .iter()
            .filter(|e| e["seq"].as_u64().unwrap_or(0) > after_seq)
        {
            let _ = tx.try_send(event.clone());
        }
        if !shell.exited {
            shell.viewers.push(tx);
        }
        Ok(rx)
    }

    pub(crate) fn close(&self, id: &str) {
        if let Some(shell) = lock(&self.shells).remove(id) {
            lock(&shell).exit(0);
        }
    }

    pub(crate) fn write(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let shell = self.shell(id)?;
        let mut run = None;
        {
            let mut s = lock(&shell);
            if s.exited {
                return Ok(());
            }
            let mut input = std::mem::take(&mut s.pending_escape);
            input.extend_from_slice(bytes);
            s.batch = Some(String::new());
            let mut i = 0;
            while i < input.len() {
                let b = input[i];
                match b {
                    0x1b => {
                        // Arrow keys (CSI or SS3); anything else is dropped.
                        if i + 2 >= input.len() {
                            s.pending_escape = input[i..].to_vec();
                            break;
                        }
                        let key = input[i + 2];
                        match key {
                            b'A' | b'B' => {
                                let len = s.history.len();
                                let at = match (key, s.history_at) {
                                    (b'A', None) if len > 0 => Some(len - 1),
                                    (b'A', Some(at)) => Some(at.saturating_sub(1)),
                                    (b'B', Some(at)) if at + 1 < len => Some(at + 1),
                                    _ => None,
                                };
                                s.history_at = at;
                                let line = at.map(|at| s.history[at].clone()).unwrap_or_default();
                                s.set_line(line);
                            }
                            _ => {}
                        }
                        i += 3;
                        continue;
                    }
                    b'\r' | b'\n' => {
                        let line = std::mem::take(&mut s.line);
                        s.history_at = None;
                        if !line.trim().is_empty() {
                            s.history.push(line.clone());
                        }
                        s.output("\r\n");
                        run = Some(line);
                        break;
                    }
                    0x7f | 0x08 => {
                        if s.line.pop().is_some() {
                            s.output("\x08 \x08");
                        }
                    }
                    0x03 => {
                        s.line.clear();
                        let p = prompt();
                        s.output(&format!("^C{p}"));
                    }
                    0x15 => {
                        let line = String::new();
                        s.set_line(line);
                    }
                    0x0c => {
                        let line = s.line.clone();
                        let p = prompt();
                        s.output(&format!(
                            "\x1b[H\x1b[2J{}{line}",
                            p.trim_start_matches("\r\n")
                        ));
                    }
                    0x04 if s.line.is_empty() => {
                        s.output("\r\n");
                        s.exit(0);
                        return Ok(());
                    }
                    b if b < 0x20 => {}
                    _ => {
                        // UTF-8: take the whole scalar.
                        let len = match b {
                            0xf0..=0xf7 => 4,
                            0xe0..=0xef => 3,
                            0xc0..=0xdf => 2,
                            _ => 1,
                        };
                        if i + len > input.len() {
                            s.pending_escape = input[i..].to_vec();
                            break;
                        }
                        let text = String::from_utf8_lossy(&input[i..i + len]).into_owned();
                        s.line.push_str(&text);
                        s.output(&text);
                        i += len;
                        continue;
                    }
                }
                i += 1;
            }
            s.flush();
        }
        if let Some(line) = run {
            let shell = shell.clone();
            crate::runtime::shared().spawn(async move { run_command(shell, line).await });
        }
        Ok(())
    }
}

async fn run_command(shell: Arc<Mutex<Shell>>, line: String) {
    let say = |text: &str| lock(&shell).output(text);
    let lines = |text: &str| text.replace('\n', "\r\n");
    let command = line.trim();
    let (program, rest) = command.split_once(' ').unwrap_or((command, ""));
    match program {
        "" => {}
        "clear" => say("\x1b[H\x1b[2J"),
        "pwd" => say("/Users/dev/zeron\r\n"),
        "whoami" => say("dev\r\n"),
        "echo" => say(&format!("{rest}\r\n")),
        "exit" | "logout" => {
            lock(&shell).exit(0);
            return;
        }
        "ls" => say(&lines(
            "\x1b[1;34mapps\x1b[0m     \x1b[1;34mcrates\x1b[0m   \x1b[1;34mdocs\x1b[0m     \x1b[1;34medge\x1b[0m     \x1b[1;34mscripts\x1b[0m\nCargo.lock  Cargo.toml  README.md  rust-toolchain.toml\n",
        )),
        "git" if rest.starts_with("status") => say(&lines(
            "On branch \x1b[1mveil-fade\x1b[0m\nYour branch is ahead of 'origin/veil-fade' by 2 commits.\n\nChanges not staged for commit:\n\t\x1b[31mmodified:   apps/ios/Zeron/Transcript/Veil.swift\x1b[0m\n\t\x1b[31mmodified:   crates/ui/src/markdown/veil.rs\x1b[0m\n\nUntracked files:\n\t\x1b[31mcrates/ui/src/markdown/veil/graphemes.rs\x1b[0m\n",
        )),
        "git" if rest.starts_with("log") => say(&lines(
            "\x1b[33m4f1c9a2\x1b[0m \x1b[2m(\x1b[0m\x1b[1;36mHEAD -> \x1b[1;32mveil-fade\x1b[0m\x1b[2m)\x1b[0m Snap veil chunk splits to grapheme clusters\n\x1b[33m7d03b6e\x1b[0m Fade duration tracks the append cadence\n\x1b[33m2d6895c\x1b[0m \x1b[2m(\x1b[0m\x1b[1;31morigin/main\x1b[0m\x1b[2m)\x1b[0m Keep the session through fold and unfold\n\x1b[33ma2c68b0\x1b[0m Find cargo where the caller's PATH does\n\x1b[33m6f2721f\x1b[0m Stream pull request status on every client\n",
        )),
        "cargo" if rest.starts_with("test") || rest.starts_with("build") => {
            let testing = rest.starts_with("test");
            let steps: &[(&str, u64)] = &[
                (
                    "\x1b[1;32m   Compiling\x1b[0m zeron-text v0.2.121 (/Users/dev/zeron/crates/text)",
                    260,
                ),
                (
                    "\x1b[1;32m   Compiling\x1b[0m zeron-markdown v0.2.121 (/Users/dev/zeron/crates/markdown)",
                    320,
                ),
                (
                    "\x1b[1;32m   Compiling\x1b[0m zeron-veil v0.2.121 (/Users/dev/zeron/crates/veil)",
                    480,
                ),
                (
                    "\x1b[1;32m    Finished\x1b[0m `test` profile [unoptimized + debuginfo] target(s) in 4.21s",
                    200,
                ),
            ];
            for (text, ms) in steps {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
                say(&format!("{text}\r\n"));
            }
            if testing {
                say(
                    "\x1b[1;32m     Running\x1b[0m unittests src/lib.rs (target/debug/deps/zeron_veil-8f3a)\r\n\r\nrunning 14 tests\r\n",
                );
                for name in [
                    "alpha_bounds",
                    "cadence_clamp_low",
                    "cadence_clamp_high",
                    "chunk_spans_keep_length",
                    "curve_is_monotonic",
                    "graphemes::combining_acute",
                    "graphemes::flags",
                    "graphemes::zwj_family",
                    "lifecycle_drop_on_complete",
                    "reattach_seeds_baseline",
                    "split_never_mid_cluster",
                    "veil_is_paint_only",
                    "warm_reopen_keeps_fade",
                    "zero_duration_is_opaque",
                ] {
                    tokio::time::sleep(Duration::from_millis(70)).await;
                    say(&format!("test {name} ... \x1b[32mok\x1b[0m\r\n"));
                }
                say(
                    "\r\ntest result: \x1b[32mok\x1b[0m. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s\r\n",
                );
            }
        }
        "top" | "htop" | "vim" | "nvim" => say(&format!(
            "\x1b[2m{program}: not available in the demo terminal\x1b[0m\r\n"
        )),
        _ => say(&format!("zsh: command not found: {program}\r\n")),
    }
    let p = prompt();
    say(&p);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rx: &mut mpsc::Receiver<Value>) -> String {
        let mut out = String::new();
        while let Ok(event) = rx.try_recv() {
            if let Some(data) = event["data"].as_str() {
                out.push_str(&String::from_utf8_lossy(&b64::decode(data).unwrap()));
            }
        }
        out
    }

    #[test]
    fn echoes_edits_and_replays_after_a_sequence() {
        let terminals = DemoTerminals::default();
        let id = terminals.open("/Users/dev/zeron")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut rx = terminals.subscribe(&id, 0).unwrap();
        assert!(text(&mut rx).contains("Last login"));
        terminals.write(&id, b"lx\x7fs").unwrap();
        assert_eq!(text(&mut rx), "lx\x08 \x08s");
        // A viewer resuming after seq 1 gets only what came later.
        let mut late = terminals.subscribe(&id, 1).unwrap();
        assert!(!text(&mut late).contains("Last login"));
        // History recall replaces the line.
        terminals.write(&id, b"\x1b[A").unwrap();
        assert!(text(&mut rx).ends_with("cargo test -p zeron-veil"));
    }

    #[test]
    fn closing_ends_the_stream_and_forgets_the_terminal() {
        let terminals = DemoTerminals::default();
        let id = terminals.open("~")["id"].as_str().unwrap().to_owned();
        let mut rx = terminals.subscribe(&id, 0).unwrap();
        terminals.close(&id);
        let mut exited = false;
        while let Ok(event) = rx.try_recv() {
            exited |= event["type"] == "exit";
        }
        assert!(exited);
        assert!(terminals.subscribe(&id, 0).is_err());
    }
}
