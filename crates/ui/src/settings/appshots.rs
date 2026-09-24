//! Appshots settings share the shortcut recorder and persistence events with
//! keyboard settings, while keeping capture setup on its own route.
use super::*;
use crate::appshots::{AppshotPlatform, CapabilityState};
use crate::i18n::{self, MessageId};
use crate::settings::widgets;

fn row(
    theme: &Theme,
    first: bool,
    title: &str,
    description: &str,
    control: gpui::AnyElement,
) -> gpui::Div {
    widgets::card_row(theme, first)
        .flex_wrap()
        .child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(widgets::row_title(theme, title.to_string()))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(px(18.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(description.to_string())),
                ),
        )
        .child(control)
}

impl ShortcutsPage {
    pub(super) fn render_appshots(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx).clone();
        let locale = i18n::locale(cx);
        let accent = theme.accent;
        let capabilities = self.appshot_capabilities;
        let toggle = |id: &'static str, label: &'static str, enabled: bool| {
            widgets::toggle_switch(&theme, enabled)
                .id(id)
                .role(gpui::Role::Switch)
                .aria_label(label)
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .tab_index(0)
                .focus_visible(move |style| style.border_2().border_color(accent))
        };
        let capture = toggle(
            "appshots-enabled",
            i18n::translate(MessageId::AppshotsCapture, locale),
            self.appshots_enabled,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.appshots_enabled = !this.appshots_enabled;
            this.commit_appshots(cx);
            cx.notify();
        }));
        let sound = toggle(
            "appshot-sound-enabled",
            i18n::translate(MessageId::AppshotsCaptureSound, locale),
            self.appshot_sound_enabled,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.appshot_sound_enabled = !this.appshot_sound_enabled;
            this.commit_appshots(cx);
            cx.notify();
        }));
        let destinations = AppshotDestination::ALL
            .into_iter()
            .enumerate()
            .map(|(ix, destination)| {
                let selected = self.appshot_destination == destination;
                div()
                    .id(("appshot-destination", ix))
                    .role(gpui::Role::Button)
                    .aria_label(i18n::translate(destination.label_message(), locale))
                    .aria_toggled(if selected {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tab_index(0)
                    .focus_visible(move |style| style.border_2().border_color(accent))
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(if selected {
                        theme.text.opacity(0.24)
                    } else {
                        theme.border
                    })
                    .bg(if selected {
                        crate::theme::ink(0.09)
                    } else {
                        gpui::transparent_black()
                    })
                    .text_size(px(11.0))
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.appshot_destination = destination;
                        this.commit_appshots(cx);
                        cx.notify();
                    }))
                    .child(i18n::translate(destination.label_message(), locale))
            })
            .collect::<Vec<_>>();
        let destination_description = match self.appshot_destination {
            AppshotDestination::Automatic => {
                i18n::translate(MessageId::AppshotsDestinationAutomaticDescription, locale)
            }
            AppshotDestination::LastSession => {
                i18n::translate(MessageId::AppshotsDestinationLastSessionDescription, locale)
            }
            AppshotDestination::NewSession => {
                i18n::translate(MessageId::AppshotsDestinationNewSessionDescription, locale)
            }
        };
        let shortcut = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(widgets::badge(
                &theme,
                i18n::translate(capabilities.global_shortcut.badge_message(), locale),
            ))
            .child(self.render_binding_control(
                ShortcutId::CaptureAppshot,
                0,
                self.recording,
                &theme,
                cx,
            ));
        let action = |id: &'static str, label: &'static str| {
            widgets::ghost_action(&theme)
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(move |style| style.border_2().border_color(accent))
                .cursor_pointer()
                .child(label)
        };
        let mut window_access = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(widgets::badge(
                &theme,
                i18n::translate(capabilities.window_capture.badge_message(), locale),
            ));
        if self.appshots_enabled
            && capabilities.window_capture == CapabilityState::PermissionRequired
        {
            let label = if self.capture_access_prompted {
                i18n::translate(MessageId::AppshotsOpenSystemSettings, locale)
            } else {
                i18n::translate(MessageId::AppshotsAllowWindowCapture, locale)
            };
            window_access = window_access.child(action("appshots-capture-access", label).on_click(
                cx.listener(|this, _, _, cx| {
                    if this.capture_access_prompted {
                        if let Some(url) = crate::appshots::capture_settings_url() {
                            cx.open_url(url);
                        }
                    } else {
                        this.capture_access_prompted = true;
                        crate::appshots::request_capture_access();
                    }
                    this.appshot_capabilities = crate::appshots::capabilities();
                    cx.notify();
                }),
            ));
        }
        let mut text_access = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(widgets::badge(
                &theme,
                i18n::translate(capabilities.application_text.badge_message(), locale),
            ));
        if self.appshots_enabled
            && capabilities.application_text == CapabilityState::PermissionRequired
            && crate::appshots::semantic_settings_url().is_some()
        {
            let label = if self.semantic_access_prompted {
                i18n::translate(MessageId::AppshotsOpenSystemSettings, locale)
            } else {
                i18n::translate(MessageId::AppshotsEnableTextCapture, locale)
            };
            text_access = text_access.child(action("appshots-semantic-access", label).on_click(
                cx.listener(|this, _, _, cx| {
                    if this.semantic_access_prompted {
                        if let Some(url) = crate::appshots::semantic_settings_url() {
                            cx.open_url(url);
                        }
                    } else {
                        this.semantic_access_prompted = true;
                        crate::appshots::request_semantic_access();
                    }
                    this.appshot_capabilities = crate::appshots::capabilities();
                    cx.notify();
                }),
            ));
        }
        let card = widgets::section_card(&theme)
            .child(row(
                &theme,
                true,
                i18n::translate(MessageId::AppshotsCapture, locale),
                i18n::translate(MessageId::AppshotsCaptureDescription, locale),
                capture.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                i18n::translate(MessageId::AppshotsCaptureSound, locale),
                i18n::translate(MessageId::AppshotsCaptureSoundDescription, locale),
                sound.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                if capabilities.platform == AppshotPlatform::LinuxWayland {
                    i18n::translate(MessageId::AppshotsPreferredShortcut, locale)
                } else {
                    i18n::translate(MessageId::AppshotsGlobalShortcut, locale)
                },
                i18n::translate(capabilities.shortcut_message(), locale),
                shortcut.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                i18n::translate(MessageId::AppshotsDestination, locale),
                destination_description,
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(destinations)
                    .into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                i18n::translate(MessageId::AppshotsWindowCapture, locale),
                i18n::translate(capabilities.capture_message(), locale),
                window_access.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                i18n::translate(MessageId::AppshotsApplicationText, locale),
                i18n::translate(capabilities.semantic_message(), locale),
                text_access.into_any_element(),
            ));
        let helper: SharedString = if self.recording.is_some() {
            i18n::translate(MessageId::CommonPressEscapeToCancel, locale).into()
        } else {
            self.conflict_notice.clone().unwrap_or_default()
        };
        let refresh = action(
            "appshots-refresh-permissions",
            i18n::translate(MessageId::AppshotsCheckAgain, locale),
        )
        .aria_label(i18n::translate(
            MessageId::AppshotsCheckPermissionsAgain,
            locale,
        ))
        .on_click(cx.listener(|this, _, _, cx| {
            this.appshot_capabilities = crate::appshots::capabilities();
            cx.notify();
        }));
        let scrollbar = self.render_scrollbar(&theme, cx);
        div()
            .id("appshots-settings-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .on_drag_move(cx.listener(Self::on_bar_drag_move))
            .child(
                div()
                    .id("appshots-settings-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .track_focus(&self.focus)
                    .tab_group()
                    .tab_index(0)
                    .tab_stop(false)
                    .on_key_down(|event, window, cx| {
                        let key = &event.keystroke;
                        if key.key == "tab"
                            && !key.modifiers.control
                            && !key.modifiers.alt
                            && !key.modifiers.platform
                        {
                            if key.modifiers.shift {
                                window.focus_prev(cx);
                            } else {
                                window.focus_next(cx);
                            }
                            cx.stop_propagation();
                        }
                    })
                    .child(
                        widgets::page_column()
                            .child(widgets::page_header(
                                &theme,
                                i18n::translate(MessageId::SettingsSectionAppshots, locale),
                                None,
                            ))
                            .child(
                                widgets::page_subtitle(
                                    &theme,
                                    i18n::translate(capabilities.setup_message(), locale),
                                )
                                .line_height(px(20.0)),
                            )
                            .child(card)
                            .child(
                                div()
                                    .min_h(px(20.0))
                                    .mt(px(8.0))
                                    .text_size(px(12.0))
                                    .text_color(theme.text_muted)
                                    .child(helper),
                            )
                            .child(
                                div()
                                    .mt(px(12.0))
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(12.0))
                                            .text_color(theme.text_muted)
                                            .child(i18n::translate(
                                                MessageId::AppshotsPermissionRefreshHint,
                                                locale,
                                            )),
                                    )
                                    .child(refresh),
                            ),
                    ),
            )
            .children(scrollbar)
            .into_any_element()
    }
}
