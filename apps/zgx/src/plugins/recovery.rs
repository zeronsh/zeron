//! When a plugin fails, Zeron goes on as if it weren't there, at once and
//! with the window's state kept: no placeholder, no broken screen. (A
//! template's files are blamed in `live`, where it's known which of the
//! window's files are the template's.)
//!
//! Every script error reaches the host (`ScriptHost::on_error`), and says
//! where it came from: a plugin's own VM's errors are named for it (`<id>:
//! runtime error: …`: its init, a contribution's build, a filter, an
//! effect), and a module it replaced, run as the app's, names a file in its
//! snapshot. The plugin is withdrawn, its contributions, filters and
//! replacements with it, until a new version or an edit of it arrives.
//!
//! Each failure is told once (`Failures::fail` answers whether it's new).

use std::collections::HashMap;

use zgx_script::Plugin;

/// The plugin `error` is on, of those loaded, if any: the VM it names
/// first (a plugin's own), else the file it's at, exactly (a module a
/// plugin replaced, run as the app's), never words anywhere in it (one
/// plugin's message naming another's snapshot once had that one withdrawn).
pub fn blame(error: &str, plugins: &[Plugin]) -> Option<String> {
    // Its own VM's error: named for it, first thing.
    if let Some(plugin) = plugins.iter().find(|p| error.starts_with(&format!("{}: ", p.name))) {
        return Some(plugin.name.clone());
    }
    // A file of its snapshot, run as the app's: where the error is.
    let at = std::path::Path::new(located(error)?);
    plugins
        .iter()
        .find(|p| p.entry.parent().is_some_and(|dir| within(at, dir)))
        .map(|p| p.name.clone())
}

/// The file an app's error is at: after its VM's preamble (`app.luau:
/// runtime error: `), up to `.luau:<line>`.
fn located(error: &str) -> Option<&str> {
    let line = error.lines().next()?;
    let rest = line.split_once(" error: ").map_or(line, |(_, rest)| rest);
    let end = rest.find(".luau:")? + ".luau".len();
    rest[end + 1..].starts_with(|c: char| c.is_ascii_digit()).then(|| &rest[..end])
}

/// Whether `file` is in `dir`, by whole parts (`/var` and `/private/var`
/// are one folder on macOS).
fn within(file: &std::path::Path, dir: &std::path::Path) -> bool {
    let bare = |p: &std::path::Path| p.strip_prefix("/private").map(|rest| std::path::Path::new("/").join(rest)).unwrap_or_else(|_| p.to_path_buf());
    bare(file).starts_with(bare(dir))
}

/// What failed, by name, with what it was when it failed (its file's print,
/// a plugin's hash) and why: told once, tried again once it changes.
#[derive(Default)]
pub struct Failures {
    by: HashMap<String, (String, String)>,
}

impl Failures {
    /// `what` failed as of `version`: whether that's news (to be told).
    pub fn fail(&mut self, what: &str, version: &str, why: &str) -> bool {
        match self.by.get(what) {
            Some((v, _)) if v == version => false,
            _ => {
                self.by.insert(what.into(), (version.into(), first_line(why)));
                true
            }
        }
    }

    /// Whether `what`, as of `version`, stays out. One that has changed
    /// since it failed is forgotten, to be tried again.
    pub fn holds(&mut self, what: &str, version: &str) -> bool {
        match self.by.get(what) {
            Some((v, _)) if v == version => true,
            Some(_) => {
                self.by.remove(what);
                false
            }
            None => false,
        }
    }

    /// `what` is gone (removed): a new one of that name starts afresh.
    pub fn forget(&mut self, what: &str) {
        self.by.remove(what);
    }

    pub fn why(&self, what: &str) -> Option<&str> {
        self.by.get(what).map(|(_, why)| why.as_str())
    }
}

