use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, EventEmitter, FontWeight, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
use serde::Deserialize;
use serde_json::json;
use zeron_proto::{ConnectivityState, WorkspaceScope};
use zeron_rpc::methods;

use crate::composer::ComposerInput;
use crate::icons;
use crate::popover::{self, Loadable};
use crate::settings::devices::{DeviceEvent, DevicesSection, NodeRole, PairedDevice};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;
use crate::typography::ui_rems;

#[derive(Clone, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum PrivateStatus {
    Unconfigured,
    Configured {
        workspace_id: String,
        hub_url: String,
        name: String,
        role: NodeRole,
        host_hub: bool,
        enabled: bool,
        nodes: Vec<PairedDevice>,
    },
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Invitation {
    code: String,
    #[serde(with = "chrono::serde::ts_milliseconds")]
    expires_at: DateTime<Utc>,
    hub_url: String,
}

fn invitation_link(invitation: &Invitation) -> String {
    let mut url = url::Url::parse("zeron://private/join").expect("fixed private pairing URL");
    url.query_pairs_mut()
        .append_pair("hub", &invitation.hub_url)
        .append_pair("code", &invitation.code);
    url.into()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Setup {
    Choose,
    Create,
    Join,
}

#[derive(Clone, Copy)]
enum ButtonStyle {
    Primary,
    Secondary,
    Quiet,
}

pub enum WorkspaceEvent {
    StartCloud,
    LeaveCloud,
    RunInBackground,
    Restart {
        target: WorkspaceScope,
        import: bool,
    },
}

impl EventEmitter<WorkspaceEvent> for WorkspacePage {}

pub struct WorkspacePage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    devices: Entity<DevicesSection>,
    _device_events: Subscription,
    status: Loadable<PrivateStatus>,
    setup: Setup,
    role: NodeRole,
    invitation_role: NodeRole,
    bring_work: bool,
    name: Entity<ComposerInput>,
    hub: Entity<ComposerInput>,
    code: Entity<ComposerInput>,
    invitation: Option<Invitation>,
    confirm_leave: bool,
    show_details: bool,
    show_modes: bool,
    error: Option<String>,
    notice: Option<String>,
    busy: bool,
    unsaved_files: bool,
    task: Option<Task<()>>,
    expiry_task: Option<Task<()>>,
    _observe: Subscription,
}

