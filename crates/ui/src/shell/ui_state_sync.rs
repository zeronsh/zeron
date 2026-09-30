//! Glue between the live UI and the persisted [`crate::ui_state`] snapshot:
//! capture the selected chat, unsent composer drafts (text and staged images)
//! and which chats had the right pane open, and give them back to the next
//! window once the chat list has synced.
//!
//! Writing rides `SettingsStore`'s 400 ms debounce and quit flush
//! ([`settings::save_ui_snapshot`]); this file only decides *what* to record
//! and *when to apply* it.

use std::collections::HashSet;

use super::*;
use crate::ui_state::{DraftSnapshot, UiSnapshot};

impl Shell {
    /// Record the current UI state for persistence. Cheap and idempotent —
    /// callers report every change; identical snapshots are dropped downstream.
    /// A no-op until the previous window's snapshot has been applied.
    pub(super) fn note_ui_state(&mut self, cx: &mut Context<Self>) {
        self.capture_ui_state(false, cx);
    }

    fn capture_ui_state(&mut self, force_prune: bool, cx: &mut Context<Self>) {
        if !self.ui_state_restored {
            return;
        }
        let Some(attachments_dir) = settings::ui_attachments_dir(cx) else {
            return;
        };
        let selected = self.state.read(cx).selected_chat.clone();

        let mut snapshot = UiSnapshot::new();
        snapshot.selected_chat = selected.clone();
        let mut live_ids = HashSet::new();
        let mut wrote_copy = false;
        for (key, text, staged) in self.composer.read(cx).ui_state_drafts(cx) {
            let mut attachments = Vec::with_capacity(staged.len());
            for attachment in &staged {
                live_ids.insert(attachment.id.clone());
                if let Some(path) = self.ui_state_attachments.get(&attachment.id) {
                    attachments.push(path.clone());
                    continue;
                }
                match crate::ui_state::materialize_attachment(
                    &attachments_dir,
                    &attachment.id,
                    &attachment.name,
                    attachment.bytes(),
                ) {
                    Ok(path) => {
                        wrote_copy = true;
                        self.ui_state_attachments
                            .insert(attachment.id.clone(), path.clone());
                        attachments.push(path);
                    }
                    // The draft text is still worth keeping without the image.
                    Err(err) => tracing::warn!(
                        error = %err,
                        name = %attachment.name,
                        "could not keep a staged attachment for ui-state"
                    ),
                }
            }
            snapshot
                .drafts
                .insert(key, DraftSnapshot { text, attachments });
        }
        // Copies of attachments that were sent or removed are dead weight.
        let stale = self
            .ui_state_attachments
            .keys()
            .any(|id| !live_ids.contains(id));
        if stale {
            self.ui_state_attachments
                .retain(|id, _| live_ids.contains(id));
        }
        // Deleted only after the snapshot that stops referencing them is on
        // disk (see `settings::save_ui_snapshot`).
        let prune = (stale || wrote_copy || force_prune).then_some((attachments_dir, live_ids));
        // Only consulted for chats that have the pane open (usually none).
        let chats = &self.state.read(cx).chats;
        snapshot.right_pane = self
            .panels
            .ui_snapshot(|id| chats.iter().any(|chat| chat.id == id));

        self.ui_state_selected = selected;
        settings::save_ui_snapshot(snapshot, prune, cx);
    }

