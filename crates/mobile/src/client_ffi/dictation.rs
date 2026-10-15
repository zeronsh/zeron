//! A dictation taken on a host (`zeron_proto::voice::dictation`): the phone
//! captures and streams PCM to the device running its chat, whose speech
//! model writes it down. Only audio chunks and words cross here; the phone
//! runs no model.
use super::*;
use base64::Engine as _;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use zeron_proto::voice::dictation as wire;
use zeron_rpc::methods;

#[derive(Clone, Debug, uniffi::Enum)]
pub enum HostDictationEvent {
    /// The host takes audio from now on.
    Listening,
    Partial {
        text: String,
    },
    /// The host closed the capture itself (the limit): the final follows.
    Finalizing,
    Final {
        text: String,
    },
    Unavailable {
        reason: DictationUnavailable,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DictationUnavailable {
    /// The host has no speech model: download it in its Settings → Voice.
    ModelMissing,
    /// The host is already taking a dictation.
    Busy,
    HostUnreachable,
    /// The host's Zeron is too old to take dictations.
    HostIncompatible,
}

#[uniffi::export(with_foreign)]
pub trait HostDictationListener: Send + Sync {
    fn on_dictation_event(&self, event: HostDictationEvent);
}

/// One dictation. Dropping it cancels.
#[derive(uniffi::Object)]
pub struct HostDictation {
    audio: mpsc::Sender<Vec<u8>>,
    finished: Arc<AtomicBool>,
    wake: Arc<Notify>,
    cancel: CancellationToken,
}

impl Drop for HostDictation {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Audio queued ahead of the relay: six seconds at 100 ms a push.
const QUEUE: usize = 64;

#[uniffi::export]
impl HostDictation {
    /// Mono PCM16LE at [`wire::SAMPLE_RATE`]; a push the relay cannot keep
    /// up with is dropped rather than held.
    pub fn push_audio(&self, pcm16le: Vec<u8>) {
        if !self.finished.load(Ordering::Acquire) {
            let _ = self.audio.try_send(pcm16le);
        }
    }

    /// No more audio: the host transcribes what it has.
    pub fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.wake.notify_one();
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

fn unavailable(error: &zeron_rpc::RpcError) -> DictationUnavailable {
    let message = error.to_string();
    if message.contains("ModelMissing") {
        DictationUnavailable::ModelMissing
    } else if message.contains("Busy") {
        DictationUnavailable::Busy
    } else if matches!(error, zeron_rpc::RpcError::UnknownMethod(_))
        || message.contains("Unsupported")
    {
        DictationUnavailable::HostIncompatible
    } else {
        DictationUnavailable::HostUnreachable
    }
}

fn map(event: wire::DictationEvent) -> HostDictationEvent {
    match event {
        wire::DictationEvent::Listening => HostDictationEvent::Listening,
        wire::DictationEvent::Partial { text } => HostDictationEvent::Partial { text },
        wire::DictationEvent::Finalizing => HostDictationEvent::Finalizing,
        wire::DictationEvent::Final { text } => HostDictationEvent::Final { text },
        wire::DictationEvent::Failed { message } => HostDictationEvent::Failed { message },
    }
}

#[uniffi::export]
impl CoreClient {
    /// Opens a dictation on `host_device_id`. Returns at once; events
    /// arrive on `listener` from a client thread.
    pub fn start_dictation(
        &self,
        host_device_id: String,
        listener: Arc<dyn HostDictationListener>,
    ) -> Arc<HostDictation> {
        let (audio, mut chunks) = mpsc::channel::<Vec<u8>>(QUEUE);
        let finished = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Notify::new());
        let cancel = self.client.voice_cancellation();
        let handle = Arc::new(HostDictation {
            audio,
            finished: finished.clone(),
            wake: wake.clone(),
            cancel: cancel.clone(),
        });
        let client = self.client.clone();
        zc::runtime::handle().spawn(async move {
            let session_id = uuid::Uuid::new_v4().to_string();
            let fail = |event: HostDictationEvent| listener.on_dictation_event(event);
            let Ok(transport) = client.voice_transport(&host_device_id).await else {
                return fail(HostDictationEvent::Unavailable {
                    reason: DictationUnavailable::HostUnreachable,
                });
            };
            let rpc = transport.client.clone();
            let envelope = |payload: serde_json::Value| {
                json!({"targetDeviceId": host_device_id, "payload": payload})
            };
            let call = |method: &'static str, payload: serde_json::Value, seconds: u64| {
                let rpc = rpc.clone();
                let params = envelope(payload);
                async move {
                    tokio::time::timeout(Duration::from_secs(seconds), rpc.call(method, params))
                        .await
                        .unwrap_or(Err(zeron_rpc::RpcError::Closed))
                }
            };
            let start = wire::Start {
                session_id: session_id.clone(),
            };
            let started = tokio::select! {biased;
                _ = cancel.cancelled() => return,
                r = call(methods::START_DICTATION_V1, json!(start), 15) => r,
            };
            if let Err(error) = started {
                return fail(HostDictationEvent::Unavailable {
                    reason: unavailable(&error),
                });
            }
            let session = json!(wire::Session {
                session_id: session_id.clone(),
            });
            // Whatever happens next, the host is told when this ends.
            struct Closing {
                rpc: Arc<zeron_rpc::RpcClient>,
                params: serde_json::Value,
                orderly: bool,
            }
            impl Drop for Closing {
                fn drop(&mut self) {
                    if self.orderly {
                        return;
                    }
                    let (rpc, params) = (self.rpc.clone(), self.params.clone());
                    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                        runtime.spawn(async move {
                            let _ = rpc.call(methods::CANCEL_DICTATION_V1, params).await;
                        });
                    }
                }
            }
            let mut closing = Closing {
                rpc: rpc.clone(),
                params: envelope(session.clone()),
                orderly: false,
            };
            let events = tokio::time::timeout(
                Duration::from_secs(10),
                rpc.subscribe_checked(methods::OWN_DICTATION_V1, envelope(session.clone())),
            )
            .await;
            let mut events = match events {
                Ok(Ok(events)) => events,
                _ => {
                    return fail(HostDictationEvent::Failed {
                        message: "The host did not answer.".into(),
                    });
                }
            };
            let heard = listener.clone();
            let ended = Arc::new(AtomicBool::new(false));
            let done = ended.clone();
            let watch = cancel.child_token();
            let feed_cancel = watch.clone();
            let listening = tokio::spawn(async move {
                loop {
                    let item = tokio::select! {biased;
                        _ = watch.cancelled() => return,
                        item = events.recv() => item,
                    };
                    let Some(item) = item else {
                        if !done.load(Ordering::Acquire) {
                            heard.on_dictation_event(HostDictationEvent::Failed {
                                message: "The host stopped answering.".into(),
                            });
                        }
                        return;
                    };
                    let Ok(event) = serde_json::from_value::<wire::DictationEvent>(item) else {
                        continue;
                    };
                    let last = matches!(
                        event,
                        wire::DictationEvent::Final { .. } | wire::DictationEvent::Failed { .. }
                    );
                    heard.on_dictation_event(map(event));
                    if last {
                        done.store(true, Ordering::Release);
                        return;
                    }
                }
            });
            let mut sequence = 0u64;
            let mut pending: Vec<u8> = Vec::new();
            loop {
                tokio::select! {biased;
                    _ = cancel.cancelled() => break,
                    chunk = chunks.recv() => {
                        if let Some(chunk) = chunk { pending.extend(chunk); }
                    }
                    _ = wake.notified() => {}
                }
                // Everything queued goes in one chunk at a time.
                while let Ok(more) = chunks.try_recv() {
                    pending.extend(more);
                }
                let mut sent_ok = true;
                while !pending.is_empty() {
                    let take = pending.len().min(wire::MAX_CHUNK_BYTES) & !1;
                    let bytes: Vec<u8> = pending.drain(..take).collect();
                    sequence += 1;
                    let chunk = wire::Chunk {
                        session_id: session_id.clone(),
                        sequence,
                        data: base64::engine::general_purpose::STANDARD.encode(bytes),
                    };
                    if call(methods::FEED_DICTATION_V1, json!(chunk), 10).await.is_err() {
                        sent_ok = false;
                        break;
                    }
                }
                if !sent_ok {
                    feed_cancel.cancel();
                    ended.store(true, Ordering::Release);
                    fail(HostDictationEvent::Failed {
                        message: "Lost the connection to the host.".into(),
                    });
                    return;
                }
                if finished.load(Ordering::Acquire) && chunks.is_empty() {
                    if call(methods::FINISH_DICTATION_V1, session.clone(), 10).await.is_err() {
                        feed_cancel.cancel();
                        ended.store(true, Ordering::Release);
                        fail(HostDictationEvent::Failed {
                            message: "Lost the connection to the host.".into(),
                        });
                        return;
                    }
                    break;
                }
            }
            if cancel.is_cancelled() {
                feed_cancel.cancel();
                return;
            }
            // Finished: the words come on the event stream.
            tokio::select! {biased;
                _ = cancel.cancelled() => { feed_cancel.cancel(); return; }
                _ = listening => {}
            }
            closing.orderly = ended.load(Ordering::Acquire);
        });
        handle
    }
}
