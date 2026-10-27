//! First-run setup: a welcome, then appearance, sync and the agents on this
//! device, ending on "You're all set" with the way to a first project.
//!
//! Shown in place of the Ready shell until it's finished
//! (`onboardingCompleted` in `ui-settings.json`); only a fresh install starts
//! it. Each step is a full-window [`render_page`]: the Settings column, type
//! and blocks straight on the window's surface. The shell owns what the steps
//! can't do from here — the sign-in flow and the add-project dialog — and
//! hears about them through [`OnboardingEvent`]. Its sign-in steps render
//! through the same page, inside the sync step while setup runs, so logging
//! in looks the same wherever it starts; the signed-in edge moves setup on to
//! the agents step.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, IntoElement, ObjectFit, SharedString,
    StyledImage as _, Subscription, Window, div, img, prelude::*, px,
};

use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::HarnessId;

use crate::appearance::{self, AppearanceMode};
use crate::icons::{self, icon};
use crate::motion;
use crate::popover::{self, Loadable};
use crate::settings::cloud::CloudPage;
use crate::settings::harnesses::HarnessesPage;
use crate::settings::{NewThreadBackgroundEffect, widgets};
use crate::state::{AppState, EngineHandle};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Appearance,
    Sync,
    /// Signed in only: GitHub for Cloud agents (skippable).
    Github,
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
            "github" => Step::Github,
            "providers" | "agents" => Step::Providers,
            "done" => Step::Done,
            _ => Step::Welcome,
        }
    }

    /// The way on. `github`: the GitHub step is in the flow (signed in).
    fn next(self, github: bool) -> Step {
        match self {
            Step::Welcome => Step::Appearance,
            Step::Appearance => Step::Sync,
            Step::Sync if github => Step::Github,
            Step::Sync | Step::Github => Step::Providers,
            Step::Providers | Step::Done => Step::Done,
        }
    }

    fn previous(self, github: bool) -> Step {
        match self {
            Step::Welcome | Step::Appearance => Step::Welcome,
            Step::Sync => Step::Appearance,
            Step::Github => Step::Sync,
            Step::Providers if github => Step::Github,
            Step::Providers => Step::Sync,
            Step::Done => Step::Providers,
        }
    }

    /// Place among the numbered steps; the welcome and the ending aren't
    /// counted, nor the GitHub step when signed out.
    fn number(self, github: bool) -> Option<usize> {
        match self {
            Step::Appearance => Some(1),
            Step::Sync => Some(2),
            Step::Github => Some(3),
            Step::Providers => Some(if github { 4 } else { 3 }),
            Step::Welcome | Step::Done => None,
        }
    }

    /// How many numbered steps there are.
    fn total(github: bool) -> usize {
        if github { 4 } else { 3 }
    }

    fn key(self) -> &'static str {
        match self {
            Step::Welcome => "welcome",
            Step::Appearance => "appearance",
            Step::Sync => "sync",
            Step::Github => "github",
            Step::Providers => "providers",
            Step::Done => "done",
        }
    }
}

/// A page's content measure: narrower than Settings' column, since one
/// question sits in it, with the same air either side.
const COLUMN_WIDTH: f32 = 560.0;
const COLUMN_PAD_X: f32 = 40.0;
/// The numbered steps' shared frame, tall enough for the agents list.
const FRAME_MIN_HEIGHT: f32 = 420.0;
const FRAME_MAX_HEIGHT: f32 = 640.0;

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

pub struct Onboarding {
    state: Entity<AppState>,
    step: Step,
    /// Log in was pressed: the signed-in edge moves on to the GitHub step.
    awaiting_sign_in: bool,
    /// What the agents page last found, mirrored for the summary and the
    /// way on.
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    /// Settings → Providers' rows for this device — toggles, installs, and
    /// each agent's settings and sign-in under its caret.
    agents_page: Option<Entity<HarnessesPage>>,
    /// The engine the agents page talks to; the sync switch replaces it.
    agents_engine: Option<EngineHandle>,
    _observe_state: Subscription,
    _observe_agents: Option<Subscription>,
    /// Settings → Cloud's GitHub block, built when the GitHub step is
    /// reached (it also turns Cloud on for the account).
    github_page: Option<Entity<CloudPage>>,
    _observe_github: Option<Subscription>,
    effect_select: widgets::SelectState,
    /// The page column as last laid out: the wallpaper clears around it the
    /// way the new-chat wallpaper clears around the composer.
    column_bounds: crate::new_thread_background_mask::SurfaceBounds,
    wallpaper: crate::new_thread_background_effects::Readiness,
    wallpaper_error: Option<SharedString>,
}

