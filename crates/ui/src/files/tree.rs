use gpui::{
    AnyElement, Context, KeyDownEvent, ListSizingBehavior, MouseButton, Window, div, list,
    prelude::*, px,
};
use zeron_proto::WorkspaceEntryKind;

use super::{
    FilesSurface, WorkspacePathDrag, model::DirectoryLoadState, model::VisibleRowKind,
    workspace_path_drag_ghost,
};
use crate::{
    file_icons::{self, FileIconIdentity},
    icons::{self, icon},
    popover,
    theme::Theme,
};

pub const TREE_ROW_HEIGHT: f32 = 27.0;
pub(super) const TREE_INDENT: f32 = 14.0;

/// Draw each ancestor's guide in the row itself so virtualized rows join
/// seamlessly, including when the parent has scrolled out of view.
pub(super) fn with_indent_guides(
    row: AnyElement,
    depth: usize,
    height: f32,
    theme: &Theme,
) -> AnyElement {
    div()
        .relative()
        .h(px(height))
        .w_full()
        .flex_none()
        .children((0..depth).map(|level| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                // Align with the center of the ancestor's 14px disclosure slot.
                .left(px(8.0 + 7.0 + level as f32 * TREE_INDENT))
                .w(px(1.0))
                .bg(theme.border)
        }))
        .child(row)
        .into_any_element()
}

/// Keep the viewport attached to a path rather than an index when rows move.
pub(super) fn sync_list_rows(
    list: &gpui::ListState,
    old: &[super::model::VisibleTreeRow],
    new: &[super::model::VisibleTreeRow],
) {
    if old == new {
        return;
    }
    let mut anchor = list.logical_scroll_top();
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let positions = new
        .iter()
        .enumerate()
        .map(|(index, row)| (row.path.as_str(), index))
        .collect::<std::collections::HashMap<_, _>>();
    let old_index = anchor.item_ix.min(old.len().saturating_sub(1));
    let mut path = old.get(old_index).map(|row| row.path.clone());
    let mut target = None;
    while let Some(candidate) = path {
        if let Some(index) = positions.get(candidate.as_str()) {
            target = Some(*index);
            break;
        }
        path = super::model::parent_path(&candidate);
    }
    let target = target.or_else(|| {
        old.iter()
            .skip(old_index)
            .chain(old[..old_index].iter().rev())
            .find_map(|row| positions.get(row.path.as_str()).copied())
    });
    list.splice(prefix..old.len() - suffix, new.len() - prefix - suffix);
    list.clone().with_uniform_item_height(px(TREE_ROW_HEIGHT));
    anchor.item_ix = target.unwrap_or(0);
    if target.is_none() {
        anchor.offset_in_item = px(0.0);
    }
    list.scroll_to(anchor);
}

/// The tree rail's geometry straight from the list state: track origin and
/// viewport from [`gpui::ListState::viewport_bounds`] (window space; zero
/// until first layout, so the rail simply stays hidden), content extent from
/// the measured item summaries via [`gpui::ListState::max_offset_for_scrollbar`],
/// and the position from [`gpui::ListState::scroll_px_offset_for_scrollbar`]
/// (negative while scrolled — flipped here to the positive distance). Row
/// counts are never multiplied in: row variants need not share a height.
fn tree_rail_geometry(
    list: &gpui::ListState,
) -> Option<(popover::MenuScrollbarMetrics, gpui::Pixels)> {
    let bounds = list.viewport_bounds();
    let viewport = f32::from(bounds.size.height);
    let max_scroll = f32::from(list.max_offset_for_scrollbar().y);
    let offset = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    let metrics =
        popover::MenuScrollbarMetrics::from_parts(viewport, viewport + max_scroll, offset)?;
    Some((metrics, bounds.top()))
}

