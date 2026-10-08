//! Bounded platform callbacks; no PCM or provider credentials cross UniFFI.
//! The call state is reduced here (`zeron_voice_session::VoiceView`), so the
//! platform renders typed snapshots exactly as the desktop reads them.
use super::*;
use crate::orb::VoiceOrb;
use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use zeron_proto::voice::{VoicePhase, VoiceRejection, VoiceRole, VoiceWork, remote::Sdp};
use zeron_voice_session::{VoiceMediaEndpoint, VoiceView};

#[derive(Clone, Copy, uniffi::Enum)]
pub enum VoiceMediaOperation {
    Prepare,
    Offer,
    ApplyAnswer,
    SetMuted,
    Levels,
    Close,
}
#[derive(Clone, uniffi::Record)]
pub struct VoiceMediaRequest {
    pub request_id: u64,
    pub operation: VoiceMediaOperation,
    pub sdp: Option<String>,
    pub muted: bool,
}
/// Why a platform media operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum VoiceMediaFailure {
    PermissionDenied,
    Unavailable,
}
#[uniffi::export(with_foreign)]
pub trait VoiceMediaListener: Send + Sync {
    fn on_request(&self, request: VoiceMediaRequest);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum VoiceCallPhase {
    Connecting,
    Active,
    Ending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum VoiceCallWork {
    Idle,
    /// The orchestrator's Codex turn (or a delegation) is running.
    Working,
    /// Codex asked the user something; answer it in the transcript.
    AwaitingInput,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum VoiceSpeaker {
    User,
    Assistant,
}
/// Everything a call screen shows. Ephemeral: never log or persist captions.
#[derive(Clone, Debug, uniffi::Record)]
pub struct VoiceCallState {
    pub phase: VoiceCallPhase,
    pub orb: VoiceOrb,
    /// Full id of the host's orchestrator chat (its canonical transcript).
    pub chat_id: Option<String>,
    pub work: VoiceCallWork,
    pub muted: bool,
    /// The assistant's voice is playing.
    pub speaking: bool,
    pub caption: String,
    /// Set once the caption is a final segment; live partials have none.
    pub caption_speaker: Option<VoiceSpeaker>,
    /// The utterance the caption belongs to: a change is a new speaker turn.
    pub caption_item: Option<String>,
    /// Normalized 0…1 peaks (microphone is 0 while muted).
    pub microphone: f32,
    pub speaker: f32,
    /// Styles the host offers, once it reports them.
    pub voices: Vec<String>,
}
/// Why a call ended without the user hanging up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum VoiceEndReason {
    MicrophoneDenied,
    AudioUnavailable,
    /// Codex on the host must be signed in with ChatGPT.
    SignInRequired,
    UsageUnavailable,
    /// The host already has a voice call.
    Busy,
    HostUnavailable,
    /// The host's Zeron is too old or has remote voice disabled.
    HostIncompatible,
    ConnectionLost,
}
impl From<VoiceRejection> for VoiceEndReason {
    fn from(reason: VoiceRejection) -> Self {
        use VoiceRejection::*;
        match reason {
            MicrophonePermissionDenied => Self::MicrophoneDenied,
            DeviceUnavailable | MicrophoneMetadataMissing => Self::AudioUnavailable,
            ChatgptRequired => Self::SignInRequired,
            IncludedUsageUnavailable | CreditExclusionUnverified => Self::UsageUnavailable,
            Busy => Self::Busy,
            RemoteHost => Self::HostUnavailable,
            Disabled
            | Unsupported
            | WrongHarness
            | NativeRuntimeUnavailable
            | AudioFormatUnverified
            | DuplexUnverified => Self::HostIncompatible,
            InvalidLease | StaleGeneration | Overflow | Protocol => Self::ConnectionLost,
        }
    }
}
#[uniffi::export(with_foreign)]
pub trait VoiceSessionListener: Send + Sync {
    fn on_voice_state(&self, state: VoiceCallState);
    /// Terminal, exactly once. `None` is an orderly end.
    fn on_voice_closed(&self, reason: Option<VoiceEndReason>);
}

/// Hosts advertising this capability accept client-media voice calls.
#[uniffi::export]
pub fn voice_host_capability() -> String {
    zeron_proto::voice::remote::CAPABILITY.into()
}
/// Codex voice styles to offer before a host reports its own.
#[uniffi::export]
pub fn default_voice_styles() -> Vec<String> {
    zeron_proto::voice::DEFAULT_VOICES
        .iter()
        .map(|voice| (*voice).to_owned())
        .collect()
}

fn call_state(view: &VoiceView) -> VoiceCallState {
    VoiceCallState {
        phase: match view.phase {
            VoicePhase::Active => VoiceCallPhase::Active,
            VoicePhase::Stopping | VoicePhase::Closed | VoicePhase::Failed => {
                VoiceCallPhase::Ending
            }
            VoicePhase::Checking | VoicePhase::Starting => VoiceCallPhase::Connecting,
        },
        orb: VoiceOrb::from_state(view.orb_state()),
        chat_id: view.chat_id().map(str::to_owned),
        work: match view.work() {
            VoiceWork::Idle => VoiceCallWork::Idle,
            VoiceWork::Working => VoiceCallWork::Working,
            VoiceWork::AwaitingInput => VoiceCallWork::AwaitingInput,
        },
        muted: view.muted(),
        speaking: view.playing(),
        caption: view.caption().to_owned(),
        caption_speaker: view.caption_role().map(|role| match role {
            VoiceRole::User => VoiceSpeaker::User,
            VoiceRole::Assistant => VoiceSpeaker::Assistant,
        }),
        caption_item: view.caption_item().map(str::to_owned),
        microphone: view.microphone_level(),
        speaker: view.speaker_level(),
        voices: view
            .snapshot
            .as_ref()
            .map(|s| s.voices.clone())
            .unwrap_or_default(),
    }
}

struct Reply {
    sdp: Option<String>,
    microphone: u16,
    speaker: u16,
}
struct PlatformMedia {
    listener: Arc<dyn VoiceMediaListener>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Reply, VoiceRejection>>>>,
    next: AtomicU64,
    closed: AtomicBool,
}
impl PlatformMedia {
    async fn request(
        &self,
        operation: VoiceMediaOperation,
        sdp: Option<String>,
        muted: bool,
    ) -> Result<Reply, VoiceRejection> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if self.closed.load(Ordering::Acquire) {
                return Err(VoiceRejection::InvalidLease);
            }
            if pending.len() >= 4 {
                return Err(VoiceRejection::Overflow);
            }
            pending.insert(id, tx);
        }
        struct Pending<'a>(&'a PlatformMedia, u64);
        impl Drop for Pending<'_> {
            fn drop(&mut self) {
                self.0.pending.lock().unwrap().remove(&self.1);
            }
        }
        let _pending = Pending(self, id);
        self.listener.on_request(VoiceMediaRequest {
            request_id: id,
            operation,
            sdp,
            muted,
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| VoiceRejection::DeviceUnavailable)?
            .map_err(|_| VoiceRejection::DeviceUnavailable)?
    }
}
#[async_trait::async_trait]
impl VoiceMediaEndpoint for PlatformMedia {
    async fn prepare(&self) -> Result<(), VoiceRejection> {
        self.request(VoiceMediaOperation::Prepare, None, true)
            .await
            .map(|_| ())
    }
    async fn offer(&self) -> Result<Sdp, VoiceRejection> {
        Sdp::new(
            self.request(VoiceMediaOperation::Offer, None, true)
                .await?
                .sdp
                .ok_or(VoiceRejection::Protocol)?,
        )
    }
    async fn apply_answer(&self, answer: Sdp) -> Result<(), VoiceRejection> {
        self.request(
            VoiceMediaOperation::ApplyAnswer,
            Some(answer.expose().into()),
            true,
        )
        .await
        .map(|_| ())
    }
    async fn set_muted(&self, muted: bool) -> Result<(), VoiceRejection> {
        self.request(VoiceMediaOperation::SetMuted, None, muted)
            .await
            .map(|_| ())
    }
    async fn levels(&self) -> Result<(u16, u16), VoiceRejection> {
        let r = self
            .request(VoiceMediaOperation::Levels, None, true)
            .await?;
        Ok((r.microphone, r.speaker))
    }
    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, tx) in pending {
            let _ = tx.send(Err(VoiceRejection::InvalidLease));
        }
        self.listener.on_request(VoiceMediaRequest {
            request_id: 0,
            operation: VoiceMediaOperation::Close,
            sdp: None,
            muted: true,
        });
    }
}

