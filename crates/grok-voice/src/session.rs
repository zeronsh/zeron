//! One call: a stable session id, redials that remint, and no `Closed` event.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;
use zeron_proto::voice::{
    VoiceEvent, VoicePhase, VoiceRejection, VoiceRole, VoiceSnapshot, VoiceTranscript, VoiceWork,
};

use crate::audio::Duplex;
use crate::auth::{Credentials, Minted};
use crate::protocol::{
    FRAME_SAMPLES, ListedTool, SeedTurn, ServerEvent, Speech, append_audio, function_output,
    mentions_conversation, parse_server_event, response_cancel, response_create, seed_frames,
    session_update_message, snap_speed, truncate_chars,
};

const GENERATION: u64 = 1;
const TOOL_QUIET_MS: u32 = 80;
const BACKOFF_MS: [u64; 3] = [200, 600, 1_500];

#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub instructions: String,
    pub tools: Vec<ToolSpec>,
    pub chat_id: String,
    pub speech: Speech,
}

/// Live speech settings. A change is sent on the socket when no reply is open,
/// and after the current reply when one is.
#[derive(Clone, Debug, PartialEq)]
pub struct Tune {
    pub instructions: String,
    pub speech: Speech,
    pub input: Option<String>,
}

#[async_trait::async_trait]
pub trait ToolExec: Send + Sync {
    async fn call(&self, name: &str, arguments: serde_json::Value) -> String;
}

#[async_trait::async_trait]
impl<T> ToolExec for Arc<T>
where
    T: ToolExec + ?Sized,
{
    async fn call(&self, name: &str, arguments: serde_json::Value) -> String {
        self.as_ref().call(name, arguments).await
    }
}

#[async_trait::async_trait]
pub trait TranscriptSink: Send + Sync {
    async fn commit(&self, transcript: &VoiceTranscript);
}

#[async_trait::async_trait]
impl<T> TranscriptSink for Arc<T>
where
    T: TranscriptSink + ?Sized,
{
    async fn commit(&self, transcript: &VoiceTranscript) {
        self.as_ref().commit(transcript).await
    }
}

pub async fn run(
    credentials: impl Credentials + 'static,
    mut duplex: impl Duplex + 'static,
    tools: impl ToolExec + 'static,
    sink: impl TranscriptSink + 'static,
    call: Call,
    cancel: CancellationToken,
    muted: watch::Receiver<bool>,
    tune: watch::Receiver<Tune>,
    events: mpsc::Sender<VoiceEvent>,
) -> Result<(), VoiceRejection> {
    let tools: Arc<dyn ToolExec> = Arc::new(tools);
    let sink: Arc<dyn TranscriptSink> = Arc::new(sink);
    duplex.open()?;
    let duplex = Mutex::new(duplex);
    let result = drive(
        &duplex,
        credentials,
        tools,
        sink,
        call,
        cancel,
        muted,
        tune,
        events,
    )
    .await;
    let mut duplex = duplex.into_inner().unwrap_or_else(|p| p.into_inner());
    duplex.close();
    result
}

enum Wait {
    Cancel,
    Device(VoiceRejection),
}

enum Retry {
    Cancel,
    Device(VoiceRejection),
    Exhausted,
}

enum Stop {
    Cancel,
    Fail(VoiceRejection),
    Redial {
        resume_failed: bool,
        established: bool,
        conversation_id: Option<String>,
    },
}

enum Step {
    Continue,
    Redial { resume_failed: bool },
    Fail(VoiceRejection),
}

enum Outgoing {
    Text(String),
    Pong(Vec<u8>),
}

struct AssistantTurn {
    item_id: String,
    text: String,
}

