//! Exercise the production drag source, titlebar drop targets and session lifetime.
use super::*;
use crate::terminal::session::TerminalSessionModel;
use gpui::{AppContext, TestAppContext, VisualTestContext, point};

struct TransferHost {
    shell: Entity<Shell>,
    _data_dir: tempfile::TempDir,
}

impl Render for TransferHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.update(cx, |shell, cx| {
            let tabs = shell.render_right_tab_strip(cx);
            let titlebar = shell.titlebar_drag_region(
                "terminal-transfer-titlebar",
                div().w_full().h(px(40.)).child(tabs),
                cx,
            );
            let right = shell.render_right_pane(window, cx);
            let drawer = shell.terminal.clone().unwrap();
            div()
                .size_full()
                .flex()
                .flex_row()
                .track_focus(&shell.shortcut_focus)
                .capture_key_down(cx.listener(Shell::on_key_down_capture))
                .on_action(cx.listener(|shell, _: &ToggleTerminal, window, cx| {
                    shell.toggle_terminal(window, cx)
                }))
                .child(
                    div()
                        .w(px(400.))
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("transfer-outside")
                                .debug_selector(|| "transfer-outside".into())
                                .flex_1(),
                        )
                        .when(shell.terminal_open(cx), |el| {
                            el.child(div().h(px(240.)).child(drawer))
                        }),
                )
                .child(
                    div()
                        .w(px(400.))
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(titlebar)
                        .child(right),
                )
        })
    }
}

fn setup(
    cx: &mut TestAppContext,
    source_tabs: usize,
    right_tabs: usize,
) -> (
    Entity<Shell>,
    Vec<Entity<TerminalSessionModel>>,
    &mut VisualTestContext,
) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        settings::init(UiSettings::default(), dir.path(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| {
        let shell = cx.new(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.selected_chat = Some("parent".into());
                state
            });
            let mut shell = Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            );
            shell.active_chat = "parent".into();
            shell.viewport_width = 800.;
            shell.settings.right_pane_width = 400.;
            shell.panels.update("parent", |panels| {
                panels.terminal_open = true;
                panels.changes_open = true;
            });
            let drawer = shell.terminal_panel(cx);
            drawer.update(cx, |panel, cx| {
                for ix in 0..source_tabs {
                    panel.reserve_tab_for_chat("parent".into(), format!("Source {ix}"), cx);
                }
                panel.set_open(true, cx);
            });
            if right_tabs > 0 {
                let right = shell.right_terminal_panel(cx);
                for ix in 0..right_tabs {
                    let key = right.update(cx, |panel, cx| {
                        panel.reserve_tab_for_chat("parent".into(), format!("Right {ix}"), cx)
                    });
                    shell
                        .right_tabs
                        .entry("parent".into())
                        .or_default()
                        .push(RightSurface::Terminal(key));
                    shell.set_right_active(RightSurface::Terminal(key), cx);
                }
                right.update(cx, |panel, cx| panel.set_open(true, cx));
            }
            shell
        });
        TransferHost {
            shell,
            _data_dir: dir,
        }
    });
    let shell = host.read_with(cx, |host, _| host.shell.clone());
    let sessions = shell.read_with(cx, |shell, cx| {
        let drawer = shell.terminal.as_ref().unwrap().read(cx);
        drawer
            .tab_summaries(cx)
            .iter()
            .map(|(key, _, _)| drawer.session_for_tab("parent", *key).unwrap())
            .collect()
    });
    cx.update(|window, cx| window.draw(cx).clear());
    (shell, sessions, cx)
}

fn begin_drag(cx: &mut VisualTestContext, key: u64, to: Point<gpui::Pixels>) {
    let selector = Box::leak(format!("terminal-tab-{key}").into_boxed_str());
    let from = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.update(|_, cx| assert!(cx.has_active_drag(), "terminal drag did not start"));
    cx.simulate_mouse_move(to, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear());
}

