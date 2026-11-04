//! The app's menu where the system has none (Linux, Windows): never a bar
//! of its own across the window, but
//!
//! - in Zeron's own interface, the title bar's ☰ and, as Alt is pressed and
//!   released alone, its controls turned into the menus' titles in place
//!   (ds/title_bar.luau), from the menus the host serves the scripts here
//!   (`serve`: `app.menus`, `app.menu_alt`, `app.menu_choose`);
//! - under a template, which may draw any title bar or none, the host's
//!   own: the same Alt shows the menus' titles over the window's top left
//!   (`bar`), the first focused, taking no room.
//!
//! Alt is the host's either way: a press and release of Alt alone with
//! nothing between (no key, click or scroll; `AltTap`), heard from the
//! window's modifier state and the document's events before any script's
//! view has them. Every item keeps its shortcut whether a menu shows or
//! not: the window's keymap is the menus' (zgx-desktop binds them on the
//! window's root), not a view's.
//!
//! The host's bar can't be taken away by a template, like the toasts
//! (host_bar.rs): it is drawn here, a layer of the host's at the window's
//! root kept over the scripts' (`host_bar::keep_on_top`); its colours,
//! type and corners are fixed here (the title bar's island and ds's menus,
//! from ui/ds/tokens.luau and ui/ds/themes), only following whether the
//! theme is light or dark; and View → Use Zeron v3 is always in
//! it (`crate::menus`). Its glass is solid (ds's opaque material) rather
//! than frosted: what a template draws under it can't change how it reads.
//! In the bar, the same again, Escape, a click outside it, choosing an item
//! or the window losing focus puts it away, focus back where it was; Left
//! and Right go from menu to menu (an open one opens the next), Down opens
//! the focused one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use zgx::compose::{ModalPlacement, PlacementEdge, View, column, component, menu, menu_item, modal, row, submenu, switch, text};
use zgx::cursor::Cursor;
use zgx::input::{EventPhase, InputEvent, Key, Modifiers};
use zgx::reactive::Signal;
use zgx::scene::{BoxShadow, Color, NodeId, Transform};
use zgx::semantics::Role;
use zgx::style::Styled;
use zgx::text_layout::FontFamily;
use zgx::compose::Context;
use zgx::widgets::{Ui, WeakUi};
use zgx_desktop::{AppMenu, AppMenuEntry, MenuAction, WindowContext};

thread_local! {
    static MENUS: RefCell<Option<Signal<Vec<AppMenu>>>> = const { RefCell::new(None) };
    /// What the scripts are told: the menus (`app.menus`), and whether
    /// Alt turned the title bar's controls into them (`app.menu_alt`).
    static SERVED: RefCell<Option<(zgx_script::Shared, zgx_script::Shared)>> = const { RefCell::new(None) };
}

/// The menus as the window was last given them (`windows.set_menus`), for
/// the host's bar and the scripts' to show.
pub fn set(menus: Vec<AppMenu>) {
    if let Some((shared, _)) = SERVED.with(|s| s.borrow().clone()) {
        shared.set(menus_value(&menus));
    }
    if let Some(signal) = MENUS.with(|m| m.borrow().clone()) {
        signal.set(menus);
    }
}

/// The menus to the main window's scripts, where the system has none: the
/// title bar's ☰ and its Alt (ds/title_bar.luau).
///
///   app.menus                 { { label, entries = { entry } } }, nil on
///                             macOS. An entry: { label, id, keys?,
///                             checked?, disabled? }, { separator = true },
///                             or { label, submenu = { entry } }
///   app.menu_alt              true while Alt has the title bar show the
///                             menus' titles; the scripts set it false as
///                             they put them away
///   app.menu_choose(id)       an item chosen, as the menu bar's would be:
///                             one of the menus', and of the host's own only
///                             the way back to the default interface
pub fn serve(host: &zgx_script::ScriptHost, window: zgx_desktop::WindowHandle, menus: &[AppMenu]) {
    if zgx_desktop::has_native_app_menu() {
        return;
    }
    let shared = host.shared("app.menus", menus_value(menus));
    let alt = host.shared("app.menu_alt", false);
    SERVED.with(|s| *s.borrow_mut() = Some((shared, alt)));
    host.action("app.menu_choose", move |id| {
        let zgx_script::Value::Str(id) = id else { return };
        if !choosable(&id, &MENUS.with(|m| m.borrow().as_ref().map(|m| m.with_untracked(Clone::clone))).unwrap_or_default()) {
            eprintln!("zeron-zgx: app.menu_choose: {id} is no item of the menus'");
            return;
        }
        window.dispatch_menu_action(id.to_string());
    });
}

