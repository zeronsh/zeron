//! Ephemeral native subscription voice in the same app-server process as text/MCP.
//! WebSocket appendAudio is deliberately unavailable: in Codex 0.159 it needs an API key.
#[path = "realtime_host.rs"]
mod host;
use crate::jsonrpc::{Incoming, RpcClient};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::{path::PathBuf, sync::atomic::Ordering, time::Duration};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
type AudioAbort = Arc<Mutex<Option<(u64, CancellationToken)>>>;
use zeron_proto::voice::*;

/// Every Zeron voice session is a projectless orchestrator. Given to both the
/// realtime voice model and the backing Codex model that runs its delegations.
pub const ORCHESTRATOR_INSTRUCTIONS: &str = "This is a Zeron voice session and you are the user's orchestrator. Use the Zeron MCP tools to do the work: list and read chats, create new chats with the right agent and project, send them messages, check on their progress, and report back. Delegate coding and file changes to those chats instead of doing them yourself in this session. Keep spoken replies short and conversational.";

#[derive(Clone)]
pub(super) struct ThreadContext {
    pub cwd: String,
    pub model_provider: Option<String>,
}

pub enum VoiceCommand {
    Probe {
        external: bool,
        reply: oneshot::Sender<Result<VoiceEligibility, VoiceRejection>>,
    },
    Start {
        voice: Option<String>,
        session_id: String,
        generation: u64,
        reply: oneshot::Sender<Result<(), VoiceRejection>>,
    },
    External {
        voice: Option<String>,
        session_id: String,
        generation: u64,
        offer: remote::Sdp,
        reply: oneshot::Sender<Result<remote::Sdp, VoiceRejection>>,
    },
    Mute {
        muted: bool,
        reply: oneshot::Sender<Result<(), VoiceRejection>>,
    },
    Append {
        frame: VoiceFrame,
        reply: oneshot::Sender<Result<(), VoiceRejection>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), VoiceRejection>>,
    },
}
pub struct RealtimeControls {
    audio_abort: AudioAbort,
    invalidated: Arc<std::sync::atomic::AtomicBool>,
    pub commands: mpsc::Receiver<VoiceCommand>,
    pub events: broadcast::Sender<VoiceEvent>,
}
#[derive(Clone)]
pub struct RealtimeHandle {
    audio_abort: AudioAbort,
    invalidated: Arc<std::sync::atomic::AtomicBool>,
    pub commands: mpsc::Sender<VoiceCommand>,
    pub events: broadcast::Sender<VoiceEvent>,
}
impl RealtimeHandle {
    pub fn invalidated(&self) -> bool {
        self.invalidated.load(Ordering::Acquire)
    }
    /// Immediately closes native capture even if the control actor is waiting on I/O.
    pub fn abort_voice(&self, generation: u64) {
        if let Some((current, cancel)) = self.audio_abort.lock().unwrap().as_ref() {
            if *current == generation {
                cancel.cancel();
            }
        }
    }

    pub async fn probe(&self) -> Result<VoiceEligibility, VoiceRejection> {
        self.probe_mode(false).await
    }
    pub async fn probe_external(&self) -> Result<VoiceEligibility, VoiceRejection> {
        self.probe_mode(true).await
    }
    async fn probe_mode(&self, external: bool) -> Result<VoiceEligibility, VoiceRejection> {
        let (reply, rx) = oneshot::channel();
        tokio::time::timeout(Duration::from_secs(15), async {
            self.commands
                .send(VoiceCommand::Probe { external, reply })
                .await
                .map_err(|_| VoiceRejection::Protocol)?;
            rx.await.map_err(|_| VoiceRejection::Protocol)?
        })
        .await
        .map_err(|_| VoiceRejection::Protocol)?
    }

