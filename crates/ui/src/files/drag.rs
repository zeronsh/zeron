//! Workspace drag targets are scoped to the tree viewport; the chat keeps its own receiver.
use super::*;
use gpui::{AnyElement, Bounds, CursorStyle, DragMoveEvent};
use std::{cell::RefCell, rc::Rc};
use zeron_proto::WorkspaceEntryKind;

#[derive(Default)]
pub(super) struct TreeDrag {
    rows: Rc<RefCell<HashMap<String, (Bounds<Pixels>, Option<String>)>>>,
    pub payload: Option<WorkspacePathDrag>,
    pub destination: Option<String>,
    pub pointer: Point<Pixels>,
    pub bounds: Bounds<Pixels>,
    hover_since: Option<std::time::Instant>,
    last_tick: Option<std::time::Instant>,
    tick_task: Option<Task<()>>,
}

/// The server is authoritative for existence/permissions/collisions. This
/// resolver handles gesture semantics without touching the filesystem.
pub(super) fn destination_path(
    source: &str,
    directory: &str,
    is_directory: bool,
) -> Option<String> {
    if source.is_empty() || model::parent_path(source).as_deref() == Some(directory) {
        return None;
    }
    if is_directory && mutations::contains_path(source, directory) {
        return None;
    }
    let name = source.rsplit('/').next()?;
    Some(if directory.is_empty() {
        name.into()
    } else {
        format!("{directory}/{name}")
    })
}

