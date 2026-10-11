//! Session-owned explorer chrome, independent of the surface tab host.

use super::*;
use crate::settings::{FILES_PANEL_MAX, FILES_PANEL_MIN};

pub(super) struct FilesPanelResize;

/// Files kept per chat for the command palette's Recent Files.
const RECENT_FILES_PER_CHAT: usize = 20;

impl Shell {
    /// Record `path` as the chat's most recently opened file.
    pub(super) fn note_recent_file(&mut self, chat_id: &str, path: &str) {
        if chat_id.is_empty() {
            return;
        }
        let recent = self.recent_files.entry(chat_id.to_owned()).or_default();
        recent.retain(|candidate| candidate != path);
        recent.push_front(path.to_owned());
        recent.truncate(RECENT_FILES_PER_CHAT);
    }

    /// The chat's opened files, most recent first.
    pub(super) fn recent_files(&self, chat_id: &str) -> impl Iterator<Item = &String> {
        self.recent_files.get(chat_id).into_iter().flatten()
    }
}

/// Allocate a real column to Files, reducing its preferred width before the
/// chat or surface minima. The shell fit policy hides a whole column when its
/// minimum cannot fit, so no settled column is rendered as a sliver.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FilesPanelLayout {
    width: f32,
    surface_max: f32,
}

/// `surface_reveal` is how far the surface host is open, from 0 to 1. Files
/// yields the host's minimum in step with its open/close tween rather than
/// all at once when the flag flips.
fn files_panel_layout(
    viewport: f32,
    sidebar: f32,
    preferred: f32,
    visible: f32,
    surface_reveal: f32,
    expanded: bool,
) -> FilesPanelLayout {
    let surfaces_open = surface_reveal > 0.0;
    let available = (viewport - sidebar).max(0.0);
    // Takeover gives the conversation's space to the surface host. It must
    // not also widen Files: that would move the seam before the right tween
    // has painted its first frame, and move it back again on reversal.
    let surface_chat_min = if surfaces_open && expanded {
        0.0
    } else {
        CHAT_PANEL_MIN
    };
    let surface_min = RIGHT_PANE_MIN * surface_reveal.clamp(0.0, 1.0);
    let max_width = if preferred > 0.0 {
        (available - CHAT_PANEL_MIN - surface_min).max(0.0)
    } else {
        0.0
    };
    let width = visible.max(0.0).min(max_width);
    FilesPanelLayout {
        width,
        surface_max: right_pane_max_width(viewport - width, sidebar, surface_chat_min),
    }
}

impl Shell {
    pub(super) fn files_panel_open(&self, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && !self.active_chat.is_empty()
            && self.panels.get(&self.panel_key(cx)).files_open
    }

    fn files_layout(&self, visible: f32, cx: &App) -> FilesPanelLayout {
        let fit = self.horizontal_fit();
        files_panel_layout(
            self.viewport_width,
            self.sidebar_now(),
            if fit.files && (self.files_panel_open(cx) || self.tween_active(self.files_tween)) {
                self.settings.files_panel_width
            } else {
                0.0
            },
            if fit.files { visible } else { 0.0 },
            if fit.right {
                self.right_reveal()
            } else {
                // A host the fit is hiding still occupies its exit width.
                self.fit_exit_width(1)
                    .map_or(0.0, |width| (width / RIGHT_PANE_MIN).clamp(0.0, 1.0))
            },
            self.right_pane_expanded && fit.right,
        )
    }

    /// How far the surface host is open: its open/close tween's progress,
    /// 1 at rest while open. Width-only tweens between open sizes stay at 1.
    fn right_reveal(&self) -> f32 {
        // What the host actually occupies, up to its minimum: continuous
        // through reversals, which restart the tween from a partial width.
        match self
            .right_tween
            .filter(|tween| self.tween_active(Some(*tween)))
        {
            Some(tween) => {
                (self.eval_tween(Some(tween), tween.to) / RIGHT_PANE_MIN).clamp(0.0, 1.0)
            }
            None => 1.0,
        }
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
        if !self.horizontal_fit().files
            && let Some(exiting) = self.fit_exit_width(2)
        {
            // Exits clamp in layout order so the columns never overflow.
            return exiting.min((self.viewport_width - self.sidebar_now()).max(0.0));
        }
        self.files_layout(
            self.ease_toward(self.files_tween, self.files_target(cx)),
            cx,
        )
        .width
    }

    pub(super) fn files_reserved_width(&self, cx: &App) -> f32 {
        self.files_visible_width(cx)
    }

    /// Files' width once every running open/close has landed. Initial surface
    /// targets use this allocation; painted widths follow the live layout.
    pub(super) fn files_settled_width(&self, cx: &App) -> f32 {
        let (fit, sidebar) = self.settled_fit();
        self.files_width_for(fit, sidebar, cx)
    }

