//! Device membership and runtime metadata in Workspace settings.

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use std::time::Duration;

use zeron_proto::WorkspaceScope;
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
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

/// Compact last-seen line. Pure.
pub fn format_last_seen(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(at) = last_seen else {
        return "never seen".to_string();
    };
    let secs = now.signed_duration_since(at).num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Scope-aware copy: a local registry describes only the active local
/// workspace and must not imply that account device metadata is already live.
pub fn devices_subtitle(scope: Option<WorkspaceScope>) -> &'static str {
    match scope {
        Some(WorkspaceScope::Local) => "Manage device details stored in this local workspace.",
        Some(WorkspaceScope::Synced) => "Manage device names and inspect synced device metadata.",
        Some(WorkspaceScope::Private) => {
            "Registered agent servers. Manage paired devices on the workspace host."
        }
        Some(WorkspaceScope::Development) | None => "Manage device names for this workspace.",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum NodeRole {
    Client,
    Server,
}

impl NodeRole {
    pub(super) fn wire(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Client => "Client",
            Self::Server => "Agent server",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PairedDevice {
    pub device_id: String,
    pub name: String,
    pub role: NodeRole,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub paired_at: DateTime<Utc>,
}

struct WorkspaceDevice {
    device: zeron_proto::Device,
    registered: bool,
    paired: bool,
}

fn workspace_devices(
    registered: &[zeron_proto::Device],
    paired: Option<&[PairedDevice]>,
) -> Vec<WorkspaceDevice> {
    match paired {
        Some(nodes) => nodes
            .iter()
            .map(|node| {
                let existing = registered.iter().find(|device| device.id == node.device_id);
                let mut device = existing.cloned().unwrap_or_else(|| zeron_proto::Device {
                    id: node.device_id.clone(),
                    name: node.name.clone(),
                    platform: String::new(),
                    role: None,
                    last_seen_at: None,
                    created_at: Some(node.paired_at),
                    version: None,
                    cursor_sdk_version: None,
                    capabilities: Vec::new(),
                });
                device.role = Some(node.role.wire().into());
                WorkspaceDevice {
                    device,
                    registered: existing.is_some(),
                    paired: true,
                }
            })
            .collect(),
        None => registered
            .iter()
            .cloned()
            .map(|device| WorkspaceDevice {
                device,
                registered: true,
                paired: false,
            })
            .collect(),
    }
}

pub(super) enum DeviceEvent {
    Revoke(String),
}
impl gpui::EventEmitter<DeviceEvent> for DevicesSection {}

struct RenameDialog {
    device_id: String,
    input: Entity<ComposerInput>,
    _events: Subscription,
}

pub struct DevicesSection {
    state: Entity<AppState>,
    paired: Option<Vec<PairedDevice>>,
    revoke: Option<String>,
    access_busy: bool,
    rename: Option<RenameDialog>,
    /// Device id whose id-chip shows "Copied" right now.
    copied: Option<String>,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    _observe: Subscription,
}

impl DevicesSection {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            paired: None,
            revoke: None,
            access_busy: false,
            rename: None,
            copied: None,
            error: None,
            task: None,
            copy_task: None,
            _observe: observe,
        }
    }

    fn open_rename(&mut self, device_id: String, current: String, cx: &mut Context<Self>) {
        let input = cx.new(|cx| ComposerInput::new("Device name", cx));
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
                    page.error = Some(format!("Rename failed: {err}").into());
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

    pub(super) fn render_rename_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let dialog = self.rename.as_ref()?;
        let input = dialog.input.clone();
        let card = popover::dialog_card(&theme)
            .child(popover::dialog_title(&theme, "Rename device"))
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
                        popover::btn_ghost(&theme, "Cancel", "rename-cancel")
                            .id("rename-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rename = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(&theme, "Rename")
                            .id("rename-save")
                            .on_click(cx.listener(|this, _, _, cx| this.submit_rename(cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("rename-device-dialog", viewport, card))
    }

    pub(super) fn set_membership(
        &mut self,
        nodes: Option<Vec<PairedDevice>>,
        busy: bool,
        cx: &mut Context<Self>,
    ) {
        if self.paired != nodes || self.access_busy != busy {
            self.paired = nodes;
            self.access_busy = busy;
            cx.notify();
        }
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

impl Render for DevicesSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let now = Utc::now();
        let (devices, local_id, workspace_scope) = {
            let state = self.state.read(cx);
            (
                state.devices.clone(),
                state.local_device_id.clone(),
                state.workspace_scope,
            )
        };
        let devices = workspace_devices(&devices, self.paired.as_deref());
        let copied = self.copied.clone();
        let emerald = theme.success; // emerald-400
        let count = devices.len();

        let rows: Vec<AnyElement> = devices
            .into_iter()
            .enumerate()
            .map(|(ix, entry)| {
                let device = entry.device;
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
                    "" if device.role.as_deref() == Some("client") => crate::icons::SMARTPHONE,
                    _ => crate::icons::MONITOR,
                };
                // Presence lives ON the identity tile: a corner dot (emerald
                // online with a soft glow, faint offline), ringed by the card
                // tone so it "cuts" the tile — zeron settings.devices.tsx
                // `border-2 border-[var(--card)]` +
                // `shadow-[0_0_6px_rgba(52,211,153,0.55)]`.
                let tile = widgets::row_tile(&theme, platform_icon).relative().when(
                    device.last_seen_at.is_some(),
                    |el| {
                        el.child(
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
                        )
                    },
                );
                // One quiet meta line: platform · version · (offline: last
                // seen) · id chip.
                let mut meta: Vec<AnyElement> = Vec::new();
                if let Some(role) = device.role.as_deref() {
                    meta.push(
                        div()
                            .child(match role {
                                "client" => "Client",
                                "server" => "Agent server",
                                _ => "Device",
                            })
                            .into_any_element(),
                    );
                }
                if !device.platform.is_empty() {
                    meta.push(
                        div()
                            .child(SharedString::from(
                                platform_label(&device.platform).to_string(),
                            ))
                            .into_any_element(),
                    );
                }
                if entry.paired && device.last_seen_at.is_none() {
                    meta.push(div().child("Paired").into_any_element());
                }
                if let Some(version) = device.version.as_deref().filter(|v| !v.is_empty()) {
                    meta.push(
                        div()
                            .child(SharedString::from(format!("v{version}")))
                            .into_any_element(),
                    );
                }
                meta.push(
                    div()
                        .child(SharedString::from(format!(
                            "Cursor SDK {}",
                            device
                                .cursor_sdk_version
                                .as_deref()
                                .unwrap_or("unknown (older engine)")
                        )))
                        .into_any_element(),
                );
                if !online && device.last_seen_at.is_some() {
                    meta.push(
                        div()
                            .child(SharedString::from(format!(
                                "Last seen {}",
                                format_last_seen(device.last_seen_at, now)
                            )))
                            .into_any_element(),
                    );
                }
                // "Added {time ago}" — always present (zeron settings.devices.tsx).
                if !entry.paired
                    && let Some(created) = device.created_at
                {
                    meta.push(
                        div()
                            .child(SharedString::from(format!(
                                "Added {}",
                                format_last_seen(Some(created), now)
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
                            "Copied".to_string()
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
                                    "Local only"
                                } else {
                                    "This device"
                                }),
                        )
                    })
                    .when(entry.registered, |el| {
                        el.child(
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
                                .child(SharedString::from("Rename")),
                        )
                    })
                    .when(entry.paired && !is_local, |el| {
                        let id = device.id.clone();
                        let confirm = self.revoke.as_ref() == Some(&id);
                        el.child(
                            widgets::ghost_action(&theme)
                                .id(("device-revoke", ix))
                                .text_color(theme.danger)
                                .opacity(if self.access_busy { 0.45 } else { 1.0 })
                                .hover(|el| el.bg(theme.danger.opacity(0.08)))
                                .child(if confirm { "Confirm revoke" } else { "Revoke" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.access_busy {
                                        return;
                                    }
                                    if this.revoke.as_ref() == Some(&id) {
                                        this.revoke = None;
                                        cx.emit(DeviceEvent::Revoke(id.clone()));
                                    } else {
                                        this.revoke = Some(id.clone());
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .into_any_element()
            })
            .collect();

        let card = widgets::section_card(&theme).mt(px(12.0));
        let card = if rows.is_empty() {
            card.child(
                div()
                    .px(px(20.0))
                    .py(px(40.0))
                    .text_center()
                    .text_size(crate::typography::ui_rems(14.0))
                    .text_color(theme.text_muted.opacity(0.6))
                    .child(SharedString::from("No devices registered")),
            )
        } else {
            card.children(rows)
        };

        div()
            .mt(px(24.0))
            .child(widgets::page_header(&theme, "Devices", Some(count)))
            .child(widgets::page_subtitle(
                &theme,
                if self.paired.is_some() {
                    "All paired devices. Agent servers run agents; clients control them."
                } else {
                    devices_subtitle(workspace_scope)
                },
            ))
            .when_some(self.error.clone(), |el, message| {
                el.child(widgets::error_strip(&theme, message))
            })
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn registered_server() -> zeron_proto::Device {
        zeron_proto::Device {
            id: "server".into(),
            name: "Renamed workstation".into(),
            platform: "linux".into(),
            role: Some("server".into()),
            last_seen_at: Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            created_at: None,
            version: Some("0.2.71".into()),
            cursor_sdk_version: Some("1.0.31".into()),
            capabilities: Vec::new(),
        }
    }

    #[test]
    fn private_devices_include_clients_and_preserve_server_metadata() {
        let server = registered_server();
        let nodes = vec![
            PairedDevice {
                device_id: "server".into(),
                name: "Old name".into(),
                role: NodeRole::Server,
                paired_at: server.last_seen_at.unwrap(),
            },
            PairedDevice {
                device_id: "client".into(),
                name: "Mobile".into(),
                role: NodeRole::Client,
                paired_at: server.last_seen_at.unwrap(),
            },
        ];
        let rows = workspace_devices(&[server], Some(&nodes));
        assert_eq!(
            rows.iter()
                .map(|row| row.device.id.as_str())
                .collect::<Vec<_>>(),
            ["server", "client"]
        );
        assert_eq!(rows[0].device.name, "Renamed workstation");
        assert_eq!(rows[0].device.version.as_deref(), Some("0.2.71"));
        assert_eq!(rows[0].device.cursor_sdk_version.as_deref(), Some("1.0.31"));
        assert_eq!(rows[0].device.platform, "linux");
        assert!(rows[0].registered && rows[0].paired);
        assert_eq!(rows[1].device.name, "Mobile");
        assert_eq!(rows[1].device.role.as_deref(), Some("client"));
        assert_eq!(rows[1].device.last_seen_at, None);
        assert_eq!(rows[1].device.cursor_sdk_version, None);
        assert!(!rows[1].registered && rows[1].paired);
    }

    #[test]
    fn private_devices_exclude_revoked_registry_entries() {
        assert!(workspace_devices(&[registered_server()], Some(&[])).is_empty());
    }

    #[test]
    fn private_membership_absence_preserves_local_and_cloud_devices() {
        let server = registered_server();
        let rows = workspace_devices(&[server.clone()], None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].device, server);
        assert!(rows[0].registered);
        assert!(!rows[0].paired);
    }

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
        assert_eq!(format_last_seen(None, now), "never seen");
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::seconds(30)), now),
            "just now"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::minutes(5)), now),
            "5m ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::hours(3)), now),
            "3h ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::days(2)), now),
            "2d ago"
        );
    }

    #[test]
    fn local_subtitle_does_not_claim_synced_metadata() {
        let copy = devices_subtitle(Some(WorkspaceScope::Local));
        assert!(copy.contains("local workspace"));
        assert!(!copy.contains("synced"));
    }
}