#[derive(uniffi::Object)]
pub struct VoiceCall {
    media: Arc<PlatformMedia>,
    cancel: CancellationToken,
    muted: watch::Sender<bool>,
}
impl Drop for VoiceCall {
    fn drop(&mut self) {
        self.media.close();
        self.cancel.cancel();
    }
}
#[uniffi::export]
impl VoiceCall {
    pub fn set_muted(&self, muted: bool) {
        if !self.cancel.is_cancelled() {
            self.muted.send_replace(muted);
        }
    }
    pub fn stop(&self) {
        self.media.close();
        self.cancel.cancel();
    }
    /// Late native callbacks simply miss the retired request. No resume is possible.
    pub fn complete_media(
        &self,
        request_id: u64,
        failure: Option<VoiceMediaFailure>,
        sdp: Option<String>,
        microphone: u16,
        speaker: u16,
    ) {
        if self.media.closed.load(Ordering::Acquire) {
            return;
        }
        if let Some(tx) = self.media.pending.lock().unwrap().remove(&request_id) {
            let reply = match failure {
                Some(VoiceMediaFailure::PermissionDenied) => {
                    Err(VoiceRejection::MicrophonePermissionDenied)
                }
                Some(VoiceMediaFailure::Unavailable) => Err(VoiceRejection::DeviceUnavailable),
                None if sdp.as_ref().is_some_and(|s| s.len() > 65_536) => {
                    Err(VoiceRejection::DeviceUnavailable)
                }
                None => Ok(Reply {
                    sdp,
                    microphone,
                    speaker,
                }),
            };
            let _ = tx.send(reply);
        }
    }
}

