//! Shared call lifecycle. Platforms own media; the execution host owns Codex.
mod view;
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
pub use view::{Caption, SpeakerActivity, VoiceView, orb_state};
use zeron_proto::voice::{remote as wire, *};
use zeron_rpc::{RpcClient, methods};

#[async_trait]
pub trait VoiceMediaEndpoint: Send + Sync {
    /// Platform permission and media runtime only; capture stays muted.
    async fn prepare(&self) -> Result<(), VoiceRejection>;
    async fn offer(&self) -> Result<wire::Sdp, VoiceRejection>;
    /// Return only when the local transport is ready. Devices remain muted.
    async fn apply_answer(&self, answer: wire::Sdp) -> Result<(), VoiceRejection>;
    async fn set_muted(&self, muted: bool) -> Result<(), VoiceRejection>;
    async fn levels(&self) -> Result<(u16, u16), VoiceRejection>;
    /// Must stop local capture without awaiting any RPC or pending media call.
    fn close(&self);
}
#[async_trait]
pub trait VoiceControlTransport: Send + Sync {
    async fn call(
        &self,
        method: &str,
        payload: Value,
        seconds: u64,
    ) -> Result<Value, VoiceRejection>;
    async fn own(
        &self,
        lease: &wire::Lease,
    ) -> Result<BoxStream<'static, Result<VoiceEvent, VoiceRejection>>, VoiceRejection>;
}
/// Uses ephemeral RPC only. No offline ledger, nudge or implicit side-effect retry.
pub struct RpcTransport {
    pub client: Arc<RpcClient>,
    pub host: String,
}
#[async_trait]
impl VoiceControlTransport for RpcTransport {
    async fn call(
        &self,
        method: &str,
        payload: Value,
        seconds: u64,
    ) -> Result<Value, VoiceRejection> {
        tokio::time::timeout(
            Duration::from_secs(seconds),
            self.client.call(
                method,
                json!({"targetDeviceId":self.host,"payload":payload}),
            ),
        )
        .await
        .map_err(|_| VoiceRejection::Protocol)?
        .map_err(rejection)
    }
    async fn own(
        &self,
        lease: &wire::Lease,
    ) -> Result<BoxStream<'static, Result<VoiceEvent, VoiceRejection>>, VoiceRejection> {
        let rx = tokio::time::timeout(
            Duration::from_secs(8),
            self.client.subscribe_checked(
                methods::OWN_VOICE_V2,
                json!({"targetDeviceId":self.host,"payload":lease}),
            ),
        )
        .await
        .map_err(|_| VoiceRejection::Protocol)?
        .map_err(rejection)?;
        let client = self.client.clone();
        Ok(
            futures::stream::unfold((rx, client), |(mut rx, client)| async move {
                rx.recv().await.map(|v| {
                    (
                        serde_json::from_value(v).map_err(|_| VoiceRejection::Protocol),
                        (rx, client),
                    )
                })
            })
            .boxed(),
        )
    }
}
pub fn rejection(error: zeron_rpc::RpcError) -> VoiceRejection {
    let message = error.to_string();
    for reason in [
        VoiceRejection::Disabled,
        VoiceRejection::NativeRuntimeUnavailable,
        VoiceRejection::ChatgptRequired,
        VoiceRejection::Busy,
        VoiceRejection::InvalidLease,
        VoiceRejection::RemoteHost,
        VoiceRejection::WrongHarness,
        VoiceRejection::Unsupported,
        VoiceRejection::Overflow,
    ] {
        if message.contains(&format!("{reason:?}")) {
            return reason;
        }
    }
    if matches!(error, zeron_rpc::RpcError::UnknownMethod(_)) {
        VoiceRejection::Unsupported
    } else {
        VoiceRejection::Protocol
    }
}
async fn call<T: serde::de::DeserializeOwned>(
    control: &dyn VoiceControlTransport,
    method: &str,
    payload: impl serde::Serialize,
    seconds: u64,
) -> Result<T, VoiceRejection> {
    serde_json::from_value(
        control
            .call(
                method,
                serde_json::to_value(payload).map_err(|_| VoiceRejection::Protocol)?,
                seconds,
            )
            .await?,
    )
    .map_err(|_| VoiceRejection::Protocol)
}
struct Scope {
    media: Arc<dyn VoiceMediaEndpoint>,
    control: Arc<dyn VoiceControlTransport>,
    key: wire::AttemptKey,
    lease: Arc<Mutex<Option<wire::Lease>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    cancel: CancellationToken,
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.media.close();
        self.cancel.cancel();
        for task in &self.tasks {
            task.abort();
        }
        let control = self.control.clone();
        let key = self.key.clone();
        let lease = self.lease.lock().unwrap().clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(lease) = lease {
                    let _ = control.call(methods::STOP_VOICE_V2, json!(lease), 8).await;
                }
                let _ = control
                    .call(
                        methods::CANCEL_VOICE_ATTEMPT_V2,
                        json!(wire::Cancel { attempt_key: key }),
                        8,
                    )
                    .await;
            });
        }
    }
}

