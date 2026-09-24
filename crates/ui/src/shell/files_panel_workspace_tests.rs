//! Exercise the explorer and editors against an isolated real workspace/RPC.
//! Set ZERON_FILES_CAPTURES to a directory to run on X11 and capture the fixture.
use super::*;
use gpui::{AppContext, AsyncApp, WindowHandle};
use std::{path::Path, sync::Arc};

struct ClosedFixture;
impl Render for ClosedFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(50))
        .await;
}

async fn wait_for(
    window: WindowHandle<Shell>,
    cx: &mut AsyncApp,
    label: &str,
    predicate: impl Fn(&Shell, &App) -> bool,
) {
    for _ in 0..200 {
        gpui::AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
        if window
            .update(cx, |shell, window, cx| {
                window.refresh();
                predicate(shell, cx)
            })
            .unwrap()
        {
            return;
        }
        pause(cx).await;
    }
    panic!("timed out waiting for {label}");
}

async fn frame(window: WindowHandle<Shell>, cx: &mut AsyncApp, output: Option<&Path>, name: &str) {
    for _ in 0..6 {
        pause(cx).await;
    }
    gpui::AnyWindowHandle::from(window)
        .update(cx, |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
    if let Some(output) = output {
        std::fs::create_dir_all(output).unwrap();
        let status = std::process::Command::new("import")
            .args(["-window", "Files panel fixture"])
            .arg(output.join(format!("{name}.png")))
            .status()
            .unwrap();
        assert!(status.success(), "fixture capture failed");
    }
}

#[test]
fn files_panel_workspace_navigation_and_external_updates() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    std::fs::create_dir_all(project.join("src/nested")).unwrap();
    std::fs::write(
        project.join("src/nested/main.rs"),
        "fn main() { println!(\"Hello\"); }\n",
    )
    .unwrap();
    std::fs::write(
        project.join("README.md"),
        "# Workspace\n\nAn independent file explorer.\n",
    )
    .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["add", "."],
        vec!["commit", "-m", "fixture"],
    ] {
        let result = std::process::Command::new("git")
            .args(args)
            .current_dir(&project)
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    std::fs::write(
        project.join("README.md"),
        "# Workspace\n\nChanged documentation.\n",
    )
    .unwrap();
    std::fs::write(project.join("src/new.rs"), "// new file\n").unwrap();
    let core = runtime
        .block_on(async {
            zeron_engine::EngineCore::assemble(
                &directory.path().join("engine"),
                Arc::new(zeron_engine::default_registry()),
                zeron_proto::HarnessId::Mock,
                None,
            )
        })
        .unwrap();
    core.workspace
        .create_space(
            "project",
            &core.device_id,
            &project.to_string_lossy(),
            Some("Workspace".into()),
            false,
        )
        .unwrap();
    for id in ["first", "second"] {
        core.workspace
            .create_chat(
                id,
                Some("project"),
                None,
                None,
                Some(project.to_string_lossy().into_owned()),
            )
            .unwrap();
        core.workspace
            .rename_chat(id, &format!("Explore files · {id}"))
            .unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let _ipc = runtime
        .block_on(zeron_engine::serve_ipc(port, core.rpc_service()))
        .unwrap();
    let output = std::env::var_os("ZERON_FILES_CAPTURES").map(PathBuf::from);
    let application = if output.is_some() {
        gpui_platform::application()
    } else {
        gpui_platform::headless()
    };
    application
        .with_assets(crate::icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let data = directory.path().join("ui");
            let settings = UiSettings::default();
            settings::init(settings.clone(), &data, cx);
            let fonts = crate::typography::register_fonts(cx);
            crate::typography::init(
                settings.ui_font_family.clone(),
                settings.ui_font_size,
                settings.terminal_font_family.clone(),
                settings.terminal_font_size,
                settings.code_font_family.clone(),
                settings.code_font_size,
                fonts,
                cx,
            );
            crate::theme_library::init(data.clone(), cx);
            crate::appearance::init(
                crate::appearance::AppearanceMode::Dark,
                settings.theme_selection.clone(),
                settings.accent,
                settings.surface,
                cx,
            );
            crate::history::init(
                settings.git_history_columns,
                settings.git_history_column_widths,
                settings.git_history_column_order,
                settings.git_history_author_display,
                cx,
            );
            crate::composer::init(cx, settings.composer_send_behavior);
            crate::terminal::panel::init(cx);
            crate::app_menus::init(cx);
            let boot = EngineBootConfig {
                data_dir: data,
                ipc_port: port,
                edge_url: String::new(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: zeron_proto::HarnessId::Mock,
            };
            let state = cx.new(|_| AppState::new());
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::Point::default(),
                            gpui::size(px(1400.0), px(800.0)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Files panel fixture");
                        cx.new(|cx| Shell::new(state.clone(), boot.clone(), cx))
                    },
                )
                .unwrap();
            AppState::bootstrap(state.clone(), boot, cx);
            cx.spawn(async move |cx| {
                // Keep the temporary store, workspace and daemon alive throughout the UI run.
                let _directory = directory;
                wait_for(window, cx, "engine and chats", |shell, cx| {
                    shell.state.read(cx).engine().is_some() && shell.state.read(cx).chats.len() == 2
                })
                .await;
                state.update(cx, |state, cx| state.select_chat(Some("first".into()), cx));
                wait_for(window, cx, "first session", |shell, _| {
                    shell.active_chat == "first"
                })
                .await;
                window
                    .update(cx, |shell, window, cx| {
                        shell.splash = SplashPhase::Gone;
                        shell.add_files_surface(window, cx);
                    })
                    .unwrap();
                wait_for(window, cx, "root listing", |shell, cx| {
                    shell.files["first"].read(cx).tree().node("src").is_some()
                })
                .await;
                frame(window, cx, output.as_deref(), "01-chat-files").await;
                wait_for(
                    window,
                    cx,
                    "Git decorations without a Diffs panel",
                    |shell, cx| {
                        let files = shell.files["first"].read(cx);
                        let theme = Theme::of(cx);
                        files.test_git_color("README.md", false, cx) == Some(theme.warning)
                            && files.test_git_color("src/new.rs", false, cx) == Some(theme.success)
                            && files.test_git_color("src", true, cx) == Some(theme.success)
                            && files
                                .test_git_color("src/nested/main.rs", false, cx)
                                .is_none()
                    },
                )
                .await;
                window
                    .update(cx, |shell, _, cx| {
                        let other = cx.new(|cx| {
                            FilesSurface::new_explorer(
                                shell.state.clone(),
                                "second".into(),
                                false,
                                cx,
                            )
                        });
                        other.update(cx, |files, cx| files.ensure_git_status(cx));
                        assert_eq!(
                            shell.files["first"].read(cx).test_git_source(),
                            other.read(cx).test_git_source(),
                            "chats on the same checkout share one status source"
                        );
                    })
                    .unwrap();
                window
                    .update(cx, |shell, window, cx| {
                        shell.settings.files_panel_width = FILES_PANEL_MAX;
                        shell.settings.right_pane_width = 760.0;
                        shell.set_surfaces_open(true, cx);
                        window.resize(gpui::size(px(1200.0), px(800.0)));
                        window.bounds_changed(cx);
                    })
                    .unwrap();
                frame(window, cx, output.as_deref(), "01b-picker-files-compact").await;
                window
                    .update(cx, |shell, window, cx| {
                        assert_eq!(shell.files_visible_width(cx), 284.0);
                        assert_eq!(shell.files_reserved_width(cx), 284.0);
                        assert_eq!(shell.right_visible_width(cx), RIGHT_PANE_MIN);
                        assert_eq!(shell.settings.files_panel_width, FILES_PANEL_MAX);
                        assert!(shell.right_surface_rows(cx).is_empty());
                        shell.set_surfaces_open(false, cx);
                        shell.settings.files_panel_width = FILES_PANEL_DEFAULT;
                        shell.settings.right_pane_width = RIGHT_PANE_DEFAULT;
                        window.resize(gpui::size(px(1400.0), px(800.0)));
                        window.bounds_changed(cx);
                    })
                    .unwrap();
                window
                    .update(cx, |shell, window, cx| {
                        shell.add_file_surface("src/nested/main.rs".into(), window, cx)
                    })
                    .unwrap();
                wait_for(window, cx, "nested file and selection", |shell, cx| {
                    shell.files["first"].read(cx).tree().selected() == Some("src/nested/main.rs")
                        && shell.file_surfaces.values().any(|file| {
                            file.read(cx)
                                .test_document_text("src/nested/main.rs")
                                .is_some()
                        })
                })
                .await;
                window
                    .update(cx, |shell, _, cx| {
                        let tree = shell.files["first"].read(cx).tree();
                        assert!(tree.is_expanded("src") && tree.is_expanded("src/nested"));
                        assert!(
                            shell.file_surfaces.values().all(|file| file
                                .read(cx)
                                .tree()
                                .visible_rows()
                                .is_empty()),
                            "editors must not load hidden trees"
                        );
                    })
                    .unwrap();
                frame(window, cx, output.as_deref(), "02-editor-files").await;
                window
                    .update(cx, |shell, window, cx| {
                        shell.add_file_surface("README.md".into(), window, cx);
                    })
                    .unwrap();
                frame(window, cx, output.as_deref(), "02b-two-file-tabs").await;
                window
                    .update(cx, |shell, window, cx| shell.toggle_files_panel(window, cx))
                    .unwrap();
                frame(
                    window,
                    cx,
                    output.as_deref(),
                    "02c-two-file-tabs-explorer-hidden",
                )
                .await;
                window
                    .update(cx, |shell, _, cx| {
                        assert!(
                            shell.files["first"].read(cx).test_git_source().is_none(),
                            "closing the explorer releases its status subscription"
                        );
                    })
                    .unwrap();
                window
                    .update(cx, |shell, window, cx| {
                        shell.toggle_files_panel(window, cx);
                        shell.add_file_surface("src/nested/main.rs".into(), window, cx);
                    })
                    .unwrap();
                window
                    .update(cx, |shell, _, cx| shell.toggle_right_pane_expand(cx))
                    .unwrap();
                frame(window, cx, output.as_deref(), "03-expanded-files").await;
                window
                    .update(cx, |shell, window, cx| {
                        shell.toggle_right_pane_expand(cx);
                        window.resize(gpui::size(px(1000.0), px(720.0)));
                        window.bounds_changed(cx);
                    })
                    .unwrap();
                frame(window, cx, output.as_deref(), "04-narrow-files").await;
                window
                    .update(cx, |shell, _, cx| {
                        let files = shell.files_visible_width(cx);
                        let surface = shell.right_visible_width(cx);
                        let sidebar = shell.eval_tween(shell.sidebar_tween, shell.sidebar_target());
                        assert_eq!(shell.files_reserved_width(cx), files);
                        assert!(files > 0.0 && surface > 0.0);
                        assert!(sidebar + files + surface < shell.viewport_width);
                        assert_eq!(shell.settings.files_panel_width, FILES_PANEL_DEFAULT);
                    })
                    .unwrap();
                window
                    .update(cx, |shell, _, cx| shell.toggle_right_pane_expand(cx))
                    .unwrap();
                frame(window, cx, output.as_deref(), "04b-expanded-narrow-files").await;
                window
                    .update(cx, |shell, _, cx| {
                        assert_eq!(shell.files_visible_width(cx), FILES_PANEL_DEFAULT);
                        assert_eq!(
                            shell.right_visible_width(cx),
                            1000.0 - 256.0 - FILES_PANEL_DEFAULT
                        );
                        shell.toggle_right_pane_expand(cx);
                    })
                    .unwrap();
                // A browser surface coexists with the explorer and keeps its own tab.
                window
                    .update(cx, |shell, window, cx| {
                        shell.add_browser_surface(None, window, cx)
                    })
                    .unwrap();
                frame(window, cx, output.as_deref(), "05-browser-files").await;
                window
                    .update(cx, |shell, window, cx| {
                        shell.add_file_surface("src/nested/main.rs".into(), window, cx);
                        shell.toggle_files_panel(window, cx);
                    })
                    .unwrap();
                std::fs::write(
                    project.join("src/nested/main.rs"),
                    "fn main() { println!(\"Updated\"); }\n",
                )
                .unwrap();
                wait_for(
                    window,
                    cx,
                    "document update with explorer hidden",
                    |shell, cx| {
                        shell.file_surfaces.values().any(|file| {
                            file.read(cx)
                                .test_document_text("src/nested/main.rs")
                                .is_some_and(|text| text.contains("Updated"))
                        })
                    },
                )
                .await;
                // Events queued in an inactive session must never open a tab in another.
                let first_explorer = window
                    .update(cx, |shell, _, _| shell.files["first"].clone())
                    .unwrap();
                state.update(cx, |state, cx| state.select_chat(Some("second".into()), cx));
                wait_for(window, cx, "second session", |shell, _| {
                    shell.active_chat == "second"
                })
                .await;
                first_explorer.update(cx, |_, cx| {
                    cx.emit(FilesEvent::OpenFile("README.md".into()))
                });
                frame(window, cx, None, "inactive-event").await;
                window
                    .update(cx, |shell, _, _| {
                        assert!(
                            !shell
                                .file_surface_keys
                                .contains_key(&("second".into(), "README.md".into()))
                        )
                    })
                    .unwrap();
                state.update(cx, |state, cx| state.select_chat(Some("first".into()), cx));
                wait_for(window, cx, "restored session", |shell, _| {
                    shell.active_chat == "first"
                })
                .await;
                window
                    .update(cx, |shell, window, cx| {
                        shell.add_files_surface(window, cx);
                        assert_eq!(shell.files["first"].entity_id(), first_explorer.entity_id());
                    })
                    .unwrap();
                std::fs::rename(
                    project.join("src/nested/main.rs"),
                    project.join("src/nested/renamed.rs"),
                )
                .unwrap();
                wait_for(window, cx, "renamed file and tab", |shell, cx| {
                    shell
                        .file_surface_keys
                        .contains_key(&("first".into(), "src/nested/renamed.rs".into()))
                        && shell.files["first"].read(cx).tree().selected()
                            == Some("src/nested/renamed.rs")
                })
                .await;
                std::fs::remove_file(project.join("src/nested/renamed.rs")).unwrap();
                wait_for(window, cx, "deleted file", |shell, cx| {
                    shell.files["first"]
                        .read(cx)
                        .tree()
                        .node("src/nested/renamed.rs")
                        .is_none()
                        && shell.file_surfaces.values().any(|file| {
                            file.read(cx)
                                .test_document_phase("src/nested/renamed.rs")
                                .as_deref()
                                == Some("DeletedOnDisk")
                        })
                })
                .await;
                core.workspace.set_chat_archived("first", true).unwrap();
                wait_for(window, cx, "archived session", |shell, cx| {
                    shell
                        .state
                        .read(cx)
                        .chats
                        .iter()
                        .any(|chat| chat.id == "first" && chat.archived)
                })
                .await;
                window
                    .update(cx, |shell, window, cx| {
                        assert!(shell.files_panel_open(cx));
                        assert_eq!(shell.files["first"].entity_id(), first_explorer.entity_id());
                        assert!(shell.files_subs.contains_key("first"));
                        shell.toggle_files_panel(window, cx);
                        assert!(!shell.files_panel_open(cx));
                        shell.toggle_files_panel(window, cx);
                        assert!(shell.files_panel_open(cx));
                    })
                    .unwrap();
                // An archived chat's explorer must survive subsequent state updates
                // and keep receiving filesystem changes after being reopened.
                state.update(cx, |_, cx| cx.notify());
                std::fs::write(project.join("archived.txt"), "still accessible\n").unwrap();
                wait_for(window, cx, "archived explorer update", |shell, cx| {
                    shell.files_panel_open(cx)
                        && shell.files.get("first").is_some_and(|files| {
                            files.read(cx).tree().node("archived.txt").is_some()
                        })
                })
                .await;
                core.workspace.delete_chat("first").unwrap();
                wait_for(window, cx, "deleted explorer cleanup", |shell, _| {
                    !shell.files.contains_key("first")
                        && !shell.files_subs.contains_key("first")
                        && !shell.panels.get("first").files_open
                })
                .await;
                drop(first_explorer);
                drop(state);
                let handle = gpui::AnyWindowHandle::from(window);
                // Render an input-free frame before closing. X11's retained
                // IME handler otherwise outlives the app's leak detector.
                handle
                    .update(cx, |_, window, cx| {
                        window.replace_root(cx, |_, _| ClosedFixture);
                        window.blur();
                        window.refresh();
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                pause(cx).await;
                handle
                    .update(cx, |_, window, _| window.remove_window())
                    .unwrap();
                pause(cx).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}
