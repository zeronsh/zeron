//! Native GPUI evidence. Isolated engine, synthetic provider, no model calls.
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeron_harness::{Harness, HarnessError, NativeForkControls, NativeForkError, RunControls};
use zeron_proto::*;
use zeron_ui::*;
struct Fixture;
#[async_trait::async_trait]
impl Harness for Fixture {
    fn id(&self) -> HarnessId {
        HarnessId::Codex
    }
    fn display_name(&self) -> &str {
        "Native fork fixture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn native_fork_support(&self, _: &std::path::Path) -> NativeForkAvailability {
        NativeForkAvailability::available()
    }
    async fn fork_native(
        &self,
        point: &NativeForkPoint,
        _: NativeForkControls,
    ) -> Result<NativeForkResult, NativeForkError> {
        assert_eq!(
            point.boundary,
            NativeForkBoundary::AppServerTurn {
                turn_id: "native-a1".into()
            }
        );
        Ok(NativeForkResult {
            session_id: "fixture-native-child".into(),
            cwd: point.cwd.clone(),
        })
    }
    async fn run(
        &self,
        _: RunRequest,
        _: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        panic!("Visual fixture must never start inference")
    }
}
async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}
fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    dir: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.present_if_needed();
        #[cfg(target_os = "linux")]
        std::thread::sleep(Duration::from_millis(100));
        #[cfg(not(target_os = "linux"))]
        w.render_to_image()?.save(dir.join(format!("{name}.png")))?;
        #[cfg(target_os = "linux")]
        {
            use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};
            let (connection, screen) = x11rb::connect(None)?;
            let root = connection.setup().roots[screen].root;
            // Run on an isolated Xvfb display; decode the sole application window.
            let mut window = connection
                .query_tree(root)?
                .reply()?
                .children
                .into_iter()
                .find(|id| {
                    connection
                        .get_geometry(*id)
                        .ok()
                        .and_then(|r| r.reply().ok())
                        .is_some_and(|g| g.width > 300 && g.height > 300)
                })
                .ok_or_else(|| anyhow::anyhow!("No fixture window"))?;
            // A window manager supplies real resize/focus events. Descend past
            // its frame so captures contain only the application's pixels.
            while let Some(child) = connection
                .query_tree(window)?
                .reply()?
                .children
                .into_iter()
                .find(|id| {
                    connection
                        .get_geometry(*id)
                        .ok()
                        .and_then(|r| r.reply().ok())
                        .is_some_and(|g| g.width > 300 && g.height > 300)
                })
            {
                window = child;
            }
            let geometry = connection.get_geometry(window)?.reply()?;
            let position = connection
                .translate_coordinates(window, root, 0, 0)?
                .reply()?;
            // Read the displayed pixels, including the Vulkan presentation surface.
            let (image, visual) = x11rb::image::Image::get(
                &connection,
                root,
                position.dst_x,
                position.dst_y,
                geometry.width,
                geometry.height,
            )?;
            let visual = connection
                .setup()
                .roots
                .iter()
                .flat_map(|s| &s.allowed_depths)
                .flat_map(|d| &d.visuals)
                .find(|v| v.visual_id == visual)
                .unwrap();
            let layout = x11rb::image::PixelLayout::from_visual_type(*visual)?;
            let mut pixels = Vec::new();
            for y in 0..geometry.height {
                for x in 0..geometry.width {
                    let (r, g, b) = layout.decode(image.get_pixel(x, y));
                    pixels.extend_from_slice(&[
                        (r >> 8) as u8,
                        (g >> 8) as u8,
                        (b >> 8) as u8,
                        255,
                    ]);
                }
            }
            image::save_buffer(
                dir.join(format!("{name}.png")),
                &pixels,
                geometry.width.into(),
                geometry.height.into(),
                image::ColorType::Rgba8,
            )?;
        }
        Ok(())
    })?
}
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _runtime_guard = runtime.enter();
    let registry = Arc::new(zeron_engine::HarnessRegistry::new());
    registry.register(Arc::new(Fixture));
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            registry,
            HarnessId::Codex,
            None,
        )
    })?;
    core.workspace
        .rename_device(&core.device_id, "Fixture host")?;
    let cwd = temp.path().to_string_lossy().into_owned();
    core.workspace.create_chat(
        "native-fixture",
        None,
        Some(&core.device_id),
        None,
        Some(cwd.clone()),
    )?;
    core.workspace
        .rename_chat("native-fixture", "Explore a different approach")?;
    let mut chat = serde_json::to_value(core.workspace.chat("native-fixture")?.unwrap())?;
    chat["config"] = serde_json::json!({"harness":"codex","sandbox":"workspace-write"});
    core.workspace
        .import_chat_row(&serde_json::from_value(chat)?)?;
    core.workspace
        .set_chat_harness_session("native-fixture", "fixture-parent", &cwd);
    let doc = core.doc_host.open("native-fixture")?;
    for (id, role, text) in [
        (
            "u1",
            "user",
            "Suggest a simple way to organize my project notes.",
        ),
        (
            "a1",
            "assistant",
            "Start with three folders: **Ideas**, **In progress**, and **Reference**.\n\nKeep one short index that links to your active notes.",
        ),
        ("u2", "user", "Now describe a more elaborate system."),
        (
            "a2",
            "assistant",
            "You could add tags, a weekly review, and a separate archive. This later reply will stay in the original conversation.",
        ),
    ] {
        let entry: zeron_doc::SessionMessageEntry = serde_json::from_value(
            serde_json::json!({"id":id,"role":role,"parts":[{"kind":"text","id":format!("{id}-text"),"text":text}],"createdAt":1790602320000_i64,"deviceId":core.device_id,"status":"complete"}),
        )?;
        doc.doc().push_message(&entry)?;
    }
    doc.doc().set_native_fork_point(
        "a1",
        &NativeForkPoint {
            format_version: 1,
            harness: HarnessId::Codex,
            source_device_id: core.device_id.clone(),
            source_session_id: "fixture-parent".into(),
            cwd,
            boundary: NativeForkBoundary::AppServerTurn {
                turn_id: "native-a1".into(),
            },
        },
    )?;
    let ipc_port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
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
        default_harness: HarnessId::Codex,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let chats = core.workspace.read_chats()?;
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let settings = settings::UiSettings::default();
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
            let state = cx.new(|cx| {
                let mut s = state::AppState::new();
                s.chats = chats;
                s.auto_selected = true;
                s.fixture_native_fork_engine(handle, cx);
                s.select_chat(Some("native-fixture".into()), cx);
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(20.), px(40.)),
                            size(px(1200.), px(850.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    pause(cx, 2200).await;
                    window.update(cx, |s, _, cx| s.fixture_native_fork_reveal("a1", cx))?;
                    pause(cx, 250).await;
                    capture(window.into(), cx, &output, "native-fork-dark")?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    pause(cx, 350).await;
                    capture(window.into(), cx, &output, "native-fork-light")?;
                    window.update(cx, |s, _, cx| s.fixture_native_fork_reveal("a2", cx))?;
                    pause(cx, 350).await;
                    capture(window.into(), cx, &output, "native-fork-no-native-point")?;
                    window.update(cx, |s, _, cx| s.fixture_native_fork_create(cx))?;
                    pause(cx, 1500).await;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-fork-historical-side-chat",
                    )?;
                    window.update(cx, |_, w, cx| {
                        w.dispatch_action(Box::new(shell::ToggleSidebar), cx);
                        w.resize(size(px(900.), px(850.)));
                    })?;
                    pause(cx, 500).await;
                    capture(window.into(), cx, &output, "native-fork-narrow")?;
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("Capture failed: {error:#}");
                    *result.lock().unwrap() = Some(error.to_string());
                }
                let any_window: gpui::AnyWindowHandle = window.into();
                let _ = any_window.update(cx, |_, w, cx| {
                    w.blur();
                    w.refresh();
                    w.draw(cx).clear();
                    w.present_if_needed();
                });
                pause(cx, 200).await;
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 300).await;
                cx.update(|cx| {
                    cx.clear_globals();
                    cx.quit();
                });
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