/// The file tree's share of the shared floating rail: it owns no scroll
/// handle, so geometry comes from the list state's own accessors and drags
/// land through the list state's scrollbar offset setter.
impl popover::ScrollRailHost for FilesSurface {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.tree_bar
    }

    fn rail_metrics(&mut self) -> Option<popover::MenuScrollbarMetrics> {
        let (metrics, _) = tree_rail_geometry(&self.tree_list)?;
        let offset = -f32::from(self.tree_list.scroll_px_offset_for_scrollbar().y);
        self.rail_bar().note_scroll_offset(offset);
        Some(metrics)
    }

    fn rail_press(&mut self, pointer_y: gpui::Pixels) -> bool {
        let Some((metrics, track_top)) = tree_rail_geometry(&self.tree_list) else {
            return false;
        };
        self.rail_bar()
            .begin_press_in(&metrics, track_top, pointer_y);
        self.apply_tree_rail_target(&metrics, track_top, pointer_y)
    }

    fn rail_drag_to(&mut self, pointer_y: gpui::Pixels) -> bool {
        let Some((metrics, track_top)) = tree_rail_geometry(&self.tree_list) else {
            return false;
        };
        self.apply_tree_rail_target(&metrics, track_top, pointer_y)
    }
}

impl FilesSurface {
    /// Apply a shared-rail drag target: the fraction of max scroll goes back
    /// through the list state's own scrollbar offset setter, which clamps to
    /// the live content height. `false` when no drag is engaged.
    fn apply_tree_rail_target(
        &mut self,
        metrics: &popover::MenuScrollbarMetrics,
        track_top: gpui::Pixels,
        pointer_y: gpui::Pixels,
    ) -> bool {
        let Some(fraction) = self.tree_bar.drag_target_in(metrics, track_top, pointer_y) else {
            return false;
        };
        let target = px(-fraction * metrics.max_scroll);
        self.tree_list
            .set_offset_from_scrollbar(gpui::Point::new(px(0.0), target));
        true
    }

    pub(super) fn on_tree_scrolled(&mut self, cx: &mut Context<Self>) {
        // The list repaints itself; the floating rail needs a view pass.
        cx.notify();
    }

