//! Offline native render of the production account popover in both themes.
//! Windows: cargo run -p zeron-ui --release --features account-usage-fixture
//! --example account-usage-fixture -- <output-directory> <capture-helper.ps1>
#[cfg(target_os = "windows")]
mod windows {
    use chrono::{TimeDelta, Utc};
    use gpui::{
        AppContext, AsyncApp, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions,
        div, prelude::*, px, size,
    };
    use std::{path::PathBuf, time::Duration};
    use zeron_proto::{AgentAccount, AgentAccountsSnapshot, AgentUsageWindow, HarnessId};
    use zeron_ui::*;

    struct Fixture {
        usage: Entity<AccountUsage>,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = theme::Theme::of(cx).clone();
            div()
                .size_full()
                .bg(theme.surface)
                .text_color(theme.text)
                .font_family(theme.font_sans.clone())
                .p(px(24.0))
                .child(
                    div()
                        .size_full()
                        .flex()
                        .items_end()
                        .justify_end()
                        .child(self.usage.clone()),
                )
        }
    }

    fn snapshot(harness: HarnessId, edge_cases: bool) -> AgentAccountsSnapshot {
        let now = Utc::now();
        let windows = if harness == HarnessId::Codex {
            vec![AgentUsageWindow {
                label: "Week".into(),
                used_fraction: 0.0,
                resets_at: Some(now + TimeDelta::days(3) + TimeDelta::hours(8)),
            }]
        } else {
            vec![
                AgentUsageWindow {
                    label: "Session".into(),
                    used_fraction: 0.02,
                    resets_at: Some(now + TimeDelta::hours(2) + TimeDelta::minutes(14)),
                },
                AgentUsageWindow {
                    label: "Week".into(),
                    used_fraction: 0.62,
                    resets_at: Some(now + TimeDelta::days(4) + TimeDelta::hours(6)),
                },
            ]
        };
        let mut account: AgentAccount = serde_json::from_value(serde_json::json!({
            "id": "fixture", "harness": harness, "email": "alex@example.com",
            "planLabel": if harness == HarnessId::Codex { "ChatGPT Pro" } else { "Max 20×" },
            "active": true, "switchable": true, "usageWindows": windows,
            "availableResets": if harness == HarnessId::Codex { Some(2) } else { None },
        }))
        .unwrap();
        if edge_cases {
            account.email = Some("a.very.long.account.name@example.com".into());
            account.usage_windows = vec![
                AgentUsageWindow {
                    label: "Session".into(),
                    used_fraction: 1.0,
                    resets_at: Some(now - TimeDelta::minutes(1)),
                },
                AgentUsageWindow {
                    label: "Week".into(),
                    used_fraction: 0.9,
                    resets_at: None,
                },
            ];
            account.available_resets = Some(0);
        }
        AgentAccountsSnapshot {
            accounts: vec![account],
            warnings: vec![],
        }
    }

    async fn capture(
        cx: &mut AsyncApp,
        helper: PathBuf,
        output: PathBuf,
        name: String,
    ) -> anyhow::Result<()> {
        cx.background_executor()
            .timer(Duration::from_millis(700))
            .await;
        let result = cx
            .background_executor()
            .spawn(async move {
                use std::os::windows::process::CommandExt;
                std::process::Command::new("powershell.exe")
                    .creation_flags(0x08000000)
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                    ])
                    .arg(helper)
                    .arg("-FixtureProcessId")
                    .arg(std::process::id().to_string())
                    .arg("-OutputFile")
                    .arg(output.join(format!("{name}.png")))
                    .arg("-MetadataFile")
                    .arg(output.join(format!("{name}.json")))
                    .output()
            })
            .await?;
        anyhow::ensure!(
            result.status.success(),
            "Capture failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(())
    }

    pub fn run() -> anyhow::Result<()> {
        let runtime = tokio::runtime::Runtime::new()?;
        let _guard = runtime.enter();
        let mut args = std::env::args().skip(1);
        let output = PathBuf::from(args.next().expect("output directory"));
        let helper = PathBuf::from(args.next().expect("capture helper"));
        std::fs::create_dir_all(&output)?;
        let temp = tempfile::tempdir()?;
        let data = temp.path().to_path_buf();
        gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let prefs = settings::UiSettings::default();
            settings::init(prefs.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(prefs.ui_font_family.clone(), prefs.ui_font_size,
                prefs.terminal_font_family.clone(), prefs.terminal_font_size,
                prefs.code_font_family.clone(), prefs.code_font_size, fonts, cx);
            theme_library::init(data, cx);
            appearance::init(appearance::AppearanceMode::Dark, prefs.theme_selection,
                prefs.accent, prefs.surface, cx);
            let state = cx.new(|_| state::AppState::new());
            let usage = cx.new(|cx| AccountUsage::new(state, cx));
            usage.update(cx, |usage, cx| usage.fixture_accounts(HarnessId::Codex, snapshot(HarnessId::Codex, false), cx));
            let window = cx.open_window(WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    gpui::point(px(40.0), px(40.0)), size(px(460.0), px(230.0))))),
                ..Default::default()
            }, |_, cx| cx.new(|_| Fixture { usage })).unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    for (mode, theme) in [(appearance::AppearanceMode::Dark, "dark"), (appearance::AppearanceMode::Light, "light")] {
                        cx.update(|cx| appearance::set_mode(mode, cx));
                        for (harness, name, edge_cases) in [(HarnessId::Codex, "codex", false),
                            (HarnessId::ClaudeCode, "claude", false), (HarnessId::ClaudeCode, "edge-cases", true)] {
                            window.update(cx, |view, _, cx| view.usage.update(cx, |usage, cx|
                                usage.fixture_accounts(harness, snapshot(harness, edge_cases), cx)))?;
                            capture(cx, helper.clone(), output.clone(), format!("{name}-{theme}")).await?;
                        }
                    }
                    std::fs::write(output.join("result.txt"), "Rendered production account popovers: Codex credits, Claude dual countdowns, expired/missing resets and long identity, in dark and light themes. Synthetic accounts; no live credentials.\n")?;
                    Ok(())
                }.await;
                if let Err(err) = result { eprintln!("Fixture failed: {err:#}"); }
                cx.update(|cx| cx.quit());
            }).detach();
        });
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    windows::run()
}
#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("This native capture fixture runs on Windows.");
}
