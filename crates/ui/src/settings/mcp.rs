//! Settings > General: opt out of Zeron chat/visual tools without disabling computer use.

use gpui::{Context, Render, Window, div, prelude::*, px};
use zeron_proto::mcp_settings;

use super::search::{SearchAnchor, SettingId};
use super::widgets;
use crate::theme::Theme;

pub(super) struct McpSettingsCard {
    data_dir: Option<std::path::PathBuf>,
    enabled: bool,
    error: Option<String>,
}

impl McpSettingsCard {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data_dir = super::data_dir(cx);
        Self {
            enabled: mcp_settings::user_enabled(data_dir.as_deref()),
            data_dir,
            error: None,
        }
    }

    fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let result = self.data_dir.as_deref()
            .ok_or_else(|| std::io::Error::other("Settings folder unavailable"))
            .and_then(|dir| mcp_settings::set_user_enabled(dir, enabled));
        match result {
            Ok(()) => {
                self.enabled = enabled;
                self.error = None;
            }
            Err(_) => self.error = Some("Unable to save MCP preference. Check folder permissions.".into()),
        }
        cx.notify();
    }
}

impl Render for McpSettingsCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let enabled = self.enabled;
        let accent = theme.accent;
        let toggle = widgets::toggle_switch(&theme, enabled, "zeron-mcp-enabled")
            .id("zeron-mcp-enabled")
            .role(gpui::Role::Switch)
            .aria_label("Zeron MCP tools and instructions")
            .aria_toggled(if enabled { gpui::Toggled::True } else { gpui::Toggled::False })
            .tab_index(0)
            .focus_visible(move |style| style.border_2().border_color(accent))
            .on_click(cx.listener(move |this, _, _, cx| this.set_enabled(!enabled, cx)));
        let card = widgets::section_card(&theme).mt_0()
            .child(widgets::card_row(&theme, true).flex_wrap()
                .child(div().flex_1().min_w(px(180.0)).flex().flex_col().gap(px(4.0))
                    .child(widgets::row_title(&theme, "Zeron MCP tools and instructions"))
                    .child(div().text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child("Allow agents to manage chats, delegate through Zeron, and create visuals. Turning this off removes these tools and their prompt text.")))
                .child(toggle)
                .search_anchor(SettingId::ZeronMcpTools))
            .child(widgets::card_row(&theme, false)
                .child(div().text_size(crate::typography::ui_rems(12.0)).text_color(theme.text_muted)
                    .child("Computer use and your own MCP servers are unaffected. Applies to newly started agents; restart Zeron to refresh existing agents. Pi's native subagents remain available.")))
            .when_some(self.error.clone(), |card, error| card.child(
                widgets::card_row(&theme, false)
                    .child(div().text_color(theme.danger).child(error))));
        widgets::section(&theme, "Agent integration", card)
    }
}
