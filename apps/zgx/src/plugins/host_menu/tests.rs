use super::*;
use zgx::input::PointerButton;
use zgx_script::{ScriptHost, testing::TestApp};

const ALT: Modifiers = Modifiers { alt: true, shift: false, control: false, meta: false };
const NONE: Modifiers = Modifiers { alt: false, shift: false, control: false, meta: false };

#[test]
fn alt_alone_is_a_tap_and_alt_with_anything_is_not() {
    let mut tap = AltTap::default();
    assert!(!tap.modifiers(ALT));
    assert!(tap.modifiers(NONE), "Alt down and up alone");

    // Alt+F: a key between.
    assert!(!tap.modifiers(ALT));
    tap.interrupt();
    assert!(!tap.modifiers(NONE), "Alt with a key is a shortcut");

    // Ctrl+Alt, either order.
    assert!(!tap.modifiers(ALT));
    assert!(!tap.modifiers(Modifiers { control: true, ..ALT }));
    assert!(!tap.modifiers(ALT));
    assert!(!tap.modifiers(NONE), "another modifier with it");
    assert!(!tap.modifiers(Modifiers { control: true, ..NONE }));
    assert!(!tap.modifiers(Modifiers { control: true, ..ALT }));
    assert!(!tap.modifiers(ALT));
    assert!(!tap.modifiers(NONE), "Alt left after Ctrl+Alt");

    // And a tap again after all that.
    assert!(!tap.modifiers(ALT));
    assert!(tap.modifiers(NONE));
}

struct Bar {
    app: TestApp,
    state: Rc<State>,
    chosen: Rc<RefCell<Vec<String>>>,
}

fn command(id: &str, title: &str, menu: &str, key: Option<&str>) -> zgx_script::Command {
    zgx_script::Command { id: id.into(), title: title.into(), menu: menu.into(), key: key.map(Into::into), separator: false }
}

/// The shell's fixture, and the host's bar over it as the main window has
/// it (`view`), its menus `commands`' (`crate::menus`).
fn mount(commands: &[zgx_script::Command]) -> Bar {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let mut app = TestApp::new(move |runtime| ScriptHost::new(runtime, path));
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    app.ui.scene.borrow_mut().resize(1240., 820.);
    let chosen: Rc<RefCell<Vec<String>>> = Default::default();
    let heard = chosen.clone();
    let menus = app.ui.signal(crate::menus(commands));
    let (view, state) = build(&mut app.ui, menus, None, Rc::new(|| false), move |action| heard.borrow_mut().push(action.0));
    app.ui.mount(view);
    app.frames(6);
    Bar { app, state, chosen }
}

impl Bar {
    fn modifiers(&mut self, held: Modifiers) {
        zgx::input::set_held_modifiers(&self.app.ui.runtime, held);
        self.app.frames(3);
    }
    fn tap_alt(&mut self) {
        self.modifiers(ALT);
        self.modifiers(NONE);
    }
    fn shown(&mut self) -> bool {
        self.app.frames(1);
        let shown = self.state.shown.with_untracked(|v| *v);
        let island = self.state.island.get().is_some_and(|n| self.app.ui.scene.borrow().visible_bounds(n).is_some_and(|b| b.width > 0.));
        assert_eq!(shown, island, "the island shows as the bar is shown");
        shown
    }
    /// The node of the bar's whose role is `role` and label `label`.
    fn node(&mut self, role: Role, label: &str) -> Option<NodeId> {
        self.app.frames(1);
        let scene = self.app.ui.scene.borrow();
        let semantics = self.app.ui.semantics.borrow();
        let mut stack = vec![scene.root()];
        while let Some(n) = stack.pop() {
            if semantics.get(n).is_some_and(|s| s.role == role && s.label == label) && scene.visible_bounds(n).is_some_and(|b| b.width > 0.) {
                return Some(n);
            }
            stack.extend(scene.children(n).iter().rev());
        }
        None
    }
    fn focused_label(&mut self) -> Option<String> {
        let node = self.app.ui.input.focused()?;
        self.app.ui.semantics.borrow().get(node).map(|s| s.label.clone())
    }
    fn bounds(&mut self, selector: &str) -> zgx::scene::Rect {
        let node = self.app.find(selector).unwrap_or_else(|| panic!("no {selector}"));
        self.app.ui.scene.borrow().transformed_bounds(node)
    }
    fn bounds_of(&mut self, role: Role, label: &str) -> zgx::scene::Rect {
        let node = self.node(role, label).unwrap_or_else(|| panic!("no {role:?} {label}"));
        self.app.ui.scene.borrow().transformed_bounds(node)
    }
    fn click(&mut self, role: Role, label: &str) {
        let b = self.bounds_of(role, label);
        self.press_at(b.x + b.width / 2., b.y + b.height / 2.);
    }
    fn press_at(&mut self, x: f32, y: f32) {
        let button = PointerButton::Primary;
        self.app.ui.dispatch(InputEvent::PointerDown { x, y, button });
        self.app.ui.dispatch(InputEvent::PointerUp { x, y, button });
        self.app.frames(3);
    }
}

