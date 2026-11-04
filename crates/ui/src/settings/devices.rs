//! Settings → Devices (feature-inventory §1.5): the device registry — name,
//! platform, last-seen, an Online/Offline badge, a "This device" badge, click-to-copy id,
//! Rename, explicit wake for registered Namespace and Boat hosts, and Add devbox.

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, EventEmitter, SharedString, Subscription, Task,
    Window, div, prelude::*, px,
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

/// Provider operations are only initiated by explicit user actions. The reply
/// must prove the same immutable binding before its fresh capabilities are used.
/// How long a caller waits for a wake reply: the engine's whole wake budget,
/// plus a grace so that the engine's own timeout error arrives first.
pub(crate) const WAKE_REPLY_DEADLINE: Duration = Duration::from_secs(
    zeron_proto::DEVBOX_WAKE_TIMEOUT_SECS + zeron_proto::DEVBOX_WAKE_REPLY_GRACE_SECS,
);

pub(crate) async fn wake_host(
    engine: &crate::state::EngineHandle,
    executor: &gpui::BackgroundExecutor,
    device: &zeron_proto::Device,
) -> Result<zeron_proto::Device, String> {
    let request = engine.client().call_scoped(
        methods::WAKE_DEVICE,
        serde_json::json!({"deviceId": device.id}),
    );
    let deadline = executor.timer(WAKE_REPLY_DEADLINE);
    futures::pin_mut!(request, deadline);
    let value = match futures::future::select(request, deadline).await {
        futures::future::Either::Left((result, _)) => result.map_err(|error| error.to_string())?,
        futures::future::Either::Right(_) => {
            return Err("Connection timed out. Your draft is preserved.".into());
        }
    };
    validate_wake_reply(device, value)
}

fn validate_wake_reply(
    expected: &zeron_proto::Device,
    value: serde_json::Value,
) -> Result<zeron_proto::Device, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Reply {
        device_id: String,
        devbox_provider: zeron_proto::DeviceEnvironment,
        devbox_id: String,
        device: zeron_proto::Device,
        ready: bool,
    }
    let reply: Reply = serde_json::from_value(value)
        .map_err(|error| format!("Invalid connection response: {error}"))?;
    if !reply.ready
        || expected.devbox_binding().is_none()
        || reply.device_id != expected.id
        || reply.device.id != expected.id
        || Some(reply.devbox_provider) != expected.devbox_provider
        || Some(reply.devbox_id.as_str()) != expected.devbox_id.as_deref()
        || reply.device.devbox_binding() != expected.devbox_binding()
    {
        return Err(
            "The connected device does not match the selected host. Nothing was sent.".into(),
        );
    }
    Ok(reply.device)
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

/// The new chat draft behind Add devbox. Crew's devbox workflow owns setup;
/// the agent follows it on this computer while the user signs in and approves.
pub const DEVBOX_SETUP_REQUEST: &str = "Set up a new personal devbox for me on Boat, following the devbox workflow: skills/devbox in Ashler-AI/ashler-platform, read from its master branch. Use an xlarge machine, a name that doesn't clash with my existing devboxes, and the same Crew channel as this computer. Leave sign-ins and approvals to me, and ask before any billing change. It's done when the new machine shows as a Devbox in Settings › Devices.";

/// What the page asks the shell to do.
pub enum DevicesEvent {
    /// Open a new chat on this computer that sets up a devbox.
    AddDevbox,
}

/// Scope-aware copy: a local registry describes only the active local
/// workspace and must not imply that account device metadata is already live.
pub fn devices_subtitle(scope: Option<WorkspaceScope>) -> &'static str {
    match scope {
        Some(WorkspaceScope::Local) => "Devices in this local workspace.",
        Some(WorkspaceScope::Synced) => "Devices synced to this workspace.",
        Some(WorkspaceScope::Development) | None => "Devices in this workspace.",
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
    waking: Option<String>,
    wake_task: Option<Task<()>>,
    _observe: Subscription,
}

