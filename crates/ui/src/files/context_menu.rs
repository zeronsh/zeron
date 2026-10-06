//! Tree menus share the editor's popup surface, positioning and dismissal.
use super::*;
use crate::{icons, popover, theme::Theme};
use gpui::{ClipboardItem, KeyDownEvent, MouseButton};
use mutations::WorkspaceInteractionOrigin;

pub(super) struct TreeContextMenu {
    pub path: String,
    pub origin: WorkspaceInteractionOrigin,
    pub position: Point<Pixels>,
    pub active: usize,
}

fn absolute_workspace_path(root: &str, relative: &str) -> String {
    // Interpret the owning host's path, not the desktop's OS. PathBuf::join
    // would mix separators when viewing a Windows workspace from Linux/macOS.
    let bytes = root.as_bytes();
    let windows = root.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    if windows {
        format!(
            "{}\\{}",
            root.trim_end_matches(['/', '\\']).replace('/', "\\"),
            relative.replace('/', "\\")
        )
    } else {
        format!("{}/{}", root.trim_end_matches('/'), relative)
    }
}

impl FilesSurface {
    pub(super) fn open_tree_context_menu(
        &mut self,
        path: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(origin) = self.interaction_origin(cx) else {
            return;
        };
        if self.tree.node(&path).is_none() {
            return;
        }
        self.tree.select(path.clone());
        self.tree_focus.focus(window, cx);
        self.close_editor_context_menu(cx);
        self.tree_context_menu.open(TreeContextMenu {
            path,
            origin,
            position,
            active: 0,
        });
        cx.notify();
    }