    /// Files' width for a given fit and sidebar width: its own width, as far
    /// as the conversation and the surface host's minimums leave room.
    pub(super) fn files_width_for(&self, fit: HorizontalPanelFit, sidebar: f32, cx: &App) -> f32 {
        let preferred = if fit.files && self.files_panel_open(cx) {
            self.settings.files_panel_width
        } else {
            0.0
        };
        let right = fit.right && self.right_pane_open(cx);
        files_panel_layout(
            self.viewport_width,
            sidebar,
            preferred,
            preferred,
            if right { 1.0 } else { 0.0 },
            self.right_pane_expanded && right,
        )
        .width
    }

    pub(super) fn surface_max_width(&self, cx: &App) -> f32 {
        self.files_layout(self.files_visible_width(cx), cx)
            .surface_max
    }

    pub(super) fn right_visible_width(&self, cx: &App) -> f32 {
        if !self.horizontal_fit().right {
            let available =
                (self.viewport_width - self.sidebar_now() - self.files_visible_width(cx)).max(0.0);
            return self
                .fit_exit_width(1)
                .map_or(0.0, |width| width.min(available));
        }
        let available =
            (self.viewport_width - self.sidebar_now() - self.files_visible_width(cx)).max(0.0);
        // Leaving takeover returns the conversation's width over the same
        // tween. Applying its settled minimum here jumps the seam by 300px
        // before that first frame (and breaks mid-flight reversals).
        let max = if self.tween_active(self.main_takeover_tween) {
            available
        } else {
            self.surface_max_width(cx)
        };
        self.right_now(max, cx).min(available)
    }

    pub(super) fn files_content_width(&self, cx: &App) -> f32 {
        self.column_content_width(
            (2, self.horizontal_fit().files),
            self.files_tween,
            self.files_content_tween,
            self.files_target(cx),
        )
    }

