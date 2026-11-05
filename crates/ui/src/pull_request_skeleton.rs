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
        // A fixed-width bar never overflows a narrow pane.
        .max_w_full()
        .h(px(height))
        .flex_none()
        .rounded(px(height / 2.0))
        .bg(theme.ink(0.07))
        .opacity(0.45 + 0.4 * crate::motion::pulse_wave(phase))
}

/// Deterministic width ladder so rows read as varied text, not slabs.
const WIDTHS: [f32; 6] = [0.62, 0.48, 0.71, 0.55, 0.39, 0.66];

/// The board's grouped list: a group label over a card of PR rows.
pub(crate) fn board(
    layout: crate::pull_requests::PullRequestTableLayout,
    view: EntityId,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let rows = (0..6)
        .map(|index| {
            let identity = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(Theme::SPACE_XS))
                .child(
                    div()
                        .h(px(20.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(bar(px(14.0), 14.0, index, view, theme, cx))
                        .child(bar(relative(WIDTHS[index]), 12.0, index, view, theme, cx)),
                )
                .child(
                    div()
                        .pl(px(22.0))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(Theme::SPACE_SM))
                        .child(bar(px(16.0), 16.0, index, view, theme, cx))
                        .child(bar(px(72.0), 10.0, index, view, theme, cx))
                        .child(bar(px(52.0), 16.0, index, view, theme, cx))
                        .child(bar(px(120.0), 12.0, index, view, theme, cx)),
                );
            let row = crate::pull_requests::table_row_shell(layout, index == 0, index == 5, theme)
                .child(identity);
            if layout == crate::pull_requests::PullRequestTableLayout::Narrow {
                row.child(
                    div()
                        .pl(px(22.0))
                        .h(px(28.0))
                        .flex()
                        .items_center()
                        .gap(px(Theme::SPACE_SM))
                        .child(bar(px(72.0), 10.0, index, view, theme, cx))
                        .child(div().flex_1())
                        .child(bar(px(44.0), 10.0, index, view, theme, cx))
                        .child(
                            div()
                                .size(px(28.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(bar(px(14.0), 14.0, index, view, theme, cx)),
                        ),
                )
                .into_any_element()
            } else {
                row.child(
                    div()
                        .w(px(112.0))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(Theme::SPACE_XS))
                        .child(div().h(px(16.0)).flex().items_center().child(bar(
                            px(44.0),
                            10.0,
                            index,
                            view,
                            theme,
                            cx,
                        )))
                        .child(div().h(px(16.0)).flex().items_center().child(bar(
                            px(72.0),
                            10.0,
                            index,
                            view,
                            theme,
                            cx,
                        ))),
                )
                .child(
                    div()
                        .size(px(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(bar(px(14.0), 14.0, index, view, theme, cx)),
                )
                .into_any_element()
            }
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
    let fields = ["Status", "Review", "Branch", "Changes"]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let height = if index < 2 { 22.0 } else { 12.0 };
            crate::pull_request_detail::field_row(
                label,
                bar(
                    px(if index == 2 { 220.0 } else { 96.0 }),
                    height,
                    index,
                    view,
                    theme,
                    cx,
                )
                .into_any_element(),
                index == 0,
                theme,
            )
        })
        .collect::<Vec<_>>();
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
        .child(widgets::section_card(theme).mt_0().children(fields))
        .child(
            widgets::section_card(theme).mt_0().child(
                widgets::card_row(theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(widgets::row_title(theme, "Current assessment"))
                            .child(bar(relative(0.6), 14.0, 4, view, theme, cx))
                            .child(bar(relative(0.4), 14.0, 4, view, theme, cx))
                            .child(bar(px(100.0), 13.0, 4, view, theme, cx)),
                    )
                    .child(
                        div()
                            .size(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(bar(px(14.0), 14.0, 4, view, theme, cx)),
                    ),
            ),
        )
        .child(
            widgets::section_card(theme)
                .mt_0()
                .px(px(16.0))
                .py(px(10.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(bar(px(14.0), 14.0, 4, view, theme, cx))
                .child(div().flex_1().child("Checks"))
                .child(bar(px(72.0), 22.0, 4, view, theme, cx)),
        )
        .child(
            widgets::section_card(theme)
                .mt_0()
                .child(
                    widgets::card_row(theme, true)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(220.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(widgets::row_title(theme, "Hand off to an agent"))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(12.0))
                                        .text_color(theme.text_muted)
                                        .child(
                                            "Opens a new session with the prompt ready to send.",
                                        ),
                                ),
                        )
                        .child(bar(px(180.0), 28.0, 5, view, theme, cx)),
                )
                .child(
                    widgets::card_row(theme, false)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(120.0))
                                .child(widgets::row_title(theme, "Verification")),
                        )
                        .child(div().flex_1().min_w_0().child(bar(
                            relative(0.8),
                            28.0,
                            6,
                            view,
                            theme,
                            cx,
                        ))),
                )
                .child(crate::pull_request_detail::field_row(
                    "Checkout",
                    bar(relative(0.7), 12.0, 6, view, theme, cx).into_any_element(),
                    false,
                    theme,
                )),
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
pub(crate) fn diff(split: bool, view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    let header = div()
        .h(px(crate::changes::FILE_HEADER_HEIGHT))
        .flex_none()
        .px(px(Theme::SPACE_MD))
        .flex()
        .items_center()
        .gap(px(8.0))
        .bg(theme.ink(0.025))
        .child(bar(px(13.0), 13.0, 0, view, theme, cx))
        .child(bar(px(14.0), 14.0, 0, view, theme, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(bar(relative(0.55), 10.0, 0, view, theme, cx)),
        )
        .child(bar(px(64.0), 10.0, 0, view, theme, cx))
        .child(bar(px(24.0), 24.0, 0, view, theme, cx));
    let cell = |index: usize, cx: &mut App| {
        let gutter = |cx: &mut App| {
            div()
                .w(px(crate::changes::GUTTER_WIDTH))
                .flex_none()
                .pr(px(8.0))
                .flex()
                .justify_end()
                .child(bar(px(12.0), 8.0, index, view, theme, cx))
        };
        div()
            .h(px(crate::changes::diff_line_height(theme)))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_hidden()
            .child(div().w(px(crate::changes::ACCENT_BAR_WIDTH)).flex_none())
            .child(gutter(cx))
            .when(!split, |el| el.child(gutter(cx)))
            .child(
                div()
                    .w(px(if split {
                        crate::changes::SPLIT_MARKER_WIDTH
                    } else {
                        crate::changes::MARKER_WIDTH
                    }))
                    .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl(px(if split {
                        crate::changes::SPLIT_CODE_PADDING_LEFT
                    } else {
                        crate::changes::UNIFIED_CODE_PADDING_LEFT
                    }))
                    .child(bar(
                        relative(WIDTHS[index % WIDTHS.len()]),
                        8.0,
                        index,
                        view,
                        theme,
                        cx,
                    )),
            )
    };
    let lines = (0..14)
        .map(|index| {
            div()
                .h(px(crate::changes::diff_line_height(theme)))
                .flex_none()
                .flex()
                .child(cell(index, cx))
                .when(split, |row| {
                    row.child(
                        div()
                            .w(px(crate::changes::SPLIT_DIVIDER_WIDTH))
                            .self_stretch()
                            .flex_none()
                            .bg(theme.border),
                    )
                    .child(cell(index, cx))
                })
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
        .child(
            div()
                .h(px(crate::changes::HUNK_HEADER_HEIGHT))
                .flex_none()
                .px(px(Theme::SPACE_MD))
                .flex()
                .items_center()
                .child(bar(px(180.0), 10.0, 0, view, theme, cx)),
        )
        .child(div().flex().flex_col().children(lines))
        .into_any_element()
}

/// Rows of the changed-file tree.
pub(crate) fn tree(view: EntityId, theme: &Theme, cx: &mut App) -> AnyElement {
    div()
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
