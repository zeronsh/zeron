//! Zeron's shortcuts as plugins come and go (src/menu_bar.rs): the shell
//! with Zeron's own plugins (git, GitHub, Parallax scenes, as the bridge
//! loads them) and test plugins with shortcuts of their own, some Zeron's,
//! some each other's, some no menu can show, turned on and off, reloaded,
//! rewritten, broken and mended, reordered, with a consent sheet and
//! another window opening over the main window, in a fixed random order.
//! After each step every Zeron shortcut reaches its command, every live
//! plugin's shortcut that's its own reaches its command, and every item in
//! the menu bar runs in a live VM.

use serde_json::json;

use super::examples_tests::{builtin, shell_with, world};
use crate::menu_bar::{Role, accelerator};
use crate::menu_bar_tests::{Desktop, Window, items, plugin, scratch, unreached, zerons};

/// A test plugin's versions: each command's id and key.
const KEYS: [&[(&str, &str)]; 3] = [
    // One free, one Zeron's ⌘K, one Zeron's ⌘N by another name.
    &[("free", "cmd+shift+y"), ("palette", "cmd+k"), ("new", "ALIAS+n")],
    // One free, one no menu can show, one in a menu with no name.
    &[("other", "cmd+alt+y"), ("odd", "cmd+shift+?"), ("nameless", "cmd+shift+u")],
    // One free, one the rival's.
    &[("third", "cmd+shift+j"), ("clash", "cmd+shift+o")],
];
/// The rival's: one of keys' first version's, one of its third's.
const RIVAL: &[(&str, &str)] = &[("mine", "cmd+shift+o"), ("also", "cmd+shift+y")];
/// The flaky one's, broken (an error after declaring it) or not.
const FLAKY: &[(&str, &str)] = &[("flake", "cmd+alt+f")];

fn alias(key: &str) -> String {
    key.replace("ALIAS", if cfg!(target_os = "macos") { "meta" } else { "ctrl" })
}

/// A plugin's init.luau declaring `commands`, then failing if `broken`.
fn body(commands: &[(&str, &str)], broken: bool) -> String {
    let mut out = String::new();
    for (id, key) in commands {
        let menu = if *id == "nameless" { "" } else { "Test" };
        out.push_str(&format!("ui.command {{ id = \"{id}\", title = \"{id}\", menu = \"{menu}\", key = \"{}\", run = function() end }}\n", alias(key)));
    }
    if broken {
        out.push_str("error(\"broken on purpose\")\n");
    }
    out
}