async fn drive<D: Duplex>(
    duplex: &Mutex<D>,
    credentials: impl Credentials,
    tools: Arc<dyn ToolExec>,
    sink: Arc<dyn TranscriptSink>,
    call: Call,
    cancel: CancellationToken,
    muted: watch::Receiver<bool>,
    tune: watch::Receiver<Tune>,
    events: mpsc::Sender<VoiceEvent>,
) -> Result<(), VoiceRejection> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut conversation_id = None;
    let mut history = VecDeque::new();
    let mut redials = 0usize;
    let mut resume_used = false;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let minted = match with_mic(duplex, &muted, &cancel, credentials.mint()).await {
            Err(Wait::Cancel) => return Ok(()),
            Err(Wait::Device(err)) => return Err(err),
            Ok(Err(VoiceRejection::GrokSignInRequired)) => {
                return Err(VoiceRejection::GrokSignInRequired);
            }
            Ok(Err(VoiceRejection::Protocol)) => {
                if let Err(err) = backoff(duplex, &muted, &cancel, &mut redials, false).await {
                    return retry_end(err);
                }
                continue;
            }
            Ok(Err(other)) => return Err(other),
            Ok(Ok(minted)) => minted,
        };
        let snapshot: Vec<SeedTurn> = history.iter().cloned().collect();
        let seeds = if conversation_id.is_none() {
            seed_frames(&snapshot)
                .into_iter()
                .map(|frame| frame.to_string())
                .collect()
        } else {
            Vec::new()
        };
        let socket = match with_mic(
            duplex,
            &muted,
            &cancel,
            connect(&minted, conversation_id.as_deref()),
        )
        .await
        {
            Err(Wait::Cancel) => return Ok(()),
            Err(Wait::Device(err)) => return Err(err),
            Ok(Err(())) => {
                tracing::debug!("grok voice connect failed");
                if let Err(err) = backoff(duplex, &muted, &cancel, &mut redials, false).await {
                    return retry_end(err);
                }
                continue;
            }
            Ok(Ok(socket)) => socket,
        };
        match talk(
            socket,
            duplex,
            &tools,
            &sink,
            &call,
            &cancel,
            &muted,
            &tune,
            &events,
            &session_id,
            &mut conversation_id,
            &mut history,
            &seeds,
            resume_used,
        )
        .await
        {
            Stop::Cancel => return Ok(()),
            Stop::Fail(err) => return Err(err),
            Stop::Redial {
                resume_failed,
                established,
                conversation_id: next,
            } => {
                if resume_failed {
                    if resume_used {
                        return Err(VoiceRejection::Protocol);
                    }
                    resume_used = true;
                    conversation_id = None;
                } else {
                    conversation_id = next;
                }
                tracing::debug!(established, resume_failed, "grok voice redial");
                if let Err(err) = backoff(duplex, &muted, &cancel, &mut redials, established).await
                {
                    return retry_end(err);
                }
            }
        }
    }
}

fn retry_end(err: Retry) -> Result<(), VoiceRejection> {
    match err {
        Retry::Cancel => Ok(()),
        Retry::Device(err) => Err(err),
        Retry::Exhausted => Err(VoiceRejection::Protocol),
    }
}

async fn backoff<D: Duplex>(
    duplex: &Mutex<D>,
    muted: &watch::Receiver<bool>,
    cancel: &CancellationToken,
    redials: &mut usize,
    established: bool,
) -> Result<(), Retry> {
    if established {
        *redials = 0;
    }
    if *redials >= BACKOFF_MS.len() {
        return Err(Retry::Exhausted);
    }
    let delay = BACKOFF_MS[*redials];
    *redials += 1;
    match with_mic(
        duplex,
        muted,
        cancel,
        tokio::time::sleep(Duration::from_millis(delay)),
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(Wait::Cancel) => Err(Retry::Cancel),
        Err(Wait::Device(err)) => Err(Retry::Device(err)),
    }
}

/// Keep the capture ring empty while a mint, connect, or backoff is in flight.
async fn with_mic<D, T>(
    duplex: &Mutex<D>,
    muted: &watch::Receiver<bool>,
    cancel: &CancellationToken,
    fut: impl Future<Output = T>,
) -> Result<T, Wait>
where
    D: Duplex,
{
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Err(Wait::Cancel),
            result = &mut fut => return Ok(result),
            _ = tick.tick() => {
                if let Err(err) = drain_mic(duplex, muted) {
                    return Err(Wait::Device(err));
                }
            }
        }
    }
}

fn lock_duplex<D>(duplex: &Mutex<D>) -> MutexGuard<'_, D> {
    duplex.lock().unwrap_or_else(|p| p.into_inner())
}

fn drain_mic<D: Duplex>(
    duplex: &Mutex<D>,
    muted: &watch::Receiver<bool>,
) -> Result<(), VoiceRejection> {
    let mut duplex = lock_duplex(duplex);
    duplex.set_muted(*muted.borrow());
    let mut dropped = Vec::new();
    duplex.pull_mic(&mut dropped)
}

