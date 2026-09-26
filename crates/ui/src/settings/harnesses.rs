//! Settings → Providers: install and enable harnesses, one card row per agent.
//!
//! The state is PER-DEVICE and lives on the engine (`harness-prefs.json` in
//! its data dir): CLI installs are per-device, so enablement is too. The
//! page-header device switcher (the Accounts pattern) retargets both the
//! `ListHarnesses` probe and the `SetHarnessEnabled` writes at any registered
//! device over the relay-forwarded RPCs.
//!
//! Enablement follows DETECTION: every harness whose CLI probe passes is on
//! unless the user switched it off, so installing an agent is all it takes
//! for it to appear here and in the composer. A harness whose CLI is missing
//! on the target device renders dimmed with an install hint and is never
//! enabled (enabling an agent that can't run would only manufacture
//! NotInstalled errors at send time); an ENABLED agent can always be turned
//! OFF except the last one standing — the composer needs something to run.
//! Catalogs from engines predating the detection model can still stamp
//! enabled-but-uninstalled rows; the hint covers that too. The engine
//! enforces the same gates where the state lives, so a raced or stale toggle
//! self-corrects from the RPC reply.

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Task, Window, div, prelude::*,
    px,
};

use zeron_engine::registry::{HarnessDescriptor, descriptor_enabled};

use zeron_proto::HarnessId;
use zeron_rpc::methods;

use crate::motion;
use crate::pickers::visible_harnesses;
use crate::popover::{self, Loadable};
use crate::settings::accounts::{self, AccountsPage};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

#[path = "completion.rs"]
mod completion;

/// Left inset of an expanded provider's details: the header trigger's
/// padding, brand tile and gap, so the details start on the title's edge.
const DETAILS_INSET: f32 = 4.0 + 36.0 + 12.0;

fn offers_install(harness: HarnessId, installed: bool, can_install: bool) -> bool {
    harness != HarnessId::Mock && !installed && can_install
}

fn install_hint(harness: HarnessId, enabled: bool, can_install: bool) -> String {
    if harness == HarnessId::Antigravity {
        return if can_install {
            "Install Antigravity to enable"
        } else {
            "Set ANTIGRAVITY_ACP_EXECUTABLE to enable Antigravity"
        }
        .into();
    }
    let hint = if enabled {
        format!(
            "{} CLI not installed — turn it off or install it",
            cli_name(harness)
        )
    } else {
        format!("Install the {} CLI to enable", cli_name(harness))
    };
    if !can_install && let Some(command) = zeron_harness::install::manual_command(harness) {
        format!("{hint}. Install with `{command}`")
    } else {
        hint
    }
}

fn install_label(name: &str) -> String {
    format!("Installing {name}…")
}

fn install_params(harness: HarnessId, target: &Option<String>) -> serde_json::Value {
    serde_json::json!({"harness": harness, "targetDeviceId": target})
}

/// The CLI named in the not-installed hint.
pub fn cli_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "claude",
        HarnessId::Codex => "codex",
        HarnessId::Cursor => "cursor-agent",
        HarnessId::Devin => "devin",
        HarnessId::Grok => "grok",
        HarnessId::Hermes => "hermes",
        HarnessId::Pi => "pi",
        HarnessId::Opencode => "opencode",
        HarnessId::Antigravity => "Antigravity",
        HarnessId::Mock => "mock",
    }
}

pub struct HarnessesPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    /// Which device's harnesses are shown/edited; `None` = this device (no
    /// passthrough). Retargeted by the page-header device switcher.
    target_device: Option<String>,
    device_select: widgets::SelectState,
    /// Last refused/failed toggle (engine guards), shown in the error strip.
    error: Option<String>,
    load_task: Option<Task<()>>,
    toggle_task: Option<Task<()>>,
    installing: Option<HarnessId>,
    install_task: Option<Task<()>>,
    expanded_harness: Option<HarnessId>,
    /// The expanded provider's Accounts section — one page, retargeted as
    /// providers expand, so every provider shares the same sign-in flow.
    accounts_page: Option<Entity<AccountsPage>>,
}

