//! Inbox-mode row actions: Settle,
//! Snooze and Wake on the chat menu and the row corner, against an in-memory
//! engine that records the ops it is sent.
use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};

struct Harness {
    _dir: tempfile::TempDir,
    _runtime: tokio::runtime::Runtime,
    requests: tokio::sync::mpsc::Receiver<String>,
    _replies: tokio::sync::mpsc::Sender<String>,
}

fn chat(id: &str, archived: bool, snoozed_until: Option<DateTime<Utc>>) -> zeron_proto::Chat {
    serde_json::from_value(serde_json::json!({
        "id": id, "title": id, "deviceId": "local", "archived": archived,
        "createdAt": Utc::now() - chrono::Duration::minutes(5),
        "snoozedUntil": snoozed_until,
    }))
    .unwrap()
}

fn setup(cx: &mut TestAppContext, inbox: bool) -> (Harness, Entity<Shell>, VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (out, requests) = tokio::sync::mpsc::channel::<String>(64);
    let (replies, inbound) = tokio::sync::mpsc::channel::<String>(64);
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        settings::init(UiSettings::default(), dir.path(), cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
    });
    let path = dir.path().to_path_buf();
    let window = cx.add_window(|_, cx| {
        let state = cx.new(|_| AppState::new());
        Shell::new(
            state,
            EngineBootConfig {
                data_dir: path,
                ipc_port: 0,
                edge_url: "http://127.0.0.1:1".into(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: zeron_proto::HarnessId::Mock,
            },
            cx,
        )
    });
    let shell = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    {
        let _guard = runtime.enter();
        let client = zeron_rpc::RpcClient::new(out, inbound);
        window
            .update(cx, |shell, _, cx| {
                shell.debug_gate = Some(GatePhase::Ready);
                shell.reduced_motion = true;
                shell.settings.sidebar_inbox = inbox;
                shell.settings.space_filter = None;
                shell.settings.sidebar_organization = SidebarOrganization::InOneList;
                shell.state.update(cx, |state, _| {
                    state.set_test_engine(crate::state::EngineHandle::from_test_client(client));
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.chats_synced = true;
                    state.no_project = true;
                    state.chats = vec![
                        chat("live", false, None),
                        chat(
                            "snoozed",
                            false,
                            Some(Utc::now() + chrono::Duration::hours(2)),
                        ),
                        chat("settled", true, None),
                    ];
                });
                cx.notify();
            })
            .unwrap();
    }
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    (
        Harness {
            _dir: dir,
            _runtime: runtime,
            requests,
            _replies: replies,
        },
        shell,
        cx,
    )
}

fn open_menu(shell: &Entity<Shell>, cx: &mut VisualTestContext, chat_id: &str) {
    shell.update(cx, |shell, cx| {
        shell.chat_menu.open(ChatMenuState {
            chat_id: chat_id.into(),
            tab: None,
            position: gpui::point(px(40.0), px(120.0)),
            page: ChatMenuPage::Root,
        });
        cx.notify();
    });
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
}

/// The Mutate ops the engine has been sent, oldest first.
fn sent_ops(harness: &mut Harness) -> Vec<serde_json::Value> {
    std::iter::from_fn(|| harness.requests.try_recv().ok())
        .map(|frame| serde_json::from_str::<zeron_rpc::ClientFrame>(&frame).unwrap())
        .filter(|frame| frame.method.as_deref() == Some(methods::MUTATE))
        .map(|frame| frame.params)
        .collect()
}

fn until_ms(op: &serde_json::Value) -> i64 {
    op["until"].as_i64().expect("an epoch-ms wake time")
}

#[test]
fn inbox_mode_renames_archive_to_settle() {
    assert_eq!(chat_archive_action(false, false).1, "Archive");
    assert_eq!(chat_archive_action(false, true).1, "Unarchive");
    assert_eq!(chat_archive_action(true, false), (icons::CHECK, "Settle"));
    assert_eq!(
        chat_archive_action(true, true),
        (icons::ARCHIVE_UP_MINIMALISTIC, "Unsettle")
    );
}

#[gpui::test]
fn snooze_submenu_snoozes_a_live_chat(cx: &mut TestAppContext) {
    let (mut harness, shell, mut cx) = setup(cx, true);
    let cx = &mut cx;
    open_menu(&shell, cx, "live");
    assert!(cx.debug_bounds("chat-menu-wake").is_none());
    click(cx, "chat-menu-snooze");
    assert!(shell.read_with(cx, |shell, _| {
        shell.chat_menu.as_open().map(|menu| menu.page) == Some(ChatMenuPage::Snooze)
    }));
    // "In 1 hour" is always offered first.
    let before = Utc::now();
    click(cx, "chat-snooze-0");
    let ops = sent_ops(&mut harness);
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0]["op"], "setChatSnoozed");
    assert_eq!(ops[0]["chatId"], "live");
    let hour = (before + chrono::Duration::hours(1)).timestamp_millis();
    assert!((until_ms(&ops[0]) - hour).abs() < 60_000, "{ops:?}");
    assert!(!shell.read_with(cx, |shell, _| shell.chat_menu.is_open()));
}

#[gpui::test]
fn snoozed_chat_menu_offers_wake_now(cx: &mut TestAppContext) {
    let (mut harness, shell, mut cx) = setup(cx, true);
    let cx = &mut cx;
    open_menu(&shell, cx, "snoozed");
    assert!(cx.debug_bounds("chat-menu-snooze").is_none());
    let before = Utc::now().timestamp_millis();
    click(cx, "chat-menu-wake");
    let ops = sent_ops(&mut harness);
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0]["op"], "setChatSnoozed");
    assert_eq!(ops[0]["chatId"], "snoozed");
    let until = until_ms(&ops[0]);
    assert!(until >= before && until - before < 60_000, "{ops:?}");
}