async fn connect(
    minted: &Minted,
    conversation_id: Option<&str>,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, ()> {
    let url = realtime_url(&minted.websocket_url, &minted.model, conversation_id)?;
    let mut request = url.as_str().into_client_request().map_err(|_| ())?;
    let protocol = format!("xai-client-secret.{}", minted.client_secret);
    let header =
        tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&protocol).map_err(|_| ())?;
    request
        .headers_mut()
        .insert("sec-websocket-protocol", header);
    let (socket, _response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|_| ())?;
    Ok(socket)
}

fn realtime_url(base: &str, model: &str, conversation_id: Option<&str>) -> Result<url::Url, ()> {
    let mut url = url::Url::parse(base).map_err(|_| ())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("model", model);
        if let Some(id) = conversation_id {
            query.append_pair("conversation_id", id);
        }
    }
    Ok(url)
}

struct Ctx<'a, D: Duplex> {
    duplex: &'a Mutex<D>,
    muted: &'a watch::Receiver<bool>,
    events: &'a mpsc::Sender<VoiceEvent>,
    sink: &'a Arc<dyn TranscriptSink>,
    tools: &'a Arc<dyn ToolExec>,
    out_tx: &'a mpsc::UnboundedSender<Outgoing>,
    tool_tx: mpsc::UnboundedSender<(String, String)>,
    session_id: &'a str,
    chat_id: &'a str,
    conversation_id: &'a mut Option<String>,
    established: &'a mut bool,
    assistant: &'a mut Option<AssistantTurn>,
    history: &'a mut VecDeque<SeedTurn>,
    seeds: &'a [String],
    seeds_sent: &'a mut bool,
    resume_used: bool,
    tool_queue: VecDeque<(String, String)>,
    pending_pcm: Vec<f32>,
    /// A model response is open until `response.done` or `response.cancelled`.
    response_open: bool,
    /// Server VAD heard the caller. Mic audio is sent either way.
    caller_speaking: bool,
    /// The caller cut into this response. Its later audio is not played.
    interrupted: bool,
    tune: &'a watch::Receiver<Tune>,
    specs: &'a [ToolSpec],
    /// Last speech settings sent on this socket, plus the mic id last requested.
    applied: Tune,
}

async fn talk<D: Duplex>(
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    duplex: &Mutex<D>,
    tools: &Arc<dyn ToolExec>,
    sink: &Arc<dyn TranscriptSink>,
    call: &Call,
    cancel: &CancellationToken,
    muted: &watch::Receiver<bool>,
    tune: &watch::Receiver<Tune>,
    events: &mpsc::Sender<VoiceEvent>,
    session_id: &str,
    conversation_id: &mut Option<String>,
    history: &mut VecDeque<SeedTurn>,
    seeds: &[String],
    resume_used: bool,
) -> Stop {
    let (mut write, mut read) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let (tool_tx, mut tool_rx) = mpsc::unbounded_channel();
    let desired = tune.borrow().clone();
    let speech = Speech {
        voice: desired.speech.voice.clone(),
        speed: snap_speed(desired.speech.speed),
    };
    let listed: Vec<ListedTool<'_>> = call
        .tools
        .iter()
        .map(|tool| ListedTool {
            name: &tool.name,
            description: &tool.description,
            parameters: &tool.parameters,
        })
        .collect();
    let update = session_update_message(&desired.instructions, &listed, &speech);
    if out_tx.send(Outgoing::Text(update.to_string())).is_err() {
        return redial(false, conversation_id, false);
    }
    let _ = lock_duplex(duplex).set_input(desired.input.as_deref());
    let applied = Tune {
        instructions: desired.instructions,
        speech,
        input: desired.input,
    };
    let mut established = false;
    let mut assistant = None;
    let mut seeds_sent = false;
    let mut ctx = Ctx {
        duplex,
        muted,
        events,
        sink,
        tools,
        out_tx: &out_tx,
        tool_tx,
        session_id,
        chat_id: &call.chat_id,
        conversation_id,
        established: &mut established,
        assistant: &mut assistant,
        history,
        seeds,
        seeds_sent: &mut seeds_sent,
        resume_used,
        tool_queue: VecDeque::new(),
        pending_pcm: Vec::new(),
        response_open: false,
        caller_speaking: false,
        interrupted: false,
        tune,
        specs: &call.tools,
        applied,
    };
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Stop::Cancel,
            outgoing = out_rx.recv() => {
                let Some(outgoing) = outgoing else {
                    return redial(false, ctx.conversation_id, *ctx.established);
                };
                let message = match outgoing {
                    Outgoing::Text(text) => Message::Text(text),
                    Outgoing::Pong(payload) => Message::Pong(payload),
                };
                if write.send(message).await.is_err() {
                    return redial(false, ctx.conversation_id, *ctx.established);
                }
            }
            incoming = read.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(_)) | None => {
                        return redial(false, ctx.conversation_id, *ctx.established);
                    }
                };
                match message {
                    Message::Ping(payload) => {
                        if ctx.out_tx.send(Outgoing::Pong(payload)).is_err() {
                            return redial(false, ctx.conversation_id, *ctx.established);
                        }
                    }
                    Message::Close(_) => {
                        return redial(false, ctx.conversation_id, *ctx.established);
                    }
                    Message::Text(text) => match ctx.handle_text(&text).await {
                        Step::Continue => {}
                        Step::Fail(err) => return Stop::Fail(err),
                        Step::Redial { resume_failed } => {
                            return redial(resume_failed, ctx.conversation_id, *ctx.established);
                        }
                    },
                    _ => {}
                }
            }
            tool = tool_rx.recv() => {
                if let Some(done) = tool {
                    ctx.tool_queue.push_back(done);
                    if let Step::Redial { resume_failed } = ctx.flush_tools() {
                        return redial(resume_failed, ctx.conversation_id, *ctx.established);
                    }
                }
            }
            _ = tick.tick() => match ctx.on_tick() {
                Step::Continue => {}
                Step::Fail(err) => return Stop::Fail(err),
                Step::Redial { resume_failed } => {
                    return redial(resume_failed, ctx.conversation_id, *ctx.established);
                }
            },
        }
    }
}

