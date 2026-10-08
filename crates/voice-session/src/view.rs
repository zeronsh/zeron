//! What a client-media call shows: phase, live caption, levels and the orb's
//! state, reduced from the owner stream. Renderer-agnostic so every platform
//! presents the orchestrator the same way.
use std::time::{Duration, Instant};
use zeron_orb::OrbState;
use zeron_proto::voice::*;

/// The orb for a voice phase and host snapshot. Speaking wins over work and
/// mute, so a muted user still sees the assistant answer.
pub fn orb_state(phase: VoicePhase, snapshot: Option<&VoiceSnapshot>) -> OrbState {
    if matches!(
        phase,
        VoicePhase::Checking | VoicePhase::Starting | VoicePhase::Stopping
    ) {
        return OrbState::Connecting;
    }
    match snapshot {
        Some(s) if s.playing => OrbState::Composing,
        Some(s) if s.work == VoiceWork::AwaitingInput => OrbState::Solving,
        Some(s) if s.work == VoiceWork::Working => OrbState::Working,
        Some(s) if s.muted => OrbState::Breathing,
        Some(_) => OrbState::Listening,
        None => OrbState::Breathing,
    }
}

/// Visual speaker activity from playout peaks: hysteresis and a short hold
/// bridge syllable gaps. Client media has no host-side meter, so the client
/// derives `playing` itself.
#[derive(Default)]
pub struct SpeakerActivity {
    playing: bool,
    last_loud: Option<Instant>,
}
impl SpeakerActivity {
    pub fn update(&mut self, peak: u16, now: Instant) -> bool {
        const ENTER: u16 = 655;
        const EXIT: u16 = 328;
        const HOLD: Duration = Duration::from_millis(250);
        if peak >= if self.playing { EXIT } else { ENTER } {
            self.playing = true;
            self.last_loud = Some(now);
        } else if self
            .last_loud
            .is_none_or(|last| now.saturating_duration_since(last) >= HOLD)
        {
            self.playing = false;
        }
        self.playing
    }
}

/// Speaker turns the caption remembers; older ones can no longer complete.
const CAPTION_TURNS: usize = 8;

/// The live caption across speaker turns. Codex interleaves them: a user
/// turn's final often lands after the assistant has started answering. Each
/// turn keeps its own text and the caption stays on the newest one, so a late
/// final never takes the caption back and deltas never restart it.
#[derive(Default)]
pub struct Caption {
    /// Oldest first; the last turn is the one shown.
    turns: Vec<CaptionTurn>,
}

struct CaptionTurn {
    item: Option<String>,
    text: String,
    /// Known once the turn's final arrives.
    role: Option<VoiceRole>,
}

impl Caption {
    pub fn text(&self) -> &str {
        self.turns.last().map_or("", |t| t.text.as_str())
    }

    /// The shown turn's item: a change is a new speaker turn.
    pub fn item(&self) -> Option<&str> {
        self.turns.last().and_then(|t| t.item.as_deref())
    }

    /// The shown turn's speaker, once its final arrived.
    pub fn role(&self) -> Option<VoiceRole> {
        self.turns.last().and_then(|t| t.role)
    }

    pub fn clear(&mut self) {
        self.turns.clear();
    }

    fn position(&self, item: &str) -> Option<usize> {
        self.turns
            .iter()
            .rposition(|t| t.item.as_deref() == Some(item))
    }

    fn push(&mut self, turn: CaptionTurn) {
        self.turns.push(turn);
        let excess = self.turns.len().saturating_sub(CAPTION_TURNS);
        self.turns.drain(..excess);
    }

    /// Append a streamed delta to its turn; a new item starts a new turn. A
    /// delta without an item continues the shown turn while it streams.
    /// Returns false when the turn outgrew [`MAX_TRANSCRIPT_BYTES`]: it then
    /// restarts from this delta, showing the tail rather than growing.
    pub fn partial(&mut self, item: Option<String>, delta: &str) -> bool {
        let index = match item {
            Some(ref item) => self.position(item),
            None => self
                .turns
                .len()
                .checked_sub(1)
                .filter(|&last| self.turns[last].role.is_none()),
        };
        let index = index.unwrap_or_else(|| {
            self.push(CaptionTurn {
                item,
                text: String::new(),
                role: None,
            });
            self.turns.len() - 1
        });
        let turn = &mut self.turns[index];
        if turn.role.is_some() {
            // The final already settled this turn.
            return true;
        }
        let fits = turn.text.len() + delta.len() <= MAX_TRANSCRIPT_BYTES;
        if !fits {
            turn.text.clear();
        }
        turn.text.push_str(delta);
        fits
    }