    pub(super) fn close_tree_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.tree_context_menu.begin_close() {
            popover::reap_popup(cx, |files: &mut Self| &mut files.tree_context_menu);
            cx.notify();
        }
    }

    pub(super) fn tree_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.tree_context_menu.is_open() {
            return false;
        }
        match popover::classify_key(
            &event.keystroke.key,
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        ) {
            popover::MenuKey::Escape => self.close_tree_context_menu(cx),
            key @ (popover::MenuKey::Up | popover::MenuKey::Down) => {
                if let Some(menu) = self.tree_context_menu.as_open() {
                    let mut next = menu.active;
                    for _ in 0..4 {
                        next = popover::menu_step(
                            Some(next),
                            4,
                            if matches!(key, popover::MenuKey::Up) {
                                -1
                            } else {
                                1
                            },
                        )
                        .unwrap_or(0);
                        if next < 2 || self.can_mutate_tree_entry(&menu.path, next == 3, cx) {
                            break;
                        }
                    }
                    self.tree_context_menu.open_mut().unwrap().active = next;
                    cx.notify();
                }
            }
            popover::MenuKey::Enter => {
                if let Some(menu) = self.tree_context_menu.as_open() {
                    self.dispatch_tree_menu(menu.active, window, cx);
                }
            }
            _ => {}
        }
        window.prevent_default();
        cx.stop_propagation();
        true
    }

    fn dispatch_tree_menu(&mut self, action: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.tree_context_menu.as_open() else {
            return;
        };
        let path = menu.path.clone();
        let origin = menu.origin.clone();
        if !self.accepts_origin(&origin, cx) {
            self.close_tree_context_menu(cx);
            return;
        }
        let Some(node) = self.tree.node(&path) else {
            self.close_tree_context_menu(cx);
            return;
        };
        let is_directory = node.entry.kind == zeron_proto::WorkspaceEntryKind::Directory;
        self.close_tree_context_menu(cx);
        match action {
            0 => cx.emit(FilesEvent::AddToChat {
                path,
                is_directory,
                origin,
            }),
            1 => {
                cx.write_to_clipboard(ClipboardItem::new_string(absolute_workspace_path(
                    &origin.context.cwd,
                    &path,
                )));
                self.tree_focus.focus(window, cx);
            }
            2 => self.begin_tree_rename(path, window, cx),
            3 => self.begin_tree_delete(path, window, cx),
            _ => {}
        }
    }

    pub(super) fn render_tree_context_menu(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.tree_context_menu.get()?;
        let theme = theme.for_popup();
        let mut card = popover::popover_card(&theme)
            .w(px(190.0))
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_tree_context_menu(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for (index, (label, icon_path)) in [
            ("Add to chat", icons::CHAT_ROUND_LINE),
            ("Copy path", icons::COPY),
            ("Rename…", icons::PEN),
            ("Delete…", icons::TRASH_BIN_MINIMALISTIC),
        ]
        .into_iter()
        .enumerate()
        {
            if index == 2 {
                card = card.child(popover::menu_separator());
            }
            let enabled = index < 2 || self.can_mutate_tree_entry(&menu.path, index == 3, cx);
            card = card.child(
                popover::menu_row(&theme, menu.active == index, format!("tree-menu-{index}"))
                    .id(gpui::SharedString::from(format!("tree-menu-{index}")))
                    .role(gpui::Role::MenuItem)
                    .aria_label(label)
                    .when(index == 3, |row| row.text_color(theme.danger))
                    .when(index == 1, |row| {
                        row.tooltip(|_, cx| {
                            cx.new(|_| preview::FileEditorTooltip {
                                text: "Copy full path".into(),
                            })
                            .into()
                        })
                    })
                    .when(!enabled, |el| el.opacity(0.38).cursor_default())
                    .when(enabled, |el| {
                        el.on_click(cx.listener(move |this, _, window, cx| {
                            this.dispatch_tree_menu(index, window, cx)
                        }))
                    })
                    .child(
                        icons::icon(icon_path)
                            .size(px(16.0))
                            .text_color(if index == 3 {
                                theme.danger
                            } else {
                                theme.text_muted
                            }),
                    )
                    .child(label),
            );
        }
        Some(popover::menu_at(
            "tree-context-menu",
            menu.position,
            card.into_any_element(),
            self.tree_context_menu.closing_since(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[test]
    fn full_paths_follow_the_owning_hosts_path_format() {
        for (root, expected) in [
            ("/workspace", "/workspace/src/a.txt"),
            ("/workspace/", "/workspace/src/a.txt"),
            ("/", "/src/a.txt"),
            (r"C:\work", r"C:\work\src\a.txt"),
            (r"C:\", r"C:\src\a.txt"),
            ("C:/work/", r"C:\work\src\a.txt"),
            (r"\\server\share\work\", r"\\server\share\work\src\a.txt"),
            (r"\\?\C:\work", r"\\?\C:\work\src\a.txt"),
        ] {
            assert_eq!(absolute_workspace_path(root, "src/a.txt"), expected);
        }
    }

    #[gpui::test]
    fn copy_uses_the_remote_workspace_root(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        for (root, expected) in [
            ("/remote/project", "/remote/project/a.txt"),
            (r"D:\remote\project", r"D:\remote\project\a.txt"),
        ] {
            let window = cx.add_window(|_, cx| {
                let state = cx.new(|_| {
                    let mut state = super::super::test_support::state();
                    state.chats[0].device_id = "remote-host".into();
                    state.chats[0].cwd = Some(root.into());
                    state
                });
                super::super::test_support::explorer(state, cx)
            });
            window
                .update(cx, |files, window, cx| {
                    assert_eq!(
                        files
                            .request_context
                            .as_ref()
                            .unwrap()
                            .target_device_id
                            .as_deref(),
                        Some("remote-host")
                    );
                    files.open_tree_context_menu(
                        "a.txt".into(),
                        gpui::point(px(50.), px(50.)),
                        window,
                        cx,
                    );
                    files.dispatch_tree_menu(1, window, cx);
                    assert_eq!(
                        cx.read_from_clipboard().unwrap().text().as_deref(),
                        Some(expected)
                    );
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn menu_selects_without_opening_and_copies_full_path(cx: &mut TestAppContext) {
        let (files, cx) = super::super::test_support::setup(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        files.update_in(cx, |files, window, cx| {
            files.open_tree_context_menu("a.txt".into(), gpui::point(px(50.), px(50.)), window, cx);
            assert_eq!(files.tree.selected(), Some("a.txt"));
            assert!(events.borrow().is_empty());
            files.dispatch_tree_menu(1, window, cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("/workspace/a.txt")
            );
            files.open_tree_context_menu(
                "folder".into(),
                gpui::point(px(50.), px(50.)),
                window,
                cx,
            );
            assert!(!files.tree.is_expanded("folder"));
            files.dispatch_tree_menu(0, window, cx);
        });
        cx.run_until_parked();
        assert!(events.borrow().iter().any(|event|matches!(event,FilesEvent::AddToChat{path,is_directory:true,..} if path=="folder")));
        assert!(
            !events
                .borrow()
                .iter()
                .any(|event| matches!(event, FilesEvent::OpenFile(_)))
        );
    }
}