fn drop_at(cx: &mut VisualTestContext, to: Point<gpui::Pixels>) {
    cx.simulate_mouse_up(to, MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
}

#[gpui::test]
fn last_terminal_moves_to_empty_strip_and_keeps_session_state(cx: &mut TestAppContext) {
    let (shell, sessions, cx) = setup(cx, 1, 0);
    let session = &sessions[0];
    let key = session.read_with(cx, |model, _| model.key);
    session.update(cx, |model, _| {
        model.terminal_id = Some("existing-pty".into());
        model.target_device_id = Some("remote-host".into());
        model.last_seq = 47;
        model
            .emulator
            .feed(b"\x1b]2;running command\x07original output\r\n");
        model.exited = Some(0);
    });
    let to = cx.debug_bounds("right-surface-strip").unwrap().center();
    begin_drag(cx, key, to);
    shell.read_with(cx, |shell, cx| {
        assert!(
            shell
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .session_for_tab("parent", key)
                .is_some(),
            "hover moved the session"
        );
    });
    drop_at(cx, to);
    shell.read_with(cx, |shell, cx| {
        let drawer = shell.terminal.as_ref().unwrap().read(cx);
        assert!(drawer.tab_summaries(cx).is_empty());
        assert!(!drawer.is_open());
        assert!(!shell.terminal_open(cx));
        assert_eq!(shell.right_tabs["parent"], [RightSurface::Terminal(key)]);
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Terminal(key));
        let right = shell.right_terminal.as_ref().unwrap().read(cx);
        assert_eq!(right.session_for_tab("parent", key).unwrap(), *session);
        assert_eq!(right.tab_summaries(cx)[0].1.as_ref(), "running command");
        let model = session.read(cx);
        assert_eq!(model.terminal_id.as_deref(), Some("existing-pty"));
        assert_eq!(model.target_device_id.as_deref(), Some("remote-host"));
        assert_eq!(model.last_seq, 47);
        assert_eq!(model.exited, Some(0));
        assert!(
            model
                .emulator
                .lines()
                .iter()
                .flatten()
                .any(|cell| cell.ch == 'o')
        );
    });
    cx.update(|window, cx| {
        assert!(
            shell
                .read(cx)
                .right_terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .focus_handle()
                .is_focused(window)
        );
    });
    // Subsequent state notifications must not auto-create another drawer tab.
    shell.update(cx, |shell, cx| shell.state.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    shell.read_with(cx, |shell, cx| {
        assert!(
            shell
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .tab_summaries(cx)
                .is_empty()
        )
    });
}

#[gpui::test]
fn terminal_inserts_on_chips_at_both_edges_and_in_scrolled_strip(cx: &mut TestAppContext) {
    let (shell, sessions, cx) = setup(cx, 3, 5);
    let source_keys: Vec<_> = sessions
        .iter()
        .map(|s| s.read_with(cx, |m, _| m.key))
        .collect();
    let original = shell.read_with(cx, |shell, _| shell.right_tabs["parent"].clone());
    let first = cx.debug_bounds("right-surface-tab-0").unwrap();
    let to = point(first.left() + px(8.), first.center().y);
    begin_drag(cx, source_keys[0], to);
    drop_at(cx, to);
    shell.read_with(cx, |shell, _| {
        assert_eq!(
            shell.right_tabs["parent"][0],
            RightSurface::Terminal(source_keys[0])
        )
    });
    shell.update(cx, |shell, _| {
        shell.right_tab_scroll.set_offset(point(px(-232.), px(0.)))
    });
    cx.update(|window, cx| window.draw(cx).clear());
    let chip = cx.debug_bounds("right-surface-tab-3").unwrap();
    let to = point(chip.left() + px(8.), chip.center().y);
    begin_drag(cx, source_keys[1], to);
    drop_at(cx, to);
    shell.read_with(cx, |shell, _| {
        assert_eq!(
            shell.right_tabs["parent"][3],
            RightSurface::Terminal(source_keys[1])
        )
    });
    shell.update(cx, |shell, _| {
        shell.right_tab_scroll.set_offset(point(px(-1000.), px(0.)))
    });
    cx.update(|window, cx| window.draw(cx).clear());
    let to = cx.debug_bounds("right-surface-add").unwrap().center();
    begin_drag(cx, source_keys[2], to);
    drop_at(cx, to);
    shell.read_with(cx, |shell, cx| {
        let tabs = &shell.right_tabs["parent"];
        assert_eq!(tabs.last(), Some(&RightSurface::Terminal(source_keys[2])));
        assert!(
            original.iter().all(|surface| tabs.contains(surface)),
            "key collision lost a destination tab"
        );
        assert_eq!(tabs.len(), 8);
        assert_eq!(
            shell
                .right_terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .tab_summaries(cx)
                .len(),
            8
        );
    });
}

#[gpui::test]
fn terminal_drop_in_unused_strip_space_appends(cx: &mut TestAppContext) {
    let (shell, sessions, cx) = setup(cx, 1, 1);
    let key = sessions[0].read_with(cx, |m, _| m.key);
    let bounds = cx.debug_bounds("right-surface-strip").unwrap();
    let to = point(bounds.right() - px(10.), bounds.center().y);
    begin_drag(cx, key, to);
    drop_at(cx, to);
    shell.read_with(cx, |shell, _| {
        assert_eq!(
            shell.right_tabs["parent"].last(),
            Some(&RightSurface::Terminal(key))
        )
    });
}

#[gpui::test]
fn cancel_outside_drop_and_session_switch_keep_terminal_in_origin(cx: &mut TestAppContext) {
    let (shell, sessions, cx) = setup(cx, 1, 0);
    let key = sessions[0].read_with(cx, |m, _| m.key);
    let to = cx.debug_bounds("right-surface-strip").unwrap().center();
    begin_drag(cx, key, to);
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert!(!cx.has_active_drag()));
    drop_at(cx, to);
    shell.read_with(cx, |shell, cx| {
        assert!(
            shell
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .session_for_tab("parent", key)
                .is_some()
        )
    });
    let outside = cx.debug_bounds("transfer-outside").unwrap().center();
    begin_drag(cx, key, outside);
    drop_at(cx, outside);
    shell.read_with(cx, |shell, cx| {
        assert!(
            shell
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .session_for_tab("parent", key)
                .is_some()
        )
    });
    begin_drag(cx, key, to);
    shell.update(cx, |shell, cx| {
        shell.active_chat = "other".into();
        shell.state.update(cx, |state, cx| {
            state.selected_chat = Some("other".into());
            cx.notify();
        });
    });
    drop_at(cx, to);
    shell.read_with(cx, |shell, cx| {
        assert!(
            shell
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .session_for_tab("parent", key)
                .is_some()
        );
        assert!(
            shell
                .right_tabs
                .values()
                .all(|tabs| !tabs.contains(&RightSurface::Terminal(key)))
        );
    });
}

