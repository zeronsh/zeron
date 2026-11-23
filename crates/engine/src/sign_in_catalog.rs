//! The models a Cloud sign-in's new account runs an agent with, discovered on
//! this computer as it signs in. Cloud has nothing to list for an account no
//! machine has run yet, and this computer holds the login only until the vault
//! has it (a sign-in's throwaway home, shredded once it's uploaded): so before
//! that home goes, the agent is asked for its models there, and the list is
//! kept as that account's (`POST /cloud/{orgId}/models`). A machine's own
//! discovery replaces it once one runs the agent.
//!
//! Discovery runs in a process of its own (`zeron-engine discover-models
//! <agent>`), its environment pointing the agent at the sign-in's home: this
//! engine's discovery environment (`zeron_harness::catalog_env`) and its
//! catalogs stay this computer's own logins'.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use zeron_proto::providers::cloud_credentials;
use zeron_proto::{HarnessId, Model};

/// How long one discovery may take: an agent's first start can install or
/// update itself (Antigravity's server, Devin's CLI).
const DISCOVERY_LIMIT: Duration = Duration::from_secs(120);

/// Where a sign-in's discovered models go: kept on the edge as the accounts
/// Cloud runs `harness` on now (`crate::cloud_accounts::catalog_publisher`).
pub type CatalogPublisher = Arc<dyn Fn(HarnessId, Vec<Model>) -> BoxFuture<'static, ()> + Send + Sync>;

/// Whether a Cloud sign-in of `harness` discovers its models as it signs in:
/// an agent whose Cloud logins and keys all stand for one provider (Grok,
/// Antigravity, Muse, Cursor, Devin), so the new account alone is what Cloud
/// runs it on. One that reaches several providers at once (Pi, OpenCode,
/// Hermes) lists the others' models too, which this sign-in doesn't hold;
/// Claude Code's and Codex's lists are kept on the edge by hand.
pub fn discovers_at_sign_in(harness: HarnessId) -> bool {
    let credentials = cloud_credentials(harness);
    !matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex)
        && credentials
            .first()
            .is_some_and(|first| credentials.iter().all(|c| c.group == first.group))
}

/// `harness`'s models, discovered live in a process of this executable's own
/// whose environment adds `env` (the agent's home: `GEMINI_HOME`,
/// `GROK_HOME`, …). `None` when it lists none live or doesn't answer in time.
pub async fn discover(harness: HarnessId, env: Vec<(String, OsString)>) -> Option<Vec<Model>> {
    #[cfg(test)]
    if let Some(exe) = test_exe::get() {
        return discover_with(&exe, harness, env).await;
    }
    let exe = std::env::current_exe().ok()?;
    discover_with(&exe, harness, env).await
}

/// What a test runs in this executable's place (the test binary would run
/// its tests again).
#[cfg(test)]
pub(crate) mod test_exe {
    use std::path::PathBuf;
    use std::sync::{Mutex, PoisonError};

    static EXE: Mutex<Option<PathBuf>> = Mutex::new(None);

    pub(crate) fn set(exe: Option<PathBuf>) {
        *EXE.lock().unwrap_or_else(PoisonError::into_inner) = exe;
    }

    pub(super) fn get() -> Option<PathBuf> {
        EXE.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

async fn discover_with(exe: &Path, harness: HarnessId, env: Vec<(String, OsString)>) -> Option<Vec<Model>> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(["discover-models", harness.as_str()])
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(DISCOVERY_LIMIT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            tracing::warn!(harness = harness.as_str(), %error, "sign-in catalog: discovery didn't start");
            return None;
        }
        Err(_) => {
            tracing::warn!(harness = harness.as_str(), "sign-in catalog: discovery took too long");
            return None;
        }
    };
    if !output.status.success() {
        tracing::info!(harness = harness.as_str(), status = %output.status, "sign-in catalog: no live list");
        return None;
    }
    let models: Vec<Model> = serde_json::from_slice(&output.stdout).ok()?;
    (!models.is_empty()).then_some(models)
}

/// Discover `harness`'s models in `home`'s environment (`env`), then publish
/// them, holding `home` (shredded as it drops) until discovery is done: the
/// tail of a Cloud sign-in, after its upload, off its own task so the sign-in
/// reads as done at once.
pub fn discover_then_publish<H: Send + 'static>(
    harness: HarnessId,
    env: Vec<(String, OsString)>,
    home: H,
    publish: Option<CatalogPublisher>,
) {
    let Some(publish) = publish.filter(|_| discovers_at_sign_in(harness)) else {
        return;
    };
    tokio::spawn(async move {
        let models = discover(harness, env).await;
        drop(home);
        match models {
            Some(models) => publish(harness, models).await,
            None => tracing::info!(harness = harness.as_str(), "sign-in catalog: nothing to keep"),
        }
    });
}

