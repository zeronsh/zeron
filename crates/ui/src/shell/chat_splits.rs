use super::*;

pub(super) struct ChatSplit {
    pair: [String; 2],
    profile: String,
    inactive_id: String,
    transcripts: [Entity<Transcript>; 2],
    ratio: f32,
}

#[derive(Clone)]
struct SplitResize;

fn valid_pair(pair: &[String; 2]) -> bool {
    !pair[0].is_empty() && !pair[1].is_empty() && pair[0] != pair[1]
}

impl Shell {
    fn split_title(&self, id: &str, cx: &App) -> String {
        self.state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == id)
            .and_then(|chat| chat.title.clone())
            .unwrap_or_else(|| "New session".into())
    }

    fn close_chat_split(&mut self, cx: &mut Context<Self>) {
        if let Some(split) = self.chat_split.take() {
            self.transcript
                .update(cx, |transcript, cx| transcript.set_split_pane(false, cx));
            self.state.update(cx, |state, _| {
                for id in &split.pair {
                    state.unwatch_subagent_doc(id);
                }
            });
        }
        cx.notify();
    }

    fn show_chat_split(&mut self, pair: [String; 2], active: String, cx: &mut Context<Self>) {
        let Some(profile) = self.active_sidebar_pin_profile_key(cx) else {
            return;
        };
        if !valid_pair(&pair)
            || !pair.contains(&active)
            || !pair.iter().all(|id| {
                self.state
                    .read(cx)
                    .visible_chats()
                    .any(|chat| &chat.id == id)
            })
        {
            return;
        }
        let inactive_id = pair.iter().find(|id| **id != active).unwrap().clone();
        if self
            .chat_split
            .as_ref()
            .is_some_and(|split| split.pair == pair)
        {
            if self.state.read(cx).selected_chat.as_ref() == Some(&active) {
                self.focus_composer(cx);
            } else {
                self.open_chat(active, cx);
            }
            self.chat_split.as_mut().unwrap().inactive_id = inactive_id;
            cx.notify();
            return;
        }
        let ratio = self
            .chat_split
            .as_ref()
            .filter(|split| split.pair == pair)
            .map_or(0.5, |split| split.ratio);
        self.close_chat_split(cx);
        self.open_chat(active.clone(), cx);
        self.state.update(cx, |state, cx| {
            for id in &pair {
                state.watch_subagent_doc(id.clone(), cx);
            }
        });
        let transcripts = pair
            .clone()
            .map(|id| cx.new(|cx| Transcript::for_doc(self.state.clone(), id, true, cx)));
        self.chat_split = Some(ChatSplit {
            pair,
            profile,
            inactive_id,
            transcripts,
            ratio,
        });
        self.transcript
            .update(cx, |transcript, cx| transcript.set_split_pane(true, cx));
        cx.notify();
    }

    pub(super) fn create_chat_split(
        &mut self,
        target: String,
        payload: &SidebarSessionDrag,
        cx: &mut Context<Self>,
    ) {
        if self.active_sidebar_pin_profile_key(cx).as_ref() != Some(&payload.profile_key) {
            return;
        }
        let pair = [target.clone(), payload.chat_id.clone()];
        if !valid_pair(&pair)
            || !pair.iter().all(|id| {
                self.state
                    .read(cx)
                    .visible_chats()
                    .any(|chat| &chat.id == id)
            })
        {
            return;
        }
        self.cancel_sidebar_session_transfer(cx);
        let pairs = self
            .settings
            .chat_splits
            .entry(payload.profile_key.clone())
            .or_default();
        let pair = if let Some(saved) = pairs
            .iter()
            .find(|saved| saved.contains(&pair[0]) && saved.contains(&pair[1]))
        {
            saved.clone()
        } else {
            pairs.push(pair.clone());
            self.schedule_save(cx);
            pair
        };
        self.show_chat_split(pair, target, cx);
    }

    pub(super) fn visible_chat_splits(&self, cx: &App) -> Vec<[String; 2]> {
        self.active_sidebar_pin_profile_key(cx)
            .and_then(|key| self.settings.chat_splits.get(&key))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|pair| valid_pair(pair))
            .filter(|pair| {
                pair.iter().all(|id| {
                    self.state
                        .read(cx)
                        .visible_chats()
                        .any(|chat| &chat.id == id)
                })
            })
            .collect()
    }

    /// Keep the inactive pane's composer chrome in the same place as the
    /// selected pane's live composer. A click selects this chat and focuses
    /// the shared editor, which owns drafts, attachments, and send actions.
    fn render_inactive_split_composer(
        &self,
        id: &str,
        pair: [String; 2],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = {
            let state = self.state.read(cx);
            let chat = state.chats.iter().find(|chat| chat.id == id).unwrap();
            chat.config.clone()
        };
        let (draft, model_label) = {
            let composer = self.composer.read(cx);
            let draft = composer.split_draft_preview(id, cx);
            let label = config
                .as_ref()
                .map(|config| composer.pickers().read(cx).model_label_for_chat(config));
            (draft, label)
        };
        let empty = draft.is_empty();
        let (brand, tint) = config
            .as_ref()
            .map(|config| crate::pickers::harness_brand_icon(config.harness))
            .unwrap_or((icons::BOT, None));
        let activate = id.to_string();
        let pill = div()
            .id("split-inactive-composer-pill")
            .debug_selector(|| "split-inactive-composer-pill".into())
            .h(px(crate::composer::COMPACT_TOTAL_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .rounded(px(crate::composer::COMPOSER_RADIUS))
            .border_1()
            .border_color(theme.border)
            .when(theme.is_frost(), |el| el.bg(theme.composer_sidebar_tint()))
            .when(!theme.is_frost(), |el| {
                el.bg(theme.input_glass_bg()).shadow_lg()
            })
            .child(
                icon(icons::PAPERCLIP)
                    .size(px(18.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(14.0))
                    .text_color(if empty { theme.text_faint } else { theme.text })
                    .child(if empty {
                        "Do anything…".to_string()
                    } else {
                        transcript::single_line(&draft)
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .child(
                        icon(brand)
                            .size(px(14.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    )
                    .children(model_label.map(|label| div().truncate().child(label))),
            )
            .child(
                div()
                    .size(px(28.0))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.text)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(icons::ARROW_UP).size(px(14.0)).text_color(theme.bg)),
            );
        let footer =
            crate::pickers::Pickers::session_footer_for_chat(self.state.read(cx), id, theme);
        let footer = div()
            .w_full()
            .h(px(24.0))
            .mb(px(-Theme::SPACE_SM))
            .relative()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .w_full()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .child(div().flex_1().min_w_0().children(footer)),
            );
        div()
            .id("split-inactive-composer")
            .debug_selector(|| "split-inactive-composer".into())
            .w_full()
            .flex_none()
            .cursor_pointer()
            .aria_label("Activate this chat composer")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_chat_split(pair.clone(), activate.clone(), cx)
            }))
            .child(div().h(px(Theme::STATUS_STRIP_HEIGHT)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(Theme::SPACE_SM))
                    .px(px(Theme::SPACE_LG))
                    .pb(px(Theme::SPACE_LG))
                    .child(crate::frost::frosted(
                        crate::composer::COMPOSER_RADIUS,
                        16.0,
                        pill,
                    ))
                    .child(footer),
            )
            .into_any_element()
    }

    pub(super) fn render_chat_split_links(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pairs = self.visible_chat_splits(cx);
        if pairs.is_empty() {
            return Empty.into_any_element();
        }
        let blue: gpui::Hsla = gpui::rgb(0x4d76f6).into();
        let selected_chat = self.state.read(cx).selected_chat.clone();
        let mut list = div().flex().flex_col().gap(px(12.0));
        for pair in pairs {
            let key = format!("{}-{}", pair[0], pair[1]);
            let active_index = self
                .chat_split
                .as_ref()
                .filter(|split| split.pair == pair)
                .and_then(|_| selected_chat.as_ref())
                .and_then(|id| pair.iter().position(|pane| pane == id));
            let remove = pair.clone();
            let header = div()
                .id(SharedString::from(format!("split-header-{key}")))
                .flex()
                .items_center()
                .gap(px(5.0))
                .h(px(24.0))
                .px(px(Theme::SPACE_SM))
                .text_size(crate::typography::ui_rems(11.0))
                .text_color(blue)
                .child(div().flex().gap(px(1.0)).children((0..2).map(|index| {
                    div()
                        .id(SharedString::from(format!("split-icon-{key}-{index}")))
                        .debug_selector({
                            let key = key.clone();
                            move || format!("split-icon-{key}-{index}")
                        })
                        .w(px(5.0))
                        .h(px(9.0))
                        .rounded(px(1.0))
                        .bg(blue.opacity(if active_index == Some(index) {
                            1.0
                        } else {
                            0.3
                        }))
                })))
                .child("Split view")
                .child(div().h(px(1.0)).flex_1().bg(blue.opacity(0.35)))
                .child(
                    div()
                        .id(SharedString::from(format!("remove-split-{key}")))
                        .aria_label("Remove split view")
                        .cursor_pointer()
                        .text_color(theme.text_muted)
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(profile) = this.active_sidebar_pin_profile_key(cx) {
                                if let Some(pairs) = this.settings.chat_splits.get_mut(&profile) {
                                    pairs.retain(|pair| pair != &remove);
                                }
                                if this
                                    .chat_split
                                    .as_ref()
                                    .is_some_and(|split| split.pair == remove)
                                {
                                    this.close_chat_split(cx);
                                }
                                this.schedule_save(cx);
                                cx.notify();
                            }
                        })),
                );
            let mut rows = div().flex().flex_col().gap(px(SIDEBAR_LIST_GAP));
            for (index, id) in pair.iter().enumerate() {
                let (title, location, time_ago) = {
                    let state = self.state.read(cx);
                    let chat = state.chats.iter().find(|chat| &chat.id == id).unwrap();
                    let project = chat
                        .space_id
                        .as_deref()
                        .and_then(|space| state.space_row(space))
                        .map_or("~".to_string(), |space| space.display_name().to_string());
                    let device = state
                        .device_name(&chat.device_id)
                        .unwrap_or("Unknown device");
                    (
                        transcript::single_line(
                            &chat.title.clone().unwrap_or_else(|| "New session".into()),
                        ),
                        format!("{project} @ {device}"),
                        format_time_ago(
                            chat.last_message_at.unwrap_or(chat.created_at),
                            Utc::now(),
                        ),
                    )
                };
                let row_key = format!("split-session-{key}-{index}");
                let open = pair.clone();
                let open_id = id.clone();
                let menu_id = id.clone();
                let target_id = id.clone();
                let drag_title: SharedString = title.clone().into();
                let drag =
                    self.active_sidebar_pin_profile_key(cx)
                        .map(|profile_key| SidebarSessionDrag {
                            chat_id: id.clone(),
                            visible_ids: std::sync::Arc::new(Vec::new()),
                            filter: self.settings.space_filter.clone(),
                            profile_key,
                        });
                let selected = active_index == Some(index);
                let row = div()
                    .id(SharedString::from(row_key.clone()))
                    .debug_selector({
                        let row_key = row_key.clone();
                        move || row_key.clone()
                    })
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(Theme::SPACE_SM))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .when(selected, |el| el.bg(crate::theme::glass_selected_bg()))
                    .hover(|style| {
                        style.bg(if selected {
                            crate::theme::glass_selected_bg()
                        } else {
                            theme.glass_hover()
                        })
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_chat_split(open.clone(), open_id.clone(), cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.chat_menu.open(ChatMenuState {
                                chat_id: menu_id.clone(),
                                position: event.position,
                                page: ChatMenuPage::Root,
                            });
                            cx.notify();
                        }),
                    )
                    .on_drop::<SidebarSessionDrag>(cx.listener(move |this, payload, _, cx| {
                        cx.stop_propagation();
                        this.create_chat_split(target_id.clone(), payload, cx);
                    }))
                    .when_some(drag, |el, payload| {
                        el.on_drag(payload, move |_, _, _, cx| {
                            cx.new(|_| SidebarSessionGhost {
                                title: drag_title.clone(),
                            })
                        })
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_size(crate::typography::ui_rems(10.0))
                            .text_color(theme.text_muted)
                            .child(div().size(px(7.0)).rounded(px(2.0)).bg(blue))
                            .child(div().min_w_0().flex_1().truncate().child(location))
                            .child(time_ago),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text)
                            .child(title),
                    );
                rows = rows.child(row);
            }
            list = list.child(
                div()
                    .id(SharedString::from(format!("split-{key}")))
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(rows),
            );
        }
        div()
            .id("sidebar-split-views")
            .debug_selector(|| "sidebar-split-views".into())
            .flex()
            .flex_col()
            .pt(px(SIDEBAR_LIST_PAD_TOP))
            .child(list)
            .into_any_element()
    }
    pub(super) fn render_chat_workspace(
        &mut self,
        window: &mut Window,
        width: f32,
        transcript_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.state.read(cx).selected_chat.clone();
        let invalid = self.chat_split.as_ref().is_some_and(|split| {
            self.active_sidebar_pin_profile_key(cx).as_ref() != Some(&split.profile)
                || !selected.as_ref().is_some_and(|id| split.pair.contains(id))
                || !split.pair.iter().all(|id| {
                    self.state
                        .read(cx)
                        .visible_chats()
                        .any(|chat| &chat.id == id)
                })
        });
        if invalid {
            self.close_chat_split(cx);
        }
        if !matches!(self.route, Route::Chat) || self.chat_split.is_none() {
            let main = self.render_main(window, width, transcript_width, cx);
            if !matches!(self.route, Route::Chat) || selected.is_none() {
                return main;
            }
            return self.render_thread_split_drop_target(main, selected.unwrap(), cx);
        }
        // Navigation via the normal sidebar must also retarget the passive watch.
        if self
            .chat_split
            .as_ref()
            .is_some_and(|split| Some(&split.inactive_id) == selected.as_ref())
        {
            let pair = self.chat_split.as_ref().unwrap().pair.clone();
            self.show_chat_split(pair, selected.clone().unwrap(), cx);
        }
        let split = self.chat_split.as_ref().unwrap();
        let pair = split.pair.clone();
        let inactive = split.inactive_id.clone();
        let transcript = split.transcripts[usize::from(inactive == pair[1])].clone();
        let (inactive_loaded, inactive_empty) = {
            let state = self.state.read(cx);
            (
                state.sub_transcript_loaded(&inactive),
                state.sub_transcript(&inactive).is_empty(),
            )
        };
        let ratio = split.ratio;
        let active_width = (width - 4.0)
            * if selected.as_ref() == Some(&pair[0]) {
                ratio
            } else {
                1.0 - ratio
            };
        let theme = Theme::of(cx).clone();
        let main = self.render_main(window, active_width, transcript_width.min(active_width), cx);
        let mut main = Some(main);
        let mut row = div()
            .id("chat-split-workspace")
            .debug_selector(|| "chat-split-workspace".into())
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .on_drag_move::<SplitResize>(cx.listener(
                |this, event: &gpui::DragMoveEvent<SplitResize>, _, cx| {
                    let width = f32::from(event.bounds.size.width);
                    if width > 0.0 {
                        if let Some(split) = this.chat_split.as_mut() {
                            split.ratio = (f32::from(event.event.position.x - event.bounds.left())
                                / width)
                                .clamp(0.25, 0.75);
                            cx.notify();
                        }
                    }
                },
            ));
        for (index, id) in pair.iter().enumerate() {
            let active = selected.as_ref() == Some(id);
            let activate = id.clone();
            let activate_pair = pair.clone();
            let title = self.split_title(id, cx);
            let header = div()
                .id(SharedString::from(format!("split-pane-{}", id)))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(10.0))
                .h(px(32.0))
                .flex_none()
                .bg(theme.surface_raised)
                .text_color(theme.text)
                .text_size(crate::typography::ui_rems(12.0))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_chat_split(activate_pair.clone(), activate.clone(), cx)
                }))
                .child(div().flex_1().min_w_0().overflow_hidden().child(format!(
                    "{}{}",
                    if active { "●  " } else { "" },
                    title
                )))
                .child(
                    div()
                        .id(SharedString::from(format!("close-split-pane-{}", id)))
                        .child("×")
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_chat_split(cx);
                        })),
                );
            let body = if active {
                main.take().unwrap()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .child(transcript.clone())
                            .when(inactive_empty, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(theme.text_muted)
                                        .text_size(crate::typography::ui_rems(12.0))
                                        .child(if inactive_loaded {
                                            "No messages in this session yet"
                                        } else {
                                            "Loading conversation…"
                                        }),
                                )
                            }),
                    )
                    .child(self.render_inactive_split_composer(id, pair.clone(), &theme, cx))
                    .into_any_element()
            };
            let body = self.render_split_pane_drop_target(body, id.clone(), cx);
            if index == 1 {
                row = row.child(
                    div()
                        .id("chat-split-resize")
                        .w(px(4.0))
                        .h_full()
                        .flex_none()
                        .cursor_ew_resize()
                        .bg(theme.border)
                        .on_drag(SplitResize, |_, _, _, cx| cx.new(|_| DragGhost)),
                );
            }
            row = row.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(px(
                        (width - 4.0) * if index == 0 { ratio } else { 1.0 - ratio }
                    ))
                    .min_w_0()
                    .h_full()
                    .pt(px(Theme::TITLEBAR_HEIGHT))
                    .border_r_1()
                    .border_color(theme.border)
                    .child(header)
                    .child(body),
            );
        }
        row.into_any_element()
    }

    fn render_split_pane_drop_target(
        &self,
        body: AnyElement,
        target: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover_target = target.clone();
        let overlay_target = target.clone();
        div()
            .id(SharedString::from(format!("split-pane-drop-{target}")))
            .debug_selector({
                let id = target.clone();
                move || format!("split-pane-drop-{id}")
            })
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .on_drop::<SidebarSessionDrag>(cx.listener(move |this, payload, _, cx| {
                cx.stop_propagation();
                this.create_chat_split(target.clone(), payload, cx);
            }))
            .child(body)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(0.0)
                    .bg(Theme::of(cx).surface_raised.opacity(0.85))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(Theme::of(cx).text)
                    .on_drop::<SidebarSessionDrag>(cx.listener(move |this, payload, _, cx| {
                        cx.stop_propagation();
                        this.create_chat_split(overlay_target.clone(), payload, cx);
                    }))
                    .drag_over::<SidebarSessionDrag>(move |style, payload, _, _| {
                        if payload.chat_id != hover_target {
                            style.opacity(1.0)
                        } else {
                            style
                        }
                    })
                    .child("Drop to open chats side by side"),
            )
            .into_any_element()
    }

    fn render_thread_split_drop_target(
        &self,
        main: AnyElement,
        target: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover_target = target.clone();
        let overlay_target = target.clone();
        div()
            .id("chat-split-drop-target")
            .debug_selector(|| "chat-split-drop-target".into())
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .on_drop::<SidebarSessionDrag>(cx.listener(move |this, payload, _, cx| {
                cx.stop_propagation();
                this.create_chat_split(target.clone(), payload, cx);
            }))
            .child(main)
            .child(
                div()
                    .id("chat-split-drop-overlay")
                    .absolute()
                    .inset_0()
                    .opacity(0.0)
                    .bg(Theme::of(cx).surface_raised.opacity(0.85))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(Theme::of(cx).text)
                    .on_drop::<SidebarSessionDrag>(cx.listener(move |this, payload, _, cx| {
                        cx.stop_propagation();
                        this.create_chat_split(overlay_target.clone(), payload, cx);
                    }))
                    .drag_over::<SidebarSessionDrag>(move |style, payload, _, _| {
                        if payload.chat_id != hover_target {
                            style.opacity(1.0)
                        } else {
                            style
                        }
                    })
                    .child("Drop to open chats side by side"),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::valid_pair;

    #[gpui::test]
    fn dropping_session_row_on_thread_opens_saved_split(cx: &mut gpui::TestAppContext) {
        use super::*;

        struct SplitDropHost {
            shell: Entity<Shell>,
            observation: Option<Subscription>,
        }
        impl Render for SplitDropHost {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                if self.observation.is_none() {
                    self.observation = Some(cx.observe(&self.shell, |_, _, cx| cx.notify()));
                }
                self.shell.update(cx, |shell, cx| {
                    let payload = SidebarSessionDrag {
                        chat_id: "dragged".into(),
                        visible_ids: std::sync::Arc::new(vec![]),
                        filter: None,
                        profile_key: shell.active_sidebar_pin_profile_key(cx).unwrap(),
                    };
                    div()
                        .w(px(700.0))
                        .h(px(400.0))
                        .flex()
                        .child(
                            div()
                                .w(px(150.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .id("split-test-session-row")
                                        .debug_selector(|| "split-test-session-row".into())
                                        .w(px(150.0))
                                        .child(shell.render_chat_row(
                                            "dragged".into(),
                                            "Dragged session".into(),
                                            "now".into(),
                                            "Project".into(),
                                            None,
                                            vec![],
                                            vec![],
                                            None,
                                            zeron_proto::ChatIndicator::Idle,
                                            false,
                                            false,
                                            false,
                                            Some(payload),
                                            None,
                                            None,
                                            false,
                                            false,
                                            &Theme::default(),
                                            cx,
                                        )),
                                )
                                .child(shell.render_chat_split_links(&Theme::default(), cx)),
                        )
                        .child(shell.render_thread_split_drop_target(
                            div().size_full().child("Current thread").into_any_element(),
                            "current".into(),
                            cx,
                        ))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let (host, cx) = cx.add_window_view(|_, cx| SplitDropHost {
            shell: cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let shell = Shell::new(
                    state.clone(),
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
                );
                state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.chats = ["current", "dragged"]
                        .into_iter()
                        .map(|id| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "title": id, "deviceId": "local", "archived": false,
                                "createdAt": Utc::now(),
                            }))
                            .unwrap()
                        })
                        .collect();
                    state.selected_chat = Some("current".into());
                });
                shell
            }),
            observation: None,
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        let from = cx.debug_bounds("split-test-session-row").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        assert!(cx.debug_bounds("sidebar-session-drag-ghost").is_some());
        let target = cx.debug_bounds("chat-split-drop-target").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        let ghost = cx.debug_bounds("sidebar-session-drag-ghost").unwrap();
        assert!(
            ghost.center().x > px(150.0),
            "dragged session preview must leave the sidebar: {ghost:?}"
        );
        assert!(
            f32::from(ghost.center().x - target.x).abs() < 130.0,
            "dragged session preview must follow the pointer: ghost={ghost:?}, target={target:?}"
        );
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.chat_split.as_ref().unwrap().pair,
                ["current", "dragged"]
            );
            let profile = shell.active_sidebar_pin_profile_key(cx).unwrap();
            assert_eq!(
                shell.settings.chat_splits[&profile],
                vec![["current".to_string(), "dragged".to_string(),]]
            );
        });
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.visible_chat_splits(cx),
                vec![["current".to_string(), "dragged".to_string(),]]
            );
        });
        assert!(cx.debug_bounds("split-icon-current-dragged-0").is_some());
        assert!(cx.debug_bounds("split-icon-current-dragged-1").is_some());
        let second = cx
            .debug_bounds("split-session-current-dragged-1")
            .unwrap()
            .center();
        cx.simulate_mouse_down(second, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(second, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.state.read(cx).selected_chat.as_deref(),
                Some("dragged")
            );
            assert_eq!(shell.chat_split.as_ref().unwrap().inactive_id, "current");
        });
        let first = cx
            .debug_bounds("split-session-current-dragged-0")
            .unwrap()
            .center();
        cx.simulate_mouse_down(first, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(first, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.state.read(cx).selected_chat.as_deref(),
                Some("current")
            );
            assert_eq!(shell.chat_split.as_ref().unwrap().inactive_id, "dragged");
        });
    }

    #[gpui::test]
    fn split_workspace_paints_the_second_sessions_messages(cx: &mut gpui::TestAppContext) {
        use super::*;
        struct SplitWorkspaceHost {
            shell: Entity<Shell>,
            observation: Option<Subscription>,
        }
        impl Render for SplitWorkspaceHost {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                if self.observation.is_none() {
                    self.observation = Some(cx.observe(&self.shell, |_, _, cx| cx.notify()));
                }
                self.shell.update(cx, |shell, cx| {
                    div()
                        .w(px(1200.0))
                        .h(px(600.0))
                        .child(shell.render_chat_workspace(window, 1024.0, 1024.0, cx))
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
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
            SplitWorkspaceHost {
                shell,
                observation: None,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        shell.update(cx, |shell, cx| {
            shell.reduced_motion = true;
            shell.state.update(cx, |state, cx| {
                state.workspace_scope = Some(WorkspaceScope::Local);
                state.local_device_id = Some("local".into());
                state.spaces = vec![zeron_proto::Space {
                    id: "repo".into(),
                    device_id: "local".into(),
                    path: "/repo".into(),
                    name: None,
                    git_detected: true,
                    git_checked_at: None,
                    checkout_id: None,
                    created_at: Utc::now(),
                }];
                state.chats = ["current", "dragged"]
                    .into_iter()
                    .map(|id| {
                        serde_json::from_value(serde_json::json!({
                            "id": id, "title": id, "deviceId": "local", "archived": false,
                            "spaceId": "repo", "cwd": "/repo", "branch": "main",
                            "createdAt": Utc::now(),
                        }))
                        .unwrap()
                    })
                    .collect();
                cx.notify();
            });
            let payload = SidebarSessionDrag {
                chat_id: "dragged".into(),
                visible_ids: std::sync::Arc::new(vec![]),
                filter: None,
                profile_key: shell.active_sidebar_pin_profile_key(cx).unwrap(),
            };
            shell.create_chat_split("current".into(), &payload, cx);
        });
        let state = shell.read_with(cx, |shell, _| shell.state.clone());
        state.update(cx, |state, cx| {
            state.set_subagent_snapshot(
                "dragged".into(),
                vec![zeron_doc::SessionMessageEntry {
                    id: "message".into(),
                    role: zeron_doc::MessageRole::User,
                    parts: vec![zeron_doc::MessagePart::Text {
                        id: "text".into(),
                        text: "A second conversation".into(),
                    }],
                    created_at: 0,
                    device_id: "local".into(),
                    status: None,
                    continuation_of: None,
                    duration_ms: None,
                }],
            );
            state.transcript = state.sub_transcript("dragged").to_vec();
            state.set_subagent_snapshot("current".into(), state.transcript.clone());
            state.transcript_replayed = true;
            cx.notify();
        });
        let bounds = cx
            .debug_bounds("chat-split-workspace")
            .expect("split workspace laid out");
        assert!(
            f32::from(bounds.size.height) > 100.0,
            "split workspace height: {bounds:?}"
        );
        assert!(cx.debug_bounds("split-pane-drop-dragged").is_some());
        let active_message = cx
            .debug_bounds("transcript-content-current-0")
            .expect("active message painted");
        let inactive_message = cx
            .debug_bounds("transcript-content-dragged-0")
            .expect("inactive message painted");
        assert!(
            (f32::from(active_message.top() - inactive_message.top())).abs() < 1.0,
            "message tops must match: {active_message:?} vs {inactive_message:?}"
        );
        let inactive_pill = cx
            .debug_bounds("split-inactive-composer-pill")
            .expect("inactive pane has the same composer chrome");
        let active_pill = cx
            .debug_bounds("composer-surface")
            .expect("selected pane has the live composer");
        assert!(
            f32::from(inactive_pill.top() - active_pill.top()).abs() < 2.0,
            "composer tops must align: inactive={inactive_pill:?}, active={active_pill:?}"
        );
        assert!(
            f32::from(inactive_pill.size.height - active_pill.size.height).abs() < 2.0,
            "composer pills must have equal height: inactive={inactive_pill:?}, active={active_pill:?}"
        );
        let footer_selectors = [
            [
                "session-footer-checkout-current",
                "session-footer-branch-current",
            ],
            [
                "session-footer-checkout-dragged",
                "session-footer-branch-dragged",
            ],
        ];
        let footer_before = footer_selectors.map(|parts| {
            parts.map(|selector| {
                cx.debug_bounds(selector)
                    .expect("both session footers show checkout and branch")
            })
        });
        cx.run_until_parked();
        let transcript = shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.state.read(cx).sub_transcript("dragged").len(), 1);
            assert_eq!(shell.chat_split.as_ref().unwrap().inactive_id, "dragged");
            shell.chat_split.as_ref().unwrap().transcripts[1].clone()
        });
        transcript.read_with(cx, |transcript, _| {
            assert!(
                transcript.row_count() > 0,
                "second pane must build its messages"
            );
            assert!(
                transcript.painted_row_count() > 0,
                "second pane must paint its messages"
            );
        });
        let inactive_composer = cx.debug_bounds("split-inactive-composer").unwrap().center();
        cx.simulate_mouse_down(
            inactive_composer,
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            inactive_composer,
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.state.read(cx).selected_chat.as_deref(),
                Some("dragged")
            );
            assert_eq!(shell.chat_split.as_ref().unwrap().inactive_id, "current");
            assert_eq!(shell.state.read(cx).sub_transcript("dragged").len(), 1);
        });
        assert!(cx.debug_bounds("split-inactive-composer-pill").is_some());
        // Reproduce the selected chat's asynchronous message replay, then
        // measure the actual message content on both sides of the switch.
        for expected in ["dragged", "current"] {
            if expected == "current" {
                let target = cx.debug_bounds("split-inactive-composer").unwrap().center();
                cx.simulate_mouse_down(target, MouseButton::Left, gpui::Modifiers::default());
                cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
            }
            state.update(cx, |state, cx| {
                assert_eq!(state.selected_chat.as_deref(), Some(expected));
                state.transcript = state.sub_transcript(expected).to_vec();
                state.transcript_replayed = true;
                cx.notify();
            });
            for (selector, before) in [
                ("transcript-content-current-0", active_message),
                ("transcript-content-dragged-0", inactive_message),
            ] {
                let after = cx
                    .debug_bounds(selector)
                    .expect("message stays visible after switch");
                assert!(
                    f32::from(after.top() - before.top()).abs() < 1.0,
                    "switching to {expected} moved {selector}: {before:?} -> {after:?}"
                );
            }
        }
        for (chat_index, selectors) in footer_selectors.into_iter().enumerate() {
            for (part_index, selector) in selectors.into_iter().enumerate() {
                let before = footer_before[chat_index][part_index];
                let after = cx.debug_bounds(selector).unwrap();
                assert!(
                    f32::from(before.left() - after.left()).abs() < 1.0
                        && f32::from(before.top() - after.top()).abs() < 1.0
                        && f32::from(before.size.width - after.size.width).abs() < 1.0
                        && f32::from(before.size.height - after.size.height).abs() < 1.0,
                    "the {selector} label must stay put when the pane activates: {before:?} -> {after:?}"
                );
            }
        }
    }

    #[gpui::test]
    fn split_lifecycle_keeps_selection_and_saved_pairs_consistent(cx: &mut gpui::TestAppContext) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
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
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.chats = ["a", "b"]
                        .into_iter()
                        .map(|id| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "title": id, "deviceId": "local", "archived": false,
                                "createdAt": Utc::now(),
                            }))
                            .unwrap()
                        })
                        .collect();
                });
                let profile = shell.active_sidebar_pin_profile_key(cx).unwrap();
                let mut payload = SidebarSessionDrag {
                    chat_id: "b".into(),
                    visible_ids: std::sync::Arc::new(vec![]),
                    filter: None,
                    profile_key: profile.clone(),
                };
                shell.create_chat_split("a".into(), &payload, cx);
                assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("a"));
                assert_eq!(shell.chat_split.as_ref().unwrap().inactive_id, "b");
                assert_eq!(shell.sidebar_visible_order(cx), vec!["a", "b"]);
                assert!(
                    shell
                        .render_active_rows(&Theme::default(), cx)
                        .rows
                        .is_empty(),
                    "split sessions should live in their split group only"
                );
                shell.chat_split.as_mut().unwrap().ratio = 0.65;
                shell.show_chat_split(["a".into(), "b".into()], "b".into(), cx);
                assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("b"));
                assert_eq!(shell.chat_split.as_ref().unwrap().inactive_id, "a");
                assert_eq!(shell.chat_split.as_ref().unwrap().ratio, 0.65);
                payload.chat_id = "a".into();
                shell.create_chat_split("b".into(), &payload, cx);
                assert_eq!(shell.settings.chat_splits[&profile].len(), 1);
                shell.close_chat_split(cx);
                assert!(shell.chat_split.is_none());
                assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("b"));
                shell.create_chat_split("a".into(), &payload, cx);
                assert!(shell.chat_split.is_none(), "self-drop must be ignored");
                payload.chat_id = "missing".into();
                shell.create_chat_split("a".into(), &payload, cx);
                assert!(shell.chat_split.is_none(), "stale chat must be ignored");
                payload.chat_id = "b".into();
                payload.profile_key = "another-profile".into();
                shell.create_chat_split("a".into(), &payload, cx);
                assert!(
                    shell.chat_split.is_none(),
                    "cross-profile drop must be ignored"
                );
                let restored: UiSettings =
                    serde_json::from_str(&serde_json::to_string(&shell.settings).unwrap()).unwrap();
                assert_eq!(
                    restored.chat_splits[&profile],
                    vec![["a".to_string(), "b".to_string()]]
                );
            })
            .unwrap();
    }

    #[test]
    fn split_requires_two_distinct_chats() {
        assert!(valid_pair(&["a".into(), "b".into()]));
        assert!(!valid_pair(&["a".into(), "a".into()]));
        assert!(!valid_pair(&[String::new(), "b".into()]));
    }
}
