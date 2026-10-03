//! Panel layout, driven through the real shell actions. One integration
//! test runs every action from every arrangement and checks each frame and
//! each settled layout; short scenarios cover what a single action cannot.
use super::*;
use gpui::{AppContext, TestAppContext, WindowHandle};

fn fixture(cx: &mut TestAppContext) -> (tempfile::TempDir, WindowHandle<Shell>) {
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
    });
    let window = cx.add_window(|_, cx| {
        let state = cx.new(|_| AppState::new());
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
        shell.active_chat = "a".into();
        shell.viewport_width = 1600.0;
        shell
    });
    (dir, window)
}

fn tween_length() -> Duration {
    RESIZE.total().mul_f32(motion::speed_scale())
}

/// Advance time by ageing every transition. The frame clock itself stays
/// frozen (see [`settle`]), so a tween an action creates reads as unstarted
/// until aged, however long the action took to run.
fn age(shell: &mut Shell, by: Duration) {
    for tween in [
        &mut shell.sidebar_tween,
        &mut shell.right_tween,
        &mut shell.files_tween,
        &mut shell.files_content_tween,
        &mut shell.right_content_tween,
        &mut shell.main_takeover_tween,
    ]
    .into_iter()
    .flatten()
    .chain(shell.fit_exits.iter_mut().flatten().map(|(exit, _)| exit))
    {
        tween.started -= by;
    }
}

/// Finish every transition and freeze the frame clock. A completed close
/// may let another column return, so run the fit a few times.
fn settle(shell: &mut Shell, cx: &App) {
    for _ in 0..3 {
        shell.render_time = Some(std::time::Instant::now());
        age(shell, tween_length() + Duration::from_millis(1));
        shell.track_horizontal_fit(cx);
    }
}

/// A settled arrangement: the panels in `mask` (bit 0 sidebar, 1 surface
/// host, 2 Files, opened in that order; bit 3 shows the surface host full
/// screen).
fn arrange(shell: &mut Shell, viewport: f32, mask: u8, cx: &App) {
    shell.active_chat = "a".into();
    shell.viewport_width = viewport;
    shell.settings.sidebar_width = 300.0;
    shell.settings.right_pane_width = 680.0;
    shell.settings.files_panel_width = 377.0;
    shell.settings.sidebar_collapsed = mask & 1 == 0;
    shell.sidebar_opened_at = 1;
    shell.panel_open_sequence = 4;
    shell.right_pane_expanded = mask & 0b1010 == 0b1010;
    shell.sidebar_tween = None;
    shell.right_tween = None;
    shell.files_tween = None;
    shell.files_content_tween = None;
    shell.right_content_tween = None;
    shell.main_takeover_tween = None;
    shell.fit_exits = [None; 3];
    shell.painted_columns = None;
    shell.panels = SessionPanels::default();
    shell.panels.update("a", |p| {
        p.changes_open = mask & 2 != 0;
        p.changes_opened_at = 2;
        p.files_open = mask & 4 != 0;
        p.files_opened_at = 3;
    });
    settle(shell, cx);
}

const ACTIONS: [&str; 4] = ["sidebar", "surface", "files", "expand"];

fn act(shell: &mut Shell, action: usize, window: &mut Window, cx: &mut Context<Shell>) {
    match action {
        0 => shell.toggle_sidebar(cx),
        1 => shell.toggle_right_pane(cx),
        2 => shell.toggle_files_panel(window, cx),
        _ => shell.toggle_right_pane_expand(cx),
    }
}

const COLUMNS: [&str; 9] = [
    "sidebar",
    "files",
    "surface",
    "conversation",
    "composer",
    "titlebar strip",
    "titlebar files controls",
    "surface content",
    "files content",
];

/// What the shell paints this frame: each column, the composer's layout
/// width, the titlebar strip's edges and the panels' content widths. NaN
/// marks what is hidden and has no position.
fn columns(shell: &Shell, cx: &App) -> [f32; 9] {
    let files = shell.files_visible_width(cx);
    let right = shell.right_visible_width(cx);
    let main = shell.main_target_width(right, cx);
    let shown = |visible: bool, value: f32| if visible { value } else { f32::NAN };
    let (strip, files_controls) = shell
        .titlebar_trailing_layout(false, cx)
        .edges(shell.viewport_width);
    [
        shell.sidebar_now(),
        files,
        right,
        main,
        // Covered by a full-screen pane, the composer has nothing to show.
        shown(main > 0.5, shell.main_content_width(main, cx)),
        shown(right > 0.5, strip),
        files_controls,
        shown(
            right > 0.5,
            shell.right_content_width(shell.right_target(cx)),
        ),
        shown(files > 0.5, shell.files_content_width(cx)),
    ]
}

