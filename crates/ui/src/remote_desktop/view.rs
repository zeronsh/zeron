use super::{
    profiles::{Profile, ViewMode},
    surface::RemoteDesktopSurface,
};
use crate::{settings, theme::Theme};
use gpui::{prelude::*, *};
use gpui_base::input::{Input, InputState};

pub(super) struct EditForm {
    pub original: Profile,
    pub fields: Vec<Entity<InputState>>,
    pub remember: bool,
}
const LABELS: [&str; 8] = [
    "Name",
    "Host",
    "Port",
    "Username",
    "Domain (optional)",
    "Width",
    "Height",
    "Password",
];
impl EditForm {
    pub fn new(profile: Profile, window: &mut Window, cx: &mut App) -> Self {
        let values = [
            profile.name.clone(),
            profile.host.clone(),
            profile.port.to_string(),
            profile.username.clone(),
            profile.domain.clone().unwrap_or_default(),
            profile.desktop_width.to_string(),
            profile.desktop_height.to_string(),
            String::new(),
        ];
        let fields = values
            .into_iter()
            .enumerate()
            .map(|(i, value)| {
                cx.new(|cx| {
                    let mut input = InputState::new(window, cx).masked(i == 7);
                    input.set_value(value, window, cx);
                    input
                })
            })
            .collect();
        Self {
            remember: profile.remember_password,
            original: profile,
            fields,
        }
    }
    pub fn value(&self, cx: &App) -> Result<(Profile, String), String> {
        let values: Vec<String> = self
            .fields
            .iter()
            .map(|f| f.read(cx).value().to_string())
            .collect();
        let mut profile = self.original.clone();
        profile.name = values[0].trim().into();
        profile.host = values[1].trim().into();
        profile.port = values[2]
            .parse()
            .map_err(|_| "Port must be between 1 and 65535")?;
        profile.username = values[3].trim().into();
        profile.domain = (!values[4].trim().is_empty()).then(|| values[4].trim().into());
        profile.desktop_width = values[5]
            .parse()
            .map_err(|_| "Enter a valid desktop width")?;
        profile.desktop_height = values[6]
            .parse()
            .map_err(|_| "Enter a valid desktop height")?;
        profile.remember_password = self.remember;
        profile.validate()?;
        Ok((profile, values[7].clone()))
    }
}
impl RemoteDesktopSurface {
    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.global::<Theme>().clone();
        let mut root = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(theme.text)
            .text_size(px(12.));
        if let Some(form) = &self.form {
            let mut body = div()
                .id("rdp-form")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(10.));
            body = body.child("Remote Desktop connection");
            for (i, field) in form.fields.iter().enumerate() {
                body = body.child(
                    div().flex().flex_col().gap(px(4.)).child(LABELS[i]).child(
                        div()
                            .h(px(30.))
                            .px(px(8.))
                            .border_1()
                            .border_color(theme.border)
                            .rounded(px(5.))
                            .child(Input::new(field)),
                    ),
                );
            }
            let layout = form.original.keyboard_layout;
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(5.))
                    .child("Keyboard layout")
                    .children(
                        [
                            ("rdp-layout-us", "English (US)", 0x0409),
                            ("rdp-layout-es", "Spanish (Spain)", 0x040a),
                            ("rdp-layout-latam", "Spanish (Latin America)", 0x080a),
                        ]
                        .into_iter()
                        .map(|(id, label, value)| {
                            action(
                                id,
                                format!("{}{}", if layout == value { "● " } else { "" }, label),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(form) = &mut this.form {
                                        form.original.keyboard_layout = value;
                                    }
                                    cx.notify();
                                },
                            ))
                        }),
                    ),
            );
            let remember = form.remember;
            body = body.child(
                action(
                    "rdp-remember",
                    if remember {
                        "☑ Remember password in system keyring"
                    } else {
                        "☐ Remember password in system keyring"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(form) = &mut this.form {
                        form.remember = !form.remember;
                    }
                    cx.notify();
                })),
            );
            body = body.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        action("rdp-save", "Save").on_click(
                            cx.listener(|this, _, w, cx| this.save_profile(false, w, cx)),
                        ),
                    )
                    .child(
                        action("rdp-save-connect", "Save and connect")
                            .on_click(cx.listener(|this, _, w, cx| this.save_profile(true, w, cx))),
                    )
                    .child(action("rdp-cancel-form", "Cancel").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.form = None;
                            cx.notify();
                        },
                    ))),
            );
            root = root.child(body);
        } else if self.profile.is_none() {
            let mut body =
                div()
                    .id("rdp-profiles")
                    .flex_1()
                    .overflow_y_scroll()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child("Remote Desktop")
                    .child(div().text_color(theme.text_muted).child(
                        "Connect directly to a Windows or Linux machine with an RDP server.",
                    ));
            for profile in settings::current(cx).remote_desktop_profiles {
                let id = profile.id;
                let edit = profile.clone();
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .p(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .rounded(px(6.))
                        .child(profile.name.clone())
                        .child(div().text_color(theme.text_muted).child(profile.endpoint()))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(
                                    action(
                                        SharedString::from(format!("rdp-connect-{id}")),
                                        "Connect",
                                    )
                                    .on_click(cx.listener(
                                        move |_, _, _, cx| {
                                            cx.emit(super::surface::SurfaceEvent::OpenProfile(id))
                                        },
                                    )),
                                )
                                .child(
                                    action(SharedString::from(format!("rdp-edit-{id}")), "Edit")
                                        .on_click(cx.listener(move |this, _, w, cx| {
                                            this.form = Some(EditForm::new(edit.clone(), w, cx));
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    action(
                                        SharedString::from(format!("rdp-delete-{id}")),
                                        "Delete",
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, w, cx| this.delete_profile(id, w, cx),
                                    )),
                                ),
                        ),
                );
            }
            body = body.child(action("rdp-add", "Add connection").on_click(cx.listener(
                |this, _, w, cx| {
                    this.form = Some(EditForm::new(Profile::default(), w, cx));
                    cx.notify();
                },
            )));
            root = root.child(body);
        } else {
            let running = self.snapshot.state.is_running();
            let connected = self.snapshot.state == zeron_rdp::SessionState::Connected;
            let name = self.profile.as_ref().unwrap().name.clone();
            let mut toolbar =
                crate::surface_chrome::toolbar(&theme).child(div().flex_1().truncate().child(name));
            toolbar = toolbar.child(
                action(
                    "rdp-connection-action",
                    if running { "Disconnect" } else { "Reconnect" },
                )
                .on_click(cx.listener(move |this, _, w, cx| {
                    if running {
                        this.disconnect(w, cx);
                    } else {
                        this.begin_connect(w, cx);
                    }
                })),
            );
            if connected {
                toolbar = toolbar
                    .child(
                        action("rdp-release", "Release keyboard").on_click(cx.listener(
                            |this, _, w, cx| {
                                this.desktop.update(cx, |d, cx| d.release_capture(w, cx));
                            },
                        )),
                    )
                    .child(action("rdp-cad", "Ctrl+Alt+Del").on_click(cx.listener(
                        |this, _, _, cx| this.send(zeron_rdp::Command::CtrlAltDelete, cx),
                    )));
            } else if !running {
                toolbar = toolbar.child(action("rdp-list", "Connections").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.profile = None;
                        cx.emit(super::surface::SurfaceEvent::Changed);
                        cx.notify();
                    },
                )));
            }
            root = root.child(toolbar);
            let status = match &self.snapshot.state {
                zeron_rdp::SessionState::Idle => {
                    "Enter a password or connect using your saved password".to_string()
                }
                zeron_rdp::SessionState::Failed(e) => e.to_string(),
                other => format!("{other:?}"),
            };
            root = root.child(
                div()
                    .px(px(10.))
                    .py(px(5.))
                    .text_color(theme.text_muted)
                    .child(format!(
                        "{} · {}",
                        self.profile.as_ref().unwrap().endpoint(),
                        status
                    )),
            );
            if let Some(challenge) = &self.snapshot.certificate {
                root = root.child(
                    div()
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child("Verify the remote server certificate")
                        .child(challenge.endpoint.clone())
                        .child(challenge.reason.clone())
                        .child(format!(
                            "SHA-256: {}",
                            display_fingerprint(&challenge.sha256)
                        ))
                        .children(challenge.previous_sha256.as_ref().map(|old| {
                            div().child(format!("Previous SHA-256: {}", display_fingerprint(old)))
                        }))
                        .child(
                            div().flex().flex_wrap().gap(px(8.)).children(
                                [
                                    (
                                        "rdp-cert-cancel",
                                        "Cancel",
                                        zeron_rdp::CertificateDecision::Reject,
                                    ),
                                    (
                                        "rdp-cert-once",
                                        "Trust once",
                                        zeron_rdp::CertificateDecision::TrustOnce,
                                    ),
                                    (
                                        "rdp-cert-save",
                                        "Trust and save fingerprint",
                                        zeron_rdp::CertificateDecision::SavePin,
                                    ),
                                ]
                                .into_iter()
                                .map(|(id, label, decision)| {
                                    action(id, label).on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.send(zeron_rdp::Command::Certificate(decision), cx)
                                        },
                                    ))
                                }),
                            ),
                        ),
                );
            } else if !running {
                root = root.child(
                    div()
                        .p(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child("Password (used only for this connection)")
                        .child(
                            div()
                                .h(px(30.))
                                .border_1()
                                .border_color(theme.border)
                                .child(Input::new(&self.password)),
                        ),
                );
            }
            if connected {
                let mode = self.desktop.read(cx).mode;
                root = root.child(
                    div().px(px(8.)).py(px(4.)).flex().gap(px(8.)).child(
                        action(
                            "rdp-fit",
                            if mode == ViewMode::Fit {
                                "View: Fit"
                            } else {
                                "View: 1:1"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_view(cx))),
                    ),
                );
            }
            if connected {
                let resize = self.profile.as_ref().is_some_and(|p| p.resize_remote);
                root = root.child(
                    div()
                        .px(px(8.))
                        .flex()
                        .flex_wrap()
                        .gap(px(5.))
                        .child(
                            action(
                                "rdp-resize",
                                if resize {
                                    "Resolution: follow panel"
                                } else {
                                    "Resolution: fixed"
                                },
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_resize(cx))),
                        )
                        .when(resize && !self.snapshot.capabilities.resize, |el| {
                            el.child("Server resize unavailable; scaling locally")
                        }),
                );
                if self.desktop.read(cx).mode == ViewMode::ActualSize {
                    root = root.child(
                        div().px(px(8.)).flex().gap(px(6.)).child("Pan").children(
                            [
                                ("rdp-pan-left", "←", -160., 0.),
                                ("rdp-pan-right", "→", 160., 0.),
                                ("rdp-pan-up", "↑", 0., -160.),
                                ("rdp-pan-down", "↓", 0., 160.),
                            ]
                            .into_iter()
                            .map(|(id, label, x, y)| {
                                action(id, label).on_click(
                                    cx.listener(move |this, _, _, cx| this.pan_desktop(x, y, cx)),
                                )
                            }),
                        ),
                    );
                }
            }
            if connected && self.snapshot.capabilities.clipboard_text {
                root = root.child(
                    div()
                        .px(px(8.))
                        .flex()
                        .flex_wrap()
                        .gap(px(5.))
                        .child(
                            action("rdp-send-clipboard", "Send clipboard text")
                                .on_click(cx.listener(|this, _, _, cx| this.send_clipboard(cx))),
                        )
                        .child(
                            action("rdp-copy-clipboard", "Copy remote text").on_click(
                                cx.listener(|this, _, w, cx| this.copy_remote_text(w, cx)),
                            ),
                        ),
                );
            }
            root = root.child(div().flex_1().min_h_0().child(self.desktop.clone()));
        }
        if let Some(notice) = &self.notice {
            root = root.child(div().p(px(8.)).child(notice.clone()));
        }
        if !settings::current(cx)
            .remote_desktop_credential_cleanup
            .is_empty()
        {
            root = root.child(
                action("rdp-cleanup", "Retry removing saved credentials")
                    .on_click(cx.listener(|this, _, w, cx| this.retry_cleanup(w, cx))),
            );
        }
        let _ = window;
        root.into_any_element()
    }
}
pub(super) fn action(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(7.))
        .py(px(4.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(crate::theme::ink(0.08)))
        .child(label.into())
}

fn display_fingerprint(value: &str) -> String {
    value
        .chars()
        .collect::<Vec<_>>()
        .chunks(8)
        .map(|part| part.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}