impl WorkspacePage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        let devices = cx.new(|cx| DevicesSection::new(state.clone(), cx));
        let device_events = cx.subscribe(&devices, |this: &mut Self, _, event, cx| match event {
            DeviceEvent::Revoke(id) => this.call(
                methods::REVOKE_PRIVATE_NODE,
                json!({"deviceId": id}),
                None,
                cx,
            ),
        });
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            devices,
            _device_events: device_events,
            status: Loadable::Idle,
            setup: Setup::Choose,
            role: NodeRole::Server,
            invitation_role: NodeRole::Client,
            bring_work: false,
            name: cx.new(|cx| ComposerInput::new("Name", cx).with_single_line()),
            hub: cx.new(|cx| {
                ComposerInput::new("https://your-hub.your-tailnet.ts.net:8443", cx)
                    .with_single_line()
            }),
            code: cx.new(|cx| ComposerInput::new("Six-digit pairing code", cx).with_single_line()),
            invitation: None,
            confirm_leave: false,
            show_details: false,
            show_modes: false,
            error: None,
            notice: None,
            busy: false,
            unsaved_files: false,
            task: None,
            expiry_task: None,
            _observe: observe,
        };
        page.load(cx);
        page
    }

    pub fn prefill_invitation(
        &mut self,
        invitation: crate::links::PrivateInvitationLink,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).workspace_scope != Some(WorkspaceScope::Local) {
            self.error = Some("Switch to Local before joining another workspace.".into());
        } else {
            self.hub
                .update(cx, |input, cx| input.set_text(invitation.hub_url, cx));
            self.code
                .update(cx, |input, cx| input.set_text(invitation.code, cx));
            self.setup = Setup::Join;
            self.role = NodeRole::Client;
        }
        cx.notify();
    }

    fn switch_blocked(&self, cx: &Context<Self>) -> bool {
        let state = self.state.read(cx);
        self.unsaved_files
            || state.sessions.iter().any(|session| {
                Some(session.device_id.as_str()) == state.local_device_id.as_deref()
                    && matches!(
                        state.indicator_for(&session.chat_id, Utc::now()),
                        crate::state::Indicator::Working | crate::state::Indicator::AwaitingInput
                    )
            })
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.status =
                Loadable::Error("The engine is not connected. Retry when it is ready.".into());
            return;
        };
        if !matches!(self.status, Loadable::Ready(_)) {
            self.status = Loadable::Loading;
        }
        self.busy = true;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::PRIVATE_STATUS, json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                page.status = match result {
                    Ok(value) => match serde_json::from_value(value) {
                        Ok(status) => Loadable::Ready(status),
                        Err(error) => Loadable::Error(format!("Invalid private status: {error}")),
                    },
                    Err(error) => Loadable::Error(error.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn configure(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.switch_blocked(cx) {
            return;
        }
        let name = self.name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            self.error = Some("Enter a name.".into());
            cx.notify();
            return;
        }
        let (method, params) = match self.setup {
            Setup::Create => (
                methods::CREATE_PRIVATE_WORKSPACE,
                json!({"name": name, "role": self.role.wire()}),
            ),
            Setup::Join => {
                let hub = self.hub.read(cx).text().trim().to_owned();
                let code = self.code.read(cx).text().trim().to_owned();
                if hub.is_empty()
                    || code.len() != 6
                    || !code.bytes().all(|byte| byte.is_ascii_digit())
                {
                    self.error =
                        Some("Enter the hub HTTPS address and its six-digit pairing code.".into());
                    cx.notify();
                    return;
                }
                (
                    methods::JOIN_PRIVATE_WORKSPACE,
                    json!({"hubUrl": hub, "code": code, "name": name, "role": self.role.wire()}),
                )
            }
            Setup::Choose => return,
        };
        self.call(
            method,
            params,
            Some((WorkspaceScope::Private, self.bring_work)),
            cx,
        );
    }

    fn call(
        &mut self,
        method: &'static str,
        params: serde_json::Value,
        restart: Option<(WorkspaceScope, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.busy || (restart.is_some() && self.switch_blocked(cx)) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error = Some("The engine is not connected.".into());
            cx.notify();
            return;
        };
        self.busy = true;
        self.error = None;
        self.notice = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Err(error) => page.error = Some(error.to_string()),
                    Ok(value) => {
                        if let Some((target, import)) = restart {
                            page.code.update(cx, |input, cx| input.set_text("", cx));
                            page.busy = true;
                            page.notice = Some("Switching workspace…".into());
                            cx.emit(WorkspaceEvent::Restart { target, import });
                        } else if method == methods::CREATE_PRIVATE_INVITATION {
                            match serde_json::from_value(value) {
                                Ok(invitation) => {
                                    page.invitation = Some(invitation);
                                    let delay = page
                                        .invitation
                                        .as_ref()
                                        .map(|invite| {
                                            (invite.expires_at - Utc::now())
                                                .to_std()
                                                .unwrap_or_default()
                                        })
                                        .unwrap_or_default();
                                    page.expiry_task = Some(cx.spawn(async move |this, cx| {
                                        cx.background_executor().timer(delay).await;
                                        this.update(cx, |_, cx| cx.notify()).ok();
                                    }));
                                }
                                Err(error) => {
                                    page.error = Some(format!("Invalid invitation: {error}"))
                                }
                            }
                        } else {
                            page.invitation = None;
                            page.load(cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn transition_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.busy = false;
        self.notice = None;
        self.error = Some(error);
        self.load(cx);
    }

    pub fn set_unsaved_files(&mut self, unsaved: bool, cx: &mut Context<Self>) {
        if self.unsaved_files != unsaved {
            self.unsaved_files = unsaved;
            cx.notify();
        }
    }

    fn button(
        &self,
        theme: &Theme,
        id: &'static str,
        title: &'static str,
        style: ButtonStyle,
        enabled: bool,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        widgets::ghost_action(theme)
            .id(id)
            .flex_none()
            .justify_center()
            .py(px(8.0))
            .px(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .when(matches!(style, ButtonStyle::Primary), |el| {
                el.bg(theme.solid).text_color(theme.on_solid)
            })
            .when(matches!(style, ButtonStyle::Secondary), |el| {
                el.border_1()
                    .border_color(theme.border_strong)
                    .text_color(theme.text)
            })
            .hover(move |el| match style {
                ButtonStyle::Primary => el.bg(theme.solid.opacity(0.85)),
                _ => el.bg(theme.element_hover),
            })
            .opacity(if enabled { 1.0 } else { 0.45 })
            .child(title)
            .on_click(cx.listener(move |this, _, _, cx| {
                if enabled && !this.busy {
                    action(this, cx);
                }
            }))
            .into_any_element()
    }

    fn role_picker(&self, theme: &Theme, invitation: bool, cx: &mut Context<Self>) -> AnyElement {
        let role = if invitation {
            self.invitation_role
        } else {
            self.role
        };
        div()
            .flex()
            .flex_wrap()
            .gap(px(10.0))
            .children(
                [NodeRole::Client, NodeRole::Server]
                    .into_iter()
                    .map(|option| {
                        let selected = option == role;
                        div()
                            .id(SharedString::from(format!(
                                "private-role-{invitation}-{}",
                                option.wire()
                            )))
                            .flex_1()
                            .min_w(px(180.0))
                            .p(px(14.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(if selected { theme.accent } else { theme.border })
                            .when(selected, |el| el.bg(theme.accent_wash.opacity(0.3)))
                            .cursor_pointer()
                            .hover(|el| el.bg(theme.element_hover))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .size(px(12.0))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(if selected {
                                                theme.accent
                                            } else {
                                                theme.text_faint
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(selected, |el| {
                                                el.child(
                                                    div()
                                                        .size(px(6.0))
                                                        .rounded_full()
                                                        .bg(theme.accent),
                                                )
                                            }),
                                    )
                                    .child(widgets::row_title(theme, option.label())),
                            )
                            .child(
                                widgets::page_subtitle(
                                    theme,
                                    match option {
                                        NodeRole::Client => {
                                            "Control agents on your other computers."
                                        }
                                        NodeRole::Server => {
                                            "Run agents with this device's repositories."
                                        }
                                    },
                                )
                                .text_size(ui_rems(12.0)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.busy {
                                    if invitation {
                                        if this.invitation_role != option {
                                            this.invitation = None;
                                            this.expiry_task = None;
                                        }
                                        this.invitation_role = option;
                                    } else {
                                        this.role = option;
                                    }
                                    cx.notify();
                                }
                            }))
                    }),
            )
            .into_any_element()
    }

    fn render_setup(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let creating = self.setup == Setup::Create;
        let ready = !self.busy && !self.switch_blocked(cx);
        let mut form = widgets::section_card(theme).p(px(20.0)).gap(px(16.0))
            .child(div()
                .child(widgets::row_title(theme, if creating { "Create a private workspace" } else { "Join a private workspace" }).text_size(ui_rems(16.0)))
                .child(widgets::page_subtitle(theme, if creating {
                    "This computer will host the workspace. Keep it on so your devices can connect."
                } else {
                    "Connect Tailscale, then enter the invitation from your workspace's host."
                })))
            .child(div().flex().flex_col().gap(px(8.0))
                .child(widgets::field_label(theme, if creating { "Workspace name" } else { "This device's name" }))
                .child(popover::dialog_field(self.name.clone().into_any_element())));
        if !creating {
            form = form
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(widgets::field_label(theme, "Hub address"))
                        .child(popover::dialog_field(self.hub.clone().into_any_element())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(widgets::field_label(theme, "Pairing code"))
                        .child(popover::dialog_field(self.code.clone().into_any_element())),
                );
        }
        form = form.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(widgets::field_label(
                    theme,
                    if creating {
                        "How will this computer be used?"
                    } else {
                        "Role from your invitation"
                    },
                ))
                .child(self.role_picker(theme, false, cx)),
        );
        if self.state.read(cx).workspace_scope == Some(WorkspaceScope::Local) {
            form = form.child(div().id("private-bring-work").flex().items_center().gap(px(14.0))
                .border_t_1().border_color(theme.border).pt(px(16.0)).cursor_pointer()
                .child(div().flex_1().min_w_0()
                    .child(widgets::row_title(theme, "Bring my local work"))
                    .child(widgets::page_subtitle(theme, "Copy projects and conversations. Your original local workspace stays separate.").text_size(ui_rems(12.0))))
                .child(widgets::toggle_switch(theme, self.bring_work))
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.busy { this.bring_work = !this.bring_work; cx.notify(); }
                })));
        }
        form = form.child(
            widgets::page_subtitle(
                theme,
                if creating {
                    "Before creating: connect Tailscale and enable HTTPS for your tailnet."
                } else {
                    "Both devices must be connected to the same Tailscale network."
                },
            )
            .text_size(ui_rems(12.0)),
        );
        form.child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.0))
                .child(self.button(
                    theme,
                    "private-submit",
                    if self.busy {
                        "Setting up…"
                    } else if creating {
                        "Create workspace"
                    } else {
                        "Join workspace"
                    },
                    ButtonStyle::Primary,
                    ready,
                    |this, cx| this.configure(cx),
                    cx,
                ))
                .child(self.button(
                    theme,
                    "private-setup-cancel",
                    "Cancel",
                    ButtonStyle::Quiet,
                    !self.busy,
                    |this, cx| {
                        this.setup = Setup::Choose;
                        this.code.update(cx, |input, cx| input.set_text("", cx));
                        cx.notify();
                    },
                    cx,
                )),
        )
        .into_any_element()
    }

    fn render_invitation(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let invitation = self.invitation.as_ref()?;
        if invitation.expires_at <= Utc::now() {
            return Some(
                widgets::warning_strip(
                    theme,
                    "This invitation expired. Create a new one to pair your device.",
                )
                .into_any_element(),
            );
        }
        let link = invitation_link(invitation);
        let code = invitation.code.clone();
        let address = invitation.hub_url.clone();
        let instructions = div().flex_1().min_w(px(210.0)).flex().flex_col().gap(px(10.0))
            .child(widgets::row_title(theme, "On the other device"))
            .child(widgets::page_subtitle(theme, "Open Zeron and choose Private via Tailscale. Scan this QR code or enter the address and code below.").mt_0())
            .child(div().flex().items_center().gap(px(8.0))
                .child(div().flex_1().min_w_0().child(widgets::field_label(theme, "Hub address"))
                    .child(widgets::page_subtitle(theme, address.clone()).text_size(ui_rems(12.0)).truncate()))
                .child(self.button(theme, "private-copy-address", "Copy", ButtonStyle::Secondary, !self.busy, move |this, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(address.clone()));
                    this.notice = Some("Hub address copied.".into());
                    cx.notify();
                }, cx)))
            .child(div().flex().items_center().gap(px(12.0))
                .child(div().flex_1().min_w_0().child(widgets::field_label(theme, "Pairing code"))
                    .child(div().text_size(ui_rems(28.0)).font_family(theme.font_mono.clone()).text_color(theme.text).child(code.clone())))
                .child(self.button(theme, "private-copy-code", "Copy", ButtonStyle::Secondary, !self.busy, move |this, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                    this.notice = Some("Pairing code copied.".into());
                    cx.notify();
                }, cx)))
            .child(widgets::page_subtitle(theme, format!("One use. Expires at {}.", invitation.expires_at.with_timezone(&chrono::Local).format("%H:%M"))).text_size(ui_rems(12.0)))
            .child(div().flex().child(self.button(theme, "private-copy-invitation", "Copy invitation link", ButtonStyle::Secondary, !self.busy, move |this, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
                this.notice = Some("Invitation link copied.".into());
                cx.notify();
            }, cx)));
        let mut card = div()
            .border_t_1()
            .border_color(theme.border)
            .pt(px(20.0))
            .mt(px(4.0))
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(20.0));
        match qrcode::QrCode::new(invitation_link(invitation).as_bytes()) {
            Ok(qr) => {
                card = card.child(
                    div()
                        .flex_none()
                        .p(px(8.0))
                        .rounded(px(8.0))
                        .bg(gpui::rgb(0xffffff))
                        .child(qr_code(qr)),
                )
            }
            Err(error) => {
                card = card.child(widgets::error_strip(
                    theme,
                    format!(
                        "Could not render QR code: {error}. Enter the address and code instead."
                    ),
                ))
            }
        }
        Some(card.child(instructions).into_any_element())
    }

    fn render_status(
        &self,
        status: PrivateStatus,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let PrivateStatus::Configured {
            workspace_id,
            hub_url,
            name,
            role,
            host_hub,
            enabled,
            ..
        } = status
        else {
            return self.render_unconfigured(theme, cx);
        };
        let (scope, connectivity, background) = {
            let state = self.state.read(cx);
            (
                state.workspace_scope,
                state.connectivity.state,
                state.engine().is_some_and(|engine| {
                    matches!(engine.mode(), crate::state::EngineMode::Remote { .. })
                }),
            )
        };
        let current = scope == Some(WorkspaceScope::Private);
        let (connection, color) = if !enabled {
            ("Access paused", theme.text_muted)
        } else if !current {
            ("Finish setup", theme.warning)
        } else {
            match connectivity {
                ConnectivityState::Connected => ("Connected", theme.success),
                ConnectivityState::Offline => ("Offline", theme.warning),
                ConnectivityState::Reconnecting | ConnectivityState::Disabled => {
                    ("Connecting", theme.warning)
                }
            }
        };
        let mut summary = widgets::section_card(theme)
            .child(div().p(px(20.0)).flex().flex_col().gap(px(8.0))
                .child(div().flex().flex_wrap().items_center().justify_between().gap(px(8.0))
                    .child(div().flex().items_center().gap(px(8.0)).text_color(theme.text_muted)
                        .child(icons::icon(icons::KEY_MINIMALISTIC).size(px(14.0)).text_color(theme.text_muted))
                        .child(widgets::field_label(theme, "Private via Tailscale").text_color(theme.text_muted)))
                    .child(widgets::badge(theme, connection).border_color(color.opacity(0.2)).bg(color.opacity(0.08)).text_color(color)))
                .child(div().text_size(ui_rems(20.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme.text).child(name))
                .child(widgets::page_subtitle(theme, match (host_hub, role) {
                    (true, NodeRole::Server) => "This computer hosts the workspace and runs your agents.",
                    (true, NodeRole::Client) => "This computer hosts the workspace. Agents run on your other servers.",
                    (false, NodeRole::Server) => "This computer runs agents for devices in the workspace.",
                    (false, NodeRole::Client) => "This device controls agents running on your servers.",
                }).mt_0()));
        if current {
            summary = summary.child(widgets::card_row(theme, false).flex_wrap()
                .child(div().flex_1().min_w(px(180.0))
                    .child(widgets::row_title(theme, if background { "Running in background" } else { "Keep available when you close Zeron" }))
                    .child(widgets::page_subtitle(theme, if background {
                        "You can close this window. Keep the computer awake for remote access."
                    } else if cfg!(any(target_os = "linux", target_os = "macos")) {
                        "Start in the background when you sign in."
                    } else { "Keep this window open for remote access." }).text_size(ui_rems(12.0))))
                .when(background, |el| el.child(widgets::badge_active(theme, "Enabled")))
                .when(!background && cfg!(any(target_os = "linux", target_os = "macos")), |el| {
                    el.child(self.button(theme, "private-background", "Run in background", ButtonStyle::Secondary,
                        !self.busy && !self.switch_blocked(cx), |_, cx| cx.emit(WorkspaceEvent::RunInBackground), cx))
                }));
        } else {
            summary = summary.child(
                widgets::card_row(theme, false)
                    .flex_wrap()
                    .child(
                        widgets::page_subtitle(
                            theme,
                            "Your workspace is saved. Finish switching to connect.",
                        )
                        .flex_1()
                        .min_w(px(180.0)),
                    )
                    .child(self.button(
                        theme,
                        "private-retry-switch",
                        "Finish setup",
                        ButtonStyle::Primary,
                        !self.busy && !self.switch_blocked(cx),
                        |this, cx| {
                            cx.emit(WorkspaceEvent::Restart {
                                target: WorkspaceScope::Private,
                                import: this.bring_work,
                            })
                        },
                        cx,
                    )),
            );
        }
        if host_hub && !enabled {
            summary = summary.child(
                widgets::card_row(theme, false)
                    .flex_wrap()
                    .child(
                        widgets::page_subtitle(
                            theme,
                            "Private access is paused. Other devices cannot connect.",
                        )
                        .flex_1()
                        .min_w(px(180.0)),
                    )
                    .child(self.button(
                        theme,
                        "private-access-resume",
                        "Enable private access",
                        ButtonStyle::Primary,
                        !self.busy,
                        |this, cx| {
                            this.call(
                                methods::SET_PRIVATE_ACCESS_ENABLED,
                                json!({"enabled": true}),
                                None,
                                cx,
                            )
                        },
                        cx,
                    )),
            );
        }
        let mut content = div().child(summary);
        if host_hub && enabled {
            let invite_live = self
                .invitation
                .as_ref()
                .is_some_and(|invite| invite.expires_at > Utc::now());
            content = content.child(widgets::section_card(theme).p(px(20.0)).gap(px(16.0))
                .child(div().child(widgets::row_title(theme, "Add a device").text_size(ui_rems(15.0)))
                    .child(widgets::page_subtitle(theme, "Connect it to the same Tailscale network, then choose what it can do.")))
                .child(self.role_picker(theme, true, cx))
                .children(self.render_invitation(theme, cx))
                .child(div().flex().items_center().flex_wrap().gap(px(12.0))
                    .child(self.button(theme, "private-create-invitation", if self.busy { "Working…" } else if invite_live { "Replace invitation" } else { "Create invitation" },
                        if invite_live { ButtonStyle::Secondary } else { ButtonStyle::Primary }, !self.busy,
                        |this, cx| this.call(methods::CREATE_PRIVATE_INVITATION, json!({"role": this.invitation_role.wire()}), None, cx), cx))
                    .child(widgets::page_subtitle(theme, if invite_live { "Replacing it invalidates the previous code." } else { "Valid for 5 minutes. Works once." }).mt_0().text_size(ui_rems(12.0)))));
        }
        content = content.child(self.devices.clone());
        if !host_hub {
            content = content.child(
                widgets::page_subtitle(
                    theme,
                    "Create invitations and manage access on the workspace's host.",
                )
                .mt(px(12.0)),
            );
        }
        let mut details = div().mt(px(20.0)).child(
            div()
                .id("private-details")
                .flex()
                .items_center()
                .gap(px(8.0))
                .py(px(8.0))
                .cursor_pointer()
                .text_color(theme.text_muted)
                .text_size(ui_rems(12.0))
                .child(
                    icons::icon(if self.show_details {
                        icons::ALT_ARROW_DOWN
                    } else {
                        icons::ALT_ARROW_RIGHT
                    })
                    .size(px(14.0))
                    .text_color(theme.text_muted),
                )
                .child("Connection details")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_details = !this.show_details;
                    cx.notify();
                })),
        );
        if self.show_details {
            details = details.child(widgets::section_card(theme).mt(px(8.0)).p(px(20.0)).gap(px(14.0))
                .child(div().flex().items_center().gap(px(12.0))
                    .child(div().flex_1().min_w_0().child(widgets::field_label(theme, "Hub address"))
                        .child(widgets::page_subtitle(theme, hub_url.clone()).truncate()))
                    .child(self.button(theme, "private-copy-hub", "Copy address", ButtonStyle::Secondary, !self.busy, move |this, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(hub_url.clone()));
                        this.notice = Some("Hub address copied.".into()); cx.notify();
                    }, cx)))
                .child(div().child(widgets::field_label(theme, "Workspace ID"))
                    .child(widgets::page_subtitle(theme, workspace_id).text_size(ui_rems(12.0)).truncate()))
                .when(host_hub, |el| el.child(div().flex().child(self.button(theme, "private-access-toggle",
                    if enabled { "Disable private access" } else { "Enable private access" }, ButtonStyle::Secondary, !self.busy,
                    move |this, cx| this.call(methods::SET_PRIVATE_ACCESS_ENABLED, json!({"enabled": !enabled}), None, cx), cx))))
                .child(if self.confirm_leave {
                    div().border_t_1().border_color(theme.border).pt(px(14.0))
                        .child(widgets::page_subtitle(theme, if host_hub {
                            "Leaving stops remote access for this workspace. Saved data stays on this computer."
                        } else { "Leave this workspace and return to local work? Saved data stays on this device." }))
                        .child(div().flex().gap(px(8.0)).mt(px(12.0))
                            .child(self.button(theme, "private-confirm-leave", "Leave and use Local", ButtonStyle::Secondary, !self.busy && !self.switch_blocked(cx),
                                |this, cx| this.call(methods::LEAVE_PRIVATE_WORKSPACE, json!({}), Some((WorkspaceScope::Local, false)), cx), cx))
                            .child(self.button(theme, "private-cancel-leave", "Cancel", ButtonStyle::Quiet, !self.busy,
                                |this, cx| { this.confirm_leave = false; cx.notify(); }, cx)))
                } else {
                    div().flex().child(self.button(theme, "private-leave", "Leave private workspace", ButtonStyle::Quiet, !self.busy,
                        |this, cx| { this.confirm_leave = true; cx.notify(); }, cx))
                }));
        }
        content.child(details).into_any_element()
    }

    fn render_unconfigured(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.state.read(cx).workspace_scope == Some(WorkspaceScope::Private) {
            return widgets::section_card(theme)
                .p(px(20.0))
                .gap(px(12.0))
                .child(widgets::page_subtitle(
                    theme,
                    "Private configuration was removed. Finish switching to Local.",
                ))
                .child(div().flex().child(self.button(
                    theme,
                    "private-retry-local",
                    "Switch to Local",
                    ButtonStyle::Primary,
                    !self.busy && !self.switch_blocked(cx),
                    |_, cx| {
                        cx.emit(WorkspaceEvent::Restart {
                            target: WorkspaceScope::Local,
                            import: false,
                        })
                    },
                    cx,
                )))
                .into_any_element();
        }
        if self.setup != Setup::Choose {
            return self.render_setup(theme, cx);
        }
        let local = self.state.read(cx).workspace_scope == Some(WorkspaceScope::Local);
        widgets::section_card(theme).p(px(20.0)).gap(px(16.0))
            .child(div().flex().items_center().gap(px(12.0))
                .child(widgets::row_tile(theme, icons::KEY_MINIMALISTIC))
                .child(div().flex_1().min_w_0()
                    .child(widgets::row_title(theme, "Private via Tailscale").text_size(ui_rems(15.0)))
                    .child(widgets::page_subtitle(theme, "Sync through a computer you control. No Zeron account needed."))))
            .child(widgets::page_subtitle(theme, "Connect your devices to the same Tailscale network. Create a workspace on the computer that stays on, or join one with an invitation."))
            .when(!local, |el| el.child(widgets::warning_strip(theme, "Switch to Local before setting up a private workspace.")))
            .child(div().flex().flex_wrap().gap(px(8.0))
                .child(self.button(theme, "private-create", "Create private workspace", ButtonStyle::Primary, local && !self.busy,
                    |this, cx| { this.setup = Setup::Create; this.role = NodeRole::Server; this.error = None; cx.notify(); }, cx))
                .child(self.button(theme, "private-join", "Join a workspace", ButtonStyle::Secondary, local && !self.busy,
                    |this, cx| { this.setup = Setup::Join; this.role = NodeRole::Client; this.error = None; cx.notify(); }, cx)))
            .into_any_element()
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

fn qr_code(qr: qrcode::QrCode) -> AnyElement {
    let width = qr.width();
    let module = (216.0 / (width + 8) as f32).floor();
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.paint_quad(gpui::fill(bounds, gpui::rgb(0xffffff)));
            for y in 0..width {
                for x in 0..width {
                    if qr[(x, y)] == qrcode::Color::Dark {
                        let origin = bounds.origin
                            + gpui::point(px((x + 4) as f32 * module), px((y + 4) as f32 * module));
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(origin, gpui::size(px(module), px(module))),
                            gpui::rgb(0x000000),
                        ));
                    }
                }
            }
        },
    )
    .size(px((width + 8) as f32 * module))
    .into_any_element()
}