/// Frames of `action`, optionally interrupted part way through its tween by
/// `then`, sampled every twentieth of a tween for three tweens, then settled.
fn frames(
    shell: &mut Shell,
    action: usize,
    then: Option<usize>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> (Vec<[f32; 9]>, f32) {
    let mut frames = vec![columns(shell, cx)];
    let mut planned = 0.0f32;
    act(shell, action, window, cx);
    for step in 0..=60 {
        if step > 0 {
            age(shell, tween_length() / 20);
        }
        // Each interrupting action lands at a different point of the first
        // tween: 15%, 35%, 55% or 75% of the way.
        if let Some(then) = then.filter(|then| step == 3 + 4 * then) {
            act(shell, then, window, cx);
        }
        shell.track_horizontal_fit(cx);
        planned = planned.max(planned_travel(shell));
        frames.push(columns(shell, cx));
    }
    settle(shell, cx);
    frames.push(columns(shell, cx));
    (frames, planned)
}

/// The longest distance any running tween sets out to cover. A reversed
/// tween only gets part of the way, but moves at its planned pace.
fn planned_travel(shell: &Shell) -> f32 {
    [
        shell.sidebar_tween,
        shell.right_tween,
        shell.files_tween,
        shell.main_takeover_tween,
    ]
    .into_iter()
    .flatten()
    .chain(shell.fit_exits.iter().flatten().map(|(exit, _)| *exit))
    .map(|tween| (tween.to - tween.from).abs())
    .fold(0.0, f32::max)
}

/// Discontinuities in `frames`. Nothing may move on the click or on the
/// settle; mid-flight a column moves at most as fast as the eased curve
/// carries the largest travel, observed or `planned` (twice that, since two
/// tweens can overlap), the titlebar strip as fast as Files and the surface
/// host together, and the columns never overflow the window.
fn jumps(frames: &[[f32; 9]], planned: f32, viewport: f32) -> Vec<String> {
    let curve_step = (0..20)
        .map(|i| RESIZE.progress((i + 1) as f32 / 20.0) - RESIZE.progress(i as f32 / 20.0))
        .fold(0.0f32, f32::max);
    let travel = |column: usize| {
        frames
            .iter()
            .map(|frame| (frame[column] - frames[0][column]).abs())
            .fold(0.0f32, f32::max)
    };
    let largest = (0..5).map(travel).fold(planned, f32::max);
    let strip = travel(1) + travel(2);
    let last = frames.len() - 2;
    let mut jumps = Vec::new();
    for (column, name) in COLUMNS.iter().enumerate() {
        for (i, pair) in frames.windows(2).enumerate() {
            let delta = pair[1][column] - pair[0][column];
            let carried = if matches!(column, 5 | 6) {
                strip.max(largest)
            } else {
                largest
            };
            let limit = if i == 0 || i == last {
                2.0
            } else {
                2.0 * curve_step * carried + 1.0
            };
            // NaN (hidden) compares false: nothing to measure.
            if delta.abs() > limit {
                let at = match i {
                    0 => "on the click".into(),
                    i if i == last => "on settling".into(),
                    i => format!("at {}%", (i - 1) * 5),
                };
                let path: Vec<i32> = frames.iter().map(|f| f[column].round() as i32).collect();
                jumps.push(format!("{name} jumps {delta:.0}px {at}: {path:?}"));
            }
        }
    }
    if let Some(frame) = frames.iter().find(|f| f[0] + f[1] + f[2] > viewport + 0.1) {
        jumps.push(format!("columns overflow the window: {frame:?}"));
    }
    jumps
}

/// What is wrong with a settled layout, whatever led to it. `split` also
/// requires the equal split an open or close leaves behind.
fn layout_faults(shell: &Shell, split: bool, cx: &App) -> Vec<String> {
    let mut faults = Vec::new();
    let fit = shell.horizontal_fit();
    let viewport = shell.viewport_width;
    let sidebar = shell.sidebar_now();
    let files = shell.files_visible_width(cx);
    let right = shell.right_visible_width(cx);
    let main = viewport - sidebar - files - right;
    let files_shown = fit.files && shell.files_panel_open(cx);
    let right_shown = fit.right && shell.right_pane_open(cx);
    for (name, shown, width, min) in [
        (
            "sidebar",
            fit.sidebar && !shell.settings.sidebar_collapsed,
            sidebar,
            SIDEBAR_MIN,
        ),
        ("files", files_shown, files, FILES_PANEL_MIN),
        ("surface", right_shown, right, RIGHT_PANE_MIN),
    ] {
        if shown && width < min - 0.1 {
            faults.push(format!("{name} is {width}px, below its {min}px minimum"));
        }
        if !shown && width > 0.1 {
            faults.push(format!("hidden {name} still paints {width}px"));
        }
    }
    let takeover = right_shown && shell.right_pane_expanded;
    if !takeover && main < CHAT_PANEL_MIN.min(viewport) - 0.1 {
        faults.push(format!("conversation is {main}px, below its minimum"));
    }
    // Splits rewrite the pane shares within their bounds, never the sidebar.
    if shell.settings.sidebar_width != 300.0
        || shell.settings.right_pane_width < RIGHT_PANE_MIN - 0.1
        || !(FILES_PANEL_MIN - 0.1..=FILES_PANEL_MAX + 0.1)
            .contains(&shell.settings.files_panel_width)
    {
        faults.push("a saved width left its bounds".into());
    }
    // The tools keep their own widths: Files shows its own, as far as the
    // conversation and the surface host's minimums leave room.
    if files_shown && (files - shell.files_settled_width(cx)).abs() > 0.5 {
        faults.push(format!("Files is {files}px, not its own width"));
    }
    // The content columns share what the tools leave in halves, unless a
    // half would break the host's or the conversation's minimum.
    let half = (viewport - sidebar - files) / 2.0;
    if split
        && right_shown
        && !takeover
        && half >= RIGHT_PANE_MIN.max(CHAT_PANEL_MIN)
        && (right - half).abs() > 0.5
    {
        faults.push(format!(
            "split is {:?}, not halves of {}",
            [main, right],
            2.0 * half
        ));
    }
    faults
}

/// One frame as the shell renders it at `viewport`: the window width, then
/// the fit.
fn render_frame_at(shell: &mut Shell, viewport: f32, cx: &mut Context<Shell>) -> [f32; 9] {
    shell.observe_viewport_width(viewport, cx);
    shell.track_horizontal_fit(cx);
    columns(shell, cx)
}

fn render_frame(shell: &mut Shell, cx: &mut Context<Shell>) -> [f32; 9] {
    render_frame_at(shell, shell.viewport_width, cx)
}

fn select_chat(shell: &mut Shell, chat: Option<&str>, cx: &mut Context<Shell>) {
    let state = shell.state.clone();
    state.update(cx, |state, _| state.selected_chat = chat.map(str::to_owned));
    shell.on_state_changed(&state, cx);
}

/// Whether two frames paint the same thing, hidden parts included.
fn same(a: &[f32; 9], b: &[f32; 9]) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| (a.is_nan() && b.is_nan()) || (a - b).abs() < 0.5)
}