fn file() -> Vec<zgx_script::Command> {
    vec![command("new-chat", "New Chat", "File", Some("cmd+n")), command("palette", "Command Palette", "Go", Some("cmd+k"))]
}

#[test]
fn hidden_until_alt_is_tapped_and_then_over_the_window_taking_no_room() {
    let mut bar = mount(&file());
    assert!(!bar.shown(), "hidden at first");
    let shell = bar.bounds("#shell");
    assert_eq!((shell.x, shell.y, shell.width, shell.height), (0., 0., 1240., 820.), "the shell has the whole window");

    bar.tap_alt();
    assert!(bar.shown(), "Alt pressed and released alone shows it");
    assert_eq!(bar.focused_label().as_deref(), Some("File"), "its first menu focused");
    let shell = bar.bounds("#shell");
    assert_eq!((shell.y, shell.height), (0., 820.), "over the window, nothing moved down");
    let island = bar.state.island.get().expect("the island");
    let island = bar.app.ui.scene.borrow().transformed_bounds(island);
    assert_eq!((island.x, island.y), (ISLAND_INSET, ISLAND_INSET), "where the title bar's island is");
    assert!((island.height - (CONTROL + 2. * ISLAND_PAD + 2.)).abs() < 0.5, "as tall as the island: {island:?}");

    bar.app.key(Key::Escape);
    assert!(!bar.shown(), "Escape puts it away");

    bar.tap_alt();
    assert!(bar.shown());
    bar.tap_alt();
    assert!(!bar.shown(), "Alt again puts it away");
}

#[test]
fn alt_with_a_key_click_or_another_modifier_shows_nothing() {
    let mut bar = mount(&file());
    bar.modifiers(ALT);
    bar.app.ui.dispatch(InputEvent::KeyDown { key: Key::Character("f".into()), modifiers: ALT, repeat: false });
    bar.app.ui.dispatch(InputEvent::KeyUp { key: Key::Character("f".into()), modifiers: ALT });
    bar.modifiers(NONE);
    assert!(!bar.shown(), "Alt+F is a shortcut, not a tap");

    bar.modifiers(ALT);
    bar.press_at(600., 400.);
    bar.modifiers(NONE);
    assert!(!bar.shown(), "Alt+click");

    bar.modifiers(ALT);
    bar.modifiers(Modifiers { control: true, ..ALT });
    bar.modifiers(NONE);
    assert!(!bar.shown(), "Ctrl+Alt");

    bar.tap_alt();
    assert!(bar.shown(), "and a tap after them still shows it");
}

#[test]
fn a_click_outside_puts_it_away_and_one_on_it_opens_a_menu() {
    let mut bar = mount(&file());
    bar.tap_alt();
    bar.click(Role::Button, "File");
    assert!(bar.shown(), "a click on it keeps it");
    assert!(bar.node(Role::MenuItem, "New Chat").is_some(), "and opens its menu");
    bar.press_at(800., 600.);
    assert!(!bar.shown(), "a click elsewhere puts it away");
}

