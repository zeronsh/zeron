//! The agent's checklist, docked above the composer.
//!
//! Every harness that plans (Claude's TodoWrite, OpenCode, ACP and Codex
//! plans) emits the *whole* list on each update, so the panel simply tracks the
//! latest `Todo` tool part of the selected chat. It sits in the same tray stack
//! as the message queue (`queue.rs`), one step narrower, so the two read as one
//! object emerging from behind the composer.
//!
//! The first half of this file is pure — which list is current, what the header
//! says, which rows a long list folds to — so it is unit-tested without a
//! window. The second half is the gpui rendering on [`Composer`].

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::time::Duration;

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::*, px,
};

use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zeron_proto::{TodoItem, TodoStatus, ToolCall};

use crate::composer::{Composer, QUEUE_COMPOSER_OVERLAP};
use crate::icons::{self, icon};
use crate::motion::{self, AnimationExt as _};
use crate::theme::Theme;

/// Lists longer than this fold to a [`FOCUS_WINDOW`] around the current item.
pub(crate) const FOLD_ABOVE: usize = 6;
pub(crate) const FOCUS_WINDOW: usize = 3;

/// The checklist the agent most recently wrote, or `None` when there is none
/// (or the latest write cleared it with an empty list).
///
/// `entries` is the selected chat's own transcript; subagent traffic lives in
/// separate docs and never reaches it. ACP plans reuse one tool id for every
/// update, so "latest part" — not "first part with this id" — is the contract.
pub(crate) fn latest_todo(entries: &[SessionMessageEntry]) -> Option<Vec<TodoItem>> {
    let items = entries
        .iter()
        .rev()
        .filter(|entry| entry.role == MessageRole::Assistant)
        .flat_map(|entry| entry.parts.iter().rev())
        .find_map(|part| match part {
            MessagePart::Tool {
                call: ToolCall::Todo { items },
                ..
            } => Some(items),
            _ => None,
        })?;
    (!items.is_empty()).then(|| items.clone())
}

/// What the collapsed header reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TodoSummary {
    pub total: usize,
    pub done: usize,
    /// First in-progress item.
    pub active: Option<usize>,
    /// First item not yet completed.
    pub next: Option<usize>,
}

impl TodoSummary {
    pub fn of(items: &[TodoItem]) -> Self {
        let mut summary = Self {
            total: items.len(),
            done: 0,
            active: None,
            next: None,
        };
        for (ix, item) in items.iter().enumerate() {
            match item.status() {
                TodoStatus::Completed => summary.done += 1,
                TodoStatus::InProgress => {
                    summary.active.get_or_insert(ix);
                    summary.next.get_or_insert(ix);
                }
                TodoStatus::Pending => {
                    summary.next.get_or_insert(ix);
                }
            }
        }
        summary
    }

    pub fn finished(&self) -> bool {
        self.total > 0 && self.done == self.total
    }

    /// The item the header names: what is being worked on, else what is next.
    pub fn headline(&self) -> Option<usize> {
        self.active.or(self.next)
    }
}

