//! Settings → Cloud (docs/design/cloud-device.md "Product shape"): the
//! user's Cloud — one device in every list, with its own projects and
//! providers, where every session runs on its own machine that sleeps when
//! idle. The account (on/off), the sessions' machines (wake, sleep, delete),
//! GitHub for the repositories projects point at, usage, and the devices
//! allowed to use stored credentials. Cloud's Codex and Claude Code accounts
//! are managed in Settings → Providers with Cloud picked. Every call here is
//! local IPC — this device's engine talks to the edge with the user's bearer;
//! no session machine is dialed.
//! The page is rebuilt on every visit, so status is read once per open and
//! after each action; only settling sessions or a turn-off are re-checked.

use std::time::Duration;

use chrono::Utc;
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};

use zeron_proto::{
    CloudState, CloudStatus, CloudUsage, GithubConnectProgress, GithubConnectState,
    GithubDeviceFlow, VaultProvider, VaultStatus,
};
use zeron_rpc::methods;

use crate::cloud::{self, CloudAction, ProviderLink};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

enum GithubFlow {
    Idle,
    Starting,
    Waiting {
        flow: GithubDeviceFlow,
        copied: bool,
    },
    Failed(SharedString),
}

/// What the Cloud page last read, so a later visit paints at once and
/// revalidates in place instead of flashing loading states.
#[derive(Default)]
struct CloudPageCache {
    vault: Option<VaultStatus>,
    usage: Option<(cloud::UsageRange, CloudUsage)>,
}

impl gpui::Global for CloudPageCache {}

pub struct CloudPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    status: Loadable<CloudStatus>,
    vault: Loadable<VaultStatus>,
    /// Credits over `usage_range`; `None` hides the section (not loaded,
    /// unavailable, or an engine without the call).
    usage: Option<CloudUsage>,
    usage_range: cloud::UsageRange,
    range_select: widgets::SelectState,
    busy: Option<CloudAction>,
    confirm_delete: bool,
    github: GithubFlow,
    /// Vault call in flight per row, so its button reads busy.
    vault_busy: Option<SharedString>,
    error: Option<SharedString>,
    status_task: Option<Task<()>>,
    recheck_task: Option<Task<()>>,
    usage_task: Option<Task<()>>,
    vault_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    vault_action_task: Option<Task<()>>,
    github_task: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    _observe: Subscription,
}