fn test_connection(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (
    tokio::sync::mpsc::Sender<String>,
    tokio::sync::mpsc::Receiver<String>,
) {
    let (out, requests) = tokio::sync::mpsc::channel(64);
    let (replies, inbound) = tokio::sync::mpsc::channel(64);
    let engine =
        crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound));
    shell.update(cx, |shell, cx| {
        shell
            .state
            .update(cx, |state, _| state.set_test_engine(engine))
    });
    (replies, requests)
}

fn pump(runtime: &tokio::runtime::Runtime, cx: &mut VisualTestContext) {
    for _ in 0..4 {
        runtime.block_on(tokio::task::yield_now());
        cx.run_until_parked();
    }
}

fn requests(rx: &mut tokio::sync::mpsc::Receiver<String>) -> Vec<zeron_rpc::ClientFrame> {
    let mut frames = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        frames.push(serde_json::from_str(&frame).unwrap());
    }
    frames
}

#[gpui::test]
fn live_stream_input_and_remote_target_follow_transfer_without_reopening(cx: &mut TestAppContext) {
    use zeron_rpc::methods;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let (shell, sessions, cx) = setup(cx, 1, 0);
    let session = &sessions[0];
    let key = session.read_with(cx, |model, _| model.key);
    let (replies, mut rx) = test_connection(&shell, cx);
    session.update(cx, |model, cx| {
        assert!(model.attach_reserved_session(
            zeron_proto::TerminalSession {
                id: "live-pty".into(),
                cwd: "/remote".into(),
                shell: "bash".into(),
            },
            Some("remote-host".into()),
            cx
        ));
    });
    pump(&runtime, cx);
    let mut frames = requests(&mut rx);
    let subscription = frames
        .iter()
        .find(|frame| frame.method.as_deref() == Some(methods::SUBSCRIBE_TERMINAL))
        .unwrap()
        .id;
    replies
        .try_send(
            serde_json::json!({ "id": subscription, "item": {
                "type": "data", "seq": 1, "data": encode(b"before move\r\n")
            }})
            .to_string(),
        )
        .unwrap();
    pump(&runtime, cx);
    cx.update(|window, cx| {
        window.focus(
            &shell
                .read(cx)
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .focus_handle(),
            cx,
        );
    });
    cx.simulate_keystrokes("x");
    let to = cx.debug_bounds("right-surface-strip").unwrap().center();
    begin_drag(cx, key, to);
    drop_at(cx, to);
    replies.try_send(serde_json::json!({ "id": subscription, "item": {
        "type": "data", "seq": 2, "data": encode(b"\x1b]2;after move\x07continuous output\r\n")
    }}).to_string()).unwrap();
    pump(&runtime, cx);
    cx.executor().advance_clock(Duration::from_millis(100));
    pump(&runtime, cx);
    frames.extend(requests(&mut rx));
    assert!(
        !frames.iter().any(|frame| matches!(
            frame.method.as_deref(),
            Some(methods::OPEN_TERMINAL | methods::CLOSE_TERMINAL)
        )),
        "transfer opened or closed a PTY: {frames:?}"
    );
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame.method.as_deref() == Some(methods::SUBSCRIBE_TERMINAL))
            .count(),
        1,
        "transfer reconnected the stream"
    );
    let writes: Vec<_> = frames
        .iter()
        .filter(|frame| frame.method.as_deref() == Some(methods::WRITE_TERMINAL))
        .collect();
    assert_eq!(
        writes.len(),
        1,
        "pending input was lost or duplicated: {frames:?}"
    );
    assert_eq!(writes[0].params["data"], encode(b"x"));
    assert!(
        frames
            .iter()
            .filter(|frame| frame.method.is_some())
            .all(|frame| frame.params["terminalId"] == "live-pty"
                && frame.params["targetDeviceId"] == "remote-host")
    );
    shell.read_with(cx, |shell, cx| {
        let right = shell.right_terminal.as_ref().unwrap().read(cx);
        assert_eq!(right.tab_summaries(cx)[0].1.as_ref(), "after move");
        assert_eq!(session.read(cx).last_seq, 2);
        let text: String = session
            .read(cx)
            .emulator
            .lines()
            .iter()
            .flatten()
            .map(|cell| cell.ch)
            .collect();
        assert!(text.contains("before move"));
        assert!(text.contains("continuous output"));
    });
    // Reconnect still resumes on the same model's sequence after the move.
    replies
        .try_send(serde_json::json!({ "id": subscription, "done": true }).to_string())
        .unwrap();
    pump(&runtime, cx);
    cx.executor().advance_clock(Duration::from_millis(500));
    pump(&runtime, cx);
    let reconnect = requests(&mut rx)
        .into_iter()
        .find(|frame| frame.method.as_deref() == Some(methods::SUBSCRIBE_TERMINAL))
        .unwrap();
    assert_eq!(reconnect.params["afterSeq"], 2);
    assert_eq!(reconnect.params["targetDeviceId"], "remote-host");
    replies.try_send(serde_json::json!({ "id": reconnect.id, "item": { "type": "exit", "seq": 3, "exitCode": 7 }}).to_string()).unwrap();
    pump(&runtime, cx);
    session.read_with(cx, |model, _| assert_eq!(model.exited, Some(7)));
}

