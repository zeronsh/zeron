//! The session card: where this chat runs (device, project, branch) and the
//! project's run actions, with the lanes card (its side chats and subagents,
//! see `session_lanes`) stacked under it, both behind one toggle.
//! While the column has room, the card shows at its right on its own (until
//! the user hides it), the transcript and composer shifting left as needed
//! to clear it. Once clearing it would narrow the transcript, the card hides,
//! and opening it floats it over the transcript instead.
use super::*;

use crate::project_actions::{ProjectActionsStatus, action_icon};

/// The card's width, its inset from the column's right edge, and the
/// least gap the reading column keeps from it.
pub(super) const SESSION_CARD_WIDTH: f32 = 272.0;
const SESSION_CARD_INSET: f32 = 12.0;
const SESSION_CARD_GAP: f32 = 24.0;
/// The gap between the session card and the lanes card under it.
const SESSION_CARDS_GAP: f32 = 8.0;
/// The transcript's own side gutters (its rows pad 48px each side).
const TRANSCRIPT_GUTTER: f32 = 48.0;

/// The room the transcript holds free on its right for the card docked in a
/// main column `main` wide, for a configured reading width `transcript`: none
/// while the centered column already clears the card, else just enough to
/// shift it left. `None` once that would narrow the column: the card no
/// longer fits beside it.
pub(super) fn session_card_reserve(main: f32, transcript: f32) -> Option<f32> {
    let card_left = main - SESSION_CARD_WIDTH - SESSION_CARD_INSET;
    // Centered in `main - reserve`, the column's right edge is
    // (main - reserve) / 2 + transcript / 2; keep it at `card_left - gap`.
    let reserve = (main + transcript - 2.0 * (card_left - SESSION_CARD_GAP)).max(0.0);
    (main - reserve - 2.0 * TRANSCRIPT_GUTTER >= transcript - 0.5).then_some(reserve)
}

/// The checkouts' working-tree diffs, for the branch row's `+N −M`: the
/// Changes pane's `WatchCheckoutDiffs` stream, kept without the pane.
#[derive(Default)]
pub(super) struct SessionDiffs {
    diffs: Vec<zeron_proto::CheckoutDiff>,
    /// Device the running watch targets: `None` the connected engine, else
    /// a remote chat's host (diffs are produced where the checkout lives).
    target: Option<Option<String>>,
    _watch: Option<Task<()>>,
}

impl SessionDiffs {
    /// The chat's uncommitted line totals; `None` while clean or unknown.
    fn totals(&self, chat: &zeron_proto::Chat) -> Option<(u32, u32)> {
        crate::changes::resolve_diff(&self.diffs, chat)
            .map(|diff| (diff.additions, diff.deletions))
            .filter(|&(adds, dels)| adds + dels > 0)
    }
}

/// A non-interactive card row: icon slot, label, optional trailing detail.
fn info_row(theme: &Theme, glyph: AnyElement, label: SharedString) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_h(px(popover::PICKER_ROW_HEIGHT))
        .px(px(9.0))
        .text_size(crate::typography::ui_rems(13.0))
        .text_color(theme.text)
        .child(popover::picker_icon_slot(glyph))
        .child(div().flex_1().min_w_0().truncate().child(label))
}

fn row_glyph(path: &'static str, theme: &Theme) -> AnyElement {
    icon(path)
        .size(px(15.0))
        .text_color(theme.text_muted)
        .into_any_element()
}

