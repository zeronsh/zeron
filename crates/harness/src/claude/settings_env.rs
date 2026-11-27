//! The provider part of the user's Claude settings `env`, for a titling run.
//!
//! A titling run starts the CLI with `--setting-sources ""`, so none of the
//! user's settings (hooks, plugins, permissions, instructions) reach it. The
//! `env` block of `settings.json` goes with them, and that is where an
//! Anthropic-compatible gateway or a local proxy is configured
//! (`ANTHROPIC_BASE_URL` with `ANTHROPIC_AUTH_TOKEN`, as cc-switch and
//! CLIProxyAPI write it). Without it the titling run signs in with the CLI's
//! own login, which a proxy user may not have, and every title fails with
//! "Authentication failed" and falls back to the prompt's first words. Chat
//! runs load the settings, so they work.
//!
//! Only what says where requests go and how they sign in is carried over:
//! nothing that changes what the CLI may do.

use std::collections::BTreeMap;
use std::path::Path;

/// Exact keys carried over (compared case-insensitively: Windows'
/// environment is).
const KEYS: &[&str] = &[
    "API_TIMEOUT_MS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
    "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
    "CLOUD_ML_REGION",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "NODE_EXTRA_CA_CERTS",
];

/// Prefixes carried over: the Anthropic API's own (base URL, key, token,
/// headers, model mapping), and Bedrock's and Vertex's credentials and
/// regions. (Not `CLAUDE_CODE_USE_`: `CLAUDE_CODE_USE_POWERSHELL_TOOL` is
/// one of those, and it's a tool switch.)
const PREFIXES: &[&str] = &["ANTHROPIC_", "AWS_", "VERTEX_REGION_"];

fn carried(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    KEYS.contains(&key.as_str()) || PREFIXES.iter().any(|prefix| key.starts_with(prefix))
}

/// The provider variables of `{config_root}/settings.json`'s `env` (empty
/// when there is no such file, it doesn't parse, or it sets none).
pub(super) fn provider_env(config_root: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(config_root.join("settings.json")) else {
        return BTreeMap::new();
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&text) else {
        tracing::debug!(target: "zeron_harness::claude", "user settings.json doesn't parse; titling runs without its env");
        return BTreeMap::new();
    };
    let Some(env) = settings.get("env").and_then(|env| env.as_object()) else {
        return BTreeMap::new();
    };
    env.iter()
        .filter(|(key, _)| carried(key))
        .filter_map(|(key, value)| {
            // The CLI takes numbers and booleans in `env` as their text.
            let value = match value {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::Bool(flag) => flag.to_string(),
                _ => return None,
            };
            Some((key.clone(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_with(settings: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), settings).unwrap();
        dir
    }

    #[test]
    fn carries_a_proxys_url_and_token_and_nothing_that_changes_behaviour() {
        let dir = root_with(
            r#"{
                "env": {
                    "ANTHROPIC_BASE_URL": "http://127.0.0.1:8317",
                    "ANTHROPIC_AUTH_TOKEN": "sk-proxy",
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "small-model",
                    "CLAUDE_CODE_USE_BEDROCK": 1,
                    "API_TIMEOUT_MS": 600000,
                    "https_proxy": "http://corp:3128",
                    "CLAUDE_CODE_USE_POWERSHELL_TOOL": "1",
                    "CLAUDE_CODE_DISABLE_TERMINAL_TITLE": "0",
                    "ENABLE_TOOL_SEARCH": "true",
                    "BASH_DEFAULT_TIMEOUT_MS": "1000",
                    "ANTHROPIC_CUSTOM_HEADERS": {"not": "a string"}
                },
                "hooks": {}
            }"#,
        );
        let env = provider_env(dir.path());
        assert_eq!(
            env,
            BTreeMap::from([
                ("ANTHROPIC_BASE_URL".to_owned(), "http://127.0.0.1:8317".to_owned()),
                ("ANTHROPIC_AUTH_TOKEN".to_owned(), "sk-proxy".to_owned()),
                ("ANTHROPIC_DEFAULT_HAIKU_MODEL".to_owned(), "small-model".to_owned()),
                ("CLAUDE_CODE_USE_BEDROCK".to_owned(), "1".to_owned()),
                ("API_TIMEOUT_MS".to_owned(), "600000".to_owned()),
                ("https_proxy".to_owned(), "http://corp:3128".to_owned()),
            ])
        );
    }

    #[test]
    fn no_file_no_env_block_or_bad_json_carries_nothing() {
        let empty = tempfile::tempdir().unwrap();
        assert!(provider_env(empty.path()).is_empty());
        assert!(provider_env(root_with(r#"{"model": "opus"}"#).path()).is_empty());
        assert!(provider_env(root_with("{ not json").path()).is_empty());
        assert!(provider_env(root_with(r#"{"env": ["ANTHROPIC_BASE_URL"]}"#).path()).is_empty());
    }
}
