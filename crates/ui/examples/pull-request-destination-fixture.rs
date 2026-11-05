//! Native captures of where pull request links open (Settings → General →
//! Links → *Open pull requests in*). Renders the production Shell offscreen,
//! never activated, against a real engine, the local `gh` and WebKit, and
//! presses links with native input events. A pull request that leaves Zeron
//! opens in the default browser for real. Checks land in
//! `<output-dir>/result.txt`, and the run fails when one does.
//!
//!   cargo run -p zeron-ui --example pull-request-destination-fixture \
//!     --features pull-request-fixture,browser-fixture -- <output-dir>
//!
//! macOS: captures read the window back with `render_to_image`, which leaves
//! out WebKit's native page. Options are picked by label, so the same run
//! captures a build without *In-app browser* and skips that step.
use gpui::{
    AnyWindowHandle, AppContext, AsyncApp, Bounds, MouseButton, PlatformInput, WindowBounds,
    WindowHandle, WindowOptions, px, size,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeron_ui::settings::PullRequestDestination;
use zeron_ui::*;

const WIDTH: f32 = 1200.;
const HEIGHT: f32 = 760.;

fn pr(number: u32) -> String {
    format!("https://github.com/zeronsh/zeron/pull/{number}")
}

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The untyped handle: drawing or dispatching through the typed one would
/// hold the Shell while it renders or handles the event.
fn any(window: WindowHandle<shell::Shell>) -> AnyWindowHandle {
    window.into()
}

struct Frame {
    image: image::RgbaImage,
    scale: f32,
}

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug)]
struct Area {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Area {
    fn center(self) -> gpui::Point<gpui::Pixels> {
        gpui::point(px((self.x0 + self.x1) / 2.), px((self.y0 + self.y1) / 2.))
    }
}

fn frame(window: WindowHandle<shell::Shell>, cx: &mut AsyncApp) -> anyhow::Result<Frame> {
    any(window).update(cx, |_, w, cx| -> anyhow::Result<Frame> {
        w.draw(cx).clear();
        let image = w.render_to_image()?;
        let scale = image.width() as f32 / f32::from(w.viewport_size().width);
        Ok(Frame { image, scale })
    })?
}

/// Save a frame, or the part of it above `bottom`.
fn save(frame: &Frame, out: &Path, name: &str, bottom: Option<f32>) -> anyhow::Result<()> {
    let (w, h) = frame.image.dimensions();
    let height = bottom.map_or(h, |bottom| ((bottom * frame.scale) as u32).min(h));
    image::imageops::crop_imm(&frame.image, 0, 0, w, height)
        .to_image()
        .save(out.join(format!("{name}.png")))?;
    Ok(())
}

/// Bounding boxes of the pixels that differ between two frames, merged across
/// gaps of about six logical pixels.
fn changes(before: &Frame, after: &Frame) -> Vec<Area> {
    let s = after.scale;
    let (w, h) = after.image.dimensions();
    let cell = (6. * s) as u32;
    let (cols, rows) = (w.div_ceil(cell), h.div_ceil(cell));
    let mut grid = vec![false; (cols * rows) as usize];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (
                before.image.get_pixel(x, y).0,
                after.image.get_pixel(x, y).0,
            );
            if a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 40) {
                grid[((y / cell) * cols + x / cell) as usize] = true;
            }
        }
    }
    let mut seen = vec![false; grid.len()];
    let mut out = Vec::new();
    for start in 0..grid.len() {
        if !grid[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let (mut c0, mut r0, mut c1, mut r1) = (u32::MAX, u32::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            let (c, r) = (i as u32 % cols, i as u32 / cols);
            (c0, c1, r0, r1) = (c0.min(c), c1.max(c), r0.min(r), r1.max(r));
            for (dc, dr) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (nc, nr) = (c as i64 + dc, r as i64 + dr);
                if (0..cols as i64).contains(&nc) && (0..rows as i64).contains(&nr) {
                    let j = (nr as u32 * cols + nc as u32) as usize;
                    if grid[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(Area {
            x0: (c0 * cell) as f32 / s,
            y0: (r0 * cell) as f32 / s,
            x1: ((c1 + 1) * cell) as f32 / s,
            y1: ((r1 + 1) * cell) as f32 / s,
        });
    }
    out
}

async fn press(
    window: WindowHandle<shell::Shell>,
    at: gpui::Point<gpui::Pixels>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let event = |kind: u8| match kind {
        0 => PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position: at,
            ..Default::default()
        }),
        1 => PlatformInput::MouseDown(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            click_count: 1,
            ..Default::default()
        }),
        _ => PlatformInput::MouseUp(gpui::MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            click_count: 1,
            ..Default::default()
        }),
    };
    for kind in 0..3 {
        any(window).update(cx, |_, w, cx| {
            w.draw(cx).clear();
            w.dispatch_event(event(kind), cx);
        })?;
        pause(cx, 100).await;
    }
    Ok(())
}

