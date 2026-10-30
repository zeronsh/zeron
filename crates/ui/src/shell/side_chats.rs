use super::*;

pub(super) struct SideChatTab {
    pub state: Entity<AppState>,
    transcript: Entity<Transcript>,
    pub(super) composer: Entity<Composer>,
    /// A fork its host has yet to make (see [`Shell::fork_chat`]). The tab
    /// shows its progress, or why it failed, in place of a composer whose
    /// first send would mint the chat a second way.
    pub(super) fork: Option<PendingFork>,
    _events: Vec<Subscription>,
}

pub(super) struct PendingFork {
    /// The request as sent. Retry replays it unchanged: forking is
    /// idempotent per chat id.
    params: serde_json::Value,
    /// The remote host doing the work, named while it does.
    host: Option<SharedString>,
    error: Option<SharedString>,
    /// Its request is in flight. Per tab: one fork never waits on another.
    running: bool,
}

/// The row of a chat about to exist under `parent_id`, on `from`'s device
/// with its checkout and configuration.
fn draft_child_row(from: &zeron_proto::Chat, parent_id: String) -> zeron_proto::Chat {
    let mut chat = from.clone();
    chat.id = uuid::Uuid::new_v4().to_string();
    chat.parent_chat_id = Some(parent_id);
    chat.title = None;
    chat.archived = false;
    chat.created_at = Utc::now();
    chat.last_message_at = None;
    chat.last_message_preview = None;
    chat.last_seen_at = None;
    chat.harness_session_id = None;
    chat.harness_session_cwd = None;
    chat.room_gen = Some(2);
    chat
}

impl Shell {
    pub(super) fn close_empty_right_pane(&mut self, key: &str, cx: &mut Context<Self>) {
        let empty = if key == self.panel_key(cx) {
            self.right_surface_rows(cx).is_empty()
        } else {
            self.right_tabs.get(key).is_none_or(Vec::is_empty)
        };
        if empty {
            if key == self.panel_key(cx) && self.right_pane_open(cx) {
                self.toggle_right_pane(cx);
            } else {
                self.panels.update(key, |p| p.changes_open = false);
            }
        }
    }

