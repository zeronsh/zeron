//! Cloud's API-key dialog: the key a provider card stores in the vault for
//! every Cloud machine — and, for an Anthropic key, the LLM gateway or proxy
//! it is for. [`AccountsPage`] opens it from the card's "Add … API key",
//! saves it through the engine and closes it once the vault holds the key.
//!
//! The rules a base URL is held to are the engine's
//! ([`VaultProvider::check_base_url`], shared with the phone): the dialog
//! shows their verdict as the user types, so a refusal reads the same here
//! as from the engine — and the engine checks again before anything leaves
//! this computer.

use gpui::{
    AnyElement, App, Context, Entity, Focusable, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use zeron_proto::{ANTHROPIC_MESSAGES_PATH, BaseUrlCheck, VaultProvider};

use super::accounts::AccountsPage;
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover;
use crate::settings::widgets;
use crate::theme::Theme;

/// Room for the two endpoints side by side; the sign-in dialog's 360px
/// squeezed their hosts into ellipses.
const DIALOG_WIDTH: f32 = 440.0;

/// Where a key's requests go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEndpoint {
    /// The provider's own API.
    Direct,
    /// An LLM gateway or proxy at the base URL typed beside the key.
    Gateway,
}

/// A provider's dialog copy. Pure.
struct KeyCopy {
    title: &'static str,
    body: &'static str,
    placeholder: &'static str,
}

fn copy(provider: VaultProvider) -> KeyCopy {
    match provider {
        VaultProvider::OpenaiKey => KeyCopy {
            title: "Add an OpenAI API key",
            body: "Cloud machines use it for Codex instead of a ChatGPT sign-in.",
            placeholder: "sk-proj-…",
        },
        _ => KeyCopy {
            title: "Add an Anthropic API key",
            body: "Cloud machines use it for Claude Code instead of a subscription sign-in.",
            placeholder: "sk-ant-api03-…",
        },
    }
}

/// A field the dialog moves the keyboard to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyField {
    Key,
    BaseUrl,
}

/// The open dialog, owned by the page whose card opened it.
pub(crate) struct KeyDialog {
    pub(crate) provider: VaultProvider,
    pub(crate) key: Entity<ComposerInput>,
    /// Only for a key that may name one ([`VaultProvider::takes_base_url`]).
    pub(crate) base_url: Option<Entity<ComposerInput>>,
    pub(crate) endpoint: KeyEndpoint,
    pub(crate) saving: bool,
    /// Why the last Save didn't go through, shown above the buttons.
    pub(crate) error: Option<SharedString>,
    /// Focused on the next frame.
    pub(crate) focus: Option<KeyField>,
    _events: Vec<Subscription>,
}

impl KeyDialog {
    pub(crate) fn open(provider: VaultProvider, cx: &mut Context<AccountsPage>) -> Self {
        let copy = copy(provider);
        let key = cx.new(|cx| {
            ComposerInput::new(copy.placeholder, cx)
                .with_single_line()
                .with_masked()
                .with_tab_stop()
        });
        // The URL in the monospace of the route it previews, a point
        // smaller so both fields keep one height.
        let base_url = provider.takes_base_url().then(|| {
            cx.new(|cx| {
                ComposerInput::new("https://gateway.example.com", cx)
                    .with_single_line()
                    .with_tab_stop()
                    .with_monospace()
                    .with_text_metrics(13.0, crate::composer::INPUT_LINE_HEIGHT)
            })
        });
        let events = std::iter::once(&key)
            .chain(&base_url)
            .map(|field| {
                cx.subscribe(field, |page: &mut AccountsPage, _, event, cx| match event {
                    ComposerInputEvent::Submitted => page.save_api_key(cx),
                    ComposerInputEvent::Edited => page.api_key_edited(cx),
                    _ => {}
                })
            })
            .collect();
        Self {
            provider,
            key,
            base_url,
            endpoint: KeyEndpoint::Direct,
            saving: false,
            error: None,
            focus: Some(KeyField::Key),
            _events: events,
        }
    }

    fn key_text(&self, cx: &App) -> String {
        self.key.read(cx).text().trim().to_string()
    }

