//! Settings → Providers / accounts (feature-inventory §1.9): provider cards
//! (Claude Code, Codex, Cursor, Antigravity) with account rows — email, plan
//! badge, Active, usage meters (indigo → amber ≥80% → red ≥95%, reset time),
//! Switch / Forget — plus the one sign-in dialog every provider shares (the
//! browser finishes the login on a loopback callback; Claude's paste-code
//! step is its fallback) and account-shaped loading skeletons. Zeron
//! retargets devices from the settings sidebar (`targetDeviceId` passthrough;
//! a remote device's callback is forwarded to this one engine-side).
//!
//! The accounts RPC surface is being implemented engine-side in parallel —
//! every call here surfaces failures as inline UI states rather than assuming
//! the methods exist.

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, Context, Entity, Hsla, SharedString, Subscription, Task, Window, div, prelude::*,
    px,
};
use std::time::Duration;

use zeron_proto::{
    AgentAccount, AgentAccountsSnapshot, AgentLoginMode, AgentLoginPoll, AgentLoginStart,
    AgentLoginStatus, HarnessId,
};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// Pure: usage meters + labels
// ---------------------------------------------------------------------------

pub const USAGE_WARN_FRACTION: f32 = 0.80;
pub const USAGE_CRITICAL_FRACTION: f32 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLevel {
    /// < 80% — indigo.
    Normal,
    /// ≥ 80% — amber.
    Warn,
    /// ≥ 95% — red.
    Critical,
}

/// Threshold classification of a usage fraction. Pure.
pub fn usage_level(fraction: f32) -> UsageLevel {
    if fraction >= USAGE_CRITICAL_FRACTION {
        UsageLevel::Critical
    } else if fraction >= USAGE_WARN_FRACTION {
        UsageLevel::Warn
    } else {
        UsageLevel::Normal
    }
}

/// Usage meter columns: window label, reset time, and the meters' overall
/// measure — long enough to read at a glance, short of a full-width gauge.
/// Compact account list geometry: every row shares one usage column and one
/// action slot, so meters and buttons line up down the list however many
/// accounts there are.
const USAGE_LABEL_WIDTH: f32 = 52.0;
const USAGE_BAR_WIDTH: f32 = 88.0;
const USAGE_PERCENT_WIDTH: f32 = 34.0;
const USAGE_COLUMN_WIDTH: f32 = USAGE_LABEL_WIDTH + USAGE_BAR_WIDTH + USAGE_PERCENT_WIDTH + 16.0;
const ACCOUNT_ACTION_WIDTH: f32 = 112.0;

pub fn usage_color(level: UsageLevel, theme: &Theme) -> Hsla {
    match level {
        UsageLevel::Normal => theme.accent,
        UsageLevel::Warn => theme.warning,
        UsageLevel::Critical => theme.danger,
    }
}

/// Why a `ListAgentAccounts` load is happening. Pure input to
/// [`force_usage_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadTrigger {
    /// Page construction — the visit's first list.
    Mount,
    /// "Click to retry" after a failed load — still the visit's first
    /// successful list.
    Retry,
    /// The explicit Refresh button.
    Refresh,
    /// After a completed add-account login flow.
    PostLogin,
    /// After Switch/Forget succeeds.
    PostAction,
}

/// Whether a load should ask the engine to probe usage (`forceUsage`). The
/// engine only hits the provider when forced; non-forced lists serve the 60s
/// usage cache or nothing (engine/src/agent_accounts.rs module docs — the
/// design expects the UI to force "on page mount/refresh"). The visit's first
/// list (mount, or retry after a failure) must force, or every first open
/// renders "Usage unavailable" until a manual Refresh — the old app fetched
/// usage on every list. Post-Switch/Forget lists ride the still-warm cache.
pub fn force_usage_for(trigger: LoadTrigger) -> bool {
    match trigger {
        LoadTrigger::Mount | LoadTrigger::Retry | LoadTrigger::Refresh | LoadTrigger::PostLogin => {
            true
        }
        LoadTrigger::PostAction => false,
    }
}

/// Whether a login URL may be opened. Remote logins relay their URL from
/// another device (or from a CLI's output there), so only web sign-in pages
/// and loopback callbacks pass — never `file://` or custom schemes. Pure.
pub fn is_openable_login_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
            let host = match authority.strip_prefix('[') {
                Some(v6) => v6.split(']').next().unwrap_or(""),
                None => authority.split(':').next().unwrap_or(""),
            };
            matches!(host, "localhost" | "127.0.0.1" | "::1")
        }
        _ => false,
    }
}

fn open_login_url(url: &str, cx: &mut gpui::App) {
    if is_openable_login_url(url) {
        cx.open_url(url);
    } else {
        tracing::warn!("refused to open a non-web sign-in URL");
    }
}

/// Full reset date and time in the computer's local timezone.
pub fn format_reset(resets_at: Option<DateTime<Utc>>) -> Option<String> {
    let local = resets_at?.with_timezone(&chrono::Local);
    Some(format!("resets {}", local.format("%Y-%m-%d %H:%M")))
}

/// The providers zeron can sign into, in display order: (harness, name, CLI
/// command — named in the empty-state copy, zeron settings.agents.tsx
/// `PROVIDERS`). Every agent with a login of its own is here; what each one
/// supports is documented engine-side (`agent_accounts` module docs).
pub const PROVIDERS: [(HarnessId, &str, &str); 9] = [
    (HarnessId::ClaudeCode, "Claude Code", "claude"),
    (HarnessId::Codex, "Codex", "codex"),
    (HarnessId::Cursor, "Cursor", "cursor-agent"),
    (HarnessId::Antigravity, "Antigravity", "Antigravity"),
    (HarnessId::Grok, "Grok", "grok login"),
    (HarnessId::Devin, "Devin", "devin auth login"),
    (HarnessId::Opencode, "OpenCode", "opencode auth login"),
    (HarnessId::Pi, "Pi", "pi"),
    (HarnessId::Hermes, "Hermes", "hermes auth add"),
];

/// Whether `harness` has an Accounts section (and sign-in flow). Pure.
pub fn signs_in(harness: HarnessId) -> bool {
    PROVIDERS
        .iter()
        .any(|(provider, _, _)| *provider == harness)
}

pub(crate) fn provider_name(harness: HarnessId) -> &'static str {
    PROVIDERS
        .iter()
        .find(|(provider, _, _)| *provider == harness)
        .map_or("this provider", |(_, name, _)| name)
}

/// Whether the provider reports plan usage. One that doesn't shows no meters
/// and no "usage unavailable" note — there is nothing missing. Pure.
pub fn reports_usage(harness: HarnessId) -> bool {
    harness != HarnessId::Antigravity
}

/// Providers whose agent holds exactly ONE login (Antigravity): once it is
/// connected there is nothing to add — signing in again only re-confirms it.
pub fn keeps_one_login(harness: HarnessId) -> bool {
    harness == HarnessId::Antigravity
}

/// Whether zeron switches this agent's logins. Hermes rotates through its
/// own credential pool (listed, never reordered); Antigravity keeps one.
pub fn switches_accounts(harness: HarnessId) -> bool {
    !matches!(harness, HarnessId::Hermes | HarnessId::Antigravity)
}

/// A standing note under a provider's Accounts label, for an agent whose
/// accounts work differently. Hermes owns its credential pool: zeron lists
/// it and adds to it through Hermes' own CLI, but never switches or removes
/// its entries. Pure.
pub fn provider_note(harness: HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::Hermes => Some(
            "Hermes manages its own credential pool and rotates through it. Accounts added \
             here go through `hermes auth add`; remove one with `hermes auth remove`.",
        ),
        _ => None,
    }
}

/// One way to add an account: agents that keep a login PER model provider
/// (OpenCode, Pi, Hermes) sign in to a named provider; the rest have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginOption {
    /// The engine's `provider` param (`None` = the agent's only login).
    pub provider: Option<&'static str>,
    /// Who the user signs in to.
    pub label: &'static str,
}

/// The sign-ins zeron offers for `harness`, in button order. Pure.
pub fn login_options(harness: HarnessId) -> Vec<LoginOption> {
    let option = |provider, label| LoginOption {
        provider: Some(provider),
        label,
    };
    match harness {
        HarnessId::Opencode => vec![
            option("openai", "ChatGPT"),
            option("github-copilot", "GitHub Copilot"),
        ],
        HarnessId::Pi => vec![option("openai-codex", "ChatGPT")],
        HarnessId::Hermes => vec![
            option("openai-codex", "ChatGPT"),
            option("nous", "Nous Portal"),
        ],
        _ => vec![LoginOption {
            provider: None,
            label: provider_name(harness),
        }],
    }
}

/// The list's last row: connect the first account, or add another. Pure.
pub fn add_account_label(harness: HarnessId, empty: bool) -> String {
    if empty {
        format!("Connect a {} account", provider_name(harness))
    } else {
        "Add account".to_string()
    }
}

/// [`add_account_label`] for one [`LoginOption`]: a per-provider sign-in
/// names its provider ("Connect ChatGPT", "Add GitHub Copilot account"). Pure.
pub fn add_option_label(harness: HarnessId, option: LoginOption, empty: bool) -> String {
    match option.provider {
        None => add_account_label(harness, empty),
        Some(_) if empty => format!("Connect {}", option.label),
        Some(_) => format!("Add {} account", option.label),
    }
}

