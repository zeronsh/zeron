//! Account rotation through the engine's local account proxy
//! (`zeron-account-proxy`): a run whose chat rotates accounts carries the
//! base URL of its proxy route in [`zeron_proto::RunRequest::env`] under one
//! of these names, one per upstream. Each harness points its CLI at it in the
//! CLI's own way; these variables themselves are not passed on
//! ([`crate::apply_run_env`] drops them), but the setting a harness turns
//! them into (Codex's thread config, Grok's environment variable, OpenCode's
//! inline config, Pi's extension environment) is the agent process's, so the
//! route URL is visible to the agent's own tools. That is the same user who
//! can read the saved logins anyway; the proxy only lets a route reach the
//! provider's inference endpoints (see `zeron-account-proxy`'s path rules).
//!
//! A route base is `http://127.0.0.1:<port>/r/<token>` and maps onto the
//! provider's host root, so the CLI's usual path follows it.

use std::collections::BTreeMap;

/// ChatGPT subscription traffic (`https://chatgpt.com/...`).
pub const CHATGPT: &str = "ZERON_ACCOUNT_PROXY_CHATGPT";
/// Claude subscription traffic (`https://api.anthropic.com/...`).
pub const ANTHROPIC: &str = "ZERON_ACCOUNT_PROXY_ANTHROPIC";
/// Grok Build traffic (`https://cli-chat-proxy.grok.com/...`).
pub const GROK: &str = "ZERON_ACCOUNT_PROXY_GROK";

const PREFIX: &str = "ZERON_ACCOUNT_PROXY_";

/// Whether `key` is one of the proxy route variables.
pub fn is_route_var(key: &str) -> bool {
    key.starts_with(PREFIX)
}

/// The route base under `var`, without a trailing slash.
pub fn route(env: &BTreeMap<String, String>, var: &str) -> Option<String> {
    env.get(var)
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
}

/// Codex's `openai_base_url` (a `thread/start` config override): its backend
/// routes hang off `/backend-api/codex`.
pub fn codex_base_url(env: &BTreeMap<String, String>) -> Option<String> {
    route(env, CHATGPT).map(|base| format!("{base}/backend-api/codex"))
}

/// Grok's `GROK_CLI_CHAT_PROXY_BASE_URL` (default
/// `https://cli-chat-proxy.grok.com/v1`).
pub fn grok_base_url(env: &BTreeMap<String, String>) -> Option<String> {
    route(env, GROK).map(|base| format!("{base}/v1"))
}

/// OpenCode's ChatGPT provider `baseURL` (default
/// `https://chatgpt.com/backend-api/codex`).
pub fn opencode_openai_base_url(env: &BTreeMap<String, String>) -> Option<String> {
    route(env, CHATGPT).map(|base| format!("{base}/backend-api/codex"))
}

/// Pi providers to redirect, by Pi provider id: `openai-codex` (default base
/// `https://chatgpt.com/backend-api`, Pi appends `/codex/responses`) and
/// `anthropic` (default `https://api.anthropic.com`).
pub fn pi_provider_base_urls(env: &BTreeMap<String, String>) -> BTreeMap<&'static str, String> {
    let mut providers = BTreeMap::new();
    if let Some(base) = route(env, CHATGPT) {
        providers.insert("openai-codex", format!("{base}/backend-api"));
    }
    if let Some(base) = route(env, ANTHROPIC) {
        providers.insert("anthropic", base);
    }
    providers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn bases_follow_each_cli_path_convention() {
        let e = env(&[
            (CHATGPT, "http://127.0.0.1:9/r/aa/"),
            (ANTHROPIC, "http://127.0.0.1:9/r/bb"),
            (GROK, "http://127.0.0.1:9/r/cc"),
        ]);
        assert_eq!(
            codex_base_url(&e).as_deref(),
            Some("http://127.0.0.1:9/r/aa/backend-api/codex")
        );
        assert_eq!(
            opencode_openai_base_url(&e).as_deref(),
            Some("http://127.0.0.1:9/r/aa/backend-api/codex")
        );
        assert_eq!(
            grok_base_url(&e).as_deref(),
            Some("http://127.0.0.1:9/r/cc/v1")
        );
        let pi = pi_provider_base_urls(&e);
        assert_eq!(pi["openai-codex"], "http://127.0.0.1:9/r/aa/backend-api");
        assert_eq!(pi["anthropic"], "http://127.0.0.1:9/r/bb");
        assert!(codex_base_url(&env(&[])).is_none());
        assert!(pi_provider_base_urls(&env(&[(GROK, "x")])).is_empty());
        assert!(is_route_var(GROK) && !is_route_var("CODEX_HOME"));
    }
}
