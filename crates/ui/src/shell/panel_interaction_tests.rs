//! The panel lifecycle through Shell's actions and actual GPUI layout.
use super::*;
use gpui::{AppContext, Bounds, TestAppContext, VisualTestContext};

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
        let mut config = UiSettings::default();
        config.sidebar_width = 300.0;
        config.files_panel_width = 377.0;
        settings::init(config, dir.path(), cx);
    });
    let (shell, cx) = cx.add_window_view(|_, cx| {
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.no_project = true;
            state.selected_chat = Some("a".into());
            state.chats = ["a", "b"]
                .into_iter()
                .map(|id| {
                    serde_json::from_value(serde_json::json!({
                        "id": id, "title": id, "deviceId": "local", "archived": false,
                        "createdAt": Utc::now(),
                    }))
                    .unwrap()
                })
                .collect();
            state
        });
        let mut shell = Shell::new(
            state,
            EngineBootConfig {
                data_dir: dir.path().into(),
                ipc_port: 0,
                edge_url: String::new(),
                edge_token: None,
                org_id: None,
                workos_client_id: None,
                default_harness: zeron_proto::HarnessId::Mock,
            },
            cx,
        );
        shell.debug_gate = Some(GatePhase::Ready);
        shell.splash = SplashPhase::Gone;
        shell.active_chat = "a".into();
        shell.sidebar_opened_at = 1;
        shell.panel_open_sequence = 1;
        shell.test_time = Some(std::time::Instant::now());
        shell
    });
    cx.simulate_resize(gpui::size(px(1600.0), px(900.0)));
    (dir, shell, cx)
}

fn change(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    action: impl FnOnce(&mut Shell, &mut Window, &mut Context<Shell>),
) {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            action(shell, window, cx);
            cx.notify();
        })
    });
}

enum Panel {
    Sidebar,
    Files,
    Right,
}

