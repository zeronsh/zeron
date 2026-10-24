//! Session notes: one Markdown scratchpad per main chat, edited in a
//! right-pane tab. The text lives on the chat doc (`zeron_doc::notes`) and
//! this device's own engine serves it, never the chat's host, so notes stay
//! editable while the host is offline.
//!
//! Writes are compare-and-set against the hash of the text this surface last
//! saw from the engine, its base. One write is in flight at a time. Changes
//! from other devices that land meanwhile are held, then merged into the
//! editor with `rebase_notes`, which keeps both sides' typing; whatever local
//! text remains unwritten goes out with the next write.
//!
//! Snapshots are versioned (`NotesSnapshot::supersedes`). The watch reports
//! this surface's own writes back, sometimes after a newer one has landed;
//! those are no newer than the base and are dropped. Anything newer is a real
//! change and merges, even when another device restored text this surface
//! wrote before. The one exception is a write whose reply never came: when
//! the engine later reports exactly that text, the editor already holds it.
//!
//! Closing the tab while a write is in flight leaves the newest text with
//! that write (`owed`): when it returns to a surface that is gone, it lands
//! the text itself (`land_owed`).

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div, px,
};
use zeron_doc::{NotesSnapshot, NotesWrite, notes_hash, rebase_notes, single_range_diff};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent, DOCUMENT_INPUT_CONTEXT};
use crate::state::AppState;
use crate::theme::Theme;

/// Quiet time after the last keystroke before the notes are written.
const WRITE_DEBOUNCE: Duration = Duration::from_millis(750);
/// Wait before redialing a dropped watch or retrying a failed write.
const RETRY_DELAY: Duration = Duration::from_secs(2);

pub struct NotesSurface {
    chat_id: String,
    state: Entity<AppState>,
    input: Entity<ComposerInput>,
    /// The notes as the engine last reported them: the base of the next
    /// write. `None` until the first snapshot arrives.
    base: Option<NotesSnapshot>,
    /// A snapshot from another device that arrived during a write.
    held: Option<NotesSnapshot>,
    /// Hash of a write that failed without a reply: it may have landed. If
    /// the engine reports that text, the editor already holds it.
    unconfirmed: Option<String>,
    in_flight: bool,
    /// Text a flush could not send because a write was in flight. That write
    /// sends it when it returns, even if this surface is gone by then.
    owed: Arc<Mutex<Option<String>>>,
    error: Option<SharedString>,
    focus_pending: bool,
    write_task: Option<Task<()>>,
    _watch: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl NotesSurface {
    pub fn new(
        chat_id: String,
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut input = ComposerInput::with_context(
                "Notes for this session. Markdown works here.",
                DOCUMENT_INPUT_CONTEXT,
                cx,
            )
            .with_markdown()
            .with_fill_height();
            // Nothing to edit until the engine says what is there.
            input.read_only = true;
            input
        });
        let edited = cx.subscribe(&input, |this, _, event: &ComposerInputEvent, cx| {
            if matches!(event, ComposerInputEvent::Edited) && this.base.is_some() {
                this.schedule_write(WRITE_DEBOUNCE, cx);
            }
        });
        // Leaving the editor writes now rather than after the debounce.
        let blurred = cx.on_blur(&input.focus_handle(cx), window, |this, _, cx| {
            this.flush(cx);
        });
        let watch = Self::spawn_watch(chat_id.clone(), cx);
        Self {
            chat_id,
            state,
            input,
            base: None,
            held: None,
            unconfirmed: None,
            in_flight: false,
            owed: Arc::default(),
            error: None,
            focus_pending: true,
            write_task: None,
            _watch: watch,
            _subscriptions: vec![edited, blurred],
        }
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    /// Put the caret in the editor on the next frame.
    pub fn request_focus(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        cx.notify();
    }

