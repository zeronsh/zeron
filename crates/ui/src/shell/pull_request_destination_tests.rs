//! Pull requests opened from what Shell painted — a sidebar badge, a board
//! row, a link in a message — through Shell's own listeners and link handler,
//! to the place Settings sends them.
use super::*;
use crate::settings::PullRequestDestination;
use gpui::{AppContext, TestAppContext, VisualTestContext};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};

fn pull_request(number: u64) -> String {
    format!("https://github.com/acme/zeron/pull/{number}")
}

/// Sessions `a` (focused) and `b`, each on a branch whose open pull request
/// the sidebar shows as a badge.
fn setup(cx: &mut TestAppContext) -> (tempfile::TempDir, Entity<Shell>, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
        settings::init(UiSettings::default(), dir.path(), cx);
        motion::set_reduced_motion(cx, true);
    });
    let (shell, cx) = cx.add_window_view(|_, cx| {
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.workspace_scope = Some(WorkspaceScope::Development);
            state.connection = ConnectionStatus::Ready;
            state.local_device_id = Some("local".into());
            for (id, number) in [("a", 90), ("b", 91)] {
                let branch = format!("feature/{id}");
                state.chats.push(
                    serde_json::from_value(serde_json::json!({
                        "id": id, "title": id, "deviceId": "local", "archived": false,
                        "createdAt": Utc::now(),
                        "sourceContext": {
                            "checkoutId": id, "repoRoot": "/repo", "cwd": "/repo",
                            "branch": branch, "observedAt": Utc::now(),
                        },
                    }))
                    .unwrap(),
                );
                state.fixture_sidebar_change_request(zeron_proto::CheckoutChangeRequestStatus {
                    checkout_id: id.into(),
                    device_id: "local".into(),
                    cwd: "/repo".into(),
                    branch: branch.clone(),
                    change_request: Some(zeron_proto::ChangeRequestSummary {
                        provider: "github".into(),
                        number,
                        title: format!("Pull request {number}"),
                        url: pull_request(number),
                        state: zeron_proto::ChangeRequestState::Open,
                        base_ref: "main".into(),
                        head_ref: branch,
                    }),
                    updated_at: Utc::now(),
                });
            }
            state.selected_chat = Some("a".into());
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
        shell.debug_gate = Some(GatePhase::Ready);
        shell.splash = SplashPhase::Gone;
        shell.reduced_motion = true;
        shell.active_chat = "a".into();
        shell
    });
    cx.simulate_resize(gpui::size(px(1400.0), px(800.0)));
    redraw(cx);
    (dir, shell, cx)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    // Cached frames omit debug selectors and painted text; refresh first.
    cx.refresh().unwrap();
    cx.update(|window, cx| window.draw(cx).clear());
    cx.run_until_parked();
}

fn click_at(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_click(position, gpui::Modifiers::default());
    redraw(cx);
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is painted"));
    click_at(cx, bounds.center());
}

fn send_pull_requests(
    cx: &mut VisualTestContext,
    destination: PullRequestDestination,
    open_web_links_in_zeron: bool,
) {
    cx.update(|_, cx| {
        settings::update(SavePolicy::Immediate, cx, |settings| {
            settings.pull_request_destination = destination;
            settings.open_web_links_in_zeron = open_web_links_in_zeron;
        })
    });
    redraw(cx);
}

/// What a right-pane tab shows: Zeron's pull request view or a Browser tab,
/// and the address it opened.
fn shown(shell: &Shell, surface: &RightSurface, cx: &App) -> Option<(&'static str, String)> {
    match surface {
        RightSurface::PullRequest(id) => Some(("view", shell.chat_pull_requests[id].url.clone())),
        RightSurface::Browser(id) => {
            Some(("browser", shell.browsers[id].read(cx).page.url.clone()?))
        }
        _ => None,
    }
}

/// How many pull request views and Browser tabs session `a`'s pane holds,
/// and what the one in front shows.
fn pane(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> (usize, Option<(&'static str, String)>) {
    shell.read_with(cx, |shell, cx| {
        let tabs = shell.right_tabs.get("a").map_or(0, |tabs| {
            tabs.iter()
                .filter(|surface| shown(shell, surface, cx).is_some())
                .count()
        });
        let front = shell
            .right_pane_open(cx)
            .then(|| shown(shell, &shell.resolved_right_active(cx), cx))
            .flatten();
        (tabs, front)
    })
}

/// A badge opens its pull request where Settings says, beside the focused
/// session, whatever "Open links in Zeron" says: a badge is not a link in a
/// message. Off a session no pane can hold a Browser tab, so "In-app
/// browser" opens the default browser there.
#[gpui::test]
fn sidebar_badges_open_pull_requests_where_settings_send_them(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = setup(cx);
    click(cx, "pull-request-badge-chat-a-compact-pr");
    assert_eq!(pane(&shell, cx), (1, Some(("view", pull_request(90)))));

    send_pull_requests(cx, PullRequestDestination::Browser, false);
    click(cx, "pull-request-badge-chat-b-compact-pr");
    assert_eq!(pane(&shell, cx), (2, Some(("browser", pull_request(91)))));
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("a"));
    });
    assert_eq!(cx.opened_url(), None);

    send_pull_requests(cx, PullRequestDestination::External, true);
    click(cx, "pull-request-badge-chat-a-compact-pr");
    assert_eq!(cx.opened_url(), Some(pull_request(90)));
    assert_eq!(pane(&shell, cx), (2, Some(("browser", pull_request(91)))));

    send_pull_requests(cx, PullRequestDestination::Browser, true);
    shell.update(cx, |shell, cx| shell.open_pull_requests(cx));
    redraw(cx);
    click(cx, "pull-request-badge-chat-b-compact-pr");
    assert_eq!(cx.opened_url(), Some(pull_request(91)));
    shell.read_with(cx, |shell, _| {
        assert!(matches!(shell.route, Route::PullRequests));
        assert_eq!(shell.browsers.len(), 1);
    });
}

