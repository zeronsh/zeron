//! Global action and conversation search, using the sidebar's conversation rows.
use super::*;
use crate::appearance::AppearanceMode;

const HISTORY_RESULT_LIMIT: usize = 30;
const RESULTS_FADE_BAND: f32 = 18.0;
const TRANSCRIPT_SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(80);

pub(super) struct CommandPalette {
    search: Entity<ComposerInput>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    active: usize,
    enter_press: EnterPress,
    // Claim focus during mount so the shell does not restore the composer
    // while this input is still absent from the dispatch tree.
    focus_pending: bool,
    scroll: gpui::ScrollHandle,
    // Kept across keystrokes until the next reply lands, so rows don't flicker.
    transcript_hits: Vec<zeron_proto::TranscriptSearchHit>,
    transcript_search: Option<gpui::Task<()>>,
    _search_events: Subscription,
}

// X11 suppresses synthetic repeat releases but sends repeated keydowns with
// is_held=false. Keep our own latch until the physical key is released.
#[derive(Default)]
struct EnterPress {
    down: bool,
}

impl EnterPress {
    fn press(&mut self, is_held: bool) -> bool {
        let was_down = std::mem::replace(&mut self.down, true);
        !was_down && !is_held
    }

    fn release(&mut self) {
        self.down = false;
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    NewChat,
    NewProject,
    Settings,
    Theme(AppearanceMode),
    /// `snippet` is set when only the transcript matched, not the title or metadata.
    Chat {
        id: String,
        snippet: Option<SharedString>,
    },
}

impl Entry {
    fn action(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::NewChat => Some(("New chat", icons::PEN_NEW_SQUARE)),
            Self::NewProject => Some(("New project", icons::FOLDER)),
            Self::Settings => Some(("Open settings", icons::SETTINGS)),
            Self::Theme(mode) => Some((
                match mode {
                    AppearanceMode::System => "Switch to system theme",
                    AppearanceMode::Light => "Switch to light theme",
                    AppearanceMode::Dark => "Switch to dark theme",
                },
                mode.icon(),
            )),
            Self::Chat { .. } => None,
        }
    }
}

fn matches_query(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query.split_whitespace().all(|word| text.contains(word))
}

fn actions_for(query: &str, is_dark: bool) -> Vec<Entry> {
    [
        Entry::NewChat,
        Entry::NewProject,
        Entry::Settings,
        Entry::Theme(if is_dark {
            AppearanceMode::Light
        } else {
            AppearanceMode::Dark
        }),
    ]
    .into_iter()
    .filter(|entry| matches_query(query, entry.action().unwrap().0))
    .collect()
}

impl Shell {
    pub(super) fn reset_command_palette_key_state(&mut self) {
        if let Some(palette) = self.command_palette.as_mut() {
            palette.enter_press.release();
        }
    }