    pub async fn stop(&self) -> Result<(), VoiceRejection> {
        let (reply, rx) = oneshot::channel();
        tokio::time::timeout(Duration::from_secs(8), async {
            self.commands
                .send(VoiceCommand::Stop { reply })
                .await
                .map_err(|_| VoiceRejection::Protocol)?;
            rx.await.map_err(|_| VoiceRejection::Protocol)?
        })
        .await
        .map_err(|_| VoiceRejection::Protocol)?
    }
    pub async fn mute(&self, muted: bool) -> Result<(), VoiceRejection> {
        let (reply, rx) = oneshot::channel();
        tokio::time::timeout(Duration::from_secs(8), async {
            self.commands
                .send(VoiceCommand::Mute { muted, reply })
                .await
                .map_err(|_| VoiceRejection::Protocol)?;
            rx.await.map_err(|_| VoiceRejection::Protocol)?
        })
        .await
        .map_err(|_| VoiceRejection::Protocol)?
    }
}
pub fn channel() -> (
    RealtimeHandle,
    RealtimeControls,
    broadcast::Receiver<VoiceEvent>,
) {
    let audio_abort = Arc::default();
    let invalidated = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (commands, rx) = mpsc::channel(8);
    let (events, receiver) = broadcast::channel(32);
    (
        RealtimeHandle {
            audio_abort: Arc::clone(&audio_abort),
            invalidated: invalidated.clone(),
            commands,
            events: events.clone(),
        },
        RealtimeControls {
            audio_abort,
            invalidated,
            commands: rx,
            events,
        },
        receiver,
    )
}
pub(crate) struct BridgeTask(tokio::task::JoinHandle<()>);
impl Drop for BridgeTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Stops routing realtime notifications once the bridge ends, so a text-only
/// runtime goes back to plain stdout handling.
struct VoiceSubscription {
    client: RpcClient,
    overflow: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for VoiceSubscription {
    fn drop(&mut self) {
        self.client.unsubscribe_voice(&self.overflow);
    }
}

pub(super) fn attach(
    client: RpcClient,
    thread: String,
    executable: PathBuf,
    context: ThreadContext,
    controls: RealtimeControls,
) -> BridgeTask {
    let abort = controls.audio_abort.clone();
    let invalidated = controls.invalidated.clone();
    let (mut wire, overflow) = client.subscribe_voice(move || {
        invalidated.store(true, Ordering::Release);
        if let Some((_, cancel)) = abort.lock().unwrap().as_ref() {
            cancel.cancel();
        }
    });
    // Moved into the task so it unsubscribes even when aborted before polling.
    let subscription = VoiceSubscription {
        client: client.clone(),
        overflow: overflow.clone(),
    };
    BridgeTask(tokio::spawn(async move {
        let _subscription = subscription;
        let RealtimeControls {
            audio_abort,
            invalidated,
            mut commands,
            events,
        } = controls;
        let mut native: Option<host::NativeHost> = None;
        let mut external_active = false;
        let mut external_reply: Option<oneshot::Sender<Result<remote::Sdp, VoiceRejection>>> = None;
        let mut starting: Option<
            futures::future::BoxFuture<'static, Result<Started, VoiceRejection>>,
        > = None;
        let mut start_reply: Option<oneshot::Sender<Result<(), VoiceRejection>>> = None;
        let mut answer_tx: Option<oneshot::Sender<String>> = None;
        let mut accepted_tx: Option<oneshot::Sender<()>> = None;
        let mut expected_session = String::new();
        let server_live = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut generation = 0;
        let mut sequence = 0;
        let mut timer = tokio::time::interval(Duration::from_millis(100));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased;
                command=commands.recv()=>match command {
                    Some(VoiceCommand::Probe{external,reply})=>{let result=if invalidated.load(Ordering::Acquire){Err(VoiceRejection::InvalidLease)}else{probe_mode(&client,&executable,&context,external).await};let _=reply.send(if invalidated.load(Ordering::Acquire){Err(VoiceRejection::InvalidLease)}else{result});},
                    Some(VoiceCommand::Start{voice,session_id,generation:next,reply})=>{
                        if invalidated.load(Ordering::Acquire){let _=reply.send(Err(VoiceRejection::InvalidLease));continue;}
                        if external_active || native.is_some() || starting.is_some() {let _=reply.send(Err(VoiceRejection::Busy));continue;}
                        generation=next;sequence=0;
                        let abort=CancellationToken::new();
                        {let mut slot=audio_abort.lock().unwrap();
                            if invalidated.load(Ordering::Acquire){let _=reply.send(Err(VoiceRejection::InvalidLease));continue;}
                            *slot=Some((next,abort.clone()));
                        }
                        let (answer,answer_rx)=oneshot::channel();let (accepted,accepted_rx)=oneshot::channel();
                        answer_tx=Some(answer);accepted_tx=Some(accepted);expected_session=session_id.clone();
                        starting=Some(Box::pin({let client=client.clone();let thread=thread.clone();let executable=executable.clone();let context=context.clone();let server_live=server_live.clone();async move {
                            start(client,thread,executable,context,voice,session_id,accepted_rx,answer_rx,abort,server_live).await.map(Started::Local)
                        }}));
                        start_reply=Some(reply);
                    },
                    Some(VoiceCommand::External{voice,session_id,generation:next,offer,reply})=>{
                        if invalidated.load(Ordering::Acquire){let _=reply.send(Err(VoiceRejection::InvalidLease));continue;}
                        if external_active || native.is_some() || starting.is_some(){let _=reply.send(Err(VoiceRejection::Busy));continue;}
                        generation=next;sequence=0;
                        let abort=CancellationToken::new();
                        *audio_abort.lock().unwrap()=Some((next,abort.clone()));
                        let (answer,answer_rx)=oneshot::channel();let (accepted,accepted_rx)=oneshot::channel();
                        answer_tx=Some(answer);accepted_tx=Some(accepted);expected_session=session_id.clone();
                        let client=client.clone();let thread=thread.clone();let context=context.clone();let server_live=server_live.clone();
                        starting=Some(Box::pin(async move {
                            tokio::select!{biased;
                                _=abort.cancelled()=>Err(VoiceRejection::InvalidLease),
                                answer=start_external(client,thread,context,voice,session_id,offer,accepted_rx,answer_rx,server_live)=>answer.map(Started::External),
                            }
                        }));
                        external_reply=Some(reply);
                    },
                    Some(VoiceCommand::Mute{muted,reply})=>{
                        let result=if let Some(host)=native.as_mut(){host.controls(muted).await}else{Err(VoiceRejection::Busy)};
                        let failed=result.is_err();let _=reply.send(result);
                        if failed {native=None;let _=events.send(VoiceEvent::Closed{generation,reason:Some(VoiceRejection::Protocol)});}
                    },
                    Some(VoiceCommand::Append{reply,..})=>{let _=reply.send(Err(VoiceRejection::Unsupported));},
                    Some(VoiceCommand::Stop{reply})=>{
                        let had_session=server_live.load(Ordering::Acquire);starting=None;native=None;external_active=false;
                        if let Some(reply)=external_reply.take(){let _=reply.send(Err(VoiceRejection::InvalidLease));}
                        if let Some(pending)=start_reply.take(){let _=pending.send(Err(VoiceRejection::InvalidLease));}
                        let result=if had_session{
                            let result=stop(&client,&thread).await;
                            // Native stop acknowledges submission first. Drain its terminal event
                            // before a successor can relabel old notifications with a new generation.
                            let closed=tokio::time::timeout(Duration::from_secs(3),async {
                                while let Some(notification)=wire.recv().await {
                                    if let Incoming::Notification{method,params}=notification {
                                        if params["threadId"]!=thread{continue;}
                                        if method=="thread/realtime/closed" && params["reason"]=="requested"{server_live.store(false,Ordering::Release);return Ok::<_,VoiceRejection>(());}
                                        if let Some(event)=normalize(&thread,generation,&mut sequence,&method,&params){let _=events.send(event);}
                                    }
                                }
                                Err(VoiceRejection::Protocol)
                            }).await;
                            if result.is_err()||!matches!(closed,Ok(Ok(()))){let _=reply.send(Err(VoiceRejection::Protocol));break;}
                            result
                        }else{Ok(())};
                        let _=events.send(VoiceEvent::Closed{generation,reason:None});let _=reply.send(result);
                    },
                    None=>break,
                },
                result=async{starting.as_mut().unwrap().await},if starting.is_some()=>{
                    starting=None;
                    match result {
                        Ok(Started::Local(host))=>{native=Some(host);if let Some(reply)=start_reply.take(){let _=reply.send(Ok(()));}},
                        Ok(Started::External(answer))=>{external_active=true;if let Some(reply)=external_reply.take(){let _=reply.send(Ok(answer));}},
                        Err(reason)=>{
                        if let Some(reply)=external_reply.take(){let _=reply.send(Err(reason));}
                        if let Some(reply)=start_reply.take(){let _=reply.send(Err(reason));}
                        let _=events.send(VoiceEvent::Closed{generation,reason:Some(reason)});
                    }}
                },
                notification=wire.recv()=>match notification {
                    Some(Incoming::Notification{method,params})=>{
                        if method=="thread/realtime/closed" && params["threadId"]==thread {server_live.store(false,Ordering::Release);}
                        if method=="account/updated" {native=None;starting=None;external_active=false;if let Some(reply)=external_reply.take(){let _=reply.send(Err(VoiceRejection::ChatgptRequired));}if let Some(reply)=start_reply.take(){let _=reply.send(Err(VoiceRejection::ChatgptRequired));}let _=events.send(VoiceEvent::Closed{generation,reason:Some(VoiceRejection::ChatgptRequired)});continue;}
                        if params["threadId"]==thread && starting.is_some() {
                            if method=="thread/realtime/started" {
                                if params["version"]=="v3" && params["realtimeSessionId"]==expected_session {if let Some(tx)=accepted_tx.take(){let _=tx.send(());}}
                                else {starting=None;if let Some(reply)=external_reply.take(){let _=reply.send(Err(VoiceRejection::Unsupported));}if let Some(reply)=start_reply.take(){let _=reply.send(Err(VoiceRejection::Unsupported));}let _=events.send(VoiceEvent::Closed{generation,reason:Some(VoiceRejection::Unsupported)});}
                                continue;
                            }
                            if method=="thread/realtime/sdp" {
                                if let Some(sdp)=params["sdp"].as_str().filter(|s|!s.is_empty()&&s.len()<=64*1024) {if let Some(tx)=answer_tx.take(){let _=tx.send(sdp.to_owned());}}
                                continue;
                            }
                        }
                        if external_active || native.is_some() || starting.is_some() {
                            if let Some(event)=normalize(&thread,generation,&mut sequence,&method,&params) {
                                let terminal=matches!(event,VoiceEvent::Closed{..}); let _=events.send(event);
                                if terminal {native=None;starting=None;external_active=false;if let Some(reply)=external_reply.take(){let _=reply.send(Err(VoiceRejection::Protocol));}if let Some(reply)=start_reply.take(){let _=reply.send(Err(VoiceRejection::Protocol));}}
                            }
                        }
                    },Some(_)=>{},None=>break,
                },
                _=timer.tick()=>{
                    if overflow.load(Ordering::Acquire)||client.is_closed(){break;}
                    if let Some(host)=native.as_mut(){ match host.levels().await {
                        Ok((microphone,speaker))=>{let _=events.send(VoiceEvent::Levels{generation,microphone,speaker});},
                        Err(reason)=>{native=None;let _=events.send(VoiceEvent::Closed{generation,reason:Some(reason)});}
                    }}
                }
            }
        }
        let had_session = starting.take().is_some()
            || native.is_some()
            || external_active
            || server_live.load(Ordering::Acquire);
        drop(native);
        if had_session {
            let _ = stop(&client, &thread).await;
        }
        let _ = events.send(VoiceEvent::Closed {
            generation,
            reason: Some(VoiceRejection::Protocol),
        });
    }))
}
enum Started {
    Local(host::NativeHost),
    External(remote::Sdp),
}

