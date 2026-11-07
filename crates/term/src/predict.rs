//! Local echo prediction for terminals on other devices.
//!
//! A keystroke to a remote shell can't show until its echo crosses the
//! network twice. The predictor paints typed characters at the cursor right
//! away and lets the host's echo confirm them, so typing feels local while the
//! screen stays the host's truth:
//!
//! - **Trust is earned per run of typing.** A run starts after Enter, a
//!   control key or a misprediction, and only once the host has drawn
//!   something since. Its first character is held back until the host echoes
//!   it — the right character in the predicted cell, with the cursor moved
//!   past it (a blank cell "matching" a space proves nothing) — and only then
//!   are the run's characters painted ahead of their echo. A password prompt
//!   never echoes, so nothing typed into one is ever painted.
//! - **Trust is lost the moment the host stops echoing.** Output that
//!   confirms none of the run's pending characters (a `Passphrase:` prompt
//!   appearing mid-run), or a cursor that left the run's row, ends it.
//! - **The host always wins.** Each output batch checks every prediction
//!   against the cell it claimed: a match retires it, a different character
//!   past its echo deadline wipes every prediction and starts a new run.
//! - **Only where it's safe.** The panel predicts on the primary screen with
//!   the view at the live bottom and a visible cursor; full-screen programs
//!   draw what they like, so they get no predictions.
//! - **Only when it helps.** Predictions show once the measured echo time is
//!   long enough to notice (with hysteresis), never on a fast link.

use std::time::{Duration, Instant};

use crate::emulator::{CursorSnapshot, Emulator};

/// The live grid a prediction checks its echo against; both emulators have
/// one ([`crate::emulator`] for the mobile apps, `ghostty` for the desktop).
pub trait LiveScreen {
    /// The cursor in live viewport coordinates.
    fn live_cursor(&self) -> CursorSnapshot;
    /// The character in a live viewport cell.
    fn live_char(&self, row: usize, col: usize) -> Option<char>;
}

impl LiveScreen for Emulator {
    fn live_cursor(&self) -> CursorSnapshot {
        Emulator::live_cursor(self)
    }
    fn live_char(&self, row: usize, col: usize) -> Option<char> {
        Emulator::live_char(self, row, col)
    }
}

/// Echo time above which predictions start showing…
const SHOW_ABOVE: Duration = Duration::from_millis(30);
/// …and below which they stop.
const HIDE_BELOW: Duration = Duration::from_millis(15);
/// An echo this much later than expected means the prediction was wrong.
const ECHO_SLACK: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Prediction {
    row: usize,
    col: usize,
    ch: char,
    run: u64,
    at: Instant,
}

#[derive(Debug, Default)]
pub struct Predictor {
    pending: Vec<Prediction>,
    /// Current run of typing.
    run: u64,
    /// The host confirmed a prediction of the current run.
    trusted: bool,
    /// Smoothed keystroke → output time.
    srtt: Option<Duration>,
    /// Earliest write still waiting for any output.
    awaiting_since: Option<Instant>,
    showing: bool,
    /// A run was interrupted and the host hasn't drawn anything since: the
    /// cursor we'd predict from may still be on the old line.
    awaiting_redraw: bool,
}

impl Predictor {
    /// Typed text is about to be written. `cursor` is the host's cursor as
    /// last drawn; predictions continue from the last pending one.
    pub fn typed(&mut self, text: &str, cursor: CursorSnapshot, cols: usize, now: Instant) {
        if self.awaiting_redraw {
            self.interrupt();
            return;
        }
        for ch in text.chars() {
            if !(ch.is_ascii_graphic() || ch == ' ') {
                self.interrupt();
                return;
            }
            let (row, col) = match self.pending.iter().rev().find(|p| p.run == self.run) {
                Some(last) => (last.row, last.col + 1),
                None => (cursor.row, cursor.col),
            };
            // The shell decides how a line wraps; stop at the edge.
            if col + 1 >= cols {
                return;
            }
            self.pending.push(Prediction {
                row,
                col,
                ch,
                run: self.run,
                at: now,
            });
        }
    }

    /// Backspace: retract this run's newest unconfirmed character. With
    /// nothing of ours to take back, what it erases is the shell's business.
    pub fn backspace(&mut self) {
        let run = self.run;
        if let Some(ix) = self.pending.iter().rposition(|p| p.run == run) {
            self.pending.remove(ix);
        } else {
            self.interrupt();
        }
    }

    /// Any other key (Enter, control keys, arrows): the line or the program
    /// may change under us, so the next run must earn trust again.
    pub fn interrupt(&mut self) {
        self.run += 1;
        self.trusted = false;
        self.awaiting_redraw = true;
    }

