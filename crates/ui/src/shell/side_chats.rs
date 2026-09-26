use super::*;

pub(super) struct SideChatTab {
    pub state: Entity<AppState>,
    transcript: Entity<Transcript>,
    pub(super) composer: Entity<Composer>,
    _events: Vec<Subscription>,
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
    pub(super) fn fork_chat(
        &mut self,
        source: zeron_proto::Chat,
        parent_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.side_chat_creating {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let key = self.panel_key(cx);
        self.side_chat_creating = true;
        let params = serde_json::json!({
            "chatId": uuid::Uuid::new_v4().to_string(),
            "sourceChatId": source.id,
            "parentChatId": parent_id,
            "targetDeviceId": source.device_id,
        });
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<zeron_proto::Chat>(methods::FORK_SIDE_CHAT, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.side_chat_creating = false;
                match result {
                    Ok(chat) => {
                        if this
                            .state
                            .read(cx)
                            .chats
                            .iter()
                            .any(|c| Some(&c.id) == chat.parent_chat_id.as_ref())
                        {
                            this.open_side_chat(chat, key, cx);
                        }
                    }
                    Err(error) => this.show_side_chat_error(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A fresh, empty side chat under `parent_id` (the active chat when
    /// None), opened in the right pane ready for its first message. Same
    /// shape the Zeron MCP server's `create_chat` mints, so agent-spawned
    /// and hand-started side chats list together.
    pub(super) fn create_child_chat(&mut self, parent_id: Option<String>, cx: &mut Context<Self>) {
        if self.side_chat_creating {
            return;
        }
        let state = self.state.read(cx);
        let parent = match parent_id {
            Some(id) => state.chats.iter().find(|c| c.id == id).cloned(),
            None => state.selected_chat_row().cloned(),
        };
        let Some(parent) = parent else {
            self.show_side_chat_error("Start a conversation before creating a side chat.", cx);
            return;
        };
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let key = self.panel_key(cx);
        let mut chat = parent.clone();
        chat.id = uuid::Uuid::new_v4().to_string();
        chat.parent_chat_id = Some(parent.id.clone());
        chat.title = None;
        chat.archived = false;
        chat.created_at = Utc::now();
        chat.last_message_at = None;
        chat.last_message_preview = None;
        chat.last_seen_at = None;
        chat.harness_session_id = None;
        chat.harness_session_cwd = None;
        chat.room_gen = Some(2);
        let params = serde_json::json!({
            "op": "createChat",
            "chatId": chat.id,
            "deviceId": chat.device_id,
            "spaceId": chat.space_id,
            "config": chat.config,
            "branch": chat.branch,
            "cwd": chat.cwd,
            "parentChatId": parent.id,
        });
        self.side_chat_creating = true;
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            let _ = this.update(cx, |this, cx| {
                this.side_chat_creating = false;
                match result {
                    Ok(_) => this.open_side_chat(chat, key, cx),
                    Err(error) => this.show_side_chat_error(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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

    /// Reveal the surface host of the pane keyed `key`: the live one through
    /// the user-visible path, a background conversation's by flag, so a
    /// fork that finishes after the user switched away opens ITS pane, not
    /// whichever is on screen.
    fn open_surfaces_for(&mut self, key: &str, cx: &mut Context<Self>) {
        if key == self.panel_key(cx) {
            // Programmatic, so an already-open pane is left alone.
            self.set_surfaces_open(true, cx);
        } else {
            self.panels.update(key, |p| p.changes_open = true);
        }
    }

    pub(super) fn open_side_chat(
        &mut self,
        chat: zeron_proto::Chat,
        key: String,
        cx: &mut Context<Self>,
    ) {
        // A footer row or header button may land while the surface host is
        // closed (explorer-only pane, or hidden with its tabs kept): open it
        // beside the explorer first, for an existing tab too.
        self.open_surfaces_for(&key, cx);
        if let Some((&id, _)) = self
            .side_chats
            .iter()
            .find(|(_, tab)| tab.state.read(cx).selected_chat.as_deref() == Some(&chat.id))
        {
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
            return;
        }
        let chat_id = chat.id.clone();
        let parent = self.state.clone();
        let state = cx.new(|cx| AppState::side_chat_state(&parent, chat, cx));
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
                        // A side chat is already minted before its composer mounts,
                        // and it inherits its parent's checkout, so it never runs
                        // worktree setup of its own.
                        ComposerEvent::NewThreadTransitionStarted
                        | ComposerEvent::WorktreeSetup { .. } => {}
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
        self.side_chat_seq += 1;
        let id = self.side_chat_seq;
        self.side_chats.insert(
            id,
            SideChatTab {
                state,
                transcript,
                composer,
                _events: events,
            },
        );
        self.right_tabs
            .entry(key.clone())
            .or_default()
            .push(RightSurface::SideChat(id));
        self.panels
            .update(&key, |p| p.right_active = RightSurface::SideChat(id));
        cx.notify();
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
                let fallback = self
                    .right_tabs
                    .get(&key)
                    .and_then(|tabs| tabs.first())
                    .copied()
                    .unwrap_or(RightSurface::Picker);
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
        let transcript = tab.transcript.clone();
        let composer = tab.composer.clone();
        // Share the main chat's docked width cap and responsive padding.
        let width = composer_target_width(
            self.right_visible_width(cx),
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
        div()
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
}
