//! `zeron-plugin.json`: parsing and validation (contract "Manifest").
//!
//! The raw JSON [`serde_json::Value`] is kept beside the typed view: the
//! signature and the content hash cover the manifest exactly as written
//! (unknown fields included), and a re-sign rewrites it with its key order.

use std::collections::BTreeMap;

use serde::Serialize;

pub const MANIFEST_FILE: &str = "zeron-plugin.json";

/// Every permission a manifest may ask for.
pub const KNOWN_PERMISSIONS: &[&str] = &[
    "ui.add",
    "ui.change",
    "ui.replace",
    "profile.read",
    "sessions.read",
    "messages.read",
    "account.email",
    "files.read",
    "panel.tabs",
    "pages.add",
    "github",
    "github.write",
    "links.open",
    "notifications",
    "navigate",
    "compose",
    "network",
    "clipboard",
    "files",
    "web",
    "windows",
    "tools",
    "devices",
    "native",
    // Running the programs its manifest names (`commands`), as you.
    "commands",
    // A headless half (`"service": "service.luau"`) the engine runs on
    // each device that allowed it, called by its windows and agents from
    // this device or another (plugins/service).
    "service",
];

/// Ids the app keeps for itself.
pub const RESERVED_IDS: &[&str] = &["templates", "zeron", "app"];

pub const RESERVED_ID_ERROR: &str = "That name is Zeron's own";

/// `^[a-z0-9][a-z0-9-]{1,39}$`, and not one of [`RESERVED_IDS`].
pub fn check_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    let valid = (2..=40).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes[1..]
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'));
    if !valid {
        return Err(format!(
            "\"{id}\" can't be a plugin's id: use 2 to 40 lowercase letters, digits and dashes, starting with a letter or digit"
        ));
    }
    if RESERVED_IDS.contains(&id) {
        return Err(RESERVED_ID_ERROR.into());
    }
    Ok(())
}

/// A program a manifest may name to run (`commands`): its name alone, as
/// it's found on the PATH, never a path (`gh`, `git`, `node`, `python3`).
pub fn valid_command(name: &str) -> bool {
    zeron_plugin_guard::commands::valid_name(name)
}

/// A service's file: a relative `.luau` path inside the plugin, without
/// `..` or dot folders (`service.luau`, `service/main.luau`).
pub fn valid_service_path(path: &str) -> bool {
    path.ends_with(".luau")
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'))
}

/// What's wrong with a `ui.change` entry, if anything. An entry names an
/// element of Zeron's by selector, as zgx matches plugin targets: `#id`,
/// `.class`, `hook:name`, `role:name`, `"name"` or a bare id, with `>>`
/// between ancestors. It never names a module (`@app/…`: zgx would let
/// the plugin run as the app), everything (`*`), a node's number, row or
/// nth match, or Zeron's plugin placeholders (`.plugin-…`, each plugin's
/// own through "panel.tabs" and "pages.add" alone).
pub fn change_target_problem(target: &str) -> Option<String> {
    let refuse = |why: &str| Some(format!("It can't change \"{target}\": {why}"));
    if target.len() > 200 || target.chars().any(char::is_control) {
        return refuse("a selector is one short line");
    }
    if target.trim_start().starts_with('@') {
        return refuse("\"ui.change\" names parts of the screen, not modules (that's \"ui.replace\")");
    }
    for piece in target.split(">>") {
        let mut rest = piece.trim();
        if rest.is_empty() {
            return refuse("it isn't a selector (#id, .class, hook:name, role:name or \"name\")");
        }
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('"') {
                let Some(end) = after.find('"') else {
                    return refuse("an unclosed quote");
                };
                rest = after[end + 1..].trim_start();
                continue;
            }
            // As zgx reads a part: a class ends at the next `.`, the rest
            // at whitespace (or a `[`, refused below).
            let (class, prefix) = match rest {
                r if r.starts_with('.') => (true, 1),
                r if r.starts_with('#') => (false, 1),
                r if r.starts_with("hook:") || r.starts_with("role:") => (false, 5),
                _ => (false, 0),
            };
            let name_end = rest[prefix..]
                .find(|c: char| c.is_whitespace() || (class && c == '.'))
                .map_or(rest.len(), |at| at + prefix);
            let name = &rest[prefix..name_end];
            rest = rest[name_end..].trim_start();
            if name.is_empty() || name.contains(['@', '*', '[', ']', '~', '"', '#']) || name == "row" {
                return refuse("it isn't a selector (#id, .class, hook:name, role:name or \"name\")");
            }
            if class && name.starts_with("plugin-") {
                return refuse("plugins' tabs, pages and badges are their own and Zeron's");
            }
        }
    }
    None
}

