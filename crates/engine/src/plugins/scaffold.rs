//! A new plugin's files (contract "Scaffold").

use std::path::Path;

use super::manifest::MANIFEST_FILE;

/// The plugin SDK, kept by the app side.
pub const SDK: &str = include_str!("../../../../apps/zgx/plugins-sdk/zeron.luau");
/// A new plugin's settings page of its own, kept by the app side too (its
/// tests draw it): `PLUGIN_ID` and `PLUGIN_NAME` in it are the plugin's.
pub const SETTINGS_PAGE: &str = include_str!("../../../../apps/zgx/plugins-sdk/settings_page.luau");
/// Its words (`locales/en.json`'s, beside the plugin's own).
pub const SETTINGS_PAGE_WORDS: &str = include_str!("../../../../apps/zgx/plugins-sdk/settings_page.en.json");

pub fn init_luau(id: &str, name: &str) -> String {
    format!(
        r#"local zeron = require("./zeron")("{id}")

-- {name}
--
-- Zeron loads this file when the plugin is on, and reloads it as you save.
-- zeron.luau's header (next to this file) lists what each helper does and
-- which permission in zeron-plugin.json it needs. This plugin asks for none
-- yet: add the ones you use there.
--
-- For example, with "ui.add":
--
--   zeron.menu {{ kind = "session", id = "hello", label = zeron.s("hello.label"),
--     run = function(session) zeron.toast(zeron.s("hello.toast")) end }}
--
-- Its words are in locales/: en.json, and a file per language beside it
-- (zh-Hans.json, …). zeron.t(key) shows one in Zeron's language as it
-- changes, zeron.s(key) gives it as text now; a language or a key the
-- plugin lacks shows its English.
"#
    )
}

/// The plugin's English words (`locales/en.json`), what `zeron.t` shows;
/// with a settings page of its own, that page's too.
pub fn english(name: &str, settings_page: bool) -> serde_json::Value {
    let mut words = serde_json::json!({
        "hello": {
            "label": "Say hello",
            "toast": format!("Hello from {name}"),
        }
    });
    if settings_page {
        // (Our own file, included as built: one that doesn't parse is a bug
        // in the tree, never an input.)
        let page: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(SETTINGS_PAGE_WORDS).expect("plugins-sdk/settings_page.en.json is an object of words");
        for (k, v) in page {
            words[k] = v;
        }
    }
    words
}

/// A settings page of its own (`settings_page.luau`), built as Zeron's pages
/// are: a section, its card of rows, an action at its title's end, an empty
/// state in the card's place, every word in `locales/`. What the guide's
/// "Settings pages" says, ready to change.
pub fn settings_page_luau(id: &str, name: &str) -> String {
    SETTINGS_PAGE.replace("PLUGIN_ID", id).replace("PLUGIN_NAME", name)
}

pub fn manifest(
    id: &str,
    name: &str,
    description: &str,
    permissions: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "version": "0.1.0",
        "description": description,
        "entry": "init.luau",
        "permissions": permissions,
    })
}

/// Write the folder's files (not yet signed). Fails if the folder exists.
/// With `settings_page`, a settings page of its own too (`settings_page.luau`,
/// required from `init.luau`, its words in `locales/en.json`).
pub fn write(
    dir: &Path,
    id: &str,
    name: &str,
    description: &str,
    permissions: &[String],
    settings_page: bool,
) -> Result<(), String> {
    if dir.exists() {
        return Err(format!("There's already a plugin called {id}"));
    }
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut init = init_luau(id, name);
        if settings_page {
            init.push_str("\n-- Its settings page, after the rows its manifest declares.\nrequire(\"./settings_page\")\n");
            std::fs::write(dir.join("settings_page.luau"), settings_page_luau(id, name))?;
        }
        std::fs::write(dir.join("init.luau"), init)?;
        std::fs::write(dir.join("zeron.luau"), SDK)?;
        std::fs::create_dir_all(dir.join("locales"))?;
        std::fs::write(
            dir.join("locales").join("en.json"),
            super::signing::pretty(&english(name, settings_page)),
        )?;
        std::fs::write(
            dir.join(MANIFEST_FILE),
            super::signing::pretty(&manifest(id, name, description, permissions)),
        )
    })();
    result.map_err(|_| {
        let _ = std::fs::remove_dir_all(dir);
        format!("Couldn't make the folder for {id}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_may_start_with_a_settings_page_of_its_own() {
        let root = std::env::temp_dir().join(format!("zeron-scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bare, paged) = (root.join("bare"), root.join("paged"));
        write(&bare, "bare", "Bare", "", &[], false).unwrap();
        write(&paged, "ledger", "Ledger", "Counts things.", &[], true).unwrap();
        // Without: as before, no page.
        assert!(!bare.join("settings_page.luau").exists());
        assert!(!std::fs::read_to_string(bare.join("init.luau")).unwrap().contains("settings_page"));
        let words: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(bare.join("locales/en.json")).unwrap()).unwrap();
        assert!(words.get("settings").is_none());
        // With: its page, required from init.luau, named for it, its words.
        let page = std::fs::read_to_string(paged.join("settings_page.luau")).unwrap();
        assert!(page.contains("require(\"./zeron\")(\"ledger\")") && page.contains("Ledger's settings page"));
        assert!(!page.contains("PLUGIN_"), "every placeholder filled");
        assert!(std::fs::read_to_string(paged.join("init.luau")).unwrap().contains("require(\"./settings_page\")"));
        let words: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(paged.join("locales/en.json")).unwrap()).unwrap();
        assert_eq!(words["settings"]["items"]["title"], "Items");
        assert_eq!(words["hello"]["label"], "Say hello", "its own words kept");
        // Every word the page says is there.
        for key in ["settings.items.title", "settings.items.description", "settings.items.add", "settings.items.new", "settings.items.remove", "settings.items.empty.title", "settings.items.empty.body", "settings.items.empty.action"] {
            assert!(page.contains(&format!("\"{key}\"")), "{key} is said");
            assert!(key.split('.').try_fold(&words, |at, k| at.get(k)).is_some_and(|v| v.is_string()), "{key} has words");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
