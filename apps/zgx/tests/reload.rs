//! A hot reload changes nothing anyone can see, and asks the host for
//! nothing it already has.
//!
//! zgx runs every module again on a reload and builds every node again,
//! keeping state by key (docs/reference/app-structure.md, "Hot reload and
//! modules"). So an animation that plays as a node mounts, a spring whose
//! target waits on a measurement, or a load asked for while building plays
//! or asks again on every save. This mounts each window's script headless
//! with every host action stubbed, lets it settle, reloads it, and checks
//! that the interface stands exactly where it stood (every node's box, at
//! once and while the frames after the reload run) and that the reload
//! asked for no data again.
//!
//! The Lua test kit (`zgx test`) can't reload; the host can.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use zgx::{inspect::Inspector, reactive::Runtime};
use zgx_script::{ScriptHost, Value, testing::TestApp};

/// What scripts call with `ui.call` (answered with nothing).
const CALLS: &[&str] = &[
    "about.apply",
    "about.check",
    "account.delete",
    "account.photo_file",
    "account.photo_size",
    "account.remove_photo",
    "account.set_photo",
    "account.set_title",
    "account.card",
    "account.check_username",
    "account.choose_org",
    "account.claim_username",
    "account.create_org",
    "account.invite",
    "account.onboarded",
    "account.onboarding",
    "account.orgs",
    "account.sign_in",
    "account.sign_out",
    "account.username",
    "account.me",
    "account.rename",
    "account.members",
    "account.invitations",
    "account.remove_member",
    "account.set_role",
    "account.revoke_invite",
    "account.resend_invite",
    "account.rename_org",
    "agents.complete",
    "agents.install",
    "agents.remove",
    "agents.set_enabled",
    "agents.sign_in",
    "agents.switch",
    "agents.update",
    "agents.set_update_policy",
    "chat.configure",
    "chat.interrupt",
    "chat.queue",
    "chat.queue_edit",
    "chat.queue_finish",
    "chat.respond",
    "chat.retry",
    "chat.send",
    "files.create",
    "files.delete",
    "files.list",
    "files.move",
    "files.open_external",
    "files.read",
    "files.reveal",
    "files.search",
    "files.watch",
    "files.write",
    "plugins.new",
    "plugins.remove",
    "plugins.set_off",
    "notifications.get",
    "notifications.set",
    "settings.engine",
    "sessions.archive",
    "sessions.create",
    "sessions.delete",
    "sessions.rename",
    "sessions.rename_project",
    "sessions.unarchive",
    "templates.make_editable",
    "templates.reset",
    "wallpaper.keep",
];

/// What they invoke with `ui.invoke` (and the host's own window actions).
const INVOKES: &[&str] = &[
    "account.cancel",
    "agents.cancel",
    "agents.dismiss",
    "agents.load",
    "app.onboard",
    "app.open_main",
    "appearance",
    "chat.catalog",
    "chat.open",
    "done",
    "files.unwatch",
    "github.cancel",
    "github.choose",
    "github.connect",
    "github.disconnect",
    "github.load",
    "github.open",
    "interactive",
    "open_url",
    "plugins.decide",
    "plugins.menu",
    "plugins.open_folder",
    "plugins.review",
    "plugins.sign",
    "prepare",
    "reveal",
    "sessions.collapse",
    "sessions.places",
    "sessions.seen",
    "sound.play",
    "templates.failed",
    "wallpaper.forget",
    "templates.open",
    "close",
    "read_selection",
    "clear_selection",
    "copy_selection",
    "look_up_selection",
    "select_all",
];

/// What asks the host for data (as against telling it something, which a
/// reload may repeat harmlessly: the scheme, where sessions are put).
fn fetches(name: &str) -> bool {
    // (A watch asked again within the host's grace is kept, not read again:
    // src/files.rs `files.watch`.)
    name != "files.watch"
        && (name.ends_with(".load")
        || name.ends_with(".list")
        || name.ends_with(".watch")
        || name.ends_with(".read")
        || name.ends_with(".search")
        || matches!(
            name,
            "chat.open"
                | "chat.catalog"
                | "account.card"
                | "account.orgs"
                | "account.username"
                | "account.onboarding"
                | "account.me"
                | "account.members"
                | "account.invitations"
                | "notifications.get"
                | "settings.engine"
                | "about.check"
                | "account.photo_file"
        ))
}

