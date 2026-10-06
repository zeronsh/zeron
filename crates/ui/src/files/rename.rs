//! Inline single-line rename and permanent-delete confirmation.
use super::*;
use crate::{popover, theme::Theme};
use gpui::{AnyElement, Focusable, KeyDownEvent, MouseButton};
use gpui_base::input::{Input, InputEvent, InputState};

pub(super) struct TreeRename {
    pub path: String,
    pub origin: mutations::WorkspaceInteractionOrigin,
    pub input: Entity<InputState>,
    pub submitted: bool,
    revision: Option<String>,
    _events: Subscription,
}
pub(super) struct TreeDelete {
    pub path: String,
    pub origin: mutations::WorkspaceInteractionOrigin,
    pub focus: FocusHandle,
    pub confirm_focused: bool,
    revision: Option<String>,
}

pub(super) fn name_selection(name: &str, directory: bool) -> std::ops::Range<usize> {
    let end = if directory {
        name.len()
    } else {
        name.rfind('.').filter(|i| *i > 0).unwrap_or(name.len())
    };
    0..end
}
pub(super) fn renamed_path(path: &str, name: &str) -> Result<String, &'static str> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.eq_ignore_ascii_case(".git")
        || name.contains(['/', '\\', '\0', ':'])
    {
        return Err("Enter a single file name without path separators");
    }
    let parent = model::parent_path(path).unwrap_or_default();
    Ok(if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}/{name}")
    })
}
impl FilesSurface {
    pub(super) fn can_mutate_tree_entry(&self, path: &str, deleting: bool, cx: &gpui::App) -> bool {
        !self.mutation_busy()
            && self.is_current_target(cx)
            && self.effective_checkout_id.is_some()
            && self.mutation_capabilities.is_some_and(|c| {
                if deleting {
                    c.delete_entry
                } else {
                    c.move_entry
                }
            })
            && self.tree.node(path).is_some_and(|n| {
                n.entry.mutation_revision.is_some()
                    && n.entry.kind != zeron_proto::WorkspaceEntryKind::Symlink
            })
    }
    pub(super) fn begin_tree_rename(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_mutate_tree_entry(&path, false, cx) {
            return;
        }
        let Some(origin) = self.interaction_origin(cx) else {
            return;
        };
        let entry = self.tree.node(&path).unwrap().entry.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(entry.name.clone()));
        input.update(cx, |state, cx| {
            state.set_selected_range(
                name_selection(
                    &entry.name,
                    entry.kind == zeron_proto::WorkspaceEntryKind::Directory,
                ),
                cx,
            )
        });
        let events = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } => this.submit_tree_rename(window, cx),
            InputEvent::Blur
                if this
                    .tree_rename
                    .as_ref()
                    .is_some_and(|rename| !rename.submitted) =>
            {
                this.tree_rename = None;
                cx.notify();
            }
            _ => {}
        });
        let focus = input.focus_handle(cx);
        self.tree.select(path.clone());
        self.reveal_tree_selection();
        self.mutation_error = None;
        self.tree_rename = Some(TreeRename {
            path,
            origin,
            input,
            submitted: false,
            revision: entry.mutation_revision,
            _events: events,
        });
        window.focus(&focus, cx);
        cx.notify();
    }
    fn submit_tree_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = &self.tree_rename else {
            return;
        };
        if rename.submitted {
            return;
        }
        if !self.accepts_origin(&rename.origin, cx) {
            self.tree_rename = None;
            cx.notify();
            return;
        }
        if self
            .tree
            .node(&rename.path)
            .and_then(|node| node.entry.mutation_revision.as_ref())
            != rename.revision.as_ref()
        {
            self.report_mutation_error("Entry changed; cancel and reopen Rename".into(), cx);
            return;
        }
        let path = rename.path.clone();
        let name = rename.input.read(cx).value().to_string();
        let destination = match renamed_path(&path, &name) {
            Ok(path) => path,
            Err(message) => {
                self.report_mutation_error(message.into(), cx);
                return;
            }
        };
        if path == destination {
            self.tree_rename = None;
            self.tree_focus.focus(window, cx);
            cx.notify();
            return;
        }
        self.tree_rename.as_mut().unwrap().submitted = true;
        self.request_mutation(&path, Some(destination), cx);
    }
    pub(super) fn render_tree_rename(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rename = self
            .tree_rename
            .as_ref()
            .filter(|rename| rename.path == path)?;
        let theme = Theme::of(cx).clone();
        // gpui-base's Input is unstyled: explicitly supply the same visible
        // caret and selection colors as the file editor, including theme changes.
        rename.input.update(cx, |input, _| {
            input.set_editor_style(super::editor_adapter::editor_style(&theme));
        });
        Some(
            div()
                .id("tree-rename-input")
                .debug_selector(|| "tree-rename-input".into())
                .flex_1()
                .min_w_0()
                .h(px(23.))
                .px(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.accent)
                .bg(theme.input_glass_bg())
                .font_family(theme.font_sans.clone())
                .text_size(px(11.5))
                .text_color(theme.text)
                .cursor_text()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        if this.tree_rename.as_ref().is_some_and(|r| !r.submitted) {
                            this.tree_rename = None;
                            this.tree_focus.focus(window, cx);
                            cx.notify();
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }))
                .child(Input::new(&rename.input))
                .into_any_element(),
        )
    }
    pub(super) fn begin_tree_delete(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_mutate_tree_entry(&path, true, cx) {
            return;
        }
        let Some(origin) = self.interaction_origin(cx) else {
            return;
        };
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        cx.emit(FilesEvent::HoldMutation {
            origin: origin.clone(),
            path: Some(path.clone()),
        });
        let revision = self
            .tree
            .node(&path)
            .and_then(|node| node.entry.mutation_revision.clone());
        self.tree_delete = Some(TreeDelete {
            path,
            origin,
            focus,
            confirm_focused: false,
            revision,
        });
        cx.notify();
    }
    pub(super) fn dismiss_tree_delete(
        &mut self,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.tree_delete.take() else {
            return;
        };
        cx.emit(FilesEvent::HoldMutation {
            origin: dialog.origin.clone(),
            path: None,
        });
        if confirm && self.accepts_origin(&dialog.origin, cx) {
            if self
                .tree
                .node(&dialog.path)
                .and_then(|node| node.entry.mutation_revision.as_ref())
                != dialog.revision.as_ref()
            {
                self.report_mutation_error(
                    "Entry changed; reopen Delete to confirm its current contents".into(),
                    cx,
                );
            } else {
                self.request_mutation(&dialog.path, None, cx);
            }
        }
        self.tree_focus.focus(window, cx);
        cx.notify();
    }
    pub(super) fn render_tree_delete(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.tree_delete.as_ref()?;
        let theme = theme.for_popup();
        let directory = self
            .tree
            .node(&dialog.path)
            .is_some_and(|n| n.entry.kind == zeron_proto::WorkspaceEntryKind::Directory);
        let name = dialog.path.rsplit('/').next().unwrap_or(&dialog.path);
        let copy = format!(
            "Permanently delete {}? {}Open editor buffers will be kept for recovery.",
            name,
            if directory {
                "All current folder contents will be deleted. "
            } else {
                "This cannot be undone. "
            }
        );
        let card = popover::dialog_card(&theme)
            .w(px(380.))
            .gap(px(12.))
            .track_focus(&dialog.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.dismiss_tree_delete(false, window, cx),
                    "tab" | "left" | "right" => {
                        if let Some(dialog) = this.tree_delete.as_mut() {
                            dialog.confirm_focused = !dialog.confirm_focused;
                            cx.notify();
                        }
                    }
                    "enter" => {
                        let confirm = this.tree_delete.as_ref().is_some_and(|d| d.confirm_focused);
                        this.dismiss_tree_delete(confirm, window, cx);
                    }
                    _ => {}
                }
                window.prevent_default();
                cx.stop_propagation();
            }))
            .child(popover::dialog_title(&theme, "Delete permanently?"))
            .child(popover::dialog_body(&theme, copy))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "tree-delete-cancel")
                            .id("tree-delete-cancel")
                            .role(gpui::Role::Button)
                            .aria_label("Cancel")
                            .when(!dialog.confirm_focused, |el| {
                                el.bg(crate::theme::wash(0.12))
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_tree_delete(false, window, cx)
                            })),
                    )
                    .child(
                        popover::btn_danger(&theme, "Delete")
                            .id("tree-delete-confirm")
                            .role(gpui::Role::Button)
                            .aria_label("Delete")
                            .when(dialog.confirm_focused, |el| {
                                el.border_1().border_color(theme.text)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_tree_delete(true, window, cx)
                            })),
                    ),
            );
        Some(popover::modal(
            "tree-delete-dialog",
            window.viewport_size(),
            card.into_any_element(),
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn rename_input_keeps_focus_and_supports_caret_editing(cx: &mut gpui::TestAppContext) {
        let (files, cx) = super::super::test_support::setup(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        files.update_in(cx, |files, window, cx| {
            files.tree.select("a.txt");
            files.tree_focus.focus(window, cx);
        });
        cx.simulate_keystrokes("f2");
        let input = files.read_with(cx, |files, _| {
            files.tree_rename.as_ref().unwrap().input.clone()
        });
        assert_eq!(input.read_with(cx, |input, _| input.selected_range()), 0..1);
        cx.simulate_keystrokes("right");
        assert_eq!(input.read_with(cx, |input, _| input.selected_range()), 1..1);
        cx.simulate_input("b");
        assert_eq!(
            input.read_with(cx, |input, _| input.value().to_string()),
            "ab.txt"
        );

        let bounds = cx.debug_bounds("tree-rename-input").unwrap();
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        files.update_in(cx, |files, window, cx| {
            assert!(
                files.tree_rename.is_some(),
                "clicking the field must not activate its tree row"
            );
            assert!(input.focus_handle(cx).is_focused(window));
            assert!(!files.tree_focus.is_focused(window));
        });
        for theme in [Theme::dark(), Theme::light()] {
            cx.update(|window, cx| {
                cx.set_global(theme);
                window.refresh();
                window.draw(cx).clear();
            });
            input.read_with(cx, |input, _| {
                let (caret, _) = input.cursor_layout().expect("rename must lay out a caret");
                assert!(caret.size.width > px(0.));
                assert!(caret.size.height > px(0.));
                assert!(caret.left() >= bounds.left());
                assert!(caret.right() <= bounds.right());
                assert!(caret.top() >= bounds.top());
                assert!(caret.bottom() <= bounds.bottom());
            });
        }
        cx.simulate_keystrokes("escape");
        files.update_in(cx, |files, window, _| {
            assert!(files.tree_rename.is_none());
            assert!(files.tree_focus.is_focused(window));
        });
        cx.run_until_parked();
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, FilesEvent::OpenFile(_) | FilesEvent::Mutate(_)))
        );

        cx.simulate_keystrokes("f2");
        cx.simulate_input("résumé");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let events = events.borrow();
        let intents = events
            .iter()
            .filter_map(|event| match event {
                FilesEvent::Mutate(intent) => Some(intent),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].destination.as_deref(), Some("résumé.txt"));
    }

    #[gpui::test]
    fn refreshed_revision_does_not_authorize_an_already_open_confirmation(
        cx: &mut gpui::TestAppContext,
    ) {
        let (files, cx) = super::super::test_support::setup(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        files.update_in(cx, |files, window, cx| {
            files.begin_tree_delete("a.txt".into(), window, cx);
            files.begin_tree_rename("a.txt".into(), window, cx);
            let input = files.tree_rename.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| input.set_value("renamed.txt", window, cx));
            let mut entry =
                super::super::test_support::entry("a.txt", zeron_proto::WorkspaceEntryKind::File);
            entry.mutation_revision = Some("external-edit".into());
            files.tree.apply_page(
                zeron_proto::WorkspaceDirectoryPage {
                    directory: "".into(),
                    checkout_id: Some("checkout".into()),
                    mutation_capabilities: files.mutation_capabilities,
                    entries: vec![entry],
                    next_cursor: None,
                    truncated: false,
                },
                files.tree.generation(),
            );
            files.submit_tree_rename(window, cx);
            assert!(!files.tree_rename.as_ref().unwrap().submitted);
            assert!(
                files
                    .mutation_error
                    .as_ref()
                    .unwrap()
                    .contains("Entry changed")
            );
            files.dismiss_tree_delete(true, window, cx);
            assert!(
                files
                    .mutation_error
                    .as_ref()
                    .unwrap()
                    .contains("Entry changed")
            );
        });
        cx.run_until_parked();
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, FilesEvent::Mutate(_)))
        );
    }

    #[gpui::test]
    fn inline_rename_submits_once_and_cancel_delete_never_mutates(cx: &mut gpui::TestAppContext) {
        let (files, cx) = super::super::test_support::setup(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        files.update_in(cx, |files, window, cx| {
            files.begin_tree_rename("a.txt".into(), window, cx);
            let input = files.tree_rename.as_ref().unwrap().input.clone();
            input.update(cx, |input, cx| input.set_value("new.txt", window, cx));
            files.submit_tree_rename(window, cx);
            files.submit_tree_rename(window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| matches!(event, FilesEvent::Mutate(_)))
                .count(),
            1
        );
        events.borrow_mut().clear();
        files.update_in(cx, |files, window, cx| {
            files.tree_rename = None;
            files.begin_tree_delete("folder".into(), window, cx);
            assert!(!files.tree_delete.as_ref().unwrap().confirm_focused);
            files.dismiss_tree_delete(false, window, cx);
        });
        cx.run_until_parked();
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, FilesEvent::Mutate(_)))
        );
    }
    #[test]
    fn rename_preserves_parent_and_selects_only_basename() {
        assert_eq!(renamed_path("src/a.txt", "é.txt").unwrap(), "src/é.txt");
        for invalid in ["", ".", "..", "../a", "a/b", ".git", "a\\b"] {
            assert!(renamed_path("src/a", invalid).is_err());
        }
        assert_eq!(name_selection("foo.test.ts", false), 0..8);
        assert_eq!(name_selection(".gitignore", false), 0..10);
        assert_eq!(name_selection("foo.ts", true), 0..6);
    }
}
