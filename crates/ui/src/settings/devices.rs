//! Settings → Devices (feature-inventory §1.5): the device registry — name,
//! platform, last-seen, presence dot, a "This device" badge, click-to-copy id,
//! and a Rename dialog (Mutate renameDevice).

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use std::time::Duration;

use zeron_proto::WorkspaceScope;
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::i18n::{self, Locale, MessageId, RelativeUnit};
use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// A device that pinged within this window shows a presence dot (engines
/// heartbeat every 15s; 70s tolerates a couple of missed beats).
pub const DEVICE_ONLINE_WINDOW_SECS: i64 = 70;

/// Presence: last-seen within the online window (future timestamps count). Pure.
pub fn device_online(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_seen
        .is_some_and(|at| now.signed_duration_since(at).num_seconds() <= DEVICE_ONLINE_WINDOW_SECS)
}

/// A language-neutral freshness bucket for a last-seen timestamp.
///
/// The sidebar keeps one of these as a change-detection key, so switching the
/// interface language can never leave a stale translated label in cached state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastSeen {
    Never,
    JustNow,
    Minutes(u64),
    Hours(u64),
    Days(u64),
}

impl LastSeen {
    /// Compact label, e.g. `5m ago` / `5 分钟前`.
    pub fn label(self, locale: Locale) -> String {
        match self {
            Self::Never => i18n::translate(MessageId::RelativeNeverSeen, locale).to_string(),
            Self::JustNow => i18n::translate(MessageId::RelativeJustNow, locale).to_string(),
            Self::Minutes(count) => i18n::relative_ago(count, RelativeUnit::Minutes, locale),
            Self::Hours(count) => i18n::relative_ago(count, RelativeUnit::Hours, locale),
            Self::Days(count) => i18n::relative_ago(count, RelativeUnit::Days, locale),
        }
    }
}

/// Bucket a last-seen timestamp. Pure.
pub fn last_seen_bucket(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> LastSeen {
    let Some(at) = last_seen else {
        return LastSeen::Never;
    };
    let secs = now.signed_duration_since(at).num_seconds();
    if secs < 60 {
        LastSeen::JustNow
    } else if secs < 3600 {
        LastSeen::Minutes((secs / 60) as u64)
    } else if secs < 86_400 {
        LastSeen::Hours((secs / 3600) as u64)
    } else {
        LastSeen::Days((secs / 86_400) as u64)
    }
}

/// Compact last-seen line. Pure.
pub fn format_last_seen(
    last_seen: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    locale: Locale,
) -> String {
    last_seen_bucket(last_seen, now).label(locale)
}

/// Scope-aware copy: a local registry describes only the active local
/// workspace and must not imply that account device metadata is already live.
pub fn devices_subtitle_message(scope: Option<WorkspaceScope>) -> MessageId {
    match scope {
        Some(WorkspaceScope::Local) => MessageId::DevicesSubtitleLocal,
        Some(WorkspaceScope::Synced) => MessageId::DevicesSubtitleSynced,
        Some(WorkspaceScope::Development) | None => MessageId::DevicesSubtitleDefault,
    }
}

struct RenameDialog {
    device_id: String,
    input: Entity<ComposerInput>,
    _events: Subscription,
}

pub struct DevicesPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    rename: Option<RenameDialog>,
    /// Device id whose id-chip shows "Copied" right now.
    copied: Option<String>,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    _observe: Subscription,
}