#[gpui::test]
fn pending_action_handle_attaches_after_move_and_rejects_after_close(cx: &mut TestAppContext) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let (shell, sessions, cx) = setup(cx, 1, 0);
    let key = sessions[0].read_with(cx, |model, _| model.key);
    // This is the handle captured by run_project_action before its RPC returns.
    let pending = sessions[0].downgrade();
    let (_replies, mut rx) = test_connection(&shell, cx);
    let to = cx.debug_bounds("right-surface-strip").unwrap().center();
    begin_drag(cx, key, to);
    drop_at(cx, to);
    assert!(
        pending
            .update(cx, |model, cx| model.attach_reserved_session(
                zeron_proto::TerminalSession {
                    id: "action-pty".into(),
                    cwd: "/project".into(),
                    shell: "bash".into(),
                },
                None,
                cx
            ))
            .unwrap()
    );
    pump(&runtime, cx);
    assert!(requests(&mut rx).iter().any(|frame| frame.method.as_deref()
        == Some(zeron_rpc::methods::SUBSCRIBE_TERMINAL)
        && frame.params["terminalId"] == "action-pty"));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(RightSurface::Terminal(key), window, cx)
        });
    });
    // The test retains a strong model handle, so also check the closed gate,
    // rather than relying only on WeakEntity failing after release.
    assert!(
        !pending
            .update(cx, |model, cx| model.attach_reserved_session(
                zeron_proto::TerminalSession {
                    id: "late-result".into(),
                    cwd: "/project".into(),
                    shell: "bash".into(),
                },
                None,
                cx
            ))
            .unwrap()
    );
    pump(&runtime, cx);
    let frames = requests(&mut rx);
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame.method.as_deref() == Some(zeron_rpc::methods::CLOSE_TERMINAL))
            .count(),
        1
    );
}