/// Whether a script may choose `id`: an item of `menus`, and of the host's
/// own (`zeron.`) only the way back to the default interface (Keep is the
/// host's alone, from its own bar or toast).
pub(crate) fn choosable(id: &str, menus: &[AppMenu]) -> bool {
    fn has(entries: &[AppMenuEntry], id: &str) -> bool {
        entries.iter().any(|e| match e {
            AppMenuEntry::Item { action, enabled, .. } => *enabled && action.0 == id,
            AppMenuEntry::Standard(item) => item.action().0 == id,
            AppMenuEntry::Submenu(m) => has(&m.entries, id),
            AppMenuEntry::Separator => false,
        })
    }
    (!id.starts_with("zeron.") || id == super::USE_ZERON_TEMPLATE) && menus.iter().any(|m| has(&m.entries, id))
}

/// The menus as the scripts read them (`serve`).
fn menus_value(menus: &[AppMenu]) -> zgx_script::Value {
    use zgx_script::Value;
    fn list(values: Vec<Value>) -> Value {
        Value::List(values.into())
    }
    fn rows(entries: &[AppMenuEntry]) -> Value {
        list(
            entries
                .iter()
                .map(|e| match e {
                    AppMenuEntry::Item { label, action, enabled, checked, accelerator } => {
                        let mut fields = vec![("label", Value::from(label.as_str())), ("id", Value::from(action.0.as_str()))];
                        if let Some(keys) = accelerator.as_ref().and_then(|a| keys(&a.0)) {
                            fields.push(("keys", Value::from(keys.as_str())));
                        }
                        if let Some(checked) = checked {
                            fields.push(("checked", Value::Bool(*checked)));
                        }
                        if !enabled {
                            fields.push(("disabled", Value::Bool(true)));
                        }
                        Value::record(fields)
                    }
                    AppMenuEntry::Standard(item) => Value::record([("label", Value::from(item.label())), ("id", Value::from(item.action().0.as_str()))]),
                    AppMenuEntry::Submenu(m) => Value::record([("label", Value::from(m.label.as_str())), ("submenu", rows(&m.entries))]),
                    AppMenuEntry::Separator => Value::record([("separator", Value::Bool(true))]),
                })
                .collect(),
        )
    }
    list(menus.iter().map(|m| Value::record([("label", Value::from(m.label.as_str())), ("entries", rows(&m.entries))])).collect())
}

/// A shortcut as the scripts write one (`ui.command`'s keys, ds/kbd.luau):
/// `cmd+shift+n`, `cmd` the platform's command modifier.
fn keys(stroke: &zgx::actions::Keystroke) -> Option<String> {
    let m = stroke.modifiers;
    let mut parts: Vec<String> = Vec::new();
    if m.primary_shortcut() {
        parts.push("cmd".into());
    }
    if m.control && cfg!(target_os = "macos") {
        parts.push("ctrl".into());
    }
    if m.meta && !cfg!(target_os = "macos") {
        parts.push("meta".into());
    }
    if m.alt {
        parts.push("alt".into());
    }
    if m.shift {
        parts.push("shift".into());
    }
    let key = match &stroke.key {
        Key::Character(c) => c.to_lowercase(),
        Key::Function(n) => format!("f{n}"),
        other => format!("{other:?}").to_lowercase(),
    };
    parts.push(key);
    Some(parts.join("+"))
}

