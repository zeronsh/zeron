//! Settings → Archived (feature-inventory §1.5): archived chats across
//! devices, with Unarchive (Mutate setChatArchived false).

use gpui::{
    AnyElement, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};

use zeron_proto::Chat;
use zeron_rpc::methods;

use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Archived rows in sidebar (recency) order. Pure. Side chats stay listed:
/// once archived and closed, this is the only place to restore them.
pub fn archived_chats(chats: &[Chat]) -> Vec<&Chat> {
    chats.iter().filter(|c| c.archived).collect()
}

const ARCHIVE_PAGE_SIZE: usize = 40;

fn archive_page(total: usize, requested: usize) -> usize {
    requested.min(total.saturating_sub(1) / ARCHIVE_PAGE_SIZE)
}

pub struct ArchivedPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    error: Option<SharedString>,
    /// Chat with an in-flight unarchive (button shows working state).
    busy: Option<String>,
    page: usize,
    task: Option<Task<()>>,
    _observe: Subscription,
}

impl ArchivedPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            scroll: widgets::PageScroll::default(),
            error: None,
            busy: None,
            page: 0,
            task: None,
            _observe: observe,
        }
    }

    fn unarchive(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.busy = Some(chat_id.clone());
        self.error = None;
        let params = serde_json::json!({
            "op": "setChatArchived",
            "chatId": chat_id,
            "archived": false,
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            this.update(cx, |page, cx| {
                page.busy = None;
                if let Err(err) = result {
                    page.error = Some(format!("Unarchive failed: {err}").into());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for ArchivedPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for ArchivedPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let now = chrono::Utc::now();
        let (rows, device_names, count): (
            Vec<Chat>,
            std::collections::HashMap<String, String>,
            usize,
        ) = {
            let state = self.state.read(cx);
            let archived = archived_chats(&state.chats);
            let count = archived.len();
            let page = archive_page(count, self.page);
            if self.page != page {
                self.page = page;
                self.scroll.reset();
            }
            let rows = archived
                .into_iter()
                .skip(self.page * 40)
                .take(40)
                .cloned()
                .collect();
            let names = state
                .devices
                .iter()
                .map(|d| (d.id.clone(), d.name.clone()))
                .collect();
            (rows, names, count)
        };
        let busy = self.busy.clone();
        let page = self.page;
        let items: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, chat)| {
                let title: SharedString = chat
                    .title
                    .clone()
                    .unwrap_or_else(|| "Untitled session".into())
                    .into();
                // Unknown device → no fragment at all (zeron renders the
                // device span only when the name resolves).
                let device: Option<SharedString> =
                    device_names.get(&chat.device_id).cloned().map(Into::into);
                let time_ago: SharedString = crate::state::format_time_ago(
                    chat.last_message_at.unwrap_or(chat.created_at),
                    now,
                )
                .into();
                let location: Option<SharedString> =
                    crate::state::chat_location(&chat).map(Into::into);
                let is_busy = busy.as_deref() == Some(chat.id.as_str());
                let chat_id = chat.id.clone();
                // zeron settings.archived.tsx row: archive tile, medium title
                // + tabular time, quiet device · location meta, Unarchive.
                div()
                    .id(("archived-row", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .rounded(px(8.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .hover(|s| s.bg(crate::theme::ink(0.03)))
                    .child(
                        div()
                            .flex_none()
                            .size(px(32.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                crate::icons::icon(crate::icons::ARCHIVE_MINIMALISTIC)
                                    .size(px(16.0))
                                    .text_color(theme.text_muted.opacity(0.6)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(crate::typography::ui_rems(13.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(crate::typography::ui_rems(11.0))
                                            .text_color(theme.text_muted)
                                            .child(time_ago),
                                    ),
                            )
                            .child({
                                // device · location, separator at the line's
                                // own tone (zeron: a plain span inheriting
                                // `text-muted-foreground/55`).
                                let mut meta = div()
                                    .mt(px(2.0))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted);
                                let both = device.is_some() && location.is_some();
                                if let Some(device) = device {
                                    meta = meta.child(device);
                                }
                                if both {
                                    meta = meta.child(SharedString::from("·"));
                                }
                                if let Some(location) = location {
                                    meta = meta.child(div().min_w_0().truncate().child(location));
                                }
                                meta
                            }),
                    )
                    .child(
                        // Keep the action visible without list-wide hover invalidation.
                        div()
                            .id(("unarchive", ix))
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .opacity(0.8)
                            .when(is_busy, |el| el.opacity(0.4))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_raised).text_color(theme.text))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.unarchive(chat_id.clone(), cx);
                            }))
                            .child(
                                crate::icons::icon(crate::icons::ARCHIVE_UP_MINIMALISTIC)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child(SharedString::from(if is_busy {
                                "Unarchiving…"
                            } else {
                                "Unarchive"
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let body: AnyElement = if items.is_empty() {
            // Centered empty state (zeron settings.archived.tsx).
            div()
                .mt(px(96.0))
                .flex()
                .flex_col()
                .items_center()
                .text_center()
                .text_color(theme.text_muted)
                .child(
                    // `opacity-40` on top of the inherited muted/50 — an
                    // effectively ~20% glyph (zeron settings.archived.tsx).
                    crate::icons::icon(crate::icons::ARCHIVE_MINIMALISTIC)
                        .size(px(28.0))
                        .text_color(theme.text_muted.opacity(0.2)),
                )
                .child(
                    div()
                        .mt(px(12.0))
                        .text_size(crate::typography::ui_rems(14.0))
                        .child(SharedString::from("Nothing archived")),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(
                            "Right-click a session in the sidebar to archive it.",
                        )),
                )
                .into_any_element()
        } else {
            div()
                .mt(px(24.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .children(items)
                .into_any_element()
        };

        let pagination = (count > ARCHIVE_PAGE_SIZE).then(|| {
            div()
                .mt(px(16.0))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    widgets::ghost_action(&theme)
                        .id("archived-previous")
                        .role(gpui::Role::Button)
                        .aria_label("Previous archived sessions")
                        .tab_index(0)
                        .opacity(if page > 0 { 1.0 } else { 0.4 })
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.page = this.page.saturating_sub(1);
                            this.scroll.reset();
                            cx.notify();
                        }))
                        .child("Previous"),
                )
                .child(format!(
                    "{}–{} of {}",
                    page * ARCHIVE_PAGE_SIZE + 1,
                    ((page + 1) * ARCHIVE_PAGE_SIZE).min(count),
                    count
                ))
                .child(
                    widgets::ghost_action(&theme)
                        .id("archived-next")
                        .role(gpui::Role::Button)
                        .aria_label("Next archived sessions")
                        .tab_index(0)
                        .opacity(if (page + 1) * ARCHIVE_PAGE_SIZE < count {
                            1.0
                        } else {
                            0.4
                        })
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.page = archive_page(count, this.page + 1);
                            this.scroll.reset();
                            cx.notify();
                        }))
                        .child("Next"),
                )
        });
        let scrollbar = popover::rail(self, "archived-page-scrollbar", &theme, cx);
        div()
            .id("archived-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("archived-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(
                                    &theme,
                                    "Archived sessions",
                                    (count > 0).then_some(count),
                                ))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    "Hidden from the sidebar until restored.",
                                ))
                                .when_some(self.error.clone(), |el, message| {
                                    el.child(
                                        widgets::error_strip(&theme, message)
                                            .id("archived-error")
                                            .cursor_pointer()
                                            .tab_index(0)
                                            .role(gpui::Role::Button)
                                            .focus_visible(|s| {
                                                s.border_2().border_color(theme.accent).opacity(1.0)
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.error = None;
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(body)
                                .children(pagination),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn chat(id: &str, archived: bool) -> Chat {
        Chat {
            id: id.into(),
            device_id: "d".into(),
            title: None,
            archived,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: None,
            created_at: Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            parent_chat_id: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    #[test]
    fn archive_page_clamps_after_removal_and_handles_exact_boundaries() {
        assert_eq!(archive_page(0, 9), 0);
        assert_eq!(archive_page(40, 1), 0);
        assert_eq!(archive_page(41, 1), 1);
        assert_eq!(archive_page(80, 2), 1);
        assert_eq!(archive_page(81, 2), 2);
    }

    #[test]
    fn only_archived_rows_show() {
        let chats = vec![chat("a", false), chat("b", true), chat("c", true)];
        let rows = archived_chats(&chats);
        let ids: Vec<&str> = rows.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
    }

    #[test]
    fn archived_side_chats_stay_restorable() {
        let mut side = chat("side", true);
        side.parent_chat_id = Some("main".into());
        let chats = vec![chat("main", false), side];
        let ids: Vec<&str> = archived_chats(&chats)
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["side"]);
    }
}