    fn on_tree_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.tree_bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    pub(super) fn render_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let scrollbar = popover::rail(self, "files-tree-scrollbar", &theme, cx);
        div()
            .id("files-tree")
            .role(gpui::Role::Tree)
            .aria_label("Workspace file tree")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&self.tree_focus)
            .on_hover(cx.listener(Self::on_tree_hovered))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.tree_focus.focus(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_tree_key_down(event, window, cx)
            }))
            .child(
                list(self.tree_list.clone(), cx.processor(Self::render_tree_row))
                    .flex_1()
                    .min_h_0()
                    .with_sizing_behavior(ListSizingBehavior::Auto),
            )
            .children(scrollbar)
            .into_any_element()
    }

    fn render_tree_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.tree.visible_rows().get(index).cloned() else {
            return gpui::Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let padding = 8.0 + row.depth as f32 * TREE_INDENT;
        let content = match row.kind {
            VisibleRowKind::Entry => {
                let Some(node) = self.tree.node(&row.path).cloned() else {
                    return gpui::Empty.into_any_element();
                };
                let path = row.path.clone();
                let selected = self.tree.selected() == Some(path.as_str());
                let focused = self.tree_focus.is_focused(window);
                let is_directory = node.entry.kind == WorkspaceEntryKind::Directory;
                let decoration = self.git_decoration(&row.path, is_directory, cx);
                let drag_payload = WorkspacePathDrag::new(path.clone(), is_directory);
                let expanded = is_directory && self.tree.is_expanded(&path);
                let text_color = if let Some(decoration) = decoration {
                    decoration.color(&theme)
                } else if selected {
                    theme.text
                } else {
                    theme.text_muted
                };
                let file_identity = match node.entry.kind {
                    WorkspaceEntryKind::Directory => {
                        FileIconIdentity::directory(&node.entry.name, expanded)
                    }
                    WorkspaceEntryKind::File => FileIconIdentity::file(&node.entry.name),
                    WorkspaceEntryKind::Symlink => FileIconIdentity::symlink(&node.entry.name),
                };
                div()
                    .id(gpui::SharedString::from(format!(
                        "files-tree-entry:{}",
                        row.path
                    )))
                    .role(gpui::Role::TreeItem)
                    .aria_label(node.entry.name.clone())
                    .aria_selected(selected)
                    .when(is_directory, |element| element.aria_expanded(expanded))
                    .h(px(TREE_ROW_HEIGHT))
                    .w_full()
                    .flex_none()
                    .pl(px(padding))
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .cursor_pointer()
                    .when(node.entry.ignored && decoration.is_none(), |element| {
                        element.opacity(0.52)
                    })
                    .when(selected, |element| {
                        element.bg(crate::theme::wash(if focused { 0.12 } else { 0.08 }))
                    })
                    .when(!selected, |element| {
                        element.hover(|style| style.bg(crate::theme::wash(0.055)))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tree_focus.focus(window, cx);
                        this.activate_tree_path(path.clone(), cx);
                    }))
                    .when(crate::click_activation_drag_enabled(), |element| {
                        element.on_drag(drag_payload, |payload, _, _, cx| {
                            cx.stop_propagation();
                            workspace_path_drag_ghost(payload, cx)
                        })
                    })
                    .child(
                        div()
                            .size(px(14.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(is_directory, |element| {
                                element.child(
                                    icon(if expanded {
                                        icons::ALT_ARROW_DOWN
                                    } else {
                                        icons::ALT_ARROW_RIGHT
                                    })
                                    .size(px(11.0))
                                    .text_color(theme.text_faint),
                                )
                            }),
                    )
                    .child(
                        file_icons::icon(file_identity, theme.appearance)
                            .size(px(14.0))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.font_sans.clone())
                            .text_size(px(11.5))
                            .text_color(text_color)
                            .child(node.entry.name),
                    )
                    .into_any_element()
            }
            VisibleRowKind::Loading { .. } => status_row(
                index,
                padding + TREE_INDENT,
                "Loading…",
                theme.text_faint,
                &theme,
            ),
            VisibleRowKind::Empty { .. } => status_row(
                index,
                padding + TREE_INDENT,
                "Empty folder",
                theme.text_faint.opacity(0.7),
                &theme,
            ),
            VisibleRowKind::Error { directory, message } => div()
                .id(("files-tree-error", index))
                .h(px(TREE_ROW_HEIGHT))
                .w_full()
                .flex_none()
                .pl(px(padding + TREE_INDENT))
                .pr(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .hover(|style| style.bg(crate::theme::wash(0.055)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let cursor = this
                        .tree
                        .node(&directory)
                        .and_then(|node| match &node.load {
                            DirectoryLoadState::Error { cursor, .. } => cursor.clone(),
                            _ => None,
                        });
                    this.load_directory(directory.clone(), cursor, cx);
                }))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(10.5))
                        .text_color(theme.danger.opacity(0.82))
                        .child(format!("{message} — Retry")),
                )
                .into_any_element(),
            VisibleRowKind::LoadMore { directory, cursor } => div()
                .id(("files-tree-more", index))
                .h(px(TREE_ROW_HEIGHT))
                .w_full()
                .flex_none()
                .pl(px(padding + TREE_INDENT))
                .pr(px(8.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(|style| style.bg(crate::theme::wash(0.055)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.load_directory(directory.clone(), Some(cursor.clone()), cx);
                }))
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.text_muted)
                        .child("Load more…"),
                )
                .into_any_element(),
        };
        with_indent_guides(content, row.depth, TREE_ROW_HEIGHT, &theme)
    }

    pub(super) fn activate_tree_path(&mut self, path: String, cx: &mut Context<Self>) {
        self.tree.select(path.clone());
        let is_directory = self
            .tree
            .node(&path)
            .is_some_and(|node| node.entry.kind == WorkspaceEntryKind::Directory);
        if is_directory {
            self.toggle_tree_directory(path, cx);
        } else {
            self.open_tree_file(path, cx);
        }
        self.reveal_tree_selection();
        cx.notify();
    }

    fn toggle_tree_directory(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.tree.toggle_expanded(&path) {
            return;
        }
        self.sync_tree_list();
        let needs_load = self.tree.node(&path).is_some_and(|node| {
            node.stale
                || matches!(
                    node.load,
                    DirectoryLoadState::Unloaded | DirectoryLoadState::Error { .. }
                )
        });
        if self.tree.is_expanded(&path) && needs_load {
            self.load_directory(path, None, cx);
        }
    }

    fn on_tree_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handled = match event.keystroke.key.as_str() {
            "up" => {
                self.tree.select_previous();
                true
            }
            "down" => {
                self.tree.select_next();
                true
            }
            "left" => {
                let selected = self.tree.selected().map(str::to_string);
                if let Some(path) = selected {
                    if self.tree.is_expanded(&path) {
                        self.tree.toggle_expanded(&path);
                        self.sync_tree_list();
                    } else {
                        self.tree.select_parent();
                    }
                }
                true
            }
            "right" => {
                let selected = self.tree.selected().map(str::to_string);
                if let Some(path) = selected
                    && self
                        .tree
                        .node(&path)
                        .is_some_and(|node| node.entry.kind == WorkspaceEntryKind::Directory)
                {
                    if self.tree.is_expanded(&path) {
                        self.tree.select_first_child();
                    } else {
                        self.toggle_tree_directory(path, cx);
                    }
                }
                true
            }
            "enter" | "space" => {
                if let Some(path) = self.tree.selected().map(str::to_string) {
                    self.activate_tree_path(path, cx);
                }
                true
            }
            _ => false,
        };
        if handled {
            window.prevent_default();
            cx.stop_propagation();
            self.reveal_tree_selection();
            cx.notify();
        }
    }

    pub(super) fn reveal_tree_selection(&self) {
        let Some(selected) = self.tree.selected() else {
            return;
        };
        if let Some(index) = self
            .tree
            .visible_rows()
            .iter()
            .position(|row| row.path == selected)
        {
            self.tree_list.scroll_to_reveal_item(index);
        }
    }
}

