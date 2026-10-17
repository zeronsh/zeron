//! First-run setup: a welcome, then appearance, sync and the agents on this
//! device, ending on "You're all set" with the way to a first project.
//!
//! Shown over the Ready shell until it's finished or skipped
//! (`onboardingCompleted` in `ui-settings.json`); only a fresh install starts
//! it. The shell owns what the steps can't do from here — the sign-in flow
//! and the add-project dialog — and hears about them through
//! [`OnboardingEvent`]. While the sign-in wizard is up the shell hides this
//! card, and the signed-in edge moves it on to the agents step.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, IntoElement, ObjectFit, Render, SharedString,
    StyledImage as _, Subscription, Task, Window, div, img, prelude::*, px,
};

use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::{AgentAccountsSnapshot, HarnessId};
use zeron_rpc::methods;

use crate::appearance::{self, AppearanceMode};
use crate::icons::{self, icon};
use crate::motion;
use crate::popover::{self, Loadable};
use crate::settings::accounts::{self, AccountsPage, AccountsSnapshotCache};
use crate::settings::{NewThreadBackgroundEffect, widgets};
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Appearance,
    Sync,
    Providers,
    Done,
}

impl Step {
    /// `ZERON_ONBOARDING` values: a step name, or anything else to start at
    /// the beginning.
    pub fn parse(value: &str) -> Step {
        match value {
            "appearance" => Step::Appearance,
            "sync" => Step::Sync,
            "providers" | "agents" => Step::Providers,
            "done" => Step::Done,
            _ => Step::Welcome,
        }
    }

    fn next(self) -> Step {
        match self {
            Step::Welcome => Step::Appearance,
            Step::Appearance => Step::Sync,
            Step::Sync => Step::Providers,
            Step::Providers | Step::Done => Step::Done,
        }
    }

    fn previous(self) -> Step {
        match self {
            Step::Welcome | Step::Appearance => Step::Welcome,
            Step::Sync => Step::Appearance,
            Step::Providers => Step::Sync,
            Step::Done => Step::Providers,
        }
    }

    /// Place among the numbered steps; the welcome and the ending aren't
    /// counted.
    fn number(self) -> Option<usize> {
        match self {
            Step::Appearance => Some(1),
            Step::Sync => Some(2),
            Step::Providers => Some(3),
            Step::Welcome | Step::Done => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Step::Welcome => "welcome",
            Step::Appearance => "appearance",
            Step::Sync => "sync",
            Step::Providers => "providers",
            Step::Done => "done",
        }
    }
}

const NUMBERED_STEPS: usize = 3;
const CARD_WIDTH: f32 = 480.0;
const COMPACT_CARD_WIDTH: f32 = 400.0;
const CARD_MAX_HEIGHT: f32 = 660.0;
const CARD_PADDING: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingEvent {
    SignIn,
    Finish { add_project: bool },
}

/// What the agents step can say about this device's agents. Pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsSummary {
    Looking,
    Unavailable(String),
    NoneInstalled,
    Installed(Vec<String>),
}

pub fn agents_summary(harnesses: &Loadable<Vec<HarnessDescriptor>>) -> AgentsSummary {
    match harnesses {
        Loadable::Idle | Loadable::Loading => AgentsSummary::Looking,
        Loadable::Error(error) => AgentsSummary::Unavailable(error.clone()),
        Loadable::Ready(list) => {
            let names: Vec<String> = crate::pickers::visible_harnesses(list)
                .into_iter()
                .filter(|descriptor| descriptor.installed && descriptor.id != HarnessId::Mock)
                .map(|descriptor| descriptor.name)
                .collect();
            if names.is_empty() {
                AgentsSummary::NoneInstalled
            } else {
                AgentsSummary::Installed(names)
            }
        }
    }
}

/// "Claude Code", "Claude Code and Codex", "Claude Code, Codex and Cursor".
pub fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A list short enough for one line: past three, the first two and a count.
pub fn short_names(names: &[String]) -> String {
    match names {
        [first, second, rest @ ..] if rest.len() > 1 => {
            format!("{first}, {second} and {} more", rest.len())
        }
        _ => join_names(names),
    }
}

fn this_device() -> &'static str {
    if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    }
}

/// The signed-in login of `harness` on this device, by its friendliest name.
fn signed_in_as(snapshot: Option<&AgentAccountsSnapshot>, harness: HarnessId) -> Option<String> {
    let accounts = accounts::provider_accounts(snapshot?, harness);
    let account = accounts
        .iter()
        .find(|account| account.active)
        .or(accounts.first())?;
    Some(
        account
            .email
            .clone()
            .or_else(|| account.display_name.clone())
            .unwrap_or_else(|| "Signed in".into()),
    )
}