/// What the stubs answer, as JSON (the rest answer nothing): an account
/// with its username and card.
const ANSWERS: &[(&str, &str)] = &[
    ("account.username", r#""ada""#),
    ("account.orgs", "[]"),
    ("account.onboarding", "[]"),
    (
        "account.card",
        r#"{"username":"ada","first_name":"Ada","number":7,"joined_on":"2026-10-30","badges":[],"scene":1,"holo":"aurora","sky":1}"#,
    ),
];

/// What the stubs answer that's made rather than written out: a folder
/// of files and a long file, for a panel scrolled down both (as JSON).
fn made_answer(name: &str) -> Option<String> {
    match name {
        "files.list" => {
            let entries: Vec<String> = (0..80)
                .map(|i| format!(r#"{{"name":"file-{i:02}.txt","path":"file-{i:02}.txt","dir":false,"kind":"file"}}"#))
                .collect();
            Some(format!(
                r#"{{"here":true,"checkout":"c","can_move":true,"can_delete":true,"truncated":false,"entries":[{}]}}"#,
                entries.join(",")
            ))
        }
        "files.read" => {
            let text: Vec<String> = (1..=400).map(|i| format!("line {i}")).collect();
            Some(format!(
                r#"{{"here":true,"checkout":"c","path":"file-70.txt","text":"{}","hash":"h","size":4000,"encoding":"utf-8","line_ending":"lf","truncated":false}}"#,
                text.join("\\n")
            ))
        }
        // (As the host answers a watch asked again within its grace, as a
        // reload asks: kept, every change since in its frames.)
        "files.watch" => Some(r#"{"kept":true}"#.into()),
        _ => None,
    }
}

type Log = Rc<RefCell<Vec<String>>>;

/// The scripts: this checkout's `ui/`, or `ZERON_ZGX_TEST_UI`'s (another
/// copy, to see the test fail on code from before a fix).
fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The app's entry for `window` ("main", or "onboarding" opening at
/// `step`), every host action a stub that notes its name.
fn mount(window: &'static str, step: &'static str, log: &Log) -> TestApp {
    mount_with(window, step, log, None)
}

/// `mount`, with the window's layout inspector as a desktop window has it
/// (`inspector`), else none.
fn mount_with(window: &'static str, step: &'static str, log: &Log, inspector: Option<&Inspector>) -> TestApp {
    let log = log.clone();
    let build = move |runtime: &Runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("app.luau"));
        host.shared("window", window);
        host.shared("onboarding.start", step);
        // Setup's window as the host shows it once the intro is over.
        host.shared("onboarding.arrived", true);
        // The models, as the host answers `chat.catalog`: looked up, ready.
        let catalog = host.shared(
            "chat.catalog",
            Value::from_json(r#"{"state":"looking","harnesses":[],"models":[]}"#).unwrap(),
        );
        for &name in INVOKES {
            let log = log.clone();
            let catalog = catalog.clone();
            host.action(name, move |_| {
                log.borrow_mut().push(name.to_owned());
                if name == "chat.catalog" {
                    catalog.set(Value::from_json(r#"{"state":"ready","harnesses":[],"models":[]}"#).unwrap());
                }
            });
        }
        for &name in CALLS {
            let log = log.clone();
            let answer = ANSWERS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, json)| (*json).to_owned())
                .or_else(|| made_answer(name))
                .and_then(|json| Value::from_json(&json))
                .unwrap_or(Value::Nil);
            host.async_action(name, move |_| {
                log.borrow_mut().push(name.to_owned());
                let answer = answer.clone();
                async move { Ok(answer) }
            });
        }
        host
    };
    let app = match inspector {
        Some(inspector) => TestApp::inspected(build, inspector),
        None => TestApp::new(build),
    };
    // As the main window opens (src/main.rs `main_window`): the shell fits
    // its columns to the window, and a test's 400 has no room for the
    // sidebar beside the chat (screens/shell/fit).
    app.ui.scene.borrow_mut().resize(1240., 820.);
    app.frames(1);
    app
}

/// Every node's place and size (its identity left out: a reload builds
/// new nodes), a line each.
///
/// What moves by the clock is left out: Sign in's wall of cards (`#wall`)
/// and a member card's art (`#card-art`) drift by `ui.now()` (each reckoned
/// from when it first stood), and a test's frames run faster than the
/// clock, so they stand elsewhere after any wait.
fn boxes(app: &mut TestApp) -> Vec<String> {
    lines(&app.tree()).into_iter().map(|(_, b)| b).collect()
}

/// The tree's lines that count, each with its line number in the tree.
fn lines(tree: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut skip: Option<usize> = None;
    for (n, line) in tree.lines().enumerate() {
        let depth = line.len() - line.trim_start().len();
        if let Some(d) = skip {
            if depth > d {
                continue;
            }
            skip = None;
        }
        let line = line.trim_start();
        if ["#wall", "#card-art"].iter().any(|id| line.contains(&format!(" {id} ")) || line.ends_with(&format!(" {id}"))) {
            skip = Some(depth);
            continue;
        }
        out.push((
            n,
            match line.find(' ') {
                Some(at) if line.starts_with('@') => line[at + 1..].to_owned(),
                _ => line.to_owned(),
            },
        ));
    }
    out
}

/// The frames looked at after a reload: at once, then as an animation
/// would run.
const LOOKS: [(usize, usize); 4] = [(1, 1), (3, 2), (10, 7), (60, 50)];

/// A line of the tree with the lines that hold it, for saying which node.
fn place(tree: &str, index: usize) -> String {
    let lines: Vec<&str> = tree.lines().collect();
    let depth = |l: &str| l.len() - l.trim_start().len();
    let mut out = vec![lines[index].trim().to_owned()];
    let mut d = depth(lines[index]);
    for l in lines[..index].iter().rev() {
        if depth(l) < d {
            out.push(l.trim().to_owned());
            d = depth(l);
        }
    }
    out.join("  <  ")
}

fn reload_changes_nothing(window: &'static str, step: &'static str) {
    let log: Log = Rc::default();
    let mut app = mount(window, step, &log);
    // Settled: every load answered, every animation done.
    app.frames(120);
    assert_eq!(app.error(), None, "{window}: the script failed");
    reload_holds_still(&mut app, &log, window, step, &[]);
}

/// A reload of `app` as it stands moves nothing and asks for nothing again
/// (but what `known` names: asked again for a reason told where it's given).
/// The tree for `ZERON_ZGX_TEST_DUMP`: the window's (200 lines), or, with
/// `ZERON_ZGX_TEST_DUMP=full`, every line of it.
fn dump(app: &mut TestApp) -> String {
    if std::env::var("ZERON_ZGX_TEST_DUMP").as_deref() != Ok("full") {
        return app.tree();
    }
    let options = zgx_script::driver::TreeOptions { budget: 100_000, ..Default::default() };
    let (lines, hidden) = zgx_script::driver::tree_lines(&app.ui, &app.handle, None, &options);
    zgx_script::driver::tree_lines_text(&lines, hidden)
}

fn reload_holds_still(app: &mut TestApp, log: &Log, window: &str, step: &str, known: &[&str]) {
    // What moves without a reload (a wall of cards drifting, a spinner):
    // not the reload's doing, left out.
    let rest = boxes(app);
    let mut live = vec![false; rest.len()];
    for (_, n) in LOOKS {
        app.frames(n);
        for (i, (a, b)) in rest.iter().zip(boxes(app)).enumerate() {
            live[i] |= *a != b;
        }
    }
    let before = boxes(app);
    log.borrow_mut().clear();

    if std::env::var_os("ZERON_ZGX_TEST_DUMP").is_some() {
        eprintln!("{window} at {step}, before the reload:\n{}", dump(app));
    }
    app.host.reload().expect("the reload failed");
    if std::env::var_os("ZERON_ZGX_TEST_DUMP").is_some() {
        app.frames(1);
        eprintln!("{window} at {step}, a frame after:\n{}", dump(app));
    }
    let mut moved = Vec::new();
    for (frame, n) in LOOKS {
        app.frames(n);
        let now = boxes(app);
        if now.len() != before.len() {
            moved.push(format!("frame {frame}: {} nodes → {}", before.len(), now.len()));
            continue;
        }
        if let Some(i) = (0..now.len()).find(|&i| !live[i] && now[i] != before[i]) {
            let tree = app.tree();
            let at = lines(&tree)[i].0;
            moved.push(format!("frame {frame}: {}  →  {}   ({})", before[i], now[i], place(&tree, at)));
        }
    }
    assert_eq!(app.error(), None, "{window}: the reload failed");
    let asked: Vec<String> = log.borrow().iter().filter(|n| fetches(n) && !known.contains(&n.as_str())).cloned().collect();
    assert!(
        moved.is_empty() && asked.is_empty(),
        "{window} at {step}: a reload moved the interface: {moved:#?}\nand asked for again: {asked:?}"
    );
}

#[test]
fn a_reload_of_the_main_window_moves_and_fetches_nothing() {
    reload_changes_nothing("main", "signin");
}

/// The project editor (screens/shell/project_editor) loads as it's first
/// opened, not with the window: open across a reload, it's there again, as
/// it stood.
#[test]
fn a_reload_keeps_the_project_editor_open() {
    let log: Log = Rc::default();
    let mut app = mount("main", "signin", &log);
    app.frames(120);
    assert_eq!(app.error(), None, "the script failed");
    // (Not loaded yet: its states aren't there until it's first opened.)
    assert!(app.host.states().iter().all(|(name, _)| !name.ends_with("::project_edit_open")));
    app.host.set_state("shell_project_editor_used", Value::Bool(true)).expect("the sidebar's flag");
    app.frames(2);
    app.host.set_state("project_edit_open", Value::Bool(true)).expect("the editor's state, once loaded");
    app.frames(120);
    assert!(app.tree().contains("dialog"), "the editor's dialog shows:\n{}", app.tree());
    reload_holds_still(&mut app, &log, "main", "signin", &[]);
    assert!(app.tree().contains("dialog"), "the editor's dialog is open again after the reload");
}

#[test]
fn a_reload_of_setup_moves_and_fetches_nothing() {
    let mut failed = Vec::new();
    for step in ["signin", "appearance", "username", "card", "org", "invite", "github", "providers", "done"] {
        if let Err(e) = std::panic::catch_unwind(|| reload_changes_nothing("onboarding", step)) {
            failed.push(e.downcast_ref::<String>().cloned().unwrap_or_default());
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// A launch with devtools (`--inspect`, as the main window runs while it's
/// worked on) comes up once, and the engine arriving after it changes the
/// sessions in place.
///
/// Devtools label every node with the script line that made it. Asked for
/// only once the window has mounted, zgx loads every script again just
/// after the first frame, so the window came up twice, the second time as
/// an engine already running connected: focus, scroll and what was loading
/// lost on every launch. src/main.rs asks for the labels before anything
/// loads (`ZGX_LAYOUT_WARNINGS`); this mounts the main window as it then
/// does, with the inspector's warnings on as devtools turn them on, and
/// checks the nodes stand as they were made once the engine's sessions land.
#[test]
fn a_launch_with_devtools_comes_up_once_as_the_engine_arrives() {
    // As src/main.rs sets it under `--inspect`, before anything reads it.
    // SAFETY: std's own readers (all this test's code, zgx's included) take
    // its lock on the environment.
    unsafe { std::env::set_var("ZGX_LAYOUT_WARNINGS", "1") };
    // The window's inspector, made from the environment as a desktop
    // window makes it; the window mounted.
    let inspector = Inspector::new();
    let log: Log = Rc::default();
    let mut app = mount_with("main", "signin", &log, Some(&inspector));
    let made = |app: &mut TestApp| {
        ["#shell", "#shell-sidebar", "#titlebar"].map(|id| app.find(id).unwrap_or_else(|| panic!("no {id}")))
    };
    let first = made(&mut app);
    // Devtools start (src/main.rs's `serve`, once the view is made): their
    // session turns the inspector's warnings on, and its labels with them.
    inspector.set_warnings(true);
    app.frames(30);
    assert_eq!(app.error(), None, "the script failed");
    assert_eq!(made(&mut app), first, "the window was made again as it came up: its scripts loaded twice");

    // The engine connects: its devices, a project and a session.
    app.host.shared("sessions", Value::Nil).set(
        Value::from_json(
            r#"{"state":"ready","here":"mac",
                "devices":[{"id":"mac","name":"This Mac","platform":"macos","here":true}],
                "projects":[{"id":"atlas","name":"Atlas","device":"mac","path":"/work/atlas","git":true}],
                "chats":[{"id":"c1","title":"Fix the build","project":"atlas","device":"mac","updated":1,"created":1,"agent":"claude"}],
                "archived":[],"places":{"pinned":[],"sections":[]},"places_ready":true}"#,
        )
        .unwrap(),
    );
    app.frames(60);
    assert_eq!(app.error(), None, "the engine's sessions failed");
    assert!(app.find("#shell-sidebar >> \"Fix the build\"").is_some(), "the session never showed");
    assert_eq!(made(&mut app), first, "the window was made again: its scripts loaded twice");
}

/// The engine's sessions, as it sends them: a session in a project here.
const SESSIONS: &str = r#"{"state":"ready","here":"mac",
    "devices":[{"id":"mac","name":"This Mac","platform":"macos","here":true}],
    "projects":[{"id":"atlas","name":"Atlas","device":"mac","path":"/work/atlas","git":true}],
    "chats":[{"id":"c1","title":"Fix the build","project":"atlas","device":"mac","updated":1,"created":1,"agent":"claude"}],
    "archived":[],"places":{"pinned":[],"sections":[]},"places_ready":true}"#;

/// The first node of `kind` (a tree line's word after its id) under the
/// node `under`, as a target. (Read from `under`'s own tree: the window's
/// whole one is cut off after 200 lines, and a long file tree before it
/// runs past that.)
fn first_under(app: &mut TestApp, under: &str, kind: &str) -> String {
    let root = app.find(under).unwrap_or_else(|| panic!("no {under}:\n{}", app.tree()));
    let options = zgx_script::driver::TreeOptions { budget: 100_000, ..Default::default() };
    let (lines, hidden) = zgx_script::driver::tree_lines(&app.ui, &app.handle, Some(root), &options);
    let tree = zgx_script::driver::tree_lines_text(&lines, hidden);
    let mut inside: Option<usize> = None;
    for line in tree.lines() {
        let depth = line.len() - line.trim_start().len();
        let t = line.trim_start();
        match inside {
            None if t.contains(&format!(" {under} ")) || t.ends_with(&format!(" {under}")) => inside = Some(depth),
            Some(d) if depth <= d => break,
            Some(_) => {
                // (`@n`, then its `#id` if it has one, then its kind.)
                let mut words = t.split(' ').filter(|w| !w.starts_with('#'));
                if let (Some(id), Some(k)) = (words.next(), words.next()) {
                    if k == kind && id.starts_with('@') {
                        return id.to_owned();
                    }
                }
            }
            None => {}
        }
    }
    panic!("no {kind} under {under}:\n{tree}")
}

/// Frames, and real time for host calls and timers, until `done` holds
/// (ten seconds at most): whether it did.
fn frames_until(app: &mut TestApp, mut done: impl FnMut(&mut TestApp) -> bool) -> bool {
    for _ in 0..2000 {
        app.frames(2);
        if done(app) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    done(app)
}

/// A session's panel scrolled every way it scrolls (its tab strip past
/// its start, its tree and its file down) stands as it was through a
/// reload, and nothing it shows is asked for again.
#[test]
fn a_reload_keeps_the_panel_where_it_was_scrolled() {
    let log: Log = Rc::default();
    let mut app = mount("main", "signin", &log);
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    app.host.shared("sessions", Value::Nil).set(Value::from_json(SESSIONS).unwrap());
    // The session open, as the host has it once the chat page asks.
    app.host
        .shared("chat.session", Value::Nil)
        .set(Value::from_json(r#"{"id":"c1","state":"ready","title":"Fix the build","running":false,"queue":[],"todos":[],"tasks":[]}"#).unwrap());
    app.frames(60);
    app.click("#shell-sidebar >> \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    // Enough tabs that the strip runs past its end (each new one shown, at
    // the end: the strip scrolled there).
    let tabs = |app: &mut TestApp| app.tree().matches(" tab \"").count();
    for n in 1..=9 {
        if n > 1 {
            app.click("#panel-strip >> role:button \"New Tab\"");
            assert!(frames_until(&mut app, |app| app.find("#panel-open-files").is_some()), "the empty page never came back");
        }
        app.click("#panel-open-files");
        assert!(frames_until(&mut app, |app| tabs(app) == n), "tab {n} never opened:\n{}", app.tree());
        app.frames(10);
    }
    assert_eq!(app.error(), None, "the panel failed");
    // The one before the last shown (in view already: the strip stays
    // scrolled, not brought to the end by the last being shown).
    app.click("#panel-tab-tab8");
    app.frames(30);
    // In it: a file far down the tree, opened, and read far down.
    assert!(frames_until(&mut app, |app| app.tree().contains(" treeitem \"")), "the folder was never read");
    app.frames(10);
    let tree = first_under(&mut app, "#files-pane", "tree");
    app.scroll(&tree, 1200.);
    app.frames(20);
    // (A row well down the tree, as it now stands.)
    let row = app
        .tree()
        .lines()
        .filter_map(|l| {
            let t = l.trim_start();
            let name = t.split('"').nth(1)?.to_owned();
            let y: f32 = t.split(' ').find(|w| w.contains(','))?.split(',').nth(1)?.parse().ok()?;
            (t.contains(" treeitem \"") && y > 400. && y < 600.).then_some(name)
        })
        .next()
        .unwrap_or_else(|| panic!("no row half way down the tree"));
    assert!(row != "file-00.txt", "the tree never scrolled");
    app.click(&format!("role:treeitem \"{row}\""));
    let editor_box = format!("role:textbox \"{row}\"");
    assert!(frames_until(&mut app, |app| app.find(&editor_box).is_some()), "{row} never opened");
    app.frames(30);
    let editor = first_under(&mut app, "#file-viewer", "scroll");
    app.scroll(&editor, 3000.);
    app.frames(120);
    assert!(app.find("role:button \"Expand panel\"").is_some(), "the panel was expanded by a click meant for its tabs");
    let first_tab = app.bounds("#panel-tab-tab1");
    let strip = app.bounds("#panel-strip");
    assert!(first_tab.x < strip.x, "the strip never scrolled past its start ({} in {})", first_tab.x, strip.x);
    reload_holds_still(&mut app, &log, "main", "a scrolled panel", &[]);
}

/// A long session's transcript, as the host publishes it: `turns` prompts,
/// each with a reply of ten paragraphs (far taller than a row's estimate).
fn long_transcript(turns: usize) -> String {
    let mut out = Vec::new();
    for i in 1..=turns {
        out.push(format!(
            r#"{{"id":"u{i}","role":"user","text":"Turn {i}: tell me more.","at":"Sep 9, 3:00 PM","copy":"Turn {i}"}}"#
        ));
        let text = (1..=10)
            .map(|k| format!("Reply {i}, paragraph {k}: the layout pass runs first, then paint, then present."))
            .collect::<Vec<_>>()
            .join("\\n\\n");
        out.push(format!(
            r#"{{"id":"a{i}","role":"assistant","text":"{text}","at":"Sep 9, 3:01 PM","copy":"x","blocks":[{{"id":"t","kind":"text","text":"{text}"}}]}}"#
        ));
    }
    format!("[{}]", out.join(","))
}

/// The message at the top of the transcript's view and where it stands:
/// the first article whose box reaches into the list.
fn top_message(app: &mut TestApp) -> (String, i32) {
    let tree = app.tree();
    let mut list_top: Option<i32> = None;
    for line in tree.lines() {
        let t = line.trim_start();
        let words: Vec<&str> = t.split_whitespace().collect();
        let place = words.windows(2).rev().find_map(|w| {
            let (_, y) = w[0].split_once(',')?;
            let (_, h) = w[1].split_once('x')?;
            Some((y.parse::<i32>().ok()?, h.parse::<i32>().ok()?))
        });
        if t.contains(" #transcript ") {
            list_top = place.map(|(y, _)| y);
            continue;
        }
        if let (Some(top), Some((y, h))) = (list_top, place)
            && t.contains("article")
            && y + h > top
        {
            let name = t.split_once(" article ").map_or(t, |(_, rest)| rest);
            let what = name.split(" \"").nth(1).unwrap_or(name).to_owned();
            return (format!("{}|{what}", words.get(1).copied().unwrap_or_default()), y);
        }
    }
    panic!("no message in view:\n{tree}")
}

/// A session scrolled half way up its conversation stands where it was
/// through a reload: the same message at the top of the view, at the same
/// height, at once and as the frames after it run. (A reload made the
/// transcript again with only estimates for the rows above, and it opened
/// at the end, or somewhere near where it was.)
#[test]
fn a_reload_keeps_a_session_scrolled_where_it_was() {
    let log: Log = Rc::default();
    let mut app = mount("main", "signin", &log);
    app.frames(30);
    app.host.shared("sessions", Value::Nil).set(Value::from_json(SESSIONS).unwrap());
    app.host
        .shared("chat.session", Value::Nil)
        .set(Value::from_json(r#"{"id":"c1","state":"ready","title":"Fix the build","running":false,"queue":[],"todos":[],"tasks":[]}"#).unwrap());
    app.host.shared("chat.messages", Value::Nil).set(Value::from_json(&long_transcript(30)).unwrap());
    app.frames(60);
    app.click("#shell-sidebar >> \"Fix the build\"");
    app.frames(60);
    // Half way up, in steps as a hand scrolls, then rested (the place is
    // kept 150 ms after the scroll stops, on the clock).
    for _ in 0..12 {
        app.scroll("#transcript", -900.);
        app.frames(4);
    }
    app.frames(60);
    std::thread::sleep(std::time::Duration::from_millis(400));
    app.frames(60);
    assert_eq!(app.error(), None, "the session failed");
    let before = top_message(&mut app);
    assert!(!before.0.contains("u30") && !before.0.contains("a30"), "never scrolled up: {before:?}");
    let asked = log.borrow().len();
    let _ = app.host.reload();
    for (frame, n) in [(1, 1), (3, 2), (10, 7), (60, 50)] {
        app.frames(n);
        let now = top_message(&mut app);
        assert_eq!(now, before, "frame {frame} after the reload: the view moved");
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.frames(60);
    assert_eq!(top_message(&mut app), before, "once settled after the reload: the view moved");
    // Nor did the session's page ask the host for it again (`chat.open`).
    let again: Vec<String> = log.borrow()[asked..].iter().filter(|n| fetches(n)).cloned().collect();
    assert!(again.is_empty(), "the reload asked the host again: {again:?}");
}

/// Sixty sessions under one project: a sidebar list to scroll.
fn many_sessions() -> String {
    let chats: Vec<String> = (1..=60)
        .map(|i| format!(r#"{{"id":"m{i}","title":"Session {i}","project":"atlas","device":"mac","updated":{},"created":1,"agent":"claude"}}"#, 1000 - i))
        .collect();
    format!(
        r#"{{"state":"ready","here":"mac",
    "devices":[{{"id":"mac","name":"This Mac","platform":"macos","here":true}}],
    "projects":[{{"id":"atlas","name":"Atlas","device":"mac","path":"/work/atlas","git":true}}],
    "chats":[{}],
    "archived":[],"places":{{"pinned":[],"sections":[]}},"places_ready":true}}"#,
        chats.join(",")
    )
}

/// Where `selector`'s matches stand on screen (a scroll's offset taken
/// in, however it scrolled): each one's top, in order.
fn on_screen(app: &mut TestApp, selector: &str) -> Vec<String> {
    let target = zgx_script::driver::parse(selector).unwrap();
    app.ui.prepare_frame();
    let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
    assert!(!found.is_empty(), "nothing matches {selector}");
    let scene = app.ui.scene.borrow();
    found
        .into_iter()
        .filter(|&id| scene.layout_visible(id))
        .map(|id| format!("{:.1}", scene.transformed_bounds(id).y))
        .collect()
}

/// The sidebar scrolled down its sessions, and settings' page scrolled
/// down, stand where they were through a reload of the scripts.
#[test]
fn a_reload_keeps_the_sidebar_and_settings_where_they_were_scrolled() {
    let log: Log = Rc::default();
    let mut app = mount("main", "signin", &log);
    app.ui.scene.borrow_mut().resize(1200., 600.);
    app.frames(30);
    app.host.shared("sessions", Value::Nil).set(Value::from_json(&many_sessions()).unwrap());
    app.frames(60);
    assert_eq!(app.error(), None, "the shell failed");
    // Down the sidebar's list, in steps as a hand scrolls.
    for _ in 0..6 {
        let row = (1..=60)
            .map(|i| format!("#shell-sidebar >> role:button \"Session {i}\""))
            .find(|row| app.find(row).is_some())
            .expect("a row in view");
        app.scroll(&row, 150.);
        app.frames(4);
    }
    app.frames(60);
    let rows = "#shell-sidebar >> role:button";
    let before = on_screen(&mut app, rows);
    assert!(app.find("#shell-sidebar >> role:button \"Session 1\"").is_none(), "the sidebar never scrolled");
    let _ = app.host.reload();
    for (frame, n) in [(1, 1), (3, 2), (10, 7), (60, 50)] {
        app.frames(n);
        assert_eq!(on_screen(&mut app, rows), before, "frame {frame} after the reload: the sidebar moved");
    }

    // Settings' page, scrolled down.
    assert!(app.host.run_command("shell.settings"));
    app.frames(60);
    for _ in 0..4 {
        // (At a heading in view: the page runs on below the window.)
        let target = zgx_script::driver::parse("#settings-page >> role:heading").unwrap();
        app.ui.prepare_frame();
        let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
        let id = found
            .into_iter()
            .find(|&id| {
                let b = app.ui.scene.borrow().hit_bounds(id);
                b.y > 40. && b.y < 560.
            })
            .expect("a heading in view");
        zgx_script::driver::scroll(&mut app.ui, id, 150.);
        app.settle();
        app.frames(4);
    }
    app.frames(60);
    let headings = "#settings-page >> role:heading";
    let before = on_screen(&mut app, headings);
    assert!(before[0] != "54.0", "settings never scrolled: {before:?}");
    let _ = app.host.reload();
    for (frame, n) in [(1, 1), (3, 2), (10, 7), (60, 50)] {
        app.frames(n);
        assert_eq!(on_screen(&mut app, headings), before, "frame {frame} after the reload: settings' page moved");
    }
    // Settled: where it is is kept, and a scroll moves it on.
    let scroll = |app: &TestApp| {
        app.host.states().into_iter().find(|(n, _)| n.ends_with("::shell_settings_scroll")).map(|(_, v)| v)
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    app.frames(30);
    assert_eq!(on_screen(&mut app, headings), before, "once settled after the reload: settings' page moved");
    let kept = scroll(&app);
    let heading = app.target("#settings-page >> role:heading \"Links\"").expect("a heading in view");
    zgx_script::driver::scroll(&mut app.ui, heading, -100.);
    app.settle();
    app.frames(30);
    assert_ne!(scroll(&app), kept, "a scroll after the reload isn't kept ({kept:?})");
}

/// A session whose replies rewrap at any width: ten long paragraphs each.
fn wrapping_transcript(turns: usize) -> String {
    let mut out = Vec::new();
    for i in 1..=turns {
        out.push(format!(r#"{{"id":"u{i}","role":"user","text":"Turn {i}: tell me more.","at":"Sep 9, 3:00 PM","copy":"x"}}"#));
        let text = (1..=10)
            .map(|k| {
                format!(
                    "Reply {i}, paragraph {k}: the layout pass runs first and measures every row it shows, then paint fills the scene, then present hands the frame to the display; a width change asks layout again for every line that wraps, so a long paragraph like this one takes a different number of lines at each width."
                )
            })
            .collect::<Vec<_>>()
            .join("\\n\\n");
        out.push(format!(
            r#"{{"id":"a{i}","role":"assistant","text":"{text}","at":"Sep 9, 3:01 PM","copy":"x","blocks":[{{"id":"t","kind":"text","text":"{text}"}}]}}"#
        ));
    }
    format!("[{}]", out.join(","))
}

/// The top of the first tree line holding `needle`, and its height.
fn place_of(app: &mut TestApp, needle: &str) -> Option<(i32, i32)> {
    app.tree().lines().find(|l| l.contains(needle)).and_then(|l| {
        let words: Vec<&str> = l.split_whitespace().collect();
        words.windows(2).rev().find_map(|w| {
            let (_, y) = w[0].split_once(',')?;
            let (_, h) = w[1].split_once('x')?;
            Some((y.parse().ok()?, h.parse().ok()?))
        })
    })
}

/// The window's size changed, laid out, as a hand dragging its edge.
fn resize(app: &mut TestApp, w: f32, h: f32) {
    app.ui.scene.borrow_mut().resize(w, h);
    app.frame();
}

/// The main window on a long session whose replies rewrap at any width,
/// scrolled half way up, its place kept.
fn mid_history(log: &Log) -> TestApp {
    let mut app = mount("main", "signin", log);
    app.frames(30);
    app.host.shared("sessions", Value::Nil).set(Value::from_json(SESSIONS).unwrap());
    app.host
        .shared("chat.session", Value::Nil)
        .set(Value::from_json(r#"{"id":"c1","state":"ready","title":"Fix the build","running":false,"queue":[],"todos":[],"tasks":[]}"#).unwrap());
    app.host.shared("chat.messages", Value::Nil).set(Value::from_json(&wrapping_transcript(30)).unwrap());
    app.frames(60);
    app.click("#shell-sidebar >> \"Fix the build\"");
    app.frames(60);
    for _ in 0..10 {
        app.scroll("#transcript", -900.);
        app.frames(4);
    }
    app.frames(60);
    std::thread::sleep(std::time::Duration::from_millis(400));
    app.frames(60);
    app
}

/// The wheel over the transcript scrolls it, with the composer there: the
/// composer's layer covers the whole page pane, and must let the pointer and
/// the wheel through to the conversation under it (Nov 1: a box round it
/// took them, and nothing moved under a person's wheel or trackpad).
#[test]
fn the_wheel_over_the_transcript_scrolls_it_with_the_composer_there() {
    let log: Log = Rc::default();
    let mut app = mount("main", "signin", &log);
    app.frames(30);
    app.host.shared("sessions", Value::Nil).set(Value::from_json(SESSIONS).unwrap());
    app.host
        .shared("chat.session", Value::Nil)
        .set(Value::from_json(r#"{"id":"c1","state":"ready","title":"Fix the build","running":false,"queue":[],"todos":[],"tasks":[]}"#).unwrap());
    app.host.shared("chat.messages", Value::Nil).set(Value::from_json(&wrapping_transcript(30)).unwrap());
    app.frames(60);
    app.click("#shell-sidebar >> \"Fix the build\"");
    app.frames(60);
    assert!(app.find("#chat-composer").is_some(), "the composer shows");
    // The last reply's last paragraph, at the end: where it stands, then
    // after a wheel up over the transcript's middle (hit-tested at that
    // point, as a person's wheel is).
    let needle = "\"Reply 30, paragraph 10:";
    let (before, _) = place_of(&mut app, needle).expect("the end shows");
    app.scroll("#transcript", -600.);
    app.frames(10);
    let after = place_of(&mut app, needle).map_or(i32::MAX, |p| p.0);
    assert!(after > before + 300, "the wheel moved nothing: {before} then {after}");
    // And back down.
    app.scroll("#transcript", 600.);
    app.frames(10);
    let back = place_of(&mut app, needle).map_or(i32::MIN, |p| p.0);
    assert!((back - before).abs() < 40, "the wheel down came back: {before}, {after}, {back}");
}

/// A paragraph starting in the view's upper half: the reading place.
fn reading_anchor(app: &mut TestApp) -> String {
    let (list_top, _) = place_of(app, " #transcript ").expect("no transcript");
    for i in 1..=30 {
        for k in 1..=10 {
            let needle = format!("\"Reply {i}, paragraph {k}:");
            if let Some((y, _)) = place_of(app, &needle)
                && y > list_top + 20
                && y < list_top + 400
            {
                return needle;
            }
        }
    }
    panic!("no paragraph in view:\n{}", app.tree())
}

/// Each frame's top of `needle` against where it started: the frames it
/// moved more than a point, if any.
fn drift(trace: &[(String, i32)], from: i32) -> Vec<String> {
    trace.iter().filter(|(_, y)| (y - from).abs() > 1).map(|(f, y)| format!("{f}: {y} (from {from})")).collect()
}

/// As the window's width changes a frame at a time (its edge dragged), the
/// conversation's lines rewrap; what is being read stays where it is: the
/// prompt at the reading place moves by no more than a point on any frame,
/// narrowing and widening. At the end of the conversation, its last line
/// stays on the foot as rows above it rewrap.
#[test]
fn a_resize_keeps_the_reading_place() {
    let log: Log = Rc::default();
    let mut app = mid_history(&log);
    assert_eq!(app.error(), None, "the session failed");
    let needle = reading_anchor(&mut app);
    let (from, _) = place_of(&mut app, &needle).unwrap();
    // 1240 down to 700 and back, 20 a frame: the replies' lines rewrap.
    let widths: Vec<f32> = (0..=27).map(|i| 1240. - 20. * i as f32).chain((0..=27).rev().map(|i| 1240. - 20. * i as f32)).collect();
    let mut trace = Vec::new();
    for w in &widths {
        resize(&mut app, *w, 820.);
        trace.push((format!("{w}"), place_of(&mut app, &needle).map_or(i32::MIN, |p| p.0)));
        if std::env::var_os("ZERON_ZGX_PROBE").is_some() && (*w == 860. || *w == 840.) {
            for l in app.tree().lines().filter(|l| l.contains("#transcript ") || l.contains("#shell-sidebar ") || l.contains("Reply 30, paragraph 1:") || l.contains("#chat-session ")) {
                eprintln!("PROBE {w} {l}");
            }
        }
    }
    if std::env::var_os("ZERON_ZGX_TRACE").is_some() {
        eprintln!("mid-history {needle}: {}", trace.iter().map(|(f, y)| format!("{f}:{y}")).collect::<Vec<_>>().join(" "));
    }
    let moved = drift(&trace, from);
    assert!(moved.is_empty(), "mid-history, the reading place moved as the width changed:\n{}", moved.join("\n"));

    // At the end: its last line on the foot through the same sweep.
    app.scroll("#transcript", 100_000.);
    app.frames(60);
    let last = "\"Reply 30, paragraph 10";
    let foot = |app: &mut TestApp| place_of(app, last).map_or(i32::MIN, |(y, h)| y + h);
    let start = foot(&mut app);
    let mut trace = Vec::new();
    for w in &widths {
        resize(&mut app, *w, 820.);
        trace.push((format!("{w}"), foot(&mut app)));
    }
    if std::env::var_os("ZERON_ZGX_TRACE").is_some() {
        eprintln!("at the end, last line's foot: {}", trace.iter().map(|(f, y)| format!("{f}:{y}")).collect::<Vec<_>>().join(" "));
    }
    let moved = drift(&trace, start);
    assert!(moved.is_empty(), "at the end, the last line left the foot as the width changed:\n{}", moved.join("\n"));
}

/// The right panel opening and closing (its width easing frame by frame)
/// leaves the reading place where it is.
#[test]
fn a_panel_opening_and_closing_keeps_the_reading_place() {
    let log: Log = Rc::default();
    let mut app = mid_history(&log);
    let needle = reading_anchor(&mut app);
    let (from, _) = place_of(&mut app, &needle).unwrap();
    let mut trace = Vec::new();
    for (step, label) in [(0, "open"), (1, "close")] {
        let _ = step;
        app.click("#titlebar >> role:button \"Panel\"");
        for f in 1..=30 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.frame();
            trace.push((format!("{label} {f}"), place_of(&mut app, &needle).map_or(i32::MIN, |p| p.0)));
            if std::env::var_os("ZERON_ZGX_TRACE2").is_some() && f <= 10 {
                let off = app.host.shared("chat.transcript.offset", Value::Nil).get();
                let line = app.tree().lines().find(|l| l.contains(" #transcript ")).unwrap_or_default().trim().to_owned();
                let para = app.tree().lines().find(|l| l.contains(&needle)).unwrap_or_default().trim().to_owned();
                eprintln!("T {label} {f}: offset {off:?} | {line} | {para}");
            }
        }
    }
    if std::env::var_os("ZERON_ZGX_TRACE").is_some() {
        eprintln!("panel {needle}: {}", trace.iter().map(|(f, y)| format!("{f}:{y}")).collect::<Vec<_>>().join(" "));
    }
    let moved = drift(&trace, from);
    assert!(moved.is_empty(), "the reading place moved as the panel opened or closed:\n{}", moved.join("\n"));
}

/// Where the list's last line stands against the view's foot: the gap
/// from the last paragraph's foot to the bottom of what shows of the list
/// (the list's box less the room held under the composer). Constant while
/// the view rests on its end.
fn end_gap(app: &mut TestApp, last: &str) -> Option<i32> {
    let (top, h) = place_of(app, " #transcript ")?;
    let (y, lh) = place_of(app, last)?;
    Some(top + h - (y + lh))
}

/// At the end of the conversation, the window shrunk (narrower, shorter,
/// both, a step at a time and in one jump) keeps the view on its end: after
/// every frame the last line stands as far from the view's foot as it did,
/// and the way back to the latest never shows.
#[test]
fn a_shrinking_window_keeps_the_end_in_view() {
    let log: Log = Rc::default();
    let mut app = mid_history(&log);
    app.scroll("#transcript", 100_000.);
    app.frames(60);
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.frames(30);
    let last = "\"Reply 30, paragraph 10";
    let start = end_gap(&mut app, last).expect("the end isn't in view");
    let mut steps: Vec<(f32, f32)> = Vec::new();
    // Narrower, then shorter, then both, a step a frame; then one jump; then back.
    for i in 1..=10 { steps.push((1240. - 30. * i as f32, 820.)); }
    for i in 1..=10 { steps.push((940., 820. - 25. * i as f32)); }
    for i in 1..=8 { steps.push((940. - 20. * i as f32, 570. - 15. * i as f32)); }
    steps.push((1240., 820.));
    steps.push((760., 480.));
    steps.push((1240., 820.));
    let mut bad = Vec::new();
    for (w, h) in &steps {
        resize(&mut app, *w, *h);
        let gap = end_gap(&mut app, last);
        let away = app.find("role:button \"Jump to latest\"").is_some();
        if gap.is_none_or(|g| (g - start).abs() > 1) || away {
            bad.push(format!("{w}x{h}: gap {gap:?} (was {start}), way back shown {away}"));
        }
    }
    // And once it has all settled.
    app.frames(30);
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.frames(30);
    let gap = end_gap(&mut app, last);
    if gap.is_none_or(|g| (g - start).abs() > 1) || app.find("role:button \"Jump to latest\"").is_some() {
        bad.push(format!("settled: gap {gap:?} (was {start})"));
    }
    assert!(bad.is_empty(), "the view left its end as the window shrank:\n{}", bad.join("\n"));
}

/// The same while a reply is being written at the end: its newest line
/// stays on the view's foot as it grows and the window shrinks, every
/// frame, and the way back to the latest never shows.
#[test]
fn a_shrinking_window_keeps_a_streaming_reply_in_view() {
    let log: Log = Rc::default();
    let mut app = mid_history(&log);
    app.scroll("#transcript", 100_000.);
    app.frames(60);
    // A turn under way: the prompt, and a reply being written.
    let mut list: Vec<String> = Vec::new();
    let base = wrapping_transcript(30);
    list.push(base[1..base.len() - 1].to_owned());
    list.push(r#"{"id":"u31","role":"user","text":"Keep going.","at":"Sep 9, 4:00 PM","copy":"x"}"#.to_owned());
    list.push(r#"{"id":"a31","role":"assistant","text":"","streaming":true,"started":0,"blocks":[{"id":"t","kind":"text","text":""}]}"#.to_owned());
    app.host
        .shared("chat.session", Value::Nil)
        .set(Value::from_json(r#"{"id":"c1","state":"ready","title":"Fix the build","running":true,"queue":[],"todos":[],"tasks":[]}"#).unwrap());
    app.host.shared("chat.messages", Value::Nil).set(Value::from_json(&format!("[{}]", list.join(","))).unwrap());
    app.frames(30);
    app.scroll("#transcript", 100_000.);
    app.frames(30);
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.frames(30);
    let draft = app.host.shared("chat.draft", Value::Nil);
    let mut words = String::new();
    let mut bad = Vec::new();
    let mut start: Option<i32> = None;
    for i in 0..40 {
        words.push_str(&format!("Streamed sentence {i} keeps the reply growing line by line as the window gets smaller. "));
        draft.set(
            Value::from_json(&format!(r#"{{"id":"a31","text":"{w}","part":"t","part_text":"{w}"}}"#, w = words)).unwrap(),
        );
        let (w, h) = if i == 25 { (760., 480.) } else { (1240. - 12. * i as f32, 820. - 8. * i as f32) };
        resize(&mut app, w, h);
        let gap = end_gap(&mut app, "\"Streamed sentence 0");
        let away = app.find("role:button \"Jump to latest\"").is_some()
            || app.find("role:button \"New reply\"").is_some();
        let s = *start.get_or_insert(gap.unwrap_or(i32::MIN));
        // (In i64: the sentinel for no gap yet is i32::MIN.)
        if gap.is_none_or(|g| (i64::from(g) - i64::from(s)).abs() > 1) || away {
            bad.push(format!("{w}x{h}: gap {gap:?} (was {s}), way back shown {away}"));
        }
    }
    assert!(bad.is_empty(), "the reply being written left the view's foot as the window shrank:\n{}", bad.join("\n"));
}

/// Two sessions under one project: a history to walk.
const TWO_SESSIONS: &str = r#"{"state":"ready","here":"mac",
    "devices":[{"id":"mac","name":"This Mac","platform":"macos","here":true}],
    "projects":[{"id":"atlas","name":"Atlas","device":"mac","path":"/work/atlas","git":true}],
    "chats":[{"id":"c1","title":"Fix the build","project":"atlas","device":"mac","updated":2,"created":1,"agent":"claude"},
        {"id":"c2","title":"Write the docs","project":"atlas","device":"mac","updated":1,"created":1,"agent":"claude"}],
    "archived":[],"places":{"pinned":[],"sections":[]},"places_ready":true}"#;

/// The title bar's arrow `name` ("Back", "Forward"): its tree line (its
/// node's id left out, new after a reload, and whether it has focus).
fn arrow(app: &mut TestApp, name: &str) -> String {
    let tree = app.tree();
    let line = tree
        .lines()
        .find(|l| l.contains(&format!("button \"{name}\"")))
        .unwrap_or_else(|| panic!("no {name} button:\n{tree}"))
        .trim();
    let line = line.split_once(' ').filter(|_| line.starts_with('@')).map_or(line, |(_, rest)| rest);
    line.replace(" focused", "")
}

/// The history behind and ahead survives a reload of the scripts: from a
/// session back to the one before it, with one ahead, both arrows work
/// after it as before. (The router was made again from where it stood
/// alone, and Back came back disabled.)
#[test]
fn a_reload_keeps_the_history_behind_and_ahead() {
    let log: Log = Rc::default();
    let mut app = mount("main", "signin", &log);
    app.frames(30);
    app.host.shared("sessions", Value::Nil).set(Value::from_json(TWO_SESSIONS).unwrap());
    app.frames(60);
    app.click("#shell-sidebar >> \"Fix the build\"");
    app.frames(30);
    app.click("#shell-sidebar >> \"Write the docs\"");
    app.frames(30);
    app.click("role:button \"Back\"");
    app.frames(30);
    let (back, forward) = (arrow(&mut app, "Back"), arrow(&mut app, "Forward"));
    assert!(!back.contains("disabled") && !forward.contains("disabled"), "before the reload: {back} / {forward}");
    let _ = app.host.reload();
    app.frames(30);
    assert_eq!(app.error(), None, "the reload failed");
    assert_eq!(arrow(&mut app, "Back"), back, "Back after the reload");
    assert_eq!(arrow(&mut app, "Forward"), forward, "Forward after the reload");
    // Forward to the docs, back twice past the build to home: as it was.
    app.click("role:button \"Forward\"");
    app.frames(30);
    assert!(arrow(&mut app, "Forward").contains("disabled"), "nothing ahead of the docs: {}", arrow(&mut app, "Forward"));
    app.click("role:button \"Back\"");
    app.frames(30);
    app.click("role:button \"Back\"");
    app.frames(30);
    assert!(arrow(&mut app, "Back").contains("disabled"), "home is the first place: {}", arrow(&mut app, "Back"));
}

/// The window's width changed in one step mid-conversation (maximized, a
/// panel's page taking its width at once): the reading place stays where it
/// stood in the view, the rows above it rewrapped and all.
#[test]
fn a_width_changed_in_one_step_keeps_the_reading_place() {
    let log: Log = Rc::default();
    let mut app = mid_history(&log);
    let needle = reading_anchor(&mut app);
    let (from, _) = place_of(&mut app, &needle).unwrap();
    let mut trace = Vec::new();
    for (label, w) in [("narrower", 800.), ("wider", 1400.)] {
        app.ui.scene.borrow_mut().resize(w, 800.);
        for f in 1..=20 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            app.frame();
            trace.push((format!("{label} {f}"), place_of(&mut app, &needle).map_or(i32::MIN, |p| p.0)));
        }
    }
    let moved = drift(&trace, from);
    assert!(moved.is_empty(), "the reading place moved as the width changed in one step:\n{}", moved.join("\n"));
}
