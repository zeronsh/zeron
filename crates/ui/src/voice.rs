//! Window-owned voice orchestrator. Each session runs in the host's own
//! projectless Codex chat, resumed across calls, and survives navigation
//! between threads; only an explicit end, an engine change or a provider
//! failure closes it. No automatic reconnect or microphone resume.
use crate::orb::OrbState;
use crate::state::EngineHandle;
use gpui::{Context, Task};
use gpui_tokio::Tokio;
use zeron_proto::voice::*;
mod call_menu;
mod grok;
mod permissions;
mod remote;

pub(crate) use call_menu::GrokCallMenu;

pub enum VoiceControl {
    Mute(bool),
    /// Voice, personality, speed, or microphone changed during a Grok call.
    Retune(GrokStyle),
}

/// Voice, speed, personality, and microphone for a Grok call. Codex ignores it.
#[derive(Clone, Debug, PartialEq)]
pub struct GrokStyle {
    pub voice: Option<String>,
    pub speed: f32,
    pub personality: crate::settings::GrokPersonality,
    pub input: Option<String>,
}

impl GrokStyle {
    pub fn from_settings(settings: &crate::settings::UiSettings) -> Self {
        Self {
            voice: settings.grok_voice.clone(),
            speed: zeron_grok_voice::snap_speed(settings.grok_voice_speed),
            personality: settings.grok_personality,
            input: settings.grok_input.clone(),
        }
    }
}

pub struct VoiceController {
    pub phase: VoicePhase,
    pub chat_id: Option<String>,
    pub reason: Option<VoiceRejection>,
    pub snapshot: Option<VoiceSnapshot>,
    /// The newest speaker turn's live text, then its final.
    pub caption: zeron_voice_session::Caption,
    pub microphone_level: u16,
    pub speaker_level: u16,
    /// The full-window stage is presented over the shell.
    pub stage_open: bool,
    pub host_name: Option<String>,
    speaker_last_loud: Option<std::time::Instant>,
    /// When the session first became active; drives the stage's call timer.
    pub active_since: Option<std::time::Instant>,
    engine: Option<EngineHandle>,
    controls: Option<tokio::sync::mpsc::Sender<VoiceControl>>,
    /// Last style handed to the running Grok call.
    sent_style: Option<GrokStyle>,
    epoch: u64,
    task: Option<Task<()>>,
    cancellation: tokio_util::sync::CancellationToken,
    /// Whether this device's Codex is installed and enabled. `None` until the
    /// catalog answers; unread still counts as available.
    codex_ready: Option<bool>,
    /// Local Grok sign-in. `None` until checked, and None is not available.
    grok_ready: Option<bool>,
    /// The connection `codex_ready` was read from, and its read in flight.
    codex_engine: Option<EngineHandle>,
    codex_check: Option<Task<()>>,
    /// This call played its start sound and owes its end sound.
    cued: bool,
    /// Ask-each-time chooser. A `Popup` so the trigger press that dismisses it
    /// does not also reopen it: the card's outside press closes on mouse-down,
    /// before the click.
    provider_menu: crate::popover::Popup<()>,
    /// Codex or Grok for the call that is up. Cleared with the session.
    provider: Option<VoiceProvider>,
}
impl Default for VoiceController {
    fn default() -> Self {
        Self {
            phase: VoicePhase::Closed,
            chat_id: None,
            reason: None,
            snapshot: None,
            caption: Default::default(),
            microphone_level: 0,
            speaker_level: 0,
            stage_open: false,
            host_name: None,
            speaker_last_loud: None,
            active_since: None,
            engine: None,
            controls: None,
            sent_style: None,
            epoch: 0,
            task: None,
            cancellation: tokio_util::sync::CancellationToken::new(),
            codex_ready: None,
            grok_ready: None,
            codex_engine: None,
            codex_check: None,
            cued: false,
            provider_menu: crate::popover::Popup::default(),
            provider: None,
        }
    }
}
impl VoiceController {
    /// Voice can be offered here. An unread Codex catalog counts as yes, so a
    /// slow or failed read never hides it. Grok counts only once sign-in is
    /// confirmed. A live call or its failure stays.
    pub fn offered(&self) -> bool {
        self.is_live()
            || self.reason.is_some()
            || self.codex_ready != Some(false)
            || self.grok_ready == Some(true)
    }

