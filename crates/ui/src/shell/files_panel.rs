//! Session-owned explorer chrome, independent of the surface tab host.

use super::*;
use crate::settings::{FILES_PANEL_DEFAULT, FILES_PANEL_MAX, FILES_PANEL_MIN};

pub(super) struct FilesPanelResize;

/// Allocate a real column to Files. Reduce its preferred width before taking
/// space from the chat/editor minima; below those minima, share the shortage
/// proportionally so no open panel covers another.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FilesPanelLayout {
    width: f32,
    surface_max: f32,
}

fn files_panel_layout(
    viewport: f32,
    sidebar: f32,
    preferred: f32,
    visible: f32,
    surfaces_open: bool,
    expanded: bool,
) -> FilesPanelLayout {
    let available = (viewport - sidebar).max(0.0);
    let chat_min = if surfaces_open && expanded {
        0.0
    } else {
        CHAT_PANEL_MIN
    };
    let surface_min = if surfaces_open { RIGHT_PANE_MIN } else { 0.0 };
    let scale = if preferred > 0.0 {
        (available / (chat_min + surface_min + preferred.min(FILES_PANEL_MIN))).min(1.0)
    } else {
        // Preserve the existing chat floor when Files is closed.
        1.0
    };
    let max_width = (available - (chat_min + surface_min) * scale).max(0.0);
    let width = visible.max(0.0).min(max_width);
    // As Files animates closed, return its space to the remaining columns
    // smoothly instead of changing their minima when the tween finishes.
    let content_scale = if preferred > 0.0 {
        ((available - width) / (chat_min + surface_min)).min(1.0)
    } else {
        1.0
    };
    FilesPanelLayout {
        width,
        surface_max: right_pane_max_width(viewport - width, sidebar, chat_min * content_scale),
    }
}

impl Shell {
    pub(super) fn files_panel_open(&self, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && !self.active_chat.is_empty()
            && self.panels.get(&self.panel_key(cx)).files_open
    }

    fn files_layout(&self, visible: f32, cx: &App) -> FilesPanelLayout {
        files_panel_layout(
            self.viewport_width,
            self.sidebar_now(),
            if self.files_panel_open(cx) || self.tween_active(self.files_tween) {
                self.settings.files_panel_width
            } else {
                0.0
            },
            visible,
            self.right_pane_open(cx),
            self.right_pane_expanded,
        )
    }

    pub(super) fn files_target(&self, cx: &App) -> f32 {
        self.files_layout(
            if self.files_panel_open(cx) {
                self.settings.files_panel_width
            } else {
                0.0
            },
            cx,
        )
        .width
    }

    pub(super) fn files_visible_width(&self, cx: &App) -> f32 {
        if !matches!(self.route, Route::Chat) || self.active_chat.is_empty() {
            return 0.0;
        }
        self.files_layout(self.eval_tween(self.files_tween, self.files_target(cx)), cx)
            .width
    }

    pub(super) fn files_reserved_width(&self, cx: &App) -> f32 {
        self.files_visible_width(cx)
    }

    pub(super) fn surface_max_width(&self, cx: &App) -> f32 {
        self.files_layout(self.files_visible_width(cx), cx)
            .surface_max
    }

    pub(super) fn right_visible_width(&self, cx: &App) -> f32 {
        let available =
            (self.viewport_width - self.sidebar_now() - self.files_visible_width(cx)).max(0.0);
        self.right_now(cx).min(available)
    }

    fn clear_surface_transitions(&mut self) {
        self.right_tween = None;
        self.main_takeover_tween = None;
        self.right_takeover_content_tween = None;
    }

    pub(super) fn accepts_file_navigation(
        &self,
        owner: &str,
        source: &Entity<FilesSurface>,
        cx: &App,
    ) -> bool {
        matches!(self.route, Route::Chat)
            && self.panel_key(cx) == owner
            && source.read(cx).is_current_target(cx)
    }

