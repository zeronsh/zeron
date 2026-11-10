//! What a plugin may do in zgx, from what it was allowed: a manifest's
//! permissions mapped onto zgx's capabilities, and what Zeron shows plugins
//! of itself, named once.
//!
//! Plugins place their nodes by selector, anywhere (`elements` names what's
//! there); what they read of Zeron comes as records the host keeps for each
//! plugin, holding only what it was allowed (`records`). Every plugin is also
//! granted `plugin:<id>`, which opens its own calls, messages, records and
//! actions (`plugin.<id>.*`) to it alone: zgx's host actions don't know which
//! plugin called, so each plugin's are named for it.

use zgx_script::Capability;

/// The capability adding anywhere takes ("ui.add"): every contribution
/// before or after an element asks it.
pub const UI_ADD: &str = "zeron.ui.add";

/// Adding into a slot whose place gives what only "files.read" reads:
/// a plugin allowed both "ui.add" and "files.read".
pub const UI_ADD_FILES: &str = "zeron.ui.add.files";

/// Those slots: Git's, whose places give a checkout's folder, its remote
/// and its commits' authors (`git_branch`, `git_commit`).
pub const FILES_SLOTS: &[&str] = &["git.branch", "git.commit", "git.commit.face", "git.commit.head"];

/// What the app shows plugins to read and follow, and the capability each
/// needs (none: every plugin). The app writes them (screens/plugins);
/// plugins can't.
pub const SHARED_READ: &[(&str, Option<&str>)] = &[
    ("zeron.route", None),
    ("zeron.appearance", None),
    // The locale the app shows (Settings → General → Language): a plugin's
    // own words follow it (the SDK's `zeron.t`).
    ("zeron.locale", None),
    // The session page's session, as an id (what a record is found by).
    ("zeron.page_session", None),
    ("zeron.profile_id", Some("zeron.profile")),
    ("zeron.session", Some("zeron.sessions")),
    ("zeron.project", Some("zeron.sessions")),
    ("zeron.device", Some("zeron.devices")),
    ("zeron.devices", Some("zeron.devices")),
    // Whether the window shows and is in front (the SDK's `zeron.window`):
    // work no one would see waits for it.
    ("zgx.window.shown", None),
    ("zgx.window.active", None),
    // Whether the system asks for no transparency (src/transparency.rs):
    // the design system's glass, lent to plugins, turns solid with it.
    ("system.reduced_transparency", None),
    ("system.popups_blur", None),
    // The window's corner radius where it follows the window's state
    // (Windows): the design system's inset cards stay concentric with it.
    ("window.corner_radius", None),
    // How much the desktop blurs behind the window's glass (src/desktop_blur.rs):
    // the design system lent to plugins reads it (ds/material), so a plugin's
    // components tint as the app's do.
    ("window.blur", None),
];

/// What plugins may change of the app's own shared state, and the grant
/// each needs: the sidebar's filter (`{ statuses, by }`, the shell's), for
/// a plugin allowed "ui.change" on the sidebar or its rows (it's one virtual
/// list, so the shell filters its sessions by this instead).
pub const SHARED_WRITE: &[(&str, &str)] = &[("sidebar.filter", "zeron.sidebar.filter")];

/// The selectors a "ui.change" plugin names to be allowed the sidebar's
/// filter.
const SIDEBAR: &[&str] = &["#sidebar", "#shell-sidebar", ".session-row", "hook:sidebar.sessions"];

/// The app's actions plugins may call, by the capability each needs.
pub const ACTIONS: &[(&str, &str)] = &[
    ("zeron.open", "zeron.navigate"),
    ("zeron.compose.insert", "zeron.compose"),
];

/// Screens no plugin may replace, whatever it was allowed: allowing
/// happens there (or Settings around it).
pub const NEVER_REPLACED: &[&str] = &["@app/screens/plugins", "@app/screens/shell/settings"];

/// Each plugin's own actions (`plugin.<id>.<name>`).
pub const OWN_ACTIONS: &[&str] = &["result", "ack", "send", "menus", "toast", "problem", "open_tab", "add_page", "github_watch", "github_call", "store_set", "setting_status", "open_url", "notify", "cost", "run", "db", "service_call", "agents", "menu_run", "compose_comment", "file", "block"];
/// Each plugin's own shared state, to read (`plugin.<id>.<name>`).
pub const OWN_SHARED: &[&str] = &["calls", "inbox", "records", "github", "store", "settings", "locales", "menu_items", "composed"];

