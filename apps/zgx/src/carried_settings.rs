//! The sidebar as you had it in the app this one replaces: the first time a
//! beta opens, what `ui-settings.json` says of the sidebar becomes its own
//! `ui.persist` values. Only what it has no value for yet, so what you've
//! changed here stays, and a value once carried is its own from then on.
//!
//! Pinned chats and sections aren't here: the engine keeps them, for either app.

use std::path::Path;

use serde_json::{Value as Json, json};

const SIDEBAR: &str = "screens%2Fshell%2Fsidebar.luau%3A%3Ashell_sidebar_";
const VIEW: &str = "screens%2Fshell%2Fview.luau%3A%3Ashell_";

/// `settings` (`ui-settings.json`) as files of `ui.persist`, each a name and
/// its value; only what it says and this app has.
fn carried(settings: &Json) -> Vec<(String, Json)> {
    let mut out = Vec::new();
    let sidebar = |name: &str| format!("{SIDEBAR}{name}.json");
    for (from, to) in [
        ("sidebarCompact", "compact"),
        ("sidebarShowProjectIcon", "show_project_icon"),
        ("sidebarShowHarness", "show_harness"),
        ("sidebarShowBranch", "show_branch"),
        ("sidebarShowPullRequest", "show_pr"),
        ("sidebarShowProjectFilter", "show_project_filter"),
        ("sidebarShowArchived", "show_archived"),
    ] {
        if let Some(on) = settings[from].as_bool() {
            out.push((sidebar(to), json!(on)));
        }
    }
    if let Some(width) = settings["sidebarWidth"].as_f64() {
        out.push((sidebar("width"), json!(width)));
    }
    match settings["sidebarSort"].as_str() {
        Some("lastUpdated") => out.push((sidebar("sort"), json!("updated"))),
        Some("created") => out.push((sidebar("sort"), json!("created"))),
        _ => {}
    }
    match settings["sidebarOrganization"].as_str() {
        Some("byProject") => out.push((sidebar("organize"), json!(["project"]))),
        Some("byDevice") => out.push((sidebar("organize"), json!(["device"]))),
        Some("inOneList") => out.push((sidebar("organize"), json!([]))),
        _ => {}
    }
    if let Some(shut) = settings["sidebarCollapsed"].as_bool() {
        out.push((format!("{VIEW}collapsed.json"), json!(shut)));
    }
    out
}

/// Writes into `state` (the app window's `ui.persist` folder) what `settings`
/// says and `state` has no value for. A setting that can't be written is left
/// at its default.
fn carry_into(state: &Path, settings: &Json) {
    let wanted = carried(settings);
    if wanted.is_empty() || std::fs::create_dir_all(state).is_err() {
        return;
    }
    for (name, value) in wanted {
        let file = state.join(name);
        if !file.exists() {
            let _ = std::fs::write(file, value.to_string());
        }
    }
}

/// Before the main window's scripts first read them.
pub fn carry(ui_settings: Option<&Path>, state: Option<&Path>) {
    let (Some(from), Some(state)) = (ui_settings, state) else { return };
    let Some(settings) = std::fs::read(from).ok().and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok()) else {
        return;
    };
    carry_into(state, &settings);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(state: &Path, name: &str) -> Json {
        serde_json::from_slice(&std::fs::read(state.join(format!("{SIDEBAR}{name}.json"))).unwrap()).unwrap()
    }

    #[test]
    fn the_sidebar_comes_across_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let settings = json!({
            "sidebarOrganization": "byDevice", "sidebarSort": "created", "sidebarCompact": false,
            "sidebarShowHarness": false, "sidebarShowPullRequest": false, "sidebarWidth": 293.5,
            "sidebarCollapsed": true,
        });
        carry_into(dir.path(), &settings);
        assert_eq!(read(dir.path(), "organize"), json!(["device"]));
        assert_eq!(read(dir.path(), "sort"), json!("created"));
        assert_eq!(read(dir.path(), "compact"), json!(false));
        assert_eq!(read(dir.path(), "show_harness"), json!(false));
        assert_eq!(read(dir.path(), "show_pr"), json!(false));
        assert_eq!(read(dir.path(), "width"), json!(293.5));
        assert!(dir.path().join(format!("{VIEW}collapsed.json")).exists());
        // What it doesn't say stays at this app's default.
        assert!(!dir.path().join(format!("{SIDEBAR}show_branch.json")).exists());
    }

    #[test]
    fn what_you_changed_here_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join(format!("{SIDEBAR}sort.json"));
        std::fs::write(&mine, "\"updated\"").unwrap();
        carry_into(dir.path(), &json!({ "sidebarSort": "created", "sidebarCompact": false }));
        assert_eq!(read(dir.path(), "sort"), json!("updated"));
        assert_eq!(read(dir.path(), "compact"), json!(false));
    }

    #[test]
    fn no_file_or_a_broken_one_carries_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        carry(Some(&dir.path().join("missing.json")), Some(&state));
        assert!(!state.exists());
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{").unwrap();
        carry(Some(&broken), Some(&state));
        assert!(!state.exists());
    }
}