#[gpui::test]
fn every_panel_action_moves_continuously_and_settles_on_a_valid_layout(cx: &mut TestAppContext) {
    let (_dir, handle) = fixture(cx);
    let mut report = Vec::new();
    handle
        .update(cx, |shell, window, cx| {
            for viewport in [300.0, 660.0, 900.0, 1000.0, 1104.0, 1280.0, 1512.0, 1920.0] {
                for mask in (0..16u8).filter(|mask| mask & 0b1010 != 0b1000) {
                    for action in 0..ACTIONS.len() {
                        // Alone, and interrupted halfway through by
                        // itself or any other action.
                        for then in [None, Some(0), Some(1), Some(2), Some(3)] {
                            arrange(shell, viewport, mask, cx);
                            let (frames, planned) = frames(shell, action, then, window, cx);
                            let mut faults = jumps(&frames, planned, viewport);
                            // An open or close of a column leaves the
                            // open columns equal.
                            let split = action < 3 && then.is_none();
                            faults.extend(layout_faults(shell, split, cx));
                            let then = then
                                .map_or(String::new(), |then| format!(" then {}", ACTIONS[then]));
                            report.extend(faults.into_iter().map(|fault| {
                                format!(
                                    "{viewport}px panels={mask:04b} {}{then}: {fault}",
                                    ACTIONS[action]
                                )
                            }));
                        }
                    }
                }
            }
        })
        .unwrap();
    assert!(
        report.is_empty(),
        "{} faults:\n{}",
        report.len(),
        report.join("\n")
    );
}