fn style_differs(applied: &Tune, desired: &Tune) -> bool {
    applied.instructions != desired.instructions
        || snap_speed(applied.speech.speed) != snap_speed(desired.speech.speed)
        || voice_token(&applied.speech.voice) != voice_token(&desired.speech.voice)
}

fn voice_token(voice: &Option<String>) -> Option<String> {
    voice
        .as_deref()
        .map(str::trim)
        .filter(|voice| !voice.is_empty())
        .map(str::to_owned)
}

fn redial(resume_failed: bool, conversation_id: &Option<String>, established: bool) -> Stop {
    Stop::Redial {
        resume_failed,
        established,
        conversation_id: conversation_id.clone(),
    }
}

impl<D: Duplex> Ctx<'_, D> {
    fn on_tick(&mut self) -> Step {
        let tick = match self.capture_tick() {
            Ok(tick) => tick,
            Err(err) => return Step::Fail(err),
        };
        let _ = self.events.try_send(VoiceEvent::Levels {
            generation: GENERATION,
            microphone: tick.microphone,
            speaker: tick.speaker,
        });
        for frame in tick.frames {
            if self.out_tx.send(Outgoing::Text(frame)).is_err() {
                return Step::Redial {
                    resume_failed: false,
                };
            }
        }
        let tools = self.flush_tools();
        if !matches!(tools, Step::Continue) {
            return tools;
        }
        self.apply_tune()
    }

    /// Send speech changes now when the model is idle, or keep them until the
    /// open reply finishes. The microphone switches immediately.
    fn apply_tune(&mut self) -> Step {
        let desired = self.tune.borrow().clone();
        if desired.input != self.applied.input {
            if let Err(err) = lock_duplex(self.duplex).set_input(desired.input.as_deref()) {
                return Step::Fail(err);
            }
            self.applied.input = desired.input.clone();
        }
        if !style_differs(&self.applied, &desired) {
            return Step::Continue;
        }
        if self.response_open {
            return Step::Continue;
        }
        let listed: Vec<ListedTool<'_>> = self
            .specs
            .iter()
            .map(|tool| ListedTool {
                name: &tool.name,
                description: &tool.description,
                parameters: &tool.parameters,
            })
            .collect();
        let speech = Speech {
            voice: desired.speech.voice.clone(),
            speed: snap_speed(desired.speech.speed),
        };
        let update = session_update_message(&desired.instructions, &listed, &speech);
        if self
            .out_tx
            .send(Outgoing::Text(update.to_string()))
            .is_err()
        {
            return Step::Redial {
                resume_failed: false,
            };
        }
        self.applied.instructions = desired.instructions;
        self.applied.speech = speech;
        Step::Continue
    }

    fn capture_tick(&mut self) -> Result<MicTick, VoiceRejection> {
        let is_muted = *self.muted.borrow();
        let mut pulled = Vec::new();
        let discard = {
            let mut duplex = lock_duplex(self.duplex);
            duplex.set_muted(is_muted);
            if let Err(err) = duplex.pull_mic(&mut pulled) {
                return Err(err);
            }
            is_muted
        };
        // Mute drops the samples. Playback does not: the server has to hear an
        // interrupt while the reply is still coming out of the speaker.
        if discard {
            self.pending_pcm.clear();
        } else {
            self.pending_pcm.append(&mut pulled);
        }
        let mut frames = Vec::new();
        if !discard {
            while self.pending_pcm.len() >= FRAME_SAMPLES {
                let frame: Vec<f32> = self.pending_pcm.drain(..FRAME_SAMPLES).collect();
                frames.push(append_audio(&frame));
            }
        }
        let (mic, speaker) = lock_duplex(self.duplex).levels();
        Ok(MicTick {
            frames,
            microphone: if discard { 0 } else { mic },
            speaker,
        })
    }

    fn flush_tools(&mut self) -> Step {
        loop {
            // Drop the duplex lock before `tool_queue` is mutated.
            let quiet = lock_duplex(self.duplex).speaker_queued_ms() < TOOL_QUIET_MS;
            if !quiet {
                break;
            }
            let Some((call_id, output)) = self.tool_queue.pop_front() else {
                break;
            };
            let output_msg = function_output(&call_id, &output);
            if self.out_tx.send(Outgoing::Text(output_msg)).is_err()
                || self.out_tx.send(Outgoing::Text(response_create())).is_err()
            {
                return Step::Redial {
                    resume_failed: false,
                };
            }
        }
        Step::Continue
    }

    async fn handle_text(&mut self, text: &str) -> Step {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
            return Step::Continue;
        };
        match parse_server_event(&value) {
            ServerEvent::Ignore => Step::Continue,
            ServerEvent::SessionReady => self.session_ready().await,
            ServerEvent::Audio(pcm) => {
                if self.caller_speaking || self.interrupted {
                    return Step::Continue;
                }
                self.response_open = true;
                lock_duplex(self.duplex).push_speaker(&pcm);
                Step::Continue
            }
            ServerEvent::SpeechStarted => self.on_speech_started(),
            ServerEvent::SpeechStopped => {
                self.caller_speaking = false;
                Step::Continue
            }
            ServerEvent::ResponseStarted => {
                self.response_open = true;
                // The next response can play once the caller is quiet.
                self.interrupted = false;
                Step::Continue
            }
            ServerEvent::ResponseFinished => {
                self.response_open = false;
                self.interrupted = false;
                self.apply_tune()
            }
            ServerEvent::TranscriptDelta { item_id, delta } => {
                let (id, text) = push_delta(self.assistant, item_id, &delta);
                // A full channel must not stall the mic drain. The final line
                // still commits the whole turn.
                match self.events.try_send(VoiceEvent::Partial {
                    generation: GENERATION,
                    item_id: Some(id),
                    text,
                }) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Step::Continue,
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        Step::Fail(VoiceRejection::Protocol)
                    }
                }
            }
            ServerEvent::TranscriptDone {
                item_id,
                transcript,
            } => {
                let Some((id, text)) = finish_turn(self.assistant, item_id, transcript) else {
                    return Step::Continue;
                };
                self.emit_final(VoiceRole::Assistant, id, text).await
            }
            ServerEvent::UserTranscript {
                item_id,
                transcript,
            } => {
                if transcript.is_empty() {
                    return Step::Continue;
                }
                let id = item_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                self.emit_final(VoiceRole::User, id, transcript).await
            }
            ServerEvent::Conversation(id) => {
                *self.conversation_id = Some(id);
                Step::Continue
            }
            ServerEvent::Function {
                name,
                call_id,
                arguments,
            } => {
                let tools = Arc::clone(self.tools);
                let tx = self.tool_tx.clone();
                tokio::spawn(async move {
                    let output = truncate_chars(&tools.call(&name, arguments).await, 8_000);
                    let _ = tx.send((call_id, output));
                });
                Step::Continue
            }
            ServerEvent::Error(message) => self.on_error(&message),
        }
    }

    fn on_speech_started(&mut self) -> Step {
        lock_duplex(self.duplex).clear_speaker();
        self.caller_speaking = true;
        if !self.response_open {
            return Step::Continue;
        }
        // Stop generation. Leftover deltas of this response stay dropped until
        // it finishes or the next one starts. Cancelling when nothing is open
        // makes the server answer with an error.
        self.response_open = false;
        self.interrupted = true;
        if self.out_tx.send(Outgoing::Text(response_cancel())).is_err() {
            return Step::Redial {
                resume_failed: false,
            };
        }
        Step::Continue
    }

    async fn session_ready(&mut self) -> Step {
        *self.established = true;
        let muted_now = *self.muted.borrow();
        let playing = lock_duplex(self.duplex).speaker_queued_ms() > 0;
        if let Step::Fail(err) = self
            .emit(snapshot(self.session_id, self.chat_id, muted_now, playing))
            .await
        {
            return Step::Fail(err);
        }
        if !*self.seeds_sent {
            *self.seeds_sent = true;
            for frame in self.seeds {
                if self.out_tx.send(Outgoing::Text(frame.clone())).is_err() {
                    return Step::Redial {
                        resume_failed: false,
                    };
                }
            }
        }
        Step::Continue
    }

    fn on_error(&mut self, message: &str) -> Step {
        if mentions_conversation(message) && self.conversation_id.is_some() {
            if self.resume_used {
                if !*self.established {
                    return Step::Fail(VoiceRejection::Protocol);
                }
                return Step::Continue;
            }
            *self.conversation_id = None;
            return Step::Redial {
                resume_failed: true,
            };
        }
        if !*self.established {
            return Step::Fail(VoiceRejection::Protocol);
        }
        Step::Continue
    }

    async fn emit_final(&mut self, role: VoiceRole, item_id: String, text: String) -> Step {
        let transcript = VoiceTranscript {
            session_id: self.session_id.to_string(),
            item_id,
            role,
            text: text.clone(),
            promoted_message_id: None,
        };
        if let Step::Fail(err) = self
            .emit(VoiceEvent::Final {
                transcript: transcript.clone(),
            })
            .await
        {
            return Step::Fail(err);
        }
        self.sink.commit(&transcript).await;
        remember(self.history, SeedTurn { role, text });
        Step::Continue
    }

    async fn emit(&self, event: VoiceEvent) -> Step {
        match self.events.send(event).await {
            Ok(()) => Step::Continue,
            Err(_) => Step::Fail(VoiceRejection::Protocol),
        }
    }
}

