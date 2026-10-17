//! Split surface hosts: each pane's tabs and shown surface, and keeping a
//! chat's split in step with its tab list.
//!
//! A host splits only when the user drags a tab onto a pane's edge. While
//! split, the chat's flat tab list stays the membership record every open
//! and close path already maintains; [`right_layout::SplitLayout`] files
//! those tabs into panes. `ChatPanels::right_active` is the focused pane's
//! tab, so everything that reads "the active surface" acts on that pane.

use super::right_layout::{
    Axis, DropZone, Edge, MIN_PANE_HEIGHT, MIN_PANE_WIDTH, Node, PaneId, Rect, SplitLayout,
};
use super::*;

/// A split pane's tabs start this far past its left edge, like the unsplit
/// host's strip.
const STRIP_GUTTER: f32 = 8.0;
/// Room a titlebar strip leaves before the next column's divider.
const STRIP_TRAILING_GAP: f32 = 4.0;
/// The hit width of a divider between split panes; the line itself is the
/// 1px border every pane after the first draws.
pub(super) const DIVIDER_HITBOX: f32 = 8.0;

/// Where a dragged tab would land: a zone of a split pane, or of the
/// unsplit host (`pane: None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RightPaneDrop {
    pub pane: Option<PaneId>,
    pub zone: DropZone,
    /// The dragged tab.
    pub surface: RightSurface,
}

/// Drag marker for a divider: the boundary before child `index` of split
/// `split`.
pub(super) struct RightSplitResize {
    panel_key: String,
    split: u64,
    index: usize,
}

impl Shell {
    /// The current chat's split, while its host holds several panes.
    pub(super) fn right_split(&self, cx: &App) -> Option<&SplitLayout<RightSurface>> {
        self.panels.right_layout(&self.panel_key(cx))
    }

    /// One pane's live rows in strip order; `None` is the unsplit host.
    pub(super) fn right_pane_rows(&self, pane: Option<PaneId>, cx: &App) -> Vec<RightRow> {
        match (pane, self.right_split(cx)) {
            (Some(pane), Some(layout)) => layout
                .pane(pane)
                .map(|pane| self.right_rows_for(&pane.tabs, cx))
                .unwrap_or_default(),
            _ => self.right_surface_rows(cx),
        }
    }

    /// The surface a pane shows: its pick while that is live, else its most
    /// recently visited live tab, else its first.
    pub(super) fn resolved_pane_active(&self, pane: PaneId, cx: &App) -> RightSurface {
        let key = self.panel_key(cx);
        let rows = self.right_pane_rows(Some(pane), cx);
        let picked = self
            .right_split(cx)
            .and_then(|layout| layout.pane(pane))
            .and_then(|pane| pane.active);
        match picked {
            Some(picked) if rows.iter().any(|(surface, ..)| *surface == picked) => picked,
            _ => self
                .panels
                .right_surface_fallback(&key, rows.iter().map(|(surface, ..)| *surface)),
        }
    }

    /// The surfaces on screen in the host: each pane's shown tab, and
    /// whether that pane meets the host's left edge.
    pub(super) fn right_shown_surfaces(&self, cx: &App) -> Vec<(RightSurface, bool)> {
        match self.right_split(cx) {
            Some(layout) => layout
                .rects()
                .into_iter()
                .map(|(pane, rect)| {
                    (
                        self.resolved_pane_active(pane, cx),
                        right_layout::touches_left(&rect),
                    )
                })
                .collect(),
            None => vec![(self.resolved_right_active(cx), true)],
        }
    }

    /// The pane holding `surface` (the unsplit host is `None`).
    pub(super) fn right_pane_of(&self, surface: RightSurface, cx: &App) -> Option<PaneId> {
        self.right_split(cx)
            .and_then(|layout| layout.pane_of(surface))
    }

