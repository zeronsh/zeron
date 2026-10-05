//! Reproducible production UI captures. Isolated sample data; no model calls.
use gpui::{
    AppContext, AsyncApp, Bounds, PlatformInput, WindowBounds, WindowOptions, point, px, size,
};
use std::{path::PathBuf, time::Duration};
use zeron_ui::*;

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}
fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        window.draw(cx).clear();
    })?;
    #[cfg(target_os = "linux")]
    {
        // GPUI's Linux platform has no render_to_image implementation. Read
        // only this fixture's own X11 framebuffer, identified by its PID.
        use x11rb::{
            connection::Connection,
            protocol::xproto::{AtomEnum, ConnectionExt, ImageFormat},
        };
        let (connection, screen) = x11rb::connect(None)?;
        let pid_atom = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
        let root = connection.setup().roots[screen].root;
        let mut windows = connection.query_tree(root)?.reply()?.children;
        let mut own_window = None;
        while let Some(candidate) = windows.pop() {
            let property = connection
                .get_property(false, candidate, pid_atom, AtomEnum::CARDINAL, 0, 1)?
                .reply()?;
            if property.value32().and_then(|mut v| v.next()) == Some(std::process::id()) {
                own_window = Some(candidate);
                break;
            }
            if let Ok(tree) = connection.query_tree(candidate)?.reply() {
                windows.extend(tree.children);
            }
        }
        let own_window = own_window.ok_or_else(|| {
            anyhow::anyhow!("fixture window not found; run with DISPLAY set to an X11 server")
        })?;
        let geometry = connection.get_geometry(own_window)?.reply()?;
        let capture_width = geometry.width;
        let capture_height = geometry.height;
        let capture_x = geometry.width.saturating_sub(capture_width) / 2;
        let capture_y = geometry.height.saturating_sub(capture_height);
        let pixels = connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                own_window,
                capture_x as i16,
                capture_y as i16,
                capture_width,
                capture_height,
                u32::MAX,
            )?
            .reply()?;
        let width = usize::from(capture_width);
        let height = usize::from(capture_height);
        anyhow::ensure!(
            pixels.data.len() == width * height * 4,
            "expected 32-bit X11 framebuffer"
        );
        let mut image = image::RgbaImage::new(width as u32, height as u32);
        for (pixel, bytes) in image.pixels_mut().zip(pixels.data.chunks_exact(4)) {
            *pixel = image::Rgba([bytes[2], bytes[1], bytes[0], 255]);
        }
        image.save(output.join(format!("{name}.png")))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        let app = objc2_app_kit::NSApplication::sharedApplication(
            objc2::MainThreadMarker::new().unwrap(),
        );
        let window = app
            .keyWindow()
            .or_else(|| app.mainWindow())
            .ok_or_else(|| anyhow::anyhow!("fixture window not available"))?;
        let status = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-o", "-l", &window.windowNumber().to_string()])
            .arg(output.join(format!("{name}.png")))
            .status()?;
        anyhow::ensure!(status.success(), "fixture capture failed");
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    anyhow::bail!("fixture capture supports Linux/X11 and macOS")
}
fn dispatch(
    window: gpui::AnyWindowHandle,
    event: PlatformInput,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.dispatch_event(event, cx);
    })?;
    Ok(())
}
async fn drag(
    window: gpui::AnyWindowHandle,
    from: gpui::Point<gpui::Pixels>,
    to: gpui::Point<gpui::Pixels>,
    cx: &mut AsyncApp,
    output: &std::path::Path,
    frame: &mut usize,
) -> anyhow::Result<()> {
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            position: from,
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    for i in 1..=36 {
        let t = i as f32 / 36.;
        let t = t * t * (3. - 2. * t);
        let position = from + (to - from) * t;
        dispatch(
            window,
            PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                pressed_button: Some(gpui::MouseButton::Left),
                modifiers: Default::default(),
            }),
            cx,
        )?;
        pause(cx, 25).await;
        capture(window, cx, output, &format!("frame-{frame:04}"))?;
        *frame += 1;
    }
    capture(window, cx, output, "dragging")?;
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            position: to,
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 600).await;
    for _ in 0..24 {
        capture(window, cx, output, &format!("frame-{frame:04}"))?;
        *frame += 1;
    }
    Ok(())
}
fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let result = std::sync::Arc::new(std::sync::Mutex::new(None));
    let failure = result.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_width = 280.;
        settings.sidebar_organization = settings::SidebarOrganization::InOneList;
        settings.surface = zeron_theme::SurfacePreference::Opaque;
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        appearance::init(appearance::AppearanceMode::Dark, settings.theme_selection, settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths, settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior); terminal::panel::init(cx); app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zeron_proto::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
            s.local_device_id = Some("local".into()); s.selected_device = Some("local".into());
            s.no_project = true; s.auto_selected = true; s.chats_synced = true; s.spaces_synced = true;
            s.devices = vec![serde_json::from_value(serde_json::json!({"id":"local","name":"This device","platform":"linux","capabilities":zeron_proto::capabilities::current()})).unwrap()];
            s.chats = [ ("target", "Implement the login flow"), ("reference", "Authentication design"), ("checklist", "Rollout checklist") ].into_iter().enumerate().map(|(i,(id,title))| serde_json::from_value(serde_json::json!({"id":id,"deviceId":"local","title":title,"archived":false,"createdAt":chrono::Utc::now()-chrono::Duration::minutes(i as i64),"config":{"harness":"claude-code","model":"claude-sonnet-4-6","sandbox":"workspace-write"}})).unwrap()).collect();
            s.selected_chat = Some("target".into());
            s
        });
        let boot = EngineBootConfig { data_dir: data, ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::Mock };
        let window = cx.open_window(WindowOptions { kind: gpui::WindowKind::Dialog, window_background: theme::Theme::of(cx).window_background_appearance(), window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(20.),px(40.)),size(px(1100.),px(740.))))), ..Default::default() }, |_, cx| cx.new(|cx| {
            let mut shell = shell::Shell::new(state.clone(), boot, cx);
            shell.fixture_chat_context_prepare(cx);
            shell
        })).unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                pause(cx, 1200).await;
                capture(window.into(), cx, &output, "before")?;
                // Coordinates are deterministic for this 1100x740 production shell.
                let viewport = window.update(cx, |_, w, _| w.viewport_size())?;
                let source = point(px(145.), px(158.));
                let mut frame = 0;
                for _ in 0..24 { capture(window.into(), cx, &output, &format!("frame-{frame:04}"))?; frame += 1; }
                drag(window.into(), source, point((viewport.width + px(280.)) / 2.,viewport.height / 2.), cx, &output, &mut frame).await?;
                let raw = window.update(cx, |s, _, cx| s.fixture_chat_context_text(false, cx))?;
                let references = zeron_proto::chat_mentions::chat_mention_links(&raw);
                anyhow::ensure!(references.len() == 1 && references[0].1.chat_id == "reference", "main drop failed: {raw:?}");
                capture(window.into(), cx, &output, "main-context-dark")?;
                window.update(cx, |s, _, cx| s.fixture_chat_context_side(cx))?;
                pause(cx, 700).await;
                drag(window.into(), source, point(viewport.width - px(100.),viewport.height / 2.), cx, &output, &mut frame).await?;
                let side = window.update(cx, |s, _, cx| s.fixture_chat_context_text(true, cx))?;
                anyhow::ensure!(zeron_proto::chat_mentions::chat_mention_links(&side).len() == 1, "side drop failed: {side:?}");
                capture(window.into(), cx, &output, "side-context-dark")?;
                cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                pause(cx, 500).await;
                capture(window.into(), cx, &output, "side-context-light")?;
                std::fs::write(output.join("result.txt"), format!("Production sidebar → main and side composer drops verified.\nMain: {raw}\nSide: {side}\nFrames: {frame}\n"))?;
                Ok(())
            }.await;
            if let Err(error) = run { eprintln!("Chat context fixture: {error:#}"); *failure.lock().unwrap() = Some(format!("{error:#}")); }
            let _ = window.update(cx, |_, w, _| w.blur());
            let _ = gpui::AnyWindowHandle::from(window).update(cx, |_, w, cx| w.draw(cx).clear());
            let _ = window.update(cx, |_, w, _| w.remove_window());
            pause(cx, 1000).await;
            cx.update(|cx| cx.quit());
        }).detach();
    });
    if let Some(error) = result.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
