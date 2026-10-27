//! Settings → Billing: the credits that run Cloud sessions — what's left
//! (this period's plan credits and credits that never expire), the monthly
//! plans, and one-off credit packs. Purchases and plan changes happen on
//! Stripe's own pages in the browser; while one is open the page re-reads
//! the balance until the change lands, then says so.
//!
//! Every call is local IPC (`BillingStatus`, `BillingCheckout`,
//! `BillingPortal`); this device's engine talks to the edge.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use zeron_proto::{BillingLink, BillingPlan, BillingStatus};
use zeron_rpc::methods;

use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// How often an open Checkout or portal page is checked on.
const POLL_EVERY: Duration = Duration::from_secs(3);
/// Stop checking after this long (the page was likely abandoned).
const POLL_FOR: Duration = Duration::from_secs(10 * 60);
/// Most packs one purchase buys (the edge's cap).
const MAX_PACKS: u32 = 20;

/// The last status this page read (and whose account it was), so a later
/// visit paints at once — never another account's balance.
#[derive(Default)]
struct BillingCache(Option<(String, BillingStatus)>);

impl gpui::Global for BillingCache {}

/// What a Stripe page opened from here may change.
#[derive(Clone, PartialEq)]
struct Snapshot {
    total: f64,
    extra: f64,
    plan: Option<String>,
    cancel_at_period_end: bool,
}

impl Snapshot {
    fn of(status: &BillingStatus) -> Self {
        Self {
            total: status.credits.total,
            extra: status.credits.extra,
            plan: status.plan.as_ref().map(|p| p.plan.id.clone()),
            cancel_at_period_end: status.plan.as_ref().is_some_and(|p| p.cancel_at_period_end),
        }
    }
}

/// A Checkout or portal page is open in the browser.
struct Pending {
    before: Snapshot,
    since: Instant,
}

/// Which button is waiting on the engine.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Plan(usize),
    Credits,
    Portal,
}

pub struct BillingPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    status: Loadable<BillingStatus>,
    packs: u32,
    busy: Option<Busy>,
    pending: Option<Pending>,
    error: Option<SharedString>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    _observe: Subscription,
}

