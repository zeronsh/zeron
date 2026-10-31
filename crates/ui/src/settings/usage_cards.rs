//! Local sidebar preferences below the device's provider settings.
use super::HarnessesPage;
use crate::{settings, settings::widgets, theme::Theme};
use gpui::{AnyElement, Context, div, prelude::*};

impl HarnessesPage {
    fn toggle_usage_cards(&mut self, cx: &mut Context<Self>) {
        settings::update(settings::SavePolicy::Immediate, cx, |settings| {
            settings.sidebar_show_usage_cards = !settings.sidebar_show_usage_cards;
        });
        cx.refresh_windows();
    }

    pub(super) fn render_usage_cards_settings(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = settings::current(cx).sidebar_show_usage_cards;
        let row = widgets::card_row(theme, true)
            .id("sidebar-usage-cards-toggle")
            .debug_selector(|| "sidebar-usage-cards-toggle".into())
            .role(gpui::Role::Switch)
            .aria_label("Show usage cards in sidebar")
            .aria_toggled(if enabled {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .tab_index(0)
            .cursor_pointer()
            .focus_visible(|style| style.border_2().border_color(theme.accent))
            .on_click(cx.listener(|page, _, _, cx| page.toggle_usage_cards(cx)))
            .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    page.toggle_usage_cards(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(theme, "Show usage cards in sidebar"))
                    .child(widgets::meta_line(
                        theme,
                        vec![
                            div()
                                .child("Keep provider plan usage visible above the sidebar footer.")
                                .into_any_element(),
                        ],
                    )),
            )
            .child(widgets::toggle_switch(
                theme,
                enabled,
                "sidebar-usage-cards",
            ));
        widgets::section(
            theme,
            "Usage cards",
            widgets::section_card(theme).mt_0().child(row),
        )
        .into_any_element()
    }
}