/// `^[a-z][a-z0-9_]{0,40}$`, without `__`.
pub fn valid_tool_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=41).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
        // (`__` is the plugin's and the tool's divide in `<plugin>__<tool>`:
        // with none in a tool's name, and a tool's starting with a letter,
        // each agent-facing name has one plugin and one tool. R29.)
        && !name.contains("__")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UiSpec {
    pub change: Vec<String>,
    pub replace: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// The manifest's `signature` block, as written (not yet checked).
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureSpec {
    pub key: String,
    pub files: BTreeMap<String, String>,
    pub sig: String,
}

/// A parsed manifest. Fields that fail validation are reported by
/// [`Manifest::problem`]; parsing itself only fails on unreadable JSON.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The whole file, as read.
    pub raw: serde_json::Value,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub entry: String,
    pub author: Option<String>,
    pub permissions: Vec<String>,
    pub ui: UiSpec,
    pub tools: Vec<ToolSpec>,
    pub mcp: Option<McpSpec>,
    /// The programs it runs (`"commands": ["gh"]`, with the permission).
    pub commands: Vec<String>,
    /// Its service's file (`"service": "service.luau"`, with the permission).
    pub service: Option<String>,
    /// The hosts its requests may reach (`"network": { "hosts": […] }`,
    /// with the permission): none named, any public host; named, those
    /// alone, a private one (`localhost:8080`) only so (plugin-guard's `net`).
    pub network_hosts: Vec<String>,
    /// `None` when absent; `Some(Err)` when present but malformed.
    pub signature: Option<Result<SignatureSpec, ()>>,
    /// The first thing wrong with it, in plain words.
    problem: Option<String>,
}

fn string_list(value: Option<&serde_json::Value>, what: &str) -> Result<Vec<String>, String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("Its {what} must be a list of text"))
            })
            .collect(),
        Some(_) => Err(format!("Its {what} must be a list of text")),
    }
}

fn parse_signature(value: &serde_json::Value) -> Result<SignatureSpec, ()> {
    let object = value.as_object().ok_or(())?;
    let key = object.get("key").and_then(|v| v.as_str()).ok_or(())?;
    let sig = object.get("sig").and_then(|v| v.as_str()).ok_or(())?;
    let files = object
        .get("files")
        .and_then(|v| v.as_object())
        .ok_or(())?
        .iter()
        .map(|(path, hash)| {
            hash.as_str()
                .map(|h| (path.clone(), h.to_owned()))
                .ok_or(())
        })
        .collect::<Result<BTreeMap<_, _>, ()>>()?;
    Ok(SignatureSpec {
        key: key.to_owned(),
        files,
        sig: sig.to_owned(),
    })
}

