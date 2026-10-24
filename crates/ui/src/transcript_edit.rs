//! Experimental transcript editing: the conversation as the agent's own CLI
//! stores it (Claude Code's session file, Codex's thread), laid out like the
//! transcript with every reply and prompt editable in place.
//!
//! What the agent reads on its next turn is the native history, not Zeron's
//! mirror, so this view loads that history from the chat's host
//! (`GetNativeHistory`) and saves by asking the host to write an edited copy
//! and resume from it (`ApplyNativeEdits`). Reasoning is sealed by the
//! provider: Claude's thinking is signed (readable, never changeable) and
//! Codex's is encrypted (not readable). Both can only be removed.
//!
//! The editor replaces the transcript in the main column while it is open;
//! the shell swaps the composer for [`TranscriptEditor::render_bar`].

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div,
    prelude::FluentBuilder, px,
};
use zeron_proto::HarnessId;
use zeron_proto::native_history::{
    NativeEdit, NativeHistory, NativeHistoryItem, NativeItemKind, NativeRole, ReasoningSeal,
};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent, TEXT_EDITOR_CONTEXT};
use crate::icons::{self, icon};
use crate::state::EngineHandle;
use crate::theme::Theme;

/// How far an edit field's wash reaches past the text column.
const FIELD_INSET: f32 = 10.0;

/// Fields grow with their text; only a reply longer than this scrolls inside.
const FIELD_MAX_HEIGHT: f32 = 100_000.0;

/// Harnesses whose native history the host can read and rewrite.
pub fn supports(harness: HarnessId) -> bool {
    matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex)
}

#[derive(Debug, Clone)]
pub enum TranscriptEditorEvent {
    /// Leave edit mode (cancelled, or saved).
    Close,
}

enum Phase {
    Loading,
    Ready,
    Saving,
    Failed(SharedString),
}

/// The user's pending change to one native item.
#[derive(Default)]
struct Draft {
    /// Editable text items only.
    input: Option<Entity<ComposerInput>>,
    removed: bool,
    /// Readable reasoning shows its text when expanded.
    expanded: bool,
}

pub struct TranscriptEditor {
    chat_id: String,
    device_id: String,
    harness: HarnessId,
    engine: EngineHandle,
    phase: Phase,
    history: Option<NativeHistory>,
    drafts: HashMap<String, Draft>,
    /// A save or undo the host refused; the edits stay in place.
    error: Option<SharedString>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The transcript column's width, kept in step by the shell.
    column_width: f32,
    /// The message the editor opened from, kept where it was on screen.
    anchor: Option<EditAnchor>,
    /// The anchored item, until it is laid out and placed.
    pending_anchor: Option<usize>,
    /// The scroll offset right after placing the anchor.
    anchored_offset: Option<gpui::Point<gpui::Pixels>>,
    /// Room above the first item and below the last, grown so an anchored
    /// message near either end can still sit where it was on screen.
    lead_space: f32,
    tail_space: f32,
    task: Option<Task<()>>,
    _inputs: Vec<Subscription>,
}

impl EventEmitter<TranscriptEditorEvent> for TranscriptEditor {}

impl Focusable for TranscriptEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TranscriptEditor {
    /// `device_id` is the chat's host, where its agent's history lives.
    pub fn new(
        chat_id: String,
        device_id: String,
        harness: HarnessId,
        engine: EngineHandle,
        anchor: Option<EditAnchor>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut editor = Self {
            chat_id,
            device_id,
            harness,
            engine,
            phase: Phase::Loading,
            history: None,
            drafts: HashMap::new(),
            error: None,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            column_width: crate::settings::TRANSCRIPT_WIDTH_DEFAULT,
            anchor,
            pending_anchor: None,
            anchored_offset: None,
            lead_space: 0.0,
            tail_space: 0.0,
            task: None,
            _inputs: Vec::new(),
        };
        editor.load(cx);
        editor
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    pub fn set_column_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if (self.column_width - width).abs() > 0.5 {
            self.column_width = width;
            cx.notify();
        }
    }