    /// Side-chat creation failed (nothing to fork yet, an engine or remote
    /// device error): say why in the conversation's composer.
    fn show_side_chat_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        self.composer
            .update(cx, |composer, cx| composer.show_error(message, cx));
    }

    /// The surface picker's "Side chat": fork the current conversation
    /// through its latest completed response, as a child of it.
    pub(super) fn create_side_chat(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.state.read(cx).selected_chat_row().cloned() else {
            self.show_side_chat_error("Start a conversation before creating a side chat.", cx);
            return;
        };
        let parent = source.id.clone();
        self.fork_chat(source, parent, cx);
    }

    /// Fork `source` through its latest completed response into a new chat
    /// hanging under `parent_id`, and open it in the right pane. A side
    /// chat's own fork button passes its parent so the copy lists as a
    /// sibling; the picker passes the source itself.
    ///
    /// The tab opens at once: a remote host can take a while to answer, and
    /// the tab says what it waits on — or, failing, why, with a retry.
    pub(super) fn fork_chat(
        &mut self,
        source: zeron_proto::Chat,
        parent_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).engine().is_none() {
            return;
        }
        let chat = draft_child_row(&source, parent_id.clone());
        let params = serde_json::json!({
            "chatId": chat.id,
            "sourceChatId": source.id,
            "parentChatId": parent_id,
            "targetDeviceId": source.device_id,
        });
        let host = {
            let state = self.state.read(cx);
            (state.local_device_id.as_deref() != Some(source.device_id.as_str()))
                .then(|| state.device_name(&source.device_id))
                .flatten()
                .map(|name| SharedString::from(name.to_owned()))
        };
        let key = self.panel_key(cx);
        let id = self.open_side_chat_tab(chat, key, true, cx);
        if let Some(tab) = self.side_chats.get_mut(&id) {
            tab.fork = Some(PendingFork {
                params,
                host,
                error: None,
                running: false,
            });
        }
        self.run_fork(id, cx);
    }

    /// Ask the host for the fork side chat tab `id` is waiting on, unless
    /// that request is already in flight.
    fn run_fork(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(fork) = self
            .side_chats
            .get_mut(&id)
            .and_then(|tab| tab.fork.as_mut())
            .filter(|fork| !fork.running)
        else {
            return;
        };
        fork.error = None;
        fork.running = true;
        let params = fork.params.clone();
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<zeron_proto::Chat>(methods::FORK_SIDE_CHAT, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                match (result, this.side_chats.get_mut(&id)) {
                    (Ok(chat), Some(tab)) if tab.fork.is_some() => this.finish_fork(id, chat, cx),
                    (Err(error), Some(tab)) => {
                        if let Some(fork) = &mut tab.fork {
                            fork.running = false;
                            fork.error = Some(error.to_string().into());
                        }
                    }
                    // Closed while forking: the fork still lists under its
                    // parent, and nothing reopens the tab.
                    (Err(error), None) => this.show_side_chat_error(error.to_string(), cx),
                    // Done already: its row opened the chat in this tab.
                    (Ok(_), _) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn retry_fork(&mut self, id: u64, cx: &mut Context<Self>) {
        self.run_fork(id, cx);
    }

    /// The host made the fork: its tab becomes the side chat, in place.
    fn finish_fork(&mut self, id: u64, chat: zeron_proto::Chat, cx: &mut Context<Self>) {
        let parent_listed = self
            .state
            .read(cx)
            .chats
            .iter()
            .any(|c| Some(&c.id) == chat.parent_chat_id.as_ref());
        if parent_listed {
            let tab = self.new_side_chat_tab(chat, false, cx);
            self.side_chats.insert(id, tab);
        } else if let Some(tab) = self.side_chats.get(&id) {
            // The parent went away meanwhile: close as a deleted side chat.
            tab.state
                .clone()
                .update(cx, |state, cx| state.select_chat(None, cx));
        }
    }

    /// A fresh, empty side chat under `parent_id` (the active chat when
    /// None), opened in the right pane ready for its first message. Nothing
    /// is written until that message: the first send mints it with the same
    /// shape the Zeron MCP server's `create_chat` does, so agent-spawned and
    /// hand-started side chats list together, and an abandoned one leaves
    /// no row behind.
    pub(super) fn create_child_chat(&mut self, parent_id: Option<String>, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let parent = match parent_id {
            Some(id) => state.chats.iter().find(|c| c.id == id).cloned(),
            None => state.selected_chat_row().cloned(),
        };
        let Some(parent) = parent else {
            self.show_side_chat_error("Start a conversation before creating a side chat.", cx);
            return;
        };
        let key = self.panel_key(cx);
        let chat = draft_child_row(&parent, parent.id.clone());
        self.open_side_chat_tab(chat, key, true, cx);
    }

    /// Open an existing side chat (a footer row) in the right pane.
    pub(super) fn open_child_chat_tab(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(chat) = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .cloned()
        else {
            return;
        };
        let key = self.panel_key(cx);
        self.open_side_chat(chat, key, cx);
    }

    /// The side chat on screen: the right pane shows its tab, open and not
    /// hidden by the window fit (which hides it off the chat route).
    pub(super) fn side_chat_on_screen(&self, cx: &App) -> Option<String> {
        if !self.right_pane_shown(cx) {
            return None;
        }
        let RightSurface::SideChat(id) = self.resolved_right_active(cx) else {
            return None;
        };
        self.side_chats
            .get(&id)?
            .state
            .read(cx)
            .selected_chat
            .clone()
    }

    /// Reveal the surface host of the pane keyed `key`: the live one through
    /// the user-visible path, a background conversation's by flag, so a
    /// fork that finishes after the user switched away opens ITS pane, not
    /// whichever is on screen.
    fn open_surfaces_for(&mut self, key: &str, cx: &mut Context<Self>) {
        if key == self.panel_key(cx) {
            // Programmatic, so an already-open pane is left alone.
            self.set_surfaces_open(true, cx);
        } else {
            // Rank the pane most recent, open or fit-hidden, so a narrow
            // window shows it on the next visit.
            self.panels.update(key, |p| p.changes_open = true);
            self.record_panel_open(AuxiliaryPanel::Right, key);
        }
    }

    pub(super) fn open_side_chat(
        &mut self,
        chat: zeron_proto::Chat,
        key: String,
        cx: &mut Context<Self>,
    ) {
        self.open_side_chat_tab(chat, key, false, cx);
    }

    /// `unsaved`: `chat` is only minted here, and its first send creates it.
    /// Returns the tab, reused when one already shows `chat`.
    fn open_side_chat_tab(
        &mut self,
        chat: zeron_proto::Chat,
        key: String,
        unsaved: bool,
        cx: &mut Context<Self>,
    ) -> u64 {
        // A footer row or header button may land while the surface host is
        // closed (explorer-only pane, or hidden with its tabs kept): open it
        // beside the explorer first, for an existing tab too.
        self.open_surfaces_for(&key, cx);
        if let Some((&id, tab)) = self
            .side_chats
            .iter()
            .find(|(_, tab)| tab.state.read(cx).selected_chat.as_deref() == Some(&chat.id))
        {
            // A fork's row can land before its host answers, or after the
            // answer timed out: the row is the chat, so its tab becomes it.
            if !unsaved && tab.fork.is_some() {
                self.finish_fork(id, chat, cx);
            }
            // A closed tab kept for its draft is detached: re-attach it.
            let tabs = self.right_tabs.entry(key.clone()).or_default();
            if !tabs.contains(&RightSurface::SideChat(id)) {
                tabs.push(RightSurface::SideChat(id));
            }
            if key == self.panel_key(cx) {
                self.set_right_active(RightSurface::SideChat(id), cx);
            } else {
                self.panels
                    .update(&key, |p| p.right_active = RightSurface::SideChat(id));
            }
            cx.notify();
            return id;
        }
        let tab = self.new_side_chat_tab(chat, unsaved, cx);
        self.side_chat_seq += 1;
        let id = self.side_chat_seq;
        self.side_chats.insert(id, tab);
        self.right_tabs
            .entry(key.clone())
            .or_default()
            .push(RightSurface::SideChat(id));
        self.panels
            .update(&key, |p| p.right_active = RightSurface::SideChat(id));
        cx.notify();
        id
    }

    /// A side chat's own state, transcript and composer over `chat`.
    fn new_side_chat_tab(
        &mut self,
        chat: zeron_proto::Chat,
        unsaved: bool,
        cx: &mut Context<Self>,
    ) -> SideChatTab {
        let chat_id = chat.id.clone();
        let parent = self.state.clone();
        let state = cx.new(|cx| AppState::side_chat_state(&parent, chat, unsaved, cx));
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        // Workspace file links open the editor and web links honor the
        // in-app preference, resolved in this side chat's context.
        let links = Self::session_links(Some(chat_id), cx);
        transcript.update(cx, |transcript, _| {
            transcript.set_workspace_link_handler(links)
        });
        let composer = cx.new(|cx| {
            let mut composer = Composer::new(state.clone(), cx);
            composer.set_side_chat(cx);
            composer
        });
        let events = vec![
            cx.subscribe(&transcript, Self::on_transcript_event),
            cx.subscribe(&composer, {
                let transcript = transcript.clone();
                move |this: &mut Self, _, event, cx| {
                    match event {
                        ComposerEvent::WorkspaceCommand(command) => {
                            this.pending_workspace_command = Some(*command);
                            cx.notify();
                        }
                        // A side chat is already selected before its composer
                        // mounts, and it inherits its parent's checkout, so it
                        // never runs worktree setup of its own.
                        ComposerEvent::NewThreadTransitionStarted
                        | ComposerEvent::WorktreeSetup { .. }
                        | ComposerEvent::AnnotationsChanged(_)
                        | ComposerEvent::AnnotationHovered(_) => {}
                        ComposerEvent::Sent {
                            chat_id,
                            message_id,
                        } => transcript.update(cx, |t, cx| {
                            t.on_own_send(chat_id.clone(), message_id.clone(), cx)
                        }),
                        ComposerEvent::Queued {
                            chat_id,
                            message_id,
                        } => transcript.update(cx, |t, cx| {
                            t.on_own_queued_send(chat_id.clone(), message_id.clone(), cx)
                        }),
                    }
                }
            }),
            cx.observe(&state, |this, state, cx| {
                if state.read(cx).selected_chat.is_none() {
                    this.remove_deleted_side_chats(cx);
                }
                cx.notify();
            }),
        ];
        SideChatTab {
            state,
            transcript,
            composer,
            fork: None,
            _events: events,
        }
    }

    fn remove_deleted_side_chats(&mut self, cx: &mut Context<Self>) {
        let removed: Vec<_> = self
            .side_chats
            .iter()
            .filter(|(_, tab)| tab.state.read(cx).selected_chat.is_none())
            .map(|(&id, _)| id)
            .collect();
        for id in removed {
            self.side_chats.remove(&id);
            let surface = RightSurface::SideChat(id);
            let keys: Vec<_> = self
                .right_tabs
                .iter_mut()
                .filter_map(|(key, tabs)| {
                    let contained = tabs.contains(&surface);
                    tabs.retain(|tab| *tab != surface);
                    contained.then(|| key.clone())
                })
                .collect();
            for key in keys {
                self.panels.forget_right_surface(&key, surface);
                let fallback = self.panels.right_surface_fallback(
                    &key,
                    self.right_tabs.get(&key).into_iter().flatten().copied(),
                );
                self.panels.update(&key, |panel| {
                    if panel.right_active == surface {
                        panel.right_active = fallback;
                    }
                });
                self.close_empty_right_pane(&key, cx);
            }
        }
    }

    pub(super) fn render_side_chat(&mut self, id: u64, cx: &mut Context<Self>) -> AnyElement {
        let Some(tab) = self
            .side_chats
            .get(&id)
            .filter(|tab| tab.state.read(cx).selected_chat.is_some())
        else {
            return self.render_surface_picker(cx);
        };
        if let Some(fork) = &tab.fork {
            let (host, error) = (fork.host.clone(), fork.error.clone());
            return self.render_pending_fork(id, host, error, cx);
        }
        let transcript = tab.transcript.clone();
        let composer = tab.composer.clone();
        // Share the main chat's docked width cap and responsive padding. Size
        // from the width the pane lays out at, not its mask, which an open or
        // close animates.
        let width = composer_target_width(
            self.right_content_width(self.right_target(cx)),
            settings::transcript_width(cx),
            true,
        );
        composer.update(cx, |composer, cx| {
            composer.set_dock_frame(crate::composer_dock::DockFrame::settled(true), cx);
            composer.set_available_width(width, cx);
        });
        let pill = transcript.read(cx).jump_button_shown().then(|| {
            div()
                .absolute()
                .bottom(px(12.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(self.jump_pill(
                    "side-chat-jump",
                    "side-chat-jump-pill",
                    transcript.clone(),
                    cx,
                ))
        });
        self.chat_dropzone(("side-chat-dropzone", id), composer.clone(), cx)
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        crate::edge_fade::edge_faded(
                            Theme::TRANSCRIPT_FADE_BAND,
                            true,
                            false,
                            div().size_full().child(transcript),
                        )
                        .inset_top(Theme::TITLEBAR_HEIGHT),
                    )
                    .children(pill),
            )
            .child(div().flex_none().child(composer))
            .child(Self::attachment_drop_overlay(Theme::of(cx)))
            .child(Self::mention_drop_overlay(Theme::of(cx)))
            .into_any_element()
    }

    /// A fork its host has yet to make: what it waits on, or why it failed
    /// and a retry.
    fn render_pending_fork(
        &mut self,
        id: u64,
        host: Option<SharedString>,
        error: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let body = match error {
            None => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .child(loaders::ring_spinner(
                    format!("pending-fork-loader-{id}"),
                    20.0,
                    theme.text_muted,
                ))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_faint)
                        .child(match host {
                            Some(host) => SharedString::from(format!("Forking on {host}…")),
                            None => SharedString::from("Forking…"),
                        }),
                )
                .into_any_element(),
            // An empty state, not an alarm: the engine's reason is the
            // detail under a plain title, and Retry is the pane's one action.
            Some(error) => div()
                .flex()
                .flex_col()
                .items_center()
                .max_w(px(420.0))
                .text_center()
                .child(super::command_palette::palette_empty(
                    &theme,
                    match host {
                        Some(host) => SharedString::from(format!("Couldn't fork on {host}")),
                        None => SharedString::from("Couldn't fork this chat"),
                    },
                    error,
                ))
                .child(
                    popover::btn_ghost(&theme, "Retry", format!("pending-fork-retry-{id}"))
                        .id(("pending-fork-retry", id))
                        .on_click(cx.listener(move |this, _, _, cx| this.retry_fork(id, cx))),
                )
                .into_any_element(),
        };
        crate::motion::fade_in(
            ("pending-fork", id),
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p(px(Theme::SPACE_LG))
                .child(body),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn deleted_side_chat_removes_tab_and_closes_empty_pane(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, _, cx| {
                shell.active_chat = "main".into();
                shell.toggle_right_pane(cx);
                let chat = serde_json::from_value(serde_json::json!({
                    "id": "side", "parentChatId": "main", "deviceId": "local",
                    "archived": false, "createdAt": Utc::now(),
                }))
                .unwrap();
                shell.open_side_chat(chat, shell.panel_key(cx), cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let id = shell.side_chat_seq;
                let side = shell.side_chats[&id].state.clone();
                side.update(cx, |state, cx| state.select_chat(None, cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.side_chats.is_empty());
                assert!(shell.right_surface_rows(cx).is_empty());
                assert!(!shell.right_pane_open(cx));
                assert_eq!(shell.resolved_right_active(cx), RightSurface::Picker);
            })
            .unwrap();
    }

    fn shell_window(dir: &std::path::Path, cx: &mut TestAppContext) -> gpui::WindowHandle<Shell> {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir, cx);
        });
        cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        })
    }

    #[gpui::test]
    fn side_chat_visits_and_deletion_restore_recent_tab(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = shell_window(dir.path(), cx);
        let (recent, side_id) = window
            .update(cx, |shell, window, cx| {
                shell.active_chat = "main".into();
                shell.add_browser_surface(None, window, cx);
                shell.add_browser_surface(None, window, cx);
                let recent = RightSurface::Browser(shell.browser_seq);
                let chat = serde_json::from_value(serde_json::json!({
                    "id": "side", "parentChatId": "main", "deviceId": "local",
                    "archived": false, "createdAt": Utc::now(),
                }))
                .unwrap();
                shell.open_side_chat(chat, shell.panel_key(cx), cx);
                let side_id = shell.side_chat_seq;
                shell.add_browser_surface(None, window, cx);
                shell.close_right_surface(RightSurface::Browser(shell.browser_seq), window, cx);
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::SideChat(side_id)
                );
                (recent, side_id)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                shell.side_chats[&side_id]
                    .state
                    .update(cx, |state, cx| state.select_chat(None, cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.resolved_right_active(cx), recent);
                assert!(
                    !shell.panels.right_tab_history[&shell.panel_key(cx)]
                        .contains(&RightSurface::SideChat(side_id))
                );
            })
            .unwrap();
    }

    /// "New side chat" writes nothing: the tab opens on a chat only its
    /// first send mints, and closing it before then leaves nothing behind.
    #[gpui::test]
    fn new_side_chat_is_unsaved_until_its_first_send(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = shell_window(dir.path(), cx);
        window
            .update(cx, |shell, window, cx| {
                shell.active_chat = "main".into();
                let main: zeron_proto::Chat = serde_json::from_value(serde_json::json!({
                    "id": "main", "deviceId": "local", "cwd": "/tmp/main",
                    "archived": false, "createdAt": Utc::now(),
                }))
                .unwrap();
                shell.state.update(cx, |state, _| {
                    state.chats = vec![main];
                    state.selected_chat = Some("main".into());
                });
                shell.toggle_right_pane(cx);
                shell.create_child_chat(None, cx);
                let id = shell.side_chat_seq;
                assert_eq!(shell.resolved_right_active(cx), RightSurface::SideChat(id));
                let side = shell.side_chats[&id].state.clone();
                let chat_id = side.read(cx).selected_chat.clone().unwrap();
                assert!(side.read(cx).side_chat_unsaved());
                // A model picked before the first send is what gets minted.
                let config: zeron_proto::ChatConfig = serde_json::from_value(serde_json::json!({
                    "harness": "codex", "sandbox": "workspace-write",
                }))
                .unwrap();
                side.update(cx, |state, _| {
                    state.apply_chat_config(&chat_id, config);
                    state.apply_chats(Vec::new());
                });
                let create = side.read(cx).unsaved_side_chat_create(&chat_id).unwrap();
                assert_eq!(create["parentChatId"], "main");
                assert_eq!(create["cwd"], "/tmp/main");
                assert_eq!(create["config"]["harness"], "codex");
                let row = side.read(cx).selected_chat_row().cloned().unwrap();
                assert_eq!(row.config.unwrap().harness, zeron_proto::HarnessId::Codex);
                assert_eq!(shell.state.read(cx).chats.len(), 1, "no row yet");
                // A draft cannot keep it: no row could reopen it.
                let composer = shell.side_chats[&id].composer.clone();
                composer.update(cx, |composer, cx| {
                    composer.stage_appshot(crate::appshots::tests::shot(), cx)
                });
                shell.close_right_surface(RightSurface::SideChat(id), window, cx);
                assert!(!shell.side_chats.contains_key(&id));
            })
            .unwrap();
    }

    /// Review feedback on the side-chat pane: its links resolve, a closed
    /// tab keeps its draft, its model menu owns the digit shortcuts, and
    /// creation failures are shown.
    #[gpui::test]
    fn side_chat_links_drafts_shortcuts_and_errors(cx: &mut TestAppContext) {
        use crate::markdown::render::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};
        let dir = tempfile::tempdir().unwrap();
        let window = shell_window(dir.path(), cx);
        // Nothing to fork yet: the reason shows in the composer.
        window
            .update(cx, |shell, _, cx| {
                shell.create_side_chat(cx);
                assert!(
                    shell
                        .composer
                        .read(cx)
                        .failure()
                        .is_some_and(|message| message.contains("Start a conversation"))
                );
            })
            .unwrap();
        let chat: zeron_proto::Chat = serde_json::from_value(serde_json::json!({
            "id": "side", "parentChatId": "main", "deviceId": "local", "cwd": "/tmp/other",
            "archived": false, "createdAt": Utc::now(),
        }))
        .unwrap();
        window
            .update(cx, |shell, _, cx| {
                shell.active_chat = "main".into();
                let main: zeron_proto::Chat = serde_json::from_value(serde_json::json!({
                    "id": "main", "deviceId": "local", "cwd": "/tmp/main",
                    "archived": false, "createdAt": Utc::now(),
                }))
                .unwrap();
                shell.state.update(cx, |state, _| {
                    state.chats = vec![main];
                    state.selected_chat = Some("main".into());
                });
                shell.toggle_right_pane(cx);
                shell.open_side_chat(chat.clone(), shell.panel_key(cx), cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, window, cx| {
                let id = shell.side_chat_seq;
                // Links from the open side chat are its own, not rejected.
                let mut activation = LinkActivation {
                    target: LinkTarget::new("Docs", "https://example.com/docs"),
                    action: LinkAction::External,
                    source_session: Some("side".into()),
                };
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::External("https://example.com/docs".into())
                );
                activation.source_session = Some("elsewhere".into());
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::Rejected
                );
                // A side chat's file link opens an editor bound to the side
                // chat's own checkout; the main chat's same relative path
                // gets its own editor, and each link reuses its editor.
                let file_link = |chat: &str, root: &str| LinkActivation {
                    target: LinkTarget::new("lib", &format!("{root}/src/lib.rs")),
                    action: LinkAction::Primary,
                    source_session: Some(chat.into()),
                };
                assert_eq!(
                    shell.activate_session_link(&file_link("side", "/tmp/other"), window, cx),
                    LinkOutcome::Internal
                );
                let side_editor = shell.file_surface_seq;
                assert_eq!(shell.file_surfaces[&side_editor].read(cx).chat_id(), "side");
                assert_eq!(shell.file_surface_paths[&side_editor], "src/lib.rs");
                assert_eq!(
                    shell.activate_session_link(&file_link("main", "/tmp/main"), window, cx),
                    LinkOutcome::Internal
                );
                let main_editor = shell.file_surface_seq;
                assert_ne!(main_editor, side_editor);
                assert_eq!(shell.file_surfaces[&main_editor].read(cx).chat_id(), "main");
                shell.activate_session_link(&file_link("side", "/tmp/other"), window, cx);
                assert_eq!(shell.file_surface_seq, main_editor, "reused, not reopened");
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::File(side_editor)
                );
                // The side chat's model menu owns Cmd/Ctrl+digit.
                let composer = shell.side_chats[&id].composer.clone();
                assert!(!shell.overlay_owns_keyboard(cx));
                composer.update(cx, |composer, cx| composer.open_model_menu(window, cx));
                assert!(shell.overlay_owns_keyboard(cx));
                assert!(shell.open_side_chat_pickers(cx).is_some());
                // Closing the tab with a draft keeps it for the reopen.
                composer.update(cx, |composer, cx| {
                    composer.stage_appshot(crate::appshots::tests::shot(), cx)
                });
                shell.close_right_surface(RightSurface::SideChat(id), window, cx);
                let listed = |shell: &Shell, cx: &App| {
                    shell
                        .right_surface_rows(cx)
                        .iter()
                        .any(|(surface, ..)| *surface == RightSurface::SideChat(id))
                };
                assert!(!listed(shell, cx));
                assert!(shell.side_chats.contains_key(&id));
                shell.open_side_chat(chat.clone(), shell.panel_key(cx), cx);
                assert_eq!(shell.side_chat_seq, id, "the kept tab is reused");
                assert!(listed(shell, cx));
                assert_eq!(shell.resolved_right_active(cx), RightSurface::SideChat(id));
                assert!(shell.side_chats[&id].composer.read(cx).has_draft(cx));
                // Without a draft, closing drops it.
                let mut other = chat.clone();
                other.id = "side-2".into();
                shell.open_side_chat(other, shell.panel_key(cx), cx);
                let other = shell.side_chat_seq;
                shell.close_right_surface(RightSurface::SideChat(other), window, cx);
                assert!(!shell.side_chats.contains_key(&other));
            })
            .unwrap();
    }

    /// The engine end of a fork: its requests, and the replies the test
    /// gives them.
    struct ForkEngine {
        requests: tokio::sync::mpsc::Receiver<String>,
        replies: tokio::sync::mpsc::Sender<String>,
    }

    impl ForkEngine {
        /// The next fork request; the tabs' standing watches go unanswered.
        fn fork_request(&mut self, cx: &mut TestAppContext) -> Option<serde_json::Value> {
            cx.run_until_parked();
            while let Ok(request) = self.requests.try_recv() {
                let request: serde_json::Value = serde_json::from_str(&request).unwrap();
                if request["method"] == methods::FORK_SIDE_CHAT {
                    return Some(request);
                }
            }
            None
        }

        fn reply(
            &self,
            runtime: &tokio::runtime::Runtime,
            request: &serde_json::Value,
            mut reply: serde_json::Value,
            cx: &mut TestAppContext,
        ) {
            reply["id"] = request["id"].clone();
            runtime.block_on(async {
                self.replies.send(reply.to_string()).await.unwrap();
                while self.replies.capacity() < self.replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            });
            cx.run_until_parked();
        }
    }

    /// A shell on "main", a conversation on the remote host omarchy, with
    /// the right pane open. Forks go to the returned engine.
    fn fork_shell(
        dir: &std::path::Path,
        cx: &mut TestAppContext,
    ) -> (
        gpui::WindowHandle<Shell>,
        tokio::runtime::Runtime,
        ForkEngine,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (out, requests) = tokio::sync::mpsc::channel(256);
        let (replies, inbound) = tokio::sync::mpsc::channel(256);
        let engine = {
            let _guard = runtime.enter();
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound))
        };
        let window = shell_window(dir, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.active_chat = "main".into();
                let main: zeron_proto::Chat = serde_json::from_value(serde_json::json!({
                    "id": "main", "deviceId": "omarchy", "cwd": "/srv/main",
                    "archived": false, "createdAt": Utc::now(),
                }))
                .unwrap();
                shell.state.update(cx, |state, _| {
                    state.local_device_id = Some("local".into());
                    state.devices = vec![zeron_proto::Device {
                        id: "omarchy".into(),
                        name: "omarchy".into(),
                        platform: "linux".into(),
                        last_seen_at: Some(Utc::now()),
                        created_at: None,
                        version: None,
                        cursor_sdk_version: None,
                        capabilities: Vec::new(),
                    }];
                    state.chats = vec![main];
                    state.selected_chat = Some("main".into());
                    state.set_test_engine(engine);
                });
                shell.toggle_right_pane(cx);
            })
            .unwrap();
        (window, runtime, ForkEngine { requests, replies })
    }

    fn pending_fork(shell: &Shell) -> Option<(&PendingFork, u64)> {
        let id = shell.side_chat_seq;
        shell
            .side_chats
            .get(&id)?
            .fork
            .as_ref()
            .map(|fork| (fork, id))
    }

    /// On a slow host the fork used to show nothing until it answered (the
    /// button only dimmed). Its tab opens at once and says what it waits on,
    /// then becomes the side chat in place.
    #[gpui::test]
    fn fork_opens_a_progress_tab_before_the_host_answers(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (window, runtime, mut engine) = fork_shell(dir.path(), cx);
        let _guard = runtime.enter();
        let (tab, fork_id) = window
            .update(cx, |shell, _, cx| {
                shell.create_side_chat(cx);
                let (fork, tab) = pending_fork(shell).expect("the tab opens before any answer");
                assert_eq!(fork.host.as_deref(), Some("omarchy"));
                assert!(fork.error.is_none());
                assert_eq!(shell.resolved_right_active(cx), RightSurface::SideChat(tab));
                let state = shell.side_chats[&tab].state.read(cx);
                assert!(
                    state.side_chat_unsaved(),
                    "nothing to open until the host made it"
                );
                (tab, state.selected_chat.clone().unwrap())
            })
            .unwrap();
        let request = engine.fork_request(cx).expect("fork sent");
        assert_eq!(request["params"]["chatId"], fork_id);
        assert_eq!(request["params"]["parentChatId"], "main");
        assert_eq!(request["params"]["targetDeviceId"], "omarchy");
        let fork = serde_json::json!({
            "id": fork_id, "parentChatId": "main", "deviceId": "omarchy", "cwd": "/srv/main",
            "archived": false, "createdAt": Utc::now(),
        });
        engine.reply(&runtime, &request, serde_json::json!({ "ok": fork }), cx);
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.side_chats.len(), 1);
                let side = &shell.side_chats[&tab];
                assert!(side.fork.is_none());
                let state = side.state.read(cx);
                assert!(!state.side_chat_unsaved());
                assert_eq!(state.selected_chat.as_deref(), Some(fork_id.as_str()));
                assert_eq!(shell.resolved_right_active(cx), RightSurface::SideChat(tab));
            })
            .unwrap();
    }

    /// A failed fork says why in its own tab, not in the conversation's
    /// composer, and retries the same chat (forking is idempotent per id).
    #[gpui::test]
    fn fork_failure_stays_in_its_tab(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (window, runtime, mut engine) = fork_shell(dir.path(), cx);
        let _guard = runtime.enter();
        window
            .update(cx, |shell, _, cx| shell.create_side_chat(cx))
            .unwrap();
        let first = engine.fork_request(cx).expect("fork sent");
        engine.reply(
            &runtime,
            &first,
            serde_json::json!({ "err": "device omarchy is offline" }),
            cx,
        );
        let tab = window
            .update(cx, |shell, _, cx| {
                let (fork, tab) = pending_fork(shell).expect("the failure stays in its tab");
                assert_eq!(fork.error.as_deref(), Some("device omarchy is offline"));
                assert!(shell.composer.read(cx).failure().is_none());
                assert!(!fork.running);
                tab
            })
            .unwrap();
        window
            .update(cx, |shell, _, cx| shell.retry_fork(tab, cx))
            .unwrap();
        let retry = engine.fork_request(cx).expect("retried");
        assert_eq!(
            retry["params"], first["params"],
            "the same fork, not a second one"
        );
        window
            .update(cx, |shell, _, _| {
                let (fork, _) = pending_fork(shell).unwrap();
                assert!(fork.error.is_none(), "retrying clears the failure");
            })
            .unwrap();
    }

    /// Closing the tab while the host works is final: its answer reopens
    /// nothing (the fork still lists under its parent).
    #[gpui::test]
    fn closing_a_pending_fork_keeps_it_closed(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (window, runtime, mut engine) = fork_shell(dir.path(), cx);
        let _guard = runtime.enter();
        let fork_id = window
            .update(cx, |shell, window, cx| {
                shell.create_side_chat(cx);
                let (_, tab) = pending_fork(shell).unwrap();
                let fork_id = shell.side_chats[&tab].state.read(cx).selected_chat.clone();
                shell.close_right_surface(RightSurface::SideChat(tab), window, cx);
                assert!(shell.side_chats.is_empty());
                fork_id.unwrap()
            })
            .unwrap();
        let request = engine.fork_request(cx).expect("fork sent");
        let fork = serde_json::json!({
            "id": fork_id, "parentChatId": "main", "deviceId": "omarchy",
            "archived": false, "createdAt": Utc::now(),
        });
        engine.reply(&runtime, &request, serde_json::json!({ "ok": fork }), cx);
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.side_chats.is_empty());
                assert!(shell.right_surface_rows(cx).is_empty());
            })
            .unwrap();
        // A failure after closing has no tab to land in: it says so where
        // the fork was asked for.
        window
            .update(cx, |shell, window, cx| {
                shell.create_side_chat(cx);
                let (_, tab) = pending_fork(shell).unwrap();
                shell.close_right_surface(RightSurface::SideChat(tab), window, cx);
            })
            .unwrap();
        let request = engine.fork_request(cx).expect("second fork sent");
        engine.reply(
            &runtime,
            &request,
            serde_json::json!({ "err": "fork failed" }),
            cx,
        );
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.side_chats.is_empty());
                assert_eq!(
                    shell.composer.read(cx).failure().map(|f| f.as_ref()),
                    Some("fork failed")
                );
            })
            .unwrap();
    }

    /// Each fork is its own: retrying one never waits on another in flight,
    /// and a retry already in flight is not sent twice.
    #[gpui::test]
    fn retrying_a_fork_never_waits_on_another(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (window, runtime, mut engine) = fork_shell(dir.path(), cx);
        let _guard = runtime.enter();
        window
            .update(cx, |shell, _, cx| shell.create_side_chat(cx))
            .unwrap();
        let first = engine.fork_request(cx).expect("fork sent");
        engine.reply(
            &runtime,
            &first,
            serde_json::json!({ "err": "device omarchy is offline" }),
            cx,
        );
        let failed = window
            .update(cx, |shell, _, _| pending_fork(shell).unwrap().1)
            .unwrap();
        window
            .update(cx, |shell, _, cx| shell.create_side_chat(cx))
            .unwrap();
        let second = engine
            .fork_request(cx)
            .expect("a second fork, left in flight");
        assert_ne!(second["params"]["chatId"], first["params"]["chatId"]);
        window
            .update(cx, |shell, _, cx| shell.retry_fork(failed, cx))
            .unwrap();
        let retry = engine.fork_request(cx).expect("the retry goes out at once");
        assert_eq!(retry["params"], first["params"]);
        window
            .update(cx, |shell, _, cx| shell.retry_fork(failed, cx))
            .unwrap();
        assert!(engine.fork_request(cx).is_none(), "one request per fork");
    }

    /// The host writes the fork's row before it answers, and the answer can
    /// time out after the fork exists: opening the row opens the chat in the
    /// fork's tab, and a late failure changes nothing.
    #[gpui::test]
    fn opening_a_pending_forks_row_opens_the_chat(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (window, runtime, mut engine) = fork_shell(dir.path(), cx);
        let _guard = runtime.enter();
        let (tab, fork_id) = window
            .update(cx, |shell, _, cx| {
                shell.create_side_chat(cx);
                let (_, tab) = pending_fork(shell).unwrap();
                let fork_id = shell.side_chats[&tab].state.read(cx).selected_chat.clone();
                (tab, fork_id.unwrap())
            })
            .unwrap();
        let request = engine.fork_request(cx).expect("fork sent");
        window
            .update(cx, |shell, _, cx| {
                let row: zeron_proto::Chat = serde_json::from_value(serde_json::json!({
                    "id": fork_id, "parentChatId": "main", "deviceId": "omarchy",
                    "cwd": "/srv/main", "archived": false, "createdAt": Utc::now(),
                }))
                .unwrap();
                shell.state.update(cx, |state, _| state.chats.push(row));
                shell.open_child_chat_tab(&fork_id, cx);
                assert_eq!(shell.side_chats.len(), 1);
                let side = &shell.side_chats[&tab];
                assert!(side.fork.is_none(), "the row opens the chat, not the wait");
                assert!(!side.state.read(cx).side_chat_unsaved());
                assert_eq!(shell.resolved_right_active(cx), RightSurface::SideChat(tab));
            })
            .unwrap();
        engine.reply(
            &runtime,
            &request,
            serde_json::json!({ "err": "relay deadline elapsed" }),
            cx,
        );
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.side_chats[&tab].fork.is_none());
                assert!(shell.composer.read(cx).failure().is_none());
            })
            .unwrap();
    }
}