/// Zeron's own interface runs (no template, none in preview): its title
/// bar has the menus (ds/title_bar.luau), so Alt goes to it.
fn own_interface() -> bool {
    super::preview::current().is_none() && super::templates_root().is_none_or(|root| super::live::active(&root) == super::live::BUILTIN)
}

/// Alt for the scripts' title bar: true if they have it (Zeron's own
/// interface), which then shows or puts away the menus' titles.
fn alt_to_scripts() -> bool {
    let Some((_, alt)) = SERVED.with(|s| s.borrow().clone()) else { return false };
    if !own_interface() {
        return false;
    }
    let now = alt.signal().with_untracked(|v| v.truthy());
    alt.set(zgx_script::Value::Bool(!now));
    true
}

/// The bar, where the system has none: a layer over the window, so it
/// takes no room in the main window's column.
pub fn view(cx: &mut WindowContext) -> Option<View> {
    if zgx_desktop::has_native_app_menu() {
        return None;
    }
    let menus = cx.ui.signal(cx.windows.menus());
    MENUS.with(|m| *m.borrow_mut() = Some(menus.clone()));
    let window = cx.window.clone();
    let tasks = cx.tasks.clone();
    let fade: Spawn = Rc::new(move |work| tasks.spawn(work));
    Some(bar(&mut cx.ui, menus, Some(fade), Rc::new(alt_to_scripts), move |action| window.dispatch_menu_action(action)))
}

/// Runs a future on the window's thread (the fade in).
type Spawn = Rc<dyn Fn(std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>)>;

// ── Alt alone ───────────────────────────────────────────────────────────────

/// A press and release of Alt alone: the modifiers going from none to Alt
/// and back with nothing between them. Another modifier with it, a key
/// (Alt+F, an Alt shortcut), a click or a scroll while it's down, or the
/// window losing focus, and it's no tap.
#[derive(Default, Debug)]
pub(crate) struct AltTap {
    held: Modifiers,
    armed: bool,
}

impl AltTap {
    /// The modifiers held now; true when that ends a tap.
    pub(crate) fn modifiers(&mut self, now: Modifiers) -> bool {
        let alt = Modifiers { alt: true, ..Modifiers::default() };
        let none = Modifiers::default();
        let before = std::mem::replace(&mut self.held, now);
        if before == now {
            return false;
        }
        if before == none && now == alt {
            self.armed = true;
            return false;
        }
        let tapped = self.armed && before == alt && now == none;
        self.armed = false;
        tapped
    }

    /// Something else happened while it was down.
    pub(crate) fn interrupt(&mut self) {
        self.armed = false;
    }
}

// ── The bar ─────────────────────────────────────────────────────────────────

/// What the bar shares between its parts.
struct State {
    ui: WeakUi,
    shown: Signal<bool>,
    /// Its look, 0 to 1, as it fades in.
    opacity: Signal<f32>,
    /// Each menu's open state and its button, in order.
    opens: RefCell<Vec<Signal<bool>>>,
    buttons: RefCell<Vec<Option<NodeId>>>,
    /// Each submenu's trigger and open state.
    subs: RefCell<Vec<(Rc<Cell<Option<NodeId>>>, Signal<bool>)>>,
    /// The key now was taken by `arrow_in_menu`.
    switched: Cell<bool>,
    /// The island (the bar's own node), to tell a click in it from one outside.
    island: Cell<Option<NodeId>>,
    /// What had focus as it showed, given it back as it goes.
    before: Cell<Option<NodeId>>,
    /// Focus the first menu once the bar is laid out.
    focus_first: Cell<bool>,
    fade: Option<Spawn>,
}

