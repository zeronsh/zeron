//! Each plugin's storage (`zeron.store`): what it keeps between runs, for
//! it alone, in `<data>/plugins/.storage/<id>.json` (removing the plugin
//! removes it, the engine's `RemovePlugin`). At most [`MAX_BYTES`] a plugin.

use std::path::PathBuf;

use serde_json::{Map, Value as Json};

/// A plugin's storage at most, as written.
pub const MAX_BYTES: usize = 1024 * 1024;

fn path(id: &str) -> Option<PathBuf> {
    let safe = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    safe.then(crate::engine::data_dir)?.map(|d| d.join("plugins").join(".storage").join(format!("{id}.json")))
}

/// What plugin `id` keeps (nothing, if it kept nothing or it can't be read).
pub fn load(id: &str) -> Map<String, Json> {
    path(id)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Json>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Keeps `held` as plugin `id`'s storage, or says why not.
pub fn save(id: &str, held: &Map<String, Json>) -> Result<(), String> {
    let text = serde_json::to_string(held).map_err(|e| e.to_string())?;
    check(&text)?;
    let path = path(id).ok_or("there's nowhere to keep it")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let aside = path.with_extension("json.new");
    std::fs::write(&aside, text).map_err(|e| e.to_string())?;
    std::fs::rename(&aside, &path).map_err(|e| e.to_string())
}

fn check(text: &str) -> Result<(), String> {
    if text.len() > MAX_BYTES {
        return Err(format!("a plugin keeps at most {} KB", MAX_BYTES / 1024));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_keeps_at_most_its_share() {
        assert!(check("{}").is_ok());
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(check(&big).unwrap_err().contains("KB"));
        assert!(path("../etc").is_none(), "an id that isn't one has nowhere");
    }
}
