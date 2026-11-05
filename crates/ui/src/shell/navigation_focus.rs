//! Focus boundaries shared by contextual navigation and focus recovery.
use super::*;

pub(super) struct NavigationFocus {
    pub(super) main: FocusHandle,
    pub(super) right: FocusHandle,
    // The titlebar strip is a sibling of the pane, not its descendant.
    pub(super) tabs: FocusHandle,
    pub(super) right_was_focused: bool,
}

impl NavigationFocus {
    pub(super) fn new(cx: &mut App) -> Self {
        Self {
            main: cx.focus_handle(),
            right: cx.focus_handle(),
            tabs: cx.focus_handle(),
            right_was_focused: false,
        }
    }

    pub(super) fn in_right(&self, window: &Window, cx: &App) -> bool {
        self.right.contains_focused(window, cx) || self.tabs.contains_focused(window, cx)
    }

    pub(super) fn remember(&mut self, root: &FocusHandle, window: &Window, cx: &App) {
        // Retain the origin while an element is being detached. A mounted
        // focus elsewhere (including the explorer or a modal) takes precedence.
        if root.contains_focused(window, cx) {
            self.right_was_focused = self.in_right(window, cx);
        }
    }
}

/// Capture establishes a fallback before child listeners run. Inputs can
/// claim focus during bubbling, including listeners that stop propagation.
/// Clicking within an already focused input must not blur it first.
fn focus_navigation_scope(scope: &FocusHandle, window: &mut Window, cx: &mut App) {
    if !scope.contains_focused(window, cx) {
        window.focus(scope, cx);
    }
    let scope = scope.clone();
    window.defer(cx, move |window, cx| {
        // ComposerInput explicitly blurs on mouse-down outside itself. Keep
        // that click in its pane without moving the caret back into an input.
        if window.focused(cx).is_none() {
            window.focus(&scope, cx);
        }
    });
}

/// Cmd/Ctrl+C for a transcript selection once the pane itself holds focus
/// (clicking the transcript blurs the composer, whose `Copy` handles this
/// otherwise). Bubble phase: focused inputs, editors and terminals see the
/// keystroke first, so this only answers when none of them consumed it.
pub(super) fn copy_transcript_selection(event: &gpui::KeyDownEvent, cx: &mut App) {
    let keystroke = &event.keystroke;
    if keystroke.key != "c"
        || !(keystroke.modifiers.platform || keystroke.modifiers.control)
        || keystroke.modifiers.shift
        || keystroke.modifiers.alt
    {
        return;
    }
    if let Some(text) = crate::markdown::selection::selected_text() {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.stop_propagation();
    }
}

impl Shell {
    pub(super) fn capture_navigation_focus(
        &mut self,
        right: bool,
        tabs: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation_focus.right_was_focused = right;
        let scope = if tabs {
            &self.navigation_focus.tabs
        } else if right {
            &self.navigation_focus.right
        } else {
            &self.navigation_focus.main
        };
        focus_navigation_scope(scope, window, cx);
    }

    pub(super) fn navigation_focus_fallback(&self, cx: &App) -> (FocusHandle, FocusHandle) {
        let right_open = match self.route {
            Route::Chat => self.right_pane_open(cx),
            Route::PullRequests => self.pull_request_pane_open(),
            Route::Settings(_) => false,
        };
        if matches!(self.route, Route::Settings(_)) {
            (self.settings_focus.clone(), self.unfocused.clone())
        } else if right_open && self.navigation_focus.right_was_focused {
            (
                self.navigation_focus.right.clone(),
                self.navigation_focus.right.clone(),
            )
        } else if self.voice.read(cx).stage_open {
            (self.voice_stage_focus.clone(), self.unfocused.clone())
        } else {
            (self.composer.focus_handle(cx), self.unfocused.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    struct FocusHost {
        root: FocusHandle,
        navigation: NavigationFocus,
        input: Entity<ComposerInput>,
        show_input: bool,
    }

    impl Render for FocusHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.root)
                .flex()
                .child(
                    div()
                        .id("main")
                        .w(px(200.))
                        .h_full()
                        .track_focus(&self.navigation.main)
                        .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                            focus_navigation_scope(&this.navigation.main, window, cx);
                        })),
                )
                .child(
                    div()
                        .id("right")
                        .w(px(200.))
                        .h_full()
                        .track_focus(&self.navigation.right)
                        .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                            focus_navigation_scope(&this.navigation.right, window, cx);
                        }))
                        .when(self.show_input, |el| el.child(self.input.clone())),
                )
        }
    }

    #[gpui::test]
    fn clicking_read_only_content_keeps_focus_in_its_pane(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::default()));
        let (host, cx) = cx.add_window_view(|_, cx| FocusHost {
            root: cx.focus_handle(),
            navigation: NavigationFocus::new(cx),
            input: cx.new(|cx| ComposerInput::new("Message", cx)),
            show_input: true,
        });
        cx.update(|window, cx| {
            window.draw(cx).clear();
            let input = host.read(cx).input.focus_handle(cx);
            window.focus(&input, cx);
        });
        // Real input click-away blurs during capture. The pane fallback must
        // preserve keyboard navigation without re-entering the input.
        cx.simulate_click(gpui::point(px(300.), px(250.)), gpui::Modifiers::default());
        cx.update(|window, cx| {
            assert!(host.read(cx).navigation.right.is_focused(window));
        });
        cx.simulate_click(gpui::point(px(100.), px(250.)), gpui::Modifiers::default());
        cx.update(|window, cx| {
            assert!(host.read(cx).navigation.main.is_focused(window));
        });
    }
}
