//! Grok call menu: voice, personality, speed, and microphone, opened from the
//! call bar. Speed and voice wait until the current reply finishes; the
//! microphone switches as soon as the choice is saved.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Entity, FocusHandle, MouseButton, Render, Subscription, Window, div, prelude::*,
    px,
};
use zeron_proto::voice::VoiceProvider;

use crate::icons::{self, icon};
use crate::popover::{self, Popup};
use crate::settings::{self, widgets};
use crate::theme::Theme;

use super::VoiceController;

pub(crate) struct GrokCallMenu {
    menu: Popup<()>,
    voice_select: widgets::SelectState,
    personality_select: widgets::SelectState,
    speed_focus: FocusHandle,
    speed_bounds: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    speed_drag: Option<f32>,
    devices: Vec<zeron_voice::InputDevice>,
    default_input: Option<String>,
    inputs_at: Option<Instant>,
    inputs_task: Option<gpui::Task<()>>,
    _voice_watch: Subscription,
    _settings_watch: Subscription,
}

impl GrokCallMenu {
    pub(crate) fn new(voice: Entity<VoiceController>, cx: &mut Context<Self>) -> Self {
        let watch = cx.observe(&voice, |this, voice, cx| {
            let voice = voice.read(cx);
            let live = voice.provider() == Some(VoiceProvider::Grok) && voice.is_live();
            if live {
                return;
            }
            if let Some(speed) = this.speed_drag.take() {
                save_speed(speed, cx);
            }
            if this.menu.get().is_some() {
                this.menu = Popup::default();
                cx.notify();
            }
        });
        let settings_watch = cx.observe_global::<settings::SettingsStore>(|_, cx| cx.notify());
        Self {
            menu: Popup::default(),
            voice_select: widgets::SelectState::default(),
            personality_select: widgets::SelectState::default(),
            speed_focus: cx.focus_handle(),
            speed_bounds: Rc::new(Cell::new(None)),
            speed_drag: None,
            devices: Vec::new(),
            default_input: None,
            inputs_at: None,
            inputs_task: None,
            _voice_watch: watch,
            _settings_watch: settings_watch,
        }
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(speed) = self.speed_drag.take() {
            save_speed(speed, cx);
        }
        if self.menu.begin_close() {
            popover::reap_popup(cx, |menu: &mut Self| &mut menu.menu);
        }
        cx.notify();
    }

    fn drag_speed(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.speed_bounds.get() else {
            return;
        };
        self.speed_drag = Some(widgets::grok_speed_at_x(bounds, x));
        cx.notify();
    }

