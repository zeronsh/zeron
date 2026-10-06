use std::{collections::HashSet, time::Duration};

use gpui::Context;
use zeron_proto::{WorkspaceFileChangeKind, WorkspaceFileChanges};

use super::{FilesEvent, FilesSurface, client::WorkspaceFilesClient, model::parent_path};

impl FilesSurface {
    pub(super) fn ensure_watch(&mut self, cx: &mut Context<Self>) {
        if self.watch_task.is_some() {
            return;
        }
        let Some(context) = self.request_context.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let client = WorkspaceFilesClient::new(engine, context);
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            loop {
                match client.watch().await {
                    Ok(mut receiver) => {
                        let _ = this.update(cx, |surface, cx| {
                            surface.watch_error = None;
                            cx.notify();
                        });
                        while let Some(value) = receiver.recv().await {
                            let frame = serde_json::from_value::<WorkspaceFileChanges>(value);
                            if this
                                .update(cx, |surface, cx| match frame {
                                    Ok(frame) => surface.apply_workspace_changes(frame, cx),
                                    Err(error) => {
                                        surface.watch_error = Some(
                                            format!("File updates could not be decoded: {error}")
                                                .into(),
                                        );
                                        cx.notify();
                                    }
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        if this
                            .update(cx, |surface, cx| {
                                surface.watch_error =
                                    Some("File updates interrupted — retrying".into());
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if this
                            .update(cx, |surface, cx| {
                                surface.watch_error = Some(error.to_string().into());
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    pub(super) fn apply_workspace_changes(
        &mut self,
        mut frame: WorkspaceFileChanges,
        cx: &mut Context<Self>,
    ) {
        if self.pending_mutation.is_some() {
            // Defer affected events; unrelated changes still flow through. Keep
            // sequence accounting on the live stream, not the deferred fragments.
            let intent = self.pending_mutation.as_ref().unwrap();
            let (affected, independent): (Vec<_>, Vec<_>) =
                frame.changes.into_iter().partition(|change| {
                    intent.affects(&change.path)
                        || change
                            .old_path
                            .as_deref()
                            .is_some_and(|old| intent.affects(old))
                });
            if !affected.is_empty() || frame.resync_required {
                self.deferred_file_changes.push(WorkspaceFileChanges {
                    sequence: frame.sequence,
                    resync_required: frame.resync_required,
                    changes: affected,
                });
            }
            frame.changes = independent;
            frame.resync_required = false;
        }

        let gap = self.pending_mutation.is_none()
            && sequence_needs_resync(self.watch_sequence, frame.sequence);
        tracing::trace!(
            sequence = frame.sequence,
            previous_sequence = ?self.watch_sequence,
            resync_required = frame.resync_required,
            gap,
            change_count = frame.changes.len(),
            "workspace file changes received by UI"
        );
        self.watch_sequence = Some(frame.sequence);
        if frame.resync_required || gap {
            self.invalidate_markdown_images(None, cx);
            self.refresh(cx);
            self.reconcile_open_documents(cx);
            return;
        }

        let mut parents = HashSet::new();
        for change in frame.changes {
            if change.operation_id.is_some() {
                self.apply_semantic_mutation(&change, None, false, cx);
                if let Some(parent) = parent_path(&change.path) {
                    parents.insert(parent);
                }
                if let Some(old) = change.old_path.as_deref().and_then(parent_path) {
                    parents.insert(old);
                }
                continue;
            }
            self.invalidate_markdown_images(Some(&change.path), cx);
            if let Some(old_path) = &change.old_path {
                self.invalidate_markdown_images(Some(old_path), cx);
            }
            match change.kind {
                WorkspaceFileChangeKind::Created => {
                    self.reconcile_created_documents(&change.path, cx);
                    if let Some(parent) = parent_path(&change.path) {
                        parents.insert(parent);
                    }
                }
                WorkspaceFileChangeKind::Modified => {
                    // Saves (including atomic replacement) change the metadata
                    // revision used by move/delete, even when the name is unchanged.
                    if let Some(parent) = parent_path(&change.path) {
                        parents.insert(parent);
                    }
                    self.reconcile_document(change.path, cx);
                }
                WorkspaceFileChangeKind::Removed => {
                    self.tree.remove(&change.path);
                    self.mark_document_deleted(&change.path, cx);
                    if let Some(parent) = parent_path(&change.path) {
                        parents.insert(parent);
                    }
                }
                WorkspaceFileChangeKind::Renamed => {
                    if let Some(old_path) = change.old_path {
                        self.tree.relocate_subtree(&old_path, &change.path, None);
                        for (old_path, new_path) in
                            self.rename_documents(&old_path, change.path.clone(), cx)
                        {
                            cx.emit(FilesEvent::FileRenamed { old_path, new_path });
                        }
                        if let Some(parent) = parent_path(&old_path) {
                            parents.insert(parent);
                        }
                    }
                    // Atomic replacement tools can report a temporary file being
                    // renamed over an already-open destination document.
                    self.reconcile_created_documents(&change.path, cx);
                    if let Some(parent) = parent_path(&change.path) {
                        parents.insert(parent);
                    }
                }
            }
        }

        if self.presentation.is_editor() {
            cx.notify();
            return;
        }
        for parent in &parents {
            self.tree.invalidate_directory(parent);
        }
        let reload = parents
            .into_iter()
            .filter(|parent| self.tree.is_expanded(parent))
            .collect::<Vec<_>>();
        self.sync_tree_list();
        for parent in reload {
            self.load_directory(parent, None, cx);
        }
        cx.notify();
    }
}

pub(super) fn sequence_needs_resync(previous: Option<u64>, next: u64) -> bool {
    previous.is_some_and(|previous| next != previous.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext;

    #[gpui::test]
    fn completed_save_refreshes_revision_before_the_next_mutation(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, mut requests) = tokio::sync::mpsc::channel(16);
        let (replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine =
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound));
        let state = cx.new(|_| {
            let mut state = super::super::test_support::state();
            state.set_test_engine(engine);
            state
        });
        let files = cx.new(|cx| super::super::test_support::explorer(state, cx));
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        files.update(cx, |files, cx| {
            files.apply_workspace_changes(
                WorkspaceFileChanges {
                    sequence: 1,
                    resync_required: false,
                    changes: vec![zeron_proto::WorkspaceFileChange {
                        operation_id: None,
                        kind: WorkspaceFileChangeKind::Modified,
                        path: "a.txt".into(),
                        old_path: None,
                    }],
                },
                cx,
            );
        });
        cx.run_until_parked();
        let request: serde_json::Value = serde_json::from_str(
            &requests
                .try_recv()
                .expect("Modified must reload the containing directory"),
        )
        .unwrap();
        assert_eq!(
            request["method"],
            zeron_rpc::methods::LIST_WORKSPACE_DIRECTORY
        );
        assert_eq!(request["params"]["directory"], "");
        let mut entry =
            super::super::test_support::entry("a.txt", zeron_proto::WorkspaceEntryKind::File);
        entry.mutation_revision = Some("after-save".into());
        runtime.block_on(async {
            replies
                .send(
                    serde_json::json!({
                        "id": request["id"], "ok": {
                            "directory": "", "checkoutId": "checkout",
                            "mutationCapabilities": {"moveEntry": true, "deleteEntry": true},
                            "entries": [entry], "nextCursor": null, "truncated": false
                        }
                    })
                    .to_string(),
                )
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while replies.capacity() < replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        cx.run_until_parked();
        files.update(cx, |files, cx| {
            assert_eq!(
                files
                    .tree
                    .node("a.txt")
                    .unwrap()
                    .entry
                    .mutation_revision
                    .as_deref(),
                Some("after-save")
            );
            files.request_mutation("a.txt", Some("renamed.txt".into()), cx);
            files.request_mutation("a.txt", Some("folder/a.txt".into()), cx);
            files.request_mutation("a.txt", None, cx);
        });
        cx.run_until_parked();
        let events = events.borrow();
        let intents = events
            .iter()
            .filter_map(|event| match event {
                FilesEvent::Mutate(intent) => Some(intent),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(intents.len(), 3);
        assert!(
            intents
                .iter()
                .all(|intent| intent.entry.mutation_revision.as_deref() == Some("after-save"))
        );
    }

    #[test]
    fn sequence_gaps_require_resync() {
        assert!(!sequence_needs_resync(None, 40));
        assert!(!sequence_needs_resync(Some(40), 41));
        assert!(sequence_needs_resync(Some(40), 42));
        assert!(sequence_needs_resync(Some(40), 40));
    }
}