    /// Write any unwritten edits now (tab closing, editor blurred). The write
    /// outlives this surface; one already in flight takes the text along.
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        self.write_task = None;
        if self.in_flight {
            let text = self.input.read(cx).text().to_owned();
            *self.owed.lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
            return;
        }
        self.write(cx);
    }

    fn spawn_watch(chat_id: String, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(engine) = this.update(cx, |this, cx| this.state.read(cx).engine().cloned())
                else {
                    return;
                };
                if let Some(engine) = engine {
                    let params = serde_json::json!({ "chatId": chat_id });
                    match engine
                        .client()
                        .subscribe_checked(methods::WATCH_SESSION_NOTES, params)
                        .await
                    {
                        Ok(mut rx) => {
                            while let Some(value) = rx.recv().await {
                                let snapshot: NotesSnapshot = match serde_json::from_value(value) {
                                    Ok(snapshot) => snapshot,
                                    Err(err) => {
                                        tracing::warn!(error = %err, "dropping malformed notes frame");
                                        continue;
                                    }
                                };
                                if this
                                    .update(cx, |this, cx| this.receive(snapshot, cx))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                        Err(err) => {
                            tracing::debug!(%chat_id, error = %err, "notes watch failed; retrying");
                        }
                    }
                }
                if this.update(cx, |_, _| {}).is_err() {
                    return;
                }
                cx.background_executor().timer(RETRY_DELAY).await;
            }
        })
    }

    fn receive(&mut self, remote: NotesSnapshot, cx: &mut Context<Self>) {
        let Some(base) = &self.base else {
            self.input.update(cx, |input, cx| {
                input.read_only = false;
                input.set_text(remote.text.clone(), cx);
            });
            self.base = Some(remote);
            cx.notify();
            return;
        };
        if !remote.supersedes(base) {
            return;
        }
        if self.in_flight {
            self.held = Some(remote);
            return;
        }
        self.take_remote(remote, cx);
    }

    /// A newer state from the engine: adopt it when the editor already holds
    /// its text, merge it otherwise.
    fn take_remote(&mut self, remote: NotesSnapshot, cx: &mut Context<Self>) {
        let Some(base) = &self.base else {
            return;
        };
        if !remote.supersedes(base) {
            return;
        }
        if remote.hash == base.hash || self.unconfirmed.as_deref() == Some(remote.hash.as_str()) {
            self.unconfirmed = None;
            self.base = Some(remote);
            return;
        }
        self.merge(remote, cx);
    }

    /// Fold another device's notes into the editor, keeping local edits that
    /// the engine has not seen yet.
    fn merge(&mut self, remote: NotesSnapshot, cx: &mut Context<Self>) {
        let base = self.base.take().unwrap_or_default();
        let local = self.input.read(cx).text().to_owned();
        let merged = rebase_notes(&base.text, &local, &remote.text);
        if let Some(edit) = single_range_diff(&local, &merged) {
            self.input.update(cx, |input, cx| {
                input.apply_external_edit(edit.range, &edit.insert, cx)
            });
        }
        let unwritten = merged != remote.text;
        self.base = Some(remote);
        if unwritten {
            self.schedule_write(Duration::ZERO, cx);
        }
        cx.notify();
    }

    fn schedule_write(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.write_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| this.write(cx)).ok();
        }));
    }

    fn write(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some((text, base, params)) = self.begin_write(cx) else {
            return;
        };
        let chat_id = self.chat_id.clone();
        let owed = self.owed.clone();
        // Detached: a closing tab's last write still lands.
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<NotesWrite>(methods::SET_SESSION_NOTES, params)
                .await
                .map_err(|err| err.to_string());
            let alive = this
                .update(cx, |this, cx| {
                    this.written(text.clone(), result.clone(), cx)
                })
                .is_ok();
            if !alive {
                let write = |base_hash: String, text: String| {
                    let engine = engine.clone();
                    let params = serde_json::json!({
                        "chatId": chat_id,
                        "baseHash": base_hash,
                        "text": text,
                    });
                    async move {
                        engine
                            .client()
                            .call_as::<NotesWrite>(methods::SET_SESSION_NOTES, params)
                            .await
                    }
                };
                land_owed(&owed, base, text, result, write).await;
            }
        })
        .detach();
    }

    /// The text to write and its request, if there is anything to write and
    /// no write in flight (that one checks for newer edits when it lands).
    fn begin_write(&mut self, cx: &App) -> Option<(String, NotesSnapshot, serde_json::Value)> {
        if self.in_flight {
            return None;
        }
        let base = self.base.clone()?;
        let text = self.input.read(cx).text().to_owned();
        if text == base.text {
            return None;
        }
        let params = serde_json::json!({
            "chatId": self.chat_id,
            "baseHash": base.hash,
            "text": text,
        });
        self.in_flight = true;
        Some((text, base, params))
    }

    fn written(
        &mut self,
        text: String,
        result: Result<NotesWrite, String>,
        cx: &mut Context<Self>,
    ) {
        self.in_flight = false;
        // Still here: the editor holds the owed text and writes it below.
        self.owed.lock().unwrap_or_else(|e| e.into_inner()).take();
        let mut retry = Some(Duration::ZERO);
        match result {
            Ok(NotesWrite::Applied { hash, epoch, seq }) => {
                self.error = None;
                self.unconfirmed = None;
                self.base = Some(NotesSnapshot {
                    text,
                    hash,
                    epoch,
                    seq,
                });
            }
            Ok(NotesWrite::Conflict { current }) => {
                // Read at the write: newer than anything the watch sent first.
                self.error = None;
                self.held = Some(current);
            }
            Ok(NotesWrite::TooLarge { limit }) => {
                self.error = Some(
                    format!(
                        "Notes are limited to {} KB. The latest changes aren't saved.",
                        limit / 1024
                    )
                    .into(),
                );
                // The next edit tries again; retrying the same text can't help.
                retry = None;
            }
            Err(err) => {
                tracing::debug!(chat = %self.chat_id, error = %err, "notes write failed");
                self.error = Some("Couldn't save the notes. Retrying…".into());
                self.unconfirmed = Some(notes_hash(&text));
                retry = Some(RETRY_DELAY);
            }
        }
        if let Some(remote) = self.held.take() {
            self.take_remote(remote, cx);
        }
        let unwritten = self
            .base
            .as_ref()
            .is_some_and(|base| base.text != self.input.read(cx).text());
        if unwritten && let Some(delay) = retry {
            self.schedule_write(delay, cx);
        }
        cx.notify();
    }
}