    /// Forget everything (resize, screen switch, tab detach).
    pub fn reset(&mut self) {
        self.pending.clear();
        self.interrupt();
    }

    /// A write left for the host.
    pub fn sent(&mut self, now: Instant) {
        self.awaiting_since.get_or_insert(now);
    }

    /// Host output landed in `emulator`: measure, then confirm or refute.
    pub fn output(&mut self, emulator: &impl LiveScreen, now: Instant) {
        if let Some(sent) = self.awaiting_since.take() {
            let sample = now.saturating_duration_since(sent);
            let srtt = match self.srtt {
                Some(srtt) => (srtt * 7 + sample) / 8,
                None => sample,
            };
            self.srtt = Some(srtt);
            if srtt > SHOW_ABOVE {
                self.showing = true;
            } else if srtt < HIDE_BELOW {
                self.showing = false;
            }
        }
        self.awaiting_redraw = false;
        let deadline = self.echo_deadline();
        let cursor = emulator.live_cursor();
        let run = self.run;
        let had_current = self.pending.iter().any(|p| p.run == run);
        if self
            .pending
            .iter()
            .find(|p| p.run == run)
            .is_some_and(|first| cursor.row != first.row)
        {
            // The host moved to another line (a prompt, a redraw): whatever
            // we predicted for this one no longer applies.
            self.reset();
            return;
        }
        let mut wrong = false;
        let mut confirmed_current = false;
        let mut trusted = self.trusted;
        self.pending.retain(|p| {
            let echoed = emulator.live_char(p.row, p.col) == Some(p.ch)
                && (cursor.row > p.row || (cursor.row == p.row && cursor.col > p.col));
            if echoed {
                if p.run == run {
                    confirmed_current = true;
                    // A space over a blank cell would match without any echo.
                    if p.ch != ' ' {
                        trusted = true;
                    }
                }
                return false;
            }
            if now.saturating_duration_since(p.at) > deadline {
                wrong = true;
            }
            true
        });
        // Output that echoes none of what we're waiting on: the host may have
        // stopped echoing (a passphrase prompt mid-run). Stop painting ahead
        // until it confirms again.
        self.trusted = trusted && (confirmed_current || !had_current);
        if wrong {
            self.reset();
        }
    }

    /// When the oldest pending prediction is due, so a silent host still
    /// gets its stale predictions withdrawn.
    pub fn next_deadline(&self) -> Option<Instant> {
        let deadline = self.echo_deadline();
        self.pending.iter().map(|p| p.at + deadline).min()
    }

    /// Withdraw predictions past their deadline without new output.
    pub fn expire(&mut self, now: Instant) -> bool {
        let deadline = self.echo_deadline();
        if self
            .pending
            .iter()
            .any(|p| now.saturating_duration_since(p.at) > deadline)
        {
            self.reset();
            return true;
        }
        false
    }

    fn echo_deadline(&self) -> Duration {
        self.srtt.unwrap_or(SHOW_ABOVE) * 2 + ECHO_SLACK
    }

    /// Cells to paint over the host's screen, and where the cursor goes.
    pub fn overlay(&self) -> (Vec<(usize, usize, char)>, Option<CursorSnapshot>) {
        if !self.showing || !self.trusted {
            return (Vec::new(), None);
        }
        let cells: Vec<_> = self
            .pending
            .iter()
            .filter(|p| p.run == self.run)
            .map(|p| (p.row, p.col, p.ch))
            .collect();
        let cursor = cells
            .last()
            .map(|&(row, col, _)| CursorSnapshot { row, col: col + 1 });
        (cells, cursor)
    }