fn encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[gpui::test]
fn open_terminal_reply_lands_in_destination_and_resizes_to_its_grid(cx: &mut TestAppContext) {
    use zeron_rpc::methods;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let (shell, original, cx) = setup(cx, 1, 0);
    let (replies, mut rx) = test_connection(&shell, cx);
    let opening = shell.update(cx, |shell, cx| {
        let drawer = shell.terminal.as_ref().unwrap();
        drawer.update(cx, |panel, cx| {
            let key = panel.open_tab_for_selected(cx).unwrap();
            panel.session_for_tab("parent", key).unwrap()
        })
    });
    pump(&runtime, cx);
    let open = requests(&mut rx)
        .into_iter()
        .find(|frame| frame.method.as_deref() == Some(methods::OPEN_TERMINAL))
        .unwrap();
    let key = opening.read_with(cx, |model, _| model.key);
    let to = cx.debug_bounds("right-surface-strip").unwrap().center();
    begin_drag(cx, key, to);
    drop_at(cx, to);
    replies
        .try_send(
            serde_json::json!({ "id": open.id, "ok": {
                "id": "opening-pty", "cwd": "/project", "shell": "bash"
            }})
            .to_string(),
        )
        .unwrap();
    pump(&runtime, cx);
    cx.executor().advance_clock(Duration::from_millis(100));
    pump(&runtime, cx);
    let frames = requests(&mut rx);
    assert!(!frames.iter().any(|frame| matches!(
        frame.method.as_deref(),
        Some(methods::OPEN_TERMINAL | methods::CLOSE_TERMINAL)
    )));
    assert!(frames.iter().any(|frame| frame.method.as_deref()
        == Some(methods::SUBSCRIBE_TERMINAL)
        && frame.params["terminalId"] == "opening-pty"));
    let resize = frames
        .iter()
        .find(|frame| {
            frame.method.as_deref() == Some(methods::RESIZE_TERMINAL)
                && frame.params["terminalId"] == "opening-pty"
        })
        .unwrap();
    shell.read_with(cx, |shell, cx| {
        let model = opening.read(cx);
        assert_eq!(model.terminal_id.as_deref(), Some("opening-pty"));
        assert_eq!(resize.params["cols"], model.emulator.cols());
        assert_eq!(resize.params["rows"], model.emulator.rows());
        assert_ne!(
            model.emulator.rows(),
            original[0].read(cx).emulator.rows(),
            "two visible terminals shared a viewport"
        );
        assert!(
            shell.terminal_open(cx),
            "moving a non-last tab closed the drawer"
        );
        assert_eq!(
            shell
                .right_terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .session_for_tab("parent", key)
                .unwrap(),
            opening
        );
    });
}

#[gpui::test]
fn closing_terminal_during_open_releases_the_late_pty(cx: &mut TestAppContext) {
    use zeron_rpc::methods;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let (shell, _, cx) = setup(cx, 1, 0);
    let (replies, mut rx) = test_connection(&shell, cx);
    let key = shell.update(cx, |shell, cx| {
        shell
            .terminal
            .as_ref()
            .unwrap()
            .update(cx, |panel, cx| panel.open_tab_for_selected(cx).unwrap())
    });
    pump(&runtime, cx);
    let open = requests(&mut rx)
        .into_iter()
        .find(|frame| frame.method.as_deref() == Some(methods::OPEN_TERMINAL))
        .unwrap();
    cx.update(|window, cx| {
        shell
            .read(cx)
            .terminal
            .as_ref()
            .unwrap()
            .clone()
            .update(cx, |panel, cx| panel.close_tab_by_key(key, window, cx))
    });
    cx.run_until_parked();
    replies
        .try_send(
            serde_json::json!({ "id": open.id, "ok": {
                "id": "late-pty", "cwd": "/project", "shell": "bash"
            }})
            .to_string(),
        )
        .unwrap();
    pump(&runtime, cx);
    let frames = requests(&mut rx);
    assert!(frames.iter().any(
        |frame| frame.method.as_deref() == Some(methods::CLOSE_TERMINAL)
            && frame.params["terminalId"] == "late-pty"
    ));
    assert!(
        !frames
            .iter()
            .any(|frame| frame.method.as_deref() == Some(methods::SUBSCRIBE_TERMINAL))
    );
}
