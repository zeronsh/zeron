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
        .focus_visible(|style| style.border_2().border_color(theme.accent))
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

/// Fixed pane tab width. Drag reordering (drop-index quantisation and slide
/// offsets) assumes uniform chips.
pub(crate) const TAB_CHIP_WIDTH: f32 = 112.0;
pub(crate) const TAB_CHIP_HEIGHT: f32 = 24.0;
/// A chip plus the strip's own gap.
pub(crate) const TAB_CHIP_SLOT: f32 = TAB_CHIP_WIDTH + CONTROL_GAP;
/// Width of the fade over tabs scrolled out of a strip.
pub(crate) const TAB_STRIP_FADE: f32 = 36.0;

/// One pane tab chip, as drawn in the titlebar band above a side pane.
pub(crate) struct TabChip {
    /// Element id prefix; `{id}-{ix}` is also the chip's debug selector.
    pub id: &'static str,
    /// Element id prefix of the close slot, debug selector `{close_id}-{ix}`.
    pub close_id: &'static str,
    pub ix: usize,
    pub active: bool,
    pub title: gpui::SharedString,
    /// Shows the unsaved dot in the close slot while the tab is not hovered.
    pub dirty: bool,
    /// The leading 18px slot's content, usually [`tab_chip_icon`].
    pub leading: gpui::AnyElement,
}

/// A pane tab: the leading icon, the truncated title, and a trailing slot
/// whose ✕ fades in on hover over the unsaved dot (same slot, no width jump).
/// Callers add activation, menus, tooltips and drag behaviour.
pub(crate) fn tab_chip(
    chip: TabChip,
    theme: &Theme,
    on_close: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    let TabChip {
        id,
        close_id,
        ix,
        active,
        title,
        dirty,
        leading,
    } = chip;
    let group: gpui::SharedString = format!("{id}-{ix}").into();
    let selector = group.clone();
    tab((id, ix), active, theme)
        .debug_selector(move || selector.to_string())
        .group(group.clone())
        .h(px(TAB_CHIP_HEIGHT))
        .w(px(TAB_CHIP_WIDTH))
        .flex_none()
        .px(px(4.0))
        .rounded(px(CONTROL_RADIUS))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(3.0))
        .cursor_pointer()
        .role(gpui::Role::Button)
        // The old session-tab strip's solved carve-out: NOT `.occlude()` — a
        // BlockMouse hitbox ends the hit test, so the scroll container behind
        // the tabs never saw wheel events and an overflowing strip could not
        // be scrolled (tabs tile the whole region). ExceptScroll keeps the
        // titlebar drag-region carve-out and lets the strip scroll.
        .block_mouse_except_scroll()
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .child(
            div()
                .flex_none()
                .size(px(18.0))
                .flex()
                .items_center()
                .justify_center()
                .child(leading),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::typography::ui_rems(11.5))
                .text_color(if active { theme.text } else { theme.text_muted })
                .child(title),
        )
        .child(
            // Trailing slot: the unsaved dot normally, ✕ on tab hover — two
            // stacked layers opacity-swapped by the group hover.
            div()
                .id((close_id, ix))
                .debug_selector(move || format!("{close_id}-{ix}"))
                .flex_none()
                .size(px(18.0))
                .rounded(px(4.0))
                .relative()
                .role(gpui::Role::Button)
                .aria_label("Close tab")
                .tooltip(crate::settings::widgets::text_tooltip("Close tab"))
                .hover(|s| s.bg(crate::theme::wash(0.12)))
                // The tab owns a drag payload. Claim the close press before
                // it reaches that parent or GPUI starts a tab drag instead of
                // delivering the close click.
                .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    on_close(event, window, cx);
                })
                .when(dirty, |slot| {
                    slot.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover(group.clone(), |s| s.opacity(0.0))
                            .child(div().size(px(6.0)).rounded_full().bg(theme.text_muted)),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(0.0)
                        .group_hover(group, |s| s.opacity(1.0))
                        .child(
                            crate::icons::icon(crate::icons::CLOSE)
                                .size(px(12.0))
                                .text_color(theme.text_muted),
                        ),
                ),
        )
}

/// A chip's leading glyph, quieter on inactive tabs.
pub(crate) fn tab_chip_icon(path: &'static str, active: bool, theme: &Theme) -> gpui::AnyElement {
    crate::icons::icon(path)
        .size(px(12.0))
        .text_color(if active {
            theme.text_muted
        } else {
            theme.text_muted.opacity(0.7)
        })
        .into_any_element()
}

/// Wrap a horizontally scrolling tab strip in its region, fading whichever
/// side hides tabs. Glass: a per-glyph EdgeFade scope over the chips' own
/// opacity ramps; opaque: painted gradients in the shell surface tone.
pub(crate) fn tab_strip_region(
    strip: impl IntoElement,
    scroll: &gpui::ScrollHandle,
    theme: &Theme,
) -> gpui::AnyElement {
    // Fade flags from the LAST frame's scroll state (invisible lag).
    let scrolled = -f32::from(scroll.offset().x);
    let max_scroll = f32::from(scroll.max_offset().x);
    let fade_left = scrolled > 1.0;
    let fade_right = scrolled < max_scroll - 1.0;
    let glass = theme.is_glass();
    let bar_bg = theme.surface;
    let edge = |angle: f32| {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(TAB_STRIP_FADE))
            .bg(gpui::linear_gradient(
                angle,
                gpui::linear_color_stop(bar_bg, 0.0),
                gpui::linear_color_stop(bar_bg.opacity(0.0), 1.0),
            ))
    };
    let region = div()
        .relative()
        .min_w_0()
        .size_full()
        .flex()
        .items_center()
        .child(strip)
        .when(fade_left && !glass, |el| el.child(edge(90.0).left_0()))
        .when(fade_right && !glass, |el| el.child(edge(270.0).right_0()));
    if glass {
        crate::edge_fade::edge_faded(TAB_STRIP_FADE, false, false, region)
            .fade_left(fade_left)
            .fade_right(fade_right)
            .into_any_element()
    } else {
        region.into_any_element()
    }
}
