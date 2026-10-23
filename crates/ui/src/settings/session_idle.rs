//! Settings → General → Idle sessions: how long a parked session keeps its
//! agent process warm after its last turn before the engine releases it.

use gpui::{AnyElement, Context, Entity, Subscription, Task, div, prelude::*, px};
use zeron_engine::registry::{SESSION_IDLE_CHOICES, SessionSettings};
use zeron_rpc::methods;

use crate::popover::Loadable;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

pub struct SessionIdleCard {
    state: Entity<AppState>,
    settings: Loadable<SessionSettings>,
    select: widgets::SelectState,
    error: Option<String>,
    task: Option<Task<()>>,
    _state: Subscription,
}

fn label(minutes: Option<u32>) -> String {
    match minutes {
        None => "Never".into(),
        Some(m) if m % (24 * 60) == 0 => match m / (24 * 60) {
            1 => "1 day".into(),
            days => format!("{days} days"),
        },
        Some(m) if m % 60 == 0 => format!("{} h", m / 60),
        Some(m) => format!("{m} min"),
    }
}

impl SessionIdleCard {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Settings can open before the engine connects: load once it does.
        let state_sub = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.settings, Loadable::Idle) {
                this.call(None, cx);
            }
        });
        let mut card = Self {
            state,
            settings: Loadable::Idle,
            select: widgets::SelectState::default(),
            error: None,
            task: None,
            _state: state_sub,
        };
        card.call(None, cx);
        card
    }

    /// `GetSessionSettings`, or `SetSessionSettings` when saving; either
    /// reply is the device's authoritative settings.
    fn call(&mut self, save: Option<SessionSettings>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (method, params) = match save {
            Some(settings) => (
                methods::SET_SESSION_SETTINGS,
                serde_json::to_value(settings).unwrap_or_default(),
            ),
            None => (methods::GET_SESSION_SETTINGS, serde_json::json!({})),
        };
        if matches!(self.settings, Loadable::Idle) {
            self.settings = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<SessionSettings>(v).map_err(|e| e.to_string())
                });
            this.update(cx, |card, cx| {
                match result {
                    Ok(settings) => {
                        card.settings = Loadable::Ready(settings);
                        card.error = None;
                    }
                    Err(error) if matches!(card.settings, Loadable::Ready(_)) => {
                        card.error = Some(error);
                    }
                    Err(error) => card.settings = Loadable::Error(error),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for SessionIdleCard {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::toast::take_error(&mut self.error, cx);
        let theme = Theme::of(cx).for_settings_surface();
        let control: AnyElement = match &self.settings {
            Loadable::Ready(settings) => {
                // A hand-edited window outside the offered ones still shows.
                let mut choices = SESSION_IDLE_CHOICES.to_vec();
                if !choices.contains(&settings.idle_minutes) {
                    choices.insert(0, settings.idle_minutes);
                }
                let selected = choices
                    .iter()
                    .position(|choice| *choice == settings.idle_minutes)
                    .unwrap_or_default();
                widgets::select(
                    "session-idle",
                    "Close idle sessions after",
                    &theme,
                    |card: &mut Self| &mut card.select,
                )
                .options(
                    choices
                        .iter()
                        .map(|minutes| widgets::SelectOption::new(label(*minutes))),
                    selected,
                )
                .width(120.0)
                .on_select(move |card, ix, _, cx| {
                    if ix != selected {
                        card.call(
                            Some(SessionSettings {
                                idle_minutes: choices[ix],
                            }),
                            cx,
                        );
                    }
                })
                .render(&self.select, cx)
                .into_any_element()
            }
            Loadable::Error(error) => div()
                .text_color(theme.text_muted)
                .child(error.clone())
                .into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        let description = if self
            .settings
            .ready()
            .is_some_and(|settings| settings.idle_minutes.is_none())
        {
            "Agents stay running between turns until the session is archived."
        } else {
            "An agent nobody returns to is stopped; the next message resumes it."
        };
        widgets::section_card(&theme).child(
            widgets::card_row(&theme, true)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(&theme, "Close idle sessions"))
                        .child(widgets::meta_line(
                            &theme,
                            vec![div().child(description).into_any_element()],
                        )),
                )
                .child(control),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_choices_read_as_durations() {
        let labels: Vec<_> = SESSION_IDLE_CHOICES.into_iter().map(label).collect();
        assert_eq!(
            labels,
            ["30 min", "1 h", "3 h", "6 h", "12 h", "1 day", "Never"]
        );
    }
}
