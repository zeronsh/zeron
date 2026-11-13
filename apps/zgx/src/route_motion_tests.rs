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
        let ui = std::env::var_os("ZERON_ZGX_TEST_UI")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"));
        let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui.join("shell_fixture.luau")));
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
