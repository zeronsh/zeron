//! The transcript as it is drawn: a long chat opened and scrolled, its rows
//! built ahead in spare time (`zgx::spare`) as Zeron.app's are, keeps its
//! message rows apart in every frame's layout, and each frame drawn only where
//! damaged (as a window draws it) shows exactly what a whole repaint does.
//! Also with other chats' long messages folded behind Show more, and with
//! rows that come whole after they show, as a synced chat's do. (Nov 10,
//! beta.4: a chat opened with rows drawn over one another and copies of a
//! bubble's lines at old places, which leaving and coming back cleared. Not
//! reproduced here, nor in a fixture window: these hold the ways it was
//! looked for.)

use std::path::PathBuf;

use zgx::scene::Rect;
use zgx_gpu::GpuRenderer;
use zgx_script::{ScriptHost, testing::TestApp};

const W: u32 = 960;
const H: u32 = 700;

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// Two copies of the same app driven alike: one drawn as a window draws it
/// (only where damaged, scroll copies reused), one drawn whole each frame.
/// Their pixels after each frame, and where they differ.
fn drawn_against_whole(command: &str) -> Vec<String> {
    drawn_against_whole_then(command, None, false)
}
/// `then`: a command run once it has opened (rows coming whole). `back`:
/// once scrolled up into it, the chat left for another and come back to, as
/// the session page puts the view back on the message it was on.
fn drawn_against_whole_then(command: &str, then: Option<&str>, back: bool) -> Vec<String> {
    // (One test drawing at a time: renderers on one device in parallel
    // tests read each other's frames, as zgx-gpu's own pixel tests say.)
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _gpu = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // (Both on one clock, the test's, a frame's time a refresh: what moves by
    // `ui.now()` (a glide, a fold, words fading in) stands at the same point
    // in both, however far apart in wall time their frames ran. On the wall
    // clock a loaded machine could draw them at different points of an
    // animation, and they'd differ with no paint at fault.)
    let clock = zgx::clock::VirtualClock::new();
    let mut at = std::time::Duration::ZERO;
    let fixture = ui().join("chat_fixture.luau");
    let mut apps: Vec<TestApp> = (0..2)
        .map(|_| {
            let fixture = fixture.clone();
            let app = TestApp::with_spare_time(move |runtime| ScriptHost::new(runtime, fixture));
            app.ui.scene.borrow_mut().resize(W as f32, H as f32);
            app
        })
        .collect();
    let mut drawn = GpuRenderer::new(W, H).unwrap();
    let mut whole = GpuRenderer::new_with_context(W, H, &drawn.context()).unwrap();
    let mut misses = Vec::new();
    let mut refresh = |apps: &mut Vec<TestApp>, what: String| {
        at += std::time::Duration::from_secs_f64(1. / 60.);
        clock.advance(at, || {});
        for app in apps.iter() {
            app.spend_spare_time();
            app.frame();
        }
        let report = apps[0].ui.scene.borrow_mut().flush();
        drawn
            .render(&apps[0].ui.scene.borrow(), &report.damage)
            .unwrap();
        apps[1].ui.scene.borrow_mut().flush();
        whole
            .render(
                &apps[1].ui.scene.borrow(),
                &[Rect::new(0., 0., W as f32, H as f32)],
            )
            .unwrap();
        let (a, b) = (drawn.readback().unwrap(), whole.readback().unwrap());
        let differ: Vec<usize> = a
            .chunks(4)
            .zip(b.chunks(4))
            .enumerate()
            .filter(|(_, (a, b))| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > 2))
            .map(|(i, _)| i)
            .collect();
        if !differ.is_empty() {
            let ys: Vec<usize> = differ.iter().map(|i| i / W as usize).collect();
            misses.push(format!(
                "{what}: {} pixels differ from a whole repaint, rows {}..={}",
                differ.len(),
                ys.iter().min().unwrap(),
                ys.iter().max().unwrap()
            ));
        }
    };
    for step in 0..20 {
        refresh(&mut apps, format!("shell {step}"));
    }
    for app in &apps {
        assert!(app.host.run_command(command));
        assert!(app.host.run_command("go_s7"));
    }
    for step in 0..60 {
        refresh(&mut apps, format!("opening {step}"));
    }
    if let Some(then) = then {
        for app in &apps {
            assert!(app.host.run_command(then));
        }
        for step in 0..30 {
            refresh(&mut apps, format!("{then} {step}"));
        }
    }
    for step in 0..60 {
        for app in apps.iter_mut() {
            let list = app.find("transcript").expect("the transcript");
            zgx_script::driver::wheel(&mut app.ui, list, 0., -24., None);
        }
        refresh(&mut apps, format!("up {step}"));
    }
    if back {
        for step in 0..20 {
            refresh(&mut apps, format!("resting {step}"));
        }
        for app in &apps {
            assert!(app.host.run_command("go_s2"));
        }
        for step in 0..20 {
            refresh(&mut apps, format!("away {step}"));
        }
        for app in &apps {
            assert!(app.host.run_command("go_s7"));
        }
        for step in 0..60 {
            refresh(&mut apps, format!("back {step}"));
        }
    }
    for app in &apps {
        assert_eq!(app.error(), None);
    }
    misses
}