struct MicTick {
    frames: Vec<String>,
    microphone: u16,
    speaker: u16,
}

fn snapshot(session_id: &str, chat_id: &str, muted: bool, playing: bool) -> VoiceEvent {
    VoiceEvent::Snapshot {
        snapshot: VoiceSnapshot {
            session_id: session_id.to_string(),
            chat_id: chat_id.to_string(),
            generation: GENERATION,
            phase: VoicePhase::Active,
            muted,
            playing,
            work: VoiceWork::Idle,
            reason: None,
            voice: None,
            voices: Vec::new(),
        },
    }
}

fn push_delta(
    turn: &mut Option<AssistantTurn>,
    hinted: Option<String>,
    delta: &str,
) -> (String, String) {
    let start_new = match (turn.as_ref(), hinted.as_deref()) {
        (None, _) => true,
        (Some(current), Some(id)) => current.item_id != id,
        (Some(_), None) => false,
    };
    if start_new {
        *turn = Some(AssistantTurn {
            item_id: hinted.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            text: String::new(),
        });
    }
    let turn = turn.as_mut().expect("assistant turn");
    turn.text.push_str(delta);
    (turn.item_id.clone(), delta.to_string())
}

fn finish_turn(
    turn: &mut Option<AssistantTurn>,
    hinted: Option<String>,
    transcript: Option<String>,
) -> Option<(String, String)> {
    let current = turn.take();
    let item_id = hinted
        .or_else(|| current.as_ref().map(|turn| turn.item_id.clone()))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let text = match transcript {
        Some(text) => text,
        None => current.map(|turn| turn.text).unwrap_or_default(),
    };
    if text.is_empty() {
        None
    } else {
        Some((item_id, text))
    }
}

fn remember(history: &mut VecDeque<SeedTurn>, turn: SeedTurn) {
    if turn.text.is_empty() {
        return;
    }
    history.push_back(turn);
    while history.len() > 24 {
        history.pop_front();
    }
}