impl EventEmitter<DevicesEvent> for DevicesPage {}

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
            waking: None,
            wake_task: None,
            _observe: observe,
        }
    }

    pub(crate) fn on_hidden(&mut self) {
        self.wake_task = None;
        self.waking = None;
    }

    pub(crate) fn add_devbox(&mut self, cx: &mut Context<Self>) {
        cx.emit(DevicesEvent::AddDevbox);
    }

    fn wake_device(&mut self, device_id: &str, cx: &mut Context<Self>) {
        if self.wake_task.is_some() {
            return;
        }
        let state = self.state.read(cx);
        let Some(device) = state
            .devices
            .iter()
            .find(|device| device.id == device_id)
            .cloned()
        else {
            return;
        };
        if state.local_device_id.as_deref() == Some(device_id)
            || state.device_online(device_id, Utc::now())
            || device.devbox_binding().is_none()
        {
            return;
        }
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        self.waking = Some(device.id.clone());
        self.error = None;
        self.wake_task = Some(cx.spawn(async move |this, cx| {
            let result = wake_host(&engine, cx.background_executor(), &device).await;
            this.update(cx, |page, cx| {
                page.waking = None;
                page.wake_task = None;
                match result {
                    Ok(fresh) => page.state.update(cx, |state, cx| {
                        let mut devices = state.devices.clone();
                        if let Some(slot) = devices.iter_mut().find(|row| row.id == device.id)
                            && slot.devbox_binding() == device.devbox_binding()
                        {
                            *slot = fresh;
                            state.apply_devices(devices);
                            cx.notify();
                        }
                    }),
                    Err(error) => page.error = Some(format!("Could not connect: {error}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(crate) fn open_rename(
        &mut self,
        device_id: String,
        current: String,
        cx: &mut Context<Self>,
    ) {
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

    /// Whether a dialog [`Self::dismiss_on_escape`] would close is open.
    pub(crate) fn has_escape_surface(&self) -> bool {
        self.rename.is_some()
    }

    /// Escape that reached Settings unclaimed closes the rename dialog first,
    /// so it never closes Settings under the dialog. Returns whether it did.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.rename.take().is_none() {
            return false;
        }
        cx.notify();
        true
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

    fn render_rename_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let dialog = self.rename.as_ref()?;
        let input = dialog.input.clone();
        let card = popover::dialog_card(&theme)
            .id("rename-device-card")
            .role(gpui::Role::Dialog)
            .aria_label("Rename device")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.rename = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
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
                        widgets::text_action(&theme, widgets::ActionTone::Quiet, "Cancel")
                            .id("rename-cancel")
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rename = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        widgets::text_action(&theme, widgets::ActionTone::Solid, "Rename")
                            .id("rename-save")
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
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
        crate::toast::take_error(&mut self.error, cx);
        let theme = Theme::of(cx).for_settings_surface();
        let now = Utc::now();
        let (devices, local_id, workspace_scope) = {
            let state = self.state.read(cx);
            (
                state.devices.clone(),
                state.local_device_id.clone(),
                state.workspace_scope,
            )
        };
        let copied = self.copied.clone();
        let waking = self.waking.clone();
        let wake_busy = self.wake_task.is_some();
        let dialog = self.render_rename_dialog(window.viewport_size(), cx);
        // Split into this device and the rest; each renders as rows in one
        // block, like every other settings page.
        let (local, others): (Vec<_>, Vec<_>) = devices
            .into_iter()
            .enumerate()
            .partition(|(_, device)| local_id.as_deref() == Some(device.id.as_str()));
        let device_row = |ix: usize, device: zeron_proto::Device, first: bool| {
            let online = device_online(device.last_seen_at, now);
            let is_local = local_id.as_deref() == Some(device.id.as_str());
            let id_copied = copied.as_deref() == Some(device.id.as_str());
            let copy_id = device.id.clone();
            let rename_id = device.id.clone();
            let rename_name = device.name.clone();
            let can_wake = !is_local && !online && device.devbox_binding().is_some();
            let wake_id = device.id.clone();
            let is_waking = waking.as_deref() == Some(device.id.as_str());
            let mut meta: Vec<AnyElement> = vec![
                div()
                    .child(SharedString::from(
                        platform_label(&device.platform).to_string(),
                    ))
                    .into_any_element(),
            ];
            if device.devbox_binding().is_some() {
                meta.push(div().child("Devbox").into_any_element());
            }
            if let Some(version) = device.version.as_deref().filter(|v| !v.is_empty()) {
                meta.push(
                    div()
                        .child(SharedString::from(format!("v{version}")))
                        .into_any_element(),
                );
            }
            // Presence only says something about other devices.
            if !is_local {
                meta.push(if online {
                    div()
                        .text_color(theme.success_muted)
                        .child(SharedString::from("Online"))
                        .into_any_element()
                } else {
                    div()
                        .child(SharedString::from(format!(
                            "{}Last seen {}",
                            if can_wake { "Idle · " } else { "" },
                            format_last_seen(device.last_seen_at, now)
                        )))
                        .into_any_element()
                });
            }
            widgets::card_row(&theme, first)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(&theme, device.name.clone()))
                        .child(widgets::meta_line(&theme, meta)),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .when(can_wake, |el| {
                            el.child(
                                widgets::text_action(
                                    &theme,
                                    widgets::ActionTone::Filled,
                                    if is_waking { "Connecting…" } else { "Wake" },
                                )
                                .id(("device-wake", ix))
                                .tab_index(if wake_busy { -1 } else { 0 })
                                .role(gpui::Role::Button)
                                .aria_label(format!("Wake {}", device.name))
                                .focus_visible(|s| s.border_2().border_color(theme.accent))
                                .when(wake_busy, |el| {
                                    el.opacity(0.5).aria_description(
                                        "Unavailable while a device is connecting",
                                    )
                                })
                                .when(!wake_busy, |el| {
                                    el.on_click(cx.listener(move |this, _, _, cx| {
                                        this.wake_device(&wake_id, cx)
                                    }))
                                }),
                            )
                        })
                        .child(
                            widgets::text_action(
                                &theme,
                                widgets::ActionTone::Quiet,
                                if id_copied { "Copied" } else { "Copy ID" },
                            )
                            .id(("device-id", ix))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .aria_label(format!("Copy device ID {}", device.id))
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.copy_id(copy_id.clone(), cx);
                                },
                            )),
                        )
                        .child(
                            widgets::text_action(&theme, widgets::ActionTone::Filled, "Rename")
                                .id(("device-rename", ix))
                                .tab_index(0)
                                .role(gpui::Role::Button)
                                .aria_label(format!("Rename {}", device.name))
                                .focus_visible(|s| s.border_2().border_color(theme.accent))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_rename(rename_id.clone(), rename_name.clone(), cx);
                                })),
                        ),
                )
                .into_any_element()
        };
        let local_block = (!local.is_empty()).then(|| {
            let mut block = widgets::section_card(&theme).mt(px(8.0));
            for (n, (ix, device)) in local.into_iter().enumerate() {
                block = block.child(device_row(ix, device, n == 0));
            }
            block
        });
        // Add devbox leads Other devices: a workspace with many sandboxes
        // would otherwise bury it below every row (user request).
        let others_block = widgets::section_card(&theme).mt(px(8.0)).child(
            widgets::card_row(&theme, true)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(&theme, "Devbox"))
                        .child(widgets::meta_line(
                            &theme,
                            vec![div()
                                .child("An agent on this computer sets up a Boat machine for you")
                                .into_any_element()],
                        )),
                )
                .child(
                    widgets::text_action(&theme, widgets::ActionTone::Filled, "Add devbox")
                        .id("device-add-devbox")
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label("Add devbox")
                        .focus_visible(|s| s.border_2().border_color(theme.accent))
                        .on_click(cx.listener(|this, _, _, cx| this.add_devbox(cx))),
                ),
        );
        let others_block = if others.is_empty() {
            others_block.child(
                widgets::card_row(&theme, false).child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(
                            "Sign in on another device to see it here.",
                        )),
                ),
            )
        } else {
            others
                .into_iter()
                .fold(others_block, |block, (ix, device)| {
                    block.child(device_row(ix, device, false))
                })
        };
        let card = div()
            .flex()
            .flex_col()
            .when_some(local_block, |el, block| {
                el.child(widgets::section_label(&theme, "This device").mt(px(28.0)))
                    .child(block)
            })
            // A local-only workspace never has other devices to list.
            .when(workspace_scope != Some(WorkspaceScope::Local), |el| {
                el.child(widgets::section_label(&theme, "Other devices").mt(px(28.0)))
                    .child(others_block)
            });

        let scrollbar = popover::rail(self, "devices-page-scrollbar", &theme, cx);
        div()
            .id("devices-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("devices-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "Devices", None))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    devices_subtitle(workspace_scope),
                                ))
                                .child(card),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .when_some(dialog, |el, dialog| el.child(dialog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    #[gpui::test]
    fn stalled_wake_times_out_and_cancels_when_hidden_or_dropped(cx: &mut gpui::TestAppContext) {
        use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        struct CancelSignal(Arc<tokio::sync::Notify>);
        impl Drop for CancelSignal {
            fn drop(&mut self) {
                self.0.notify_one();
            }
        }

        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let started = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(tokio::sync::Notify::new());
        let rpc = ScriptedRpc::new({
            let started = started.clone();
            let cancelled = cancelled.clone();
            move |method, params| {
                let started = started.clone();
                let cancelled = cancelled.clone();
                async move {
                    assert_eq!(method, methods::WAKE_DEVICE);
                    assert_eq!(params, serde_json::json!({"deviceId": "remote"}));
                    let _signal = CancelSignal(cancelled);
                    started.fetch_add(1, Ordering::SeqCst);
                    futures::future::pending().await
                }
            }
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
            state.local_device_id = Some("local".into());
            state.devices = vec![zeron_proto::Device {
                id: "remote".into(),
                name: "Boat".into(),
                platform: "linux".into(),
                last_seen_at: None,
                created_at: None,
                version: None,
                cursor_sdk_version: None,
                capabilities: Vec::new(),
                devbox_provider: Some(zeron_proto::DeviceEnvironment::Boat),
                devbox_id: Some("bx_23456789".into()),
            }];
            state
        });
        let page = cx.new(|cx| DevicesPage::new(state.clone(), cx));
        page.update(cx, |page, cx| page.wake_device("remote".into(), cx));
        rpc.settle(cx, &runtime, |_| started.load(Ordering::SeqCst) == 1);
        // A cold wake can outlast the old 180 s cap: wait out the engine's budget.
        cx.executor().advance_clock(WAKE_REPLY_DEADLINE - Duration::from_secs(1));
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert!(page.wake_task.is_some(), "still within the wake budget"));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.wake_task.is_none());
            assert!(page.waking.is_none());
            assert!(page.error.as_ref().unwrap().contains("timed out"));
        });
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), cancelled.notified())
                .await
                .unwrap();
        });

        page.update(cx, |page, cx| page.wake_device("remote".into(), cx));
        rpc.settle(cx, &runtime, |_| started.load(Ordering::SeqCst) == 2);
        page.update(cx, |page, _| {
            page.on_hidden();
            assert!(page.wake_task.is_none());
            assert!(page.waking.is_none());
            assert!(page.error.is_none());
        });
        cx.run_until_parked();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), cancelled.notified())
                .await
                .unwrap();
        });

        page.update(cx, |page, cx| page.wake_device("remote".into(), cx));
        rpc.settle(cx, &runtime, |_| started.load(Ordering::SeqCst) == 3);
        let released = Arc::new(AtomicUsize::new(0));
        page.update(cx, |_, cx| {
            let released = released.clone();
            cx.on_release(move |_, _| {
                released.fetch_add(1, Ordering::SeqCst);
            })
            .detach();
        });
        drop(page);
        // Dropping Entity queues disposal; only an App update flushes the
        // deferred entity value (and its owned wake_task). Running the task
        // dispatcher alone leaves that task alive while waiting on Tokio.
        cx.update(|_| {});
        assert_eq!(
            released.load(Ordering::SeqCst),
            1,
            "the page must actually be disposed before awaiting server cancellation"
        );
        cx.run_until_parked();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), cancelled.notified())
                .await
                .unwrap();
        });
        state.read_with(cx, |state, _| {
            assert!(state.devices[0].last_seen_at.is_none());
        });
    }

    #[gpui::test]
    fn wake_is_explicit_single_flight_and_requires_exact_ready_ids(cx: &mut gpui::TestAppContext) {
        use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
        use std::sync::{Arc, Mutex};

        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(tokio::sync::Notify::new());
        let rpc = ScriptedRpc::new({
            let calls = calls.clone();
            let release = release.clone();
            move |method, params| {
                let calls = calls.clone();
                let release = release.clone();
                async move {
                    assert_eq!(method, methods::WAKE_DEVICE);
                    assert_eq!(params, serde_json::json!({"deviceId": "remote"}));
                    let attempt = {
                        let mut calls = calls.lock().unwrap();
                        calls.push(params);
                        calls.len()
                    };
                    release.notified().await;
                    let mut response = serde_json::json!({
                        "deviceId": "remote",
                        "devboxProvider": "boat", "devboxId": "bx_23456789", "ready": true,
                        "device": {
                            "id": "remote",
                            "name": "Ready Boat",
                            "platform": "linux",
                            "lastSeenAt": Utc::now(),
                            "version": "1.2.3",
                            "capabilities": ["readFiles"],
                            "devboxProvider": "boat", "devboxId": "bx_23456789"
                        }
                    });
                    match attempt {
                        1 => response["deviceId"] = "other".into(),
                        2 => response["devboxId"] = "bx_abcdefgh".into(),
                        3 => {
                            response.as_object_mut().unwrap().remove("devboxId");
                        }
                        4 => return Err(zeron_rpc::RpcError::Failed("Unavailable".into())),
                        5 => {
                            response.as_object_mut().unwrap().remove("device");
                        }
                        6 => response["device"]["id"] = "other".into(),
                        7 => response["device"]["devboxId"] = "bx_abcdefgh".into(),
                        8 => {}
                        _ => panic!("duplicate wake request"),
                    }
                    zeron_rpc::RpcReply::value(&response)
                }
            }
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
            state.local_device_id = Some("local".into());
            state.devices = ["remote", "local", "online", "invalid", "unbound"]
                .into_iter()
                .map(|id| zeron_proto::Device {
                    id: id.into(),
                    name: id.into(),
                    platform: "linux".into(),
                    last_seen_at: (id == "online").then(Utc::now),
                    created_at: None,
                    version: None,
                    cursor_sdk_version: None,
                    capabilities: Vec::new(),
                    devbox_provider: Some(zeron_proto::DeviceEnvironment::Boat),
                    devbox_id: match id {
                        "invalid" => Some("display-name".into()),
                        "unbound" => None,
                        _ => Some("bx_23456789".into()),
                    },
                })
                .collect();
            state
        });
        let page = cx.new(|cx| DevicesPage::new(state.clone(), cx));
        state.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert!(calls.lock().unwrap().is_empty(), "mounting must not wake");
        page.update(cx, |page, cx| {
            for id in ["local", "online", "invalid", "unbound", "missing"] {
                page.wake_device(id.into(), cx);
            }
            assert!(page.wake_task.is_none());
        });
        cx.run_until_parked();
        assert!(calls.lock().unwrap().is_empty());

        let before = state.read_with(cx, |state, _| state.devices.clone());
        for attempt in 1..=8 {
            page.update(cx, |page, cx| {
                page.wake_device("remote".into(), cx);
                page.wake_device("remote".into(), cx);
            });
            rpc.settle(cx, &runtime, |_| calls.lock().unwrap().len() >= attempt);
            page.read_with(cx, |page, _| {
                assert_eq!(page.waking.as_deref(), Some("remote"));
                assert!(page.wake_task.is_some());
            });
            release.notify_one();
            rpc.settle(cx, &runtime, |cx| {
                page.read_with(cx, |page, _| page.wake_task.is_none())
            });
            page.read_with(cx, |page, _| {
                assert!(page.waking.is_none());
                assert_eq!(page.error.is_some(), attempt != 8);
            });
            assert_eq!(calls.lock().unwrap().len(), attempt);
            state.read_with(cx, |state, _| {
                assert_eq!(&state.devices[1..], &before[1..]);
                if attempt != 8 {
                    assert_eq!(state.devices, before);
                }
            });
        }
        state.read_with(cx, |state, _| {
            let ready = &state.devices[0];
            assert!(ready.last_seen_at.is_some());
            assert_eq!(ready.name, "Ready Boat");
            assert_eq!(ready.version.as_deref(), Some("1.2.3"));
            assert_eq!(ready.capabilities, ["readFiles"]);
        });
    }

    #[test]
    fn wake_reply_requires_ready_and_the_exact_registered_binding() {
        for (provider, id) in [("namespace", "ofpf7g22n4412"), ("boat", "bx_234abcxy")] {
            let device: zeron_proto::Device = serde_json::from_value(serde_json::json!({
                "id": "remote", "name": "host", "platform": "linux",
                "devboxProvider": provider, "devboxId": id,
            }))
            .unwrap();
            let reply = serde_json::json!({
                "deviceId": "remote", "devboxProvider": provider, "devboxId": id,
                "device": device, "ready": true,
            });
            assert!(validate_wake_reply(&device, reply.clone()).is_ok());
            for (field, value) in [
                ("ready", serde_json::json!(false)),
                ("deviceId", serde_json::json!("other")),
                ("devboxId", serde_json::json!("other")),
                ("devboxProvider", serde_json::json!("unknown")),
            ] {
                let mut wrong = reply.clone();
                wrong[field] = value;
                assert!(validate_wake_reply(&device, wrong).is_err());
            }
            let mut wrong = reply.clone();
            wrong["device"]["devboxId"] = serde_json::json!("other");
            assert!(validate_wake_reply(&device, wrong).is_err());
            let mut unbound = device.clone();
            unbound.devbox_provider = None;
            assert!(validate_wake_reply(&unbound, reply).is_err());
        }
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
