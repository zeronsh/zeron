//! Terminal sessions for zgx: a shell (or any command) on a pseudoterminal,
//! openpty on Unix and ConPTY on Windows, and the screen it draws, kept by
//! [`alacritty_terminal`]'s model (parser, grid, scrollback, modes).
//!
//! A [`Session`] runs its I/O on a thread of its own that sleeps until the
//! shell writes or the app does: it parses output into the screen and wakes
//! the app once (`wake`, then nothing until the app reads with
//! [`Session::take_events`]), so a flood of output costs the app one update
//! per read, and an idle shell costs nothing.
//!
//! ```no_run
//! use zgx_terminal::{GridSize, Session, SessionOptions};
//! let session = Session::spawn(&SessionOptions::default(), GridSize::new(80, 24), || {
//!     // Another thread: wake the UI to read.
//! })?;
//! session.write(b"echo hi\r".as_slice());
//! # Ok::<(), std::io::Error>(())
//! ```
#![warn(missing_docs)]

pub mod front;
mod keys;
mod news;
mod osc;
mod paint;
mod palette;
mod process;
mod session;
mod view;

pub use alacritty_terminal;
pub use front::Foreground;
pub use palette::{Palette, dim};
pub use process::process_name;
pub use session::{
    ClipboardRequest, GridSize, Listener, Session, SessionEvent, SessionOptions, Term,
};
pub use view::{EngineTerminal, is_engine_drawn, reads, terminal_view};

/// A program's progress report (OSC 9;4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// The report is over.
    Remove,
    /// This far, 0 to 100.
    Set(u8),
    /// Failed, this far if it says.
    Error(Option<u8>),
    /// Under way, how far unknown.
    Indeterminate,
    /// Paused, this far if it says.
    Paused(Option<u8>),
}
impl Progress {
    /// The report `state;progress` gives (the OSC's fields after `9;4;`).
    fn parse(fields: &str) -> Self {
        let mut fields = fields.split(';');
        let state = fields.next().unwrap_or("");
        let value = fields
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .map(|value| value.min(100) as u8);
        match state {
            "1" => Self::Set(value.unwrap_or(0)),
            "2" => Self::Error(value),
            "3" => Self::Indeterminate,
            "4" => Self::Paused(value),
            _ => Self::Remove,
        }
    }
    /// How far, 0 to 1, or negative while unknown; `None` once removed (as
    /// `zgx::terminal::TerminalState::progress` holds it).
    pub fn fraction(self) -> Option<f32> {
        match self {
            Self::Remove => None,
            Self::Set(value) | Self::Error(Some(value)) | Self::Paused(Some(value)) => {
                Some(f32::from(value) / 100.)
            }
            Self::Error(None) | Self::Paused(None) | Self::Indeterminate => Some(-1.),
        }
    }
}
