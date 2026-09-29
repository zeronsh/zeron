use super::*;

impl Shell {
    /// The entire conversation receives attachments in its own composer,
    /// regardless of which pane had keyboard focus when the drag started.
    pub(super) fn chat_dropzone(
        &self,
        id: impl Into<gpui::ElementId>,
        composer: Entity<Composer>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let images = composer.clone();
        let workspace = composer.clone();
        div()
            .id(id)
            .relative()
            .on_drop(cx.listener(move |_, paths: &gpui::ExternalPaths, _, cx| {
                images.update(cx, |composer, cx| {
                    composer.add_paths(paths.paths().to_vec(), cx)
                });
                cx.notify();
            }))
            .on_drop::<WorkspacePathDrag>(cx.listener(
                move |_, payload: &WorkspacePathDrag, window, cx| {
                    workspace.update(cx, |composer, cx| {
                        composer.add_workspace_path(&payload.path, payload.is_directory, window, cx)
                    });
                    cx.notify();
                },
            ))
            .on_drop::<RightTabDrag>(cx.listener(move |_, payload: &RightTabDrag, window, cx| {
                if let Some(path) = &payload.workspace_path {
                    composer.update(cx, |composer, cx| {
                        composer.add_workspace_path(&path.path, path.is_directory, window, cx)
                    });
                }
                cx.notify();
            }))
    }

    pub(super) fn attachment_drop_overlay(theme: &Theme) -> gpui::Stateful<gpui::Div> {
        // Use typed drag styles rather than cached shell state: an external
        // FileDrop::Exited clears the payload without a final mouse move.
        // Resize markers and non-file tabs must never reveal the overlay.
        div()
            .id("attachment-drop-overlay")
            .absolute()
            .inset_0()
            .opacity(0.0)
            .bg(theme.scrim().opacity(0.4 / 0.6))
            .flex()
            .items_center()
            .justify_center()
            .text_size(crate::typography::ui_rems(13.0))
            .text_color(theme.text)
            .drag_over::<gpui::ExternalPaths>(|style, _, _, _| style.opacity(1.0))
            .drag_over::<WorkspacePathDrag>(|style, _, _, _| style.opacity(1.0))
            .drag_over::<RightTabDrag>(|style, tab, _, _| {
                if tab.workspace_path.is_some() {
                    style.opacity(1.0)
                } else {
                    style
                }
            })
            .child("Drop to attach")
    }
}