pub struct Onboarding {
    state: Entity<AppState>,
    step: Step,
    /// Log in was pressed: the signed-in edge moves on to the agents step.
    awaiting_sign_in: bool,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    installing: Option<HarnessId>,
    error: Option<SharedString>,
    expanded: Option<HarnessId>,
    accounts_page: Option<Entity<AccountsPage>>,
    load_task: Option<Task<()>>,
    install_task: Option<Task<()>>,
    _observe_state: Subscription,
    _observe_accounts: Option<Subscription>,
    /// The card as last laid out: the wallpaper clears around it the way the
    /// new-chat wallpaper clears around the composer.
    card_bounds: crate::new_thread_background_mask::SurfaceBounds,
    wallpaper: crate::new_thread_background_effects::Readiness,
    wallpaper_error: Option<SharedString>,
}

impl EventEmitter<OnboardingEvent> for Onboarding {}

impl Onboarding {
    pub fn new(state: Entity<AppState>, step: Step, cx: &mut Context<Self>) -> Self {
        let observe_state = cx.observe(&state, |this, state, cx| {
            if this.awaiting_sign_in && state.read(cx).signed_in() {
                this.awaiting_sign_in = false;
                this.go(Step::Providers, cx);
            }
            // The engine arrived (or was replaced by the sync switch) while
            // the agents step had nothing to ask.
            if this.step == Step::Providers && matches!(this.harnesses, Loadable::Idle) {
                this.load(cx);
            }
            cx.notify();
        });
        let mut onboarding = Self {
            state,
            step: Step::Welcome,
            awaiting_sign_in: false,
            harnesses: Loadable::Idle,
            installing: None,
            error: None,
            expanded: None,
            accounts_page: None,
            load_task: None,
            install_task: None,
            _observe_state: observe_state,
            _observe_accounts: None,
            card_bounds: Default::default(),
            wallpaper: Default::default(),
            wallpaper_error: None,
        };
        onboarding.go(step, cx);
        onboarding
    }

    pub fn step(&self) -> Step {
        self.step
    }

    fn go(&mut self, step: Step, cx: &mut Context<Self>) {
        self.step = step;
        // Look for agents a step early, so the list is there on arrival.
        if matches!(step, Step::Sync | Step::Providers | Step::Done)
            && !matches!(self.harnesses, Loadable::Ready(_) | Loadable::Loading)
        {
            self.load(cx);
        }
        cx.notify();
    }

    fn finish(&mut self, add_project: bool, cx: &mut Context<Self>) {
        cx.emit(OnboardingEvent::Finish { add_project });
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        self.awaiting_sign_in = true;
        cx.emit(OnboardingEvent::SignIn);
    }

