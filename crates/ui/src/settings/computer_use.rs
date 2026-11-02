//! Settings → Computer use: turn agent desktop control on or off,
//! grant the macOS permissions the bundled cua-driver needs, and pick the
//! agent cursor's colors.
//!
//! Permissions are checked in this process on purpose: agents' MCP bridges
//! (and the driver and helpers they spawn) are descendants of Zeron.app, so
//! macOS attributes Accessibility, Screen Recording and Input Monitoring to
//! Zeron itself.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AnyElement, Context, SharedString, Task, Window, div, prelude::*, px};

use crate::appshots::CapabilityState;
use crate::settings::widgets;
use crate::theme::Theme;

#[cfg(target_os = "macos")]
const AUTOMATION_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation";
#[cfg(target_os = "macos")]
const SCREEN_RECORDING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
#[cfg(target_os = "macos")]
const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
#[cfg(target_os = "macos")]
const INPUT_MONITORING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";

/// Input Monitoring is what lets the Escape stop hear a physical key press
/// while another app is focused (a listen-only event tap).
#[cfg(target_os = "macos")]
mod listen {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }

    pub fn granted() -> bool {
        // SAFETY: argument-less TCC queries, callable from any thread.
        unsafe { CGPreflightListenEventAccess() }
    }

    pub fn request() {
        // SAFETY: as above; shows the system prompt the first time.
        unsafe {
            CGRequestListenEventAccess();
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod listen {
    pub fn granted() -> bool {
        false
    }

    pub fn request() {}
}

/// Agent cursor theme presets: name, theme id, and the swatch colors the tile
/// previews. `None` is the driver's stock `cua.default` pointer.
const CURSOR_PRESETS: [(&str, Option<CursorTheme>); 2] = [
    ("Stock", None),
    (
        "Aurora",
        Some(CursorTheme {
            id: zeron_mcp::computer::CURSOR_THEME_ID,
            fill: 0x3edaa8,
            rim: 0x8f5df0,
        }),
    ),
];

#[derive(Clone, Copy)]
struct CursorTheme {
    id: &'static str,
    fill: u32,
    rim: u32,
}

/// The preset matching the saved theme, if any (a hand-edited file may match none).
fn preset_index(style: &Option<zeron_mcp::computer::CursorStyle>) -> Option<usize> {
    CURSOR_PRESETS
        .iter()
        .position(|(_, theme)| match (theme, style) {
            (None, None) => true,
            (Some(theme), Some(style)) => style.theme.as_deref() == Some(theme.id),
            _ => false,
        })
}

/// How often the page re-reads permission state while it is open, so a
/// grant made in System Settings shows up without a manual refresh.
const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Permissions {
    accessibility: bool,
    screen_recording: bool,
    input_monitoring: bool,
}

impl Permissions {
    fn read() -> Self {
        let capabilities = crate::appshots::capabilities();
        Self {
            accessibility: capabilities.application_text == CapabilityState::Ready,
            screen_recording: capabilities.window_capture == CapabilityState::Ready,
            input_monitoring: listen::granted(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Driver {
    Checking,
    Ready { path: PathBuf, version: String },
    Missing,
}

pub struct ComputerUsePage {
    data_dir: Option<PathBuf>,
    enabled: bool,
    permissions: Permissions,
    driver: Driver,
    accessibility_prompted: bool,
    screen_prompted: bool,
    input_prompted: bool,
    cursor: Option<zeron_mcp::computer::CursorStyle>,
    error: Option<SharedString>,
    _poll: Task<()>,
    _driver_probe: Task<()>,
}

impl ComputerUsePage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data_dir = crate::settings::data_dir(cx);
        let enabled = zeron_mcp::computer::user_enabled(data_dir.as_deref());
        let cursor = zeron_mcp::computer::cursor_style(data_dir.as_deref());
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let permissions = Permissions::read();
                if this
                    .update(cx, |page, cx| {
                        if page.permissions != permissions {
                            page.permissions = permissions;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        let probe = cx.spawn(async move |this, cx| {
            let driver = cx
                .background_executor()
                .spawn(async move { probe_driver() })
                .await;
            this.update(cx, |page, cx| {
                page.driver = driver;
                cx.notify();
            })
            .ok();
        });
        Self {
            data_dir,
            enabled,
            permissions: Permissions::read(),
            driver: Driver::Checking,
            accessibility_prompted: false,
            screen_prompted: false,
            input_prompted: false,
            cursor,
            error: None,
            _poll: poll,
            _driver_probe: probe,
        }
    }

    fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let Some(dir) = self.data_dir.clone() else {
            self.error = Some("Unable to save this setting. Restart Zeron and try again.".into());
            cx.notify();
            return;
        };
        match zeron_mcp::computer::set_user_enabled(&dir, enabled) {
            Ok(()) => self.enabled = enabled,
            Err(_) => {
                self.error = Some("Unable to save this setting. Check folder permissions.".into());
            }
        }
        cx.notify();
    }

    fn set_cursor(&mut self, preset: usize, cx: &mut Context<Self>) {
        let Some(dir) = self.data_dir.clone() else {
            self.error = Some("Unable to save this setting. Restart Zeron and try again.".into());
            cx.notify();
            return;
        };
        let style = CURSOR_PRESETS[preset]
            .1
            .map(|theme| zeron_mcp::computer::CursorStyle {
                theme: Some(theme.id.to_owned()),
            });
        match zeron_mcp::computer::set_cursor_style(&dir, style.clone()) {
            Ok(()) => self.cursor = style,
            Err(_) => {
                self.error = Some("Unable to save this setting. Check folder permissions.".into());
            }
        }
        cx.notify();
    }
}

/// Locate the driver and read its version (off the UI thread).
fn probe_driver() -> Driver {
    let Some(path) = zeron_mcp::computer::locate_driver() else {
        return Driver::Missing;
    };
    let version = std::process::Command::new(&path)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "cua-driver".into());
    Driver::Ready { path, version }
}

fn row(
    theme: &Theme,
    first: bool,
    title: &str,
    description: impl Into<SharedString>,
    control: AnyElement,
) -> gpui::Div {
    widgets::card_row(theme, first)
        .flex_wrap()
        .child(
            div()
                .flex_1()
                .min_w(px(160.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(widgets::row_title(theme, title.to_string()))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(crate::typography::ui_rems(16.0))
                        .text_color(theme.text_muted)
                        .child(description.into()),
                ),
        )
        .child(control)
}

impl Render for ComputerUsePage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::toast::take_error(&mut self.error, cx);
        let theme = Theme::of(cx).for_settings_surface();
        let accent = theme.accent;
        let permissions = self.permissions;
        let status = |ready: bool| {
            if ready {
                widgets::badge_active(&theme, "Granted")
            } else {
                widgets::badge(&theme, "Required")
            }
        };
        let action = |id: &'static str, label: &'static str| {
            widgets::ghost_action(&theme)
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(move |style| style.border_2().border_color(accent))
                .cursor_pointer()
                .child(label)
        };

        let enabled = self.enabled;
        let toggle = widgets::toggle_switch(&theme, enabled, "computer-use-enabled")
            .id("computer-use-enabled")
            .role(gpui::Role::Switch)
            .aria_label("Allow agents to control this computer")
            .aria_toggled(if enabled {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .tab_index(0)
            .focus_visible(move |style| style.border_2().border_color(accent))
            .on_click(cx.listener(move |this, _, _, cx| this.set_enabled(!enabled, cx)));

        let driver_description: SharedString = match &self.driver {
            Driver::Checking => "Looking for the bundled cua-driver…".into(),
            Driver::Ready { path, version } => format!("{version} · {}", path.display()).into(),
            Driver::Missing => "cua-driver is not installed. Reinstall Zeron, or run \
                                scripts/provision-cua-driver.sh before packaging."
                .into(),
        };
        let driver_badge = match &self.driver {
            Driver::Checking => widgets::badge(&theme, "Checking"),
            Driver::Ready { .. } => widgets::badge_active(&theme, "Ready"),
            Driver::Missing => widgets::badge(&theme, "Missing"),
        };

        let accessibility_label = if self.accessibility_prompted {
            "Open System Settings"
        } else {
            "Grant access"
        };
        let mut accessibility = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(status(permissions.accessibility));
        if !permissions.accessibility {
            accessibility = accessibility.child(
                action("computer-use-accessibility", accessibility_label).on_click(cx.listener(
                    |this, _, _, cx| {
                        if this.accessibility_prompted {
                            #[cfg(target_os = "macos")]
                            cx.open_url(ACCESSIBILITY_SETTINGS_URL);
                        } else {
                            this.accessibility_prompted = true;
                            crate::appshots::request_semantic_access();
                        }
                        this.permissions = Permissions::read();
                        cx.notify();
                    },
                )),
            );
        }

        let screen_label = if self.screen_prompted {
            "Open System Settings"
        } else {
            "Grant access"
        };
        let mut screen = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(status(permissions.screen_recording));
        if !permissions.screen_recording {
            screen = screen.child(action("computer-use-screen", screen_label).on_click(
                cx.listener(|this, _, _, cx| {
                    if this.screen_prompted {
                        #[cfg(target_os = "macos")]
                        cx.open_url(SCREEN_RECORDING_SETTINGS_URL);
                    } else {
                        this.screen_prompted = true;
                        crate::appshots::request_capture_access();
                    }
                    this.permissions = Permissions::read();
                    cx.notify();
                }),
            ));
        }

        let input_label = if self.input_prompted {
            "Open System Settings"
        } else {
            "Grant access"
        };
        let mut input = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(status(permissions.input_monitoring));
        if !permissions.input_monitoring {
            input = input.child(
                action("computer-use-input-monitoring", input_label).on_click(cx.listener(
                    |this, _, _, cx| {
                        if this.input_prompted {
                            #[cfg(target_os = "macos")]
                            cx.open_url(INPUT_MONITORING_SETTINGS_URL);
                        } else {
                            this.input_prompted = true;
                            listen::request();
                        }
                        this.permissions = Permissions::read();
                        cx.notify();
                    },
                )),
            );
        }

        let selected = preset_index(&self.cursor);
        let border = theme.border;
        let swatches = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .children(
                CURSOR_PRESETS
                    .iter()
                    .enumerate()
                    .map(|(index, (name, preset))| {
                        let active = selected == Some(index);
                        let dot = div().size(px(14.0)).rounded_full();
                        let dot = match preset {
                            Some(preset) => dot
                                .bg(gpui::rgb(preset.fill))
                                .border_2()
                                .border_color(gpui::rgb(preset.rim)),
                            None => dot.bg(theme.text).border_2().border_color(theme.wash(0.25)),
                        };
                        div()
                            .id(SharedString::from(format!("computer-use-cursor-{index}")))
                            .role(gpui::Role::RadioButton)
                            .aria_label(format!("{name} cursor"))
                            .aria_toggled(if active {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .tab_index(0)
                            .focus_visible(move |style| style.border_color(accent))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded_full()
                            .border_1()
                            .border_color(if active { accent } else { border })
                            .when(active, |el| el.bg(accent.opacity(0.12)))
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(if active { theme.text } else { theme.text_muted })
                            .child(dot)
                            .child(*name)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_cursor(index, cx)))
                    }),
            );

        let automation = action("computer-use-automation", "Open System Settings").on_click(
            cx.listener(|_, _, _, cx| {
                #[cfg(target_os = "macos")]
                cx.open_url(AUTOMATION_SETTINGS_URL);
            }),
        );

        let card = widgets::section_card(&theme)
            .child(row(
                &theme,
                true,
                "Allow computer control",
                "Agents get computer_* tools: screenshots, clicks, typing, apps, windows \
                 and browser automation. Takes effect for newly started agent turns.",
                toggle.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                "Driver",
                driver_description,
                driver_badge.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                "Stop with Escape",
                "Press Escape on the keyboard to stop the agent's mouse and keyboard input at \
                 once; Stop in the chat does the same. Needs Input Monitoring.",
                div().into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                "Cursor color",
                "The agent's pointer, labeled with its chat. Applies to newly started agent turns.",
                swatches.into_any_element(),
            ));

        let permissions_card = widgets::section_card(&theme)
            .child(row(
                &theme,
                true,
                "Accessibility",
                "Read on-screen controls and click, type and scroll in other apps.",
                accessibility.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                "Screen Recording",
                "Take screenshots so agents can see what is on screen.",
                screen.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                "Input Monitoring",
                "Notice a physical Escape press while agents work in other apps.",
                input.into_any_element(),
            ))
            .child(row(
                &theme,
                false,
                "Automation",
                "macOS asks the first time an agent controls a specific app; manage \
                 those choices here.",
                automation.into_any_element(),
            ));

        let all_granted = permissions.accessibility
            && permissions.screen_recording
            && permissions.input_monitoring;
        let footer: SharedString = if all_granted {
            "All permissions are granted. They stay with Zeron across updates as long as \
             it keeps the same signature and location."
                .into()
        } else {
            "After you enable Zeron in System Settings, macOS may ask you to quit and reopen \
             it. This page updates on its own."
                .into()
        };

        widgets::scroll_faded(
            "computer-use-settings-scroll",
            div()
                .id("computer-use-settings-page")
                .size_full()
                .overflow_y_scroll()
                .child(
                    widgets::page_column()
                        .child(widgets::page_header(&theme, "Computer use", None))
                        .child(
                            widgets::page_subtitle(
                                &theme,
                                "Let agents see and operate this Mac through the bundled \
                                 cua-driver.",
                            )
                            .line_height(px(20.0)),
                        )
                        .child(card)
                        .child(
                            div()
                                .mt(px(24.0))
                                .child(widgets::section_label(&theme, "Permissions")),
                        )
                        .child(permissions_card)
                        .child(
                            div()
                                .mt(px(12.0))
                                .px(px(8.0))
                                .text_size(px(12.0))
                                .text_color(theme.text_muted)
                                .child(footer),
                        ),
                ),
        )
    }
}