impl Shell {
    /// Watch the selected chat's host for checkout diffs (idempotent per
    /// host); it retries every 2s while the stream fails or ends.
    fn ensure_session_diffs(&mut self, cx: &mut Context<Self>) {
        let (target, engine) = {
            let state = self.state.read(cx);
            let Some(chat) = state.selected_chat_row() else {
                return;
            };
            let device = chat.device_id.clone();
            let target = (state.local_device_id.as_deref() != Some(device.as_str()))
                .then_some(device);
            (target, state.engine().cloned())
        };
        if self.session_diffs.target.as_ref() == Some(&target) {
            return;
        }
        // Engine still booting: the next render retries.
        let Some(engine) = engine else {
            return;
        };
        self.session_diffs.diffs.clear();
        self.session_diffs.target = Some(target.clone());
        self.session_diffs._watch = Some(cx.spawn(async move |this, cx| {
            loop {
                let mut params = serde_json::Map::new();
                if let Some(target) = &target {
                    params.insert(
                        "targetDeviceId".into(),
                        serde_json::Value::String(target.clone()),
                    );
                }
                if let Ok(mut rx) = engine
                    .client()
                    .subscribe(
                        methods::WATCH_CHECKOUT_DIFFS,
                        serde_json::Value::Object(params),
                    )
                    .await
                {
                    while let Some(value) = rx.recv().await {
                        let alive = this.update(cx, |this, cx| {
                            // Repaint only when the open chat's totals move.
                            let chat = this.state.read(cx).selected_chat_row().cloned();
                            let totals = |diffs: &SessionDiffs| {
                                chat.as_ref().and_then(|chat| diffs.totals(chat))
                            };
                            let before = totals(&this.session_diffs);
                            if crate::changes::apply_diff_frame(
                                &mut this.session_diffs.diffs,
                                value,
                            ) && totals(&this.session_diffs) != before
                            {
                                cx.notify();
                            }
                        });
                        if alive.is_err() {
                            return;
                        }
                    }
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    /// The card's reserve at the main column's LIVE width (see
    /// [`session_card_reserve`]): while the sidebar or a pane animates, the
    /// settled width would move or hide the card ahead of the column.
    fn session_info_reserve(&self, cx: &App) -> Option<f32> {
        let main = (self.viewport_width
            - self.sidebar_now()
            - self.right_visible_width(cx)
            - self.files_visible_width(cx))
        .max(0.0);
        session_card_reserve(main, crate::settings::transcript_width(cx))
    }

    /// Whether the card fits beside the transcript without narrowing it.
    pub(super) fn session_info_fits(&self, cx: &App) -> bool {
        self.session_info_reserve(cx).is_some()
    }

    /// Beside the transcript it shows unless hidden; over it, only while
    /// opened.
    pub(super) fn session_info_visible(&self, cx: &App) -> bool {
        if self.session_info_fits(cx) {
            !self.settings.session_info_hidden
        } else {
            self.session_info_overlay
        }
    }

    /// The toggle's mouse-down: note whether the card showed, so the click
    /// that follows closes it rather than reopening it. Reads the last
    /// painted state, which the overlay's click-outside dismissal (handled
    /// on the same press, in either order) leaves alone.
    pub(super) fn note_session_info_press(&mut self) {
        self.session_info_pressed_open = Some(self.session_info_shown);
    }

    pub(super) fn toggle_session_info(&mut self, cx: &mut Context<Self>) {
        let show = !self
            .session_info_pressed_open
            .take()
            .unwrap_or(self.session_info_shown);
        if self.session_info_fits(cx) {
            self.settings.session_info_hidden = !show;
            self.schedule_save(cx);
        } else {
            self.session_info_overlay = show;
        }
        cx.notify();
    }

    /// Show the card: beside the transcript when it fits there, else over it.
    pub(super) fn show_session_info(&mut self, cx: &mut Context<Self>) {
        if !self.session_info_fits(cx) {
            self.session_info_overlay = true;
        } else if self.settings.session_info_hidden {
            self.settings.session_info_hidden = false;
            self.schedule_save(cx);
        }
        cx.notify();
    }

    /// Dismiss the card floating over the transcript; one shown beside it
    /// stays put.
    pub(super) fn close_session_info_overlay(&mut self, cx: &mut Context<Self>) {
        if self.session_info_overlay {
            self.session_info_overlay = false;
            cx.notify();
        }
    }

    /// This frame's card: docked beside the transcript (else floating over
    /// it), its reveal, 0 (hidden) → 1 (shown), and the room the transcript
    /// holds free for it. The reveal eases on the shell's resize clock
    /// whenever the card shows or hides, by toggle or by the column crossing
    /// the fit, and the reserve eases with it.
    pub(super) fn session_info_frame(&mut self, has_selection: bool, cx: &App) -> (bool, f32, f32) {
        let reserve = self.session_info_reserve(cx);
        if let Some(reserve) = reserve {
            // An overlay opened on a tighter column ends once the card fits.
            self.session_info_overlay = false;
            self.session_info_last_reserve = reserve;
        }
        let shown = has_selection && self.session_info_visible(cx);
        if shown {
            self.session_info_docked = reserve.is_some();
        }
        let target = if shown { 1.0 } else { 0.0 };
        if shown != self.session_info_shown {
            let from = self.eval_tween(self.session_info_tween, 1.0 - target);
            self.session_info_tween = Some(WidthTween::new(from, target));
            self.session_info_shown = shown;
        }
        let reveal = self.eval_tween(self.session_info_tween, target);
        // Hiding as the column stops fitting, the card keeps its docked look
        // and the transcript eases back from the last room it held.
        let docked = self.session_info_docked;
        let reserve = if docked {
            reserve.unwrap_or(self.session_info_last_reserve) * reveal
        } else {
            0.0
        };
        (docked, reveal, reserve)
    }

    /// The card, positioned within the main column; `None` while closed or
    /// with no chat selected.
    pub(super) fn render_session_info(
        &mut self,
        docked: bool,
        reveal: f32,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if reveal <= 0.001 {
            // Nothing shows the totals: stop streaming every checkout's patch.
            self.session_diffs = SessionDiffs::default();
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let now = Utc::now();
        let (chat, device, local, online, project, branch) = {
            let state = self.state.read(cx);
            let chat = state.selected_chat_row()?.clone();
            let device: SharedString = state
                .device_name(&chat.device_id)
                .unwrap_or("Unknown device")
                .to_owned()
                .into();
            let local = state.local_device_id.as_deref() == Some(chat.device_id.as_str());
            let online = state.device_online(&chat.device_id, now);
            let project = chat
                .space_id
                .as_deref()
                .and_then(|id| state.space_row(id))
                .map(|space| {
                    (
                        SharedString::from(space.display_name().to_owned()),
                        SharedString::from(space.path.clone()),
                    )
                });
            let branch = crate::change_requests::conversation_branch(&chat, &state.spaces)
                .map(str::trim)
                .filter(|branch| !branch.is_empty())
                .map(|branch| SharedString::from(branch.to_owned()));
            (chat, device, local, online, project, branch)
        };

        // ── Where it runs.
        let device_row = info_row(
            &theme,
            row_glyph(if local { icons::LAPTOP } else { icons::MONITOR }, &theme),
            device,
        )
        .when(!online, |row| {
            row.child(
                icon(icons::WIFI_OFF)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.warning.opacity(0.8)),
            )
        });
        let project_row = match project {
            Some((name, path)) => info_row(
                &theme,
                self.render_project_icon(&chat.id, 16.0, false, cx),
                name,
            )
            .id("session-info-project")
            .tooltip(crate::settings::widgets::text_tooltip(path))
            .into_any_element(),
            None => info_row(&theme, row_glyph(icons::HOME, &theme), "No project".into())
                .into_any_element(),
        };
        self.ensure_session_diffs(cx);
        let totals = self.session_diffs.totals(&chat);
        let branch_row = branch.map(|branch| {
            info_row(&theme, row_glyph(icons::GIT_BRANCH, &theme), branch.clone())
                .id("session-info-branch")
                .tooltip(crate::settings::widgets::text_tooltip(branch))
                // What the checkout holds uncommitted; it opens the diff.
                .children(totals.map(|(adds, dels)| {
                    let stat = |text: String, color: gpui::Hsla| {
                        div()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(11.0))
                            .text_color(color)
                            .child(SharedString::from(text))
                    };
                    div()
                        .id("session-info-diff")
                        .debug_selector(|| "session-info-diff".into())
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(5.0))
                        .mr(px(-5.0))
                        .py(px(2.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(crate::theme::ink(0.08)))
                        .tooltip(crate::settings::widgets::text_tooltip("Uncommitted changes"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_session_info_overlay(cx);
                            this.add_diff_surface(window, cx);
                        }))
                        .child(stat(format!("+{adds}"), crate::changes::add_color(&theme)))
                        .child(stat(format!("−{dels}"), crate::changes::del_color(&theme)))
                }))
        });

        // ── The project's run actions.
        let actions = self.render_session_info_actions(&theme, cx);

        // ── What runs alongside it.
        // It stops at half the window's height, however much runs.
        let lanes_height = self.viewport_height / 2.0;
        let lanes = self.render_session_lanes(&chat, !docked, lanes_height, &theme, cx);

        let body = div()
            .id("session-info-body")
            .max_h(px(max_height.max(120.0)))
            .overflow_y_scroll()
            // The card's inset lives inside the scroller, so dividers can
            // bleed through it edge to edge without being clipped.
            .p(px(popover::CARD_INSET))
            .flex()
            .flex_col()
            .gap(px(popover::MENU_GAP))
            .child(device_row)
            .child(project_row)
            .children(branch_row)
            .children(actions);

        // The session card keeps its height; only the lanes under it give.
        let card = div()
            .w(px(SESSION_CARD_WIDTH))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .rounded(px(popover::CARD_RADIUS))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(popover::surface_bg(&theme))
            .when(!docked && !theme.is_frost(), |card| card.shadow_lg())
            .child(body);
        let frosted =
            |card| crate::frost::frosted(popover::CARD_RADIUS, crate::frost::MENU_BLUR, card);

        // Docked, the card slides in from the right beside the column;
        // floating, it drops in from just above. Either way it is placed from
        // the column's right edge, so it rides that edge while panes animate.
        let ease = crate::motion::RESIZE.progress(reveal.clamp(0.0, 1.0));
        let (dx, dy) = if docked {
            (12.0 * (1.0 - ease), 0.0)
        } else {
            (0.0, -6.0 * (1.0 - ease))
        };
        Some(
            div()
                .id("session-info")
                .debug_selector(|| "session-info".into())
                .absolute()
                .top(px(Theme::TITLEBAR_HEIGHT + 12.0 + dy))
                .right(px(SESSION_CARD_INSET - dx))
                // The lanes card scrolls inside what the session card leaves
                // it below the titlebar.
                .max_h(px(max_height.max(240.0)))
                .flex()
                .flex_col()
                .gap(px(SESSION_CARDS_GAP))
                .opacity(ease)
                .occlude()
                // Over the transcript it is a popover: a press elsewhere, or
                // choosing one of its rows, dismisses it.
                .when(!docked, |card| {
                    card.on_mouse_down_out(
                        cx.listener(|this, _, _, cx| this.close_session_info_overlay(cx)),
                    )
                })
                .child(frosted(card.into_any_element()))
                .child(frosted(lanes))
                .into_any_element(),
        )
    }

    /// The Actions section: a row per project action (click runs it; the
    /// gear edits it), imports from zeron.json, and Add action.
    fn render_session_info_actions(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.ensure_project_actions(cx);
        let (Some(status), Some(snapshot), Some(key)) = (
            self.project_actions.active_status().cloned(),
            self.project_actions.visible_snapshot(),
            self.project_actions.active.clone(),
        ) else {
            return Vec::new();
        };
        let mut rows: Vec<AnyElement> = vec![
            popover::picker_divider().into_any_element(),
            popover::menu_heading(theme, "Actions").into_any_element(),
        ];
        let note = |text: SharedString, color| {
            div()
                .px(px(9.0))
                .py(px(5.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        match &status {
            ProjectActionsStatus::Idle | ProjectActionsStatus::Loading => {
                rows.push(note("Loading actions…".into(), theme.text_muted));
                return rows;
            }
            ProjectActionsStatus::Unavailable { message, .. } => {
                rows.push(note(message.clone().into(), theme.text_muted));
                rows.push(
                    popover::picker_row(theme, false, false, "session-info-actions-retry")
                        .id("session-info-actions-retry")
                        .on_click(cx.listener(|this, _, _, cx| this.retry_project_actions(cx)))
                        .child(popover::picker_icon_slot(row_glyph(icons::REFRESH, theme)))
                        .child("Retry")
                        .into_any_element(),
                );
            }
            _ => {}
        }
        let can_run = status.can_run();
        for action in snapshot.actions.clone() {
            let row_id = SharedString::from(format!("session-info-action-{}", action.id));
            let group = SharedString::from(format!("session-info-action-group-{}", action.id));
            let key = key.clone();
            let run = action.clone();
            let edit = action.clone();
            let label: SharedString = if action.run_on_worktree_create {
                format!("{} (setup)", action.name).into()
            } else {
                action.name.clone().into()
            };
            rows.push(
                popover::picker_row(theme, false, false, row_id.clone())
                    .id(row_id)
                    .group(group.clone())
                    .when(!can_run, |row| row.opacity(0.5).cursor_default())
                    .when(can_run, |row| {
                        row.on_click(cx.listener(move |this, _, _, cx| {
                            this.close_session_info_overlay(cx);
                            this.run_project_action(&key, run.clone(), cx)
                        }))
                    })
                    .child(popover::picker_icon_slot(row_glyph(
                        action_icon(action.icon),
                        theme,
                    )))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "session-info-edit-action-{}",
                                edit.id
                            )))
                            .size(px(22.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .hover(|style| style.bg(crate::theme::ink(0.08)))
                            .tooltip(crate::settings::widgets::text_tooltip("Edit action"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_session_info_overlay(cx);
                                this.open_project_action_editor(Some(edit.clone()), None, cx)
                            }))
                            .child(
                                icon(icons::SETTINGS_MINIMALISTIC)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            ),
                    )
                    .into_any_element(),
            );
        }
        for draft in snapshot.importable_actions.clone() {
            let row_id = SharedString::from(format!("session-info-import-{}", draft.name));
            let import = draft.clone();
            rows.push(
                popover::picker_row(theme, false, false, row_id.clone())
                    .id(row_id)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_session_info_overlay(cx);
                        this.open_project_action_editor(None, Some(import.clone()), cx)
                    }))
                    .child(popover::picker_icon_slot(row_glyph(
                        action_icon(draft.icon),
                        theme,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(draft.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child("Import"),
                    )
                    .into_any_element(),
            );
        }
        if let Some(issue) = snapshot.project_file_issue.clone() {
            rows.push(note(issue.into(), theme.text_muted));
        }
        rows.push(
            popover::picker_row(theme, false, false, "session-info-add-action")
                .id("session-info-add-action")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_session_info_overlay(cx);
                    this.open_project_action_editor(None, None, cx)
                }))
                .child(popover::picker_icon_slot(row_glyph(icons::PLUS, theme)))
                .child("Add action")
                .into_any_element(),
        );
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_column_keeps_the_transcript_centered_beside_the_card() {
        assert_eq!(session_card_reserve(1600.0, 736.0), Some(0.0));
        // The centered column's right edge clears the card by the gap.
        let main = 1600.0;
        let card_left = main - SESSION_CARD_WIDTH - SESSION_CARD_INSET;
        assert!(main / 2.0 + 736.0 / 2.0 <= card_left - SESSION_CARD_GAP);
    }

    #[test]
    fn a_tighter_column_shifts_the_transcript_just_enough() {
        let main = 1200.0;
        let reserve = session_card_reserve(main, 736.0).expect("it still fits");
        assert!(reserve > 0.0);
        let card_left = main - SESSION_CARD_WIDTH - SESSION_CARD_INSET;
        let right = (main - reserve) / 2.0 + 736.0 / 2.0;
        assert!((right - (card_left - SESSION_CARD_GAP)).abs() < 0.01);
        // At full width: the column keeps its configured width.
        assert!(main - reserve - 2.0 * TRANSCRIPT_GUTTER >= 736.0);
    }

    #[test]
    fn the_card_stops_fitting_where_the_transcript_would_narrow() {
        // Gutter, column, gap, card, inset.
        let least =
            TRANSCRIPT_GUTTER + 736.0 + SESSION_CARD_GAP + SESSION_CARD_WIDTH + SESSION_CARD_INSET;
        assert!(session_card_reserve(least, 736.0).is_some());
        assert!(session_card_reserve(least - 1.0, 736.0).is_none());
        assert!(session_card_reserve(700.0, 736.0).is_none());
        // A narrower reading column leaves room sooner.
        assert!(session_card_reserve(least - 1.0, 560.0).is_some());
    }
}