#[gpui::test]
fn the_content_columns_share_what_the_tools_leave(cx: &mut TestAppContext) {
    let (_dir, handle) = fixture(cx);
    handle
        .update(cx, |shell, window, cx| {
            let widths = |shell: &Shell, cx: &App| {
                let right = shell.right_visible_width(cx);
                (
                    shell.main_target_width(right, cx),
                    shell.files_visible_width(cx),
                    right,
                )
            };
            arrange(shell, 1500.0, 0b0001, cx);
            shell.toggle_right_pane(cx);
            shell.toggle_files_panel(window, cx);
            settle(shell, cx);
            // The sidebar and Files keep their widths (300 and 377); the
            // conversation and the surface host share the rest.
            assert_eq!(widths(shell, cx), (411.5, 377.0, 411.5));
            // Closing or opening the surface host never moves the explorer.
            shell.toggle_right_pane(cx);
            settle(shell, cx);
            assert_eq!(widths(shell, cx), (823.0, 377.0, 0.0));
            shell.toggle_right_pane(cx);
            settle(shell, cx);
            assert_eq!(widths(shell, cx), (411.5, 377.0, 411.5));
            // A dragged divider holds until the next open or close.
            shell.settings.right_pane_width = 500.0;
            assert_eq!(widths(shell, cx).2, 500.0);
            shell.toggle_sidebar(cx);
            settle(shell, cx);
            assert_eq!(widths(shell, cx), (561.5, 377.0, 561.5));
            // Double-clicking a seam restores the defaults: the content split
            // from the surface host's seam, Files' own width from its seam.
            shell.settings.right_pane_width = 500.0;
            shell.reset_panel_widths(PaneResizeKind::Right, cx);
            settle(shell, cx);
            assert_eq!(widths(shell, cx), (561.5, 377.0, 561.5));
            shell.reset_panel_widths(PaneResizeKind::Files, cx);
            settle(shell, cx);
            assert_eq!(widths(shell, cx), (607.0, 286.0, 607.0));

            // A window resize keeps the split on every step, applied
            // directly: no tween lags the window edge.
            let mut viewport = 1500.0;
            while viewport > 1200.0 {
                viewport -= 0.5;
                shell.observe_viewport_width(viewport, cx);
                assert!(!shell.tween_active(shell.right_tween));
                assert_eq!(layout_faults(shell, true, cx), [""; 0], "{viewport}px");
            }
        })
        .unwrap();
}

#[gpui::test]
fn a_narrow_window_hides_the_oldest_column_until_there_is_room(cx: &mut TestAppContext) {
    let (_dir, handle) = fixture(cx);
    handle
        .update(cx, |shell, window, cx| {
            let shown = |shell: &Shell| {
                let fit = shell.horizontal_fit();
                (fit.sidebar, fit.right, fit.files)
            };
            // Sidebar, then the surface host, then Files: 1000px cannot hold
            // all three beside the conversation, so the sidebar yields.
            arrange(shell, 1000.0, 0b0111, cx);
            assert_eq!(shown(shell), (false, true, true));
            assert_eq!(shell.sidebar_now(), 0.0);
            // Hidden is not closed: nothing the user chose has changed.
            assert!(!shell.settings.sidebar_collapsed && shell.right_pane_open(cx));

            // Asking for a hidden panel brings it forward over the oldest.
            shell.toggle_sidebar(cx);
            assert_eq!(shown(shell), (true, false, true));
            shell.set_surfaces_open(true, cx);
            assert_eq!(shown(shell), (true, true, false));
            shell.toggle_files_panel(window, cx);
            assert_eq!(shown(shell), (false, true, true));
            settle(shell, cx);

            // Closing a panel makes room: the hidden column returns from zero
            // width alongside the close, not in a jump.
            shell.toggle_right_pane(cx);
            shell.track_horizontal_fit(cx);
            assert_eq!(shell.sidebar_now(), 0.0);
            age(shell, tween_length() / 2);
            shell.track_horizontal_fit(cx);
            settle(shell, cx);
            assert_eq!(shown(shell), (true, false, true));
            assert_eq!(shell.sidebar_now(), 300.0);
        })
        .unwrap();
}

