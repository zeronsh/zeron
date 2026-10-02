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

pub(crate) use zeron_proto::todo_view::{FoldSide, TodoPanelState, TodoRow, TodoSummary, rows};

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
const ROW_RADIUS: f32 = 8.0;
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
        if panel.is_dismissed(&items, summary.finished()) {
            return None;
        }
        let expanded = panel.is_expanded(summary.finished());
        let (show_earlier, show_later, epoch) = (panel.show_earlier, panel.show_later, panel.epoch);

        let theme = Theme::of(cx).clone();
        let view = cx.entity_id();
        let header = self.todo_header(&chat_id, &items, summary, expanded, settled, &theme, cx);

        let surface = crate::queue::queue_panel_surface(&theme).child(header);
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
            .fade_overflow_y(&self.todo_scroll)
            .outset_bottom(TEXT_SIZE);
            surface.child(
                div()
                    .mt(px(2.0))
                    // The tray's lowest strip hides behind whatever follows
                    // (the queue tray or the composer); keep the last row clear
                    // of that edge.
                    .pb(px(crate::goal_panel::BODY_BOTTOM_CLEARANCE))
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
    /// list, plus a dismiss once everything is done.
    #[allow(clippy::too_many_arguments)]
    fn todo_header(
        &self,
        chat_id: &str,
        items: &[TodoItem],
        summary: TodoSummary,
        expanded: bool,
        settled: bool,
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
            .when(settled, |row| {
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
            origin: None,
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
            origin: None,
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