impl EventEmitter<OnboardingEvent> for Onboarding {}

impl Onboarding {
    pub fn new(state: Entity<AppState>, step: Step, cx: &mut Context<Self>) -> Self {
        let observe_state = cx.observe(&state, |this, state, cx| {
            if this.awaiting_sign_in && state.read(cx).signed_in() {
                this.awaiting_sign_in = false;
                let next = if this.github_in_flow(cx) { Step::Github } else { Step::Providers };
                this.go(next, cx);
            }
            // The edge answered that this account has no Cloud while on its
            // GitHub step: move past it.
            if this.step == Step::Github && !this.github_in_flow(cx) {
                this.go(Step::Providers, cx);
            }
            // The engine arrived, or the sync switch replaced it: the agents
            // page has to ask the new one.
            let engine_changed = match (state.read(cx).engine(), &this.agents_engine) {
                (Some(engine), Some(seen)) => !engine.same_connection(seen),
                (Some(_), None) => true,
                (None, _) => false,
            };
            if engine_changed
                && matches!(
                    this.step,
                    Step::Sync | Step::Github | Step::Providers | Step::Done
                )
            {
                this.load(cx);
            }
            cx.notify();
        });
        let mut onboarding = Self {
            state,
            step: Step::Welcome,
            awaiting_sign_in: false,
            harnesses: Loadable::Idle,
            agents_page: None,
            agents_engine: None,
            _observe_state: observe_state,
            _observe_agents: None,
            github_page: None,
            _observe_github: None,
            effect_select: Default::default(),
            column_bounds: Default::default(),
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
        if matches!(
            step,
            Step::Sync | Step::Github | Step::Providers | Step::Done
        ) && !matches!(self.harnesses, Loadable::Ready(_) | Loadable::Loading)
        {
            self.load(cx);
        }
        if step == Step::Github && self.github_page.is_none() {
            let state = self.state.clone();
            let page = cx.new(|cx| CloudPage::new_github_only(state, cx));
            self._observe_github = Some(cx.observe(&page, |_, _, cx| cx.notify()));
            self.github_page = Some(page);
        }
        cx.notify();
    }

    /// Whether the GitHub step is part of setup: signed in, and Cloud offered
    /// to the account (kept while its status is still unknown).
    fn github_in_flow(&self, cx: &Context<Self>) -> bool {
        self.state.read(cx).cloud_maybe_offered()
    }

    fn github_connected(&self, cx: &Context<Self>) -> bool {
        self.github_page
            .as_ref()
            .is_some_and(|page| page.read(cx).github_connected())
    }

    fn finish(&mut self, add_project: bool, cx: &mut Context<Self>) {
        cx.emit(OnboardingEvent::Finish { add_project });
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        self.awaiting_sign_in = true;
        cx.emit(OnboardingEvent::SignIn);
    }

    /// This device's agents: the Providers page, built against the current
    /// engine, or asked again. A painted list stays while a rescan runs.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if let Some(page) = &self.agents_page
            && self
                .agents_engine
                .as_ref()
                .is_some_and(|seen| seen.same_connection(&engine))
        {
            page.update(cx, |page, cx| page.reload(cx));
            return;
        }
        let state = self.state.clone();
        let page = cx.new(|cx| HarnessesPage::new_embedded(state, cx));
        self._observe_agents = Some(cx.observe(&page, |this, page, cx| {
            this.harnesses = page.read(cx).harnesses().clone();
            cx.notify();
        }));
        self.harnesses = page.read(cx).harnesses().clone();
        self.agents_page = Some(page);
        self.agents_engine = Some(engine);
        cx.notify();
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
    /// it — same crop, effect and fade — with the column as the cleared shape.
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
                self.column_bounds.clone(),
                0.0,
                (1.0 - frame.mix) * opacity,
            ))
            .child(crate::shell::new_thread_background(
                frame.current,
                frame.current_adjustment,
                height,
                width,
                self.column_bounds.clone(),
                0.0,
                frame.mix * opacity,
            ))
            .into_any_element()
    }
}

