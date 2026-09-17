//! Global action and conversation search, using the sidebar's conversation rows.
use super::*;

const HISTORY_RESULT_LIMIT: usize = 30;
const RESULTS_SCROLL_GUTTER: f32 = popover::CARD_INSET;

pub(super) struct CommandPalette {
    search: Entity<ComposerInput>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    active: usize,
    // Claim focus during mount so the shell does not restore the composer
    // while this input is still absent from the dispatch tree.
    focus_pending: bool,
    scroll: gpui::ScrollHandle,
    _search_events: Subscription,
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    NewChat,
    NewProject,
    ImportClaude,
    Settings,
    Chat(String),
}

impl Entry {
    fn action(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::NewChat => Some(("New chat", icons::PEN_NEW_SQUARE)),
            Self::NewProject => Some(("New project", icons::FOLDER)),
            Self::ImportClaude => Some(("Import Claude Code session", icons::PEN_NEW_SQUARE)),
            Self::Settings => Some(("Open settings", icons::SETTINGS_MINIMALISTIC)),
            Self::Chat(_) => None,
        }
    }
}

fn matches_query(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query.split_whitespace().all(|word| text.contains(word))
}

fn actions_for(query: &str) -> Vec<Entry> {
    [
        Entry::NewChat,
        Entry::NewProject,
        Entry::ImportClaude,
        Entry::Settings,
    ]
    .into_iter()
    .filter(|entry| matches_query(query, entry.action().unwrap().0))
    .collect()
}

