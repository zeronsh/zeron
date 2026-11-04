//! Theme tools: read, make, change, delete and wear the colours Zeron's
//! window wears (the engine's `ListThemes` … `UseTheme`, over this device's
//! theme files). A theme is data: no tool here asks the person, and only
//! the person's own themes can be changed. The guide is
//! apps/zgx/docs/themes.md, which `list_themes` links.

use serde_json::{Value, json};
use zeron_rpc::{RpcError, methods};

use super::{ToolDef, Tools};

pub(super) const LIST_THEMES: &str = "list_themes";
pub(super) const GET_THEME: &str = "get_theme";
pub(super) const CREATE_THEME: &str = "create_theme";
pub(super) const UPDATE_THEME: &str = "update_theme";
pub(super) const DELETE_THEME: &str = "delete_theme";
pub(super) const USE_THEME: &str = "use_theme";

pub(super) const NAMES: [&str; 6] = [LIST_THEMES, GET_THEME, CREATE_THEME, UPDATE_THEME, DELETE_THEME, USE_THEME];

/// A palette's fields, said once for every tool that takes one.
macro_rules! palette_said {
    () => {
        "A palette is one scheme's colours, each \"#rrggbb\" (or \"#rrggbbaa\"): background (the window), raised (chips, selected rows), text, muted, faint (three text inks), accent, danger, warning, success, and syntax: 12 code colours in this order: comment, keyword, string, number, type, function, property, variable, punctuation, tag, attribute, invalid. Optional: shell (sidebar and title bar), card, terminal (its background) and ansi (16 terminal colours). Example dark palette: {\"background\":\"#14181f\",\"raised\":\"#232a35\",\"text\":\"#d8dee9\",\"muted\":\"#a3acb9\",\"faint\":\"#7c8594\",\"accent\":\"#7aa2f7\",\"danger\":\"#f7768e\",\"warning\":\"#e0af68\",\"success\":\"#9ece6a\",\"syntax\":[\"#5c6773\",\"#c792ea\",\"#c3e88d\",\"#f78c6c\",\"#ffcb6b\",\"#82aaff\",\"#d8dee9\",\"#d8dee9\",\"#a3acb9\",\"#f7768e\",\"#ffcb6b\",\"#f7768e\"]}. The window keeps text and code readable: `adjusted` in the reply lists any colour it wears darker or lighter than given."
    };
}


fn palette_schema(scheme: &str) -> Value {
    json!({
        "type": ["object", "null"],
        "description": format!("The {scheme} palette (see the tool's description). In update_theme, null takes the {scheme} version away."),
        "properties": {
            "background": { "type": "string" }, "raised": { "type": "string" },
            "text": { "type": "string" }, "muted": { "type": "string" }, "faint": { "type": "string" },
            "accent": { "type": "string" }, "danger": { "type": "string" }, "warning": { "type": "string" }, "success": { "type": "string" },
            "syntax": { "type": "array", "items": { "type": "string" }, "minItems": 12, "maxItems": 12 },
            "shell": { "type": "string" }, "card": { "type": "string" }, "terminal": { "type": "string" },
            "ansi": { "type": "array", "items": { "type": "string" }, "minItems": 16, "maxItems": 16 }
        }
    })
}

pub(super) fn catalog() -> Vec<ToolDef> {
    let id = |what: &str| json!({ "type": "string", "description": what });
    vec![
        ToolDef {
            name: LIST_THEMES,
            description: "Every theme Zeron's window can wear on this device: built in, a template's own, and yours (imported or made with create_theme). Each has its id, name, source (\"built in\", \"a template's\" or \"yours\"), whether you can change it (editable: yours only), its variant for each scheme (variants: {light?, dark?}) and the schemes it's worn for now (in_use). `wearing` says what light and dark wear now; `guide` is the full reference (docs/themes.md).",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: GET_THEME,
            description: "A theme's colours: a palette for each scheme it has (light, dark), in the shape create_theme and update_theme take, so you can read one, change it and save it back. Built-in and template themes are read-only: to change one, call create_theme with its colours and a new name. `adjusted` lists what the window wears instead of a colour too faint to read.",
            input_schema: json!({ "type": "object", "properties": { "id": id("A theme's id, from list_themes.") }, "required": ["id"] }),
        },
        ToolDef {
            name: CREATE_THEME,
            description: concat!(
                "Make a theme of yours on this device, from palettes (`light`, `dark`: one at least) or from a VS Code colour theme (`vscode`: its JSON, with colors and tokenColors). Returns its id, its variant ids and `adjusted`. To wear it, call use_theme. ",
                palette_said!()
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "What it's called (one line, at most 80 characters). With `vscode`, the theme's own name by default." },
                    "light": palette_schema("light"),
                    "dark": palette_schema("dark"),
                    "vscode": { "type": ["object", "string"], "description": "Instead of palettes: a VS Code colour theme's JSON (as an object or as text), with no `include`." }
                }
            }),
        },
        ToolDef {
            name: UPDATE_THEME,
            description: concat!(
                "Change one of your themes (list_themes: editable): rename it, give a scheme a new palette, or take a scheme away with null. Its ids stay, so while it's worn the window restyles at once. To change a built-in or template theme, make a copy with create_theme instead. ",
                palette_said!()
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": id("One of your themes' ids, from list_themes (editable: true)."),
                    "name": { "type": "string", "description": "A new name." },
                    "light": palette_schema("light"),
                    "dark": palette_schema("dark")
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: DELETE_THEME,
            description: "Delete one of your themes from this device (built-in and template themes can't be deleted). A scheme it was worn for goes back to the template's theme, or to Zeron's.",
            input_schema: json!({ "type": "object", "properties": { "id": id("One of your themes' ids, from list_themes.") }, "required": ["id"] }),
        },
        ToolDef {
            name: USE_THEME,
            description: "Wear a theme: any theme from list_themes, or a variant of one. The window changes at once. `slot` is \"light\", \"dark\" or \"both\" (the default: each scheme the theme has). A theme with only a dark version can't be worn for light.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": id("A theme's id (or one of its variants'), from list_themes."),
                    "slot": { "type": "string", "enum": ["light", "dark", "both"], "description": "Which scheme wears it (default both)." }
                },
                "required": ["id"]
            }),
        },
    ]
}

