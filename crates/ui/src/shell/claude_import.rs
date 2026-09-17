//! Pick a Claude Code session recorded on this device and import it as a chat.
use super::*;

use zeron_engine::claude_import::ImportableSession;

/// Rows are the sessions themselves, plus a bulk row when anything is new.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    /// Import every session not already here.
    All(usize),
    /// Index into the filtered session list.
    Session(usize),
}

/// Where a running or finished import has got to.
#[derive(Clone, Debug)]
pub(super) enum ImportProgress {
    Running {
        done: usize,
        total: usize,
        current: Option<SharedString>,
    },
    Done {
        imported: usize,
        skipped: usize,
        messages: usize,
        /// Set when exactly one session was asked for, so it can be opened.
        opened: Option<String>,
    },
    Failed(SharedString),
}

pub(super) struct ClaudeImportFlow {
    search: Entity<ComposerInput>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    sessions: popover::Loadable<Vec<ImportableSession>>,
    active: usize,
    focus_pending: bool,
    scroll: gpui::ScrollHandle,
    progress: Option<ImportProgress>,
    /// The session this run asked for, when it asked for exactly one.
    requested: Option<String>,
    load_task: Option<Task<()>>,
    import_task: Option<Task<()>>,
    _search_events: Subscription,
}

fn matches_query(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query.split_whitespace().all(|word| text.contains(word))
}

