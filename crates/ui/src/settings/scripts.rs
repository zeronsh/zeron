use std::collections::HashSet;

use gpui::{AnyElement, Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};

use zeron_proto::scripts::ScriptInfo;
use zeron_rpc::methods;

use crate::icons;
use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

pub struct ScriptsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    open_errors: HashSet<String>,
    _observe: Subscription,
}

fn count_label(count: usize, noun: &str) -> SharedString {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
    .into()
}

fn script_meta(theme: &Theme, script: &ScriptInfo) -> Vec<AnyElement> {
    if script.error.is_some() {
        return vec![
            div()
                .text_color(theme.danger_muted)
                .child("Failed to load")
                .into_any_element(),
        ];
    }
    let mut meta = Vec::new();
    if !script.commands.is_empty() {
        meta.push(
            div()
                .child(count_label(script.commands.len(), "command"))
                .into_any_element(),
        );
    }
    if !script.tools.is_empty() {
        meta.push(
            div()
                .child(count_label(script.tools.len(), "tool"))
                .into_any_element(),
        );
    }
    if meta.is_empty() {
        meta.push(div().child("No commands").into_any_element());
    }
    meta
}

fn script_row(
    theme: &Theme,
    ix: usize,
    script: &ScriptInfo,
    cx: &Context<ScriptsPage>,
) -> AnyElement {
    let enabled = script.enabled;
    let name = SharedString::from(script.name.clone());
    let click_name = name.clone();
    let key_name = name.clone();
    let error_name = name.clone();
    widgets::card_row(theme, ix == 0)
        .child(widgets::row_tile(theme, icons::TERMINAL))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(!enabled, |el| el.opacity(0.55))
                .child(widgets::row_title(theme, name.clone()))
                .child(widgets::meta_line(theme, script_meta(theme, script))),
        )
        .when(script.error.is_some(), |row| {
            row.child(
                div()
                    .id(("script-error", ix))
                    .flex_none()
                    .cursor_pointer()
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .aria_label("Error details")
                    .rounded_full()
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .child(
                        icons::icon(icons::INFO_CIRCLE)
                            .size(px(16.0))
                            .text_color(theme.danger),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_error(&error_name, cx);
                    })),
            )
        })
        .child(
            widgets::toggle_switch(theme, enabled, format!("script-switch-{name}"))
                .id(("script-toggle", ix))
                .cursor_pointer()
                .tab_index(0)
                .role(gpui::Role::Switch)
                .aria_label(name)
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_enabled(&click_name, !enabled, cx);
                }))
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                    if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.set_enabled(&key_name, !enabled, cx);
                        cx.stop_propagation();
                    }
                })),
        )
        .into_any_element()
}

fn error_details(theme: &Theme, error: &str) -> AnyElement {
    div()
        .mx(px(16.0))
        .mb(px(12.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(widgets::block_fill(theme))
        .text_size(crate::typography::ui_rems(12.0))
        .line_height(crate::typography::ui_rems(18.0))
        .text_color(theme.danger_muted)
        .whitespace_normal()
        .child(SharedString::from(error.to_owned()))
        .into_any_element()
}

impl ScriptsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            scroll: widgets::PageScroll::default(),
            open_errors: HashSet::new(),
            _observe: observe,
        }
    }

    fn toggle_error(&mut self, script: &str, cx: &mut Context<Self>) {
        if !self.open_errors.remove(script) {
            self.open_errors.insert(script.to_owned());
        }
        cx.notify();
    }

    fn call(&mut self, method: &'static str, params: serde_json::Value, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            crate::toast::error(cx, "Engine not connected");
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Err(err) = engine.client().call(method, params).await {
                this.update(cx, |_, cx| crate::toast::error(cx, err.to_string()))
                    .ok();
            }
        })
        .detach();
    }

    fn set_enabled(&mut self, script: &str, enabled: bool, cx: &mut Context<Self>) {
        self.call(
            methods::SET_SCRIPT_ENABLED,
            serde_json::json!({ "script": script, "enabled": enabled }),
            cx,
        );
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.call(methods::RELOAD_SCRIPTS, serde_json::json!({}), cx);
    }

    fn open_folder(&self, cx: &mut Context<Self>) {
        let folder = &self.state.read(cx).scripts.folder;
        if !folder.is_empty() {
            cx.reveal_path(std::path::Path::new(folder));
        }
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for ScriptsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for ScriptsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let (count, folder, card) = {
            let snapshot = &self.state.read(cx).scripts;
            let card = if snapshot.scripts.is_empty() {
                widgets::section_card(&theme).child(
                    widgets::card_row(&theme, true).child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .text_color(theme.text_muted)
                            .child("No scripts yet"),
                    ),
                )
            } else {
                widgets::section_card(&theme).children(
                    snapshot
                        .scripts
                        .iter()
                        .enumerate()
                        .flat_map(|(ix, script)| {
                            let details = script
                                .error
                                .as_deref()
                                .filter(|_| self.open_errors.contains(&script.name))
                                .map(|error| error_details(&theme, error));
                            std::iter::once(script_row(&theme, ix, script, cx)).chain(details)
                        }),
                )
            };
            (
                snapshot.scripts.len(),
                SharedString::from(snapshot.folder.clone()),
                card,
            )
        };
        let action = |id: &'static str, label: &'static str| {
            widgets::text_action(&theme, widgets::ActionTone::Filled, label)
                .id(id)
                .tab_index(0)
                .role(gpui::Role::Button)
                .focus_visible(|s| s.border_2().border_color(theme.accent))
        };
        let reload = action("scripts-reload", "Reload")
            .on_click(cx.listener(|this, _, _, cx| this.reload(cx)));
        let open_folder = action("scripts-open-folder", "Open folder")
            .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx)));

        let scrollbar = popover::rail(self, "scripts-page-scrollbar", &theme, cx);
        div()
            .id("scripts-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("scripts-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .child(widgets::page_header(&theme, "Scripts", Some(count)))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .child(reload)
                                                .child(open_folder),
                                        ),
                                )
                                .when(!folder.is_empty(), |el| {
                                    el.child(widgets::page_subtitle(&theme, folder))
                                })
                                .child(card),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}