/// `zeron-engine discover-models <agent>`: the agent's models discovered now,
/// live, as JSON on stdout. Its credentials are wherever this process's
/// environment points the agent. Fails with no live list.
pub async fn print_discovered(harness: &str) -> anyhow::Result<()> {
    let id: HarnessId = serde_json::from_value(serde_json::Value::String(harness.to_owned()))
        .map_err(|_| anyhow::anyhow!("unknown agent `{harness}`"))?;
    let registry = crate::registry::default_registry();
    let catalog = registry.discover_catalog(id).await?;
    anyhow::ensure!(catalog.source == "live", "{harness} listed no models live");
    anyhow::ensure!(!catalog.models.is_empty(), "{harness} listed no models");
    println!("{}", serde_json::to_string(&catalog.models)?);
    Ok(())
}

/// A sign-in home's environment for `harness`'s discovery: the variables that
/// point the agent's own store at `home` as its sign-in left it.
pub fn home_env(harness: HarnessId, home: &Path) -> Vec<(String, OsString)> {
    let at = |path: PathBuf| path.into_os_string();
    match harness {
        HarnessId::Antigravity => vec![
            ("GEMINI_HOME".into(), at(home.to_path_buf())),
            ("AGY_ACP_FORCE_FILE_STORAGE".into(), "1".into()),
        ],
        HarnessId::Grok => vec![("GROK_HOME".into(), at(home.to_path_buf()))],
        HarnessId::Devin => vec![
            ("XDG_DATA_HOME".into(), at(home.join("data"))),
            ("XDG_CONFIG_HOME".into(), at(home.join("config"))),
            ("XDG_CACHE_HOME".into(), at(home.join("cache"))),
        ],
        HarnessId::Muse => vec![
            ("XDG_CONFIG_HOME".into(), at(home.join("config"))),
            ("MUSE_AUTH_PATH".into(), at(home.join("config").join("muse").join("auth.json"))),
            ("TBH_CREDENTIAL_BACKEND".into(), "file".into()),
        ],
        // The SDK's store lives under `$HOME`, which also finds the agent:
        // its key goes as the variable a run takes instead.
        HarnessId::Cursor => std::fs::read(home.join("auth.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|store| store.get("apiKey")?.as_str().map(str::to_owned))
            .map(|key| vec![("CURSOR_API_KEY".to_string(), OsString::from(key))])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an agent whose Cloud logins all stand for one provider: the new
    /// account is then all Cloud runs it on.
    #[test]
    fn single_provider_agents_discover_at_sign_in() {
        for id in [HarnessId::Antigravity, HarnessId::Grok, HarnessId::Muse, HarnessId::Cursor, HarnessId::Devin] {
            assert!(discovers_at_sign_in(id), "{id:?}");
        }
        for id in [HarnessId::Pi, HarnessId::Opencode, HarnessId::Hermes, HarnessId::ClaudeCode, HarnessId::Codex, HarnessId::Mock] {
            assert!(!discovers_at_sign_in(id), "{id:?}");
        }
    }

    /// A child that lists models prints them; one that fails, prints
    /// nothing usable or hangs gives none.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_child_s_live_list_or_none() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let lists = script(
            "lists",
            r#"[ "$1 $2" = "discover-models grok" ] && [ "$GROK_HOME" = "/h" ] && echo '[{"id":"grok-4.7","label":"Grok 4.7"}]'"#,
        );
        let env = vec![("GROK_HOME".to_string(), OsString::from("/h"))];
        let models = discover_with(&lists, HarnessId::Grok, env.clone()).await.unwrap();
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["grok-4.7"]);
        assert!(discover_with(&lists, HarnessId::Grok, Vec::new()).await.is_none(), "not pointed at the home");
        assert!(discover_with(&script("fails", "exit 1"), HarnessId::Grok, env.clone()).await.is_none());
        assert!(discover_with(&script("empty", "echo '[]'"), HarnessId::Grok, env.clone()).await.is_none());
        assert!(discover_with(&script("junk", "echo nope"), HarnessId::Grok, env).await.is_none());
    }
}
