//! Settings → Notifications: independently configurable session chimes plus
//! desktop banners on the same status transitions (`notification_service`).
//!
//! The ShortcutsPage arrangement: the page holds a working copy, every flip
//! emits [`NotificationsEvent::Changed`], and the shell persists it. Nothing
//! here talks RPC — all preferences are device-local UI settings.

use gpui::{Context, EventEmitter, Window, div, prelude::*, px};

use crate::popover;
use crate::settings::widgets;
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub enum NotificationsEvent {
    /// A toggle flipped — persist the complete notification preference set.
    Changed {
        sound: bool,
        completion_sound: bool,
        input_sound: bool,
        attention_sound: bool,
        desktop: bool,
        background_only: bool,
    },
}

pub struct NotificationsPage {
    scroll: widgets::PageScroll,
    sound: bool,
    completion_sound: bool,
    input_sound: bool,
    attention_sound: bool,
    desktop: bool,
    background_only: bool,
}

impl EventEmitter<NotificationsEvent> for NotificationsPage {}

#[derive(Clone, Copy)]
enum NotificationPreference {
    Sound,
    CompletionSound,
    InputSound,
    AttentionSound,
    Desktop,
    BackgroundOnly,
}

fn is_switch_activation(key: &str, is_held: bool) -> bool {
    !is_held && matches!(key, "enter" | "space")
}

fn interactive_switch(
    element: gpui::Stateful<gpui::Div>,
    accent: gpui::Hsla,
    preference: NotificationPreference,
    cx: &mut Context<NotificationsPage>,
) -> gpui::Stateful<gpui::Div> {
    element
        .tab_index(0)
        .focus_visible(move |style| style.border_2().border_color(accent))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle(preference, cx);
        }))
        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
            if is_switch_activation(&event.keystroke.key, event.is_held) {
                cx.stop_propagation();
                this.toggle(preference, cx);
            }
        }))
}

impl NotificationsPage {
    fn sync_preferences(&mut self, cx: &gpui::App) {
        if cx.has_global::<crate::app_runtime::AppRuntime>() {
            let settings = crate::settings::current(cx);
            self.sound = settings.sound_enabled;
            self.completion_sound = settings.sound_completion_enabled;
            self.input_sound = settings.sound_input_enabled;
            self.attention_sound = settings.sound_attention_enabled;
            self.desktop = settings.notifications_enabled;
            self.background_only = settings.notifications_background_only;
        }
    }

