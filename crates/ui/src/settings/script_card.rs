//! A settings card holding one Luau script: a code field, Apply, and Clear.
//! Apply checks the draft off the main thread the way its feature needs and
//! saves it only if it passes, so a broken script never reaches the view
//! that runs it; the error comes back with its line. The home visual and
//! the generative background each describe themselves with a
//! [`ScriptSpec`].

use gpui::{App, Context, Entity, SharedString, Subscription, Task, div, prelude::*, px};

use crate::composer::{CODE_INPUT_CONTEXT, ComposerInput, ComposerInputEvent};
use crate::popover;
use crate::settings::{UiSettings, widgets};
use crate::theme::Theme;

/// Saves a script that passed its check (on the main thread).
pub type Commit = Box<dyn FnOnce(&mut App) + Send>;

/// One script setting.
pub struct ScriptSpec {
    /// Element ids start with it.
    pub key: &'static str,
    pub title: &'static str,
    /// What the script is, while none is saved.
    pub empty_status: &'static str,
    /// The saved script, if any.
    pub saved: fn(&UiSettings) -> Option<String>,
    /// Where the saved script shows, or why it doesn't.
    pub saved_status: fn(&UiSettings) -> &'static str,
    /// Check a draft (blocking, off the main thread); on success, how to
    /// save it.
    pub check: fn(String) -> Result<Commit, String>,
    pub clear: fn(&mut App),
}

pub struct ScriptCard {
    spec: &'static ScriptSpec,
    input: Entity<ComposerInput>,
    error: Option<SharedString>,
    checking: Option<Task<()>>,
    _input_events: Subscription,
}

impl ScriptCard {
    pub fn new(spec: &'static ScriptSpec, cx: &mut Context<Self>) -> Self {
        let saved = (spec.saved)(&crate::settings::current(cx));
        let input = cx.new(|cx| {
            ComposerInput::with_context("-- Paste a Luau script", CODE_INPUT_CONTEXT, cx)
                .with_monospace()
                .with_text_metrics(12.0, 18.0)
                .with_viewport_height(240.0)
                .with_tab_stop()
        });
        if let Some(saved) = saved {
            input.update(cx, |input, cx| input.set_text(saved, cx));
        }
        let input_events = cx.subscribe(&input, |card: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                card.error = None;
                cx.notify();
            }
        });
        Self {
            spec,
            input,
            error: None,
            checking: None,
            _input_events: input_events,
        }
    }

    fn draft(&self, cx: &App) -> String {
        self.input.read(cx).text().trim().to_owned()
    }

    /// Check the draft off the main thread, then save it (an empty draft
    /// clears the setting).
    fn apply(&mut self, cx: &mut Context<Self>) {
        let script = self.draft(cx);
        if script.is_empty() {
            self.clear(cx);
            return;
        }
        let check = self.spec.check;
        let checked = cx.background_executor().spawn(async move { check(script) });
        self.error = None;
        self.checking = Some(cx.spawn(async move |this, cx| {
            let checked = checked.await;
            this.update(cx, |card, cx| {
                card.checking = None;
                match checked {
                    Ok(commit) => commit(cx),
                    Err(error) => card.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.checking = None;
        self.error = None;
        self.input.update(cx, |input, cx| input.set_text("", cx));
        (self.spec.clear)(cx);
        cx.notify();
    }
}

impl Render for ScriptCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let spec = self.spec;
        let settings = crate::settings::current(cx);
        let saved = (spec.saved)(&settings);
        let draft = self.draft(cx);
        let checking = self.checking.is_some();
        let applied = saved.as_deref().map(str::trim) == Some(draft.as_str())
            || (saved.is_none() && draft.is_empty());
        let status = if checking {
            "Checking the script…"
        } else if saved.is_some() && applied {
            (spec.saved_status)(&settings)
        } else if saved.is_some() {
            "Unapplied changes; the saved script is still in use."
        } else {
            spec.empty_status
        };
        let id = |part: &str| SharedString::from(format!("{}-{part}", spec.key));
        let actions = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .when(saved.is_some() || !draft.is_empty(), |row| {
                row.child(
                    widgets::text_action(&theme, widgets::ActionTone::Quiet, "Clear")
                        .id(id("clear"))
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label(format!("Remove the {}", spec.title.to_lowercase()))
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(|card, _, _, cx| card.clear(cx))),
                )
            })
            .child(
                widgets::text_action(
                    &theme,
                    widgets::ActionTone::Solid,
                    if checking { "Checking…" } else { "Apply" },
                )
                .id(id("apply"))
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label(format!("Check and apply the {}", spec.title.to_lowercase()))
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .when(applied || checking, |button| button.opacity(0.5))
                .when(!applied && !checking, |button| {
                    button.on_click(cx.listener(|card, _, _, cx| card.apply(cx)))
                }),
            );
        widgets::section_card(&theme)
            .mt_0()
            .child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .child(widgets::row_title(&theme, spec.title))
                            .child(widgets::meta_line(
                                &theme,
                                vec![div().child(status).into_any_element()],
                            )),
                    )
                    .child(actions),
            )
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(16.0))
                    .child(popover::dialog_field(self.input.clone().into_any_element()))
                    .when_some(self.error.clone(), |block, error| {
                        block.child(widgets::error_strip(&theme, error).mt(px(12.0)))
                    }),
            )
    }
}