    pub(super) fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette.is_some() {
            self.close_command_palette(window, cx);
            return;
        }
        self.add_space = None;
        let search = cx.new(|cx| {
            ComposerInput::with_context("Search commands and chats…", "PaletteSearch", cx)
        });
        let events = cx.subscribe(&search, |this, search, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                let query = search.read(cx).text().trim().to_string();
                if let Some(palette) = this.command_palette.as_mut() {
                    palette.active = 0;
                    palette.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
                    if query.is_empty() {
                        palette.transcript_hits.clear();
                    }
                }
                this.search_transcripts(query, TRANSCRIPT_SEARCH_DEBOUNCE, cx);
                cx.notify();
            }
        });
        let previous_focus = window.focused(cx);
        self.command_palette = Some(CommandPalette {
            search,
            focus: cx.focus_handle(),
            previous_focus,
            active: 0,
            enter_press: EnterPress::default(),
            focus_pending: true,
            scroll: gpui::ScrollHandle::new(),
            transcript_hits: Vec::new(),
            transcript_search: None,
            _search_events: events,
        });
        // An empty query refreshes the engine's index before the first keystroke.
        self.search_transcripts(String::new(), std::time::Duration::ZERO, cx);
        cx.notify();
    }

    /// Replacing the task drops the previous request, so only the latest query lands.
    fn search_transcripts(
        &mut self,
        query: String,
        delay: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(palette) = self.command_palette.as_mut() else {
            return;
        };
        palette.transcript_search = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let request = zeron_proto::SearchTranscriptsRequest {
                query: query.clone(),
                limit: HISTORY_RESULT_LIMIT as u16,
            };
            let Ok(params) = serde_json::to_value(&request) else {
                return;
            };
            // Older engines lack the method; the palette keeps metadata matches.
            let hits = match engine
                .client()
                .call(methods::SEARCH_TRANSCRIPTS, params)
                .await
                .map(serde_json::from_value::<Vec<zeron_proto::TranscriptSearchHit>>)
            {
                Ok(Ok(hits)) => hits,
                _ => return,
            };
            if query.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                if let Some(palette) = this.command_palette.as_mut() {
                    palette.transcript_hits = hits;
                    cx.notify();
                }
            })
            .ok();
        }));
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
        let mut entries = actions_for(&query, Theme::of(cx).appearance.is_dark());
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
        let mut shown: std::collections::HashSet<&str> =
            chats.iter().map(|chat| chat.id.as_str()).collect();
        // Metadata matches lead; transcript-only matches follow in relevance order.
        let transcript_only = palette
            .transcript_hits
            .iter()
            .filter(|_| !query.is_empty())
            .filter(|hit| state.chats.iter().any(|chat| chat.id == hit.chat_id))
            .filter(|hit| shown.insert(hit.chat_id.as_str()))
            .map(|hit| Entry::Chat {
                id: hit.chat_id.clone(),
                snippet: Some(transcript::single_line(&hit.snippet).into()),
            });
        // Limit after filtering and sorting so every chat remains searchable.
        entries.extend(
            chats
                .into_iter()
                .map(|chat| Entry::Chat {
                    id: chat.id.clone(),
                    snippet: None,
                })
                .chain(transcript_only)
                .take(HISTORY_RESULT_LIMIT),
        );
        entries
    }

    fn activate_command(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if let Entry::Theme(mode) = entry {
            // Keep the palette open so this ordinary action updates to its next state.
            crate::appearance::set_mode(mode, cx);
            cx.notify();
            return;
        }
        self.close_command_palette(window, cx);
        match entry {
            Entry::NewChat => self.open_new_session(cx),
            Entry::NewProject => self.open_add_space(cx),
            Entry::Settings => self.open_settings(SettingsSection::General, cx),
            Entry::Theme(_) => unreachable!(),
            Entry::Chat { id, .. } => self.open_chat(id, cx),
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
            // End spacing belongs to the content, so it scrolls out of the
            // fade instead of leaving a permanent gutter beside the chrome.
            let mut row = div()
                .id(("command-result", ix))
                .flex_none()
                .when(ix == 0, |row| row.pt(px(8.0)))
                .when(ix + 1 == entries.len(), |row| row.pb(px(8.0)));
            if ix == action_count && action_count > 0 {
                row = row.child(spaces::sidebar_separator(&theme).w_full().my(px(8.0)));
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
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .min_h(px(30.0))
                    .py(px(4.0))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_command(entry.clone(), window, cx)
                    }))
                    .child(
                        icon(glyph)
                            .size(px(16.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(div().flex_1().min_w_0().child(popover::search_highlight(
                        label.into(),
                        Some(&query),
                        &theme,
                    )))
                    .when_some(shortcut, |row, shortcut| {
                        row.child(popover::kbd_hint(&theme, &shortcut))
                    })
                    .into_any_element()
            } else if let Entry::Chat { id, snippet } = entry {
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
                    false,
                    None,
                    None,
                    Some(&query),
                    snippet.clone(),
                    &theme,
                    cx,
                )
            } else {
                unreachable!()
            };
            rows.push(row.child(div().px(px(8.0)).child(content)));
        }
        let height = (f32::from(viewport.height) - 180.0).clamp(100.0, 360.0);
        let body = div()
            .id("command-results")
            .min_h_0()
            .max_h(px(height))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_LIST_GAP))
            .children(rows)
            .when(entries.is_empty(), |el| {
                el.child(
                    div()
                        .w_full()
                        .py(px(24.0))
                        .px(px(16.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(crate::typography::ui_rems(13.0))
                        .child("No results")
                        .child(div().text_color(theme.text_muted).child(
                            "Try a command, chat title, project, device, or words from a chat.",
                        )),
                )
            });
        let body = crate::edge_fade::edge_faded(RESULTS_FADE_BAND, true, true, body)
            .fade_overflow_y(&scroll);
        let card = div()
            .id("command-palette")
            .track_focus(&focus)
            .w(px(560.0_f32.min(f32::from(viewport.width) - 32.0)))
            .flex()
            .flex_col()
            .rounded(px(16.0))
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
                            let activate = this
                                .command_palette
                                .as_mut()
                                .is_some_and(|palette| palette.enter_press.press(event.is_held));
                            if !activate {
                                cx.stop_propagation();
                                return;
                            }
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
            .on_key_up(cx.listener(|this, event: &gpui::KeyUpEvent, _, cx| {
                if event.keystroke.key == "enter" {
                    if let Some(palette) = this.command_palette.as_mut() {
                        palette.enter_press.release();
                    }
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_command_palette(window, cx)),
            )
            .child(
                div()
                    .min_h(px(44.0))
                    .flex_none()
                    .px(px(16.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(crate::theme::hairline(0.06))
                    .child(popover::palette_search_icon(&theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::typography::ui_rems(14.0))
                            .child(search),
                    )
                    .child(popover::kbd_hint(
                        &theme,
                        &crate::settings::badge_combo("mod-k"),
                    )),
            )
            .child(body)
            .child(
                div()
                    .flex_none()
                    .px(px(16.0))
                    .py(px(7.0))
                    .border_t_1()
                    .border_color(crate::theme::hairline(0.06))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .child(command_key_hint(&theme, "↑ ↓", "Navigate"))
                    .child(command_key_hint(&theme, "↵", "Select"))
                    .child(command_key_hint(&theme, "Esc", "Close")),
            );
        // Match the composer's 16px backdrop blur, including its opaque fallback.
        let card = crate::frost::frosted(16.0, crate::frost::MENU_BLUR, card);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::point(px(0.0), px(0.0)))
                    .child(
                        div()
                            .occlude()
                            .w(viewport.width)
                            .h(viewport.height)
                            // Match glass modals: quiet the background while
                            // preserving its color through the frosted palette.
                            .bg(popover::scrim_alpha(0.35))
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

fn command_key_hint(theme: &Theme, keys: &str, label: &'static str) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(popover::kbd_hint(theme, keys))
        .child(
            div()
                .text_size(crate::typography::ui_rems(10.0))
                .text_color(theme.text_muted)
                .child(label),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::AppearanceMode;

    #[test]
    fn x11_unflagged_enter_repeats_activate_once_until_release() {
        let mut enter = EnterPress::default();
        // The pinned X11 backend drops synthetic repeat releases and emits
        // every repeated KeyDownEvent with is_held=false.
        assert!(enter.press(false));
        for _ in 0..35 {
            assert!(!enter.press(false));
        }
        enter.release();
        assert!(enter.press(false));
    }

    #[test]
    fn flagged_enter_repeats_do_not_activate() {
        let mut enter = EnterPress::default();
        assert!(!enter.press(true));
        assert!(!enter.press(false));
        enter.release();
        assert!(enter.press(false));
        assert!(!enter.press(true));
    }

    #[test]
    fn action_search_hides_empty_section_and_preserves_order() {
        assert_eq!(
            actions_for("", true),
            vec![
                Entry::NewChat,
                Entry::NewProject,
                Entry::Settings,
                Entry::Theme(AppearanceMode::Light)
            ]
        );
        assert_eq!(
            actions_for("new", true),
            vec![Entry::NewChat, Entry::NewProject]
        );
        assert_eq!(actions_for("settings", true), vec![Entry::Settings]);
        assert_eq!(
            actions_for("theme", true),
            vec![Entry::Theme(AppearanceMode::Light)]
        );
        assert!(actions_for("deployment", true).is_empty());
    }

    #[test]
    fn theme_action_targets_the_opposite_resolved_appearance() {
        assert_eq!(
            actions_for("theme", true),
            vec![Entry::Theme(AppearanceMode::Light)]
        );
        assert_eq!(
            actions_for("theme", false),
            vec![Entry::Theme(AppearanceMode::Dark)]
        );
        assert_eq!(
            actions_for("light", true),
            vec![Entry::Theme(AppearanceMode::Light)]
        );
        assert_eq!(
            actions_for("dark", false),
            vec![Entry::Theme(AppearanceMode::Dark)]
        );
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

    fn chat_ids(entries: &[Entry]) -> Vec<(String, bool)> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Chat { id, snippet } => Some((id.clone(), snippet.is_some())),
                _ => None,
            })
            .collect()
    }

    /// Drives the real palette against a real engine. Three chats run
    /// mock-harness turns. Typing a word that appears only in a transcript
    /// lists that chat after the title matches, with a snippet. A query that
    /// gets replaced before its reply arrives never reaches the palette.
    #[gpui::test]
    fn palette_lists_chats_whose_transcripts_match(cx: &mut gpui::TestAppContext) {
        use crate::settings;
        use crate::state::{AppState, EngineBootConfig, EngineHandle};
        use crate::theme::Theme;
        use gpui::AppContext as _;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let registry = zeron_engine::HarnessRegistry::new();
        registry.register(std::sync::Arc::new(zeron_harness::mock::MockHarness {
            script: vec![
                zeron_proto::AgentEvent::TextDelta {
                    text: "Sure, here is what I found.".into(),
                },
                zeron_proto::AgentEvent::Done {
                    status: zeron_proto::DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }));
        let core = zeron_engine::EngineCore::assemble(
            &dir.path().join("engine"),
            std::sync::Arc::new(registry),
            zeron_proto::HarnessId::Mock,
            None,
        )
        .unwrap();
        let chats = [
            ("c-deploy", "Friday cleanup", "the helm configmap is stale"),
            ("c-lunch", "Team offsite", "find a vegetarian ramen place"),
            ("c-ramen", "Ramen notes", "summarize the agenda"),
        ];
        runtime.block_on(async {
            let client = zeron_rpc::memory_client(core.rpc_service());
            for (chat, title, prompt) in chats {
                client
                    .call(
                        methods::MUTATE,
                        serde_json::json!({"op": "createChat", "chatId": chat, "deviceId": core.device_id}),
                    )
                    .await
                    .unwrap();
                core.workspace.rename_chat(chat, title).unwrap();
                client
                    .call(
                        methods::QUEUE_COMMAND,
                        serde_json::json!({"chatId": chat, "command": {
                            "kind": "run", "messageId": format!("{chat}-m1"),
                            "request": {"prompt": prompt, "cwd": "/tmp",
                                "sandbox": "workspace-write", "autoApprove": true},
                        }}),
                    )
                    .await
                    .unwrap();
            }
            let search = serde_json::json!({"query": "found", "limit": 30});
            for _ in 0..300 {
                let hits = client.call(methods::SEARCH_TRANSCRIPTS, search.clone()).await;
                if hits.is_ok_and(|hits| hits.as_array().is_some_and(|h| h.len() == 3)) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!("every turn reaches the index");
        });

        cx.executor().allow_parking();
        cx.update(|cx| {
            settings::init(settings::UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let engine = EngineHandle::from_test_client(zeron_rpc::memory_client(core.rpc_service()));
        let rows = core.workspace.read_chats().unwrap();
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
            .update(cx, |shell, window, cx| {
                shell.state.update(cx, |state, _| {
                    state.chats = rows;
                    state.set_test_engine(engine);
                });
                shell.toggle_command_palette(window, cx);
            })
            .unwrap();

        let type_query = |cx: &mut gpui::TestAppContext, query: &str| {
            window
                .update(cx, |shell, _, cx| {
                    let search = shell.command_palette.as_ref().unwrap().search.clone();
                    search.update(cx, |search, cx| search.set_text(query, cx));
                })
                .unwrap();
        };
        let settle = |cx: &mut gpui::TestAppContext, want: &[(&str, bool)]| {
            for _ in 0..200 {
                cx.executor().advance_clock(TRANSCRIPT_SEARCH_DEBOUNCE);
                cx.run_until_parked();
                let got = window
                    .update(cx, |shell, _, cx| chat_ids(&shell.command_entries(cx)))
                    .unwrap();
                if got
                    .iter()
                    .map(|(id, snippet)| (id.as_str(), *snippet))
                    .eq(want.iter().copied())
                {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let got = window
                .update(cx, |shell, _, cx| chat_ids(&shell.command_entries(cx)))
                .unwrap();
            panic!("wanted {want:?}, palette shows {got:?}");
        };

        type_query(cx, "configmap");
        settle(cx, &[("c-deploy", true)]);

        type_query(cx, "ramen");
        settle(cx, &[("c-ramen", false), ("c-lunch", true)]);

        type_query(cx, "vegetarian");
        type_query(cx, "helm");
        settle(cx, &[("c-deploy", true)]);
        window
            .update(cx, |shell, _, _| {
                let hits = &shell.command_palette.as_ref().unwrap().transcript_hits;
                assert_eq!(hits.len(), 1);
                assert!(hits[0].snippet.contains("helm"), "{hits:?}");
            })
            .unwrap();

        type_query(cx, "");
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let mut got = chat_ids(&shell.command_entries(cx));
                got.sort();
                assert_eq!(
                    got,
                    [("c-deploy", false), ("c-lunch", false), ("c-ramen", false)]
                        .map(|(id, snippet)| (id.to_string(), snippet)),
                    "clearing the query drops transcript snippets"
                );
            })
            .unwrap();
        runtime.block_on(core.shutdown());
    }
}
