//! The sidebar footer's Usage button (after t3code's Usage page). A
//! click opens a card above the footer with what the agents' API traffic
//! would cost at list prices: the total for a period, a daily spend chart,
//! and the split by provider and model. The numbers come from the agents'
//! local transcripts ([`crate::token_usage`]), scanned off the UI thread.
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Context, EventEmitter, IntoElement, Render, SharedString, Task, Window, div, prelude::*, px,
};

use crate::icons::{self, icon};
use crate::popover;
use crate::settings::widgets;
use crate::theme::Theme;
use crate::token_usage::{self, Period, Report, format_tokens, format_usd};

/// Opening the card again within this window reuses the last scan.
const RESCAN_AFTER: Duration = Duration::from_secs(20);
/// Footer button edge, matching the Pull requests and Settings buttons.
const BUTTON_SIZE: f32 = 28.0;
const CARD_WIDTH: f32 = 320.0;
const CHART_HEIGHT: f32 = 44.0;
const TOP_MODELS: usize = 5;

/// Kept so the shell's subscription compiles unchanged if events return.
pub enum SidebarUsageEvent {}

pub struct SidebarUsage {
    popup: popover::Popup<()>,
    period: Period,
    report: Option<Arc<Report>>,
    scanned_at: Option<Instant>,
    loading: bool,
    scan_task: Option<Task<()>>,
}

impl EventEmitter<SidebarUsageEvent> for SidebarUsage {}

fn provider_label(harness: zeron_proto::HarnessId) -> &'static str {
    zeron_proto::providers::provider_name(harness)
}

impl SidebarUsage {
    pub fn new(_state: gpui::Entity<crate::state::AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            popup: popover::Popup::default(),
            period: Period::Month,
            report: None,
            scanned_at: None,
            loading: false,
            scan_task: None,
        }
    }

    fn scan(&mut self, cx: &mut Context<Self>) {
        if self.loading
            || self
                .scanned_at
                .is_some_and(|at| at.elapsed() < RESCAN_AFTER)
        {
            return;
        }
        let Some(data_dir) = crate::settings::data_dir(cx) else {
            return;
        };
        self.loading = true;
        let scan = cx
            .background_executor()
            .spawn(async move { token_usage::scan(&data_dir) });
        self.scan_task = Some(cx.spawn(async move |this, cx| {
            let report = scan.await;
            this.update(cx, |usage, cx| {
                usage.report = Some(Arc::new(report));
                usage.scanned_at = Some(Instant::now());
                usage.loading = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.popup.take_press_was_open() || self.popup.is_open() {
            self.dismiss(cx);
            return;
        }
        self.scan(cx);
        self.popup.open(());
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.popup.begin_close() {
            popover::reap_popup(cx, |usage: &mut Self| &mut usage.popup);
        }
        cx.notify();
    }

    fn period_tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(7.0))
            .bg(theme.glass_hover().opacity(0.6))
            .children(Period::ALL.into_iter().map(|period| {
                let active = period == self.period;
                div()
                    .id(SharedString::from(format!("usage-period-{}", period.label())))
                    .flex_1()
                    .flex()
                    .justify_center()
                    .py(px(3.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_size(crate::typography::ui_rems(11.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if active { theme.text } else { theme.text_muted })
                    .when(active, |tab| tab.bg(theme.surface))
                    .when(!active, |tab| tab.hover(|tab| tab.text_color(theme.text)))
                    .on_click(cx.listener(move |usage, _, _, cx| {
                        usage.period = period;
                        cx.notify();
                    }))
                    .child(period.label())
            }))
    }

    fn chart(daily: &[(chrono::NaiveDate, f64)], theme: &Theme) -> gpui::Div {
        let peak = daily.iter().map(|(_, usd)| *usd).fold(0.0_f64, f64::max);
        div()
            .h(px(CHART_HEIGHT))
            .flex()
            .items_end()
            .gap(px(2.0))
            .children(daily.iter().map(|(day, usd)| {
                let fraction = if peak > 0.0 { (usd / peak) as f32 } else { 0.0 };
                let height = if *usd > 0.0 {
                    (fraction * CHART_HEIGHT).max(2.0)
                } else {
                    1.0
                };
                div()
                    .id(SharedString::from(format!("usage-day-{day}")))
                    .flex_1()
                    .h(px(height))
                    .rounded_t(px(2.0))
                    .bg(if *usd > 0.0 {
                        theme.accent.opacity(0.35 + 0.55 * fraction)
                    } else {
                        theme.border.opacity(0.6)
                    })
                    .tooltip(widgets::text_tooltip(format!(
                        "{} · {}",
                        day.format("%b %-d"),
                        format_usd(*usd)
                    )))
            }))
    }

    fn line(theme: &Theme, label: SharedString, sub: Option<SharedString>, cost: f64, share: f32) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(theme.text)
                            .child(label),
                    )
                    .children(sub.map(|sub| {
                        div()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .text_size(crate::typography::ui_rems(11.0))
                            .child(sub)
                    }))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(SharedString::from(format_usd(cost))),
                    ),
            )
            .child(
                div()
                    .h(px(3.0))
                    .w_full()
                    .rounded_full()
                    .bg(theme.border.opacity(0.5))
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .w(gpui::relative(share.clamp(0.015, 1.0)))
                            .bg(theme.accent.opacity(0.8)),
                    ),
            )
    }

    fn card(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = &Theme::of(cx).for_popup().clone();
        let today = chrono::Local::now().date_naive();
        let summary = self
            .report
            .as_ref()
            .map(|report| token_usage::summarize(&report.buckets, &report.rates, self.period, today));
        let has_rates = self.report.as_ref().is_some_and(|report| report.has_rates);
        let section = |label: &'static str| {
            div()
                .mt(px(4.0))
                .text_size(crate::typography::ui_rems(11.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted.opacity(0.8))
                .child(label)
        };
        let mut body = div().px(px(8.0)).pb(px(6.0)).flex().flex_col().gap(px(8.0));
        match &summary {
            None => {
                body = body.child(
                    div()
                        .py(px(10.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child("Reading agent logs…"),
                );
            }
            Some(summary) => {
                let tokens = summary.tokens;
                body = body
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(24.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(SharedString::from(format_usd(summary.cost_usd))),
                            )
                            .child(
                                div()
                                    .pb(px(4.0))
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(format!(
                                        "{} tokens · {} requests",
                                        format_tokens(tokens.total()),
                                        summary.requests
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(10.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!("In {}", format_tokens(tokens.input))))
                            .child(SharedString::from(format!(
                                "Cache {}",
                                format_tokens(tokens.cache_read + tokens.cache_write)
                            )))
                            .child(SharedString::from(format!("Out {}", format_tokens(tokens.output)))),
                    )
                    .child(Self::chart(&summary.daily, theme));
                if summary.by_provider.is_empty() {
                    body = body.child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child("No agent traffic in this period."),
                    );
                } else {
                    let total = summary.cost_usd.max(f64::EPSILON);
                    body = body.child(section("Providers")).children(
                        summary.by_provider.iter().map(|(harness, line)| {
                            Self::line(
                                theme,
                                provider_label(*harness).into(),
                                Some(format_tokens(line.tokens).into()),
                                line.cost_usd,
                                (line.cost_usd / total) as f32,
                            )
                        }),
                    );
                    body = body.child(section("Top models")).children(
                        summary
                            .by_model
                            .iter()
                            .take(TOP_MODELS)
                            .map(|(model, _, line)| {
                                Self::line(
                                    theme,
                                    SharedString::from(model.clone()),
                                    Some(format_tokens(line.tokens).into()),
                                    line.cost_usd,
                                    (line.cost_usd / total) as f32,
                                )
                            }),
                    );
                }
                let note: Option<SharedString> = if !has_rates {
                    Some("Prices unavailable offline: only costs the agents logged are counted.".into())
                } else if summary.unpriced_tokens > 0 {
                    Some(
                        format!(
                            "{} tokens from models without a public price aren't counted.",
                            format_tokens(summary.unpriced_tokens)
                        )
                        .into(),
                    )
                } else {
                    None
                };
                body = body.child(
                    div()
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.text_muted.opacity(0.75))
                        .child(note.unwrap_or_else(|| {
                            "API list prices (LiteLLM) for Claude Code, Codex and Pi logs. Subscriptions bill differently.".into()
                        })),
                );
            }
        }
        popover::popover_card(theme)
            .w(px(CARD_WIDTH))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(popover::menu_heading(theme, "Usage"))
                    .when(self.loading && self.report.is_some(), |row| {
                        row.child(
                            div()
                                .pr(px(8.0))
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child("Updating…"),
                        )
                    }),
            )
            .child(div().px(px(8.0)).child(self.period_tabs(theme, cx)))
            .child(body)
    }
}

