//! Native production Shell/Transcript evidence with a deterministic child-control
//! harness and isolated data. No model calls or user conversations are used.
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, ToolCall,
};
use zeron_ui::*;

const CHAT: &str = "subagent-fixture";
struct FixtureHarness;
#[async_trait]
impl Harness for FixtureHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Codex
    }
    fn display_name(&self) -> &str {
        "Fixture"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn supports_subagent_stop(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        _: RunRequest,
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut events = vec![AgentEvent::TextDelta { text: "I’ve split the investigation across three subagents. You can stop an active subagent from its task card while the other agents continue.".into() }];
            for (id, name, description, finished) in [
                (
                    "researcher",
                    "researcher",
                    "Trace child status notifications",
                    false,
                ),
                (
                    "reviewer",
                    "reviewer",
                    "Check cancellation and recovery",
                    false,
                ),
                (
                    "reporter",
                    "reporter",
                    "Review the regression coverage",
                    true,
                ),
            ] {
                events.push(AgentEvent::ToolCall {
                    id: id.into(),
                    call: ToolCall::Unknown {
                        name: format!("Agent: {name}"),
                        input: Some(
                            serde_json::json!({"description":description,"model":"gpt-6.1-sol"}),
                        ),
                    },
                });
                events.push(AgentEvent::Subagent {
                    parent_tool_use_id: id.into(),
                    event: Box::new(AgentEvent::TextDelta {
                        text: "Inspecting the assigned code paths.".into(),
                    }),
                });
                if finished {
                    events.push(AgentEvent::Subagent {
                        parent_tool_use_id: id.into(),
                        event: Box::new(AgentEvent::Done {
                            status: DoneStatus::Completed,
                            result: None,
                            error: None,
                            session_id: None,
                        }),
                    });
                }
                events.push(AgentEvent::ToolResult {
                    id: id.into(),
                    is_error: false,
                    output: None,
                    diff: None,
                });
            }
            events.push(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            });
            for event in events {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            let mut control = controls.subagent_control.take().unwrap();
            loop {
                tokio::select! {
                    _ = controls.interrupt.cancelled() => break,
                    Some(command) = control.recv() => {
                        // Leave enough time to capture the pending affordance.
                        tokio::time::sleep(Duration::from_millis(700)).await;
                        let _ = command.reply.send(Ok(()));
                    }
                }
            }
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
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
        // Capture the conversation region so the lifecycle controls remain
        // legible even when a tiling compositor ignores requested bounds.
        let capture_width = geometry.width.min(1320);
        let capture_height = geometry.height.min(520);
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
fn entries(core: &zeron_engine::EngineCore) -> Vec<zeron_doc::SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
}
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let registry = zeron_engine::HarnessRegistry::new();
    registry.register(Arc::new(FixtureHarness));
    let core = Arc::new(runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(registry),
            HarnessId::Codex,
            None,
        )
    })?);
    let project = temp.path().join("status-investigation");
    std::fs::create_dir(&project)?;
    core.workspace.create_space(
        "project",
        &core.device_id,
        &project.to_string_lossy(),
        Some("Subagent lifecycle".into()),
        false,
    )?;
    core.workspace.create_chat(
        CHAT,
        Some("project"),
        Some(&core.device_id),
        Some(zeron_proto::ChatConfig {
            harness: HarnessId::Codex,
            model: Some("gpt-6.1-sol".into()),
            reasoning: None,
            model_options: Default::default(),
            sandbox: SandboxLevel::WorkspaceWrite,
        }),
        None,
    )?;
    core.workspace
        .rename_chat(CHAT, "Investigate subagent status")?;
    runtime.block_on(core.sessions.dispatch(CHAT, HarnessId::Codex, RunRequest { mcp: None, prompt: "Investigate the subagent status bug and check that individual agents can be stopped.".into(), harness: None, model: None, reasoning: None, model_options: Default::default(), cwd: project.to_string_lossy().into_owned(), sandbox: SandboxLevel::WorkspaceWrite, auto_approve: false, attachments: vec![], worktree: None, resume: None }, None))?;
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
    let spaces = core.workspace.read_spaces()?;
    let device = core.device_id.clone();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    let running_core = core.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let settings = settings::UiSettings::default(); settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx); typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx); appearance::init(appearance::AppearanceMode::Dark, settings.theme_selection, settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths, settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior); terminal::panel::init(cx); app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new(); s.fixture_subagent_engine(handle);
            s.connection = zeron_proto::view::ConnectionStatus::Ready; s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
            s.local_device_id = Some(device.clone()); s.devices = vec![serde_json::from_value(serde_json::json!({"id":device,"name":"This device","platform":std::env::consts::OS})).unwrap()];
            s.chats = chats; s.spaces = spaces; s.selected_chat = Some(CHAT.into()); s.selected_space = Some("project".into()); s.auto_selected = true; s.chats_synced = true; s.spaces_synced = true; s
        });
        let window = cx.open_window(WindowOptions { window_background: theme::Theme::of(cx).window_background_appearance(), window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(20.),px(40.)),size(px(1100.),px(720.))))), ..Default::default() }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                pause(cx, 800).await;
                state.update(cx, |s, cx| { s.receive_transcript_frame(zeron_doc::TranscriptFrame::reset(&entries(&running_core)), cx).unwrap(); cx.notify(); });
                pause(cx, 600).await;
                window.update(cx, |s, _, cx| s.fixture_subagent_start(cx))?;
                pause(cx, 300).await; capture(window.into(), cx, &output, "subagents-active-dark")?;
                window.update(cx, |s, _, cx| s.fixture_stop_subagent(CHAT, "researcher", cx))?;
                pause(cx, 200).await; capture(window.into(), cx, &output, "subagent-stopping-dark")?;
                pause(cx, 900).await;
                let stopped = entries(&running_core);
                let status = |id: &str| stopped.iter().flat_map(|e| &e.parts).find_map(|p| match p { zeron_doc::MessagePart::Tool { id: pid, subagent_status, .. } if pid == id => *subagent_status, _ => None });
                anyhow::ensure!(status("researcher") == Some(zeron_doc::SubagentStatus::Done), "target did not stop");
                anyhow::ensure!(status("reviewer") == Some(zeron_doc::SubagentStatus::Running), "sibling was stopped");
                state.update(cx, |s, cx| { s.receive_transcript_frame(zeron_doc::TranscriptFrame::reset(&stopped), cx).unwrap(); cx.notify(); });
                pause(cx, 300).await; capture(window.into(), cx, &output, "subagent-stopped-dark")?;
                cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx)); pause(cx, 500).await;
                capture(window.into(), cx, &output, "subagent-stopped-light")?;
                std::fs::write(output.join("result.txt"), "PASS: production Shell/Transcript; child Stop RPC acknowledged; target Done, sibling Running; dark/light native captures. Deterministic fixture, no model calls.\n")?;
                Ok(())
            }.await;
            if let Err(error) = run { eprintln!("Subagent fixture failed: {error:#}"); *result.lock().unwrap() = Some(error.to_string()); }
            drop(state);
            let _ = gpui::AnyWindowHandle::from(window).update(cx, |_, window, cx| { window.disable_focus(); window.draw(cx).clear(); });
            pause(cx, 100).await;
            let _ = window.update(cx, |_, window, _| window.remove_window());
            pause(cx, 100).await; cx.update(|cx| cx.quit());
        }).detach();
    });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
