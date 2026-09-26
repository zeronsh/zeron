use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};
use zeron_proto::{WorkspaceDirectoryPage, WorkspaceEntry, WorkspaceEntryKind};

pub(crate) fn state() -> AppState {
    let mut state = AppState::new();
    state.local_device_id = Some("device".into());
    state.chats.push(
        serde_json::from_value(serde_json::json!({
            "id":"chat", "deviceId":"device", "archived":false, "cwd":"/workspace",
            "checkoutId":"checkout", "createdAt":"2026-01-01T00:00:00Z"
        }))
        .unwrap(),
    );
    state
}

pub(crate) fn explorer(state: Entity<AppState>, cx: &mut Context<FilesSurface>) -> FilesSurface {
    let mut files = FilesSurface::new_explorer(state, "chat".into(), false, cx);
    files.effective_checkout_id = Some("checkout".into());
    files.mutation_capabilities = Some(zeron_proto::WorkspaceMutationCapabilities {
        move_entry: true,
        delete_entry: true,
    });
    files.tree.begin_load("", None, files.tree.generation());
    files.tree.apply_page(
        WorkspaceDirectoryPage {
            directory: "".into(),
            checkout_id: Some("checkout".into()),
            mutation_capabilities: files.mutation_capabilities,
            entries: vec![
                entry("a.txt", WorkspaceEntryKind::File),
                entry("folder", WorkspaceEntryKind::Directory),
            ],
            next_cursor: None,
            truncated: false,
        },
        files.tree.generation(),
    );
    files.sync_tree_list();
    files
}

pub(super) fn setup(cx: &mut TestAppContext) -> (Entity<FilesSurface>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(crate::theme::Theme::dark());
    });
    let (files, cx) = cx.add_window_view(|_, cx| {
        let state = cx.new(|_| state());
        explorer(state, cx)
    });
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear();
    });
    (files, cx)
}
pub(super) fn entry(path: &str, kind: WorkspaceEntryKind) -> WorkspaceEntry {
    WorkspaceEntry {
        path: path.into(),
        name: path.rsplit('/').next().unwrap().into(),
        kind,
        size: Some(1),
        modified_at: None,
        ignored: false,
        read_only: false,
        mutation_revision: Some("rev".into()),
    }
}