// ---- the page ----

/// One full-window page of setup. The sign-in flow fills one too, so it reads
/// as part of setup wherever it started.
pub(crate) struct PageParts {
    /// Keys the body's entrance: a new key replays it.
    pub key: SharedString,
    /// The step progress, on the numbered steps.
    pub header: Option<AnyElement>,
    pub body: AnyElement,
    pub footer: Option<AnyElement>,
    /// One frame shared by the numbered steps (header on top, footer at the
    /// bottom) so Back and Continue never move; otherwise the column fits
    /// its content.
    pub fixed: bool,
    /// Painted under the column: the wallpaper.
    pub backdrop: Option<AnyElement>,
    /// Receives the column's bounds each frame, before anything paints.
    pub measure: Option<crate::new_thread_background_mask::SurfaceBounds>,
}

/// A sign-in step the shell owns, shown in the sync step's place while it
/// runs.
pub(crate) struct SignInSlot {
    pub key: SharedString,
    pub body: AnyElement,
    pub footer: AnyElement,
}

impl SignInSlot {
    /// The slot on its own, outside setup: the same page without progress.
    pub fn into_page(self) -> PageParts {
        PageParts {
            key: self.key,
            header: None,
            body: self.body,
            footer: Some(self.footer),
            fixed: false,
            backdrop: None,
            measure: None,
        }
    }
}