/// The slice of a long list that stays visible when folded: three items with
/// the current one in the middle (one of context before it, one after),
/// clamped to the list ends. A finished list shows its last three. Lists up to
/// [`FOLD_ABOVE`] are never folded.
pub(crate) fn focus_window(items: &[TodoItem]) -> Range<usize> {
    let total = items.len();
    if total <= FOLD_ABOVE {
        return 0..total;
    }
    let summary = TodoSummary::of(items);
    let focus = summary.headline().unwrap_or(total - 1);
    let start = focus.saturating_sub(1).min(total - FOCUS_WINDOW);
    start..start + FOCUS_WINDOW
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldSide {
    Earlier,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TodoRow {
    /// An item, by its index in the agent's list.
    Item(usize),
    /// A fold toggle standing in for `count` hidden items (`open`: they are
    /// currently revealed, and the row hides them again).
    Fold {
        side: FoldSide,
        count: usize,
        open: bool,
    },
}

/// The rows of the expanded list, top to bottom. Items always keep the
/// agent's order; folds sit at the edges of the list they stand for.
pub(crate) fn rows(items: &[TodoItem], show_earlier: bool, show_later: bool) -> Vec<TodoRow> {
    let window = focus_window(items);
    let earlier = window.start;
    let later = items.len() - window.end;
    let mut out = Vec::with_capacity(items.len() + 2);
    if earlier > 0 {
        out.push(TodoRow::Fold {
            side: FoldSide::Earlier,
            count: earlier,
            open: show_earlier,
        });
        if show_earlier {
            out.extend((0..earlier).map(TodoRow::Item));
        }
    }
    out.extend(window.clone().map(TodoRow::Item));
    if later > 0 {
        if show_later {
            out.extend((window.end..items.len()).map(TodoRow::Item));
        }
        out.push(TodoRow::Fold {
            side: FoldSide::Later,
            count: later,
            open: show_later,
        });
    }
    out
}

/// Identity of a finished list, so a dismissal holds until the agent writes a
/// different one.
fn signature(items: &[TodoItem]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for item in items {
        item.text.hash(&mut hasher);
        item.status().hash(&mut hasher);
    }
    hasher.finish()
}

/// Per-chat presentation state. In memory only — like the right-pane flags in
/// `shell::SessionPanels` it lasts for the app run, not across restarts.
#[derive(Debug, Default, Clone)]
pub(crate) struct TodoPanelState {
    /// The user's explicit choice; `None` follows the automatic rule: open
    /// while work remains, compact once everything is done.
    expanded: Option<bool>,
    show_earlier: bool,
    show_later: bool,
    /// Signature of the list the user dismissed.
    dismissed: Option<u64>,
    /// Whether the previous frame was settled, to detect the transition.
    was_settled: bool,
    /// Bumped on every toggle so the list's fade-in replays.
    epoch: u32,
}

impl TodoPanelState {
    /// Feed the frame's facts in. `settled` is "everything done and the turn
    /// idle": reaching it drops any explicit choice, so the panel tidies itself
    /// to the compact state exactly once — a later manual expand is respected.
    pub fn observe(&mut self, settled: bool) {
        if settled && !self.was_settled {
            self.expanded = None;
        }
        self.was_settled = settled;
    }

    /// A finished list is compact by default even while a new turn runs, so
    /// last turn's checklist does not pop open again each time you send.
    pub fn is_expanded(&self, finished: bool) -> bool {
        self.expanded.unwrap_or(!finished)
    }

    pub fn toggle(&mut self, finished: bool) {
        self.expanded = Some(!self.is_expanded(finished));
        self.epoch = self.epoch.wrapping_add(1);
    }

    pub fn toggle_fold(&mut self, side: FoldSide) {
        match side {
            FoldSide::Earlier => self.show_earlier = !self.show_earlier,
            FoldSide::Later => self.show_later = !self.show_later,
        }
    }

    pub fn dismiss(&mut self, items: &[TodoItem]) {
        self.dismissed = Some(signature(items));
    }

    /// A dismissal holds until the agent writes a different list. It covers
    /// unfinished lists too: one the agent abandoned (an interrupted turn, a
    /// cancelled item that never completes) must not stay pinned forever.
    pub fn is_dismissed(&self, items: &[TodoItem]) -> bool {
        self.dismissed == Some(signature(items))
    }
}

/// The latest list for the selected chat, recomputed only when its transcript
/// changes (the scan walks back from the end, but a chat with no todo walks all
/// of it, and the composer renders far more often than the transcript changes).
#[derive(Debug, Default)]
pub(crate) struct TodoCache {
    key: Option<(String, u64, usize)>,
    items: Option<Vec<TodoItem>>,
}

impl TodoCache {
    pub fn latest(
        &mut self,
        chat: &str,
        revision: u64,
        entries: &[SessionMessageEntry],
    ) -> Option<&[TodoItem]> {
        let key = (chat.to_owned(), revision, entries.len());
        if self.key.as_ref() != Some(&key) {
            self.items = latest_todo(entries);
            self.key = Some(key);
        }
        self.items.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const HEADER_HEIGHT: f32 = 32.0;
const ROW_PAD_X: f32 = 8.0;
/// Rows sit in the queue tray's surface, so they share its concentric radius.
const ROW_RADIUS: f32 = crate::queue::ROW_RADIUS;
const TEXT_SIZE: f32 = 12.5;
const TEXT_LINE: f32 = 17.0;
const GLYPH_SLOT: f32 = 14.0;
const ACTION_SIZE: f32 = 24.0;
/// One step narrower than the queue tray, which is itself inset from the
/// composer; stacked above a queue the todo tray steps in once more.
const SIDE_INSET: f32 = 16.0;

impl Composer {
    /// The checklist tray, or `None` when the chat has no todo (or the user
    /// dismissed a finished one). `below_queue`: a queue tray follows.
    pub(crate) fn render_todo_panel(
        &mut self,
        below_queue: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let live = self.run_live(cx);
        let (chat_id, items) = {
            let state = self.state.read(cx);
            let chat_id = state.selected_chat.clone()?;
            let items = self
                .todo_cache
                .latest(&chat_id, state.transcript_revision, &state.transcript)?
                .to_vec();
            (chat_id, items)
        };
        let summary = TodoSummary::of(&items);
        let settled = summary.finished() && !live;

        let panel = self.todo_panels.entry(chat_id.clone()).or_default();
        panel.observe(settled);
        if panel.is_dismissed(&items) {
            return None;
        }
        let expanded = panel.is_expanded(summary.finished());
        let (show_earlier, show_later, epoch) = (panel.show_earlier, panel.show_later, panel.epoch);

        let theme = Theme::of(cx).clone();
        let view = cx.entity_id();
        // Dismissable whenever the turn is idle, finished or not.
        let header = self.todo_header(&chat_id, &items, summary, expanded, !live, &theme, cx);

        let surface =
            crate::queue::queue_panel_surface(&theme, cx.has_active_drag()).child(header);
        let surface = if expanded {
            let list = div().flex().flex_col().children(
                rows(&items, show_earlier, show_later)
                    .into_iter()
                    .map(|row| match row {
                        TodoRow::Item(ix) => todo_item_row(ix, &items[ix], live, &theme, view, cx),
                        TodoRow::Fold { side, count, open } => {
                            self.todo_fold_row(&chat_id, side, count, open, &theme, cx)
                        }
                    }),
            );
            // Same fade-and-scroll treatment as the queue's rows; a long
            // expanded fold scrolls instead of pushing the transcript up.
            let rows = crate::edge_fade::edge_faded(
                Theme::TRANSCRIPT_FADE_BAND,
                true,
                true,
                div()
                    .id("todo-panel-rows")
                    .max_h(window.viewport_size().height * 0.3)
                    .overflow_y_scroll()
                    .track_scroll(&self.todo_scroll)
                    .child(list),
            )
            // The ramp ends at the clip edge; glyphs fade per pixel.
            .fade_overflow_y(&self.todo_scroll);
            surface.child(
                div()
                    .mt(px(2.0))
                    // The tray's lowest strip hides behind the composer; keep
                    // the last row clear of that edge.
                    .pb(px(6.0))
                    .child(rows)
                    .with_animation(
                        SharedString::from(format!("todo-rows-{epoch}")),
                        motion::FADE_QUICK.animation(),
                        |el, t| el.opacity(t),
                    ),
            )
        } else {
            surface
        };

        let inset = if below_queue {
            SIDE_INSET * 2.0
        } else {
            SIDE_INSET
        };
        Some(
            div()
                .mx(px(inset))
                // Cancel the column gap and tuck the tray behind what follows
                // (the queue tray, or the composer itself).
                .mb(px(-(Theme::SPACE_SM + QUEUE_COMPOSER_OVERLAP)))
                .child(crate::frost::frosted(
                    crate::queue::PANEL_RADIUS,
                    crate::frost::MENU_BLUR,
                    surface,
                ))
                .into_any_element(),
        )
    }

    /// `Todo · 2/5` and what is being worked on; one button that toggles the
    /// list, plus a dismiss while the turn is idle.
    #[allow(clippy::too_many_arguments)]
    fn todo_header(
        &self,
        chat_id: &str,
        items: &[TodoItem],
        summary: TodoSummary,
        expanded: bool,
        dismissable: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let label = if expanded {
            "Collapse todo list"
        } else {
            "Expand todo list"
        };
        let headline: Option<(SharedString, gpui::Hsla)> = if summary.finished() {
            Some(("All done".into(), theme.text_faint))
        } else {
            summary
                .headline()
                .map(|ix| (items[ix].text.clone().into(), theme.text_muted))
        };

        let finished = summary.finished();
        let toggle_chat = chat_id.to_owned();
        let toggle =
            div()
                .id("todo-panel-toggle")
                .role(gpui::Role::Button)
                .aria_label(label)
                .flex_1()
                .min_w_0()
                .h(px(HEADER_HEIGHT))
                .px(px(ROW_PAD_X))
                .rounded(px(ROW_RADIUS))
                .flex()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .hover(|s| s.bg(theme.element_hover))
                .tab_index(0)
                .focus_visible(move |s| s.bg(accent.opacity(0.14)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.todo_panels
                        .entry(toggle_chat.clone())
                        .or_default()
                        .toggle(finished);
                    cx.notify();
                }))
                .tooltip(crate::settings::widgets::text_tooltip(label))
                .tooltip_show_delay(Duration::from_millis(350))
                .child(
                    icon(if summary.finished() {
                        icons::CHECK
                    } else {
                        icons::CHECKLIST
                    })
                    .size(px(14.0))
                    .text_color(if summary.finished() {
                        theme.success
                    } else {
                        theme.text_muted
                    }),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_baseline()
                        .gap(px(6.0))
                        .text_size(px(TEXT_SIZE))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child("Todo"),
                        )
                        .child(div().text_color(theme.text_faint).child(SharedString::from(
                            format!("{}/{}", summary.done, summary.total),
                        ))),
                )
                // The expanded list already names every item; the headline is the
                // collapsed state's whole point.
                .when(!expanded, |el| {
                    el.when_some(headline, |el, (text, color)| {
                        el.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(TEXT_SIZE))
                                .text_color(color)
                                .child(text),
                        )
                    })
                })
                .when(expanded, |el| el.child(div().flex_1()))
                .child(
                    icon(if expanded {
                        icons::ALT_ARROW_DOWN
                    } else {
                        icons::ALT_ARROW_UP
                    })
                    .size(px(13.0))
                    .text_color(theme.text_muted.opacity(0.7)),
                );

        let dismiss_chat = chat_id.to_owned();
        let dismiss_items = items.to_vec();
        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(toggle)
            .when(dismissable, |row| {
                row.child(
                    div()
                        .id("todo-panel-dismiss")
                        .role(gpui::Role::Button)
                        .aria_label("Dismiss todo list")
                        .size(px(ACTION_SIZE))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.element_hover))
                        .tab_index(0)
                        .focus_visible(move |s| s.bg(accent.opacity(0.18)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.todo_panels
                                .entry(dismiss_chat.clone())
                                .or_default()
                                .dismiss(&dismiss_items);
                            cx.notify();
                        }))
                        .tooltip(crate::settings::widgets::text_tooltip("Dismiss"))
                        .tooltip_show_delay(Duration::from_millis(350))
                        .child(
                            icon(icons::CLOSE)
                                .size(px(11.0))
                                .text_color(theme.text_muted.opacity(0.8)),
                        ),
                )
            })
            .into_any_element()
    }

    /// "N earlier" / "N later": stands in for the items folded off one end of a
    /// long list; clicking reveals them in place, clicking again hides them.
    fn todo_fold_row(
        &self,
        chat_id: &str,
        side: FoldSide,
        count: usize,
        open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let noun = match side {
            FoldSide::Earlier => "earlier",
            FoldSide::Later => "later",
        };
        let text = SharedString::from(format!("{count} {noun}"));
        let aria = format!(
            "{} {count} {noun} items",
            if open { "Hide" } else { "Show" }
        );
        // The arrow points to where the hidden items are (or, once revealed,
        // back to where they fold away).
        let glyph = match (side, open) {
            (FoldSide::Earlier, false) | (FoldSide::Later, true) => icons::ALT_ARROW_UP,
            _ => icons::ALT_ARROW_DOWN,
        };
        let chat_id = chat_id.to_owned();
        div()
            .id(SharedString::from(format!("todo-fold-{noun}")))
            .role(gpui::Role::Button)
            .aria_label(aria)
            .h(px(24.0))
            .px(px(ROW_PAD_X))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme.element_hover))
            .tab_index(0)
            .focus_visible(move |s| s.bg(accent.opacity(0.14)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.todo_panels
                    .entry(chat_id.clone())
                    .or_default()
                    .toggle_fold(side);
                cx.notify();
            }))
            .child(
                div()
                    .w(px(GLYPH_SLOT))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(icon(glyph).size(px(12.0)).text_color(theme.text_faint)),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.text_faint)
                    .child(text),
            )
            .into_any_element()
    }
}

