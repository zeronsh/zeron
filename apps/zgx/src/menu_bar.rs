//! The app's menu bar: the scripts' commands (`ui.command`) as menu items,
//! each running its command, its shortcut the item's key equivalent. A
//! shortcut reaches the app only that way (headless tests run commands by
//! id), so this bar *is* the app's keyboard: ⌘K, ⌘N, ⌘W and the rest.
//!
//! Two things about zgx-desktop's `set_menus` shape this module:
//!
//! - **It is the whole application's.** One call replaces the bar, and the
//!   keymap bound at every window's root, for every window. So one window
//!   has the bar at a time: the windows that have one (the main window,
//!   setup, the intro, the gallery) stack up, the last opened on top, and
//!   its commands are the bar. A sheet in a window of its own (a plugin's
//!   or a template's consent, src/plugins/consent.rs) never touches it, and
//!   a window closing puts back the bar of the one under it. (Before Nov 3
//!   every window set its own: the consent sheet a plugin asks for as it's
//!   added set a bar with none of Zeron's commands, and when it closed
//!   nothing set the main window's back. Every shortcut was gone until a
//!   relaunch: src/menu_bar_tests.rs.)
//! - **It refuses a whole bar for one bad item**, keeping the last: a
//!   label or action empty, an action twice, a shortcut a menu can't name
//!   (`⌘?`, `⌘é`) or one named twice. So the bar is made to be one it
//!   takes, item by item (`Bar::of`): Zeron's commands first, then the
//!   plugins', each command once, each shortcut once (as the menu names
//!   it: `meta+k` is `cmd+k` on a Mac). Zeron's always keep theirs; a
//!   plugin's that is taken, or no key at all, is left off that one item,
//!   which stays in the menu, and its plugin's row on the Plugins page says
//!   so (`Dropped`). Should the platform refuse a bar anyway, Zeron's own
//!   goes in, never nothing, and it says so loudly.
//!
//! Each change makes the whole bar again from the one window's commands and
//! sets it at once, on the main thread: there is no partial bar to race.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::{Rc, Weak},
};

use zgx::reactive::{Effect, Runtime, Signal};
use zgx_desktop::{AppMenu, AppMenuEntry, AppMenuError, MenuAccelerator, WindowFactory};
use zgx_script::{Command, DroppedShortcut, ScriptHost};

use crate::plugins::{self, preview::Preview};

/// Where a bar goes: zgx-desktop's application (every window's), or a
/// test's stand-in.
pub trait Platform {
    /// Replace every window's menu bar with `menus`, or refuse it whole.
    fn set_menus(&self, menus: Vec<AppMenu>) -> Result<(), AppMenuError>;
    /// The shortcuts a focused web page or terminal never has first.
    fn set_reserved_shortcuts(&self, keys: Vec<MenuAccelerator>) -> Result<(), AppMenuError>;
}

impl Platform for WindowFactory {
    fn set_menus(&self, menus: Vec<AppMenu>) -> Result<(), AppMenuError> {
        WindowFactory::set_menus(self, menus)
    }
    fn set_reserved_shortcuts(&self, keys: Vec<MenuAccelerator>) -> Result<(), AppMenuError> {
        WindowFactory::set_reserved_shortcuts(self, keys)
    }
}

/// A window's part in the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Its commands are the bar while it's the last opened that has one.
    Bar,
    /// A sheet in a window of its own: the bar stays the one under it.
    Sheet,
}

/// The host's own command ids: no script's command may have one.
pub const HOST_COMMANDS: &str = "zeron.";

/// "Use Zeron v3"'s shortcut, the host's whatever a script declares:
/// ⌘⌥⇧Home on a Mac, Ctrl+Alt+Shift+Home on Linux and Windows, where
/// there's no menu bar to reach the item from and a template draws the
/// palette. (Home for the default; no letter, which AltGr+Shift types on
/// some layouts, and no F key, which Ctrl+Alt switches consoles with on
/// Linux.) Reserved too, so a focused web page or terminal never has it.
pub const DEFAULT_INTERFACE_KEY: &str = "cmd+alt+shift+home";