    pub(super) fn prune_file_explorers(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if !state.chats_synced {
            return;
        }
        let live = state
            .chats
            .iter()
            .filter(|chat| !chat.archived)
            .map(|chat| chat.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        for key in self.files.keys().filter(|key| !live.contains(key.as_str())) {
            self.panels.update(key, |panels| panels.files_open = false);
        }
        self.files.retain(|key, _| live.contains(key.as_str()));
        self.files_subs.retain(|key, _| live.contains(key.as_str()));
    }

    pub(super) fn sync_explorer_selection(&mut self, cx: &mut Context<Self>) {
        let RightSurface::File(id) = self.resolved_right_active(cx) else {
            return;
        };
        let Some(path) = self.file_surface_paths.get(&id).cloned() else {
            return;
        };
        if let Some(files) = self.files.get(&self.panel_key(cx)).cloned() {
            files.update(cx, |files, cx| files.reveal_file(path, cx));
        }
    }

    pub(super) fn add_files_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_chat.is_empty() {
            return;
        }
        let key = self.panel_key(cx);
        if !self.files.contains_key(&key) {
            let files = cx.new(|cx| {
                FilesSurface::new_explorer(
                    self.state.clone(),
                    self.active_chat.clone(),
                    self.settings.files_show_all,
                    cx,
                )
            });
            let owner = key.clone();
            let sub = cx.subscribe_in(
                &files,
                window,
                move |this: &mut Self, source, event, window, cx| match event {
                    FilesEvent::OpenFile(path)
                        if this.accepts_file_navigation(&owner, &source, cx) =>
                    {
                        this.add_file_surface(path.clone(), window, cx);
                    }
                    FilesEvent::OpenWebLink(activation) => {
                        if let crate::markdown::render::LinkOutcome::External(url) =
                            this.activate_session_link(activation, window, cx)
                        {
                            cx.open_url(&url);
                        }
                    }
                    FilesEvent::ShowAllFilesChanged(show_all) => {
                        this.set_files_show_all(*show_all, cx)
                    }
                    _ => cx.notify(),
                },
            );
            self.files.insert(key.clone(), files);
            self.files_subs.insert(key.clone(), sub);
        }
        let from = self.files_visible_width(cx);
        let was_open = self.files_panel_open(cx);
        self.panels.update(&key, |p| p.files_open = true);
        if !was_open {
            self.clear_surface_transitions();
            self.files_tween = Some(WidthTween::new(from, self.files_target(cx)));
        }
        if let Some(files) = self.files.get(&key).cloned() {
            files.update(cx, |files, cx| {
                files.ensure_loaded(cx);
                files.focus_explorer(window, cx);
            });
        }
        self.composer
            .update(cx, |composer, _| composer.focus_pending = false);
        cx.notify();
    }

    /// Undock the explorer portion. With the surface host closed too, this
    /// closes the right pane entirely.
    pub(super) fn close_files_panel(&mut self, cx: &mut Context<Self>) {
        if !self.files_panel_open(cx) {
            return;
        }
        let from = self.files_visible_width(cx);
        self.panels
            .update(&self.panel_key(cx), |p| p.files_open = false);
        self.clear_surface_transitions();
        self.files_tween = Some(WidthTween::new(from, 0.0));
        cx.notify();
    }

