//! Isolated visual evidence for Cloud's API keys: Settings → Providers on
//! Cloud, Claude Code open, then its "Add an Anthropic API key" dialog —
//! Anthropic's API, an LLM gateway with its route preview, a refused base
//! URL, a save in flight — and the saved gateway key's row. Synthetic
//! accounts, temporary settings, no engine.
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use std::{path::Path, path::PathBuf, time::Duration};
use zeron_proto::{AgentAccount, AgentAuthKind, AgentUsageWindow, HarnessId};
use zeron_ui::settings::accounts::KeyDialogScene;
use zeron_ui::*;

const KEY: &str = "sk-gateway-7d1f4c0a2b9e81e5";
const GATEWAY: &str = "https://llm-gateway.example.com";

struct Fixture {
    page: Entity<settings::harnesses::HarnessesPage>,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::Theme::of(cx).for_settings_surface();
        div()
            .id("cloud-api-key-fixture")
            .size_full()
            .bg(theme.surface)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .child(self.page.clone())
    }
}

fn login() -> AgentAccount {
    AgentAccount {
        id: "claude:s1".into(),
        harness: HarnessId::ClaudeCode,
        email: Some("alex@example.com".into()),
        plan_label: Some("Max".into()),
        active: false,
        usage_windows: vec![
            AgentUsageWindow {
                label: "Session".into(),
                used_fraction: 0.32,
                resets_at: None,
            },
            AgentUsageWindow {
                label: "Week".into(),
                used_fraction: 0.58,
                resets_at: None,
            },
        ],
        usage_fetched_at: None,
        usage_error: None,
        display_name: None,
        organization: None,
        auth_kind: Some(AgentAuthKind::Oauth),
        switchable: true,
        saved_at: None,
        provider: None,
        gateway: None,
    }
}

/// The key as the engine lists it once saved
/// (`cloud_accounts::to_agent_account` over the vault's `…81e5 · host`).
fn gateway_key() -> AgentAccount {
    AgentAccount {
        id: "anthropic-key:a1".into(),
        email: None,
        plan_label: Some("…81e5".into()),
        display_name: Some("Anthropic API key".into()),
        active: true,
        usage_windows: vec![],
        auth_kind: Some(AgentAuthKind::ApiKey),
        gateway: Some("llm-gateway.example.com".into()),
        ..login()
    }
}

fn dialog(gateway: Option<&'static str>) -> KeyDialogScene {
    KeyDialogScene {
        harness: HarnessId::ClaudeCode,
        key: KEY,
        gateway,
        focus_base_url: gateway.is_some(),
        saving: false,
        error: None,
    }
}

struct Scene {
    name: &'static str,
    appearance: appearance::AppearanceMode,
    accounts: Vec<AgentAccount>,
    dialog: Option<KeyDialogScene>,
}

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        window.draw(cx).clear();
        window.render_to_image()?.save(output.join(name))?;
        Ok(())
    })?
}

async fn scene(
    window: gpui::WindowHandle<Fixture>,
    cx: &mut AsyncApp,
    output: &Path,
    scene: Scene,
) -> anyhow::Result<()> {
    window.update(cx, |view, _, cx| {
        appearance::set_mode(scene.appearance, cx);
        view.page.update(cx, |page, cx| {
            page.fixture_cloud_providers(scene.accounts, scene.dialog, cx)
        });
        cx.notify();
    })?;
    // The dialog's entrance and the gateway field's reveal settle first.
    pause(cx, 700).await;
    capture(window.into(), cx, output, scene.name)
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let prefs = settings::UiSettings::default();
            settings::init(prefs.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                prefs.ui_font_family.clone(),
                prefs.ui_font_size,
                prefs.terminal_font_family.clone(),
                prefs.terminal_font_size,
                prefs.code_font_family.clone(),
                prefs.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data, cx);
            appearance::init(
                appearance::AppearanceMode::Dark,
                prefs.theme_selection,
                prefs.accent,
                prefs.surface,
                cx,
            );
            composer::init(cx, prefs.composer_send_behavior);

            let state = cx.new(|_| state::AppState::new());
            let page = cx.new(|cx| settings::harnesses::HarnessesPage::new(state, cx));
            // Rendered off screen and far from any display: the fixture never
            // takes focus, and the real pointer can't hover its controls.
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(-12000.0), px(-12000.0)),
                            size(px(880.0), px(800.0)),
                        ))),
                        focus: false,
                        show: false,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| Fixture { page }),
                )
                .unwrap();

            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    pause(cx, 300).await;
                    let (dark, light) = (
                        appearance::AppearanceMode::Dark,
                        appearance::AppearanceMode::Light,
                    );
                    let refused = "http://llm-gateway.example.com";
                    for spec in [
                        Scene {
                            name: "card-dark.png",
                            appearance: dark,
                            accounts: vec![login()],
                            dialog: None,
                        },
                        Scene {
                            name: "dialog-anthropic-dark.png",
                            appearance: dark,
                            accounts: vec![login()],
                            dialog: Some(dialog(None)),
                        },
                        Scene {
                            name: "dialog-gateway-dark.png",
                            appearance: dark,
                            accounts: vec![login()],
                            dialog: Some(dialog(Some(GATEWAY))),
                        },
                        Scene {
                            name: "dialog-gateway-light.png",
                            appearance: light,
                            accounts: vec![login()],
                            dialog: Some(dialog(Some(GATEWAY))),
                        },
                        Scene {
                            name: "dialog-refused-dark.png",
                            appearance: dark,
                            accounts: vec![login()],
                            dialog: Some(dialog(Some(refused))),
                        },
                        Scene {
                            name: "dialog-saving-dark.png",
                            appearance: dark,
                            accounts: vec![login()],
                            dialog: Some(KeyDialogScene {
                                saving: true,
                                focus_base_url: false,
                                ..dialog(Some(GATEWAY))
                            }),
                        },
                        Scene {
                            name: "dialog-openai-dark.png",
                            appearance: dark,
                            accounts: vec![],
                            dialog: Some(KeyDialogScene {
                                harness: HarnessId::Codex,
                                key: "sk-proj-4b1e9c0d7a2f",
                                ..dialog(None)
                            }),
                        },
                        Scene {
                            name: "saved-dark.png",
                            appearance: dark,
                            accounts: vec![login(), gateway_key()],
                            dialog: None,
                        },
                        Scene {
                            name: "saved-light.png",
                            appearance: light,
                            accounts: vec![login(), gateway_key()],
                            dialog: None,
                        },
                    ] {
                        scene(window, cx, &output, spec).await?;
                    }
                    std::fs::write(
                        output.join("result.txt"),
                        "PASS: Cloud API key — card, dialog (Anthropic, gateway dark/light, \
                         refused, saving, OpenAI), saved gateway row (dark/light).\n",
                    )?;
                    Ok(())
                }
                .await;
                if let Err(error) = result {
                    eprintln!("cloud API key fixture failed: {error:#}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