    fn agent(&self) -> &'static str {
        match self.harness {
            HarnessId::Codex => "Codex",
            _ => "Claude Code",
        }
    }

    fn provider(&self) -> &'static str {
        match self.harness {
            HarnessId::Codex => "OpenAI",
            _ => "Anthropic",
        }
    }

    fn params(&self, extra: serde_json::Value) -> serde_json::Value {
        let mut params = serde_json::json!({
            "chatId": self.chat_id,
            "targetDeviceId": self.device_id,
        });
        if let (Some(params), serde_json::Value::Object(extra)) = (params.as_object_mut(), extra) {
            params.extend(extra);
        }
        params
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Loading;
        self.error = None;
        let engine = self.engine.clone();
        let params = self.params(serde_json::Value::Null);
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<NativeHistory>(methods::GET_NATIVE_HISTORY, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(history) => this.adopt(history, cx),
                    Err(error) => this.phase = Phase::Failed(error.to_string().into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Show `history` with no pending edits.
    fn adopt(&mut self, history: NativeHistory, cx: &mut Context<Self>) {
        let mut drafts = HashMap::new();
        let mut subscriptions = Vec::new();
        for item in &history.items {
            let mut draft = Draft::default();
            if item.editable
                && let Some(text) = item_text(item)
            {
                let input = cx.new(|cx| {
                    let mut input = ComposerInput::with_context("", TEXT_EDITOR_CONTEXT, cx)
                        .with_text_metrics(14.0, 22.0)
                        .with_viewport_height(FIELD_MAX_HEIGHT);
                    input.set_text(text.to_string(), cx);
                    input
                });
                subscriptions.push(cx.subscribe(&input, |_, _, event, cx| {
                    if matches!(event, ComposerInputEvent::Edited) {
                        cx.notify();
                    }
                }));
                draft.input = Some(input);
            }
            drafts.insert(item.id.clone(), draft);
        }
        self.drafts = drafts;
        self._inputs = subscriptions;
        self.history = Some(history);
        self.phase = Phase::Ready;
        if self.anchored_offset.is_none() {
            self.pending_anchor = self.anchor.as_ref().and_then(|a| self.anchor_item(a));
        }
    }

    /// The edits the user made, in history order.
    fn edits(&self, cx: &App) -> Vec<NativeEdit> {
        let Some(history) = &self.history else {
            return Vec::new();
        };
        history
            .items
            .iter()
            .filter_map(|item| {
                let draft = self.drafts.get(&item.id)?;
                if draft.removed {
                    return Some(NativeEdit::Remove {
                        id: item.id.clone(),
                    });
                }
                let text = draft.input.as_ref()?.read(cx).text();
                (Some(text) != item_text(item)).then(|| NativeEdit::Replace {
                    id: item.id.clone(),
                    text: text.to_string(),
                })
            })
            .collect()
    }

    fn has_empty_text(&self, cx: &App) -> bool {
        self.drafts.values().any(|draft| {
            !draft.removed
                && draft
                    .input
                    .as_ref()
                    .is_some_and(|input| input.read(cx).text().trim().is_empty())
        })
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        !self.edits(cx).is_empty()
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        cx.emit(TranscriptEditorEvent::Close);
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Ready) || self.has_empty_text(cx) {
            return;
        }
        let Some(revision) = self.history.as_ref().map(|h| h.revision.clone()) else {
            return;
        };
        let edits = self.edits(cx);
        if edits.is_empty() {
            return;
        }
        self.phase = Phase::Saving;
        self.error = None;
        let engine = self.engine.clone();
        let params = self.params(serde_json::json!({ "revision": revision, "edits": edits }));
        let agent = self.agent();
        let undo = UndoTarget {
            engine: self.engine.clone(),
            params: self.params(serde_json::Value::Null),
        };
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<NativeHistory>(methods::APPLY_NATIVE_EDITS, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(history) => {
                        this.adopt(history, cx);
                        crate::toast::show_with_action(
                            cx,
                            crate::toast::ToastKind::Success,
                            format!(
                                "Transcript updated. {agent} reads the edited version next turn."
                            ),
                            "Undo",
                            move |_, cx| undo.run(cx),
                        );
                        cx.emit(TranscriptEditorEvent::Close);
                    }
                    Err(error) => {
                        this.phase = Phase::Ready;
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn undo_last_edit(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Ready) {
            return;
        }
        self.phase = Phase::Saving;
        self.error = None;
        let engine = self.engine.clone();
        let params = self.params(serde_json::Value::Null);
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<NativeHistory>(methods::UNDO_NATIVE_EDIT, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(history) => this.adopt(history, cx),
                    Err(error) => {
                        this.phase = Phase::Ready;
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Visual fixtures: stage an edit without a keyboard.
    #[doc(hidden)]
    pub fn fixture_replace(&mut self, id: &str, text: &str, cx: &mut Context<Self>) {
        if let Some(input) = self.drafts.get(id).and_then(|draft| draft.input.clone()) {
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
    }

    #[doc(hidden)]
    pub fn fixture_remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.toggle_removed(id, cx);
    }

    #[doc(hidden)]
    pub fn fixture_expand(&mut self, id: &str, cx: &mut Context<Self>) {
        self.toggle_expanded(id, cx);
    }

    #[doc(hidden)]
    pub fn is_loaded(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }

    fn toggle_removed(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.drafts.get_mut(id) {
            draft.removed = !draft.removed;
            cx.notify();
        }
    }

    fn toggle_expanded(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.drafts.get_mut(id) {
            draft.expanded = !draft.expanded;
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let save_modifier = if cfg!(target_os = "macos") {
            keystroke.modifiers.platform
        } else {
            keystroke.modifiers.control
        };
        if keystroke.key == "s" && save_modifier {
            cx.stop_propagation();
            self.save(cx);
        } else if keystroke.key == "escape" && !self.is_dirty(cx) {
            cx.stop_propagation();
            self.cancel(cx);
        }
    }

    /// Stands in for the composer while editing: what will change, and the
    /// Cancel / Save pair. Rendered by the shell inside the composer's slot.
    pub fn render_bar(this: &Entity<Self>, width: f32, cx: &mut App) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let editor = this.read(cx);
        let edits = editor.edits(cx).len();
        let saving = matches!(editor.phase, Phase::Saving);
        let ready = matches!(editor.phase, Phase::Ready);
        let blank = editor.has_empty_text(cx);
        let can_save = ready && edits > 0 && !blank;
        let can_undo = ready && editor.history.as_ref().is_some_and(|h| h.can_undo);
        let status: SharedString = if let Some(error) = editor.error.clone() {
            error
        } else if saving {
            "Saving…".into()
        } else if blank {
            "A message can't be empty.".into()
        } else if edits == 0 {
            "Edit any message, then save.".into()
        } else if edits == 1 {
            "1 change".into()
        } else {
            format!("{edits} changes").into()
        };
        let failed = editor.error.is_some() || blank;
        let cancel = this.clone();
        let save = this.clone();
        let undo = this.clone();
        div()
            .id("transcript-edit-bar")
            .w(px(width))
            .mx_auto()
            .min_h(px(52.0))
            .px(px(Theme::SPACE_LG))
            .py(px(10.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(Theme::SPACE_SM))
            .rounded(px(Theme::BUBBLE_RADIUS + 4.0))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border)
            .child(
                icon(icons::PEN)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(crate::typography::ui_rems(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(SharedString::from("Editing transcript"))
                            .child(crate::settings::widgets::badge(&theme, "Experimental")),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(if failed {
                                theme.danger
                            } else {
                                theme.text_muted
                            })
                            .child(status),
                    ),
            )
            .when(can_undo && edits == 0, |el| {
                el.child(
                    crate::popover::btn_ghost(&theme, "Undo last edit", "transcript-edit-undo")
                        .id("transcript-edit-undo")
                        .flex_none()
                        .on_click(move |_, _, cx| {
                            undo.update(cx, |editor, cx| editor.undo_last_edit(cx))
                        }),
                )
            })
            .child(
                crate::popover::btn_ghost(&theme, "Cancel", "transcript-edit-cancel")
                    .id("transcript-edit-cancel")
                    .flex_none()
                    .on_click(move |_, _, cx| cancel.update(cx, |editor, cx| editor.cancel(cx))),
            )
            .child(
                crate::popover::btn_primary(&theme, if saving { "Saving…" } else { "Save" })
                    .id("transcript-edit-save")
                    .flex_none()
                    .when(!can_save, |el| el.opacity(0.4).cursor_default())
                    .tooltip(crate::settings::widgets::text_tooltip_above(
                        if cfg!(target_os = "macos") {
                            "Save (⌘S)"
                        } else {
                            "Save (Ctrl+S)"
                        },
                    ))
                    .on_click(move |_, _, cx| save.update(cx, |editor, cx| editor.save(cx))),
            )
            .into_any_element()
    }

    fn render_item(
        &self,
        item: &NativeHistoryItem,
        content_width: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(draft) = self.drafts.get(&item.id) else {
            return gpui::Empty.into_any_element();
        };
        let edited = draft
            .input
            .as_ref()
            .is_some_and(|input| Some(input.read(cx).text()) != item_text(item) && !draft.removed);
        match (&item.kind, &draft.input) {
            (NativeItemKind::Text { .. }, Some(input)) if item.role == NativeRole::User => div()
                .w_full()
                .flex()
                .justify_end()
                .child(
                    div()
                        .min_w(px(content_width * 0.3))
                        .max_w(px(content_width * 0.8))
                        .bg(crate::theme::user_bubble_bg())
                        .rounded(px(Theme::BUBBLE_RADIUS))
                        .px(px(16.0))
                        .py(px(10.0))
                        .border_1()
                        .border_color(if edited {
                            theme.accent.opacity(0.6)
                        } else {
                            gpui::transparent_black()
                        })
                        .child(input.clone()),
                )
                .into_any_element(),
            (NativeItemKind::Text { .. }, Some(input)) => {
                editable_block(&item.id, input.clone(), edited, content_width, theme)
            }
            (NativeItemKind::CompactionSummary { .. }, Some(input)) => div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(note_line(
                    theme,
                    icons::FOLD_VERTICAL,
                    "Summary of the earlier conversation. The agent reads this instead of it.",
                ))
                .child(editable_block(
                    &item.id,
                    input.clone(),
                    edited,
                    content_width,
                    theme,
                ))
                .into_any_element(),
            (NativeItemKind::Reasoning { text, seal }, _) => {
                self.render_reasoning(item, text.as_deref(), *seal, draft, theme, cx)
            }
            (NativeItemKind::ToolCall { name, summary }, _) => {
                quiet_row(theme, icons::TERMINAL, name, summary, false)
            }
            (NativeItemKind::ToolResult { summary, is_error }, _) => {
                quiet_row(theme, icons::RETURN, "Output", summary, *is_error)
            }
            (NativeItemKind::Text { text } | NativeItemKind::CompactionSummary { text }, None) => {
                div()
                    .w_full()
                    .text_size(crate::typography::ui_rems(14.0))
                    .line_height(crate::typography::ui_rems(22.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(text.clone()))
                    .into_any_element()
            }
        }
    }

    fn render_reasoning(
        &self,
        item: &NativeHistoryItem,
        text: Option<&str>,
        seal: ReasoningSeal,
        draft: &Draft,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let removed = draft.removed;
        let readable = text.is_some_and(|t| !t.trim().is_empty());
        let (seal_icon, seal_label, seal_detail) = match seal {
            ReasoningSeal::Signed => (
                icons::KEY_MINIMALISTIC,
                "Signed",
                format!(
                    "Signed by {}. Changing it would break the signature, so it can only be removed.",
                    self.provider()
                ),
            ),
            ReasoningSeal::Encrypted => (
                icons::EYE_CLOSED,
                "Encrypted",
                format!(
                    "Encrypted by {}. Only the model can read it; you can remove it.",
                    self.provider()
                ),
            ),
        };
        let id = item.id.clone();
        let toggle_id = item.id.clone();
        let row_key = SharedString::from(format!("native-reasoning-{}", item.id));
        let header = div()
            .id(row_key.clone())
            .h(px(crate::transcript::CHIP_CARD_HEIGHT))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .when(readable, |el| {
                el.cursor_pointer().on_click(
                    cx.listener(move |this, _, _, cx| this.toggle_expanded(&toggle_id, cx)),
                )
            })
            .child(
                icon(icons::CHAT_ROUND_LINE)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.text_muted)
                    .when(removed, |el| el.line_through())
                    .child(SharedString::from("Thought process")),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{row_key}-seal")))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(6.0))
                    .h(px(18.0))
                    .rounded(px(5.0))
                    .bg(crate::theme::wash(0.06))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_faint)
                    .tooltip(crate::settings::widgets::text_tooltip(seal_detail))
                    .child(icon(seal_icon).size(px(11.0)).text_color(theme.text_faint))
                    .child(SharedString::from(seal_label)),
            )
            .when(readable, |el| {
                el.child(
                    icon(if draft.expanded {
                        icons::ALT_ARROW_UP
                    } else {
                        icons::ALT_ARROW_DOWN
                    })
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_faint),
                )
            })
            .child(div().flex_1())
            .when(removed, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_faint)
                        .child(SharedString::from("Removed when you save")),
                )
            })
            .when(item.removable, |el| {
                let key = format!("{row_key}-remove");
                el.child(
                    crate::popover::btn_ghost(
                        theme,
                        if removed { "Keep" } else { "Remove" },
                        key.clone(),
                    )
                    .id(SharedString::from(key))
                    .flex_none()
                    .py(px(3.0))
                    .px(px(8.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_removed(&id, cx)
                    })),
                )
            });
        div()
            .w_full()
            .flex()
            .flex_col()
            .when(removed, |el| el.opacity(0.55))
            .child(header)
            .when(readable && draft.expanded, |el| {
                el.child(
                    div()
                        .pl(px(22.0))
                        .pb(px(6.0))
                        .text_size(crate::typography::ui_rems(13.0))
                        .line_height(crate::typography::ui_rems(20.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(text.unwrap_or_default().to_string())),
                )
            })
            .into_any_element()
    }

    fn render_body(
        &self,
        theme: &Theme,
        content_width: f32,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let agent = self.agent();
        match &self.phase {
            Phase::Loading => {
                return vec![centered_note(
                    theme,
                    format!("Reading what {agent} remembers…"),
                )];
            }
            Phase::Failed(error) => {
                return vec![
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .max_w(px(420.0))
                                .text_center()
                                .text_size(crate::typography::ui_rems(13.0))
                                .text_color(theme.text_muted)
                                .child(error.clone()),
                        )
                        .child(
                            crate::popover::btn_ghost(theme, "Try again", "transcript-edit-retry")
                                .id("transcript-edit-retry")
                                .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                        )
                        .into_any_element(),
                ];
            }
            Phase::Ready | Phase::Saving => {}
        }
        let Some(history) = &self.history else {
            return vec![gpui::Empty.into_any_element()];
        };
        // Every note and item is a direct child of the scroll container, so
        // the scroll handle can place any item (see `Self::item_child`).
        let row = |el: AnyElement| div().flex_none().w(px(content_width)).mx_auto().child(el);
        let mut children = vec![
            div().flex_none().h(px(self.lead_space)).into_any_element(),
            row(note_line(
                theme,
                icons::INFO_CIRCLE,
                &format!(
                    "This is what {agent} reads on its next turn. Edits change its memory of the \
                     chat, not the files it touched."
                ),
            ))
            .into_any_element(),
        ];
        if history.compacted {
            children.push(
                row(note_line(
                    theme,
                    icons::FOLD_VERTICAL,
                    &format!(
                        "{agent} compacted earlier messages, so editing starts at its summary."
                    ),
                ))
                .into_any_element(),
            );
        }
        if history.items.is_empty() {
            children
                .push(row(centered_note(theme, "Nothing to edit yet.".into())).into_any_element());
        }
        let mut turn = None;
        for item in &history.items {
            let new_turn =
                turn.is_some() && turn != Some(item.turn) && item.role == NativeRole::User;
            turn = Some(item.turn);
            children.push(
                row(self.render_item(item, content_width, theme, cx))
                    .when(new_turn, |el| el.pt(px(10.0)))
                    .into_any_element(),
            );
        }
        children.push(div().flex_none().h(px(self.tail_space)).into_any_element());
        children
    }

    /// The scroll container's child index of `items[ix]`: the notes come
    /// first.
    fn item_child(&self, ix: usize) -> usize {
        let notes = 1 + self.history.as_ref().is_some_and(|h| h.compacted) as usize;
        1 + notes + ix
    }

    /// The anchored message's item: the editable text of the same role whose
    /// text matches the message's, the `occurrence`th such match when the
    /// same words appear more than once.
    fn anchor_item(&self, anchor: &EditAnchor) -> Option<usize> {
        let history = self.history.as_ref()?;
        let norm = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let wanted: Vec<String> = anchor.texts.iter().map(|t| norm(t)).collect();
        let candidates = |exact: bool| -> Vec<usize> {
            history
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.role == anchor.role)
                .filter_map(|(ix, item)| Some((ix, norm(item_text(item)?))))
                .filter(|(_, text)| !text.is_empty())
                .filter(|(_, text)| {
                    wanted.iter().any(|want| {
                        if exact {
                            want == text
                        } else {
                            want.contains(text.as_str()) || text.contains(want.as_str())
                        }
                    })
                })
                .map(|(ix, _)| ix)
                .collect()
        };
        let exact = candidates(true);
        let matches = if exact.is_empty() {
            candidates(false)
        } else {
            exact
        };
        matches.get(anchor.occurrence).or(matches.last()).copied()
    }

    /// Land the anchored item at its message's screen position once laid
    /// out, and put the caret in it.
    fn apply_anchor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.pending_anchor else {
            // Opened without an anchor: the first laid-out position is the
            // baseline closing compares against.
            if self.anchored_offset.is_none() && self.history.is_some() {
                if self.scroll.bounds_for_item(0).is_some() {
                    self.anchored_offset = Some(self.scroll.offset());
                } else {
                    window.request_animation_frame();
                }
            }
            return;
        };
        let Some(content_top) = self.anchor.as_ref().map(|a| a.content_top) else {
            self.pending_anchor = None;
            return;
        };
        let Some(bounds) = self.scroll.bounds_for_item(self.item_child(ix)) else {
            window.request_animation_frame();
            return;
        };
        let delta = f32::from(bounds.top()) - content_top;
        let offset = self.scroll.offset();
        let target = f32::from(offset.y) - delta;
        let max = f32::from(self.scroll.max_offset().y);
        // Past either end the scroll range can't reach: make room, then
        // place it on the next frame.
        if target > 0.5 {
            self.lead_space += target;
            self.scroll.set_offset(gpui::point(offset.x, px(0.0)));
            window.request_animation_frame();
            cx.notify();
            return;
        }
        if -target > max + 0.5 {
            self.tail_space += -target - max;
            window.request_animation_frame();
            cx.notify();
            return;
        }
        self.scroll.set_offset(gpui::point(offset.x, px(target)));
        self.pending_anchor = None;
        let id = self.history.as_ref().map(|h| h.items[ix].id.clone());
        if let Some(input) = id.and_then(|id| self.drafts.get(&id)?.input.clone()) {
            input.update(cx, |input, cx| input.focus_handle(cx).focus(window, cx));
        }
        window.request_animation_frame();
        self.anchored_offset = Some(self.scroll.offset());
    }

    /// Where the editor sits when it closes: the first text item still in
    /// view, its current words and its screen y. None when nothing moved
    /// since it opened, so the transcript stays exactly as it was.
    pub fn exit_anchor(&self, cx: &App) -> Option<(NativeRole, Vec<String>, f32)> {
        if self.anchored_offset == Some(self.scroll.offset()) {
            return None;
        }
        let history = self.history.as_ref()?;
        let top = f32::from(self.scroll.bounds().top()) + Theme::TITLEBAR_HEIGHT;
        history.items.iter().enumerate().find_map(|(ix, item)| {
            let bounds = self.scroll.bounds_for_item(self.item_child(ix))?;
            if f32::from(bounds.bottom()) <= top {
                return None;
            }
            let draft = self.drafts.get(&item.id)?;
            // The words as edited and as they were: Zeron's transcript takes
            // the edit asynchronously, so either may be what it shows.
            let original = item_text(item)?.to_string();
            let mut texts = vec![original];
            if let Some(input) = &draft.input {
                texts.insert(0, input.read(cx).text().to_string());
            }
            Some((item.role, texts, f32::from(bounds.top())))
        })
    }
}

