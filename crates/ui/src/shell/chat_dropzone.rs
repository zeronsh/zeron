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
        let mention = composer.clone();
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
                move |this, payload: &WorkspacePathDrag, window, cx| {
                    this.attach_workspace_drag(payload, &workspace, window, cx);
                    cx.notify();
                },
            ))
            // A session dragged from the sidebar becomes a chat chip here,
            // and its row slides home.
            .on_drop::<SidebarSessionDrag>(cx.listener(
                move |this, payload: &SidebarSessionDrag, window, cx| {
                    let title = this
                        .state
                        .read(cx)
                        .chats
                        .iter()
                        .find(|chat| chat.id == payload.chat_id)
                        .map(|chat| chat.title.clone().unwrap_or_default());
                    if let Some(title) = title {
                        mention.update(cx, |composer, cx| {
                            composer.add_chat_mention(&payload.chat_id, &title, window, cx)
                        });
                    }
                    this.cancel_sidebar_session_transfer(cx);
                    cx.stop_propagation();
                    cx.notify();
                },
            ))
            .on_drop::<RightTabDrag>(cx.listener(
                move |this, payload: &RightTabDrag, window, cx| {
                    if let Some(path) = &payload.workspace_path {
                        this.attach_workspace_drag(path, &composer, window, cx);
                    }
                    cx.notify();
                },
            ))
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

    /// The hint while a sidebar session is carried over a chat.
    pub(super) fn mention_drop_overlay(theme: &Theme) -> gpui::Stateful<gpui::Div> {
        div()
            .id("mention-drop-overlay")
            .absolute()
            .inset_0()
            .opacity(0.0)
            .bg(theme.scrim().opacity(0.4 / 0.6))
            .flex()
            .items_center()
            .justify_center()
            .text_size(crate::typography::ui_rems(13.0))
            .text_color(theme.text)
            .drag_over::<SidebarSessionDrag>(|style, _, _, _| style.opacity(1.0))
            .child("Drop to mention")
    }
}