impl FilesSurface {
    pub(super) fn track_tree_drop_row(
        &self,
        key: String,
        target: Option<String>,
        height: f32,
        content: AnyElement,
    ) -> AnyElement {
        let rows = self.tree_drag.rows.clone();
        div()
            .relative()
            .w_full()
            .h(px(height))
            .flex_none()
            .child(content)
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        rows.borrow_mut()
                            .insert(key.clone(), (bounds, target.clone()));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    pub(super) fn render_tree_root_target(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = crate::theme::Theme::of(cx);
        let active = self.tree_drag.destination.as_deref() == Some("");
        let label = if self.tree_drag.payload.is_some() {
            "Move to workspace root"
        } else {
            "Workspace root"
        };
        self.track_tree_drop_row(
            "".into(),
            Some("".into()),
            24.,
            div()
                .id("tree-workspace-root")
                .debug_selector(|| "tree-workspace-root".into())
                .h(px(24.))
                .w_full()
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(px(10.5))
                .text_color(theme.text_muted)
                .when(active, |el| el.bg(crate::theme::wash(0.16)))
                .aria_label(label)
                .child(label)
                .into_any_element(),
        )
    }

    pub(super) fn reset_tree_drop_rows(&self) {
        self.tree_drag.rows.borrow_mut().clear();
    }

    pub(super) fn tree_drag_compatible(&self, payload: &WorkspacePathDrag, cx: &gpui::App) -> bool {
        payload.source == WorkspacePathSource::Tree
            && payload.origin.as_ref().is_some_and(|origin| {
                self.accepts_origin(origin, cx) && origin.checkout_id == self.effective_checkout_id
            })
            && !self.mutation_busy()
            && self.tree_rename.is_none()
            && self.tree_delete.is_none()
            && self
                .mutation_capabilities
                .is_some_and(|caps| caps.move_entry)
            && self.tree.node(&payload.path).is_some_and(|node| {
                node.entry.kind != WorkspaceEntryKind::Symlink
                    && node.entry.mutation_revision.is_some()
                    && node.entry.mutation_revision == payload.revision
            })
    }

    fn drop_directory_at(&self, point: Point<Pixels>) -> Option<String> {
        if !self.tree_drag.bounds.contains(&point) {
            return None;
        }
        // The rail remains a scroll control, not a root drop target.
        if point.x > self.tree_drag.bounds.right() - px(12.) {
            return None;
        }
        let rows = self.tree_drag.rows.borrow();
        for (bounds, target) in rows.values() {
            if bounds.contains(&point) {
                let path = target.as_ref()?;
                if path.is_empty() {
                    return Some(String::new());
                }
                let node = self.tree.node(path)?;
                return match node.entry.kind {
                    WorkspaceEntryKind::Directory => Some(path.clone()),
                    WorkspaceEntryKind::File => model::parent_path(path),
                    WorkspaceEntryKind::Symlink => None,
                };
            }
        }
        let viewport = self.tree_list.viewport_bounds();
        let last = rows.values().map(|(bounds, _)| bounds.bottom()).max();
        if viewport.contains(&point) && last.is_none_or(|bottom| point.y >= bottom) {
            Some(String::new())
        } else {
            None
        }
    }

    pub(super) fn on_tree_drag_move(
        &mut self,
        event: &DragMoveEvent<WorkspacePathDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let payload = event.drag(cx).clone();
        self.tree_drag.bounds = event.bounds;
        if !event.bounds.contains(&event.event.position) || !self.tree_drag_compatible(&payload, cx)
        {
            self.clear_tree_drag(window, cx);
            return;
        }
        self.close_tree_context_menu(cx);
        self.tree_drag.pointer = event.event.position;
        self.tree_drag.payload = Some(payload.clone());
        let destination = self
            .drop_directory_at(event.event.position)
            .filter(|directory| {
                destination_path(&payload.path, directory, payload.is_directory).is_some()
            });
        if self.tree_drag.destination != destination {
            self.tree_drag.hover_since = Some(cx.background_executor().now());
            self.tree_drag.destination = destination;
            cx.notify();
        }
        if self.tree_drag.tick_task.is_none() {
            self.tree_drag.last_tick = Some(cx.background_executor().now());
            self.tree_drag.tick_task = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    if !this
                        .update_in(cx, |files, window, cx| files.tree_drag_tick(window, cx))
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            }));
        }
        cx.set_active_drag_cursor_style(
            if self.tree_drag.destination.is_some() {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OperationNotAllowed
            },
            window,
        );
    }

    pub(super) fn on_tree_drop(
        &mut self,
        payload: &WorkspacePathDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = if self.tree_drag_compatible(payload, cx) {
            self.drop_directory_at(window.mouse_position())
                .and_then(|directory| {
                    destination_path(&payload.path, &directory, payload.is_directory)
                })
        } else {
            None
        };
        self.clear_tree_drag(window, cx);
        if let Some(destination) = destination {
            self.request_mutation(&payload.path, Some(destination), cx);
        }
    }

    pub(super) fn tree_drag_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(payload) = self.tree_drag.payload.clone() else {
            return false;
        };
        let pointer = window.mouse_position();
        if !cx.has_active_drag()
            || !window.is_window_active()
            || !self.tree_drag_compatible(&payload, cx)
            || !self.tree_drag.bounds.contains(&pointer)
        {
            self.clear_tree_drag(window, cx);
            return false;
        }
        let now = cx.background_executor().now();
        let elapsed = self
            .tree_drag
            .last_tick
            .replace(now)
            .map(|last| now.saturating_duration_since(last).as_secs_f32())
            .unwrap_or(0.)
            .min(0.05);
        let viewport = self.tree_list.viewport_bounds();
        let speed = if viewport.contains(&pointer) {
            edge_scroll_speed(
                f32::from(pointer.y),
                f32::from(viewport.top()),
                f32::from(viewport.bottom()),
            )
        } else {
            0.
        };
        if speed != 0. {
            let offset = self.tree_list.scroll_px_offset_for_scrollbar();
            self.tree_list
                .set_offset_from_scrollbar(gpui::point(offset.x, offset.y - px(speed * elapsed)));
            cx.notify();
        }
        let destination = self.drop_directory_at(pointer).filter(|directory| {
            destination_path(&payload.path, directory, payload.is_directory).is_some()
        });
        if destination != self.tree_drag.destination {
            self.tree_drag.destination = destination;
            self.tree_drag.hover_since = Some(now);
            cx.notify();
        }
        if self
            .tree_drag
            .hover_since
            .is_some_and(|since| now.saturating_duration_since(since) >= Duration::from_millis(650))
        {
            self.tree_drag.hover_since = None;
            if let Some(directory) = self.tree_drag.destination.clone().filter(|p| !p.is_empty()) {
                if self.tree.expand(&directory) {
                    self.sync_tree_list();
                    let needs_load = self
                        .tree
                        .node(&directory)
                        .is_some_and(|node| node.stale || !node.has_loaded);
                    if needs_load {
                        self.load_directory(directory, None, cx);
                    }
                    cx.notify();
                }
            }
        }
        true
    }

    pub(crate) fn suspend_tree_interactions(&mut self, cx: &mut Context<Self>) {
        if self.tree_drag.payload.is_some()
            || self.tree_context_menu.is_open()
            || self
                .tree_rename
                .as_ref()
                .is_some_and(|rename| !rename.submitted)
            || self.tree_delete.is_some()
        {
            self.interaction_generation = self.interaction_generation.wrapping_add(1);
            self.tree_drag = TreeDrag::default();
            self.close_tree_context_menu(cx);
            if self
                .tree_rename
                .as_ref()
                .is_some_and(|rename| !rename.submitted)
            {
                self.tree_rename = None;
            }
            if let Some(dialog) = self.tree_delete.take() {
                cx.emit(FilesEvent::HoldMutation {
                    origin: dialog.origin,
                    path: None,
                });
            }
            cx.notify();
        }
    }

    pub(super) fn clear_tree_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_drag.tick_task = None;
        self.tree_drag.hover_since = None;
        self.tree_drag.last_tick = None;
        if self.tree_drag.payload.take().is_some() || self.tree_drag.destination.take().is_some() {
            self.tree_drag.destination = None;
            cx.set_active_drag_cursor_style(CursorStyle::Arrow, window);
            cx.notify();
        }
    }
}

