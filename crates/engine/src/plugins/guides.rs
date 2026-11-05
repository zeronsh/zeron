//! The authoring guides agents read: how to write a plugin, a template and
//! a theme (apps/zgx/docs/plugins.md, templates.md, themes.md), kept in the engine so every
//! device has them, written into `<data>/docs/` as the tools that need them
//! answer, and linked from each plugin's and template's README.

use std::path::{Path, PathBuf};

pub const PLUGINS: &str = include_str!("../../../../apps/zgx/docs/plugins.md");
pub const TEMPLATES: &str = include_str!("../../../../apps/zgx/docs/templates.md");
pub const THEMES: &str = include_str!("../../../../apps/zgx/docs/themes.md");

/// Where the guides are written, in the data dir.
pub const DIR: &str = "docs";
/// A plugin's or template's own README (never part of a template's app).
pub const README: &str = "README.md";

/// What each guide covers, said in the replies that link it.
pub const PLUGINS_SUMMARY: &str = "How to write a Zeron plugin: the manifest and permissions, placing by selector (zeron.place, the stable element names and what each provides), context (zeron.context, need, live; records per grant), actions, tools for agents, commands, services (service.luau run headless on each device, zeron.service(device):call from windows, failure reasons), the loop (create_plugin, edit, plugin_status with snapshot, fix) and three worked examples.";
pub const TEMPLATES_SUMMARY: &str = "How to write a Zeron template: how it's laid over Zeron's own with per-file fallback, the folder map (app.luau, screens/, ds/ tokens and themes), the rules (tokens, concentric corners, data in place, reloads, keeping element names), what's never templated, the loop (create_template, use_template, edit, template_status) and two worked examples.";

/// Both guides in `<data_dir>/docs/`, written where missing or changed (an
/// update brings new ones): their paths, plugins' then templates'.
pub fn ensure(data_dir: &Path) -> (PathBuf, PathBuf) {
    let dir = data_dir.join(DIR);
    let _ = std::fs::create_dir_all(&dir);
    let write = |name: &str, text: &str| {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
            let _ = std::fs::write(&path, text);
        }
        path
    };
    (write("plugins.md", PLUGINS), write("templates.md", TEMPLATES))
}

/// The themes guide in `<data_dir>/docs/`, written where missing or
/// changed: its path.
pub fn themes(data_dir: &Path) -> PathBuf {
    let dir = data_dir.join(DIR);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("themes.md");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(THEMES) {
        let _ = std::fs::write(&path, THEMES);
    }
    path
}

/// A new plugin's README: what it is and where the guide is.
pub fn plugin_readme(name: &str, guide: &Path) -> String {
    format!(
        "# {name}\n\nA Zeron plugin. `init.luau` is its code, `zeron-plugin.json` its manifest (permissions, what it may change, its tools) and `zeron.luau` Zeron's helpers. Zeron re-signs and reloads it each time you save.\n\nHow to write one: {guide}\n\nCheck your work with the `plugin_status` tool (`snapshot: true` for a picture of where it shows), and `describe_elements` for where it can go.\n",
        guide = guide.display()
    )
}

/// A new template's README: what it is and where the guide is.
pub fn template_readme(name: &str, guide: &Path) -> String {
    format!(
        "# {name}\n\nA Zeron template: a whole interface, laid over Zeron's own. Any file here replaces Zeron's; delete one to inherit Zeron's. This README isn't part of the interface.\n\nHow to write one: {guide}\n\nUse it with `use_template`, check it with `template_status`, and see what each screen is with `describe_app`.\n",
        guide = guide.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_guides_are_written_once_and_linked() {
        let data = std::env::temp_dir().join(format!("zeron-guides-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let (plugins, templates) = ensure(&data);
        assert_eq!(std::fs::read_to_string(&plugins).unwrap(), PLUGINS);
        assert_eq!(std::fs::read_to_string(&templates).unwrap(), TEMPLATES);
        assert!(PLUGINS.contains("zeron.place") && TEMPLATES.contains("template_status"));
        assert!(plugin_readme("Focus", &plugins).contains(&plugins.display().to_string()));
        assert!(template_readme("Dusk", &templates).contains("isn't part of the interface"));
        let _ = std::fs::remove_dir_all(&data);
    }
}