impl State {
    fn reveal(self: &Rc<Self>) {
        if self.shown.with_untracked(|v| *v) {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        self.before.set(ui.input.focused());
        self.focus_first.set(true);
        let still = zgx::theme::Themes::of(&ui.runtime).system_preferences().reduced_motion;
        match (&self.fade, still) {
            (Some(spawn), false) => {
                self.opacity.set(0.);
                let opacity = self.opacity.clone();
                spawn(Box::pin(async move {
                    // ds's short fade (motion.FADE): 120 ms, eased out.
                    const STEPS: u32 = 8;
                    for i in 1..=STEPS {
                        zgx::timer::sleep(Duration::from_millis(15)).await;
                        let t = i as f32 / STEPS as f32;
                        opacity.set(1. - (1. - t) * (1. - t));
                    }
                }));
            }
            _ => {
                self.opacity.set(1.);
            }
        }
        self.shown.set(true);
    }

    /// Put away; focus back where it was (`restore`), or left where a click
    /// put it.
    fn hide(&self, restore: bool) {
        if !self.shown.with_untracked(|v| *v) {
            return;
        }
        for open in self.opens.borrow().iter() {
            if open.with_untracked(|v| *v) {
                open.set(false);
            }
        }
        self.shown.set(false);
        self.focus_first.set(false);
        let before = self.before.take();
        if restore && let Some(ui) = self.ui.upgrade() {
            let focused_here = ui.input.focused().is_none_or(|f| self.holds(&ui, f));
            if focused_here {
                ui.input.focus(&ui.scene, before);
            }
        }
    }

    fn toggle(self: &Rc<Self>) {
        if self.shown.with_untracked(|v| *v) { self.hide(true) } else { self.reveal() }
    }

    /// Whether `node` is the bar's: in the island or in one of its menus.
    fn holds(&self, ui: &Ui, node: NodeId) -> bool {
        let scene = ui.scene.borrow();
        let semantics = ui.semantics.borrow();
        let island = self.island.get();
        std::iter::once(node).chain(scene.ancestors(node)).any(|n| {
            Some(n) == island || semantics.get(n).is_some_and(|s| matches!(s.role, Role::Menu | Role::MenuItem))
        })
    }

    /// The menu open or the button focused, by its place.
    fn current(&self, ui: &Ui) -> Option<(usize, bool)> {
        if let Some(i) = self.opens.borrow().iter().position(|o| o.with_untracked(|v| *v)) {
            return Some((i, true));
        }
        let focused = ui.input.focused()?;
        self.buttons.borrow().iter().position(|b| *b == Some(focused)).map(|i| (i, false))
    }

    /// Menu `i`'s button focused, and its menu opened (`open`).
    fn go(&self, ui: &Ui, i: usize, open: bool) {
        for (j, o) in self.opens.borrow().iter().enumerate() {
            if j != i && o.with_untracked(|v| *v) {
                o.set(false);
            }
        }
        if let Some(Some(button)) = self.buttons.borrow().get(i).copied() {
            ui.input.focus(&ui.scene, Some(button));
        }
        if open && let Some(o) = self.opens.borrow().get(i) {
            o.set(true);
        }
    }

    /// Left or Right in one of the bar's open menus (heard before the menu
    /// has it: an open menu keeps its keys): the menu beside it, opened,
    /// unless a submenu is open or Right opens one.
    fn arrow_in_menu(&self, right: bool) {
        if !self.shown.with_untracked(|v| *v) {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        let Some((i, true)) = self.current(&ui) else { return };
        let count = self.buttons.borrow().len().max(1);
        let sub_open = self.subs.borrow().iter().any(|(_, o)| o.with_untracked(|v| *v));
        let on_trigger = ui.input.focused().is_some_and(|f| self.subs.borrow().iter().any(|(t, _)| t.get() == Some(f)));
        if sub_open || (right && on_trigger) {
            return;
        }
        let next = if right { (i + 1) % count } else { (i + count - 1) % count };
        self.go(&ui, next, true);
        // (The key goes on to where focus is now: not the bar's again.)
        self.switched.set(true);
    }

    /// The first menu's button focused, once it is there.
    fn focus_first_now(&self) {
        if !self.focus_first.get() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else { return };
        if let Some(Some(button)) = self.buttons.borrow().first().copied()
            && ui.scene.borrow().contains(button)
        {
            self.focus_first.set(false);
            ui.input.focus(&ui.scene, Some(button));
        }
    }
}

/// The bar of `menus`, each item's choosing `dispatch`ed, over the window
/// `ui` lays out; `fade` runs its fade in (none: it shows at once).
/// `scripts` is asked first as Alt is tapped: true if the scripts' title
/// bar took it (Zeron's own interface, `alt_to_scripts`).
pub(crate) fn bar(ui: &mut Ui, menus: Signal<Vec<AppMenu>>, fade: Option<Spawn>, scripts: Rc<dyn Fn() -> bool>, dispatch: impl Fn(MenuAction) + 'static) -> View {
    build(ui, menus, fade, scripts, dispatch).0
}

fn build(ui: &mut Ui, menus: Signal<Vec<AppMenu>>, fade: Option<Spawn>, scripts: Rc<dyn Fn() -> bool>, dispatch: impl Fn(MenuAction) + 'static) -> (View, Rc<State>) {
    let state = Rc::new(State {
        ui: ui.downgrade(),
        shown: ui.signal(false),
        opacity: ui.signal(1.),
        opens: RefCell::default(),
        buttons: RefCell::default(),
        subs: RefCell::default(),
        switched: Cell::new(false),
        island: Cell::new(None),
        before: Cell::new(None),
        focus_first: Cell::new(false),
        fade,
    });
    let tap = Rc::new(RefCell::new(AltTap::default()));

    // Alt, from the window's modifiers (a modifier alone is no key event).
    let held = zgx::input::held_modifiers(&ui.runtime);
    let following = {
        let (state, tap) = (state.clone(), tap.clone());
        ui.runtime.effect(move || {
            let now = held.get();
            if tap.borrow_mut().modifiers(now) && (state.shown.with_untracked(|v| *v) || !scripts()) {
                state.toggle();
            }
        })
    };
    // Away as the window loses focus (Alt+Tab, a click in another window):
    // its Alt is no tap.
    let active = ui.observe_window_active();
    let leaving = {
        let (state, tap) = (state.clone(), tap.clone());
        ui.runtime.effect(move || {
            if !active.get() {
                tap.borrow_mut().interrupt();
                state.hide(false);
            }
        })
    };
    // Everything the window is given, before any view has it: what breaks
    // a tap, and a click outside the bar.
    let root = ui.root();
    {
        let (state, tap) = (state.clone(), tap.clone());
        let keep = (following, leaving);
        ui.on_document_event(root, move |event| {
            let _ = &keep;
            match event {
                InputEvent::KeyDown { key, modifiers, .. } => {
                    tap.borrow_mut().interrupt();
                    state.switched.set(false);
                    if *modifiers == Modifiers::default() && matches!(key, Key::ArrowLeft | Key::ArrowRight) {
                        state.arrow_in_menu(*key == Key::ArrowRight);
                    }
                }
                InputEvent::Text(_) | InputEvent::Scroll { .. } | InputEvent::Pinch { .. } => tap.borrow_mut().interrupt(),
                InputEvent::PointerDown { x, y, .. } => {
                    tap.borrow_mut().interrupt();
                    if state.shown.with_untracked(|v| *v)
                        && let Some(ui) = state.ui.upgrade()
                    {
                        let hit = ui.input.pointer_target(&ui.scene.borrow(), *x, *y);
                        if !hit.is_some_and(|hit| state.holds(&ui, hit)) {
                            state.hide(false);
                        }
                    }
                }
                _ => {}
            }
        });
    }
    // Its keys, as they come back up from the bar unhandled: Escape, Left
    // and Right between menus, Down to open one.
    {
        let state = state.clone();
        let binding = ui.input.listen(root, move |event| {
            if event.default_prevented() || !state.shown.with_untracked(|v| *v) {
                return;
            }
            let InputEvent::KeyDown { key, modifiers, .. } = &event.event else { return };
            if *modifiers != Modifiers::default() {
                return;
            }
            let Some(ui) = state.ui.upgrade() else { return };
            let count = state.buttons.borrow().len();
            let current = state.current(&ui);
            if event.phase == EventPhase::Capture {
                return;
            }
            if state.switched.take() {
                event.prevent_default();
                return;
            }
            match key {
                Key::Escape => {
                    state.hide(true);
                    event.prevent_default();
                }
                Key::ArrowLeft | Key::ArrowRight if count > 0 => {
                    let Some((i, open)) = current else { return };
                    let next = if *key == Key::ArrowRight { (i + 1) % count } else { (i + count - 1) % count };
                    state.go(&ui, next, open);
                    event.prevent_default();
                }
                Key::ArrowDown => {
                    if let Some((i, false)) = current {
                        state.go(&ui, i, true);
                        event.prevent_default();
                    }
                }
                _ => {}
            }
        });
        ui.on_document_event(root, move |_| {
            let _ = &binding;
        });
    }

    let dispatch: Rc<dyn Fn(MenuAction)> = Rc::new(dispatch);
    let themes = zgx::theme::Themes::of(&ui.runtime);
    let island = {
        let state = state.clone();
        switch(
            move || {
                themes.current();
                (menus.get(), themes.kind().is_dark())
            },
            move |(menus, dark), cx| {
                let ink = if dark { DARK } else { LIGHT };
                let opens: Vec<Signal<bool>> = menus.iter().map(|_| cx.state(false)).collect();
                *state.opens.borrow_mut() = opens.clone();
                *state.buttons.borrow_mut() = vec![None; menus.len()];
                state.subs.borrow_mut().clear();
                let pills = menus.into_iter().zip(opens).enumerate().map(|(i, (m, open))| top(cx, &state, i, m, open, ink, &dispatch)).collect::<Vec<_>>();
                let opacity = state.opacity.clone();
                let (mounted, laid) = (state.clone(), state.clone());
                row()
                    .id("host-menu-bar")
                    .role(Role::Toolbar)
                    .accessible_label("Menu bar")
                    .pointer_events(true)
                    .items_center()
                    .gap(ISLAND_GAP)
                    .p(ISLAND_PAD)
                    .rounded(ISLAND_RADIUS)
                    .border(1.)
                    .border_color(ink.subtle)
                    .bg(ink.solid)
                    .shadow(ink.island_shadow)
                    .reactive_style(move || zgx::style::Styles::new().opacity(opacity.get()))
                    .on_mount(move |handle| {
                        mounted.island.set(Some(handle.node()));
                        super::host_bar::keep_on_top(handle.clone());
                        let laid = laid.clone();
                        laid.focus_first_now();
                        handle.on_layout(move || laid.focus_first_now());
                    })
                    .children(pills)
            },
        )
    };
    let shown = state.shown.clone();
    let view = component(move |_| {
        modal("Menu bar", shown)
            .layer()
            .autofocus(false)
            // Over the title bar's island: as far in from the window's top
            // left (`zeron.island.inset`).
            .modal_placement(ModalPlacement { x: PlacementEdge::Start, y: PlacementEdge::Start, insets: [ISLAND_INSET, 0., 0., ISLAND_INSET] })
            .p(0.)
            .bg(Color(0, 0, 0, 0))
            .border(0.)
            .shadow_none()
            .overflow_visible()
            .pointer_events(false)
            .child(island)
    });
    (view, state)
}

/// A top-level menu: its pill in the island, and its panel.
fn top(cx: &mut Context, state: &Rc<State>, i: usize, m: AppMenu, open: Signal<bool>, ink: Ink, dispatch: &Rc<dyn Fn(MenuAction)>) -> View {
    let lit = open.clone();
    let toggle = open.clone();
    let mounted = state.clone();
    let anchor = zgx::compose::button()
        .child(words(m.label.clone(), 13., 18., ink.text).font_weight(500))
        .accessible_label(m.label.clone())
        .h(CONTROL)
        .px(10.)
        .items_center()
        .justify_center()
        .rounded(ISLAND_RADIUS - ISLAND_PAD)
        .border(0.)
        .cursor(Cursor::Pointer)
        .reactive_style(move || if lit.get() { zgx::style::Styles::new().bg(ink.pressed) } else { zgx::style::Styles::new() })
        .hover(|s| s.bg(ink.hover))
        .active(|s| s.bg(ink.pressed))
        .focus(|s| s.bg(ink.hover).outline_none())
        .on_mount(move |handle| {
            if let Some(slot) = mounted.buttons.borrow_mut().get_mut(i) {
                *slot = Some(handle.node());
            }
        })
        .on_click(move || {
            toggle.set(!toggle.with_untracked(|v| *v));
        });
    panel(menu(m.label, open, anchor), ink).children(entries(cx, state, m.entries, ink, dispatch))
}

/// A menu's panel: ds's menus' glass (solid), corner, edge and shadow.
fn panel(view: View, ink: Ink) -> View {
    view.min_w(MENU_WIDTH)
        .p(PANEL_PAD)
        .gap(0.)
        .rounded(PANEL_RADIUS)
        .border(1.)
        .border_color(ink.edge)
        .bg(ink.solid)
        .shadow(ink.menu_shadow)
}

fn entries(cx: &mut Context, state: &Rc<State>, entries: Vec<AppMenuEntry>, ink: Ink, dispatch: &Rc<dyn Fn(MenuAction)>) -> Vec<View> {
    entries
        .into_iter()
        .map(|entry| match entry {
            AppMenuEntry::Item { label, action, enabled, checked, accelerator } => {
                let shortcut = accelerator.map(|a| a.label()).filter(|l| !l.is_empty());
                item(state, &label, checked, shortcut, enabled, action, ink, dispatch)
            }
            AppMenuEntry::Standard(standard) => item(state, standard.label(), None, None, true, standard.action(), ink, dispatch),
            AppMenuEntry::Submenu(m) => {
                let open = cx.state(false);
                let node = Rc::new(Cell::new(None));
                state.subs.borrow_mut().push((node.clone(), open.clone()));
                let trigger = row_look(menu_item(m.label.clone()), ink)
                    .on_mount(move |handle| node.set(Some(handle.node())))
                    .child(words(m.label.clone(), 13., 18., ink.text).grow().min_w(0.))
                    .child(words("›", 13., 18., ink.faint));
                let inner = self::entries(cx, state, m.entries, ink, dispatch);
                panel(submenu(m.label, open).trigger(trigger), ink).children(inner)
            }
            AppMenuEntry::Separator => column().h(1.).mx(PANEL_PAD).my(PANEL_PAD).bg(ink.edge),
        })
        .collect()
}

/// A row: ds's menu row (`ds/menu.luau` `look`), lit as the pointer or the
/// keyboard is on it.
fn row_look(view: View, ink: Ink) -> View {
    view.w_full()
        .h(ROW)
        .px(8.)
        .gap(8.)
        .items_center()
        .rounded(ROW_RADIUS)
        .border(0.)
        .bg(Color(0, 0, 0, 0))
        .cursor(Cursor::Pointer)
        .hover(|s| s.bg(ink.hover))
        .active(|s| s.bg(ink.pressed))
        .focus(|s| s.bg(ink.hover).outline_none())
}

#[allow(clippy::too_many_arguments)]
fn item(state: &Rc<State>, label: &str, checked: Option<bool>, shortcut: Option<String>, enabled: bool, action: MenuAction, ink: Ink, dispatch: &Rc<dyn Fn(MenuAction)>) -> View {
    let ink_text = if enabled { ink.text } else { ink.faint };
    let mut content = row().grow().items_center().gap(8.);
    if let Some(checked) = checked {
        content = content.child(words(if checked { "✓" } else { "" }, 13., 18., ink.muted).w(14.).shrink_0());
    }
    content = content.child(words(label, 13., 18., ink_text).grow().min_w(0.));
    if let Some(shortcut) = shortcut {
        content = content.child(words(shortcut, 12., 16., ink.faint).shrink_0().pl(16.));
    }
    let mut view = row_look(menu_item(label), ink).disabled(!enabled).child(content);
    if let Some(checked) = checked {
        view = view.menu_checked(checked);
    }
    let (state, dispatch) = (state.clone(), dispatch.clone());
    view.on_click(move || {
        // Away first, focus back where it was: the action goes to what
        // had it (Copy, the editor's).
        state.hide(true);
        dispatch(action.clone());
    })
}

fn words(value: impl Into<String>, size: f32, line: f32, color: Color) -> View {
    text(value).font_family(FontFamily::from("Geist")).text_size(size).line_height(line).text_color(color).text_wrap(false)
}

// ── Look ────────────────────────────────────────────────────────────────────

/// The title bar's island (ds/tokens.luau `zeron.island`): its corner,
/// its inset from the window's edges, its padding, its controls' height
/// and the gap between them.
const ISLAND_RADIUS: f32 = 12.;
const ISLAND_INSET: f32 = 6.;
const ISLAND_PAD: f32 = 4.;
const CONTROL: f32 = 24.;
const ISLAND_GAP: f32 = 2.;
/// ds's menus (`zeron.menu`, ds/menu.luau, ds/context_menu.luau): a
/// panel's width, corner (`radius.lg`) and padding (`space.1`); a row's
/// height and its corner, nested in the panel's.
const MENU_WIDTH: f32 = 232.;
const PANEL_RADIUS: f32 = 8.;
const PANEL_PAD: f32 = 4.;
const ROW: f32 = 28.;
const ROW_RADIUS: f32 = PANEL_RADIUS - 1. - PANEL_PAD;

/// The bar's colours in a theme, from ui/ds/themes (zeron-dark.json,
/// zeron-light.json): the menus' solid fill, the edges, text and washes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ink {
    solid: Color,
    /// `border` (a menu's edge, a separator) and `border.subtle` (the island's).
    edge: Color,
    subtle: Color,
    island_shadow: BoxShadow,
    menu_shadow: BoxShadow,
    text: Color,
    muted: Color,
    faint: Color,
    hover: Color,
    pressed: Color,
}

const DARK: Ink = Ink {
    solid: Color(0x1c, 0x1c, 0x1f, 0xff),
    edge: Color(0xff, 0xff, 0xff, 0x12),
    subtle: Color(0xff, 0xff, 0xff, 0x0a),
    // shadow.sm: 0 1 2 0 #00000059
    island_shadow: BoxShadow { color: Color(0, 0, 0, 0x59), offset: Transform { x: 0., y: 1. }, blur_radius: 2., spread: 0. },
    // zeron.menu.shadow: 0 20 56 0 #00000052 (CSS blur, twice the sigma)
    menu_shadow: BoxShadow { color: Color(0, 0, 0, 0x52), offset: Transform { x: 0., y: 20. }, blur_radius: 28., spread: 0. },
    text: Color(0xed, 0xed, 0xef, 0xff),
    muted: Color(0xed, 0xed, 0xef, 153),
    faint: Color(0xed, 0xed, 0xef, 107),
    hover: Color(0xff, 0xff, 0xff, 0x0d),
    pressed: Color(0xff, 0xff, 0xff, 0x17),
};

const LIGHT: Ink = Ink {
    solid: Color(0xfb, 0xfb, 0xfc, 0xff),
    edge: Color(0, 0, 0, 0x24),
    subtle: Color(0, 0, 0, 0x17),
    // shadow.sm: 0 1 2 0 #0000000f
    island_shadow: BoxShadow { color: Color(0, 0, 0, 0x0f), offset: Transform { x: 0., y: 1. }, blur_radius: 2., spread: 0. },
    // zeron.menu.shadow: 0 18 48 0 #0000001c
    menu_shadow: BoxShadow { color: Color(0, 0, 0, 0x1c), offset: Transform { x: 0., y: 18. }, blur_radius: 24., spread: 0. },
    text: Color(0x1b, 0x1b, 0x1d, 0xff),
    muted: Color(0x1b, 0x1b, 0x1d, 163),
    faint: Color(0x1b, 0x1b, 0x1d, 115),
    hover: Color(0, 0, 0, 0x0a),
    pressed: Color(0, 0, 0, 0x12),
};

#[cfg(test)]
mod tests;
