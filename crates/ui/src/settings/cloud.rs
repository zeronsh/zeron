//! Settings → Cloud (docs/design/cloud-device.md "Product shape"): the
//! user's Cloud — one device in every list, with its own projects and
//! providers, where every session runs on its own machine that sleeps when
//! idle. The account (on/off), the sessions' machines (wake, sleep, delete),
//! the two providers connected *for* Cloud (Codex and Claude Code), GitHub
//! for the repositories projects point at, usage, and the devices allowed to
//! use stored credentials.
//!
//! Codex and Claude sign in through the browser on THIS computer and are kept
//! for Cloud, so every session machine can use them; an API key is the
//! alternative. Every call here is local IPC — this device's engine talks to
//! the edge with the user's bearer; no session machine is dialed.
//! The page is rebuilt on every visit, so status is read once per open and
//! after each action; only settling sessions or a turn-off are re-checked.

use std::time::Duration;

use chrono::Utc;
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, Focusable, SharedString, Subscription, Task,
    Window, div, prelude::*, px,
};

use zeron_proto::{
    AgentLoginMode, AgentLoginPoll, AgentLoginStart, AgentLoginStatus, CloudState, CloudStatus,
    CloudUsage, GithubConnectProgress, GithubConnectState, GithubDeviceFlow, HarnessId,
    VaultProvider, VaultStatus,
};
use zeron_rpc::methods;

use crate::cloud::{self, CloudAction, ProviderLink};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// How often a browser sign-in is asked whether it has landed (the
/// Providers page's cadence).
const SIGN_IN_POLL: Duration = Duration::from_millis(1500);

/// The inline API-key field under a provider row.
struct ApiKeyEditor {
    provider: VaultProvider,
    input: Entity<ComposerInput>,
    saving: bool,
    error: Option<SharedString>,
    focus_pending: bool,
    _events: Subscription,
}

/// A browser sign-in on THIS computer whose result goes to the vault for
/// Cloud (`VaultConnectCodex` / `VaultConnectClaude`), driven like a
/// Providers-page login: poll until it lands, or paste Claude's code.
struct SignInFlow {
    provider: VaultProvider,
    attempt: u64,
    login_id: Option<String>,
    url: Option<String>,
    step: SignInStep,
}

enum SignInStep {
    Starting,
    Browser {
        message: Option<SharedString>,
    },
    /// Claude without a loopback port: the user pastes the code Anthropic
    /// shows.
    PasteCode {
        input: Entity<ComposerInput>,
        submitting: bool,
        error: Option<SharedString>,
        focus_pending: bool,
        _events: Subscription,
    },
}

enum GithubFlow {
    Idle,
    Starting,
    Waiting {
        flow: GithubDeviceFlow,
        copied: bool,
    },
    Failed(SharedString),
}

pub struct CloudPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    status: Loadable<CloudStatus>,
    vault: Loadable<VaultStatus>,
    /// This month's metered machine time; `None` hides the section (not
    /// loaded, unavailable, or an engine without the call).
    usage: Option<CloudUsage>,
    busy: Option<CloudAction>,
    confirm_delete: bool,
    /// The browser sign-in (Codex or Claude) this computer is running.
    sign_in: Option<SignInFlow>,
    sign_in_attempts: u64,
    api_key: Option<ApiKeyEditor>,
    github: GithubFlow,
    /// Vault call in flight per row, so its button reads busy.
    vault_busy: Option<SharedString>,
    error: Option<SharedString>,
    status_task: Option<Task<()>>,
    recheck_task: Option<Task<()>>,
    usage_task: Option<Task<()>>,
    vault_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    sign_in_task: Option<Task<()>>,
    sign_in_poll: Option<Task<()>>,
    vault_action_task: Option<Task<()>>,
    github_task: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    _observe: Subscription,
}