    /// A turn's final text and speaker. An unseen turn that completes while
    /// another one streams is older than it, so it never takes the caption.
    pub fn complete(&mut self, transcript: &VoiceTranscript) {
        let found = self.position(&transcript.item_id).or_else(|| {
            // Deltas without an item belong to the turn this final completes.
            self.turns
                .last()
                .filter(|t| t.item.is_none() && t.role.is_none())
                .map(|_| self.turns.len() - 1)
        });
        if let Some(index) = found {
            let turn = &mut self.turns[index];
            turn.item = Some(transcript.item_id.clone());
            turn.text.clone_from(&transcript.text);
            turn.role = Some(transcript.role);
            return;
        }
        let turn = CaptionTurn {
            item: Some(transcript.item_id.clone()),
            text: transcript.text.clone(),
            role: Some(transcript.role),
        };
        match self.turns.last() {
            Some(last) if last.role.is_none() => {
                let streaming = self.turns.len() - 1;
                self.turns.insert(streaming, turn);
                let excess = self.turns.len().saturating_sub(CAPTION_TURNS);
                self.turns.drain(..excess);
            }
            _ => self.push(turn),
        }
    }
}

/// Reduced state of one remote call. Events from another generation or
/// session are ignored; the first snapshot names the orchestrator chat.
#[derive(Default)]
pub struct VoiceView {
    pub phase: VoicePhase,
    pub snapshot: Option<VoiceSnapshot>,
    pub microphone: u16,
    pub speaker: u16,
    caption: Caption,
    muted: bool,
    speaker_activity: SpeakerActivity,
}

impl VoiceView {
    pub fn new() -> Self {
        Self {
            phase: VoicePhase::Checking,
            ..Self::default()
        }
    }

    pub fn chat_id(&self) -> Option<&str> {
        self.snapshot.as_ref().map(|s| s.chat_id.as_str())
    }

    /// The newest speaker turn: live deltas, then its final text.
    pub fn caption(&self) -> &str {
        self.caption.text()
    }

    /// Set once the caption is a final segment; live deltas have none.
    pub fn caption_role(&self) -> Option<VoiceRole> {
        self.caption.role()
    }

    /// The utterance (speaker turn) the caption belongs to, if named.
    pub fn caption_item(&self) -> Option<&str> {
        self.caption.item()
    }

    pub fn work(&self) -> VoiceWork {
        self.snapshot.as_ref().map_or(VoiceWork::Idle, |s| s.work)
    }

