//! Right-click menu for file and folder rows, in the tree and in search
//! results. It opens on the row under the pointer and acts on that row's
//! workspace-relative path.

use std::path::PathBuf;

use gpui::{
    AnyElement, ClipboardItem, Context, Pixels, Point, SharedString, Window, prelude::*, px,
};

use super::{FilesEvent, FilesSurface, path_bar::display_path};
use crate::{popover, theme::Theme};

pub(super) struct TreeContextMenu {
    path: String,
    is_directory: bool,
    expanded: bool,
    position: Point<Pixels>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TreeMenuAction {
    Open,
    ToggleFolder,
    Attach,
    Refresh,
    CopyPath,
    CopyRelativePath,
    CopyName,
    RevealInFileManager,
}

/// The on-disk location of `path` when it is on this machine; `None` for a
/// remote device's workspace, where a file manager has nothing to show.
fn local_disk_path(cwd: &str, path: &str) -> Option<PathBuf> {
    let full = display_path(cwd, path);
    if let Some(rest) = full.strip_prefix("~/") {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        return Some(PathBuf::from(home).join(rest));
    }
    full.starts_with('/').then(|| PathBuf::from(full))
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

impl FilesSurface {
    pub(super) fn open_tree_context_menu(
        &mut self,
        path: String,
        is_directory: bool,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let expanded = is_directory && self.tree.is_expanded(&path);
        self.tree_context_menu.open(TreeContextMenu {
            path,
            is_directory,
            expanded,
            position,
        });
        cx.notify();
    }

    pub(super) fn close_tree_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.tree_context_menu.begin_close() {
            popover::reap_popup(cx, |surface: &mut Self| &mut surface.tree_context_menu);
            cx.notify();
        }
    }

    pub(super) fn tree_context_menu_open(&self) -> bool {
        self.tree_context_menu.is_open()
    }

    fn run_tree_menu_action(
        &mut self,
        path: String,
        is_directory: bool,
        action: TreeMenuAction,
        cx: &mut Context<Self>,
    ) {
        self.close_tree_context_menu(cx);
        let cwd = self.request_context.as_ref().map(|c| c.cwd.clone());
        match action {
            TreeMenuAction::Open | TreeMenuAction::ToggleFolder => {
                self.activate_tree_path(path, cx)
            }
            TreeMenuAction::Attach => cx.emit(FilesEvent::AttachPath { path, is_directory }),
            TreeMenuAction::Refresh => {
                self.load_directory(path, None, cx);
            }
            TreeMenuAction::CopyPath => {
                if let Some(cwd) = cwd {
                    cx.write_to_clipboard(ClipboardItem::new_string(display_path(&cwd, &path)));
                }
            }
            TreeMenuAction::CopyRelativePath => {
                cx.write_to_clipboard(ClipboardItem::new_string(path));
            }
            TreeMenuAction::CopyName => {
                cx.write_to_clipboard(ClipboardItem::new_string(file_name(&path).to_string()));
            }
            TreeMenuAction::RevealInFileManager => {
                if let Some(disk) = cwd.and_then(|cwd| local_disk_path(&cwd, &path)) {
                    cx.reveal_path(&disk);
                }
            }
        }
    }

    fn tree_menu_row(
        theme: &Theme,
        id: &'static str,
        label: &'static str,
        menu: &TreeContextMenu,
        action: TreeMenuAction,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = menu.path.clone();
        let is_directory = menu.is_directory;
        popover::menu_row(theme, false, id)
            .id(id)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.run_tree_menu_action(path.clone(), is_directory, action, cx)
            }))
            .child(label)
            .into_any_element()
    }

    pub(super) fn render_tree_context_menu(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = &theme.for_popup();
        let menu = self.tree_context_menu.get()?;
        let closing = self.tree_context_menu.closing_since();
        let local = self.request_context.as_ref().is_some_and(|context| {
            context.target_device_id.is_none()
                && local_disk_path(&context.cwd, &menu.path).is_some()
        });
        let position = menu.position;

        let (open_id, open_label, open_action) = match (menu.is_directory, menu.expanded) {
            (false, _) => ("files-tree-menu-open", "Open", TreeMenuAction::Open),
            (true, false) => (
                "files-tree-menu-toggle",
                "Expand",
                TreeMenuAction::ToggleFolder,
            ),
            (true, true) => (
                "files-tree-menu-toggle",
                "Collapse",
                TreeMenuAction::ToggleFolder,
            ),
        };
        let mut card = popover::popover_card(theme)
            .w(px(200.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_tree_context_menu(cx)))
            .flex()
            .flex_col()
            .child(Self::tree_menu_row(
                theme,
                open_id,
                open_label,
                menu,
                open_action,
                cx,
            ))
            .child(Self::tree_menu_row(
                theme,
                "files-tree-menu-attach",
                "Add to chat",
                menu,
                TreeMenuAction::Attach,
                cx,
            ));
        if menu.is_directory {
            card = card.child(Self::tree_menu_row(
                theme,
                "files-tree-menu-refresh",
                "Refresh",
                menu,
                TreeMenuAction::Refresh,
                cx,
            ));
        }
        card = card
            .child(popover::menu_separator())
            .child(Self::tree_menu_row(
                theme,
                "files-tree-menu-copy-path",
                "Copy path",
                menu,
                TreeMenuAction::CopyPath,
                cx,
            ))
            .child(Self::tree_menu_row(
                theme,
                "files-tree-menu-copy-relative",
                "Copy relative path",
                menu,
                TreeMenuAction::CopyRelativePath,
                cx,
            ))
            .child(Self::tree_menu_row(
                theme,
                "files-tree-menu-copy-name",
                "Copy name",
                menu,
                TreeMenuAction::CopyName,
                cx,
            ));
        if local {
            card = card
                .child(popover::menu_separator())
                .child(Self::tree_menu_row(
                    theme,
                    "files-tree-menu-reveal",
                    "Reveal in file manager",
                    menu,
                    TreeMenuAction::RevealInFileManager,
                    cx,
                ));
        }
        Some(popover::menu_at(
            SharedString::from("files-tree-context-menu"),
            position,
            card.into_any_element(),
            closing,
        ))
    }

    /// Right-click on a row: select it, then open the menu at the pointer.
    pub(super) fn on_row_secondary_click(
        &mut self,
        path: String,
        is_directory: bool,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tree_focus.focus(window, cx);
        self.tree.select(path.clone());
        self.open_tree_context_menu(path, is_directory, position, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_path_joins_absolute_workspaces() {
        assert_eq!(
            local_disk_path("/w/app", "src/a.rs"),
            Some(PathBuf::from("/w/app/src/a.rs"))
        );
        assert_eq!(
            local_disk_path("/w/app", "/etc/hosts"),
            Some(PathBuf::from("/etc/hosts"))
        );
    }

    #[test]
    fn names_come_from_the_last_segment() {
        assert_eq!(file_name("src/main.rs"), "main.rs");
        assert_eq!(file_name("src/"), "src");
        assert_eq!(file_name("README.md"), "README.md");
    }

    fn menu_surface(
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::Entity<FilesSurface>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        let (files, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            FilesSurface::new_explorer(state, "chat".into(), false, cx)
        });
        files.update(cx, |files, _| {
            let page = zeron_proto::WorkspaceDirectoryPage {
                directory: String::new(),
                entries: vec![zeron_proto::WorkspaceEntry {
                    path: "src".into(),
                    name: "src".into(),
                    kind: zeron_proto::WorkspaceEntryKind::Directory,
                    size: None,
                    modified_at: None,
                    ignored: false,
                    read_only: false,
                }],
                next_cursor: None,
                truncated: false,
            };
            let generation = files.tree.generation();
            files.tree.apply_page(page, generation);
        });
        (files, cx)
    }

    #[gpui::test]
    fn right_click_selects_the_row_and_opens_its_menu(cx: &mut gpui::TestAppContext) {
        let (files, cx) = menu_surface(cx);
        files.update_in(cx, |files, window, cx| {
            assert!(!files.tree_context_menu_open());
            files.on_row_secondary_click(
                "src".into(),
                true,
                gpui::point(px(10.0), px(10.0)),
                window,
                cx,
            );
            assert!(files.tree_context_menu_open());
            assert_eq!(files.tree.selected(), Some("src"));
            files.close_tree_context_menu(cx);
        });
    }

    #[gpui::test]
    fn menu_actions_copy_and_attach_the_row_path(cx: &mut gpui::TestAppContext) {
        let (files, cx) = menu_surface(cx);
        let attached = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = attached.clone();
        let _events = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                if let FilesEvent::AttachPath { path, is_directory } = event {
                    sink.borrow_mut().push((path.clone(), *is_directory));
                }
            })
        });
        files.update(cx, |files, cx| {
            files.run_tree_menu_action("src/lib".into(), false, TreeMenuAction::CopyName, cx);
        });
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("lib")
        );
        files.update(cx, |files, cx| {
            files.run_tree_menu_action(
                "src/lib".into(),
                false,
                TreeMenuAction::CopyRelativePath,
                cx,
            );
        });
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("src/lib")
        );
        files.update(cx, |files, cx| {
            files.run_tree_menu_action("src".into(), true, TreeMenuAction::Attach, cx);
        });
        assert_eq!(*attached.borrow(), [("src".to_string(), true)]);
    }
}
