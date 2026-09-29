//! Loading placeholders shaped like the content they stand in for, so the
//! board and detail keep their layout while GitHub answers. Bars pulse on the
//! shared loader clock (static under reduced motion).
use crate::{settings::widgets, theme::Theme};
use gpui::{AnyElement, App, EntityId, div, prelude::*, px, relative};

/// One pulsing placeholder bar; `index` staggers its phase down a column.
pub(crate) fn bar(
    width: impl Into<gpui::Length>,
    height: f32,
    index: usize,
    view: EntityId,
    theme: &Theme,
    cx: &mut App,
) -> gpui::Div {
    let delta = crate::motion::pulse_delta(&crate::motion::ZERON_PULSE, view, cx);
    let phase = crate::motion::staggered_phase(delta, index, 0.08);
    div()
        .w(width.into())
        .h(px(height))
        .flex_none()
        .rounded(px(height / 2.0))
        .bg(theme.ink(0.07))
        .opacity(0.45 + 0.4 * crate::motion::pulse_wave(phase))
}

/// Deterministic width ladder so rows read as varied text, not slabs.
const WIDTHS: [f32; 6] = [0.62, 0.48, 0.71, 0.55, 0.39, 0.66];

/// The board's grouped list: a group label over a card of PR rows.
pub(crate) fn board(row_height: f32, view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    let rows = (0..6)
        .map(|index| {
            widgets::card_row(theme, index == 0)
                .mx_0()
                .px(px(16.0))
                .min_h(px(row_height))
                .flex_nowrap()
                .child(bar(px(14.0), 14.0, index, view, theme, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(bar(relative(WIDTHS[index]), 12.0, index, view, theme, cx))
                        .child(bar(px(160.0), 10.0, index, view, theme, cx)),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(8.0))
                        .child(bar(px(44.0), 10.0, index, view, theme, cx))
                        .child(bar(px(72.0), 10.0, index, view, theme, cx)),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();
    div()
        .id("pull-requests-skeleton")
        .debug_selector(|| "pull-requests-skeleton".into())
        .role(gpui::Role::ProgressIndicator)
        .aria_label("Loading pull requests")
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .min_h(px(24.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .child(bar(px(120.0), 10.0, 0, view, theme, cx)),
        )
        .child(widgets::section_card(theme).mt_0().w_full().children(rows))
        .into_any_element()
}

/// The Summary's card stack below a real header.
pub(crate) fn summary(gap: f32, view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    let field = |index: usize, width: f32, cx: &mut App| {
        widgets::card_row(theme, false)
            .min_h(px(44.0))
            .py(px(8.0))
            .child(
                div()
                    .w(px(80.0))
                    .flex_none()
                    .child(bar(px(56.0), 10.0, index, view, theme, cx)),
            )
            .child(bar(px(width), 10.0, index, view, theme, cx))
    };
    let status = |index: usize, cx: &mut App| {
        div()
            .flex_1()
            .min_w(px(160.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(bar(px(48.0), 10.0, index, view, theme, cx))
            .child(bar(px(96.0), 22.0, index, view, theme, cx))
    };
    let lines = (0..5)
        .map(|index| {
            bar(
                relative(if index == 4 { 0.4 } else { 0.92 }),
                10.0,
                index,
                view,
                theme,
                cx,
            )
            .into_any_element()
        })
        .collect::<Vec<_>>();
    div()
        .id("pr-detail-skeleton")
        .debug_selector(|| "pr-detail-skeleton".into())
        .role(gpui::Role::ProgressIndicator)
        .aria_label("Loading pull request")
        .mt(px(24.0))
        .flex()
        .flex_col()
        .gap(px(gap))
        .child(
            widgets::section_card(theme)
                .mt_0()
                .child(
                    div()
                        .p(px(16.0))
                        .flex()
                        .flex_wrap()
                        .gap(px(16.0))
                        .child(status(0, cx))
                        .child(status(1, cx)),
                )
                .child(field(2, 220.0, cx))
                .child(field(3, 150.0, cx)),
        )
        .child(
            widgets::section_card(theme)
                .mt_0()
                .h(px(44.0))
                .px(px(16.0))
                .flex_row()
                .items_center()
                .justify_between()
                .child(bar(px(88.0), 10.0, 4, view, theme, cx))
                .child(bar(px(72.0), 22.0, 4, view, theme, cx)),
        )
        .child(
            widgets::section_card(theme)
                .mt_0()
                .p(px(16.0))
                .gap(px(12.0))
                .children(lines),
        )
        .into_any_element()
}

/// Diff lines inside the stream card, under a file header.
pub(crate) fn diff(view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    let header = div()
        .h(px(crate::changes::FILE_HEADER_HEIGHT))
        .flex_none()
        .px(px(Theme::SPACE_MD))
        .flex()
        .items_center()
        .gap(px(8.0))
        .bg(theme.ink(0.025))
        .child(bar(px(14.0), 14.0, 0, view, theme, cx))
        .child(bar(px(220.0), 10.0, 0, view, theme, cx));
    let lines = (0..14)
        .map(|index| {
            div()
                .h(px(crate::changes::diff_line_height(theme)))
                .flex_none()
                .px(px(Theme::SPACE_MD))
                .flex()
                .items_center()
                .gap(px(16.0))
                .child(bar(px(28.0), 8.0, index, view, theme, cx))
                .child(bar(
                    relative(WIDTHS[index % WIDTHS.len()] * 0.8),
                    8.0,
                    index,
                    view,
                    theme,
                    cx,
                ))
                .into_any_element()
        })
        .collect::<Vec<_>>();
    div()
        .id("pr-code-skeleton")
        .debug_selector(|| "pr-code-skeleton".into())
        .role(gpui::Role::ProgressIndicator)
        .aria_label("Loading diff")
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(div().py(px(8.0)).flex().flex_col().children(lines))
        .into_any_element()
}

/// Rows of the changed-file tree.
pub(crate) fn tree(view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    div()
        .pt(px(4.0))
        .flex()
        .flex_col()
        .gap(px(1.0))
        .children((0..12).map(|index| {
            let nested = index % 4 != 0;
            div()
                .h(px(28.0))
                .flex_none()
                .pl(px(if nested { 22.0 } else { 8.0 }))
                .pr(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(bar(px(14.0), 14.0, index, view, theme, cx))
                .child(bar(
                    relative(WIDTHS[index % WIDTHS.len()] * 0.7),
                    10.0,
                    index,
                    view,
                    theme,
                    cx,
                ))
        }))
        .into_any_element()
}
