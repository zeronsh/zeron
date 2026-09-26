//! Context actions for the session-owned right surface tabs.

use super::*;
use std::collections::VecDeque;

#[derive(Clone)]
pub(super) struct RightTabMenuState {
    panel_key: String,
    target: RightSurface,
    position: Point<Pixels>,
}

pub(super) struct RightTabCloseBatch {
    panel_key: String,
    pub(super) target: RightSurface,
    selection: RightSurface,
    remaining: VecDeque<RightSurface>,
    pub(super) waiting: Option<RightSurface>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RightTabCloseMode {
    Left,
    Right,
    Others,
}

/// Work from the currently rendered order, never a captured tab index.
fn close_targets(
    order: &[RightSurface],
    target: RightSurface,
    mode: RightTabCloseMode,
) -> Vec<RightSurface> {
    if target == RightSurface::Picker {
        return Vec::new();
    }
    let Some(at) = order.iter().position(|surface| *surface == target) else {
        return Vec::new();
    };
    order
        .iter()
        .enumerate()
        .filter(|(index, surface)| {
            **surface != target
                && **surface != RightSurface::Picker
                && match mode {
                    RightTabCloseMode::Left => *index < at,
                    RightTabCloseMode::Right => *index > at,
                    RightTabCloseMode::Others => true,
                }
        })
        .map(|(_, surface)| *surface)
        .collect()
}

impl Shell {
    fn right_tab_menu_is_valid(&self, menu: &RightTabMenuState, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && self.right_pane_open(cx)
            && menu.panel_key == self.panel_key(cx)
            && self.right_tab_order(cx).contains(&menu.target)
    }

    pub(super) fn open_right_tab_menu(
        &mut self,
        target: RightSurface,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let menu = RightTabMenuState {
            panel_key: self.panel_key(cx),
            target,
            position,
        };
        if !self.right_tab_menu_is_valid(&menu, cx) {
            return;
        }
        self.close_right_plus(cx);
        self.close_chat_menu(cx);
        self.close_user_menu(cx);
        self.close_spaces_menu(cx);
        self.close_space_menu(cx);
        self.right_tab_menu.open(menu);
        cx.notify();
    }