impl HarnessesPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            harnesses: Loadable::Idle,
            target_device: None,
            device_select: widgets::SelectState::default(),
            error: None,
            load_task: None,
            toggle_task: None,
            installing: None,
            install_task: None,
            expanded_harness: None,
            accounts_page: None,
        };
        page.load(cx);
        page
    }

    fn toggle_agent_details(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if !self.harnesses.ready().is_some_and(|items| {
            items
                .iter()
                .any(|item| item.id == harness && descriptor_enabled(item))
        }) {
            return;
        }
        if self.expanded_harness == Some(harness) {
            self.expanded_harness = None;
        } else {
            self.expanded_harness = Some(harness);
            if accounts::signs_in(harness) {
                if let Some(accounts) = &self.accounts_page {
                    accounts.update(cx, |page, cx| page.set_embedded_harness(harness, cx));
                } else {
                    let state = self.state.clone();
                    let target = self.target_device.clone();
                    self.accounts_page =
                        Some(cx.new(|cx| AccountsPage::new_embedded(state, target, harness, cx)));
                }
            }
        }
        cx.notify();
    }

    fn render_agent_details(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accounts = accounts::signs_in(harness)
            .then(|| self.accounts_page.clone())
            .flatten()
            .map(|page| page.into_any_element());
        // No box of its own: the details continue the provider row on the
        // block's fill, indented to the row's title so they read as its
        // children, with switches and actions on the row's control edge.
        let content = div()
            .mx(px(16.0))
            .pl(px(DETAILS_INSET))
            .pb(px(16.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(self.render_completion_for(harness, theme, cx))
            .when_some(accounts, |details, accounts| details.child(accounts));
        if motion::reduced_motion(cx) {
            content.into_any_element()
        } else {
            motion::menu_in(format!("agent-details-{harness:?}"), content).into_any_element()
        }
    }

    /// Params with the `targetDeviceId` passthrough merged in.
    fn with_target(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let (Some(target), Some(object)) = (&self.target_device, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    /// Escape that reached Settings unclaimed goes to the expanded provider's
    /// accounts (an open login) first. Returns whether it was consumed.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        self.expanded_harness.is_some()
            && self
                .accounts_page
                .as_ref()
                .is_some_and(|accounts| accounts.update(cx, |page, cx| page.dismiss_on_escape(cx)))
    }

    /// Retarget the page at another device: a different device is a different
    /// install/enablement world, so drop the rows and reload through it.
    fn set_target_device(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        widgets::close_select(self, |page: &mut Self| &mut page.device_select, cx);
        if self.target_device == target {
            cx.notify();
            return;
        }
        self.installing = None;
        self.install_task = None;
        self.target_device = target;
        if let Some(accounts) = &self.accounts_page {
            accounts.update(cx, |page, cx| {
                page.set_target_device(self.target_device.clone(), cx)
            });
        }
        self.error = None;
        self.harnesses = Loadable::Idle;
        self.load(cx);
        cx.notify();
    }

    /// `ListHarnesses` against the target device (installed probe + enabled
    /// set both come from where the CLIs actually live).
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({}));
        self.harnesses = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::LIST_HARNESSES, params).await;
            this.update(cx, |page, cx| {
                page.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Flip one harness on the target device. The reply carries the device's
    /// fresh catalog, so the rows repaint from the authoritative state in one
    /// round trip; refusals (engine guards) land in the error strip.
    fn toggle(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        self.set_enabled(harness, enabled, cx);
    }

    fn cancel_install(&mut self, cx: &mut Context<Self>) {
        let Some(harness) = self.installing else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target_device.clone();
        let params = install_params(harness, &target);
        cx.spawn(async move |this, cx| {
            if let Err(error) = engine.client().call(methods::CANCEL_INSTALL, params).await {
                this.update(cx, |page, cx| {
                    if page.target_device == target && page.installing == Some(harness) {
                        page.error = Some(format!("Cancellation failed — {error}"));
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn install(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if self.installing.is_some() {
            return;
        }
        let params = install_params(harness, &self.target_device);
        let target = self.target_device.clone();
        self.installing = Some(harness);
        self.error = None;
        self.install_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::INSTALL_HARNESS, params)
                .await
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<Vec<HarnessDescriptor>>(value)
                        .map_err(|error| error.to_string())
                });
            this.update(cx, |page, cx| {
                if page.target_device != target {
                    return;
                }
                page.installing = None;
                match result {
                    Ok(list) => {
                        page.harnesses = Loadable::Ready(list);
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(error) => page.error = Some(format!("Installation failed — {error}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_enabled(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.with_target(serde_json::json!({
            "harness": harness,
            "enabled": enabled,
        }));
        self.error = None;
        self.toggle_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_HARNESS_ENABLED, params)
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(value) => {
                        if let Ok(list) = serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                            page.harnesses = Loadable::Ready(list);
                        }
                        if !enabled && page.expanded_harness == Some(harness) {
                            page.expanded_harness = None;
                        }
                        // The composer caches its catalog per space — poke
                        // every Pickers to re-fetch, or the rail keeps the
                        // old set until restart.
                        crate::pickers::bump_harness_catalog(cx);
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The page-header device switcher (the Accounts pattern): platform glyph
    /// · name · presence dot · sort glyph, opening a dropdown of every
    /// registered device.
    fn render_device_switcher(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use crate::icons::{self, icon};
        let (mut devices, local_id) = {
            let s = self.state.read(cx);
            (s.devices.clone(), s.local_device_id.clone())
        };
        devices.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let effective = self.target_device.clone().or_else(|| local_id.clone());
        let platform_glyph = |platform: &str| match platform {
            "macos" | "darwin" => icons::LAPTOP,
            "ios" | "android" => icons::SMARTPHONE,
            _ => icons::MONITOR,
        };
        // Local device = no passthrough (calls stay direct).
        let mut targets: Vec<Option<String>> = Vec::new();
        let mut options = Vec::new();
        for device in &devices {
            let is_local = local_id.as_deref() == Some(device.id.as_str());
            let glyph = platform_glyph(&device.platform);
            let muted = theme.text_muted;
            let option = widgets::SelectOption::new(device.name.clone()).leading(move || {
                icon(glyph)
                    .size(px(16.0))
                    .flex_none()
                    .text_color(muted)
                    .into_any_element()
            });
            options.push(if is_local {
                option.detail("You")
            } else {
                option
            });
            targets.push((!is_local).then(|| device.id.clone()));
        }
        let selected = match devices
            .iter()
            .position(|d| Some(d.id.as_str()) == effective.as_deref())
        {
            Some(ix) => ix,
            // Not registered (yet): keep the current target reachable.
            None => {
                let muted = theme.text_muted;
                options.push(widgets::SelectOption::new("This device").leading(move || {
                    icon(icons::LAPTOP)
                        .size(px(16.0))
                        .flex_none()
                        .text_color(muted)
                        .into_any_element()
                }));
                targets.push(self.target_device.clone());
                options.len() - 1
            }
        };
        widgets::select(
            "harnesses-device-switcher",
            "Device",
            theme,
            |page: &mut Self| &mut page.device_select,
        )
        .options(options, selected)
        .menu_width(260.0)
        .heading("Devices")
        .on_select(move |page, ix, _, cx| {
            if let Some(target) = targets.get(ix) {
                page.set_target_device(target.clone(), cx);
            }
        })
        .render(&self.device_select, cx)
        .into_any_element()
    }

    fn rows(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let theme = Theme::of(cx).for_settings_surface();
        let Loadable::Ready(list) = &self.harnesses else {
            return Vec::new();
        };
        let descriptors = visible_harnesses(list);
        let enabled_count = descriptors.iter().filter(|d| descriptor_enabled(d)).count();
        descriptors
            .into_iter()
            .enumerate()
            .map(|(ix, descriptor)| {
                let harness = descriptor.id;
                let installed = descriptor.installed;
                let enabled = descriptor_enabled(&descriptor);
                // The one enabled harness left can't be switched off — the
                // composer needs something to run — but only when it could
                // actually run: an uninstalled last harness stays togglable
                // (its hint says to turn it off) and the composer handles the
                // resulting empty set (mirrors the engine guard).
                let last_enabled = enabled && enabled_count == 1 && installed;
                // Turning OFF never needs the CLI (a default-on agent the
                // user doesn't want must not be stuck on because it isn't
                // installed); turning ON still does.
                let interactive = !last_enabled && (enabled || installed);
                let (icon_path, tint) = crate::pickers::harness_brand_icon(harness);
                let mut meta: Vec<gpui::AnyElement> = Vec::new();
                // Installing REPLACES the not-installed hint in place, so the
                // row's text never shifts while the install runs.
                if self.installing == Some(harness) {
                    meta.push(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(crate::loaders::mini_mono_spinner(
                                format!("harness-install-spinner-{harness:?}"),
                                1.5,
                                theme.text_muted,
                                cx.entity_id(),
                                cx,
                            ))
                            .child(SharedString::from(install_label(&descriptor.name)))
                            .into_any_element(),
                    );
                } else if !installed {
                    meta.push(
                        div()
                            .text_color(theme.warning_muted.opacity(0.9))
                            .child(SharedString::from(install_hint(
                                harness,
                                enabled,
                                descriptor.can_install,
                            )))
                            .into_any_element(),
                    );
                }
                // widgets::row_tile with the brand tint honored (the Claude
                // mark keeps its orange, like the picker rail).
                let tile = div()
                    .flex_none()
                    .size(px(36.0))
                    .rounded(px(10.0))
                    .bg(theme.wash(0.06))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::icons::icon(icon_path)
                            .size(px(16.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    );
                let expanded = enabled && self.expanded_harness == Some(harness);
                let header = widgets::card_row(&theme, ix == 0)
                    .id(("harness-row", ix))
                    .when(!installed && self.installing != Some(harness), |el| {
                        el.opacity(0.55)
                    })
                    .child(
                        div()
                            .id(("harness-details-trigger", ix))
                            .group(format!("harness-details-{ix}"))
                            .flex_1()
                            .min_w(px(180.0))
                            .min_h(px(44.0))
                            .px(px(4.0))
                            .rounded(px(8.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .when(enabled, |el| {
                                el.role(gpui::Role::Button)
                                    .aria_label(format!("{} preferences", descriptor.name))
                                    .aria_expanded(expanded)
                                    .tab_index(0)
                                    .cursor_pointer()
                                    // No hover slab (it stopped short of the
                                    // toggle and boxed the row); the chevron
                                    // lifts instead, like a list disclosure.
                                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.toggle_agent_details(harness, cx)
                                    }))
                                    .on_key_down(cx.listener(
                                        move |page, event: &gpui::KeyDownEvent, _, cx| {
                                            if !event.is_held
                                                && matches!(
                                                    event.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                )
                                            {
                                                page.toggle_agent_details(harness, cx);
                                                cx.stop_propagation();
                                            }
                                        },
                                    ))
                            })
                            .child(tile)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(widgets::row_title(&theme, descriptor.name.clone()))
                                    .when(!meta.is_empty(), |label| {
                                        label.child(widgets::meta_line(&theme, meta))
                                    }),
                            )
                            .when(enabled, |el| {
                                el.child(
                                    crate::icons::icon(if expanded {
                                        crate::icons::ALT_ARROW_DOWN
                                    } else {
                                        crate::icons::ALT_ARROW_RIGHT
                                    })
                                    .size(px(14.0))
                                    .text_color(theme.text_muted)
                                    .group_hover(format!("harness-details-{ix}"), |s| {
                                        s.text_color(theme.text)
                                    }),
                                )
                            }),
                    )
                    .when(
                        offers_install(harness, installed, descriptor.can_install)
                            && self.installing != Some(harness),
                        |el| {
                            el.child(
                                widgets::ghost_action(&theme)
                                    .id(("harness-install", ix))
                                    .when(self.installing.is_none(), |el| {
                                        el.on_click(cx.listener(move |this, _, _, cx| {
                                            this.install(harness, cx)
                                        }))
                                    })
                                    .child("Install"),
                            )
                        },
                    )
                    .when(self.installing == Some(harness), |el| {
                        el.child(
                            widgets::ghost_action(&theme)
                                .id(("harness-cancel-install", ix))
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_install(cx)))
                                .child("Cancel"),
                        )
                    })
                    .child(
                        widgets::toggle_switch(
                            &theme,
                            enabled,
                            format!("harness-switch-{harness:?}"),
                        )
                        .id(("harness-toggle", ix))
                        .when(!interactive && !enabled, |el| el.opacity(0.55))
                        .when(interactive, |el| {
                            el.cursor_pointer()
                                .tab_index(0)
                                .role(gpui::Role::Switch)
                                .aria_label(descriptor.name.clone())
                                .aria_toggled(if enabled {
                                    gpui::Toggled::True
                                } else {
                                    gpui::Toggled::False
                                })
                                .focus_visible(|s| {
                                    s.border_2().border_color(theme.accent).opacity(1.0)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle(harness, !enabled, cx);
                                }))
                                .on_key_down(cx.listener(
                                    move |this, event: &gpui::KeyDownEvent, _, cx| {
                                        if !event.is_held
                                            && matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | "space"
                                            )
                                        {
                                            this.toggle(harness, !enabled, cx);
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                        }),
                    );
                div()
                    .flex()
                    .flex_col()
                    .child(header)
                    .when(expanded, |row| {
                        row.child(self.render_agent_details(harness, &theme, cx))
                    })
                    .into_any_element()
            })
            .collect()
    }
    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for HarnessesPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for HarnessesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let body: gpui::AnyElement = match &self.harnesses {
            Loadable::Idle | Loadable::Loading => widgets::section_card(&theme)
                .p(px(16.0))
                .child(popover::skeleton_rows(
                    "harnesses-skeleton",
                    &theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Loadable::Error(message) => {
                let message = message.clone();
                div()
                    .child(widgets::error_strip(&theme, message))
                    .child(
                        widgets::ghost_action(&theme)
                            .id("harnesses-retry")
                            .mt(px(8.0))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(cx);
                                cx.notify();
                            }))
                            .child(SharedString::from("Retry")),
                    )
                    .into_any_element()
            }
            Loadable::Ready(_) => {
                let rows = self.rows(cx);
                widgets::section_card(&theme)
                    .children(rows)
                    .into_any_element()
            }
        };
        let error = self
            .error
            .clone()
            .map(|message| widgets::error_strip(&theme, message).into_any_element());
        let switcher = self.render_device_switcher(&theme, cx);
        let scrollbar = popover::rail(self, "harnesses-page-scrollbar", &theme, cx);

        div()
            .id("harnesses-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("harnesses-page")
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
                                        .child(widgets::page_header(&theme, "Providers", None))
                                        .child(switcher),
                                )
                                .children(error)
                                .child(body),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    #[gpui::test]
    fn expanded_agent_preferences_render_inside_the_agent_row(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            super::HarnessesPage::new(state, cx)
        });
        let descriptor = |id, name: &str| zeron_engine::registry::HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: false,
            steering_mode: zeron_proto::SteeringMode::TurnBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            can_install: false,
            enabled: Some(true),
        };
        window
            .update(cx, |page, _, cx| {
                page.harnesses = super::Loadable::Ready(vec![
                    descriptor(zeron_proto::HarnessId::ClaudeCode, "Claude Code"),
                    descriptor(zeron_proto::HarnessId::Codex, "Codex"),
                ]);
                page.toggle_agent_details(zeron_proto::HarnessId::ClaudeCode, cx);
                assert_eq!(
                    page.expanded_harness,
                    Some(zeron_proto::HarnessId::ClaudeCode)
                );
                assert_eq!(
                    page.accounts_page
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .embedded_harness(),
                    Some(zeron_proto::HarnessId::ClaudeCode)
                );
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        window
            .update(cx, |page, _, cx| {
                page.toggle_agent_details(zeron_proto::HarnessId::Codex, cx);
                assert_eq!(page.expanded_harness, Some(zeron_proto::HarnessId::Codex));
                assert_eq!(
                    page.accounts_page
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .embedded_harness(),
                    Some(zeron_proto::HarnessId::Codex)
                );
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        // Antigravity signs in through the very same Accounts section.
        window
            .update(cx, |page, _, cx| {
                page.harnesses = super::Loadable::Ready(vec![descriptor(
                    zeron_proto::HarnessId::Antigravity,
                    "Antigravity",
                )]);
                page.toggle_agent_details(zeron_proto::HarnessId::Antigravity, cx);
                assert_eq!(
                    page.accounts_page
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .embedded_harness(),
                    Some(zeron_proto::HarnessId::Antigravity)
                );
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }
}

#[cfg(test)]
#[test]
fn install_visibility_and_hint_follow_target_capabilities() {
    for id in [
        HarnessId::Antigravity,
        HarnessId::Codex,
        HarnessId::Opencode,
        HarnessId::ClaudeCode,
        HarnessId::Cursor,
        HarnessId::Pi,
        HarnessId::Grok,
        HarnessId::Hermes,
        HarnessId::Devin,
        HarnessId::Mock,
    ] {
        for installed in [false, true] {
            for available in [false, true] {
                assert_eq!(
                    offers_install(id, installed, available),
                    id != HarnessId::Mock && !installed && available
                );
            }
        }
    }
    assert_eq!(cli_name(HarnessId::Antigravity), "Antigravity");
    assert_eq!(
        install_hint(HarnessId::Antigravity, false, true),
        "Install Antigravity to enable"
    );
    assert!(
        install_hint(HarnessId::Antigravity, false, false).contains("ANTIGRAVITY_ACP_EXECUTABLE")
    );
}

#[cfg(test)]
#[test]
fn install_phase_copy_and_cancel_target_match_install() {
    assert_eq!(install_label("Claude Code"), "Installing Claude Code…");
    assert_eq!(install_label("Pi"), "Installing Pi…");
    for target in [None, Some("remote-device".to_string())] {
        let params = install_params(HarnessId::Pi, &target);
        assert_eq!(params["harness"], "pi");
        assert_eq!(
            params["targetDeviceId"],
            serde_json::to_value(&target).unwrap()
        );
    }
    assert_eq!(
        install_hint(HarnessId::Codex, false, false),
        "Install the codex CLI to enable. Install with `npm install -g @openai/codex`"
    );
    assert!(
        install_hint(HarnessId::Codex, true, false)
            .starts_with("codex CLI not installed — turn it off or install it.")
    );
}
