//! Themes for agents: the theme tools' methods (`ListThemes`, `GetTheme`,
//! `CreateTheme`, `UpdateTheme`, `DeleteTheme`, `UseTheme`) over this
//! device's library and choice in the data dir, through the catalogue the
//! window shares (`zeron_theme::catalog`). A theme is data, so nothing here
//! asks the person: it holds colours and a name, never code, and only yours
//! can be changed or deleted. Each change is told to the window
//! (`WatchThemes`), which reads the files again and restyles.
//!
//! These are local-only (`zeron_rpc::local_only`): a peer device never reads
//! or writes another's themes, as a device's templates are its own.

use std::path::Path;

use serde_json::{Value as Json, json};
use zeron_rpc::methods;
use zeron_theme::catalog;

use super::guides;

/// The methods this answers (the watch is the module's).
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::LIST_THEMES
            | methods::GET_THEME
            | methods::CREATE_THEME
            | methods::UPDATE_THEME
            | methods::DELETE_THEME
            | methods::USE_THEME
    )
}

/// Whether `method` changes the files (and the window should look again).
pub fn changes(method: &str) -> bool {
    matches!(method, methods::CREATE_THEME | methods::UPDATE_THEME | methods::DELETE_THEME | methods::USE_THEME)
}

fn text<'a>(params: &'a Json, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Json::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn id(params: &Json) -> Result<&str, String> {
    text(params, "id").ok_or_else(|| "Say which theme: `id`, from list_themes.".to_string())
}

/// `method` with `params`, over the files in `data_dir`.
pub fn answer(data_dir: &Path, method: &str, params: &Json) -> Result<Json, String> {
    let said = |e: anyhow::Error| e.to_string();
    match method {
        methods::LIST_THEMES => {
            let mut out = catalog::list(data_dir);
            out["guide"] = json!(guides::themes(data_dir).display().to_string());
            Ok(out)
        }
        methods::GET_THEME => catalog::get(data_dir, id(params)?).map_err(said),
        methods::CREATE_THEME => {
            let name = text(params, "name");
            let saved = match params.get("vscode").filter(|v| !v.is_null()) {
                Some(theme) => {
                    if params.get("light").is_some() || params.get("dark").is_some() {
                        return Err("Give palettes (`light`, `dark`) or a VS Code theme (`vscode`), not both.".into());
                    }
                    catalog::create_from_vscode(data_dir, name, theme)
                }
                None => catalog::create(data_dir, name.unwrap_or(""), params.get("light"), params.get("dark")),
            };
            saved.map(|s| s.to_json()).map_err(said)
        }
        methods::UPDATE_THEME => {
            let id = id(params)?;
            if params.get("vscode").is_some() {
                return Err("update_theme takes palettes (`light`, `dark`). To bring in a VS Code theme, make a new one with create_theme.".into());
            }
            catalog::update(data_dir, id, text(params, "name"), params.get("light"), params.get("dark")).map(|s| s.to_json()).map_err(said)
        }
        methods::DELETE_THEME => {
            let id = id(params)?;
            catalog::delete(data_dir, id).map(|()| json!({ "deleted": id })).map_err(said)
        }
        methods::USE_THEME => catalog::use_theme(data_dir, id(params)?, text(params, "slot").unwrap_or("both")).map_err(said),
        other => Err(format!("unknown theme method {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(seed: &str) -> Json {
        let v = zeron_theme::builtin_registry().families.iter().flat_map(|f| &f.variants).find(|v| v.id == seed).unwrap();
        catalog::Palette::of(v).to_json(None)
    }

    #[test]
    fn made_got_changed_worn_and_deleted_by_method() {
        let data = tempfile::tempdir().unwrap();
        let dir = data.path();
        let made = answer(dir, methods::CREATE_THEME, &json!({ "name": "Ink", "dark": palette("nord") })).unwrap();
        assert_eq!(made["id"], "custom-ink");
        let listed = answer(dir, methods::LIST_THEMES, &json!({})).unwrap();
        assert!(listed["themes"].as_array().unwrap().iter().any(|t| t["id"] == "custom-ink" && t["editable"] == true));
        assert!(std::fs::read_to_string(listed["guide"].as_str().unwrap()).unwrap().contains("# Themes"), "the guide is written");
        let mut dark = answer(dir, methods::GET_THEME, &json!({ "id": "custom-ink" })).unwrap()["dark"].clone();
        dark["accent"] = json!("#ff3e9a");
        answer(dir, methods::UPDATE_THEME, &json!({ "id": "custom-ink", "dark": dark })).unwrap();
        let used = answer(dir, methods::USE_THEME, &json!({ "id": "custom-ink" })).unwrap();
        assert_eq!(used["chosen"], json!({ "dark": "custom-ink-dark" }));
        assert_eq!(catalog::chosen(dir).dark.as_deref(), Some("custom-ink-dark"), "what the window reads");
        assert!(answer(dir, methods::UPDATE_THEME, &json!({ "id": "gruvbox", "dark": palette("nord") })).unwrap_err().contains("built in"));
        assert!(answer(dir, methods::CREATE_THEME, &json!({ "name": "Both", "dark": palette("nord"), "vscode": {} })).is_err());
        assert!(answer(dir, methods::GET_THEME, &json!({})).unwrap_err().contains("Say which theme"));
        assert_eq!(answer(dir, methods::DELETE_THEME, &json!({ "id": "custom-ink" })).unwrap(), json!({ "deleted": "custom-ink" }));
        assert!(changes(methods::USE_THEME) && !changes(methods::GET_THEME));
    }
}
