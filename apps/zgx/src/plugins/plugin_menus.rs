//! Menus a plugin offers other plugins. Zeron's own menus take plugins'
//! items by kind (`zeron.menu { kind = "session" | "file", … }`); a plugin
//! can offer menus of its own the same way, named under its id (the Git
//! plugin's `git.commit`, a History row's), and other plugins add items to
//! them with the same call (`zeron.menu { kind = "git.commit", … }`, "ui.add").
//!
//! The host keeps every plugin's items and publishes to each plugin only
//! those for its own menus (`plugin.<id>.menu_items`). Choosing one, the
//! offering plugin asks the host (`plugin.<id>.menu_run`), which runs it in
//! the plugin that added it (a `menu` call, as Zeron's menus do), given the
//! context the offering plugin says: plain data, never a function. A plugin
//! can run items only in its own menus.

use zgx_script::Value;

use crate::auth::text;

/// A menu's kind: one of Zeron's (`session`, `file`), or a plugin's
/// (`<plugin id>.<name>`).
pub fn valid_kind(kind: &str) -> bool {
    if kind == "session" || kind == "file" {
        return true;
    }
    let Some((plugin, name)) = kind.split_once('.') else { return false };
    let id_ok = (2..=40).contains(&plugin.len()) && plugin.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    let name_ok = (1..=40).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.'));
    id_ok && name_ok
}

/// Whether `kind` is a menu `plugin` offers.
fn offered_by(plugin: &str, kind: &str) -> bool {
    kind.strip_prefix(plugin).is_some_and(|rest| rest.starts_with('.'))
}

/// The items for `plugin`'s own menus, from every plugin's (`all`).
pub fn offered_to(plugin: &str, all: &[Value]) -> Vec<Value> {
    all.iter().filter(|item| item.field("kind").map(text).is_some_and(|k| offered_by(plugin, &k))).cloned().collect()
}

/// Who runs `asked` (`{ plugin, id, kind }`) for `offering`: the plugin
/// that added that item to one of `offering`'s menus, or none.
pub fn route(offering: &str, asked: &Value, all: &[Value]) -> Option<(String, String)> {
    let kind = asked.field("kind").map(text)?;
    let plugin = asked.field("plugin").map(text)?;
    let id = asked.field("id").map(text)?;
    if !offered_by(offering, &kind) {
        return None;
    }
    all.iter()
        .any(|item| {
            item.field("kind").map(text).as_deref() == Some(kind.as_str())
                && item.field("plugin").map(text).as_deref() == Some(plugin.as_str())
                && item.field("id").map(text).as_deref() == Some(id.as_str())
        })
        .then_some((plugin, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(plugin: &str, kind: &str, id: &str) -> Value {
        Value::record([("plugin", Value::from(plugin)), ("kind", Value::from(kind)), ("id", Value::from(id)), ("label", Value::from("L"))])
    }

    #[test]
    fn a_plugin_offers_menus_under_its_id_and_runs_only_its_own() {
        for ok in ["session", "file", "git.commit", "git.commit.files", "my-plugin.row_menu"] {
            assert!(valid_kind(ok), "{ok}");
        }
        for bad in ["", "git", ".commit", "Git.commit", "git.", "g.commit", "git.Commit", "git.c m"] {
            assert!(!valid_kind(bad), "{bad}");
        }
        let all = vec![item("github", "git.commit", "view"), item("github", "session", "pr"), item("other", "gitx.commit", "x")];
        // Git sees only what's added to its own menus (not gitx's).
        let mine = offered_to("git", &all);
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].field("id").map(text).as_deref(), Some("view"));
        let ask = |kind: &str, plugin: &str, id: &str| Value::record([("kind", Value::from(kind)), ("plugin", Value::from(plugin)), ("id", Value::from(id))]);
        assert_eq!(route("git", &ask("git.commit", "github", "view"), &all), Some(("github".into(), "view".into())));
        // Not its menu, an item nobody added, another menu's item.
        assert_eq!(route("git", &ask("gitx.commit", "other", "x"), &all), None);
        assert_eq!(route("git", &ask("git.commit", "github", "nope"), &all), None);
        assert_eq!(route("git", &ask("session", "github", "pr"), &all), None);
    }
}
