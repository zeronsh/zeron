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

    pub(super) fn create_side_chat(&mut self, cx: &mut Context<Self>) {
        if self.side_chat_creating {
            return;
        }
        let state = self.state.read(cx);
        let Some(source) = state.selected_chat_row().cloned() else {
            self.side_chat_error = Some("Start a conversation before creating a side chat.".into());
            cx.notify();
            return;
        };
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let key = self.panel_key(cx);
        self.side_chat_creating = true;
        self.side_chat_error = None;
        let params = serde_json::json!({
            "chatId": uuid::Uuid::new_v4().to_string(),
            "sourceChatId": source.id,
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
                    Err(error) => this.side_chat_error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_side_chat(&mut self, chat: zeron_proto::Chat, key: String, cx: &mut Context<Self>) {
        if let Some((&id, _)) = self
            .side_chats
            .iter()
            .find(|(_, tab)| tab.state.read(cx).selected_chat.as_deref() == Some(&chat.id))
        {
            self.set_right_active(RightSurface::SideChat(id), cx);
            return;
        }
        let parent = self.state.clone();
        let state = cx.new(|cx| AppState::side_chat_state(&parent, chat, cx));
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        let events = vec![
            cx.subscribe(&transcript, Self::on_transcript_event),
            cx.subscribe(&composer, {
                let transcript = transcript.clone();
                move |_: &mut Self, _, event, cx| {
                    transcript.update(cx, |t, cx| match event {
                        // A side chat is already minted before its composer mounts.
                        ComposerEvent::NewThreadTransitionStarted => {}
                        ComposerEvent::Sent {
                            chat_id,
                            message_id,
                        } => t.on_own_send(chat_id.clone(), message_id.clone(), cx),
                        ComposerEvent::Queued {
                            chat_id,
                            message_id,
                        } => t.on_own_queued_send(chat_id.clone(), message_id.clone(), cx),
                    })
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

    pub(super) fn close_side_chat_history(&mut self, cx: &mut Context<Self>) {
        if self.side_chat_history_popup.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.side_chat_history_popup);
            cx.notify();
        }
    }

    pub(super) fn side_chat_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let mut chats: Vec<_> = state
            .chats
            .iter()
            .filter(|chat| {
                chat.parent_chat_id.is_some() && chat.parent_chat_id == state.selected_chat
            })
            .cloned()
            .collect();
        chats
            .sort_by_key(|chat| std::cmp::Reverse(chat.last_message_at.unwrap_or(chat.created_at)));
        let theme = Theme::of(cx).clone();
        if chats.is_empty() {
            return gpui::Empty.into_any_element();
        }
        let mut trigger = div()
            .id("side-chat-history-button")
            .role(gpui::Role::Button)
            .aria_label("Side chat history")
            .tooltip(|_, cx| {
                cx.new(|_| SurfaceTabTooltip {
                    text: "Side chat history".into(),
                })
                .into()
            })
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .h(px(20.0))
            .gap(px(5.0))
            .px(px(7.0))
            .rounded(px(6.0))
            .bg(theme.text_muted.opacity(0.08))
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_muted.opacity(0.85))
            .cursor_pointer()
            .hover(|s| {
                s.bg(theme.text_muted.opacity(0.16))
                    .text_color(theme.text_muted)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.side_chat_history_popup.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.side_chat_history_popup.take_press_was_open() {
                    this.close_side_chat_history(cx);
                } else {
                    this.side_chat_history_popup.open(());
                    cx.notify();
                }
            }))
            .child(
                icon(icons::CHAT_ROUND_LINE)
                    .size(px(11.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.85)),
            )
            .child(
                div()
                    .font_family(theme.font_mono.clone())
                    .child(chats.len().to_string()),
            );
        if self.side_chat_history_popup.get().is_some() {
            let mut list = div()
                .id("side-chat-history-list")
                .max_h(px(280.0))
                .overflow_y_scroll()
                .flex()
                .flex_col();
            for chat in chats {
                let title = chat
                    .title
                    .clone()
                    .or(chat.last_message_preview.clone())
                    .unwrap_or_else(|| "New side chat".into());
                let time =
                    format_time_ago(chat.last_message_at.unwrap_or(chat.created_at), Utc::now());
                let menu_id = chat.id.clone();
                list = list.child(
                    popover::menu_row(&theme, false, format!("side-chat-{}", chat.id))
                        .id(SharedString::from(format!("side-chat-{}", chat.id)))
                        .flex_none()
                        .child(
                            icon(icons::CHAT_ROUND_LINE)
                                .size(px(15.0))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(time),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_side_chat_history(cx);
                            cx.stop_propagation();
                            this.open_side_chat(chat.clone(), this.panel_key(cx), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_side_chat_history(cx);
                                this.chat_menu.open(ChatMenuState {
                                    tab: None,
                                    chat_id: menu_id.clone(),
                                    position: event.position,
                                    page: ChatMenuPage::Root,
                                });
                                cx.notify();
                            }),
                        ),
                );
            }
            let menu = popover::popover_card(&theme)
                .w(px(300.0))
                .flex()
                .flex_col()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_side_chat_history(cx)))
                .child(list)
                .into_any_element();
            trigger = trigger.child(popover::anchored_menu_above_end(
                "side-chat-history-popover",
                menu,
                self.side_chat_history_popup.closing_since(),
            ));
        }
        div()
            .absolute()
            // Match the composer's footer row, including its negative bottom margin.
            .bottom(px(Theme::SPACE_LG - Theme::SPACE_SM))
            .h(px(crate::composer::SESSION_FOOTER_HEIGHT))
            .flex()
            .items_center()
            .right(px(Theme::SPACE_LG))
            .child(trigger)
            .into_any_element()
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
}