fn status_row(
    index: usize,
    padding: f32,
    label: &'static str,
    color: gpui::Hsla,
    theme: &Theme,
) -> AnyElement {
    div()
        .id(("files-tree-status", index))
        .h(px(TREE_ROW_HEIGHT))
        .w_full()
        .flex_none()
        .pl(px(padding))
        .pr(px(8.0))
        .flex()
        .items_center()
        .font_family(theme.font_sans.clone())
        .text_size(px(10.5))
        .text_color(color)
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::super::model::VisibleTreeRow;
    use super::*;
    use gpui::{ListAlignment, ListOffset, ListState};

    fn rows(paths: &[&str]) -> Vec<VisibleTreeRow> {
        paths
            .iter()
            .map(|path| VisibleTreeRow {
                path: (*path).into(),
                depth: 0,
                kind: VisibleRowKind::Entry,
            })
            .collect()
    }

    #[test]
    fn directory_load_keeps_the_viewport_on_the_same_path() {
        let old = rows(&["a", "src", "z"]);
        let loading = rows(&["a", "src", "src/loading", "z"]);
        let loaded = rows(&["a", "src", "src/a", "src/b", "z"]);
        let list = ListState::new(old.len(), ListAlignment::Top, px(100.0))
            .with_uniform_item_height(px(TREE_ROW_HEIGHT));
        list.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(7.0),
        });
        sync_list_rows(&list, &old, &loading);
        assert_eq!(list.logical_scroll_top().item_ix, 3);
        sync_list_rows(&list, &loading, &loaded);
        assert_eq!(list.logical_scroll_top().item_ix, 4);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(7.0));
        sync_list_rows(&list, &loaded, &loaded);
        assert_eq!(list.logical_scroll_top().item_ix, 4);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(7.0));
    }

    #[test]
    fn removed_anchor_falls_back_to_parent_then_neighbor() {
        let old = rows(&["a", "src", "src/child", "z"]);
        let collapsed = rows(&["a", "src", "z"]);
        let removed = rows(&["a", "z"]);
        let list = ListState::new(old.len(), ListAlignment::Top, px(100.0));
        list.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(3.0),
        });
        sync_list_rows(&list, &old, &collapsed);
        assert_eq!(list.logical_scroll_top().item_ix, 1);
        sync_list_rows(&list, &collapsed, &removed);
        assert_eq!(list.logical_scroll_top().item_ix, 1);
        sync_list_rows(&list, &removed, &[]);
        assert_eq!(list.item_count(), 0);
        assert_eq!(list.logical_scroll_top().item_ix, 0);
    }
}
