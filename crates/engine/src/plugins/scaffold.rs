//! A new plugin's files (contract "Scaffold").

use std::path::Path;

use super::manifest::MANIFEST_FILE;

/// The plugin SDK, kept by the app side.
pub const SDK: &str = include_str!("../../../../apps/zgx/plugins-sdk/zeron.luau");

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

/// The plugin's English words (`locales/en.json`), what `zeron.t` shows.
pub fn english(name: &str) -> serde_json::Value {
    serde_json::json!({
        "hello": {
            "label": "Say hello",
            "toast": format!("Hello from {name}"),
        }
    })
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
pub fn write(
    dir: &Path,
    id: &str,
    name: &str,
    description: &str,
    permissions: &[String],
) -> Result<(), String> {
    if dir.exists() {
        return Err(format!("There's already a plugin called {id}"));
    }
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("init.luau"), init_luau(id, name))?;
        std::fs::write(dir.join("zeron.luau"), SDK)?;
        std::fs::create_dir_all(dir.join("locales"))?;
        std::fs::write(
            dir.join("locales").join("en.json"),
            super::signing::pretty(&english(name)),
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