    pub(super) fn close_right_tab_menu(&mut self, cx: &mut Context<Self>) {
        if self.right_tab_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.right_tab_menu);
            cx.notify();
        }
    }

    fn dispatch_right_tab_close(
        &mut self,
        mode: RightTabCloseMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.right_tab_menu.as_open().cloned() else {
            return;
        };
        let valid = self.right_tab_menu_is_valid(&menu, cx);
        self.close_right_tab_menu(cx);
        if valid {
            self.start_right_tab_close(&menu.panel_key, menu.target, mode, window, cx);
        }
    }

    fn dispatch_right_tab_close_this(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.right_tab_menu.as_open().cloned() else {
            return;
        };
        let valid = self.right_tab_menu_is_valid(&menu, cx);
        self.close_right_tab_menu(cx);
        if valid {
            self.close_right_surface(menu.target, window, cx);
        }
    }

    fn right_tab_copy_path(&self, menu: &RightTabMenuState, cx: &App) -> Option<String> {
        if !self.right_tab_menu_is_valid(menu, cx) {
            return None;
        }
        let RightSurface::File(id) = menu.target else {
            return None;
        };
        let path = self.file_surface_paths.get(&id)?;
        self.file_surfaces
            .get(&id)?
            .read(cx)
            .absolute_file_path(path)
    }

    fn copy_right_tab_path(&mut self, cx: &mut Context<Self>) {
        let path = self
            .right_tab_menu
            .as_open()
            .and_then(|menu| self.right_tab_copy_path(menu, cx));
        self.close_right_tab_menu(cx);
        if let Some(path) = path {
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }
    }

    pub(super) fn render_right_tab_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.right_tab_menu.get()?.clone();
        if !self.right_tab_menu_is_valid(&menu, cx) {
            self.close_right_tab_menu(cx);
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let order = self.right_tab_order(cx);
        let mut card = popover::popover_card(&theme)
            .id("right-tab-context-menu")
            .debug_selector(|| "right-tab-context-menu".into())
            .role(gpui::Role::Menu)
            .aria_label("Tab actions")
            .w(px(236.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_right_tab_menu(cx)))
            .flex()
            .flex_col();
        card = card.child(
            popover::menu_row(&theme, false, "right-tab-close-this")
                .id("right-tab-close-this")
                .role(gpui::Role::MenuItem)
                .aria_label("Close Tab")
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.dispatch_right_tab_close_this(window, cx);
                }))
                .child("Close Tab"),
        );
        for (mode, id, label) in [
            (
                RightTabCloseMode::Left,
                "right-tab-close-left",
                "Close Tabs to the Left",
            ),
            (
                RightTabCloseMode::Right,
                "right-tab-close-right",
                "Close Tabs to the Right",
            ),
            (
                RightTabCloseMode::Others,
                "right-tab-close-others",
                "Close Other Tabs",
            ),
        ] {
            let enabled = self.right_tab_menu.is_open()
                && self.can_start_right_tab_close()
                && !close_targets(&order, menu.target, mode).is_empty();
            card = card.child(
                popover::menu_row(&theme, false, id)
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(gpui::Role::MenuItem)
                    .aria_label(label)
                    .when(!enabled, |row| {
                        row.opacity(0.38)
                            .cursor_default()
                            .aria_description("Unavailable")
                    })
                    .when(enabled, |row| {
                        row.on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.dispatch_right_tab_close(mode, window, cx);
                        }))
                    })
                    .child(label),
            );
        }
        if matches!(menu.target, RightSurface::File(_)) {
            let enabled =
                self.right_tab_menu.is_open() && self.right_tab_copy_path(&menu, cx).is_some();
            let id = "right-tab-copy-path";
            card = card.child(popover::menu_separator()).child(
                popover::menu_row(&theme, false, id)
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(gpui::Role::MenuItem)
                    .aria_label("Copy Path")
                    .when(!enabled, |row| {
                        row.opacity(0.38)
                            .cursor_default()
                            .aria_description("Unavailable")
                    })
                    .when(enabled, |row| {
                        row.on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.copy_right_tab_path(cx);
                        }))
                    })
                    .child("Copy Path"),
            );
        }
        Some(popover::menu_at(
            "right-tab-context-menu-overlay",
            menu.position,
            card.into_any_element(),
            self.right_tab_menu.closing_since(),
        ))
    }

    fn right_tab_order(&self, cx: &App) -> Vec<RightSurface> {
        self.right_surface_rows(cx)
            .into_iter()
            .map(|row| row.0)
            .collect()
    }

    pub(super) fn can_start_right_tab_close(&self) -> bool {
        self.right_tab_close_batch.is_none()
            && self.pending_file_closes.is_empty()
            && self.pending_exit.is_none()
    }

    pub(super) fn start_right_tab_close(
        &mut self,
        panel_key: &str,
        target: RightSurface,
        mode: RightTabCloseMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if panel_key != self.panel_key(cx) || !self.can_start_right_tab_close() {
            return;
        }
        let targets = close_targets(&self.right_tab_order(cx), target, mode);
        if targets.is_empty() {
            return;
        }
        let active = self.resolved_right_active(cx);
        let selection = if mode == RightTabCloseMode::Others || targets.contains(&active) {
            target
        } else {
            active
        };
        self.right_tab_close_batch = Some(RightTabCloseBatch {
            panel_key: panel_key.to_owned(),
            target,
            selection,
            remaining: targets.into(),
            waiting: None,
        });
        self.advance_right_tab_close(window, cx);
    }

    fn advance_right_tab_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        loop {
            let Some(batch) = self.right_tab_close_batch.as_ref() else {
                return;
            };
            let order = self.right_tab_order(cx);
            if batch.panel_key != self.panel_key(cx) || !order.contains(&batch.target) {
                self.right_tab_close_batch = None;
                return;
            }
            if batch.waiting.is_some() {
                return;
            }
            let batch = self.right_tab_close_batch.as_mut().unwrap();
            let Some(surface) = batch.remaining.pop_front() else {
                let selection = if order.contains(&batch.selection) {
                    batch.selection
                } else {
                    batch.target
                };
                self.right_tab_close_batch = None;
                self.set_right_active(selection, cx);
                self.focus_right_file_editor(selection, window, cx);
                return;
            };
            if !order.contains(&surface) {
                continue;
            }
            // The individual close owns saving, PTYs, browser teardown and watches.
            self.close_right_surface(surface, window, cx);
            if self.pending_file_closes.contains(&surface) {
                if let Some(batch) = self.right_tab_close_batch.as_mut() {
                    batch.waiting = Some(surface);
                }
                return;
            }
        }
    }

    pub(super) fn resume_right_tab_close(
        &mut self,
        surface: RightSurface,
        panel_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(batch) = self.right_tab_close_batch.as_mut()
            && batch.panel_key == panel_key
            && batch.waiting == Some(surface)
        {
            batch.waiting = None;
            self.advance_right_tab_close(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RightSurface::{Browser, Diff, File, Picker, Subagent, Terminal};
    use RightTabCloseMode::{Left, Others, Right};
    use gpui::{AppContext, TestAppContext};

    fn setup(cx: &mut TestAppContext) -> (tempfile::TempDir, gpui::WindowHandle<Shell>) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.selected_chat = Some("owner".into());
                state
            });
            let mut shell = Shell::new(
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
            );
            shell.active_chat = "owner".into();
            shell
        });
        (dir, window)
    }

    fn add_subagent(shell: &mut Shell, name: &str, cx: &mut Context<Shell>) -> RightSurface {
        shell.add_subagent_surface("owner".into(), name.into(), name.into(), false, cx);
        Subagent(shell.subagent_seq)
    }

    #[gpui::test]
    fn bulk_close_releases_mixed_surfaces_and_selects_the_inactive_target(cx: &mut TestAppContext) {
        let (_dir, handle) = setup(cx);
        let weak = handle
            .update(cx, |shell, window, cx| {
                shell.add_file_surface("clean.rs".into(), window, cx);
                let file = shell.file_surfaces[&shell.file_surface_seq].downgrade();
                let target = add_subagent(shell, "keep", cx);
                let removed = add_subagent(shell, "remove", cx);
                shell.add_diff_surface(window, cx);
                shell.add_browser_surface(None, window, cx);
                // A real tab entity without an engine/PTY process in this fixture.
                let terminal = shell.right_terminal_panel(cx);
                let tab = terminal.update(cx, |panel, cx| {
                    panel.reserve_tab_for_chat("owner".into(), "Terminal", cx)
                });
                shell
                    .right_tabs
                    .entry("owner".into())
                    .or_default()
                    .push(Terminal(tab));
                assert!(
                    shell
                        .right_tab_order(cx)
                        .iter()
                        .any(|s| matches!(s, Terminal(_)))
                );
                shell.start_right_tab_close("owner", target, Others, window, cx);
                assert_eq!(shell.right_tab_order(cx), [target]);
                assert_eq!(shell.resolved_right_active(cx), target);
                assert!(shell.right_tab_close_batch.is_none());
                assert!(shell.file_surfaces.is_empty());
                assert!(shell.file_surface_keys.is_empty());
                assert!(shell.file_surface_subs.is_empty());
                assert!(shell.diffs.is_empty());
                assert!(shell.diff_subs.is_empty());
                assert!(shell.browsers.is_empty());
                assert!(shell.browser_subs.is_empty());
                assert!(!shell.right_tab_order(cx).contains(&removed));
                assert!(
                    shell
                        .right_terminal
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .tab_summaries(cx)
                        .is_empty()
                );
                file
            })
            .unwrap();
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
    }

    #[gpui::test]
    fn directional_closes_preserve_surviving_selection_or_select_target(cx: &mut TestAppContext) {
        let (_dir, handle) = setup(cx);
        handle
            .update(cx, |shell, window, cx| {
                let a = add_subagent(shell, "a", cx);
                let b = add_subagent(shell, "b", cx);
                let c = add_subagent(shell, "c", cx);
                shell.start_right_tab_close("other-session", b, Others, window, cx);
                assert_eq!(shell.right_tab_order(cx), [a, b, c]);
                shell.start_right_tab_close("owner", b, Left, window, cx);
                assert_eq!(shell.right_tab_order(cx), [b, c]);
                assert_eq!(shell.resolved_right_active(cx), c);
                shell.start_right_tab_close("owner", b, Right, window, cx);
                assert_eq!(shell.right_tab_order(cx), [b]);
                assert_eq!(shell.resolved_right_active(cx), b);
            })
            .unwrap();
    }

    #[gpui::test]
    fn pending_and_failed_files_pause_batches_until_discard_or_keep_open(cx: &mut TestAppContext) {
        let (_dir, handle) = setup(cx);
        for (failed, keep_open) in [(false, false), (true, false), (true, true)] {
            let (file, target, tail) = handle
                .update(cx, |shell, window, cx| {
                    // Clear the previous scenario using the ordinary close path.
                    for surface in shell.right_tab_order(cx) {
                        shell.close_right_surface(surface, window, cx);
                    }
                    shell.add_file_surface("test.rs".into(), window, cx);
                    let file = shell.file_surfaces[&shell.file_surface_seq].clone();
                    file.update(cx, |file, _| file.seed_pending_exit_test_document(failed));
                    let target = add_subagent(shell, "keep", cx);
                    let tail = add_subagent(shell, "tail", cx);
                    shell.start_right_tab_close("owner", target, Others, window, cx);
                    assert_eq!(shell.right_tab_order(cx).len(), 3);
                    let waiting = shell.right_tab_close_batch.as_ref().unwrap().waiting;
                    assert_eq!(waiting, Some(File(shell.file_surface_seq)));
                    assert_eq!(shell.resolved_right_active(cx), waiting.unwrap());
                    shell.start_right_tab_close("owner", tail, Left, window, cx);
                    assert_eq!(shell.right_tab_close_batch.as_ref().unwrap().target, target);
                    (file, target, tail)
                })
                .unwrap();
            file.update(cx, |file, cx| file.resolve_test_file_close(keep_open, cx));
            cx.run_until_parked();
            handle
                .update(cx, |shell, _, cx| {
                    assert!(shell.right_tab_close_batch.is_none());
                    assert!(shell.pending_file_closes.is_empty());
                    if keep_open {
                        assert_eq!(shell.right_tab_order(cx).len(), 3);
                        assert!(shell.right_tab_order(cx).contains(&tail));
                        assert!(matches!(shell.resolved_right_active(cx), File(_)));
                    } else {
                        assert_eq!(shell.right_tab_order(cx), [target]);
                        assert_eq!(shell.resolved_right_active(cx), target);
                    }
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn successful_save_resumes_snapshot_and_target_close_cancels_it(cx: &mut TestAppContext) {
        let (_dir, handle) = setup(cx);
        for (failed, close_target) in [(false, false), (true, false), (false, true)] {
            let (file, target, added) = handle
                .update(cx, |shell, window, cx| {
                    for surface in shell.right_tab_order(cx) {
                        shell.close_right_surface(surface, window, cx);
                    }
                    shell.add_file_surface("test.rs".into(), window, cx);
                    let file = shell.file_surfaces[&shell.file_surface_seq].clone();
                    file.update(cx, |file, _| file.seed_pending_exit_test_document(failed));
                    let target = add_subagent(shell, "keep", cx);
                    let tail = add_subagent(shell, "tail", cx);
                    shell.start_right_tab_close("owner", target, Others, window, cx);
                    assert!(
                        shell
                            .right_tab_close_batch
                            .as_ref()
                            .unwrap()
                            .waiting
                            .is_some()
                    );
                    let added = add_subagent(shell, "opened-after-snapshot", cx);
                    // A queued candidate can disappear independently while saving.
                    shell.close_right_surface(tail, window, cx);
                    if close_target {
                        shell.close_right_surface(target, window, cx);
                        assert!(shell.right_tab_close_batch.is_none());
                    }
                    (file, target, added)
                })
                .unwrap();
            file.update(cx, |file, cx| file.finish_test_file_save(cx));
            cx.run_until_parked();
            handle
                .update(cx, |shell, _, cx| {
                    assert!(shell.right_tab_close_batch.is_none());
                    assert!(shell.pending_file_closes.is_empty());
                    if close_target {
                        assert_eq!(shell.right_tab_order(cx), [added]);
                        assert_eq!(shell.resolved_right_active(cx), added);
                    } else {
                        assert_eq!(shell.right_tab_order(cx), [target, added]);
                        assert_eq!(shell.resolved_right_active(cx), target);
                    }
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn pending_close_cannot_continue_in_another_session(cx: &mut TestAppContext) {
        let (_dir, handle) = setup(cx);
        let (file, target, tail) = handle
            .update(cx, |shell, window, cx| {
                shell.add_file_surface("test.rs".into(), window, cx);
                let file = shell.file_surfaces[&shell.file_surface_seq].clone();
                file.update(cx, |file, _| file.seed_pending_exit_test_document(false));
                let target = add_subagent(shell, "keep", cx);
                let tail = add_subagent(shell, "tail", cx);
                shell.start_right_tab_close("owner", target, Others, window, cx);
                shell.state.update(cx, |state, cx| {
                    state.select_chat(Some("new-session".into()), cx)
                });
                (file, target, tail)
            })
            .unwrap();
        cx.run_until_parked();
        let other = handle
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.active_chat, "new-session");
                assert!(shell.right_tab_close_batch.is_none());
                add_subagent(shell, "other", cx)
            })
            .unwrap();
        file.update(cx, |file, cx| file.resolve_test_file_close(false, cx));
        cx.run_until_parked();
        handle
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.right_tab_order(cx), [other]);
                assert_eq!(shell.resolved_right_active(cx), other);
                assert_eq!(shell.right_tabs["owner"], [target, tail]);
                assert!(shell.pending_file_closes.is_empty());
            })
            .unwrap();
    }

    fn seed_remote_chat(shell: &mut Shell, cx: &mut Context<Shell>) {
        shell.state.update(cx, |state, _| {
            state.local_device_id = Some("viewer-host".into());
            state.chats.push(
                serde_json::from_value(serde_json::json!({
                    "id": "owner", "deviceId": "remote-host", "archived": false,
                    "cwd": "/only/on/remote", "createdAt": "2026-09-23T00:00:00Z"
                }))
                .unwrap(),
            );
        });
    }

    #[gpui::test]
    fn copy_path_requires_context_and_keeps_the_dirty_editors_original_host(
        cx: &mut TestAppContext,
    ) {
        let (_dir, handle) = setup(cx);
        handle
            .update(cx, |shell, window, cx| {
                shell.add_file_surface("test.rs".into(), window, cx);
                let target = File(shell.file_surface_seq);
                let position = gpui::point(px(20.), px(20.));
                cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
                shell.open_right_tab_menu(target, position, cx);
                shell.copy_right_tab_path(cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("unchanged")
                );

                seed_remote_chat(shell, cx);
                let file = shell.file_surfaces[&shell.file_surface_seq].clone();
                file.update(cx, |file, cx| {
                    file.ensure_loaded(cx);
                    file.seed_pending_exit_test_document(true);
                });
                shell.state.update(cx, |state, _| {
                    state.chats[0].cwd = Some("/different/checkout".into());
                    state.chats[0].device_id = "different-host".into();
                });
                file.update(cx, |file, cx| file.ensure_loaded(cx));
                shell.open_right_tab_menu(target, position, cx);
                shell.copy_right_tab_path(cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("/only/on/remote/test.rs")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn copy_path_reads_current_workspace_path_and_rejects_stale_contexts(cx: &mut TestAppContext) {
        let (_dir, handle) = setup(cx);
        handle
            .update(cx, |shell, window, cx| {
                seed_remote_chat(shell, cx);
                shell.add_file_surface("src/original.rs".into(), window, cx);
                let id = shell.file_surface_seq;
                let target = File(id);
                let active = add_subagent(shell, "active", cx);
                let position = gpui::point(px(20.), px(20.));
                shell.open_right_tab_menu(target, position, cx);
                shell.rename_file_surface(id, "owner", "src/original.rs", "src/área nueva.rs", cx);
                shell.copy_right_tab_path(cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("/only/on/remote/src/área nueva.rs")
                );
                assert_eq!(shell.resolved_right_active(cx), active);
                assert!(!shell.right_tab_menu.is_open());

                cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
                shell.open_right_tab_menu(target, position, cx);
                shell.file_surface_paths.remove(&id);
                assert!(
                    shell
                        .right_tab_copy_path(shell.right_tab_menu.as_open().unwrap(), cx)
                        .is_none()
                );
                shell.copy_right_tab_path(cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("unchanged")
                );

                shell.file_surface_paths.insert(id, "src/remote.rs".into());
                shell.open_right_tab_menu(target, position, cx);
                shell.active_chat = "different-session".into();
                shell.copy_right_tab_path(cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("unchanged")
                );
                shell.active_chat = "owner".into();
                shell.open_right_tab_menu(target, position, cx);
                shell.close_right_surface(target, window, cx);
                shell.copy_right_tab_path(cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some("unchanged")
                );
                for target in [Picker, Diff(1), Terminal(1), Browser(1), active] {
                    let menu = RightTabMenuState {
                        panel_key: "owner".into(),
                        target,
                        position,
                    };
                    assert!(shell.right_tab_copy_path(&menu, cx).is_none());
                }
            })
            .unwrap();
    }

    #[test]
    fn targets_follow_visible_order_for_every_surface_kind() {
        let order = [File(1), Terminal(2), Diff(3), Browser(4), Subagent(5)];
        for (at, target) in order.iter().enumerate() {
            assert_eq!(close_targets(&order, *target, Left), order[..at]);
            assert_eq!(close_targets(&order, *target, Right), order[at + 1..]);
            assert_eq!(
                close_targets(&order, *target, Others),
                order
                    .iter()
                    .copied()
                    .filter(|s| s != target)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn empty_single_missing_and_picker_targets_cannot_close_neighbors() {
        for mode in [Left, Right, Others] {
            assert!(close_targets(&[], File(1), mode).is_empty());
            assert!(close_targets(&[File(1)], File(1), mode).is_empty());
            assert!(close_targets(&[File(1)], File(2), mode).is_empty());
            assert!(close_targets(&[Picker, File(1)], Picker, mode).is_empty());
            assert!(close_targets(&[Picker, File(1), Picker], File(1), mode).is_empty());
        }
    }

    #[test]
    fn reordering_changes_neighbors_without_changing_target_identity() {
        let before = [File(1), File(2), File(3)];
        let after = [File(3), File(1), File(2)];
        assert_eq!(close_targets(&before, File(2), Left), [File(1)]);
        assert_eq!(close_targets(&after, File(2), Left), [File(3), File(1)]);
        assert!(close_targets(&after, File(2), Right).is_empty());
    }
}
