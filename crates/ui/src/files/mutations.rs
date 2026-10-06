//! Shared identity and reconciliation for tree mutations and workspace references.
use super::*;
use zeron_proto::{WorkspaceEntry, WorkspaceEntryKind, WorkspaceMutationOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInteractionOrigin {
    pub context: FilesRequestContext,
    pub surface_id: gpui::EntityId,
    pub generation: u64,
    pub checkout_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationIntent {
    pub operation_id: String,
    pub origin: WorkspaceInteractionOrigin,
    pub entry: WorkspaceEntry,
    /// None deletes; Some moves without replacement (including rename).
    pub destination: Option<String>,
}

pub(crate) fn contains_path(ancestor: &str, path: &str) -> bool {
    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

impl MutationIntent {
    pub(crate) fn affects(&self, path: &str) -> bool {
        contains_path(&self.entry.path, path)
            || self
                .destination
                .as_deref()
                .is_some_and(|to| contains_path(to, path))
    }
}

impl FilesSurface {
    pub(crate) fn interaction_origin(&self, cx: &gpui::App) -> Option<WorkspaceInteractionOrigin> {
        self.is_current_target(cx)
            .then(|| WorkspaceInteractionOrigin {
                context: self.request_context.clone().unwrap(),
                surface_id: self.surface_id,
                generation: self.interaction_generation,
                checkout_id: self.effective_checkout_id.clone().or_else(|| {
                    self.request_context
                        .as_ref()
                        .and_then(|c| c.checkout_id.clone())
                }),
            })
    }

    pub(crate) fn accepts_origin(
        &self,
        origin: &WorkspaceInteractionOrigin,
        cx: &gpui::App,
    ) -> bool {
        self.is_current_target(cx)
            && self.surface_id == origin.surface_id
            && self.interaction_generation == origin.generation
            && self.request_context.as_ref() == Some(&origin.context)
    }

    pub(crate) fn shares_workspace(&self, origin: &WorkspaceInteractionOrigin) -> bool {
        let Some(context) = &self.request_context else {
            return false;
        };
        if context.target_device_id != origin.context.target_device_id {
            return false;
        }
        let checkout = self
            .effective_checkout_id
            .as_ref()
            .or(context.checkout_id.as_ref());
        match (checkout, origin.checkout_id.as_ref()) {
            (Some(a), Some(b)) => a == b,
            _ => context.cwd == origin.context.cwd,
        }
    }

    pub(crate) fn mutation_busy(&self) -> bool {
        self.pending_mutation.is_some()
    }

    pub(crate) fn prepare_mutation(&mut self, intent: MutationIntent, cx: &mut Context<Self>) {
        self.pending_mutation = Some(intent);
        for (path, document) in &mut self.preview.documents {
            if self.pending_mutation.as_ref().unwrap().affects(path) {
                document.autosave_task = None;
            }
        }
        self.mutation_error = None;
        cx.notify();
    }

    pub(crate) fn mutation_has_save(&self) -> bool {
        self.pending_mutation.as_ref().is_some_and(|intent| {
            self.preview
                .documents
                .iter()
                .any(|(path, doc)| intent.affects(path) && doc.pending_save.is_some())
        })
    }

    pub(crate) fn hold_mutation(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        self.mutation_hold = path;
        let paths = self.preview.documents.keys().cloned().collect::<Vec<_>>();
        for path in paths {
            if self.mutation_blocks_path(&path) {
                self.preview.documents.get_mut(&path).unwrap().autosave_task = None;
            } else {
                self.schedule_autosave(path, cx);
            }
        }
    }
    pub(super) fn mutation_blocks_path(&self, path: &str) -> bool {
        if self
            .mutation_hold
            .as_deref()
            .is_some_and(|hold| contains_path(hold, path))
        {
            return true;
        }
        self.pending_mutation
            .as_ref()
            .is_some_and(|intent| intent.affects(path))
    }

    pub(crate) fn report_mutation_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.mutation_error = Some(message.into());
        if let Some(rename) = self.tree_rename.as_mut() {
            rename.submitted = false;
        }
        cx.notify();
    }

    pub(super) fn request_mutation(
        &mut self,
        path: &str,
        destination: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.pending_mutation.is_some() {
            return;
        }
        let Some(origin) = self.interaction_origin(cx) else {
            return;
        };
        let Some(entry) = self.tree.node(path).map(|n| n.entry.clone()) else {
            return;
        };
        let available = self.mutation_capabilities.is_some_and(|c| {
            if destination.is_some() {
                c.move_entry
            } else {
                c.delete_entry
            }
        });
        if !available
            || origin.checkout_id.is_none()
            || entry.mutation_revision.is_none()
            || entry.kind == WorkspaceEntryKind::Symlink
        {
            return;
        }
        cx.emit(FilesEvent::Mutate(MutationIntent {
            operation_id: uuid::Uuid::new_v4().to_string(),
            origin,
            entry,
            destination,
        }));
    }

    pub(crate) fn finish_mutation(
        &mut self,
        intent: &MutationIntent,
        result: &Result<WorkspaceMutationOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        if self.pending_mutation.as_ref().map(|p| &p.operation_id) != Some(&intent.operation_id) {
            return;
        }
        // A semantic watch event is authoritative even if the RPC reply was lost.
        let observed = self
            .deferred_file_changes
            .iter()
            .flat_map(|frame| &frame.changes)
            .find(|change| change.operation_id.as_deref() == Some(&intent.operation_id))
            .cloned();
        let observed_applied = observed.is_some();
        if let Some(change) = observed {
            self.apply_semantic_mutation(&change, None, true, cx);
        }
        match result {
            Ok(WorkspaceMutationOutcome::Applied {
                checkout_id,
                change,
                entry,
                ..
            }) if intent.origin.checkout_id.as_ref() == Some(checkout_id) => {
                self.apply_semantic_mutation(change, entry.clone(), true, cx);
            }
            Ok(WorkspaceMutationOutcome::Rejected { message, .. }) | Err(message) => {
                if !observed_applied {
                    self.mutation_error = Some(message.clone().into());
                }
            }
            _ => self.mutation_error = Some("Workspace changed before operation completed".into()),
        }
        if observed_applied || matches!(result, Ok(WorkspaceMutationOutcome::Applied { .. })) {
            self.tree_rename = None;
        } else if let Some(rename) = &mut self.tree_rename {
            rename.submitted = false;
        }
        self.pending_mutation = None;
        let frames = std::mem::take(&mut self.deferred_file_changes);
        // Queued events may contain duplicate native remove/create pairs. After
        // applying the semantic transition, reconcile rather than destroying state.
        drop(frames);
        self.loads.clear();
        self.tree.invalidate_loads();
        self.tree_list_generation = self.tree.generation();
        self.refresh(cx);
        self.reconcile_open_documents(cx);
        let paths = self.preview.documents.keys().cloned().collect::<Vec<_>>();
        for path in paths {
            self.schedule_autosave(path, cx);
        }
        if !self.search_state.query.is_empty() {
            self.search_state.query.clear();
            self.on_search_edited(cx);
        }
        cx.notify();
    }

    pub(super) fn apply_semantic_mutation(
        &mut self,
        change: &zeron_proto::WorkspaceFileChange,
        entry: Option<WorkspaceEntry>,
        local: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = &change.operation_id {
            if self.applied_mutations.contains(id) {
                return;
            }
            self.applied_mutations.push_back(id.clone());
            if self.applied_mutations.len() > 128 {
                self.applied_mutations.pop_front();
            }
        }
        self.loads.clear();
        self.tree.invalidate_loads();
        match change.kind {
            zeron_proto::WorkspaceFileChangeKind::Renamed => {
                if let Some(old) = &change.old_path {
                    self.tree.relocate_subtree(old, &change.path, entry);
                    let phases = if local {
                        self.preview
                            .documents
                            .iter()
                            .filter(|(p, _)| contains_path(old, p))
                            .map(|(p, d)| {
                                (
                                    format!("{}{}", change.path, &p[old.len()..]),
                                    d.phase.clone(),
                                )
                            })
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    for (old_path, new_path) in self.rename_documents(old, change.path.clone(), cx)
                    {
                        cx.emit(FilesEvent::FileRenamed { old_path, new_path });
                    }
                    for (path, phase) in phases {
                        if let Some(doc) = self.preview.documents.get_mut(&path) {
                            doc.phase = phase;
                        }
                    }
                    if !self.presentation.is_editor() {
                        self.tree.select(change.path.clone());
                    }
                }
            }
            zeron_proto::WorkspaceFileChangeKind::Removed => {
                let rows = self.tree.visible_rows().to_vec();
                let index = rows.iter().position(|r| r.path == change.path);
                let next = index
                    .and_then(|i| {
                        rows[i + 1..]
                            .iter()
                            .chain(rows[..i].iter().rev())
                            .find(|r| {
                                matches!(r.kind, model::VisibleRowKind::Entry)
                                    && !contains_path(&change.path, &r.path)
                            })
                    })
                    .map(|r| r.path.clone());
                self.tree.remove(&change.path);
                self.mark_document_deleted(&change.path, cx);
                if let Some(path) = next {
                    self.tree.select(path);
                }
            }
            _ => {}
        }
        self.invalidate_markdown_images(None, cx);
        self.tree_list_generation = self.tree.generation();
        self.sync_tree_list();
        self.reveal_tree_selection();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::super::document::{DocumentKey, DocumentPhase, FileDocument};
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn watch_before_lost_reply_still_remaps_the_dirty_buffer(cx: &mut TestAppContext) {
        let (files, cx) = super::super::test_support::setup(cx);
        files.update_in(cx, |files, _, cx| {
            let origin = files.interaction_origin(cx).unwrap();
            let intent = MutationIntent {
                operation_id: "observed".into(),
                origin,
                entry: files.tree.node("a.txt").unwrap().entry.clone(),
                destination: Some("b.txt".into()),
            };
            let mut document = FileDocument::loading(DocumentKey {
                chat_id: "chat".into(),
                checkout_id: Some("checkout".into()),
                path: "a.txt".into(),
            });
            document.phase = DocumentPhase::Ready;
            document.revision = 4;
            document.saved_revision = 1;
            files.preview.documents.insert("a.txt".into(), document);
            files.prepare_mutation(intent.clone(), cx);
            let change = zeron_proto::WorkspaceFileChange {
                operation_id: Some("observed".into()),
                kind: zeron_proto::WorkspaceFileChangeKind::Renamed,
                path: "b.txt".into(),
                old_path: Some("a.txt".into()),
            };
            files.apply_workspace_changes(
                zeron_proto::WorkspaceFileChanges {
                    sequence: 2,
                    resync_required: false,
                    changes: vec![change],
                },
                cx,
            );
            assert!(files.preview.documents.contains_key("a.txt"));
            files.apply_workspace_changes(
                zeron_proto::WorkspaceFileChanges {
                    sequence: 3,
                    resync_required: false,
                    changes: vec![],
                },
                cx,
            );
            files.finish_mutation(&intent, &Err("reply lost".into()), cx);
            assert_eq!(files.watch_sequence, Some(3));
            assert!(files.preview.documents["b.txt"].is_dirty());
            assert_eq!(files.preview.documents["b.txt"].phase, DocumentPhase::Ready);
            assert!(files.mutation_error.is_none());
            assert!(!files.mutation_busy());
        });
    }
    #[test]
    fn path_membership_uses_component_boundaries() {
        assert!(contains_path("a", "a/child"));
        assert!(contains_path("a", "a"));
        assert!(!contains_path("a", "ab/child"));
    }

    #[gpui::test]
    fn confirmed_move_preserves_dirty_editor_and_duplicate_events_are_idempotent(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::dark());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            FilesSurface::new_explorer(state, "chat".into(), false, cx)
        });
        window
            .update(cx, |files, window, cx| {
                let theme = crate::theme::Theme::of(cx).clone();
                let editor = super::super::editor::new_file_editor(
                    "unsaved text",
                    "folder/a.txt",
                    false,
                    &theme,
                    window,
                    cx,
                );
                let mut doc = FileDocument::loading(DocumentKey {
                    chat_id: "chat".into(),
                    checkout_id: Some("checkout".into()),
                    path: "folder/a.txt".into(),
                });
                doc.editor = Some(editor.clone());
                doc.phase = DocumentPhase::Ready;
                doc.revision = 3;
                doc.saved_revision = 1;
                files.preview.documents.insert("folder/a.txt".into(), doc);
                let change = zeron_proto::WorkspaceFileChange {
                    operation_id: Some("op".into()),
                    kind: zeron_proto::WorkspaceFileChangeKind::Renamed,
                    path: "other".into(),
                    old_path: Some("folder".into()),
                };
                files.apply_semantic_mutation(&change, None, true, cx);
                files.apply_semantic_mutation(&change, None, true, cx);
                let doc = &files.preview.documents["other/a.txt"];
                assert_eq!(doc.editor.as_ref(), Some(&editor));
                assert_eq!(editor.read(cx).value().as_ref(), "unsaved text");
                assert!(doc.is_dirty());
                assert_eq!(doc.phase, DocumentPhase::Ready);
                assert!(!files.preview.documents.contains_key("folder/a.txt"));
                let removed = zeron_proto::WorkspaceFileChange {
                    operation_id: Some("delete".into()),
                    kind: zeron_proto::WorkspaceFileChangeKind::Removed,
                    path: "other".into(),
                    old_path: None,
                };
                files.apply_semantic_mutation(&removed, None, true, cx);
                assert_eq!(
                    files.preview.documents["other/a.txt"].phase,
                    DocumentPhase::DeletedOnDisk
                );
                assert_eq!(editor.read(cx).value().as_ref(), "unsaved text");
            })
            .unwrap();
    }
}
