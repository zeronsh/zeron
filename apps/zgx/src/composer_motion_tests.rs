//! The composer as a column glides past it (the panel opening and closing, the
//! sidebar folding, the window resized): its footer folds once, at the width
//! it lands at, not at each width on the way; and its height follows its
//! width at once, a draft's lines rewrapping with it, not easing behind.

use std::path::PathBuf;

use zgx_script::{ScriptHost, testing::TestApp};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-composer-motion-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The shell's fixture at `w`, `chat` open (none: a new chat's page, its
/// footer the fullest: the place it starts in, the model and its effort),
/// `draft` typed in its composer.
fn chat_at(name: &str, w: f32, chat: Option<&str>, draft: &str) -> TestApp {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui").join("shell_fixture.luau");
    let storage = scratch(name);
    let mut app = TestApp::new(move |runtime| ScriptHost::new(runtime, fixture).storage(storage));
    app.ui.scene.borrow_mut().resize(w, 860.);
    app.frames(30);
    if let Some(title) = chat {
        app.click(&format!("#shell-sidebar >> role:button \"{title}\""));
        app.frames(40);
    }
    if !draft.is_empty() {
        app.type_text("#chat-composer >> role:textbox", draft);
    }
    // (Settled: past any ease.)
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(8));
        app.frame();
    }
    app
}

/// Each frame of a move: the footer as it shows (its words, and the model
/// chip's width), the composer's width and its height.
struct Frame {
    footer: String,
    width: f32,
    height: f32,
}

fn watch(app: &mut TestApp, frames: usize, act: impl FnOnce(&mut TestApp)) -> Vec<Frame> {
    act(app);
    (0..frames)
        .map(|_| {
            // (Real time too: what settles a size reads the clock.)
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.frame();
            Frame {
                footer: footer(app),
                width: app.bounds("chat-composer").width,
                height: app.bounds("chat-composer").height,
            }
        })
        .collect()
}

/// The composer's footer as it shows: its controls' and words' names, a
/// line each (the draft's own words left out), and the model chip's width.
fn footer(app: &mut TestApp) -> String {
    let root = app.find("chat-composer").or_else(|| app.find("composer")).expect("a composer");
    app.ui.prepare_frame();
    let opts = zgx_script::driver::TreeOptions { depth: None, budget: usize::MAX, all: false };
    let (lines, _) = zgx_script::driver::tree_lines(&app.ui, &app.handle, Some(root), &opts);
    let lines: Vec<zgx_script::driver::TreeLine> = lines.into_iter().filter(|l| !l.hidden).collect();
    let text = zgx_script::driver::tree_lines_text(&lines, 0);
    let shown: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| (l.contains("button") || l.contains("text \"")) && !l.contains("draft") && !l.contains("textbox"))
        .map(|l| l.split(" @").next().unwrap_or(&l).chars().filter(|c| !c.is_ascii_digit()).collect())
        .collect();
    let model = app.bounds("#chat-composer >> role:button \"Model\"").width;
    format!("{} | model {}", shown.join(" · "), model.round())
}

/// How many times the footer changed fold over the move.
fn folds(seen: &[Frame]) -> usize {
    seen.windows(2).filter(|w| w[0].footer != w[1].footer).count()
}

/// Whether the footer folded where it lands: narrowing, by the first frame
/// the width moves; widening, not before the last.
fn folds_at_the_ends(seen: &[Frame]) -> bool {
    let moves: Vec<usize> = seen.windows(2).enumerate().filter(|(_, w)| (w[0].width - w[1].width).abs() > 0.5).map(|(i, _)| i).collect();
    let (Some(&first), Some(&last)) = (moves.first(), moves.last()) else {
        return true;
    };
    let narrowing = seen[seen.len() - 1].width < seen[0].width;
    seen.windows(2)
        .enumerate()
        .filter(|(_, w)| w[0].footer != w[1].footer)
        .all(|(i, _)| if narrowing { i <= first } else { i >= last })
}