/// The host's own item that switches to the default interface.
const USE_ZERON_TITLE: &str = "Use Zeron v3";

/// A plugin's command (zgx names each `<plugin>/<id>`; Zeron's have no `/`).
pub fn is_plugins(command: &Command) -> bool {
    command.id.contains('/')
}

/// A plugin command shown without the shortcut it asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dropped {
    /// The plugin's id.
    pub plugin: String,
    /// The shortcut as it wrote it (`cmd+j`).
    pub key: String,
    /// The command's title.
    pub title: String,
    /// Why it has none.
    pub why: Why,
}

/// Why a plugin command has no shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// It's Zeron's: the title of Zeron's command with it.
    Zerons(String),
    /// Another plugin (its id) had it first.
    Plugins(String),
    /// It isn't a key a menu can show (`cmd+é`, `cmd+?`, a misspelling).
    NoKey,
    /// A key alone, from a state or function (zgx refuses one written
    /// plainly): it would be taken from everything you type in.
    NoModifier,
}

/// One window's menu bar, made to be one the platform takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    /// Zeron's commands and the plugins'.
    pub menus: Vec<AppMenu>,
    /// Zeron's alone: what goes in should the platform refuse `menus`.
    pub own: Vec<AppMenu>,
    /// The reserved commands' shortcuts (Zeron's alone reserve).
    pub reserved: Vec<MenuAccelerator>,
    /// The plugin commands shown without their shortcut, and why.
    pub dropped: Vec<Dropped>,
}

impl Bar {
    /// The bar of `commands` (published, `reserved` the reserved ones'
    /// shortcuts), with what zgx already took off plugins' commands
    /// (`dropped`, `ScriptHost::dropped_shortcuts`) and the template in
    /// preview, if any.
    pub fn of(commands: &[Command], reserved: &[String], dropped: &[DroppedShortcut], preview: Option<&Preview>) -> Bar {
        let mut menus = Menus::new();
        for (at, command) in commands.iter().enumerate().filter(|(_, c)| !is_plugins(c)) {
            menus.add(at, command);
        }
        let own = menus.clone().finish(preview);
        for (at, command) in commands.iter().enumerate().filter(|(_, c)| is_plugins(c)) {
            menus.add(at, command);
        }
        // What zgx took off before the list reached here: a key Zeron's (or
        // an earlier plugin's) commands have, spelled the same.
        let find = |id: &str| commands.iter().position(|c| c.id == id);
        for d in dropped.iter().filter(|d| !d.plugin.is_empty()) {
            let why = match d.holder.split_once('/') {
                // (Nothing holds it: zgx refused it for what it is.)
                _ if d.holder.is_empty() => Why::NoModifier,
                Some((plugin, _)) => Why::Plugins(plugin.to_string()),
                None => Why::Zerons(find(&d.holder).map_or_else(|| d.holder.clone(), |at| commands[at].title.clone())),
            };
            let at = find(&d.command).unwrap_or(usize::MAX);
            let title = commands.get(at).map(|c| c.title.clone()).unwrap_or_default();
            menus.dropped.push((at, Dropped { plugin: d.plugin.clone(), key: d.key.clone(), title, why }));
        }
        let mut reserved: Vec<MenuAccelerator> = reserved
            .iter()
            .filter_map(|key| accelerator(key))
            .filter(|key| menus.keys.get(&key.label()).is_some_and(|holder| !holder.contains('/')))
            .collect();
        if let Some(key) = accelerator(DEFAULT_INTERFACE_KEY)
            && !reserved.contains(&key)
        {
            reserved.push(key);
        }
        // In the order they're declared.
        let mut dropped = std::mem::take(&mut menus.dropped);
        dropped.sort_by_key(|(at, _)| *at);
        let dropped = dropped.into_iter().map(|(_, d)| d).collect();
        Bar { menus: menus.finish(preview), own, reserved, dropped }
    }

    /// The host's own items alone: the last resort.
    fn host_only(preview: Option<&Preview>) -> Vec<AppMenu> {
        Menus::new().finish(preview)
    }
}