/// The capability naming plugin `id`'s own calls and actions.
pub fn own(id: &str) -> Capability {
    Capability::Named(format!("plugin:{id}"))
}

/// Where plugin `id`'s tabs are drawn, as the SDK places them.
pub fn tab_selector(id: &str) -> String {
    format!(".plugin-tab.plugin-{id}")
}

/// Where plugin `id`'s pages are drawn, as the SDK places them.
pub fn page_selector(id: &str) -> String {
    format!(".plugin-page.plugin-{id}")
}

/// May `module` be replaced by a plugin allowed "ui.replace"? Granted as
/// that module alone (`OverrideModule`), never `*`.
pub fn replaceable(module: &str) -> bool {
    module.starts_with("@app/screens/")
        && !NEVER_REPLACED
            .iter()
            .any(|never| module == *never || module.starts_with(&format!("{never}/")))
}

/// May a plugin allowed "ui.change" be given `target`? Only an element's
/// selector, as zgx reads one: a target naming a module or everything (`*`)
/// is never given (zgx's `OverrideElement` doesn't reach modules anyway); nor
/// Zeron's plugin placeholders (`.plugin-…`: a plugin's tabs, pages and the
/// Unverified badge, each plugin's own through "panel.tabs" and
/// "pages.add" alone). The engine refuses such a manifest too
/// (`change_target_problem`); this holds whatever it lists.
pub fn change_target(target: &str) -> bool {
    let trimmed = target.trim();
    if trimmed.is_empty()
        || target.len() > 200
        || target.chars().any(char::is_control)
        // Outside quoted names: a module (`@app/…`), a node's number
        // (`@n`), everything (`*`), a row or nth match.
        || unquoted(target).any(|c| matches!(c, '@' | '*' | '[' | ']' | '~'))
        || zgx_script::driver::parse(target).is_err()
    {
        return false;
    }
    !classes(target).any(|class| class.starts_with("plugin-"))
        && last_id(target).is_none_or(|id| !UNTIL_ZGX_KEEPS_PRIVATE.contains(&id))
}

/// Temporary, emptied the day zgx keeps private controls over an
/// ancestor's replacement (its R8 (c), asked for 0.1.10): the composer and
/// the chat are meant to be changeable. The elements holding a control
/// you decide with (an agent's tool approval, Settings, sign-in in it):
/// replacing or filtering one would take the control with it. zgx keeps a `private` control out of every
/// target's reach, but not yet out of an ancestor's replacement (asked of
/// zgx: private controls kept over it and drawn above plugin layers), so
/// until then these aren't given to "ui.change". Everything else stays
/// open, these elements' insides included (adding, and changing their
/// other parts).
pub const UNTIL_ZGX_KEEPS_PRIVATE: &[&str] = &[
    "shell",
    "shell-chats",
    "shell-page",
    "shell-settings",
    "settings",
    "settings-nav",
    "settings-page",
    "chat-home",
    "chat-session",
    "chat-composer-dock",
    "chat-composer",
];

/// The id a target's last part names (`#id`, or a bare word), if any.
fn last_id(target: &str) -> Option<&str> {
    let last = target.rsplit(">>").next()?.trim();
    if let Some(rest) = last.strip_prefix('#') {
        return rest.split(|c: char| c.is_whitespace() || c == '[').next();
    }
    let word = last.split(|c: char| c.is_whitespace() || c == '[').next()?;
    let bare = !word.is_empty() && !word.starts_with(['.', '"']) && !word.starts_with("hook:") && !word.starts_with("role:");
    bare.then_some(word)
}

/// `target`'s characters outside its quoted names.
fn unquoted(target: &str) -> impl Iterator<Item = char> + '_ {
    let mut quoted = false;
    target.chars().filter(move |c| {
        if *c == '"' {
            quoted = !quoted;
            return false;
        }
        !quoted
    })
}

/// The class names a target asks for (`.name`), as zgx reads them.
fn classes(target: &str) -> impl Iterator<Item = &str> {
    let mut quoted = false;
    let mut starts = Vec::new();
    for (at, c) in target.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '.' if !quoted => starts.push(at + 1),
            _ => {}
        }
    }
    starts.into_iter().map(move |start| {
        let rest = &target[start..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '[' || c == '.')
            .unwrap_or(rest.len());
        &rest[..end]
    })
}