/// Press empty transcript space and leave the pointer there, so a frame shows
/// what a link opened rather than its destination disclosures.
async fn settle(window: WindowHandle<shell::Shell>, cx: &mut AsyncApp) -> anyhow::Result<()> {
    press(window, gpui::point(px(WIDTH * 0.45), px(120.)), cx).await?;
    // Longer than a hoverable tooltip outlives the pointer.
    pause(cx, 1000).await;
    Ok(())
}

async fn key(
    window: WindowHandle<shell::Shell>,
    key: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let keystroke = gpui::Keystroke::parse(key)?;
    any(window).update(cx, |_, w, cx| {
        w.dispatch_event(
            PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        w.dispatch_event(PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }), cx);
    })?;
    pause(cx, 300).await;
    Ok(())
}

/// The destination Settings lists under `label`, if this build has it.
fn destination(label: &str) -> Option<PullRequestDestination> {
    PullRequestDestination::ALL
        .into_iter()
        .find(|destination| destination.label() == label)
}

/// What the Settings menu and the link menu's toggle commit.
fn configure(destination: PullRequestDestination, open_links_in_zeron: bool, cx: &mut AsyncApp) {
    cx.update(|cx| {
        settings::update(settings::SavePolicy::Immediate, cx, |settings| {
            settings.pull_request_destination = destination;
            settings.open_web_links_in_zeron = open_links_in_zeron;
        });
        cx.refresh_windows();
    });
}

/// The URL of the Browser tab the right pane shows, if it shows one.
fn browser(
    window: WindowHandle<shell::Shell>,
    cx: &mut AsyncApp,
) -> anyhow::Result<Option<String>> {
    window.update(cx, |shell, _, cx| {
        shell
            .fixture_active_browser(cx)
            .and_then(|(_, browser)| browser.read(cx).page.url.clone())
    })
}

async fn toggle_settings(
    window: WindowHandle<shell::Shell>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    any(window).update(cx, |_, w, cx| {
        w.dispatch_action(Box::new(shell::OpenSettings), cx)
    })?;
    pause(cx, 1500).await;
    let _ = frame(window, cx)?;
    pause(cx, 1500).await;
    Ok(())
}

struct Run {
    out: PathBuf,
    lines: Vec<String>,
    failed: usize,
}

impl Run {
    fn check(&mut self, name: &str, expected: &str, ok: bool, observed: String) {
        let line = format!(
            "{} {name}: {expected} (observed {observed})",
            if ok { "PASS" } else { "FAIL" }
        );
        eprintln!("{line}");
        self.failed += usize::from(!ok);
        self.lines.push(line);
    }
}

