//! The built-in themes Zeron once installed as templates, retired.
//!
//! Until themes were chosen apart from templates (src/themes.rs), each of
//! Zeron's built-in themes was written as a colour-only template under
//! `<data>/templates/theme-<key>/`: its manifest, marked with the theme's key
//! (`builtin_theme`), and a `theme.luau` palette, nothing else. Only one
//! template runs at a time, so a template of yours and a theme couldn't both
//! be worn. Now they're written no more, and each launch (`retire`):
//!
//! - moves anyone running one to the default template, with that theme
//!   chosen for light and dark (unless they've chosen themes already);
//! - deletes each such folder, but only one shaped exactly as Zeron wrote it
//!   (`written_by_zeron`): a folder that isn't marked, holds anything more
//!   (screens, an imported package's record), is under another id or says
//!   another author is never touched, whatever it says.

use std::path::Path;

use serde_json::Value as Json;

use super::live;

/// The manifest field that marked a folder as a built-in theme's (its key).
const MARK: &str = "builtin_theme";
/// The file the theme's palette was in.
const FILE: &str = "theme.luau";

fn read_json(path: &Path) -> Json {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The built-in theme key folder `id` in `templates` held, if Zeron wrote
/// it: a real folder at `theme-<key>` (or `theme-<key>-N`, where yours had
/// that id) holding exactly its manifest and `theme.luau`, both regular
/// files; the manifest marked with the key and by Zeron; the palette data,
/// not code.
pub fn written_by_zeron(templates: &Path, id: &str) -> Option<String> {
    let dir = templates.join(id);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // (`live::put`'s temporary files, mid-write.)
        if name.starts_with('.') && name.ends_with(".tmp") {
            continue;
        }
        if !entry.file_type().is_ok_and(|k| k.is_file()) {
            return None;
        }
        names.push(name);
    }
    names.sort();
    if names != [FILE, "zeron-template.json"] {
        return None;
    }
    let manifest = read_json(&dir.join("zeron-template.json"));
    let key = manifest[MARK].as_str().filter(|k| !k.is_empty())?;
    let base = format!("theme-{key}");
    let at_its_id = id == base || id.strip_prefix(&format!("{base}-")).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    let palette = std::fs::read_to_string(dir.join(FILE)).ok().and_then(|t| zeron_theme::luau_data::parse(&t));
    (at_its_id && manifest["author"] == "Zeron" && palette.is_some()).then(|| key.to_string())
}

/// What `retire` did: the theme the window was moved off (its key), and the
/// folders it deleted.
#[derive(Debug, Default, PartialEq)]
pub struct Retired {
    pub moved_from: Option<String>,
    pub removed: Vec<String>,
}