    fn base_url_text<'a>(&self, cx: &'a App) -> Option<&'a str> {
        let field = self
            .base_url
            .as_ref()
            .filter(|_| self.endpoint == KeyEndpoint::Gateway)?;
        Some(field.read(cx).text())
    }

    /// The live verdict on the base URL; `None` for the provider's own API.
    pub(crate) fn base_url_check(&self, cx: &App) -> Option<BaseUrlCheck> {
        self.base_url_text(cx)
            .map(|raw| self.provider.check_base_url(raw))
    }

    /// What Save sends — the key, and the base URL as the vault will keep
    /// it — or why it can't yet, in the words shown when the user tries.
    pub(crate) fn submission(&self, cx: &App) -> Result<(String, Option<String>), &'static str> {
        let key = self.key_text(cx);
        if key.is_empty() {
            return Err("Paste a key first.");
        }
        let Some(raw) = self.base_url_text(cx) else {
            return Ok((key, None));
        };
        match self.provider.api_base_url(raw) {
            Ok(Some(url)) => Ok((key, Some(url))),
            Ok(None) => Err("Add the gateway's base URL, or choose Anthropic API."),
            Err(why) => Err(why),
        }
    }

    pub(crate) fn focus_handle(&self, field: KeyField, cx: &App) -> Option<gpui::FocusHandle> {
        match field {
            KeyField::Key => Some(self.key.focus_handle(cx)),
            KeyField::BaseUrl => self.base_url.as_ref().map(|input| input.focus_handle(cx)),
        }
    }
}

