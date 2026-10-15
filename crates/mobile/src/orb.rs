//! The call's state, in the terms the phone's voice surfaces read: the
//! glyph ([`crate::glyph`]), the controls and the Live Activity.
use zeron_orb::OrbState;

/// What the voice glyph is showing, in call terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VoiceOrb {
    /// No call: the calm form of a muted orchestrator.
    Idle,
    Connecting,
    Listening,
    Speaking,
    Working,
    AwaitingInput,
    Muted,
    /// A dictation's capture has ended and is being turned into words.
    Transcribing,
}

impl VoiceOrb {
    pub(crate) fn from_state(state: OrbState) -> Self {
        match state {
            OrbState::Connecting => VoiceOrb::Connecting,
            OrbState::Listening => VoiceOrb::Listening,
            OrbState::Composing => VoiceOrb::Speaking,
            OrbState::Working => VoiceOrb::Working,
            OrbState::Solving => VoiceOrb::AwaitingInput,
            _ => VoiceOrb::Muted,
        }
    }
}