impl CloudPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.on_state_changed(cx));
        // Paint what is known (the last status any surface read, this page's
        // last vault and usage reads), then revalidate.
        let status = state
            .read(cx)
            .cloud_status
            .clone()
            .map_or(Loadable::Idle, Loadable::Ready);
        let cache = cx.try_global::<CloudPageCache>();
        let vault = cache
            .and_then(|cache| cache.vault.clone())
            .map_or(Loadable::Idle, Loadable::Ready);
        let (usage_range, usage) = cache
            .and_then(|cache| cache.usage.clone())
            .map_or((cloud::UsageRange::default(), None), |(range, usage)| {
                (range, Some(usage))
            });
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            status,
            vault,
            usage,
            usage_range,
            range_select: widgets::SelectState::default(),
            busy: None,
            confirm_delete: false,
            github: GithubFlow::Idle,
            vault_busy: None,
            error: None,
            status_task: None,
            recheck_task: None,
            usage_task: None,
            vault_task: None,
            action_task: None,
            vault_action_task: None,
            github_task: None,
            copy_task: None,
            _observe: observe,
        };
        page.load_status(cx);
        if page.cloud_id().is_some() {
            page.load_vault(cx);
        }
        page.load_usage(cx);
        page
    }

    /// Escape that reached Settings unclaimed closes the delete confirmation
    /// first. Returns whether it did.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.confirm_delete {
            return false;
        }
        self.confirm_delete = false;
        cx.notify();
        true
    }

    fn cloud_status(&self) -> Option<&CloudStatus> {
        self.status.ready()
    }

    /// The logical Cloud device, while Cloud is on: what providers are
    /// authorized for (it covers every session machine).
    fn cloud_id(&self) -> Option<String> {
        self.cloud_status()
            .filter(|status| cloud::account_enabled(status))
            .and_then(|status| status.device_id.clone())
    }

    /// Sessions and chat titles live in `AppState`; repaint when they move.
    fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    // ---- loads ----

    fn load_status(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.status = Loadable::Error("Engine not connected".into());
            return;
        };
        if !matches!(self.status, Loadable::Ready(_)) {
            self.status = Loadable::Loading;
        }
        self.status_task = Some(cx.spawn(async move |this, cx| {
            let result = cloud::fetch_status(engine).await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(status) => page.apply_status(status, cx),
                    // A failed re-read keeps the painted status, and a
                    // settling one keeps being followed.
                    Err(error) if matches!(page.status, Loadable::Ready(_)) => {
                        page.error = Some(error.into());
                        page.schedule_recheck(cx);
                    }
                    Err(error) => page.status = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Take a `CloudStatus` (a read or an action's reply): publish it for
    /// every device list, load what depends on the device existing, and
    /// re-check slowly while it settles.
    fn apply_status(&mut self, status: CloudStatus, cx: &mut Context<Self>) {
        let had_device = self.cloud_id();
        let enabled = cloud::account_enabled(&status);
        self.state.update(cx, |state, cx| {
            state.set_cloud_status(status.clone(), cx);
            if enabled {
                state.refresh_cloud_sessions(cx);
                state.refresh_cloud_repos(cx);
            }
        });
        // A finished delete has recorded the machine's final usage.
        let delete_settled = status.state != CloudState::Deleting
            && self
                .cloud_status()
                .is_some_and(|previous| previous.state == CloudState::Deleting);
        self.status = Loadable::Ready(status);
        let has_device = self.cloud_id();
        if has_device.is_some() && (had_device != has_device || self.vault.ready().is_none()) {
            self.load_vault(cx);
        }
        if delete_settled {
            self.load_usage(cx);
        }
        self.schedule_recheck(cx);
    }

    /// Only turning Cloud off settles on its own (every session machine is
    /// stopped, metered and deleted) and is re-read until it lands; a
    /// settled account is never polled. Sessions are followed by `AppState`.
    fn schedule_recheck(&mut self, cx: &mut Context<Self>) {
        let recheck = self
            .cloud_status()
            .and_then(|status| cloud::account_recheck_after(status.state));
        self.recheck_task = recheck.map(|after| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(after).await;
                this.update(cx, |page, cx| page.load_status(cx)).ok();
            })
        });
    }

    /// Metered machine time this month. Read on open and after lifecycle
    /// actions only; any failure just hides the section.
    fn load_usage(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let range = self.usage_range;
        let params = range.params(Utc::now().date_naive());
        self.usage_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<CloudUsage>(methods::CLOUD_USAGE, params)
                .await;
            this.update(cx, |page, cx| {
                if page.usage_range != range {
                    return;
                }
                page.usage = result.ok().filter(|usage| usage.available);
                cx.default_global::<CloudPageCache>().usage =
                    page.usage.clone().map(|usage| (range, usage));
                cx.notify();
            })
            .ok();
        }));
    }

    /// Show credits over another range (the chart keeps the last range's
    /// bars until the new ones land).
    fn set_usage_range(&mut self, range: cloud::UsageRange, cx: &mut Context<Self>) {
        widgets::close_select(self, |page: &mut Self| &mut page.range_select, cx);
        if self.usage_range == range {
            return;
        }
        self.usage_range = range;
        self.load_usage(cx);
        cx.notify();
    }

    fn load_vault(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if !matches!(self.vault, Loadable::Ready(_)) {
            self.vault = Loadable::Loading;
        }
        self.vault_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<VaultStatus>(methods::VAULT_STATUS, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(vault) => {
                        cx.default_global::<CloudPageCache>().vault = Some(vault.clone());
                        page.vault = Loadable::Ready(vault);
                    }
                    Err(error) if matches!(page.vault, Loadable::Ready(_)) => {
                        page.error = Some(error.to_string().into());
                    }
                    Err(error) => page.vault = Loadable::Error(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // ---- lifecycle ----

    /// The switch: on turns Cloud on at once; off asks first (it deletes
    /// every session's machine).
    fn toggle_cloud(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            self.confirm_delete = true;
            cx.notify();
        } else {
            self.run(CloudAction::Enable, cx);
        }
    }

    fn run(&mut self, action: CloudAction, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.busy = Some(action);
        self.error = None;
        if action == CloudAction::Delete {
            self.confirm_delete = false;
        }
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<CloudStatus>(action.method(), serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.busy = None;
                match result {
                    Ok(status) => {
                        if action == CloudAction::Delete {
                            page.vault = Loadable::Idle;
                            page.github = GithubFlow::Idle;
                            page.github_task = None;
                            cx.default_global::<CloudPageCache>().vault = None;
                        }
                        page.apply_status(status, cx);
                        page.load_usage(cx);
                    }
                    Err(error) => {
                        page.error =
                            Some(format!("{} failed: {error}", action_name(action)).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ---- providers ----

    fn authorized_with(&self, provider: VaultProvider, cloud_id: &str) -> Vec<String> {
        self.vault.ready().map_or_else(
            || vec![cloud_id.to_string()],
            |vault| cloud::authorized_with(vault, provider, cloud_id),
        )
    }

    /// Stop `provider` being used on Cloud. A credential that other devices
    /// still use only loses Cloud's access; otherwise it is removed.
    fn disconnect(&mut self, provider: VaultProvider, cx: &mut Context<Self>) {
        let Some(cloud_id) = self.cloud_id() else {
            return;
        };
        let others: Vec<String> = self
            .vault
            .ready()
            .and_then(|vault| cloud::connection(vault, provider))
            .map(|c| {
                c.authorized_devices
                    .iter()
                    .filter(|d| **d != cloud_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let (method, params) = if others.is_empty() {
            (
                methods::VAULT_DISCONNECT,
                serde_json::json!({ "provider": provider.as_str() }),
            )
        } else {
            (
                methods::VAULT_AUTHORIZE,
                serde_json::json!({ "provider": provider.as_str(), "authorizedDevices": others }),
            )
        };
        self.vault_call(provider.as_str().into(), method, params, cx);
    }

    /// Allow an already-stored credential on the Cloud device.
    fn authorize(&mut self, provider: VaultProvider, cx: &mut Context<Self>) {
        let Some(cloud_id) = self.cloud_id() else {
            return;
        };
        let params = serde_json::json!({
            "provider": provider.as_str(),
            "authorizedDevices": self.authorized_with(provider, &cloud_id),
        });
        self.vault_call(
            provider.as_str().into(),
            methods::VAULT_AUTHORIZE,
            params,
            cx,
        );
    }

    /// One credential mutation whose reply is the fresh `VaultStatus`.
    fn vault_call(
        &mut self,
        row: SharedString,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if self.vault_busy.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.vault_busy = Some(row);
        self.error = None;
        self.vault_action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call_as::<VaultStatus>(method, params).await;
            this.update(cx, |page, cx| {
                page.vault_busy = None;
                match result {
                    Ok(vault) => page.vault = Loadable::Ready(vault),
                    Err(error) => page.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ---- GitHub device flow ----

    fn start_github(&mut self, cx: &mut Context<Self>) {
        let (Some(engine), Some(cloud_id)) =
            (self.state.read(cx).engine().cloned(), self.cloud_id())
        else {
            return;
        };
        let devices = self.authorized_with(VaultProvider::Github, &cloud_id);
        self.github = GithubFlow::Starting;
        self.error = None;
        self.github_task = Some(cx.spawn(async move |this, cx| {
            let started = engine
                .client()
                .call_as::<GithubDeviceFlow>(
                    methods::GITHUB_CONNECT_START,
                    serde_json::json!({ "authorizedDevices": devices }),
                )
                .await;
            let flow = match started {
                Ok(flow) => flow,
                Err(error) => {
                    this.update(cx, |page, cx| {
                        page.github = GithubFlow::Failed(error.to_string().into());
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let interval = Duration::from_secs(flow.interval_secs.max(1));
            let expires_at = flow.expires_at;
            let flow_id = flow.flow_id.clone();
            if this
                .update(cx, |page, cx| {
                    page.github = GithubFlow::Waiting {
                        flow,
                        copied: false,
                    };
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            // Poll until GitHub answers. Dropping this task (Cancel, or the
            // page closing) stops it.
            let mut failures = 0;
            let outcome = loop {
                cx.background_executor().timer(interval).await;
                if expires_at > 0 && Utc::now().timestamp_millis() > expires_at {
                    break Err("The code expired before it was approved.".to_string());
                }
                let polled = engine
                    .client()
                    .call_as::<GithubConnectProgress>(
                        methods::GITHUB_CONNECT_POLL,
                        serde_json::json!({ "flowId": flow_id }),
                    )
                    .await;
                match polled {
                    Ok(progress) => match progress.state {
                        GithubConnectState::Pending => failures = 0,
                        GithubConnectState::Connected => break Ok(()),
                        GithubConnectState::Failed => {
                            break Err(progress.error.unwrap_or_else(|| {
                                "GitHub didn't approve the connection.".into()
                            }));
                        }
                    },
                    // Ride out a blip; give up when GitHub stays unreachable.
                    Err(error) => {
                        failures += 1;
                        if failures >= 3 {
                            break Err(error.to_string());
                        }
                    }
                }
            };
            this.update(cx, |page, cx| {
                match outcome {
                    Ok(()) => {
                        page.github = GithubFlow::Idle;
                        page.load_vault(cx);
                        // Projects on these repositories can now run on Cloud.
                        page.state
                            .update(cx, |state, cx| state.refresh_cloud_repos(cx));
                    }
                    Err(message) => page.github = GithubFlow::Failed(message.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn cancel_github(&mut self, cx: &mut Context<Self>) {
        self.github_task = None;
        self.github = GithubFlow::Idle;
        cx.notify();
    }

    fn copy_github_code(&mut self, cx: &mut Context<Self>) {
        let GithubFlow::Waiting { flow, copied } = &mut self.github else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(flow.user_code.clone()));
        *copied = true;
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |page, cx| {
                if let GithubFlow::Waiting { copied, .. } = &mut page.github {
                    *copied = false;
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Open GitHub's code page, with the code already on the clipboard.
    fn open_github(&mut self, cx: &mut Context<Self>) {
        let GithubFlow::Waiting { flow, .. } = &self.github else {
            return;
        };
        let uri = flow.verification_uri.clone();
        self.copy_github_code(cx);
        crate::settings::accounts::open_login_url(&uri, cx);
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

/// The browser sign-in call and brand for a subscription provider.
fn action_name(action: CloudAction) -> &'static str {
    match action {
        CloudAction::Enable => "Turning on Cloud",
        CloudAction::Delete => "Turning off Cloud",
    }
}

impl popover::ScrollRailHost for CloudPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

// ---- rendering ----

/// A settings action with keyboard focus and an accessible role. Disabled
/// actions dim and drop their click.
fn action(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    tone: widgets::ActionTone,
    label: impl Into<SharedString>,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let accent = theme.accent;
    widgets::text_action(theme, tone, label)
        .id(id)
        .tab_index(0)
        .role(gpui::Role::Button)
        .focus_visible(move |s| s.border_2().border_color(accent).opacity(1.0))
        .when(!enabled, |el| el.opacity(0.5).cursor_default())
}

/// The 36px brand tile the Providers page uses for a row's identity.
fn tile(theme: &Theme, icon_path: &'static str, tint: Option<gpui::Hsla>) -> gpui::Div {
    div()
        .flex_none()
        .size(px(36.0))
        .rounded(px(10.0))
        .bg(theme.wash(0.06))
        .flex()
        .items_center()
        .justify_center()
        .child(
            crate::icons::icon(icon_path)
                .size(px(16.0))
                .text_color(tint.unwrap_or(theme.text_muted)),
        )
}

/// A short line under a section's card.
fn footnote(theme: &Theme, copy: &'static str) -> gpui::Div {
    div()
        .px(px(4.0))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text_muted.opacity(0.75))
        .child(SharedString::from(copy))
}

fn text(color: gpui::Hsla, copy: impl Into<SharedString>) -> AnyElement {
    div()
        .text_color(color)
        .child(copy.into())
        .into_any_element()
}

/// Row body: tile, title over meta, actions on the trailing edge.
fn row(
    theme: &Theme,
    first: bool,
    leading: Option<gpui::Div>,
    title: &str,
    meta: Vec<AnyElement>,
    note: Option<SharedString>,
    actions: Vec<AnyElement>,
) -> gpui::Div {
    widgets::card_row(theme, first)
        .children(leading)
        .child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .child(widgets::row_title(theme, title.to_string()))
                .when(!meta.is_empty(), |el| {
                    el.child(widgets::meta_line(theme, meta))
                })
                .when_some(note, |el, note| {
                    el.child(widgets::meta_line(
                        theme,
                        vec![text(theme.text_muted.opacity(0.75), note)],
                    ))
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.0))
                .children(actions),
        )
}

fn busy_meta(
    theme: &Theme,
    key: &'static str,
    label: impl Into<SharedString>,
    cx: &mut Context<CloudPage>,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(crate::loaders::mini_mono_spinner(
            key,
            1.5,
            theme.text_muted,
            cx.entity_id(),
            cx,
        ))
        .child(label.into())
        .into_any_element()
}

impl CloudPage {
    fn render_status(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let leading = Some(tile(theme, crate::icons::CLOUD, None));
        let status = match &self.status {
            Loadable::Idle | Loadable::Loading => {
                let meta = vec![busy_meta(
                    theme,
                    "cloud-status-loading",
                    "Checking Cloud…",
                    cx,
                )];
                return widgets::section_card(theme)
                    .mt(px(28.0))
                    .child(row(theme, true, leading, "Cloud", meta, None, Vec::new()))
                    .into_any_element();
            }
            Loadable::Error(error) => {
                let copy = if error.starts_with("unknown method") {
                    "This version of Zeron can't manage Cloud yet.".to_string()
                } else {
                    format!("Couldn't check Cloud: {error}")
                };
                let retry = action(
                    theme,
                    "cloud-status-retry",
                    widgets::ActionTone::Filled,
                    "Retry",
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.status = Loadable::Idle;
                    this.load_status(cx);
                }))
                .into_any_element();
                return widgets::section_card(theme)
                    .mt(px(28.0))
                    .child(row(
                        theme,
                        true,
                        leading,
                        "Cloud",
                        vec![text(theme.danger_muted.opacity(0.9), copy)],
                        None,
                        vec![retry],
                    ))
                    .into_any_element();
            }
            Loadable::Ready(status) => status.clone(),
        };
        let busy = self.busy;
        if !status.available {
            return widgets::section_card(theme)
                .mt(px(28.0))
                .child(row(
                    theme,
                    true,
                    leading,
                    "Cloud",
                    vec![text(
                        theme.text_muted,
                        "Cloud needs a signed-in account with sync turned on.",
                    )],
                    None,
                    Vec::new(),
                ))
                .into_any_element();
        }
        let on = cloud::account_enabled(&status);
        let mut actions: Vec<AnyElement> = Vec::new();
        let meta: Vec<AnyElement> = if let Some(action) = busy {
            vec![busy_meta(
                theme,
                "cloud-status-busy",
                action.busy_label(),
                cx,
            )]
        } else {
            match status.state {
                CloudState::Deleting => vec![busy_meta(
                    theme,
                    "cloud-status-settling",
                    cloud::account_state_label(CloudState::Deleting),
                    cx,
                )],
                CloudState::Error => {
                    let retry_with = cloud::retry_action(&status);
                    actions.push(
                        action(
                            theme,
                            "cloud-retry",
                            widgets::ActionTone::Quiet,
                            "Retry",
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.run(retry_with, cx)))
                        .into_any_element(),
                    );
                    vec![text(
                        theme.danger_muted.opacity(0.9),
                        status
                            .error
                            .clone()
                            .unwrap_or_else(|| "Something went wrong with Cloud.".into()),
                    )]
                }
                // The switch says on or off; the row only adds what it can't.
                _ if on => {
                    let mut meta = Vec::new();
                    match status.credits {
                        Some(credits) if credits <= 0.0 => {
                            meta.push(text(theme.warning_muted.opacity(0.9), "Out of credits"))
                        }
                        Some(credits) => meta.push(text(
                            theme.text_muted,
                            format!("{} left", cloud::format_credits(credits)),
                        )),
                        None => {}
                    }
                    meta
                }
                _ => Vec::new(),
            }
        };
        // On ⇄ off. Turning it off deletes every machine, so it asks first.
        let interactive = busy.is_none() && status.state != CloudState::Deleting;
        let accent = theme.accent;
        actions.push(
            widgets::toggle_switch(theme, on, "cloud-enabled")
                .id("cloud-switch")
                .when(!interactive, |el| el.opacity(0.55))
                .when(interactive, |el| {
                    el.cursor_pointer()
                        .tab_index(0)
                        .role(gpui::Role::Switch)
                        .aria_label("Cloud")
                        .aria_toggled(if on {
                            gpui::Toggled::True
                        } else {
                            gpui::Toggled::False
                        })
                        .focus_visible(move |s| s.border_2().border_color(accent).opacity(1.0))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_cloud(on, cx)))
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if !event.is_held
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            {
                                this.toggle_cloud(on, cx);
                                cx.stop_propagation();
                            }
                        }))
                })
                .into_any_element(),
        );
        let note: Option<SharedString> = (status.state == CloudState::Off && busy.is_none())
            .then(|| "Nothing is created until you turn it on.".into());
        widgets::section_card(theme)
            .mt(px(28.0))
            .child(row(theme, true, leading, "Cloud", meta, note, actions))
            .into_any_element()
    }

    fn disconnect_action(
        &self,
        theme: &Theme,
        provider: VaultProvider,
        label: &'static str,
        idle: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        action(
            theme,
            SharedString::from(format!("disconnect-{}", provider.as_str())),
            widgets::ActionTone::Quiet,
            if busy { "Removing…" } else { label },
            idle,
        )
        .when(idle, |el| {
            el.on_click(cx.listener(move |this, _, _, cx| this.disconnect(provider, cx)))
        })
        .into_any_element()
    }

    /// GitHub, for the repositories Cloud sessions clone and push to. Always
    /// shown while signed in; with Cloud off it is dimmed and says why.
    fn render_github(
        &mut self,
        cloud_id: Option<&str>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let vault = self.vault.ready().filter(|v| v.available).cloned();
        if !self.cloud_status().is_some_and(|status| status.available) {
            return None;
        }
        let idle = self.vault_busy.is_none();
        let busy = self.vault_busy.as_deref() == Some(VaultProvider::Github.as_str());
        let mut meta: Vec<AnyElement> = Vec::new();
        let mut actions: Vec<AnyElement> = Vec::new();
        let connect = |label: &'static str, enabled: bool, cx: &mut Context<Self>| {
            action(
                theme,
                "github-connect",
                widgets::ActionTone::Filled,
                label,
                enabled,
            )
            .when(enabled, |el| {
                el.on_click(cx.listener(|this, _, _, cx| this.start_github(cx)))
            })
            .into_any_element()
        };
        let link = match (cloud_id, vault.as_ref()) {
            (Some(cloud_id), Some(vault)) => {
                Some(cloud::provider_link(vault, VaultProvider::Github, cloud_id))
            }
            _ => None,
        };
        match (&self.github, &link) {
            (_, None) if cloud_id.is_none() => {
                meta.push(text(theme.text_muted, "Turn on Cloud to connect GitHub."));
                actions.push(connect("Connect", false, cx));
            }
            (_, None) => {
                meta.push(busy_meta(theme, "github-loading", "Checking GitHub…", cx));
            }
            (GithubFlow::Starting, _) => {
                meta.push(busy_meta(
                    theme,
                    "github-starting",
                    "Contacting GitHub…",
                    cx,
                ));
            }
            (GithubFlow::Waiting { .. }, _) => {
                meta.push(busy_meta(
                    theme,
                    "github-waiting",
                    "Waiting for you to approve on GitHub…",
                    cx,
                ));
                actions.push(
                    action(
                        theme,
                        "github-cancel",
                        widgets::ActionTone::Quiet,
                        "Cancel",
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_github(cx)))
                    .into_any_element(),
                );
            }
            (GithubFlow::Failed(message), _) => {
                meta.push(text(theme.danger_muted.opacity(0.9), message.clone()));
                actions.push(connect("Try again", true, cx));
            }
            (GithubFlow::Idle, Some(ProviderLink::Connected { account })) => {
                meta.push(text(
                    theme.text_muted,
                    account.clone().unwrap_or_else(|| "Connected".into()),
                ));
                if let Some(url) = vault.as_ref().and_then(|v| v.github_install_url.clone()) {
                    actions.push(
                        action(
                            theme,
                            "github-install",
                            widgets::ActionTone::Quiet,
                            "Repositories",
                            true,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            crate::settings::accounts::open_login_url(&url, cx);
                        }))
                        .into_any_element(),
                    );
                }
                actions.push(self.disconnect_action(
                    theme,
                    VaultProvider::Github,
                    "Disconnect",
                    idle,
                    busy,
                    cx,
                ));
            }
            (GithubFlow::Idle, Some(ProviderLink::NeedsReconnect { account })) => {
                meta.push(text(theme.warning_muted.opacity(0.9), "Needs reconnect"));
                if let Some(account) = account {
                    meta.push(text(theme.text_muted, account.clone()));
                }
                actions.push(connect("Reconnect", true, cx));
            }
            // Connected for other devices only: connecting just lets Cloud
            // use it (no new sign-in).
            (GithubFlow::Idle, Some(ProviderLink::NotAuthorized { account })) => {
                if let Some(account) = account {
                    meta.push(text(theme.text_muted, account.clone()));
                }
                actions.push(
                    action(
                        theme,
                        "github-authorize",
                        widgets::ActionTone::Filled,
                        if busy { "Connecting…" } else { "Connect" },
                        idle,
                    )
                    .when(idle, |el| {
                        el.on_click(
                            cx.listener(|this, _, _, cx| this.authorize(VaultProvider::Github, cx)),
                        )
                    })
                    .into_any_element(),
                );
            }
            (GithubFlow::Idle, Some(_)) => {
                meta.push(text(theme.text_muted, "Not connected"));
                actions.push(connect("Connect", true, cx));
            }
        }
        let code = match &self.github {
            GithubFlow::Waiting { flow, copied } => {
                Some(self.render_github_code(flow, *copied, theme, cx))
            }
            _ => None,
        };
        let enabled = cloud_id.is_some();
        let block = widgets::section_card(theme)
            .mt(px(0.0))
            .child(
                row(
                    theme,
                    true,
                    Some(tile(theme, crate::icons::GITHUB_MARK, None)),
                    "GitHub",
                    meta,
                    None,
                    actions,
                )
                .when(!enabled, |el| el.opacity(0.6)),
            )
            .children(code);
        Some(widgets::section(theme, "GitHub", block).into_any_element())
    }

    /// The device-flow code, large enough to read across to the browser.
    fn render_github_code(
        &self,
        flow: &GithubDeviceFlow,
        copied: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .mx(px(16.0))
            .pl(px(36.0 + 16.0))
            .pb(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(widgets::details_label(theme, "Enter this code on GitHub"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .id("github-user-code")
                            .px(px(14.0))
                            .py(px(8.0))
                            .rounded(px(10.0))
                            .bg(theme.wash(0.06))
                            .font_family(theme.font_mono.clone())
                            .text_size(crate::typography::ui_rems(22.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .aria_label(format!("GitHub code {}", flow.user_code))
                            .child(SharedString::from(flow.user_code.clone())),
                    )
                    .child(
                        action(
                            theme,
                            "github-copy-code",
                            widgets::ActionTone::Quiet,
                            if copied { "Copied" } else { "Copy code" },
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.copy_github_code(cx))),
                    )
                    .child(
                        action(
                            theme,
                            "github-open",
                            widgets::ActionTone::Solid,
                            "Open GitHub",
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.open_github(cx))),
                    ),
            )
            .into_any_element()
    }

    /// Credits: used over the picked range against what's left, and a bar
    /// per day.
    fn render_usage(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let usage = self.usage.clone()?;
        let selected = cloud::UsageRange::ALL
            .iter()
            .position(|range| *range == self.usage_range)
            .unwrap_or(0);
        let range_select = widgets::select(
            "cloud-usage-range",
            "Usage range",
            theme,
            |page: &mut Self| &mut page.range_select,
        )
        .options(
            cloud::UsageRange::ALL
                .iter()
                .map(|range| widgets::SelectOption::new(range.label())),
            selected,
        )
        // Fits the longest label, so switching never resizes the trigger.
        .width(128.0)
        .on_select(|page, ix, _, cx| {
            if let Some(range) = cloud::UsageRange::ALL.get(ix) {
                page.set_usage_range(*range, cx);
            }
        })
        .render(&self.range_select, cx);
        let fractions = cloud::bar_fractions(&usage.days);
        let gap = if usage.days.len() > 45 { 1.0 } else { 3.0 };
        let bar_color = theme.accent.opacity(0.85);
        let empty_color = theme.wash(0.06);
        let bars = usage
            .days
            .iter()
            .zip(fractions)
            .enumerate()
            .map(|(ix, (day, fraction))| {
                let ran = day.credits > 0.0;
                div()
                    .id(("cloud-usage-bar", ix))
                    .flex_1()
                    .min_w(px(1.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .tooltip(widgets::text_tooltip(format!(
                        "{} · {}",
                        cloud::short_day(&day.day),
                        cloud::format_credits(day.credits)
                    )))
                    .child(
                        div()
                            .w_full()
                            // A day that ran stays visible next to a busy one.
                            .h(if ran {
                                gpui::relative(fraction.max(0.04))
                            } else {
                                px(2.0).into()
                            })
                            .rounded(px(2.0))
                            .bg(if ran { bar_color } else { empty_color }),
                    )
            });
        let axis = |day: Option<&zeron_proto::CloudUsageDay>| {
            day.map(|d| cloud::short_day(&d.day)).unwrap_or_default()
        };
        let mut meta = vec![text(
            theme.text_muted,
            format!("{} left", cloud::format_credits(usage.balance)),
        )];
        if usage.balance <= 0.0 {
            meta = vec![text(theme.warning_muted.opacity(0.9), "Out of credits")];
        }
        let block = widgets::section_card(theme)
            .mt(px(0.0))
            .child(
                widgets::card_row(theme, true)
                    .min_h(px(52.0))
                    .py(px(10.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(
                                theme,
                                format!("{} used", cloud::format_credits(usage.credits)),
                            ))
                            .child(widgets::meta_line(theme, meta)),
                    )
                    .child(range_select),
            )
            .child(
                div()
                    .px(px(16.0))
                    .pb(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("cloud-usage-chart")
                            .h(px(88.0))
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap(px(gap))
                            .children(bars),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted.opacity(0.75))
                            .child(axis(usage.days.first()))
                            .child(axis(usage.days.last())),
                    ),
            );
        Some(
            widgets::section(theme, "Usage", block)
                .child(footnote(
                    theme,
                    "A credit is a minute of a small machine. Machines only use credits while awake.",
                ))
                .into_any_element(),
        )
    }

    fn render_delete_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.confirm_delete {
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let accent = theme.accent;
        let card = popover::dialog_card(&theme)
            .id("delete-cloud-card")
            .role(gpui::Role::AlertDialog)
            .aria_label("Turn off Cloud")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.confirm_delete = false;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(popover::dialog_title(&theme, "Turn off Cloud?"))
            .child(div().mt(px(8.0)).child(popover::dialog_body(
                &theme,
                "This permanently deletes every session's machine and everything on them, \
                 including work that hasn't been pushed. It can't be undone.",
            )))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "delete-cloud-cancel")
                            .id("delete-cloud-cancel")
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(move |s| s.border_2().border_color(accent))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_delete = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_danger(&theme, "Turn off Cloud")
                            .id("delete-cloud-confirm")
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(move |s| s.border_2().border_color(accent))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.run(CloudAction::Delete, cx)),
                            ),
                    ),
            )
            .into_any_element();
        Some(popover::modal("delete-cloud-dialog", viewport, card))
    }
}

impl Render for CloudPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let dialog = self.render_delete_dialog(window.viewport_size(), cx);
        let status = self.render_status(&theme, cx);
        let usage = self.render_usage(&theme, cx);
        let cloud_id = self
            .cloud_status()
            .filter(|s| s.available)
            .and_then(|_| self.cloud_id());
        let github = self.render_github(cloud_id.as_deref(), &theme, cx);
        let scrollbar = popover::rail(self, "cloud-page-scrollbar", &theme, cx);
        div()
            .id("cloud-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("cloud-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "Cloud", None))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    "Run your agents in cloud sandboxes.",
                                ))
                                .when_some(self.error.clone(), |el, message| {
                                    el.child(
                                        widgets::error_strip(&theme, message)
                                            .id("cloud-error")
                                            .cursor_pointer()
                                            .tab_index(0)
                                            .role(gpui::Role::Button)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.error = None;
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(status)
                                .children(usage)
                                .children(github),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .when_some(dialog, |el, dialog| el.child(dialog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(
        provider: VaultProvider,
        status: zeron_proto::VaultConnectionStatus,
        devices: &[&str],
        account: &str,
    ) -> zeron_proto::VaultConnection {
        zeron_proto::VaultConnection {
            provider,
            status,
            authorized_devices: devices.iter().map(|d| d.to_string()).collect(),
            account: Some(account.into()),
            updated_at: 0,
            accounts: 1,
            has_active: true,
        }
    }

    /// Every lifecycle state and provider standing paints without a panic,
    /// with the GitHub code and the delete dialog open.
    #[gpui::test]
    fn every_state_renders(cx: &mut gpui::TestAppContext) {
        use zeron_proto::VaultConnectionStatus::{Connected, NeedsReconnect};
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.local_device_id = Some("mac".into());
            state.devices = serde_json::from_value(serde_json::json!([
                {"id": "mac", "name": "Studio", "platform": "macos", "lastSeenAt": null},
                {"id": "cloud-1", "name": "Cloud", "platform": "cloud", "lastSeenAt": null,
                    "capabilities": ["cloud-account"]},
            ]))
            .unwrap();
            state
        });
        let (page, cx) = cx.add_window_view(|_, cx| CloudPage::new(state.clone(), cx));
        let vaults = [
            VaultStatus {
                available: true,
                ..Default::default()
            },
            VaultStatus {
                available: true,
                connections: vec![
                    connection(
                        VaultProvider::Codex,
                        Connected,
                        &["cloud-1"],
                        "me@example.com",
                    ),
                    connection(
                        VaultProvider::Claude,
                        NeedsReconnect,
                        &["cloud-1"],
                        "me@example.com",
                    ),
                    connection(VaultProvider::Github, Connected, &["cloud-1"], "octocat"),
                ],
                devices: vec![
                    zeron_proto::VaultDevice {
                        device_id: "cloud-s1".into(),
                        kind: "cloud".into(),
                        enrolled_at: 0,
                        revoked_at: None,
                        parent_id: Some("cloud-1".into()),
                    },
                    zeron_proto::VaultDevice {
                        device_id: "old".into(),
                        kind: "laptop".into(),
                        enrolled_at: 0,
                        revoked_at: Some(1),
                        parent_id: None,
                    },
                ],
                github_install_url: Some("https://github.com/apps/zeron/installations/new".into()),
            },
            VaultStatus {
                available: true,
                connections: vec![
                    connection(VaultProvider::OpenaiKey, Connected, &["cloud-1"], "…abcd"),
                    connection(
                        VaultProvider::AnthropicKey,
                        Connected,
                        &["cloud-1"],
                        "…wxyz",
                    ),
                    connection(VaultProvider::Github, Connected, &["cloud-old"], "octocat"),
                ],
                devices: Vec::new(),
                github_install_url: None,
            },
            VaultStatus::default(),
        ];
        let states = [
            CloudState::Off,
            CloudState::Provisioning,
            CloudState::Starting,
            CloudState::Ready,
            CloudState::Sleeping,
            CloudState::Stopping,
            CloudState::Deleting,
            CloudState::Error,
        ];
        for (ix, cloud_state) in states.into_iter().enumerate() {
            for available in [true, false] {
                let vault = vaults[ix % vaults.len()].clone();
                page.update(cx, |page, cx| {
                    page.status = Loadable::Ready(CloudStatus {
                        state: cloud_state,
                        device_id: Some("cloud-1".into()),
                        error: Some("Sandbox failed to start".into()),
                        failed_action: Some(["enable", "delete"][ix % 2].into()),
                        last_active_at: Some(0),
                        awake_sessions: 2,
                        max_awake_sessions: 5,
                        available,
                        credits: [Some(1240.0), Some(0.0), None][ix % 3],
                    });
                    page.vault = Loadable::Ready(vault);
                    let days = if ix % 2 == 0 { 7 } else { 90 };
                    page.usage = Some(CloudUsage {
                        from: "2026-07-08".into(),
                        to: "2026-10-05".into(),
                        days: (0..days)
                            .map(|d| zeron_proto::CloudUsageDay {
                                day: format!("2026-10-{:02}", d % 28 + 1),
                                credits: [0.0, 12.0, 340.0][d % 3],
                            })
                            .collect(),
                        credits: 1240.0,
                        balance: [500.0, -3.0][ix % 2],
                        available: true,
                    });
                    page.confirm_delete = ix % 2 == 0;
                    page.github = match ix % 3 {
                        0 => GithubFlow::Waiting {
                            flow: GithubDeviceFlow {
                                flow_id: "f".into(),
                                user_code: "WDJB-MJHT".into(),
                                verification_uri: "https://github.com/login/device".into(),
                                interval_secs: 5,
                                expires_at: 0,
                            },
                            copied: false,
                        },
                        1 => GithubFlow::Failed("Denied".into()),
                        _ => GithubFlow::Idle,
                    };
                    cx.notify();
                });
                cx.update(|window, cx| window.draw(cx).clear());
            }
        }
        // A delete is followed while Deleting and stops once it lands.
        page.update(cx, |page, cx| {
            let deleting = CloudStatus {
                state: CloudState::Deleting,
                device_id: Some("cloud-1".into()),
                available: true,
                ..Default::default()
            };
            page.apply_status(deleting, cx);
            assert!(page.recheck_task.is_some(), "deleting is re-read");
            assert!(
                page.cloud_id().is_none(),
                "nothing to manage while deleting"
            );
            page.apply_status(
                CloudStatus {
                    state: CloudState::Off,
                    available: true,
                    ..Default::default()
                },
                cx,
            );
            assert!(page.recheck_task.is_none(), "off is never polled");
            let failed = CloudStatus {
                state: CloudState::Error,
                device_id: Some("cloud-1".into()),
                failed_action: Some("delete".into()),
                available: true,
                ..Default::default()
            };
            assert_eq!(cloud::retry_action(&failed), CloudAction::Delete);
            page.apply_status(failed, cx);
            assert!(page.recheck_task.is_none());
        });
        cx.update(|window, cx| window.draw(cx).clear());
        // Loading and failure before any status.
        for status in [
            Loadable::Loading,
            Loadable::Error("unknown method: CloudStatus".into()),
            Loadable::Error("edge unreachable".into()),
        ] {
            page.update(cx, |page, cx| {
                page.status = status;
                cx.notify();
            });
            cx.update(|window, cx| window.draw(cx).clear());
        }
    }
}