    pub fn playing(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.playing)
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    /// The local mute is authoritative for presentation: it applies before
    /// the host acknowledges it.
    pub fn set_muted(&mut self, muted: bool) -> bool {
        let changed = self.muted != muted;
        self.muted = muted;
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.muted = muted;
        }
        changed
    }

    pub fn orb_state(&self) -> OrbState {
        orb_state(self.phase, self.snapshot.as_ref())
    }

    /// Normalized 0…1 microphone loudness; zero while muted.
    pub fn microphone_level(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            f32::from(self.microphone) / f32::from(u16::MAX)
        }
    }

    pub fn speaker_level(&self) -> f32 {
        f32::from(self.speaker) / f32::from(u16::MAX)
    }

    fn current(&self, generation: u64) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s.generation == generation)
    }

    /// Whether the event changed what the call shows.
    pub fn reduce(&mut self, event: VoiceEvent, now: Instant) -> bool {
        match event {
            VoiceEvent::Snapshot { mut snapshot } => {
                if let Some(old) = &self.snapshot {
                    if old.generation != snapshot.generation
                        || old.session_id != snapshot.session_id
                    {
                        return false;
                    }
                    snapshot.playing = old.playing;
                }
                snapshot.muted = self.muted;
                self.phase = snapshot.phase;
                self.snapshot = Some(snapshot);
            }
            VoiceEvent::Partial {
                generation,
                item_id,
                text,
            } if self.current(generation) => {
                // A runaway utterance shows its tail rather than grow.
                self.caption.partial(item_id, &text);
            }
            VoiceEvent::Final { transcript }
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.session_id == transcript.session_id) =>
            {
                self.caption.complete(&transcript);
            }
            VoiceEvent::Levels {
                generation,
                microphone,
                speaker,
            } if self.current(generation) => {
                self.microphone = microphone;
                self.speaker = speaker;
                let playing = self.speaker_activity.update(speaker, now);
                if let Some(snapshot) = &mut self.snapshot {
                    snapshot.playing = playing;
                }
            }
            VoiceEvent::Closed { generation, reason } if self.current(generation) => {
                self.phase = if reason.is_some() {
                    VoicePhase::Failed
                } else {
                    VoicePhase::Closed
                };
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(generation: u64) -> VoiceSnapshot {
        VoiceSnapshot {
            session_id: "session".into(),
            chat_id: "voice-orchestrator-full-id".into(),
            generation,
            phase: VoicePhase::Active,
            muted: false,
            playing: false,
            work: VoiceWork::Idle,
            reason: None,
            voice: None,
            voices: vec![],
        }
    }

    #[test]
    fn speaker_activity_ignores_noise_and_holds_through_syllable_gaps() {
        let now = Instant::now();
        let mut activity = SpeakerActivity::default();
        assert!(!activity.update(500, now));
        assert!(activity.update(1024, now));
        assert!(activity.update(0, now + Duration::from_millis(249)));
        assert!(!activity.update(0, now + Duration::from_millis(250)));
        assert!(!activity.update(500, now + Duration::from_millis(300)));
    }

    #[test]
    fn remote_view_derives_speaking_and_keeps_it_across_snapshots() {
        let now = Instant::now();
        let mut view = VoiceView::new();
        assert_eq!(view.orb_state(), OrbState::Connecting);
        assert!(view.reduce(
            VoiceEvent::Snapshot {
                snapshot: snapshot(2)
            },
            now
        ));
        assert_eq!(view.chat_id(), Some("voice-orchestrator-full-id"));
        assert_eq!(view.orb_state(), OrbState::Listening);
        view.reduce(
            VoiceEvent::Levels {
                generation: 2,
                microphone: 0,
                speaker: 4000,
            },
            now,
        );
        assert_eq!(view.orb_state(), OrbState::Composing);
        let mut working = snapshot(2);
        working.work = VoiceWork::AwaitingInput;
        view.reduce(VoiceEvent::Snapshot { snapshot: working }, now);
        // The host never meters client media; its `playing: false` is stale.
        assert!(view.playing());
        view.reduce(
            VoiceEvent::Levels {
                generation: 2,
                microphone: 0,
                speaker: 0,
            },
            now + Duration::from_secs(1),
        );
        assert_eq!(view.orb_state(), OrbState::Solving);
        assert!(view.set_muted(true));
        assert_eq!(view.microphone_level(), 0.0);
    }

    #[test]
    fn stale_events_and_runaway_partials_stay_bounded() {
        let now = Instant::now();
        let mut view = VoiceView::new();
        assert!(!view.reduce(
            VoiceEvent::Partial {
                generation: 1,
                item_id: None,
                text: "early".into()
            },
            now
        ));
        view.reduce(
            VoiceEvent::Snapshot {
                snapshot: snapshot(2),
            },
            now,
        );
        assert!(!view.reduce(
            VoiceEvent::Snapshot {
                snapshot: snapshot(3)
            },
            now
        ));
        assert!(!view.reduce(
            VoiceEvent::Closed {
                generation: 1,
                reason: None
            },
            now
        ));
        for _ in 0..5 {
            view.reduce(
                VoiceEvent::Partial {
                    generation: 2,
                    item_id: Some("item".into()),
                    text: "x".repeat(MAX_TRANSCRIPT_BYTES / 2),
                },
                now,
            );
        }
        assert!(view.caption().len() <= MAX_TRANSCRIPT_BYTES);
        view.reduce(
            VoiceEvent::Final {
                transcript: VoiceTranscript {
                    session_id: "session".into(),
                    item_id: "item".into(),
                    role: VoiceRole::Assistant,
                    text: "done".into(),
                    promoted_message_id: None,
                },
            },
            now,
        );
        assert_eq!(view.caption(), "done");
        assert_eq!(view.caption_role(), Some(VoiceRole::Assistant));
        view.reduce(
            VoiceEvent::Partial {
                generation: 2,
                item_id: Some("next-item".into()),
                text: "next".into(),
            },
            now,
        );
        assert_eq!(view.caption(), "next");
        assert_eq!(view.caption_role(), None);
        assert!(view.reduce(
            VoiceEvent::Closed {
                generation: 2,
                reason: Some(VoiceRejection::Protocol)
            },
            now
        ));
        assert_eq!(view.phase, VoicePhase::Failed);
    }

    fn final_of(item: &str, role: VoiceRole, text: &str) -> VoiceTranscript {
        VoiceTranscript {
            session_id: "session".into(),
            item_id: item.into(),
            role,
            text: text.into(),
            promoted_message_id: None,
        }
    }

    /// The order Codex really sends: the user's final lands after the
    /// assistant has started answering.
    #[test]
    fn a_late_user_final_never_takes_the_caption_from_the_assistant() {
        let mut caption = Caption::default();
        caption.partial(Some("user".into()), "Open the ");
        caption.partial(Some("user".into()), "sync test");
        caption.partial(Some("assistant".into()), "Sure, ");
        caption.partial(Some("assistant".into()), "opening it");
        assert_eq!(caption.text(), "Sure, opening it");
        caption.complete(&final_of("user", VoiceRole::User, "Open the sync test."));
        assert_eq!(caption.text(), "Sure, opening it");
        assert_eq!(caption.item(), Some("assistant"));
        assert_eq!(caption.role(), None);
        caption.partial(Some("assistant".into()), " now.");
        assert_eq!(caption.text(), "Sure, opening it now.");
        caption.complete(&final_of(
            "assistant",
            VoiceRole::Assistant,
            "Sure, opening it now.",
        ));
        assert_eq!(caption.text(), "Sure, opening it now.");
        assert_eq!(caption.role(), Some(VoiceRole::Assistant));
    }

    /// A user turn transcribed only as a final, after the answer started.
    #[test]
    fn an_unseen_final_is_older_than_the_streaming_turn() {
        let mut caption = Caption::default();
        caption.partial(Some("assistant".into()), "On it");
        caption.complete(&final_of("user", VoiceRole::User, "Open the test"));
        assert_eq!(caption.text(), "On it");
        assert_eq!(caption.item(), Some("assistant"));
        // With nothing streaming, a final is the newest turn.
        caption.complete(&final_of("assistant", VoiceRole::Assistant, "On it."));
        caption.complete(&final_of("user-2", VoiceRole::User, "Thanks"));
        assert_eq!(caption.text(), "Thanks");
        assert_eq!(caption.role(), Some(VoiceRole::User));
    }

    #[test]
    fn deltas_without_an_item_continue_the_streaming_turn() {
        let mut caption = Caption::default();
        caption.partial(None, "Hello ");
        caption.partial(None, "there");
        assert_eq!(caption.text(), "Hello there");
        caption.complete(&final_of("a", VoiceRole::Assistant, "Hello there."));
        assert_eq!(caption.item(), Some("a"));
        // A settled turn is not continued: the next delta starts a new one.
        caption.partial(None, "Next");
        assert_eq!(caption.text(), "Next");
        assert_eq!(caption.role(), None);
    }

    #[test]
    fn a_settled_turn_ignores_late_deltas_and_old_turns_are_forgotten() {
        let mut caption = Caption::default();
        caption.partial(Some("a".into()), "Done");
        caption.complete(&final_of("a", VoiceRole::Assistant, "Done."));
        caption.partial(Some("a".into()), " again");
        assert_eq!(caption.text(), "Done.");
        for i in 0..CAPTION_TURNS * 2 {
            caption.partial(Some(format!("turn-{i}")), "x");
        }
        assert_eq!(caption.turns.len(), CAPTION_TURNS);
        assert_eq!(
            caption.item(),
            Some(format!("turn-{}", CAPTION_TURNS * 2 - 1).as_str())
        );
    }
}