impl DevicesPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            scroll: widgets::PageScroll::default(),
            rename: None,
            copied: None,
            error: None,
            task: None,
            copy_task: None,
            _observe: observe,
        }
    }

    fn open_rename(&mut self, device_id: String, current: String, cx: &mut Context<Self>) {
        let locale = i18n::locale(cx);
        let input = cx.new(|cx| {
            ComposerInput::new(
                i18n::translate(MessageId::DevicesRenamePlaceholder, locale),
                cx,
            )
        });
        input.update(cx, |input, cx| input.set_text(current, cx));
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_rename(cx);
            }
        });
        self.rename = Some(RenameDialog {
            device_id,
            input,
            _events: events,
        });
        cx.notify();
    }

    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.rename.take() else {
            return;
        };
        let name = dialog.input.read(cx).text().trim().to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = serde_json::json!({
            "op": "renameDevice",
            "deviceId": dialog.device_id,
            "name": name,
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            this.update(cx, |page, cx| {
                if let Err(err) = result {
                    page.error = Some(
                        i18n::fill(
                            MessageId::DevicesRenameFailed,
                            "{err}",
                            &err.to_string(),
                            i18n::locale(cx),
                        )
                        .into(),
                    );
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn copy_id(&mut self, device_id: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(device_id.clone()));
        self.copied = Some(device_id);
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |page, cx| {
                page.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_rename_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let locale = i18n::locale(cx);
        let dialog = self.rename.as_ref()?;
        let input = dialog.input.clone();
        let card = popover::dialog_card(&theme)
            .child(popover::dialog_title(
                &theme,
                i18n::translate(MessageId::DevicesRenameTitle, locale),
            ))
            .child(
                div()
                    .mt(px(12.0))
                    .child(popover::dialog_field(input.into_any_element())),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(
                            &theme,
                            i18n::translate(MessageId::CommonCancel, locale),
                            "rename-cancel",
                        )
                        .id("rename-cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.rename = None;
                            cx.notify();
                        })),
                    )
                    .child(
                        popover::btn_primary(
                            &theme,
                            i18n::translate(MessageId::CommonRename, locale),
                        )
                        .id("rename-save")
                        .on_click(cx.listener(|this, _, _, cx| this.submit_rename(cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("rename-device-dialog", viewport, card))
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for DevicesPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

/// Human platform label (zeron settings.devices.tsx `platformLabel`).
pub fn platform_label(platform: &str) -> &str {
    match platform {
        "macos" | "darwin" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        "web" => "Web",
        "ios" => "iOS",
        "android" => "Android",
        other => other,
    }
}

/// Short device id for the click-to-copy chip (`abcd1234…wxyz`).
pub fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…{}", &id[..8], &id[id.len() - 4..])
    } else {
        id.to_string()
    }
}

impl Render for DevicesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let now = Utc::now();
        let locale = i18n::locale(cx);
        let (devices, local_id, workspace_scope) = {
            let state = self.state.read(cx);
            (
                state.devices.clone(),
                state.local_device_id.clone(),
                state.workspace_scope,
            )
        };
        let copied = self.copied.clone();
        let dialog = self.render_rename_dialog(window.viewport_size(), cx);
        let emerald = theme.success; // emerald-400
        let count = devices.len();

        let rows: Vec<AnyElement> = devices
            .into_iter()
            .enumerate()
            .map(|(ix, device)| {
                let online = device_online(device.last_seen_at, now);
                let is_local = local_id.as_deref() == Some(device.id.as_str());
                let id_copied = copied.as_deref() == Some(device.id.as_str());
                let copy_id = device.id.clone();
                let rename_id = device.id.clone();
                let rename_name = device.name.clone();
                let platform_icon = match device.platform.as_str() {
                    "macos" | "darwin" => crate::icons::LAPTOP,
                    "web" => crate::icons::GLOBAL,
                    "ios" | "android" => crate::icons::SMARTPHONE,
                    _ => crate::icons::MONITOR,
                };
                // Presence lives ON the identity tile: a corner dot (emerald
                // online with a soft glow, faint offline), ringed by the card
                // tone so it "cuts" the tile — zeron settings.devices.tsx
                // `border-2 border-[var(--card)]` +
                // `shadow-[0_0_6px_rgba(52,211,153,0.55)]`.
                let tile = widgets::row_tile(&theme, platform_icon).relative().child(
                    div()
                        .absolute()
                        .bottom(px(-3.0))
                        .right(px(-3.0))
                        .size(px(9.0))
                        .rounded_full()
                        .border_2()
                        .border_color(theme.surface)
                        .when(online, |el| {
                            el.bg(emerald).shadow(vec![gpui::BoxShadow {
                                color: emerald.opacity(0.55),
                                offset: gpui::point(px(0.0), px(0.0)),
                                blur_radius: px(6.0),
                                spread_radius: px(0.0),
                                inset: false,
                            }])
                        })
                        .when(!online, |el| el.bg(crate::theme::ink(0.22))),
                );
                // One quiet meta line: platform · version · (offline: last
                // seen) · id chip.
                let mut meta: Vec<AnyElement> = vec![
                    div()
                        .child(SharedString::from(
                            platform_label(&device.platform).to_string(),
                        ))
                        .into_any_element(),
                ];
                if let Some(version) = device.version.as_deref().filter(|v| !v.is_empty()) {
                    meta.push(
                        div()
                            .child(SharedString::from(format!("v{version}")))
                            .into_any_element(),
                    );
                }
                meta.push(
                    div()
                        .child(SharedString::from(i18n::fill(
                            MessageId::DevicesCursorSdk,
                            "{version}",
                            device.cursor_sdk_version.as_deref().unwrap_or_else(|| {
                                i18n::translate(MessageId::DevicesVersionUnknown, locale)
                            }),
                            locale,
                        )))
                        .into_any_element(),
                );
                if !online {
                    meta.push(
                        div()
                            .child(SharedString::from(i18n::fill(
                                MessageId::DevicesLastSeen,
                                "{time}",
                                &format_last_seen(device.last_seen_at, now, locale),
                                locale,
                            )))
                            .into_any_element(),
                    );
                }
                // "Added {time ago}" — always present (zeron settings.devices.tsx).
                if let Some(created) = device.created_at {
                    meta.push(
                        div()
                            .child(SharedString::from(i18n::fill(
                                MessageId::DevicesAdded,
                                "{time}",
                                &format_last_seen(Some(created), now, locale),
                                locale,
                            )))
                            .into_any_element(),
                    );
                }
                meta.push(
                    div()
                        .id(("device-id", ix))
                        .font_family(theme.font_mono.clone())
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(if id_copied {
                            theme.success_muted.opacity(0.9)
                        } else {
                            theme.text_muted.opacity(0.5)
                        })
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.text_muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.copy_id(copy_id.clone(), cx);
                        }))
                        .child(SharedString::from(if id_copied {
                            i18n::translate(MessageId::CommonCopied, i18n::locale(cx)).to_string()
                        } else {
                            short_id(&device.id)
                        }))
                        .into_any_element(),
                );

                widgets::card_row(&theme, ix == 0)
                    .child(tile)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, device.name.clone()))
                            .child(widgets::meta_line(&theme, meta)),
                    )
                    .when(is_local, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(px(10.5))
                                .text_color(theme.text_muted)
                                .child(if workspace_scope == Some(WorkspaceScope::Local) {
                                    i18n::translate(MessageId::SidebarLocalOnly, locale)
                                } else {
                                    i18n::translate(MessageId::PickerThisDevice, locale)
                                }),
                        )
                    })
                    .child(
                        // `opacity-70 hover:opacity-100` (zeron: also rises on
                        // row hover — gpui has no group-hover, so the button's
                        // own hover carries the reveal).
                        widgets::ghost_action(&theme)
                            .id(("device-rename", ix))
                            .opacity(0.7)
                            .hover(|s| {
                                s.opacity(1.0)
                                    .bg(crate::theme::ink(0.06))
                                    .text_color(theme.text)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_rename(rename_id.clone(), rename_name.clone(), cx);
                            }))
                            .child(
                                crate::icons::icon(crate::icons::PEN)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child(SharedString::from(i18n::translate(
                                MessageId::CommonRename,
                                locale,
                            ))),
                    )
                    .into_any_element()
            })
            .collect();

        let card = widgets::section_card(&theme);
        let card = if rows.is_empty() {
            card.child(
                div()
                    .px(px(20.0))
                    .py(px(40.0))
                    .text_center()
                    .text_size(crate::typography::ui_rems(14.0))
                    .text_color(theme.text_muted.opacity(0.6))
                    .child(SharedString::from(i18n::translate(
                        MessageId::DevicesEmpty,
                        locale,
                    ))),
            )
        } else {
            card.children(rows)
        };

        let scrollbar = popover::rail(self, "devices-page-scrollbar", &theme, cx);
        div()
            .id("devices-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("devices-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(widgets::page_header(
                                &theme,
                                i18n::translate(MessageId::SettingsSectionDevices, locale),
                                (count > 0).then_some(count),
                            ))
                            .child(widgets::page_subtitle(
                                &theme,
                                i18n::translate(devices_subtitle_message(workspace_scope), locale),
                            ))
                            .when_some(self.error.clone(), |el, message| {
                                el.child(
                                    widgets::error_strip(&theme, message)
                                        .id("devices-error")
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.error = None;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(card),
                    ),
            )
            .children(scrollbar)
            .when_some(dialog, |el, dialog| el.child(dialog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    #[test]
    fn presence_window() {
        let now = Utc::now();
        assert!(device_online(Some(now - TimeDelta::seconds(10)), now));
        assert!(device_online(Some(now - TimeDelta::seconds(70)), now));
        assert!(!device_online(Some(now - TimeDelta::seconds(71)), now));
        assert!(!device_online(None, now));
        // Clock skew (future) counts as online.
        assert!(device_online(Some(now + TimeDelta::seconds(30)), now));
    }

    #[test]
    fn last_seen_formatting() {
        let now = Utc::now();
        assert_eq!(format_last_seen(None, now, Locale::En), "never seen");
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::seconds(30)), now, Locale::En),
            "just now"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::minutes(5)), now, Locale::En),
            "5m ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::hours(3)), now, Locale::En),
            "3h ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::days(2)), now, Locale::En),
            "2d ago"
        );
    }

    /// The Chinese path, and the bucket the sidebar caches: the bucket carries
    /// no language, so a language switch cannot leave a stale label behind.
    #[test]
    fn last_seen_buckets_are_locale_neutral() {
        let now = Utc::now();
        assert_eq!(last_seen_bucket(None, now), LastSeen::Never);
        assert_eq!(
            last_seen_bucket(Some(now - TimeDelta::seconds(30)), now),
            LastSeen::JustNow
        );
        assert_eq!(
            last_seen_bucket(Some(now - TimeDelta::minutes(5)), now),
            LastSeen::Minutes(5)
        );
        assert_eq!(
            last_seen_bucket(Some(now - TimeDelta::hours(3)), now),
            LastSeen::Hours(3)
        );
        assert_eq!(
            last_seen_bucket(Some(now - TimeDelta::days(2)), now),
            LastSeen::Days(2)
        );

        assert_eq!(LastSeen::Never.label(Locale::ZhCn), "从未在线");
        assert_eq!(LastSeen::JustNow.label(Locale::ZhCn), "刚刚");
        assert_eq!(LastSeen::Minutes(5).label(Locale::ZhCn), "5 分钟前");
        assert_eq!(LastSeen::Hours(3).label(Locale::ZhCn), "3 小时前");
        assert_eq!(LastSeen::Days(2).label(Locale::ZhCn), "2 天前");
        assert_eq!(
            i18n::fill(
                MessageId::DevicesLastSeen,
                "{time}",
                &LastSeen::Minutes(5).label(Locale::ZhCn),
                Locale::ZhCn
            ),
            "上次在线 5 分钟前"
        );
    }

    #[test]
    fn local_subtitle_does_not_claim_synced_metadata() {
        let copy = i18n::translate(
            devices_subtitle_message(Some(WorkspaceScope::Local)),
            Locale::En,
        );
        assert!(copy.contains("local workspace"));
        assert!(!copy.contains("synced"));
    }
}
