//! The session card: where this chat runs (device, project, branch), its
//! side-chat actions, and the project's run actions, in one toggleable card.
//! With room, the card docks at the right of the main column and the
//! transcript keeps to its left (staying centered when it fits); on a narrow
//! column it floats over the transcript's top-right corner instead.
use super::*;

use crate::project_actions::{ProjectActionsStatus, action_icon};

/// The card's width, its inset from the column's right edge, and the
/// least gap the reading column keeps from it.
pub(super) const SESSION_CARD_WIDTH: f32 = 272.0;
const SESSION_CARD_INSET: f32 = 12.0;
const SESSION_CARD_GAP: f32 = 24.0;
/// The transcript's own side gutters (its rows pad 48px each side).
const TRANSCRIPT_GUTTER: f32 = 48.0;
/// Below this reading-column width the card floats rather than docking.
const DOCKED_MIN_COLUMN: f32 = 480.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SessionCardLayout {
    /// Docked at the column's right (else floating over the transcript).
    pub docked: bool,
    /// Room the transcript holds free on its right while docked: none while
    /// its centered column already clears the card.
    pub reserve: f32,
    /// The card's left edge within the main column.
    pub left: f32,
}

/// Where the card sits in a main column `main` wide, for a configured
/// reading width `transcript`. Either way the card hugs the right edge.
/// Docked, the transcript stays centered on the whole column while it
/// clears the card by the gap; otherwise it shifts left just enough, and
/// only then narrows.
pub(super) fn session_card_layout(main: f32, transcript: f32) -> SessionCardLayout {
    let left = (main - SESSION_CARD_WIDTH - SESSION_CARD_INSET).max(0.0);
    // The most the transcript can give up: its right gutter then ends at
    // the gap before the card.
    let max_reserve = SESSION_CARD_WIDTH + SESSION_CARD_INSET + SESSION_CARD_GAP - TRANSCRIPT_GUTTER;
    let docked = main - 2.0 * TRANSCRIPT_GUTTER - max_reserve >= DOCKED_MIN_COLUMN;
    if !docked {
        return SessionCardLayout {
            docked: false,
            reserve: 0.0,
            left,
        };
    }
    // Centered in `main - reserve`, the column's right edge is
    // (main - reserve) / 2 + transcript / 2; keep it at `left - gap`.
    let needed = main + transcript - 2.0 * (left - SESSION_CARD_GAP);
    SessionCardLayout {
        docked: true,
        reserve: needed.clamp(0.0, max_reserve),
        left,
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
    /// The persisted open state; the card only exists for a selected chat.
    pub(super) fn session_info_open(&self) -> bool {
        self.settings.session_info_open
    }

    pub(super) fn toggle_session_info(&mut self, cx: &mut Context<Self>) {
        let from = self.session_info_reveal();
        self.settings.session_info_open = !self.settings.session_info_open;
        let to = if self.settings.session_info_open { 1.0 } else { 0.0 };
        self.session_info_tween = Some(WidthTween::new(from, to));
        self.schedule_save(cx);
        cx.notify();
    }

    /// 0 (closed) → 1 (open), easing on the shell's resize clock.
    pub(super) fn session_info_reveal(&self) -> f32 {
        let target = if self.settings.session_info_open {
            1.0
        } else {
            0.0
        };
        self.eval_tween(self.session_info_tween, target)
    }

    /// The card, positioned within the main column; `None` while closed or
    /// with no chat selected.
    pub(super) fn render_session_info(
        &mut self,
        layout: SessionCardLayout,
        reveal: f32,
        max_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if reveal <= 0.001 {
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
        let branch_row = branch.map(|branch| {
            info_row(&theme, row_glyph(icons::GIT_BRANCH, &theme), branch.clone())
                .id("session-info-branch")
                .tooltip(crate::settings::widgets::text_tooltip(branch))
        });

        // ── This conversation.
        let busy = self.side_chat_creating;
        let side_chat = popover::picker_row(&theme, false, false, "session-info-side-chat")
            .id("session-info-side-chat")
            .debug_selector(|| "session-info-side-chat".into())
            .when(busy, |row| row.opacity(0.5))
            .on_click(cx.listener(|this, _, _, cx| this.create_child_chat(None, cx)))
            .child(popover::picker_icon_slot(row_glyph(icons::PLUS, &theme)))
            .child(div().flex_1().child("New side chat"));
        let fork = popover::picker_row(&theme, false, false, "session-info-fork")
            .id("session-info-fork")
            .debug_selector(|| "session-info-fork".into())
            .when(busy, |row| row.opacity(0.5))
            .on_click(cx.listener(|this, _, _, cx| this.create_side_chat(cx)))
            .child(popover::picker_icon_slot(row_glyph(icons::FORK, &theme)))
            .child(div().flex_1().child("Fork chat"));

        // ── The project's run actions.
        let actions = self.render_session_info_actions(&theme, cx);

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
            .child(popover::picker_divider())
            .child(side_chat)
            .child(fork)
            .children(actions);

        let card = div()
            .w(px(SESSION_CARD_WIDTH))
            .rounded(px(popover::CARD_RADIUS))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(popover::surface_bg(&theme))
            .when(!layout.docked && !theme.is_frost(), |card| card.shadow_lg())
            .child(body);

        // Docked, the card slides in from the right beside the column;
        // floating, it drops in from just above. Either way it is placed from
        // the column's right edge, so it rides that edge while panes animate.
        let ease = crate::motion::RESIZE.progress(reveal.clamp(0.0, 1.0));
        let (dx, dy) = if layout.docked {
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
                .opacity(ease)
                .occlude()
                .child(crate::frost::frosted(
                    popover::CARD_RADIUS,
                    crate::frost::MENU_BLUR,
                    card,
                ))
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
                        this.open_project_action_editor(None, Some(import.clone()), cx)
                    }))
                    .child(popover::picker_icon_slot(row_glyph(
                        action_icon(draft.icon),
                        theme,
                    )))
                    .child(div().flex_1().min_w_0().truncate().child(draft.name.clone()))
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

    /// The reading column's right edge for a layout (rows pad 48 each side
    /// and center in what the reserve leaves).
    fn column_right(main: f32, transcript: f32, layout: SessionCardLayout) -> f32 {
        let column = transcript.min(main - 2.0 * TRANSCRIPT_GUTTER - layout.reserve);
        (main - layout.reserve) / 2.0 + column / 2.0
    }

    #[test]
    fn a_wide_column_keeps_the_transcript_centered_beside_the_card() {
        let layout = session_card_layout(1400.0, 736.0);
        assert!(layout.docked);
        assert_eq!(layout.reserve, 0.0, "no shift while it clears the card");
        assert_eq!(layout.left + SESSION_CARD_WIDTH + SESSION_CARD_INSET, 1400.0);
        assert!(column_right(1400.0, 736.0, layout) <= layout.left - SESSION_CARD_GAP);
    }

    #[test]
    fn a_tighter_column_shifts_the_transcript_just_enough_then_narrows_it() {
        let layout = session_card_layout(1200.0, 736.0);
        assert!(layout.docked && layout.reserve > 0.0);
        let right = column_right(1200.0, 736.0, layout);
        assert!((right - (layout.left - SESSION_CARD_GAP)).abs() < 0.01);
        // At the docking floor the column is narrower than configured but
        // still clears the card.
        let main = 2.0 * TRANSCRIPT_GUTTER
            + SESSION_CARD_WIDTH
            + SESSION_CARD_INSET
            + SESSION_CARD_GAP
            - TRANSCRIPT_GUTTER
            + DOCKED_MIN_COLUMN;
        let floor = session_card_layout(main, 736.0);
        assert!(floor.docked);
        assert!(column_right(main, 736.0, floor) <= floor.left - SESSION_CARD_GAP + 0.01);
    }

    #[test]
    fn a_narrow_column_floats_the_card_at_the_same_corner() {
        let layout = session_card_layout(700.0, 736.0);
        assert!(!layout.docked);
        assert_eq!(layout.reserve, 0.0);
        assert_eq!(layout.left + SESSION_CARD_WIDTH + SESSION_CARD_INSET, 700.0);
    }
}