impl BillingPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        let account = state.read(cx).auth_user().map(|user| user.id.clone());
        let status = cx
            .try_global::<BillingCache>()
            .and_then(|cache| cache.0.clone())
            .filter(|(owner, _)| Some(owner) == account.as_ref())
            .map_or(Loadable::Idle, |(_, status)| Loadable::Ready(status));
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            status,
            packs: 1,
            busy: None,
            pending: None,
            error: None,
            load_task: None,
            action_task: None,
            poll_task: None,
            _observe: observe,
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.status = Loadable::Error("Engine not connected".into());
            return;
        };
        if !matches!(self.status, Loadable::Ready(_)) {
            self.status = Loadable::Loading;
        }
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<BillingStatus>(methods::BILLING_STATUS, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(status) => page.apply(status, cx),
                    Err(error) if matches!(page.status, Loadable::Ready(_)) => {
                        page.error = Some(error.to_string().into());
                    }
                    Err(error) => page.status = Loadable::Error(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Take a fresh status; when a Stripe page was open and something it
    /// could change did, say what happened and stop checking.
    fn apply(&mut self, status: BillingStatus, cx: &mut Context<Self>) {
        if let Some(account) = self.state.read(cx).auth_user().map(|user| user.id.clone()) {
            cx.default_global::<BillingCache>().0 = Some((account, status.clone()));
        }
        if let Some(pending) = &self.pending {
            let now = Snapshot::of(&status);
            if now != pending.before {
                if let Some(message) = change_message(&pending.before, &now, &status) {
                    crate::toast::show(cx, crate::toast::ToastKind::Success, message);
                }
                self.pending = None;
                self.poll_task = None;
                // The composer's Cloud gating reads the balance from here.
                self.state.update(cx, |state, cx| state.refresh_cloud(cx));
            }
        }
        self.status = Loadable::Ready(status);
    }

    /// Open a Stripe page (`BillingCheckout` / `BillingPortal`) in the
    /// browser and watch for what it changes.
    fn open(
        &mut self,
        busy: Busy,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.busy = Some(busy);
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call_as::<BillingLink>(method, params).await;
            this.update(cx, |page, cx| {
                page.busy = None;
                match result {
                    Ok(link) => {
                        cx.open_url(&link.url);
                        page.watch(cx);
                    }
                    Err(error) => page.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(status) = self.status.ready() else {
            return;
        };
        self.pending = Some(Pending {
            before: Snapshot::of(status),
            since: Instant::now(),
        });
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_EVERY).await;
                let keep = this
                    .update(cx, |page, cx| {
                        let alive = page
                            .pending
                            .as_ref()
                            .is_some_and(|p| p.since.elapsed() < POLL_FOR);
                        if alive {
                            page.load(cx);
                        } else {
                            page.pending = None;
                            cx.notify();
                        }
                        alive
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn stop_watching(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        self.poll_task = None;
        cx.notify();
    }

    fn subscribe(&mut self, ix: usize, plan: String, cx: &mut Context<Self>) {
        self.open(
            Busy::Plan(ix),
            methods::BILLING_CHECKOUT,
            serde_json::json!({ "kind": "subscription", "plan": plan }),
            cx,
        );
    }

    fn buy_credits(&mut self, cx: &mut Context<Self>) {
        let packs = self.packs;
        self.open(
            Busy::Credits,
            methods::BILLING_CHECKOUT,
            serde_json::json!({ "kind": "credits", "packs": packs }),
            cx,
        );
    }

    fn manage(&mut self, busy: Busy, cx: &mut Context<Self>) {
        self.open(busy, methods::BILLING_PORTAL, serde_json::json!({}), cx);
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

/// What landed, in a sentence, or `None` when it isn't worth a toast.
fn change_message(before: &Snapshot, now: &Snapshot, status: &BillingStatus) -> Option<String> {
    if now.plan != before.plan {
        return Some(match (&now.plan, status.plan.as_ref()) {
            (Some(_), Some(plan)) => format!(
                "You're on {} — {} a month",
                plan.plan.name,
                crate::cloud::format_credits(plan.plan.monthly_credits)
            ),
            _ => "Your plan has ended".to_string(),
        });
    }
    if now.cancel_at_period_end != before.cancel_at_period_end {
        return Some(if now.cancel_at_period_end {
            "Your plan won't renew".to_string()
        } else {
            "Your plan will renew".to_string()
        });
    }
    let added = now.extra - before.extra;
    (added >= 1.0).then(|| format!("Added {}", crate::cloud::format_credits(added)))
}

/// `$20` (whole dollars), `$12.50` otherwise.
pub fn format_price(cents: u32) -> String {
    if cents % 100 == 0 {
        format!("${}", cents / 100)
    } else {
        format!("${}.{:02}", cents / 100, cents % 100)
    }
}

/// "Nov 5" in the local calendar.
fn short_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%b %-d")
                .to_string()
        })
        .unwrap_or_default()
}

/// "about 750 hours" of the default machine.
fn hours(credits: f64) -> String {
    format!(
        "about {} hours",
        crate::cloud::group_number((credits / 60.0).round())
    )
}

impl popover::ScrollRailHost for BillingPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

fn caption(theme: &Theme, copy: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
        .text_color(theme.text_muted)
        .child(copy.into())
}

/// Geist's ascent and descent per em. A line box puts its baseline
/// `(line_height - ascent + descent) / 2` above its bottom edge.
const FONT_ASCENT: f32 = 1.005;
const FONT_DESCENT: f32 = 0.295;

/// The line height that puts a `size` caption's baseline as far above its
/// box's bottom as a `figure`-sized line with a line height of one em.
fn unit_line_height(figure: f32, size: f32) -> f32 {
    figure - (FONT_ASCENT - FONT_DESCENT) * (figure - size)
}

/// A large figure with its small unit after it, on one baseline. Both line
/// boxes end together and the unit's line height lands its baseline on the
/// figure's; flex baseline alignment doesn't line up text of different sizes.
/// More captions on the same baseline can follow via [`figure_caption`].
fn figure_row(theme: &Theme, size: f32, figure: String, unit: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_end()
        .gap(px(6.0))
        .child(
            div()
                .text_size(crate::typography::ui_rems(size))
                .line_height(crate::typography::ui_rems(size))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(figure),
        )
        .child(figure_caption(theme, size, unit))
}

/// A caption sitting on a [`figure_row`]'s baseline.
fn figure_caption(theme: &Theme, figure: f32, copy: impl Into<SharedString>) -> gpui::Div {
    let size = widgets::ROW_DESCRIPTION_SIZE;
    caption(theme, copy).line_height(crate::typography::ui_rems(unit_line_height(figure, size)))
}

fn button(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    tone: widgets::ActionTone,
    label: impl Into<SharedString>,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let accent = theme.accent;
    widgets::text_action(theme, tone, label)
        .id(id)
        .justify_center()
        .tab_index(0)
        .role(gpui::Role::Button)
        .focus_visible(move |s| s.border_2().border_color(accent).opacity(1.0))
        .when(!enabled, |el| el.opacity(0.5).cursor_default())
}

impl BillingPage {
    /// The balance: one number and the time it buys, a quiet bar split into
    /// plan and extra credits, then a row for each kind.
    fn render_credits(&self, status: &BillingStatus, theme: &Theme) -> AnyElement {
        let credits = &status.credits;
        let total = credits.total.max(0.0);
        let monthly = credits.monthly.max(0.0);
        let extra = credits.extra.max(0.0);
        let parts = (monthly + extra).max(1.0);
        let bar = (monthly + extra > 0.0).then(|| {
            div()
                .mt(px(14.0))
                .h(px(4.0))
                .w_full()
                .rounded_full()
                .bg(theme.wash(0.08))
                .flex()
                .flex_row()
                .overflow_hidden()
                .when(monthly > 0.0, |el| {
                    el.child(
                        div()
                            .h_full()
                            .w(gpui::relative((monthly / parts) as f32))
                            .bg(theme.text.opacity(0.7)),
                    )
                })
                .when(extra > 0.0, |el| {
                    el.child(
                        div()
                            .h_full()
                            .w(gpui::relative((extra / parts) as f32))
                            .bg(theme.text.opacity(0.3)),
                    )
                })
        });
        let summary = div()
            .px(px(16.0))
            .py(px(16.0))
            .flex()
            .flex_col()
            .child(
                figure_row(
                    theme,
                    26.0,
                    crate::cloud::group_number(total.floor()),
                    if total == 1.0 { "credit" } else { "credits" },
                )
                .child(div().flex_1())
                .child(figure_caption(
                    theme,
                    26.0,
                    if total > 0.0 {
                        hours(total)
                    } else {
                        "Choose a plan or buy credits to run Cloud".to_string()
                    },
                )),
            )
            .children(bar);
        let line = |title: &'static str, detail: String, value: String| {
            widgets::card_row(theme, false)
                .min_h(px(52.0))
                .py(px(10.0))
                .flex_nowrap()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(theme, title))
                        .child(caption(theme, detail)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text)
                        .child(value),
                )
        };
        let (plan_detail, plan_value) = match (credits.monthly_total > 0.0, credits.monthly_ends_at)
        {
            (true, Some(ends)) => (
                format!(
                    "Of {} · resets {}",
                    crate::cloud::group_number(credits.monthly_total),
                    short_date(ends)
                ),
                crate::cloud::group_number(monthly.floor()),
            ),
            (true, None) => (
                format!("Of {}", crate::cloud::group_number(credits.monthly_total)),
                crate::cloud::group_number(monthly.floor()),
            ),
            (false, _) => (
                "No plan yet — choose one below".to_string(),
                "—".to_string(),
            ),
        };
        let block = widgets::section_card(theme)
            .mt(px(0.0))
            .child(summary)
            .child(line("Plan credits", plan_detail, plan_value))
            .child(line(
                "Extra credits",
                "Never expire · used once plan credits run out".to_string(),
                crate::cloud::group_number(extra.floor()),
            ));
        widgets::section(theme, "Credits", block).into_any_element()
    }

    /// The three plans side by side, the current one marked; on a plan,
    /// switching and canceling go through Stripe's portal.
    fn render_plans(
        &self,
        status: &BillingStatus,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = status.plan.as_ref();
        let idle = self.busy.is_none() && status.available;
        let cards = status.plans.iter().enumerate().map(|(ix, plan)| {
            let is_current = current.is_some_and(|c| c.plan.id == plan.id);
            let busy = self.busy == Some(Busy::Plan(ix));
            let action: AnyElement = if is_current {
                button(
                    theme,
                    ("plan-current", ix),
                    widgets::ActionTone::Filled,
                    "Current plan",
                    false,
                )
                .w_full()
                .into_any_element()
            } else if current.is_some() {
                button(
                    theme,
                    ("plan-switch", ix),
                    widgets::ActionTone::Filled,
                    if busy { "Opening…" } else { "Switch" },
                    idle,
                )
                .w_full()
                .when(idle, |el| {
                    el.on_click(cx.listener(move |this, _, _, cx| this.manage(Busy::Plan(ix), cx)))
                })
                .into_any_element()
            } else {
                let id = plan.id.clone();
                button(
                    theme,
                    ("plan-subscribe", ix),
                    if ix == 0 {
                        widgets::ActionTone::Solid
                    } else {
                        widgets::ActionTone::Filled
                    },
                    if busy {
                        "Opening…".to_string()
                    } else {
                        format!("Choose {}", plan.name)
                    },
                    idle,
                )
                .w_full()
                .when(idle, |el| {
                    el.on_click(
                        cx.listener(move |this, _, _, cx| this.subscribe(ix, id.clone(), cx)),
                    )
                })
                .into_any_element()
            };
            plan_card(theme, plan, is_current, action)
        });
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(div().flex().flex_row().gap(px(12.0)).children(cards));
        if let Some(plan) = current {
            let (detail, color) = if plan.status == "past_due" {
                (
                    "Your last payment didn't go through — update your card to keep your plan."
                        .to_string(),
                    theme.warning_muted,
                )
            } else if plan.cancel_at_period_end {
                (
                    format!(
                        "Ends {}. You keep its credits until then.",
                        short_date(plan.period_end)
                    ),
                    theme.text_muted,
                )
            } else {
                (
                    format!("Renews {}.", short_date(plan.period_end)),
                    theme.text_muted,
                )
            };
            let busy = self.busy == Some(Busy::Portal);
            let idle = self.busy.is_none();
            block = block.child(
                widgets::section_card(theme).mt(px(0.0)).child(
                    widgets::card_row(theme, true)
                        .min_h(px(52.0))
                        .py(px(10.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(160.0))
                                .child(widgets::row_title(
                                    theme,
                                    format!(
                                        "{} · {} a month",
                                        plan.plan.name,
                                        format_price(plan.plan.price_cents)
                                    ),
                                ))
                                .child(caption(theme, detail).text_color(color)),
                        )
                        .child(
                            button(
                                theme,
                                "billing-manage",
                                widgets::ActionTone::Quiet,
                                if busy {
                                    "Opening…"
                                } else {
                                    "Manage subscription"
                                },
                                idle,
                            )
                            .when(idle, |el| {
                                el.on_click(
                                    cx.listener(|this, _, _, cx| this.manage(Busy::Portal, cx)),
                                )
                            }),
                        ),
                ),
            );
        }
        widgets::section(theme, "Plan", block).into_any_element()
    }

    /// One-off packs: how many in one pill (− count +), then Buy.
    fn render_extra(
        &self,
        status: &BillingStatus,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pack = &status.pack;
        let packs = self.packs;
        let idle = self.busy.is_none() && status.available;
        let busy = self.busy == Some(Busy::Credits);
        let step = |id: &'static str,
                    glyph: &'static str,
                    enabled: bool,
                    delta: i32,
                    cx: &mut Context<Self>| {
            let accent = theme.accent;
            let label = if delta < 0 {
                "Fewer packs"
            } else {
                "More packs"
            };
            div()
                .id(id)
                .flex_none()
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.0))
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label(label)
                .tooltip(widgets::text_tooltip(label))
                .focus_visible(move |s| s.border_2().border_color(accent))
                .when(!enabled, |el| el.opacity(0.35))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(theme.wash(0.06)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.packs =
                                (this.packs as i32 + delta).clamp(1, MAX_PACKS as i32) as u32;
                            cx.notify();
                        }))
                })
                .child(
                    crate::icons::icon(glyph)
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                )
        };
        let stepper = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .h(px(32.0))
            .rounded(px(8.0))
            .bg(widgets::select_fill(theme, false))
            .child(step("pack-fewer", crate::icons::MINUS, packs > 1, -1, cx))
            .child(
                div()
                    .min_w(px(24.0))
                    .text_center()
                    .text_size(crate::typography::ui_rems(13.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(packs.to_string()),
            )
            .child(step(
                "pack-more",
                crate::icons::PLUS,
                packs < MAX_PACKS,
                1,
                cx,
            ));
        let credits = pack.credits * packs as f64;
        let block = widgets::section_card(theme).mt(px(0.0)).child(
            widgets::card_row(theme, true)
                .min_h(px(64.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(180.0))
                        .child(widgets::row_title(
                            theme,
                            crate::cloud::format_credits(credits),
                        ))
                        .child(caption(theme, format!("Never expire · {}", hours(credits)))),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(stepper)
                        .child(
                            button(
                                theme,
                                "billing-buy",
                                widgets::ActionTone::Filled,
                                if busy {
                                    "Opening…".to_string()
                                } else {
                                    format!("Buy for {}", format_price(pack.price_cents * packs))
                                },
                                idle,
                            )
                            .min_w(px(96.0))
                            .when(idle, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| this.buy_credits(cx)))
                            }),
                        ),
                ),
        );
        widgets::section(theme, "Extra credits", block).into_any_element()
    }

    /// Shown while a Stripe page is open: what we're waiting on, and a way
    /// to stop.
    fn render_pending(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.pending.as_ref()?;
        Some(
            div()
                .mt(px(16.0))
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(12.0))
                .bg(widgets::block_fill(theme))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(crate::loaders::ring_spinner(
                    "billing-pending",
                    12.0,
                    theme.text_muted,
                ))
                .child(
                    div()
                        .flex_1()
                        .text_size(crate::typography::ui_rems(12.5))
                        .text_color(theme.text)
                        .child("Finish in your browser — this page updates when it's done."),
                )
                .child(
                    button(
                        theme,
                        "billing-pending-stop",
                        widgets::ActionTone::Quiet,
                        "Dismiss",
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.stop_watching(cx))),
                )
                .into_any_element(),
        )
    }
}