#[gpui::test]
fn settle_row_archives_and_unsettles(cx: &mut TestAppContext) {
    let (mut harness, shell, mut cx) = setup(cx, true);
    let cx = &mut cx;
    open_menu(&shell, cx, "live");
    click(cx, "chat-menu-archive");
    open_menu(&shell, cx, "settled");
    // Settled rows can't be snoozed.
    assert!(cx.debug_bounds("chat-menu-snooze").is_none());
    assert!(cx.debug_bounds("chat-menu-wake").is_none());
    click(cx, "chat-menu-archive");
    let ops = sent_ops(&mut harness);
    assert_eq!(
        ops,
        [
            serde_json::json!({"op": "setChatArchived", "chatId": "live", "archived": true}),
            serde_json::json!({"op": "setChatArchived", "chatId": "settled", "archived": false}),
        ]
    );
}

#[gpui::test]
fn pinned_chats_are_not_snoozed(cx: &mut TestAppContext) {
    let (_harness, shell, mut cx) = setup(cx, true);
    let cx = &mut cx;
    shell.update(cx, |shell, cx| {
        shell.set_chat_pinned("live".into(), true, cx)
    });
    open_menu(&shell, cx, "live");
    assert!(cx.debug_bounds("chat-menu-archive").is_some());
    assert!(cx.debug_bounds("chat-menu-snooze").is_none());
}

#[gpui::test]
fn mode_off_keeps_the_archive_menu(cx: &mut TestAppContext) {
    let (mut harness, shell, mut cx) = setup(cx, false);
    let cx = &mut cx;
    for chat_id in ["live", "snoozed"] {
        open_menu(&shell, cx, chat_id);
        assert!(cx.debug_bounds("chat-menu-snooze").is_none());
        assert!(cx.debug_bounds("chat-menu-wake").is_none());
    }
    click(cx, "chat-menu-archive");
    assert_eq!(
        sent_ops(&mut harness),
        [serde_json::json!({"op": "setChatArchived", "chatId": "snoozed", "archived": true})]
    );
}

#[gpui::test]
fn a_helper_asking_the_user_wakes_its_snoozed_parent(cx: &mut TestAppContext) {
    let (_harness, shell, mut cx) = setup(cx, true);
    let cx = &mut cx;
    shell.update(cx, |shell, cx| {
        shell.state.update(cx, |state, _| {
            let mut helper = chat("helper", false, None);
            helper.parent_chat_id = Some("snoozed".into());
            state.chats.push(helper);
            state.apply_sessions(vec![zeron_proto::Session {
                last_completed_turn: None,
                last_turn_outcome: None,
                terminal_activity: None,
                running_subagents: 0,
                chat_id: "helper".into(),
                device_id: "local".into(),
                status: zeron_proto::SessionStatus::AwaitingInput,
                started_at: None,
                updated_at: Utc::now(),
                background_tasks: Vec::new(),
            }]);
        });
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |shell, cx| shell.chat_inbox_section("snoozed", None, cx)),
        Some(inbox::InboxSection::Inbox)
    );
    // Back in the Inbox, waiting on the user: neither Wake nor Snooze.
    open_menu(&shell, cx, "snoozed");
    assert!(cx.debug_bounds("chat-menu-wake").is_none());
    assert!(cx.debug_bounds("chat-menu-snooze").is_none());
    // The helper never gets a row of its own.
    assert!(
        !shell
            .update(cx, |shell, cx| shell.sidebar_visible_order(cx))
            .contains(&"helper".to_string())
    );
}

/// Hover each row's corner (Snoozed, then Inbox) and click it, in the
/// compact (default) or the regular row layout, whose corners differ.
fn click_hover_corners(cx: &mut TestAppContext, compact: bool) {
    let (mut harness, shell, mut cx) = setup(cx, true);
    let cx = &mut cx;
    shell.update(cx, |shell, cx| {
        shell.settings.sidebar_compact = compact;
        cx.notify();
    });
    for row in ["snoozed", "live"] {
        // A real pointer: after a corner click the row hover resyncs to it.
        let bounds = cx
            .debug_bounds(Box::leak(format!("chat-{row}").into_boxed_str()))
            .unwrap_or_else(|| panic!("{row}'s row is drawn"));
        cx.simulate_mouse_move(bounds.center(), None, gpui::Modifiers::none());
        shell.update(cx, |shell, cx| {
            shell.chat_status_hover = Some(format!("chat-{row}"));
            cx.notify();
        });
        cx.run_until_parked();
        let selector = if compact {
            format!("chat-time-{row}")
        } else {
            format!("chat-{row}-corner")
        };
        let corner = cx
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap_or_else(|| panic!("{row}'s corner is drawn"));
        cx.simulate_click(corner.center(), gpui::Modifiers::none());
        cx.run_until_parked();
    }
    let ops = sent_ops(&mut harness);
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[0]["op"], "setChatSnoozed");
    assert_eq!(ops[0]["chatId"], "snoozed");
    assert_eq!(
        ops[1],
        serde_json::json!({"op": "setChatArchived", "chatId": "live", "archived": true})
    );
}

#[gpui::test]
fn hover_corner_wakes_a_snoozed_row_and_settles_a_live_one(cx: &mut TestAppContext) {
    click_hover_corners(cx, true);
}

#[gpui::test]
fn regular_rows_hover_corner_wakes_and_settles(cx: &mut TestAppContext) {
    click_hover_corners(cx, false);
}
