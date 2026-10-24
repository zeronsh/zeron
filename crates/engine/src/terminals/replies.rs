//! Answers to terminal queries, given by the host beside the PTY.
//!
//! Programs ask the terminal about itself at startup (device attributes,
//! version, cursor position, keyboard modes) and wait briefly for the answer.
//! A viewer answering from its own emulator is fine locally, but for a
//! terminal on another device the answer crosses the network twice and lands
//! after the program stopped waiting — where it reads as typed keys (an
//! agent CLI sees stray Escapes and quits). So the host runs a grid-only
//! emulator over the same output and answers at once; viewers that know this
//! stay silent (`TerminalSession::host_replies`). The host's emulator keeps no
//! scrollback: it exists to know the modes, the size and the cursor.

use std::sync::{Arc, Mutex, PoisonError};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Collects the emulator's answers (its `PtyWrite` events).
#[derive(Clone, Default)]
struct Answers(Arc<Mutex<Vec<u8>>>);

impl EventListener for Answers {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(text.as_bytes());
        }
    }
}

pub(super) struct HostReplies {
    term: Term<Answers>,
    parser: Processor,
    answers: Answers,
}

impl HostReplies {
    pub fn new(cols: u16, rows: u16) -> Self {
        let answers = Answers::default();
        let config = Config {
            scrolling_history: 0,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &size(cols, rows), answers.clone()),
            parser: Processor::new(),
            answers,
        }
    }

    /// Fold PTY output in; returns the answers it calls for, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, bytes);
        // Nothing is drawn here, so a synchronized update has no reason to
        // hold queries back: apply it now.
        if self.parser.sync_timeout().sync_timeout().is_some() {
            self.parser.stop_sync(&mut self.term);
        }
        std::mem::take(
            &mut *self
                .answers
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(size(cols, rows));
    }
}

fn size(cols: u16, rows: u16) -> Size {
    Size {
        cols: usize::from(cols.max(2)),
        rows: usize::from(rows.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_queries_are_answered_at_once() {
        let mut host = HostReplies::new(80, 24);
        // Primary device attributes.
        assert_eq!(host.feed(b"\x1b[c"), b"\x1b[?6c");
        // Cursor position, 1-based, tracking what was drawn.
        assert_eq!(host.feed(b"hello\x1b[6n"), b"\x1b[1;6R");
        // Plain output asks nothing.
        assert!(host.feed(b"just text\r\n").is_empty());
    }

    /// End to end in a real PTY: a program asking for the cursor position
    /// gets its answer from the host — and none while a viewer that answers
    /// queries itself is attached (two answers would read as typed keys).
    #[cfg(unix)]
    #[tokio::test]
    async fn the_host_answers_in_the_pty_unless_a_self_answering_viewer_is_attached() {
        use base64::Engine as _;
        use std::time::Duration;
        if !std::path::Path::new("/bin/bash").exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let terminals = crate::Terminals::new();
        // The echoed command line never contains `GOT[`; only its output does.
        let ask = "stty -echo; printf '\\033[6n'; IFS= read -rsd R -t 2 pos; \
                   printf '%s[%s]\\n' GOT \"${pos#*[}\"; stty echo\n";
        let got =
            |events: &mut tokio::sync::mpsc::UnboundedReceiver<zeron_proto::TerminalEvent>| {
                let mut text = String::new();
                while let Ok(event) = events.try_recv() {
                    if let zeron_proto::TerminalEvent::Data { data, .. } = event {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap();
                        text.push_str(&String::from_utf8_lossy(&bytes));
                    }
                }
                text
            };
        for (answers_itself, expect_answer) in [(false, true), (true, false)] {
            let session = terminals
                .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/bash"))
                .unwrap();
            let mut events = terminals
                .subscribe_viewer(&session.id, None, answers_itself)
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            terminals.write_bytes(&session.id, ask.as_bytes()).unwrap();
            let mut seen = String::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            while !seen.contains("GOT[") || !seen[seen.find("GOT[").unwrap()..].contains(']') {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "no reply line: {seen:?}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
                seen.push_str(&got(&mut events));
            }
            let reply = &seen[seen.find("GOT[").unwrap()..];
            let reply = &reply[..=reply.find(']').unwrap()];
            if expect_answer {
                assert!(reply.contains(';'), "host should answer: {reply:?}");
            } else {
                assert_eq!(reply, "GOT[]", "host must stay silent: {reply:?}");
            }
            terminals.close(&session.id).unwrap();
        }
    }

    #[test]
    fn answers_follow_the_pty_size_and_ignore_synchronized_updates() {
        let mut host = HostReplies::new(80, 24);
        host.resize(100, 30);
        assert_eq!(host.feed(b"\x1b[999;999H\x1b[6n"), b"\x1b[30;100R");
        // A query inside an open synchronized update is still answered.
        assert_eq!(host.feed(b"\x1b[?2026h\x1b[1;1H\x1b[6n"), b"\x1b[1;1R");
    }
}