/// The frames after the composer's width last changed in which its height
/// still moved (a size easing behind its width).
fn trailing(seen: &[Frame]) -> usize {
    let last = seen.windows(2).rposition(|w| (w[0].width - w[1].width).abs() > 0.5).map_or(0, |i| i + 1);
    seen[last..].windows(2).filter(|w| (w[0].height - w[1].height).abs() > 0.5).count()
}

/// What a move did, for the log: the composer's widths, its folds, its
/// height's frames behind.
fn summary(what: &str, seen: &[Frame]) {
    let (lo, hi) = seen.iter().fold((f32::MAX, 0f32), |(lo, hi), f| (lo.min(f.width), hi.max(f.width)));
    eprintln!(
        "MOTION {what}: width {lo:.0}–{hi:.0}, {} fold changes{}, height {} frames behind",
        folds(seen),
        if folds_at_the_ends(seen) { "" } else { " (mid-move)" },
        trailing(seen)
    );
}

fn report(what: &str, seen: &[Frame]) -> String {
    let lines: Vec<String> = seen.iter().map(|f| format!("{:.0}×{:.0} {}", f.width, f.height, f.footer)).collect();
    format!("{what}:\n{}", lines.join("\n"))
}

/// The panel opening and closing over a chat, at 1440 and 1100: the footer
/// folds at most once each way, and a draft's height keeps up.
#[test]
fn the_composer_folds_once_as_the_panel_glides_and_its_height_keeps_up() {
    let draft = "A draft long enough to wrap over a few lines in the composer, so that its height depends on the width the composer is given as the panel glides open and shut again beside it.";
    for w in [1440., 1100.] {
        for (name, chat, text) in [("new", None, ""), ("draft", Some("Fix the build"), draft)] {
            let mut app = chat_at(&format!("panel-{w}-{name}"), w, chat, text);
            for way in ["opening", "closing"] {
                let seen = watch(&mut app, 40, |app| app.click("#titlebar >> role:button \"Panel\""));
                let what = format!("{w}, {name}, the panel {way}");
                summary(&what, &seen);
                assert!(folds(&seen) <= 1 && folds_at_the_ends(&seen), "{}", report(&what, &seen));
                assert_eq!(trailing(&seen), 0, "{}", report(&what, &seen));
            }
        }
    }
}

/// The sidebar folding and coming back, and the window narrowed and widened:
/// the same.
#[test]
fn the_composer_folds_once_as_the_sidebar_folds_and_the_window_resizes() {
    let draft = "A draft long enough to wrap over a few lines in the composer, so that its height depends on the width the composer is given as the columns beside it move.";
    let mut app = chat_at("sidebar", 900., None, draft);
    for way in ["folding", "coming back"] {
        let seen = watch(&mut app, 40, |app| app.click("#titlebar >> role:button \"Toggle sidebar\""));
        let what = format!("the sidebar {way}");
        summary(&what, &seen);
        assert!(folds(&seen) <= 1 && folds_at_the_ends(&seen), "{}", report(&what, &seen));
        assert_eq!(trailing(&seen), 0, "{}", report(&what, &seen));
    }
    // A resize steps the width, a frame at a time, as a window's edge drags.
    for (way, from, to) in [("narrowed", 900., 760.), ("widened", 760., 1200.)] {
        watch(&mut app, 30, |_| {});
        let mut seen = Vec::new();
        for i in 1..=12 {
            let at = from + (to - from) * i as f32 / 12.;
            app.ui.scene.borrow_mut().resize(at, 860.);
            seen.extend(watch(&mut app, 1, |_| {}));
        }
        seen.extend(watch(&mut app, 20, |_| {}));
        let what = format!("the window {way}");
        summary(&what, &seen);
        assert!(folds(&seen) <= 1, "{}", report(&what, &seen));
        assert_eq!(trailing(&seen), 0, "{}", report(&what, &seen));
    }
}