/// The page: a centered column under the titlebar strip, on the window's own
/// surface the way Settings sits on it — no card. The header and footer keep
/// their own entrance keys, so moving between numbered steps fades only the
/// body while the progress and the buttons hold still.
pub(crate) fn render_page(parts: PageParts, window: &Window) -> AnyElement {
    let fixed = parts.fixed;
    let height = (f32::from(window.viewport_size().height) - Theme::TITLEBAR_HEIGHT - 64.0)
        .clamp(FRAME_MIN_HEIGHT, FRAME_MAX_HEIGHT);
    let column = div()
        .relative()
        .w_full()
        .max_w(px(COLUMN_WIDTH + 2.0 * COLUMN_PAD_X))
        .px(px(COLUMN_PAD_X))
        .when(fixed, |column| column.h(px(height)))
        .flex()
        .flex_col()
        .children(parts.header.map(|header| {
            motion::fade_in(
                "onboarding-header",
                div().flex_none().h(px(20.0)).mb(px(28.0)).child(header),
            )
        }))
        .child(
            div()
                .when(fixed, |el| el.flex_1().min_h_0())
                .flex()
                .flex_col()
                .child(motion::fade_in(
                    parts.key,
                    div()
                        .w_full()
                        .when(fixed, |el| el.h_full())
                        .flex()
                        .flex_col()
                        .child(parts.body),
                )),
        )
        .children(parts.footer.map(|footer| {
            motion::fade_in(
                "onboarding-footer",
                div().flex_none().pt(px(24.0)).child(footer),
            )
        }))
        .when_some(parts.measure, |column, measured| {
            // Prepaint runs before any paint, so the wallpaper reads this
            // frame's column, never the last one's.
            column.child(
                gpui::canvas(
                    move |bounds, _, _| measured.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
        });
    div()
        .absolute()
        .inset_0()
        .children(parts.backdrop)
        .child(
            div()
                .absolute()
                .inset_0()
                .pt(px(Theme::TITLEBAR_HEIGHT))
                .pb(px(Theme::TITLEBAR_HEIGHT / 2.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(column),
        )
        .into_any_element()
}

// ---- pieces ----

/// A page's headline and the line under it: the Settings page header.
pub(crate) fn heading(theme: &Theme, title: &str, subtitle: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .child(widgets::page_header(theme, title, None))
        .child(widgets::page_subtitle(theme, subtitle))
}

/// A row's title over its muted explainer, wrapping in the row's width.
pub(crate) fn row_text(
    theme: &Theme,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(widgets::row_title(theme, title))
        .child(
            div()
                .mt(px(Theme::TEXT_STACK_GAP))
                .text_size(crate::typography::ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                .line_height(crate::typography::ui_rems(16.0))
                .text_color(theme.text_muted)
                .child(detail.into()),
        )
}

/// One block holding a single quiet line: where sync stands, what the
/// sign-in is waiting on.
pub(crate) fn status_block(
    theme: &Theme,
    leading: AnyElement,
    text: impl Into<SharedString>,
    emphasized: bool,
) -> gpui::Div {
    widgets::section_card(theme).mt(px(28.0)).child(
        widgets::card_row(theme, true)
            .min_h(px(44.0))
            .py(px(10.0))
            .flex_nowrap()
            .gap(px(10.0))
            .child(div().flex_none().flex().items_center().child(leading))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(12.5))
                    .line_height(px(17.0))
                    .text_color(if emphasized {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .child(text.into()),
            ),
    )
}

/// The wide white button the welcome and the ending end on.
pub(crate) fn wide_button(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
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

pub(crate) fn primary(
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

pub(crate) fn ghost(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
) -> gpui::Stateful<gpui::Div> {
    popover::btn_ghost(theme, label, format!("{id}-fade")).id(id)
}

/// The way back (or out) on the left, the way forward on the right. The
/// leading button sits its label, not its hover pad, on the content edge.
pub(crate) fn footer_row(
    back: Option<gpui::Stateful<gpui::Div>>,
    forward: Vec<AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(div().children(back.map(|back| back.ml(px(-12.0)))))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .children(forward),
        )
        .into_any_element()
}

/// One reason to do a step, as a settings row: a quiet glyph, a line, a
/// muted explainer.
fn benefit(
    theme: &Theme,
    first: bool,
    glyph: &'static str,
    heading: &'static str,
    detail: &'static str,
) -> gpui::Div {
    widgets::card_row(theme, first)
        .flex_nowrap()
        .gap(px(12.0))
        .child(widgets::row_tile(theme, glyph))
        .child(row_text(theme, heading, detail))
}

/// One short segment per step: done and current in the accent, the rest a
/// wash.
pub(crate) fn progress(theme: &Theme, number: usize, total: usize) -> gpui::Div {
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
                .children((1..=total).map(|ix| {
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
                .child(SharedString::from(format!("{number} of {total}"))),
        )
}

impl Onboarding {
    /// The current step as a page. `sign_in` stands in for the sync step's
    /// body and footer while the shell's sign-in flow runs, under the same
    /// progress, so it reads as this step.
    pub(crate) fn page_parts(
        &mut self,
        sign_in: Option<SignInSlot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PageParts {
        let theme = Theme::of(cx).for_settings_surface();
        let backdrop = Some(self.render_wallpaper(&theme, window, cx));
        let measure = Some(self.column_bounds.clone());
        let github = self.github_in_flow(cx);
        let total = Step::total(github);
        if let Some(slot) = sign_in {
            return PageParts {
                key: slot.key,
                header: Step::Sync
                    .number(github)
                    .map(|number| progress(&theme, number, total).into_any_element()),
                body: slot.body,
                footer: Some(slot.footer),
                fixed: true,
                backdrop,
                measure,
            };
        }
        let step = self.step;
        let body = match step {
            Step::Welcome => self.render_welcome(&theme, cx),
            Step::Appearance => self.render_appearance(&theme, window, cx),
            Step::Sync => self.render_sync(&theme, cx),
            Step::Github => self.render_github(&theme),
            Step::Providers => self.render_providers(&theme, cx),
            Step::Done => self.render_done(&theme, cx),
        };
        PageParts {
            key: SharedString::from(format!("onboarding-{}", step.key())),
            header: step
                .number(github)
                .map(|number| progress(&theme, number, total).into_any_element()),
            body,
            footer: step.number(github).map(|_| self.render_footer(&theme, cx)),
            fixed: step.number(github).is_some(),
            backdrop,
            measure,
        }
    }

    fn render_welcome(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
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
                    .max_w(px(360.0))
                    .text_size(crate::typography::ui_rems(14.0))
                    .line_height(px(21.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        "Run your coding agents side by side — on this computer, your other devices, or in the cloud.",
                    )),
            )
            .child(
                div().mt(px(36.0)).w(px(320.0)).child(
                    wide_button(theme, "onboarding-start", "Get started")
                        .on_click(cx.listener(|this, _, _, cx| this.go(Step::Appearance, cx))),
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
            .id("onboarding-appearance")
            .size_full()
            .overflow_y_scroll()
            .child(heading(
                theme,
                "Make it yours",
                "Pick a look. You can change it any time in Settings → Appearance.",
            ))
            .child(
                widgets::section(
                    theme,
                    "Color scheme",
                    widgets::option_card_row().gap(px(12.0)).children(cards),
                )
                .mt(px(24.0))
                .gap(px(12.0)),
            )
            .child(
                widgets::section_card(theme).mt(px(16.0)).child(
                    widgets::card_row(theme, true)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(80.0))
                                .child(widgets::row_title(theme, "Accent")),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(8.0))
                                .children(swatches),
                        ),
                ),
            )
            .child(self.render_wallpaper_choice(theme, cx))
            .into_any_element()
    }

    /// The new-chat wallpaper: an image of your own, then a treatment for it.
    /// It shows behind the page as soon as it's chosen.
    fn render_wallpaper_choice(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let settings = crate::settings::current(cx);
        let background = settings
            .new_thread_composer_background
            .filter(|background| Path::new(&background.path).is_file());
        let current_effect = settings.new_thread_background_effect;
        let tile = div()
            .flex_none()
            .size(px(36.0))
            .rounded(px(8.0))
            .overflow_hidden()
            .bg(theme.wash(0.06))
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
        let image_row = widgets::card_row(theme, true)
            .flex_nowrap()
            .gap(px(12.0))
            .child(tile)
            .child(row_text(
                theme,
                background
                    .as_ref()
                    .map(|background| background.name.clone())
                    .unwrap_or_else(|| "None".into()),
                "Shows behind every new chat.",
            ))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(8.0))
                    .when(background.is_some(), |actions| {
                        actions.child(
                            widgets::text_action(theme, widgets::ActionTone::Quiet, "Remove")
                                .id("onboarding-wallpaper-remove")
                                .on_click(cx.listener(|this, _, _, cx| this.remove_wallpaper(cx))),
                        )
                    })
                    .child(
                        widgets::text_action(
                            theme,
                            widgets::ActionTone::Filled,
                            if background.is_some() {
                                "Change"
                            } else {
                                "Choose image"
                            },
                        )
                        .id("onboarding-wallpaper-choose")
                        .on_click(cx.listener(|this, _, _, cx| this.choose_wallpaper(cx))),
                    ),
            );
        let effect_row = background.is_some().then(|| {
            let control = widgets::select(
                "onboarding-wallpaper-effect",
                "Background effect",
                theme,
                |this: &mut Self| &mut this.effect_select,
            )
            .options(
                NewThreadBackgroundEffect::ALL
                    .into_iter()
                    .map(|effect| widgets::SelectOption::new(effect.label())),
                NewThreadBackgroundEffect::ALL
                    .into_iter()
                    .position(|effect| effect == current_effect)
                    .unwrap_or_default(),
            )
            .width(128.0)
            .on_select(|_, ix, _, cx| {
                crate::settings::set_new_thread_background_effect(
                    NewThreadBackgroundEffect::ALL[ix],
                    cx,
                );
                cx.notify();
            })
            .render(&self.effect_select, cx);
            widgets::card_row(theme, false)
                .flex_nowrap()
                .child(row_text(theme, "Effect", current_effect.description()))
                .child(control)
        });
        widgets::section(
            theme,
            "New chat wallpaper",
            widgets::section_card(theme)
                .mt_0()
                .child(image_row)
                .children(effect_row),
        )
        .mt(px(28.0))
        .when_some(self.wallpaper_error.clone(), |section, error| {
            section.child(
                div()
                    .px(px(8.0))
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
            .child(heading(
                theme,
                "Sync your work",
                "Log in to keep your chats, projects and settings in step everywhere you use Zeron.",
            ))
            .child(
                widgets::section_card(theme)
                    .child(benefit(
                        theme,
                        true,
                        icons::SMARTPHONE,
                        "On every device",
                        "Pick up a chat on your phone or another computer, right where you left it.",
                    ))
                    .child(benefit(
                        theme,
                        false,
                        icons::REMOTE_SERVER,
                        "Run agents anywhere",
                        "Start work on any of your machines from whichever one you're at.",
                    ))
                    .child(benefit(
                        theme,
                        false,
                        icons::CLOUD,
                        "Zeron Cloud",
                        "Hand chats to cloud machines that keep going while your computer sleeps.",
                    )),
            )
            .child(status_block(
                theme,
                icon(if signed_in { icons::CHECK } else { icons::LAPTOP })
                    .size(px(14.0))
                    .text_color(if signed_in { theme.accent } else { theme.text_muted })
                    .into_any_element(),
                match (&email, signed_in) {
                    (Some(email), true) => format!("Signed in as {email}"),
                    (None, true) => "You're signed in".into(),
                    _ => format!(
                        "Not ready? Everything stays on {} until you log in.",
                        this_device()
                    ),
                },
                signed_in,
            ))
            .into_any_element()
    }

    /// GitHub for Cloud agents: Settings → Cloud's own GitHub block, which
    /// shows the account when it's already connected.
    fn render_github(&mut self, theme: &Theme) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .child(heading(
                theme,
                "Connect GitHub",
                "Cloud agents work in your repositories on machines of their own. Connect GitHub so they can clone and push — or skip and do it later in Settings → Cloud.",
            ))
            .children(
                self.github_page
                    .clone()
                    .map(|page| div().mt(px(24.0)).child(page)),
            )
            .into_any_element()
    }

    fn render_providers(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let signed_in = self.state.read(cx).signed_in();
        // Settings → Providers' own header controls: refresh, and once
        // you're signed in the device switcher, Cloud's logins among them.
        let toolbar = self
            .agents_page
            .clone()
            .map(|page| page.update(cx, |page, cx| page.render_toolbar(theme, signed_in, cx)));
        let place = self.agents_place(cx);
        let summary = agents_summary(&self.harnesses);
        let (title, detail): (&'static str, String) = match &summary {
            AgentsSummary::Looking => (
                "Your agents",
                format!("Looking for coding agents on {place}…"),
            ),
            AgentsSummary::Unavailable(_) => {
                ("Your agents", format!("Couldn't check {place} for agents."))
            }
            AgentsSummary::NoneInstalled => (
                "Install an agent",
                format!(
                    "Zeron runs the coding agents you already use, and none are on {place} yet. Install one to continue."
                ),
            ),
            AgentsSummary::Installed(names) => (
                "Your agents",
                format!(
                    "Found {} on {place}. Sign in to any you'd like to use.",
                    if names.len() > 3 {
                        format!("{} agents", names.len())
                    } else {
                        join_names(names)
                    },
                ),
            ),
        };
        // Settings → Providers' own rows: the switch, the caret and the
        // agent's settings and sign-in in its accordion.
        let list = self
            .agents_page
            .clone()
            .map(|page| div().mt(px(24.0)).flex_1().min_h_0().child(page));
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(heading(theme, title, detail)),
                    )
                    .children(toolbar),
            )
            .children(list)
            .into_any_element()
    }

    /// Where the agents page is looking, for its subtitle: this device, one
    /// of your others, or Cloud.
    fn agents_place(&self, cx: &Context<Self>) -> String {
        let target = self
            .agents_page
            .as_ref()
            .and_then(|page| page.read(cx).target_device().map(str::to_owned));
        match target {
            None => this_device().into(),
            Some(id) if id == zeron_proto::CLOUD_ACCOUNTS_DEVICE => "Cloud".into(),
            Some(id) => self
                .state
                .read(cx)
                .provider_devices()
                .find(|device| device.id == id)
                .map(|device| device.name.clone())
                .unwrap_or_else(|| "that device".into()),
        }
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
        // Signed in: whether Cloud agents can reach GitHub yet.
        let github = self.github_in_flow(cx).then(|| {
            if self.github_connected(cx) {
                "Connected".to_string()
            } else {
                "Not yet · Settings → Cloud".to_string()
            }
        });
        let agents = match agents_summary(&self.harnesses) {
            AgentsSummary::Installed(names) => short_names(&names),
            AgentsSummary::NoneInstalled => "None yet".into(),
            AgentsSummary::Looking | AgentsSummary::Unavailable(_) => "—".into(),
        };
        let line = |label: &'static str, value: String, first: bool| {
            widgets::card_row(theme, first)
                .min_h(px(44.0))
                .py(px(10.0))
                .flex_nowrap()
                .justify_between()
                .child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::ui_rems(widgets::ROW_TITLE_SIZE))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(widgets::ROW_TITLE_SIZE))
                        .text_color(theme.text)
                        .child(SharedString::from(value)),
                )
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
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
                    .max_w(px(340.0))
                    .text_center()
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
                widgets::section_card(theme)
                    .mt(px(28.0))
                    .w(px(400.0))
                    .child(line("Appearance", look, true))
                    .child(line("Sync", sync, false))
                    .when_some(github, |card, github| card.child(line("GitHub", github, false)))
                    .child(line("Agents", agents, false)),
            )
            .child(
                div()
                    .w(px(320.0))
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
        let back =
            ghost(theme, "onboarding-back", "Back").on_click(cx.listener(|this, _, _, cx| {
                let github = this.github_in_flow(cx);
                this.go(this.step.previous(github), cx)
            }));
        let forward: Vec<AnyElement> = match self.step {
            Step::Sync if !self.state.read(cx).signed_in() => vec![
                ghost(theme, "onboarding-skip-sync", "Skip for now")
                    .on_click(cx.listener(|this, _, _, cx| this.go(Step::Providers, cx)))
                    .into_any_element(),
                primary(theme, "onboarding-log-in", "Log in", true)
                    .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx)))
                    .into_any_element(),
            ],
            // Connected (now or before): on. Otherwise skippable.
            Step::Github if self.github_connected(cx) => vec![
                primary(theme, "onboarding-continue", "Continue", true)
                    .on_click(cx.listener(|this, _, _, cx| this.go(Step::Providers, cx)))
                    .into_any_element(),
            ],
            Step::Github => vec![
                ghost(theme, "onboarding-skip-github", "Skip for now")
                    .on_click(cx.listener(|this, _, _, cx| this.go(Step::Providers, cx)))
                    .into_any_element(),
            ],
            Step::Providers => {
                let ready = matches!(agents_summary(&self.harnesses), AgentsSummary::Installed(_));
                let checked = matches!(
                    agents_summary(&self.harnesses),
                    AgentsSummary::NoneInstalled | AgentsSummary::Unavailable(_)
                );
                let mut forward = Vec::new();
                // Nothing to run yet: the way on is installing one, but the
                // rest of the app stays reachable.
                if checked {
                    forward.push(
                        ghost(theme, "onboarding-skip-agents", "Skip for now")
                            .on_click(cx.listener(|this, _, _, cx| this.go(Step::Done, cx)))
                            .into_any_element(),
                    );
                }
                forward.push(
                    primary(theme, "onboarding-continue", "Continue", ready)
                        .when(ready, |el| {
                            el.on_click(cx.listener(|this, _, _, cx| this.go(Step::Done, cx)))
                        })
                        .into_any_element(),
                );
                forward
            }
            _ => vec![
                primary(theme, "onboarding-continue", "Continue", true)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let github = this.github_in_flow(cx);
                        this.go(this.step.next(github), cx)
                    }))
                    .into_any_element(),
            ],
        };
        footer_row(Some(back), forward)
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
            access_modes: Vec::new(),
            supports_plan: false,
        }
    }

    #[test]
    fn steps_walk_forward_and_back() {
        let walk = |github: bool| {
            let mut step = Step::Welcome;
            let mut seen = vec![step];
            while step != Step::Done {
                step = step.next(github);
                seen.push(step);
            }
            seen
        };
        use Step::*;
        // Signed out: no GitHub step, three numbered.
        assert_eq!(walk(false), [Welcome, Appearance, Sync, Providers, Done]);
        assert_eq!(
            [Welcome, Appearance, Sync, Providers, Done].map(|s| s.number(false)),
            [None, Some(1), Some(2), Some(3), None]
        );
        assert_eq!(Step::total(false), 3);
        assert_eq!(Providers.previous(false), Sync);
        // Signed in: GitHub after sync, four numbered.
        assert_eq!(
            walk(true),
            [Welcome, Appearance, Sync, Github, Providers, Done]
        );
        assert_eq!(
            [Appearance, Sync, Github, Providers].map(|s| s.number(true)),
            [Some(1), Some(2), Some(3), Some(4)]
        );
        assert_eq!(Step::total(true), 4);
        assert_eq!(Providers.previous(true), Github);
        assert_eq!(Github.previous(true), Sync);
        assert_eq!(Done.previous(true), Providers);
        assert_eq!(Appearance.previous(true), Welcome);
        assert_eq!(Step::parse("agents"), Providers);
        assert_eq!(Step::parse("github"), Github);
        assert_eq!(Step::parse("1"), Welcome);
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
