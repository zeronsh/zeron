//! Grok subscription voice. Mints a realtime credential from the local Grok Bot
//! session and keeps microphone and speaker audio on this machine.

mod audio;
mod auth;
mod protocol;
mod session;

pub use audio::{DesktopDuplex, Duplex};
pub use auth::{Credentials, GrokBotCredentials, Minted, signed_in};
pub use protocol::{
    SPEED_MAX, SPEED_MIN, SPEED_STEP, Speech, VOICES, snap_speed, speed_fraction,
    speed_from_fraction, speed_label, voice_label,
};
pub use session::{Call, ToolExec, ToolSpec, TranscriptSink, Tune, run};

#[cfg(test)]
mod tests;