impl Render for SidebarUsage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let open = self.popup.is_open();
        let mut button = div()
            .id("sidebar-usage")
            .debug_selector(|| "sidebar-usage".into())
            .role(gpui::Role::Button)
            .aria_label("Usage")
            .aria_expanded(open)
            .tab_index(0)
            .focus_visible(|style| style.border_2().border_color(theme.accent))
            .relative()
            .size(px(BUTTON_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .cursor_pointer()
            .bg(if open {
                theme.glass_hover()
            } else {
                crate::motion::hover_blend(
                    "sidebar-usage",
                    theme.glass_hover().opacity(0.0),
                    theme.glass_hover(),
                )
            })
            .on_hover(crate::motion::hover_listener("sidebar-usage"))
            .when(!open, |el| el.tooltip(widgets::text_tooltip("Usage")))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|usage, _, _, _| usage.popup.note_trigger_press()),
            )
            .on_click(cx.listener(|usage, _, _, cx| usage.toggle(cx)))
            .child(icon(icons::CHART_BARS).size(px(15.0)).text_color(if open {
                theme.text
            } else {
                crate::motion::hover_blend("sidebar-usage", theme.text_muted, theme.text)
            }));
        if self.popup.get().is_some() {
            let card = self
                .card(cx)
                .on_mouse_down_out(cx.listener(|usage, _, _, cx| usage.dismiss(cx)))
                .into_any_element();
            button = button.child(popover::anchored_menu_above(
                "sidebar-usage-menu",
                card,
                self.popup.closing_since(),
            ));
        }
        button
    }
}