/// The dialog: the provider's mark and what the key is for, the masked
/// key (echoing the four characters it will be known by), then — for an
/// Anthropic key — where its requests go: Anthropic's API, or a gateway
/// whose base URL is checked as it is typed and previewed as the URL Claude
/// Code will call. The vault note and the actions close it.
pub(crate) fn render_dialog(
    dialog: &KeyDialog,
    viewport: gpui::Size<gpui::Pixels>,
    window: &mut Window,
    cx: &mut Context<AccountsPage>,
) -> AnyElement {
    let theme = Theme::of(cx).for_popup();
    let copy = copy(dialog.provider);
    let ready = dialog.submission(cx).is_ok();
    let saving = dialog.saving;
    let reduced = crate::motion::reduced_motion(cx);

    // The provider's tile, as its row on the Providers page draws it.
    let (mark, tint) = crate::pickers::harness_brand_icon(match dialog.provider {
        VaultProvider::OpenaiKey => zeron_proto::HarnessId::Codex,
        _ => zeron_proto::HarnessId::ClaudeCode,
    });
    let header = div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .flex_none()
                .size(px(36.0))
                .rounded(px(10.0))
                .bg(theme.wash(0.06))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    crate::icons::icon(mark)
                        .size(px(16.0))
                        .text_color(tint.unwrap_or(theme.text_muted)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pt(px(1.0))
                .child(popover::dialog_title(&theme, copy.title))
                .child(
                    div()
                        .mt(px(3.0))
                        .child(popover::dialog_body(&theme, copy.body)),
                ),
        );

    let suffix = zeron_proto::api_key_suffix(&dialog.key_text(cx)).map(|suffix| {
        div()
            .flex_none()
            .text_color(theme.text_muted)
            .child(SharedString::from(suffix))
            .into_any_element()
    });
    let key_block = div()
        .mt(px(20.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(widgets::field_label(&theme, "API key"))
        .child(text_field(&theme, &dialog.key, suffix, window, cx));

    let endpoint_block = dialog.base_url.as_ref().map(|base_url| {
        let check = dialog.base_url_check(cx);
        // Once the base URL holds up, the gateway choice names its host.
        let gateway_detail: SharedString = match &check {
            Some(BaseUrlCheck::Ready(url)) => zeron_proto::base_url_host(url).into(),
            _ => "Your own base URL".into(),
        };
        let gateway = (dialog.endpoint == KeyEndpoint::Gateway).then(|| {
            let status = match check.clone().unwrap_or(BaseUrlCheck::Empty) {
                BaseUrlCheck::Ready(url) => route_line(&theme, &url),
                BaseUrlCheck::Refused(why) => problem_line(&theme, why),
                BaseUrlCheck::Empty | BaseUrlCheck::Typing => note_line(
                    &theme,
                    crate::icons::INFO_CIRCLE,
                    "Claude Code adds /v1/messages to this URL.",
                ),
            };
            let content = div()
                .mt(px(10.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(text_field(&theme, base_url, None, window, cx))
                .child(status);
            if reduced {
                content.into_any_element()
            } else {
                crate::motion::menu_in("api-key-gateway", content).into_any_element()
            }
        });
        div()
            .mt(px(18.0))
            .flex()
            .flex_col()
            .child(widgets::field_label(&theme, "Endpoint"))
            .child(
                // One control, two choices: the PR filter tray's language,
                // tall enough to say where each one sends the key.
                div()
                    .id("api-key-endpoint")
                    .mt(px(8.0))
                    .p(px(3.0))
                    .rounded(px(12.0))
                    .bg(crate::theme::ink(0.035))
                    .flex()
                    .flex_row()
                    .gap(px(3.0))
                    .role(gpui::Role::RadioGroup)
                    .aria_label("Endpoint")
                    .child(endpoint_choice(
                        &theme,
                        dialog,
                        KeyEndpoint::Direct,
                        crate::icons::icon(crate::icons::CLAUDE_MARK)
                            .size(px(14.0))
                            .text_color(crate::icons::claude_brand())
                            .into_any_element(),
                        ("Anthropic API", "api.anthropic.com".into()),
                        cx,
                    ))
                    .child(endpoint_choice(
                        &theme,
                        dialog,
                        KeyEndpoint::Gateway,
                        crate::icons::icon(crate::icons::GLOBAL)
                            .size(px(14.0))
                            .text_color(theme.text_muted)
                            .into_any_element(),
                        ("LLM gateway", gateway_detail),
                        cx,
                    )),
            )
            .children(gateway)
    });

    let error = dialog
        .error
        .clone()
        .map(|message| div().mt(px(14.0)).child(problem_line(&theme, message)));

    let action = |id: &'static str, tone: widgets::ActionTone| {
        widgets::action_button(&theme, tone)
            .id(id)
            .tab_index(0)
            .role(gpui::Role::Button)
            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
    };
    let footer = div()
        .mt(px(20.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .id("api-key-vault-note")
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .tooltip(widgets::text_tooltip(
                    "Sealed in your vault: only your Cloud machines can use it.",
                ))
                .child(
                    crate::icons::icon(crate::icons::LOCK)
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                )
                .child(div().truncate().child("Encrypted in your vault")),
        )
        .child(
            action("api-key-cancel", widgets::ActionTone::Quiet)
                .on_click(cx.listener(|page, _, _, cx| page.close_api_key(cx)))
                .child("Cancel"),
        )
        .child(
            // Dim until there is something to save; a press still says
            // what's missing instead of doing nothing.
            action("api-key-save", widgets::ActionTone::Solid)
                .aria_label("Save key")
                .when(!ready || saving, |el| el.opacity(0.45))
                .when(saving, |el| {
                    el.child(crate::loaders::ring_spinner(
                        "api-key-saving",
                        12.0,
                        theme.on_solid,
                    ))
                })
                .on_click(cx.listener(|page, _, _, cx| page.save_api_key(cx)))
                .child(if saving { "Saving…" } else { "Save key" }),
        );

    let card = popover::dialog_card(&theme)
        .w(px(DIALOG_WIDTH))
        .id("api-key-card")
        .role(gpui::Role::Dialog)
        .aria_label(copy.title)
        .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                page.close_api_key(cx);
                cx.stop_propagation();
            }
        }))
        .child(header)
        .child(key_block)
        .children(endpoint_block)
        .children(error)
        .child(footer)
        .into_any_element();
    popover::modal("api-key-dialog", viewport, card)
}

