//! Settings → Agents: enable/disable harnesses (the t3code models-page
//! arrangement — one card row per agent with a trailing toggle).
//!
//! The state is PER-DEVICE and lives on the engine (`harness-prefs.json` in
//! its data dir): CLI installs are per-device, so enablement is too. The
//! page-header device switcher (the Accounts pattern) retargets both the
//! `ListHarnesses` probe and the `SetHarnessEnabled` writes at any registered
//! device over the relay-forwarded RPCs.
//!
//! Enablement follows DETECTION: every harness whose CLI probe passes is on
//! unless the user switched it off, so installing an agent is all it takes
//! for it to appear here and in the composer. A harness whose CLI is missing
//! on the target device renders dimmed with an install hint and is never
//! enabled (enabling an agent that can't run would only manufacture
//! NotInstalled errors at send time); an ENABLED agent can always be turned
//! OFF except the last one standing — the composer needs something to run.
//! Catalogs from engines predating the detection model can still stamp
//! enabled-but-uninstalled rows; the hint covers that too. The engine
//! enforces the same gates where the state lives, so a raced or stale toggle
//! self-corrects from the RPC reply.

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Task, Window, div, prelude::*,
    px,
};

use std::time::Duration;
use zeron_engine::registry::TitleSettings;
use zeron_engine::registry::{HarnessDescriptor, descriptor_enabled};

use zeron_proto::Model;
use zeron_proto::{AgentLoginPoll, AgentLoginStart, AgentLoginStatus, HarnessId};
use zeron_rpc::methods;

use crate::pickers::visible_harnesses;
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// One-line blurb per agent (the t3code models page pairs every toggle row
/// with a description; the catalog descriptor doesn't carry one).
pub fn blurb(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Anthropic's coding agent, driven through the Claude Code CLI.",
        HarnessId::Codex => "OpenAI's coding agent, driven through the Codex CLI.",
        HarnessId::Cursor => "Cursor's coding agent, driven through the cursor-agent CLI.",
        HarnessId::Devin => "Cognition's Devin agent (devin CLI).",
        HarnessId::Grok => "xAI's Grok Build agent (grok CLI).",
        HarnessId::Hermes => "Nous Research's Hermes Agent (hermes CLI).",
        HarnessId::Pi => "The pi coding agent (pi CLI).",
        HarnessId::Omp => "Oh My Pi's coding agent, driven over ACP (omp CLI).",
        HarnessId::Opencode => "SST's opencode agent (opencode CLI).",
        HarnessId::Antigravity => "Google's Antigravity agent (Antigravity ACP server).",
        HarnessId::Mock => "Scripted test harness.",
    }
}

/// The CLI named in the not-installed hint.
pub fn cli_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "claude",
        HarnessId::Codex => "codex",
        HarnessId::Cursor => "cursor-agent",
        HarnessId::Devin => "devin",
        HarnessId::Grok => "grok",
        HarnessId::Hermes => "hermes",
        HarnessId::Pi => "pi",
        HarnessId::Omp => "omp",
        HarnessId::Opencode => "opencode",
        HarnessId::Antigravity => "agy",
        HarnessId::Mock => "mock",
    }
}

pub struct HarnessesPage {
    title_settings: Loadable<TitleSettings>,
    title_models: Loadable<Vec<Model>>,
    title_menu: Option<bool>, // false = harness, true = model
    title_task: Option<Task<()>>,
    title_saving: bool,
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    /// Which device's harnesses are shown/edited; `None` = this device (no
    /// passthrough). Retargeted by the page-header device switcher.
    target_device: Option<String>,
    device_menu_open: bool,
    /// Whether the menu was open when the trigger press began — the menu's
    /// `on_mouse_down_out` closes it on that same press, so by click time a
    /// plain toggle would reopen (the [`popover::Popup`] press note, for
    /// this page's bool-state menu).
    device_menu_pressed_open: bool,
    /// Last refused/failed toggle (engine guards), shown in the error strip.
    error: Option<String>,
    load_task: Option<Task<()>>,
    toggle_task: Option<Task<()>>,
    /// a sign-in that switches its harness on once it succeeds.
    sign_in: Option<SignIn>,
    sign_in_failure: Option<SignInFailure>,
    sign_in_task: Option<Task<()>>,
}