    /// Apply the previous window's snapshot once the chat list has synced:
    /// re-select its chat, give back drafts and staged images, and re-open the
    /// right pane. Runs once; runs before the generic "land on the most recent
    /// chat" boot selection so it can take precedence. Restored drafts never
    /// replace text already typed (see `Composer::restore_draft`).
    pub(super) fn restore_ui_state(&mut self, cx: &mut Context<Self>) {
        if self.ui_state_restored || !self.state.read(cx).chats_synced {
            return;
        }
        let mut snapshot = settings::load_ui_snapshot(cx);
        let attachments_dir = settings::ui_attachments_dir(cx);
        if snapshot.is_present() {
            let known: HashSet<String> = self
                .state
                .read(cx)
                .chats
                .iter()
                .filter(|chat| !chat.archived)
                .map(|chat| chat.id.clone())
                .collect();
            // "Was on the new-chat canvas" and "was on a chat that no longer
            // exists" both leave `selected_chat` empty after pruning.
            let was_selected = snapshot.selected_chat.clone();
            snapshot.retain_known(|id| known.contains(id));

            let (nothing_selected, boot_pending) = {
                let state = self.state.read(cx);
                (state.selected_chat.is_none(), !state.auto_selected)
            };
            if nothing_selected && boot_pending {
                match (was_selected, snapshot.selected_chat.clone()) {
                    (_, Some(chat_id)) => {
                        self.focus_composer(cx);
                        self.state
                            .update(cx, |state, cx| state.select_chat(Some(chat_id), cx));
                    }
                    // It was the canvas: keep it instead of auto-selecting.
                    (None, None) => self.state.update(cx, |state, _| state.auto_selected = true),
                    (Some(_), None) => {}
                }
            }

            self.panels.restore_ui_snapshot(&snapshot.right_pane);

            for (key, draft) in std::mem::take(&mut snapshot.drafts) {
                let staged = attachments_dir
                    .as_deref()
                    .map(|dir| crate::ui_state::restorable_attachments(dir, &draft.attachments))
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|path| crate::attachments::stage_file(path).ok())
                    .collect::<Vec<_>>();
                self.composer.update(cx, |composer, cx| {
                    composer.restore_draft(key, draft.text, staged, cx)
                });
            }
            cx.notify();
        }
        // From here on the live state is authoritative and gets written.
        self.ui_state_restored = true;
        self.capture_ui_state(true, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_state::{RIGHT_PANE_SURFACES, SnapshotStore};

    fn chat(id: &str) -> zeron_proto::Chat {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": id, "deviceId": "local", "archived": false,
            "createdAt": Utc::now(),
        }))
        .unwrap()
    }

    fn shell(
        cx: &mut gpui::TestAppContext,
        dir: &std::path::Path,
    ) -> (gpui::WindowHandle<Shell>, Entity<AppState>) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir, cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let state = cx.update(|cx| cx.new(|_| AppState::new()));
        let window = cx.add_window({
            let state = state.clone();
            let dir = dir.to_path_buf();
            move |_, cx| {
                Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir,
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                )
            }
        });
        (window, state)
    }

    fn sync_chats(state: &Entity<AppState>, ids: &[&str], cx: &mut gpui::TestAppContext) {
        state.update(cx, |state, cx| {
            state.chats = ids.iter().map(|id| chat(id)).collect();
            state.chats_synced = true;
            cx.notify();
        });
        cx.run_until_parked();
    }

    fn type_into_composer(
        shell: &gpui::WindowHandle<Shell>,
        text: &str,
        cx: &mut gpui::TestAppContext,
    ) {
        shell
            .update(cx, |shell, _, cx| {
                shell
                    .composer
                    .update(cx, |c, cx| c.input.update(cx, |i, cx| i.set_text(text, cx)));
            })
            .unwrap();
        cx.run_until_parked();
    }

    fn drafts(
        shell: &gpui::WindowHandle<Shell>,
        cx: &mut gpui::TestAppContext,
    ) -> std::collections::BTreeMap<String, (String, usize)> {
        shell
            .update(cx, |shell, _, cx| {
                shell
                    .composer
                    .read(cx)
                    .ui_state_drafts(cx)
                    .into_iter()
                    .map(|(key, text, staged)| (key, (text, staged.len())))
                    .collect()
            })
            .unwrap()
    }

    #[gpui::test]
    fn restores_selection_drafts_and_pane_without_clobbering_typed_text(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let image = crate::ui_state::materialize_attachment(
            &store.attachments_dir(),
            "old-id",
            "shot.png",
            b"png",
        )
        .unwrap();
        let missing = store.attachments_dir().join("gone-id").join("gone.png");
        let mut snapshot = UiSnapshot::new();
        snapshot.selected_chat = Some("b".into());
        for (key, text, attachments) in [
            ("a", "saved a", vec![]),
            ("b", "saved b", vec![image, missing]),
            ("", "saved canvas", vec![]),
            ("gone", "for a deleted chat", vec![]),
        ] {
            snapshot.drafts.insert(
                key.into(),
                DraftSnapshot {
                    text: text.into(),
                    attachments,
                },
            );
        }
        snapshot
            .right_pane
            .insert("b".into(), RIGHT_PANE_SURFACES.into());
        store.save_now(&snapshot).unwrap();

        let (shell, state) = shell(cx, dir.path());
        // The user starts typing on the canvas before the chat list arrives.
        type_into_composer(&shell, "typed already", cx);
        sync_chats(&state, &["a", "b"], cx);

        assert_eq!(
            state
                .read_with(cx, |s, _| s.selected_chat.clone())
                .as_deref(),
            Some("b")
        );
        let drafts = drafts(&shell, cx);
        assert_eq!(
            drafts["b"],
            ("saved b".to_string(), 1),
            "missing file skipped"
        );
        assert_eq!(drafts["a"].0, "saved a");
        // The canvas keeps what was typed; the restored canvas draft lost.
        assert_eq!(drafts[""].0, "typed already");
        assert!(!drafts.contains_key("gone"));
        shell
            .update(cx, |shell, _, _| {
                assert!(shell.panels.get("b").changes_open);
                assert!(!shell.panels.get("a").changes_open);
            })
            .unwrap();

        // Once restored, the live state is what gets written.
        cx.update(settings::flush);
        let saved = store.load();
        assert_eq!(saved.selected_chat.as_deref(), Some("b"));
        assert_eq!(saved.drafts[""].text, "typed already");
        assert_eq!(saved.drafts["b"].text, "saved b");
        assert_eq!(saved.drafts["b"].attachments.len(), 1);
        assert!(saved.drafts["b"].attachments[0].is_file());
        assert_eq!(saved.right_pane["b"], RIGHT_PANE_SURFACES);
        assert!(!saved.drafts.contains_key("gone"));
    }

    #[gpui::test]
    fn nothing_is_written_before_the_previous_snapshot_is_applied(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let mut snapshot = UiSnapshot::new();
        snapshot.drafts.insert(
            "a".into(),
            DraftSnapshot {
                text: "precious".into(),
                attachments: vec![],
            },
        );
        store.save_now(&snapshot).unwrap();

        let (shell, state) = shell(cx, dir.path());
        // Chats not synced yet: edits must not overwrite the stored drafts.
        type_into_composer(&shell, "early text", cx);
        cx.update(settings::flush);
        assert_eq!(store.load(), snapshot);

        sync_chats(&state, &["a"], cx);
        assert_eq!(drafts(&shell, cx)["a"].0, "precious");
    }

    #[gpui::test]
    fn edits_reach_disk_on_the_shared_debounce_and_on_flush(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let (shell, state) = shell(cx, dir.path());
        sync_chats(&state, &["a"], cx);
        state.update(cx, |state, cx| state.select_chat(Some("a".into()), cx));
        cx.run_until_parked();
        type_into_composer(&shell, "typing in a", cx);

        // Inside the debounce window nothing new is on disk yet...
        assert_ne!(
            store.load().drafts.get("a").map(|d| d.text.as_str()),
            Some("typing in a")
        );
        // ...and the timer (not just quit) persists it.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(
                settings::SAVE_DEBOUNCE_MS + 50,
            ));
        cx.run_until_parked();
        assert_eq!(store.load().drafts["a"].text, "typing in a");

        type_into_composer(&shell, "typing in a, more", cx);
        cx.update(settings::flush);
        assert_eq!(store.load().drafts["a"].text, "typing in a, more");
    }

    #[test]
    fn right_pane_snapshot_lists_only_open_known_chats() {
        let mut panels = SessionPanels::default();
        panels.update("a", |p| p.changes_open = true);
        panels.update("b", |p| p.terminal_open = true);
        panels.update("gone", |p| p.changes_open = true);
        panels.update("", |p| p.changes_open = true);
        let snapshot = panels.ui_snapshot(|id| id != "gone");
        assert_eq!(
            snapshot.keys().map(String::as_str).collect::<Vec<_>>(),
            ["a"]
        );

        let mut fresh = SessionPanels::default();
        fresh.restore_ui_snapshot(&snapshot);
        assert!(fresh.get("a").changes_open);
        assert!(!fresh.get("b").changes_open);
    }
}