/// The retired theme templates in `templates` gone (see the module's doc),
/// one in use first swapped for the default template with its theme chosen
/// (in `data_dir`, where src/themes.rs keeps the choice).
pub fn retire(templates: &Path, data_dir: &Path) -> Retired {
    let mut out = Retired::default();
    let active = live::chosen(templates);
    if let Some(key) = written_by_zeron(templates, &active) {
        if crate::themes::chosen(data_dir).is_empty()
            && let Some(theme) = crate::themes::find_builtin(&key)
            && let Err(error) = crate::themes::save_choice(data_dir, &crate::themes::choice_of(&theme))
        {
            eprintln!("zeron-zgx: the theme {key} wasn't kept: {error}");
        }
        match live::set_active(templates, live::BUILTIN) {
            Ok(_) => out.moved_from = Some(key),
            // (Still in use: it stays, to try again next launch.)
            Err(error) => {
                eprintln!("zeron-zgx: the theme template {active} is still in use: {error}");
                return out;
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(templates) else { return out };
    let mut ids: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|id| id.starts_with("theme-")).collect();
    ids.sort();
    for id in ids {
        if written_by_zeron(templates, &id).is_some() {
            match std::fs::remove_dir_all(templates.join(&id)) {
                Ok(()) => out.removed.push(id),
                Err(error) => eprintln!("zeron-zgx: the theme template {id} wasn't removed: {error}"),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::{Choice, chosen, save_choice};
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-retired-themes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A theme template as an earlier Zeron wrote it.
    fn zeron_s(templates: &Path, id: &str, key: &str) {
        write(
            templates,
            &format!("{id}/zeron-template.json"),
            &format!(r#"{{ "id": "{id}", "name": "{key}", "description": "Its colours on Zeron's interface.", "author": "Zeron", "version": "0.2.131", "zeron_version": "0.2.131", "builtin_theme": "{key}" }}"#),
        );
        write(templates, &format!("{id}/theme.luau"), "-- Its colours.\nreturn {\n\tname = \"Theme\",\n\tdark = { background = \"#282828\" },\n}\n");
    }

    #[test]
    fn running_one_moves_you_to_the_default_with_its_theme_chosen() {
        let d = dir("moved");
        let t = d.join("templates");
        zeron_s(&t, "theme-gruvbox", "gruvbox");
        live::set_active(&t, "theme-gruvbox").unwrap();
        let retired = retire(&t, &d);
        assert_eq!(retired, Retired { moved_from: Some("gruvbox".into()), removed: vec!["theme-gruvbox".into()] });
        assert_eq!(live::chosen(&t), live::BUILTIN);
        assert_eq!(chosen(&d), Choice { light: Some("gruvbox-light".into()), dark: Some("gruvbox-dark".into()) });
        assert!(!t.join("theme-gruvbox").exists());
        // Again: nothing more to do.
        assert_eq!(retire(&t, &d), Retired::default());
    }

    #[test]
    fn a_choice_of_yours_is_kept_and_a_single_variant_theme_fills_its_slot() {
        let d = dir("kept");
        let t = d.join("templates");
        zeron_s(&t, "theme-tokyo-night", "tokyo-night");
        live::set_active(&t, "theme-tokyo-night").unwrap();
        let mine = Choice { light: Some("github-light".into()), dark: None };
        save_choice(&d, &mine).unwrap();
        retire(&t, &d);
        assert_eq!(chosen(&d), mine, "what you chose stands");
        assert_eq!(live::chosen(&t), live::BUILTIN);
        // An earlier build's single-variant theme, under its second id.
        let d = dir("single");
        let t = d.join("templates");
        zeron_s(&t, "theme-dracula-2", "dracula");
        live::set_active(&t, "theme-dracula-2").unwrap();
        retire(&t, &d);
        assert_eq!(chosen(&d), Choice { light: None, dark: Some("dracula".into()) });
    }

    #[test]
    fn only_a_folder_shaped_as_zeron_wrote_it_is_removed() {
        let d = dir("only");
        let t = d.join("templates");
        zeron_s(&t, "theme-ayu", "ayu");
        zeron_s(&t, "theme-ayu-2", "ayu");
        // Unmarked, at a theme's id: yours.
        write(&t, "theme-nord/zeron-template.json", r#"{"name":"Nord","author":"you"}"#);
        write(&t, "theme-nord/theme.luau", "return nil");
        // Marked, with screens of its own.
        zeron_s(&t, "theme-catppuccin", "catppuccin");
        write(&t, "theme-catppuccin/screens/shell/sidebar.luau", "-- takes over");
        // Marked, an imported package's.
        zeron_s(&t, "theme-github", "github");
        write(&t, "theme-github/.imported.json", r#"{"key":"k","author":{"kind":"unknown"}}"#);
        // Marked, but another author's, another id, or a palette that's code.
        zeron_s(&t, "theme-rose-pine", "rose-pine");
        write(&t, "theme-rose-pine/zeron-template.json", r#"{"name":"Rosé","author":"someone","builtin_theme":"rose-pine"}"#);
        zeron_s(&t, "my-dracula", "dracula");
        zeron_s(&t, "theme-night-owl", "night-owl");
        write(&t, "theme-night-owl/theme.luau", "ui.call('terminal.start', {}) return nil");
        // Marked with another theme's key than its id says.
        zeron_s(&t, "theme-nord-dark", "gruvbox");
        // Yours in use: it stays in use.
        write(&t, "airy/zeron-template.json", r#"{"name":"Airy"}"#);
        live::set_active(&t, "airy").unwrap();
        let retired = retire(&t, &d);
        assert_eq!(retired.removed, vec!["theme-ayu".to_string(), "theme-ayu-2".to_string()]);
        assert_eq!(retired.moved_from, None);
        for kept in ["theme-nord", "theme-catppuccin", "theme-github", "theme-rose-pine", "my-dracula", "theme-night-owl", "theme-nord-dark", "airy"] {
            assert!(t.join(kept).is_dir(), "{kept} is never touched");
        }
        assert_eq!(std::fs::read_to_string(t.join("theme-catppuccin/screens/shell/sidebar.luau")).unwrap(), "-- takes over");
        assert_eq!(live::chosen(&t), "airy");
        assert!(chosen(&d).is_empty());
    }

    #[test]
    fn one_of_yours_running_under_a_theme_s_id_isn_t_moved() {
        let d = dir("yours");
        let t = d.join("templates");
        write(&t, "theme-gruvbox/zeron-template.json", r#"{"name":"Gruvbox","author":"you"}"#);
        write(&t, "theme-gruvbox/screens/x.luau", "-- mine");
        live::set_active(&t, "theme-gruvbox").unwrap();
        assert_eq!(retire(&t, &d), Retired::default());
        assert_eq!(live::chosen(&t), "theme-gruvbox");
    }
}