/// Reduce owner events and local mute into call snapshots for the platform.
async fn present(
    listener: Arc<dyn VoiceSessionListener>,
    mut events: mpsc::Receiver<zeron_proto::voice::VoiceEvent>,
    muted: watch::Receiver<bool>,
) {
    let mut view = VoiceView::new();
    listener.on_voice_state(call_state(&view));
    let mut muted = Some(muted);
    loop {
        let changed = tokio::select! {
            event = events.recv() => match event {
                Some(event) => view.reduce(event, std::time::Instant::now()),
                None => return,
            },
            changed = async { muted.as_mut().unwrap().changed().await }, if muted.is_some() => {
                match changed {
                    Ok(()) => {
                        let value = *muted.as_mut().unwrap().borrow_and_update();
                        view.set_muted(value)
                    }
                    // The call handle is gone; its cancellation ends us next.
                    Err(_) => {
                        muted = None;
                        false
                    }
                }
            }
        };
        if changed {
            listener.on_voice_state(call_state(&view));
        }
    }
}

#[uniffi::export]
impl CoreClient {
    /// Returns immediately. Callbacks are dispatched on the core runtime; the
    /// platform must hop to its media/UI executor and call complete_media.
    pub fn start_voice(
        &self,
        host_device_id: String,
        voice: Option<String>,
        media: Arc<dyn VoiceMediaListener>,
        listener: Arc<dyn VoiceSessionListener>,
    ) -> Arc<VoiceCall> {
        let platform = Arc::new(PlatformMedia {
            listener: media,
            pending: Default::default(),
            next: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        });
        let cancel = self.client.voice_cancellation();
        let (muted, rx) = watch::channel(false);
        let presented_mute = rx.clone();
        let handle = Arc::new(VoiceCall {
            media: platform.clone(),
            cancel: cancel.clone(),
            muted,
        });
        let client = self.client.clone();
        zc::runtime::handle().spawn(async move {
            let (events, receiver) = mpsc::channel(32);
            let forward = tokio::spawn(present(listener.clone(), receiver, presented_mute));
            let operation = async {
                let control = client
                    .voice_transport(&host_device_id)
                    .await
                    .map_err(|_| VoiceRejection::RemoteHost)?;
                let config = zeron_proto::ChatConfig {
                    harness: zeron_proto::HarnessId::Codex,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                };
                zeron_voice_session::run(
                    control,
                    platform.clone(),
                    config,
                    voice,
                    cancel.clone(),
                    events,
                    rx,
                )
                .await
            };
            let result = tokio::select! {biased;_=cancel.cancelled()=>Ok(()),r=operation=>r};
            platform.close();
            // Every sender is gone: the presenter drains what was queued.
            let _ = forward.await;
            listener.on_voice_closed(result.err().map(VoiceEndReason::from));
        });
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::voice::{VoiceEvent, VoiceSnapshot};
    struct Listener(Mutex<Vec<u64>>);
    impl VoiceMediaListener for Listener {
        fn on_request(&self, r: VoiceMediaRequest) {
            self.0.lock().unwrap().push(r.request_id);
        }
    }
    fn media(listener: Arc<Listener>) -> Arc<PlatformMedia> {
        Arc::new(PlatformMedia {
            listener,
            pending: Default::default(),
            next: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        })
    }
    #[tokio::test]
    async fn close_releases_pending_callbacks_and_rejects_late_replies() {
        let listener = Arc::new(Listener(Mutex::new(vec![])));
        let media = media(listener.clone());
        let clone = media.clone();
        let task = tokio::spawn(async move { clone.offer().await });
        tokio::task::yield_now().await;
        media.close();
        assert!(task.await.unwrap().is_err());
        assert!(media.pending.lock().unwrap().is_empty());
        let (muted, _) = watch::channel(false);
        let call = VoiceCall {
            media: media.clone(),
            cancel: CancellationToken::new(),
            muted,
        };
        call.complete_media(1, None, Some("late SDP".into()), 0, 0);
        assert!(media.offer().await.is_err());
        assert_eq!(
            listener
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|id| **id == 0)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn denied_microphone_is_reported_as_such() {
        let listener = Arc::new(Listener(Mutex::new(vec![])));
        let media = media(listener.clone());
        let (muted, _) = watch::channel(false);
        let call = VoiceCall {
            media: media.clone(),
            cancel: CancellationToken::new(),
            muted,
        };
        let clone = media.clone();
        let task = tokio::spawn(async move { clone.prepare().await });
        while listener.0.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        call.complete_media(1, Some(VoiceMediaFailure::PermissionDenied), None, 0, 0);
        let reason = task.await.unwrap().unwrap_err();
        assert_eq!(
            VoiceEndReason::from(reason),
            VoiceEndReason::MicrophoneDenied
        );
    }

    struct States(Mutex<Vec<VoiceCallState>>);
    impl VoiceSessionListener for States {
        fn on_voice_state(&self, state: VoiceCallState) {
            self.0.lock().unwrap().push(state);
        }
        fn on_voice_closed(&self, _: Option<VoiceEndReason>) {}
    }

    #[tokio::test]
    async fn presenter_names_the_orchestrator_and_applies_local_mute_first() {
        let states = Arc::new(States(Mutex::new(vec![])));
        let (events, receiver) = mpsc::channel(8);
        let (mute, muted) = watch::channel(false);
        let task = tokio::spawn(present(states.clone(), receiver, muted));
        events
            .send(VoiceEvent::Snapshot {
                snapshot: VoiceSnapshot {
                    session_id: "s".into(),
                    chat_id: "voice-orchestrator-full".into(),
                    generation: 1,
                    phase: VoicePhase::Active,
                    muted: false,
                    playing: false,
                    work: VoiceWork::AwaitingInput,
                    reason: None,
                    voice: None,
                    voices: vec!["maple".into()],
                },
            })
            .await
            .unwrap();
        while states.0.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
        mute.send_replace(true);
        while states.0.lock().unwrap().len() < 3 {
            tokio::task::yield_now().await;
        }
        drop(events);
        task.await.unwrap();
        let states = states.0.lock().unwrap();
        assert_eq!(states[0].phase, VoiceCallPhase::Connecting);
        assert_eq!(states[0].orb, VoiceOrb::Connecting);
        assert_eq!(
            states[1].chat_id.as_deref(),
            Some("voice-orchestrator-full")
        );
        assert_eq!(states[1].work, VoiceCallWork::AwaitingInput);
        assert_eq!(states[1].orb, VoiceOrb::AwaitingInput);
        assert_eq!(states[1].voices, vec!["maple".to_owned()]);
        assert!(states[2].muted);
    }
}