/// A bar being made: each item checked as zgx-desktop's `set_menus` checks
/// a whole bar, so a bad one is left out (or its shortcut is) instead of
/// the bar refused.
#[derive(Clone, Default)]
struct Menus {
    menus: Vec<AppMenu>,
    /// Actions in the bar so far (no script's is `zeron.`: the host's own,
    /// added by `finish`, are unique by that).
    ids: HashSet<String>,
    /// Shortcuts in the bar so far, as the platform names them (its
    /// uniqueness check): whose command each is.
    keys: HashMap<String, String>,
    /// Plugin commands left without their shortcut, by where each is
    /// declared.
    dropped: Vec<(usize, Dropped)>,
}

impl Menus {
    /// None yet, the host's shortcut held (`DEFAULT_INTERFACE_KEY`): a
    /// script's command declaring it goes without.
    fn new() -> Self {
        let mut menus = Menus::default();
        if let Some(key) = accelerator(DEFAULT_INTERFACE_KEY) {
            menus.keys.insert(key.label(), plugins::USE_ZERON_TEMPLATE.to_string());
        }
        menus
    }

    /// `command`, the `at`th declared, as an item.
    fn add(&mut self, at: usize, command: &Command) {
        let menu = clean(&command.menu);
        let menu = if menu.is_empty() { "File".to_string() } else { menu };
        if command.separator {
            if let Some(m) = self.menus.iter_mut().find(|m| m.label == menu)
                && m.entries.last().is_some_and(|e| *e != AppMenuEntry::Separator)
            {
                m.entries.push(AppMenuEntry::Separator);
            }
            return;
        }
        let title = clean(&command.title);
        if command.id.is_empty() || command.id.contains('\0') || title.is_empty() {
            eprintln!("zeron-zgx: menus: `{}` has no id or title a menu can show; left out", command.id);
            return;
        }
        // `zeron.` is the host's: a script's (a template's) command by such
        // an id would be a second item with the host's action.
        if command.id.starts_with(HOST_COMMANDS) {
            eprintln!("zeron-zgx: menus: {} is the host's; the script's is left out", command.id);
            return;
        }
        if !self.ids.insert(command.id.clone()) {
            eprintln!("zeron-zgx: menus: {} is declared twice; shown once", command.id);
            return;
        }
        let mut item = AppMenuEntry::item(title.clone(), command.id.as_str());
        if let Some(key) = command.key.as_deref().filter(|k| !k.is_empty()) {
            match self.claim(command, key) {
                Ok(accelerator) => item = item.accelerator(accelerator),
                Err(why) => {
                    eprintln!("zeron-zgx: menus: {}'s shortcut `{key}` left off ({why:?})", command.id);
                    if let Some((plugin, _)) = command.id.split_once('/') {
                        self.dropped.push((at, Dropped { plugin: plugin.to_string(), key: key.to_string(), title, why }));
                    }
                }
            }
        }
        match self.menus.iter_mut().find(|m| m.label == menu) {
            Some(m) => m.entries.push(item),
            None => self.menus.push(AppMenu::new(menu, [item])),
        }
    }

    /// `key` for `command`, if it's one a menu can show and no command
    /// before it has it.
    fn claim(&mut self, command: &Command, key: &str) -> Result<MenuAccelerator, Why> {
        let accelerator = accelerator(key).ok_or(Why::NoKey)?;
        // The platform's name for it (`Super+Shift+K`), empty for a key it
        // can't name: what it compares, and refuses a bar for.
        let name = accelerator.label();
        if name.is_empty() {
            return Err(Why::NoKey);
        }
        if let Some(holder) = self.keys.get(&name) {
            return Err(match holder.split_once('/') {
                Some((plugin, _)) => Why::Plugins(plugin.to_string()),
                None => Why::Zerons(self.title_of(holder)),
            });
        }
        self.keys.insert(name, command.id.clone());
        Ok(accelerator)
    }

