//! Session notes as a right-pane tab: one per main chat, opened from the
//! session card.

use super::*;
use crate::notes::NotesSurface;

impl Shell {
    /// The chat whose notes can open here: the main chat on screen. Side
    /// chats have none of their own; a fork already carries its source's
    /// notes for when it becomes a main chat.
    pub(super) fn notes_chat(&self, cx: &App) -> Option<String> {
        if !matches!(self.route, Route::Chat) || self.active_chat.is_empty() {
            return None;
        }
        let chat = self.state.read(cx).selected_chat_row()?;
        chat.is_top_level().then(|| chat.id.clone())
    }

    /// Open the main chat's notes in its pane, or activate their tab.
    pub(super) fn open_session_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat_id) = self.notes_chat(cx) else {
            return;
        };
        let key = self.panel_key(cx);
        let existing = self.right_tabs.get(&key).and_then(|tabs| {
            tabs.iter().copied().find(|surface| match surface {
                RightSurface::Notes(id) => self
                    .notes_surfaces
                    .get(id)
                    .is_some_and(|notes| notes.read(cx).chat_id() == chat_id),
                _ => false,
            })
        });
        let surface = match existing {
            Some(surface) => surface,
            None => {
                self.notes_surface_seq += 1;
                let id = self.notes_surface_seq;
                let state = self.state.clone();
                let notes = cx.new(|cx| NotesSurface::new(chat_id, state, window, cx));
                self.notes_surfaces.insert(id, notes);
                let surface = RightSurface::Notes(id);
                self.right_tabs.entry(key).or_default().push(surface);
                surface
            }
        };
        self.set_surfaces_open(true, cx);
        self.set_right_active(surface, cx);
        cx.notify();
    }
}
