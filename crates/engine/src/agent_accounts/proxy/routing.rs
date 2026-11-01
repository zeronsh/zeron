//! When a rotating chat's agent may go through the proxy at all: only for a
//! subscription login of the CLI's own (an API-key login bills per token and
//! never rotates), and never over a base URL the user configured (a gateway,
//! a self-hosted endpoint: Zeron must not reroute it).

use std::path::Path;

use zeron_proto::HarnessId;

use super::super::{AgentAccounts, AgentAuthKind, read_json, str_field};

impl AgentAccounts {
    /// Whether the CLI's live login for `pool`'s provider is a subscription
    /// (OAuth) login: what the proxy can swap per request.
    pub(super) async fn live_login_is_subscription(
        &self,
        harness: HarnessId,
        pool: Option<&str>,
    ) -> bool {
        match (harness, pool) {
            (HarnessId::Codex, _) => self
                .detect_codex()
                .is_some_and(|d| d.profile.auth_kind == AgentAuthKind::Oauth),
            // Grok's logins are OIDC sessions.
            (HarnessId::Grok, _) => self.detect_grok().is_some(),
            (HarnessId::Opencode, Some(pool)) => self.opencode_live_entry(pool).is_some(),
            (HarnessId::Pi, Some(pool)) => self.live_keyed_entry(harness, pool).is_some(),
            _ => false,
        }
    }

    /// OpenCode's live OAuth entry for `store_key`, in `auth.json`'s shape.
    /// OpenCode 2.x keeps its logins in `opencode.db` (it imports `auth.json`
    /// once, then never reads it again), so when that database exists it is
    /// the truth, read read-only; otherwise `auth.json`.
    pub(super) fn opencode_live_entry(&self, store_key: &str) -> Option<serde_json::Value> {
        let db = self
            .inner
            .config
            .opencode_auth_file
            .with_file_name("opencode.db");
        if !db.exists() {
            return self.live_keyed_entry(HarnessId::Opencode, store_key);
        }
        match opencode_db_entry(&db, store_key) {
            Ok(entry) => entry,
            Err(err) => {
                // Unreadable: treat as no subscription login (no routing).
                tracing::debug!(error = %err, "opencode.db unreadable");
                None
            }
        }
    }

    /// Whether the user pointed `pool`'s provider at an endpoint of their
    /// own: then Zeron leaves the agent's traffic alone. `cwd` adds the
    /// chat's project config where the CLI reads one.
    pub(super) fn custom_endpoint(
        &self,
        harness: HarnessId,
        pool: Option<&str>,
        cwd: Option<&str>,
    ) -> bool {
        let config = &self.inner.config;
        match harness {
            HarnessId::Codex => codex_custom_endpoint(&config.codex_home.join("config.toml")),
            HarnessId::Grok => {
                std::env::var_os("GROK_CLI_CHAT_PROXY_BASE_URL").is_some_and(|v| !v.is_empty())
                    || toml_file(&config.grok_home.join("config.toml")).is_some_and(|c| {
                        c.get("endpoints")
                            .and_then(|e| e.get("cli_chat_proxy_base_url"))
                            .is_some()
                    })
            }
            HarnessId::Opencode => {
                let inline = std::env::var("OPENCODE_CONFIG_CONTENT").ok();
                let global = config.muse_config_root.join("opencode");
                let mut files = vec![
                    global.join("opencode.json"),
                    global.join("opencode.jsonc"),
                    global.join("config.json"),
                ];
                if let Some(cwd) = cwd.filter(|c| !c.is_empty()).map(Path::new) {
                    for dir in [cwd.to_path_buf(), cwd.join(".opencode")] {
                        files.push(dir.join("opencode.json"));
                        files.push(dir.join("opencode.jsonc"));
                    }
                }
                inline.as_deref().is_some_and(opencode_custom_endpoint)
                    || files.iter().any(|file| {
                        std::fs::read_to_string(file)
                            .is_ok_and(|raw| opencode_custom_endpoint(&raw))
                    })
            }
            HarnessId::Pi => pool.is_some_and(|pool| {
                read_json(&config.pi_agent_dir.join("models.json")).is_some_and(|models| {
                    models
                        .get("providers")
                        .and_then(|p| p.get(pool))
                        .and_then(|p| str_field(p, "baseUrl"))
                        .is_some()
                })
            }),
            _ => false,
        }
    }
}

fn toml_file(file: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(file).ok()?.parse().ok()
}

/// Codex config that sends the built-in OpenAI provider elsewhere or picks
/// another provider: `openai_base_url`, a `model_provider` other than
/// `openai` (also in the active profile), or an `openai` provider override.
fn codex_custom_endpoint(file: &Path) -> bool {
    let Some(config) = toml_file(file) else {
        // Unparseable but present: don't guess.
        return file.exists();
    };
    let elsewhere = |table: &toml::Table| {
        table.contains_key("openai_base_url")
            || table
                .get("model_provider")
                .and_then(|p| p.as_str())
                .is_some_and(|p| p != "openai")
    };
    let profile = config
        .get("profile")
        .and_then(|p| p.as_str())
        .and_then(|name| config.get("profiles")?.get(name)?.as_table());
    elsewhere(&config)
        || profile.is_some_and(elsewhere)
        || config
            .get("model_providers")
            .and_then(|p| p.get("openai"))
            .is_some()
}

