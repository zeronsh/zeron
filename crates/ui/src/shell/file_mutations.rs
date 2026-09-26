//! Coordinate structural changes across every editor of the affected workspace.
use super::*;
use crate::files::{client::WorkspaceFilesClient, mutations::MutationIntent};
use zeron_proto::{
    DeleteWorkspaceEntryRequest, MoveWorkspaceEntryRequest, WorkspaceMutationOutcome,
};

impl Shell {
    pub(super) fn start_file_mutation(
        &mut self,
        source: Entity<FilesSurface>,
        mut intent: MutationIntent,
        cx: &mut Context<Self>,
    ) {
        if !source.read(cx).accepts_origin(&intent.origin, cx) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            source.update(cx, |files, cx| {
                files.report_mutation_error("Workspace service is unavailable".into(), cx)
            });
            return;
        };
        let surfaces = self
            .files
            .values()
            .chain(self.file_surfaces.values())
            .filter(|surface| surface.read(cx).shares_workspace(&intent.origin))
            .cloned()
            .collect::<Vec<_>>();
        if surfaces
            .iter()
            .any(|surface| surface.read(cx).mutation_busy())
        {
            source.update(cx, |files, cx| {
                files.report_mutation_error("Another file operation is still running".into(), cx)
            });
            return;
        }
        for surface in &surfaces {
            surface.update(cx, |files, cx| files.prepare_mutation(intent.clone(), cx));
        }
        let waited_for_save = surfaces
            .iter()
            .any(|surface| surface.read(cx).mutation_has_save());
        let client = WorkspaceFilesClient::new(engine, intent.origin.context.clone());
        cx.spawn(async move |shell, cx| {
            let result: Result<WorkspaceMutationOutcome, String> = async {
                let mut ready = false;
                for _ in 0..400 {
                    if !surfaces
                        .iter()
                        .any(|surface| surface.read_with(cx, |files, _| files.mutation_has_save()))
                    {
                        ready = true;
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(25))
                        .await;
                }
                if !ready {
                    return Err("Wait for the pending save to finish and try again".into());
                }
                if !source.read_with(cx, |files, cx| files.accepts_origin(&intent.origin, cx)) {
                    return Err("Workspace changed before the operation started".into());
                }
                // Only refresh the source revision when our own save was awaited.
                // Otherwise an external edit must cause SourceChanged, not consent.
                if waited_for_save {
                    let page = client
                        .list_directory_snapshot(
                            zeron_proto::ListWorkspaceDirectoryRequest {
                                target: intent.origin.context.target.clone(),
                                directory: crate::files::model::parent_path(&intent.entry.path)
                                    .unwrap_or_default(),
                                include_ignored: true,
                                cursor: None,
                            },
                            &[intent.entry.path.clone()],
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    if page.checkout_id != intent.origin.checkout_id {
                        return Err("Workspace changed while saving".into());
                    }
                    let entry = page
                        .entries
                        .into_iter()
                        .find(|entry| entry.path == intent.entry.path)
                        .ok_or("Source no longer exists")?;
                    intent.entry.mutation_revision = entry.mutation_revision;
                }
                let owner_active = shell
                    .update(cx, |shell, cx| {
                        matches!(shell.route, Route::Chat)
                            && intent.origin.context.target.chat_id.as_deref()
                                == Some(shell.panel_key(cx).as_str())
                    })
                    .unwrap_or(false);
                if !owner_active
                    || !source.read_with(cx, |files, cx| files.accepts_origin(&intent.origin, cx))
                {
                    return Err("File operation cancelled after switching sessions".into());
                }
                let checkout = intent
                    .origin
                    .checkout_id
                    .clone()
                    .ok_or("Workspace identity unavailable")?;
                let revision = intent
                    .entry
                    .mutation_revision
                    .clone()
                    .ok_or("Refresh the tree before trying again")?;
                if let Some(destination) = &intent.destination {
                    client
                        .move_entry(MoveWorkspaceEntryRequest {
                            target: intent.origin.context.target.clone(),
                            operation_id: intent.operation_id.clone(),
                            expected_checkout_id: checkout,
                            source_path: intent.entry.path.clone(),
                            destination_path: destination.clone(),
                            expected_source_revision: revision,
                            expected_kind: intent.entry.kind,
                        })
                        .await
                        .map_err(|e| e.to_string())
                } else {
                    client
                        .delete_entry(DeleteWorkspaceEntryRequest {
                            target: intent.origin.context.target.clone(),
                            operation_id: intent.operation_id.clone(),
                            expected_checkout_id: checkout,
                            path: intent.entry.path.clone(),
                            expected_source_revision: revision,
                            expected_kind: intent.entry.kind,
                            recursive: intent.entry.kind
                                == zeron_proto::WorkspaceEntryKind::Directory,
                        })
                        .await
                        .map_err(|e| e.to_string())
                }
            }
            .await;
            for surface in &surfaces {
                let _ = surface.update(cx, |files, cx| files.finish_mutation(&intent, &result, cx));
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext, VisualTestContext};
    use std::{cell::RefCell, rc::Rc};

    struct DragHost {
        shell: Entity<Shell>,
        files: Entity<FilesSurface>,
        _directory: tempfile::TempDir,
    }
    impl Render for DragHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let main = self
                .shell
                .update(cx, |shell, cx| shell.render_main(window, 600., 600., cx));
            div()
                .flex()
                .w(px(900.))
                .h(px(600.))
                .child(
                    div()
                        .w(px(280.))
                        .h_full()
                        .flex_none()
                        .child(self.files.clone()),
                )
                .child(main)
        }
    }
    fn setup(
        cx: &mut TestAppContext,
    ) -> (Entity<Shell>, Entity<FilesSurface>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::app_menus::init(cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let directory = tempfile::tempdir().unwrap();
            let state = cx.new(|_| {
                let mut state = crate::files::test_support::state();
                state.selected_chat = Some("chat".into());
                state.no_project = true;
                state
            });
            let files = cx.new(|cx| crate::files::test_support::explorer(state.clone(), cx));
            let shell = cx.new(|cx| {
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: directory.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.active_chat = "chat".into();
                shell.route = Route::Chat;
                shell.files.insert("chat".into(), files.clone());
                shell
            });
            DragHost {
                shell,
                files,
                _directory: directory,
            }
        });
        let (shell, files) = host.read_with(cx, |host, _| (host.shell.clone(), host.files.clone()));
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear();
        });
        (shell, files, cx)
    }

    #[gpui::test]
    fn tree_and_production_chat_dropzones_are_exclusive(cx: &mut TestAppContext) {
        let (shell, files, cx) = setup(cx);
        let events = Rc::new(RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        let input = shell.read_with(cx, |shell, cx| shell.composer.read(cx).input.clone());
        input.update(cx, |input, cx| input.set_text("keep this ", cx));
        cx.update(|window, cx| window.draw(cx).clear());
        let start = cx.debug_bounds("tree-entry:a.txt").unwrap().center();
        let folder = cx.debug_bounds("tree-entry:folder").unwrap().center();
        let chat = cx.debug_bounds("chat-dropzone").unwrap().center();
        // Visit the tree target, then finish over the actual conversation.
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            start + gpui::point(px(9.), px(0.)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(folder, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_move(chat, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(chat, MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();
        let text = input.read_with(cx, |input, _| input.text().to_string());
        assert!(text.contains("keep this"));
        assert!(text.contains("a.txt"));
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, FilesEvent::Mutate(_)))
        );
        // Visit the chat, then finish over the tree target.
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            start + gpui::point(px(9.), px(0.)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(chat, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_move(folder, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(folder, MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            input.read_with(cx, |input, _| input.text().to_string()),
            text
        );
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| matches!(event, FilesEvent::Mutate(_)))
                .count(),
            1
        );
        cx.update(|_, cx| assert!(!cx.has_active_drag()));
    }

    #[gpui::test]
    fn stale_workspace_reference_cannot_attach_to_a_new_chat(cx: &mut TestAppContext) {
        let (shell, files, cx) = setup(cx);
        let origin = files
            .read_with(cx, |files, cx| files.interaction_origin(cx))
            .unwrap();
        let payload = WorkspacePathDrag::new("a.txt".into(), false).with_origin(
            Some(origin),
            crate::files::WorkspacePathSource::Tree,
            Some("rev".into()),
        );
        shell.update_in(cx, |shell, window, cx| {
            shell.active_chat = "other".into();
            let input = shell.composer.read(cx).input.clone();
            let before = input.read(cx).text().to_string();
            shell.attach_workspace_drag(&payload, window, cx);
            assert_eq!(input.read(cx).text(), before);
        });
    }
}