#[gpui::test]
fn takeover_holds_the_conversation_layout_and_belongs_to_its_chat(cx: &mut TestAppContext) {
    let (_dir, handle) = fixture(cx);
    handle
        .update(cx, |shell, window, cx| {
            let composer = |shell: &Shell, cx: &App| {
                let right = shell.right_visible_width(cx);
                shell.main_content_width(shell.main_target_width(right, cx), cx)
            };
            arrange(shell, 1600.0, 0b0111, cx);
            let ordinary = composer(shell, cx);
            let beside = (shell.sidebar_now(), shell.files_visible_width(cx));
            // The surface host covers the conversation, which keeps its
            // layout beneath it; the other columns stay where they are.
            for _ in 0..2 {
                shell.toggle_right_pane_expand(cx);
                for _ in 0..3 {
                    age(shell, tween_length() / 3);
                    assert!((composer(shell, cx) - ordinary).abs() < 0.1);
                    assert_eq!((shell.sidebar_now(), shell.files_visible_width(cx)), beside);
                }
            }
            assert!(!shell.right_pane_expanded);

            // The header stays painted while its host closes; it cannot
            // expand a pane that is going away.
            shell.toggle_right_pane(cx);
            age(shell, tween_length() / 10);
            shell.toggle_right_pane_expand(cx);
            assert!(!shell.right_pane_expanded);
            assert_eq!(shell.right_tween.unwrap().to, 0.0);
            shell.toggle_right_pane(cx);
            settle(shell, cx);

            // Takeover stays with the chat that asked for it.
            shell.toggle_right_pane_expand(cx);
            shell.panels.update("b", |p| p.changes_open = true);
            let state = shell.state.clone();
            for (chat, expanded) in [(Some("b"), false), (Some("a"), true), (None, false)] {
                state.update(cx, |s, _| s.selected_chat = chat.map(str::to_owned));
                shell.on_state_changed(&state, cx);
                assert_eq!(shell.right_pane_expanded, expanded, "{chat:?}");
            }

            // Too narrow for both, full screen gives the host the whole
            // window rather than hiding it.
            arrange(shell, 500.0, 0b1111, cx);
            let fit = shell.horizontal_fit();
            assert!(fit.right && !fit.sidebar && !fit.files);
            assert_eq!(shell.right_visible_width(cx), 500.0);
            let _ = window;
        })
        .unwrap();
}

