//! What a call sounds like, for its glyph: each of its two voices as how loud
//! a few bands of it are, lowest first.
//!
//! The user's voice is measured from the microphone itself
//! ([`zeron_voice::spectrum`]), beside the call's own capture: levels only,
//! and only while the call is live and the microphone is not muted. Codex's
//! voice never leaves the call's audio process, which reports only how loud
//! it is, so its bands are the rhythms of that loudness instead
//! ([`Rhythm`]): the phrase, the syllable, the attack.
use std::time::Instant;

use zeron_orb::glyph::{Rhythm, Voice};
use zeron_voice::spectrum::Listener;

const _: () = assert!(zeron_voice::spectrum::BANDS == zeron_orb::glyph::BANDS);

/// The two voices of a call.
#[derive(Default)]
pub(crate) struct Hearing {
    ears: Option<Listener>,
    user: Rhythm,
    agent: Rhythm,
    /// When Codex's voice was last heard playing: while it plays, what the
    /// microphone hears is mostly Codex through the speakers, so the user's
    /// voice is not read from it.
    playing: Option<Instant>,
}

/// Codex's voice counts as playing this long after its last sound, and a
/// speaker peak over this much counts as sound.
const PLAYING_HOLD: std::time::Duration = std::time::Duration::from_millis(250);
const PLAYING_PEAK: f32 = 0.02;

impl Hearing {
    /// Follows a call while it is `open`, listening to the microphone if
    /// `listen` (measuring nothing while it is `muted`), and forgets it
    /// otherwise.
    pub(crate) fn follow(&mut self, open: bool, listen: bool, muted: bool) {
        if !open {
            *self = Self::default();
            return;
        }
        if !listen {
            self.ears = None;
            return;
        }
        // Never from a test, and not when asked not to: without it the
        // user's voice is followed by its loudness, as Codex's is.
        if self.ears.is_none()
            && !cfg!(test)
            && std::env::var("ZERON_VOICE_SPECTRUM").as_deref() != Ok("0")
        {
            self.ears = Listener::start().ok();
        }
        if let Some(ears) = &self.ears {
            ears.pause(muted);
        }
    }

    /// The call's own meters: the peak of each voice, 0–1.
    pub(crate) fn levels(&mut self, microphone: f32, speaker: f32, now: Instant) {
        self.user.hear(microphone, now);
        self.agent.hear(speaker, now);
        if speaker > PLAYING_PEAK {
            self.playing = Some(now);
        }
    }

    /// Whether Codex's voice is playing now, or was a moment ago.
    fn playing(&self, now: Instant) -> bool {
        self.playing
            .is_some_and(|at| now.saturating_duration_since(at) < PLAYING_HOLD)
    }

    pub(crate) fn voice(&self) -> Voice {
        // Codex's voice comes back through the microphone as a voice of its
        // own: while Codex plays, the user is taken to be listening, and
        // only Codex shows. The call's audio has its own echo cancelling;
        // the glyph's microphone does not.
        let user = if self.playing(Instant::now()) {
            Default::default()
        } else {
            self.ears
                .as_ref()
                .and_then(Listener::levels)
                .unwrap_or(self.user.bands())
        };
        Voice {
            user,
            agent: self.agent.bands(),
        }
    }
}