/// A board row opens its pull request where Settings says. The board has no
/// session pane, so "In-app browser" opens the default browser there.
#[gpui::test]
fn board_rows_open_pull_requests_where_settings_send_them(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| shell.open_pull_requests(cx));
    redraw(cx);
    let show = |cx: &mut VisualTestContext, number: u64| {
        let page = shell
            .read_with(cx, |shell, _| shell.pull_requests_page.clone())
            .expect("the board is open");
        let item = serde_json::from_value(serde_json::json!({
            "provider": "github", "repository": "acme/zeron", "number": number,
            "title": format!("Pull request {number}"), "url": pull_request(number),
            "state": "open", "isDraft": false, "additions": 1, "deletions": 1,
            "mergeability": "mergeable", "createdAt": Utc::now(), "updatedAt": Utc::now(),
        }))
        .unwrap();
        page.update(cx, |page, cx| page.show_test_items(vec![item], cx));
        redraw(cx);
    };

    show(cx, 92);
    send_pull_requests(cx, PullRequestDestination::External, true);
    click(cx, "pull-request-row");
    assert_eq!(cx.opened_url(), Some(pull_request(92)));

    show(cx, 93);
    send_pull_requests(cx, PullRequestDestination::Browser, true);
    click(cx, "pull-request-row");
    assert_eq!(cx.opened_url(), Some(pull_request(93)));
    shell.read_with(cx, |shell, _| assert!(shell.browsers.is_empty()));

    show(cx, 94);
    send_pull_requests(cx, PullRequestDestination::Native, true);
    click(cx, "pull-request-row");
    assert_eq!(cx.opened_url(), Some(pull_request(93)));
    shell.read_with(cx, |shell, _| {
        assert!(matches!(shell.route, Route::PullRequests));
        let tabs = shell
            .pull_request_pane
            .tabs
            .iter()
            .map(|tab| tab.url.clone())
            .collect::<Vec<_>>();
        assert_eq!(tabs, [pull_request(94)]);
    });
}

/// A pull request link in a message, clicked where the transcript painted
/// it, opens where Settings says while "Open links in Zeron" is on and in the
/// default browser while it is off (#956). "Open in Zeron" from the link's
/// menu always stays in Zeron.
#[gpui::test]
fn transcript_pull_request_links_open_where_settings_send_them(cx: &mut TestAppContext) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (_dir, shell, cx) = setup(cx);
    let reply = (1..=6)
        .map(|n| format!("[fix {n}]({})", pull_request(n)))
        .collect::<Vec<_>>()
        .join(", ");
    shell.update(cx, |shell, cx| {
        shell.state.update(cx, |state, cx| {
            state.apply_transcript(vec![SessionMessageEntry {
                id: "reply".into(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::Text {
                    id: "text".into(),
                    text: format!("Opened {reply}."),
                }],
                created_at: 0,
                device_id: "local".into(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            }]);
            cx.notify();
        });
    });
    redraw(cx);

    click_at(cx, painted("fix 1"));
    assert_eq!(pane(&shell, cx), (1, Some(("view", pull_request(1)))));

    send_pull_requests(cx, PullRequestDestination::Browser, true);
    click_at(cx, painted("fix 2"));
    assert_eq!(pane(&shell, cx), (2, Some(("browser", pull_request(2)))));
    assert_eq!(cx.opened_url(), None);

    send_pull_requests(cx, PullRequestDestination::External, true);
    click_at(cx, painted("fix 3"));
    assert_eq!(cx.opened_url(), Some(pull_request(3)));
    assert_eq!(pane(&shell, cx), (2, Some(("browser", pull_request(2)))));

    // The link that left Zeron keeps focus and shows its destination; the
    // next link in the same message still opens on the first press.
    send_pull_requests(cx, PullRequestDestination::Native, false);
    click_at(cx, painted("fix 4"));
    assert_eq!(cx.opened_url(), Some(pull_request(4)));
    assert_eq!(pane(&shell, cx), (2, Some(("browser", pull_request(2)))));

    open_in_zeron(cx, "fix 5");
    assert_eq!(pane(&shell, cx), (3, Some(("view", pull_request(5)))));

    send_pull_requests(cx, PullRequestDestination::External, true);
    open_in_zeron(cx, "fix 6");
    assert_eq!(pane(&shell, cx), (4, Some(("browser", pull_request(6)))));
    assert_eq!(cx.opened_url(), Some(pull_request(4)));
}

/// A point inside `label` where the transcript painted it.
fn painted(label: &str) -> Point<Pixels> {
    crate::markdown::render::selection_test_position(label) + gpui::point(px(2.), px(8.))
}

/// "Open in Zeron" from the menu of the link labelled `label`.
fn open_in_zeron(cx: &mut VisualTestContext, label: &str) {
    let position = painted(label);
    cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
    redraw(cx);
    click(cx, "link-menu-open-zeron");
}