/// The message an editor opened from: its role, its words and where its
/// content starts on screen.
#[derive(Debug, Clone)]
pub struct EditAnchor {
    pub role: NativeRole,
    /// The message's text parts.
    pub texts: Vec<String>,
    /// How many earlier messages of the same role carry the same words.
    pub occurrence: usize,
    pub content_top: f32,
}

impl Render for TranscriptEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_anchor(window, cx);
        let theme = Theme::of(cx).clone();
        let content_width = self.column_width;
        let body = self.render_body(&theme, content_width, cx);
        div()
            .id("transcript-editor")
            .key_context("TranscriptEditor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .pt(px(Theme::TITLEBAR_HEIGHT + 12.0))
            .pb(px(160.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .children(body)
    }
}

/// The host's undo, runnable from a toast after the editor closed.
struct UndoTarget {
    engine: EngineHandle,
    params: serde_json::Value,
}

impl UndoTarget {
    fn run(&self, cx: &mut App) {
        let engine = self.engine.clone();
        let params = self.params.clone();
        cx.spawn(async move |cx| {
            let result = engine
                .client()
                .call(methods::UNDO_NATIVE_EDIT, params)
                .await;
            let _ = cx.update(|cx| match result {
                Ok(_) => crate::toast::success(cx, "Edit undone"),
                Err(error) => crate::toast::error(cx, error.to_string()),
            });
        })
        .detach();
    }
}

fn item_text(item: &NativeHistoryItem) -> Option<&str> {
    match &item.kind {
        NativeItemKind::Text { text } | NativeItemKind::CompactionSummary { text } => Some(text),
        _ => None,
    }
}

/// An assistant reply as an edit field: reads as transcript text at rest,
/// shows its field on hover and focus, and keeps an accent edge once changed.
fn editable_block(
    id: &str,
    input: Entity<ComposerInput>,
    edited: bool,
    width: f32,
    theme: &Theme,
) -> AnyElement {
    // The field's wash overhangs the column so its text stays aligned with
    // the rows around it.
    div()
        .id(SharedString::from(format!("native-text-{id}")))
        .relative()
        .w(px(width + 2.0 * FIELD_INSET))
        .ml(px(-FIELD_INSET))
        .px(px(FIELD_INSET))
        .py(px(6.0))
        .rounded(px(8.0))
        .when(edited, |el| el.bg(theme.accent_wash.opacity(0.35)))
        .hover(|s| s.bg(crate::theme::wash(0.04)))
        .child(input)
        .when(edited, |el| {
            el.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(6.0))
                    .bottom(px(6.0))
                    .w(px(2.0))
                    .rounded(px(1.0))
                    .bg(theme.accent),
            )
        })
        .into_any_element()
}

