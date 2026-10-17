//! Context occupancy is read from the replicated chat snapshot, never local CLI state.
use crate::theme::Theme;
use gpui::{IntoElement, PathBuilder, SharedString, canvas, div, point, prelude::*, px};
use zeron_proto::ContextUsage;

/// The popover's fixed width — about 3/4 of the first wide build (was double
/// the hug-content width, read as a billboard), still enough for the
/// used/used-up and remaining lines to pull to opposite corners around the
/// bar (user request).
const CONTEXT_CARD_WIDTH: f32 = 322.0;

/// The context ring's trigger chip; the footer ([`crate::account_usage`])
/// opens [`card`] from it on click.
pub fn chip(usage: Option<ContextUsage>, open: bool, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let fraction = usage.and_then(ContextUsage::fraction);
    let color = match fraction {
        Some(f) if f >= 0.9 => theme.danger,
        Some(f) if f >= 0.75 => theme.warning,
        Some(_) => theme.text_muted,
        None => theme.text_faint,
    };
    let label = fraction
        .map(|f| format!("{:.0}%", f * 100.0))
        .unwrap_or_else(|| "—".into());
    ring_chip(
        "context-usage",
        fraction.unwrap_or(0.0) as f32,
        color,
        color,
        label,
        open,
        theme,
    )
}

/// One footer ring indicator: ring + percent, identical geometry for every
/// ring so they sit side by side as equals. `arc` colours the ring's fill,
/// `text` the label; `open` holds the hover wash while its popover is up.
pub(crate) fn ring_chip(
    id: &'static str,
    fraction: f32,
    arc: gpui::Hsla,
    text: gpui::Hsla,
    label: String,
    open: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .h(px(24.0))
        .px(px(6.0))
        .rounded(px(6.0))
        .text_size(px(11.0))
        .text_color(text)
        .cursor_pointer()
        .when(open, |s| s.bg(crate::theme::ink(0.05)))
        .hover(|s| s.bg(crate::theme::ink(0.05)))
        .child(ring(fraction, arc, theme))
        .child(SharedString::from(label))
}

/// The footer's 16px progress ring: a faint full track under a `fraction` arc
/// starting at twelve o'clock. Shared with the account usage indicator.
pub(crate) fn ring(fraction: f32, color: gpui::Hsla, theme: &Theme) -> impl IntoElement {
    let track = theme.text_faint.opacity(0.25);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let mut arc = |fraction: f32, color| {
                if fraction <= 0.0 {
                    return;
                }
                let steps = (64.0 * fraction).ceil().max(2.0) as usize;
                let mut path = PathBuilder::stroke(px(1.8));
                for i in 0..=steps {
                    let angle = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * fraction * i as f32 / steps as f32;
                    let p = point(
                        center.x + px(6.0 * angle.cos()),
                        center.y + px(6.0 * angle.sin()),
                    );
                    if i == 0 {
                        path.move_to(p);
                    } else {
                        path.line_to(p);
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            };
            arc(1.0, track);
            arc(fraction.clamp(0.0, 1.0), color);
        },
    )
    .size(px(16.0))
}

/// Compact token counts for the popover ("11.7k / 200k", "188k remaining"):
/// plain below a thousand, one decimal under 100k, round k/m above — the
/// raw digit strings read as noise at a glance (user request).
fn compact_tokens(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    let (scale, suffix) = if count < 1_000_000 {
        (1_000.0, "k")
    } else {
        (1_000_000.0, "m")
    };
    let value = count as f64 / scale;
    let mut text = if value < 100.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    };
    if text.ends_with(".0") {
        text.truncate(text.len() - 2);
    }
    text.push_str(suffix);
    text
}

/// whether the indicator has anything to measure against: harnesses that
/// never report a window (antigravity) get no indicator at all, rather than a
/// permanently empty ring.
pub fn has_window(usage: Option<ContextUsage>) -> bool {
    usage
        .and_then(|usage| usage.window)
        .is_some_and(|window| window > 0)
}

