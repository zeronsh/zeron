use super::*;
use zeron_proto::{HarnessId, NativeForkAvailability, NativeForkDestination};

pub(super) struct ForkMenu {
    chat: String,
    message: String,
    active: usize,
    return_focus: Option<gpui::FocusHandle>,
}

pub(super) fn eligible(entry: &SessionMessageEntry, harness: HarnessId, subagent: bool) -> bool {
    !subagent
        && zeron_proto::native_fork_provider(harness)
        && entry.role == MessageRole::Assistant
        && entry.status == Some(MessageStatus::Complete)
}

impl Transcript {
    /// Availability is resolved on state changes, never while rendering a row.
    pub(super) fn refresh_native_forks(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(chat) = state
            .selected_chat_row()
            .filter(|_| self.doc_override.is_none())
            .cloned()
        else {
            self.native_forks.clear();
            self.native_fork_key.clear();
            self.native_fork_availability_task = None;
            return;
        };
        let harness = chat.config.as_ref().map(|c| c.harness).or_else(|| {
            state
                .transcript
                .iter()
                .find_map(|e| e.native_fork_point.as_ref().map(|p| p.harness))
        });
        let Some(harness) = harness else {
            self.native_forks.clear();
            self.native_fork_key.clear();
            self.native_fork_availability_task = None;
            return;
        };
        let supported = state.device_supports(
            &chat.device_id,
            zeron_proto::capabilities::NATIVE_MESSAGE_FORK_V1,
        );
        let entries: Vec<_> = state
            .transcript
            .iter()
            .filter(|e| eligible(e, harness, false))
            .collect();
        let points: Vec<_> = entries
            .iter()
            .map(|e| (&e.id, &e.native_fork_point))
            .collect();
        let versions: Vec<_> = state
            .harness_updates
            .iter()
            .filter(|s| s.harness == harness)
            .map(|s| (&s.installed_version, &s.phase))
            .collect();
        // Remote delivery can recover while the viewer's local engine remains
        // Ready. Track only this host/chat's health, not heartbeat timestamps,
        // pending update counts, or connectivity changes in unrelated chats.
        let remote_health = (state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
            .then(|| {
                (
                    state.connectivity.state,
                    state.device_online(&chat.device_id, chrono::Utc::now()),
                    state
                        .connectivity
                        .chats
                        .iter()
                        .find(|net| net.chat_id == chat.id)
                        .map(|net| (net.sync_state, net.connected, net.delivery_live)),
                )
            });
        let key = serde_json::to_string(&(
            &chat.id,
            &chat.device_id,
            harness,
            supported,
            state.device_supports(
                &chat.device_id,
                zeron_proto::capabilities::NATIVE_MESSAGE_FORK_MAIN_V1,
            ),
            format!("{:?}", state.connection),
            remote_health,
            points,
            versions,
        ))
        .unwrap_or_default();
        if self.native_fork_key == key {
            return;
        }
        self.native_fork_key = key.clone();
        self.native_fork_menu = Default::default();
        // Dropping the previous task cancels stale requests and retry timers,
        // including when navigation returns to a previously used cache key.
        self.native_fork_availability_task = None;
        self.native_forks.clear();
        let mut ids = Vec::new();
        for entry in entries {
            if !supported || entry.native_fork_point.is_none() {
                continue;
            }
            ids.push(entry.id.clone());
            self.native_forks.insert(
                entry.id.clone(),
                NativeForkAvailability::unavailable("Checking native fork availability…"),
            );
        }
        let engine = state.engine().cloned();
        cx.notify();
        if !supported || ids.is_empty() {
            return;
        }
        let Some(engine) = engine else {
            return;
        };
        self.native_fork_availability_task = Some(cx.spawn(async move |this, cx| {
            let mut delay = Duration::from_secs(1);
            loop {
                let mut retry = Vec::new();
                for chunk in ids.chunks(512) {
                    let result = engine.client().call_as::<HashMap<String, NativeForkAvailability>>(
                        zeron_rpc::methods::GET_NATIVE_FORK_AVAILABILITY,
                        serde_json::json!({"sourceChatId":chat.id,"targetDeviceId":chat.device_id,"messageIds":chunk}),
                    ).await;
                    if result.is_err() {
                        retry.extend_from_slice(chunk);
                    }
                    let current = this.update(cx, |this, cx| {
                        if this.native_fork_key != key { return false; }
                        match result {
                            Ok(values) => this.native_forks.extend(values),
                            Err(error) => for id in chunk {
                                this.native_forks.insert(id.clone(), NativeForkAvailability::unavailable(error.to_string()));
                            }
                        }
                        cx.notify();
                        true
                    });
                    if !matches!(current, Ok(true)) {
                        return;
                    }
                }
                // Retry failed batches only. A successful availability reply,
                // including an unsupported boundary, remains cached normally.
                if retry.is_empty() {
                    return;
                }
                cx.background_executor().timer(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
                ids = retry;
            }
        }));
    }

    pub(crate) fn native_fork_finished(
        &mut self,
        chat: &str,
        message: &str,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.native_fork_pending
            .remove(&(chat.to_owned(), message.to_owned()));
        if let Some(error) = error {
            self.native_fork_errors
                .insert((chat.to_owned(), message.to_owned()), error);
        }
        cx.notify();
    }

    fn close_native_fork_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let return_focus = self
            .native_fork_menu
            .as_open()
            .and_then(|menu| menu.return_focus.clone());
        if let Some(focus) = return_focus {
            window.focus(&focus, cx);
        }
        if self.native_fork_menu.begin_close() {
            crate::popover::reap_popup(cx, |this| &mut this.native_fork_menu);
        }
        cx.notify();
    }

    fn choose_native_fork(
        &mut self,
        destination: NativeForkDestination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.native_fork_menu.as_open() else {
            return;
        };
        let chat = menu.chat.clone();
        let message = menu.message.clone();
        if self.chat_id.as_deref() != Some(&chat)
            || !self.native_forks.get(&message).is_some_and(|a| a.available)
            || (destination == NativeForkDestination::MainConversation
                && !self.main_fork_supported(cx))
        {
            self.close_native_fork_menu(window, cx);
            return;
        }
        let identity = (chat.clone(), message.clone());
        if !self.native_fork_pending.insert(identity.clone()) {
            return;
        }
        self.native_fork_errors.remove(&identity);
        self.close_native_fork_menu(window, cx);
        cx.emit(TranscriptEvent::ForkMessage {
            chat_id: chat,
            message_id: message,
            destination,
        });
    }

    fn main_fork_supported(&self, cx: &Context<Self>) -> bool {
        let state = self.state.read(cx);
        state.selected_chat_row().is_some_and(|chat| {
            state.device_supports(
                &chat.device_id,
                zeron_proto::capabilities::NATIVE_MESSAGE_FORK_MAIN_V1,
            )
        })
    }

    fn native_fork_menu(
        &self,
        entry: &SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.native_fork_menu.get()?;
        if menu.message != entry.as_ref() || self.chat_id.as_deref() != Some(&menu.chat) {
            return None;
        }
        let active = menu.active;
        let main_supported = self.main_fork_supported(cx);
        let theme = theme.for_popup();
        let mut card = crate::popover::popover_card(&theme)
            .w(px(220.0))
            .track_focus(&self.native_fork_menu_focus)
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_native_fork_menu(window, cx)),
            )
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    use crate::popover::MenuKey;
                    match crate::popover::classify_key(
                        &event.keystroke.key,
                        event.keystroke.modifiers.platform,
                        event.keystroke.modifiers.control,
                    ) {
                        MenuKey::Escape => this.close_native_fork_menu(window, cx),
                        MenuKey::Up | MenuKey::Down => {
                            if let Some(menu) = this.native_fork_menu.open_mut() {
                                menu.active = if main_supported { 1 - menu.active } else { 0 };
                            }
                            cx.notify();
                        }
                        MenuKey::Enter | MenuKey::ModEnter => {
                            if let Some(menu) = this.native_fork_menu.as_open() {
                                let destination = if menu.active == 0 {
                                    NativeForkDestination::SideChat
                                } else {
                                    NativeForkDestination::MainConversation
                                };
                                this.choose_native_fork(destination, window, cx);
                            }
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .flex()
            .flex_col();
        for (index, id, label, destination) in [
            (
                0,
                "native-fork-side-chat",
                "Fork in side chat",
                NativeForkDestination::SideChat,
            ),
            (
                1,
                "native-fork-main-conversation",
                "Fork as main conversation",
                NativeForkDestination::MainConversation,
            ),
        ] {
            let enabled = index == 0 || main_supported;
            card = card.child(
                crate::popover::menu_row_nav(&theme, false, active == index, id)
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(gpui::Role::MenuItem)
                    .aria_label(label)
                    .when(!enabled, |row| {
                        row.opacity(0.4).cursor_default().tooltip(
                            crate::settings::widgets::text_tooltip(
                                "Update the chat host to fork as a main conversation",
                            ),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if enabled {
                            this.choose_native_fork(destination, window, cx);
                        }
                    }))
                    .child(label),
            );
        }
        Some(crate::popover::anchored_menu_right(
            "native-fork-destination-menu",
            card.into_any_element(),
            self.native_fork_menu.closing_since(),
        ))
    }

    pub(super) fn native_fork_button(
        &self,
        entry: &SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let availability = self.native_forks.get(entry.as_ref())?;
        let chat = self.chat_id.clone()?;
        let message = entry.to_string();
        let identity = (chat.clone(), message.clone());
        let busy = self.native_fork_pending.contains(&identity);
        let enabled = availability.available && !busy;
        let label = if busy {
            "Creating conversation…"
        } else {
            "Fork conversation"
        };
        let tooltip = if busy {
            label.to_owned()
        } else {
            self.native_fork_errors
                .get(&identity)
                .cloned()
                .or_else(|| availability.reason.clone())
                .unwrap_or_else(|| label.to_owned())
        };
        let menu = self.native_fork_menu(entry, theme, cx);
        let trigger_message = message.clone();
        Some(
            div()
                .id(SharedString::from(format!("native-fork-{entry}")))
                .role(gpui::Role::Button)
                .aria_label(label)
                .aria_description(tooltip.clone())
                .tab_index(0)
                .focus_visible(|s| s.border_1().border_color(theme.accent))
                .size(px(Theme::SPACE_MD * 2.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(Theme::CONTROL_RADIUS))
                .when(enabled, |el| {
                    el.cursor_pointer().hover(|s| s.bg(crate::theme::ink(0.08)))
                })
                .tooltip(crate::settings::widgets::text_tooltip(tooltip))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, _| {
                        this.native_fork_menu
                            .note_trigger_press_matching(|menu| menu.message == trigger_message);
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !enabled || this.native_fork_pending.contains(&identity) {
                        return;
                    }
                    if this.native_fork_menu.take_press_was_open() {
                        this.close_native_fork_menu(window, cx);
                        return;
                    }
                    this.native_fork_menu.open(ForkMenu {
                        chat: chat.clone(),
                        message: message.clone(),
                        active: 0,
                        return_focus: window.focused(cx),
                    });
                    window.focus(&this.native_fork_menu_focus, cx);
                    cx.notify();
                }))
                .children(menu)
                .child(
                    crate::icons::icon(crate::icons::GIT_BRANCH)
                        .size(px(14.0))
                        .text_color(theme.text_muted.opacity(if enabled { 1.0 } else { 0.4 })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use zeron_proto::view::ConnectionStatus;
    use zeron_proto::{ChatConnectivity, ChatSyncState, ConnectivityState};
    use zeron_rpc::{ClientFrame, ServerFrame, methods};

    struct Fixture {
        state: Entity<AppState>,
        view: Entity<Transcript>,
        requests: tokio::sync::mpsc::Receiver<String>,
        replies: tokio::sync::mpsc::Sender<String>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(cx: &mut TestAppContext, host_online: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let (out, requests) = tokio::sync::mpsc::channel(64);
            let (replies, inbound) = tokio::sync::mpsc::channel(64);
            let (state, view) = cx.update(|cx| {
                gpui_base::init(cx);
                cx.set_global(Theme::dark());
                crate::settings::init(Default::default(), dir.path(), cx);
                let state =
                    cx.new(|_| {
                        let mut state = AppState::new();
                        state.set_test_engine(crate::state::EngineHandle::from_test_client(
                            zeron_rpc::RpcClient::new(out, inbound),
                        ));
                        state.connection = ConnectionStatus::Ready;
                        state.local_device_id = Some("viewer".into());
                        state.selected_chat = Some("remote-chat".into());
                        state.chats = vec![serde_json::from_value(serde_json::json!({
                        "id":"remote-chat", "deviceId":"host", "createdAt":chrono::Utc::now(),
                        "archived":false, "config":{"harness":"codex", "sandbox":"workspace-write"}
                    })).unwrap()];
                        state.devices =
                            vec![serde_json::from_value(serde_json::json!({
                        "id":"host", "name":"Remote host", "platform":"linux",
                        "lastSeenAt":host_online.then(chrono::Utc::now),
                        "capabilities":[zeron_proto::capabilities::NATIVE_MESSAGE_FORK_V1]
                    })).unwrap()];
                        state.connectivity.state = ConnectivityState::Connected;
                        state.connectivity.chats = vec![ChatConnectivity {
                            chat_id: "remote-chat".into(),
                            sync_state: ChatSyncState::Waiting,
                            connected: false,
                            delivery_live: false,
                            pending_pushes: 0,
                        }];
                        let mut entry: SessionMessageEntry =
                            serde_json::from_value(serde_json::json!({
                                "id":"a1", "role":"assistant", "status":"complete", "parts":[],
                                "createdAt":1, "deviceId":"host"
                            }))
                            .unwrap();
                        entry.native_fork_point = Some(zeron_proto::NativeForkPoint {
                            format_version: 1,
                            harness: HarnessId::Codex,
                            source_device_id: "host".into(),
                            source_session_id: "native-parent".into(),
                            cwd: "/project".into(),
                            boundary: zeron_proto::NativeForkBoundary::AppServerTurn {
                                turn_id: "turn1".into(),
                            },
                        });
                        state.transcript = vec![entry];
                        state
                    });
                let view = cx.new(|cx| Transcript::new(state.clone(), cx));
                (state, view)
            });
            Self {
                state,
                view,
                requests,
                replies,
                _dir: dir,
            }
        }

        fn queries(&mut self, cx: &mut TestAppContext) -> Vec<ClientFrame> {
            cx.run_until_parked();
            let mut queries = Vec::new();
            while let Ok(frame) = self.requests.try_recv() {
                let frame: ClientFrame = serde_json::from_str(&frame).unwrap();
                if frame.method.as_deref() == Some(methods::GET_NATIVE_FORK_AVAILABILITY) {
                    assert_eq!(frame.params["targetDeviceId"], "host");
                    queries.push(frame);
                }
            }
            queries
        }

        fn query(&mut self, cx: &mut TestAppContext) -> ClientFrame {
            let mut queries = self.queries(cx);
            assert_eq!(queries.len(), 1);
            queries.pop().unwrap()
        }

        fn reply(
            &self,
            query: &ClientFrame,
            value: Result<NativeForkAvailability, &str>,
            runtime: &tokio::runtime::Runtime,
            cx: &mut TestAppContext,
        ) {
            let frame = match value {
                Ok(value) => ServerFrame {
                    id: query.id,
                    ok: Some(serde_json::json!({"a1":value})),
                    ..Default::default()
                },
                Err(error) => ServerFrame {
                    id: query.id,
                    err: Some(error.into()),
                    ..Default::default()
                },
            };
            self.replies
                .try_send(serde_json::to_string(&frame).unwrap())
                .unwrap();
            runtime.block_on(async { tokio::task::yield_now().await });
            cx.run_until_parked();
        }

        fn available(&self, cx: &TestAppContext) -> bool {
            self.view
                .read_with(cx, |view, _| view.native_forks["a1"].available)
        }

        fn button_visible(&self, cx: &mut TestAppContext) -> bool {
            self.view.update(cx, |view, cx| {
                view.native_fork_button(&"a1".into(), &Theme::of(cx).clone(), cx)
                    .is_some()
            })
        }
    }

    #[gpui::test]
    fn native_fork_hides_missing_points_and_unsupported_hosts(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        for missing_point in [true, false] {
            let mut fixture = Fixture::new(cx, true);
            let first = fixture.query(cx);
            assert!(fixture.button_visible(cx));
            let (point, capabilities) = fixture.state.update(cx, |state, cx| {
                let original = (
                    state.transcript[0].native_fork_point.clone(),
                    state.devices[0].capabilities.clone(),
                );
                if missing_point {
                    state.transcript[0].native_fork_point = None;
                } else {
                    state.devices[0].capabilities.clear();
                }
                cx.notify();
                original
            });
            assert!(fixture.queries(cx).is_empty());
            assert!(!fixture.button_visible(cx));
            // An answer to the previous query must not restore a hidden action.
            fixture.reply(
                &first,
                Ok(NativeForkAvailability::available()),
                &runtime,
                cx,
            );
            assert!(!fixture.button_visible(cx));
            cx.executor().advance_clock(Duration::from_secs(30));
            assert!(fixture.queries(cx).is_empty());

            fixture.state.update(cx, |state, cx| {
                state.transcript[0].native_fork_point = point;
                state.devices[0].capabilities = capabilities;
                cx.notify();
            });
            let recovered = fixture.query(cx);
            fixture.reply(
                &recovered,
                Ok(NativeForkAvailability::available()),
                &runtime,
                cx,
            );
            assert!(fixture.button_visible(cx));
            assert!(fixture.available(cx));
        }
    }

    #[gpui::test]
    fn native_fork_remote_recovery_refreshes_without_local_reconnect(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        for presence_recovers in [true, false] {
            let mut fixture = Fixture::new(cx, !presence_recovers);
            let first = fixture.query(cx);
            fixture.reply(&first, Err("Host is offline"), &runtime, cx);
            assert!(!fixture.available(cx));
            fixture.state.update(cx, |state, cx| {
                if presence_recovers {
                    state.devices[0].last_seen_at = Some(chrono::Utc::now());
                } else {
                    state.connectivity.chats[0].delivery_live = true;
                }
                assert_eq!(state.connection, ConnectionStatus::Ready);
                cx.notify();
            });
            let recovered = fixture.query(cx);
            fixture.reply(
                &recovered,
                Ok(NativeForkAvailability::available()),
                &runtime,
                cx,
            );
            assert!(fixture.available(cx));
            fixture.state.update(cx, |state, cx| {
                state.devices[0].last_seen_at = Some(chrono::Utc::now());
                state.connectivity.chats[0].pending_pushes += 1;
                let mut other = state.connectivity.chats[0].clone();
                other.chat_id = "unrelated".into();
                state.connectivity.chats.push(other);
                cx.notify();
            });
            cx.executor().advance_clock(Duration::from_secs(30));
            assert!(
                fixture.queries(cx).is_empty(),
                "Health noise and old retry timers must not query again"
            );
        }
    }

    #[gpui::test]
    fn native_fork_query_errors_retry_with_backoff_and_stop_on_answer(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let mut fixture = Fixture::new(cx, true);
        let first = fixture.query(cx);
        fixture.reply(&first, Err("Temporary timeout"), &runtime, cx);
        assert!(fixture.queries(cx).is_empty());
        cx.executor().advance_clock(Duration::from_secs(1));
        let retry = fixture.query(cx);
        assert_eq!(retry.params, first.params);
        fixture.reply(&retry, Err("Temporary timeout"), &runtime, cx);
        cx.executor().advance_clock(Duration::from_secs(1));
        assert!(
            fixture.queries(cx).is_empty(),
            "Repeated errors must back off"
        );
        cx.executor().advance_clock(Duration::from_secs(1));
        let retry = fixture.query(cx);
        fixture.reply(
            &retry,
            Ok(NativeForkAvailability::unavailable(
                "Unsupported native boundary",
            )),
            &runtime,
            cx,
        );
        assert!(!fixture.available(cx));
        cx.executor().advance_clock(Duration::from_secs(60));
        assert!(
            fixture.queries(cx).is_empty(),
            "Definitive availability is cached"
        );
    }

    #[gpui::test]
    fn native_fork_recovery_discards_stale_queries_and_navigation_cancels_retries(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let mut fixture = Fixture::new(cx, false);
        let stale = fixture.query(cx);
        fixture.state.update(cx, |state, cx| {
            state.devices[0].last_seen_at = Some(chrono::Utc::now());
            cx.notify();
        });
        let recovered = fixture.query(cx);
        fixture.reply(
            &recovered,
            Ok(NativeForkAvailability::available()),
            &runtime,
            cx,
        );
        fixture.reply(&stale, Err("Old host connection failed"), &runtime, cx);
        assert!(
            fixture.available(cx),
            "An old failure must not replace recovery"
        );
        cx.executor().advance_clock(Duration::from_secs(30));
        assert!(fixture.queries(cx).is_empty());
        fixture.state.update(cx, |state, cx| {
            state.connectivity.chats[0].connected = true;
            cx.notify();
        });
        let query = fixture.query(cx);
        fixture.reply(&query, Err("Temporary failure"), &runtime, cx);
        fixture.state.update(cx, |state, cx| {
            state.selected_chat = None;
            cx.notify();
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(30));
        assert!(
            fixture.queries(cx).is_empty(),
            "Leaving the chat cancels retries"
        );
    }
}