struct SignIn {
    harness: HarnessId,
    /// known once the engine accepted the start.
    login_id: Option<String>,
    message: Option<String>,
    phase: SignInPhase,
}

#[derive(Clone, Copy)]
enum SignInPhase {
    Starting,
    Installing,
    Authenticating,
    Enabling,
}

struct SignInFailure {
    harness: HarnessId,
    message: String,
    phase: SignInPhase,
}

impl SignInPhase {
    fn pending_label(self) -> &'static str {
        match self {
            Self::Starting => "Preparing Antigravity…",
            Self::Installing => "Installing Antigravity…",
            Self::Authenticating => "Finish signing in in your browser.",
            Self::Enabling => "Enabling Antigravity…",
        }
    }

    fn failure_label(self) -> &'static str {
        match self {
            Self::Starting => "Setup failed",
            Self::Installing => "Installation failed",
            Self::Authenticating => "Sign-in failed",
            Self::Enabling => "Enable failed",
        }
    }
}

/// harnesses whose toggle runs the agent's own sign-in before switching on.
fn signs_in_on_enable(harness: HarnessId) -> bool {
    harness == HarnessId::Antigravity
}

impl HarnessesPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            title_settings: Loadable::Idle,
            title_models: Loadable::Idle,
            title_menu: None,
            title_task: None,
            title_saving: false,
            state,
            scroll: widgets::PageScroll::default(),
            harnesses: Loadable::Idle,
            target_device: None,
            device_menu_open: false,
            device_menu_pressed_open: false,
            error: None,
            load_task: None,
            toggle_task: None,
            sign_in: None,
            sign_in_failure: None,
            sign_in_task: None,
        };
        page.load(cx);
        page
    }

    /// Params with the `targetDeviceId` passthrough merged in.
    fn with_target(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let (Some(target), Some(object)) = (&self.target_device, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    /// Retarget the page at another device: a different device is a different
    /// install/enablement world, so drop the rows and reload through it.
    fn set_target_device(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.device_menu_open = false;
        if self.target_device == target {
            cx.notify();
            return;
        }
        self.cancel_sign_in(cx);
        self.title_task = None;
        self.title_settings = Loadable::Idle;
        self.title_models = Loadable::Idle;
        self.title_menu = None;
        self.title_saving = false;
        self.target_device = target;
        self.error = None;
        self.sign_in_failure = None;
        self.harnesses = Loadable::Idle;
        self.load(cx);
        cx.notify();
    }

    /// `ListHarnesses` against the target device (installed probe + enabled
    /// set both come from where the CLIs actually live).
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({}));
        self.load_titles(None, cx);
        self.harnesses = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::LIST_HARNESSES, params).await;
            this.update(cx, |page, cx| {
                page.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn load_titles(&mut self, save: Option<TitleSettings>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let saving = save.is_some();
        let method = if saving {
            methods::SET_TITLE_SETTINGS
        } else {
            methods::GET_TITLE_SETTINGS
        };
        let params = self.with_target(
            save.map(|s| serde_json::to_value(s).unwrap())
                .unwrap_or_else(|| serde_json::json!({})),
        );
        let target = self.target_device.clone();
        self.title_menu = None;
        self.title_saving = saving;
        self.title_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<TitleSettings>(v).map_err(|e| e.to_string())
                });
            let settings = match result {
                Ok(settings) => settings,
                Err(error) => {
                    this.update(cx, |page, cx| {
                        if saving {
                            page.error = Some(error);
                        } else {
                            page.title_settings = Loadable::Error(error);
                        }
                        page.title_saving = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let harness = settings.harness;
            this.update(cx, |page, cx| {
                page.title_settings = Loadable::Ready(settings);
                page.title_models = if harness.is_some() {
                    Loadable::Loading
                } else {
                    Loadable::Idle
                };
                page.title_saving = false;
                page.error = None;
                cx.notify();
            })
            .ok();
            if let Some(harness) = harness {
                let result = engine
                    .client()
                    .call(
                        methods::LIST_MODELS,
                        serde_json::json!({"harness": harness, "targetDeviceId": target}),
                    )
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| {
                        serde_json::from_value::<Vec<Model>>(v).map_err(|e| e.to_string())
                    });
                this.update(cx, |page, cx| {
                    page.title_models = match result {
                        Ok(models) => Loadable::Ready(models),
                        Err(error) => Loadable::Error(error),
                    };
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }

    fn render_titles(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut card = widgets::section_card(theme).mt(px(20.0)).p(px(16.0))
            .child(widgets::row_title(theme, "Session titles"))
            .child(widgets::page_subtitle(theme, "Choose the agent and model for automatic titles on this device. Claude Code and Codex support restricted title generation."));
        let Loadable::Ready(settings) = &self.title_settings else {
            let message = match &self.title_settings {
                Loadable::Error(error) => error.clone(),
                _ => "Loading title settings…".into(),
            };
            return card
                .child(div().mt(px(8.0)).child(message))
                .into_any_element();
        };
        for is_model in [false, true] {
            let label = if is_model {
                settings
                    .model
                    .as_ref()
                    .map(|id| {
                        if let Loadable::Ready(models) = &self.title_models {
                            models
                                .iter()
                                .find(|m| &m.id == id)
                                .map(|m| m.label.clone())
                                .unwrap_or_else(|| id.clone())
                        } else {
                            id.clone()
                        }
                    })
                    .unwrap_or_else(|| "Automatic (cheapest model)".into())
            } else {
                settings
                    .harness
                    .map(|id| match id {
                        HarnessId::ClaudeCode => "Claude Code".to_string(),
                        HarnessId::Codex => "Codex".to_string(),
                        _ => format!("{id:?}"),
                    })
                    .unwrap_or_else(|| "Automatic (session agent when supported)".into())
            };
            let interactive = !self.title_saving && (!is_model || settings.harness.is_some());
            let mut row = div()
                .mt(px(12.0))
                .child(widgets::row_title(
                    theme,
                    if is_model {
                        "Title model"
                    } else {
                        "Title harness"
                    },
                ))
                .child(
                    widgets::ghost_action(theme)
                        .id(if is_model {
                            "title-model"
                        } else {
                            "title-harness"
                        })
                        .when(interactive, |el| {
                            el.cursor_pointer()
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    page.title_menu = if page.title_menu == Some(is_model) {
                                        None
                                    } else {
                                        Some(is_model)
                                    };
                                    cx.notify();
                                }))
                        })
                        .when(!interactive, |el| el.opacity(0.5))
                        .child(label),
                );
            if self.title_menu == Some(is_model) {
                let mut choices = vec![(
                    "Automatic".to_string(),
                    TitleSettings {
                        harness: if is_model { settings.harness } else { None },
                        model: None,
                    },
                )];
                if is_model {
                    if let Loadable::Ready(models) = &self.title_models {
                        choices.extend(models.iter().map(|m| {
                            (
                                m.label.clone(),
                                TitleSettings {
                                    harness: settings.harness,
                                    model: Some(m.id.clone()),
                                },
                            )
                        }));
                    }
                } else if let Loadable::Ready(harnesses) = &self.harnesses {
                    choices.extend(
                        harnesses
                            .iter()
                            .filter(|h| {
                                descriptor_enabled(h)
                                    && h.installed
                                    && zeron_harness::supports_titles(h.id)
                                    && h.id != HarnessId::Mock
                            })
                            .map(|h| {
                                (
                                    h.name.clone(),
                                    TitleSettings {
                                        harness: Some(h.id),
                                        model: None,
                                    },
                                )
                            }),
                    );
                }
                row =
                    row.child(
                        div()
                            .id(if is_model {
                                "title-model-options"
                            } else {
                                "title-harness-options"
                            })
                            .max_h(px(240.0))
                            .overflow_y_scroll()
                            .children(choices.into_iter().enumerate().map(
                                |(ix, (label, choice))| {
                                    popover::menu_row(
                                        theme,
                                        &choice == settings,
                                        format!("title-choice-{is_model}-{ix}"),
                                    )
                                    .id(("title-choice", ix))
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.load_titles(Some(choice.clone()), cx)
                                    }))
                                    .child(label)
                                },
                            )),
                    );
            }
            card = card.child(row);
        }
        if let Loadable::Error(error) = &self.title_models {
            card = card.child(widgets::error_strip(theme, error.clone()));
        }
        card.into_any_element()
    }

    /// Flip one harness on the target device. The reply carries the device's
    /// fresh catalog, so the rows repaint from the authoritative state in one
    /// round trip; refusals (engine guards) land in the error strip.
    fn toggle(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        if enabled && signs_in_on_enable(harness) {
            self.start_sign_in(harness, cx);
        } else {
            self.set_enabled(harness, enabled, cx);
        }
    }

    /// sign in first, then switch on: StartAgentLogin, then PollAgentLogin
    /// until the engine reports the outcome, opening the sign-in page the
    /// first time a poll names it.
    fn start_sign_in(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if self.target_device.is_some() {
            // the sign-in redirect lands on a loopback port of the device
            // running the agent, which a browser here can't reach
            self.error = Some("Turn this agent on from its own device to sign in.".into());
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.error = None;
        self.sign_in_failure = None;
        self.sign_in = Some(SignIn {
            harness,
            login_id: None,
            message: None,
            phase: SignInPhase::Starting,
        });
        let start_params = serde_json::json!({ "harness": harness });
        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let started = engine
                .client()
                .call(methods::START_AGENT_LOGIN, start_params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value::<AgentLoginStart>(value).map_err(|e| e.to_string())
                });
            let login_id = match started {
                Ok(start) => start.login_id,
                Err(error) => {
                    this.update(cx, |page, cx| {
                        page.fail_sign_in(harness, format!("Sign-in failed to start: {error}"));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |page, _| {
                if let Some(sign_in) = &mut page.sign_in {
                    sign_in.login_id = Some(login_id.clone());
                    sign_in.phase = SignInPhase::Installing;
                }
            })
            .ok();
            let poll_params = serde_json::json!({ "loginId": login_id });
            let mut opened = false;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
                let poll = engine
                    .client()
                    .call(methods::POLL_AGENT_LOGIN, poll_params.clone())
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|value| {
                        serde_json::from_value::<AgentLoginPoll>(value).map_err(|e| e.to_string())
                    });
                let finished = this.update(cx, |page, cx| {
                    let finished = match poll {
                        Ok(poll) => match poll.status {
                            AgentLoginStatus::Pending => {
                                if !opened && let Some(url) = &poll.url {
                                    opened = true;
                                    cx.open_url(url);
                                }
                                if let Some(sign_in) = &mut page.sign_in {
                                    if poll.url.is_some() {
                                        sign_in.phase = SignInPhase::Authenticating;
                                    }
                                    sign_in.message = poll.message;
                                }
                                false
                            }
                            AgentLoginStatus::Done => {
                                page.sign_in_failure = None;
                                if let Some(sign_in) = &mut page.sign_in {
                                    sign_in.phase = SignInPhase::Enabling;
                                    sign_in.message = None;
                                }
                                page.finish_sign_in(harness, cx);
                                true
                            }
                            AgentLoginStatus::Error => {
                                page.fail_sign_in(
                                    harness,
                                    poll.message.unwrap_or_else(|| "Unknown error".into()),
                                );
                                true
                            }
                        },
                        Err(error) => {
                            page.fail_sign_in(harness, error);
                            true
                        }
                    };
                    cx.notify();
                    finished
                });
                if finished.unwrap_or(true) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn fail_sign_in(&mut self, harness: HarnessId, message: String) {
        let phase = self
            .sign_in
            .take()
            .filter(|sign_in| sign_in.harness == harness)
            .map(|sign_in| sign_in.phase)
            .unwrap_or(SignInPhase::Starting);
        self.sign_in_failure = Some(SignInFailure {
            harness,
            message,
            phase,
        });
    }

    fn finish_sign_in(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.fail_sign_in(harness, "Engine unavailable".into());
            return;
        };
        let params = self.with_target(serde_json::json!({
            "harness": harness,
            "enabled": true,
        }));
        self.toggle_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_HARNESS_ENABLED, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                match result {
                    Ok(list) => {
                        page.harnesses = Loadable::Ready(list);
                        page.sign_in = None;
                        page.sign_in_failure = None;
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(error) => page.fail_sign_in(harness, error),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.sign_in.take() else {
            return;
        };
        self.sign_in_task = None;
        if let (Some(login_id), Some(engine)) =
            (sign_in.login_id, self.state.read(cx).engine().cloned())
        {
            cx.spawn(async move |_, _| {
                if let Err(err) = engine
                    .client()
                    .call(
                        methods::CANCEL_AGENT_LOGIN,
                        serde_json::json!({ "loginId": login_id }),
                    )
                    .await
                {
                    tracing::debug!(error = %err, "CancelAgentLogin failed (best-effort)");
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn set_enabled(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({
            "harness": harness,
            "enabled": enabled,
        }));
        self.error = None;
        self.toggle_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_HARNESS_ENABLED, params)
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(value) => {
                        if let Ok(list) = serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                            page.harnesses = Loadable::Ready(list);
                        }
                        // The composer caches its catalog per space — poke
                        // every Pickers to re-fetch, or the rail keeps the
                        // old set until restart.
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The page-header device switcher (the Accounts pattern): platform glyph
    /// · name · presence dot · sort glyph, opening a dropdown of every
    /// registered device.
    fn render_device_switcher(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use crate::icons::{self, icon};
        let (mut devices, local_id) = {
            let s = self.state.read(cx);
            (s.devices.clone(), s.local_device_id.clone())
        };
        devices.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let effective = self.target_device.clone().or_else(|| local_id.clone());
        let selected = devices
            .iter()
            .find(|d| Some(d.id.as_str()) == effective.as_deref())
            .cloned();
        let platform_glyph = |platform: &str| match platform {
            "macos" | "darwin" => icons::LAPTOP,
            "ios" | "android" => icons::SMARTPHONE,
            _ => icons::MONITOR,
        };
        let trigger_glyph = platform_glyph(
            selected
                .as_ref()
                .map(|d| d.platform.as_str())
                .unwrap_or("macos"),
        );
        let trigger_label: SharedString = selected
            .as_ref()
            .map(|d| d.name.clone().into())
            .unwrap_or_else(|| SharedString::from("This device"));
        let emerald = theme.success;
        let open = self.device_menu_open;

        let mut trigger =
            div()
                .id("harnesses-device-switcher")
                .flex_none()
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .bg(if open {
                    crate::theme::ink(0.06)
                } else {
                    gpui::transparent_black()
                })
                .when(!open, |el| el.hover(|s| s.bg(crate::theme::ink(0.04))))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.device_menu_pressed_open = this.device_menu_open;
                    }),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    // A press that found the menu open closes it — never
                    // reopen on the same gesture.
                    let pressed_open = std::mem::take(&mut this.device_menu_pressed_open);
                    this.device_menu_open = !pressed_open && !this.device_menu_open;
                    cx.notify();
                }))
                .child(
                    icon(trigger_glyph)
                        .size(px(16.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(trigger_label),
                )
                .child(div().size(px(6.0)).rounded_full().flex_none().bg(
                    if effective == local_id {
                        emerald
                    } else {
                        crate::theme::ink(0.2)
                    },
                ))
                .child(
                    icon(icons::SORT_VERTICAL)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(theme.text_muted.opacity(if open { 0.9 } else { 0.4 })),
                );

        if open {
            let theme = &theme.for_popup();
            let menu = popover::popover_card(theme)
                .w(px(220.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.device_menu_open = false;
                    cx.notify();
                }))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(popover::menu_heading(theme, "Devices"))
                .children(devices.into_iter().enumerate().map(|(ix, d)| {
                    let is_active = Some(d.id.as_str()) == effective.as_deref();
                    let is_local = local_id.as_deref() == Some(d.id.as_str());
                    let glyph = platform_glyph(&d.platform);
                    let name: SharedString = d.name.clone().into();
                    let pick_local = is_local;
                    let pick_id = d.id.clone();
                    popover::menu_row(theme, is_active, format!("harnesses-device-row-{ix}"))
                        .id(("harnesses-device-row", ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Local device = no passthrough (calls stay direct).
                            let target = (!pick_local).then(|| pick_id.clone());
                            this.set_target_device(target, cx);
                        }))
                        .child(
                            icon(glyph)
                                .size(px(16.0))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .when(is_local, |el| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from("You")),
                            )
                        })
                        .child(
                            div()
                                .size(px(6.0))
                                .rounded_full()
                                .flex_none()
                                .bg(if is_local {
                                    emerald
                                } else {
                                    crate::theme::ink(0.2)
                                }),
                        )
                }))
                .into_any_element();
            trigger = trigger.child(popover::anchored_menu("harnesses-device-menu", menu, None));
        }
        trigger.into_any_element()
    }

    fn rows(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let theme = Theme::of(cx).clone();
        let Loadable::Ready(list) = &self.harnesses else {
            return Vec::new();
        };
        let descriptors = visible_harnesses(list);
        let enabled_count = descriptors.iter().filter(|d| descriptor_enabled(d)).count();
        descriptors
            .into_iter()
            .enumerate()
            .map(|(ix, descriptor)| {
                let harness = descriptor.id;
                let installed = descriptor.installed;
                let enabled = descriptor_enabled(&descriptor);
                // The one enabled harness left can't be switched off — the
                // composer needs something to run — but only when it could
                // actually run: an uninstalled last harness stays togglable
                // (its hint says to turn it off) and the composer handles the
                // resulting empty set (mirrors the engine guard).
                let last_enabled = enabled && enabled_count == 1 && installed;
                let signing_in = self
                    .sign_in
                    .as_ref()
                    .filter(|sign_in| sign_in.harness == harness);
                let sign_in_failure = self
                    .sign_in_failure
                    .as_ref()
                    .filter(|failure| failure.harness == harness);
                let sign_in_cancellable = signing_in
                    .is_some_and(|sign_in| !matches!(sign_in.phase, SignInPhase::Enabling));
                // Turning OFF never needs the CLI (a default-on agent the
                // user doesn't want must not be stuck on because it isn't
                // installed); turning ON still does.
                let interactive = signing_in.is_none()
                    && sign_in_failure.is_none()
                    && !last_enabled
                    && (enabled || installed);
                let (icon_path, tint) = crate::pickers::harness_brand_icon(harness);
                let mut meta: Vec<gpui::AnyElement> = vec![
                    div()
                        .child(SharedString::from(blurb(harness)))
                        .into_any_element(),
                ];
                if let Some(sign_in) = signing_in {
                    meta.push(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(crate::loaders::mini_mono_spinner(
                                format!("harness-setup-spinner-{harness:?}"),
                                1.5,
                                theme.text_muted,
                                cx.entity_id(),
                                cx,
                            ))
                            .child(SharedString::from(
                                sign_in
                                    .message
                                    .clone()
                                    .unwrap_or_else(|| sign_in.phase.pending_label().into()),
                            ))
                            .into_any_element(),
                    );
                }
                if let Some(failure) = sign_in_failure {
                    meta.push(
                        div()
                            .text_color(theme.danger_muted.opacity(0.9))
                            .child(SharedString::from(format!(
                                "{} — {}",
                                failure.phase.failure_label(),
                                failure.message
                            )))
                            .into_any_element(),
                    );
                }
                if !installed {
                    meta.push(
                        div()
                            .text_color(theme.warning_muted.opacity(0.9))
                            .child(SharedString::from(if enabled {
                                format!(
                                    "{} CLI not installed — turn it off or install it",
                                    cli_name(harness)
                                )
                            } else {
                                format!("Install the {} CLI to enable", cli_name(harness))
                            }))
                            .into_any_element(),
                    );
                }
                // widgets::row_tile with the brand tint honored (the Claude
                // mark keeps its orange, like the picker rail).
                let tile = div()
                    .flex_none()
                    .size(px(36.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(crate::theme::ink(0.03))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::icons::icon(icon_path)
                            .size(px(16.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    );
                widgets::card_row(&theme, ix == 0)
                    .id(("harness-row", ix))
                    .when(!installed, |el| el.opacity(0.55))
                    .when(signing_in.is_some(), |el| el.opacity(0.65))
                    .child(tile)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, descriptor.name.clone()))
                            .child(widgets::meta_line(&theme, meta)),
                    )
                    .when(sign_in_cancellable, |el| {
                        el.child(
                            widgets::ghost_action(&theme)
                                .id(("harness-cancel-sign-in", ix))
                                .hover(|s| widgets::ghost_hover(&theme, s))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_sign_in(cx);
                                }))
                                .child(SharedString::from("Cancel")),
                        )
                    })
                    .when(sign_in_failure.is_some(), |el| {
                        el.child(
                            widgets::ghost_action(&theme)
                                .id(("harness-retry-sign-in", ix))
                                .hover(|s| widgets::ghost_hover(&theme, s))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_sign_in(harness, cx);
                                }))
                                .child(SharedString::from("Retry")),
                        )
                    })
                    .child(
                        widgets::toggle_switch(&theme, enabled)
                            .id(("harness-toggle", ix))
                            .when(!interactive, |el| el.opacity(0.35))
                            .when(interactive, |el| {
                                el.cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle(harness, !enabled, cx);
                                    }))
                            }),
                    )
                    .into_any_element()
            })
            .collect()
    }
    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for HarnessesPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for HarnessesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let body: gpui::AnyElement = match &self.harnesses {
            Loadable::Idle | Loadable::Loading => widgets::section_card(&theme)
                .p(px(16.0))
                .child(popover::skeleton_rows(
                    "harnesses-skeleton",
                    &theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Loadable::Error(message) => {
                let message = message.clone();
                div()
                    .child(widgets::error_strip(&theme, message))
                    .child(
                        widgets::ghost_action(&theme)
                            .id("harnesses-retry")
                            .mt(px(8.0))
                            .hover(|s| widgets::ghost_hover(&theme, s))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(cx);
                                cx.notify();
                            }))
                            .child(SharedString::from("Retry")),
                    )
                    .into_any_element()
            }
            Loadable::Ready(_) => {
                let rows = self.rows(cx);
                widgets::section_card(&theme)
                    .children(rows)
                    .into_any_element()
            }
        };
        let error = self
            .error
            .clone()
            .map(|message| widgets::error_strip(&theme, message).into_any_element());
        let switcher = self.render_device_switcher(&theme, cx);
        let titles = self.render_titles(&theme, cx);
        let scrollbar = popover::rail(self, "harnesses-page-scrollbar", &theme, cx);

        div()
            .id("harnesses-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("harnesses-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(widgets::page_header(&theme, "Agents", None))
                                    .child(switcher),
                            )
                            .child(
                                widgets::page_subtitle(
                                    &theme,
                                    "Choose which coding agents the composer offers. The setting is per \
                                     device — switch devices in the header. Agents whose CLI isn't \
                                     installed on a device can't be enabled there.",
                                )
                                .max_w(px(512.0))
                                .line_height(px(20.0)),
                            )
                            .children(error)
                            .child(body)
                            .child(titles),
                    ),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::SignInPhase;

    #[test]
    fn antigravity_setup_copy_matches_each_phase() {
        assert_eq!(
            SignInPhase::Starting.pending_label(),
            "Preparing Antigravity…"
        );
        assert_eq!(SignInPhase::Starting.failure_label(), "Setup failed");
        assert_eq!(
            SignInPhase::Installing.pending_label(),
            "Installing Antigravity…"
        );
        assert_eq!(
            SignInPhase::Installing.failure_label(),
            "Installation failed"
        );
        assert_eq!(
            SignInPhase::Authenticating.pending_label(),
            "Finish signing in in your browser."
        );
        assert_eq!(
            SignInPhase::Authenticating.failure_label(),
            "Sign-in failed"
        );
        assert_eq!(
            SignInPhase::Enabling.pending_label(),
            "Enabling Antigravity…"
        );
        assert_eq!(SignInPhase::Enabling.failure_label(), "Enable failed");
    }
}