    pub fn new(
        sound: bool,
        completion_sound: bool,
        input_sound: bool,
        attention_sound: bool,
        desktop: bool,
        background_only: bool,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            scroll: widgets::PageScroll::default(),
            sound,
            completion_sound,
            input_sound,
            attention_sound,
            desktop,
            background_only,
        }
    }

    fn emit(&self, cx: &mut Context<Self>) {
        cx.emit(NotificationsEvent::Changed {
            sound: self.sound,
            completion_sound: self.completion_sound,
            input_sound: self.input_sound,
            attention_sound: self.attention_sound,
            desktop: self.desktop,
            background_only: self.background_only,
        });
    }

    fn toggle(&mut self, preference: NotificationPreference, cx: &mut Context<Self>) {
        self.sync_preferences(cx);
        let value = match preference {
            NotificationPreference::Sound => &mut self.sound,
            NotificationPreference::CompletionSound => &mut self.completion_sound,
            NotificationPreference::InputSound => &mut self.input_sound,
            NotificationPreference::AttentionSound => &mut self.attention_sound,
            NotificationPreference::Desktop => &mut self.desktop,
            NotificationPreference::BackgroundOnly => &mut self.background_only,
        };
        *value = !*value;
        self.emit(cx);
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for NotificationsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for NotificationsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_preferences(cx);
        let theme = Theme::of(cx).for_settings_surface();
        let accent = theme.accent;
        let sound = self.sound;
        let completion_sound = self.completion_sound;
        let input_sound = self.input_sound;
        let attention_sound = self.attention_sound;
        let desktop = self.desktop;
        let background_only = self.background_only;
        let toggle = |id: &'static str, label: &'static str, enabled: bool, interactive: bool| {
            // Keep the visual inside its 56×40 activation target.
            // Disabled subordinate controls remain named switches in the
            // accessibility tree, but have no focus or input handlers.
            div()
                .id(id)
                .flex_none()
                .w(px(widgets::SWITCH_WIDTH))
                .h(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .role(gpui::Role::Switch)
                .aria_label(label)
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .when(!interactive, |el| {
                    el.aria_description("Unavailable while its parent setting is off")
                })
                .child(widgets::toggle_switch(&theme, enabled, id))
        };
        let card = widgets::section_card(&theme)
            .mt_0()
            .child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Session sounds")),
                    )
                    .child(interactive_switch(
                        toggle("notifications-sound-toggle", "Session sounds", sound, true),
                        accent,
                        NotificationPreference::Sound,
                        cx,
                    )),
            )
            .child(
                widgets::card_row(&theme, false)
                    .when(!sound, |el| el.opacity(0.55))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Task completed")),
                    )
                    .child(
                        toggle(
                            "notifications-completion-sound-toggle",
                            "Task completed sound",
                            completion_sound,
                            sound,
                        )
                        .when(sound, |el| {
                            interactive_switch(
                                el,
                                accent,
                                NotificationPreference::CompletionSound,
                                cx,
                            )
                        }),
                    ),
            )
            .child(
                widgets::card_row(&theme, false)
                    .when(!sound, |el| el.opacity(0.55))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Input required")),
                    )
                    .child(
                        toggle(
                            "notifications-input-sound-toggle",
                            "Input required sound",
                            input_sound,
                            sound,
                        )
                        .when(sound, |el| {
                            interactive_switch(el, accent, NotificationPreference::InputSound, cx)
                        }),
                    ),
            )
            .child(
                widgets::card_row(&theme, false)
                    .when(!sound, |el| el.opacity(0.55))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Errors and disconnections")),
                    )
                    .child(
                        toggle(
                            "notifications-attention-sound-toggle",
                            "Errors and disconnections sound",
                            attention_sound,
                            sound,
                        )
                        .when(sound, |el| {
                            interactive_switch(
                                el,
                                accent,
                                NotificationPreference::AttentionSound,
                                cx,
                            )
                        }),
                    ),
            );
        let desktop_card = widgets::section_card(&theme)
            .mt_0()
            .child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Desktop notifications")),
                    )
                    .child(interactive_switch(
                        toggle(
                            "notifications-desktop-toggle",
                            "Desktop notifications",
                            desktop,
                            true,
                        ),
                        accent,
                        NotificationPreference::Desktop,
                        cx,
                    )),
            )
            .child(
                // Sub-option of the banner row: dimmed + inert while banners
                // are off (the harnesses not-installed treatment).
                widgets::card_row(&theme, false)
                    .when(!desktop, |el| el.opacity(0.55))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Only when in the background")),
                    )
                    .child(
                        toggle(
                            "notifications-background-toggle",
                            "Only notify when Zeron is in the background",
                            background_only,
                            desktop,
                        )
                        .when(desktop, |el| {
                            interactive_switch(
                                el,
                                accent,
                                NotificationPreference::BackgroundOnly,
                                cx,
                            )
                        }),
                    ),
            );

        let scrollbar = popover::rail(self, "notifications-page-scrollbar", &theme, cx);
        div()
            .id("notifications-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("notifications-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "Notifications", None))
                                .child(
                                    widgets::section(&theme, "Desktop", desktop_card).mt(px(24.0)),
                                )
                                .child(widgets::section(&theme, "Sounds", card)),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::is_switch_activation;

    #[test]
    fn switches_accept_enter_or_space_once_per_press() {
        assert!(is_switch_activation("enter", false));
        assert!(is_switch_activation("space", false));
        assert!(!is_switch_activation("escape", false));
        assert!(!is_switch_activation("space", true));
    }
}