    fn ensure_inputs(&mut self, cx: &mut Context<Self>) {
        if !self.menu.is_open()
            || self.inputs_task.is_some()
            || self
                .inputs_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(3))
        {
            return;
        }
        let scan = cx.background_executor().spawn(async {
            (
                zeron_voice::input_devices(),
                zeron_voice::default_input_device(),
            )
        });
        self.inputs_task = Some(cx.spawn(async move |this, cx| {
            let (devices, default) = scan.await;
            this.update(cx, |this, cx| {
                this.devices = devices;
                this.default_input = default;
                this.inputs_at = Some(Instant::now());
                cx.notify();
            })
            .ok();
            cx.background_executor().timer(Duration::from_secs(3)).await;
            this.update(cx, |this, cx| {
                this.inputs_task = None;
                this.inputs_at = None;
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for GrokCallMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(cx);
        let theme = Theme::of(cx).for_popup();
        let saved = settings::current(cx);
        let voice = zeron_grok_voice::voice_label(saved.grok_voice.as_deref());
        let personality = saved.grok_personality.label();
        let open = self.menu.is_open();
        let pill = div()
            .id("grok-call-style")
            .role(gpui::Role::Button)
            .tab_index(0)
            .aria_label(format!("Voice settings, {voice}, {personality}"))
            .aria_expanded(open)
            .h(px(32.0))
            .px(px(12.0))
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .border_1()
            .border_color(theme.border)
            .bg(if open {
                theme.element_hover
            } else {
                theme.glass_hover().opacity(0.35)
            })
            .hover(|style| style.bg(theme.glass_hover()))
            .focus_visible(|style| style.border_2().border_color(theme.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.menu.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.menu.take_press_was_open() || this.menu.is_open() {
                    this.close_menu(cx);
                    return;
                }
                this.menu.open(());
                cx.notify();
            }))
            .child(
                div()
                    .max_w(px(180.0))
                    .truncate()
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(format!("{voice} · {personality}")),
            )
            .child(
                icon(icons::ALT_ARROW_UP)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            );

        let menu = self.menu.get().is_some().then(|| {
            popover::anchored_menu_above(
                "grok-call-menu",
                self.menu_card(&theme, cx),
                self.menu.closing_since(),
            )
        });

        div().relative().child(pill).children(menu)
    }
}

impl GrokCallMenu {
    fn menu_card(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let saved = settings::current(cx);
        let shown = self
            .speed_drag
            .unwrap_or_else(|| zeron_grok_voice::snap_speed(saved.grok_voice_speed));
        let voice = self.voice_picker(theme, &saved, cx);
        let personality = self.personality_picker(theme, &saved, cx);
        let speed = self.speed_row(theme, shown, cx);
        let mics = self.microphones(theme, saved.grok_input.as_deref(), cx);
        div()
            .w(px(280.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(12.0))
            .rounded(px(popover::CARD_RADIUS))
            .border_1()
            .border_color(theme.border)
            .bg(popover::surface_bg(theme))
            .when(!theme.is_frost(), |card| card.shadow_lg())
            .text_color(theme.text)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                // A nested voice or personality menu extends outside this card.
                // Let that menu handle the press; the next outside press closes.
                if this.voice_select.is_open() || this.personality_select.is_open() {
                    return;
                }
                this.close_menu(cx);
            }))
            .on_drag_move(
                cx.listener(|this, event: &gpui::DragMoveEvent<CallSpeedDrag>, _, cx| {
                    this.drag_speed(event.event.position.x, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if let Some(speed) = this.speed_drag.take() {
                        save_speed(speed, cx);
                        cx.notify();
                    }
                }),
            )
            .child(field_label(theme, "Voice"))
            .child(voice)
            .child(field_label(theme, "Personality"))
            .child(personality)
            .child(field_label(theme, "Speed"))
            .child(speed)
            .child(field_label(theme, "Microphone"))
            .child(mics)
            .into_any_element()
    }

    fn voice_picker(
        &mut self,
        theme: &Theme,
        saved: &settings::UiSettings,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let current = saved.grok_voice.clone();
        let mut ids: Vec<Option<String>> = std::iter::once(None)
            .chain(
                zeron_grok_voice::VOICES
                    .iter()
                    .map(|(id, _)| Some((*id).to_owned())),
            )
            .collect();
        if current.as_deref().is_some_and(|id| {
            !zeron_grok_voice::VOICES
                .iter()
                .any(|(known, _)| *known == id)
        }) {
            ids.push(current.clone());
        }
        let selected = ids.iter().position(|id| id == &current).unwrap_or(0);
        let options: Vec<_> = ids
            .iter()
            .map(|id| widgets::SelectOption::new(zeron_grok_voice::voice_label(id.as_deref())))
            .collect();
        widgets::select("grok-call-voice", "Voice", theme, |menu: &mut Self| {
            &mut menu.voice_select
        })
        .options(options, selected)
        .width(256.0)
        .on_select(move |_, ix, _, cx| {
            let voice = ids.get(ix).cloned().flatten();
            settings::update(settings::SavePolicy::Immediate, cx, |settings| {
                settings.grok_voice = voice;
            });
            cx.notify();
        })
        .render(&self.voice_select, cx)
    }

    fn personality_picker(
        &mut self,
        theme: &Theme,
        saved: &settings::UiSettings,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let choices = settings::GrokPersonality::ALL;
        let current = saved.grok_personality;
        let selected = choices
            .iter()
            .position(|choice| *choice == current)
            .unwrap_or(0);
        let options: Vec<_> = choices
            .iter()
            .map(|choice| widgets::SelectOption::new(choice.label()))
            .collect();
        widgets::select(
            "grok-call-personality",
            "Personality",
            theme,
            |menu: &mut Self| &mut menu.personality_select,
        )
        .options(options, selected)
        .width(256.0)
        .on_select(move |_, ix, _, cx| {
            let Some(choice) = choices.get(ix).copied() else {
                return;
            };
            settings::update(settings::SavePolicy::Immediate, cx, |settings| {
                settings.grok_personality = choice;
            });
            cx.notify();
        })
        .render(&self.personality_select, cx)
    }

    fn speed_row(&mut self, theme: &Theme, shown: f32, cx: &mut Context<Self>) -> gpui::Div {
        let fraction = zeron_grok_voice::speed_fraction(shown);
        let label = zeron_grok_voice::speed_label(shown);
        let slot = Rc::clone(&self.speed_bounds);
        let focus = self.speed_focus.clone();
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id("grok-call-speed")
                    .flex_1()
                    .track_focus(&focus)
                    .tab_index(0)
                    .role(gpui::Role::Slider)
                    .aria_label("Speed")
                    .aria_value(label.clone())
                    .aria_numeric_value(f64::from(shown))
                    .aria_min_numeric_value(f64::from(zeron_grok_voice::SPEED_MIN))
                    .aria_max_numeric_value(f64::from(zeron_grok_voice::SPEED_MAX))
                    .aria_numeric_value_step(f64::from(zeron_grok_voice::SPEED_STEP))
                    .focus_visible(|style| style.border_2().border_color(theme.accent))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                            window.focus(&this.speed_focus, cx);
                            this.drag_speed(event.position.x, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_drag(CallSpeedDrag, |drag, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| drag.clone())
                    })
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                        let current = this.speed_drag.unwrap_or_else(|| {
                            zeron_grok_voice::snap_speed(settings::current(cx).grok_voice_speed)
                        });
                        let next = match event.keystroke.key.as_str() {
                            "left" | "down" => current - zeron_grok_voice::SPEED_STEP,
                            "right" | "up" => current + zeron_grok_voice::SPEED_STEP,
                            "home" => zeron_grok_voice::SPEED_MIN,
                            "end" => zeron_grok_voice::SPEED_MAX,
                            _ => return,
                        };
                        this.speed_drag = None;
                        save_speed(next, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(
                        gpui::canvas(
                            move |measured, _, _| slot.set(Some(measured)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(widgets::grok_speed_track(theme, fraction)),
            )
            .child(
                div()
                    .w(px(48.0))
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(label),
            )
    }

    fn microphones(
        &mut self,
        theme: &Theme,
        saved: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if self.devices.is_empty() {
            return div()
                .text_size(crate::typography::ui_rems(12.5))
                .text_color(theme.text_muted)
                .child(if self.inputs_task.is_some() {
                    "Looking for microphones"
                } else {
                    "No microphone found"
                });
        }
        let selected = saved
            .filter(|id| self.devices.iter().any(|device| device.id == *id))
            .map(str::to_owned)
            .or_else(|| self.default_input.clone());
        let devices = self.devices.clone();
        let mut list = div().flex().flex_col().gap(px(6.0));
        for (ix, device) in devices.iter().enumerate() {
            let id = device.id.clone();
            let name = device.name.clone();
            let on = selected.as_deref() == Some(device.id.as_str());
            let row_id: gpui::SharedString = format!("grok-call-mic-{ix}").into();
            list = list.child(
                div()
                    .id(row_id)
                    .role(gpui::Role::Button)
                    .tab_index(0)
                    .h(px(36.0))
                    .px(px(10.0))
                    .rounded(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(if on { theme.text } else { theme.border })
                    .hover(|style| style.bg(theme.glass_hover().opacity(0.4)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        let chosen = id.clone();
                        settings::update(settings::SavePolicy::Immediate, cx, |settings| {
                            settings.grok_input = Some(chosen);
                        });
                        cx.notify();
                    }))
                    .child(
                        icon(icons::MICROPHONE)
                            .size(px(14.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(13.0))
                            .child(name),
                    ),
            );
        }
        if let Some(saved) = saved
            && !self.devices.iter().any(|device| device.id == saved)
        {
            list = list.child(
                div()
                    .h(px(36.0))
                    .px(px(10.0))
                    .rounded(px(10.0))
                    .flex()
                    .items_center()
                    .border_1()
                    .border_color(theme.text)
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.text_muted)
                    .child("Disconnected microphone"),
            );
        }
        list
    }
}

#[derive(Clone)]
struct CallSpeedDrag;

impl Render for CallSpeedDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

fn field_label(theme: &Theme, text: &'static str) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(13.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text)
        .child(text)
}

fn save_speed(speed: f32, cx: &mut App) {
    let speed = zeron_grok_voice::snap_speed(speed);
    settings::update(settings::SavePolicy::Immediate, cx, |settings| {
        settings.grok_voice_speed = speed;
    });
}