fn plain(error: RpcError) -> String {
    match error {
        RpcError::Failed(message) => message,
        RpcError::UnknownMethod(_) => "This Zeron doesn't have theme tools yet; update Zeron to use them.".into(),
        other => other.to_string(),
    }
}

impl Tools {
    /// A theme tool: its arguments go to the engine as they are (it reads
    /// and checks them, saying what's wrong in words to fix it by).
    pub(super) async fn themes(&self, tool: &str, args: Value) -> Result<Value, String> {
        let args = if args.is_null() { json!({}) } else { args };
        if !args.is_object() {
            return Err("invalid arguments: expected an object".into());
        }
        let method = match tool {
            LIST_THEMES => methods::LIST_THEMES,
            GET_THEME => methods::GET_THEME,
            CREATE_THEME => methods::CREATE_THEME,
            UPDATE_THEME => methods::UPDATE_THEME,
            DELETE_THEME => methods::DELETE_THEME,
            _ => methods::USE_THEME,
        };
        let mut reply = self.zeron.call_rpc(method, args).await.map_err(plain)?;
        if method == methods::CREATE_THEME {
            reply["next"] = json!("To wear it, call use_theme with this id.");
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zeron::{Origin, Zeron};
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Arc;
    use zeron_rpc::{RpcReply, RpcService, memory_client};

    /// The engine's theme half, on a data dir of its own: the catalogue it
    /// answers from, as the engine does.
    struct Engine {
        dir: PathBuf,
    }

    #[async_trait]
    impl RpcService for Engine {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            use zeron_theme::catalog;
            let dir = &self.dir;
            let said = |e: anyhow::Error| RpcError::Failed(e.to_string());
            let text = |k: &str| params.get(k).and_then(Value::as_str);
            let need = || text("id").ok_or_else(|| RpcError::Failed("Say which theme".into()));
            let reply = match method {
                methods::LIST_THEMES => catalog::list(dir),
                methods::GET_THEME => catalog::get(dir, need()?).map_err(said)?,
                methods::CREATE_THEME => match params.get("vscode") {
                    Some(v) => catalog::create_from_vscode(dir, text("name"), v),
                    None => catalog::create(dir, text("name").unwrap_or(""), params.get("light"), params.get("dark")),
                }
                .map_err(said)?
                .to_json(),
                methods::UPDATE_THEME => catalog::update(dir, need()?, text("name"), params.get("light"), params.get("dark")).map_err(said)?.to_json(),
                methods::DELETE_THEME => {
                    catalog::delete(dir, need()?).map_err(said)?;
                    json!({ "deleted": need()? })
                }
                methods::USE_THEME => catalog::use_theme(dir, need()?, text("slot").unwrap_or("both")).map_err(said)?,
                other => return Err(RpcError::UnknownMethod(other.into())),
            };
            Ok(RpcReply::Value(reply))
        }
    }

    fn tools(dir: &std::path::Path) -> Tools {
        let engine = Arc::new(Engine { dir: dir.to_path_buf() });
        Tools::new(Arc::new(Zeron::with_client(memory_client(engine), Origin::default())))
    }

    fn palette(seed: &str) -> Value {
        let v = zeron_theme::builtin_registry().families.iter().flat_map(|f| &f.variants).find(|v| v.id == seed).unwrap();
        zeron_theme::catalog::Palette::of(v).to_json(None)
    }

    #[tokio::test]
    async fn a_theme_is_made_read_changed_worn_and_deleted() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let made = tools.call(CREATE_THEME, json!({ "name": "Ink", "light": palette("github-light"), "dark": palette("tokyo-night") })).await.unwrap();
        assert_eq!(made["id"], "custom-ink");
        assert!(made["next"].as_str().unwrap().contains("use_theme"));
        let got = tools.call(GET_THEME, json!({ "id": "custom-ink" })).await.unwrap();
        assert_eq!(got["dark"], palette("tokyo-night"), "as given");
        assert_eq!(got["editable"], true);
        // Read, changed, saved back.
        let mut dark = got["dark"].clone();
        dark["accent"] = json!("#ff3e9a");
        let changed = tools.call(UPDATE_THEME, json!({ "id": "custom-ink", "name": "Ink Pink", "dark": dark })).await.unwrap();
        assert_eq!(changed["variants"]["dark"], "custom-ink-dark");
        let again = tools.call(GET_THEME, json!({ "id": "custom-ink" })).await.unwrap();
        assert_eq!((again["name"].as_str(), again["dark"]["accent"].as_str()), (Some("Ink Pink"), Some("#ff3e9a")));
        // Worn: list_themes says so.
        let used = tools.call(USE_THEME, json!({ "id": "custom-ink" })).await.unwrap();
        assert_eq!(used["chosen"], json!({ "light": "custom-ink-light", "dark": "custom-ink-dark" }));
        let listed = tools.call(LIST_THEMES, json!({})).await.unwrap();
        let ink = listed["themes"].as_array().unwrap().iter().find(|t| t["id"] == "custom-ink").unwrap();
        assert_eq!(ink["in_use"], json!(["light", "dark"]));
        assert_eq!(listed["wearing"]["dark"]["variant"], "custom-ink-dark");
        // Gone, and its schemes back to the default.
        assert_eq!(tools.call(DELETE_THEME, json!({ "id": "custom-ink" })).await.unwrap(), json!({ "deleted": "custom-ink" }));
        let listed = tools.call(LIST_THEMES, json!({})).await.unwrap();
        assert_eq!(listed["wearing"]["dark"]["variant"], "zeron-dark");
    }

