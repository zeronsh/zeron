//! Presentation helpers for provider-account usage.
//!
//! These are the pieces the composer footer's account chip and its card paint
//! with — window selection, labels, the meter bar, the brand mark. All pure,
//! or pure given a theme. The elements live in [`crate::pickers`] with the
//! other footer chips.
//!
//! The snapshot they read comes from [`crate::state::AppState::agent_accounts`],
//! shared with Settings → Accounts rather than fetched twice.

use chrono::{DateTime, Utc};
use gpui::{AnyElement, Hsla, SharedString, div, prelude::*, px};

use zeron_proto::{AgentAccount, AgentUsageWindow, HarnessId};

use crate::settings::accounts::{usage_color, usage_level};
use crate::theme::Theme;

/// The one window worth the chip: the short rolling window when the plan has
/// one, else whatever the plan does report.
///
/// Claude bills a 5-hour window ("Session") and a 7-day one ("Week"); the
/// engine emits them in that order and only for keys the plan actually returns
/// (`engine/src/agent_accounts.rs`), so a weekly-only plan yields "Week" alone
/// and that becomes the honest headline. Matching the label rather than taking
/// the first states the intent and survives a reordering upstream.
///
/// Note this is deliberately *not* the tightest window: a nearly-spent weekly
/// cap sits behind one click in the card rather than on the chip. Pure.
pub fn headline_window(windows: &[AgentUsageWindow]) -> Option<&AgentUsageWindow> {
    windows
        .iter()
        .find(|w| w.label.eq_ignore_ascii_case("session"))
        .or_else(|| windows.first())
}

/// Display name for an account: the email it signed in with, else whatever the
/// provider gave us. Pure.
pub fn account_label(account: &AgentAccount) -> SharedString {
    account
        .email
        .clone()
        .or_else(|| account.display_name.clone())
        .unwrap_or_else(|| "Unknown account".into())
        .into()
}

/// What is left of a window, from the fraction *used* — the reading people
/// actually act on ("how much have I got?"), not the one providers report.
///
/// Rounds the remainder itself rather than subtracting a rounded used-percent:
/// both are defensible, but this one is computed from the same clamped value
/// the meter draws, so bar and label can never disagree. Pure.
pub fn remaining_label(used_fraction: f32) -> SharedString {
    let remaining = 1.0 - used_fraction.clamp(0.0, 1.0);
    format!("{}% left", (remaining * 100.0).round() as u32).into()
}

/// Colour for a usage fraction at this call site.
pub fn level_color(fraction: f32, theme: &Theme) -> Hsla {
    usage_color(usage_level(fraction), theme)
}

/// A meter bar, taking the fraction **used** and drawing what is **left** —
/// it drains as the window is spent, matching the label beside it. Colour
/// still keys off usage, so a nearly-empty bar is the red one.
///
/// Callers size it: the card's meters want a wider bar than a switch row.
pub fn meter(used_fraction: f32, height: f32, theme: &Theme) -> AnyElement {
    let used = used_fraction.clamp(0.0, 1.0);
    let remaining = 1.0 - used;
    div()
        .flex_1()
        .h(px(height))
        .rounded_full()
        .overflow_hidden()
        .bg(crate::theme::ink(0.07))
        .when(remaining > 0.0, |el| {
            el.child(
                div()
                    // A 1.5% floor keeps a nearly-exhausted window showing a
                    // sliver rather than reading as an empty track.
                    .w(gpui::relative(remaining.max(0.015)))
                    .h_full()
                    .rounded_full()
                    .bg(level_color(used, theme).opacity(0.85)),
            )
        })
        .into_any_element()
}

pub fn brand_mark(harness: HarnessId, size: f32, theme: &Theme) -> AnyElement {
    let (mark, tint) = crate::pickers::harness_brand_icon(harness);
    crate::icons::icon(mark)
        .size(px(size))
        .text_color(tint.unwrap_or(theme.text_muted))
        .into_any_element()
}

/// `"Session · resets 8:29 PM"` — the sub-line under a headline percentage.
/// Drops the reset clause when the provider did not give one. Pure given
/// `now`.
pub fn window_caption(window: &AgentUsageWindow, now: DateTime<Utc>) -> SharedString {
    match crate::settings::accounts::format_reset(window.resets_at, now) {
        Some(reset) => format!("{} · {reset}", window.label).into(),
        None => window.label.clone().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, used: f32) -> AgentUsageWindow {
        AgentUsageWindow {
            label: label.into(),
            used_fraction: used,
            resets_at: None,
        }
    }

    #[test]
    fn headline_prefers_the_session_window() {
        let windows = [window("Session", 0.07), window("Week", 0.99)];
        // The calm number wins on the row; the 99% is one click away.
        assert_eq!(headline_window(&windows).unwrap().label, "Session");
    }

    #[test]
    fn headline_falls_back_to_a_weekly_only_plan() {
        let windows = [window("Week", 0.42)];
        assert_eq!(headline_window(&windows).unwrap().label, "Week");
    }

    #[test]
    fn headline_matches_the_label_regardless_of_order_or_case() {
        let windows = [window("Week", 0.99), window("session", 0.07)];
        assert_eq!(headline_window(&windows).unwrap().used_fraction, 0.07);
    }

    #[test]
    fn headline_is_none_without_windows() {
        assert!(headline_window(&[]).is_none());
    }

    #[test]
    fn remaining_label_inverts_usage() {
        assert_eq!(remaining_label(0.0), "100% left");
        assert_eq!(remaining_label(0.07), "93% left");
        assert_eq!(remaining_label(0.99), "1% left");
        assert_eq!(remaining_label(1.0), "0% left");
    }

    #[test]
    fn remaining_label_agrees_with_the_meter_it_labels() {
        // Both read the same clamped remainder, so the bar and the text can
        // never round to different stories.
        for used in [0.0f32, 0.005, 0.26, 0.735, 0.999, 1.0] {
            let shown: u32 = remaining_label(used)
                .trim_end_matches("% left")
                .parse()
                .expect("label is `<n>% left`");
            let drawn = ((1.0 - used.clamp(0.0, 1.0)) * 100.0).round() as u32;
            assert_eq!(shown, drawn, "used={used}");
        }
    }

    #[test]
    fn remaining_label_clamps_an_over_reporting_provider() {
        assert_eq!(remaining_label(1.2), "0% left");
        assert_eq!(remaining_label(-0.1), "100% left");
    }

    #[test]
    fn window_caption_drops_a_missing_reset() {
        assert_eq!(window_caption(&window("Week", 0.1), Utc::now()), "Week");
    }
}