/// A seeded xorshift, the same steps every run.
struct Steps(u64);
impl Steps {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[test]
fn zeron_s_shortcuts_survive_plugins_coming_and_going() {
    let dir = scratch("soak");
    let builtins = ["git", "github", "parallax-scenes"];
    let builtin_items: Vec<super::Item> = builtins.iter().map(|id| builtin(id)).collect();
    let (app, _heard) = shell_with(builtin_items.clone(), world(json!({ "viewer": "ada" })));
    assert_eq!(app.error(), None, "the shell with Zeron's plugins failed");
    let desktop = Desktop::new();
    let bar = desktop.bar();
    let mut w = Window::serve(&bar, app, Role::Bar);
    let expected = zerons(&w);
    assert!(expected.len() > 20, "{expected:?}");
    // Zeron's shortcuts as the platform names them.
    let zeron_names: Vec<String> = expected.iter().filter_map(|(_, k)| accelerator(k)).map(|a| a.label()).collect();

    let write = |name: &str, commands: &[(&str, &str)], broken: bool| plugin(&dir, name, &body(commands, broken));
    let mut version = 0;
    let mut broken = false;
    write("keys", KEYS[version], false);
    write("rival", RIVAL, false);
    write("flaky", FLAKY, false);
    // What's on, in the order it was turned on (zgx loads them so, and the
    // one loaded first keeps a key two ask for).
    let mut on: Vec<&'static str> = builtins.to_vec();
    let all = ["git", "github", "parallax-scenes", "keys", "rival", "flaky"];
    let set = |w: &mut Window, on: &[&str]| {
        let plugins: Vec<zgx_script::Plugin> = on
            .iter()
            .map(|id| match builtin_items.iter().find(|i| i.id() == *id) {
                Some(item) => item.plugin().expect("a built-in loads"),
                None => zgx_script::Plugin { name: (*id).into(), entry: dir.join(id).join("init.luau"), grants: vec![] },
            })
            .collect();
        w.app.host.set_plugins(plugins);
        // (A reload that fails keeps the window as it was, as the app's.)
        let _ = w.app.host.reload();
        w.app.frames(5);
    };
    set(&mut w, &on);

    // The flaky one failed and is held out (the bridge's `failed_plugins`)
    // until a new version of it comes.
    let mut held = false;
    let live = |on: &[&'static str], held: bool| -> Vec<&'static str> { on.iter().copied().filter(|id| !(held && *id == "flaky")).collect() };
    // What the bridge loads (`apply`): what's on, less one held out; one
    // that fails as it loads is withdrawn and held out, its commands with
    // it (src/plugins `blame` → `plugin_failed`).
    let load = |w: &mut Window, on: &[&'static str], held: &mut bool, broken: bool| {
        set(w, &live(on, *held));
        if broken && !*held && on.contains(&"flaky") {
            *held = true;
            set(w, &live(on, *held));
        }
    };
    let mut steps = Steps(0x5eed_2026_1103);
    let mut log: Vec<String> = Vec::new();
    for step in 0..150 {
        let what = match steps.below(7) {
            0 => {
                let id = all[steps.below(all.len())];
                match on.iter().position(|o| *o == id) {
                    Some(at) => {
                        on.remove(at);
                    }
                    None => on.push(id),
                }
                load(&mut w, &on, &mut held, broken);
                format!("toggled {id}")
            }
            1 => {
                let _ = w.app.host.reload();
                w.app.frames(5);
                "reloaded".into()
            }
            2 => {
                version = (version + 1 + steps.below(2)) % KEYS.len();
                write("keys", KEYS[version], false);
                load(&mut w, &on, &mut held, broken);
                format!("keys rewritten (v{version})")
            }
            3 => {
                // A new version of it, broken or mended: loaded again.
                broken = !broken;
                write("flaky", FLAKY, broken);
                held = false;
                load(&mut w, &on, &mut held, broken);
                format!("flaky {}", if broken { "broke" } else { "mended" })
            }
            4 => {
                let sheet = Window::open(&bar, "consent.luau", Role::Sheet, |host| {
                    host.action("appearance", |_| {});
                    host
                });
                let over = unreached(&mut w, &desktop, &expected);
                assert!(over.is_empty(), "step {step}, a consent sheet open:\n{}\n(after {log:?})", over.join("\n"));
                drop(sheet);
                "a consent sheet came and went".into()
            }
            5 => {
                let path = dir.join("over.luau");
                std::fs::write(&path, "ui.command { id = \"over\", title = \"Over\", key = \"cmd+k\", run = function() end }\nreturn ui.column {}\n").unwrap();
                let over = Window::open(&bar, path.to_str().unwrap(), Role::Bar, |host| host);
                assert_eq!(desktop.action_of("cmd+k").as_deref(), Some("over"), "step {step}: the window over it has the bar");
                drop(over);
                "a window with a bar came and went".into()
            }
            _ => {
                if on.len() > 1 {
                    let first = on.remove(0);
                    on.push(first);
                }
                load(&mut w, &on, &mut held, broken);
                format!("reordered: {on:?}")
            }
        };
        log.push(format!("{step}: {what}"));
        let at = || log[log.len().saturating_sub(6)..].join("; ");

        assert!(desktop.refusals.borrow().is_empty(), "step {step}: a bar refused: {:?} ({})", desktop.refusals.borrow(), at());
        let wrong = unreached(&mut w, &desktop, &expected);
        assert!(wrong.is_empty(), "step {step}, Zeron's:\n{}\n({})", wrong.join("\n"), at());
        assert!(w.app.host.run_command("shell.toggle_sidebar") && w.app.host.run_command("shell.toggle_sidebar"), "step {step}: Zeron's commands run");

        // The live test plugins' own keys: the first to ask has each, and
        // never one of Zeron's or one no menu can name.
        let mut taken = zeron_names.clone();
        let mut theirs: Vec<(String, String)> = Vec::new();
        let loaded = live(&on, held);
        for id in &loaded {
            let commands: &[(&str, &str)] = match *id {
                "keys" => KEYS[version],
                "rival" => RIVAL,
                "flaky" if !broken => FLAKY,
                _ => continue,
            };
            for (cmd, key) in commands {
                let key = alias(key);
                let Some(name) = accelerator(&key).map(|a| a.label()).filter(|n| !n.is_empty()) else { continue };
                if taken.contains(&name) {
                    continue;
                }
                taken.push(name);
                theirs.push((format!("{id}/{cmd}"), key));
            }
        }
        let wrong = unreached(&mut w, &desktop, &theirs);
        assert!(wrong.is_empty(), "step {step}, the plugins':\n{}\n({})", wrong.join("\n"), at());

        // Every item in the bar runs, in a live VM; none of a plugin that's off.
        for (_, action, _) in items(&desktop.menus.borrow()) {
            if action.starts_with(crate::menu_bar::HOST_COMMANDS) || !action.contains('/') {
                continue;
            }
            let plugin = action.split('/').next().unwrap_or_default();
            assert!(loaded.contains(&plugin), "step {step}: {action} of {plugin}, which isn't loaded ({})", at());
            assert!(w.app.host.run_command(&action), "step {step}: {action} is in the menu bar but runs nothing ({})", at());
        }
    }
    // (Every kind of step ran.)
    for kind in ["toggled", "reloaded", "rewritten", "flaky", "consent sheet", "window with a bar", "reordered"] {
        assert!(log.iter().any(|l| l.contains(kind)), "no step {kind}: {log:?}");
    }
}
