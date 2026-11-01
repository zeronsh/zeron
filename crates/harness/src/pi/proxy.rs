//! Send this Pi process's subscription providers through Zeron's account
//! proxy ([`crate::account_proxy`]) with a per-run extension that redirects
//! each one (Pi's documented "redirect an existing provider" form). The
//! extension reads the routes from its own environment variable (visible to
//! Pi's tools, like any of its settings; see [`crate::account_proxy`]); the
//! engine's route variables are dropped by [`crate::apply_run_env`].
use std::collections::BTreeMap;

use crate::{HarnessError, process::Command, scratch::ScratchDir};

/// The variable the extension reads: `{provider id: base URL}`.
const ROUTES_ENV: &str = "ZERON_PI_PROXY";

/// Pi provider ids to redirect, with their route bases (empty: no rotation).
pub(super) fn routes(run_env: &BTreeMap<String, String>) -> Option<String> {
    let providers = crate::account_proxy::pi_provider_base_urls(run_env);
    (!providers.is_empty())
        .then(|| serde_json::to_string(&providers).expect("serializable provider routes"))
}

pub(super) fn configure(
    cmd: &mut Command,
    run_env: &BTreeMap<String, String>,
) -> Result<Option<ScratchDir>, HarnessError> {
    let Some(routes) = routes(run_env) else {
        return Ok(None);
    };
    let scratch = ScratchDir::new("pi-proxy")?;
    let extension = scratch.path().join("zeron-proxy.mjs");
    std::fs::write(&extension, include_str!("proxy.mjs"))?;
    cmd.arg("--extension")
        .arg(extension)
        .env(ROUTES_ENV, routes);
    Ok(Some(scratch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotating_chats_redirect_pi_providers_to_their_routes() {
        let env = BTreeMap::from([
            (
                crate::account_proxy::CHATGPT.to_string(),
                "http://127.0.0.1:41000/r/aa".to_string(),
            ),
            (
                crate::account_proxy::ANTHROPIC.to_string(),
                "http://127.0.0.1:41000/r/bb".to_string(),
            ),
        ]);
        let routes: serde_json::Value =
            serde_json::from_str(&super::routes(&env).unwrap()).unwrap();
        assert_eq!(
            routes,
            serde_json::json!({
                "openai-codex": "http://127.0.0.1:41000/r/aa/backend-api",
                "anthropic": "http://127.0.0.1:41000/r/bb",
            })
        );
        assert!(super::routes(&BTreeMap::new()).is_none());
        let extension = include_str!("proxy.mjs");
        assert!(extension.contains("process.env.ZERON_PI_PROXY"));
        assert!(extension.contains("pi.registerProvider(provider, { baseUrl })"));
    }
}
