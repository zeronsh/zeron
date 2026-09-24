//! Context occupancy is read from the replicated chat snapshot, never local CLI state.
use crate::{
    i18n::{self, Locale, MessageId},
    theme::Theme,
};
use gpui::{
    Context, IntoElement, PathBuilder, Render, SharedString, Window, canvas, div, point,
    prelude::*, px,
};
use zeron_proto::ContextUsage;

pub fn render(
    usage: Option<ContextUsage>,
    state: gpui::Entity<crate::state::AppState>,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let fraction = usage.and_then(ContextUsage::fraction);
    let color = match fraction {
        Some(f) if f >= 0.9 => theme.danger,
        Some(f) if f >= 0.75 => theme.warning,
        Some(_) => theme.text_muted,
        None => theme.text_faint,
    };
    let track = theme.text_faint.opacity(0.25);
    let ring = canvas(
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
            arc(fraction.unwrap_or(0.0).clamp(0.0, 1.0) as f32, color);
        },
    )
    .size(px(16.0));
    let label = fraction
        .map(|f| format!("{:.0}%", f * 100.0))
        .unwrap_or_else(|| "—".into());
    div()
        .id("context-usage")
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .h(px(24.0))
        .px(px(6.0))
        .rounded(px(6.0))
        .text_size(px(11.0))
        .text_color(color)
        .hover(|s| s.bg(crate::theme::ink(0.05)))
        .child(ring)
        .child(SharedString::from(label))
        .tooltip(move |_, cx| {
            cx.new(|cx| UsageCard {
                _subscription: cx.observe(&state, |_, _, cx| cx.notify()),
                state: state.clone(),
            })
            .into()
        })
}

struct UsageCard {
    state: gpui::Entity<crate::state::AppState>,
    _subscription: gpui::Subscription,
}

fn with_separators(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// whether the indicator has anything to measure against: harnesses that
/// never report a window (antigravity) get no indicator at all, rather than a
/// permanently empty ring.
pub fn has_window(usage: Option<ContextUsage>) -> bool {
    usage
        .and_then(|usage| usage.window)
        .is_some_and(|window| window > 0)
}

fn details(usage: Option<ContextUsage>, locale: Locale) -> gpui::SharedString {
    match usage.unwrap_or_default() {
        ContextUsage {
            tokens: Some(tokens),
            window: Some(window),
        } if window > 0 => i18n::fill_many(
            MessageId::ContextUsageRemaining,
            &[
                ("{used}", &with_separators(tokens)),
                ("{total}", &with_separators(window)),
                (
                    "{remaining}",
                    &with_separators(window.saturating_sub(tokens)),
                ),
            ],
            locale,
        )
        .into(),
        ContextUsage {
            tokens: Some(tokens),
            ..
        } => i18n::fill(
            MessageId::ContextUsageUsed,
            "{used}",
            &with_separators(tokens),
            locale,
        )
        .into(),
        ContextUsage {
            window: Some(window),
            ..
        } if window > 0 => i18n::fill(
            MessageId::ContextUsageWaiting,
            "{capacity}",
            &with_separators(window),
            locale,
        )
        .into(),
        _ => i18n::translate(MessageId::ContextUsageNotReported, locale).into(),
    }
}

impl Render for UsageCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &Theme::of(cx).for_popup();
        let card = crate::popover::popover_card(theme)
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(i18n::translate(
                        MessageId::ContextUsageTitle,
                        i18n::locale(cx),
                    )),
            )
            .child(
                // the lines break only at their own newlines: a tooltip sizes
                // from the unwrapped text, so soft wrapping clipped the last line
                div()
                    .text_size(px(12.0))
                    .line_height(px(19.0))
                    .whitespace_nowrap()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(details(
                        self.state.read(cx).context_usage,
                        i18n::locale(cx),
                    ))),
            );
        crate::frost::frosted(crate::popover::CARD_RADIUS, crate::frost::MENU_BLUR, card)
    }
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
        assert!(details(None, Locale::En).contains("not reported"));
        assert!(
            details(
                Some(ContextUsage {
                    tokens: Some(0),
                    window: Some(200)
                }),
                Locale::En
            )
            .contains("200 tokens remaining")
        );
        assert!(
            details(
                Some(ContextUsage {
                    tokens: Some(250),
                    window: Some(200)
                }),
                Locale::En
            )
            .contains("0 tokens remaining")
        );
        assert!(
            details(
                Some(ContextUsage {
                    tokens: Some(10),
                    window: Some(0)
                }),
                Locale::En
            )
            .contains("limit not reported")
        );
    }

    #[test]
    fn every_detail_branch_reads_its_template_in_both_locales() {
        let remaining = ContextUsage {
            tokens: Some(5417),
            window: Some(1_048_576),
        };
        assert_eq!(
            details(Some(remaining), Locale::En),
            "5,417 / 1,048,576 tokens\n1,043,159 tokens remaining"
        );
        assert_eq!(
            details(Some(remaining), Locale::ZhCn),
            "5,417 / 1,048,576 tokens\n剩余 1,043,159 tokens"
        );
        assert_ne!(
            details(Some(remaining), Locale::ZhCn),
            details(Some(remaining), Locale::En)
        );

        let used = ContextUsage {
            tokens: Some(1200),
            window: None,
        };
        assert_eq!(
            details(Some(used), Locale::En),
            "1,200 tokens used\nContext limit not reported"
        );
        assert_eq!(
            details(Some(used), Locale::ZhCn),
            "已使用 1,200 tokens\n未上报上下文上限"
        );

        let waiting = ContextUsage {
            tokens: None,
            window: Some(200_000),
        };
        assert_eq!(
            details(Some(waiting), Locale::En),
            "200,000 token capacity\nWaiting for context usage"
        );
        assert_eq!(
            details(Some(waiting), Locale::ZhCn),
            "200,000 tokens 容量\n正在等待上下文用量"
        );

        assert_eq!(
            details(None, Locale::En),
            "Context usage not reported by this harness yet"
        );
        assert_eq!(details(None, Locale::ZhCn), "此 Harness 尚未上报上下文用量");
    }

    #[test]
    fn token_counts_are_grouped_by_thousands() {
        assert_eq!(with_separators(0), "0");
        assert_eq!(with_separators(999), "999");
        assert_eq!(with_separators(5417), "5,417");
        assert_eq!(with_separators(1_048_576), "1,048,576");
        assert_eq!(
            with_separators(1_043_159),
            "1,043,159",
            "the remaining count is formatted, not translated"
        );
    }
}