impl Shell {
    pub(super) fn open_claude_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.claude_import.is_some() {
            return;
        }
        self.command_palette = None;
        self.add_space = None;
        let search = cx.new(|cx| {
            ComposerInput::with_context("Search Claude Code sessions…", "PaletteSearch", cx)
        });
        let events = cx.subscribe(&search, |this, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited)
                && let Some(flow) = this.claude_import.as_mut()
            {
                flow.active = 0;
                flow.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
                cx.notify();
            }
        });
        self.claude_import = Some(ClaudeImportFlow {
            search,
            focus: cx.focus_handle(),
            previous_focus: window.focused(cx),
            sessions: popover::Loadable::Loading,
            active: 0,
            focus_pending: true,
            scroll: gpui::ScrollHandle::new(),
            progress: None,
            requested: None,
            load_task: None,
            import_task: None,
            _search_events: events,
        });
        self.load_claude_sessions(cx);
        cx.notify();
    }

    pub(super) fn close_claude_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(flow) = self.claude_import.take() {
            if let Some(focus) = flow.previous_focus {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }

    fn load_claude_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if let Some(flow) = self.claude_import.as_mut() {
                flow.sessions = popover::Loadable::Error("Engine not connected".into());
            }
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::LIST_CLAUDE_SESSIONS, serde_json::json!({}))
                .await;
            this.update(cx, |shell, cx| {
                let Some(flow) = shell.claude_import.as_mut() else {
                    return;
                };
                flow.sessions = match result {
                    Ok(value) => match serde_json::from_value::<Vec<ImportableSession>>(
                        value
                            .get("sessions")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    ) {
                        Ok(sessions) => popover::Loadable::Ready(sessions),
                        Err(err) => popover::Loadable::Error(err.to_string()),
                    },
                    Err(err) => popover::Loadable::Error(err.to_string()),
                };
                flow.load_task = None;
                cx.notify();
            })
            .ok();
        });
        if let Some(flow) = self.claude_import.as_mut() {
            flow.load_task = Some(task);
        }
    }

    /// Sessions matching the query, newest first as the engine listed them.
    fn claude_import_sessions(&self, cx: &App) -> Vec<ImportableSession> {
        let Some(flow) = &self.claude_import else {
            return Vec::new();
        };
        let Some(sessions) = flow.sessions.ready() else {
            return Vec::new();
        };
        let query = flow.search.read(cx).text().trim().to_lowercase();
        sessions
            .iter()
            .filter(|s| matches_query(&query, &format!("{} {}", s.session.title, s.session.cwd)))
            .cloned()
            .collect()
    }

    fn claude_import_rows(&self, cx: &App) -> Vec<Row> {
        let sessions = self.claude_import_sessions(cx);
        let new = sessions.iter().filter(|s| !s.already_imported).count();
        let mut rows = Vec::new();
        if new > 1 {
            rows.push(Row::All(new));
        }
        rows.extend((0..sessions.len()).map(Row::Session));
        rows
    }

    fn activate_claude_import(&mut self, row: Row, cx: &mut Context<Self>) {
        let sessions = self.claude_import_sessions(cx);
        match row {
            Row::All(_) => self.start_claude_import(Vec::new(), cx),
            Row::Session(ix) => {
                let Some(session) = sessions.get(ix) else {
                    return;
                };
                if session.already_imported {
                    let chat_id = session.session.session_id.clone();
                    self.claude_import = None;
                    self.open_chat(chat_id, cx);
                    return;
                }
                self.start_claude_import(vec![session.session.session_id.clone()], cx);
            }
        }
    }

    /// Stream one import run, mirroring the profile importer's plumbing.
    fn start_claude_import(&mut self, session_ids: Vec<String>, cx: &mut Context<Self>) {
        let Some(flow) = self.claude_import.as_mut() else {
            return;
        };
        if flow.import_task.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if let Some(flow) = self.claude_import.as_mut() {
                flow.progress = Some(ImportProgress::Failed("Engine not connected".into()));
            }
            cx.notify();
            return;
        };
        let requested = (session_ids.len() == 1).then(|| session_ids[0].clone());
        let Some(flow) = self.claude_import.as_mut() else {
            return;
        };
        flow.requested = requested;
        flow.progress = Some(ImportProgress::Running {
            done: 0,
            total: 0,
            current: None,
        });
        let params = serde_json::json!({ "sessionIds": session_ids });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let stream = Tokio::spawn(cx, async move {
            let mut items = engine
                .client()
                .subscribe(methods::IMPORT_CLAUDE_SESSIONS, params)
                .await
                .map_err(|error| error.to_string())?;
            while let Some(item) = items.recv().await {
                let _ = tx.send(item);
            }
            Ok::<(), String>(())
        });
        let task = cx.spawn(async move |this, cx| {
            loop {
                let item = rx.recv().await;
                let ended = item.is_none();
                this.update(cx, |shell, cx| {
                    if let Some(item) = &item {
                        shell.apply_claude_import_event(item, cx);
                    }
                    if ended && let Some(flow) = shell.claude_import.as_mut() {
                        flow.import_task = None;
                        // A stream that ended before its summary never finished.
                        if matches!(flow.progress, Some(ImportProgress::Running { .. })) {
                            flow.progress = Some(ImportProgress::Failed(
                                "The import stopped before it finished.".into(),
                            ));
                        }
                        cx.notify();
                    }
                })
                .ok();
                if ended {
                    break;
                }
            }
            if let Ok(Err(error)) = stream.await {
                this.update(cx, |shell, cx| {
                    if let Some(flow) = shell.claude_import.as_mut() {
                        flow.import_task = None;
                        flow.progress = Some(ImportProgress::Failed(error.into()));
                        cx.notify();
                    }
                })
                .ok();
            }
        });
        if let Some(flow) = self.claude_import.as_mut() {
            flow.import_task = Some(task);
        }
        cx.notify();
    }

    fn apply_claude_import_event(&mut self, item: &serde_json::Value, cx: &mut Context<Self>) {
        let requested = self
            .claude_import
            .as_ref()
            .and_then(|f| f.requested.clone());
        let Some(flow) = self.claude_import.as_mut() else {
            return;
        };
        let number = |key: &str| item.get(key).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        match item.get("kind").and_then(|k| k.as_str()) {
            Some("start") => {
                flow.progress = Some(ImportProgress::Running {
                    done: 0,
                    total: number("sessions"),
                    current: None,
                });
            }
            Some("session") => {
                flow.progress = Some(ImportProgress::Running {
                    done: number("index"),
                    total: number("total"),
                    current: item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(|t| SharedString::from(t.to_string())),
                });
            }
            Some("summary") => {
                let errors: Vec<String> = item
                    .get("errors")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                flow.progress = Some(if errors.is_empty() {
                    ImportProgress::Done {
                        imported: number("imported"),
                        skipped: number("skipped"),
                        messages: number("messages"),
                        opened: requested,
                    }
                } else {
                    ImportProgress::Failed(errors.join("; ").into())
                });
            }
            _ => {}
        }
        cx.notify();
    }

    /// After a single-session import, land in the chat that was just written.
    fn finish_claude_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let opened = match self.claude_import.as_ref().and_then(|f| f.progress.clone()) {
            Some(ImportProgress::Done { opened, .. }) => opened,
            _ => None,
        };
        self.close_claude_import(window, cx);
        if let Some(chat_id) = opened {
            self.open_chat(chat_id, cx);
        }
    }

    pub(super) fn render_claude_import_overlay(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        if let Some(progress) = self.claude_import.as_ref().and_then(|f| f.progress.clone()) {
            return Some(self.render_claude_import_progress(progress, viewport, &theme, cx));
        }
        let rows = self.claude_import_rows(cx);
        let sessions = self.claude_import_sessions(cx);
        let flow = self.claude_import.as_mut()?;
        if std::mem::take(&mut flow.focus_pending) {
            window.focus(&flow.search.focus_handle(cx), cx);
        }
        flow.active = flow.active.min(rows.len().saturating_sub(1));
        let active = flow.active;
        let search = flow.search.clone();
        let query = search.read(cx).text().to_string();
        let focus = flow.focus.clone();
        let scroll = flow.scroll.clone();
        let state = flow.sessions.clone();

        let mut list = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            let row = row.clone();
            let content = match &row {
                Row::All(count) => {
                    popover::menu_row(&theme, ix == active, format!("claude-all-{ix}"))
                        .id(("claude-import-all", ix))
                        .rounded(px(popover::PALETTE_ITEM_RADIUS))
                        .h(px(32.0))
                        .child(
                            icon(icons::FOLDER)
                                .size(px(17.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from(format!(
                            "Import all {count} new sessions"
                        )))
                        .into_any_element()
                }
                Row::Session(session_ix) => {
                    let session = sessions.get(*session_ix)?;
                    let when = chrono::DateTime::from_timestamp_millis(session.session.modified_ms)
                        .map(|at| format_time_ago(at, Utc::now()))
                        .unwrap_or_default();
                    popover::menu_row(&theme, ix == active, format!("claude-session-{ix}"))
                        .id(("claude-import-session", ix))
                        .rounded(px(popover::PALETTE_ITEM_RADIUS))
                        .h(px(44.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .flex_1()
                                .child(popover::search_highlight(
                                    transcript::single_line(&session.session.title).into(),
                                    Some(&query),
                                    &theme,
                                ))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(11.0))
                                        .text_color(theme.text_muted)
                                        .child(SharedString::from(session.session.cwd.clone())),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(if session.already_imported {
                                    "Already here".to_string()
                                } else {
                                    when
                                })),
                        )
                        .into_any_element()
                }
            };
            let clickable = div()
                .id(("claude-import-row", ix))
                .flex_none()
                .on_click(
                    cx.listener(move |this, _, _, cx| this.activate_claude_import(row.clone(), cx)),
                )
                .child(content);
            list.push(
                div()
                    .px(px(popover::CARD_INSET))
                    .child(clickable)
                    .into_any_element(),
            );
        }

        let height = (f32::from(viewport.height) - 180.0).clamp(100.0, 440.0);
        let body = div()
            .id("claude-import-results")
            .min_h_0()
            .max_h(px(height))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_LIST_GAP))
            .children(list)
            .when(rows.is_empty(), |el| {
                el.child(
                    div()
                        .p(px(24.0))
                        .text_color(theme.text_muted)
                        .child(match &state {
                            popover::Loadable::Loading => "Looking for Claude Code sessions…",
                            popover::Loadable::Error(_) => "Could not read Claude Code sessions",
                            _ => "No Claude Code sessions found on this device",
                        }),
                )
            });

        let card = div()
            .id("claude-import")
            .track_focus(&focus)
            .w(px(620.0_f32.min(f32::from(viewport.width) - 32.0)))
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
                            let count = this.claude_import_rows(cx).len();
                            if count > 0
                                && let Some(flow) = this.claude_import.as_mut()
                            {
                                flow.active = if event.keystroke.key == "down" {
                                    (flow.active + 1) % count
                                } else {
                                    (flow.active + count - 1) % count
                                };
                                flow.scroll.scroll_to_item(flow.active);
                                cx.notify();
                            }
                        }
                        "enter" => {
                            let rows = this.claude_import_rows(cx);
                            if let Some(row) = this
                                .claude_import
                                .as_ref()
                                .and_then(|f| rows.get(f.active))
                                .cloned()
                            {
                                this.activate_claude_import(row, cx);
                            }
                        }
                        "escape" => this.close_claude_import(window, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_claude_import(window, cx)),
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
                    ),
            )
            .child(div().min_h_0().py(px(popover::CARD_INSET)).child(body))
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
                    .child(popover::key_hint_text(&theme, "↵", "Import"))
                    .child(popover::key_hint_text(&theme, "esc", "Close")),
            );
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

    fn render_claude_import_progress(
        &mut self,
        progress: ImportProgress,
        viewport: gpui::Size<Pixels>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = match progress {
            ImportProgress::Running {
                done,
                total,
                current,
            } => {
                let label = match (&current, total) {
                    (Some(title), total) if total > 1 => {
                        format!("{title} ({} of {total})", done + 1)
                    }
                    (Some(title), _) => title.to_string(),
                    (None, _) => "Reading transcripts…".to_string(),
                };
                popover::dialog_card(theme)
                    .child(popover::dialog_title(theme, "Importing"))
                    .child(popover::dialog_body(theme, label))
            }
            ImportProgress::Done {
                imported,
                skipped,
                messages,
                ..
            } => {
                let body = if imported == 0 && skipped > 0 {
                    "That session is already here.".to_string()
                } else if imported == 1 {
                    format!("Imported 1 session with {messages} messages.")
                } else {
                    format!("Imported {imported} sessions with {messages} messages.")
                };
                popover::dialog_card(theme)
                    .child(popover::dialog_title(theme, "Import finished"))
                    .child(popover::dialog_body(theme, body))
                    .child(
                        div().flex().justify_end().child(
                            popover::btn_primary(theme, "Open")
                                .id("claude-import-open")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.finish_claude_import(window, cx)
                                })),
                        ),
                    )
            }
            ImportProgress::Failed(error) => popover::dialog_card(theme)
                .child(popover::dialog_title(theme, "Import failed"))
                .child(popover::dialog_body(theme, error))
                .child(
                    div().flex().justify_end().child(
                        popover::btn_ghost(theme, "Close", "claude-import-close")
                            .id("claude-import-dismiss")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_claude_import(window, cx)
                            })),
                    ),
                ),
        };
        popover::modal("claude-import-dialog", viewport, card.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_title_and_folder() {
        assert!(matches_query(
            "nix zeron",
            "NixOS installability /home/u/zeron"
        ));
        assert!(matches_query("", "anything"));
        assert!(!matches_query(
            "codex",
            "NixOS installability /home/u/zeron"
        ));
    }
}