/// An OpenCode config (JSON or JSONC) that sets the ChatGPT provider's base
/// URL, in the 2.x (`providers.openai.settings.baseURL`) or 1.x
/// (`provider.openai.options.baseURL`) shape. JSONC that plain JSON can't
/// read counts when it mentions `baseURL` at all: never reroute on a guess.
fn opencode_custom_endpoint(raw: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(config) => {
            config
                .pointer("/providers/openai/settings/baseURL")
                .is_some()
                || config.pointer("/provider/openai/options/baseURL").is_some()
        }
        Err(_) => raw.contains("baseURL"),
    }
}

/// The active OAuth credential of `integration` in OpenCode 2.x's database
/// (`credential` table, value `{type, access, refresh, expires, metadata}`),
/// in `auth.json`'s entry shape.
fn opencode_db_entry(db: &Path, integration: &str) -> rusqlite::Result<Option<serde_json::Value>> {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(250))?;
    let mut statement = conn.prepare(
        "SELECT value FROM credential WHERE integration_id = ?1 \
         ORDER BY COALESCE(active, 0) DESC LIMIT 1",
    )?;
    let mut rows = statement.query([integration])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let raw: String = row.get(0)?;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    if value.get("type").and_then(|t| t.as_str()) != Some("oauth") {
        return Ok(None);
    }
    let mut entry = serde_json::json!({
        "type": "oauth",
        "access": value.get("access").cloned().unwrap_or_default(),
        "refresh": value.get("refresh").cloned().unwrap_or_default(),
        "expires": value.get("expires").cloned().unwrap_or_default(),
    });
    if let Some(account) = value
        .pointer("/metadata/accountID")
        .and_then(|v| v.as_str())
    {
        entry["accountId"] = account.into();
    }
    Ok(Some(entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_endpoints_of_the_users_own_are_recognized() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        assert!(!codex_custom_endpoint(&file));
        for (config, custom) in [
            ("model = \"gpt\"\n", false),
            ("model_provider = \"openai\"\n", false),
            ("openai_base_url = \"https://gw.example/v1\"\n", true),
            ("model_provider = \"azure\"\n", true),
            (
                "[model_providers.openai]\nbase_url = \"https://gw\"\n",
                true,
            ),
            (
                "profile = \"work\"\n[profiles.work]\nmodel_provider = \"ollama\"\n",
                true,
            ),
            ("not = [valid toml", true),
        ] {
            std::fs::write(&file, config).unwrap();
            assert_eq!(codex_custom_endpoint(&file), custom, "{config}");
        }
    }

    #[test]
    fn opencode_endpoints_of_the_users_own_are_recognized() {
        assert!(!opencode_custom_endpoint(r#"{"model":"openai/gpt"}"#));
        assert!(opencode_custom_endpoint(
            r#"{"providers":{"openai":{"settings":{"baseURL":"https://gw"}}}}"#
        ));
        assert!(opencode_custom_endpoint(
            r#"{"provider":{"openai":{"options":{"baseURL":"https://gw"}}}}"#
        ));
        assert!(!opencode_custom_endpoint(
            r#"{"provider":{"anthropic":{"options":{"baseURL":"https://gw"}}}}"#
        ));
        assert!(opencode_custom_endpoint("// jsonc\n{\"baseURL\": \"x\"}"));
        assert!(!opencode_custom_endpoint("// jsonc\n{\"model\": \"x\"}"));
    }

    #[test]
    fn opencode_2_logins_are_read_from_its_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE credential (id TEXT PRIMARY KEY, integration_id TEXT, label TEXT NOT NULL,
               value TEXT NOT NULL, connector_id TEXT, method_id TEXT, active INTEGER);",
        )
        .unwrap();
        let row = |id: &str, integration: &str, value: serde_json::Value, active: i64| {
            conn.execute(
                "INSERT INTO credential (id, integration_id, label, value, active) VALUES (?1, ?2, 'x', ?3, ?4)",
                rusqlite::params![id, integration, value.to_string(), active],
            )
            .unwrap();
        };
        row(
            "1",
            "openai",
            serde_json::json!({"type":"oauth","methodID":"m","access":"old","refresh":"r","expires":1}),
            0,
        );
        row(
            "2",
            "openai",
            serde_json::json!({"type":"oauth","methodID":"m","access":"a","refresh":"r","expires":5,
                "metadata":{"accountID":"ws-1"}}),
            1,
        );
        row(
            "3",
            "anthropic",
            serde_json::json!({"type":"key","key":"sk"}),
            1,
        );
        let entry = opencode_db_entry(&db, "openai").unwrap().unwrap();
        assert_eq!(
            entry,
            serde_json::json!({"type":"oauth","access":"a","refresh":"r","expires":5,"accountId":"ws-1"})
        );
        // An API key is no subscription login; an absent one is nothing.
        assert_eq!(opencode_db_entry(&db, "anthropic").unwrap(), None);
        assert_eq!(opencode_db_entry(&db, "github-copilot").unwrap(), None);
    }
}
