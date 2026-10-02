//! Native framebuffer fixture; optional direct session via ZERON_RDP_HOST.
//! Never initializes the engine. See docs/reference/remote-desktop.md.
use gpui::{prelude::*, *};
use std::{sync::Arc, time::Duration};
use zeron_ui::remote_desktop::desktop::Desktop;
struct Fixture {
    desktop: Entity<Desktop>,
    live: Option<Entity<zeron_ui::remote_desktop::RemoteDesktopSurface>>,
    _task: Task<()>,
}
impl Fixture {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let desktop = cx.new(|cx| Desktop::new(window, cx));
        let output = desktop.downgrade();
        let task = cx.spawn_in(window, async move |_, cx| {
            let mut sequence = 0;
            loop {
                if std::env::var_os("ZERON_RDP_HOST").is_some() {
                    break;
                }
                sequence += 1;
                let (width, height) = (1280u16, 800u16);
                let mut bgra = vec![0; width as usize * height as usize * 4];
                for y in 0..height as usize {
                    for x in 0..width as usize {
                        let colors = [
                            [0, 0, 255, 255],
                            [0, 255, 0, 255],
                            [255, 0, 0, 255],
                            [255, 255, 255, 255],
                        ];
                        let mut color = colors[(x / 320).min(3)];
                        if y > 500 {
                            color = if (x / 2 + y / 2) % 2 == 0 {
                                [255; 4]
                            } else {
                                [0, 0, 0, 255]
                            };
                        }
                        if x.abs_diff((sequence as usize * 8) % 1280) < 15
                            && (250..350).contains(&y)
                        {
                            color = [0, 255, 255, 255];
                        }
                        bgra[(y * width as usize + x) * 4..(y * width as usize + x + 1) * 4]
                            .copy_from_slice(&color);
                    }
                }
                let frame = zeron_rdp::Frame {
                    generation: 1,
                    sequence,
                    width,
                    height,
                    bgra: Arc::from(bgra),
                };
                if output
                    .update_in(cx, |view, window, cx| view.update_frame(&frame, window, cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(34))
                    .await;
            }
        });
        let live = std::env::var("ZERON_RDP_HOST").ok().map(|host| {
            let profile = zeron_ui::remote_desktop::profiles::Profile {
                name: "Local RDP fixture".into(),
                host,
                port: std::env::var("ZERON_RDP_PORT")
                    .unwrap_or_else(|_| "3389".into())
                    .parse()
                    .unwrap(),
                username: std::env::var("ZERON_RDP_USER").expect("ZERON_RDP_USER"),
                keyboard_layout: 0x040a,
                trusted_certificate_sha256: std::env::var("ZERON_RDP_CERT_SHA256").ok(),
                ..Default::default()
            };
            cx.new(|cx| {
                let mut view = zeron_ui::remote_desktop::RemoteDesktopSurface::new(
                    std::env::temp_dir(),
                    window,
                    cx,
                );
                view.fixture_connect(
                    profile,
                    std::env::var("ZERON_RDP_PASSWORD").expect("ZERON_RDP_PASSWORD"),
                    window,
                    cx,
                );
                view
            })
        });
        Self {
            desktop,
            live,
            _task: task,
        }
    }
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if let Some(live) = &self.live {
            return div()
                .size_full()
                .bg(rgb(0x20242a))
                .child(live.clone())
                .into_any_element();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x20242a))
            .text_color(rgb(0xffffff))
            .child(div().h(px(36.)).child(
                "Remote Desktop · red / green / blue / white · 1280 × 800 · offline fixture",
            ))
            .child(div().flex_1().min_h_0().child(self.desktop.clone()))
            .into_any_element()
    }
}
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("zeron_rdp=info,gpui_wgpu=info")
        .init();
    let data = tempfile::tempdir()?;
    let output = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    if let Some(output) = &output {
        std::fs::create_dir_all(output)?;
    }
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    let platform = gpui_platform::current_platform(false);
    gpui::Application::with_platform(platform.clone())
        .with_assets(zeron_ui::icons::Assets)
        .run(move |cx| {
            zeron_ui::remote_desktop::cursor::init(platform, cx);
            gpui_base::init(cx);
            zeron_ui::remote_desktop::input::bind_keys(cx);
            cx.set_global(zeron_ui::theme::Theme::default());
            zeron_ui::settings::init(Default::default(), data.path(), cx);
            let settings = zeron_ui::settings::current(cx);
            let fonts = zeron_ui::typography::register_fonts(cx);
            zeron_ui::typography::init(
                settings.ui_font_family,
                settings.ui_font_size,
                settings.terminal_font_family,
                settings.terminal_font_size,
                settings.code_font_family,
                settings.code_font_size,
                fonts,
                cx,
            );
            let window = cx
                .open_window(
                    WindowOptions {
                        titlebar: Some(TitlebarOptions {
                            title: Some("Zeron Remote Desktop fixture".into()),
                            ..Default::default()
                        }),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(
                                px(std::env::var("ZERON_RDP_FIXTURE_WIDTH")
                                    .ok()
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(800.)),
                                px(600.),
                            ),
                            cx,
                        ))),
                        ..Default::default()
                    },
                    |window, cx| cx.new(|cx| Fixture::new(window, cx)),
                )
                .unwrap();
            cx.activate(true);
            if let Some(output) = output {
                cx.spawn(async move |cx| {
                    let duration = std::env::var("ZERON_RDP_FIXTURE_SECONDS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(8);
                    cx.background_executor()
                        .timer(Duration::from_secs(duration))
                        .await;
                    let captured = (|| -> anyhow::Result<()> {
                        window.update(cx, |fixture, _, cx| {
                            if let Some(live) = &fixture.live {
                                anyhow::ensure!(
                                    live.read(cx).fixture_has_frame(cx),
                                    "Live fixture did not receive a desktop frame"
                                );
                            }
                            Ok::<_, anyhow::Error>(())
                        })??;
                        capture(&output, "desktop")?;
                        if std::env::var_os("ZERON_RDP_HOST").is_none() {
                            validate_colors(&output.join("desktop.png"))?;
                        }
                        std::fs::write(output.join("result.txt"), "ok\n")?;
                        Ok(())
                    })();
                    if let Err(error) = captured {
                        *failure.lock().unwrap() = Some(error.to_string());
                    }
                    let _ = window.update(cx, |_, window, _| window.remove_window());
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        });
    if let Some(error) = result.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
fn validate_colors(path: &std::path::Path) -> anyhow::Result<()> {
    let frame = image::open(path)?.to_rgb8();
    for color in [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]] {
        let count = frame
            .pixels()
            .filter(|p| p.0.iter().zip(color).all(|(a, b)| a.abs_diff(b) < 8))
            .count();
        anyhow::ensure!(
            count > 500,
            "Missing expected color {color:?}: only {count} pixels"
        );
    }
    Ok(())
}

fn capture(directory: &std::path::Path, name: &str) -> anyhow::Result<()> {
    let path = directory.join(format!("{name}.png"));
    #[cfg(target_os = "macos")]
    let status = {
        let app = objc2_app_kit::NSApplication::sharedApplication(
            objc2::MainThreadMarker::new().unwrap(),
        );
        let window = app
            .keyWindow()
            .or_else(|| app.mainWindow())
            .ok_or_else(|| anyhow::anyhow!("fixture window is not available"))?;
        std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", "-l", &window.windowNumber().to_string()])
            .arg(&path)
            .status()?
    };
    #[cfg(not(target_os = "macos"))]
    let status = {
        let capture_window = std::env::var("ZERON_RDP_CAPTURE_WINDOW").ok();
        let windows = std::process::Command::new("xdotool")
            .args([
                "search",
                "--onlyvisible",
                "--pid",
                &std::process::id().to_string(),
            ])
            .output()?;
        let id = capture_window
            .or_else(|| {
                String::from_utf8(windows.stdout)
                    .ok()?
                    .lines()
                    .next()
                    .map(str::to_owned)
            })
            .ok_or_else(|| anyhow::anyhow!("fixture window not visible"))?;
        std::process::Command::new("import")
            .args(["-window", &id])
            .arg(&path)
            .status()?
    };
    anyhow::ensure!(status.success(), "screenshot capture failed");
    Ok(())
}