/// A dialog field in the key's monospace, its frame lit while it has the
/// keyboard; `trailing` sits inside the frame after the text.
fn text_field(
    theme: &Theme,
    input: &Entity<ComposerInput>,
    trailing: Option<AnyElement>,
    window: &Window,
    cx: &App,
) -> gpui::Div {
    let focused = input.focus_handle(cx).is_focused(window);
    popover::dialog_field(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(div().flex_1().min_w_0().child(input.clone()))
            .children(trailing)
            .into_any_element(),
    )
    .font_family(theme.font_mono.clone())
    .text_size(crate::typography::ui_rems(13.0))
    .when(focused, |el| el.border_color(theme.accent.opacity(0.6)))
}

/// One endpoint in the tray: lifted like a selected chip when chosen.
fn endpoint_choice(
    theme: &Theme,
    dialog: &KeyDialog,
    endpoint: KeyEndpoint,
    glyph: AnyElement,
    (title, detail): (&'static str, SharedString),
    cx: &mut Context<AccountsPage>,
) -> AnyElement {
    let chosen = dialog.endpoint == endpoint;
    let id = match endpoint {
        KeyEndpoint::Direct => "api-key-endpoint-direct",
        KeyEndpoint::Gateway => "api-key-endpoint-gateway",
    };
    let hover_key = format!("{id}-{}", cx.entity_id());
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(9.0))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(10.0))
        .cursor_pointer()
        .role(gpui::Role::RadioButton)
        .aria_label(title)
        .aria_toggled(if chosen {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .tab_index(0)
        .focus_visible(|s| s.border_2().border_color(theme.accent))
        .map(|el| {
            if chosen {
                el.bg(crate::theme::card_selected_bg())
                    .shadow(crate::theme::card_selected_shadows())
            } else {
                el.bg(crate::motion::hover_blend(
                    &hover_key,
                    crate::theme::wash(0.0),
                    crate::theme::wash(0.05),
                ))
                .on_hover(crate::motion::hover_listener(hover_key))
            }
        })
        .on_click(cx.listener(move |page, _, _, cx| page.choose_key_endpoint(endpoint, cx)))
        .child(div().mt(px(2.0)).flex_none().child(glyph))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(if chosen { theme.text } else { theme.text_muted })
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(detail),
                ),
        )
        .into_any_element()
}

/// "→ https://gateway/v1/messages", the URL Claude Code will call: the
/// typed base URL in full ink, the path it adds muted. The path never
/// truncates, so a long base URL gives way first; the words live in the
/// tooltip (a "Requests go to" label cost the host its last characters).
fn route_line(theme: &Theme, base_url: &str) -> AnyElement {
    div()
        .id("api-key-route")
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text_muted)
        .tooltip(widgets::text_tooltip(format!(
            "Claude Code sends every request to {base_url}{ANTHROPIC_MESSAGES_PATH}"
        )))
        .child(
            crate::icons::icon(crate::icons::ARROW_RIGHT)
                .size(px(12.0))
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_row()
                .font_family(theme.font_mono.clone())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .child(SharedString::from(base_url.to_string())),
                )
                .child(div().flex_none().child(ANTHROPIC_MESSAGES_PATH)),
        )
        .into_any_element()
}

/// A muted line under a field, with its glyph.
fn note_line(theme: &Theme, glyph: &'static str, copy: &'static str) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text_muted)
        .child(
            crate::icons::icon(glyph)
                .size(px(12.0))
                .text_color(theme.text_muted),
        )
        .child(div().min_w_0().child(copy))
        .into_any_element()
}

/// Why the key can't be saved as it stands, in red beside the warning
/// glyph (the sign-in dialog's error text).
fn problem_line(theme: &Theme, message: impl Into<SharedString>) -> AnyElement {
    let red = theme.danger_muted.opacity(0.9);
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(6.0))
        .text_size(crate::typography::ui_rems(12.0))
        .line_height(px(16.0))
        .text_color(red)
        .child(
            div().mt(px(2.0)).flex_none().child(
                crate::icons::icon(crate::icons::DANGER_TRIANGLE)
                    .size(px(12.0))
                    .text_color(red),
            ),
        )
        .child(div().min_w_0().child(message.into()))
        .into_any_element()
}