#[gpui::test]
fn switching_views_lands_at_once_and_returns_to_the_same_layout(cx: &mut TestAppContext) {
    let (_dir, handle) = fixture(cx);
    let mut report = Vec::new();
    handle
        .update(cx, |shell, window, cx| {
            for viewport in [660.0, 1000.0, 1280.0, 1920.0] {
                for mask in (0..16u8).filter(|mask| mask & 0b1010 != 0b1000) {
                    // Another chat with each panel set, the new-chat canvas,
                    // and Settings; left at rest or mid-transition.
                    for destination in 0..10u8 {
                        for midway in [None, Some(1), Some(2)] {
                            arrange(shell, viewport, mask, cx);
                            select_chat(shell, Some("a"), cx);
                            shell.split_context = Default::default();
                            render_frame(shell, cx);
                            settle(shell, cx);
                            let before = render_frame(shell, cx);
                            if let Some(action) = midway {
                                act(shell, action, window, cx);
                                age(shell, tween_length() / 2);
                                render_frame(shell, cx);
                            }
                            let case =
                                format!("{viewport}px {mask:04b} to {destination} {midway:?}");
                            match destination {
                                8 => select_chat(shell, None, cx),
                                9 => shell.open_settings(SettingsSection::Appearance, cx),
                                other => {
                                    shell.panels.update("b", |p| {
                                        p.changes_open = other & 2 != 0;
                                        p.files_open = other & 4 != 0;
                                    });
                                    select_chat(shell, Some("b"), cx);
                                }
                            }
                            // The destination lands in one frame (Settings
                            // covers the chat, whatever it does meanwhile).
                            let arrived = render_frame(shell, cx);
                            age(shell, tween_length() * 2);
                            if destination < 9 && !same(&arrived, &render_frame(shell, cx)) {
                                report
                                    .push(format!("{case}: the destination moved after arriving"));
                            }
                            if destination < 8 {
                                report.extend(
                                    layout_faults(shell, true, cx)
                                        .into_iter()
                                        .map(|f| format!("{case}: {f}")),
                                );
                            }
                            // Coming back restores the chat's own layout at once.
                            match destination {
                                9 => shell.close_settings(cx),
                                _ => select_chat(shell, Some("a"), cx),
                            }
                            let back = render_frame(shell, cx);
                            age(shell, tween_length() * 2);
                            if !same(&back, &render_frame(shell, cx)) {
                                report.push(format!("{case}: the chat moved after returning"));
                            }
                            if midway.is_none() && !same(&back, &before) {
                                report
                                    .push(format!("{case}: returned to {back:?}, not {before:?}"));
                            }
                        }
                    }
                }
            }
        })
        .unwrap();
    assert!(
        report.is_empty(),
        "{} faults:\n{}",
        report.len(),
        report.join("\n")
    );
}

#[gpui::test]
fn window_resizes_and_reduced_motion_land_on_valid_layouts(cx: &mut TestAppContext) {
    let (_dir, handle) = fixture(cx);
    let mut report = Vec::new();
    handle
        .update(cx, |shell, window, cx| {
            // A window drag is direct manipulation: across every fit
            // threshold, each frame at rest is a valid, equally split layout.
            for mask in (0..16u8).filter(|mask| mask & 0b1010 != 0b1000) {
                for midway in [None, Some(1), Some(2)] {
                    arrange(shell, 1920.0, mask, cx);
                    if let Some(action) = midway {
                        act(shell, action, window, cx);
                        age(shell, tween_length() / 2);
                    }
                    let widths = (600..=1920).rev().step_by(7).chain((600..=1920).step_by(7));
                    for viewport in widths {
                        age(shell, tween_length() / 20);
                        let frame = render_frame_at(shell, viewport as f32, cx);
                        if frame[0] + frame[1] + frame[2] > viewport as f32 + 0.5 {
                            report.push(format!("{mask:04b} {midway:?} {viewport}px: overflow"));
                        }
                        let resting = [shell.sidebar_tween, shell.right_tween, shell.files_tween]
                            .into_iter()
                            .all(|tween| !shell.tween_active(tween));
                        if resting {
                            report.extend(
                                layout_faults(shell, true, cx)
                                    .into_iter()
                                    .map(|f| format!("{mask:04b} {midway:?} {viewport}px: {f}")),
                            );
                        }
                    }
                }
            }
            // Reduced motion: every action lands at once on a valid layout.
            for viewport in [300.0, 660.0, 1000.0, 1280.0, 1920.0] {
                for mask in (0..16u8).filter(|mask| mask & 0b1010 != 0b1000) {
                    for action in 0..ACTIONS.len() {
                        arrange(shell, viewport, mask, cx);
                        shell.reduced_motion = true;
                        act(shell, action, window, cx);
                        let landed = render_frame(shell, cx);
                        settle(shell, cx);
                        if !same(&landed, &render_frame(shell, cx)) {
                            report.push(format!(
                                "reduced {viewport}px {mask:04b} {}: moved",
                                ACTIONS[action]
                            ));
                        }
                        report.extend(layout_faults(shell, action < 3, cx).into_iter().map(|f| {
                            format!("reduced {viewport}px {mask:04b} {}: {f}", ACTIONS[action])
                        }));
                        shell.reduced_motion = false;
                    }
                }
            }
        })
        .unwrap();
    assert!(
        report.is_empty(),
        "{} faults:\n{}",
        report.len(),
        report.join("\n")
    );
}