async fn scenario(
    window: WindowHandle<shell::Shell>,
    run: &mut Run,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let out = run.out.clone();
    let view = destination("Pull request view").expect("the native view");
    let default_browser = destination("Default browser").expect("the default browser");
    let in_app = destination("In-app browser");
    let link = |number: u32| {
        markdown::render::fixture_link(&pr(number))
            .map(|(at, _)| at)
            .ok_or_else(|| anyhow::anyhow!("link #{number} was not painted"))
    };
    pause(cx, 1500).await;
    let _ = frame(window, cx)?;

    // Settings: the select is what changes when the destination does.
    configure(view, true, cx);
    toggle_settings(window, cx).await?;
    let native = frame(window, cx)?;
    configure(default_browser, true, cx);
    toggle_settings(window, cx).await?;
    toggle_settings(window, cx).await?;
    let external = frame(window, cx)?;
    let select = changes(&native, &external)
        .into_iter()
        .filter(|r| r.x0 > WIDTH * 0.5 && r.y1 - r.y0 <= 26.)
        .max_by(|a, b| a.x1.total_cmp(&b.x1))
        .ok_or_else(|| anyhow::anyhow!("the destination select was not found"))?;
    press(window, select.center(), cx).await?;
    pause(cx, 500).await;
    let open = frame(window, cx)?;
    let menu = changes(&external, &open)
        .into_iter()
        .max_by(|a, b| ((a.x1 - a.x0) * (a.y1 - a.y0)).total_cmp(&((b.x1 - b.x0) * (b.y1 - b.y0))))
        .ok_or_else(|| anyhow::anyhow!("the destination menu did not open"))?;
    save(&open, &out, "settings-menu", Some(menu.y1 + 16.))?;
    key(window, "escape", cx).await?;
    toggle_settings(window, cx).await?;

    // "Default browser": a pull request link leaves Zeron.
    configure(default_browser, true, cx);
    settle(window, cx).await?;
    let before = frame(window, cx)?;
    press(window, link(1005)?, cx).await?;
    pause(cx, 2500).await;
    settle(window, cx).await?;
    let after = frame(window, cx)?;
    save(&after, &out, "default-browser", None)?;
    let (shown, changed) = (browser(window, cx)?, changes(&before, &after).len());
    run.check(
        "default-browser",
        "nothing opens in Zeron",
        shown.is_none() && changed == 0,
        format!("Browser tab {shown:?}, {changed} changed areas"),
    );

    // "Open links in Zeron" off: pull request links leave Zeron too (#956).
    configure(view, false, cx);
    let before = frame(window, cx)?;
    press(window, link(1012)?, cx).await?;
    pause(cx, 6000).await;
    settle(window, cx).await?;
    let after = frame(window, cx)?;
    save(&after, &out, "links-off", None)?;
    let (shown, changed) = (browser(window, cx)?, changes(&before, &after).len());
    run.check(
        "links-off",
        "nothing opens in Zeron",
        shown.is_none() && changed == 0,
        format!("Browser tab {shown:?}, {changed} changed areas"),
    );

    // After a link left Zeron, the next link in the message opens on its
    // first press.
    configure(default_browser, false, cx);
    press(window, link(997)?, cx).await?;
    pause(cx, 2500).await;
    configure(in_app.unwrap_or(default_browser), true, cx);
    press(window, link(1019)?, cx).await?;
    pause(cx, 3000).await;
    settle(window, cx).await?;
    let shown = browser(window, cx)?;
    save(&frame(window, cx)?, &out, "next-link", None)?;
    run.check(
        "next-link",
        "#1019 opens on its first press",
        shown.as_deref() == Some(pr(1019).as_str()),
        format!("Browser tab {shown:?}"),
    );

    // "In-app browser": a Browser tab beside the conversation.
    if let Some(in_app) = in_app {
        configure(in_app, true, cx);
        press(window, link(1020)?, cx).await?;
        pause(cx, 3000).await;
        settle(window, cx).await?;
        let shown = browser(window, cx)?;
        save(&frame(window, cx)?, &out, "in-app-browser", None)?;
        run.check(
            "in-app-browser",
            "a Browser tab on #1020",
            shown.as_deref() == Some(pr(1020).as_str()),
            format!("Browser tab {shown:?}"),
        );
    }

    // "Pull request view": Zeron's own view, as before.
    configure(view, true, cx);
    let before = frame(window, cx)?;
    press(window, link(1013)?, cx).await?;
    pause(cx, 6000).await;
    settle(window, cx).await?;
    let after = frame(window, cx)?;
    save(&after, &out, "pull-request-view", None)?;
    let (shown, changed) = (browser(window, cx)?, changes(&before, &after).len());
    run.check(
        "pull-request-view",
        "the view opens, not a Browser tab",
        shown.is_none() && changed > 0,
        format!("Browser tab {shown:?}, {changed} changed areas"),
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("error").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
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
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    // The viewport reads the IPC bearer from its own data directory.
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, core.rpc_service()))?;
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
    let failure = Arc::new(std::sync::Mutex::new(None::<String>));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.surface = zeron_theme::SurfacePreference::Opaque;
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
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                s.local_device_id = Some(device.clone());
                s.devices = vec![
                    serde_json::from_value(serde_json::json!({
                        "id": device, "name": "This device",
                        "platform": std::env::consts::OS, "lastSeenAt": null
                    }))
                    .unwrap(),
                ];
                s.chats = vec![
                    serde_json::from_value(serde_json::json!({
                        "id": "pull-request-links", "deviceId": device,
                        "title": "Review the open pull requests", "archived": false,
                        "createdAt": "2026-10-24T00:00:00Z",
                        "config": {"harness": "claude-code", "model": "claude-sonnet-4-6",
                            "reasoning": null, "sandbox": "workspace-write"}
                    }))
                    .unwrap(),
                ];
                s.selected_chat = Some("pull-request-links".into());
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
                            size(px(WIDTH), px(HEIGHT)),
                        ))),
                        focus: false,
                        show: true,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            let text = format!(
                "Ready for review: {}.",
                [1005, 1012, 997, 1019, 1020, 1013]
                    .map(|n| format!("[#{n}]({})", pr(n)))
                    .join(", ")
            );
            state.update(cx, |s, cx| {
                let entries = serde_json::from_value(serde_json::json!([
                    {"id": "links", "role": "assistant", "deviceId": "local", "status": "complete",
                     "createdAt": 1788900001000_i64,
                     "parts": [{"id": "text", "kind": "text", "text": text}]}
                ]))
                .unwrap();
                s.receive_transcript_frame(
                    zeron_doc::TranscriptFrame::Reset { reset: entries },
                    cx,
                )
                .unwrap();
            });
            cx.spawn(async move |cx| {
                let mut run = Run {
                    out: output.clone(),
                    lines: Vec::new(),
                    failed: 0,
                };
                if let Err(error) = scenario(window, &mut run, cx).await {
                    run.check("scenario", "ran to the end", false, format!("{error:#}"));
                }
                let _ = std::fs::write(output.join("result.txt"), run.lines.join("\n") + "\n");
                if run.failed > 0 {
                    *result.lock().unwrap() = Some(format!("{} check(s) failed", run.failed));
                }
                let _ = any(window).update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                // AppKit's terminate exits without returning from `run`, so a
                // failure has to end the process here to be seen.
                if run.failed > 0 {
                    eprintln!("{} check(s) failed", run.failed);
                    std::process::exit(1);
                }
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
