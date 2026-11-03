//! An editor's context menu, the design system's (ds/editor_menu) rather
//! than zgx's own: zgx gives an editor its menu only when it has none, and
//! its actions aren't open to scripts, so the host does them here, as zgx's
//! own menu does them (editor_menu.rs), for the editor the menu is about.
//!
//!   editor_menu.at { x, y }  the editor at window point x, y (or, with none
//!                            there, the one with focus), and its selection
//!                            now, are the menu's from now on; `editor_menu`
//!                            (shared) says what it can do, or nil with no
//!                            editor:
//!                            { selected, copyable, editable, clipboard,
//!                              platform, looks_up }
//!   editor_menu.cut / .copy / .paste / .select_all / .look_up
//!                            on that editor, as zgx's menu items do them:
//!                            a paste through the editor's own paste hook
//!                            first (a picture pasted into the composer)
//!
//! The menu takes the focus while it is open, so the editor hides its
//! selection (zgx draws a field's only while it has the focus), and what
//! happens to the editor until an item is chosen isn't the menu's to say.
//! So each item first gives the editor the focus back and puts back the
//! selection the menu opened on (when that text is still there), then
//! acts, as AppKit's menu does on the selection it opened on: the result
//! selected, or the caret placed, in the focused editor.

use std::{cell::RefCell, rc::Rc};

use zgx::{
    platform::PlatformService,
    widgets::{EditorHandle, Ui, WeakUi},
};
use zgx_script::{ScriptHost, Value};

type Platform = Rc<RefCell<Option<PlatformService>>>;

/// What the open menu is about: the editor, where it was asked, and its
/// selection then (the range and its text).
#[derive(Clone)]
struct Opened {
    editor: EditorHandle,
    at: (f32, f32),
    range: (usize, usize),
    selected: String,
}

type Current = Rc<RefCell<Option<Opened>>>;

pub fn install(host: &ScriptHost, ui: WeakUi, platform: Platform) {
    let current: Current = Rc::default();
    let said = host.shared("editor_menu", Value::Nil);
    {
        let (ui, platform, current) = (ui.clone(), platform.clone(), current.clone());
        host.action("editor_menu.at", move |at| {
            let Some(ui) = ui.upgrade() else { return };
            let point = |key: &str| at.field(key).and_then(Value::as_f64).unwrap_or(0.) as f32;
            let (x, y) = (point("x"), point("y"));
            let Some(editor) = editor_at(&ui, x, y).or_else(|| ui.focused_editor()) else {
                *current.borrow_mut() = None;
                said.set(Value::Nil);
                return;
            };
            let service = platform.borrow().clone();
            // (A password field's text never leaves it: `copy` gives nothing.)
            let selected = editor.copy();
            let clipboard = service
                .as_ref()
                .and_then(|p| p.0.clipboard_text())
                .is_some_and(|text| !text.is_empty());
            said.set(Value::record([
                ("selected", Value::from(selected.as_str())),
                ("copyable", Value::Bool(!selected.is_empty())),
                ("editable", Value::Bool(!editor.is_read_only())),
                ("clipboard", Value::Bool(clipboard)),
                ("platform", Value::Bool(service.is_some())),
                ("looks_up", Value::Bool(service.as_ref().is_some_and(|p| p.0.looks_up()))),
            ]));
            let range = editor.selection();
            *current.borrow_mut() = Some(Opened { editor, at: (x, y), range, selected });
        });
    }
    // The menu's editor, ready for an item: focused, its selection the one
    // the menu opened on.
    let ready = {
        let (ui, current) = (ui.clone(), current.clone());
        move || -> Option<EditorHandle> {
            let ui = ui.upgrade()?;
            let opened = current.borrow().clone()?;
            ready(&ui, &opened)
        }
    };
    let ready = Rc::new(ready);
    {
        let (platform, ready) = (platform.clone(), ready.clone());
        host.action("editor_menu.cut", move |_| {
            if let (Some(editor), Some(service)) = (ready(), &*platform.borrow())
                && !editor.is_read_only()
                && service.0.set_clipboard_text(editor.copy())
            {
                editor.cut();
            }
        });
    }
    {
        let (platform, ready) = (platform.clone(), ready.clone());
        host.action("editor_menu.copy", move |_| {
            if let (Some(editor), Some(service)) = (ready(), &*platform.borrow()) {
                service.0.set_clipboard_text(editor.copy());
            }
        });
    }
    {
        let (ui, platform, ready) = (ui.clone(), platform.clone(), ready.clone());
        host.action("editor_menu.paste", move |_| {
            let (Some(ui), Some(editor)) = (ui.upgrade(), ready()) else { return };
            let text = platform
                .borrow()
                .as_ref()
                .and_then(|p| p.0.clipboard_text())
                .filter(|text| !text.is_empty());
            ui.paste_into(&editor, text.as_deref());
        });
    }
    {
        let ready = ready.clone();
        host.action("editor_menu.select_all", move |_| {
            if let Some(editor) = ready() {
                editor.select_all();
            }
        });
    }
    // (Over the selected text: the definition hangs from the selection's
    // start, so it needs the selection back first.)
    // (zgx looks up in an editor shown a line at a time too, the composer's,
    // since 6f8ad524.)
    host.action("editor_menu.look_up", move |_| {
        if let (Some(editor), Some(service)) = (ready(), &*platform.borrow()) {
            editor.look_up(service);
        }
    });
}