async fn probe(
    client: &RpcClient,
    executable: &std::path::Path,
    context: &ThreadContext,
) -> Result<VoiceEligibility, VoiceRejection> {
    probe_mode(client, executable, context, false).await
}
async fn probe_mode(
    client: &RpcClient,
    executable: &std::path::Path,
    context: &ThreadContext,
    external: bool,
) -> Result<VoiceEligibility, VoiceRejection> {
    if !external {
        host::helper_path(executable)?;
    }
    tokio::time::timeout(Duration::from_secs(15), probe_details(client, context))
        .await
        .map_err(|_| VoiceRejection::Protocol)?
}
async fn probe_details(
    client: &RpcClient,
    context: &ThreadContext,
) -> Result<VoiceEligibility, VoiceRejection> {
    if context.model_provider.as_deref() != Some("openai")
        || !std::path::Path::new(&context.cwd).is_absolute()
    {
        return Err(VoiceRejection::ChatgptRequired);
    }
    let account = client
        .request("account/read", json!({"refreshToken":false}))
        .await
        .map_err(|_| VoiceRejection::Protocol)?;
    if account["account"]["type"] != "chatgpt" {
        return Err(VoiceRejection::ChatgptRequired);
    }
    let config = client
        .request(
            "config/read",
            json!({"includeLayers":false,"cwd":context.cwd}),
        )
        .await
        .map_err(|_| VoiceRejection::Protocol)?;
    let config = &config["config"];
    if config["model_provider"]
        .as_str()
        .is_some_and(|p| p != "openai")
        || config["model_providers"]["openai"].is_object()
        || [
            "experimental_realtime_ws_base_url",
            "experimental_realtime_webrtc_call_base_url",
            "experimental_realtime_ws_model",
        ]
        .iter()
        .any(|k| !config[*k].is_null())
    {
        return Err(VoiceRejection::ChatgptRequired);
    }
    // No account/rateLimits/read: it is a ~1 s backend round trip, and ordinary
    // quota may be exhausted while permitted credits remain. The native backend
    // applies spend controls and the final usage decision.
    let v = client
        .request("thread/realtime/listVoices", json!({}))
        .await
        .map_err(|_| VoiceRejection::Unsupported)?;
    let voices = v["voices"]["v1"]
        .as_array()
        .ok_or(VoiceRejection::Unsupported)?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    Ok(VoiceEligibility {
        available: true,
        reason: None,
        ordinary_usage_allowed: None,
        credits_excluded: false,
        format: None,
        duplex_verified: true,
        native_webrtc: true,
        voices,
    })
}
async fn start(
    client: RpcClient,
    thread: String,
    executable: PathBuf,
    context: ThreadContext,
    voice: Option<String>,
    session_id: String,
    accepted: oneshot::Receiver<()>,
    answer: oneshot::Receiver<String>,
    abort: CancellationToken,
    server_live: Arc<std::sync::atomic::AtomicBool>,
) -> Result<host::NativeHost, VoiceRejection> {
    let eligible = probe(&client, &executable, &context).await?;
    if voice.as_ref().is_some_and(|v| !eligible.voices.contains(v)) {
        return Err(VoiceRejection::Unsupported);
    }
    let mut host = host::NativeHost::open(&host::helper_path(&executable)?, abort).await?;
    let offer = host.exchange(json!({"type":"startTransport"}), 20).await?;
    let sdp = offer["sdp"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 64 * 1024)
        .ok_or(VoiceRejection::Protocol)?;
    if offer["type"] != "offer" {
        return Err(VoiceRejection::Protocol);
    }
    let mut params = json!({"threadId":thread,"transport":{"type":"webrtc","sdp":sdp},"version":"v3","outputModality":"audio","realtimeSessionId":session_id,"clientManagedHandoffs":false,"includeStartupContext":true,
        "realtimeStartInstructions":ORCHESTRATOR_INSTRUCTIONS,
        "initialItems":[{"role":"developer","text":ORCHESTRATOR_INSTRUCTIONS}]});
    if let Some(voice) = voice {
        params["voice"] = json!(voice);
    }
    server_live.store(true, Ordering::Release);
    tokio::time::timeout(
        Duration::from_secs(30),
        client.request("thread/realtime/start", params),
    )
    .await
    .map_err(|_| VoiceRejection::Protocol)?
    .map_err(|_| VoiceRejection::Protocol)?;
    tokio::time::timeout(Duration::from_secs(30), accepted)
        .await
        .map_err(|_| VoiceRejection::Protocol)?
        .map_err(|_| VoiceRejection::Protocol)?;
    let answer = tokio::time::timeout(Duration::from_secs(30), answer)
        .await
        .map_err(|_| VoiceRejection::Protocol)?
        .map_err(|_| VoiceRejection::Protocol)?;
    host.expect(
        json!({"type":"applyAnswer","sdp":answer}),
        "transportReady",
        20,
    )
    .await?;
    host.open_devices()
        .await
        .map_err(|_| VoiceRejection::DeviceUnavailable)?;
    // Keep devices muted/suppressed until the engine confirms the owner stream.
    Ok(host)
}
async fn start_external(
    client: RpcClient,
    thread: String,
    context: ThreadContext,
    voice: Option<String>,
    session_id: String,
    offer: remote::Sdp,
    accepted: oneshot::Receiver<()>,
    answer: oneshot::Receiver<String>,
    server_live: Arc<std::sync::atomic::AtomicBool>,
) -> Result<remote::Sdp, VoiceRejection> {
    let eligible = tokio::time::timeout(Duration::from_secs(15), probe_details(&client, &context))
        .await
        .map_err(|_| VoiceRejection::Protocol)??;
    if voice.as_ref().is_some_and(|v| !eligible.voices.contains(v)) {
        return Err(VoiceRejection::Unsupported);
    }
    let mut params = json!({"threadId":thread,"transport":{"type":"webrtc","sdp":offer.expose()},"version":"v3","outputModality":"audio","realtimeSessionId":session_id,"clientManagedHandoffs":false,"includeStartupContext":true,
        "realtimeStartInstructions":ORCHESTRATOR_INSTRUCTIONS,"initialItems":[{"role":"developer","text":ORCHESTRATOR_INSTRUCTIONS}]});
    if let Some(voice) = voice {
        params["voice"] = json!(voice);
    }
    server_live.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(90), async {
        client
            .request("thread/realtime/start", params)
            .await
            .map_err(|_| VoiceRejection::Protocol)?;
        accepted.await.map_err(|_| VoiceRejection::Protocol)?;
        remote::Sdp::new(answer.await.map_err(|_| VoiceRejection::Protocol)?)
    })
    .await
    .map_err(|_| VoiceRejection::Protocol)?
}