    /// Ease Files from what it painted (`from`, its content laid out at
    /// `content`) to `to`, both read before the action changed anything.
    pub(super) fn transition_files(&mut self, from: f32, content: f32, to: f32) {
        let (content_from, content_to) = panel_content_tween(content, from, to);
        let mask = self.panel_tween(from, to);
        self.files_tween = Some(mask);
        self.files_content_tween = Some(WidthTween {
            from: content_from,
            to: content_to,
            ..mask
        });
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
                    FilesEvent::HoldMutation { origin, path } => {
                        let surfaces = this
                            .files
                            .values()
                            .chain(this.file_surfaces.values())
                            .filter(|s| s.read(cx).shares_workspace(origin))
                            .cloned()
                            .collect::<Vec<_>>();
                        for surface in surfaces {
                            surface.update(cx, |files, cx| files.hold_mutation(path.clone(), cx));
                        }
                    }
                    FilesEvent::AddToChat {
                        path,
                        is_directory,
                        origin,
                    } => {
                        let payload = WorkspacePathDrag::new(path.clone(), *is_directory)
                            .with_origin(
                                Some(origin.clone()),
                                crate::files::WorkspacePathSource::Tree,
                                None,
                            );
                        let composer = this.composer.clone();
                        this.attach_workspace_drag(&payload, &composer, window, cx);
                    }
                    FilesEvent::Mutate(intent)
                        if this.accepts_file_navigation(&owner, &source, cx) =>
                    {
                        this.start_file_mutation(source.clone(), intent.clone(), cx);
                    }
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
                    // Footer rows land in the surface host beside the explorer,
                    // through the same paths a spawn chip and a side-chat tab use.
                    FilesEvent::OpenSubagent {
                        doc_id,
                        title,
                        frozen,
                    } => this.add_subagent_surface(
                        this.active_chat.clone(),
                        doc_id.clone(),
                        title.clone(),
                        *frozen,
                        cx,
                    ),
                    FilesEvent::OpenChildChat(chat_id) => this.open_child_chat_tab(chat_id, cx),
                    FilesEvent::RenameChildChat(chat_id) => {
                        this.open_rename_chat(chat_id.clone(), cx)
                    }
                    FilesEvent::ChildChatContextMenu { chat_id, position } => {
                        this.chat_menu.open(ChatMenuState {
                            chat_id: chat_id.clone(),
                            tab: None,
                            position: *position,
                            page: ChatMenuPage::Root,
                        });
                        cx.notify();
                    }
                    FilesEvent::NewChildChat => this.create_child_chat(None, cx),
                    FilesEvent::ForkChat => this.create_side_chat(cx),
                    _ => cx.notify(),
                },
            );
            self.files.insert(key.clone(), files);
            self.files_subs.insert(key.clone(), sub);
        }
        let before = self.painted_panels(cx);
        let from = before.files;
        let was_open = self.files_panel_open(cx);
        let opening = !was_open || !self.horizontal_fit().files;
        self.panels.update(&key, |p| p.files_open = true);
        if opening {
            self.record_panel_open(AuxiliaryPanel::Files, &key);
            self.resplit_panels(before, Some(AuxiliaryPanel::Files), cx);
            self.transition_files(from, before.files_content, self.files_settled_width(cx));
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
        if let Some(files) = self.files.get(&self.panel_key(cx)).cloned() {
            files.update(cx, |files, cx| files.suspend_tree_interactions(cx));
        }
        let before = self.painted_panels(cx);
        self.panels
            .update(&self.panel_key(cx), |p| p.files_open = false);
        self.transition_files(before.files, before.files_content, 0.0);
        self.resplit_panels(before, Some(AuxiliaryPanel::Files), cx);
        cx.notify();
    }

    /// The explorer's own toggle. Opening docks the explorer into the right
    /// pane — opening the pane with just that portion when it was closed.
    pub(super) fn toggle_files_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_panel_open(cx) {
            self.add_files_surface(window, cx);
            return;
        }
        if self.promote_hidden_panel(AuxiliaryPanel::Files, cx) {
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
        self.schedule_save(cx);
        cx.notify();
    }

    pub(super) fn render_files_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.panel_key(cx);
        let visible = matches!(self.route, Route::Chat)
            && self.horizontal_fit().files
            && self.files_panel_open(cx);
        for (key, files) in &self.files {
            if !visible || *key != active {
                files.update(cx, |files, _| files.release_git_status());
            }
        }
        let exiting = self.fit_exit_width(2).is_some();
        if !matches!(self.route, Route::Chat)
            || self.active_chat.is_empty()
            || (!self.horizontal_fit().files && !exiting)
            || (!self.files_panel_open(cx) && !self.tween_active(self.files_tween) && !exiting)
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
        let content_width = self.files_content_width(cx);
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
            .debug_selector(|| "files-panel".into())
            .h_full()
            .flex_none()
            .relative()
            .child(
                div()
                    .h_full()
                    .relative()
                    .w(px(self.files_visible_width(cx)))
                    .overflow_hidden()
                    .child(inner.absolute().top_0().right_0()),
            )
            .when(
                self.files_panel_open(cx) && !self.tween_active(self.files_tween),
                |panel| {
                    panel.child(
                        self.resize_handle(
                            "files-panel-resize",
                            PaneResizeKind::Files,
                            || FilesPanelResize,
                            |shell, cx| shell.reset_panel_widths(PaneResizeKind::Files, cx),
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
        let docked = files_panel_layout(1400.0, 256.0, 286.0, 286.0, 1.0, false);
        assert_eq!(
            docked,
            FilesPanelLayout {
                width: 286.0,
                surface_max: 558.0,
            }
        );
        let narrow = files_panel_layout(1200.0, 256.0, 440.0, 440.0, 1.0, false);
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
        // Closing the surface releases space for the tree. Expanding the
        // surface keeps the tree's existing allocation stable.
        assert_eq!(
            files_panel_layout(1200.0, 256.0, 440.0, 440.0, 0.0, false).width,
            440.0
        );
        let expanded = files_panel_layout(1100.0, 256.0, 286.0, 286.0, 1.0, true);
        assert_eq!(expanded.width, 184.0);
        assert_eq!(expanded.surface_max, 660.0);
        // Growing the viewport restores the preferred width.
        assert_eq!(
            files_panel_layout(1600.0, 256.0, 440.0, 440.0, 1.0, false).width,
            440.0
        );
    }

    #[test]
    fn files_layout_returns_space_smoothly_during_close() {
        let mut previous_chat = 0.0;
        for visible in [186.0, 140.0, 84.0, 40.0, 0.0] {
            let layout = files_panel_layout(1200.0, 256.0, 286.0, visible, 1.0, false);
            let chat = 944.0 - layout.width - layout.surface_max;
            assert!(chat >= previous_chat && chat >= CHAT_PANEL_MIN);
            previous_chat = chat;
        }
        assert_eq!(
            files_panel_layout(1200.0, 256.0, 286.0, 0.0, 1.0, false),
            files_panel_layout(1200.0, 256.0, 0.0, 0.0, 1.0, false),
        );
    }

    #[test]
    fn files_layout_never_takes_space_reserved_for_the_chat_or_surface() {
        for viewport in [0.0, 120.0, 280.0, 600.0, 1000.0, 1200.0, 1600.0] {
            for sidebar in [0.0, 256.0, 400.0] {
                for surfaces in [false, true] {
                    for expanded in [false, true] {
                        for visible in [0.0, 1.0, 140.0, 440.0] {
                            let layout = files_panel_layout(
                                viewport,
                                sidebar,
                                440.0,
                                visible,
                                if surfaces { 1.0 } else { 0.0 },
                                expanded,
                            );
                            let available = (viewport - sidebar).max(0.0);
                            assert!(layout.width >= 0.0 && layout.width <= visible);
                            assert!(layout.surface_max >= 0.0);
                            assert!(layout.width + layout.surface_max <= available + 0.001);
                            if available
                                >= CHAT_PANEL_MIN + if surfaces { RIGHT_PANE_MIN } else { 0.0 }
                            {
                                assert!(
                                    available - layout.width - layout.surface_max >= CHAT_PANEL_MIN
                                        || surfaces && expanded
                                );
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
    fn pane_toggle_drives_surfaces_only_and_last_tab_close_collapses_them(cx: &mut TestAppContext) {
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