/// Runs once. Reconnection requires a new explicit call and fresh attempt key.
pub async fn run(
    control: Arc<dyn VoiceControlTransport>,
    media: Arc<dyn VoiceMediaEndpoint>,
    config: zeron_proto::ChatConfig,
    voice: Option<String>,
    cancel: CancellationToken,
    events: mpsc::Sender<VoiceEvent>,
    muted: watch::Receiver<bool>,
) -> Result<(), VoiceRejection> {
    let started = tokio::time::Instant::now();
    let local_cancel = cancel.child_token();
    let failure = Arc::new(Mutex::new(None));
    let mut scope = Scope {
        media: media.clone(),
        control: control.clone(),
        key: wire::AttemptKey::new(),
        lease: Arc::default(),
        tasks: vec![],
        cancel: local_cancel.clone(),
    };
    let operation = async {
        tracing::info!(
            stage = "capabilities",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        let cap: wire::Capabilities = call(
            control.as_ref(),
            methods::VOICE_CAPABILITIES_V2,
            json!({}),
            8,
        )
        .await?;
        if !cap.client_webrtc || cap.protocol != wire::CAPABILITY {
            return Err(VoiceRejection::Unsupported);
        }
        tracing::info!(
            stage = "local_media",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        media.prepare().await?;
        tracing::info!(
            stage = "prepare_host",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        let prepared: wire::Prepared = call(
            control.as_ref(),
            methods::PREPARE_VOICE_V2,
            wire::Prepare {
                attempt_key: scope.key.clone(),
                config,
                voice,
            },
            65,
        )
        .await?;
        *scope.lease.lock().unwrap() = Some(prepared.lease.clone());
        tracing::info!(
            stage = "attach_owner",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        let mut owner = control.own(&prepared.lease).await?;
        let lease = prepared.lease.clone();
        let owner_cancel = local_cancel.clone();
        let owner_events = events.clone();
        let owner_failure = failure.clone();
        let owner_media = media.clone();
        let owner_mute = muted.clone();
        scope.tasks.push(tokio::spawn(async move {
            let result = async {
                while let Some(event) = owner.next().await {
                    let mut event = event?;
                    if let VoiceEvent::Snapshot { snapshot } = &mut event {
                        snapshot.muted = *owner_mute.borrow();
                    }
                    let current = match &event {
                        VoiceEvent::Snapshot { snapshot } => {
                            snapshot.generation == lease.voice.generation
                                && snapshot.session_id == lease.voice.session_id
                        }
                        VoiceEvent::Final { transcript } => {
                            transcript.session_id == lease.voice.session_id
                        }
                        VoiceEvent::Partial { generation, .. }
                        | VoiceEvent::Closed { generation, .. }
                        | VoiceEvent::InvalidatePlayout { generation, .. } => {
                            *generation == lease.voice.generation
                        }
                        _ => return Err(VoiceRejection::Protocol),
                    };
                    if !current {
                        continue;
                    }
                    let closed = if let VoiceEvent::Closed { reason, .. } = &event {
                        Some(*reason)
                    } else {
                        None
                    };
                    owner_events
                        .try_send(event)
                        .map_err(|_| VoiceRejection::Overflow)?;
                    if let Some(reason) = closed {
                        return reason.map_or(Ok(()), Err);
                    }
                }
                Err(VoiceRejection::Protocol)
            }
            .await;
            *owner_failure.lock().unwrap() = result.err();
            owner_media.close();
            owner_cancel.cancel();
        }));
        let heartbeat_control = control.clone();
        let lease = prepared.lease.clone();
        let heartbeat_cancel = local_cancel.clone();
        let heartbeat_failure = failure.clone();
        let heartbeat_media = media.clone();
        let heartbeat_mute = muted.clone();
        scope.tasks.push(tokio::spawn(async move {
            let mut sequence = 0;
            let mut interval = tokio::time::interval(Duration::from_secs(wire::HEARTBEAT_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                sequence += 1;
                let muted_now = *heartbeat_mute.borrow();
                let result: Result<wire::Ack, _> = call(
                    heartbeat_control.as_ref(),
                    methods::REPORT_VOICE_MEDIA_V2,
                    wire::Report {
                        lease: lease.clone(),
                        sequence,
                        muted: muted_now,
                        state: wire::MediaState::Ready,
                    },
                    10,
                )
                .await;
                if !matches!(result,Ok(wire::Ack{sequence:s}) if s==sequence) {
                    *heartbeat_failure.lock().unwrap() = Some(VoiceRejection::Protocol);
                    heartbeat_media.close();
                    heartbeat_cancel.cancel();
                    return;
                }
            }
        }));
        tracing::info!(
            stage = "local_offer",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        let offer = media.offer().await?;
        let negotiation_id = wire::AttemptKey::new();
        tracing::info!(
            stage = "negotiate",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        let answer: wire::Negotiated = call(
            control.as_ref(),
            methods::NEGOTIATE_VOICE_V2,
            wire::Negotiate {
                lease: prepared.lease.clone(),
                negotiation_id: negotiation_id.clone(),
                offer,
            },
            95,
        )
        .await?;
        if answer.negotiation_id != negotiation_id {
            return Err(VoiceRejection::Protocol);
        }
        tracing::info!(
            stage = "apply_answer",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        media.apply_answer(answer.answer).await?;
        let initial_muted = *muted.borrow();
        tracing::info!(
            stage = "confirm",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        let snapshot: VoiceSnapshot = call(
            control.as_ref(),
            methods::CONFIRM_VOICE_MEDIA_V2,
            wire::Confirm {
                lease: prepared.lease.clone(),
                negotiation_id,
                muted: initial_muted,
            },
            8,
        )
        .await?;
        if snapshot.session_id != prepared.lease.voice.session_id
            || snapshot.generation != prepared.lease.voice.generation
        {
            return Err(VoiceRejection::InvalidLease);
        }
        if local_cancel.is_cancelled() {
            return Err(VoiceRejection::InvalidLease);
        }
        let muted_now = *muted.borrow();
        tracing::info!(
            stage = "active",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "remote voice stage"
        );
        media.set_muted(muted_now).await?;
        let mut muted = muted;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {biased;
                changed=muted.changed()=>{changed.map_err(|_|VoiceRejection::Protocol)?;let value=*muted.borrow_and_update();media.set_muted(value).await?;},
                _=tick.tick()=>{
                    let (microphone,speaker)=media.levels().await?;
                    // Visual samples are lossy and leave headroom: a slow consumer
                    // must drop meters, never overflow on owner finals/controls.
                    if events.capacity()>events.max_capacity()/4{
                        let _=events.try_send(VoiceEvent::Levels{generation:prepared.lease.voice.generation,microphone,speaker});
                    }
                },
                _=events.closed()=>return Ok(()),
            }
        }
    };
    let result = tokio::select! {biased;
        _=local_cancel.cancelled()=>failure.lock().unwrap().map_or(Ok(()),Err),
        result=operation=>result,
    };
    drop(scope);
    tracing::info!(elapsed_ms = started.elapsed().as_millis() as u64, reason = ?result.as_ref().err(), "remote voice closed");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Media {
        prepared: AtomicBool,
        closed: AtomicBool,
        block: bool,
    }
    #[async_trait]
    impl VoiceMediaEndpoint for Media {
        async fn prepare(&self) -> Result<(), VoiceRejection> {
            self.prepared.store(true, Ordering::SeqCst);
            Ok(())
        }
        async fn offer(&self) -> Result<wire::Sdp, VoiceRejection> {
            if self.block {
                futures::future::pending::<()>().await;
            }
            wire::Sdp::new("offer".into())
        }
        async fn apply_answer(&self, _: wire::Sdp) -> Result<(), VoiceRejection> {
            Ok(())
        }
        async fn set_muted(&self, _: bool) -> Result<(), VoiceRejection> {
            assert!(!self.closed.load(Ordering::SeqCst));
            Ok(())
        }
        async fn levels(&self) -> Result<(u16, u16), VoiceRejection> {
            Ok((100, 200))
        }
        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }
    struct Control {
        supported: bool,
        half_open: bool,
        calls: Mutex<Vec<String>>,
        lease: wire::Lease,
    }
    #[async_trait]
    impl VoiceControlTransport for Control {
        async fn call(&self, method: &str, p: Value, _: u64) -> Result<Value, VoiceRejection> {
            self.calls.lock().unwrap().push(method.into());
            if self.half_open && method == methods::REPORT_VOICE_MEDIA_V2 {
                tokio::time::sleep(Duration::from_secs(10)).await;
                return Err(VoiceRejection::Protocol);
            }
            Ok(match method {
                methods::VOICE_CAPABILITIES_V2 => json!(wire::Capabilities {
                    protocol: wire::CAPABILITY.into(),
                    client_webrtc: self.supported,
                    voices: vec![]
                }),
                methods::PREPARE_VOICE_V2 => json!(wire::Prepared {
                    lease: self.lease.clone(),
                    chat_id: "voice-orchestrator-full-id".into(),
                    voices: vec![],
                    heartbeat_seconds: 5,
                    lease_seconds: 15
                }),
                methods::NEGOTIATE_VOICE_V2 => json!(wire::Negotiated {
                    negotiation_id: serde_json::from_value(p["negotiationId"].clone()).unwrap(),
                    answer: wire::Sdp::new("answer".into()).unwrap()
                }),
                methods::REPORT_VOICE_MEDIA_V2 => json!({"sequence":p["sequence"]}),
                methods::CONFIRM_VOICE_MEDIA_V2 => json!(VoiceSnapshot {
                    session_id: self.lease.voice.session_id.clone(),
                    chat_id: "voice-orchestrator-full-id".into(),
                    generation: 1,
                    phase: VoicePhase::Active,
                    muted: false,
                    playing: false,
                    work: VoiceWork::Idle,
                    reason: None,
                    voice: None,
                    voices: vec![]
                }),
                _ => json!({}),
            })
        }
        async fn own(
            &self,
            _: &wire::Lease,
        ) -> Result<BoxStream<'static, Result<VoiceEvent, VoiceRejection>>, VoiceRejection>
        {
            Ok(futures::stream::pending().boxed())
        }
    }
    fn control(supported: bool) -> Arc<Control> {
        Arc::new(Control {
            supported,
            half_open: false,
            calls: Default::default(),
            lease: wire::Lease {
                host_device_id: "host".into(),
                voice: VoiceLease {
                    session_id: "session".into(),
                    generation: 1,
                    token: "secret".into(),
                },
            },
        })
    }
    fn config() -> zeron_proto::ChatConfig {
        serde_json::from_value(json!({"harness":"codex","sandbox":"danger-full-access"})).unwrap()
    }
    #[tokio::test]
    async fn incompatible_host_never_requests_microphone() {
        let media = Arc::new(Media {
            prepared: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            block: false,
        });
        let (tx, _rx) = mpsc::channel(8);
        let (_mute, muted) = watch::channel(false);
        assert_eq!(
            run(
                control(false),
                media.clone(),
                config(),
                None,
                CancellationToken::new(),
                tx,
                muted
            )
            .await
            .unwrap_err(),
            VoiceRejection::Unsupported
        );
        assert!(!media.prepared.load(Ordering::SeqCst));
        assert!(media.closed.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn cancelling_a_stalled_offer_closes_media_before_remote_cleanup() {
        let media = Arc::new(Media {
            prepared: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            block: true,
        });
        let control = control(true);
        let (tx, _rx) = mpsc::channel(8);
        let (_mute, muted) = watch::channel(false);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(
            control.clone(),
            media.clone(),
            config(),
            None,
            cancel.clone(),
            tx,
            muted,
        ));
        for _ in 0..100 {
            if media.prepared.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        cancel.cancel();
        task.await.unwrap().unwrap();
        assert!(media.closed.load(Ordering::SeqCst));
        tokio::task::yield_now().await;
        assert!(
            control
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|m| m == methods::CANCEL_VOICE_ATTEMPT_V2)
        );
    }
    #[tokio::test]
    async fn active_call_uses_local_levels_and_drop_closes_media() {
        let media = Arc::new(Media {
            prepared: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            block: false,
        });
        let (tx, mut rx) = mpsc::channel(8);
        let (_mute, muted) = watch::channel(false);
        let task = tokio::spawn(run(
            control(true),
            media.clone(),
            config(),
            None,
            CancellationToken::new(),
            tx,
            muted,
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap(),
            Some(VoiceEvent::Levels {
                microphone: 100,
                speaker: 200,
                ..
            })
        ));
        task.abort();
        let _ = task.await;
        assert!(media.closed.load(Ordering::SeqCst));
    }
    #[tokio::test(start_paused = true)]
    async fn half_open_heartbeat_closes_stalled_media_without_user_action() {
        let media = Arc::new(Media {
            prepared: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            block: true,
        });
        let mut control = control(true);
        Arc::get_mut(&mut control).unwrap().half_open = true;
        let (tx, _rx) = mpsc::channel(8);
        let (_mute, muted) = watch::channel(false);
        let started = tokio::time::Instant::now();
        let result = run(
            control,
            media.clone(),
            config(),
            None,
            CancellationToken::new(),
            tx,
            muted,
        )
        .await;
        assert_eq!(result.unwrap_err(), VoiceRejection::Protocol);
        assert!(media.closed.load(Ordering::SeqCst));
        assert!(started.elapsed() <= Duration::from_secs(wire::LEASE_SECS));
    }
}
