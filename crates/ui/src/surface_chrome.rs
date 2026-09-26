//! Shared metrics for sidebar surface toolbars and their controls.

use gpui::{Div, div, prelude::*, px};

use crate::theme::Theme;

pub(crate) const HEADER_HEIGHT: f32 = Theme::TITLEBAR_HEIGHT;
pub(crate) const CONTROL_SIZE: f32 = 24.0;
pub(crate) const CONTROL_RADIUS: f32 = 6.0;
pub(crate) const ICON_SIZE: f32 = 14.0;
pub(crate) const CONTROL_GAP: f32 = 4.0;
pub(crate) const EDGE_INSET: f32 = 8.0;

/// Shared field treatment for the file search and browser address controls.
pub(crate) fn input() -> Div {
    div()
        .h(px(CONTROL_SIZE))
        .min_w_0()
        .flex_1()
        .px(px(8.0))
        .rounded(px(CONTROL_RADIUS))
        .bg(crate::theme::ink(0.035))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.5))
}

pub(crate) fn toolbar(theme: &Theme) -> Div {
    div()
        .h(px(HEADER_HEIGHT))
        .w_full()
        .flex_none()
        .px(px(EDGE_INSET))
        .flex()
        .items_center()
        .gap(px(CONTROL_GAP))
        .border_t_1()
        .border_b_1()
        .border_color(theme.border)
        .bg(if theme.is_glass() {
            theme.surface.opacity(0.26)
        } else {
            theme.surface
        })
}

/// Neutral navigation chip shared by the right-panel strip and PR navigation.
pub(crate) fn tab_frame(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(CONTROL_SIZE))
        .flex_none()
        .rounded(px(CONTROL_RADIUS))
        .flex()
        .items_center()
        .gap(px(CONTROL_GAP))
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_selected(selected)
        .tab_index(0)
        .text_size(px(12.0))
        .text_color(if selected {
            theme.text
        } else {
            theme.text_muted
        })
        .focus_visible(|style| style.bg(crate::theme::wash(0.14)))
        .when(selected, |el| el.bg(crate::theme::wash(0.10)))
}

pub(crate) fn tab(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    tab_frame(id, selected, theme).when(!selected, |el| {
        el.hover(|style| style.bg(crate::theme::wash(0.06)))
    })
}