    /// What an idle microphone press does. `None` when neither provider can run.
    pub(crate) fn resolve_provider(
        &self,
        preference: crate::settings::VoiceProviderChoice,
    ) -> Option<ProviderDecision> {
        let codex_possible = self.codex_ready != Some(false);
        let grok_available = self.grok_ready == Some(true);
        if codex_possible && grok_available {
            return Some(match preference {
                crate::settings::VoiceProviderChoice::Ask => ProviderDecision::Menu,
                crate::settings::VoiceProviderChoice::Codex => {
                    ProviderDecision::Start(VoiceProvider::Codex)
                }
                crate::settings::VoiceProviderChoice::Grok => {
                    ProviderDecision::Start(VoiceProvider::Grok)
                }
            });
        }
        if codex_possible {
            return Some(ProviderDecision::Start(VoiceProvider::Codex));
        }
        grok_available.then_some(ProviderDecision::Start(VoiceProvider::Grok))
    }

    /// Idle tooltip when nothing has failed. Codex-only keeps its old label.
    pub(crate) fn idle_tooltip(
        &self,
        preference: crate::settings::VoiceProviderChoice,
    ) -> &'static str {
        match self.resolve_provider(preference) {
            Some(ProviderDecision::Menu) => "Choose Codex or Grok",
            Some(ProviderDecision::Start(VoiceProvider::Grok)) => "Start voice · Grok",
            Some(ProviderDecision::Start(VoiceProvider::Codex)) | None => {
                "Start voice · Codex orchestrator"
            }
        }
    }

    pub(crate) fn provider_menu_open(&self) -> bool {
        self.provider_menu.is_open()
    }

    /// Mounted, including the exit animation. Render uses this.
    pub(crate) fn provider_menu_mounted(&self) -> bool {
        self.provider_menu.get().is_some()
    }

    pub(crate) fn provider_menu_closing(&self) -> Option<std::time::Instant> {
        self.provider_menu.closing_since()
    }

    /// From the trigger's mouse-down, before the card's outside press closes it.
    pub(crate) fn note_provider_press(&mut self) {
        self.provider_menu.note_trigger_press();
    }

    /// `true` when this click's press found the menu mounted. Leave it closed.
    pub(crate) fn take_provider_press_was_open(&mut self) -> bool {
        self.provider_menu.take_press_was_open()
    }

    pub(crate) fn close_provider_menu(&mut self, cx: &mut Context<Self>) {
        if self.provider_menu.begin_close() {
            crate::popover::reap_popup(cx, |voice: &mut Self| &mut voice.provider_menu);
            cx.notify();
        }
    }

    pub(crate) fn toggle_provider_menu(&mut self, cx: &mut Context<Self>) {
        if self.provider_menu.is_open() {
            self.close_provider_menu(cx);
        } else if !self.provider_menu.is_closing() {
            self.provider_menu.open(());
            cx.notify();
        }
    }

    /// `(codex_ready, grok_ready)`. Grok's `None` is not availability.
    pub(crate) fn provider_readiness(&self) -> (Option<bool>, Option<bool>) {
        (self.codex_ready, self.grok_ready)
    }

    pub(crate) fn provider(&self) -> Option<VoiceProvider> {
        self.provider
    }

    /// Read whether `engine`'s device can take a call. Codex must be installed
    /// and enabled; Grok is a local sign-in file. Both answers land together.
    /// `force` re-reads a connection already read.
    pub fn check_codex(&mut self, engine: &EngineHandle, force: bool, cx: &mut Context<Self>) {
        let same = self
            .codex_engine
            .as_ref()
            .is_some_and(|e| e.same_connection(engine));
        if same && !force {
            return;
        }
        if !same {
            self.codex_ready = None;
            self.grok_ready = None;
            self.provider_menu = crate::popover::Popup::default();
        }
        self.codex_engine = Some(engine.clone());
        let engine = engine.clone();
        self.codex_check = Some(cx.spawn(async move |this, cx| {
            let ready = engine
                .client()
                .call(zeron_rpc::methods::LIST_HARNESSES, serde_json::json!({}))
                .await
                .ok()
                .and_then(|value| {
                    serde_json::from_value::<Vec<zeron_engine::registry::HarnessDescriptor>>(value)
                        .ok()
                })
                .map(|list| {
                    list.iter().any(|d| {
                        d.id == zeron_proto::HarnessId::Codex
                            && d.installed
                            && zeron_engine::registry::descriptor_enabled(d)
                    })
                });
            // GPUI polls this task, so `spawn_blocking` has no current runtime
            // unless it goes through the app handle. Both flags land below.
            let Ok(runtime) = this.update(cx, |_, cx| Tokio::handle(cx)) else {
                return;
            };
            let grok_signed_in = runtime
                .spawn_blocking(zeron_grok_voice::signed_in)
                .await
                .unwrap_or(false);
            this.update(cx, |voice, cx| {
                let mut changed = false;
                if ready.is_some() && voice.codex_ready != ready {
                    voice.codex_ready = ready;
                    changed = true;
                }
                if voice.grok_ready != Some(grok_signed_in) {
                    voice.grok_ready = Some(grok_signed_in);
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// A session is underway (including its preparation and native stop).
    pub fn is_live(&self) -> bool {
        matches!(self.phase, VoicePhase::Checking) || self.phase.replaces_composer()
    }

    /// Start `provider` on `host_device_id`. Codex keeps a projectless Codex
    /// chat; Grok uses the prepared chat on this computer.
    pub fn start(
        &mut self,
        engine: EngineHandle,
        host_device_id: String,
        config: zeron_proto::ChatConfig,
        voice: Option<String>,
        grok: GrokStyle,
        provider: VoiceProvider,
        cx: &mut Context<Self>,
    ) {
        self.cancel(cx);
        // The call sounds as it is placed, well before the microphone opens.
        crate::sound::play_voice(true);
        self.cued = true;
        self.engine = Some(engine.clone());
        self.phase = VoicePhase::Checking;
        self.chat_id = None;
        self.reason = None;
        self.provider = Some(provider);
        let epoch = self.epoch;
        let cancellation = self.cancellation.clone();
        let (controls, control_rx) = tokio::sync::mpsc::channel(8);
        self.controls = Some(controls);
        self.sent_style = Some(grok.clone());
        let (events, mut event_rx) = tokio::sync::mpsc::channel(32);
        let query = Tokio::spawn(cx, async move {
            // Media belongs to this viewport, even when the chosen host is
            // this device. The execution host never opens audio devices.
            remote::run(
                engine,
                host_device_id,
                config,
                voice,
                grok,
                provider,
                cancellation,
                events,
                control_rx,
            )
            .await
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let receive = async {
                while let Some(event) = event_rx.recv().await {
                    if this
                        .update(cx, |controller, cx| {
                            if controller.epoch == epoch {
                                controller.reduce(event, cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            };
            receive.await;
            let result = query.await.unwrap_or(Err(VoiceRejection::Protocol));
            let _ = this.update(cx, |controller, cx| {
                if controller.epoch != epoch {
                    return;
                }
                match result {
                    // A provider failure already reduced to Failed keeps its reason.
                    Ok(()) if controller.phase == VoicePhase::Failed => {}
                    Ok(()) => {
                        controller.phase = VoicePhase::Closed;
                    }
                    Err(reason) => {
                        controller.phase = VoicePhase::Failed;
                        controller.reason = Some(reason);
                    }
                }
                controller.reset_session();
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Ending now sounds: a hang-up or a connected call ending. A failure to
    /// connect only explains itself.
    fn end_sounds(&self) -> bool {
        self.cued && (self.active_since.is_some() || self.reason.is_none())
    }

    fn reset_session(&mut self) {
        if self.end_sounds() {
            crate::sound::play_voice(false);
        }
        self.cued = false;
        self.snapshot = None;
        self.caption.clear();
        self.microphone_level = 0;
        self.speaker_level = 0;
        self.controls = None;
        self.sent_style = None;
        self.task = None;
        self.stage_open = false;
        self.active_since = None;
    }

    /// End the session (any phase). The provider stop runs in the background.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.epoch = self.epoch.wrapping_add(1);
        self.cancellation.cancel();
        self.cancellation = tokio_util::sync::CancellationToken::new();
        self.reset_session();
        self.phase = VoicePhase::Closed;
        self.chat_id = None;
        self.reason = None;
        self.engine = None;
        self.provider = None;
        self.provider_menu = crate::popover::Popup::default();
        cx.notify();
    }

    pub fn set_stage_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let open = open && self.is_live();
        if self.stage_open != open {
            self.stage_open = open;
            cx.notify();
        }
    }

    /// Forget a failure once it has been seen.
    pub fn dismiss_reason(&mut self, cx: &mut Context<Self>) {
        if self.reason.take().is_some() {
            if self.phase == VoicePhase::Failed {
                self.phase = VoicePhase::Closed;
            }
            cx.notify();
        }
    }

    pub fn belongs_to(&self, engine: &EngineHandle) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.same_connection(engine))
    }
    pub fn muted(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.muted)
    }
    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = self.muted();
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.muted = !muted;
        }
        cx.notify();
        if let Some(controls) = &self.controls {
            if controls.try_send(VoiceControl::Mute(!muted)).is_err() {
                self.cancel(cx);
                self.reason = Some(VoiceRejection::Overflow);
            }
        }
    }

    /// Push the saved Grok voice, personality, speed, and microphone into the
    /// live call. Unrelated settings writes are ignored. A full control queue
    /// does not end the call.
    pub fn sync_grok_style(&mut self, cx: &mut Context<Self>) {
        if self.provider != Some(VoiceProvider::Grok) {
            return;
        }
        let Some(controls) = &self.controls else {
            return;
        };
        let style = GrokStyle::from_settings(&crate::settings::current(cx));
        if self.sent_style.as_ref() == Some(&style) {
            return;
        }
        if controls
            .try_send(VoiceControl::Retune(style.clone()))
            .is_ok()
        {
            self.sent_style = Some(style);
            cx.notify();
        }
    }

    /// The utterance the caption shows (a speaker turn), if the provider names it.
    pub fn caption_item(&self) -> Option<&str> {
        self.caption.item()
    }

    pub fn orb_state(&self) -> OrbState {
        orb_state(self.phase, self.snapshot.as_ref())
    }

    /// Normalized 0…1 microphone loudness; zero while muted.
    pub fn microphone_level(&self) -> f32 {
        if self.muted() {
            return 0.0;
        }
        f32::from(self.microphone_level) / f32::from(u16::MAX)
    }

    /// Normalized 0…1 speaker peak; the orb smooths each channel separately.
    pub fn speaker_level(&self) -> f32 {
        f32::from(self.speaker_level) / f32::from(u16::MAX)
    }

    pub fn reduce(&mut self, event: VoiceEvent, cx: &mut Context<Self>) {
        match event {
            VoiceEvent::Snapshot { mut snapshot } => {
                if let Some(previous) = &self.snapshot {
                    snapshot.playing = previous.playing;
                }
                if self.chat_id.is_none() {
                    self.chat_id = Some(snapshot.chat_id.clone());
                }

                if self.chat_id.as_deref() != Some(snapshot.chat_id.as_str()) {
                    return;
                }
                if self.snapshot.as_ref().is_some_and(|old| {
                    old.generation != snapshot.generation || old.session_id != snapshot.session_id
                }) {
                    return;
                }
                self.phase = snapshot.phase;
                if self.phase == VoicePhase::Active && self.active_since.is_none() {
                    self.active_since = Some(std::time::Instant::now());
                }
                self.snapshot = Some(snapshot);
            }
            VoiceEvent::Partial {
                generation,
                item_id,
                text,
            } if self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.generation == generation) =>
            {
                if !self.caption.partial(item_id, &text) {
                    self.cancel(cx);
                    self.phase = VoicePhase::Failed;
                    self.reason = Some(VoiceRejection::Overflow);
                }
            }
            VoiceEvent::Levels {
                generation,
                microphone,
                speaker,
            } if self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.generation == generation) =>
            {
                self.microphone_level = microphone;
                self.speaker_level = speaker;
                if let Some(snapshot) = &mut self.snapshot {
                    let threshold = if snapshot.playing { 328 } else { 655 };
                    if speaker >= threshold {
                        snapshot.playing = true;
                        self.speaker_last_loud = Some(std::time::Instant::now());
                    } else if self
                        .speaker_last_loud
                        .is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(250))
                    {
                        snapshot.playing = false;
                    }
                }
            }
            // The final segment keeps showing as the caption until the next
            // item starts; the canonical copy is already in the transcript.
            VoiceEvent::Final { transcript }
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.session_id == transcript.session_id) =>
            {
                self.caption.complete(&transcript);
            }
            VoiceEvent::Closed { generation, reason }
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.generation == generation) =>
            {
                self.phase = if reason.is_some() {
                    VoicePhase::Failed
                } else {
                    VoicePhase::Closed
                };
                self.reason = reason;
                self.reset_session();
            }
            _ => return,
        }
        cx.notify();
    }
    pub fn reason_text(&self) -> &'static str {
        match self.reason {
            Some(VoiceRejection::CreditExclusionUnverified) => {
                "Codex controls subscription usage and any enabled additional credits."
            }
            Some(VoiceRejection::WrongHarness) => "Voice requires Codex.",
            Some(VoiceRejection::RemoteHost) => "Voice runs on this device only.",
            Some(VoiceRejection::IncludedUsageUnavailable) => {
                "Codex usage is currently unavailable."
            }
            Some(VoiceRejection::AudioFormatUnverified) => {
                "This Codex voice format has not been verified."
            }
            Some(VoiceRejection::ChatgptRequired) => "Sign in to Codex with ChatGPT to use voice.",
            Some(VoiceRejection::GrokSignInRequired) => {
                "Sign in to Grok Bot on this computer to use Grok voice."
            }
            // Remote calls still play audio here, through this device's Codex helper.
            Some(VoiceRejection::NativeRuntimeUnavailable) => {
                "Install or update Codex on this device with its official installer to use voice. This copy of Codex has no voice runtime."
            }
            Some(VoiceRejection::DeviceUnavailable) => {
                "Check microphone permission and your audio devices."
            }
            Some(VoiceRejection::MicrophonePermissionDenied) => {
                "Allow Zeron to use the microphone in System Settings → Privacy & Security → Microphone."
            }
            Some(VoiceRejection::MicrophoneMetadataMissing) => {
                "Microphone setup is missing. Rebuild or reinstall Zeron, then restart it."
            }
            Some(VoiceRejection::Busy) => "Another window already owns the voice session.",
            Some(_) => "Voice could not connect. Try again in a moment.",
            None => "",
        }
    }
}

/// What an idle footer press should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderDecision {
    Menu,
    Start(VoiceProvider),
}

/// Call-timer text: `m:ss`, or `h:mm:ss` past the hour.
pub fn format_elapsed(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Shared with the mobile apps so every orb reads the call the same way.
pub use zeron_voice_session::orb_state;

impl Drop for VoiceController {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext;

    fn snapshot() -> VoiceSnapshot {
        VoiceSnapshot {
            session_id: "one".into(),
            chat_id: "chat".into(),
            generation: 2,
            phase: VoicePhase::Active,
            muted: false,
            playing: false,
            work: VoiceWork::Idle,
            reason: None,
            voice: None,
            voices: Vec::new(),
        }
    }

    #[test]
    fn voice_is_offered_unless_codex_is_known_missing() {
        let mut voice = VoiceController::default();
        assert!(voice.offered(), "an unread catalog never hides voice");
        voice.codex_ready = Some(true);
        assert!(voice.offered());
        voice.codex_ready = Some(false);
        assert!(!voice.offered());
        voice.grok_ready = Some(true);
        assert!(voice.offered(), "grok alone offers voice");
        voice.grok_ready = Some(false);
        assert!(!voice.offered());
        voice.grok_ready = None;
        assert!(!voice.offered(), "an unread grok check is not availability");
        // A failure explains itself before the trigger goes away.
        voice.reason = Some(VoiceRejection::NativeRuntimeUnavailable);
        assert!(voice.offered());
        voice.reason = None;
        voice.phase = VoicePhase::Checking;
        assert!(voice.offered(), "a call in progress keeps its trigger");
    }

    #[test]
    fn resolving_a_provider_asks_only_when_both_can_run() {
        let mut voice = VoiceController::default();
        let ask = crate::settings::VoiceProviderChoice::Ask;
        let codex = crate::settings::VoiceProviderChoice::Codex;
        let grok = crate::settings::VoiceProviderChoice::Grok;
        assert_eq!(
            voice.resolve_provider(ask),
            Some(ProviderDecision::Start(VoiceProvider::Codex)),
            "an unread codex catalog starts codex"
        );
        assert_eq!(voice.idle_tooltip(ask), "Start voice · Codex orchestrator");

        voice.codex_ready = Some(true);
        voice.grok_ready = Some(true);
        assert_eq!(voice.resolve_provider(ask), Some(ProviderDecision::Menu));
        assert_eq!(voice.idle_tooltip(ask), "Choose Codex or Grok");
        assert_eq!(
            voice.resolve_provider(codex),
            Some(ProviderDecision::Start(VoiceProvider::Codex))
        );
        assert_eq!(
            voice.resolve_provider(grok),
            Some(ProviderDecision::Start(VoiceProvider::Grok))
        );
        assert_eq!(voice.idle_tooltip(grok), "Start voice · Grok");
        assert_eq!(
            voice.idle_tooltip(codex),
            "Start voice · Codex orchestrator"
        );

        voice.codex_ready = Some(false);
        assert_eq!(
            voice.resolve_provider(codex),
            Some(ProviderDecision::Start(VoiceProvider::Grok)),
            "a codex preference falls back when codex is missing"
        );
        assert_eq!(
            voice.resolve_provider(ask),
            Some(ProviderDecision::Start(VoiceProvider::Grok)),
            "only grok starts grok"
        );

        voice.codex_ready = Some(true);
        voice.grok_ready = None;
        assert_eq!(
            voice.resolve_provider(grok),
            Some(ProviderDecision::Start(VoiceProvider::Codex)),
            "a grok preference falls back while grok is unchecked"
        );
        voice.grok_ready = Some(false);
        assert_eq!(
            voice.resolve_provider(grok),
            Some(ProviderDecision::Start(VoiceProvider::Codex))
        );
        assert_eq!(voice.idle_tooltip(ask), "Start voice · Codex orchestrator");

        voice.codex_ready = Some(false);
        voice.grok_ready = Some(false);
        assert_eq!(voice.resolve_provider(ask), None);
    }

    #[test]
    fn a_press_that_dismisses_the_provider_menu_does_not_reopen_it() {
        let mut voice = VoiceController::default();
        assert!(!voice.provider_menu_open());
        voice.provider_menu.open(());
        assert!(voice.provider_menu_open());
        assert!(voice.provider_menu_mounted());
        // Mouse-down on the trigger, then the card's outside press begins the
        // close, then the click must leave it closed.
        voice.note_provider_press();
        assert!(voice.provider_menu.begin_close());
        assert!(!voice.provider_menu_open());
        assert!(voice.provider_menu_mounted());
        assert!(voice.take_provider_press_was_open());
        assert!(!voice.provider_menu_open());
    }

    #[test]
    fn grok_sign_in_failure_names_grok() {
        let mut voice = VoiceController::default();
        voice.reason = Some(VoiceRejection::GrokSignInRequired);
        assert_eq!(
            voice.reason_text(),
            "Sign in to Grok Bot on this computer to use Grok voice."
        );
    }

    #[test]
    fn only_a_hang_up_or_a_connected_call_sounds_its_end() {
        let mut voice = VoiceController::default();
        assert!(!voice.end_sounds(), "no call was placed");
        voice.cued = true;
        assert!(voice.end_sounds(), "hanging up while connecting");
        voice.reason = Some(VoiceRejection::Busy);
        assert!(!voice.end_sounds(), "a failure to connect stays quiet");
        voice.active_since = Some(std::time::Instant::now());
        assert!(voice.end_sounds(), "a connected call that drops");
    }

    #[gpui::test]
    fn voice_reducer_ignores_stale_and_foreign_events(cx: &mut gpui::TestAppContext) {
        let voice = cx.new(|_| VoiceController::default());
        voice.update(cx, |voice, cx| {
            voice.chat_id = Some("chat".into());
            let snapshot = snapshot();
            voice.reduce(
                VoiceEvent::Snapshot {
                    snapshot: snapshot.clone(),
                },
                cx,
            );
            voice.reduce(
                VoiceEvent::Partial {
                    generation: 1,
                    item_id: None,
                    text: "old".into(),
                },
                cx,
            );
            assert!(voice.caption.text().is_empty());
            voice.reduce(
                VoiceEvent::Partial {
                    generation: 2,
                    item_id: None,
                    text: "current".into(),
                },
                cx,
            );
            let mut foreign = snapshot;
            foreign.chat_id = "other".into();
            foreign.generation = 3;
            voice.reduce(VoiceEvent::Snapshot { snapshot: foreign }, cx);
            assert_eq!(voice.snapshot.as_ref().unwrap().generation, 2);
            voice.reduce(
                VoiceEvent::Closed {
                    generation: 1,
                    reason: None,
                },
                cx,
            );
            assert!(voice.is_live());
            voice.set_stage_open(true, cx);
            assert!(voice.stage_open);
            voice.cancel(cx);
            assert!(!voice.is_live());
            assert!(!voice.stage_open);
            assert!(voice.caption.text().is_empty());
            // A closed session never reopens the stage.
            voice.set_stage_open(true, cx);
            assert!(!voice.stage_open);
        });
    }

    /// Codex delivers the user's final after the answer started streaming:
    /// the caption stays on the answer and keeps every streamed word.
    #[gpui::test]
    fn late_user_final_keeps_the_streaming_answer_captioned(cx: &mut gpui::TestAppContext) {
        let voice = cx.new(|_| VoiceController::default());
        voice.update(cx, |voice, cx| {
            voice.chat_id = Some("chat".into());
            voice.reduce(
                VoiceEvent::Snapshot {
                    snapshot: snapshot(),
                },
                cx,
            );
            let partial = |item: &str, text: &str| VoiceEvent::Partial {
                generation: 2,
                item_id: Some(item.into()),
                text: text.into(),
            };
            voice.reduce(partial("user", "Open the test"), cx);
            voice.reduce(partial("answer", "Sure, "), cx);
            voice.reduce(
                VoiceEvent::Final {
                    transcript: VoiceTranscript {
                        session_id: "one".into(),
                        item_id: "user".into(),
                        role: VoiceRole::User,
                        text: "Open the test.".into(),
                        promoted_message_id: None,
                    },
                },
                cx,
            );
            voice.reduce(partial("answer", "opening it."), cx);
            assert_eq!(voice.caption.text(), "Sure, opening it.");
            assert_eq!(voice.caption_item(), Some("answer"));
        });
    }

    #[gpui::test]
    fn provider_failure_closes_the_stage_and_keeps_its_reason(cx: &mut gpui::TestAppContext) {
        let voice = cx.new(|_| VoiceController::default());
        voice.update(cx, |voice, cx| {
            voice.chat_id = Some("chat".into());
            voice.reduce(
                VoiceEvent::Snapshot {
                    snapshot: snapshot(),
                },
                cx,
            );
            voice.set_stage_open(true, cx);
            voice.reduce(
                VoiceEvent::Closed {
                    generation: 2,
                    reason: Some(VoiceRejection::DeviceUnavailable),
                },
                cx,
            );
            assert_eq!(voice.phase, VoicePhase::Failed);
            assert!(!voice.stage_open);
            assert!(!voice.reason_text().is_empty());
            voice.dismiss_reason(cx);
            assert_eq!(voice.phase, VoicePhase::Closed);
        });
    }

    #[test]
    fn call_timer_reads_like_a_call() {
        assert_eq!(format_elapsed(0), "0:00");
        assert_eq!(format_elapsed(75), "1:15");
        assert_eq!(format_elapsed(3600 + 62), "1:01:02");
    }

    #[test]
    fn visual_state_keeps_muting_and_playback_orthogonal() {
        let mut s = snapshot();
        s.muted = true;
        s.playing = true;
        assert_eq!(orb_state(s.phase, Some(&s)), OrbState::Composing);
        s.playing = false;
        assert_eq!(orb_state(s.phase, Some(&s)), OrbState::Breathing);
        s.muted = false;
        assert_eq!(orb_state(s.phase, Some(&s)), OrbState::Listening);
        s.work = VoiceWork::Working;
        assert_eq!(orb_state(s.phase, Some(&s)), OrbState::Working);
        s.work = VoiceWork::AwaitingInput;
        assert_eq!(orb_state(s.phase, Some(&s)), OrbState::Solving);
        assert_eq!(
            orb_state(VoicePhase::Starting, Some(&s)),
            OrbState::Connecting
        );
    }
}