/// What a plugin asking `permissions` (all allowed), with these `change`
/// targets and `replace` modules, may do.
pub fn capabilities(id: &str, permissions: &[String], change: &[String], replace: &[String]) -> Vec<Capability> {
    let mut out = vec![own(id)];
    let named = |name: &str| Capability::Named(name.into());
    for permission in permissions {
        match permission.as_str() {
            "ui.add" => {
                out.push(named(UI_ADD));
                if permissions.iter().any(|p| p == "files.read") {
                    out.push(named(UI_ADD_FILES));
                }
            }
            "ui.change" => {
                out.extend(
                    change
                        .iter()
                        // A raw target may not reach what's private: zgx
                        // keeps `hook = false` out of every target's reach.
                        .filter(|t| change_target(t))
                        .map(|t| Capability::OverrideElement(t.clone())),
                );
                if change.iter().any(|t| SIDEBAR.contains(&t.as_str())) {
                    out.push(named("zeron.sidebar.filter"));
                }
            }
            "ui.replace" => out.extend(
                replace
                    .iter()
                    .filter(|m| replaceable(m))
                    .map(|m| Capability::OverrideModule(m.clone())),
            ),
            // Read through records (`records`), but these two follow what
            // Zeron keeps window-wide too.
            "profile.read" => out.push(named("zeron.profile")),
            "sessions.read" => out.push(named("zeron.sessions")),
            // Records only: what's read is the host's to give.
            "messages.read" | "account.email" | "files.read" => {}
            // Zeron's GitHub, lent by proxy: the host checks it on each ask
            // (`github`), and no token reaches the plugin.
            "github" | "github.write" => {}
            // Links go where Zeron opens links; the host checks each.
            "links.open" => {}
            // The programs its manifest names: the host checks each run
            // (`commands`).
            "commands" => {}
            // Banners go through Zeron's own, under its Settings.
            "notifications" => {}
            // The agents' usage, accounts and models: the engine checks each
            // ask against what this device allowed (`plugin.<id>.agents`).
            "usage.read" | "accounts.read" | "accounts.switch" | "accounts.pin" | "models.read" | "models.set" | "messages.send" => {}
            // Its own tabs and pages: Zeron draws a placeholder of each,
            // which only this plugin may fill (replace).
            "panel.tabs" => out.push(Capability::OverrideElement(tab_selector(id))),
            "pages.add" => out.push(Capability::OverrideElement(page_selector(id))),
            "navigate" => out.push(named("zeron.navigate")),
            "compose" => out.push(named("zeron.compose")),
            "devices" => out.push(named("zeron.devices")),
            "network" => out.push(Capability::Fetch),
            "clipboard" => out.push(Capability::Clipboard),
            "files" => out.push(Capability::Files),
            "web" => out.push(Capability::Web),
            "windows" => out.push(Capability::Windows),
            // The engine's: tools reach agents through Zeron's MCP server,
            // a native server runs beside agents.
            "tools" | "native" => {}
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_plugin_with_nothing_allowed_reaches_only_its_own() {
        assert_eq!(capabilities("focus", &[], &[], &[]), vec![own("focus")]);
    }

    #[test]
    fn permissions_map_onto_zgx_grants() {
        let got = capabilities(
            "focus",
            &s(&["ui.add", "ui.change", "sessions.read", "messages.read", "network", "tools"]),
            &s(&[".session-row", "*"]),
            &[],
        );
        assert_eq!(
            got,
            vec![
                own("focus"),
                Capability::Named("zeron.ui.add".into()),
                Capability::OverrideElement(".session-row".into()),
                Capability::Named("zeron.sidebar.filter".into()),
                Capability::Named("zeron.sessions".into()),
                Capability::Fetch,
            ]
        );
    }

    #[test]
    fn replacing_never_reaches_plugins_or_settings_and_never_everything() {
        let got = capabilities(
            "x",
            &s(&["ui.replace"]),
            &[],
            &s(&[
                "@app/screens/shell/sidebar",
                "@app/screens/plugins",
                "@app/screens/plugins/page",
                "@app/screens/shell/settings",
                "*",
                "@ds/button",
            ]),
        );
        assert_eq!(got, vec![own("x"), Capability::OverrideModule("@app/screens/shell/sidebar".into())]);
    }

    #[test]
    fn tabs_and_pages_fill_only_the_plugin_s_own_placeholders() {
        let got = capabilities("prs", &s(&["panel.tabs", "pages.add"]), &[], &[]);
        assert_eq!(
            got,
            vec![
                own("prs"),
                Capability::OverrideElement(".plugin-tab.plugin-prs".into()),
                Capability::OverrideElement(".plugin-page.plugin-prs".into()),
            ]
        );
    }

    /// R1 and R8: "ui.change" grants selectors of elements alone, never a
    /// module (`@app/…`, a zgx module override), everything, or another
    /// plugin's placeholders.
    #[test]
    fn change_grants_elements_never_modules_or_placeholders() {
        let got = capabilities(
            "x",
            &s(&["ui.change"]),
            &s(&[
                "@app/screens/plugins/state",
                " @app/screens/chat",
                "@app/ds/button",
                "*",
                "",
                "@3",
                ".row[2]",
                ".plugin-tab.plugin-github",
                ".plugin-page",
                "#panel >> .plugin-unverified",
                // (Holding what you decide with: refused only until zgx keeps
                // private controls over a replaced ancestor, R8 (c).)
                "#chat-composer",
                "#shell >> #settings-page",
                "shell",
                "#chat-session[0]",
                "#composer-toolbar",
                "hook:sidebar.sessions",
                "\"Send @ once\"",
            ]),
            &[],
        );
        assert_eq!(
            got,
            vec![
                own("x"),
                Capability::OverrideElement("#composer-toolbar".into()),
                Capability::OverrideElement("hook:sidebar.sessions".into()),
                Capability::OverrideElement("\"Send @ once\"".into()),
                Capability::Named("zeron.sidebar.filter".into()),
            ]
        );
    }

    /// R1: "ui.change" names what it changes and reaches no module, and
    /// "ui.replace" reaches the screens it names and no element: neither
    /// holds the other's power, nor zgx's older `Override`, which reads by
    /// what it names.
    #[test]
    fn change_and_replace_grant_apart() {
        let got = capabilities(
            "x",
            &s(&["ui.change", "ui.replace", "panel.tabs", "pages.add"]),
            &s(&["#composer-toolbar", "@app/screens/chat"]),
            &s(&["@app/screens/chat", "#composer-toolbar"]),
        );
        assert!(got.contains(&Capability::OverrideElement("#composer-toolbar".into())));
        assert!(got.contains(&Capability::OverrideModule("@app/screens/chat".into())));
        assert!(!got.contains(&Capability::OverrideElement("@app/screens/chat".into())));
        assert!(!got.contains(&Capability::OverrideModule("#composer-toolbar".into())));
        assert!(!got.iter().any(|c| matches!(c, Capability::Override(_))));
    }

    #[test]
    fn listed_targets_without_the_permission_give_nothing() {
        let got = capabilities("x", &[], &s(&["#titlebar-actions"]), &s(&["@app/screens/chat"]));
        assert_eq!(got, vec![own("x")]);
    }

    /// Allowing and claiming a plugin, and installing someone else's
    /// template, are the consent windows' alone: no
    /// other file in the app asks the engine to decide or sign, or names
    /// the actions that do, so nothing the main window runs (plugins and
    /// templates among it) can say yes on your behalf.
    #[test]
    fn only_the_consent_window_decides_or_signs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut found = Vec::new();
        let mut stack = vec![root.join("src"), root.join("ui")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // (Not `.zgx/`: generated types, not the app's own code.)
                    if path.file_name().is_some_and(|n| n != ".zgx") {
                        stack.push(path);
                    }
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                let allowed = matches!(
                    name.as_str(),
                    "consent.rs"
                        | "consent.luau"
                        | "consent_fixture.luau"
                        | "plugins_consent.test.luau"
                        | "grants.rs"
                        // Installing someone else's template: its own window.
                        | "template_consent.rs"
                        | "template_consent.luau"
                        | "template_consent_fixture.luau"
                        | "templates_consent.test.luau"
                );
                if allowed || !(name.ends_with(".rs") || name.ends_with(".luau")) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for word in ["DecidePlugin", "SignPlugin", "ReviewPlugin", "ReviewTemplate", "plugins.decide", "plugins.sign", "DecideTemplate", "templates.decide"] {
                    if text.contains(word) {
                        found.push(format!("{word} in {}", path.display()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "only the consent window may decide or sign: {found:?}");
    }
}