/// One checklist row: status glyph + text. Completed work recedes, the
/// in-progress item is the brightest thing in the list.
fn todo_item_row(
    ix: usize,
    item: &TodoItem,
    live: bool,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let status = item.status();
    let (color, weight) = match status {
        TodoStatus::Completed => (theme.text_faint, gpui::FontWeight::NORMAL),
        TodoStatus::InProgress => (theme.text, gpui::FontWeight::MEDIUM),
        TodoStatus::Pending => (theme.text_muted, gpui::FontWeight::NORMAL),
    };
    div()
        .id(SharedString::from(format!("todo-item-{ix}")))
        .min_h(px(26.0))
        .px(px(ROW_PAD_X))
        .py(px(4.0))
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(
            div()
                .w(px(GLYPH_SLOT))
                .h(px(TEXT_LINE))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(status_glyph(ix, status, live, theme, view, cx)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(TEXT_SIZE))
                .line_height(px(TEXT_LINE))
                .font_weight(weight)
                .text_color(color)
                .child(SharedString::from(item.text.clone())),
        )
        .into_any_element()
}

/// check / active / pending. An in-progress item only animates while the turn
/// is live; on an idle chat it is a still ring (the agent is not working on
/// it), so a stopped run does not look busy forever.
fn status_glyph(
    ix: usize,
    status: TodoStatus,
    live: bool,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    match status {
        TodoStatus::Completed => icon(icons::CHECK)
            .size(px(12.0))
            .text_color(theme.success)
            .into_any_element(),
        TodoStatus::InProgress if live => crate::loaders::mini_glyph_spinner(
            format!("todo-active-{ix}"),
            2.5,
            theme.glyph,
            view,
            cx,
        )
        .into_any_element(),
        TodoStatus::InProgress => div()
            .size(px(10.0))
            .rounded_full()
            .border_1()
            .border_color(theme.accent)
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(4.0)).rounded_full().bg(theme.accent))
            .into_any_element(),
        TodoStatus::Pending => div()
            .size(px(10.0))
            .rounded_full()
            .border_1()
            .border_color(theme.text_faint.opacity(0.7))
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(spec: &str) -> Vec<TodoItem> {
        // x = completed, > = in progress, . = pending
        spec.chars()
            .enumerate()
            .map(|(i, c)| {
                TodoItem::new(
                    format!("item {i}"),
                    match c {
                        'x' => TodoStatus::Completed,
                        '>' => TodoStatus::InProgress,
                        _ => TodoStatus::Pending,
                    },
                )
            })
            .collect()
    }

    fn entry(role: MessageRole, parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: "m".into(),
            role,
            parts,
            created_at: 0,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn todo_part(id: &str, list: Vec<TodoItem>) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Todo { items: list },
            is_error: false,
            resolved: true,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: None,
            subagent_status: None,
            subagent_tail: None,
        }
    }

    #[test]
    fn latest_todo_is_the_last_write_across_entries() {
        let first = entry(MessageRole::Assistant, vec![todo_part("a", items("x.."))]);
        let second = entry(
            MessageRole::Assistant,
            vec![
                todo_part("b", items("xx.")),
                MessagePart::Text {
                    id: "t".into(),
                    text: "working".into(),
                },
                todo_part("c", items("xx>")),
            ],
        );
        assert_eq!(latest_todo(&[first, second]), Some(items("xx>")));
    }

    #[test]
    fn acp_plan_reusing_one_id_resolves_to_the_newest_segment() {
        let plan = zeron_proto::LIVE_PLAN_TOOL_ID;
        let old = entry(MessageRole::Assistant, vec![todo_part(plan, items(">.."))]);
        let new = entry(MessageRole::Assistant, vec![todo_part(plan, items("x>."))]);
        assert_eq!(latest_todo(&[old, new]), Some(items("x>.")));
    }

    #[test]
    fn an_empty_write_clears_and_user_rows_are_ignored() {
        let list = entry(MessageRole::Assistant, vec![todo_part("a", items("x."))]);
        let cleared = entry(MessageRole::Assistant, vec![todo_part("b", Vec::new())]);
        assert_eq!(latest_todo(&[list.clone(), cleared]), None);
        let user = entry(MessageRole::User, vec![todo_part("u", items("."))]);
        assert_eq!(latest_todo(&[user]), None);
        assert_eq!(latest_todo(&[list]), Some(items("x.")));
        assert_eq!(latest_todo(&[]), None);
    }

    #[test]
    fn summary_counts_and_names_the_current_item() {
        let s = TodoSummary::of(&items("xx>.."));
        assert_eq!(
            (s.total, s.done, s.active, s.next),
            (5, 2, Some(2), Some(2))
        );
        assert_eq!(s.headline(), Some(2));
        assert!(!s.finished());
        // Nothing started: the first unfinished item is the headline.
        let s = TodoSummary::of(&items("x..."));
        assert_eq!((s.active, s.headline()), (None, Some(1)));
        // An in-progress item later in the list still wins over an earlier
        // pending one: the agent is working on it.
        assert_eq!(TodoSummary::of(&items(".>.")).headline(), Some(1));
        let s = TodoSummary::of(&items("xxx"));
        assert!(s.finished());
        assert_eq!(s.headline(), None);
        assert!(!TodoSummary::of(&[]).finished());
    }

    #[test]
    fn short_lists_never_fold() {
        for n in 0..=FOLD_ABOVE {
            let list = items(&".".repeat(n));
            assert_eq!(focus_window(&list), 0..n);
            assert!(
                rows(&list, false, false)
                    .iter()
                    .all(|r| matches!(r, TodoRow::Item(_)))
            );
        }
    }

    #[test]
    fn long_lists_fold_to_three_around_the_current_item() {
        // Current item in the middle: one before, it, one after.
        assert_eq!(focus_window(&items("xxxx>.....")), 3..6);
        // At the start / end the window clamps instead of shrinking.
        assert_eq!(focus_window(&items(">.......")), 0..3);
        assert_eq!(focus_window(&items("xxxxxxx>")), 5..8);
        // No in-progress item: the first unfinished one is the focus.
        assert_eq!(focus_window(&items("xxxx.....")), 3..6);
        // Everything done: the tail.
        assert_eq!(focus_window(&items("xxxxxxxx")), 5..8);
        // Always exactly three when folded.
        for spec in ["xxxxxxx>", ">.......", "xx>.....", "xxxxxxxx", "........"] {
            assert_eq!(focus_window(&items(spec)).len(), FOCUS_WINDOW, "{spec}");
        }
    }

    #[test]
    fn rows_put_folds_at_the_edges_with_counts() {
        let list = items("xxxx>.....");
        assert_eq!(
            rows(&list, false, false),
            vec![
                TodoRow::Fold {
                    side: FoldSide::Earlier,
                    count: 3,
                    open: false
                },
                TodoRow::Item(3),
                TodoRow::Item(4),
                TodoRow::Item(5),
                TodoRow::Fold {
                    side: FoldSide::Later,
                    count: 4,
                    open: false
                },
            ]
        );
        // No fold on a side with nothing hidden.
        let head = rows(&items(">......."), false, false);
        assert!(matches!(head.first(), Some(TodoRow::Item(0))));
        assert!(matches!(
            head.last(),
            Some(TodoRow::Fold {
                side: FoldSide::Later,
                count: 5,
                ..
            })
        ));
    }

    #[test]
    fn expanding_a_fold_reveals_items_without_reordering() {
        let list = items("xxxx>.....");
        let order = |rows: &[TodoRow]| -> Vec<usize> {
            rows.iter()
                .filter_map(|r| match r {
                    TodoRow::Item(ix) => Some(*ix),
                    _ => None,
                })
                .collect()
        };
        for (earlier, later) in [(false, false), (true, false), (false, true), (true, true)] {
            let shown = order(&rows(&list, earlier, later));
            assert!(shown.windows(2).all(|w| w[0] < w[1]), "{earlier} {later}");
        }
        // Both open: the whole list, in order, and each toggle still present
        // so it can be closed again.
        let all = rows(&list, true, true);
        assert_eq!(order(&all), (0..list.len()).collect::<Vec<_>>());
        assert!(matches!(
            all.first(),
            Some(TodoRow::Fold { open: true, .. })
        ));
        assert!(matches!(all.last(), Some(TodoRow::Fold { open: true, .. })));
        // Earlier items appear after their fold row, later items before theirs.
        assert_eq!(all[1], TodoRow::Item(0));
        assert_eq!(all[all.len() - 2], TodoRow::Item(9));
    }

    #[test]
    fn panel_follows_the_work_then_tidies_itself_once() {
        let mut state = TodoPanelState::default();
        // Working: open by default.
        state.observe(false);
        assert!(state.is_expanded(false));
        // A finished list stays compact when a new turn starts (not settled,
        // since the turn is live) instead of popping open again.
        assert!(!state.is_expanded(true));
        // User collapses mid-run: respected.
        state.toggle(false);
        state.observe(false);
        assert!(!state.is_expanded(false));
        // Everything done and idle: compact, and the explicit choice resets.
        state.toggle(false); // user had re-expanded
        assert!(state.is_expanded(false));
        state.observe(true);
        assert!(!state.is_expanded(true));
        // Opening the finished list by hand sticks (no re-collapse each frame).
        state.toggle(true);
        state.observe(true);
        assert!(state.is_expanded(true));
        // New work arrives: back to the automatic open state...
        state.observe(false);
        assert!(state.is_expanded(false));
        // ...and finishing again tidies again.
        state.observe(true);
        assert!(!state.is_expanded(true));
    }

    #[test]
    fn toggle_replays_the_fade_and_folds_toggle_independently() {
        let mut state = TodoPanelState::default();
        let before = state.epoch;
        state.toggle(false);
        assert_ne!(state.epoch, before);
        state.toggle_fold(FoldSide::Later);
        assert!(state.show_later && !state.show_earlier);
        state.toggle_fold(FoldSide::Later);
        assert!(!state.show_later);
    }

    #[test]
    fn dismissal_holds_only_for_the_list_it_was_made_on() {
        let done = items("xxx");
        let mut state = TodoPanelState::default();
        assert!(!state.is_dismissed(&done));
        state.dismiss(&done);
        assert!(state.is_dismissed(&done));
        // A changed list (reopened, or a new one) is shown again.
        assert!(!state.is_dismissed(&items("xx.")));
        assert!(!state.is_dismissed(&items("xxxx")));
        // An abandoned list that will never finish (interrupted turn, a
        // cancelled item) can be dismissed too, until the agent rewrites it.
        let stuck = items("x..");
        state.dismiss(&stuck);
        assert!(state.is_dismissed(&stuck));
        assert!(!state.is_dismissed(&items("xx.")));
    }

    #[test]
    fn cache_rescans_only_when_the_transcript_changes() {
        let mut cache = TodoCache::default();
        let one = vec![entry(
            MessageRole::Assistant,
            vec![todo_part("a", items("x."))],
        )];
        assert_eq!(cache.latest("chat", 1, &one), Some(items("x.").as_slice()));
        // Same chat, revision and length: served from cache, not rescanned.
        assert_eq!(cache.latest("chat", 1, &one), Some(items("x.").as_slice()));
        let two = vec![
            one[0].clone(),
            entry(MessageRole::Assistant, vec![todo_part("b", items("xx"))]),
        ];
        assert_eq!(cache.latest("chat", 2, &two), Some(items("xx").as_slice()));
        // Another chat never reuses the first chat's list.
        assert_eq!(cache.latest("other", 2, &[]), None);
    }
}