    /// The explorer's own toggle. Opening docks the explorer into the right
    /// pane — opening the pane with just that portion when it was closed.
    pub(super) fn toggle_files_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_panel_open(cx) {
            self.add_files_surface(window, cx);
            return;
        }
        self.close_files_panel(cx);
        window.focus(&self.composer.focus_handle(cx), cx);
        if self.right_pane_open(cx) {
            self.focus_right_file_editor(self.resolved_right_active(cx), window, cx);
        }
        cx.notify();
    }

    pub(super) fn on_files_panel_drag(
        &mut self,
        event: &gpui::DragMoveEvent<FilesPanelResize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested = f32::from(window.viewport_size().width) - f32::from(event.event.position.x);
        self.settings.files_panel_width = requested.clamp(FILES_PANEL_MIN, FILES_PANEL_MAX);
        self.pane_resize_dragging = Some(PaneResizeKind::Files);
        self.pane_resize_active = (requested > FILES_PANEL_MIN && requested < FILES_PANEL_MAX)
            .then_some(PaneResizeKind::Files);
        self.files_tween = None;
        self.clear_surface_transitions();
        self.schedule_save(cx);
        cx.notify();
    }

    pub(super) fn render_files_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.panel_key(cx);
        let visible = matches!(self.route, Route::Chat) && self.files_panel_open(cx);
        for (key, files) in &self.files {
            if !visible || *key != active {
                files.update(cx, |files, _| files.release_git_status());
            }
        }
        if !matches!(self.route, Route::Chat)
            || self.active_chat.is_empty()
            || (!self.files_panel_open(cx) && !self.tween_active(self.files_tween))
        {
            return Empty.into_any_element();
        }
        let theme = Theme::of(cx).clone();
        let content = self.files.get(&self.panel_key(cx)).cloned();
        if let Some(files) = &content {
            files.update(cx, |files, cx| {
                files.ensure_loaded(cx);
                if visible {
                    files.ensure_git_status(cx);
                }
            });
        }
        self.sync_explorer_selection(cx);
        let target = self.files_target(cx);
        let content_width =
            stable_panel_content_width(target, self.active_tween_endpoints(self.files_tween));
        // The explorer is the right pane's rightmost column: its left hairline
        // is the divider from the surface host (or the pane's own edge when
        // that portion is closed), and it carries the CSD window's right
        // corners in place of the surface column (see `render_right_pane`).
        let corner = Self::window_corner_radius(window);
        let inner = div()
            .w(px(content_width))
            .h_full()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .occlude()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.panel_bg())
            .when(corner > 0.0, |el| {
                el.rounded_tr(px(corner)).rounded_br(px(corner))
            })
            .overflow_hidden()
            .children(content);
        div()
            .id("files-panel")
            .h_full()
            .flex_none()
            .relative()
            .child(
                div()
                    .h_full()
                    .w(px(self.files_visible_width(cx)))
                    .overflow_hidden()
                    .child(inner),
            )
            .when(
                self.files_panel_open(cx) && !self.tween_active(self.files_tween),
                |panel| {
                    panel.child(
                        self.resize_handle(
                            "files-panel-resize",
                            PaneResizeKind::Files,
                            || FilesPanelResize,
                            |shell, _| shell.settings.files_panel_width = FILES_PANEL_DEFAULT,
                            cx,
                        )
                        .left(px(-PANE_RESIZE_HITBOX_HALF_WIDTH)),
                    )
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[test]
    fn files_layout_shrinks_the_tree_before_the_chat_or_editor() {
        let docked = files_panel_layout(1400.0, 256.0, 286.0, 286.0, true, false);
        assert_eq!(
            docked,
            FilesPanelLayout {
                width: 286.0,
                surface_max: 558.0,
            }
        );
        let narrow = files_panel_layout(1200.0, 256.0, 440.0, 440.0, true, false);
        assert_eq!(
            narrow,
            FilesPanelLayout {
                width: 284.0,
                surface_max: 360.0,
            }
        );
        assert_eq!(
            1200.0 - 256.0 - narrow.width - narrow.surface_max,
            CHAT_PANEL_MIN
        );
        // Closing the surface or expanding it releases space for the tree.
        assert_eq!(
            files_panel_layout(1200.0, 256.0, 440.0, 440.0, false, false).width,
            440.0
        );
        let expanded = files_panel_layout(1100.0, 256.0, 286.0, 286.0, true, true);
        assert_eq!(expanded.width, 286.0);
        assert_eq!(expanded.surface_max, 558.0);
        // Growing the viewport restores the preferred width.
        assert_eq!(
            files_panel_layout(1600.0, 256.0, 440.0, 440.0, true, false).width,
            440.0
        );
    }

    #[test]
    fn files_layout_returns_space_smoothly_during_close() {
        let mut previous_chat = 0.0;
        for visible in [186.0, 140.0, 84.0, 40.0, 0.0] {
            let layout = files_panel_layout(1000.0, 256.0, 286.0, visible, true, false);
            let chat = 744.0 - layout.width - layout.surface_max;
            assert!(chat >= previous_chat && chat <= CHAT_PANEL_MIN);
            previous_chat = chat;
        }
        assert_eq!(
            files_panel_layout(1000.0, 256.0, 286.0, 0.0, true, false),
            files_panel_layout(1000.0, 256.0, 0.0, 0.0, true, false),
        );
    }

    #[test]
    fn files_layout_shares_tight_windows_without_covering_any_column() {
        let compact = files_panel_layout(1000.0, 256.0, 440.0, 440.0, true, false);
        let chat = 1000.0 - 256.0 - compact.width - compact.surface_max;
        assert!((compact.width / FILES_PANEL_MIN - chat / CHAT_PANEL_MIN).abs() < 0.001);
        assert!((compact.surface_max / RIGHT_PANE_MIN - chat / CHAT_PANEL_MIN).abs() < 0.001);
        for viewport in [0.0, 120.0, 280.0, 600.0, 1000.0, 1200.0, 1600.0] {
            for sidebar in [0.0, 256.0, 400.0] {
                for surfaces in [false, true] {
                    for expanded in [false, true] {
                        for visible in [0.0, 1.0, 140.0, 440.0] {
                            let layout = files_panel_layout(
                                viewport, sidebar, 440.0, visible, surfaces, expanded,
                            );
                            let available = (viewport - sidebar).max(0.0);
                            assert!(layout.width >= 0.0 && layout.width <= visible);
                            assert!(layout.surface_max >= 0.0);
                            assert!(layout.width + layout.surface_max <= available + 0.001);
                            if available > 0.0 && surfaces {
                                assert!(layout.surface_max > 0.0, "the editor must remain visible");
                                if visible > 0.0 {
                                    assert!(layout.width > 0.0, "the tree must remain visible");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn explorer_and_editor_panels_have_independent_session_lifetimes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, window, cx| {
                shell.add_files_surface(window, cx);
                assert!(
                    shell.files.is_empty(),
                    "the new-session canvas has no explorer"
                );
                shell.active_chat = "first".into();
                shell.add_files_surface(window, cx);
                let explorer = shell.files["first"].entity_id();
                assert!(shell.files_panel_open(cx));
                assert!(!shell.right_pane_open(cx));
                assert!(shell.right_surface_rows(cx).is_empty());
                shell.add_files_surface(window, cx);
                assert_eq!(shell.files["first"].entity_id(), explorer);
                shell.add_file_surface("src/main.rs".into(), window, cx);
                shell.add_file_surface("src/main.rs".into(), window, cx);
                assert_eq!(shell.file_surfaces.len(), 1);
                assert_eq!(shell.right_surface_rows(cx).len(), 1);
                assert!(shell.right_pane_open(cx));
                shell.toggle_files_panel(window, cx);
                assert!(!shell.files_panel_open(cx));
                assert!(shell.right_pane_open(cx));
                assert_eq!(shell.file_surfaces.len(), 1);
                assert!(shell.pending_file_closes.is_empty());
                shell.active_chat = "second".into();
                assert!(!shell.files_panel_open(cx));
                shell.add_files_surface(window, cx);
                assert_ne!(shell.files["second"].entity_id(), explorer);
                shell.active_chat = "first".into();
                assert!(!shell.files_panel_open(cx));
                shell.add_files_surface(window, cx);
                assert_eq!(shell.files["first"].entity_id(), explorer);
                shell.route = Route::Settings(SettingsSection::Files);
                assert!(!shell.files_panel_open(cx));
                assert_eq!(shell.files_reserved_width(cx), 0.0);
                shell.route = Route::Chat;
                assert!(shell.files_panel_open(cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn pane_toggle_drives_surfaces_only_and_last_tab_close_collapses_them(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, window, cx| {
                shell.active_chat = "chat".into();
                // Widths settle immediately so the assertions see end states.
                shell.reduced_motion = true;
                // A fresh pane toggle lands on the surface host alone.
                shell.toggle_right_pane(cx);
                assert!(shell.right_pane_open(cx));
                assert!(!shell.files_panel_open(cx));
                shell.toggle_right_pane(cx);
                assert!(!shell.right_pane_open(cx));
                assert!(!shell.files_panel_open(cx));

                // The explorer toggle opens the pane with only its portion.
                shell.toggle_files_panel(window, cx);
                assert!(shell.files_panel_open(cx));
                assert!(!shell.right_pane_open(cx));
                assert!(shell.files_visible_width(cx) > 0.0);
                // With only the explorer docked, the pane toggle opens the
                // surface host beside it instead of closing the pane, and it
                // never hides the explorer.
                shell.toggle_right_pane(cx);
                assert!(shell.right_pane_open(cx) && shell.files_panel_open(cx));
                shell.toggle_right_pane(cx);
                assert!(!shell.right_pane_open(cx) && shell.files_panel_open(cx));

                // Opening a file docks the surface host beside the explorer;
                // programmatic opens never close an open pane.
                shell.add_file_surface("src/main.rs".into(), window, cx);
                assert!(shell.right_pane_open(cx) && shell.files_panel_open(cx));
                shell.set_surfaces_open(true, cx);
                assert!(shell.right_pane_open(cx) && shell.files_panel_open(cx));
                assert_eq!(shell.file_surfaces.len(), 1);

                // Closing the last surface tab collapses the surface host and
                // leaves the pane open with just the explorer.
                let file = shell.right_surface_rows(cx)[0].0;
                shell.close_right_surface(file, window, cx);
                assert!(shell.file_surfaces.is_empty());
                assert!(!shell.right_pane_open(cx) && shell.files_panel_open(cx));

                // Only the explorer toggle undocks it; with nothing else open
                // that closes the pane entirely.
                shell.toggle_files_panel(window, cx);
                assert!(!shell.right_pane_open(cx) && !shell.files_panel_open(cx));
                assert_eq!(shell.files_reserved_width(cx), 0.0);

                // With the explorer undocked, the last tab close closes the
                // whole pane.
                shell.add_file_surface("src/main.rs".into(), window, cx);
                assert!(shell.right_pane_open(cx));
                let file = shell.right_surface_rows(cx)[0].0;
                shell.close_right_surface(file, window, cx);
                assert!(!shell.right_pane_open(cx) && !shell.files_panel_open(cx));
            })
            .unwrap();
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "files_panel_workspace_tests.rs"]
mod workspace_tests;