/// An error's first line, without the VM's preamble and the copy's path: what
/// a person reads ("sidebar.luau:12: attempt to index nil").
pub fn first_line(error: &str) -> String {
    let line = error.lines().next().unwrap_or(error);
    let line = line.split(": runtime error: ").last().unwrap_or(line);
    // A path: its file name onward.
    let line = match line.find(".luau:") {
        Some(at) => {
            let start = line[..at].rfind(['/', ' ']).map(|i| i + 1).unwrap_or(0);
            &line[start..]
        }
        None => line,
    };
    line.chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plugin(name: &str, entry: &str) -> Plugin {
        Plugin { name: name.into(), entry: PathBuf::from(entry), grants: vec![] }
    }

    #[test]
    fn an_error_is_blamed_on_the_plugin_it_came_from() {
        let plugins = vec![plugin("focus", "/d/plugins/.loaded/focus/0123/init.luau")];
        assert_eq!(blame("focus: runtime error: focus:3: filter broke", &plugins).as_deref(), Some("focus"));
        let replaced = "app.luau: runtime error: /d/plugins/.loaded/focus/0123/sidebar.luau:9: nil";
        assert_eq!(blame(replaced, &plugins).as_deref(), Some("focus"));
        // The app's own, or a plugin whose name only starts another's: nobody's.
        assert_eq!(blame("app.luau: runtime error: screens/card.luau:3: oops", &plugins), None);
        assert_eq!(blame("focused: runtime error: x", &plugins), None);
        let private = "app.luau: runtime error: /private/d/plugins/.loaded/focus/0123/a.luau:2: x";
        assert_eq!(blame(private, &plugins).as_deref(), Some("focus"));
    }

    /// R26: one plugin can't get another withdrawn by naming its snapshot
    /// in an error of its own (or of a module it replaced).
    #[test]
    fn an_error_naming_another_plugin_s_files_is_never_blamed_on_it() {
        let plugins = vec![
            plugin("victim", "/d/plugins/.loaded/victim/aa/init.luau"),
            plugin("evil", "/d/plugins/.loaded/evil/bb/init.luau"),
        ];
        let own = "evil: runtime error: /d/plugins/.loaded/victim/aa/init.luau:1: framed";
        assert_eq!(blame(own, &plugins).as_deref(), Some("evil"));
        let replaced = "app.luau: runtime error: /d/plugins/.loaded/evil/bb/x.luau:3: /d/plugins/.loaded/victim/aa/init.luau:1: framed";
        assert_eq!(blame(replaced, &plugins).as_deref(), Some("evil"));
        let app = "app.luau: runtime error: screens/x.luau:3: see /d/plugins/.loaded/victim/aa/init.luau:1";
        assert_eq!(blame(app, &plugins), None);
        // (A folder whose name only starts another's isn't in it.)
        let near = "app.luau: runtime error: /d/plugins/.loaded/victim/aaa/x.luau:3: x";
        assert_eq!(blame(near, &plugins), None);
    }

    #[test]
    fn a_failure_is_told_once_and_tried_again_once_it_changes() {
        let mut f = Failures::default();
        assert!(f.fail("focus", "h1", "focus: runtime error: focus:3: filter broke"));
        assert!(!f.fail("focus", "h1", "focus: runtime error: focus:3: filter broke"));
        assert!(f.holds("focus", "h1"));
        assert_eq!(f.why("focus"), Some("focus:3: filter broke"));
        // A new version: tried again, and told again if it fails again.
        assert!(!f.holds("focus", "h2"));
        assert!(f.fail("focus", "h2", "again"));
    }

    #[test]
    fn what_a_person_reads_of_an_error() {
        assert_eq!(
            first_line("app.luau: runtime error: /d/templates/.loaded/ab/screens/shell/sidebar.luau:12: attempt to index nil\nstack traceback:\n…"),
            "sidebar.luau:12: attempt to index nil"
        );
        assert_eq!(first_line("focus: runtime error: focus:3: filter broke"), "focus:3: filter broke");
    }
}
