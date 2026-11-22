//! Composer focus and layout transitions on the real shell fixture.
//! TestApp supplies the frame/timer clock; readiness is revision-based.

use zgx::{
    input::{InputEvent, Key},
    scene::{NodeId, Rect},
};
use zgx_script::{ScriptHost, Value, testing::TestApp};

// Characterization, not a reproduction of the reported visual glitch.
// These snapshots observe the scene, not GPU pixels. See
// docs/composer.md for the separate visual procedure.
const FIELD: &str = "#chat-composer >> role:textbox";
const BUILD: &str = "#shell-sidebar >> role:button \"Fix the build\"";
const DOCS: &str = "#shell-sidebar >> role:button \"Write the docs\"";
const NEW: &str = "#shell-sidebar >> role:button \"New chat\"";

struct Shell {
    app: TestApp,
    frame: u32,
}

impl Shell {
    fn new(reduced: bool) -> Self {
        Self::of("shell_fixture.luau", reduced)
    }

    fn of(fixture: &str, reduced: bool) -> Self {
        let ui = std::env::var_os("ZERON_ZGX_TEST_UI")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"));
        let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui.join(fixture)));
        app.ui.scene.borrow_mut().resize(1200., 800.);
        let motion = app
            .host
            .states()
            .into_iter()
            .find(|(name, _)| name.ends_with("::motion_reduce"))
            .expect("the shell's motion preference")
            .0;
        app.host.set_state(&motion, Value::Bool(reduced)).unwrap();
        let mut shell = Self { app, frame: 0 };
        shell.frames(60);
        shell
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame += 1;
            self.app.frame();
            assert_eq!(
                self.app.error(),
                None,
                "script error at frame {}",
                self.frame
            );
        }
    }

    fn geometry(&mut self) -> Geometry {
        let composer = self.app.find("#chat-composer").expect("composer");
        let field = self.app.find(FIELD).expect("draft field");
        let scene = self.app.ui.scene.borrow();
        Geometry {
            composer: scene.transformed_bounds(composer),
            field: scene.transformed_bounds(field),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Geometry {
    composer: Rect,
    field: Rect,
}

#[derive(Debug)]
struct Sample {
    frame: u32,
    editor: NodeId,
    focused: Option<NodeId>,
    selection: (usize, usize),
    text: String,
    presented: Geometry,
    target: Geometry,
    opacity: f32,
    visible: bool,
}

struct Composer {
    shown: Shell,
    target: Shell,
    editor: NodeId,
    trace: Vec<Sample>,
    print_trace: bool,
}

impl Composer {
    fn new(session: bool) -> Self {
        let mut shown = Shell::new(false);
        let mut target = Shell::new(true);
        if session {
            shown.app.click(BUILD);
            target.app.click(BUILD);
            shown.frames(60);
            target.frames(60);
        }
        let field = shown.app.find(FIELD).unwrap();
        let editor = shown.app.ui.editor_within(field).unwrap().node;
        Self {
            shown,
            target,
            editor,
            trace: Vec::new(),
            print_trace: std::env::var_os("ZERON_ZGX_COMPOSER_TRACE").is_some_and(|v| v != "0"),
        }
    }

    /// Same input in both shells. The reduced-motion shell supplies the
    /// destination through real layout rather than duplicating its formulas.
    fn act(&mut self, action: impl Fn(&mut TestApp)) {
        action(&mut self.shown.app);
        action(&mut self.target.app);
        self.target.frames(2);
    }

    fn watch(&mut self, n: usize) {
        for _ in 0..n {
            self.shown.frames(1);
            let presented = self.shown.geometry();
            let target = self.target.geometry();
            let field = self.shown.app.find(FIELD).unwrap();
            let editor = self.shown.app.ui.editor_within(field).unwrap();
            let scene = self.shown.app.ui.scene.borrow();
            let opacity = std::iter::once(editor.node)
                .chain(scene.ancestors(editor.node))
                .map(|id| scene.effects(id).opacity)
                .product();
            let sample = Sample {
                frame: self.shown.frame,
                editor: editor.node,
                focused: self.shown.app.ui.input.focused(),
                selection: editor.selection(),
                text: editor.editor.borrow().text().into_owned(),
                presented,
                target,
                opacity,
                visible: scene.visible_bounds(editor.node).is_some(),
            };
            drop(scene);
            self.trace.push(sample);
            let s = self.trace.last().unwrap();
            if self.print_trace {
                eprintln!(
                    "composer-trace {} {s:?}",
                    std::thread::current().name().unwrap_or("unnamed")
                );
            }
            assert_eq!(s.editor, self.editor, "editor replaced: {:?}", self.trace);
            assert!(
                s.visible && s.opacity > 0.99,
                "editor hidden: {:?}",
                self.trace
            );
            for r in [
                s.presented.composer,
                s.presented.field,
                s.target.composer,
                s.target.field,
            ] {
                assert!(
                    r.x.is_finite()
                        && r.y.is_finite()
                        && r.width.is_finite()
                        && r.height.is_finite()
                        && r.width > 0.
                        && r.height > 0.,
                    "invalid geometry: {:?}",
                    self.trace
                );
            }
        }
    }

    fn last(&self) -> &Sample {
        self.trace.last().expect("observed frames")
    }

    fn draft(&self, text: &str, selection: (usize, usize)) {
        for s in &self.trace {
            assert_eq!(
                s.text, text,
                "draft changed at frame {}: {:?}",
                s.frame, self.trace
            );
            assert_eq!(
                s.selection, selection,
                "selection changed: {:?}",
                self.trace
            );
            assert_eq!(s.focused, Some(self.editor), "focus lost: {:?}", self.trace);
        }
    }

    fn at_target(&self) {
        let s = self.last();
        for (a, b) in [
            (s.presented.composer, s.target.composer),
            (s.presented.field, s.target.field),
        ] {
            assert!(
                (a.x - b.x).abs() < 1.
                    && (a.y - b.y).abs() < 1.
                    && (a.width - b.width).abs() < 1.
                    && (a.height - b.height).abs() < 1.,
                "did not reach reduced-motion target: {:?}",
                self.trace
            );
        }
    }
}

fn select(app: &mut TestApp, anchor: usize, focus: usize) {
    let field = app.find(FIELD).unwrap();
    app.ui
        .editor_within(field)
        .unwrap()
        .set_selection(anchor, focus);
}

fn type_at_caret(app: &mut TestApp, text: &str) {
    // TestApp::type_text clicks again; dispatch directly so typing really
    // preserves the current caret/selection, including during a transition.
    app.ui.dispatch(InputEvent::Text(text.into()));
    app.settle();
}

#[test]
fn composer_rest_size_is_independent_of_animated_draft_size_and_picker_placement() {
    let mut c = Composer::new(false);
    let sizes = |app: &TestApp| {
        app.host
            .states()
            .into_iter()
            .find(|(n, _)| n.ends_with("::chat_composer_sizes"))
            .unwrap()
            .1
    };
    let rest = sizes(&c.shown.app).field("rest").unwrap().as_f64().unwrap();
    c.act(|app| app.fill(FIELD, "one\ntwo\nthree\nfour\nfive\nsix"));
    c.watch(3);
    let moving = sizes(&c.shown.app);
    assert_eq!(moving.field("rest").unwrap().as_f64(), Some(rest));
    assert!(moving.field("target").unwrap().as_f64().unwrap() > rest);
    assert!(
        moving.field("presented").unwrap().as_f64().unwrap()
            < moving.field("target").unwrap().as_f64().unwrap()
    );
    c.watch(40);
    c.act(|app| app.fill(FIELD, ""));
    c.watch(1);
    assert_eq!(
        sizes(&c.shown.app).field("rest").unwrap().as_f64(),
        Some(rest),
        "clearing needs no settle timer"
    );
    c.watch(40);
    c.act(|app| app.click("#chat-composer >> role:button \"Model\""));
    c.watch(2);
    let button = c
        .shown
        .app
        .find("#chat-composer >> role:button \"Model\"")
        .unwrap();
    let y = c.shown.app.ui.scene.borrow().transformed_bounds(button).y;
    c.act(|app| {
        app.host
            .set_state(
                "chat.composer.text",
                Value::from("one\ntwo\nthree\nfour\nfive\nsix"),
            )
            .unwrap()
    });
    c.watch(40);
    let held = c.shown.app.ui.scene.borrow().transformed_bounds(button).y;
    assert!(
        (held - y).abs() < 1.,
        "open picker retains its anchor: {y} -> {held}"
    );
}

#[test]
fn composer_transition_near_the_height_cap_preserves_scroll_editing_and_the_caret() {
    for session in [false, true] {
        let mut c = Composer::new(session);
        c.act(|app| app.click(FIELD));
        let first = (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        c.act(|app| type_at_caret(app, &first));
        c.watch(3);
        let more = (10..80).map(|i| format!("\nline {i}")).collect::<String>();
        c.act(|app| type_at_caret(app, &more));
        c.trace.clear();
        c.watch(40);
        let text = format!("{first}{more}");
        c.draft(&text, (text.len(), text.len()));
        c.at_target();
        let cap = c.last().presented;
        let field = c.shown.app.find(FIELD).unwrap();
        let editor = c.shown.app.ui.editor_within(field).unwrap();
        let caret_before = c
            .shown
            .app
            .ui
            .scene
            .borrow()
            .transformed_bounds(editor.caret);
        c.act(|app| app.scroll(FIELD, -120.));
        c.watch(3);
        let caret_scrolled = c
            .shown
            .app
            .ui
            .scene
            .borrow()
            .transformed_bounds(editor.caret);
        assert!(
            (caret_before.y - caret_scrolled.y).abs() > 20.,
            "the test must actually scroll the editor"
        );
        c.act(|app| type_at_caret(app, " typed after scrolling"));
        c.trace.clear();
        c.watch(20);
        let text = format!("{text} typed after scrolling");
        c.draft(&text, (text.len(), text.len()));
        for s in &c.trace {
            assert_eq!(
                s.presented.composer.height, cap.composer.height,
                "typing above the cap must not resize the composer"
            );
            assert_eq!(s.presented.field, cap.field);
        }
        let caret = c
            .shown
            .app
            .ui
            .scene
            .borrow()
            .transformed_bounds(editor.caret);
        assert!(
            caret.y >= cap.field.y - 1.
                && caret.y + caret.height <= cap.field.y + cap.field.height + 1.,
            "typing must reveal the caret inside the viewport: {caret:?}, {cap:?}"
        );
    }
}

#[test]
fn composer_click_and_focus_preserve_geometry_and_the_selected_draft() {
    for session in [false, true] {
        let mut c = Composer::new(session);
        c.watch(1);
        let empty = c.last().presented;
        c.act(|app| app.click(FIELD));
        c.trace.clear();
        c.watch(6);
        c.draft("", (0, 0));
        for s in &c.trace {
            assert_eq!(
                s.presented.composer, empty.composer,
                "empty click resized composer: {s:?}"
            );
            assert_eq!(
                s.presented.field, empty.field,
                "empty click moved field: {s:?}"
            );
        }
        c.trace.clear();
        c.act(|app| app.fill(FIELD, "hello composer"));
        c.watch(40);
        c.at_target();
        let before = c.last().presented;
        c.trace.clear();
        c.act(|app| {
            app.ui.input.focus(&app.ui.scene, None);
        });
        c.watch(2);
        assert_eq!(c.last().focused, None);
        c.act(|app| app.click(FIELD));
        c.trace.clear();
        c.watch(2);
        let clicked = c.last().selection;
        c.draft("hello composer", clicked);
        for s in &c.trace {
            assert_eq!(s.presented.field, before.field, "click moved field: {s:?}");
        }
        c.act(|app| select(app, 1, 8));
        c.trace.clear();
        c.watch(40);
        c.draft("hello composer", (1, 8));
        for s in &c.trace {
            assert_eq!(
                s.presented.composer, before.composer,
                "focus resized composer: {s:?}"
            );
            assert_eq!(s.presented.field, before.field, "focus moved field: {s:?}");
        }
        c.at_target();
    }
}

#[test]
fn composer_character_typing_crosses_the_wrap_boundary_and_folds_back() {
    let mut c = Composer::new(true);
    c.act(|app| app.click(FIELD));
    c.watch(1);
    let compact = c.last().target.composer.height;
    let compact_width = c.last().presented.field.width;
    let mut text = String::new();
    // Find the boundary with the actual font, without copying width rules.
    while c.last().target.composer.height <= compact + 1. {
        assert!(text.len() < 256, "never expanded: {:?}", c.trace);
        text.push('W');
        c.act(|app| type_at_caret(app, "W"));
        c.trace.clear();
        c.watch(1);
        c.draft(&text, (text.len(), text.len()));
    }
    c.watch(30);
    c.at_target();
    let expanded_width = c.last().target.field.width;
    let mut width = compact_width;
    let mut changes = 0;
    for s in &c.trace {
        let next = s.presented.field.width;
        assert!(
            (next - compact_width).abs() < 1. || (next - expanded_width).abs() < 1.,
            "mode change must not rewrap through intermediate widths: {s:?}"
        );
        if (next - width).abs() >= 1. {
            changes += 1;
        }
        width = next;
    }
    assert_eq!(
        changes, 1,
        "the real editor changes width once at expansion"
    );
    assert!(c.last().presented.composer.height > compact + 20.);
    while !text.is_empty() {
        c.act(|app| app.key(Key::Backspace));
        text.pop();
        c.trace.clear();
        c.watch(1);
        c.draft(&text, (text.len(), text.len()));
    }
    c.watch(40);
    c.at_target();
    assert!((c.last().presented.composer.height - compact).abs() < 1.);
}

#[test]
fn composer_growth_shrink_and_typing_during_a_transition_keep_the_editor() {
    for session in [false, true] {
        let mut c = Composer::new(session);
        c.act(|app| app.click(FIELD));
        c.watch(1);
        let short = c.last().presented.composer.height;
        let first = "one\ntwo\nthree\nfour\nfive\nsix";
        c.act(|app| type_at_caret(app, first));
        c.trace.clear();
        c.watch(3);
        c.draft(first, (first.len(), first.len()));
        let mid = c.last();
        assert!(
            mid.presented.composer.height > short + 1.
                && mid.presented.composer.height < mid.target.composer.height - 1.,
            "must interrupt an actual growth transition: {:?}",
            c.trace
        );
        c.act(|app| type_at_caret(app, "\nseven"));
        c.trace.clear();
        c.watch(40);
        let text = format!("{first}\nseven");
        c.draft(&text, (text.len(), text.len()));
        c.at_target();
        let tall = c.last().presented.composer.height;
        c.act(|app| {
            select(app, 0, text.len());
            type_at_caret(app, "short");
        });
        c.trace.clear();
        c.watch(3);
        assert!(
            c.last().presented.composer.height < tall - 1.
                && c.last().presented.composer.height > c.last().target.composer.height + 1.,
            "shrink should ease: {:?}",
            c.trace
        );
        c.watch(40);
        c.draft("short", (5, 5));
        c.at_target();
    }
}

#[test]
fn a_new_chat_and_a_session_switch_the_composer_at_once() {
    let mut c = Composer::new(true);
    c.watch(1);
    let docked = c.last().presented.composer.y;
    for selector in [NEW, BUILD] {
        c.act(|app| app.click(selector));
        c.trace.clear();
        c.watch(30);
        for s in &c.trace {
            assert!(
                (s.presented.composer.y - s.target.composer.y).abs() < 1.
                    && (s.presented.composer.height - s.target.composer.height).abs() < 1.,
                "route switch did not snap: {:?}",
                c.trace
            );
        }
        c.at_target();
        if selector == NEW {
            assert!(c.last().presented.composer.y < docked - 50.);
        } else {
            assert!((c.last().presented.composer.y - docked).abs() < 1.);
        }
    }
    c.act(|app| app.fill(FIELD, "build draft"));
    c.watch(40);
    c.act(|app| app.click(DOCS));
    c.trace.clear();
    c.watch(30);
    assert_eq!(c.last().text, "");
    c.act(|app| app.fill(FIELD, "docs draft"));
    c.watch(40);
    c.act(|app| app.click(BUILD));
    c.trace.clear();
    c.watch(30);
    assert_eq!(c.last().text, "build draft");
    c.act(|app| {
        app.click(FIELD);
        select(app, 0, 5);
    });
    c.trace.clear();
    c.watch(30);
    c.draft("build draft", (0, 5));
    c.at_target();
}

/// A send from a new chat: the composer slides down to the dock with its
/// prompt and eases into its docked form on the way (no snap: its size
/// passes through between), as the page's one slide; with motion reduced
/// it's there at once. Any other move to or from a new chat snaps (above).
#[test]
fn a_send_from_a_new_chat_slides_the_composer_down_and_eases_it_docked() {
    let snaps = |shell: &Shell| {
        shell
            .app
            .host
            .states()
            .into_iter()
            .find(|(n, _)| n.ends_with("::chat_composer_snap"))
            .and_then(|(_, v)| v.as_f64())
            .expect("the composer's snaps")
    };
    let mut shells = [false, true].map(|reduced| {
        let mut shell = Shell::of("chat_fixture.luau", reduced);
        assert!(shell.app.host.run_command("go_new"));
        shell.frames(30);
        shell.app.fill(FIELD, "please fix the build");
        shell.frames(40);
        shell
    });
    let start = shells[0].geometry().composer;
    let snapped = snaps(&shells[0]);
    for shell in &mut shells {
        shell.app.key(Key::Enter);
    }
    shells[1].frames(2);
    let end = shells[1].geometry().composer;
    assert!(
        end.y > start.y + 100. && end.height < start.height - 20.,
        "the session docks it lower, one line: {start:?} -> {end:?}"
    );
    let mut trace = Vec::new();
    for _ in 0..30 {
        shells[0].frames(1);
        trace.push(shells[0].geometry().composer);
    }
    let between = trace
        .iter()
        .filter(|r| {
            r.y > start.y + 1.
                && r.y < end.y - 1.
                && r.height < start.height - 1.
                && r.height > end.height + 1.
        })
        .count();
    assert!(
        between >= 5,
        "its place and its size ease together, neither snapped: {start:?} -> {end:?}, {trace:?}"
    );
    for pair in trace.windows(2) {
        assert!(
            pair[1].y >= pair[0].y - 0.01 && pair[1].height <= pair[0].height + 0.01,
            "one way, down and smaller: {trace:?}"
        );
    }
    let last = trace.last().unwrap();
    assert!(
        (last.y - end.y).abs() < 1. && (last.height - end.height).abs() < 1.,
        "lands where the reduced-motion shell put it: {end:?}, {trace:?}"
    );
    assert_eq!(snaps(&shells[0]), snapped, "a send snaps nothing");
}

#[test]
fn composer_resize_theme_transparency_and_zoom_reflow_preserve_the_draft() {
    for session in [false, true] {
        let mut c = Composer::new(session);
        let text = "**bold** café 你好 @mention and words to wrap\n".repeat(8);
        c.act(|app| {
            app.fill(FIELD, &text);
            select(app, 2, 6);
        });
        c.watch(40);
        for (width, height, zoom, theme, frosted) in [
            (1200., 800., 1., "zeron-dark", true),
            (950., 800., 1.25, "zeron-light", false),
            (1400., 900., 0.9, "zeron-dark", true),
        ] {
            c.trace.clear();
            c.act(|app| {
                app.ui.theme().set(theme).unwrap();
                app.ui.set_zoom(zoom);
                // The native host reflows at window points / zoom. Model
                // that logical layout here; this does not validate GPU scale.
                app.ui
                    .scene
                    .borrow_mut()
                    .resize(width / zoom as f32, height / zoom as f32);
                app.ui.refresh_text_geometry();
                let key = app
                    .host
                    .states()
                    .into_iter()
                    .find(|(key, _)| key.ends_with("::frosted"))
                    .unwrap()
                    .0;
                app.host.set_state(&key, Value::Bool(frosted)).unwrap();
            });
            c.target.frames(30);
            c.watch(40);
            c.draft(&text, (2, 6));
            c.at_target();
        }
    }
}

/// Typing in a long draft costs what typing in a short one past the cap
/// does: the composer's natural size can't change there (open, capped), so
/// the draft isn't measured again. Measuring walked every line twice at each
/// keystroke, past what the editor's span cache holds (a 5,000-line draft:
/// 10,039 script calls and 47 ms a key; 30,000 lines: 294 ms).
#[test]
fn typing_in_a_long_draft_costs_what_a_short_one_does() {
    let per_key = |lines: usize| {
        let mut shell = Shell::new(false);
        shell.app.click(BUILD);
        shell.frames(60);
        let text: String = (0..lines)
            .map(|i| format!("line {i} with some words that go on for a while here\n"))
            .collect();
        shell.app.fill(FIELD, &text);
        shell.frames(60);
        let capped = shell.geometry().composer.height;
        let before = shell.app.calls();
        for _ in 0..20 {
            shell.app.ui.dispatch(InputEvent::Text("x".into()));
            shell.app.settle();
            shell.frames(3);
        }
        let calls = (shell.app.calls() - before) / 20;
        let field = shell.app.find(FIELD).unwrap();
        let editor = shell.app.ui.editor_within(field).unwrap();
        assert!(
            editor
                .editor
                .borrow()
                .text()
                .ends_with("xxxxxxxxxxxxxxxxxxxx")
        );
        assert!(
            (shell.geometry().composer.height - capped).abs() < 0.5,
            "typing past the cap must not resize the composer"
        );
        calls
    };
    let (short, long) = (per_key(20), per_key(5000));
    assert!(
        long <= short && long <= 20,
        "{long} script calls a key in 5,000 lines, {short} in 20"
    );
}

/// What comes and goes with the panel stands still and only fades a little
/// (ds/motion `content`), never popping: the panel's tabs as it opens and
/// closes, the page as the panel expands over it and gives it back. Over
/// each, it stays where it's laid out (no drift: a hundredth of a point at
/// most), its opacity passes through between, a step at a time, never
/// under the fade's floor (0.3) as it shows, and ends where it goes (the
/// page under the panel expanded, hidden; the panel's content, closed, at
/// the floor in a panel that shows none of it); settled, the layout warns
/// of nothing new. (ui/tests/shell_route_motion.test.luau has their widths.)
#[test]
fn the_panel_s_content_and_the_page_only_fade_as_they_come_and_go() {
    const FLOOR: f32 = 0.3;
    let mut app = Shell::new(false).app;
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    let warnings = |app: &mut TestApp| -> Vec<String> {
        app.ui.prepare_frame();
        zgx::inspect::layout_warnings(&app.ui)
            .iter()
            .map(|w| w.to_string())
            .collect()
    };
    // (Where its right edge is painted: it stands at its column's side by
    // the window's edge, the panel's content at the window's right, the
    // page at its left.)
    let edge = |app: &mut TestApp, id: &str| -> f32 {
        let node = app.find(&format!("#{id}")).expect("it");
        app.ui.prepare_frame();
        let b = app.ui.scene.borrow().transformed_bounds(node);
        if id == "shell-page-room" {
            b.x
        } else {
            b.x + b.width
        }
    };
    // (What the fixture has before the panel ever opens.)
    let before = warnings(&mut app);
    let mut fade = |click: &str, id: &str, to: f32, what: &str| {
        app.click(click);
        let seen: Vec<(f32, f32, bool)> = (0..30)
            .map(|_| {
                app.frames(1);
                // (Whether its column shows: the panel under 2 points shows none.)
                let shows = id == "shell-page-room" || app.opacity("shell-files") > 0.;
                (app.opacity(id), edge(&mut app, id), shows)
            })
            .collect();
        let (lo, hi) = seen
            .iter()
            .filter(|s| s.2)
            .fold((f32::MAX, f32::MIN), |(lo, hi), s| {
                (lo.min(s.1), hi.max(s.1))
            });
        let drift = hi - lo;
        assert!(
            drift <= 0.011,
            "{what}: {id} moved {drift} points: {seen:?}"
        );
        let between = seen.iter().filter(|s| s.0 > 0.02 && s.0 < 0.98).count();
        let jump = seen
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).abs())
            .fold(0., f32::max);
        assert!(between >= 2 && jump <= 0.7, "{what}: {id} at {seen:?}");
        let showing = seen
            .iter()
            .filter(|s| (s.0 - to).abs() > 0.01)
            .map(|s| s.0)
            .fold(1., f32::min);
        assert!(
            showing >= FLOOR - 0.01,
            "{what}: {id} fades only to the floor as it shows: {seen:?}"
        );
        assert!(
            (seen.last().unwrap().0 - to).abs() < 0.01,
            "{what}: {id} ends at {seen:?}"
        );
        // At rest, nothing it holds is laid out in a box that shows none of
        // it (`zgx inspect errors` names nothing new).
        app.frames(30);
        assert_eq!(
            warnings(&mut app),
            before,
            "{what}: settled, the layout's warnings"
        );
    };
    fade(
        "#titlebar >> role:button \"Panel\"",
        "shell-files-content",
        1.,
        "opening",
    );
    fade("#panel-expand", "shell-page-room", 0., "expanding");
    fade("#panel-expand", "shell-page-room", 1., "collapsing");
    fade(
        "#titlebar >> role:button \"Panel\"",
        "shell-files-content",
        FLOOR,
        "closing",
    );
    assert!(
        app.opacity("shell-files") < 0.01,
        "closed, the panel shows none of it"
    );
}

/// The sidebar folds and comes back as the panel opens and closes: its
/// card's left edge and what's in it stand still as its right edge sweeps
/// over them (no slide: the window's gap before it goes only with its last
/// 5 points), what's in it only fades a little, from 0.3 (ds/motion
/// `content`), and the page beside it keeps the gap between cards (4) as
/// it follows the edge. (It used to slide 5 points with the gap closing on
/// its own clock, the page's gap easing from 4 to 5 beside it.)
#[test]
fn the_sidebar_s_content_stands_still_and_only_fades_as_it_folds() {
    const FLOOR: f32 = 0.3;
    let mut app = Shell::new(false).app;
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    let at = |app: &mut TestApp, sel: &str| -> Option<(f32, f32)> {
        let node = app.find(sel)?;
        app.ui.prepare_frame();
        let b = app.ui.scene.borrow().transformed_bounds(node);
        Some((b.x, b.width))
    };
    for what in ["folding", "coming back"] {
        assert!(app.host.run_command("shell.toggle_sidebar"));
        let mut rows = Vec::new();
        for _ in 0..24 {
            app.frames(1);
            let side = at(&mut app, "#shell-sidebar").expect("its column");
            let page = at(&mut app, "#shell-page-room").expect("the page");
            let row = at(&mut app, "#shell-sidebar >> role:button \"Fix the build\"");
            let fade = if app.find("#sidebar-content").is_some() {
                app.opacity("sidebar-content")
            } else {
                1.
            };
            // (Its card more than the window's gap wide: the gap is whole.)
            if side.1 > 10. {
                rows.push(row.expect("its row").0);
                assert!(
                    (page.0 - (side.0 + side.1) - 4.).abs() < 0.05,
                    "{what}: the gap beside it, {}",
                    page.0 - side.0 - side.1
                );
                assert!(
                    fade >= FLOOR - 0.01,
                    "{what}: it fades only to the floor as it shows: {fade}"
                );
            }
        }
        let (lo, hi) = rows
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        assert!(
            rows.len() >= 4 && hi - lo <= 0.011,
            "{what}: its row stands at {lo}..{hi} over {} frames",
            rows.len()
        );
        let end = if what == "folding" { FLOOR } else { 1. };
        assert!(
            (app.opacity("sidebar-content") - end).abs() < 0.01,
            "{what}: it ends faded to {end}"
        );
        app.frames(30);
    }
}

/// Inspect the composer pill, text viewport clip, scroll offset, topmost
/// rendered text line, and caret bounds/visibility on the current frame.
#[derive(Clone, Copy, Debug)]
struct TextFrame {
    composer: Rect,
    field: Rect,
    scroll_y: f32,
    line_top_full: f32,
    line_top_visible: Option<f32>,
    line_height_full: f32,
    line_height_visible: f32,
    caret_full: Rect,
    caret_visible: Option<Rect>,
    caret_painted_over: bool,
}

fn inspect_composer_text(app: &mut TestApp) -> TextFrame {
    let field = app
        .find(FIELD)
        .or_else(|| app.find("role:textbox"))
        .expect("draft field");
    let composer = app.find("#chat-composer").unwrap_or(field);
    let editor = app.ui.editor_within(field).expect("editor handle");
    let scroll_y = editor.scroll_offset().1;
    let scene = app.ui.scene.borrow();
    let composer_rect = scene.transformed_bounds(composer);
    let field_rect = scene.transformed_bounds(field);
    let caret_full = scene.transformed_bounds(editor.caret);
    let caret_visible = scene.visible_bounds(editor.caret);
    let caret_painted_over = scene
        .plain_box(editor.caret)
        .is_some_and(|r| scene.painted_over(editor.caret, r));
    // Find the topmost non-empty painted text node inside the editor.
    let mut stack = vec![editor.node];
    let mut top_line: Option<(Rect, Option<Rect>)> = None;
    while let Some(id) = stack.pop() {
        let has_text = match scene.kind(id) {
            zgx::scene::NodeKind::Text { text, .. } => !text.is_empty(),
            zgx::scene::NodeKind::RichText { text } => !text.text().is_empty(),
            _ => false,
        };
        if has_text {
            let full = scene.transformed_bounds(id);
            let vis = scene.visible_bounds(id);
            if top_line.is_none_or(|(best, _)| full.y < best.y) {
                top_line = Some((full, vis));
            }
        }
        stack.extend_from_slice(scene.children(id));
    }
    let (full, vis) = top_line.expect("non-empty rendered text line inside composer");
    TextFrame {
        composer: composer_rect,
        field: field_rect,
        scroll_y,
        line_top_full: full.y,
        line_top_visible: vis.map(|r| r.y),
        line_height_full: full.height,
        line_height_visible: vis.map_or(0., |r| r.height),
        caret_full,
        caret_visible,
        caret_painted_over,
    }
}

fn assert_caret_unclipped(frame: TextFrame, context: &str) {
    let vis = frame
        .caret_visible
        .unwrap_or_else(|| panic!("{context}: caret completely clipped out: {frame:?}"));
    assert!(
        (vis.y - frame.caret_full.y).abs() <= 0.5,
        "{context}: caret top clipped by {}px: {frame:?}",
        vis.y - frame.caret_full.y
    );
    assert!(
        vis.height >= frame.caret_full.height - 0.5,
        "{context}: caret bottom clipped (visible {} of {}): {frame:?}",
        vis.height,
        frame.caret_full.height
    );
    assert!(
        !frame.caret_painted_over,
        "{context}: caret painted over by another control: {frame:?}"
    );
}

fn assert_no_top_clip(frame: TextFrame, context: &str) {
    assert!(
        frame.scroll_y <= 0.05,
        "{context}: top scrolled off by {}px during resize: {frame:?}",
        frame.scroll_y
    );
    let vis_y = frame
        .line_top_visible
        .unwrap_or_else(|| panic!("{context}: top line completely clipped out: {frame:?}"));
    assert!(
        (vis_y - frame.line_top_full).abs() <= 0.5,
        "{context}: top of first line clipped by {}px (full.y={}, visible.y={}): {frame:?}",
        vis_y - frame.line_top_full,
        frame.line_top_full,
        vis_y
    );
    // A wrapped paragraph is a single logical text node whose height is N * 22px;
    // the full first visual line (at least min(line_height_full, 22px)) must stay
    // completely unclipped at the top of the viewport on every frame.
    let first_visual_line = frame.line_height_full.min(22.0);
    assert!(
        frame.line_height_visible >= first_visual_line - 0.5,
        "{context}: first visual line clipped vertically (visible {} of {}): {frame:?}",
        frame.line_height_visible,
        first_visual_line
    );
    assert!(
        frame.field.height >= first_visual_line - 0.5,
        "{context}: editor height collapsed below a line: {frame:?}"
    );
}

#[test]
fn composer_resize_never_clips_top_of_text_and_keeps_text_position_stable() {
    // 1. Expanded growth & shrink in both New-Chat (top-anchored) and Docked (bottom-anchored).
    for session in [false, true] {
        let mode = if session { "docked" } else { "new-chat" };
        let mut shell = Shell::new(false);
        if session {
            shell.app.click(BUILD);
            shell.frames(60);
        }
        shell.app.click(FIELD);
        shell.frames(2);
        // Start with 3 lines (settled expanded).
        type_at_caret(&mut shell.app, "Line 1\nLine 2\nLine 3");
        shell.frames(40);
        let base = inspect_composer_text(&mut shell.app);
        let base_rel = base.line_top_full - base.composer.y;
        assert_no_top_clip(base, &format!("{mode} base 3 lines"));
        let base_warnings = zgx::inspect::layout_warnings(&shell.app.ui).len();

        // Grow line by line (3 -> 4 -> 5 -> 6) via Shift+Enter / newline typing.
        for n in 4..=6 {
            type_at_caret(&mut shell.app, &format!("\nLine {n}"));
            // Check IMMEDIATELY on frame 0 (before animation advances) and every frame through settle.
            let f0 = inspect_composer_text(&mut shell.app);
            assert_no_top_clip(f0, &format!("{mode} grow to {n} lines frame 0"));
            assert!(
                ((f0.line_top_full - f0.composer.y) - base_rel).abs() <= 0.5,
                "{mode} grow to {n} frame 0: text shifted relative to composer top ({} vs {}): {f0:?}",
                f0.line_top_full - f0.composer.y,
                base_rel
            );
            for step in 1..=35 {
                shell.frames(1);
                let f = inspect_composer_text(&mut shell.app);
                assert_no_top_clip(f, &format!("{mode} grow to {n} lines frame {step}"));
                let rel = f.line_top_full - f.composer.y;
                assert!(
                    (rel - base_rel).abs() <= 0.5,
                    "{mode} grow to {n} frame {step}: text shifted relative to composer top ({rel} vs {base_rel}): {f:?}"
                );
                if !session {
                    assert!(
                        (f.line_top_full - base.line_top_full).abs() <= 0.5,
                        "new-chat grow to {n} frame {step}: text jumped vertically on screen ({} vs {}): {f:?}",
                        f.line_top_full,
                        base.line_top_full
                    );
                }
                if step == 3 || step == 35 {
                    assert!(
                        zgx::inspect::layout_warnings(&shell.app.ui).len() <= base_warnings,
                        "{mode} grow to {n} frame {step}: decoupled textarea introduced layout warnings"
                    );
                }
            }
            // Settled, the textarea's own height matches its settled text height (n * 22).
            let settled = inspect_composer_text(&mut shell.app);
            assert!(
                (settled.field.height - (n as f32) * 22.0).abs() <= 0.5,
                "{mode} grow to {n} settled: expected field.height={}, got {:?}",
                (n as f32) * 22.0,
                settled.field
            );
        }

        // Shrink from 6 lines back to 2 lines: text stays stable relative to composer top.
        let field = shell.app.find(FIELD).unwrap();
        let editor = shell.app.ui.editor_within(field).unwrap();
        let len = editor.editor.borrow().text().len();
        editor.set_selection(7, len); // keep "Line 1\n" -> replace tail with "Line 2"
        type_at_caret(&mut shell.app, "Line 2");
        for step in 0..=35 {
            if step > 0 {
                shell.frames(1);
            }
            let f = inspect_composer_text(&mut shell.app);
            assert_no_top_clip(f, &format!("{mode} shrink frame {step}"));
            let rel = f.line_top_full - f.composer.y;
            assert!(
                (rel - base_rel).abs() <= 0.5,
                "{mode} shrink frame {step}: text shifted relative to composer top ({rel} vs {base_rel}): {f:?}"
            );
        }
        // Settled back at 2 lines (below min 56px): editor height settles to 56px.
        let settled_shrink = inspect_composer_text(&mut shell.app);
        assert!(
            (settled_shrink.field.height - 56.0).abs() <= 0.5,
            "{mode} shrink settled: expected min viewport 56, got {:?}",
            settled_shrink.field
        );
    }
}

#[test]
fn composer_compact_to_expanded_transitions_never_clip_top_of_text() {
    // Test: (a) Shift+Enter from compact, (b) typing across compact wrap boundary,
    // (c) pasting 6 lines into compact, (d) pasting a long unwrapped paragraph that wraps
    // to MORE lines at compact width than at expanded width.
    let paragraph = "word ".repeat(55); // ~4 lines expanded, ~5 lines compact
    let cases: [(&str, &str); 4] = [
        ("newline", "First line\nSecond line"),
        ("wrap-boundary", &"W".repeat(65)),
        ("paste-6-lines", "L1\nL2\nL3\nL4\nL5\nL6"),
        ("paste-long-paragraph", paragraph.trim_end()),
    ];
    for (name, payload) in cases {
        let mut shell = Shell::new(false);
        shell.app.click(BUILD);
        shell.frames(60);
        shell.app.click(FIELD);
        shell.frames(2);
        type_at_caret(&mut shell.app, "a");
        shell.frames(30);
        let compact = inspect_composer_text(&mut shell.app);
        let compact_rel = compact.line_top_full - compact.composer.y;
        assert_no_top_clip(compact, &format!("{name} compact"));

        // Replace / append payload from compact mode.
        let field = shell.app.find(FIELD).unwrap();
        let editor = shell.app.ui.editor_within(field).unwrap();
        editor.set_selection(0, 1);
        type_at_caret(&mut shell.app, payload);

        let mut prev_rel = compact_rel;
        for step in 0..=35 {
            if step > 0 {
                shell.frames(1);
            }
            let f = inspect_composer_text(&mut shell.app);
            assert_no_top_clip(f, &format!("{name} frame {step}"));
            let rel = f.line_top_full - f.composer.y;
            // Compact -> expanded moves text.top monotonically from compact_rel (13.5) to expanded (17.0).
            assert!(
                rel >= compact_rel - 0.5 && rel <= 18.0,
                "{name} frame {step}: relative text top out of range ({rel}, compact={compact_rel}): {f:?}"
            );
            assert!(
                rel >= prev_rel - 0.5,
                "{name} frame {step}: relative text top moved backwards ({prev_rel} -> {rel}): {f:?}"
            );
            prev_rel = rel;
        }
    }

    // (e) Width-only resize while compact (target_geometry keeps previous width),
    // immediately followed by typing across the NEW narrower compact wrap boundary.
    let mut shell = Shell::new(false);
    shell.app.click(BUILD);
    shell.frames(60);
    shell.app.click(FIELD);
    shell.frames(2);
    type_at_caret(&mut shell.app, "short");
    shell.frames(30);
    // Narrow window while still compact on 1 line.
    shell.app.ui.scene.borrow_mut().resize(760., 800.);
    shell.app.settle();
    shell.frames(10);
    let compact_narrow = inspect_composer_text(&mut shell.app);
    assert_no_top_clip(compact_narrow, "post-narrow compact");
    // Cross the narrower compact room without crossing the old wider room.
    type_at_caret(&mut shell.app, &" m".repeat(26));
    for step in 0..=35 {
        if step > 0 {
            shell.frames(1);
        }
        let f = inspect_composer_text(&mut shell.app);
        assert_no_top_clip(f, &format!("post-narrow wrap frame {step}"));
    }
}

#[test]
fn composer_horizontal_window_resize_never_clips_top_of_text() {
    for session in [false, true] {
        let mode = if session { "docked" } else { "new-chat" };
        let mut shell = Shell::new(false);
        if session {
            shell.app.click(BUILD);
            shell.frames(60);
        }
        shell.app.click(FIELD);
        shell.frames(2);
        let text = "The quick brown fox jumps over the lazy dog and keeps running smoothly across the meadow while the window resizes.";
        type_at_caret(&mut shell.app, text);
        shell.frames(40);
        let base = inspect_composer_text(&mut shell.app);
        let base_rel = base.line_top_full - base.composer.y;
        assert_no_top_clip(base, &format!("{mode} wide"));

        // Smoothly drag window narrower (causing repeated line wraps), then wider again.
        for width in [1100., 980., 860., 760., 680., 620., 760., 960., 1200.] {
            shell.app.ui.scene.borrow_mut().resize(width, 800.);
            shell.app.settle();
            for step in 0..8 {
                if step > 0 {
                    shell.frames(1);
                }
                let f = inspect_composer_text(&mut shell.app);
                assert_no_top_clip(f, &format!("{mode} width={width} frame {step}"));
                let rel = f.line_top_full - f.composer.y;
                assert!(
                    (rel - base_rel).abs() <= 0.5,
                    "{mode} width={width} frame {step}: relative text top drifted ({rel} vs {base_rel}): {f:?}"
                );
            }
        }

        // Caret in the middle of a draft that wraps PAST the 240px cap when narrowed sharply:
        // Pass-1 pill.width must expand editor_height before caret reveal runs so Line 1 stays unclipped.
        let field = shell.app.find(FIELD).unwrap();
        let editor = shell.app.ui.editor_within(field).unwrap();
        let len = editor.editor.borrow().text().len();
        editor.set_selection(0, len);
        let mid_para = "alpha beta gamma delta epsilon zeta eta theta ".repeat(14);
        type_at_caret(&mut shell.app, mid_para.trim_end());
        shell.frames(40);
        // Place caret near line 2 (well inside 240px even when narrowed).
        let field = shell.app.find(FIELD).unwrap();
        let editor = shell.app.ui.editor_within(field).unwrap();
        editor.set_selection(40, 40);
        shell.frames(5);
        shell.app.ui.scene.borrow_mut().resize(620., 800.);
        shell.app.settle();
        for step in 0..=20 {
            if step > 0 {
                shell.frames(1);
            }
            let f = inspect_composer_text(&mut shell.app);
            assert_no_top_clip(f, &format!("{mode} mid-caret narrow past cap frame {step}"));
        }
    }
}

#[test]
fn composer_mid_animation_interruption_and_headers_never_clip_top_of_text() {
    // 1. Rapid mid-animation interruptions (grow -> paste more -> shrink -> grow).
    let mut shell = Shell::new(false);
    shell.app.click(BUILD);
    shell.frames(60);
    shell.app.click(FIELD);
    shell.frames(2);
    type_at_caret(&mut shell.app, "L1\nL2\nL3");
    shell.frames(35);
    let base = inspect_composer_text(&mut shell.app);
    let base_rel = base.line_top_full - base.composer.y;

    // Grow to 5 lines, interrupt at frame 3 with 3 more lines, interrupt at frame 3 with shrink to 4, interrupt with +2.
    type_at_caret(&mut shell.app, "\nL4\nL5");
    for step in 0..3 {
        if step > 0 { shell.frames(1); }
        let f = inspect_composer_text(&mut shell.app);
        assert_no_top_clip(f, &format!("interrupt phase 1 frame {step}"));
        assert!(((f.line_top_full - f.composer.y) - base_rel).abs() <= 0.5);
    }
    type_at_caret(&mut shell.app, "\nL6\nL7\nL8");
    for step in 0..3 {
        if step > 0 { shell.frames(1); }
        let f = inspect_composer_text(&mut shell.app);
        assert_no_top_clip(f, &format!("interrupt phase 2 frame {step}"));
        assert!(((f.line_top_full - f.composer.y) - base_rel).abs() <= 0.5);
    }
    let field = shell.app.find(FIELD).unwrap();
    let editor = shell.app.ui.editor_within(field).unwrap();
    let len = editor.editor.borrow().text().len();
    editor.set_selection(11, len); // keep "L1\nL2\nL3\nL4"
    type_at_caret(&mut shell.app, "");
    for step in 0..3 {
        if step > 0 { shell.frames(1); }
        let f = inspect_composer_text(&mut shell.app);
        assert_no_top_clip(f, &format!("interrupt phase 3 frame {step}"));
        assert!(((f.line_top_full - f.composer.y) - base_rel).abs() <= 0.5);
    }
    type_at_caret(&mut shell.app, "\nL5\nL6");
    for step in 0..=35 {
        if step > 0 { shell.frames(1); }
        let f = inspect_composer_text(&mut shell.app);
        assert_no_top_clip(f, &format!("interrupt phase 4 frame {step}"));
        assert!(((f.line_top_full - f.composer.y) - base_rel).abs() <= 0.5);
    }

    // 2. Cargo and question headers appearing/disappearing over a multi-line draft
    // never clip the top of text or let the header overlap line 1.
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/composer_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(700., 600.);
    app.frames(30);
    app.click("role:textbox");
    app.frames(2);
    type_at_caret(&mut app, "Header test line 1\nLine 2\nLine 3\nLine 4");
    app.frames(35);
    let no_header_warnings = zgx::inspect::layout_warnings(&app.ui).len();

    for (state, on, off, label) in [
        (
            "test_composer_cargo",
            r#"[{"id":"f1","kind":"file","name":"notes.md"}]"#,
            "[]",
            "cargo",
        ),
        (
            "test_composer_question",
            r#"{"text":"Choose a branch","options":[{"value":"main","label":"Main"}]}"#,
            "null",
            "question",
        ),
    ] {
        app.host.set_state(state, Value::from_json(on).unwrap()).unwrap();
        for step in 0..=30 {
            if step > 0 { app.frames(1); }
            let f = inspect_composer_text(&mut app);
            assert_no_top_clip(f, &format!("{label} open frame {step}"));
            if step == 3 || step == 30 {
                assert!(zgx::inspect::layout_warnings(&app.ui).len() <= no_header_warnings);
            }
        }
        app.host.set_state(state, Value::from_json(off).unwrap()).unwrap();
        for step in 0..=30 {
            if step > 0 { app.frames(1); }
            let f = inspect_composer_text(&mut app);
            assert_no_top_clip(f, &format!("{label} close frame {step}"));
        }
    }
}

#[test]
fn composer_writing_new_lines_never_clips_caret_at_bottom_or_text_at_top() {
    for session in [false, true] {
        let mode = if session { "docked" } else { "new-chat" };
        let mut shell = Shell::new(false);
        if session {
            shell.app.click(BUILD);
            shell.frames(60);
        }
        shell.app.click(FIELD);
        shell.frames(2);
        type_at_caret(&mut shell.app, "Line 1");
        shell.frames(30);
        let first = inspect_composer_text(&mut shell.app);
        assert_no_top_clip(first, &format!("{mode} line 1"));
        assert_caret_unclipped(first, &format!("{mode} line 1"));

        // Press Shift+Enter (newline) line by line from 1 -> 13 (crossing compact->expanded,
        // expanded min->3, 3->4..10->11 cap, and past cap 11->13).
        for line in 2..=13 {
            type_at_caret(&mut shell.app, "\n");
            let from_compact = session && line == 2;
            for step in 0..=25 {
                if step > 0 {
                    shell.frames(1);
                }
                let f = inspect_composer_text(&mut shell.app);
                // Lines 1..=10 fit under the 240px viewport cap so line 1 must stay unclipped;
                // at 11+ lines the viewport is capped and scrolls internally by design.
                if line <= 10 {
                    assert_no_top_clip(f, &format!("{mode} newline to {line} frame {step}"));
                }
                // Compact (49px) -> Expanded (120px) starts at p=0 on frame 0 before the first
                // animation tick; by frame 1 (16ms) line 2's caret must already be fully visible.
                if !from_compact || step >= 1 {
                    assert_caret_unclipped(f, &format!("{mode} newline to {line} frame {step}"));
                }
            }
            // Type text on the newly created line and verify caret stays completely unclipped.
            type_at_caret(&mut shell.app, &format!("Line {line}"));
            let typed = inspect_composer_text(&mut shell.app);
            assert_caret_unclipped(typed, &format!("{mode} typed on line {line}"));
        }

        // Also test auto-wrap growth (typing words across the right edge in expanded mode):
        // top line and bottom caret must both stay unclipped on every single character.
        let field = shell.app.find(FIELD).unwrap();
        let editor = shell.app.ui.editor_within(field).unwrap();
        let len = editor.editor.borrow().text().len();
        editor.set_selection(0, len);
        type_at_caret(&mut shell.app, "Line 1\nLine 2\n");
        shell.frames(30);
        for word in 0..22 {
            type_at_caret(&mut shell.app, "meadow ");
            let f0 = inspect_composer_text(&mut shell.app);
            assert_no_top_clip(f0, &format!("{mode} wrap word {word} frame 0"));
            assert_caret_unclipped(f0, &format!("{mode} wrap word {word} frame 0"));
            shell.frames(1);
            let f1 = inspect_composer_text(&mut shell.app);
            assert_no_top_clip(f1, &format!("{mode} wrap word {word} frame 1"));
            assert_caret_unclipped(f1, &format!("{mode} wrap word {word} frame 1"));
        }
    }
}