    fn title_of(&self, id: &str) -> String {
        if id == plugins::USE_ZERON_TEMPLATE {
            return USE_ZERON_TITLE.to_string();
        }
        for menu in &self.menus {
            for entry in &menu.entries {
                if let AppMenuEntry::Item { label, action, .. } = entry
                    && action.0 == id
                {
                    return label.clone();
                }
            }
        }
        id.to_string()
    }

    /// The bar, with the host's own after the scripts' View items:
    /// "Use Zeron v3" (the default's way back), which no template can take
    /// away, and, with a template in preview, "Keep <name>…" (its ellipsis
    /// the macOS one for an item that asks before acting: it opens the keep
    /// sheet).
    fn finish(mut self, preview: Option<&Preview>) -> Vec<AppMenu> {
        let view = match self.menus.iter().position(|m| m.label == "View") {
            Some(index) => index,
            None => {
                self.menus.push(AppMenu::new("View", []));
                self.menus.len() - 1
            }
        };
        let entries = &mut self.menus[view].entries;
        if entries.last().is_some_and(|e| *e != AppMenuEntry::Separator) {
            entries.push(AppMenuEntry::Separator);
        }
        if let Some(preview) = preview {
            entries.push(AppMenuEntry::item(format!("Keep {}…", clean(&preview.name)), plugins::KEEP_TEMPLATE));
        }
        let mut back = AppMenuEntry::item(USE_ZERON_TITLE, plugins::USE_ZERON_TEMPLATE);
        if let Some(key) = accelerator(DEFAULT_INTERFACE_KEY) {
            back = back.accelerator(key);
        }
        entries.push(back);
        self.menus.retain(|m| !m.entries.is_empty());
        self.menus
    }
}

/// `text` without the NULs a menu refuses, trimmed.
fn clean(text: &str) -> String {
    text.replace('\0', "").trim().to_string()
}

/// `cmd+shift+d`, `cmd+alt+arrowleft`, `cmd+[`, `cmd+,` as a menu's
/// accelerator: `cmd` is Command on macOS, Control elsewhere. None for one
/// it can't read (it says which).
pub fn accelerator(text: &str) -> Option<MenuAccelerator> {
    use zgx::input::{Key, Modifiers};
    let mut modifiers = Modifiers::default();
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let key = parts.pop()?.to_ascii_lowercase();
    for part in parts {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" if cfg!(target_os = "macos") => modifiers.meta = true,
            "cmd" | "command" => modifiers.control = true,
            "super" | "meta" => modifiers.meta = true,
            "ctrl" | "control" => modifiers.control = true,
            "alt" | "opt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            other => {
                eprintln!("zeron-zgx: menus: unknown modifier `{other}` in `{text}`");
                return None;
            }
        }
    }
    let key = match key.as_str() {
        "arrowleft" | "left" => Key::ArrowLeft,
        "arrowright" | "right" => Key::ArrowRight,
        "arrowup" | "up" => Key::ArrowUp,
        "arrowdown" | "down" => Key::ArrowDown,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "space" => Key::Space,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        single if single.chars().count() == 1 => Key::Character(single.into()),
        other => {
            eprintln!("zeron-zgx: menus: unknown key `{other}` in `{text}`");
            return None;
        }
    };
    Some(MenuAccelerator::new(key, modifiers))
}

/// The app's one menu bar, over the windows that have one.
pub struct MenuBar {
    platform: Box<dyn Platform>,
    /// The windows with a bar, the last opened last, and the bar each has
    /// made of its commands so far: the last one's shows.
    owners: RefCell<Vec<(u64, Option<Bar>)>>,
    next: Cell<u64>,
    /// What the platform was given last (the same bar isn't given twice).
    installed: RefCell<Option<Bar>>,
}

/// A window's part in the bar: kept while the window lives. Dropped, the
/// bar is the next window's.
pub struct Served {
    // (Its effect ends before it gives the bar up.)
    _follow: Option<Effect>,
    _owner: Option<Owner>,
}

struct Owner {
    bar: Weak<MenuBar>,
    id: u64,
}

impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(bar) = self.bar.upgrade() {
            bar.withdraw(self.id);
        }
    }
}

thread_local! {
    static APP: RefCell<Option<Rc<MenuBar>>> = const { RefCell::new(None) };
}

/// The application's bar (`windows`' app: there is one).
pub fn of(windows: &WindowFactory) -> Rc<MenuBar> {
    APP.with(|app| app.borrow_mut().get_or_insert_with(|| MenuBar::new(windows.clone())).clone())
}

impl MenuBar {
    pub fn new(platform: impl Platform + 'static) -> Rc<Self> {
        Rc::new(MenuBar { platform: Box::new(platform), owners: RefCell::default(), next: Cell::new(0), installed: RefCell::default() })
    }

    /// `host`'s window in the bar as `role`: as a `Bar`, its commands are
    /// the bar from now (and as they change, `preview` too) until a window
    /// opened later has it or this one's `Served` is dropped.
    pub fn serve(self: &Rc<Self>, runtime: &Runtime, host: &ScriptHost, preview: Signal<Option<Preview>>, role: Role) -> Served {
        if role == Role::Sheet {
            return Served { _follow: None, _owner: None };
        }
        let id = self.next.get();
        self.next.set(id + 1);
        self.owners.borrow_mut().push((id, None));
        let owner = Owner { bar: Rc::downgrade(self), id };
        let (commands, reserved, me, host) = (host.commands(), host.reserved_keys(), Rc::downgrade(self), host.clone());
        let follow = runtime.effect(move || {
            let commands = commands.get();
            let reserved = reserved.get();
            let bar = preview.with(|preview| Bar::of(&commands, &reserved, &host.dropped_shortcuts(), preview.as_ref()));
            if let Some(me) = me.upgrade() {
                me.offer(id, bar);
            }
        });
        Served { _follow: Some(follow), _owner: Some(owner) }
    }

    /// Window `id`'s bar is now `bar`: set if it's the one shown.
    fn offer(&self, id: u64, bar: Bar) {
        let top = {
            let mut owners = self.owners.borrow_mut();
            let Some(at) = owners.iter().position(|(o, _)| *o == id) else { return };
            owners[at].1 = Some(bar);
            at + 1 == owners.len()
        };
        if top {
            self.install_top();
        }
    }

    /// Window `id` closed: if its bar showed, the one under it's does.
    fn withdraw(&self, id: u64) {
        let was_top = {
            let mut owners = self.owners.borrow_mut();
            let Some(at) = owners.iter().position(|(o, _)| *o == id) else { return };
            owners.remove(at);
            at == owners.len()
        };
        if was_top {
            self.install_top();
        }
    }

    /// The last opened window's bar to the platform, unless it has it.
    fn install_top(&self) {
        let Some(bar) = self.owners.borrow().iter().rev().find_map(|(_, bar)| bar.clone()) else { return };
        if self.installed.borrow().as_ref() == Some(&bar) {
            return;
        }
        if let Err(error) = self.platform.set_menus(bar.menus.clone()) {
            // Never a bar without Zeron's shortcuts: `Bar::of` made one the
            // platform should take, so this is a bug to find.
            eprintln!("zeron-zgx: MENU BAR REFUSED: {error}. Zeron's own commands alone instead; plugins' are out of the menu bar until the next change.");
            if let Err(error) = self.platform.set_menus(bar.own.clone()) {
                eprintln!("zeron-zgx: MENU BAR REFUSED AGAIN (Zeron's own): {error}. The host's items alone.");
                if let Err(error) = self.platform.set_menus(Bar::host_only(None)) {
                    eprintln!("zeron-zgx: menus: {error}");
                }
            }
        }
        if let Err(error) = self.platform.set_reserved_shortcuts(bar.reserved.clone()) {
            eprintln!("zeron-zgx: reserved shortcuts: {error}");
        }
        // A plugin whose shortcut is left off says so on its Plugins row.
        plugins::shortcuts_taken(bar.dropped.clone());
        *self.installed.borrow_mut() = Some(bar);
    }
}