impl Manifest {
    /// Parse `zeron-plugin.json`'s bytes. `Err` = not a JSON object.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let raw: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|_| "Its zeron-plugin.json isn't valid JSON".to_string())?;
        Self::from_value(raw)
    }

    pub fn from_value(raw: serde_json::Value) -> Result<Self, String> {
        let Some(object) = raw.as_object() else {
            return Err("Its zeron-plugin.json isn't a JSON object".into());
        };
        let mut problems: Vec<String> = Vec::new();
        let text = |key: &str, problems: &mut Vec<String>, required: bool| -> String {
            match object.get(key) {
                Some(serde_json::Value::String(s)) if !s.trim().is_empty() || !required => {
                    s.clone()
                }
                None | Some(serde_json::Value::Null) if !required => String::new(),
                _ => {
                    problems.push(if required {
                        format!("Its manifest is missing \"{key}\"")
                    } else {
                        format!("Its \"{key}\" must be text")
                    });
                    String::new()
                }
            }
        };
        let id = text("id", &mut problems, true);
        let name = text("name", &mut problems, true);
        let version = text("version", &mut problems, true);
        let entry = text("entry", &mut problems, true);
        let description = text("description", &mut problems, false);
        let author = object
            .get("author")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let permissions =
            string_list(object.get("permissions"), "permissions").unwrap_or_else(|e| {
                problems.push(e);
                Vec::new()
            });
        let ui = match object.get("ui") {
            None | Some(serde_json::Value::Null) => UiSpec::default(),
            Some(serde_json::Value::Object(ui)) => UiSpec {
                change: string_list(ui.get("change"), "ui.change").unwrap_or_else(|e| {
                    problems.push(e);
                    Vec::new()
                }),
                replace: string_list(ui.get("replace"), "ui.replace").unwrap_or_else(|e| {
                    problems.push(e);
                    Vec::new()
                }),
            },
            Some(_) => {
                problems.push("Its \"ui\" must be an object".into());
                UiSpec::default()
            }
        };
        let tools = match object.get("tools") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|item| {
                    let tool = item.as_object();
                    let name = tool.and_then(|t| t.get("name")).and_then(|v| v.as_str());
                    match (tool, name) {
                        (Some(tool), Some(name)) => Some(ToolSpec {
                            name: name.to_owned(),
                            description: tool
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_owned(),
                            input: tool
                                .get("input")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
                        }),
                        _ => {
                            problems.push("Each of its tools needs a \"name\"".into());
                            None
                        }
                    }
                })
                .collect(),
            Some(_) => {
                problems.push("Its \"tools\" must be a list".into());
                Vec::new()
            }
        };
        let mcp = match object.get("mcp") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Object(mcp)) => {
                match mcp.get("command").and_then(|v| v.as_str()) {
                    Some(command) if !command.trim().is_empty() => Some(McpSpec {
                        command: command.to_owned(),
                        args: string_list(mcp.get("args"), "mcp.args").unwrap_or_else(|e| {
                            problems.push(e);
                            Vec::new()
                        }),
                        env: mcp
                            .get("env")
                            .and_then(|v| v.as_object())
                            .map(|env| {
                                env.iter()
                                    .filter_map(|(k, v)| {
                                        v.as_str().map(|v| (k.clone(), v.to_owned()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }),
                    _ => {
                        problems.push("Its \"mcp\" needs a \"command\"".into());
                        None
                    }
                }
            }
            Some(_) => {
                problems.push("Its \"mcp\" must be an object".into());
                None
            }
        };
        let commands = string_list(object.get("commands"), "\"commands\"").unwrap_or_else(|e| {
            problems.push(e);
            Vec::new()
        });
        let service = match object.get("service") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(path)) => Some(path.clone()),
            Some(_) => {
                problems.push("Its \"service\" must be the path of a .luau file in it".into());
                None
            }
        };
        let network_hosts = match object.get("network") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Object(network)) => {
                string_list(network.get("hosts"), "network.hosts").unwrap_or_else(|e| {
                    problems.push(e);
                    Vec::new()
                })
            }
            Some(_) => {
                problems.push("Its \"network\" must be an object (\"hosts\": [\"api.example.com\"])".into());
                Vec::new()
            }
        };
        let signature = object.get("signature").map(parse_signature);
        let mut manifest = Manifest {
            raw: raw.clone(),
            id,
            name,
            version,
            description,
            entry,
            author,
            permissions,
            ui,
            tools,
            mcp,
            commands,
            service,
            network_hosts,
            signature,
            problem: None,
        };
        manifest.problem = problems.into_iter().next().or_else(|| manifest.rules());
        Ok(manifest)
    }

    /// The contract's "a manifest is broken when" list.
    fn rules(&self) -> Option<String> {
        if let Some(unknown) = self
            .permissions
            .iter()
            .find(|p| !KNOWN_PERMISSIONS.contains(&p.as_str()))
        {
            return Some(format!(
                "It asks for \"{unknown}\", which isn't a permission Zeron knows"
            ));
        }
        let asks = |p: &str| self.permissions.iter().any(|have| have == p);
        if !self.network_hosts.is_empty() && !asks("network") {
            return Some("It names hosts to reach but doesn't ask for \"network\"".into());
        }
        if let Err(problem) = zeron_plugin_guard::net::Policy::new(&self.network_hosts) {
            return Some(problem);
        }
        if !self.tools.is_empty() && !asks("tools") {
            return Some("It lists tools but doesn't ask for the \"tools\" permission".into());
        }
        if self.mcp.is_some() && !asks("native") {
            return Some(
                "It has an MCP server but doesn't ask for the \"native\" permission".into(),
            );
        }
        if !self.commands.is_empty() && !asks("commands") {
            return Some("It names programs to run but doesn't ask for \"commands\"".into());
        }
        if asks("commands") && self.commands.is_empty() {
            return Some("It asks for \"commands\" but names no program to run".into());
        }
        if let Some(bad) = self.commands.iter().find(|c| !valid_command(c)) {
            return Some(format!("\"{bad}\" isn't a program's name: name it as it's found on the PATH"));
        }
        match &self.service {
            Some(_) if !asks("service") => {
                return Some("It has a service but doesn't ask for \"service\"".into());
            }
            None if asks("service") => {
                return Some("It asks for \"service\" but has none (\"service\": \"service.luau\")".into());
            }
            Some(path) if !valid_service_path(path) => {
                return Some(format!(
                    "Its service \"{path}\" must be a .luau file inside it, by a relative path"
                ));
            }
            _ => {}
        }
        if !self.ui.change.is_empty() && !asks("ui.change") {
            return Some("It changes parts of Zeron but doesn't ask for \"ui.change\"".into());
        }
        if let Some(problem) = self.ui.change.iter().find_map(|t| change_target_problem(t)) {
            return Some(problem);
        }
        if !self.ui.replace.is_empty() && !asks("ui.replace") {
            return Some("It replaces screens but doesn't ask for \"ui.replace\"".into());
        }
        for screen in &self.ui.replace {
            if !screen.starts_with("@app/screens/") {
                return Some(format!(
                    "It can only replace Zeron's screens, not \"{screen}\""
                ));
            }
            let protected = ["@app/screens/plugins", "@app/screens/shell/settings"];
            if protected
                .iter()
                .any(|p| screen == p || screen.starts_with(&format!("{p}/")))
            {
                return Some(format!("\"{screen}\" can't be replaced by a plugin"));
            }
        }
        if let Some(tool) = self.tools.iter().find(|t| !valid_tool_name(&t.name)) {
            return Some(format!(
                "Its tool \"{}\" needs a name of lowercase letters, digits and underscores",
                tool.name
            ));
        }
        None
    }

    /// The first thing wrong with it, in plain words.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The manifest without its `signature`: what the content hash covers.
    pub fn unsigned_value(&self) -> serde_json::Value {
        let mut value = self.raw.clone();
        if let Some(object) = value.as_object_mut() {
            object.remove("signature");
        }
        value
    }

    pub fn asks(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(value: serde_json::Value) -> Option<String> {
        Manifest::from_value(value)
            .unwrap()
            .problem()
            .map(str::to_owned)
    }

    fn base() -> serde_json::Value {
        serde_json::json!({ "id": "focus", "name": "Focus", "version": "1.0.0", "entry": "init.luau" })
    }

    #[test]
    fn a_plugin_runs_only_the_programs_it_names_with_the_permission() {
        let with = |permissions: serde_json::Value, commands: serde_json::Value| {
            let mut m = base();
            m["permissions"] = permissions;
            if !commands.is_null() {
                m["commands"] = commands;
            }
            m
        };
        let ok = Manifest::from_value(with(serde_json::json!(["commands"]), serde_json::json!(["gh", "git"]))).unwrap();
        assert_eq!((ok.problem(), ok.commands.clone()), (None, vec!["gh".to_string(), "git".to_string()]));
        assert!(problem(with(serde_json::json!([]), serde_json::json!(["gh"]))).unwrap().contains("doesn't ask for \"commands\""));
        assert!(problem(with(serde_json::json!(["commands"]), serde_json::Value::Null)).unwrap().contains("names no program"));
        for bad in ["/usr/bin/gh", "../gh", "gh git", "", "-rf"] {
            assert!(problem(with(serde_json::json!(["commands"]), serde_json::json!([bad]))).is_some(), "{bad}");
        }
    }

    fn with(key: &str, value: serde_json::Value) -> serde_json::Value {
        let mut manifest = base();
        manifest[key] = value;
        manifest
    }

    /// R1: a "ui.change" entry naming a module (`@app/…`) would be a zgx
    /// module override, the plugin running as the app.
    #[test]
    fn plugins_manifest_change_names_elements_never_modules() {
        let change = |target: &str| {
            let mut m = with("ui", serde_json::json!({ "change": [target] }));
            m["permissions"] = serde_json::json!(["ui.change"]);
            problem(m)
        };
        for good in [
            "#chat-composer",
            ".session-row",
            "hook:sidebar.sessions",
            "role:toolbar",
            "\"Send\"",
            "#sidebar >> .session-row",
            "titlebar-actions",
        ] {
            assert_eq!(change(good), None, "{good}");
        }
        for bad in [
            "@app/screens/plugins/state",
            " @app/screens/shell/settings",
            "@1",
            "*",
            "",
            "#a >> ",
            ".row[2]",
            "~\"Allow\"",
            "#x >> @app/screens/chat",
            ".plugin-tab.plugin-github",
            ".plugin-unverified",
            "\"open",
        ] {
            assert!(change(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn plugins_manifest_minimal_is_fine() {
        assert_eq!(problem(base()), None);
        let manifest = Manifest::from_value(base()).unwrap();
        assert_eq!(manifest.description, "");
        assert!(manifest.permissions.is_empty());
        assert!(manifest.signature.is_none());
    }

    #[test]
    fn plugins_manifest_broken_rules() {
        assert!(
            problem(serde_json::json!({ "id": "focus", "name": "F", "version": "1" })).is_some(),
            "no entry"
        );
        assert!(problem(with("permissions", serde_json::json!(["root"]))).is_some());
        assert!(problem(with("tools", serde_json::json!([{ "name": "count" }]))).is_some());
        assert!(problem(with("mcp", serde_json::json!({ "command": "./bin/x" }))).is_some());
        let mut ui = with("ui", serde_json::json!({ "change": ["hook:sidebar"] }));
        assert!(problem(ui.clone()).is_some());
        ui["permissions"] = serde_json::json!(["ui.change"]);
        assert_eq!(problem(ui), None);
        let replace = |screen: &str| {
            let mut m = with("ui", serde_json::json!({ "replace": [screen] }));
            m["permissions"] = serde_json::json!(["ui.replace"]);
            problem(m)
        };
        assert!(replace("@app/ds/button").is_some());
        assert!(replace("@app/screens/plugins").is_some());
        assert!(replace("@app/screens/plugins/review").is_some());
        assert!(replace("@app/screens/shell/settings").is_some());
        assert_eq!(replace("@app/screens/sidebar"), None);
        assert!(replace("@app/screens/sidebar").is_none());
        let mut ui = with(
            "ui",
            serde_json::json!({ "replace": ["@app/screens/sidebar"] }),
        );
        assert!(problem(ui.clone()).is_some(), "replace without ui.replace");
        ui["permissions"] = serde_json::json!(["ui.replace"]);
        assert_eq!(problem(ui), None);
        let mut tools = with("tools", serde_json::json!([{ "name": "Count" }]));
        tools["permissions"] = serde_json::json!(["tools"]);
        assert!(problem(tools.clone()).is_some(), "bad tool name");
        tools["tools"] = serde_json::json!([{ "name": "count_words" }]);
        assert_eq!(problem(tools), None);
    }

    #[test]
    fn plugins_ids() {
        assert!(check_id("focus").is_ok());
        assert!(check_id("compact-sidebar").is_ok());
        assert!(check_id("0x").is_ok());
        assert!(check_id("a").is_err());
        assert!(check_id("-a").is_err());
        assert!(check_id("Focus").is_err());
        assert!(check_id(&"a".repeat(41)).is_err());
        assert_eq!(check_id("zeron").unwrap_err(), RESERVED_ID_ERROR);
        assert_eq!(check_id("app").unwrap_err(), RESERVED_ID_ERROR);
        assert_eq!(check_id("templates").unwrap_err(), RESERVED_ID_ERROR);
        assert!(valid_tool_name("count_words"));
        assert!(!valid_tool_name("_x"));
        assert!(!valid_tool_name(&format!("a{}", "b".repeat(41))));
        // R29: `<plugin>__<tool>` names one plugin's one tool. Before, plugin
        // `a--b`'s tool `c` and plugin `a`'s tool `b__c` were both `a__b__c`.
        assert!(!valid_tool_name("b__c"));
        assert_ne!(crate::plugins::mcp_name("a--b", "c"), crate::plugins::mcp_name("a", "b_c"));
    }
}