#[test]
fn a_long_chat_opened_and_scrolled_draws_what_a_whole_repaint_draws() {
    let misses = drawn_against_whole("long_chat");
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}

#[test]
fn a_long_chat_come_back_to_mid_scroll_draws_what_a_whole_repaint_draws() {
    for command in ["long_chat", "long_chat_cross"] {
        let misses = drawn_against_whole_then(command, None, true);
        assert!(misses.is_empty(), "{command}: {}", misses.join("\n"));
    }
}

#[test]
fn a_long_chat_of_other_chats_messages_opened_and_scrolled_draws_what_a_whole_repaint_draws() {
    let misses = drawn_against_whole("long_chat_cross");
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}

/// The transcript's message rows, in the tree's order: each one's top and
/// height. (A message is an `article`, the rows' outer box the line above.)
fn rows(app: &mut TestApp) -> Vec<(String, f32, f32)> {
    let tree = app.tree();
    let mut out = Vec::new();
    let mut inside = false;
    let mut depth = 0;
    for line in tree.lines() {
        let indent = line.len() - line.trim_start().len();
        if line.contains("#transcript ") {
            inside = true;
            depth = indent;
            continue;
        }
        if inside && indent <= depth {
            break;
        }
        // A message: an article.
        if inside && line.contains(" article ") {
            let mut parts = line.split_whitespace().rev();
            let (Some(size), Some(place)) = (parts.next(), parts.next()) else {
                continue;
            };
            let (Some((_, y)), Some((_, h))) = (place.split_once(','), size.split_once('x')) else {
                continue;
            };
            if let (Ok(y), Ok(h)) = (y.parse::<f32>(), h.parse::<f32>()) {
                out.push((line.trim().chars().take(60).collect(), y, h));
            }
        }
    }
    out
}

fn overlaps(app: &mut TestApp) -> Option<String> {
    let rows = rows(app);
    rows.windows(2).find_map(|pair| {
        let ((a, ay, ah), (b, by, _)) = (&pair[0], &pair[1]);
        (*by < ay + ah - 0.5)
            .then(|| format!("{b:?} at {by} starts above the foot of {a:?} ({ay} + {ah})"))
    })
}

fn rows_kept_apart(command: &str) {
    rows_kept_apart_then(command, None)
}
fn rows_kept_apart_then(command: &str, then: Option<&str>) {
    let fixture = ui().join("chat_fixture.luau");
    let mut app = TestApp::with_spare_time(move |runtime| ScriptHost::new(runtime, fixture));
    app.ui.scene.borrow_mut().resize(W as f32, H as f32);
    app.frames(20);
    assert!(app.host.run_command(command));
    assert!(app.host.run_command("go_s7"));
    let mut found = Vec::new();
    for step in 0..60 {
        app.spend_spare_time();
        app.frame();
        if let Some(o) = overlaps(&mut app) {
            found.push(format!("opening {step}: {o}"));
        }
    }
    if let Some(then) = then {
        assert!(app.host.run_command(then));
        for step in 0..30 {
            app.spend_spare_time();
            app.frame();
            if let Some(o) = overlaps(&mut app) {
                found.push(format!("{then} {step}: {o}"));
            }
        }
    }
    let list = app.find("transcript").expect("the transcript");
    for step in 0..120 {
        zgx_script::driver::wheel(&mut app.ui, list, 0., -40., None);
        app.spend_spare_time();
        app.frame();
        if let Some(o) = overlaps(&mut app) {
            found.push(format!("up {step}: {o}"));
        }
    }
    if std::env::var_os("SHOW_TREE").is_some() {
        let tree = app.tree();
        let at = tree.find("#transcript ").unwrap();
        eprintln!(
            "{}",
            &tree[at..].lines().take(60).collect::<Vec<_>>().join("\n")
        );
    }
    assert!(!rows(&mut app).is_empty(), "rows read");
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn a_long_chat_of_other_chats_messages_keeps_its_rows_apart() {
    rows_kept_apart("long_chat_cross");
}

#[test]
fn rows_that_come_whole_after_they_show_stay_apart_and_drawn() {
    rows_kept_apart_then("long_chat_cross_late", Some("long_chat_cross_fill"));
    let misses =
        drawn_against_whole_then("long_chat_cross_late", Some("long_chat_cross_fill"), false);
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}
