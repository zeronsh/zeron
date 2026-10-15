//! The dictation track's phases and its clock. The track itself is the voice
//! glyph's beam ([`crate::voice::glyph`]), drawn by the composer.

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    /// Microphone is opening.
    Waiting,
    /// Live input: the beam follows the voice.
    Live,
    /// Capture ended: a highlight travels across while it is transcribed.
    Processing,
}

impl Mode {
    /// What the glyph shows in this phase.
    pub(crate) fn glyph(self) -> crate::voice::glyph::Mode {
        use crate::voice::glyph::Mode as Glyph;
        match self {
            Self::Waiting => Glyph::Connecting,
            Self::Live => Glyph::Listening,
            Self::Processing => Glyph::Processing,
        }
    }
}

/// Display time for the recording clock: `0:07`.
pub(crate) fn clock(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_minutes_and_padded_seconds() {
        assert_eq!(clock(std::time::Duration::from_millis(7_900)), "0:07");
        assert_eq!(clock(std::time::Duration::from_secs(60)), "1:00");
    }
}