/// How many writes an orphaned flush tries before giving up on conflicts.
const OWED_ATTEMPTS: usize = 4;

/// Land the text a closed surface still owed. `sent` went out on `base` and
/// came back as `result`; the owed text is newer than `sent`. Conflicts merge
/// with `rebase_notes`, except one naming `sent` after its reply was lost:
/// that write landed, and the owed text already holds it.
async fn land_owed<W, F>(
    owed: &Mutex<Option<String>>,
    base: NotesSnapshot,
    sent: String,
    result: Result<NotesWrite, String>,
    write: W,
) where
    W: Fn(String, String) -> F,
    F: Future<Output = Result<NotesWrite, zeron_rpc::RpcError>>,
{
    let Some(mut text) = owed.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let mut unconfirmed = result.is_err().then(|| notes_hash(&sent));
    let mut base = match result {
        Ok(NotesWrite::Applied { hash, epoch, seq }) => NotesSnapshot {
            text: sent,
            hash,
            epoch,
            seq,
        },
        // Lost or refused: start from the old base; a conflict sorts it out.
        Ok(NotesWrite::Conflict { .. }) | Err(_) => base,
        Ok(NotesWrite::TooLarge { .. }) => return,
    };
    for _ in 0..OWED_ATTEMPTS {
        if text == base.text {
            return;
        }
        match write(base.hash.clone(), text.clone()).await {
            Ok(NotesWrite::Conflict { current })
                if unconfirmed.as_deref() == Some(current.hash.as_str()) =>
            {
                unconfirmed = None;
                base = current;
            }
            Ok(NotesWrite::Conflict { current }) => {
                text = rebase_notes(&base.text, &text, &current.text);
                base = current;
            }
            Ok(NotesWrite::Applied { .. } | NotesWrite::TooLarge { .. }) => return,
            Err(err) => {
                tracing::debug!(error = %err, "owed notes write failed");
                return;
            }
        }
    }
}

impl Render for NotesSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_pending) {
            window.focus(&self.input.focus_handle(cx), cx);
        }
        let theme = Theme::of(cx);
        div()
            .id("session-notes")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().relative().child(
                    div()
                        .absolute()
                        .inset_0()
                        .px(px(20.0))
                        .pt(px(14.0))
                        .pb(px(10.0))
                        .child(self.input.clone()),
                ),
            )
            .children(self.error.clone().map(|error| {
                div()
                    .flex_none()
                    .px(px(20.0))
                    .pb(px(12.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.danger)
                    .child(error)
            }))
    }
}