fn edge_scroll_speed(y: f32, top: f32, bottom: f32) -> f32 {
    if y < top || y > bottom || bottom <= top {
        return 0.;
    }
    let band = 28.0_f32.min((bottom - top) / 2.0);
    if y < top + band {
        -540.0 * (1.0 - (y - top) / band)
    } else if y > bottom - band {
        540.0 * (1.0 - (bottom - y) / band)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn real_tree_drop_emits_one_move_and_no_chat_reference(cx: &mut gpui::TestAppContext) {
        let (files, cx) = super::super::test_support::setup(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        let start = cx.debug_bounds("tree-entry:a.txt").unwrap().center();
        let end = cx.debug_bounds("tree-entry:folder").unwrap().center();
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            start + gpui::point(px(9.), px(0.)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            end,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();
        let events = events.borrow();
        let moves = events
            .iter()
            .filter_map(|event| {
                if let FilesEvent::Mutate(intent) = event {
                    Some(intent)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].destination.as_deref(), Some("folder/a.txt"));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FilesEvent::AddToChat { .. }))
        );
    }
    #[gpui::test]
    fn hover_expands_only_after_delay_and_escape_stops_tasks(cx: &mut gpui::TestAppContext) {
        let (files, cx) = super::super::test_support::setup(cx);
        let start = cx.debug_bounds("tree-entry:a.txt").unwrap().center();
        let end = cx.debug_bounds("tree-entry:folder").unwrap().center();
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            start + gpui::point(px(9.), px(0.)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            end,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.executor().advance_clock(Duration::from_millis(649));
        files.update_in(cx, |files, window, cx| {
            files.tree_drag_tick(window, cx);
            assert!(!files.tree.is_expanded("folder"));
        });
        cx.executor().advance_clock(Duration::from_millis(1));
        files.update_in(cx, |files, window, cx| {
            files.tree_drag_tick(window, cx);
            assert!(files.tree.is_expanded("folder"));
        });
        cx.simulate_keystrokes("escape");
        files.read_with(cx, |files, cx| {
            assert!(files.tree_drag.tick_task.is_none());
            assert!(files.tree_drag.destination.is_none());
            assert!(!cx.has_active_drag());
        });
    }
    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn windows_row_jitter_preserves_click_and_row_starts_drag(cx: &mut gpui::TestAppContext) {
        let (files, cx) = super::super::test_support::setup(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                recorded.borrow_mut().push(event.clone())
            })
        });
        let row = cx.debug_bounds("tree-entry:a.txt").unwrap().center();
        cx.simulate_mouse_down(row, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            row + gpui::point(px(3.), px(0.)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.update(|_, cx| assert!(!cx.has_active_drag()));
        cx.simulate_mouse_up(
            row + gpui::point(px(3.), px(0.)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event,FilesEvent::OpenFile(path) if path=="a.txt"))
        );
        cx.simulate_mouse_down(row, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            row + gpui::point(px(9.), px(0.)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.update(|_, cx| assert!(cx.has_active_drag()));
        cx.simulate_keystrokes("escape");
    }
    #[test]
    fn autoscroll_is_bounded_and_stops_outside_the_viewport() {
        assert_eq!(edge_scroll_speed(100., 100., 500.), -540.);
        assert_eq!(edge_scroll_speed(500., 100., 500.), 540.);
        assert_eq!(edge_scroll_speed(300., 100., 500.), 0.);
        assert_eq!(edge_scroll_speed(99., 100., 500.), 0.);
        assert!(edge_scroll_speed(486., 100., 500.) > 0.);
    }
    #[test]
    fn tree_drop_resolves_parent_root_and_descendants() {
        assert_eq!(
            destination_path("a/file", "b", false).as_deref(),
            Some("b/file")
        );
        assert_eq!(
            destination_path("a/file", "", false).as_deref(),
            Some("file")
        );
        assert_eq!(destination_path("a/file", "a", false), None);
        assert_eq!(destination_path("a", "a/sub", true), None);
        assert_eq!(destination_path("a", "a", true), None);
        assert_eq!(destination_path("a", "ab", true).as_deref(), Some("ab/a"));
    }
}