/// `opened`'s editor, given the focus back with the selection the menu
/// opened on: the same editor, or, rebuilt since (its node gone), the one
/// at the same point, if its text there is the same.
fn ready(ui: &Ui, opened: &Opened) -> Option<EditorHandle> {
    let editor = if ui.scene.borrow().contains(opened.editor.node) {
        opened.editor.clone()
    } else {
        editor_at(ui, opened.at.0, opened.at.1)?
    };
    ui.input.focus(&ui.scene, Some(editor.node));
    let (anchor, focus) = opened.range;
    if editor.selection() != opened.range {
        let was = editor.selection();
        editor.set_selection(anchor, focus);
        // (Its text changed under the menu: what it shows now stands.)
        if editor.copy() != opened.selected {
            editor.set_selection(was.0, was.1);
        }
    }
    Some(editor)
}

/// The editor at window point `x`, `y`: the node there, or the nearest
/// above it that is one.
fn editor_at(ui: &Ui, x: f32, y: f32) -> Option<EditorHandle> {
    let scene = ui.scene.borrow();
    let mut node = scene.hit_test(x, y);
    while let Some(id) = node {
        if let Some(editor) = ui.editor(id) {
            return Some(editor);
        }
        node = scene.parent(id);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use std::path::PathBuf;

    use zgx::{
        input::{InputEvent, PointerButton},
        platform::{FilePicker, Pending, Platform, PlatformService},
        widgets::EditorHandle,
    };
    use zgx_script::{
        ScriptHost, Value,
        testing::{FakeDesktop, TestApp},
    };

    // An editor, and a command for each action (the menu asks the host
    // through `ui.invoke`, as ds/editor_menu does).
    const APP: &str = r#"
local read_only = ui.state("read_only", false)
local function asks(id, action, arg)
    ui.command { id = id, title = id, run = function() ui.invoke(action, arg) end }
end
asks("at", "editor_menu.at", { x = 40, y = 30 })
asks("nowhere", "editor_menu.at", { x = 400, y = 300 })
asks("cut", "editor_menu.cut", nil)
asks("copy", "editor_menu.copy", nil)
asks("paste", "editor_menu.paste", nil)
asks("all", "editor_menu.select_all", nil)
return ui.column {
    ui.textarea { id = "t", value = ui.state("v", "hello world"), w = 200, h = 40, read_only = function() return read_only:get() end },
}
"#;

    fn app() -> TestApp {
        let app = TestApp::with_desktop(|runtime| ScriptHost::inline(runtime, "app", APP));
        app.ui.scene.borrow_mut().resize(500., 400.);
        let service = PlatformService(app.desktop.clone().unwrap());
        super::install(&app.host, app.ui.downgrade(), Rc::new(RefCell::new(Some(service))));
        app.frames(2);
        app
    }

    fn said(app: &TestApp) -> Value {
        app.host.shared("editor_menu", Value::Nil).signal().get()
    }

    fn select(app: &mut TestApp, from: usize, to: usize) {
        let id = app.find("#t").expect("the editor");
        let editor = app.ui.editor_within(id).expect("an editor");
        editor.set_selection(from, to);
        app.frames(1);
    }

    /// The editor's text, as its state holds it.
    fn text(app: &TestApp) -> Option<String> {
        app.host
            .states()
            .into_iter()
            .find(|(name, _)| name == "v")
            .map(|(_, v)| match v {
                Value::Str(s) => s.to_string(),
                other => format!("{other:?}"),
            })
    }

    fn field(v: &Value, key: &str) -> Value {
        v.field(key).cloned().unwrap_or(Value::Nil)
    }

    #[test]
    fn the_menu_knows_the_editor_at_the_click_and_does_what_zgx_s_did() {
        let mut app = app();
        // Nothing selected: nothing to copy; it edits.
        assert!(app.host.run_command("at"));
        let state = said(&app);
        assert_eq!(field(&state, "copyable"), Value::Bool(false));
        assert_eq!(field(&state, "editable"), Value::Bool(true));
        assert_eq!(field(&state, "platform"), Value::Bool(true));
        // No editor there: nil.
        assert!(app.host.run_command("nowhere"));
        assert_eq!(said(&app), Value::Nil);
        // "hello" selected: copied to the clipboard, then cut from the text.
        select(&mut app, 0, 5);
        assert!(app.host.run_command("at"));
        assert_eq!(field(&said(&app), "selected"), Value::from("hello"));
        assert!(app.host.run_command("copy"));
        assert_eq!(app.desktop.as_ref().unwrap().clipboard(), "hello");
        assert!(app.host.run_command("cut"));
        app.frames(1);
        assert_eq!(text(&app), Some(" world".into()));
        // The clipboard pasted back in, at the caret.
        assert!(app.host.run_command("at"));
        assert_eq!(field(&said(&app), "clipboard"), Value::Bool(true));
        assert!(app.host.run_command("paste"));
        app.frames(1);
        assert_eq!(text(&app), Some("hello world".into()));
        // Select All, then read-only: it copies but doesn't edit.
        assert!(app.host.run_command("all"));
        assert!(app.host.run_command("at"));
        assert_eq!(field(&said(&app), "selected"), Value::from("hello world"));
        app.host.set_state("read_only", Value::Bool(true)).unwrap();
        app.frames(2);
        assert!(app.host.run_command("at"));
        assert_eq!(field(&said(&app), "editable"), Value::Bool(false));
        assert!(app.host.run_command("cut"));
        app.frames(1);
        assert_eq!(text(&app), Some("hello world".into()), "a read-only editor isn't cut");
    }

    /// A platform that looks words up (the fake desktop doesn't), noting
    /// each word asked and the point its definition hangs from; the rest is
    /// the fake desktop's (its clipboard).
    struct LooksUp {
        desktop: Rc<FakeDesktop>,
        asked: RefCell<Vec<(String, (f32, f32))>>,
    }

    impl Platform for LooksUp {
        fn clipboard_text(&self) -> Option<String> {
            self.desktop.clipboard_text()
        }
        fn set_clipboard_text(&self, text: String) -> bool {
            self.desktop.set_clipboard_text(text)
        }
        fn open_files(&self, picker: FilePicker) -> Pending<Result<Vec<PathBuf>, String>> {
            self.desktop.open_files(picker)
        }
        fn save_file(&self, picker: FilePicker) -> Pending<Result<Option<PathBuf>, String>> {
            self.desktop.save_file(picker)
        }
        fn looks_up(&self) -> bool {
            true
        }
        fn look_up(&self, text: &str, baseline: (f32, f32), _size: f32) -> bool {
            self.asked.borrow_mut().push((text.to_string(), baseline));
            true
        }
    }

    /// The window's composer (ui/composer_place_fixture.luau: ds/composer,
    /// its editor with ds/editor_menu, the window's menu host), the real
    /// host installed over a platform that looks words up.
    fn composer() -> (TestApp, Rc<LooksUp>) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/composer_place_fixture.luau");
        let app = TestApp::with_desktop(move |runtime| ScriptHost::new(runtime, path));
        app.ui.scene.borrow_mut().resize(900., 700.);
        let platform = Rc::new(LooksUp { desktop: app.desktop.clone().unwrap(), asked: RefCell::default() });
        let service = PlatformService(platform.clone());
        super::install(&app.host, app.ui.downgrade(), Rc::new(RefCell::new(Some(service))));
        app.host.set_state("test_place_draft", Value::from("hello world")).unwrap();
        settle(&app);
        (app, platform)
    }

    /// A few frames over real time (the menu opens, and an item runs, on
    /// timers).
    fn settle(app: &TestApp) {
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(15));
            app.frames(2);
        }
    }

    fn draft(app: &TestApp) -> Option<String> {
        state(app, "test_place_draft")
    }

    fn state(app: &TestApp, name: &str) -> Option<String> {
        app.host.states().into_iter().find(|(n, _)| n == name).map(|(_, v)| match v {
            Value::Str(s) => s.to_string(),
            other => format!("{other:?}"),
        })
    }

    fn middle(r: zgx::scene::Rect) -> (f32, f32) {
        (r.x + r.width / 2., r.y + r.height / 2.)
    }

    /// A hand's drag across bytes `from..to` of the editor's text.
    fn drag(app: &mut TestApp, editor: &EditorHandle, from: usize, to: usize) {
        let (a, b) = (editor.text_area(from..from).unwrap(), editor.text_area(to..to).unwrap());
        let (ay, by) = (a.y + a.height / 2., b.y + b.height / 2.);
        app.ui.dispatch(InputEvent::PointerMove { x: a.x + 0.5, y: ay });
        app.ui.dispatch(InputEvent::PointerDown { x: a.x + 0.5, y: ay, button: PointerButton::Primary });
        app.ui.dispatch(InputEvent::PointerMove { x: b.x, y: by });
        app.ui.dispatch(InputEvent::PointerUp { x: b.x, y: by, button: PointerButton::Primary });
        settle(app);
    }

    /// A hand's secondary click at `(x, y)`: the button comes up a few
    /// frames on, the menu open under it by then.
    fn secondary_click(app: &mut TestApp, (x, y): (f32, f32)) {
        app.ui.dispatch(InputEvent::PointerMove { x, y });
        app.ui.dispatch(InputEvent::PointerDown { x, y, button: PointerButton::Secondary });
        settle(app);
        app.ui.dispatch(InputEvent::PointerUp { x, y, button: PointerButton::Secondary });
        settle(app);
    }

    /// The pointer moved from `(x, y)` onto the menu's item `label`, and
    /// clicked; then time for it to blink, close and run.
    fn choose(app: &mut TestApp, from: (f32, f32), label: &str) {
        let item = app.find(&format!("role:menuitem \"{label}\"")).unwrap_or_else(|| panic!("{label} in the menu"));
        let (ix, iy) = middle(app.ui.scene.borrow().transformed_bounds(item));
        for step in 1..=4 {
            let t = step as f32 / 4.;
            app.ui.dispatch(InputEvent::PointerMove { x: from.0 + (ix - from.0) * t, y: from.1 + (iy - from.1) * t });
            app.frames(1);
        }
        app.ui.dispatch(InputEvent::PointerDown { x: ix, y: iy, button: PointerButton::Primary });
        app.ui.dispatch(InputEvent::PointerUp { x: ix, y: iy, button: PointerButton::Primary });
        settle(app);
        settle(app);
    }

    fn focused(app: &TestApp, editor: &EditorHandle) -> bool {
        app.ui.focused_editor().is_some_and(|e| e.node == editor.node)
    }

    // The bug as it was met in the composer: a selection, a secondary
    // click inside it, then Cut or Look Up did nothing the first time.
    // Each item acts on the selection the menu opened on, the first time,
    // and gives the editor the focus back.
    #[test]
    fn each_item_acts_on_the_selection_the_menu_opened_on_the_first_time() {
        let (mut app, platform) = composer();
        let id = app.find("role:textbox \"Message\"").expect("the draft");
        let editor = app.ui.editor_within(id).expect("an editor");

        // "hello" selected; a secondary click inside it keeps it, and it
        // survives the menu opening (and taking the focus).
        drag(&mut app, &editor, 0, 5);
        assert_eq!(editor.selection(), (0, 5));
        let inside = middle(editor.text_area(1..2).unwrap());
        secondary_click(&mut app, inside);
        assert!(app.find("role:menuitem \"Cut\"").is_some(), "the menu is open");
        assert_eq!(field(&said(&app), "selected"), Value::from("hello"));
        assert_eq!(editor.selection(), (0, 5), "the selection survives the menu opening");
        // Cut, the first time: on the clipboard, out of the text, the caret
        // where it was, in the focused editor.
        choose(&mut app, inside, "Cut");
        assert_eq!(app.desktop.as_ref().unwrap().clipboard(), "hello");
        assert_eq!(draft(&app), Some(" world".into()));
        assert!(focused(&app, &editor), "the editor has the focus back");
        assert_eq!(editor.selection(), (0, 0));

        // Look Up, the first time, on "world": over the selected text (its
        // definition hangs from the selection's start), the selection kept.
        drag(&mut app, &editor, 1, 6);
        let inside = middle(editor.text_area(2..3).unwrap());
        secondary_click(&mut app, inside);
        assert_eq!(editor.selection(), (1, 6));
        choose(&mut app, inside, "Look Up “world”");
        let asked = platform.asked.borrow().clone();
        assert_eq!(asked.len(), 1, "looked up once: {asked:?}");
        assert_eq!(asked[0].0, "world");
        let word = editor.text_area(1..6).unwrap();
        let (x, y) = asked[0].1;
        assert!((x - word.x).abs() < 1. && y > word.y && y <= word.y + word.height, "over the word: {:?} in {word:?}", asked[0].1);
        assert!(focused(&app, &editor));
        assert_eq!(editor.selection(), (1, 6));

        // Whatever befalls the selection while the menu is open (here,
        // gone to a caret), Copy copies what it opened on, and that is
        // selected again.
        app.desktop.as_ref().unwrap().copy("");
        secondary_click(&mut app, inside);
        editor.set_selection(6, 6);
        app.frames(2);
        choose(&mut app, inside, "Copy");
        assert_eq!(app.desktop.as_ref().unwrap().clipboard(), "world");
        assert_eq!(editor.selection(), (1, 6));
        assert!(focused(&app, &editor));

        // Paste over it, the first time: the clipboard's text in place of
        // the selection, the caret after it.
        app.desktop.as_ref().unwrap().copy("there");
        secondary_click(&mut app, inside);
        choose(&mut app, inside, "Paste");
        assert_eq!(draft(&app), Some(" there".into()));
        assert_eq!(editor.selection(), (6, 6));
        assert!(focused(&app, &editor));

        // Outside the selection (here a caret), as macOS has it: the word
        // there selected, and the menu about it.
        let off = middle(editor.text_area(3..4).unwrap());
        secondary_click(&mut app, off);
        assert_eq!(field(&said(&app), "selected"), Value::from("there"));
        assert_eq!(editor.selection(), (1, 6));
        app.desktop.as_ref().unwrap().copy("");
        choose(&mut app, off, "Copy");
        assert_eq!(app.desktop.as_ref().unwrap().clipboard(), "there");
        assert_eq!(app.error(), None);
    }
}
