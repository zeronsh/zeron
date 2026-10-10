//! A terminal session outlives the panel presenting it. Streams and timers
//! address this entity, so moving a tab never reconnects or opens a second PTY.

use super::emulator::Emulator;
use super::panel::{backoff_ms, decode_base64, encode_base64, exit_message, with_target};
use super::view::{COALESCE_MS, InputCoalescer, RESIZE_DEBOUNCE_MS};
use crate::state::{AppState, EngineHandle};
use gpui::{App, Context, Entity, SharedString, Task};
use std::time::Duration;
use zeron_proto::{TerminalEvent, TerminalSession};
use zeron_rpc::methods;

pub(crate) struct TerminalSessionModel {
    state: Entity<AppState>,
    pub(crate) key: u64,
    chat: String,
    title: SharedString,
    pub(crate) terminal_id: Option<String>,
    pub(crate) target_device_id: Option<String>,
    pub(crate) emulator: Emulator,
    pub(super) scroll_remainder: f32,
    pub(crate) exited: Option<i32>,
    pub(crate) last_seq: u64,
    coalescer: InputCoalescer,
    flush_task: Option<Task<()>>,
    resize_task: Option<Task<()>>,
    _run: Option<Task<()>>,
    closed: bool,
}

impl TerminalSessionModel {
    pub(super) fn new(
        state: Entity<AppState>,
        chat: String,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state,
            key: cx.entity_id().as_u64(),
            chat,
            title,
            terminal_id: None,
            target_device_id: None,
            emulator: Emulator::new(80, 24),
            scroll_remainder: 0.0,
            exited: None,
            last_seq: 0,
            coalescer: InputCoalescer::default(),
            flush_task: None,
            resize_task: None,
            _run: None,
            closed: false,
        }
    }

    pub(super) fn display_title(&self) -> SharedString {
        match self.emulator.title().map(str::trim) {
            Some(title) if !title.is_empty() => title.to_string().into(),
            _ => self.title.clone(),
        }
    }

    fn engine(&self, cx: &App) -> Option<EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    pub(super) fn open(
        &mut self,
        engine: EngineHandle,
        target: Option<String>,
        cwd: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self._run = Some(Self::spawn_session(
            self.chat.clone(),
            engine,
            target,
            None,
            cwd,
            cx,
        ));
    }

    /// Project Actions retain a weak reference to this entity while their RPC
    /// runs. A moved placeholder still accepts the result; a closed one cannot.
    pub(crate) fn attach_reserved_session(
        &mut self,
        session: TerminalSession,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.closed || self._run.is_some() {
            return false;
        }
        let Some(engine) = self.engine(cx) else {
            return false;
        };
        self._run = Some(Self::spawn_session(
            self.chat.clone(),
            engine,
            target,
            Some(session),
            None,
            cx,
        ));
        true
    }

    pub(crate) fn fail_reserved_tab(&mut self, message: &str, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        self.emulator
            .feed(format!("\x1b[31mfailed to run action: {message}\x1b[0m\r\n").as_bytes());
        self.exited = Some(-1);
        cx.notify();
    }

    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.flush_task = None;
        self.resize_task = None;
        // An in-flight open must finish so it can release the returned PTY.
        // Its weak entity check below rejects closed/released models.
        if self.terminal_id.is_none() {
            if let Some(run) = self._run.take() {
                run.detach();
            }
        } else {
            self._run = None;
        }
        if let (Some(engine), Some(id)) = (self.engine(cx), self.terminal_id.clone()) {
            let target = self.target_device_id.clone();
            cx.spawn(async move |_, _| {
                let _ = engine
                    .client()
                    .call(
                        methods::CLOSE_TERMINAL,
                        with_target(serde_json::json!({ "terminalId": id }), &target),
                    )
                    .await;
            })
            .detach();
        }
        cx.notify();
    }

    /// OpenTerminal, then pump SubscribeTerminal with reconnect backoff.
    fn spawn_session(
        chat: String,
        engine: EngineHandle,
        target: Option<String>,
        existing_session: Option<TerminalSession>,
        cwd: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let (cols, rows) = this
                .update(cx, |model, _| (model.emulator.cols() as u16, model.emulator.rows() as u16))
                .unwrap_or((80, 24));

            let session = match existing_session {
                Some(session) => session,
                None => {
                    let mut params =
                        serde_json::json!({ "chatId": chat, "cols": cols, "rows": rows });
                    if let (Some(cwd), Some(object)) = (cwd, params.as_object_mut()) {
                        object.insert("cwd".into(), serde_json::Value::String(cwd));
                    }
                    match engine
                        .client()
                        .call_as::<TerminalSession>(
                            methods::OPEN_TERMINAL,
                            with_target(params, &target),
                        )
                        .await
                    {
                        Ok(session) => session,
                        Err(err) => {
                    tracing::warn!(error = %err, "OpenTerminal failed");
                    let _ = this.update(cx, |model, cx| {
                        if !model.closed {
                            model.emulator.feed(
                                format!("\x1b[31mfailed to open terminal: {err}\x1b[0m\r\n")
                                    .as_bytes(),
                            );
                            model.exited = Some(-1);
                            cx.notify();
                        }
                    });
                    return;
                        }
                    }
                }
            };
            let terminal_id = session.id.clone();
            let attached = this
                .update(cx, |model, cx| {
                    if !model.closed {
                        model.terminal_id = Some(terminal_id.clone());
                        model.target_device_id = target.clone();
                        // The destination may have resized while OpenTerminal
                        // was pending, after its initial dimensions were sent.
                        model.schedule_resize(cx);
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !attached {
                // Tab was closed before the open completed — release the PTY.
                let _ = engine
                    .client()
                    .call(
                        methods::CLOSE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": terminal_id }),
                            &target,
                        ),
                    )
                    .await;
                return;
            }

            let mut attempt: u32 = 0;
            loop {
                let Ok(after_seq) = this.update(cx, |model, _| {
                    (!model.closed).then_some(model.last_seq)
                }) else {
                    return; // entity released
                };
                let Some(after_seq) = after_seq else { return }; // tab closed

                let subscribed = engine
                    .client()
                    .subscribe(
                        methods::SUBSCRIBE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": terminal_id, "afterSeq": after_seq }),
                            &target,
                        ),
                    )
                    .await;
                let mut rx = match subscribed {
                    Ok(rx) => rx,
                    Err(err) => {
                        tracing::debug!(error = %err, attempt, "SubscribeTerminal failed; backing off");
                        cx.background_executor()
                            .timer(Duration::from_millis(backoff_ms(attempt)))
                            .await;
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
                };

                while let Some(value) = rx.recv().await {
                    let event: TerminalEvent = match serde_json::from_value(value) {
                        Ok(event) => event,
                        Err(err) => {
                            tracing::warn!(error = %err, "terminal: malformed stream frame");
                            continue;
                        }
                    };
                    attempt = 0;
                    let outcome = this.update(cx, |model, cx| {
                        model.apply_stream_event(&engine, event, cx)
                    });
                    match outcome {
                        Ok(StreamDisposition::Continue) => {}
                        Ok(StreamDisposition::Stop) => return,
                        Err(_) => return,
                    }
                }

                // Stream dropped without an exit — reconnect from afterSeq.
                let done = this
                    .update(cx, |model, _| {
                        model.closed || model.exited.is_some()
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(backoff_ms(attempt)))
                    .await;
                attempt = attempt.saturating_add(1);
            }
        })
    }

    fn apply_stream_event(
        &mut self,
        engine: &EngineHandle,
        event: TerminalEvent,
        cx: &mut Context<Self>,
    ) -> StreamDisposition {
        if self.closed {
            return StreamDisposition::Stop;
        }
        let target = self.target_device_id.clone();
        match event {
            TerminalEvent::Data { seq, data } => {
                self.last_seq = seq;
                let responses = self.emulator.feed(&decode_base64(&data));
                if !responses.is_empty()
                    && let Some(id) = self.terminal_id.clone()
                {
                    // Query responses (DSR etc.) go straight back, no coalescing.
                    let engine = engine.clone();
                    let data = encode_base64(&responses);
                    cx.spawn(async move |_, _| {
                        let _ = engine
                            .client()
                            .call(
                                methods::WRITE_TERMINAL,
                                with_target(
                                    serde_json::json!({ "terminalId": id, "data": data }),
                                    &target,
                                ),
                            )
                            .await;
                    })
                    .detach();
                }
                cx.notify();
                StreamDisposition::Continue
            }
            TerminalEvent::Exit { seq, exit_code, .. } => {
                self.last_seq = seq;
                self.exited = Some(exit_code);
                self.emulator.feed(&exit_message(exit_code));
                cx.notify();
                StreamDisposition::Stop
            }
        }
    }

    /// Queue keyboard bytes on the active tab (12 ms coalescing window).
    pub(super) fn queue_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if self.closed || self.exited.is_some() {
            return;
        }
        // A keypress while scrolled back snaps to the live bottom (xterm).
        if self.emulator.display_offset() > 0 {
            self.emulator.scroll_to_bottom();
        }
        if self.coalescer.push(bytes) {
            self.flush_task = Some(Self::schedule_flush(cx));
        }
    }

    fn schedule_flush(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(COALESCE_MS))
                .await;
            let _ = this.update(cx, |model, cx| model.flush_input(cx));
        })
    }

    fn flush_input(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if self.closed {
            return;
        }
        let target = self.target_device_id.clone();
        if self.coalescer.is_empty() {
            return;
        }
        let Some(id) = self.terminal_id.clone() else {
            // OpenTerminal still in flight — keep the buffer, retry shortly.
            if self.exited.is_none() {
                self.flush_task = Some(Self::schedule_flush(cx));
            }
            return;
        };
        let data = encode_base64(&self.coalescer.take());
        cx.spawn(async move |_, _| {
            let _ = engine
                .client()
                .call(
                    methods::WRITE_TERMINAL,
                    with_target(
                        serde_json::json!({ "terminalId": id, "data": data }),
                        &target,
                    ),
                )
                .await;
        })
        .detach();
    }

    /// Resize the emulator immediately; debounce only the host RPC. Timers
    /// follow the session, including an opening terminal moved to another view.
    pub(super) fn resize(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        if self.closed
            || (self.emulator.cols() == cols as usize && self.emulator.rows() == rows as usize)
        {
            return;
        }
        self.emulator.resize(cols, rows);
        self.schedule_resize(cx);
    }

    fn schedule_resize(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.resize_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(RESIZE_DEBOUNCE_MS))
                .await;
            let Ok(current) = this.update(cx, |model, _| {
                if model.closed {
                    return None;
                }
                Some((
                    model.terminal_id.clone()?,
                    model.target_device_id.clone(),
                    model.emulator.cols(),
                    model.emulator.rows(),
                ))
            }) else {
                return;
            };
            let Some((id, target, cols, rows)) = current else {
                return;
            };
            let _ = engine
                .client()
                .call(
                    methods::RESIZE_TERMINAL,
                    with_target(
                        serde_json::json!({ "terminalId": id, "cols": cols, "rows": rows }),
                        &target,
                    ),
                )
                .await;
        }));
    }
}

enum StreamDisposition {
    Continue,
    Stop,
}