    #[tokio::test]
    async fn built_in_and_template_themes_are_read_only() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let gruvbox = tools.call(GET_THEME, json!({ "id": "gruvbox" })).await.unwrap();
        assert_eq!(gruvbox["editable"], false);
        assert!(gruvbox["light"].is_object() && gruvbox["dark"].is_object());
        for (tool, args) in [
            (UPDATE_THEME, json!({ "id": "gruvbox", "name": "Mine" })),
            (DELETE_THEME, json!({ "id": "gruvbox" })),
        ] {
            let err = tools.call(tool, args).await.unwrap_err();
            assert!(err.contains("built in") && err.contains("create_theme"), "{tool}: {err}");
        }
        // A copy is yours.
        let copy = tools.call(CREATE_THEME, json!({ "name": "My Gruvbox", "light": gruvbox["light"], "dark": gruvbox["dark"] })).await.unwrap();
        assert_eq!(tools.call(GET_THEME, json!({ "id": copy["id"] })).await.unwrap()["editable"], true);
        let err = tools.call(USE_THEME, json!({ "id": "dracula", "slot": "light" })).await.unwrap_err();
        assert!(err.contains("no light version"), "{err}");
    }

    #[tokio::test]
    async fn a_bad_palette_says_what_to_fix() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let mut bad = palette("nord");
        bad["accent"] = json!("purple");
        bad["syntax"] = json!(["#ffffff", "#000000"]);
        let err = tools.call(CREATE_THEME, json!({ "name": "Bad", "dark": bad })).await.unwrap_err();
        assert!(err.contains("`dark.accent`: \"purple\" isn't a colour"), "{err}");
        assert!(err.contains("`dark.syntax` needs 12 colours"), "{err}");
        let err = tools.call(CREATE_THEME, json!({ "name": "Empty" })).await.unwrap_err();
        assert!(err.contains("`light`, `dark` or both"), "{err}");
        assert!(tools.call(GET_THEME, json!({ "id": "nope" })).await.unwrap_err().contains("list_themes"));
        assert!(tools.call(LIST_THEMES, json!("x")).await.is_err());
    }

    #[tokio::test]
    async fn a_vs_code_theme_comes_in_through_the_importer() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let theme = json!({
            "name": "Paper Mill", "type": "light",
            "colors": { "editor.background": "#f7f3ea", "editor.foreground": "#2b2a27", "focusBorder": "#b5651d" },
            "tokenColors": [{ "scope": "keyword", "settings": { "foreground": "#8a3b12" } }],
        });
        let made = tools.call(CREATE_THEME, json!({ "vscode": theme })).await.unwrap();
        assert_eq!(made["name"], "Paper Mill");
        let got = tools.call(GET_THEME, json!({ "id": made["id"] })).await.unwrap();
        assert_eq!(got["light"]["background"], "#f7f3ea");
        assert_eq!(got["light"]["syntax"][1], "#8a3b12", "keywords from tokenColors");
    }

    #[test]
    fn every_theme_tool_is_listed_once_with_its_palette_said() {
        let defs = catalog();
        assert_eq!(defs.iter().map(|d| d.name).collect::<Vec<_>>(), NAMES);
        for def in &defs[2..4] {
            assert!(def.description.contains("12 code colours in this order") && def.description.contains("\"background\":\"#14181f\""), "{}", def.name);
        }
    }
}