impl gpui::Focusable for NotesSurface {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.input.focus_handle(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn surface(cx: &mut TestAppContext) -> (Entity<NotesSurface>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::composer::init(cx, Default::default());
        });
        let (surface, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|_| AppState::new());
            NotesSurface::new("chat".into(), state, window, cx)
        });
        (surface, cx)
    }

    /// The engine's report of `text` as its `seq`th state.
    fn at(text: &str, seq: u64) -> NotesSnapshot {
        NotesSnapshot {
            epoch: 7,
            seq,
            ..NotesSnapshot::of(text)
        }
    }

    fn type_text(surface: &mut NotesSurface, text: &str, cx: &mut Context<NotesSurface>) {
        surface.input.update(cx, |input, cx| {
            input.apply_external_edit(0..input.text().len(), text, cx)
        });
    }

    fn applied(text: &str, seq: u64) -> Result<NotesWrite, String> {
        Ok(NotesWrite::Applied {
            hash: notes_hash(text),
            epoch: 7,
            seq,
        })
    }

    /// The engine side of a write, on a real doc.
    fn doc_writer(
        doc: &std::rc::Rc<zeron_doc::SessionDoc>,
    ) -> impl Fn(String, String) -> std::future::Ready<Result<NotesWrite, zeron_rpc::RpcError>>
    {
        let doc = doc.clone();
        move |base_hash: String, text: String| {
            std::future::ready(Ok(doc.write_notes(&base_hash, &text).unwrap()))
        }
    }

    #[test]
    fn closing_during_a_write_still_lands_the_newest_text() {
        let doc = std::rc::Rc::new(zeron_doc::SessionDoc::init("chat").unwrap());
        doc.set_notes_text("a").unwrap();
        let base = doc.notes_snapshot();
        // "ab" is in flight when the user types "c" and closes the tab.
        let sent = doc.write_notes(&base.hash, "ab").unwrap();
        let owed = Mutex::new(Some("abc".to_owned()));
        futures::executor::block_on(land_owed(
            &owed,
            base,
            "ab".into(),
            Ok(sent),
            doc_writer(&doc),
        ));
        assert_eq!(doc.notes_text(), "abc");
        assert!(owed.lock().unwrap().is_none());
    }

    #[test]
    fn an_owed_write_merges_changes_from_elsewhere() {
        let doc = std::rc::Rc::new(zeron_doc::SessionDoc::init("chat").unwrap());
        doc.set_notes_text("mine\n").unwrap();
        let base = doc.notes_snapshot();
        let sent = doc.write_notes(&base.hash, "mine\nab\n").unwrap();
        // Another device edits before the owed text goes out.
        doc.set_notes_text("theirs\nmine\nab\n").unwrap();
        let owed = Mutex::new(Some("mine\nab\nc\n".to_owned()));
        futures::executor::block_on(land_owed(
            &owed,
            base,
            "mine\nab\n".into(),
            Ok(sent),
            doc_writer(&doc),
        ));
        assert_eq!(doc.notes_text(), "theirs\nmine\nab\nc\n");
    }

    #[test]
    fn an_owed_write_after_a_lost_reply_does_not_duplicate() {
        let doc = std::rc::Rc::new(zeron_doc::SessionDoc::init("chat").unwrap());
        doc.set_notes_text("x").unwrap();
        let base = doc.notes_snapshot();
        // "xy" landed, but its reply was lost.
        doc.write_notes(&base.hash, "xy").unwrap();
        let owed = Mutex::new(Some("xyz".to_owned()));
        futures::executor::block_on(land_owed(
            &owed,
            base,
            "xy".into(),
            Err("connection closed".into()),
            doc_writer(&doc),
        ));
        assert_eq!(doc.notes_text(), "xyz");
    }

    #[gpui::test]
    fn a_flush_during_a_write_hands_the_text_to_that_write(cx: &mut TestAppContext) {
        let (surface, cx) = surface(cx);
        surface.update(cx, |surface, cx| {
            surface.receive(at("a", 1), cx);
            type_text(surface, "ab", cx);
            let (text, _, _) = surface.begin_write(cx).unwrap();
            type_text(surface, "abc", cx);
            surface.flush(cx);
            assert_eq!(surface.owed.lock().unwrap().as_deref(), Some("abc"));
            // The surface outlived the write: it takes the text back and the
            // owed text is not sent twice.
            surface.written(text, applied("ab", 2), cx);
            assert!(surface.owed.lock().unwrap().is_none());
        });
    }

    #[gpui::test]
    fn a_late_echo_of_an_older_write_does_not_revert_the_editor(cx: &mut TestAppContext) {
        let (surface, cx) = surface(cx);
        surface.update(cx, |surface, cx| {
            surface.receive(at("a", 1), cx);
            assert!(!surface.input.read(cx).read_only);
            // First write lands; its echo is still on the way.
            type_text(surface, "ab", cx);
            let (text, _, _) = surface.begin_write(cx).unwrap();
            surface.written(text, applied("ab", 2), cx);
            // Second write goes out; the first one's echo arrives meanwhile.
            type_text(surface, "abc", cx);
            let (text, _, _) = surface.begin_write(cx).unwrap();
            surface.receive(at("ab", 2), cx);
            surface.written(text, applied("abc", 3), cx);
            // And once more after it, as a straggler.
            surface.receive(at("ab", 2), cx);
            assert_eq!(surface.input.read(cx).text(), "abc");
            assert_eq!(surface.base.as_ref().unwrap().text, "abc");
        });
    }

    #[gpui::test]
    fn a_remote_change_restoring_our_older_text_is_kept(cx: &mut TestAppContext) {
        let (surface, cx) = surface(cx);
        surface.update(cx, |surface, cx| {
            // This surface wrote "a", then "ab".
            surface.receive(at("", 1), cx);
            type_text(surface, "a", cx);
            let (text, _, _) = surface.begin_write(cx).unwrap();
            surface.written(text, applied("a", 2), cx);
            type_text(surface, "ab", cx);
            let (text, _, _) = surface.begin_write(cx).unwrap();
            surface.written(text, applied("ab", 3), cx);
            // Another device deletes "b": the text this surface wrote before,
            // as a newer state.
            surface.receive(at("a", 4), cx);
            assert_eq!(surface.input.read(cx).text(), "a");
            // The user's next edit builds on it.
            type_text(surface, "a!", cx);
            let (text, base, params) = surface.begin_write(cx).unwrap();
            assert_eq!(text, "a!");
            assert_eq!(base.text, "a");
            assert_eq!(params["baseHash"], notes_hash("a"));
        });
    }

    #[gpui::test]
    fn another_devices_change_merges_with_unwritten_typing(cx: &mut TestAppContext) {
        let (surface, cx) = surface(cx);
        surface.update(cx, |surface, cx| {
            surface.receive(at("line\n", 1), cx);
            type_text(surface, "line\nmine\n", cx);
            surface.receive(at("theirs\nline\n", 2), cx);
            assert_eq!(surface.input.read(cx).text(), "theirs\nline\nmine\n");
            assert_eq!(surface.base.as_ref().unwrap().text, "theirs\nline\n");
            // During a write the change waits for it, then merges.
            let (text, _, _) = surface.begin_write(cx).unwrap();
            surface.receive(at("theirs\nline\nmore\n", 3), cx);
            assert_eq!(surface.input.read(cx).text(), "theirs\nline\nmine\n");
            surface.written(
                text,
                Ok(NotesWrite::Conflict {
                    current: at("theirs\nline\nmore\n", 3),
                }),
                cx,
            );
            // Their insertion first where both typed at the same spot.
            assert_eq!(surface.input.read(cx).text(), "theirs\nline\nmore\nmine\n");
        });
    }

    #[gpui::test]
    fn a_write_whose_reply_was_lost_is_adopted_not_merged(cx: &mut TestAppContext) {
        let (surface, cx) = surface(cx);
        surface.update(cx, |surface, cx| {
            surface.receive(at("x", 1), cx);
            type_text(surface, "xy", cx);
            let (text, _, _) = surface.begin_write(cx).unwrap();
            // The write landed but its reply was lost.
            surface.written(text, Err("connection closed".into()), cx);
            type_text(surface, "xyz", cx);
            let (text, _, _) = surface.begin_write(cx).unwrap();
            surface.written(
                text,
                Ok(NotesWrite::Conflict {
                    current: at("xy", 2),
                }),
                cx,
            );
            // No duplicated "y": the editor already held that write.
            assert_eq!(surface.input.read(cx).text(), "xyz");
            assert_eq!(surface.base.as_ref().unwrap().text, "xy");
            // An older text this surface wrote is not adopted that way.
            assert!(surface.unconfirmed.is_none());
        });
    }
}