impl popover::ScrollRailHost for WorkspacePage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }
    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for WorkspacePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let scope = self.state.read(cx).workspace_scope;
        let local = scope == Some(WorkspaceScope::Local);
        let cloud = scope == Some(WorkspaceScope::Synced);
        let active = self.switch_blocked(cx);
        let configured = matches!(
            &self.status,
            Loadable::Ready(PrivateStatus::Configured { .. })
        );
        let paired = match &self.status {
            Loadable::Ready(PrivateStatus::Configured {
                host_hub: true,
                nodes,
                ..
            }) => Some(nodes.clone()),
            _ => None,
        };
        self.devices.update(cx, |devices, cx| {
            devices.set_membership(paired, self.busy, cx)
        });
        let dialog = self.devices.update(cx, |devices, cx| {
            devices.render_rename_dialog(window.viewport_size(), cx)
        });
        let private = match self.status.clone() {
            Loadable::Idle | Loadable::Loading => {
                widgets::page_subtitle(&theme, "Loading workspace settings…")
                    .mt(px(24.0))
                    .into_any_element()
            }
            Loadable::Error(error) => widgets::error_strip(&theme, error).into_any_element(),
            Loadable::Ready(status) => self.render_status(status, &theme, cx),
        };
        let mut content = widgets::page_column()
            .child(div().flex().items_center().justify_between()
                .child(widgets::page_header(&theme, "Workspace", None))
                .child(self.button(&theme, "workspace-status-refresh", "Refresh", ButtonStyle::Quiet, !self.busy, |this, cx| this.load(cx), cx)))
            .child(widgets::page_subtitle(&theme, "Connect your devices and choose where work syncs."))
            .when_some(self.error.clone(), |el, error| el.child(widgets::error_strip(&theme, error)))
            .when_some(self.notice.clone(), |el, notice| el.child(div().mt(px(16.0)).p(px(12.0)).rounded(px(8.0))
                .bg(theme.success.opacity(0.08)).text_color(theme.success).text_size(ui_rems(12.0)).child(notice)))
            .when(active, |el| el.child(widgets::warning_strip(&theme, "Finish active agent turns and save or close edited files before switching workspaces.")));
        if !configured && self.setup == Setup::Choose && (local || cloud) {
            content = content.child(widgets::section_card(&theme)
                .child(widgets::card_row(&theme, true)
                    .child(widgets::row_tile(&theme, if cloud { icons::CLOUD } else { icons::LAPTOP }))
                    .child(div().flex_1().min_w_0()
                        .child(widgets::row_title(&theme, if cloud { "Zeron Cloud" } else { "Local workspace" }).text_size(ui_rems(15.0)))
                        .child(widgets::page_subtitle(&theme, if cloud { "Your workspace syncs through Zeron's hosted service." } else { "Projects and conversations stay on this device." }).text_size(ui_rems(12.0))))
                    .child(widgets::badge(&theme, "Current")))
                .when(cloud, |el| el.child(widgets::card_row(&theme, false)
                    .child(widgets::page_subtitle(&theme, "Switch to Local to set up private sync.").flex_1())
                    .child(self.button(&theme, "workspace-use-local", "Use Local", ButtonStyle::Secondary, !self.busy && !active, |_, cx| cx.emit(WorkspaceEvent::LeaveCloud), cx)))));
        }
        content = content.child(private);
        if !configured && self.setup == Setup::Choose && (local || cloud) {
            content = content.child(self.devices.clone());
        }
        if configured {
            content = content.child(
                div()
                    .id("workspace-other-modes")
                    .mt(px(8.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .text_size(ui_rems(12.0))
                    .child(
                        icons::icon(if self.show_modes {
                            icons::ALT_ARROW_DOWN
                        } else {
                            icons::ALT_ARROW_RIGHT
                        })
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                    )
                    .child("Other workspace options")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_modes = !this.show_modes;
                        cx.notify();
                    })),
            );
        }
        if (!configured && self.setup == Setup::Choose) || self.show_modes {
            let alternatives = widgets::section_card(&theme).mt(px(12.0))
                .when(configured, |el| el.child(widgets::card_row(&theme, true)
                    .child(widgets::row_tile(&theme, icons::LAPTOP))
                    .child(div().flex_1().min_w_0().child(widgets::row_title(&theme, "Local"))
                        .child(widgets::page_subtitle(&theme, "Leave the private workspace in Connection details to work only on this device.").text_size(ui_rems(12.0))))))
                .when(!cloud, |el| el.child(widgets::card_row(&theme, !configured).flex_wrap()
                    .child(widgets::row_tile(&theme, icons::CLOUD))
                    .child(div().flex_1().min_w(px(180.0)).child(widgets::row_title(&theme, "Zeron Cloud"))
                        .child(widgets::page_subtitle(&theme, if configured { "Leave the private workspace before signing in to Zeron Cloud." } else { "Sign in to sync through Zeron's hosted service." }).text_size(ui_rems(12.0))))
                    .when(local, |el| el.child(self.button(&theme, "workspace-use-cloud", "Set up Zeron Cloud", ButtonStyle::Secondary, !self.busy && !active, |_, cx| cx.emit(WorkspaceEvent::StartCloud), cx)))));
            content = content.child(alternatives);
        }
        let scrollbar = popover::rail(self, "workspace-scrollbar", &theme, cx);
        div()
            .id("workspace-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("workspace-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(content),
            )
            .children(scrollbar)
            .children(dialog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invitation_link_preserves_hub_and_code_without_credentials() {
        let invitation = Invitation {
            code: "012345".into(),
            expires_at: Utc::now(),
            hub_url: "https://host.example.ts.net:8443".into(),
        };
        assert_eq!(
            invitation_link(&invitation),
            "zeron://private/join?hub=https%3A%2F%2Fhost.example.ts.net%3A8443&code=012345"
        );
    }

    #[test]
    fn status_requires_known_roles_and_complete_configuration() {
        assert!(serde_json::from_value::<PrivateStatus>(json!({"state":"unconfigured"})).is_ok());
        assert!(
            serde_json::from_value::<PrivateStatus>(
                json!({"state":"configured","workspaceId":"a"})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<NodeRole>(json!("administrator")).is_err());
    }

    #[test]
    fn status_and_invitation_accept_hub_millisecond_timestamps() {
        let status: PrivateStatus = serde_json::from_value(json!({
            "state":"configured", "workspaceId":"w", "hubUrl":"https://hub.example.ts.net:8443", "name":"Workspace", "role":"server", "hostHub":true, "enabled":true,
            "nodes":[{"deviceId":"client", "name":"Client", "role":"client", "pairedAt":1_700_000_000_000_i64}]
        })).unwrap();
        let PrivateStatus::Configured { nodes, .. } = status else {
            panic!("expected configured workspace");
        };
        assert_eq!(nodes[0].paired_at.timestamp_millis(), 1_700_000_000_000);
        let invite: Invitation = serde_json::from_value(json!({"code":"012345","expiresAt":1_700_000_300_000_i64,"hubUrl":"https://hub.example.ts.net:8443"})).unwrap();
        assert_eq!(invite.expires_at.timestamp_millis(), 1_700_000_300_000);
    }
}
