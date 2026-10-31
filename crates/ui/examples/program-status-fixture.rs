//! Production Shell screenshots driven by real OSC reports in isolated PTYs.
//! All profiles, reports, and PNGs live outside the repository. No agent runs.
#[cfg(not(unix))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("This capture fixture requires Unix PTYs")
}

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
    use std::{path::PathBuf, sync::Arc, time::Duration};
    use zeron_proto::{ChatIndicator, Session};
    use zeron_ui::*;

    fn summary(core: &zeron_engine::EngineCore) -> (Vec<Session>, String) {
        let sessions = core.sessions.watch_sessions().borrow().clone();
        let mut text = String::from(
            "# Agent status\n\nThese sessions report their state through OSC 7501. The sidebar and MCP use the same status derivation.\n\n| Session | Sidebar / MCP status | Program report |\n| --- | --- | --- |\n",
        );
        for id in ["working", "blocked", "done"] {
            let chat = core.workspace.chat(id).unwrap().unwrap();
            let session = sessions.iter().find(|s| s.chat_id == id);
            let status = zeron_proto::view::display_status(&chat, session, chrono::Utc::now());
            let report = session
                .and_then(|s| s.terminal_activity.as_ref())
                .map(|a| serde_json::to_value(a.report.state).unwrap())
                .unwrap_or(serde_json::Value::String("acknowledged".into()));
            text.push_str(&format!(
                "| {} | `{}` | `{}` |\n",
                chat.title.as_deref().unwrap(),
                serde_json::to_value(status).unwrap().as_str().unwrap(),
                report.as_str().unwrap()
            ));
        }
        text.push_str("\n**Needs input:** Approve the deployment?\n\nA blocked session stays visible until the program resumes. Done marks a result ready to review and clears when acknowledged.\n\nTerminal reports describe activity. Native turn completion receipts and Stop controls stay independent.");
        (sessions, text)
    }

    fn show_report(
        state: &gpui::Entity<state::AppState>,
        core: &zeron_engine::EngineCore,
        cx: &mut gpui::AsyncApp,
    ) {
        let (sessions, text) = summary(core);
        state.update(cx, |state, cx| {
            state.apply_chats(core.workspace.read_chats().unwrap());
            state.apply_sessions(sessions);
            state
                .receive_transcript_frame(
                    zeron_doc::TranscriptFrame::Reset {
                        reset: serde_json::from_value(serde_json::json!([{
                            "id":"status-report", "role":"assistant", "deviceId":core.device_id,
                            "createdAt":chrono::Utc::now().timestamp_millis(), "status":"complete",
                            "parts":[{"id":"text","kind":"text","text":text}]
                        }]))
                        .unwrap(),
                    },
                    cx,
                )
                .unwrap();
            cx.notify();
        });
    }

    async fn capture(
        window: gpui::AnyWindowHandle,
        cx: &mut gpui::AsyncApp,
        output: &std::path::Path,
        name: &str,
    ) -> anyhow::Result<()> {
        #[cfg(target_os = "linux")]
        {
            let selector = format!("\"pid:{}\"", std::process::id());
            let clients = std::process::Command::new("hyprctl")
                .args(["-j", "clients"])
                .output()?;
            let clients: Vec<serde_json::Value> = serde_json::from_slice(&clients.stdout)?;
            let own = clients
                .iter()
                .find(|client| client["pid"].as_u64() == Some(u64::from(std::process::id())))
                .ok_or_else(|| anyhow::anyhow!("Fixture window is not mapped"))?;
            if own["floating"] != true {
                let status = std::process::Command::new("hyprctl")
                    .args([
                        "eval",
                        &format!("hl.dispatch(hl.dsp.window.float({{window={selector}}}))"),
                    ])
                    .status()?;
                anyhow::ensure!(status.success(), "Could not float the capture fixture");
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }
            let status = std::process::Command::new("hyprctl").args(["eval", &format!(
                "local w={selector}; hl.dispatch(hl.dsp.window.resize({{window=w,x=1200,y=820}})); hl.dispatch(hl.dsp.window.center({{window=w}})); hl.dispatch(hl.dsp.focus({{window=w}}))"
            )]).status()?;
            anyhow::ensure!(status.success(), "Could not size the capture fixture");
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
        }
        window.update(cx, |_, window, cx| {
            window.draw(cx).clear();
            window.present_if_needed();
        })?;
        cx.background_executor()
            .timer(Duration::from_millis(120))
            .await;
        #[cfg(target_os = "linux")]
        {
            // Linux has no PlatformWindow::render_to_image implementation.
            // Crop exactly this fixture's visible Hyprland window; refuse a
            // focus change instead of capturing another application's content.
            let active = std::process::Command::new("hyprctl")
                .args(["-j", "activewindow"])
                .output()?;
            let active: serde_json::Value = serde_json::from_slice(&active.stdout)?;
            anyhow::ensure!(
                active["pid"].as_u64() == Some(u64::from(std::process::id())),
                "Fixture lost focus before capture"
            );
            let position = active["at"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Missing window position"))?;
            let dimensions = active["size"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Missing window dimensions"))?;
            let region = format!(
                "{},{} {}x{}",
                position[0], position[1], dimensions[0], dimensions[1]
            );
            let status = std::process::Command::new("grim")
                .arg("-g")
                .arg(region)
                .arg(output.join(format!("{name}.png")))
                .status()?;
            anyhow::ensure!(status.success(), "Native window capture failed");
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        window.update(cx, |_, window, _| {
            window
                .render_to_image()?
                .save(output.join(format!("{name}.png")))?;
            anyhow::Ok(())
        })?
    }

    tracing_subscriber::fmt().with_env_filter("warn").init();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    anyhow::ensure!(
        !output.starts_with(&repository),
        "Captures must stay outside the repository"
    );
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let core = Arc::new(runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::HarnessRegistry::new()),
            HarnessId::Mock,
            None,
        )
    })?);
    let project = temp.path().join("program-status");
    std::fs::create_dir_all(&project)?;
    core.workspace.create_space(
        "project",
        &core.device_id,
        project.to_str().unwrap(),
        Some("Program status".into()),
        false,
    )?;
    for (id, title, report) in [
        ("overview", "Agent status overview", None),
        (
            "working",
            "Compile workspace",
            Some("state=working:app=cargo:progress=42"),
        ),
        (
            "blocked",
            "Approve deployment",
            Some("state=blocked:app=deploy:kind=permission:msg=QXBwcm92ZSB0aGUgZGVwbG95bWVudD8="),
        ),
        ("done", "Run test suite", Some("state=done:app=cargo")),
    ] {
        core.workspace
            .create_chat(id, Some("project"), None, None, None)?;
        core.workspace.rename_chat(id, title)?;
        if let Some(report) = report {
            let terminal = core.terminals.open_with_shell(
                project.to_str().unwrap(),
                80,
                24,
                Some("/bin/sh"),
            )?;
            core.terminals.associate_chat(&terminal.id, id)?;
            core.terminals.write_bytes(
                &terminal.id,
                format!("printf '\\033]7501;{report}\\007'\r").as_bytes(),
            )?;
        }
    }
    runtime.block_on(async {
        let mut rows = core.sessions.watch_sessions();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let ready = [
                    ("working", ChatIndicator::Working),
                    ("blocked", ChatIndicator::AwaitingInput),
                    ("done", ChatIndicator::Completed),
                ]
                .iter()
                .all(|(id, expected)| {
                    let chat = core.workspace.chat(id).unwrap().unwrap();
                    let sessions = rows.borrow();
                    let session = sessions.iter().find(|row| row.chat_id == *id);
                    zeron_proto::view::display_status(&chat, session, chrono::Utc::now())
                        == *expected
                });
                if ready {
                    break;
                }
                rows.changed().await.unwrap();
            }
        })
        .await
    })?;
    std::fs::write(
        output.join("session-snapshot.json"),
        serde_json::to_vec_pretty(&summary(&core).0)?,
    )?;
    let data = temp.path().join("ui");
    std::fs::create_dir_all(&data)?;
    let render_core = core.clone();
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx); gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.sidebar_width = 310.;
        settings.sidebar_organization = settings::SidebarOrganization::InOneList;
        settings.surface = zeron_theme::SurfacePreference::Opaque;
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size,
            settings.terminal_font_family.clone(), settings.terminal_font_size,
            settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        appearance::init(appearance::AppearanceMode::Dark, settings.theme_selection,
            settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths,
            settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior);
        terminal::panel::init(cx); app_menus::init(cx);
        let state = cx.new(|_| {
            let mut state = state::AppState::new();
            state.connection = zeron_proto::view::ConnectionStatus::Ready;
            state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
            state.local_device_id = Some(render_core.device_id.clone());
            state.devices = serde_json::from_value(serde_json::json!([{
                "id":render_core.device_id,"name":"This device","platform":std::env::consts::OS,"lastSeenAt":null
            }])).unwrap();
            state.chats = render_core.workspace.read_chats().unwrap();
            state.spaces = render_core.workspace.read_spaces().unwrap();
            state.apply_sessions(summary(&render_core).0);
            state.selected_chat = Some("overview".into());
            state.selected_space = Some("project".into());
            state.auto_selected = true; state.chats_synced = true; state.spaces_synced = true;
            state
        });
        let boot = EngineBootConfig { data_dir: data, ipc_port: 0, edge_url: String::new(),
            edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::Mock };
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(20.), px(40.)), size(px(1200.), px(820.))))),
            window_background: theme::Theme::of(cx).window_background_appearance(),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let run: anyhow::Result<()> = async {
                show_report(&state, &render_core, cx);
                cx.background_executor().timer(Duration::from_millis(900)).await;
                capture(window.into(), cx, &output, "osc7501-dark").await?;
                cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                cx.background_executor().timer(Duration::from_millis(500)).await;
                capture(window.into(), cx, &output, "osc7501-light").await?;
                render_core.workspace.mark_chat_seen("done", chrono::Utc::now())?;
                show_report(&state, &render_core, cx);
                cx.background_executor().timer(Duration::from_millis(500)).await;
                capture(window.into(), cx, &output, "osc7501-acknowledged").await?;
                std::fs::write(output.join("result.txt"), "Production Shell and shared status derivation driven by real PTY OSC 7501 reports. Captured dark, light, and acknowledged-result states.\n")?;
                Ok(())
            }.await;
            if let Err(error) = run { eprintln!("Program status capture failed: {error:#}"); *result.lock().unwrap() = Some(format!("{error:#}")); }
            let native_window: gpui::AnyWindowHandle = window.into();
            let _ = native_window.update(cx, |_, window, cx| {
                window.blur();
                window.draw(cx).clear();
                window.present_if_needed();
            });
            drop(state);
            let _ = native_window.update(cx, |_, window, _| window.remove_window());
            cx.background_executor().timer(Duration::from_millis(1000)).await;
            cx.update(|cx| cx.quit());
        }).detach();
    });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