async fn stop(client: &RpcClient, thread: &str) -> Result<(), VoiceRejection> {
    tokio::time::timeout(
        Duration::from_secs(2),
        client.request("thread/realtime/stop", json!({"threadId":thread})),
    )
    .await
    .map_err(|_| VoiceRejection::Protocol)?
    .map_err(|_| VoiceRejection::Protocol)?;
    Ok(())
}

/// Canonical completed items only. Legacy transcript/done has no stable item
/// identity and is deliberately not committed alongside the canonical stream.
pub(crate) fn normalize(
    thread: &str,
    generation: u64,
    sequence: &mut u64,
    method: &str,
    p: &Value,
) -> Option<VoiceEvent> {
    if p.get("threadId")?.as_str()? != thread {
        return None;
    }
    match method {
        "thread/realtime/item/completed" => {
            let item = p.get("item")?;
            if item.get("type")?.as_str()? != "transcriptSegment" {
                return None;
            }
            let text = item.get("text")?.as_str()?;
            if text.len() > MAX_TRANSCRIPT_BYTES {
                return None;
            }
            let role = match item.get("role")?.as_str()? {
                "user" => VoiceRole::User,
                "assistant" => VoiceRole::Assistant,
                _ => return None,
            };
            Some(VoiceEvent::Final {
                transcript: VoiceTranscript {
                    session_id: item.get("realtimeSessionId")?.as_str()?.to_owned(),
                    item_id: item.get("id")?.as_str()?.to_owned(),
                    role,
                    text: text.to_owned(),
                    promoted_message_id: None,
                },
            })
        }
        "thread/realtime/outputAudio/delta" => {
            let a = p.get("audio")?;
            let data = a.get("data")?.as_str()?;
            if data.len() > MAX_AUDIO_BYTES.div_ceil(3) * 4 {
                return None;
            }
            let format = VoiceFormat {
                encoding: VoiceEncoding::Pcm16Le,
                sample_rate: u32::try_from(a.get("sampleRate")?.as_u64()?).ok()?,
                channels: u16::try_from(a.get("numChannels")?.as_u64()?).ok()?,
            };
            if !format.validate(STANDARD.decode(data).ok()?.len()) {
                return None;
            }
            *sequence += 1;
            Some(VoiceEvent::Audio {
                frame: VoiceFrame {
                    generation,
                    sequence: *sequence,
                    format,
                    data: data.to_owned(),
                    item_id: a.get("itemId").and_then(Value::as_str).map(str::to_owned),
                },
            })
        }
        "thread/realtime/item/transcript/delta" => {
            let text = p.get("delta")?.as_str()?;
            (text.len() <= MAX_TRANSCRIPT_BYTES).then(|| VoiceEvent::Partial {
                generation,
                item_id: p.get("itemId").and_then(Value::as_str).map(str::to_owned),
                text: text.to_owned(),
            })
        }
        "thread/realtime/itemAdded"
            if matches!(
                p["item"]["type"].as_str(),
                Some("input_audio_buffer.speech_started" | "response.cancelled")
            ) =>
        {
            Some(VoiceEvent::InvalidatePlayout {
                generation,
                item_id: p["item"]["item_id"].as_str().unwrap_or_default().to_owned(),
            })
        }
        "thread/realtime/closed" | "thread/realtime/error" => Some(VoiceEvent::Closed {
            generation,
            reason: (method.ends_with("error")).then_some(VoiceRejection::Protocol),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_items_do_not_duplicate_legacy_or_foreign_threads() {
        let p = json!({"threadId":"parent", "item":{"type":"transcriptSegment", "id":"item", "realtimeSessionId":"voice", "role":"user", "text":"hello"}});
        let mut seq = 0;
        assert!(matches!(
            normalize("parent", 1, &mut seq, "thread/realtime/item/completed", &p),
            Some(VoiceEvent::Final { .. })
        ));
        assert!(normalize("other", 1, &mut seq, "thread/realtime/item/completed", &p).is_none());
        assert!(normalize("parent", 1, &mut seq, "thread/realtime/transcript/done", &p).is_none());
    }
}