#[test]
fn the_keyboard_goes_from_menu_to_menu_and_chooses() {
    let mut bar = mount(&file());
    bar.tap_alt();
    assert_eq!(bar.focused_label().as_deref(), Some("File"));
    bar.app.key(Key::ArrowRight);
    assert_eq!(bar.focused_label().as_deref(), Some("Go"), "Right: the next menu");
    bar.app.key(Key::ArrowLeft);
    assert_eq!(bar.focused_label().as_deref(), Some("File"), "Left: back");
    bar.app.key(Key::ArrowDown);
    assert!(bar.node(Role::MenuItem, "New Chat").is_some(), "Down opens it");
    bar.app.key(Key::ArrowRight);
    assert!(bar.node(Role::MenuItem, "Command Palette").is_some(), "Right in an open menu opens the next");
    let item = bar.node(Role::MenuItem, "Command Palette").unwrap();
    bar.app.ui.input.focus(&bar.app.ui.scene, Some(item));
    bar.app.key(Key::Enter);
    assert_eq!(bar.chosen.borrow().as_slice(), ["palette"], "Enter chooses");
    assert!(!bar.shown(), "and the bar goes with the menu");
}

/// R12 with the bar hidden: a template that claims the host's ids still
/// leaves View → Use Zeron v3 one Alt away, and choosing it runs
/// the host's command.
#[test]
fn the_way_back_to_the_default_interface_is_one_alt_away_whatever_the_template() {
    let hostile = vec![
        command(crate::plugins::USE_ZERON_TEMPLATE, "Stay Here", "View", Some("cmd+shift+d")),
        command("zeron.anything", "Mine", "File", None),
        command("new-chat", "New Chat", "File", Some("cmd+n")),
    ];
    let mut bar = mount(&hostile);
    assert!(!bar.shown());
    bar.tap_alt();
    bar.click(Role::Button, "View");
    assert!(bar.node(Role::MenuItem, "Stay Here").is_none(), "the template's own is left out");
    bar.click(Role::MenuItem, "Use Zeron v3");
    assert_eq!(bar.chosen.borrow().as_slice(), [crate::plugins::USE_ZERON_TEMPLATE]);
    assert!(!bar.shown());
}

/// Zeron's own interface has Alt (its title bar's): the host's bar stays
/// away, and the scripts are told.
#[test]
fn alt_goes_to_the_scripts_title_bar_when_it_has_the_menus() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let mut app = TestApp::new(move |runtime| ScriptHost::new(runtime, path));
    app.frames(30);
    let asked = Rc::new(Cell::new(0));
    let heard = asked.clone();
    let menus = app.ui.signal(crate::menus(&file()));
    let (view, state) = build(&mut app.ui, menus, None, Rc::new(move || {
        heard.set(heard.get() + 1);
        true
    }), |_| {});
    app.ui.mount(view);
    app.frames(6);
    zgx::input::set_held_modifiers(&app.ui.runtime, ALT);
    app.frames(2);
    zgx::input::set_held_modifiers(&app.ui.runtime, NONE);
    app.frames(2);
    assert_eq!(asked.get(), 1, "the scripts asked");
    assert!(!state.shown.with_untracked(|v| *v), "and the host's bar stays away");
}

#[test]
fn scripts_choose_the_menus_items_and_of_the_host_s_only_the_way_back() {
    let menus = crate::menus(&file());
    assert!(choosable("new-chat", &menus));
    assert!(choosable(crate::plugins::USE_ZERON_TEMPLATE, &menus));
    assert!(!choosable("rm-rf", &menus), "not an item");
    crate::plugins::preview::set(Some(crate::plugins::preview::Preview { id: "x".into(), name: "X".into() }));
    let with_keep = crate::menus(&file());
    crate::plugins::preview::set(None);
    assert!(format!("{with_keep:?}").contains(crate::plugins::KEEP_TEMPLATE), "Keep is in the bar in preview");
    assert!(!choosable(crate::plugins::KEEP_TEMPLATE, &with_keep), "but the host's alone to choose");
}

#[test]
fn the_scripts_read_the_menus_with_their_keys() {
    let value = menus_value(&crate::menus(&file()));
    let json = value.to_json();
    assert!(json.contains(r#""label":"File""#) && json.contains(r#""id":"new-chat""#), "{json}");
    assert!(json.contains(r#""keys":"cmd+n""#), "{json}");
    assert!(json.contains(r#""id":"zeron.use-zeron-template""#), "{json}");
}