fn plan_card(theme: &Theme, plan: &BillingPlan, current: bool, action: AnyElement) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .p(px(16.0))
        .rounded(px(12.0))
        .bg(widgets::block_fill(theme))
        .border_1()
        .border_color(if current {
            theme.text.opacity(0.25)
        } else {
            gpui::transparent_black()
        })
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(plan.name.clone()),
                )
                .when(current, |el| el.child(widgets::badge(theme, "Current"))),
        )
        .child(figure_row(theme, 24.0, format_price(plan.price_cents), "/ month").mt(px(4.0)))
        .child(
            div()
                .mt(px(4.0))
                .text_size(crate::typography::ui_rems(12.5))
                .text_color(theme.text)
                .child(format!(
                    "{} a month",
                    crate::cloud::format_credits(plan.monthly_credits)
                )),
        )
        .child(caption(theme, hours(plan.monthly_credits)))
        .child(div().mt(px(12.0)).child(action))
        .into_any_element()
}

impl Render for BillingPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let body: Vec<AnyElement> = match self.status.clone() {
            Loadable::Idle | Loadable::Loading => vec![
                widgets::section_card(&theme)
                    .mt(px(28.0))
                    .p(px(16.0))
                    .child(popover::skeleton_rows(
                        "billing-skeleton",
                        &theme,
                        3,
                        cx.entity_id(),
                        cx,
                    ))
                    .into_any_element(),
            ],
            Loadable::Error(message) => vec![
                widgets::error_strip(&theme, format!("Couldn't load billing: {message}"))
                    .id("billing-retry")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.status = Loadable::Idle;
                        this.load(cx);
                    }))
                    .into_any_element(),
            ],
            Loadable::Ready(status) => {
                let mut sections = Vec::new();
                if !status.available {
                    sections.push(
                        widgets::warning_strip(
                            &theme,
                            "Billing isn't set up on this server yet, so plans and credits can't be bought here.",
                        )
                        .into_any_element(),
                    );
                }
                sections.extend(self.render_pending(&theme, cx));
                sections.push(self.render_credits(&status, &theme));
                sections.push(self.render_plans(&status, &theme, cx));
                sections.push(self.render_extra(&status, &theme, cx));
                sections
            }
        };
        let scrollbar = popover::rail(self, "billing-page-scrollbar", &theme, cx);
        div()
            .id("billing-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("billing-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "Billing", None))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    "Credits run your Cloud sessions.",
                                ))
                                .when_some(self.error.clone(), |el, message| {
                                    el.child(
                                        widgets::error_strip(&theme, message)
                                            .id("billing-error")
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.error = None;
                                                cx.notify();
                                            })),
                                    )
                                })
                                .children(body),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{BillingCredits, BillingPack, BillingSubscription};

    fn status(plan: Option<&str>, extra: f64, cancel: bool) -> BillingStatus {
        let plans = vec![
            BillingPlan {
                id: "base".into(),
                name: "Base".into(),
                price_cents: 2000,
                monthly_credits: 45000.0,
            },
            BillingPlan {
                id: "pro".into(),
                name: "Pro".into(),
                price_cents: 5000,
                monthly_credits: 112500.0,
            },
            BillingPlan {
                id: "max".into(),
                name: "Max".into(),
                price_cents: 20000,
                monthly_credits: 450000.0,
            },
        ];
        BillingStatus {
            available: true,
            plan: plan.map(|id| BillingSubscription {
                plan: plans.iter().find(|p| p.id == id).unwrap().clone(),
                status: "active".into(),
                period_end: 1_800_000_000_000,
                cancel_at_period_end: cancel,
            }),
            credits: BillingCredits {
                monthly: 30000.0,
                monthly_total: 45000.0,
                monthly_ends_at: Some(1_800_000_000_000),
                extra,
                total: 30000.0 + extra,
            },
            plans,
            pack: BillingPack {
                price_cents: 1000,
                credits: 12000.0,
            },
            has_customer: plan.is_some(),
        }
    }

    #[test]
    fn prices_and_changes_read_naturally() {
        assert_eq!(format_price(2000), "$20");
        assert_eq!(format_price(1250), "$12.50");
        assert_eq!(hours(45000.0), "about 750 hours");
        let before = status(None, 0.0, false);
        let bought = status(None, 24000.0, false);
        assert_eq!(
            change_message(&Snapshot::of(&before), &Snapshot::of(&bought), &bought).as_deref(),
            Some("Added 24,000 credits")
        );
        let pro = status(Some("pro"), 0.0, false);
        assert_eq!(
            change_message(&Snapshot::of(&before), &Snapshot::of(&pro), &pro).as_deref(),
            Some("You're on Pro — 112,500 credits a month")
        );
        let canceling = status(Some("pro"), 0.0, true);
        assert_eq!(
            change_message(&Snapshot::of(&pro), &Snapshot::of(&canceling), &canceling).as_deref(),
            Some("Your plan won't renew")
        );
    }

    /// Every state paints: loading, unavailable, no plan, on a plan
    /// (renewing, ending, past due), with a Stripe page open.
    #[gpui::test]
    fn every_state_renders(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let state = cx.new(|_| AppState::new());
        let (page, cx) = cx.add_window_view(|_, cx| BillingPage::new(state.clone(), cx));
        cx.update(|window, cx| window.draw(cx).clear());
        let variants = [
            BillingStatus {
                available: false,
                ..status(None, 0.0, false)
            },
            status(None, 0.0, false),
            status(Some("base"), 12000.0, false),
            status(Some("pro"), 0.0, true),
            BillingStatus {
                plan: status(Some("max"), 0.0, false).plan.map(|mut p| {
                    p.status = "past_due".into();
                    p
                }),
                ..status(Some("max"), 0.0, false)
            },
        ];
        for (ix, variant) in variants.into_iter().enumerate() {
            page.update(cx, |page, cx| {
                page.status = Loadable::Ready(variant);
                page.packs = 1 + ix as u32;
                if ix == 2 {
                    page.watch(cx);
                }
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear());
        }
        page.update(cx, |page, cx| {
            page.status = Loadable::Error("edge unreachable".into());
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear());
    }
}