    /// This device's agents and their logins. A painted list stays while a
    /// rescan runs.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.harnesses = Loadable::Idle;
            return;
        };
        if !matches!(self.harnesses, Loadable::Ready(_)) {
            self.harnesses = Loadable::Loading;
        }
        self.error = None;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let harnesses = engine
                .client()
                .call(methods::LIST_HARNESSES, serde_json::json!({}))
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |this, cx| {
                this.harnesses = match harnesses {
                    Ok(list) => Loadable::Ready(list),
                    Err(error) => Loadable::Error(error),
                };
                cx.notify();
            })
            .ok();
            let accounts = engine
                .client()
                .call(
                    methods::LIST_AGENT_ACCOUNTS,
                    serde_json::json!({ "forceUsage": false }),
                )
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<AgentAccountsSnapshot>(value).ok());
            if let Some(snapshot) = accounts {
                this.update(cx, |_, cx| {
                    cx.default_global::<AccountsSnapshotCache>()
                        .0
                        .insert(None, snapshot);
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }

    fn install(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.installing = Some(harness);
        self.error = None;
        self.install_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::INSTALL_HARNESS,
                    serde_json::json!({ "harness": harness, "targetDeviceId": null }),
                )
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |this, cx| {
                this.installing = None;
                match result {
                    Ok(list) => {
                        this.harnesses = Loadable::Ready(list);
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(error) => {
                        this.error = Some(format!("Couldn't install it — {error}").into())
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn cancel_install(&mut self, cx: &mut Context<Self>) {
        let Some(harness) = self.installing else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        cx.background_spawn(async move {
            engine
                .client()
                .call(
                    methods::CANCEL_INSTALL,
                    serde_json::json!({ "harness": harness, "targetDeviceId": null }),
                )
                .await
                .ok();
        })
        .detach();
    }

    fn choose_wallpaper(&mut self, cx: &mut Context<Self>) {
        self.wallpaper_error = None;
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose Wallpaper".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.wallpaper_error =
                    crate::settings::install_new_thread_composer_background(&path, cx)
                        .err()
                        .map(SharedString::from);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remove_wallpaper(&mut self, cx: &mut Context<Self>) {
        self.wallpaper_error = crate::settings::remove_new_thread_composer_background(cx)
            .err()
            .map(SharedString::from);
        cx.notify();
    }

    /// The new-chat wallpaper, painted exactly as the new-chat canvas paints
    /// it — same crop, effect and fade — with the card as the cleared shape.
    fn render_wallpaper(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let settings = crate::settings::current(cx);
        let background = settings.new_thread_composer_background;
        let path = background
            .as_ref()
            .map(|background| PathBuf::from(&background.path));
        let adjustment = background
            .as_ref()
            .map(|background| background.adjustment)
            .unwrap_or_default();
        let image = path.as_deref().and_then(|path| {
            crate::new_thread_background_effects::prepare(
                settings.new_thread_background_effect,
                theme,
                path,
                cx,
            )
        });
        let frame = self.wallpaper.frame(
            image,
            path.as_deref(),
            adjustment,
            path.is_some(),
            motion::reduced_motion(cx),
            std::time::Instant::now(),
        );
        if frame.active {
            window.request_animation_frame();
        }
        let viewport = window.viewport_size();
        let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
        let opacity = crate::shell::new_thread_background_opacity(theme.is_frost());
        div()
            .absolute()
            .inset_0()
            .child(crate::shell::new_thread_background(
                frame.previous,
                frame.previous_adjustment,
                height,
                width,
                self.card_bounds.clone(),
                0.0,
                (1.0 - frame.mix) * opacity,
            ))
            .child(crate::shell::new_thread_background(
                frame.current,
                frame.current_adjustment,
                height,
                width,
                self.card_bounds.clone(),
                0.0,
                frame.mix * opacity,
            ))
            .into_any_element()
    }

    /// Open (or close) one agent's sign-in, the same accounts section
    /// Settings → Providers shows under an expanded agent.
    fn toggle_accounts(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if self.expanded == Some(harness) {
            self.expanded = None;
            cx.notify();
            return;
        }
        self.expanded = Some(harness);
        if let Some(page) = &self.accounts_page {
            page.update(cx, |page, cx| page.set_embedded_harness(harness, cx));
        } else {
            let state = self.state.clone();
            let page = cx.new(|cx| AccountsPage::new_embedded(state, None, harness, cx));
            self._observe_accounts = Some(cx.observe(&page, |_, _, cx| cx.notify()));
            self.accounts_page = Some(page);
        }
        cx.notify();
    }
}

// ---- pieces ----

fn title(theme: &Theme, text: &'static str) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(20.0))
        .line_height(px(26.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .child(SharedString::from(text))
}

fn blurb(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .mt(px(6.0))
        .text_size(crate::typography::ui_rems(13.0))
        .line_height(px(19.0))
        .text_color(theme.text_muted)
        .child(text.into())
}

/// The gate's full-width white button.
fn wide_button(theme: &Theme, id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w_full()
        .h(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .bg(theme.text)
        .text_size(crate::typography::ui_rems(14.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.on_solid)
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(SharedString::from(label))
}

fn quiet_link(theme: &Theme, id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text_muted.opacity(0.7))
        .cursor_pointer()
        .hover(|s| s.text_color(theme.text))
        .child(SharedString::from(label))
}

fn primary(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    popover::btn_primary(theme, label)
        .id(id)
        .px(px(14.0))
        .when(!enabled, |el| el.opacity(0.35).cursor_default())
}

/// One reason to do a step: a quiet glyph, a line, a muted explainer.
fn benefit(
    theme: &Theme,
    glyph: &'static str,
    heading: &'static str,
    detail: &'static str,
) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .flex_none()
                .size(px(32.0))
                .rounded(px(8.0))
                .bg(theme.wash(0.06))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph).size(px(16.0)).text_color(theme.text_muted)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(heading)),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(px(17.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(detail)),
                ),
        )
}

/// Three short segments: done and current in the accent, the rest a wash.
fn progress(theme: &Theme, number: usize) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(4.0))
                .children((1..=NUMBERED_STEPS).map(|ix| {
                    div()
                        .w(px(20.0))
                        .h(px(3.0))
                        .rounded(px(2.0))
                        .bg(if ix <= number {
                            theme.accent
                        } else {
                            theme.wash(0.12)
                        })
                })),
        )
        .child(
            div()
                .text_size(crate::typography::ui_rems(11.0))
                .text_color(theme.text_muted.opacity(0.7))
                .child(SharedString::from(format!("{number} of {NUMBERED_STEPS}"))),
        )
}

impl Onboarding {
    fn render_welcome(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .pt(px(8.0))
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .child(
                icon(icons::ZERON_LOGO)
                    .w(px(36.6))
                    .h(px(42.0))
                    .text_color(theme.text),
            )
            .child(
                div()
                    .mt(px(28.0))
                    .text_size(crate::typography::ui_rems(24.0))
                    .line_height(px(30.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(SharedString::from("Welcome to Zeron")),
            )
            .child(
                div()
                    .mt(px(10.0))
                    .max_w(px(330.0))
                    .text_size(crate::typography::ui_rems(14.0))
                    .line_height(px(21.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        "Run your coding agents side by side — on this computer, your other devices, or in the cloud.",
                    )),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .mt(px(36.0))
                    .items_center()
                    .gap(px(16.0))
                    .child(
                        wide_button(theme, "onboarding-start", "Get started")
                            .on_click(cx.listener(|this, _, _, cx| this.go(Step::Appearance, cx))),
                    )
                    .child(
                        quiet_link(theme, "onboarding-skip-welcome", "Skip setup")
                            .on_click(cx.listener(|this, _, _, cx| this.finish(false, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_appearance(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current_mode = appearance::mode(cx);
        let themes = appearance::themes(cx);
        let current_accent = appearance::accent(cx);
        let surface = appearance::surface(cx);
        let reduced_motion = motion::reduced_motion(cx);
        let cards = AppearanceMode::ALL.into_iter().map(|mode| {
            let selected = mode == current_mode;
            let selection_t = widgets::tab_selection_t(
                window,
                format!("onboarding-mode-{}-selection", mode.label()),
                selected,
                reduced_motion,
            );
            widgets::option_card(
                theme,
                mode.icon(),
                mode.label(),
                selected,
                selection_t,
                crate::settings::appearance::preview(mode, &themes, current_accent, surface),
            )
            .id(SharedString::from(format!(
                "onboarding-mode-{}",
                mode.label()
            )))
            .on_click(cx.listener(move |_, _, _, cx| {
                appearance::set_mode(mode, cx);
                cx.notify();
            }))
        });
        let mut accents = vec![zeron_theme::AccentSelection::ThemeDefault];
        accents.extend(zeron_theme::AccentPreset::ALL.map(zeron_theme::AccentSelection::Preset));
        let swatches = accents.into_iter().map(|selection| {
            crate::settings::appearance::accent_swatch(
                theme,
                selection,
                selection == current_accent,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                appearance::set_accent(selection, cx);
                cx.notify();
            }))
        });
        div()
            .flex()
            .flex_col()
            .child(title(theme, "Make it yours"))
            .child(blurb(
                theme,
                "Pick a look. You can change it any time in Settings → Appearance.",
            ))
            .child(
                widgets::option_card_row()
                    .mt(px(24.0))
                    .gap(px(12.0))
                    .children(cards),
            )
            .child(
                div()
                    .mt(px(24.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(SharedString::from("Accent")),
            )
            .child(
                div()
                    .mt(px(10.0))
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(10.0))
                    .children(swatches),
            )
            .child(self.render_wallpaper_choice(theme, cx))
            .into_any_element()
    }

    /// The new-chat wallpaper: an image of your own, then a treatment for it.
    /// It shows behind this card as soon as it's chosen.
    fn render_wallpaper_choice(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let settings = crate::settings::current(cx);
        let background = settings
            .new_thread_composer_background
            .filter(|background| Path::new(&background.path).is_file());
        let current_effect = settings.new_thread_background_effect;
        let tile = div()
            .flex_none()
            .size(px(40.0))
            .rounded(px(8.0))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border)
            .bg(theme.wash(0.04))
            .flex()
            .items_center()
            .justify_center()
            .map(|tile| match &background {
                Some(background) => tile.child(
                    img(PathBuf::from(&background.path))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                ),
                None => tile.child(
                    icon(icons::GALLERY)
                        .size(px(16.0))
                        .text_color(theme.text_muted.opacity(0.7)),
                ),
            });
        let effects = NewThreadBackgroundEffect::ALL.into_iter().map(|effect| {
            let selected = effect == current_effect;
            div()
                .id(SharedString::from(format!(
                    "onboarding-effect-{}",
                    effect.label()
                )))
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .text_size(crate::typography::ui_rems(12.0))
                .cursor_pointer()
                .map(|chip| {
                    if selected {
                        chip.bg(theme.accent_wash)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.accent)
                    } else {
                        chip.text_color(theme.text_muted)
                            .hover(|s| s.bg(theme.wash(0.06)).text_color(theme.text))
                    }
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    crate::settings::set_new_thread_background_effect(effect, cx);
                    cx.notify();
                }))
                .child(SharedString::from(effect.label()))
        });
        div()
            .mt(px(24.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(SharedString::from("New chat wallpaper")),
            )
            .child(
                div()
                    .mt(px(10.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(tile)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(13.0))
                                    .text_color(theme.text)
                                    .child(SharedString::from(
                                        background
                                            .as_ref()
                                            .map(|background| background.name.clone())
                                            .unwrap_or_else(|| "None".into()),
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted.opacity(0.8))
                                    .child(SharedString::from("Shows behind every new chat.")),
                            ),
                    )
                    .when(background.is_some(), |row| {
                        row.child(
                            widgets::ghost_action(theme)
                                .id("onboarding-wallpaper-remove")
                                .flex_none()
                                .on_click(cx.listener(|this, _, _, cx| this.remove_wallpaper(cx)))
                                .child("Remove"),
                        )
                    })
                    .child(
                        widgets::ghost_action(theme)
                            .id("onboarding-wallpaper-choose")
                            .flex_none()
                            .on_click(cx.listener(|this, _, _, cx| this.choose_wallpaper(cx)))
                            .child(if background.is_some() {
                                "Change"
                            } else {
                                "Choose image"
                            }),
                    ),
            )
            .when(background.is_some(), |section| {
                section.child(
                    div()
                        .mt(px(12.0))
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(4.0))
                        .children(effects),
                )
            })
            .when_some(self.wallpaper_error.clone(), |section, error| {
                section.child(
                    div()
                        .mt(px(8.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.danger_muted.opacity(0.9))
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn render_sync(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let email = state
            .signed_in()
            .then(|| state.auth_user().map(|user| user.email.clone()))
            .flatten();
        let signed_in = state.signed_in();
        div()
            .flex()
            .flex_col()
            .child(title(theme, "Sync your work"))
            .child(blurb(
                theme,
                "Log in to keep your chats, projects and settings in step everywhere you use Zeron.",
            ))
            .child(
                div()
                    .mt(px(28.0))
                    .flex()
                    .flex_col()
                    .gap(px(18.0))
                    .child(benefit(
                        theme,
                        icons::SMARTPHONE,
                        "On every device",
                        "Pick up a chat on your phone or another computer, right where you left it.",
                    ))
                    .child(benefit(
                        theme,
                        icons::REMOTE_SERVER,
                        "Run agents anywhere",
                        "Start work on any of your machines from whichever one you're at.",
                    ))
                    .child(benefit(
                        theme,
                        icons::CLOUD,
                        "Zeron Cloud",
                        "Hand chats to cloud machines that keep going while your computer sleeps.",
                    )),
            )
            .child(
                div()
                    .mt(px(28.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(8.0))
                    .bg(if signed_in {
                        theme.accent_wash
                    } else {
                        theme.wash(0.04)
                    })
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .line_height(px(17.0))
                    .child(
                        icon(if signed_in { icons::CHECK } else { icons::LAPTOP })
                            .size(px(14.0))
                            .flex_none()
                            .text_color(if signed_in { theme.accent } else { theme.text_muted }),
                    )
                    .child(
                        div()
                            .text_color(if signed_in { theme.text } else { theme.text_muted })
                            .child(SharedString::from(match (&email, signed_in) {
                                (Some(email), true) => format!("Signed in as {email}"),
                                (None, true) => "You're signed in".into(),
                                _ => format!(
                                    "Not ready? Everything stays on {} until you log in.",
                                    this_device()
                                ),
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_providers(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let summary = agents_summary(&self.harnesses);
        let (heading, detail): (&'static str, String) = match &summary {
            AgentsSummary::Looking => (
                "Your agents",
                format!("Looking for coding agents on {}…", this_device()),
            ),
            AgentsSummary::Unavailable(_) => (
                "Your agents",
                format!("Couldn't check {} for agents.", this_device()),
            ),
            AgentsSummary::NoneInstalled => (
                "Install an agent",
                format!(
                    "Zeron runs the coding agents you already use, and none are on {} yet. Install one to continue.",
                    this_device()
                ),
            ),
            AgentsSummary::Installed(names) => (
                "Your agents",
                format!(
                    "Found {} on {}. Sign in to any you'd like to use.",
                    if names.len() > 3 {
                        format!("{} agents", names.len())
                    } else {
                        join_names(names)
                    },
                    this_device()
                ),
            ),
        };
        let list: AnyElement = match &self.harnesses {
            Loadable::Idle | Loadable::Loading => div()
                .mt(px(20.0))
                .child(popover::skeleton_rows(
                    "onboarding-agents-skeleton",
                    theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Loadable::Error(error) => div()
                .mt(px(20.0))
                .child(popover::error_row(theme, error))
                .into_any_element(),
            Loadable::Ready(list) => {
                let snapshot = cx
                    .try_global::<AccountsSnapshotCache>()
                    .and_then(|cache| cache.0.get(&None))
                    .cloned();
                let mut descriptors: Vec<HarnessDescriptor> =
                    crate::pickers::visible_harnesses(list)
                        .into_iter()
                        .filter(|descriptor| descriptor.id != HarnessId::Mock)
                        .collect();
                // Installed agents first; otherwise the engine's order.
                descriptors.sort_by_key(|descriptor| !descriptor.installed);
                let rows: Vec<AnyElement> = descriptors
                    .iter()
                    .enumerate()
                    .map(|(ix, descriptor)| {
                        self.provider_row(ix, descriptor, snapshot.as_ref(), theme, cx)
                    })
                    .collect();
                div()
                    .id("onboarding-agents")
                    .mt(px(20.0))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(widgets::section_card(theme).mt(px(0.0)).children(rows))
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_between()
                    .gap(px(12.0))
                    .child(title(theme, heading))
                    .when(
                        matches!(self.harnesses, Loadable::Ready(_) | Loadable::Error(_)),
                        |el| {
                            el.child(
                                div()
                                    .id("onboarding-rescan")
                                    .mt(px(4.0))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(5.0))
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted.opacity(0.8))
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(theme.text))
                                    .on_click(cx.listener(|this, _, _, cx| this.load(cx)))
                                    .child(
                                        icon(icons::REFRESH)
                                            .size(px(12.0))
                                            .text_color(theme.text_muted),
                                    )
                                    .child(SharedString::from("Check again")),
                            )
                        },
                    ),
            )
            .child(blurb(theme, detail))
            .child(list)
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .mt(px(12.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(px(17.0))
                        .text_color(theme.danger_muted.opacity(0.9))
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn provider_row(
        &self,
        ix: usize,
        descriptor: &HarnessDescriptor,
        snapshot: Option<&AgentAccountsSnapshot>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let harness = descriptor.id;
        let installed = descriptor.installed;
        let installing = self.installing == Some(harness);
        let account = installed.then(|| signed_in_as(snapshot, harness)).flatten();
        let signs_in = installed && accounts::signs_in(harness);
        let expanded = signs_in && self.expanded == Some(harness);
        let (icon_path, tint) = crate::pickers::harness_brand_icon(harness);
        let meta: AnyElement = if installing {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(crate::loaders::mini_mono_spinner(
                    format!("onboarding-install-{harness:?}"),
                    1.5,
                    theme.text_muted,
                    cx.entity_id(),
                    cx,
                ))
                .child(SharedString::from("Installing…"))
                .into_any_element()
        } else if !installed {
            let text = match (
                descriptor.can_install,
                zeron_harness::install::manual_command(harness),
            ) {
                (false, Some(command)) => format!("Install with {command}"),
                _ => "Not installed".into(),
            };
            div()
                .text_color(theme.text_muted.opacity(0.75))
                .child(SharedString::from(text))
                .into_any_element()
        } else if let Some(account) = &account {
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(icon(icons::CHECK).size(px(12.0)).text_color(theme.accent))
                .child(SharedString::from(account.clone()))
                .into_any_element()
        } else if signs_in {
            div()
                .child(SharedString::from("Installed · not signed in"))
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(icon(icons::CHECK).size(px(12.0)).text_color(theme.accent))
                .child(SharedString::from("Ready"))
                .into_any_element()
        };
        let tile = div()
            .flex_none()
            .size(px(36.0))
            .rounded(px(10.0))
            .bg(theme.wash(0.06))
            .flex()
            .items_center()
            .justify_center()
            .child(
                icon(icon_path)
                    .size(px(16.0))
                    .text_color(tint.unwrap_or(theme.text_muted)),
            );
        let action: Option<AnyElement> = if installing {
            Some(
                widgets::ghost_action(theme)
                    .id(("onboarding-cancel-install", ix))
                    .flex_none()
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_install(cx)))
                    .child("Cancel")
                    .into_any_element(),
            )
        } else if !installed && descriptor.can_install {
            let free = self.installing.is_none();
            Some(
                widgets::ghost_action(theme)
                    .id(("onboarding-install", ix))
                    .flex_none()
                    .when(!free, |el| el.opacity(0.4))
                    .when(free, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| this.install(harness, cx)))
                    })
                    .child("Install")
                    .into_any_element(),
            )
        } else if signs_in {
            Some(
                widgets::ghost_action(theme)
                    .id(("onboarding-sign-in", ix))
                    .flex_none()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_accounts(harness, cx)))
                    .child(if expanded {
                        "Done"
                    } else if account.is_some() {
                        "Accounts"
                    } else {
                        "Sign in"
                    })
                    .into_any_element(),
            )
        } else {
            None
        };
        let header = widgets::card_row(theme, ix == 0)
            .min_h(px(56.0))
            .py(px(10.0))
            .flex_nowrap()
            .gap(px(12.0))
            .when(!installed && !installing, |el| el.opacity(0.7))
            .child(tile)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(widgets::row_title(theme, descriptor.name.clone()))
                    .child(widgets::meta_line(theme, vec![meta])),
            )
            .children(action);
        div()
            .flex()
            .flex_col()
            .child(header)
            .when(expanded, |row| {
                row.when_some(self.accounts_page.clone(), |row, page| {
                    row.child(motion::menu_in(
                        format!("onboarding-accounts-{harness:?}"),
                        div().mx(px(16.0)).pb(px(14.0)).child(page),
                    ))
                })
            })
            .into_any_element()
    }

    fn render_done(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state.read(cx);
        let has_projects = !state.spaces.is_empty();
        let sync = if state.signed_in() {
            state
                .auth_user()
                .map(|user| format!("Signed in as {}", user.email))
                .unwrap_or_else(|| "Signed in".into())
        } else {
            format!("Only on {}", this_device())
        };
        let look = format!(
            "{} · {}",
            appearance::mode(cx).label(),
            appearance::accent(cx).label()
        );
        let agents = match agents_summary(&self.harnesses) {
            AgentsSummary::Installed(names) => short_names(&names),
            AgentsSummary::NoneInstalled => "None yet".into(),
            AgentsSummary::Looking | AgentsSummary::Unavailable(_) => "—".into(),
        };
        let line = |label: &'static str, value: String, first: bool| {
            div()
                .px(px(14.0))
                .py(px(10.0))
                .when(!first, |el| el.border_t_1().border_color(theme.border))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .text_size(crate::typography::ui_rems(12.5))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.text_muted)
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .child(SharedString::from(value)),
                )
        };
        div()
            .w_full()
            .pt(px(8.0))
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .child(
                div()
                    .size(px(48.0))
                    .rounded_full()
                    .bg(theme.accent_wash)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(icons::CHECK).size(px(22.0)).text_color(theme.accent)),
            )
            .child(
                div()
                    .mt(px(20.0))
                    .text_size(crate::typography::ui_rems(22.0))
                    .line_height(px(28.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(SharedString::from("You're all set")),
            )
            .child(
                div()
                    .mt(px(8.0))
                    .max_w(px(320.0))
                    .text_size(crate::typography::ui_rems(13.0))
                    .line_height(px(19.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(if has_projects {
                        "Pick a project and start a chat."
                    } else {
                        "Add a project to start your first chat. A project is a folder on one of your devices."
                    })),
            )
            .child(
                div()
                    .mt(px(24.0))
                    .w_full()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_col()
                    .text_left()
                    .child(line("Appearance", look, true))
                    .child(line("Sync", sync, false))
                    .child(line("Agents", agents, false)),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .mt(px(28.0))
                    .items_center()
                    .gap(px(14.0))
                    .map(|el| {
                        if has_projects {
                            el.child(
                                wide_button(theme, "onboarding-finish", "Start using Zeron")
                                    .on_click(cx.listener(|this, _, _, cx| this.finish(false, cx))),
                            )
                        } else {
                            el.child(
                                wide_button(theme, "onboarding-add-project", "Add a project")
                                    .on_click(cx.listener(|this, _, _, cx| this.finish(true, cx))),
                            )
                            .child(
                                quiet_link(theme, "onboarding-later", "I'll do this later")
                                    .on_click(cx.listener(|this, _, _, cx| this.finish(false, cx))),
                            )
                        }
                    }),
            )
            .into_any_element()
    }

    /// Back on the left; the step's way forward on the right.
    fn render_footer(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let back = popover::btn_ghost(theme, "Back", "onboarding-back-fade")
            .id("onboarding-back")
            // Sit the label, not the hover pad, on the content edge.
            .ml(px(-12.0))
            .on_click(cx.listener(|this, _, _, cx| this.go(this.step.previous(), cx)));
        let forward: AnyElement = match self.step {
            Step::Sync if !self.state.read(cx).signed_in() => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    popover::btn_ghost(theme, "Skip for now", "onboarding-skip-sync-fade")
                        .id("onboarding-skip-sync")
                        .on_click(cx.listener(|this, _, _, cx| this.go(Step::Providers, cx))),
                )
                .child(
                    primary(theme, "onboarding-log-in", "Log in", true)
                        .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                )
                .into_any_element(),
            Step::Providers => {
                let ready = matches!(agents_summary(&self.harnesses), AgentsSummary::Installed(_));
                let checked = matches!(
                    agents_summary(&self.harnesses),
                    AgentsSummary::NoneInstalled | AgentsSummary::Unavailable(_)
                );
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    // Nothing to run yet: the way on is installing one, but
                    // the rest of the app stays reachable.
                    .when(checked, |el| {
                        el.child(
                            popover::btn_ghost(
                                theme,
                                "Skip for now",
                                "onboarding-skip-agents-fade",
                            )
                            .id("onboarding-skip-agents")
                            .on_click(cx.listener(|this, _, _, cx| this.go(Step::Done, cx))),
                        )
                    })
                    .child(
                        primary(theme, "onboarding-continue", "Continue", ready)
                            .when(ready, |el| {
                                el.on_click(cx.listener(|this, _, _, cx| this.go(Step::Done, cx)))
                            }),
                    )
                    .into_any_element()
            }
            _ => primary(theme, "onboarding-continue", "Continue", true)
                .on_click(cx.listener(|this, _, _, cx| this.go(this.step.next(), cx)))
                .into_any_element(),
        };
        div()
            .flex_none()
            .pt(px(20.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(back)
            .child(forward)
            .into_any_element()
    }
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let step = self.step;
        let body = match step {
            Step::Welcome => self.render_welcome(&theme, cx),
            Step::Appearance => self.render_appearance(&theme, window, cx),
            Step::Sync => self.render_sync(&theme, cx),
            Step::Providers => self.render_providers(&theme, cx),
            Step::Done => self.render_done(&theme, cx),
        };
        let height =
            (f32::from(window.viewport_size().height) - 96.0).clamp(420.0, CARD_MAX_HEIGHT);
        let header = step.number().map(|number| {
            div()
                .flex_none()
                .h(px(20.0))
                .mb(px(24.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(progress(&theme, number))
                .child(
                    quiet_link(&theme, "onboarding-skip", "Skip setup")
                        .on_click(cx.listener(|this, _, _, cx| this.finish(false, cx))),
                )
        });
        let footer = step.number().map(|_| self.render_footer(&theme, cx));
        // The numbered steps share one fixed frame so Back/Continue never
        // move; the welcome and the ending fit their content.
        let fixed = step.number().is_some();
        let card = div()
            .w(px(if fixed {
                CARD_WIDTH
            } else {
                COMPACT_CARD_WIDTH
            }))
            .when(fixed, |card| card.h(px(height)))
            .p(px(CARD_PADDING))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_card)
            .shadow_lg()
            .flex()
            .flex_col()
            .children(header)
            .child(
                // Keyed per step: every step change replays the entrance.
                div()
                    .when(fixed, |el| el.flex_1().min_h_0())
                    .flex()
                    .flex_col()
                    .child(motion::fade_in(
                        SharedString::from(format!("onboarding-{}", step.key())),
                        div()
                            .w_full()
                            .when(fixed, |el| el.h_full())
                            .flex()
                            .flex_col()
                            .child(body),
                    )),
            )
            .children(footer)
            .child({
                let measured = self.card_bounds.clone();
                // Prepaint runs before any paint, so the wallpaper reads this
                // frame's card, never the last one's.
                gpui::canvas(
                    move |bounds, _, _| measured.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
            });
        let wallpaper = self.render_wallpaper(&theme, window, cx);
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(theme.bg)
            .child(wallpaper)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(motion::fade_in("onboarding-card", card)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(id: HarnessId, name: &str, installed: bool) -> HarnessDescriptor {
        HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: false,
            steering_mode: zeron_proto::SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed,
            can_install: true,
            enabled: None,
        }
    }

    #[test]
    fn steps_walk_forward_and_back() {
        let mut step = Step::Welcome;
        let mut seen = vec![step];
        while step != Step::Done {
            step = step.next();
            seen.push(step);
        }
        assert_eq!(
            seen,
            [
                Step::Welcome,
                Step::Appearance,
                Step::Sync,
                Step::Providers,
                Step::Done
            ]
        );
        assert_eq!(Step::Done.previous(), Step::Providers);
        assert_eq!(Step::Appearance.previous(), Step::Welcome);
        assert_eq!(Step::parse("agents"), Step::Providers);
        assert_eq!(Step::parse("1"), Step::Welcome);
        assert_eq!(
            [
                Step::Welcome,
                Step::Appearance,
                Step::Sync,
                Step::Providers,
                Step::Done
            ]
            .map(Step::number),
            [None, Some(1), Some(2), Some(3), None]
        );
    }

    #[test]
    fn the_agents_step_says_what_it_found() {
        assert_eq!(agents_summary(&Loadable::Loading), AgentsSummary::Looking);
        let none = Loadable::Ready(vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code", false),
            descriptor(HarnessId::Codex, "Codex", false),
        ]);
        assert_eq!(agents_summary(&none), AgentsSummary::NoneInstalled);
        let some = Loadable::Ready(vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code", true),
            descriptor(HarnessId::Codex, "Codex", false),
            descriptor(HarnessId::Cursor, "Cursor", true),
        ]);
        let AgentsSummary::Installed(names) = agents_summary(&some) else {
            panic!("expected installed agents");
        };
        assert_eq!(join_names(&names), "Claude Code and Cursor");
        let many: Vec<String> = ["A", "B", "C", "D", "E"].map(String::from).into();
        assert_eq!(short_names(&many), "A, B and 3 more");
        assert_eq!(short_names(&many[..3]), "A, B and C");
        assert_eq!(
            join_names(&["A".into(), "B".into(), "C".into()]),
            "A, B and C"
        );
    }
}