/// The two lines the wide card sets around the bar: used/window top-left,
/// remaining bottom-right.
fn usage_lines(usage: Option<ContextUsage>) -> (String, String) {
    match usage.unwrap_or_default() {
        ContextUsage {
            tokens: Some(tokens),
            window: Some(window),
        } if window > 0 => (
            format!(
                "{} / {} used",
                compact_tokens(tokens),
                compact_tokens(window)
            ),
            format!(
                "{} remaining",
                compact_tokens(window.saturating_sub(tokens))
            ),
        ),
        ContextUsage {
            tokens: Some(tokens),
            ..
        } => (
            format!("{} used", compact_tokens(tokens)),
            "Context limit not reported".into(),
        ),
        ContextUsage {
            window: Some(window),
            ..
        } if window > 0 => (
            format!("{} capacity", compact_tokens(window)),
            "Waiting for context usage".into(),
        ),
        _ => (
            "Context usage not reported".into(),
            "by this harness yet".into(),
        ),
    }
}

/// The context ring's popover content: used/window top-left, a horizontal
/// occupancy bar through the middle (the ring's threshold colors), remaining
/// bottom-right — twice the old hug-content width (user request).
pub fn card(usage: Option<ContextUsage>, theme: &Theme) -> gpui::Div {
    let fraction = usage.and_then(ContextUsage::fraction);
    let (used, remaining) = usage_lines(usage);
    let line = |text: String| {
        div()
            .text_size(px(12.0))
            .line_height(px(19.0))
            .whitespace_nowrap()
            .text_color(theme.text_muted)
            .child(SharedString::from(text))
    };
    crate::popover::popover_card(theme)
        .flex()
        .flex_col()
        .w(px(CONTEXT_CARD_WIDTH))
        .child(crate::popover::menu_heading(theme, "Context window"))
        .child(
            div()
                .px(px(4.0))
                .flex()
                .flex_col()
                .child(line(used))
                .when_some(fraction, |column, fraction| {
                    // Same tiering as the ring chip: the bar turns for
                    // trouble before the text has to say so.
                    let fill = if fraction >= 0.9 {
                        theme.danger
                    } else if fraction >= 0.75 {
                        theme.warning
                    } else {
                        theme.text_muted
                    };
                    column.child(
                        div()
                            .my(px(6.0))
                            .h(px(4.0))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(theme.text_faint.opacity(0.25))
                            .child(
                                div()
                                    .w(gpui::relative(fraction.clamp(0.0, 1.0) as f32))
                                    .h_full()
                                    .rounded_full()
                                    .bg(fill),
                            ),
                    )
                })
                // Bottom-right: the trailing line hugs the card's far
                // corner (user request).
                .child(div().flex().justify_end().child(line(remaining))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indicator_needs_a_reported_window() {
        assert!(!has_window(None));
        assert!(!has_window(Some(ContextUsage {
            tokens: Some(1_200),
            window: None,
        })));
        assert!(!has_window(Some(ContextUsage {
            tokens: Some(1_200),
            window: Some(0),
        })));
        assert!(has_window(Some(ContextUsage {
            tokens: None,
            window: Some(200_000),
        })));
    }

    #[test]
    fn missing_usage_is_distinct_from_zero_and_overflow() {
        let (used, remaining) = usage_lines(None);
        assert!(used.contains("not reported"));
        assert_eq!(remaining, "by this harness yet");
        let (used, remaining) = usage_lines(Some(ContextUsage {
            tokens: Some(0),
            window: Some(200),
        }));
        assert_eq!(used, "0 / 200 used");
        assert_eq!(remaining, "200 remaining");
        let (_, remaining) = usage_lines(Some(ContextUsage {
            tokens: Some(250),
            window: Some(200),
        }));
        assert_eq!(remaining, "0 remaining");
        let (used, remaining) = usage_lines(Some(ContextUsage {
            tokens: Some(10),
            window: Some(0),
        }));
        assert_eq!(used, "10 used");
        assert_eq!(remaining, "Context limit not reported");
    }

    #[test]
    fn token_counts_compact_to_k_and_m() {
        assert_eq!(compact_tokens(0), "0");
        assert_eq!(compact_tokens(999), "999");
        assert_eq!(compact_tokens(5417), "5.4k");
        assert_eq!(compact_tokens(11_731), "11.7k");
        assert_eq!(compact_tokens(200_000), "200k");
        assert_eq!(compact_tokens(562_285), "562k");
        assert_eq!(compact_tokens(1_000_000), "1m");
        assert_eq!(compact_tokens(1_048_576), "1m");
        let (used, remaining) = usage_lines(Some(ContextUsage {
            tokens: Some(5417),
            window: Some(1_048_576),
        }));
        assert_eq!(used, "5.4k / 1m used");
        assert_eq!(remaining, "1m remaining");
    }
}