impl Shell {
    pub(super) fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette.is_some() {
            self.close_command_palette(window, cx);
            return;
        }
        self.add_space = None;
        let search = cx.new(|cx| {
            ComposerInput::with_context("Type a command or search chats…", "PaletteSearch", cx)
        });
        let events = cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                if let Some(palette) = this.command_palette.as_mut() {
                    palette.active = 0;
                    palette.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
                }
                cx.notify();
            }
        });
        let previous_focus = window.focused(cx);
        self.command_palette = Some(CommandPalette {
            search,
            focus: cx.focus_handle(),
            previous_focus,
            active: 0,
            focus_pending: true,
            scroll: gpui::ScrollHandle::new(),
            _search_events: events,
        });
        cx.notify();
    }

    pub(super) fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.command_palette.take() {
            if let Some(focus) = palette.previous_focus {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }

    fn command_entries(&self, cx: &App) -> Vec<Entry> {
        let Some(palette) = &self.command_palette else {
            return Vec::new();
        };
        let query = palette.search.read(cx).text().trim().to_lowercase();
        let mut entries = actions_for(&query);
        let state = self.state.read(cx);
        // Global history deliberately ignores the sidebar's project filter and
        // collapsed groups. Archived conversations remain searchable too.
        let mut chats: Vec<_> = state
            .chats
            .iter()
            .filter(|chat| {
                let project = state
                    .space_for_chat(chat)
                    .map(|s| s.display_name())
                    .unwrap_or("~");
                let device = state.device_name(&chat.device_id).unwrap_or("");
                let branch =
                    crate::change_requests::conversation_branch(chat, &state.spaces).unwrap_or("");
                let pr = state
                    .change_request_for_chat(chat)
                    .map(|pr| {
                        format!(
                            "#{} {} {} {}",
                            pr.number, pr.title, pr.head_ref, pr.base_ref
                        )
                    })
                    .unwrap_or_default();
                matches_query(
                    &query,
                    &format!(
                        "{} {project} {device} {branch} {pr}",
                        chat.title.as_deref().unwrap_or("New session")
                    ),
                )
            })
            .collect();
        chats.sort_by(|a, b| spaces::compare_sidebar_chats(self.settings.sidebar_sort, a, b));
        // Limit after filtering and sorting so every chat remains searchable.
        entries.extend(
            chats
                .into_iter()
                .take(HISTORY_RESULT_LIMIT)
                .map(|chat| Entry::Chat(chat.id.clone())),
        );
        entries
    }

    fn activate_command(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.close_command_palette(window, cx);
        match entry {
            Entry::NewChat => self.open_new_session(cx),
            Entry::NewProject => self.open_add_space(cx),
            Entry::ImportClaude => self.open_claude_import(window, cx),
            Entry::Settings => self.open_settings(SettingsSection::Devices, cx),
            Entry::Chat(id) => self.open_chat(id, cx),
        }
    }

    pub(super) fn render_command_palette(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let entries = self.command_entries(cx);
        let palette = self.command_palette.as_mut()?;
        if std::mem::take(&mut palette.focus_pending) {
            window.focus(&palette.search.focus_handle(cx), cx);
        }
        palette.active = palette.active.min(entries.len().saturating_sub(1));
        let active = palette.active;
        let search = palette.search.clone();
        let query = search.read(cx).text().to_string();
        let focus = palette.focus.clone();
        let scroll = palette.scroll.clone();
        let theme = Theme::of(cx).for_popup();
        let action_count = entries.iter().take_while(|e| e.action().is_some()).count();
        let mut rows = Vec::new();
        for (ix, entry) in entries.iter().enumerate() {
            let mut row = div().id(("command-result", ix)).flex_none();
            if ix == 0 && action_count > 0 {
                row = row.child(div().px(px(10.0)).child(section_label("Actions", &theme)));
            }
            if ix == action_count && action_count > 0 {
                row = row.child(
                    div()
                        .mt(px(8.0))
                        .mb(px(8.0))
                        .h(px(1.0))
                        .w_full()
                        .bg(theme.border),
                );
            }
            let content = if let Some((label, glyph)) = entry.action() {
                let shortcut = match entry {
                    Entry::NewChat | Entry::NewProject => {
                        let id = if *entry == Entry::NewChat {
                            ShortcutId::NewSession
                        } else {
                            ShortcutId::NewProject
                        };
                        let combo = self.settings.keymap.get(id);
                        let valid = Keystroke::parse(&platform_combo(combo)).is_ok();
                        Some(crate::settings::badge_combo(if valid {
                            combo
                        } else {
                            id.default_combo()
                        }))
                    }
                    Entry::Settings => Some(crate::settings::badge_combo("mod-,")),
                    _ => None,
                };
                let entry = entry.clone();
                popover::menu_row(&theme, ix == active, format!("command-action-{ix}"))
                    .id(("command-action", ix))
                    .rounded(px(popover::PALETTE_ITEM_RADIUS))
                    .h(px(32.0))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_command(entry.clone(), window, cx)
                    }))
                    .child(icon(glyph).size(px(17.0)).text_color(theme.text_muted))
                    .child(popover::search_highlight(
                        label.into(),
                        Some(&query),
                        &theme,
                    ))
                    .child(div().flex_1())
                    .when_some(shortcut, |row, shortcut| {
                        row.child(
                            popover::key_cap(&theme)
                                .flex_none()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(shortcut)),
                        )
                    })
                    .into_any_element()
            } else if let Entry::Chat(id) = entry {
                let state = self.state.read(cx);
                let chat = state.chats.iter().find(|chat| &chat.id == id)?;
                let project = match (state.space_for_chat(chat), chat.space_id.as_deref()) {
                    (Some(space), _) => space.display_name(),
                    (None, None) => "~",
                    _ => "?",
                };
                let folder = match state.device_name(&chat.device_id) {
                    Some(device) => format!("{project} @ {device}"),
                    None => project.to_string(),
                };
                let branch = self
                    .settings
                    .sidebar_show_branch
                    .then(|| crate::change_requests::conversation_branch(chat, &state.spaces))
                    .flatten()
                    .map(str::trim)
                    .filter(|branch| !branch.is_empty())
                    .map(SharedString::from);
                let pr = self
                    .settings
                    .sidebar_show_pull_request
                    .then(|| state.change_request_for_chat(chat).cloned())
                    .flatten();
                let harness = self
                    .settings
                    .sidebar_show_harness
                    .then(|| chat.config.as_ref().map(|c| c.harness))
                    .flatten();
                self.render_chat_row(
                    id.clone(),
                    transcript::single_line(chat.title.as_deref().unwrap_or("New session")).into(),
                    format_time_ago(chat.last_message_at.unwrap_or(chat.created_at), Utc::now())
                        .into(),
                    folder.into(),
                    branch,
                    pr,
                    harness,
                    state.display_status_for(chat, Utc::now()),
                    ix == active,
                    chat.archived,
                    None,
                    Some(&query),
                    &theme,
                    cx,
                )
            } else {
                unreachable!()
            };
            rows.push(row.child(div().px(px(popover::CARD_INSET)).child(content)));
        }
        let height = (f32::from(viewport.height) - 180.0).clamp(100.0, 440.0);
        let body = div()
            .id("command-results")
            .min_h_0()
            .max_h(px(height - RESULTS_SCROLL_GUTTER * 2.0))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_LIST_GAP))
            .children(rows)
            .when(entries.is_empty(), |el| {
                el.child(
                    div()
                        .p(px(24.0))
                        .text_color(theme.text_muted)
                        .child("No actions or chats found"),
                )
            });
        let card = div()
            .id("command-palette")
            .track_focus(&focus)
            .w(px(600.0_f32.min(f32::from(viewport.width) - 32.0)))
            .flex()
            .flex_col()
            .rounded(px(14.0))
            .border_1()
            .border_color(theme.border)
            .when(!theme.is_frost(), |el| el.shadow_lg())
            .bg(popover::surface_bg(&theme))
            .text_color(theme.text)
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "up" | "down" => {
                            let count = this.command_entries(cx).len();
                            if count > 0
                                && let Some(palette) = this.command_palette.as_mut()
                            {
                                palette.active = if event.keystroke.key == "down" {
                                    (palette.active + 1) % count
                                } else {
                                    (palette.active + count - 1) % count
                                };
                                palette.scroll.scroll_to_item(palette.active);
                                cx.notify();
                            }
                        }
                        "enter" => {
                            let entries = this.command_entries(cx);
                            if let Some(entry) = this
                                .command_palette
                                .as_ref()
                                .and_then(|p| entries.get(p.active))
                                .cloned()
                            {
                                this.activate_command(entry, window, cx);
                            }
                        }
                        "escape" => this.close_command_palette(window, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_command_palette(window, cx)),
            )
            .child(
                div()
                    .h(px(58.0))
                    .flex_none()
                    .px(px(18.0))
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(popover::palette_search_icon(&theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::typography::ui_rems(15.0))
                            .child(search),
                    )
                    .child(popover::key_hint_text(
                        &theme,
                        if cfg!(target_os = "macos") {
                            "⌘K"
                        } else {
                            "Ctrl K"
                        },
                        "",
                    )),
            )
            // Keep these gutters outside the scroll viewport: content padding
            // scrolls away, and keyboard reveal otherwise pins rows to an edge.
            .child(div().min_h_0().py(px(RESULTS_SCROLL_GUTTER)).child(body))
            .child(
                div()
                    .flex_none()
                    .px(px(18.0))
                    .py(px(12.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .gap(px(18.0))
                    .child(popover::key_hint_pair(
                        &theme,
                        icons::ARROW_UP,
                        icons::ARROW_DOWN,
                        "Navigate",
                    ))
                    .child(popover::key_hint_text(&theme, "↵", "Open"))
                    .child(popover::key_hint_text(&theme, "esc", "Close")),
            );
        // Match the composer's 16px backdrop blur, including its opaque fallback.
        let card = crate::frost::frosted(14.0, crate::frost::MENU_BLUR, card);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::point(px(0.0), px(0.0)))
                    .child(
                        div()
                            .occlude()
                            .w(viewport.width)
                            .h(viewport.height)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(card),
                    ),
            )
            .priority(2)
            .into_any_element(),
        )
    }
}

fn section_label(label: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .px(px(8.0))
        .py(px(8.0))
        .text_size(crate::typography::ui_rems(11.0))
        .text_color(theme.text_muted)
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_search_hides_empty_section_and_preserves_order() {
        assert_eq!(
            actions_for(""),
            vec![
                Entry::NewChat,
                Entry::NewProject,
                Entry::ImportClaude,
                Entry::Settings
            ]
        );
        assert_eq!(actions_for("new"), vec![Entry::NewChat, Entry::NewProject]);
        assert_eq!(actions_for("settings"), vec![Entry::Settings]);
        assert!(actions_for("deployment").is_empty());
    }

    #[test]
    fn search_matches_words_across_chat_metadata() {
        assert!(matches_query(
            "mac auth",
            "Fix authentication Zeron @ MacBook main"
        ));
        assert!(matches_query("  ", "Any chat"));
        assert!(!matches_query("mac windows", "Zeron @ MacBook"));
    }
}