/// What the sign-in dialog says while the browser finishes — one sentence
/// per provider, same shape. Pure.
fn login_copy(harness: HarnessId, provider: Option<&str>) -> &'static str {
    match (harness, provider) {
        (HarnessId::ClaudeCode, _) => {
            "Finish signing in to Claude in your browser. The new login is saved next to \
             your current one — nothing changes until you switch."
        }
        (HarnessId::Codex, _) => {
            "Finish signing in to ChatGPT in your browser. The new login is saved next to \
             your current one — nothing changes until you switch."
        }
        (HarnessId::Cursor, _) => {
            "Finish signing in to Cursor in your browser. This mints a zeron-named API key \
             you can revoke any time from Cursor's dashboard."
        }
        (HarnessId::Antigravity, _) => {
            "Finish signing in to Google in your browser. Antigravity keeps one login on \
             this device; if it is already signed in, this just confirms it."
        }
        (HarnessId::Grok, _) => {
            "Finish signing in to Grok in your browser — approve the code shown below. The \
             new login is saved next to your current one — nothing changes until you switch."
        }
        (HarnessId::Devin, _) => {
            "Finish signing in to Devin in your browser. The new login is saved next to your \
             current one — nothing changes until you switch."
        }
        (HarnessId::Opencode, Some("github-copilot")) => {
            "Finish signing in to GitHub in your browser — enter the code shown below. The \
             new login is saved next to your current one — nothing changes until you switch."
        }
        (HarnessId::Opencode | HarnessId::Pi, _) => {
            "Finish signing in to ChatGPT in your browser. The agent gets its own login, saved \
             next to any current one — nothing changes until you switch."
        }
        (HarnessId::Hermes, _) => {
            "Finish signing in in your browser — enter the code shown below. Hermes adds the \
             login to its own credential pool and rotates through it itself."
        }
        _ => "Finish signing in in your browser.",
    }
}

/// Accounts of one provider, in the engine's order (slot creation). No
/// active-first re-sort: switching accounts must not move the switched-to
/// card — the Active badge already says which one is live, and a list that
/// reshuffles under the click reads as broken. Pure.
pub fn provider_accounts(
    snapshot: &AgentAccountsSnapshot,
    harness: HarnessId,
) -> Vec<&AgentAccount> {
    snapshot
        .accounts
        .iter()
        .filter(|a| a.harness == harness)
        .collect()
}

/// The optimistic half of a switch: `account` becomes the live login of its
/// group — its agent, or for agents that keep a login per model provider,
/// that provider — and every other group keeps its own. Pure.
pub fn mark_switched(snapshot: &mut AgentAccountsSnapshot, account: &AgentAccount) {
    for row in snapshot.accounts.iter_mut() {
        if row.harness == account.harness && row.provider == account.provider {
            row.active = row.id == account.id;
        }
    }
}

