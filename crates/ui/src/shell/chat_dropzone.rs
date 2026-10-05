use super::*;

pub(super) struct ChatContextDragGhost {
    pub shell: gpui::WeakEntity<Shell>,
    pub chat_id: String,
}

impl Render for ChatContextDragGhost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let shell = shell.read(cx);
        // Inside the sidebar the existing animated row is the drag preview.
        if f32::from(window.mouse_position().x) <= shell.settings.sidebar_width {
            return div().into_any_element();
        }
        let title = shell
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == self.chat_id)
            .and_then(|chat| chat.title.clone())
            .unwrap_or_else(|| "Untitled chat".into());
        let theme = Theme::of(cx);
        div()
            .px(px(12.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(px(8.))
            .max_w(px(280.))
            .text_size(crate::typography::ui_rems(12.))
            .text_color(theme.text)
            .child(icon(icons::CHAT_ROUND_LINE).size(px(14.)))
            .child(div().truncate().child(SharedString::from(title)))
            .into_any_element()
    }
}

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
        let chats = composer.clone();
        div()
            .id(id)
            .relative()
            .on_drop::<SidebarSessionDrag>(cx.listener(
                move |this, payload: &SidebarSessionDrag, window, cx| {
                    // Revalidate profile and source at release. Navigation or
                    // deletion during a drag must not attach a stale identity.
                    if this.sidebar_session_transfer_is_valid(payload, cx)
                        && chats.read(cx).context_chat_id(cx).as_deref() != Some(&payload.chat_id)
                    {
                        let reference = this
                            .state
                            .read(cx)
                            .chats
                            .iter()
                            .find(|chat| chat.id == payload.chat_id)
                            .and_then(|chat| {
                                zeron_proto::chat_mentions::ChatReference::new(
                                    &chat.id,
                                    chat.title.as_deref().unwrap_or("Untitled chat"),
                                )
                            });
                        if let Some(reference) = reference {
                            chats.update(cx, |composer, cx| {
                                composer.add_chat_reference(&reference, window, cx)
                            });
                        }
                    }
                    this.cancel_sidebar_session_transfer(cx);
                    cx.stop_propagation();
                },
            ))
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

    pub(super) fn chat_context_drop_overlay(
        &self,
        composer: &Entity<Composer>,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = Theme::of(cx);
        let own_id = composer.read(cx).context_chat_id(cx);
        div()
            .id("chat-context-drop-overlay")
            .absolute()
            .inset_0()
            .opacity(0.0)
            .bg(theme.scrim().opacity(0.4 / 0.6))
            .flex()
            .items_center()
            .justify_center()
            .text_size(crate::typography::ui_rems(13.0))
            .text_color(theme.text)
            .drag_over::<SidebarSessionDrag>(move |style, payload, _, _| {
                if own_id.as_deref() != Some(payload.chat_id.as_str()) {
                    style.opacity(1.0)
                } else {
                    style
                }
            })
            .child("Drop to add chat context")
    }
}

/// Isolated visual evidence uses the production surfaces without a live agent.
#[cfg(feature = "chat-context-fixture")]
impl Shell {
    pub fn fixture_chat_context_prepare(&mut self, cx: &mut Context<Self>) {
        self._state_observation = cx.observe(&self.state, |_, _, _| {});
        self.active_chat = self.state.read(cx).selected_chat.clone().unwrap();
        self.splash = SplashPhase::Gone;
        self.composer.update(cx, |composer, cx| {
            composer.input.update(cx, |input, cx| {
                input.set_text("Use the decisions from ", cx)
            });
        });
        cx.notify();
    }

    pub fn fixture_chat_context_text(&self, side: bool, cx: &App) -> String {
        let composer = if side {
            &self.side_chats[&1].composer
        } else {
            &self.composer
        };
        composer.read(cx).input.read(cx).text().to_owned()
    }

    pub fn fixture_chat_context_side(&mut self, cx: &mut Context<Self>) {
        self.create_child_chat(None, cx);
        self.right_tween = None;
        cx.notify();
    }
}
