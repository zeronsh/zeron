//! Experimental transcript editing (see [`crate::transcript_edit`]): the
//! editor takes the selected chat's transcript slot, and its Save bar takes
//! the composer's.
use super::*;
use crate::transcript_edit::{EditAnchor, TranscriptEditor, TranscriptEditorEvent};
use zeron_proto::native_history::NativeRole;

pub(super) struct TranscriptEditing {
    pub(super) editor: Entity<TranscriptEditor>,
    _events: Subscription,
}

impl Shell {
    /// Whether the chat's menu offers "Edit transcript": the experiment is
    /// on and the chat's agent keeps a history the host can rewrite.
    pub(super) fn can_edit_transcript(&self, chat_id: &str, cx: &App) -> bool {
        crate::settings::experimental_transcript_editing(cx)
            && self
                .state
                .read(cx)
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .and_then(|chat| chat.config.as_ref())
                .is_some_and(|config| crate::transcript_edit::supports(config.harness))
    }

    /// "Edit transcript" from a message in the open chat: the editor opens
    /// with that message where it is on screen.
    pub(super) fn edit_transcript_from(
        &mut self,
        entry_id: &str,
        content_top: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        let anchor = {
            let state = self.state.read(cx);
            let Some(at) = state.transcript.iter().position(|e| e.id == entry_id) else {
                return;
            };
            let entry = &state.transcript[at];
            let role = match entry.role {
                zeron_doc::MessageRole::User => NativeRole::User,
                _ => NativeRole::Assistant,
            };
            let texts = entry_texts(entry);
            let occurrence = state.transcript[..at]
                .iter()
                .filter(|e| e.role == entry.role && entry_texts(e) == texts)
                .count();
            EditAnchor {
                role,
                texts,
                occurrence,
                content_top,
            }
        };
        self.open_transcript_editor(chat_id, Some(anchor), cx);
    }

    /// Open the editor over `chat_id`, selecting it in the main area first.
    pub(super) fn open_transcript_editor(
        &mut self,
        chat_id: String,
        anchor: Option<EditAnchor>,
        cx: &mut Context<Self>,
    ) {
        self.close_chat_menu(cx);
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(chat) = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .cloned()
        else {
            return;
        };
        let Some(harness) = chat.config.as_ref().map(|config| config.harness) else {
            return;
        };
        if self.state.read(cx).selected_chat.as_deref() != Some(chat_id.as_str()) {
            self.open_chat(chat_id.clone(), cx);
        }
        let editor = cx.new(|cx| {
            TranscriptEditor::new(
                chat.id.clone(),
                chat.device_id.clone(),
                harness,
                engine,
                anchor,
                cx,
            )
        });
        let events = cx.subscribe(&editor, |this, _, event, cx| match event {
            TranscriptEditorEvent::Close => this.close_transcript_editor(cx),
        });
        self.transcript_editing = Some(TranscriptEditing {
            editor,
            _events: events,
        });
        cx.notify();
    }

    /// Leave the editor where it was: the message at the top of its view
    /// lands at the same place in the transcript.
    pub(super) fn close_transcript_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = self.transcript_editing.take() else {
            return;
        };
        let place = editing.editor.read(cx).exit_anchor(cx);
        if let Some((role, texts, content_top)) = place {
            let norm = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
            let wanted: Vec<String> = texts.iter().map(|t| norm(t)).collect();
            let entry_id = self
                .state
                .read(cx)
                .transcript
                .iter()
                .find(|entry| {
                    let same_role = match role {
                        NativeRole::User => entry.role == zeron_doc::MessageRole::User,
                        NativeRole::Assistant => entry.role == zeron_doc::MessageRole::Assistant,
                    };
                    same_role
                        && entry_texts(entry).iter().map(|t| norm(t)).any(|text| {
                            wanted
                                .iter()
                                .any(|want| text.contains(want.as_str()) || want.contains(&text))
                        })
                })
                .map(|entry| entry.id.clone());
            if let Some(entry_id) = entry_id {
                self.transcript.update(cx, |transcript, cx| {
                    transcript.align_entry(&entry_id, content_top, cx)
                });
            }
        }
        self.focus_composer(cx);
        cx.notify();
    }

    /// The open editor, when it belongs to the chat on screen. An editor
    /// left behind by navigating away waits for its chat to come back.
    pub(super) fn visible_transcript_editor(&self, cx: &App) -> Option<Entity<TranscriptEditor>> {
        let editing = self.transcript_editing.as_ref()?;
        let selected = self.state.read(cx).selected_chat.clone()?;
        (editing.editor.read(cx).chat_id() == selected).then(|| editing.editor.clone())
    }
}

/// A message's non-empty text parts.
fn entry_texts(entry: &zeron_doc::SessionMessageEntry) -> Vec<String> {
    entry
        .parts
        .iter()
        .filter_map(|part| match part {
            zeron_doc::MessagePart::Text { text, .. } if !text.trim().is_empty() => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect()
}