/// One mini meter line of the usage column: label, a short bar, percent.
/// Settings adds the reset date below this shared compact meter.
pub(crate) fn render_usage_meter(
    window: &zeron_proto::AgentUsageWindow,
    theme: &Theme,
) -> AnyElement {
    let fraction = window.used_fraction.clamp(0.0, 1.0);
    let level = usage_level(fraction);
    let fill = usage_color(level, theme).opacity(match level {
        UsageLevel::Normal => 0.8,
        _ => 0.9,
    });
    div()
        .h(px(16.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .text_size(crate::typography::ui_rems(11.5))
        .child(
            div()
                .w(px(USAGE_LABEL_WIDTH))
                .flex_none()
                .truncate()
                .text_color(theme.text_muted)
                .child(SharedString::from(window.label.clone())),
        )
        .child(
            div()
                .w(px(USAGE_BAR_WIDTH))
                .flex_none()
                .h(px(4.0))
                .rounded_full()
                .overflow_hidden()
                .bg(theme.wash(0.08))
                .when(fraction > 0.0, |el| {
                    el.child(
                        div()
                            .h_full()
                            // A 1.5% floor keeps tiny non-zero usage
                            // visible (zeron `max(used, 1.5)%`).
                            .w(gpui::relative(fraction.max(0.015)))
                            .rounded_full()
                            .bg(fill),
                    )
                }),
        )
        .child(
            div()
                .w(px(USAGE_PERCENT_WIDTH))
                .flex_none()
                .text_right()
                .text_color(match level {
                    UsageLevel::Normal => theme.text_muted,
                    _ => usage_color(level, theme),
                })
                .child(SharedString::from(format!(
                    "{}%",
                    (fraction * 100.0).round() as u32
                ))),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Entity
// ---------------------------------------------------------------------------

/// One sign-in, whichever provider: started, then waiting on the browser
/// (or, Claude's fallback, on a pasted code), until it lands or fails.
struct LoginFlow {
    harness: HarnessId,
    /// The model provider signed in to, for per-provider agents.
    provider: Option<&'static str>,
    /// Which start this is — replies to a cancelled or superseded attempt
    /// must not touch the current one.
    attempt: u64,
    /// Known once StartAgentLogin replied.
    login_id: Option<String>,
    /// The sign-in page, once known (the start reply, or a later poll).
    url: Option<String>,
    step: LoginStep,
}

#[derive(Debug, Clone, PartialEq)]
enum LoginStep {
    /// Starting, then waiting for the browser to reach the loopback callback.
    Browser { message: Option<SharedString> },
    /// Claude's fallback when no loopback port could be bound.
    PasteCode {
        submitting: bool,
        error: Option<SharedString>,
    },
    /// The start, a poll, or the provider failed — shown with Retry.
    Failed { message: SharedString },
}

impl LoginFlow {
    fn title(&self) -> String {
        let option = login_options(self.harness)
            .into_iter()
            .find(|option| option.provider == self.provider);
        match option {
            Some(LoginOption {
                provider: Some(_),
                label,
            }) => format!("Sign in to {label} for {}", provider_name(self.harness)),
            _ => format!("Sign in to {}", provider_name(self.harness)),
        }
    }

    /// The Browser step's progress line.
    fn status(&self) -> SharedString {
        match &self.step {
            LoginStep::Browser {
                message: Some(message),
            } => message.clone(),
            LoginStep::Browser { .. } if self.url.is_some() => {
                "Waiting for you to finish in the browser…".into()
            }
            _ => "Starting sign-in…".into(),
        }
    }
}

/// The last accounts list per target device (`None` = this device), shared
/// by every accounts view so a re-opened Settings paints instantly and
/// revalidates in place instead of flashing a skeleton.
#[derive(Default)]
pub(crate) struct AccountsSnapshotCache(
    pub(crate) std::collections::HashMap<Option<String>, AgentAccountsSnapshot>,
);

impl gpui::Global for AccountsSnapshotCache {}

pub struct AccountsPage {
    state: Entity<AppState>,
    embedded: bool,
    embedded_harness: Option<HarnessId>,
    scroll: widgets::PageScroll,
    /// Which device's logins are shown; `None` = this device (no passthrough).
    /// Retargeted by the page-header device switcher (zeron parity: the
    /// accounts RPCs are relay-forwardable, CLI logins are per-device).
    target_device: Option<String>,
    device_select: widgets::SelectState,
    snapshot: Loadable<AgentAccountsSnapshot>,
    /// A list is in flight over an already-painted snapshot.
    refreshing: bool,
    /// Account id with an in-flight Switch/Forget.
    busy_account: Option<String>,
    /// The account whose `⋯` actions menu is open.
    row_menu: popover::Popup<String>,
    login: Option<LoginFlow>,
    login_attempts: u64,
    error: Option<SharedString>,
    code_input: Entity<ComposerInput>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    _observe: Subscription,
    _code_events: Subscription,
}

impl AccountsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self::new_with_layout(state, false, None, None, cx)
    }

    pub fn new_embedded(
        state: Entity<AppState>,
        target_device: Option<String>,
        harness: HarnessId,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_layout(state, true, target_device, Some(harness), cx)
    }

    fn new_with_layout(
        state: Entity<AppState>,
        embedded: bool,
        target_device: Option<String>,
        embedded_harness: Option<HarnessId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        let code_input = cx.new(|cx| ComposerInput::new("Paste the authorization code", cx));
        let code_events = cx.subscribe(&code_input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_code(cx);
            }
        });
        let mut page = Self {
            state,
            embedded,
            embedded_harness,
            scroll: widgets::PageScroll::default(),
            target_device,
            device_select: widgets::SelectState::default(),
            snapshot: Loadable::Idle,
            refreshing: false,
            busy_account: None,
            row_menu: popover::Popup::default(),
            login: None,
            login_attempts: 0,
            error: None,
            code_input,
            load_task: None,
            action_task: None,
            poll_task: None,
            _observe: observe,
            _code_events: code_events,
        };
        // Paint from the last list (this session) or the engine's persisted
        // usage, then force one probe to update in place — the skeleton only
        // ever shows on a genuinely first load.
        page.load(force_usage_for(LoadTrigger::Mount), cx);
        page
    }

    /// Retarget the page at another device's logins: every accounts RPC is
    /// relay-forwardable, so the whole page — list, usage probes, switch,
    /// forget, login flows — follows the passthrough.
    pub(crate) fn set_target_device(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        widgets::close_select(self, |page: &mut Self| &mut page.device_select, cx);
        if self.target_device == target {
            cx.notify();
            return;
        }
        self.target_device = target;
        // A different device = a different accounts world: drop in-flight
        // login/action state and reload with a forced usage probe (the new
        // device's cache is cold).
        self.login = None;
        self.busy_account = None;
        self.error = None;
        self.load(force_usage_for(LoadTrigger::Mount), cx);
    }

    pub(crate) fn set_embedded_harness(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if self.embedded_harness != Some(harness) {
            self.embedded_harness = Some(harness);
            self.login = None;
            self.error = None;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn embedded_harness(&self) -> Option<HarnessId> {
        self.embedded_harness
    }

    /// Params with the `targetDeviceId` passthrough merged in.
    fn params(&self, value: serde_json::Value) -> serde_json::Value {
        let mut value = value;
        if let (Some(target), Some(object)) = (&self.target_device, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    /// The page-header device switcher (zeron device-switcher.tsx): a quiet
    /// trigger — platform glyph · name · presence dot · sort glyph — opening a
    /// dropdown of every registered device. Selecting one retargets the page.
    fn render_device_switcher(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use crate::icons::{self, icon};
        let (mut devices, local_id) = {
            let s = self.state.read(cx);
            (s.devices.clone(), s.local_device_id.clone())
        };
        // Stable row order (registration time, then id) — zeron's switcher
        // sorts the same way so rows never reshuffle on heartbeats.
        devices.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let effective = self.target_device.clone().or_else(|| local_id.clone());
        let platform_glyph = |platform: &str| match platform {
            "macos" | "darwin" => icons::LAPTOP,
            "ios" | "android" => icons::SMARTPHONE,
            _ => icons::MONITOR,
        };
        // Local device = no passthrough (calls stay direct).
        let mut targets: Vec<Option<String>> = Vec::new();
        let mut options = Vec::new();
        for device in &devices {
            let is_local = local_id.as_deref() == Some(device.id.as_str());
            let glyph = platform_glyph(&device.platform);
            let muted = theme.text_muted;
            let option = widgets::SelectOption::new(device.name.clone()).leading(move || {
                icon(glyph)
                    .size(px(16.0))
                    .flex_none()
                    .text_color(muted)
                    .into_any_element()
            });
            options.push(if is_local {
                option.detail("You")
            } else {
                option
            });
            targets.push((!is_local).then(|| device.id.clone()));
        }
        let selected = match devices
            .iter()
            .position(|d| Some(d.id.as_str()) == effective.as_deref())
        {
            Some(ix) => ix,
            // Not registered (yet): keep the current target reachable.
            None => {
                let muted = theme.text_muted;
                options.push(widgets::SelectOption::new("This device").leading(move || {
                    icon(icons::LAPTOP)
                        .size(px(16.0))
                        .flex_none()
                        .text_color(muted)
                        .into_any_element()
                }));
                targets.push(self.target_device.clone());
                options.len() - 1
            }
        };
        widgets::select(
            "accounts-device-switcher",
            "Device",
            theme,
            |page: &mut Self| &mut page.device_select,
        )
        .options(options, selected)
        .menu_width(260.0)
        .heading("Devices")
        .on_select(move |page, ix, _, cx| {
            if let Some(target) = targets.get(ix) {
                page.set_target_device(target.clone(), cx);
            }
        })
        .render(&self.device_select, cx)
        .into_any_element()
    }

    /// Stale-while-revalidate: a painted snapshot stays on screen while the
    /// list runs. With nothing to paint yet, a forced load first takes the
    /// engine's plain list (last known usage, no network) so rows appear at
    /// once, then the probed list replaces it.
    fn load(&mut self, force_usage: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.snapshot = Loadable::Error("Engine not connected".into());
            return;
        };
        let key = self.target_device.clone();
        if !matches!(self.snapshot, Loadable::Ready(_)) {
            self.snapshot = match cx
                .try_global::<AccountsSnapshotCache>()
                .and_then(|cache| cache.0.get(&key))
            {
                Some(cached) => Loadable::Ready(cached.clone()),
                None => Loadable::Loading,
            };
        }
        let paint_first = force_usage && !matches!(self.snapshot, Loadable::Ready(_));
        self.refreshing = true;
        let params = self.params(serde_json::json!({ "forceUsage": force_usage }));
        let plain = self.params(serde_json::json!({ "forceUsage": false }));
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let fetch = |params: serde_json::Value| {
                let engine = engine.clone();
                async move {
                    engine
                        .client()
                        .call(methods::LIST_AGENT_ACCOUNTS, params)
                        .await
                        .map_err(|err| err.to_string())
                        .and_then(|value| {
                            serde_json::from_value::<AgentAccountsSnapshot>(value)
                                .map_err(|err| err.to_string())
                        })
                }
            };
            if paint_first && let Ok(snapshot) = fetch(plain).await {
                let key = key.clone();
                this.update(cx, |page, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(key, snapshot.clone());
                    page.snapshot = Loadable::Ready(snapshot);
                    cx.notify();
                })
                .ok();
            }
            let result = fetch(params).await;
            this.update(cx, |page, cx| {
                page.refreshing = false;
                match result {
                    Ok(snapshot) => {
                        cx.default_global::<AccountsSnapshotCache>()
                            .0
                            .insert(key, snapshot.clone());
                        page.snapshot = Loadable::Ready(snapshot);
                    }
                    // A failed revalidation keeps the painted rows.
                    Err(err) if matches!(page.snapshot, Loadable::Ready(_)) => {
                        page.error = Some(err.into());
                    }
                    Err(err) => page.snapshot = Loadable::Error(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Switch / Forget an account, optimistically: the row changes at once
    /// and the engine's reply (the fresh list) replaces it — no reload, no
    /// spinner, no flicker. A refusal restores the previous list.
    fn account_action(
        &mut self,
        method: &'static str,
        account: &AgentAccount,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let previous = self.snapshot.ready().cloned();
        if let Loadable::Ready(snapshot) = &mut self.snapshot {
            if method == methods::ACTIVATE_AGENT_ACCOUNT {
                mark_switched(snapshot, account);
            } else {
                snapshot.accounts.retain(|row| row.id != account.id);
            }
        }
        self.busy_account = Some(account.id.clone());
        self.error = None;
        // Tolerant param shape: both `id` and `accountId` plus the harness.
        let params = self.params(serde_json::json!({
            "id": account.id,
            "accountId": account.id,
            "harness": account.harness,
        }));
        let key = self.target_device.clone();
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            this.update(cx, |page, cx| {
                page.busy_account = None;
                match result.map(serde_json::from_value::<AgentAccountsSnapshot>) {
                    Ok(Ok(snapshot)) => {
                        cx.default_global::<AccountsSnapshotCache>()
                            .0
                            .insert(key, snapshot.clone());
                        page.snapshot = Loadable::Ready(snapshot);
                    }
                    // An older engine's reply without the list: fetch it.
                    Ok(Err(_)) => page.load(force_usage_for(LoadTrigger::PostAction), cx),
                    Err(err) => {
                        if let Some(previous) = previous {
                            page.snapshot = Loadable::Ready(previous);
                        }
                        page.error = Some(format!("{err}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ---- add-account flows ----

    /// Start (or Retry) a sign-in: the same dialog for every provider.
    fn start_login(
        &mut self,
        harness: HarnessId,
        provider: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.login_attempts += 1;
        let attempt = self.login_attempts;
        self.login = Some(LoginFlow {
            harness,
            provider,
            attempt,
            login_id: None,
            url: None,
            step: LoginStep::Browser { message: None },
        });
        self.poll_task = None;
        self.error = None;
        let mut params = serde_json::json!({ "harness": harness });
        if let (Some(provider), Some(object)) = (provider, params.as_object_mut()) {
            object.insert("provider".into(), serde_json::json!(provider));
        }
        let params = self.params(params);
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::START_AGENT_LOGIN, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value::<AgentLoginStart>(value).map_err(|e| e.to_string())
                });
            this.update(cx, |page, cx| page.apply_start(attempt, result, cx))
                .ok();
        }));
        cx.notify();
    }

    /// Fold a StartAgentLogin reply into the flow `attempt` (if still open).
    fn apply_start(
        &mut self,
        attempt: u64,
        result: Result<AgentLoginStart, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(flow) = self.login.as_mut().filter(|flow| flow.attempt == attempt) else {
            return;
        };
        match result {
            Ok(start) => {
                if !start.url.is_empty() {
                    open_login_url(&start.url, cx);
                    flow.url = Some(start.url.clone());
                }
                flow.login_id = Some(start.login_id);
                match start.mode {
                    AgentLoginMode::PasteCode => {
                        flow.step = LoginStep::PasteCode {
                            submitting: false,
                            error: None,
                        };
                        self.code_input
                            .update(cx, |input, cx| input.set_text("", cx));
                    }
                    AgentLoginMode::Browser => self.spawn_poll(cx),
                }
            }
            Err(error) => {
                flow.step = LoginStep::Failed {
                    message: format!("Couldn't start the sign-in: {error}").into(),
                };
            }
        }
        cx.notify();
    }

    fn submit_code(&mut self, cx: &mut Context<Self>) {
        let Some(LoginFlow {
            login_id: Some(login_id),
            step: LoginStep::PasteCode { submitting, .. },
            attempt,
            ..
        }) = &mut self.login
        else {
            return;
        };
        if *submitting {
            return;
        }
        let code = self.code_input.read(cx).text().trim().to_string();
        if code.is_empty() {
            return;
        }
        let (login_id, attempt) = (login_id.clone(), *attempt);
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        *submitting = true;
        let params = self.params(serde_json::json!({ "loginId": login_id, "code": code }));
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::COMPLETE_AGENT_LOGIN, params)
                .await;
            this.update(cx, |page, cx| {
                let Some(flow) = page.login.as_mut().filter(|f| f.attempt == attempt) else {
                    return;
                };
                match result {
                    Ok(_) => {
                        page.login = None;
                        page.load(force_usage_for(LoadTrigger::PostLogin), cx);
                    }
                    Err(err) => {
                        flow.step = LoginStep::PasteCode {
                            submitting: false,
                            error: Some(format!("{err}").into()),
                        };
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The browser-wait poll loop: PollAgentLogin every 1.5s until the login
    /// lands or fails.
    fn spawn_poll(&mut self, cx: &mut Context<Self>) {
        let Some(LoginFlow {
            login_id: Some(login_id),
            attempt,
            ..
        }) = &self.login
        else {
            return;
        };
        let attempt = *attempt;
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({ "loginId": login_id }));
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                let result = engine
                    .client()
                    .call(methods::POLL_AGENT_LOGIN, params.clone())
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|value| {
                        serde_json::from_value::<AgentLoginPoll>(value)
                            .map_err(|_| "malformed reply".to_string())
                    });
                match this.update(cx, |page, cx| page.apply_poll(attempt, result, cx)) {
                    Ok(false) => {}
                    Ok(true) | Err(_) => break,
                }
            }
        }));
    }

    /// Fold one poll into the flow `attempt`. `true` once polling is over —
    /// the login landed (the list reloads and shows it), failed (the dialog
    /// says why and offers Retry), or the dialog is gone. Never a silent
    /// reset: every ending either shows the account or an error.
    fn apply_poll(
        &mut self,
        attempt: u64,
        result: Result<AgentLoginPoll, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(flow) = self.login.as_mut().filter(|flow| flow.attempt == attempt) else {
            return true;
        };
        let finished = match result {
            Ok(poll) => match poll.status {
                AgentLoginStatus::Done => {
                    self.login = None;
                    self.load(force_usage_for(LoadTrigger::PostLogin), cx);
                    true
                }
                AgentLoginStatus::Error => {
                    flow.step = LoginStep::Failed {
                        message: poll
                            .message
                            .unwrap_or_else(|| "The sign-in failed.".to_string())
                            .into(),
                    };
                    true
                }
                AgentLoginStatus::Pending => {
                    // A page learned after the start (Antigravity prints its
                    // own once its server is up): open it once.
                    if let Some(url) = poll.url.filter(|url| flow.url.as_ref() != Some(url)) {
                        open_login_url(&url, cx);
                        flow.url = Some(url);
                    }
                    flow.step = LoginStep::Browser {
                        message: poll.message.map(Into::into),
                    };
                    false
                }
            },
            Err(error) => {
                flow.step = LoginStep::Failed {
                    message: format!("Lost track of the sign-in: {error}").into(),
                };
                true
            }
        };
        cx.notify();
        finished
    }

    fn cancel_login(&mut self, cx: &mut Context<Self>) {
        let login_id = match self.login.take() {
            // A failed login is already over engine-side.
            Some(LoginFlow {
                login_id: Some(login_id),
                step: LoginStep::Browser { .. } | LoginStep::PasteCode { .. },
                ..
            }) => Some(login_id),
            _ => None,
        };
        self.poll_task = None;
        if let (Some(login_id), Some(engine)) = (login_id, self.state.read(cx).engine().cloned()) {
            let params = self.params(serde_json::json!({ "loginId": login_id }));
            self.action_task = Some(cx.spawn(async move |_, _| {
                if let Err(err) = engine
                    .client()
                    .call(methods::CANCEL_AGENT_LOGIN, params)
                    .await
                {
                    tracing::debug!(error = %err, "CancelAgentLogin failed (best-effort)");
                }
            }));
        }
        cx.notify();
    }

    /// Escape that reached Settings unclaimed cancels an open login first,
    /// so it never closes Settings under the dialog. Returns whether it did.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.login.is_none() {
            return false;
        }
        self.cancel_login(cx);
        true
    }

    fn retry_login(&mut self, cx: &mut Context<Self>) {
        if let Some((harness, provider)) = self.login.as_ref().map(|f| (f.harness, f.provider)) {
            self.start_login(harness, provider, cx);
        }
    }

    // ---- render pieces ----

    /// The quiet note an account shows in its meta line instead of meters.
    /// Render hook for the usage data flow: a reason the probe came back
    /// empty belongs here. A provider without usage gets no note at all.
    fn render_usage_missing(&self, account: &AgentAccount, _theme: &Theme) -> Option<AnyElement> {
        if !reports_usage(account.harness) {
            return None;
        }
        let note = if !account.switchable && switches_accounts(account.harness) {
            match account.harness {
                // Keychain denied: the login is there, its secret isn't.
                HarnessId::ClaudeCode => "Credentials unavailable".to_string(),
                // An opaque token whose account couldn't be looked up.
                _ => "Couldn't identify this login".to_string(),
            }
        } else if let Some(reason) = &account.usage_error {
            // The engine's reason ("Rate limited by Anthropic — retrying in
            // 2m", "Signed out — sign in again"), not a bare shrug.
            reason.clone()
        } else if self.refreshing {
            "Checking usage…".to_string()
        } else {
            "Usage unavailable".to_string()
        };
        Some(div().child(SharedString::from(note)).into_any_element())
    }

    /// Trailing mark on an expanded provider's "Accounts" label: the app's
    /// mini loader while a list is in flight over painted rows, else nothing.
    fn render_accounts_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !(self.refreshing && matches!(self.snapshot, Loadable::Ready(_))) {
            return None;
        }
        let theme = Theme::of(cx).for_settings_surface();
        Some(
            crate::loaders::mini_mono_spinner(
                "accounts-refreshing",
                1.5,
                theme.text_muted,
                cx.entity_id(),
                cx,
            )
            .into_any_element(),
        )
    }

    fn close_row_menu(&mut self, cx: &mut Context<Self>) {
        if self.row_menu.begin_close() {
            popover::reap_popup(cx, |page: &mut Self| &mut page.row_menu);
        }
        cx.notify();
    }

    /// One account row. The list is a single choice, so the row itself says
    /// which account is live: its avatar wears an accent ring and its meta
    /// line reads "In use". Any other row switches when clicked; every row
    /// ends in the same quiet `⋯` menu (Switch / Remove), so the trailing
    /// edge never changes shape between states.
    fn render_account_row(
        &self,
        account: &AgentAccount,
        ix: usize,
        first: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let email: SharedString = account
            .email
            .clone()
            .or_else(|| account.display_name.clone())
            .unwrap_or_else(|| "Unknown account".into())
            .into();
        let can_switch = !account.active && account.switchable;
        let switch_account = account.clone();

        // Personal Claude orgs are named "<email>'s Organization" — noise
        // right under the email itself.
        let organization = account.organization.clone().filter(|org| {
            account
                .email
                .as_deref()
                .is_none_or(|email| !org.starts_with(email))
        });
        let mut meta: Vec<AnyElement> = [account.plan_label.clone(), organization]
            .into_iter()
            .flatten()
            .map(|fragment| div().child(SharedString::from(fragment)).into_any_element())
            .collect();
        if account.active {
            meta.push(
                div()
                    .text_color(theme.accent)
                    .child(SharedString::from("In use"))
                    .into_any_element(),
            );
        }
        // When meters are missing, their reason joins the account meta line.
        let usage: AnyElement = if account.usage_windows.is_empty() {
            meta.extend(self.render_usage_missing(account, theme));
            div().w(px(USAGE_COLUMN_WIDTH)).flex_none().into_any_element()
        } else {
            div()
                .id(("account-usage", ix))
                .w(px(USAGE_COLUMN_WIDTH))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .children(
                    account
                        .usage_windows
                        .iter()
                        .take(2)
                        .map(|window| {
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(render_usage_meter(window, theme))
                                .when_some(format_reset(window.resets_at), |el, reset| {
                                    el.child(
                                        div()
                                            .w_full()
                                            .text_right()
                                            .whitespace_nowrap()
                                            .text_size(crate::typography::ui_rems(10.5))
                                            .text_color(theme.text_muted)
                                            .child(SharedString::from(reset)),
                                    )
                                })
                        }),
                )
                .into_any_element()
        };

        // Avatar: the account's initial on a soft disc; the live account gets
        // an accent ring with a hairline of air between ring and disc.
        let initial: SharedString = email
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into())
            .into();
        let avatar = div()
            .flex_none()
            .size(px(32.0))
            .rounded_full()
            .border(px(1.5))
            .border_color(if account.active {
                theme.accent
            } else {
                gpui::transparent_black()
            })
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size(px(25.0))
                    .rounded_full()
                    .bg(theme.wash(0.09))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::typography::ui_rems(11.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if account.active {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .child(initial),
            );

        let menu_open = self.row_menu.get() == Some(&account.id);
        let menu_account = account.clone();
        let menu_trigger_id = account.id.clone();
        let mut more = widgets::action_button(theme, widgets::ActionTone::Quiet)
            .id(("account-more", ix))
            .w(px(28.0))
            .px_0()
            .justify_center()
            .when(menu_open, |el| el.bg(theme.glass_hover()))
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(format!("Actions for {email}"))
            .aria_expanded(menu_open)
            .focus_visible(|s| s.border_2().border_color(theme.accent))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |page, _, _, _| {
                    page.row_menu
                        .note_trigger_press_matching(|open| *open == menu_trigger_id);
                }),
            )
            .on_click(cx.listener(move |page, _, _, cx| {
                cx.stop_propagation();
                if page.row_menu.take_press_was_open() {
                    page.close_row_menu(cx);
                } else {
                    page.row_menu.open(menu_account.id.clone());
                    cx.notify();
                }
            }))
            .child(
                crate::icons::icon(crate::icons::MORE_HORIZONTAL)
                    .size(px(16.0))
                    .text_color(theme.text_muted),
            );
        if menu_open {
            let popup = Theme::of(cx).for_popup();
            let switch_from_menu = account.clone();
            let forget_account = account.clone();
            let menu = popover::popover_card(&popup)
                .w(px(208.0))
                .flex()
                .flex_col()
                .on_mouse_down_out(cx.listener(|page, _, _, cx| page.close_row_menu(cx)))
                .when(can_switch, |menu| {
                    menu.child(
                        popover::menu_row(&popup, false, format!("account-menu-switch-{ix}"))
                            .id(("account-menu-switch", ix))
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.close_row_menu(cx);
                                page.account_action(
                                    methods::ACTIVATE_AGENT_ACCOUNT,
                                    &switch_from_menu,
                                    cx,
                                );
                            }))
                            .child(SharedString::from("Switch to this account")),
                    )
                })
                .when(account.switchable, |menu| {
                    menu.child(
                        popover::menu_row(&popup, false, format!("account-menu-remove-{ix}"))
                            .id(("account-menu-remove", ix))
                            .text_color(popup.danger_muted)
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.close_row_menu(cx);
                                page.account_action(
                                    methods::FORGET_AGENT_ACCOUNT,
                                    &forget_account,
                                    cx,
                                );
                            }))
                            .child(SharedString::from("Remove account")),
                    )
                })
                .into_any_element();
            more = more.relative().child(popover::anchored_menu_below_end(
                format!("account-menu-{ix}"),
                menu,
                self.row_menu.closing_since(),
            ));
        }
        // Accounts with nothing to offer (an unreadable live login,
        // Antigravity's single login) keep the slot for alignment only.
        let has_actions = account.switchable;

        let body = div()
            .id(("account-row", ix))
            .mx(px(-10.0))
            .px(px(10.0))
            .py(px(10.0))
            .min_h(px(56.0))
            .rounded(px(10.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.0))
            .when(can_switch, |row| {
                let hover_key = format!("account-row-{ix}-hover");
                row.cursor_pointer()
                    .bg(crate::motion::hover_blend(
                        &hover_key,
                        theme.wash(0.0),
                        theme.wash(0.04),
                    ))
                    .on_hover(crate::motion::hover_listener(hover_key))
                    .tooltip(widgets::text_tooltip("Switch to this account"))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.account_action(methods::ACTIVATE_AGENT_ACCOUNT, &switch_account, cx);
                    }))
            })
            .child(avatar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(theme, email.clone()).truncate())
                    .when(!meta.is_empty(), |el| el.child(widgets::meta_line(theme, meta))),
            )
            .child(usage)
            .child(
                div()
                    .w(px(28.0))
                    .flex_none()
                    .when(has_actions, |slot| slot.child(more)),
            );
        div()
            .when(!self.embedded, |el| el.mx(px(16.0)))
            .when(!first, |el| {
                el.border_t_1().border_color(widgets::row_divider(theme))
            })
            .py(px(2.0))
            .child(body)
            .into_any_element()
    }

    /// A ghost of [`Self::render_account_row`] for an expanded provider:
    /// same geometry, so loaded rows land without a layout jump.
    fn render_embedded_skeleton(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use crate::motion;
        let delta = motion::pulse_delta(&motion::ZERON_PULSE, cx.entity_id(), cx);
        let ghost = |w: f32, h: f32| {
            div()
                .flex_none()
                .w(px(w))
                .h(px(h))
                .rounded(px(4.0))
                .bg(theme.wash(0.07))
        };
        let row = |first: bool, dim: bool| {
            div()
                .min_h(px(56.0))
                .py(px(10.0))
                .when(!first, |el| {
                    el.border_t_1().border_color(widgets::row_divider(theme))
                })
                .when(dim, |el| el.opacity(0.6))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(16.0))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(ghost(176.0, 11.0))
                        .child(ghost(48.0, 9.0)),
                )
                .child(
                    div()
                        .w(px(USAGE_COLUMN_WIDTH))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .children((0..2).map(|_| {
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(ghost(USAGE_LABEL_WIDTH - 12.0, 8.0))
                                .child(
                                    div()
                                        .w(px(USAGE_BAR_WIDTH))
                                        .h(px(4.0))
                                        .rounded_full()
                                        .bg(theme.wash(0.06)),
                                )
                        })),
                )
                .child(div().w(px(ACCOUNT_ACTION_WIDTH)).flex_none())
        };
        div()
            .child(row(true, false))
            .child(row(false, true))
            .opacity(0.55 + 0.35 * motion::pulse_wave(delta))
            .into_any_element()
    }

    /// The sign-in dialog — one layout for every provider: what to do in the
    /// browser, a way back to the page, then the progress line (or, on
    /// failure, the reason in the same place), with Cancel — or Close and
    /// Retry — on the right. Claude's paste-code fallback swaps the progress
    /// line for the code field.
    fn render_login_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let red_text = theme.danger_muted.opacity(0.9); // red-300
        let login = self.login.as_ref()?;
        let title = login.title();
        let failed = matches!(login.step, LoginStep::Failed { .. });
        let action = |id: &'static str, tone: widgets::ActionTone, label: &'static str| {
            widgets::text_action(&theme, tone, label)
                .id(id)
                .tab_index(0)
                .role(gpui::Role::Button)
                .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
        };
        let copy = match login.step {
            LoginStep::PasteCode { .. } => {
                "Your browser opened Claude's sign-in page. Approve access, then paste the \
                 code Anthropic shows you below. Your current login is untouched until you \
                 switch."
            }
            _ => login_copy(login.harness, login.provider),
        };
        // "Reopen the sign-in page" (zeron: `text-[12px]
        // text-muted-foreground/60 hover:underline`), once there is a page.
        let reopen = login.url.clone().filter(|_| !failed).map(|url| {
            div()
                .id("login-open-url")
                .mt(px(6.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .truncate()
                .cursor_pointer()
                .hover(|s| s.text_color(theme.text))
                .tab_index(0)
                .role(gpui::Role::Button)
                .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                .on_click(cx.listener(move |_, _, _, cx| open_login_url(&url, cx)))
                .child(SharedString::from("Reopen the sign-in page"))
        });
        let error_line = |message: SharedString| {
            div()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(red_text)
                .child(message)
        };
        let middle: AnyElement = match &login.step {
            LoginStep::Browser { .. } => div()
                .mt(px(16.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(crate::loaders::gradient_spinner(
                    "login-poll",
                    &theme,
                    3.0,
                    cx.entity_id(),
                    cx,
                ))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.5))
                        .text_color(theme.text_muted)
                        .child(login.status()),
                )
                .into_any_element(),
            LoginStep::PasteCode { error, .. } => div()
                .mt(px(12.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    popover::dialog_field(self.code_input.clone().into_any_element())
                        .font_family(theme.font_mono.clone())
                        .text_size(crate::typography::ui_rems(13.0)),
                )
                .when_some(error.clone(), |el, message| el.child(error_line(message)))
                .into_any_element(),
            LoginStep::Failed { message } => div()
                .mt(px(16.0))
                .child(error_line(message.clone()))
                .into_any_element(),
        };
        let buttons = div()
            .mt(px(16.0))
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(8.0));
        let buttons = match &login.step {
            LoginStep::Browser { .. } => buttons.child(
                action("login-cancel", widgets::ActionTone::Quiet, "Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx))),
            ),
            LoginStep::PasteCode { submitting, .. } => {
                let submitting = *submitting;
                buttons
                    .child(
                        action("login-cancel", widgets::ActionTone::Quiet, "Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx))),
                    )
                    .child(
                        action(
                            "login-submit-code",
                            widgets::ActionTone::Solid,
                            if submitting {
                                "Verifying…"
                            } else {
                                "Add account"
                            },
                        )
                        .when(submitting, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_code(cx))),
                    )
            }
            LoginStep::Failed { .. } => buttons
                .child(
                    action("login-cancel", widgets::ActionTone::Quiet, "Close")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx))),
                )
                .child(
                    action("login-retry", widgets::ActionTone::Solid, "Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.retry_login(cx))),
                ),
        };
        let card = popover::dialog_card(&theme)
            .id("add-account-card")
            .role(gpui::Role::Dialog)
            .aria_label(title.clone())
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel_login(cx);
                    cx.stop_propagation();
                }
            }))
            .child(popover::dialog_title(&theme, &title))
            .child(div().mt(px(8.0)).child(popover::dialog_body(&theme, copy)))
            .children(reopen)
            .child(middle)
            .child(buttons)
            .into_any_element();
        Some(popover::modal("add-account-dialog", viewport, card))
    }

    /// A ghost account row (zeron settings.agents.tsx `SkeletonRow`): avatar,
    /// email line, two usage-meter ghosts, a badge — same geometry as the real
    /// row so loaded data lands without a layout jump. `dim` fades row two.
    fn render_skeleton_row(
        &self,
        _id: (&'static str, usize),
        dim: bool,
        first: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::motion;
        let delta = motion::pulse_delta(&motion::ZERON_PULSE, cx.entity_id(), cx);
        let ghost = |w: gpui::Length, h: f32, round_full: bool| {
            div()
                .w(w)
                .h(px(h))
                .flex_none()
                .map(|el| {
                    if round_full {
                        el.rounded_full()
                    } else {
                        el.rounded(px(4.0))
                    }
                })
                .bg(crate::theme::ink(0.05))
        };
        let meters = div()
            .mt(px(8.0))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .children((0..2).map(|_| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(ghost(px(48.0).into(), 9.0, false))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(56.0))
                            .max_w(px(230.0))
                            .h(px(5.0))
                            .rounded_full()
                            .bg(crate::theme::ink(0.04)),
                    )
                    .child(ghost(px(64.0).into(), 9.0, false))
            }));
        let inner = div()
            .flex()
            .flex_row()
            .items_stretch()
            .gap(px(12.0))
            .child(
                div()
                    .flex_none()
                    .self_center()
                    .size(px(32.0))
                    .rounded_full()
                    .bg(crate::theme::ink(0.05)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(ghost(px(176.0).into(), 13.0, false).max_w(gpui::relative(0.6)))
                    .child(meters),
            )
            .child(div().flex_none().flex().flex_col().items_end().child(ghost(
                px(64.0).into(),
                21.0,
                true,
            )));
        div()
            .px(px(20.0))
            .py(px(14.0))
            .when(!first, |el| el.border_t_1().border_color(theme.border))
            .when(dim, |el| el.opacity(0.6))
            .child(inner.opacity(0.55 + 0.35 * motion::pulse_wave(delta)))
            .into_any_element()
    }
}