impl Panel {
    fn toggle(self, shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) {
        match self {
            Self::Sidebar => shell.toggle_sidebar(cx),
            Self::Files => shell.toggle_files_panel(window, cx),
            Self::Right => shell.toggle_right_pane(cx),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    // Sidebar, Files, surface host, visible conversation.
    widths: [f32; 4],
    composer: Bounds<Pixels>,
    strip: Option<Bounds<Pixels>>,
}

fn draw(shell: &Entity<Shell>, cx: &mut VisualTestContext, millis: u64) -> Frame {
    shell.update(cx, |shell, cx| {
        *shell.test_time.as_mut().unwrap() +=
            Duration::from_millis(millis).mul_f32(motion::speed_scale());
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear());
    let mut width = |id| cx.debug_bounds(id).map_or(0.0, |b| f32::from(b.size.width));
    Frame {
        widths: [
            width("sidebar-column"),
            width("files-panel"),
            width("right-panel"),
            width("conversation-column"),
        ],
        composer: shell.read_with(cx, |s, cx| {
            s.composer.read(cx).surface_bounds().get().unwrap()
        }),
        strip: cx.debug_bounds("right-titlebar-controls"),
    }
}

#[track_caller]
fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 0.5,
        "expected {expected}px, got {actual}px"
    );
}

#[track_caller]
fn expect(frame: Frame, widths: [f32; 4]) {
    for (actual, expected) in frame.widths.into_iter().zip(widths) {
        near(actual, expected);
    }
}

#[track_caller]
fn continuous(before: Frame, after: Frame) {
    expect(after, before.widths);
    near(
        f32::from(after.composer.left()),
        f32::from(before.composer.left()),
    );
    near(
        f32::from(after.composer.size.width),
        f32::from(before.composer.size.width),
    );
    if let (Some(before), Some(after)) = (before.strip, after.strip) {
        near(f32::from(after.left()), f32::from(before.left()));
    }
}

fn settled(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Frame {
    // A closing column may make room for a hidden neighbour to return.
    draw(shell, cx, 600);
    draw(shell, cx, 600)
}

fn select(shell: &mut Shell, chat: Option<&str>, cx: &mut Context<Shell>) {
    let state = shell.state.clone();
    state.update(cx, |s, cx| s.select_chat(chat.map(str::to_owned), cx));
    shell.on_state_changed(&state, cx);
}

#[gpui::test]
fn panels_share_space_and_keep_rendered_geometry_through_their_lifecycle(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = setup(cx);
    let initial = settled(&shell, cx);
    expect(initial, [300.0, 0.0, 0.0, 1300.0]);

    // Each column responds gently, with no click jump or overshoot. Measure
    // the rendered widths, not a second implementation of the easing curve.
    for (panel, column, target) in [
        (Panel::Right, 2, 650.0),
        (Panel::Files, 1, 377.0),
        (Panel::Sidebar, 0, 0.0),
    ] {
        let before = draw(&shell, cx, 0);
        change(&shell, cx, |s, window, cx| panel.toggle(s, window, cx));
        continuous(before, draw(&shell, cx, 0));
        let source = before.widths[column];
        let first = draw(&shell, cx, 16);
        let progress = (first.widths[column] - source) / (target - source);
        assert!(
            progress > 0.0 && progress < 0.1,
            "column {column} lurches: {progress}"
        );
        let mut previous = first.widths[column];
        for _ in 0..15 {
            let frame = draw(&shell, cx, 16);
            let width = frame.widths[column];
            assert!(width >= source.min(target) - 0.5 && width <= source.max(target) + 0.5);
            assert!((width - previous) * (target - source) >= -0.5);
            // The visible composer follows its column throughout panel motion.
            let composer_width = f32::from(frame.composer.size.width);
            near(
                composer_width,
                (frame.widths[3] - 2.0 * Theme::SPACE_LG)
                    .min(f32::from(initial.composer.size.width)),
            );
            near(
                f32::from(frame.composer.left()),
                frame.widths[0] + (frame.widths[3] - composer_width) / 2.0,
            );
            if frame.widths[2] > 128.0 {
                near(
                    f32::from(frame.strip.unwrap().left()),
                    frame.widths[0] + frame.widths[3],
                );
            }
            previous = width;
        }
        near(previous, target);
    }
    let wide = settled(&shell, cx);
    expect(wide, [0.0, 377.0, 611.5, 611.5]);
    near(f32::from(wide.strip.unwrap().left()), 611.5);
    // A collapsed sidebar leaves the left edge to the window's own resize.
    let edge = |x| gpui::point(px(x), px(400.0));
    cx.simulate_mouse_down(edge(3.0), MouseButton::Left, Default::default());
    for x in [40.0, 80.0] {
        cx.simulate_mouse_move(edge(x), Some(MouseButton::Left), Default::default());
    }
    cx.simulate_mouse_up(edge(80.0), MouseButton::Left, Default::default());
    expect(settled(&shell, cx), wide.widths);

    // Mid-flight reversal, then a different panel interrupting the return.
    change(&shell, cx, |s, _, cx| s.toggle_right_pane(cx));
    let closing = draw(&shell, cx, 75);
    change(&shell, cx, |s, _, cx| s.toggle_right_pane(cx));
    continuous(closing, draw(&shell, cx, 0));
    let reopening = draw(&shell, cx, 40);
    change(&shell, cx, |s, _, cx| s.toggle_sidebar(cx));
    continuous(reopening, draw(&shell, cx, 0));
    expect(settled(&shell, cx), [300.0, 377.0, 461.5, 461.5]);

    // A drag holds until a toggle; a seam reset restores the allocation.
    let seam = gpui::point(px(761.5), px(400.0));
    let dragged = gpui::point(px(723.0), px(400.0));
    cx.simulate_mouse_down(seam, MouseButton::Left, Default::default());
    for _ in 0..2 {
        cx.simulate_mouse_move(dragged, Some(MouseButton::Left), Default::default());
    }
    cx.simulate_mouse_up(dragged, MouseButton::Left, Default::default());
    expect(draw(&shell, cx, 0), [300.0, 377.0, 500.0, 423.0]);
    expect(draw(&shell, cx, 300), [300.0, 377.0, 500.0, 423.0]);
    // Visiting a chat with other panels open is not a toggle.
    change(&shell, cx, |s, _, cx| {
        s.panels.update("b", |p| p.changes_open = true);
        select(s, Some("b"), cx);
    });
    expect(settled(&shell, cx), [300.0, 0.0, 500.0, 800.0]);
    change(&shell, cx, |s, _, cx| {
        s.panels.update("b", |p| p.changes_open = false);
        select(s, Some("a"), cx);
    });
    expect(settled(&shell, cx), [300.0, 377.0, 500.0, 423.0]);
    change(&shell, cx, |s, _, cx| {
        s.reset_panel_widths(PaneResizeKind::Right, cx)
    });
    expect(settled(&shell, cx), [300.0, 377.0, 461.5, 461.5]);
    change(&shell, cx, |s, _, cx| {
        s.reset_panel_widths(PaneResizeKind::Files, cx)
    });
    expect(settled(&shell, cx), [300.0, 286.0, 507.0, 507.0]);

    // Narrowing hides the oldest open column without closing it. Asking
    // for it promotes it; closing a neighbour makes room for its return.
    cx.simulate_resize(gpui::size(px(1000.0), px(900.0)));
    let narrow = draw(&shell, cx, 0);
    expect(narrow, [300.0, 0.0, 360.0, 340.0]);
    assert!(shell.read_with(cx, |s, cx| s.files_panel_open(cx)));
    change(&shell, cx, |s, window, cx| s.toggle_files_panel(window, cx));
    expect(settled(&shell, cx), [300.0, 286.0, 0.0, 414.0]);
    change(&shell, cx, |s, window, cx| s.toggle_files_panel(window, cx));
    expect(settled(&shell, cx), [300.0, 0.0, 360.0, 340.0]);
    cx.simulate_resize(gpui::size(px(1600.0), px(900.0)));
    change(&shell, cx, |s, window, cx| s.toggle_files_panel(window, cx));
    let ordinary = settled(&shell, cx);
    expect(ordinary, [300.0, 286.0, 507.0, 507.0]);
    // The sidebar's seam follows the pointer, not a tween behind it.
    let sidebar_seam = |x| gpui::point(px(x), px(400.0));
    cx.simulate_mouse_down(sidebar_seam(300.0), MouseButton::Left, Default::default());
    // The first move starts the drag; the following ones move the seam.
    for x in [310.0, 320.0, 340.0] {
        cx.simulate_mouse_move(sidebar_seam(x), Some(MouseButton::Left), Default::default());
        let width = draw(&shell, cx, 16).widths[0];
        if x > 310.0 {
            near(width, x);
        }
    }
    cx.simulate_mouse_move(
        sidebar_seam(300.0),
        Some(MouseButton::Left),
        Default::default(),
    );
    cx.simulate_mouse_up(sidebar_seam(300.0), MouseButton::Left, Default::default());
    // A squeezed sidebar lays its rows and menus out within its column.
    cx.simulate_resize(gpui::size(px(1130.0), px(900.0)));
    near(draw(&shell, cx, 0).widths[0], 250.0);
    near(shell.read_with(cx, |s, _| s.sidebar_content_width()), 250.0);
    cx.simulate_resize(gpui::size(px(1600.0), px(900.0)));
    expect(settled(&shell, cx), ordinary.widths);

    // Fullscreen clips the conversation, retaining its composer layout.
    change(&shell, cx, |s, _, cx| s.toggle_right_pane_expand(cx));
    for millis in [0, 75, 180] {
        let frame = draw(&shell, cx, millis);
        near(
            f32::from(frame.composer.size.width),
            f32::from(ordinary.composer.size.width),
        );
    }
    expect(draw(&shell, cx, 0), [300.0, 286.0, 1014.0, 0.0]);
    change(&shell, cx, |s, _, cx| select(s, Some("b"), cx));
    expect(settled(&shell, cx), [300.0, 0.0, 0.0, 1300.0]);
    assert!(!shell.read_with(cx, |s, _| s.right_pane_expanded));
    change(&shell, cx, |s, _, cx| select(s, Some("a"), cx));
    expect(draw(&shell, cx, 0), [300.0, 286.0, 1014.0, 0.0]);
    change(&shell, cx, |s, _, cx| s.toggle_right_pane_expand(cx));
    expect(settled(&shell, cx), ordinary.widths);

    // Navigation interrupts a close and lands directly on the destination.
    change(&shell, cx, |s, _, cx| s.toggle_right_pane(cx));
    draw(&shell, cx, 75);
    change(&shell, cx, |s, _, cx| select(s, None, cx));
    expect(draw(&shell, cx, 0), [300.0, 0.0, 0.0, 1300.0]);
    expect(settled(&shell, cx), [300.0, 0.0, 0.0, 1300.0]);
    change(&shell, cx, |s, _, cx| select(s, Some("a"), cx));
    expect(settled(&shell, cx), [300.0, 286.0, 0.0, 1014.0]);
    change(&shell, cx, |s, _, cx| {
        s.open_settings(SettingsSection::Appearance, cx)
    });
    draw(&shell, cx, 0);
    assert!(cx.debug_bounds("conversation-column").is_none());
    change(&shell, cx, |s, _, cx| s.close_settings(cx));
    expect(draw(&shell, cx, 0), [300.0, 286.0, 0.0, 1014.0]);

    // Window resizing and reduced motion apply directly, without a lagging tween.
    for width in [1024.0, 800.0, 600.0, 800.0, 1600.0] {
        cx.simulate_resize(gpui::size(px(width), px(900.0)));
        let frame = draw(&shell, cx, 0);
        near(frame.widths.iter().sum(), width);
        expect(draw(&shell, cx, 300), frame.widths);
    }
    // A side chat's composer keeps its column's layout under the pane's mask.
    change(&shell, cx, |s, _, cx| {
        let chat = serde_json::from_value(serde_json::json!({
            "id": "side", "parentChatId": "a", "deviceId": "local",
            "archived": false, "createdAt": Utc::now(),
        }))
        .unwrap();
        s.open_side_chat(chat, s.panel_key(cx), cx);
    });
    settled(&shell, cx);
    let side_composer = |cx: &mut VisualTestContext| {
        shell.read_with(cx, |s, cx| {
            let tab = s.side_chats.values().next().unwrap();
            tab.composer.read(cx).surface_bounds().get().unwrap()
        })
    };
    let resting = side_composer(cx);
    for _ in 0..2 {
        change(&shell, cx, |s, _, cx| s.toggle_right_pane(cx));
        draw(&shell, cx, 75);
        let moving = side_composer(cx);
        near(f32::from(moving.size.width), f32::from(resting.size.width));
        near(
            f32::from(moving.size.height),
            f32::from(resting.size.height),
        );
        settled(&shell, cx);
    }
    change(&shell, cx, |s, _, cx| s.toggle_right_pane(cx));
    settled(&shell, cx);
    cx.update(|_, cx| motion::set_reduced_motion(cx, true));
    for panel in [
        Panel::Sidebar,
        Panel::Right,
        Panel::Files,
        Panel::Files,
        Panel::Right,
        Panel::Sidebar,
    ] {
        change(&shell, cx, |s, window, cx| panel.toggle(s, window, cx));
        let landed = draw(&shell, cx, 0);
        near(landed.widths.iter().sum(), 1600.0);
        expect(settled(&shell, cx), landed.widths);
    }
}