#[cfg(test)]
mod composer_tests {
    use super::*;
    use crate::state::AppState;

    fn assistant_with_todo(list: Vec<TodoItem>) -> SessionMessageEntry {
        SessionMessageEntry {
            id: "m".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Tool {
                id: "t".into(),
                call: ToolCall::Todo { items: list },
                is_error: false,
                resolved: true,
                output: None,
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
            }],
            created_at: 0,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn items(done: usize, total: usize) -> Vec<TodoItem> {
        (0..total)
            .map(|i| {
                TodoItem::new(
                    format!("step {i}"),
                    if i < done {
                        TodoStatus::Completed
                    } else {
                        TodoStatus::Pending
                    },
                )
            })
            .collect()
    }

    fn window(
        cx: &mut gpui::TestAppContext,
    ) -> (
        tempfile::TempDir,
        gpui::Entity<AppState>,
        gpui::WindowHandle<Composer>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let state = cx.new(|_| AppState::new());
        let handle = {
            let state = state.clone();
            cx.add_window(move |_, cx| Composer::new(state, cx))
        };
        (dir, state, handle)
    }

    fn show_todo(
        state: &gpui::Entity<AppState>,
        handle: &gpui::WindowHandle<Composer>,
        chat: &str,
        entries: Vec<SessionMessageEntry>,
        cx: &mut gpui::TestAppContext,
    ) {
        state.update(cx, |state, _| {
            state.selected_chat = Some(chat.into());
            state.transcript = entries;
            state.transcript_revision += 1;
        });
        handle
            .update(cx, |composer, _, _| composer.current_key = chat.into())
            .unwrap();
    }

    /// Whether the tray is on screen for the selected chat right now.
    fn rendered(handle: &gpui::WindowHandle<Composer>, cx: &mut gpui::TestAppContext) -> bool {
        handle
            .update(cx, |composer, window, cx| {
                composer.render_todo_panel(false, window, cx).is_some()
            })
            .unwrap()
    }

    #[gpui::test]
    fn tray_appears_only_when_the_chat_has_a_todo(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        show_todo(&state, &handle, "a", Vec::new(), cx);
        assert!(!rendered(&handle, cx));
        show_todo(
            &state,
            &handle,
            "a",
            vec![assistant_with_todo(items(1, 3))],
            cx,
        );
        assert!(rendered(&handle, cx));
        // The agent clearing its list (an empty write) takes the tray away.
        show_todo(
            &state,
            &handle,
            "a",
            vec![assistant_with_todo(Vec::new())],
            cx,
        );
        assert!(!rendered(&handle, cx));
    }

    #[gpui::test]
    fn open_state_is_remembered_per_chat(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        let entries = || vec![assistant_with_todo(items(1, 3))];
        show_todo(&state, &handle, "a", entries(), cx);
        assert!(rendered(&handle, cx));
        handle
            .update(cx, |composer, _, _| {
                composer.todo_panels.get_mut("a").unwrap().toggle(false);
            })
            .unwrap();
        // Another chat starts from the automatic state, and A keeps its choice.
        show_todo(&state, &handle, "b", entries(), cx);
        assert!(rendered(&handle, cx));
        show_todo(&state, &handle, "a", entries(), cx);
        assert!(rendered(&handle, cx));
        handle
            .read_with(cx, |composer, _| {
                assert!(!composer.todo_panels["a"].is_expanded(false));
                assert!(composer.todo_panels["b"].is_expanded(false));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_dismissed_finished_list_stays_gone_until_the_list_changes(cx: &mut gpui::TestAppContext) {
        let (_dir, state, handle) = window(cx);
        let finished = || vec![assistant_with_todo(items(3, 3))];
        show_todo(&state, &handle, "a", finished(), cx);
        assert!(rendered(&handle, cx));
        handle
            .update(cx, |composer, _, _| {
                composer
                    .todo_panels
                    .get_mut("a")
                    .unwrap()
                    .dismiss(&items(3, 3));
            })
            .unwrap();
        assert!(!rendered(&handle, cx));
        // A new list (more work) is shown again.
        show_todo(
            &state,
            &handle,
            "a",
            vec![assistant_with_todo(items(3, 5))],
            cx,
        );
        assert!(rendered(&handle, cx));
    }
}