impl CloudPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.on_state_changed(cx));
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            status: Loadable::Idle,
            vault: Loadable::Idle,
            usage: None,
            busy: None,
            confirm_delete: false,
            sign_in: None,
            sign_in_attempts: 0,
            api_key: None,
            github: GithubFlow::Idle,
            vault_busy: None,
            error: None,
            status_task: None,
            recheck_task: None,
            usage_task: None,
            vault_task: None,
            action_task: None,
            sign_in_task: None,
            sign_in_poll: None,
            vault_action_task: None,
            github_task: None,
            copy_task: None,
            _observe: observe,
        };
        page.load_status(cx);
        page.load_usage(cx);
        page
    }

    /// Escape that reached Settings unclaimed closes the delete confirmation
    /// or the API-key field first. Returns whether it did.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.confirm_delete {
            self.confirm_delete = false;
        } else if self.api_key.is_some() {
            self.api_key = None;
        } else if self.sign_in.is_some() {
            self.cancel_sign_in(cx);
        } else {
            return false;
        }
        cx.notify();
        true
    }

    fn signing_in(&self, provider: VaultProvider) -> bool {
        self.sign_in
            .as_ref()
            .is_some_and(|f| f.provider == provider)
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
        self.usage_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<CloudUsage>(methods::CLOUD_USAGE, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.usage = result.ok().filter(|usage| usage.available);
                cx.notify();
            })
            .ok();
        }));
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
                    Ok(vault) => page.vault = Loadable::Ready(vault),
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
                            page.api_key = None;
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

    /// Sign in to ChatGPT (Codex) or Claude on THIS computer, into a
    /// throwaway credential slot, and keep the result for Cloud. The
    /// computer's own login is untouched. The start returns the sign-in page;
    /// the flow then runs like a Providers-page login on this same engine.
    fn connect_subscription(&mut self, provider: VaultProvider, cx: &mut Context<Self>) {
        let (Some(engine), Some(cloud_id)) =
            (self.state.read(cx).engine().cloned(), self.cloud_id())
        else {
            return;
        };
        let Some((method, _)) = subscription_sign_in(provider) else {
            return;
        };
        if self.sign_in.is_some() {
            return;
        }
        let devices = self.authorized_with(provider, &cloud_id);
        self.sign_in_attempts += 1;
        let attempt = self.sign_in_attempts;
        self.sign_in = Some(SignInFlow {
            provider,
            attempt,
            login_id: None,
            url: None,
            step: SignInStep::Starting,
        });
        self.sign_in_poll = None;
        self.error = None;
        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<AgentLoginStart>(
                    method,
                    serde_json::json!({ "authorizedDevices": devices }),
                )
                .await
                .map_err(|error| error.to_string());
            this.update(cx, |page, cx| page.apply_sign_in_start(attempt, result, cx))
                .ok();
        }));
        cx.notify();
    }

    fn apply_sign_in_start(
        &mut self,
        attempt: u64,
        result: Result<AgentLoginStart, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(flow) = self.sign_in.as_mut().filter(|f| f.attempt == attempt) else {
            return;
        };
        let brand = subscription_sign_in(flow.provider).map_or("", |(_, brand)| brand);
        match result {
            Ok(start) => {
                if !start.url.is_empty() {
                    crate::settings::accounts::open_login_url(&start.url, cx);
                    flow.url = Some(start.url.clone());
                }
                flow.login_id = Some(start.login_id);
                match start.mode {
                    AgentLoginMode::PasteCode => {
                        let input = cx
                            .new(|cx| ComposerInput::new("Paste the code", cx).with_single_line());
                        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| {
                            if matches!(event, ComposerInputEvent::Submitted) {
                                this.submit_sign_in_code(cx);
                            }
                        });
                        flow.step = SignInStep::PasteCode {
                            input,
                            submitting: false,
                            error: None,
                            focus_pending: true,
                            _events: events,
                        };
                    }
                    AgentLoginMode::Browser => {
                        flow.step = SignInStep::Browser { message: None };
                        self.spawn_sign_in_poll(cx);
                    }
                }
            }
            Err(error) => {
                self.sign_in = None;
                self.error = Some(format!("Couldn't start the {brand} sign-in: {error}").into());
            }
        }
        cx.notify();
    }

    /// Ask every [`SIGN_IN_POLL`] whether the browser sign-in has landed in
    /// the vault. Dropping the task (Cancel, the page closing) stops asking;
    /// the sign-in itself still completes if the user finishes it.
    fn spawn_sign_in_poll(&mut self, cx: &mut Context<Self>) {
        let Some(SignInFlow {
            login_id: Some(login_id),
            attempt,
            ..
        }) = &self.sign_in
        else {
            return;
        };
        let attempt = *attempt;
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = serde_json::json!({ "loginId": login_id });
        self.sign_in_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SIGN_IN_POLL).await;
                let result = engine
                    .client()
                    .call_as::<AgentLoginPoll>(methods::POLL_AGENT_LOGIN, params.clone())
                    .await
                    .map_err(|error| error.to_string());
                match this.update(cx, |page, cx| page.apply_sign_in_poll(attempt, result, cx)) {
                    Ok(false) => {}
                    Ok(true) | Err(_) => break,
                }
            }
        }));
    }

    /// Fold one poll in; `true` once polling is over.
    fn apply_sign_in_poll(
        &mut self,
        attempt: u64,
        result: Result<AgentLoginPoll, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(flow) = self.sign_in.as_mut().filter(|f| f.attempt == attempt) else {
            return true;
        };
        let brand = subscription_sign_in(flow.provider).map_or("", |(_, brand)| brand);
        let finished = match result {
            Ok(poll) => match poll.status {
                AgentLoginStatus::Done => {
                    self.sign_in = None;
                    self.load_vault(cx);
                    true
                }
                AgentLoginStatus::Error => {
                    self.sign_in = None;
                    self.error = Some(
                        poll.message
                            .unwrap_or_else(|| format!("The {brand} sign-in failed."))
                            .into(),
                    );
                    true
                }
                AgentLoginStatus::Pending => {
                    if let Some(url) = poll.url.filter(|url| flow.url.as_ref() != Some(url)) {
                        crate::settings::accounts::open_login_url(&url, cx);
                        flow.url = Some(url);
                    }
                    flow.step = SignInStep::Browser {
                        message: poll.message.map(Into::into),
                    };
                    false
                }
            },
            Err(error) => {
                self.sign_in = None;
                self.error = Some(format!("Lost track of the {brand} sign-in: {error}").into());
                true
            }
        };
        cx.notify();
        finished
    }

    /// Claude's paste-code fallback: hand the code back to finish the
    /// sign-in, then re-read the vault.
    fn submit_sign_in_code(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(SignInFlow {
            login_id: Some(login_id),
            attempt,
            step:
                SignInStep::PasteCode {
                    input,
                    submitting,
                    error,
                    ..
                },
            ..
        }) = &mut self.sign_in
        else {
            return;
        };
        if *submitting {
            return;
        }
        let code = input.read(cx).text().trim().to_string();
        if code.is_empty() {
            *error = Some("Paste the code first.".into());
            cx.notify();
            return;
        }
        *submitting = true;
        *error = None;
        let attempt = *attempt;
        let params = serde_json::json!({ "loginId": login_id, "code": code });
        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::COMPLETE_AGENT_LOGIN, params)
                .await;
            this.update(cx, |page, cx| {
                if page.sign_in.as_ref().is_none_or(|f| f.attempt != attempt) {
                    return;
                }
                match result {
                    Ok(_) => {
                        page.sign_in = None;
                        page.load_vault(cx);
                    }
                    Err(failure) => {
                        if let Some(SignInFlow {
                            step:
                                SignInStep::PasteCode {
                                    submitting, error, ..
                                },
                            ..
                        }) = &mut page.sign_in
                        {
                            *submitting = false;
                            *error = Some(failure.to_string().into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Stop the browser sign-in on this computer (best effort).
    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        let login_id = self.sign_in.take().and_then(|flow| flow.login_id);
        self.sign_in_task = None;
        self.sign_in_poll = None;
        if let (Some(login_id), Some(engine)) = (login_id, self.state.read(cx).engine().cloned()) {
            cx.spawn(async move |_, _| {
                if let Err(error) = engine
                    .client()
                    .call(
                        methods::CANCEL_AGENT_LOGIN,
                        serde_json::json!({ "loginId": login_id }),
                    )
                    .await
                {
                    tracing::debug!(%error, "CancelAgentLogin failed (best-effort)");
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn authorized_with(&self, provider: VaultProvider, cloud_id: &str) -> Vec<String> {
        self.vault.ready().map_or_else(
            || vec![cloud_id.to_string()],
            |vault| cloud::authorized_with(vault, provider, cloud_id),
        )
    }

    fn open_api_key(&mut self, provider: VaultProvider, cx: &mut Context<Self>) {
        let placeholder = match provider {
            VaultProvider::AnthropicKey => "Anthropic API key",
            _ => "OpenAI API key",
        };
        let input = cx.new(|cx| {
            ComposerInput::new(placeholder, cx)
                .with_single_line()
                .with_masked()
        });
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.save_api_key(cx);
            }
        });
        self.api_key = Some(ApiKeyEditor {
            provider,
            input,
            saving: false,
            error: None,
            focus_pending: true,
            _events: events,
        });
        cx.notify();
    }

    fn save_api_key(&mut self, cx: &mut Context<Self>) {
        let (Some(engine), Some(cloud_id)) =
            (self.state.read(cx).engine().cloned(), self.cloud_id())
        else {
            return;
        };
        let Some(editor) = self.api_key.as_ref() else {
            return;
        };
        if editor.saving {
            return;
        }
        let key = editor.input.read(cx).text().trim().to_string();
        let provider = editor.provider;
        if key.is_empty() {
            if let Some(editor) = self.api_key.as_mut() {
                editor.error = Some("Paste a key first.".into());
            }
            cx.notify();
            return;
        }
        let params = serde_json::json!({
            "provider": provider.as_str(),
            "key": key,
            "authorizedDevices": self.authorized_with(provider, &cloud_id),
        });
        if let Some(editor) = self.api_key.as_mut() {
            editor.saving = true;
            editor.error = None;
        }
        self.vault_action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_as::<VaultStatus>(methods::VAULT_PUT_API_KEY, params)
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(vault) => {
                        page.vault = Loadable::Ready(vault);
                        page.api_key = None;
                    }
                    Err(error) => {
                        if let Some(editor) = page.api_key.as_mut() {
                            editor.saving = false;
                            editor.error = Some(error.to_string().into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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

    fn revoke_device(&mut self, device_id: String, cx: &mut Context<Self>) {
        let params = serde_json::json!({ "deviceId": device_id });
        self.vault_call(device_id.into(), methods::VAULT_REVOKE_DEVICE, params, cx);
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
fn subscription_sign_in(provider: VaultProvider) -> Option<(&'static str, &'static str)> {
    match provider {
        VaultProvider::Codex => Some((methods::VAULT_CONNECT_CODEX, "ChatGPT")),
        VaultProvider::Claude => Some((methods::VAULT_CONNECT_CLAUDE, "Claude")),
        _ => None,
    }
}

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

/// What distinguishes one subscription provider row from another.
struct SubscriptionRow {
    harness: HarnessId,
    title: &'static str,
    subscription: VaultProvider,
    key: VaultProvider,
    /// Whose account the browser sign-in uses ("ChatGPT", "Claude").
    brand: &'static str,
    key_label: &'static str,
}

/// A provider row's pieces, rendered once its position in the block is known.
struct RowParts {
    leading: Option<gpui::Div>,
    title: &'static str,
    meta: Vec<AnyElement>,
    note: Option<SharedString>,
    actions: Vec<AnyElement>,
}

impl RowParts {
    fn render(self, theme: &Theme, first: bool) -> gpui::Div {
        row(
            theme,
            first,
            self.leading,
            self.title,
            self.meta,
            self.note,
            self.actions,
        )
    }
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
        let label_for = |action: CloudAction, idle: &'static str| -> &'static str {
            if busy == Some(action) {
                action.busy_label()
            } else {
                idle
            }
        };
        let (meta, note, actions): (Vec<AnyElement>, Option<SharedString>, Vec<AnyElement>) =
            if !status.available {
                (
                    vec![text(
                        theme.text_muted,
                        "Cloud needs a signed-in account with sync turned on.",
                    )],
                    Some("Sign in to Zeron on this computer to use Cloud.".into()),
                    Vec::new(),
                )
            } else {
                match status.state {
                    CloudState::Off => (
                        vec![text(
                            theme.text_muted,
                            "Run Codex and Claude Code on your GitHub repositories in the \
                             cloud — each session on its own machine, asleep when idle — even \
                             while your computers are off.",
                        )],
                        Some("Nothing is created and no sign-in leaves this computer until you turn it on.".into()),
                        vec![
                            action(
                                theme,
                                "cloud-enable",
                                widgets::ActionTone::Solid,
                                label_for(CloudAction::Enable, "Turn on Cloud"),
                                busy.is_none(),
                            )
                            .when(busy.is_none(), |el| {
                                el.on_click(cx.listener(|this, _, _, cx| this.run(CloudAction::Enable, cx)))
                            })
                            .into_any_element(),
                        ],
                    ),
                    CloudState::Deleting => (
                        vec![busy_meta(
                            theme,
                            "cloud-status-settling",
                            cloud::account_state_label(CloudState::Deleting),
                            cx,
                        )],
                        Some(
                            "Stopping every session's machine, then deleting them and their files."
                                .into(),
                        ),
                        Vec::new(),
                    ),
                    CloudState::Error => {
                        let retry_with = cloud::retry_action(&status);
                        let message = status
                            .error
                            .clone()
                            .unwrap_or_else(|| "Something went wrong with Cloud.".into());
                        (
                            vec![text(theme.danger_muted.opacity(0.9), message)],
                            None,
                            vec![
                                action(
                                    theme,
                                    "cloud-retry",
                                    widgets::ActionTone::Filled,
                                    label_for(retry_with, "Retry"),
                                    busy.is_none(),
                                )
                                .when(busy.is_none(), |el| {
                                    el.on_click(cx.listener(move |this, _, _, cx| this.run(retry_with, cx)))
                                })
                                .into_any_element(),
                            ],
                        )
                    }
                    _ => {
                        let mut meta = vec![text(theme.success_muted, "On")];
                        if let Some(at) = status.last_active_at {
                            meta.push(text(
                                theme.text_muted,
                                format!("Active {}", cloud::ago(at, Utc::now())),
                            ));
                        }
                        (
                            meta,
                            Some(
                                "Pick Cloud in a project's checkout menu to run a session on its \
                                 own machine. Machines sleep when idle and wake when you send."
                                    .into(),
                            ),
                            Vec::new(),
                        )
                    }
                }
            };
        widgets::section_card(theme)
            .mt(px(28.0))
            .child(row(theme, true, leading, "Cloud", meta, note, actions))
            .into_any_element()
    }

    /// Claude's paste-code field, inset under its row while that step is up.
    fn render_sign_in_code(
        &mut self,
        provider: VaultProvider,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(SignInFlow {
            step:
                SignInStep::PasteCode {
                    input,
                    submitting,
                    error,
                    focus_pending,
                    ..
                },
            ..
        }) = self.sign_in.as_mut().filter(|f| f.provider == provider)
        else {
            return None;
        };
        if std::mem::take(focus_pending) {
            window.focus(&input.focus_handle(cx), cx);
        }
        let (input, submitting, error) = (input.clone(), *submitting, error.clone());
        Some(
            div()
                .mx(px(16.0))
                .pl(px(36.0 + 16.0))
                .pb(px(14.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div().flex_1().min_w(px(160.0)).child(
                                popover::dialog_field(input.into_any_element())
                                    .font_family(theme.font_mono.clone()),
                            ),
                        )
                        .child(
                            action(
                                theme,
                                "sign-in-code-submit",
                                widgets::ActionTone::Solid,
                                if submitting {
                                    "Checking…"
                                } else {
                                    "Finish sign-in"
                                },
                                !submitting,
                            )
                            .when(!submitting, |el| {
                                el.on_click(
                                    cx.listener(|this, _, _, cx| this.submit_sign_in_code(cx)),
                                )
                            }),
                        ),
                )
                .when_some(error, |el, error| {
                    el.child(
                        div()
                            .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                            .text_color(theme.danger_muted.opacity(0.9))
                            .child(error),
                    )
                })
                .into_any_element(),
        )
    }

    /// The masked key field, inset under its provider row.
    fn render_api_key_editor(
        &mut self,
        provider: VaultProvider,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.api_key.as_mut().filter(|e| e.provider == provider)?;
        if std::mem::take(&mut editor.focus_pending) {
            window.focus(&editor.input.focus_handle(cx), cx);
        }
        let saving = editor.saving;
        let error = editor.error.clone();
        let input = editor.input.clone();
        Some(
            div()
                .mx(px(16.0))
                .pl(px(36.0 + 16.0))
                .pb(px(14.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(160.0))
                                .child(popover::dialog_field(input.into_any_element())),
                        )
                        .child(
                            action(theme, "api-key-cancel", widgets::ActionTone::Quiet, "Cancel", true)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.api_key = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            action(
                                theme,
                                "api-key-save",
                                widgets::ActionTone::Solid,
                                if saving { "Saving…" } else { "Save key" },
                                !saving,
                            )
                            .when(!saving, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| this.save_api_key(cx)))
                            }),
                        ),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                        .text_color(theme.text_muted.opacity(0.75))
                        .child(SharedString::from(
                            "Stored encrypted in your account and used only on the devices you allow.",
                        )),
                )
                .when_some(error, |el, error| {
                    el.child(
                        div()
                            .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                            .text_color(theme.danger_muted.opacity(0.9))
                            .child(error),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_providers(
        &mut self,
        cloud_id: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let block = widgets::section_card(theme);
        let vault = match &self.vault {
            Loadable::Ready(vault) if vault.available => vault.clone(),
            Loadable::Ready(_) => {
                return widgets::section(
                    theme,
                    "Providers on Cloud",
                    block.mt(px(0.0)).child(row(
                        theme,
                        true,
                        None,
                        "Sign-ins for Cloud aren't available right now",
                        vec![text(
                            theme.text_muted,
                            "Your account can't store provider sign-ins at the moment. Try again later.",
                        )],
                        None,
                        Vec::new(),
                    )),
                )
                .into_any_element();
            }
            Loadable::Error(error) => {
                let error = error.clone();
                return widgets::section(
                    theme,
                    "Providers on Cloud",
                    block.mt(px(0.0)).child(row(
                        theme,
                        true,
                        None,
                        "Couldn't load providers",
                        vec![text(theme.danger_muted.opacity(0.9), error)],
                        None,
                        vec![
                            action(
                                theme,
                                "vault-retry",
                                widgets::ActionTone::Filled,
                                "Retry",
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.vault = Loadable::Idle;
                                this.load_vault(cx);
                            }))
                            .into_any_element(),
                        ],
                    )),
                )
                .into_any_element();
            }
            Loadable::Idle | Loadable::Loading => {
                return widgets::section(
                    theme,
                    "Providers on Cloud",
                    block.mt(px(0.0)).child(row(
                        theme,
                        true,
                        None,
                        "Providers",
                        vec![busy_meta(theme, "vault-loading", "Loading…", cx)],
                        None,
                        Vec::new(),
                    )),
                )
                .into_any_element();
            }
        };
        let codex = self.subscription_row(
            SubscriptionRow {
                harness: HarnessId::Codex,
                title: "Codex",
                subscription: VaultProvider::Codex,
                key: VaultProvider::OpenaiKey,
                brand: "ChatGPT",
                key_label: "OpenAI API key",
            },
            &vault,
            cloud_id,
            theme,
            cx,
        );
        let claude = self.subscription_row(
            SubscriptionRow {
                harness: HarnessId::ClaudeCode,
                title: "Claude Code",
                subscription: VaultProvider::Claude,
                key: VaultProvider::AnthropicKey,
                brand: "Claude",
                key_label: "Anthropic API key",
            },
            &vault,
            cloud_id,
            theme,
            cx,
        );
        let codex_editor = self.render_api_key_editor(VaultProvider::OpenaiKey, theme, window, cx);
        let claude_editor =
            self.render_api_key_editor(VaultProvider::AnthropicKey, theme, window, cx);
        let claude_code = self.render_sign_in_code(VaultProvider::Claude, theme, window, cx);
        let block = block
            .mt(px(0.0))
            .child(codex.render(theme, true))
            .children(codex_editor)
            .child(claude.render(theme, false))
            .children(claude_code)
            .children(claude_editor);
        widgets::section(theme, "Providers on Cloud", block).into_any_element()
    }

    /// One subscription provider: a browser sign-in on this computer
    /// (stored for Cloud) or the user's own API key.
    fn subscription_row(
        &self,
        spec: SubscriptionRow,
        vault: &VaultStatus,
        cloud_id: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> RowParts {
        let SubscriptionRow {
            harness,
            title,
            subscription,
            key,
            brand,
            key_label,
        } = spec;
        let link = cloud::provider_link(vault, subscription, cloud_id);
        let key_link = cloud::provider_link(vault, key, cloud_id);
        let idle = self.vault_busy.is_none();
        let row_busy =
            |provider: VaultProvider| self.vault_busy.as_deref() == Some(provider.as_str());
        let id = |suffix: &str| SharedString::from(format!("{}-{suffix}", subscription.as_str()));
        let sign_in = |label: &'static str, cx: &mut Context<Self>| {
            let enabled = self.sign_in.is_none();
            action(
                theme,
                id("sign-in"),
                widgets::ActionTone::Filled,
                label,
                enabled,
            )
            .when(enabled, |el| {
                el.on_click(
                    cx.listener(move |this, _, _, cx| this.connect_subscription(subscription, cx)),
                )
            })
            .into_any_element()
        };
        let account_text = |label: &str, account: &Option<String>| {
            account
                .as_deref()
                .map_or(label.to_string(), |a| format!("{label} · {a}"))
        };
        let mut meta: Vec<AnyElement> = Vec::new();
        let mut actions: Vec<AnyElement> = Vec::new();
        if let Some(flow) = self.sign_in.as_ref().filter(|f| f.provider == subscription) {
            let key = if subscription == VaultProvider::Claude {
                "claude-signing-in"
            } else {
                "codex-signing-in"
            };
            match &flow.step {
                SignInStep::Starting => meta.push(busy_meta(
                    theme,
                    key,
                    format!("Opening the {brand} sign-in…"),
                    cx,
                )),
                SignInStep::Browser { message } => meta.push(busy_meta(
                    theme,
                    key,
                    message.clone().unwrap_or_else(|| {
                        format!("Finish signing in with {brand} in your browser…").into()
                    }),
                    cx,
                )),
                SignInStep::PasteCode { .. } => meta.push(text(
                    theme.text_muted,
                    format!("Paste the code {brand} shows you to finish"),
                )),
            }
            if let Some(url) = flow.url.clone() {
                actions.push(
                    action(
                        theme,
                        id("reopen"),
                        widgets::ActionTone::Quiet,
                        "Reopen page",
                        true,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        crate::settings::accounts::open_login_url(&url, cx)
                    }))
                    .into_any_element(),
                );
            }
            actions.push(
                action(
                    theme,
                    id("cancel"),
                    widgets::ActionTone::Quiet,
                    "Cancel",
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)))
                .into_any_element(),
            );
        } else {
            match (&link, &key_link) {
                (ProviderLink::Connected { account }, _) => {
                    meta.push(text(theme.success_muted, "Connected"));
                    meta.push(text(theme.text_muted, account_text(brand, account)));
                    actions.push(self.disconnect_action(
                        theme,
                        subscription,
                        "Disconnect",
                        idle,
                        row_busy(subscription),
                        cx,
                    ));
                }
                (ProviderLink::NeedsReconnect { account }, _) => {
                    meta.push(text(theme.warning_muted.opacity(0.9), "Needs reconnect"));
                    if let Some(account) = account {
                        meta.push(text(theme.text_muted, account.clone()));
                    }
                    actions.push(self.disconnect_action(
                        theme,
                        subscription,
                        "Disconnect",
                        idle,
                        row_busy(subscription),
                        cx,
                    ));
                    actions.push(sign_in("Sign in again", cx));
                }
                (_, ProviderLink::Connected { account }) => {
                    meta.push(text(theme.success_muted, "Connected"));
                    meta.push(text(theme.text_muted, account_text(key_label, account)));
                    actions.push(self.disconnect_action(
                        theme,
                        key,
                        "Remove key",
                        idle,
                        row_busy(key),
                        cx,
                    ));
                }
                (ProviderLink::NotAuthorized { account }, _) => {
                    meta.push(text(
                        theme.text_muted,
                        account
                            .as_deref()
                            .map_or(format!("Signed in with {brand} for other devices"), |a| {
                                format!("Signed in with {brand} as {a} for other devices")
                            }),
                    ));
                    actions.push(self.authorize_action(theme, subscription, idle, cx));
                }
                _ => {
                    meta.push(text(theme.text_muted, "Not connected"));
                    actions.push(
                        action(
                            theme,
                            id("api-key"),
                            widgets::ActionTone::Quiet,
                            "Use an API key",
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.open_api_key(key, cx)))
                        .into_any_element(),
                    );
                    actions.push(sign_in(
                        if subscription == VaultProvider::Claude {
                            "Sign in with Claude"
                        } else {
                            "Sign in with ChatGPT"
                        },
                        cx,
                    ));
                }
            }
        }
        let note = (!link.is_stored() && !key_link.is_stored() && !self.signing_in(subscription))
            .then(|| {
                SharedString::from(format!(
                    "Signs in on this computer and keeps the sign-in for every Cloud session. \
                     Your own {title} login here is untouched."
                ))
            });
        let (icon, tint) = crate::pickers::harness_brand_icon(harness);
        RowParts {
            leading: Some(tile(theme, icon, tint)),
            title,
            meta,
            note,
            actions,
        }
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

    fn authorize_action(
        &self,
        theme: &Theme,
        provider: VaultProvider,
        idle: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        action(
            theme,
            SharedString::from(format!("authorize-{}", provider.as_str())),
            widgets::ActionTone::Filled,
            "Use on Cloud",
            idle,
        )
        .when(idle, |el| {
            el.on_click(cx.listener(move |this, _, _, cx| this.authorize(provider, cx)))
        })
        .into_any_element()
    }

    fn render_github(
        &mut self,
        cloud_id: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let vault = self.vault.ready().filter(|v| v.available)?.clone();
        let link = cloud::provider_link(&vault, VaultProvider::Github, cloud_id);
        let idle = self.vault_busy.is_none();
        let busy = self.vault_busy.as_deref() == Some(VaultProvider::Github.as_str());
        let mut meta: Vec<AnyElement> = Vec::new();
        let mut actions: Vec<AnyElement> = Vec::new();
        let mut note: Option<SharedString> = None;
        let connect = |label: &'static str, cx: &mut Context<Self>| {
            action(
                theme,
                "github-connect",
                widgets::ActionTone::Filled,
                label,
                true,
            )
            .on_click(cx.listener(|this, _, _, cx| this.start_github(cx)))
            .into_any_element()
        };
        match &self.github {
            GithubFlow::Starting => {
                meta.push(busy_meta(
                    theme,
                    "github-starting",
                    "Contacting GitHub…",
                    cx,
                ));
            }
            GithubFlow::Waiting { .. } => {
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
            GithubFlow::Failed(message) => {
                meta.push(text(theme.danger_muted.opacity(0.9), message.clone()));
                actions.push(connect("Try again", cx));
            }
            GithubFlow::Idle => match &link {
                ProviderLink::Connected { account } => {
                    meta.push(text(
                        theme.success_muted,
                        account.as_deref().map_or("Connected".to_string(), |login| {
                            format!("Connected as {login}")
                        }),
                    ));
                    if let Some(url) = vault.github_install_url.clone() {
                        note = Some(
                            "Sessions reach the repositories the Zeron GitHub App is installed on, \
                             and push and open pull requests as the App."
                                .into(),
                        );
                        actions.push(
                            action(
                                theme,
                                "github-install",
                                widgets::ActionTone::Quiet,
                                "Choose repositories",
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
                ProviderLink::NeedsReconnect { account } => {
                    meta.push(text(theme.warning_muted.opacity(0.9), "Needs reconnect"));
                    if let Some(account) = account {
                        meta.push(text(theme.text_muted, account.clone()));
                    }
                    actions.push(connect("Reconnect", cx));
                }
                ProviderLink::NotAuthorized { account } => {
                    meta.push(text(
                        theme.text_muted,
                        account
                            .as_deref()
                            .map_or("Connected for other devices".to_string(), |login| {
                                format!("Connected as {login} for other devices")
                            }),
                    ));
                    actions.push(self.authorize_action(theme, VaultProvider::Github, idle, cx));
                }
                ProviderLink::NotConnected => {
                    meta.push(text(theme.text_muted, "Not connected"));
                    note = Some(
                        "Connect, then install the Zeron GitHub App on the repositories Cloud \
                         sessions should clone, push to and open pull requests on."
                            .into(),
                    );
                    actions.push(connect("Connect GitHub", cx));
                }
            },
        }
        let code = match &self.github {
            GithubFlow::Waiting { flow, copied } => {
                Some(self.render_github_code(flow, *copied, theme, cx))
            }
            _ => None,
        };
        let block = widgets::section_card(theme)
            .mt(px(0.0))
            .child(row(
                theme,
                true,
                Some(tile(theme, crate::icons::GIT_BRANCH, None)),
                "GitHub",
                meta,
                note,
                actions,
            ))
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

    fn render_vault_devices(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let vault = self.vault.ready().filter(|v| v.available)?;
        let groups = cloud::group_vault_devices(&vault.devices);
        let cloud_id = self.cloud_id();
        if groups.computers.is_empty() && groups.cloud_machines == 0 {
            return None;
        }
        let mut computers = groups.computers;
        // Active first, newest enrollment first.
        computers.sort_by(|a, b| {
            a.revoked_at
                .is_some()
                .cmp(&b.revoked_at.is_some())
                .then_with(|| b.enrolled_at.cmp(&a.enrolled_at))
        });
        let names: Vec<(String, String)> = {
            let state = self.state.read(cx);
            state
                .devices
                .iter()
                .map(|d| (d.id.clone(), d.name.clone()))
                .collect()
        };
        let now = Utc::now();
        let idle = self.vault_busy.is_none();
        // (title, meta, the device to revoke when active)
        let mut rows: Vec<(String, Vec<AnyElement>, Option<String>)> = Vec::new();
        if let Some(cloud_id) = cloud_id.filter(|_| groups.cloud_machines > 0) {
            let machines = groups.cloud_machines;
            rows.push((
                cloud::CLOUD_LABEL.to_string(),
                vec![text(
                    theme.text_muted,
                    format!(
                        "{machines} session machine{}",
                        if machines == 1 { "" } else { "s" }
                    ),
                )],
                Some(cloud_id),
            ));
        }
        for device in computers {
            let name = names
                .iter()
                .find(|(id, _)| *id == device.device_id)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| crate::settings::devices::short_id(&device.device_id));
            let mut meta = vec![text(
                theme.text_muted,
                format!("Added {}", cloud::ago(device.enrolled_at, now)),
            )];
            let revoked = device.revoked_at.is_some();
            if revoked {
                meta.push(text(theme.warning_muted.opacity(0.9), "Access revoked"));
            }
            rows.push((name, meta, (!revoked).then_some(device.device_id)));
        }
        let mut block = widgets::section_card(theme).mt(px(0.0));
        for (ix, (name, meta, revoke)) in rows.into_iter().enumerate() {
            let actions = match revoke {
                None => Vec::new(),
                Some(device_id) => {
                    let busy = self.vault_busy.as_deref() == Some(device_id.as_str());
                    vec![
                        action(
                            theme,
                            ("vault-revoke", ix),
                            widgets::ActionTone::Quiet,
                            if busy { "Revoking…" } else { "Revoke" },
                            idle,
                        )
                        .aria_label(format!("Revoke access for {name}"))
                        .when(idle, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.revoke_device(device_id.clone(), cx)
                            }))
                        })
                        .into_any_element(),
                    ]
                }
            };
            // Compact: no tile, a shorter row.
            block = block.child(
                widgets::card_row(theme, ix == 0)
                    .min_h(px(48.0))
                    .py(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(theme, name))
                            .child(widgets::meta_line(theme, meta)),
                    )
                    .child(div().flex_none().flex().items_center().children(actions)),
            );
        }
        Some(widgets::section(theme, "Devices with access", block).into_any_element())
    }

    fn render_usage(&self, theme: &Theme) -> Option<AnyElement> {
        let usage = self.usage.as_ref()?;
        let (summary, cost) = cloud::usage_summary(usage);
        let mut meta = Vec::new();
        if let Some(cost) = cost {
            meta.push(text(theme.text_muted, cost));
        }
        let block = widgets::section_card(theme).mt(px(0.0)).child(
            widgets::card_row(theme, true)
                .min_h(px(48.0))
                .py(px(10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(theme, summary))
                        .when(!meta.is_empty(), |el| {
                            el.child(widgets::meta_line(theme, meta))
                        }),
                ),
        );
        Some(widgets::section(theme, "Usage", block).into_any_element())
    }

    fn render_danger_zone(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let idle = self.busy.is_none();
        let deleting = self.busy == Some(CloudAction::Delete);
        let danger = theme.danger_muted;
        let block = widgets::section_card(theme).mt(px(0.0)).child(row(
            theme,
            true,
            None,
            "Turn off Cloud",
            vec![text(
                theme.text_muted,
                "Permanently deletes every session's machine and every file on them, including \
                 work that hasn't been pushed. Transcripts, projects and connections stay.",
            )],
            None,
            vec![
                action(
                    theme,
                    "cloud-delete",
                    widgets::ActionTone::Quiet,
                    if deleting {
                        "Turning off…"
                    } else {
                        "Turn off…"
                    },
                    idle,
                )
                .text_color(danger)
                .when(idle, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_delete = true;
                        cx.notify();
                    }))
                })
                .into_any_element(),
            ],
        ));
        widgets::section(theme, "Danger zone", block).into_any_element()
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
        let usage = self.render_usage(&theme);
        let cloud_id = self
            .cloud_status()
            .filter(|s| s.available)
            .and_then(|_| self.cloud_id());
        let (providers, github, devices, danger) = match cloud_id.as_deref() {
            Some(id) => (
                Some(self.render_providers(id, &theme, window, cx)),
                self.render_github(id, &theme, cx),
                self.render_vault_devices(&theme, cx),
                Some(self.render_danger_zone(&theme, cx)),
            ),
            None => (None, None, None, None),
        };
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
                                    "Your own device in the cloud, with its own projects and providers. Every session runs on its own machine.",
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
                                .children(providers)
                                .children(github)
                                .children(usage)
                                .children(devices)
                                .children(danger),
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

    #[test]
    fn subscription_sign_ins_map_to_their_vault_calls() {
        assert_eq!(
            subscription_sign_in(VaultProvider::Codex),
            Some((methods::VAULT_CONNECT_CODEX, "ChatGPT"))
        );
        assert_eq!(
            subscription_sign_in(VaultProvider::Claude),
            Some((methods::VAULT_CONNECT_CLAUDE, "Claude"))
        );
        assert_eq!(subscription_sign_in(VaultProvider::Github), None);
    }

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
        }
    }

    /// Every lifecycle state and provider standing paints without a panic,
    /// with the inline key field, the GitHub code and the delete dialog open.
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
                    });
                    page.vault = Loadable::Ready(vault);
                    page.usage = Some(CloudUsage {
                        month: "2026-10".into(),
                        seconds: 44_640,
                        dollars: 1.25,
                        available: true,
                        ..Default::default()
                    });
                    page.sign_in = (ix % 3 == 0).then(|| SignInFlow {
                        provider: if ix % 2 == 0 {
                            VaultProvider::Codex
                        } else {
                            VaultProvider::Claude
                        },
                        attempt: 0,
                        login_id: Some("login".into()),
                        url: Some("https://example.com/sign-in".into()),
                        step: SignInStep::Browser { message: None },
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
                    if ix % 2 == 1 {
                        page.open_api_key(VaultProvider::AnthropicKey, cx);
                    } else {
                        page.api_key = None;
                    }
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
        // Claude's paste-code fallback, then the poll outcomes.
        page.update(cx, |page, cx| {
            page.vault = Loadable::Ready(VaultStatus {
                available: true,
                ..Default::default()
            });
            page.api_key = None;
            page.sign_in = Some(SignInFlow {
                provider: VaultProvider::Claude,
                attempt: 7,
                login_id: None,
                url: None,
                step: SignInStep::Starting,
            });
            page.apply_sign_in_start(
                7,
                Ok(AgentLoginStart {
                    login_id: "login-7".into(),
                    url: String::new(),
                    mode: AgentLoginMode::PasteCode,
                    callback_port: None,
                }),
                cx,
            );
            assert!(matches!(
                page.sign_in.as_ref().map(|f| &f.step),
                Some(SignInStep::PasteCode { .. })
            ));
            // An empty code is refused locally.
            page.submit_sign_in_code(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear());
        page.update(cx, |page, cx| {
            // A stale attempt's poll is ignored and ends its loop.
            assert!(page.apply_sign_in_poll(
                6,
                Ok(AgentLoginPoll {
                    status: AgentLoginStatus::Done,
                    message: None,
                    url: None,
                    callback_port: None,
                }),
                cx,
            ));
            assert!(page.sign_in.is_some());
            if let Some(flow) = page.sign_in.as_mut() {
                flow.step = SignInStep::Browser { message: None };
            }
            assert!(!page.apply_sign_in_poll(
                7,
                Ok(AgentLoginPoll {
                    status: AgentLoginStatus::Pending,
                    message: Some("Waiting for the browser…".into()),
                    url: None,
                    callback_port: None,
                }),
                cx,
            ));
            assert!(page.apply_sign_in_poll(
                7,
                Ok(AgentLoginPoll {
                    status: AgentLoginStatus::Error,
                    message: Some("Denied".into()),
                    url: None,
                    callback_port: None,
                }),
                cx,
            ));
            assert!(page.sign_in.is_none());
            assert_eq!(page.error.as_deref(), Some("Denied"));
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
