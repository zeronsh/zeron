//! Native visual QA for the pull request board and its side pane. Renders the
//! production Shell offscreen (never activated) against a real engine and the
//! local `gh`, whose calls are delayed on demand so loading states can be
//! captured.
//!
//!   cargo run -p zeron-ui --example pull-request-fixture \
//!     --features pull-request-fixture -- <output-dir> [owner/repo] [number]
//!
//! On Linux, run it on its own X display with a window manager; xdotool and
//! ImageMagick's `import` capture the window:
//!
//!   Xvfb :99 -screen 0 1400x900x24 & DISPLAY=:99 openbox &
//!   WAYLAND_DISPLAY= DISPLAY=:99 cargo run …
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    let path = directory.join(format!("{name}.png"));
    #[cfg(not(target_os = "linux"))]
    return window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.render_to_image()?.save(&path)?;
        Ok(())
    })?;
    // No readback on Linux: capture the window from its X display (run under
    // a dedicated `Xvfb` with a window manager, like the browser fixture).
    #[cfg(target_os = "linux")]
    {
        // The frame loop can be idle: present the current scene first.
        window.update(cx, |_, w, cx| {
            w.refresh();
            w.draw(cx).clear();
        })?;
        std::thread::sleep(Duration::from_millis(100));
        let windows = std::process::Command::new("xdotool")
            .args([
                "search",
                "--onlyvisible",
                "--pid",
                &std::process::id().to_string(),
            ])
            .output()?;
        let id = String::from_utf8(windows.stdout)?
            .lines()
            .next()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("fixture window not visible"))?;
        let status = std::process::Command::new("import")
            .args(["-window", &id])
            .arg(&path)
            .status()?;
        anyhow::ensure!(status.success(), "screenshot capture failed");
        Ok(())
    }
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let mut args = std::env::args().skip(1);
    let output = PathBuf::from(args.next().expect("output directory"));
    let repository = args.next().unwrap_or_else(|| "zeronsh/zeron".into());
    let number = args.next().unwrap_or_else(|| "591".into());
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;

    // A `gh` shim ahead of the real one: sleeps for the seconds in `delay`.
    let real_gh = which_gh()?;
    let shim = temp.path().join("bin");
    std::fs::create_dir(&shim)?;
    let delay = temp.path().join("delay");
    std::fs::write(&delay, "0")?;
    std::fs::write(
        shim.join("gh"),
        format!(
            "#!/bin/sh\nsleep \"$(cat '{}')\"\nexec '{}' \"$@\"\n",
            delay.display(),
            real_gh.display()
        ),
    )?;
    std::process::Command::new("chmod")
        .args(["+x", &shim.join("gh").to_string_lossy()])
        .status()?;
    let path = format!(
        "{}:{}",
        shim.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("PATH", path);
        std::env::set_var("ZERON_OPEN_ROUTE", "pull-requests");
    }
    let set_delay = move |seconds: u64| std::fs::write(&delay, seconds.to_string());

    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::default_registry()),
            zeron_proto::HarnessId::ClaudeCode,
            None,
        )
    })?;
    let ipc_port = port();
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, core.rpc_service()))?;
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let boot = EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zeron_proto::HarnessId::ClaudeCode,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let device = core.device_id.clone();
    let url = format!("https://github.com/{repository}/pull/{number}");
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    set_delay(3)?;
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.last_pull_request_repository = Some(repository.clone());
            settings.pull_request_destination = settings::PullRequestDestination::Native;
            // Match the default desktop look: translucent, frosted surfaces.
            settings.surface = zeron_theme::SurfacePreference::Frosted;
            settings::init(settings.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                settings.ui_font_family.clone(),
                settings.ui_font_size,
                settings.terminal_font_family.clone(),
                settings.terminal_font_size,
                settings.code_font_family.clone(),
                settings.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data.clone(), cx);
            appearance::init(
                appearance::AppearanceMode::Dark,
                settings.theme_selection,
                settings.accent,
                settings.surface,
                cx,
            );
            history::init(
                settings.git_history_columns,
                settings.git_history_column_widths,
                settings.git_history_column_order,
                settings.git_history_author_display,
                cx,
            );
            composer::init(cx, settings.composer_send_behavior);
            terminal::panel::init(cx);
            app_menus::init(cx);
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Development);
                s.local_device_id = Some(device.clone());
                s.devices = vec![
                    serde_json::from_value(serde_json::json!({
                        "id": device, "name": "This device",
                        "platform": std::env::consts::OS, "lastSeenAt": null
                    }))
                    .unwrap(),
                ];
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            // Offscreen and never activated: the user's windows keep focus.
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(-12000.), px(-12000.)),
                            size(px(1320.), px(848.)),
                        ))),
                        focus: false,
                        show: true,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            state.update(cx, |_, cx| cx.notify());
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    let any = window.into();
                    let shot = |cx: &mut AsyncApp, name: &str| capture(any, cx, &output, name);
                    pause(cx, 900).await;
                    shot(cx, "01-board-loading")?;
                    pause(cx, 6000).await;
                    set_delay(0)?;
                    shot(cx, "02-board")?;
                    window.update(cx, |s, _, cx| s.fixture_pull_request_settings(true, cx))?;
                    pause(cx, 600).await;
                    shot(cx, "02b-settings")?;
                    window.update(cx, |s, _, cx| s.fixture_pull_request_settings(false, cx))?;
                    pause(cx, 300).await;
                    window.update(cx, |_, w, _| w.resize(size(px(760.), px(848.))))?;
                    pause(cx, 600).await;
                    shot(cx, "03-board-760")?;
                    window.update(cx, |_, w, _| w.resize(size(px(1320.), px(848.))))?;
                    set_delay(3)?;
                    window.update(cx, |_, w, cx| {
                        w.dispatch_action(
                            Box::new(pull_request_detail::OpenPullRequest(url.clone(), None)),
                            cx,
                        )
                    })?;
                    pause(cx, 700).await;
                    shot(cx, "04-detail-loading")?;
                    pause(cx, 7000).await;
                    shot(cx, "05-summary")?;
                    let detail = window
                        .update(cx, |s, _, _| s.fixture_pull_request_detail())?
                        .ok_or_else(|| anyhow::anyhow!("detail did not open"))?;
                    // The pane at its narrowest beside the board.
                    window.update(cx, |s, _, cx| s.fixture_pull_request_pane_width(360.0, cx))?;
                    pause(cx, 500).await;
                    shot(cx, "05b-summary-pane-360")?;
                    detail.update(cx, |d, cx| d.fixture_select_tab(2, cx));
                    pause(cx, 500).await;
                    shot(cx, "05c-activity-pane-360")?;
                    detail.update(cx, |d, cx| d.fixture_select_tab(0, cx));
                    window.update(cx, |s, _, cx| s.fixture_pull_request_pane_width(640.0, cx))?;
                    detail.update(cx, |d, cx| d.fixture_select_tab(1, cx));
                    pause(cx, 700).await;
                    shot(cx, "06-code-loading")?;
                    pause(cx, 8000).await;
                    set_delay(0)?;
                    shot(cx, "07-code")?;
                    detail.update(cx, |d, cx| d.fixture_select_file(6, cx));
                    pause(cx, 500).await;
                    shot(cx, "08-code-jump")?;
                    // Expanded, the pane covers the board for code review.
                    window.update(cx, |s, _, cx| s.fixture_toggle_pull_request_pane_expand(cx))?;
                    pause(cx, 500).await;
                    shot(cx, "08b-code-expanded")?;
                    window.update(cx, |s, _, cx| s.fixture_toggle_pull_request_pane_expand(cx))?;
                    window.update(cx, |_, w, _| w.resize(size(px(760.), px(848.))))?;
                    pause(cx, 600).await;
                    shot(cx, "09-code-760")?;
                    window.update(cx, |_, w, _| w.resize(size(px(1320.), px(848.))))?;
                    detail.update(cx, |d, cx| d.fixture_select_tab(2, cx));
                    pause(cx, 800).await;
                    shot(cx, "10-activity")?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    detail.update(cx, |d, cx| d.fixture_select_tab(0, cx));
                    pause(cx, 800).await;
                    shot(cx, "11-summary-light")?;
                    detail.update(cx, |d, cx| d.fixture_select_tab(1, cx));
                    pause(cx, 800).await;
                    shot(cx, "12-code-light")?;
                    detail.update(cx, |d, cx| {
                        d.fixture_select_file(1, cx);
                        d.fixture_scroll_code(240.0, cx);
                    });
                    pause(cx, 500).await;
                    shot(cx, "12b-sticky-light")?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Dark, cx));
                    pause(cx, 500).await;
                    shot(cx, "12c-sticky-dark")?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    window.update(cx, |_, w, _| w.resize(size(px(600.), px(848.))))?;
                    detail.update(cx, |d, cx| d.fixture_select_tab(0, cx));
                    pause(cx, 800).await;
                    shot(cx, "13-summary-600-light")?;
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("pull request fixture failed: {error:#}");
                    *result.lock().unwrap() = Some(format!("{error:#}"));
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}

fn which_gh() -> anyhow::Result<PathBuf> {
    let output = std::process::Command::new("sh")
        .args(["-c", "command -v gh"])
        .output()?;
    let path = String::from_utf8(output.stdout)?.trim().to_owned();
    anyhow::ensure!(!path.is_empty(), "gh is not installed");
    Ok(path.into())
}
