//! Composer completion preferences in Settings → Shortcuts.

use super::ShortcutsPage;
use crate::{
    i18n::{self, MessageId},
    popover::Loadable,
    settings,
    settings::widgets,
    theme::Theme,
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::HarnessId;

/// Use the same installed/enabled gate as the composer, in settings order.
fn active_agents(list: &[HarnessDescriptor]) -> Vec<(HarnessId, &'static str)> {
    let offered = crate::pickers::offered_harnesses(list);
    settings::SKILL_COMPLETION_HARNESSES
        .into_iter()
        .filter(|(id, _)| offered.iter().any(|agent| agent.id == *id))
        .collect()
}

impl ShortcutsPage {
    pub(crate) fn load_completion_harnesses(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.completion_harnesses = Loadable::Error(
                i18n::translate(MessageId::CompletionConnectDevice, i18n::locale(cx)).to_string(),
            );
            return;
        };
        self.completion_harnesses = Loadable::Loading;
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(zeron_rpc::methods::LIST_HARNESSES, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.completion_harnesses = match result {
                    Ok(value) => match serde_json::from_value(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(error) => Loadable::Error(error.to_string()),
                    },
                    Err(error) => Loadable::Error(error.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle_completion(&mut self, harness: HarnessId, dollar: bool, cx: &mut Context<Self>) {
        settings::update(settings::SavePolicy::Immediate, cx, |settings| {
            let mut preferences = settings.skill_completion(harness);
            if dollar {
                preferences.dollar = !preferences.dollar;
            } else {
                preferences.separate_from_slash = !preferences.separate_from_slash;
            }
            settings
                .skill_completion_by_harness
                .insert(harness, preferences);
        });
        cx.notify();
    }

    fn reset_completion(&mut self, cx: &mut Context<Self>) {
        settings::update(settings::SavePolicy::Immediate, cx, |settings| {
            settings.skill_completion_by_harness.clear();
            settings.skills_in_slash_menu = false;
        });
        cx.notify();
    }

    pub(super) fn render_completion(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let locale = i18n::locale(cx);
        let current = settings::current(cx);
        let customized =
            !current.skill_completion_by_harness.is_empty() || current.skills_in_slash_menu;
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .flex_wrap()
            .gap(px(12.0))
            .child(widgets::field_label(
                theme,
                i18n::translate(MessageId::CompletionTitle, locale),
            ))
            .when(customized, |header| {
                header.child(
                    widgets::ghost_action(theme)
                        .id("reset-completion")
                        .role(gpui::Role::Button)
                        .aria_label(i18n::translate(MessageId::CompletionResetAria, locale))
                        .tab_index(0)
                        .border_1()
                        .border_color(gpui::transparent_black())
                        .focus_visible(|s| s.border_color(theme.accent))
                        .on_click(cx.listener(|page, _, _, cx| page.reset_completion(cx)))
                        .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                            if !event.is_held
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            {
                                cx.stop_propagation();
                                page.reset_completion(cx);
                            }
                        }))
                        .child(i18n::translate(MessageId::ShortcutsRestoreDefaults, locale)),
                )
            });
        let mut section = div().mt(px(28.0)).flex().flex_col().gap(px(12.0)).child(
            div().flex().flex_col().gap(px(4.0)).child(header).child(
                widgets::page_subtitle(
                    theme,
                    i18n::translate(MessageId::CompletionSubtitle, locale),
                )
                .mt(px(0.0))
                .line_height(px(20.0)),
            ),
        );
        match &self.completion_harnesses {
            Loadable::Idle | Loadable::Loading => {
                section = section.child(widgets::page_subtitle(
                    theme,
                    i18n::translate(MessageId::CompletionLoading, locale),
                ));
            }
            Loadable::Error(_) => {
                section = section.child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(12.0))
                        .child(widgets::page_subtitle(
                            theme,
                            i18n::translate(MessageId::CompletionLoadFailed, locale),
                        ))
                        .child(
                            widgets::ghost_action(theme)
                                .id("retry-completion-agents")
                                .role(gpui::Role::Button)
                                .aria_label(i18n::translate(MessageId::CompletionRetryAria, locale))
                                .tab_index(0)
                                .border_1()
                                .border_color(gpui::transparent_black())
                                .focus_visible(|s| s.border_color(theme.accent))
                                .on_click(
                                    cx.listener(|page, _, _, cx| {
                                        page.load_completion_harnesses(cx)
                                    }),
                                )
                                .on_key_down(cx.listener(
                                    |page, event: &gpui::KeyDownEvent, _, cx| {
                                        if !event.is_held
                                            && matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | "space"
                                            )
                                        {
                                            cx.stop_propagation();
                                            page.load_completion_harnesses(cx);
                                        }
                                    },
                                ))
                                .child(i18n::translate(MessageId::CommonRetry, locale)),
                        ),
                );
            }
            Loadable::Ready(list) => {
                let agents = active_agents(list);
                if agents.is_empty() {
                    section = section.child(widgets::page_subtitle(
                        theme,
                        i18n::translate(MessageId::CompletionNoAgents, locale),
                    ));
                }
                for (harness, name) in agents {
                    let preferences = current.skill_completion(harness);
                    let (logo, tint) = crate::pickers::harness_brand_icon(harness);
                    let mut card = widgets::section_card(theme).mt(px(0.0)).child(
                        div()
                            .px(px(20.0))
                            .pt(px(16.0))
                            .pb(px(4.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                crate::icons::icon(logo)
                                    .size(px(18.0))
                                    .flex_none()
                                    .text_color(tint.unwrap_or(theme.text)),
                            )
                            .child(widgets::row_title(theme, name)),
                    );
                    for (dollar, label, description, enabled) in [
                        (
                            true,
                            MessageId::CompletionUseDollar,
                            MessageId::CompletionUseDollarHint,
                            preferences.dollar,
                        ),
                        (
                            false,
                            MessageId::CompletionSeparateCommands,
                            MessageId::CompletionSeparateCommandsHint,
                            preferences.separate_from_slash,
                        ),
                    ]
                    .into_iter()
                    {
                        let label = i18n::translate(label, locale);
                        let description = i18n::translate(description, locale);
                        let state = i18n::translate(
                            if enabled {
                                MessageId::CommonOn
                            } else {
                                MessageId::CommonOff
                            },
                            locale,
                        );
                        card = card.child(
                            widgets::card_row(theme, true)
                                .id(SharedString::from(format!(
                                    "completion-{harness:?}-{dollar}"
                                )))
                                .role(gpui::Role::Switch)
                                .aria_label(i18n::fill_many(
                                    MessageId::CompletionToggleAria,
                                    &[("{name}", name), ("{label}", label), ("{state}", state)],
                                    locale,
                                ))
                                .tab_index(0)
                                .cursor_pointer()
                                .border_1()
                                .border_color(gpui::transparent_black())
                                .focus_visible(|s| s.border_color(theme.accent))
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    page.toggle_completion(harness, dollar, cx)
                                }))
                                .on_key_down(cx.listener(
                                    move |page, event: &gpui::KeyDownEvent, _, cx| {
                                        if !event.is_held
                                            && matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | "space"
                                            )
                                        {
                                            cx.stop_propagation();
                                            page.toggle_completion(harness, dollar, cx);
                                        }
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(widgets::row_title(theme, label))
                                        .child(
                                            div()
                                                .text_size(crate::typography::ui_rems(
                                                    widgets::ROW_DESCRIPTION_SIZE,
                                                ))
                                                .line_height(px(18.0))
                                                .text_color(theme.text_muted)
                                                .child(description),
                                        ),
                                )
                                .child(widgets::toggle_switch(theme, enabled).flex_none()),
                        );
                    }
                    section = section.child(card);
                }
            }
        }
        section.into_any_element()
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    fn descriptor(id: HarnessId, installed: bool, enabled: Option<bool>) -> HarnessDescriptor {
        HarnessDescriptor {
            id,
            name: format!("{id:?}"),
            supports_steering: false,
            steering_mode: zeron_proto::SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed,
            can_install: false,
            enabled,
        }
    }

    #[test]
    fn completion_only_lists_installed_enabled_agents_in_settings_order() {
        let list = [
            descriptor(HarnessId::Opencode, true, Some(true)),
            descriptor(HarnessId::Cursor, true, Some(false)),
            descriptor(HarnessId::Devin, false, Some(true)),
            descriptor(HarnessId::Codex, true, Some(true)),
            descriptor(HarnessId::ClaudeCode, true, None),
            descriptor(HarnessId::Grok, false, None),
            descriptor(HarnessId::Mock, true, Some(true)),
        ];
        assert_eq!(
            active_agents(&list),
            vec![
                (HarnessId::ClaudeCode, "Claude Code"),
                (HarnessId::Codex, "Codex"),
                (HarnessId::Opencode, "OpenCode"),
            ]
        );
        assert!(active_agents(&[]).is_empty());
        assert!(active_agents(&[descriptor(HarnessId::Codex, true, Some(false))]).is_empty());
    }

    #[gpui::test]
    fn completion_preferences_save_locally_and_independently(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(Default::default(), dir.path(), cx));
        let state = cx.new(|_| crate::state::AppState::new());
        let page = cx.new(|cx| {
            ShortcutsPage::new(
                state,
                Default::default(),
                false,
                Default::default(),
                false,
                false,
                crate::appshots::AppshotDestination::Automatic,
                cx,
            )
        });
        page.update(cx, |page, cx| {
            page.toggle_completion(HarnessId::ClaudeCode, true, cx);
            page.toggle_completion(HarnessId::ClaudeCode, false, cx);
            page.toggle_completion(HarnessId::Opencode, true, cx);
        });
        let loaded = settings::UiSettings::load(dir.path());
        let claude = loaded.skill_completion(HarnessId::ClaudeCode);
        assert!(claude.dollar && claude.separate_from_slash);
        let opencode = loaded.skill_completion(HarnessId::Opencode);
        assert!(opencode.dollar && !opencode.separate_from_slash);
        assert!(!loaded.skill_completion(HarnessId::Cursor).dollar);
        page.update(cx, |page, cx| page.reset_completion(cx));
        let reset = settings::UiSettings::load(dir.path());
        assert!(reset.skill_completion_by_harness.is_empty());
        assert!(reset.skill_completion(HarnessId::Codex).dollar);
        assert!(!reset.skill_completion(HarnessId::ClaudeCode).dollar);
    }
}
