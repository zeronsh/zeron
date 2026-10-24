//! A terminal session outlives the panel presenting it. Streams and timers
//! address this entity, so moving a tab never reconnects or opens a second PTY.

use super::emulator::Emulator;
use super::panel::{backoff_ms, decode_base64, encode_base64, exit_message, with_target};
use super::predict::Predictor;
use super::view::{InputQueue, RESIZE_DEBOUNCE_MS};
use crate::state::{AppState, EngineHandle};
use gpui::{App, Context, Entity, SharedString, Task};
use std::time::{Duration, Instant};
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
    /// Fraction of a scrollback row still visible below the live grid.
    pub(super) scroll_remainder: f32,
    pub(crate) exited: Option<i32>,
    pub(crate) last_seq: u64,
    /// Fractional wheel rows still owed to a program's mouse/arrow reports.
    pub(super) wheel_remainder: f32,
    pub(super) input: InputQueue,
    /// Remote echo and its deadlines move with the session, not the view.
    pub(super) predictor: Predictor,
    predict_task: Option<Task<()>>,
    /// Flush a synchronized update if the program leaves it open.
    sync_task: Option<Task<()>>,
    pub(super) marked_text: Option<String>,
    /// Hosts answering terminal queries suppress the emulator's duplicates.
    host_replies: bool,
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
            wheel_remainder: 0.0,
            input: InputQueue::default(),
            predictor: Predictor::default(),
            predict_task: None,
            sync_task: None,
            marked_text: None,
            host_replies: false,
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
        self.predict_task = None;
        self.sync_task = None;
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

            let opened_elsewhere = existing_session.is_some();
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
                    if model.closed {
                        return None;
                    }
                    model.terminal_id = Some(terminal_id.clone());
                    model.target_device_id = target.clone();
                    model.host_replies = session.host_replies;
                    let size = (model.emulator.cols() as u16, model.emulator.rows() as u16);
                    model.pump_input(cx);
                    cx.notify();
                    Some(size)
                })
                .ok()
                .flatten();
            // Opening (or moving an opening tab) may outlast the resize
            // debounce. Apply the destination's measured size to the host.
            if let Some((now_cols, now_rows)) = attached
                && (opened_elsewhere || (now_cols, now_rows) != (cols, rows))
            {
                let _ = engine
                    .client()
                    .call(
                        methods::RESIZE_TERMINAL,
                        with_target(
                            serde_json::json!({
                                "terminalId": terminal_id,
                                "cols": now_cols,
                                "rows": now_rows,
                            }),
                            &target,
                        ),
                    )
                    .await;
            }
            if attached.is_none() {
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
                let Ok(resume) = this.update(cx, |model, _| {
                    (!model.closed).then_some((model.last_seq, model.host_replies))
                }) else {
                    return; // entity released
                };
                let Some((after_seq, host_replies)) = resume else { return }; // tab closed

                let subscribed = engine
                    .client()
                    .subscribe(
                        methods::SUBSCRIBE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": terminal_id, "afterSeq": after_seq, "hostReplies": host_replies }),
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
                        model.apply_stream_event(event, cx)
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
        event: TerminalEvent,
        cx: &mut Context<Self>,
    ) -> StreamDisposition {
        if self.closed {
            return StreamDisposition::Stop;
        }
        match event {
            TerminalEvent::Data { seq, data } => {
                self.last_seq = seq;
                let responses = self.emulator.feed(&decode_base64(&data));
                if self.emulator.alt_screen() {
                    self.predictor.reset();
                }
                self.predictor.output(&self.emulator, Instant::now());
                let sync_deadline = self.emulator.sync_deadline();
                if !responses.is_empty() && !self.host_replies {
                    // Query responses (DSR etc.) share the key queue, so they
                    // reach the PTY in order with what was typed. Hosts that
                    // answer queries themselves get none from here.
                    self.input.push(&responses);
                    self.pump_input(cx);
                }
                if let Some(deadline) = sync_deadline {
                    self.schedule_sync_flush(deadline, cx);
                }
                cx.notify();
                StreamDisposition::Continue
            }
            TerminalEvent::Exit { seq, exit_code, .. } => {
                self.last_seq = seq;
                self.exited = Some(exit_code);
                self.predictor.reset();
                self.marked_text = None;
                self.emulator.feed(&exit_message(exit_code));
                cx.notify();
                StreamDisposition::Stop
            }
        }
    }

    /// Keyboard writes begin immediately and remain ordered across moves.
    pub(super) fn queue_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if self.closed || self.exited.is_some() {
            return;
        }
        if self.emulator.display_offset() > 0 || self.scroll_remainder != 0.0 {
            self.emulator.scroll_to_bottom();
            self.scroll_remainder = 0.0;
        }
        self.input.push(bytes);
        self.pump_input(cx);
    }

    pub(super) fn send_report(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        self.input.push(bytes);
        self.pump_input(cx);
    }

    fn pump_input(&mut self, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        // OpenTerminal still in flight: the attach pumps once it lands.
        let Some(id) = self.terminal_id.clone() else {
            return;
        };
        let Some(bytes) = self.input.start() else {
            return;
        };
        let target = self.target_device_id.clone();
        if target.is_some() {
            self.predictor.sent(Instant::now());
        }
        let data = encode_base64(&bytes);
        cx.spawn(async move |this, cx| {
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
            let _ = this.update(cx, |model, cx| {
                model.input.finish();
                model.pump_input(cx);
            });
        })
        .detach();
    }

    pub(super) fn schedule_prediction_expiry(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.predictor.next_deadline() else {
            return;
        };
        self.predict_task = Some(cx.spawn(async move |this, cx| {
            let wait = deadline.saturating_duration_since(Instant::now());
            cx.background_executor()
                .timer(wait + Duration::from_millis(1))
                .await;
            let _ = this.update(cx, |model, cx| {
                if !model.closed && model.predictor.expire(Instant::now()) {
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_sync_flush(&mut self, deadline: Instant, cx: &mut Context<Self>) {
        self.sync_task = Some(cx.spawn(async move |this, cx| {
            let wait = deadline.saturating_duration_since(Instant::now());
            cx.background_executor()
                .timer(wait + Duration::from_millis(1))
                .await;
            let _ = this.update(cx, |model, cx| {
                if model.closed {
                    return;
                }
                if model
                    .emulator
                    .sync_deadline()
                    .is_some_and(|due| due <= Instant::now())
                {
                    let responses = model.emulator.finish_sync();
                    if !responses.is_empty() && !model.host_replies {
                        model.input.push(&responses);
                        model.pump_input(cx);
                    }
                    cx.notify();
                }
            });
        }));
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
        self.predictor.reset();
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