    /// Reconcile `key`'s split with its tab list: closed and dead tabs leave
    /// their panes, an emptied pane closes, and a split left with one pane
    /// dissolves back into the plain tab list. The chat's shown surface
    /// stays one the focused pane holds.
    pub(super) fn sync_right_layout(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.panels.right_layout(key).is_none() {
            return;
        }
        let members: Vec<RightSurface> = self.right_tabs.get(key).cloned().unwrap_or_default();
        // Entities can only be checked for the chat on screen; another
        // chat's tabs are reconciled when it shows again.
        let live: Option<Vec<RightSurface>> = (key == self.panel_key(cx)).then(|| {
            self.right_rows_for(&members, cx)
                .into_iter()
                .map(|(surface, ..)| surface)
                .collect()
        });
        let keep = |tab: &RightSurface| {
            members.contains(tab) && live.as_ref().is_none_or(|live| live.contains(tab))
        };
        let Some(layout) = self.panels.right_layouts.get_mut(key) else {
            return;
        };
        let mut changed = layout.retain(&keep);
        for tab in members.iter().filter(|tab| keep(tab)) {
            changed |= layout.adopt(*tab);
        }
        if let Some(sole) = layout.sole_pane() {
            // Back to one pane: its strip order becomes the chat's tab order.
            let mut tabs = sole.tabs.clone();
            let active = sole.active;
            let unfiled: Vec<RightSurface> = members
                .iter()
                .filter(|tab| !tabs.contains(tab))
                .copied()
                .collect();
            tabs.extend(unfiled);
            self.panels.right_layouts.remove(key);
            self.right_tabs.insert(key.to_string(), tabs);
            if let Some(active) = active {
                self.panels.update(key, |panel| panel.right_active = active);
            }
            cx.notify();
            return;
        }
        let focused = layout.focused();
        let shown = layout.pane(focused).and_then(|pane| pane.active);
        let current = self.panels.get(key).right_active;
        let target = match shown {
            Some(shown) => shown,
            None if key == self.panel_key(cx) => self.resolved_pane_active(focused, cx),
            None => return,
        };
        if current != target {
            self.panels.update(key, |panel| panel.right_active = target);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Focus a pane of the current chat's split (a press anywhere in it).
    pub(super) fn focus_right_pane(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(layout) = self.right_split(cx) else {
            return;
        };
        if layout.focused() == pane || layout.pane(pane).is_none() {
            return;
        }
        let surface = self.resolved_pane_active(pane, cx);
        if surface != RightSurface::Picker {
            self.set_right_active(surface, cx);
        }
    }

    /// Suspend image previews only in the file a change of tab hides: in a
    /// split, the other panes keep showing theirs.
    pub(super) fn suspend_replaced_file_images(
        &mut self,
        surface: RightSurface,
        cx: &mut Context<Self>,
    ) {
        let Some(layout) = self.right_split(cx) else {
            if self.resolved_right_active(cx) != surface {
                self.suspend_file_images(cx);
            }
            return;
        };
        let pane = layout.pane_of(surface).unwrap_or(layout.focused());
        let replaced = self.resolved_pane_active(pane, cx);
        if replaced != surface
            && let RightSurface::File(id) = replaced
            && let Some(file) = self.file_surfaces.get(&id).cloned()
        {
            file.update(cx, |file, cx| file.suspend_images(cx));
        }
    }

    /// Move `surface` into a new pane on `edge` of `target` (the unsplit
    /// host is `None`). The first split turns the host's tab list into a
    /// two-pane layout.
    pub(super) fn split_right_tab(
        &mut self,
        surface: RightSurface,
        target: Option<PaneId>,
        edge: Edge,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = self.panel_key(cx);
        let split = match (target, self.panels.right_layouts.get_mut(&key)) {
            (Some(target), Some(layout)) => layout.split(surface, target, edge),
            (None, None) => {
                let tabs: Vec<RightSurface> = self
                    .right_surface_rows(cx)
                    .into_iter()
                    .map(|(tab, ..)| tab)
                    .collect();
                let active = Some(self.resolved_right_active(cx));
                match SplitLayout::split_off(tabs, active, surface, edge) {
                    Some(layout) => {
                        self.panels.right_layouts.insert(key.clone(), layout);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        };
        if split {
            self.panels
                .update(&key, |panel| panel.right_active = surface);
            self.sync_right_layout(&key, cx);
            cx.notify();
        }
        split
    }

    /// Close every tab of the pane holding `surface`; the emptied pane
    /// closes with them. Unsaved files ask first, as from their own ✕.
    pub(super) fn close_right_pane(
        &mut self,
        surface: RightSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.right_pane_of(surface, cx) else {
            return;
        };
        let tabs: Vec<RightSurface> = self
            .right_pane_rows(Some(pane), cx)
            .into_iter()
            .map(|(tab, ..)| tab)
            .collect();
        for tab in tabs {
            self.close_right_surface(tab, window, cx);
        }
    }

    /// Move `surface` into pane `target` at `index` (its end when `None`).
    pub(super) fn move_right_tab(
        &mut self,
        surface: RightSurface,
        target: PaneId,
        index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = self.panel_key(cx);
        let moved = self
            .panels
            .right_layouts
            .get_mut(&key)
            .is_some_and(|layout| layout.move_to(surface, target, index));
        if moved {
            self.panels
                .update(&key, |panel| panel.right_active = surface);
            self.sync_right_layout(&key, cx);
            cx.notify();
        }
        moved
    }
}

impl Shell {
    /// The split host: the layout tree, each pane with its surface.
    pub(super) fn render_right_split(
        &mut self,
        layout: &SplitLayout<RightSurface>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rects = layout.rects();
        self.render_right_node(layout.root(), &rects, width, cx)
    }

    fn render_right_node(
        &mut self,
        node: &Node,
        rects: &[(PaneId, Rect)],
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (split, axis, children, sizes) = match node {
            Node::Pane(id) => {
                let rect = rects
                    .iter()
                    .find(|(pane, _)| pane == id)
                    .map(|(_, rect)| *rect)
                    .unwrap_or(Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 1.0,
                        h: 1.0,
                    });
                return self.render_right_split_pane(*id, rect, width, cx);
            }
            Node::Split {
                id,
                axis,
                children,
                sizes,
            } => (*id, *axis, children, sizes),
        };
        let theme = Theme::of(cx).clone();
        let row = axis == Axis::Row;
        let key = self.panel_key(cx);
        let mut container = div()
            .id(("right-split", split as usize))
            .size_full()
            .relative()
            .flex()
            .when(row, |el| el.flex_row())
            .when(!row, |el| el.flex_col())
            .on_drag_move::<RightSplitResize>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<RightSplitResize>, _, cx| {
                    let drag = event.drag(cx);
                    if drag.split != split || drag.panel_key != this.panel_key(cx) {
                        return;
                    }
                    let (pointer, start, extent, min) = if row {
                        (
                            event.event.position.x,
                            event.bounds.left(),
                            event.bounds.size.width,
                            MIN_PANE_WIDTH,
                        )
                    } else {
                        (
                            event.event.position.y,
                            event.bounds.top(),
                            event.bounds.size.height,
                            MIN_PANE_HEIGHT,
                        )
                    };
                    let extent = f32::from(extent).max(1.0);
                    let fraction = f32::from(pointer - start) / extent;
                    let index = drag.index;
                    let key = this.panel_key(cx);
                    if let Some(layout) = this.panels.right_layouts.get_mut(&key)
                        && layout.resize(split, index, fraction, min / extent)
                    {
                        cx.notify();
                    }
                },
            ));
        for (ix, (child, size)) in children.iter().zip(sizes).enumerate() {
            let inner = self.render_right_node(child, rects, width, cx);
            container = container.child(
                div()
                    .relative()
                    .overflow_hidden()
                    .flex_grow_0()
                    .flex_shrink_0()
                    .flex_basis(gpui::relative(*size))
                    .min_w_0()
                    .min_h_0()
                    .when(row, |cell| cell.h_full())
                    .when(!row, |cell| cell.w_full())
                    // The divider: the same 1px hairline as the host's seam.
                    .when(ix > 0 && row, |cell| {
                        cell.border_l_1().border_color(theme.border)
                    })
                    .when(ix > 0 && !row, |cell| {
                        cell.border_t_1().border_color(theme.border)
                    })
                    .child(inner),
            );
        }
        let mut offset = 0.0;
        for ix in 1..children.len() {
            offset += sizes[ix - 1];
            container = container.child(self.right_split_divider(
                key.clone(),
                split,
                ix,
                row,
                offset,
                &theme,
                cx,
            ));
        }
        container.into_any_element()
    }

    /// The invisible grab area over a divider; hovering strengthens the
    /// hairline, a double click evens the split out.
    #[allow(clippy::too_many_arguments)]
    fn right_split_divider(
        &self,
        panel_key: String,
        split: u64,
        index: usize,
        row: bool,
        offset: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let group: SharedString = format!("right-split-divider-{split}-{index}").into();
        let line = theme.border_strong;
        div()
            .id(("right-split-divider", (split as usize) << 8 | index))
            .group(group.clone())
            .absolute()
            .when(row, |el| {
                el.top_0()
                    .bottom_0()
                    .w(px(DIVIDER_HITBOX))
                    .left(gpui::relative(offset))
                    .ml(px(-DIVIDER_HITBOX / 2.0))
                    .cursor_col_resize()
            })
            .when(!row, |el| {
                el.left_0()
                    .right_0()
                    .h(px(DIVIDER_HITBOX))
                    .top(gpui::relative(offset))
                    .mt(px(-DIVIDER_HITBOX / 2.0))
                    .cursor_row_resize()
            })
            .occlude()
            .child(
                div()
                    .absolute()
                    .when(row, |el| {
                        el.top_0()
                            .bottom_0()
                            .left(px(DIVIDER_HITBOX / 2.0))
                            .w(px(1.0))
                    })
                    .when(!row, |el| {
                        el.left_0()
                            .right_0()
                            .top(px(DIVIDER_HITBOX / 2.0))
                            .h(px(1.0))
                    })
                    .group_hover(group, move |style| style.bg(line)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if event.click_count == 2 {
                        let key = this.panel_key(cx);
                        if let Some(layout) = this.panels.right_layouts.get_mut(&key)
                            && layout.balance(split)
                        {
                            cx.notify();
                        }
                    }
                }),
            )
            .on_drag(
                RightSplitResize {
                    panel_key,
                    split,
                    index,
                },
                |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| DragGhost)
                },
            )
    }

    /// Wrap a pane's content so tabs dragged over it can land: its edges
    /// split it, its middle takes the tab. Nothing shows until a drag
    /// reaches a zone that would change something.
    pub(super) fn right_pane_drop_target(
        &mut self,
        pane: Option<PaneId>,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let shown = self.right_pane_drop.filter(|drop| drop.pane == pane);
        let id: gpui::ElementId = match pane {
            Some(pane) => ("right-pane-drop", pane as usize).into(),
            None => "right-pane-drop".into(),
        };
        div()
            .id(id)
            .relative()
            .size_full()
            .on_drag_move::<RightTabDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<RightTabDrag>, _, cx| {
                    let payload = event.drag(cx);
                    if payload.panel_key != this.panel_key(cx) {
                        return;
                    }
                    let position = event.event.position;
                    let next = if event.bounds.contains(&position) {
                        let local = position - event.bounds.origin;
                        let zone = this.right_drop_zone(
                            payload,
                            pane,
                            f32::from(local.x),
                            f32::from(local.y),
                            f32::from(event.bounds.size.width),
                            f32::from(event.bounds.size.height),
                            cx,
                        );
                        zone.map(|zone| RightPaneDrop {
                            pane,
                            zone,
                            surface: payload.surface,
                        })
                    } else if this.right_pane_drop.is_some_and(|drop| drop.pane == pane) {
                        None
                    } else {
                        return;
                    };
                    if this.right_pane_drop != next {
                        this.right_pane_drop = next;
                        cx.notify();
                    }
                },
            ))
            .child(content)
            .when_some(shown, |el, drop| {
                // A quiet accent wash over the half the new pane would take
                // (all of it for a move).
                let preview = div()
                    .absolute()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.accent.opacity(0.35))
                    .bg(theme.accent.opacity(0.08));
                let preview = match drop.zone {
                    DropZone::Center => preview.inset(px(4.0)),
                    DropZone::Edge(Edge::Left) => preview
                        .top(px(4.0))
                        .bottom(px(4.0))
                        .left(px(4.0))
                        .w(gpui::relative(0.5)),
                    DropZone::Edge(Edge::Right) => preview
                        .top(px(4.0))
                        .bottom(px(4.0))
                        .right(px(4.0))
                        .w(gpui::relative(0.5)),
                    DropZone::Edge(Edge::Top) => preview
                        .left(px(4.0))
                        .right(px(4.0))
                        .top(px(4.0))
                        .h(gpui::relative(0.5)),
                    DropZone::Edge(Edge::Bottom) => preview
                        .left(px(4.0))
                        .right(px(4.0))
                        .bottom(px(4.0))
                        .h(gpui::relative(0.5)),
                };
                el.child(preview)
            })
            .into_any_element()
    }

    /// Land a dragged tab on the pane zone under it. Pane content (a live
    /// page, a terminal grid) claims its own mouse-ups, so the drop is taken
    /// in the capture phase, ahead of everything, while a zone is armed.
    pub(super) fn right_pane_drop_listener(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.right_pane_drop?;
        let shell = cx.entity().downgrade();
        Some(
            gpui::canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let shell = shell.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase != gpui::DispatchPhase::Capture || !cx.has_active_drag() {
                            return;
                        }
                        let _ = shell.update(cx, |this, cx| {
                            if let Some(drop) = this.right_pane_drop.take() {
                                this.right_tab_drag = None;
                                this.commit_right_pane_drop(drop, cx);
                                cx.stop_propagation();
                            }
                        });
                    });
                },
            )
            .absolute()
            .size_0()
            .into_any_element(),
        )
    }

    pub(super) fn commit_right_pane_drop(&mut self, drop: RightPaneDrop, cx: &mut Context<Self>) {
        match (drop.zone, drop.pane) {
            (DropZone::Edge(edge), pane) => {
                self.split_right_tab(drop.surface, pane, edge, cx);
            }
            (DropZone::Center, Some(target)) => {
                self.move_right_tab(drop.surface, target, None, cx);
            }
            (DropZone::Center, None) => {}
        }
        cx.notify();
    }

    /// The zone a drag at (`x`, `y`) in a `w`×`h` pane targets, if landing
    /// there changes anything: a pane's lone tab cannot split off itself,
    /// and dropping a tab into its own pane is no move. A side chat takes a
    /// dragged file tab anywhere as an attachment, so it offers no zones to
    /// one.
    #[allow(clippy::too_many_arguments)]
    fn right_drop_zone(
        &self,
        payload: &RightTabDrag,
        pane: Option<PaneId>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        cx: &App,
    ) -> Option<DropZone> {
        let shown = match pane {
            Some(pane) => self.resolved_pane_active(pane, cx),
            None => self.resolved_right_active(cx),
        };
        if payload.workspace_path.is_some() && matches!(shown, RightSurface::SideChat(_)) {
            return None;
        }
        let own = payload.pane == pane;
        match right_layout::drop_zone(x, y, w, h) {
            DropZone::Center => (!own).then_some(DropZone::Center),
            DropZone::Edge(edge) => {
                let lone = own && self.right_pane_rows(pane, cx).len() <= 1;
                (!lone).then_some(DropZone::Edge(edge))
            }
        }
    }

    /// One split pane. Panes along the host's top edge clear the titlebar
    /// band, where their strips live; the rest carry their own strip.
    fn render_right_split_pane(
        &mut self,
        id: PaneId,
        rect: Rect,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let surface = self.resolved_pane_active(id, cx);
        let top = rect.y < 1e-4;
        let content = self.render_right_surface(surface, rect.w * width, cx);
        let content = self.right_pane_drop_target(Some(id), content, cx);
        let header = (!top).then(|| {
            div()
                .h(px(crate::surface_chrome::HEADER_HEIGHT))
                .w_full()
                .flex_none()
                .flex()
                .items_center()
                .pl(px(STRIP_GUTTER))
                .pr(px(STRIP_TRAILING_GAP))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(self.render_right_tab_strip(Some(id), cx)),
                )
        });
        div()
            .id(("right-split-pane", id as usize))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                this.focus_right_pane(id, cx);
            }))
            // A side chat's transcript runs under the band, as unsplit.
            .when(top && !matches!(surface, RightSurface::SideChat(_)), |el| {
                el.pt(px(Theme::TITLEBAR_HEIGHT))
            })
            .children(header)
            .child(div().flex_1().min_h_0().relative().child(content))
            .into_any_element()
    }

    /// The titlebar's tab area over the right pane. Unsplit, the host's one
    /// strip; split, one strip per top-edge pane over its column. `origin`
    /// is the host's left edge relative to this area's.
    pub(super) fn render_right_titlebar_strips(
        &mut self,
        origin: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(layout) = self.right_split(cx).cloned() else {
            return self.render_right_tab_strip(None, cx);
        };
        let width = self.right_content_width(self.right_target(cx));
        let top = right_layout::top_row(&layout.rects());
        let last = top.len().saturating_sub(1);
        let mut area = div().relative().size_full();
        for (ix, (id, rect)) in top.into_iter().enumerate() {
            let left = (origin + rect.x * width + STRIP_GUTTER).max(0.0);
            let cell = div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(left))
                .overflow_hidden()
                .flex()
                .items_center();
            let cell = if ix == last {
                cell.right_0()
            } else {
                let right = origin + (rect.x + rect.w) * width - STRIP_TRAILING_GAP;
                cell.w(px((right - left).max(0.0)))
            };
            area = area.child(cell.child(self.render_right_tab_strip(Some(id), cx)));
        }
        area.into_any_element()
    }
}
