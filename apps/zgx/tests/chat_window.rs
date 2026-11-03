//! A long reply costs about what shows of it: the transcript lays out a
//! message by its blocks, so scrolling through a reply of hundreds of blocks
//! keeps the nodes mounted bounded by the view, not by the reply. The page
//! is the chat fixture (`ui/chat_fixture.luau`) with its long reply (about
//! 400 top-level blocks), mounted headless.

use std::path::PathBuf;

use zgx_script::{ScriptHost, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The chat fixture on `s6`, with its long reply, 960 by 700.
fn long_chat() -> TestApp {
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s6"));
    app.frames(20);
    assert!(app.host.run_command("long_reply"));
    app.frames(30);
    settle(&app);
    app
}

/// Frames over a little real time: timers a scroll starts (each windowed
/// run judging whether it's near the view, a tick on) run, then settle.
fn settle(app: &TestApp) {
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(12));
        app.frames(3);
    }
}

/// Nodes mounted in the window's scene (all of them, shown or not).
fn nodes(app: &mut TestApp) -> usize {
    app.ui.scene.borrow().len()
}

/// The reply's words in view: the "Part N" headings whose boxes lie in the
/// view (the transcript from 38 to 652 down).
fn parts_in_view(app: &mut TestApp) -> Vec<u32> {
    app.tree()
        .lines()
        .filter_map(|l| {
            let (_, rest) = l.split_once("heading \"Part ")?;
            let (n, geo) = rest.split_once('"')?;
            let y: f32 = geo.trim().split(',').nth(1)?.split_whitespace().next()?.parse().ok()?;
            ((38.0..652.0).contains(&y)).then(|| n.parse().ok()).flatten()
        })
        .collect()
}

#[test]
fn a_long_reply_mounts_about_what_shows() {
    // The page as it stands before the reply: what the reply adds is the
    // difference.
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s6"));
    app.frames(20);
    let page = app.ui.scene.borrow().len();
    drop(app);

    let mut app = long_chat();
    let at_end = nodes(&mut app);
    assert!(parts_in_view(&mut app).contains(&100), "the reply's end shows");
    // Through the reply, a screen at a time, to its top: what shows is
    // always built (a heading in view at each step), the nodes bounded.
    let mut most = at_end;
    let mut seen: Vec<u32> = Vec::new();
    for _ in 0..80 {
        let list = app.target("#transcript").expect("the transcript");
        zgx_script::driver::scroll(&mut app.ui, list, -450.);
        settle(&app);
        most = most.max(nodes(&mut app));
        let shown = parts_in_view(&mut app);
        assert!(!shown.is_empty(), "a screen of the reply showed none of its words (built late)");
        seen.extend(shown);
    }
    let at_top = nodes(&mut app);
    assert!(seen.contains(&1), "the scroll reached the reply's top");
    eprintln!("nodes: the page alone {page}, with the reply at its end {at_end}, at its top {at_top}, most while scrolling {most}");
    if std::env::var_os("CHAT_WINDOW_MEASURE").is_none() {
        // The reply costs a screen or two of blocks, not its 400.
        assert!(most < page + 700, "scrolling a long reply mounted {most} nodes (the page alone {page})");
    }
}


#[test]
fn a_selection_across_two_runs_of_a_long_reply_copies_both() {
    // Up until Parts 96 and 97 show: the last blocks of one run (four
    // blocks a part, sixteen a run) and the first of the next.
    let mut app = long_chat();
    for _ in 0..20 {
        let shown = parts_in_view(&mut app);
        if shown.contains(&96) && shown.contains(&97) {
            break;
        }
        let list = app.target("#transcript").expect("the transcript");
        zgx_script::driver::scroll(&mut app.ui, list, -150.);
        settle(&app);
    }
    let shown = parts_in_view(&mut app);
    assert!(shown.contains(&96) && shown.contains(&97), "Parts 96 and 97 in view: {shown:?}");
    app.drag("~\"Part 96 explains\"", "~\"Part 97 explains\"");
    settle(&app);
    assert!(app.ui.has_text_selection(), "the drag selected the reply's words");
    // What Copy takes (the app's `copy_selection`, src/main.rs).
    let copied = app.ui.selected_text();
    assert!(copied.contains("section 96"), "copied: {copied:?}");
    assert!(copied.contains("fn part_96"), "the code between, in the run before: {copied:?}");
    assert!(copied.contains("Part 97"), "into the next run: {copied:?}");
}
