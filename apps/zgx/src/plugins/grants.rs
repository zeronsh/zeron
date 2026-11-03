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
pub const OWN_ACTIONS: &[&str] = &["result", "ack", "send", "menus", "toast", "problem", "open_tab", "add_page", "github_watch", "github_call", "store_set", "setting_status", "open_url", "notify", "cost", "run", "db", "service_call", "menu_run", "compose_comment", "notes_watch", "notes_set"];
/// Each plugin's own shared state, to read (`plugin.<id>.<name>`).
pub const OWN_SHARED: &[&str] = &["calls", "inbox", "records", "github", "store", "settings", "locales", "menu_items", "composed", "notes"];

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

/// May `module` be replaced by a plugin allowed "ui.replace"?
pub fn replaceable(module: &str) -> bool {
    module.starts_with("@app/screens/")
        && !NEVER_REPLACED
            .iter()
            .any(|never| module == *never || module.starts_with(&format!("{never}/")))
}

/// What a plugin asking `permissions` (all allowed), with these `change`
/// targets and `replace` modules, may do.
pub fn capabilities(id: &str, permissions: &[String], change: &[String], replace: &[String]) -> Vec<Capability> {
    let mut out = vec![own(id)];
    let named = |name: &str| Capability::Named(name.into());
    for permission in permissions {
        match permission.as_str() {
            "ui.add" => out.push(named(UI_ADD)),
            "ui.change" => {
                out.extend(
                    change
                        .iter()
                        // A raw target may not reach what's private: zgx
                        // keeps `hook = false` out of every target's reach.
                        .filter(|t| !t.is_empty() && t.as_str() != "*")
                        .map(|t| Capability::Override(t.clone())),
                );
                if change.iter().any(|t| SIDEBAR.contains(&t.as_str())) {
                    out.push(named("zeron.sidebar.filter"));
                }
            }
            "ui.replace" => out.extend(
                replace
                    .iter()
                    .filter(|m| replaceable(m))
                    .map(|m| Capability::Override(m.clone())),
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
            // The notes of the chat it's shown in, and no other's: the host
            // checks each ask (`notes`), through the plugin's own names.
            "notes" => {}
            // Its own tabs and pages: Zeron draws a placeholder of each,
            // which only this plugin may fill (replace).
            "panel.tabs" => out.push(Capability::Override(tab_selector(id))),
            "pages.add" => out.push(Capability::Override(page_selector(id))),
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
                Capability::Override(".session-row".into()),
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
        assert_eq!(got, vec![own("x"), Capability::Override("@app/screens/shell/sidebar".into())]);
    }

    #[test]
    fn tabs_and_pages_fill_only_the_plugin_s_own_placeholders() {
        let got = capabilities("prs", &s(&["panel.tabs", "pages.add"]), &[], &[]);
        assert_eq!(
            got,
            vec![
                own("prs"),
                Capability::Override(".plugin-tab.plugin-prs".into()),
                Capability::Override(".plugin-page.plugin-prs".into()),
            ]
        );
    }

    /// "notes" opens nothing of zgx's: no element, shared state or action
    /// of the app's own. What it lets a plugin do is the host's to check on
    /// each ask, through `plugin.<id>.notes*`, which are the plugin's own
    /// names (src/plugins/notes.rs; plugins/notes_tests.rs plays it).
    #[test]
    fn notes_is_checked_by_the_host_alone() {
        assert_eq!(capabilities("notes", &s(&["notes"]), &[], &[]), vec![own("notes")]);
        for name in ["notes_watch", "notes_set"] {
            assert!(OWN_ACTIONS.contains(&name), "{name}");
        }
        assert!(OWN_SHARED.contains(&"notes"));
        assert!(!SHARED_READ.iter().any(|(k, _)| k.contains("notes")));
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
                for word in ["DecidePlugin", "SignPlugin", "plugins.decide", "plugins.sign", "DecideTemplate", "templates.decide"] {
                    if text.contains(word) {
                        found.push(format!("{word} in {}", path.display()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "only the consent window may decide or sign: {found:?}");
    }
}