    /// Smoothed keystroke → output time, once measured.
    pub fn srtt(&self) -> Option<Duration> {
        self.srtt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTT: Duration = Duration::from_millis(80);

    /// A host 80 ms away whose shell echoes what it is sent.
    struct Remote {
        emulator: Emulator,
        predictor: Predictor,
        now: Instant,
    }

    impl Remote {
        fn new() -> Self {
            let mut emulator = Emulator::new(20, 4);
            emulator.feed(b"$ ");
            Self {
                emulator,
                predictor: Predictor::default(),
                now: Instant::now(),
            }
        }
        fn type_text(&mut self, text: &str) {
            let cursor = self.emulator.live_cursor();
            self.predictor.typed(text, cursor, 20, self.now);
            self.predictor.sent(self.now);
        }
        fn host_writes(&mut self, bytes: &[u8]) {
            self.now += RTT;
            self.emulator.feed(bytes);
            self.predictor.output(&self.emulator, self.now);
        }
        fn shown(&self) -> String {
            self.predictor.overlay().0.iter().map(|c| c.2).collect()
        }
    }

    #[test]
    fn a_run_shows_predictions_only_after_its_first_echo() {
        let mut r = Remote::new();
        r.type_text("l");
        assert_eq!(
            r.shown(),
            "",
            "nothing painted before the host proves it echoes"
        );
        r.host_writes(b"l");
        r.type_text("s");
        assert_eq!(r.shown(), "s", "trusted run paints ahead of the echo");
        assert_eq!(
            r.predictor.overlay().1,
            Some(CursorSnapshot { row: 0, col: 4 }),
            "cursor moves past the prediction"
        );
        r.host_writes(b"s");
        assert_eq!(r.shown(), "", "the echo retires it");
    }

    #[test]
    fn a_password_prompt_never_paints_what_is_typed() {
        let mut r = Remote::new();
        r.type_text("x");
        r.host_writes(b"x");
        r.type_text("\r");
        r.predictor.interrupt(); // Enter
        r.host_writes(b"\r\nPassword: ");
        for ch in ["h", "u", "n", "t", "e", "r", "2"] {
            r.type_text(ch);
            assert_eq!(r.shown(), "", "a silent prompt never earns trust");
        }
        r.now += Duration::from_secs(1);
        assert!(
            r.predictor.expire(r.now),
            "unechoed predictions are withdrawn"
        );
        assert_eq!(r.predictor.next_deadline(), None);
    }

    #[test]
    fn a_wrong_echo_wipes_every_prediction() {
        let mut r = Remote::new();
        r.type_text("a");
        r.host_writes(b"a");
        r.type_text("bc");
        assert_eq!(r.shown(), "bc");
        // The host draws something else where "b" was predicted, late.
        r.now += Duration::from_secs(1);
        r.host_writes(b"Z");
        assert_eq!(r.shown(), "");
        r.type_text("d");
        assert_eq!(r.shown(), "", "trust must be earned again");
    }

    #[test]
    fn backspace_takes_back_our_own_prediction_only() {
        let mut r = Remote::new();
        r.type_text("a");
        r.host_writes(b"a");
        r.type_text("bc");
        r.predictor.backspace();
        assert_eq!(r.shown(), "b");
        r.host_writes(b"b");
        r.predictor.backspace(); // erases host text: not ours to predict
        r.type_text("x");
        assert_eq!(r.shown(), "", "a new run after an unpredicted erase");
    }

    #[test]
    fn a_fast_link_never_paints_predictions() {
        let mut r = Remote::new();
        for ch in ["a", "b", "c", "d"] {
            r.type_text(ch);
            r.now += Duration::from_millis(2);
            r.emulator.feed(ch.as_bytes());
            r.predictor.output(&r.emulator, r.now);
        }
        r.type_text("e");
        assert_eq!(r.shown(), "", "2 ms echoes need no prediction");
    }

    #[test]
    fn a_prompt_appearing_mid_run_stops_the_painting() {
        let mut r = Remote::new();
        r.type_text("g");
        r.host_writes(b"g");
        r.type_text("i");
        assert_eq!(r.shown(), "i");
        // Output that echoes nothing we typed (a passphrase prompt redraws
        // the line): painting stops before the next key.
        r.host_writes(b"\r\x1b[KPassphrase: ");
        r.type_text("s");
        assert_eq!(r.shown(), "", "untrusted after a non-echo");
    }

    #[test]
    fn after_enter_nothing_is_predicted_until_the_host_redraws() {
        let mut r = Remote::new();
        r.type_text("a");
        r.host_writes(b"a");
        r.predictor.interrupt(); // Enter, prompt not drawn yet
        r.type_text(" ");
        r.type_text("x");
        assert!(
            r.predictor.next_deadline().is_none(),
            "no stale-cursor guesses"
        );
    }

    #[test]
    fn a_space_over_a_blank_cell_is_not_an_echo() {
        let mut r = Remote::new();
        // The cell after the prompt is blank: a predicted space "matches" it
        // with no echo, and the cursor never moved past it.
        r.type_text(" ");
        r.now += RTT;
        let emulator = std::mem::replace(&mut r.emulator, Emulator::new(1, 1));
        r.predictor.output(&emulator, r.now);
        r.emulator = emulator;
        r.type_text("p");
        assert_eq!(r.shown(), "", "no trust from a blank cell");
    }

    #[test]
    fn predictions_stop_at_the_line_edge() {
        let mut r = Remote::new();
        r.type_text("a");
        r.host_writes(b"a");
        r.type_text(&"x".repeat(40));
        assert_eq!(r.shown().len(), 20 - 3 - 1, "no guesses about wrapping");
    }
}