impl popover::ScrollRailHost for AccountsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl AccountsPage {
    fn render_embedded_provider(
        &self,
        harness: HarnessId,
        theme: &Theme,
        dialog: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let refreshing = self.refreshing || matches!(self.snapshot, Loadable::Loading);
        let content: AnyElement =
            match &self.snapshot {
                Loadable::Idle | Loadable::Loading => self.render_embedded_skeleton(theme, cx),
                Loadable::Error(message) => widgets::error_strip(theme, message.clone())
                    .mt(px(4.0))
                    .id("accounts-inline-retry")
                    .role(gpui::Role::Button)
                    .aria_label("Retry loading accounts")
                    .tab_index(0)
                    .cursor_pointer()
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.load(force_usage_for(LoadTrigger::Retry), cx)
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.text_muted)
                            .child(SharedString::from("Retry")),
                    )
                    .into_any_element(),
                Loadable::Ready(snapshot) => {
                    let rows: Vec<_> = provider_accounts(snapshot, harness)
                        .into_iter()
                        .enumerate()
                        .map(|(ix, account)| {
                            self.render_account_row(account, ix, ix == 0, theme, cx)
                        })
                        .collect();
                    let empty = rows.is_empty();
                    // Adding is the list's last row, not a header button —
                    // it sits where the next account will appear. A
                    // one-login agent that is connected has nothing to add.
                    let add_row = (empty || !keeps_one_login(harness)).then(|| {
                        div()
                            .py(px(8.0))
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(4.0))
                            .when(!empty, |el| {
                                el.border_t_1().border_color(widgets::row_divider(theme))
                            })
                            .children(login_options(harness).into_iter().enumerate().map(
                                |(ix, option)| {
                                    let label = add_option_label(harness, option, empty);
                                    widgets::action_button(theme, widgets::ActionTone::Quiet)
                                        .id(("accounts-add", ix))
                                        .when(ix == 0, |el| el.ml(px(-10.0)))
                                        .role(gpui::Role::Button)
                                        .aria_label(label.clone())
                                        .tab_index(0)
                                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                                        .on_click(cx.listener(move |page, _, _, cx| {
                                            page.start_login(harness, option.provider, cx)
                                        }))
                                        .child(
                                            crate::icons::icon(crate::icons::PLUS)
                                                .size(px(14.0))
                                                .text_color(theme.text_muted),
                                        )
                                        .child(SharedString::from(label))
                                },
                            ))
                    });
                    div()
                        .flex()
                        .flex_col()
                        .children(rows)
                        .children(add_row)
                        .into_any_element()
                }
            };
        div()
            .id("accounts-embedded")
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(widgets::details_label(theme, "Accounts"))
                    .children(
                        self.render_accounts_status(cx)
                            .map(|status| div().ml(px(6.0)).flex_none().child(status)),
                    )
                    .child(div().flex_1())
                    .child(
                        widgets::action_button(theme, widgets::ActionTone::Quiet)
                            .id("accounts-refresh")
                            // Optically align the glyph with the badges'
                            // right edge below.
                            .mr(px(-8.0))
                            .w(px(32.0))
                            .px_0()
                            .justify_center()
                            .when(refreshing, |el| el.opacity(0.5))
                            .role(gpui::Role::Button)
                            .aria_label("Refresh accounts")
                            .tooltip(widgets::text_tooltip("Refresh usage"))
                            .tab_index(0)
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(force_usage_for(LoadTrigger::Refresh), cx)
                            }))
                            .child(
                                crate::icons::icon(crate::icons::REFRESH)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            ),
                    ),
            )
            .when_some(self.error.clone(), |el, message| {
                el.child(
                    widgets::error_strip(theme, message)
                        .mt(px(4.0))
                        .id("accounts-action-error")
                        .role(gpui::Role::Button)
                        .aria_label("Dismiss account error")
                        .tab_index(0)
                        .cursor_pointer()
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.error = None;
                            cx.notify();
                        })),
                )
            })
            .children(match &self.snapshot {
                Loadable::Ready(snapshot) => snapshot
                    .warnings
                    .iter()
                    .filter(|warning| warning.harness == harness)
                    .map(|warning| {
                        widgets::warning_strip(theme, warning.message.clone()).mt(px(4.0))
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .when_some(provider_note(harness), |el, note| {
                el.child(
                    div()
                        .mt(px(4.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(note)),
                )
            })
            .child(content)
            .when_some(dialog, |el, dialog| el.child(dialog))
            .into_any_element()
    }
}

impl Render for AccountsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let dialog = self.render_login_dialog(window.viewport_size(), cx);
        if let Some(harness) = self.embedded_harness {
            return self.render_embedded_provider(harness, &theme, dialog, cx);
        }
        let refreshing = self.refreshing || matches!(self.snapshot, Loadable::Loading);
        let account_count = self
            .snapshot
            .ready()
            .map(|s| s.accounts.len())
            .filter(|&n| n > 0);

        let provider_icon = |harness: HarnessId| match harness {
            HarnessId::Codex => (crate::icons::OPENAI_MARK, None),
            HarnessId::Cursor => (crate::icons::CURSOR_MARK, None),
            HarnessId::Devin => (crate::icons::DEVIN_MARK, None),
            HarnessId::Grok => (crate::icons::GROK_MARK, None),
            HarnessId::Hermes => (crate::icons::HERMES_MARK, None),
            HarnessId::Pi => (crate::icons::PI_MARK, None),
            HarnessId::Opencode => (crate::icons::OPENCODE_MARK, None),
            HarnessId::Antigravity => (crate::icons::ANTIGRAVITY_MARK, None),
            _ => (
                crate::icons::CLAUDE_MARK,
                Some(crate::icons::claude_brand()),
            ),
        };
        // Brand mark inside a 24px centered box (zeron: `grid size-6
        // place-items-center [&_svg]:size-4`).
        let provider_mark = |harness: HarnessId, theme: &Theme| {
            let (mark, tint) = provider_icon(harness);
            div()
                .flex_none()
                .size(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    crate::icons::icon(mark)
                        .size(px(16.0))
                        .text_color(tint.unwrap_or(theme.text_muted)),
                )
        };

        // One section per provider (zeron settings.agents.tsx `ProviderSection`):
        // brand header + Add account, then the account rows card.
        let sections: Vec<AnyElement> = match &self.snapshot {
            Loadable::Idle | Loadable::Loading => PROVIDERS
                .into_iter()
                .map(|(harness, name, _cli)| {
                    let skeleton_id = match harness {
                        HarnessId::Codex => "accounts-skeleton-codex",
                        HarnessId::Cursor => "accounts-skeleton-cursor",
                        HarnessId::Antigravity => "accounts-skeleton-antigravity",
                        HarnessId::Grok => "accounts-skeleton-grok",
                        HarnessId::Devin => "accounts-skeleton-devin",
                        HarnessId::Opencode => "accounts-skeleton-opencode",
                        HarnessId::Pi => "accounts-skeleton-pi",
                        HarnessId::Hermes => "accounts-skeleton-hermes",
                        _ => "accounts-skeleton-claude",
                    };
                    div()
                        .mt(px(24.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(provider_mark(harness, &theme))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(14.0))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme.text)
                                        .child(SharedString::from(name)),
                                ),
                        )
                        .child(
                            // Ghost rows shaped like real ones (row two dimmed)
                            // so the card keeps its size while data develops.
                            widgets::section_card(&theme)
                                .mt(px(8.0))
                                .child(self.render_skeleton_row(
                                    (skeleton_id, 0),
                                    false,
                                    true,
                                    &theme,
                                    cx,
                                ))
                                .child(self.render_skeleton_row(
                                    (skeleton_id, 1),
                                    true,
                                    false,
                                    &theme,
                                    cx,
                                )),
                        )
                        .into_any_element()
                })
                .collect(),
            Loadable::Error(message) => {
                let message = message.clone();
                vec![
                    widgets::error_strip(&theme, message)
                        .id("accounts-load-error")
                        .cursor_pointer()
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            // Retry IS the visit's first successful list — force usage.
                            this.load(force_usage_for(LoadTrigger::Retry), cx)
                        }))
                        .child(
                            div()
                                .mt(px(4.0))
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(SharedString::from("Click to retry")),
                        )
                        .into_any_element(),
                ]
            }
            Loadable::Ready(snapshot) => {
                let snapshot = snapshot.clone();
                PROVIDERS
                    .into_iter()
                    .map(|(harness, name, cli)| {
                        let accounts = provider_accounts(&snapshot, harness);
                        // EVERY warning renders its own strip (zeron maps them).
                        let warnings: Vec<String> = snapshot
                            .warnings
                            .iter()
                            .filter(|w| w.harness == harness)
                            .map(|w| w.message.clone())
                            .collect();
                        let rows: Vec<AnyElement> = accounts
                            .iter()
                            .enumerate()
                            .map(|(ix, account)| {
                                self.render_account_row(account, ix, ix == 0, &theme, cx)
                            })
                            .collect();
                        let add_id: SharedString = format!("add-account-{name}").into();
                        let empty = rows.is_empty();
                        let can_add = empty || !keeps_one_login(harness);
                        let card = widgets::section_card(&theme).mt(px(8.0));
                        let empty_copy = match harness {
                            // Cursor's app login is SEPARATE from `cursor-agent
                            // login` — pointing at the CLI would send users to a
                            // sign-in that does not light this up. Antigravity
                            // has no CLI login at all.
                            HarnessId::Cursor | HarnessId::Antigravity => format!(
                                "{name} isn't connected on this device — connect it to run \
                                 {name} sessions."
                            ),
                            _ => format!(
                                "No {name} login detected on this device — sign in \
                                 with \u{201C}{cli}\u{201D} or add an account."
                            ),
                        };
                        let card = if empty {
                            card.child(
                                div()
                                    .px(px(16.0))
                                    .py(px(16.0))
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(empty_copy)),
                            )
                        } else {
                            card.children(rows)
                        };
                        div()
                            .mt(px(16.0))
                            .p(px(16.0))
                            .rounded(px(12.0))
                            .bg(crate::theme::wash(0.035))
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(provider_mark(harness, &theme))
                                    .child(
                                        div()
                                            .text_size(crate::typography::ui_rems(14.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text)
                                            .child(SharedString::from(name)),
                                    )
                                    .child(div().flex_1())
                                    .when(can_add, |header| {
                                        header.children(
                                            login_options(harness).into_iter().enumerate().map(
                                                |(ix, option)| {
                                                    let label =
                                                        add_option_label(harness, option, empty);
                                                    widgets::ghost_action(&theme)
                                                        .id((add_id.clone(), ix))
                                                        .tab_index(0)
                                                        .role(gpui::Role::Button)
                                                        .focus_visible(|s| {
                                                            s.border_2()
                                                                .border_color(theme.accent)
                                                                .opacity(1.0)
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.start_login(
                                                                    harness,
                                                                    option.provider,
                                                                    cx,
                                                                );
                                                            },
                                                        ))
                                                        .child(
                                                            crate::icons::icon(
                                                                crate::icons::ADD_CIRCLE,
                                                            )
                                                            .size(px(16.0))
                                                            .text_color(theme.text_muted),
                                                        )
                                                        .child(SharedString::from(label))
                                                },
                                            ),
                                        )
                                    }),
                            )
                            .children(
                                warnings
                                    .into_iter()
                                    .map(|warning| widgets::warning_strip(&theme, warning)),
                            )
                            .children(provider_note(harness).map(|note| {
                                div()
                                    .mt(px(8.0))
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(note))
                            }))
                            .child(card)
                            .into_any_element()
                    })
                    .collect()
            }
        };

        let scrollbar = popover::rail(self, "accounts-page-scrollbar", &theme, cx);
        div()
            .id("accounts-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(16.0, true, true, div()
                    .id("accounts-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(widgets::page_header(&theme, "Accounts", account_count))
                                    .child(div().flex_1())
                                    .child(
                                        // `text-[12.5px]` + leading 16px Refresh icon,
                                        // dimmed while a refresh is in flight (zeron
                                        // `disabled:opacity-50`).
                                        widgets::ghost_action(&theme)
                                            .id("accounts-refresh")
                                            .flex_none()
                                            .text_size(crate::typography::ui_rems(12.5))
                                            .when(refreshing, |el| el.opacity(0.5))
                                            .tab_index(0)
.role(gpui::Role::Button)
.focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
.on_click(cx.listener(|this, _, _, cx| {
                                                this.load(
                                                    force_usage_for(LoadTrigger::Refresh),
                                                    cx,
                                                )
                                            }))
                                            .child(
                                                crate::icons::icon(crate::icons::REFRESH)
                                                    .size(px(16.0))
                                                    .text_color(theme.text_muted),
                                            )
                                            .child(SharedString::from("Refresh")),
                                    )
                                    .child(self.render_device_switcher(&theme, cx)),
                            )
                            .when_some(self.error.clone(), |el, message| {
                                el.child(
                                    widgets::error_strip(&theme, message)
                                        .id("accounts-action-error")
                                        .cursor_pointer()
                                        .tab_index(0)
.role(gpui::Role::Button)
.focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
.on_click(cx.listener(|this, _, _, cx| {
                                            this.error = None;
                                            cx.notify();
                                        })),
                                )
                            })
                            .children(sections)
                            // Footer note (zeron: `mt-6 text-[12px] leading-relaxed
                            // text-muted-foreground/60`).
                            .child(
                                div()
                                    .mt(px(24.0))
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .line_height(px(19.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(
                                        "Account changes affect new sessions. On macOS, Claude Code may take about 30 seconds to pick up a switched account.",
                                    )),
                            ),
                    )).fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .when_some(dialog, |el, dialog| el.child(dialog))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_web_and_loopback_login_urls_open() {
        assert!(is_openable_login_url("https://claude.com/cai/oauth/authorize?x=1"));
        assert!(is_openable_login_url("http://localhost:1455/auth/callback"));
        assert!(is_openable_login_url("http://127.0.0.1:8080/"));
        assert!(is_openable_login_url("http://[::1]:9000/cb"));
        assert!(!is_openable_login_url("http://evil.example/login"));
        assert!(!is_openable_login_url("http://localhost.evil.example/"));
        assert!(!is_openable_login_url("http://localhost@evil.example/"));
        assert!(!is_openable_login_url("file:///etc/passwd"));
        assert!(!is_openable_login_url("javascript:alert(1)"));
        assert!(!is_openable_login_url("vscode://extension/x"));
    }

    use super::*;
    use chrono::TimeDelta;

    #[test]
    fn first_load_of_a_visit_forces_the_usage_probe() {
        // The engine only probes usage when forced (M5c); without forcing on
        // mount, the first Accounts open always rendered "Usage unavailable".
        assert!(force_usage_for(LoadTrigger::Mount));
        // A retry after a failed load is still the visit's first successful
        // list — same requirement.
        assert!(force_usage_for(LoadTrigger::Retry));
        // Explicit refresh and a just-completed login always re-probe.
        assert!(force_usage_for(LoadTrigger::Refresh));
        assert!(force_usage_for(LoadTrigger::PostLogin));
        // Switch/Forget re-lists ride the still-warm 60s cache.
        assert!(!force_usage_for(LoadTrigger::PostAction));
    }

    #[test]
    fn usage_thresholds_match_zeron() {
        assert_eq!(usage_level(0.0), UsageLevel::Normal);
        assert_eq!(usage_level(0.79), UsageLevel::Normal);
        assert_eq!(usage_level(0.80), UsageLevel::Warn);
        assert_eq!(usage_level(0.94), UsageLevel::Warn);
        assert_eq!(usage_level(0.95), UsageLevel::Critical);
        assert_eq!(usage_level(1.0), UsageLevel::Critical);
    }

    #[test]
    fn usage_colors_map_to_theme_accents() {
        let theme = Theme::dark();
        assert_eq!(usage_color(UsageLevel::Normal, &theme), theme.accent);
        assert_eq!(usage_color(UsageLevel::Warn, &theme), theme.warning);
        assert_eq!(usage_color(UsageLevel::Critical, &theme), theme.danger);
    }

    #[test]
    fn reset_formatting_uses_full_local_date_and_time() {
        use chrono::{Datelike, Local, Timelike};
        assert_eq!(format_reset(None), None);
        let base: DateTime<Utc> = "2026-12-31T23:30:00Z".parse().unwrap();
        // Past, near-term, weekly and monthly resets all retain the full date.
        for offset in [-1, 0, 3, 26] {
            let at = base + TimeDelta::days(offset);
            let local = at.with_timezone(&Local);
            assert_eq!(
                format_reset(Some(at)),
                Some(format!(
                    "resets {:04}-{:02}-{:02} {:02}:{:02}",
                    local.year(),
                    local.month(),
                    local.day(),
                    local.hour(),
                    local.minute(),
                ))
            );
        }
    }

    #[test]
    fn every_sign_in_provider_shares_one_accounts_flow() {
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Antigravity,
            HarnessId::Grok,
            HarnessId::Devin,
        ] {
            assert!(signs_in(harness), "{harness:?}");
            assert!(add_account_label(harness, true).starts_with("Connect a "));
            assert_eq!(add_account_label(harness, false), "Add account");
            assert!(login_copy(harness, None).starts_with("Finish signing in to "));
            // One login per agent: one add button, no provider param.
            let options = login_options(harness);
            assert_eq!(options.len(), 1, "{harness:?}");
            assert_eq!(options[0].provider, None);
            assert_eq!(add_option_label(harness, options[0], false), "Add account");
        }
        assert_eq!(
            add_account_label(HarnessId::Antigravity, true),
            "Connect a Antigravity account"
        );
        // Every agent with a login of its own has an Accounts section.
        for harness in [HarnessId::Opencode, HarnessId::Pi, HarnessId::Hermes] {
            assert!(signs_in(harness), "{harness:?}");
            assert!(reports_usage(harness));
            for option in login_options(harness) {
                assert!(option.provider.is_some(), "{harness:?} names its provider");
                assert!(login_copy(harness, option.provider).starts_with("Finish signing in"));
            }
        }
        assert!(!signs_in(HarnessId::Mock));
        // Antigravity has no quota to show and keeps a single login.
        assert!(!reports_usage(HarnessId::Antigravity) && reports_usage(HarnessId::Codex));
        assert!(keeps_one_login(HarnessId::Antigravity) && !keeps_one_login(HarnessId::Cursor));
        // Hermes rotates its own pool; zeron never switches it, and says so.
        assert!(!switches_accounts(HarnessId::Hermes) && switches_accounts(HarnessId::Grok));
        assert!(provider_note(HarnessId::Hermes).is_some_and(|n| n.contains("hermes auth add")));
        assert!(provider_note(HarnessId::Grok).is_none());
    }

    #[test]
    fn per_provider_agents_offer_one_sign_in_per_provider() {
        let opencode = login_options(HarnessId::Opencode);
        let labels: Vec<_> = opencode.iter().map(|o| o.label).collect();
        assert_eq!(labels, ["ChatGPT", "GitHub Copilot"]);
        assert_eq!(
            add_option_label(HarnessId::Opencode, opencode[0], true),
            "Connect ChatGPT"
        );
        assert_eq!(
            add_option_label(HarnessId::Opencode, opencode[1], false),
            "Add GitHub Copilot account"
        );
        let providers: Vec<_> = login_options(HarnessId::Hermes)
            .iter()
            .map(|o| o.provider.unwrap())
            .collect();
        assert_eq!(providers, ["openai-codex", "nous"]);
        let flow = LoginFlow {
            provider: Some("openai-codex"),
            ..waiting(HarnessId::Pi, 1)
        };
        assert_eq!(flow.title(), "Sign in to ChatGPT for Pi");
        assert_eq!(waiting(HarnessId::Grok, 1).title(), "Sign in to Grok");
    }

    #[test]
    fn a_switch_only_moves_the_live_login_within_its_provider_group() {
        let row = |id: &str, harness, provider: Option<&str>, active| AgentAccount {
            id: id.into(),
            harness,
            email: None,
            plan_label: None,
            active,
            usage_windows: vec![],
            usage_fetched_at: None,
            usage_error: None,
            display_name: None,
            organization: None,
            auth_kind: None,
            switchable: true,
            saved_at: None,
            provider: provider.map(str::to_string),
        };
        let mut snapshot = AgentAccountsSnapshot {
            accounts: vec![
                row("gpt-a", HarnessId::Opencode, Some("openai"), true),
                row("gpt-b", HarnessId::Opencode, Some("openai"), false),
                row("copilot", HarnessId::Opencode, Some("github-copilot"), true),
                row("grok-a", HarnessId::Grok, None, true),
            ],
            warnings: vec![],
        };
        let target = snapshot.accounts[1].clone();
        mark_switched(&mut snapshot, &target);
        let live: Vec<&str> = snapshot
            .accounts
            .iter()
            .filter(|a| a.active)
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(live, ["gpt-b", "copilot", "grok-a"]);
    }

    fn page(cx: &mut gpui::TestAppContext) -> gpui::WindowHandle<AccountsPage> {
        use gpui::AppContext;
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            AccountsPage::new_embedded(state, None, HarnessId::Antigravity, cx)
        })
    }

    fn waiting(harness: HarnessId, attempt: u64) -> LoginFlow {
        LoginFlow {
            harness,
            provider: None,
            attempt,
            login_id: Some("login-1".into()),
            url: None,
            step: LoginStep::Browser { message: None },
        }
    }

    fn poll(status: AgentLoginStatus, message: Option<&str>, url: Option<&str>) -> AgentLoginPoll {
        AgentLoginPoll {
            status,
            message: message.map(str::to_string),
            url: url.map(str::to_string),
            callback_port: None,
        }
    }

    #[gpui::test]
    fn a_sign_in_ends_with_the_account_or_an_error_never_a_silent_reset(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = page(cx);
        window
            .update(cx, |page, _, cx| {
                // Starting: the dialog's progress line before any page.
                page.login = Some(waiting(HarnessId::Antigravity, 1));
                let flow = page.login.as_ref().unwrap();
                assert_eq!(flow.title(), "Sign in to Antigravity");
                assert_eq!(flow.status().as_ref(), "Starting sign-in…");
                // The page arrives with a poll: opened once, then waited on.
                let url = "https://accounts.google.com/o/oauth2/auth?x=1";
                assert!(!page.apply_poll(
                    1,
                    Ok(poll(AgentLoginStatus::Pending, None, Some(url))),
                    cx
                ));
                let flow = page.login.as_ref().unwrap();
                assert_eq!(flow.url.as_deref(), Some(url));
                assert_eq!(
                    flow.status().as_ref(),
                    "Waiting for you to finish in the browser…"
                );
                // A reply for an older attempt never touches this one.
                assert!(page.apply_poll(0, Ok(poll(AgentLoginStatus::Done, None, None)), cx));
                assert!(page.login.is_some());
                // An already-signed-in agent finishes at once: the dialog
                // closes and the list reloads to show the account.
                assert!(page.apply_poll(1, Ok(poll(AgentLoginStatus::Done, None, None)), cx));
                assert!(page.login.is_none());

                // A failure stays on screen, in the dialog, with Retry.
                page.login = Some(waiting(HarnessId::Codex, 2));
                assert!(page.apply_poll(
                    2,
                    Ok(poll(
                        AgentLoginStatus::Error,
                        Some("Port 1455 is in use"),
                        None
                    )),
                    cx
                ));
                assert_eq!(
                    page.login.as_ref().unwrap().step,
                    LoginStep::Failed {
                        message: "Port 1455 is in use".into()
                    }
                );
                // So does a lost poll or a start that never got going.
                page.login = Some(waiting(HarnessId::Cursor, 3));
                assert!(page.apply_poll(3, Err("engine gone".into()), cx));
                assert!(matches!(
                    &page.login.as_ref().unwrap().step,
                    LoginStep::Failed { message } if message.contains("engine gone")
                ));
                page.login = Some(waiting(HarnessId::ClaudeCode, 4));
                page.apply_start(4, Err("no network".into()), cx);
                assert!(matches!(
                    &page.login.as_ref().unwrap().step,
                    LoginStep::Failed { message } if message.contains("Couldn't start the sign-in")
                ));
                // Closing a failed dialog clears it.
                page.cancel_login(cx);
                assert!(page.login.is_none());
            })
            .unwrap();
    }

    /// A per-provider agent's section renders its rows (a live login, a
    /// saved one, an unidentified one) and one add button per provider,
    /// and a device-code sign-in's dialog shows the code line.
    #[gpui::test]
    fn per_provider_accounts_render_with_one_add_button_per_provider(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = page(cx);
        let row = |id: &str, email: &str, active, switchable| AgentAccount {
            id: id.into(),
            harness: HarnessId::Opencode,
            email: Some(email.into()),
            plan_label: Some("ChatGPT Plus".into()),
            active,
            usage_windows: vec![zeron_proto::AgentUsageWindow {
                label: "Session".into(),
                used_fraction: 0.4,
                resets_at: None,
            }],
            usage_fetched_at: None,
            usage_error: None,
            display_name: None,
            organization: None,
            auth_kind: None,
            switchable,
            saved_at: None,
            provider: Some("openai".into()),
        };
        window
            .update(cx, |page, _, cx| {
                page.set_embedded_harness(HarnessId::Opencode, cx);
                page.snapshot = Loadable::Ready(AgentAccountsSnapshot {
                    accounts: vec![
                        row("a", "a@example.com", true, true),
                        row("b", "b@example.com", false, true),
                        AgentAccount {
                            usage_windows: vec![],
                            provider: Some("github-copilot".into()),
                            ..row("c", "GitHub account", true, false)
                        },
                    ],
                    warnings: vec![],
                });
                page.login = Some(LoginFlow {
                    provider: Some("github-copilot"),
                    url: Some("https://github.com/login/device".into()),
                    step: LoginStep::Browser {
                        message: Some("Enter the code ABCD-1234 on GitHub.".into()),
                    },
                    ..waiting(HarnessId::Opencode, 1)
                });
                assert_eq!(
                    page.login.as_ref().unwrap().status().as_ref(),
                    "Enter the code ABCD-1234 on GitHub."
                );
                assert_eq!(
                    page.login.as_ref().unwrap().title(),
                    "Sign in to GitHub Copilot for OpenCode"
                );
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }

    #[gpui::test]
    fn claudes_paste_fallback_joins_the_same_dialog(cx: &mut gpui::TestAppContext) {
        let window = page(cx);
        window
            .update(cx, |page, _, cx| {
                page.login = Some(LoginFlow {
                    login_id: None,
                    ..waiting(HarnessId::ClaudeCode, 1)
                });
                page.apply_start(
                    1,
                    Ok(AgentLoginStart {
                        login_id: "login-1".into(),
                        url: "https://claude.ai/oauth/authorize?code=true".into(),
                        mode: AgentLoginMode::PasteCode,
                        callback_port: None,
                    }),
                    cx,
                );
                let flow = page.login.as_ref().unwrap();
                assert_eq!(flow.title(), "Sign in to Claude Code");
                assert_eq!(flow.login_id.as_deref(), Some("login-1"));
                assert_eq!(
                    flow.step,
                    LoginStep::PasteCode {
                        submitting: false,
                        error: None
                    }
                );
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }

    #[test]
    fn provider_grouping_keeps_engine_order_even_when_active_is_later() {
        let account = |id: &str, harness: HarnessId, active: bool| AgentAccount {
            id: id.into(),
            harness,
            email: None,
            plan_label: None,
            active,
            usage_windows: vec![],
            usage_fetched_at: None,
            usage_error: None,
            display_name: None,
            organization: None,
            auth_kind: None,
            switchable: true,
            saved_at: None,
            provider: None,
        };
        let snapshot = AgentAccountsSnapshot {
            accounts: vec![
                account("c1", HarnessId::ClaudeCode, false),
                account("x1", HarnessId::Codex, false),
                account("c2", HarnessId::ClaudeCode, true),
            ],
            warnings: vec![],
        };
        let claude = provider_accounts(&snapshot, HarnessId::ClaudeCode);
        let ids: Vec<&str> = claude.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            ["c1", "c2"],
            "engine (creation) order holds — switching must not move a card"
        );
        assert_eq!(provider_accounts(&snapshot, HarnessId::Codex).len(), 1);
        assert!(provider_accounts(&snapshot, HarnessId::Cursor).is_empty());
    }
}
