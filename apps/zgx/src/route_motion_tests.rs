//! Composer focus and layout transitions on the real shell fixture.
//! The fixture controls composer deadlines alongside TestApp's frame/timer clocks.

use zgx::{
    input::{InputEvent, Key},
    scene::{NodeId, Rect},
};
use zgx_script::{ScriptHost, Value, testing::TestApp};

// Characterization, not a reproduction of the reported visual glitch.
// These snapshots observe the scene, not GPU pixels. See
// docs/composer-characterization.md for the separate visual procedure.
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
        let app = TestApp::new(|runtime| {
            ScriptHost::new(runtime, ui.join("composer_motion_fixture.luau"))
        });
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
            self.app
                .host
                .set_state(
                    "fixture_composer_now",
                    Value::Num(f64::from(self.frame) * 1000. / 60.),
                )
                .unwrap();
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