fn quiet_row(
    theme: &Theme,
    glyph: &'static str,
    label: &str,
    detail: &str,
    error: bool,
) -> AnyElement {
    div()
        .w_full()
        .h(px(24.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            icon(glyph)
                .size(px(13.0))
                .flex_none()
                .text_color(theme.text_faint),
        )
        .child(
            div()
                .flex_none()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(theme.font_mono.clone())
                .text_size(crate::typography::ui_rems(11.5))
                .text_color(if error {
                    theme.danger
                } else {
                    theme.text_faint
                })
                .child(SharedString::from(
                    detail.lines().next().unwrap_or("").to_string(),
                )),
        )
        .into_any_element()
}

fn note_line(theme: &Theme, glyph: &'static str, text: &str) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .text_size(crate::typography::ui_rems(12.0))
        .line_height(crate::typography::ui_rems(18.0))
        .text_color(theme.text_faint)
        .child(
            div()
                .h(crate::typography::ui_rems(18.0))
                .flex()
                .items_center()
                .child(
                    icon(glyph)
                        .size(px(13.0))
                        .flex_none()
                        .text_color(theme.text_faint),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(SharedString::from(text.to_string())),
        )
        .into_any_element()
}

fn centered_note(theme: &Theme, text: String) -> AnyElement {
    div()
        .w_full()
        .h(px(240.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(crate::typography::ui_rems(13.0))
        .text_color(theme.text_muted)
        .child(SharedString::from(text))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use gpui::TestAppContext;

    use super::*;

    fn item(id: &str, role: NativeRole, turn: u32, kind: NativeItemKind) -> NativeHistoryItem {
        let editable = matches!(kind, NativeItemKind::Text { .. });
        let removable = matches!(kind, NativeItemKind::Reasoning { .. });
        NativeHistoryItem {
            id: id.into(),
            role,
            turn,
            kind,
            editable,
            removable,
        }
    }

    fn history(revision: &str, can_undo: bool) -> NativeHistory {
        NativeHistory {
            chat_id: "chat".into(),
            harness: "claude-code".into(),
            session_id: "session".into(),
            revision: revision.into(),
            items: vec![
                item(
                    "u0",
                    NativeRole::User,
                    0,
                    NativeItemKind::Text {
                        text: "Pick a code word".into(),
                    },
                ),
                item(
                    "r0",
                    NativeRole::Assistant,
                    0,
                    NativeItemKind::Reasoning {
                        text: None,
                        seal: ReasoningSeal::Encrypted,
                    },
                ),
                item(
                    "t0",
                    NativeRole::Assistant,
                    0,
                    NativeItemKind::ToolCall {
                        name: "Bash".into(),
                        summary: "ls".into(),
                    },
                ),
                item(
                    "a0",
                    NativeRole::Assistant,
                    0,
                    NativeItemKind::Text {
                        text: "The code word is APPLE.".into(),
                    },
                ),
            ],
            compacted: false,
            can_undo,
        }
    }

    /// The host side of the three native-history methods; records calls.
    struct NativeRpc {
        calls: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    }

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for NativeRpc {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            self.calls
                .lock()
                .unwrap()
                .push((method.to_string(), params.clone()));
            match method {
                methods::GET_NATIVE_HISTORY => zeron_rpc::RpcReply::value(&history("r1", false)),
                methods::APPLY_NATIVE_EDITS if params["revision"] == "r1" => {
                    zeron_rpc::RpcReply::value(&history("r2", true))
                }
                methods::APPLY_NATIVE_EDITS => Err(zeron_rpc::RpcError::Failed(
                    "The conversation changed since you started editing".into(),
                )),
                _ => Err(zeron_rpc::RpcError::UnknownMethod(method.into())),
            }
        }
    }

    /// Drive the in-memory host on this thread between GPUI passes, so its
    /// replies wake the editor deterministically.
    fn settle(
        cx: &mut gpui::VisualTestContext,
        runtime: &tokio::runtime::Runtime,
        done: impl Fn(&mut gpui::VisualTestContext) -> bool,
    ) {
        for _ in 0..200 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            runtime.block_on(async {
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
            });
        }
        panic!("the editor never settled");
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[gpui::test]
    fn save_sends_replacements_and_removals_to_the_chats_host(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::composer::init(cx, Default::default());
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let engine =
            EngineHandle::from_test_client(zeron_rpc::memory_client(Arc::new(NativeRpc {
                calls: calls.clone(),
            })));
        let (editor, cx) = cx.add_window_view(|_, cx| {
            TranscriptEditor::new(
                "chat".into(),
                "host".into(),
                HarnessId::ClaudeCode,
                engine,
                None,
                cx,
            )
        });
        settle(cx, &runtime, |cx| {
            editor.read_with(cx, |editor, _| matches!(editor.phase, Phase::Ready))
        });
        editor.update(cx, |editor, cx| {
            // Only text is editable; reasoning is removable, tools neither.
            assert!(editor.drafts["u0"].input.is_some());
            assert!(editor.drafts["a0"].input.is_some());
            assert!(editor.drafts["r0"].input.is_none());
            assert!(editor.drafts["t0"].input.is_none());
            assert!(!editor.is_dirty(cx));
            let reply = editor.drafts["a0"].input.clone().unwrap();
            reply.update(cx, |input, cx| {
                input.set_text("The code word is BANANA.", cx)
            });
            editor.toggle_removed("r0", cx);
            assert_eq!(
                editor.edits(cx),
                vec![
                    NativeEdit::Remove { id: "r0".into() },
                    NativeEdit::Replace {
                        id: "a0".into(),
                        text: "The code word is BANANA.".into(),
                    },
                ]
            );
            editor.save(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear());
        settle(cx, &runtime, |cx| {
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|(method, _)| method == methods::APPLY_NATIVE_EDITS)
                && editor.read_with(cx, |editor, _| matches!(editor.phase, Phase::Ready))
        });
        let calls = calls.lock().unwrap();
        let (_, get) = &calls[0];
        assert_eq!(get["targetDeviceId"], "host");
        let (_, apply) = calls
            .iter()
            .find(|(method, _)| method == methods::APPLY_NATIVE_EDITS)
            .unwrap();
        assert_eq!(apply["chatId"], "chat");
        assert_eq!(apply["targetDeviceId"], "host");
        assert_eq!(apply["revision"], "r1");
        assert_eq!(
            apply["edits"][0],
            serde_json::json!({ "op": "remove", "id": "r0" })
        );
        assert_eq!(apply["edits"][1]["op"], "replace");
        // The saved history replaces the drafts: nothing is pending anymore.
        editor.read_with(cx, |editor, cx| {
            assert!(!editor.is_dirty(cx));
            assert!(editor.history.as_ref().unwrap().can_undo);
        });
    }

    #[gpui::test]
    fn blank_text_blocks_save(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::composer::init(cx, Default::default());
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let engine =
            EngineHandle::from_test_client(zeron_rpc::memory_client(Arc::new(NativeRpc {
                calls: calls.clone(),
            })));
        let (editor, cx) = cx.add_window_view(|_, cx| {
            TranscriptEditor::new(
                "chat".into(),
                "host".into(),
                HarnessId::Codex,
                engine,
                None,
                cx,
            )
        });
        settle(cx, &runtime, |cx| {
            editor.read_with(cx, |editor, _| matches!(editor.phase, Phase::Ready))
        });
        editor.update(cx, |editor, cx| {
            let prompt = editor.drafts["u0"].input.clone().unwrap();
            prompt.update(cx, |input, cx| input.set_text("  ", cx));
            assert!(editor.has_empty_text(cx));
            editor.save(cx);
            assert!(matches!(editor.phase, Phase::Ready));
        });
        cx.update(|window, cx| window.draw(cx).clear());
        cx.run_until_parked();
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|(method, _)| method == methods::APPLY_NATIVE_EDITS)
        );
    }

    /// Opened from a message, the editor puts that message's field where
    /// the message was on screen; closing untouched leaves the transcript
    /// where it is.
    #[gpui::test]
    fn an_anchored_editor_keeps_the_message_in_place(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::composer::init(cx, Default::default());
        });
        let engine =
            EngineHandle::from_test_client(zeron_rpc::memory_client(Arc::new(NativeRpc {
                calls: Arc::default(),
            })));
        let anchor = EditAnchor {
            role: NativeRole::Assistant,
            texts: vec!["The code word is  APPLE.".into()],
            occurrence: 0,
            content_top: 300.0,
        };
        let (editor, cx) = cx.add_window_view(|_, cx| {
            TranscriptEditor::new(
                "chat".into(),
                "host".into(),
                HarnessId::ClaudeCode,
                engine,
                Some(anchor),
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(900.0), px(360.0)));
        settle(cx, &runtime, |cx| {
            editor.read_with(cx, |editor, _| matches!(editor.phase, Phase::Ready))
        });
        // Layout first, then the anchor lands over the next frames (room
        // past either end is made one frame before placing).
        for _ in 0..6 {
            cx.update(|window, cx| window.draw(cx).clear());
            cx.run_until_parked();
        }
        assert!(editor.read_with(cx, |editor, _| editor.anchored_offset.is_some()));
        editor.read_with(cx, |editor, cx| {
            // Whitespace differences still match the reply "a0" (item 3).
            let child = editor.item_child(3);
            let top = f32::from(editor.scroll.bounds_for_item(child).unwrap().top());
            assert!(
                (top - 300.0).abs() < 1.0,
                "the reply sits at {top}, not 300"
            );
            assert_eq!(editor.exit_anchor(cx), None, "nothing moved");
        });
        // Scrolling inside the editor makes closing report the message now
        // on top, so the transcript can follow.
        editor.update(cx, |editor, cx| {
            let offset = editor.scroll.offset();
            editor
                .scroll
                .set_offset(gpui::point(offset.x, offset.y - px(60.0)));
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let place = editor.read_with(cx, |editor, cx| editor.exit_anchor(cx));
        assert!(place.is_some(), "a scrolled editor reports where it is");
    }
}
