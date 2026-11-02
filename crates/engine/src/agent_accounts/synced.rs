//! The local half of synced subscriptions ([`crate::account_sync`]): what
//! the vault needs from this device's Claude Code / Codex logins, and how a
//! vault grant becomes the CLI's live login.
//!
//! A synced login refreshes ONLY in the vault. So once a local login is
//! uploaded, its slot is deleted (a slot is a second refresher: usage probes
//! and switches refresh saved tokens) and the CLI's own store gets the
//! grant's access token with an EMPTY refresh token — the shape the Cloud
//! broker writes (see [`crate::credentials`]). A login the CLI wrote itself
//! (a real refresh token) is therefore always one this device hasn't
//! uploaded yet.

use zeron_proto::{AgentAuthKind, HarnessId};

use super::{
    AgentAccounts, Slot, compose_claude_credentials, lock, parse_codex_auth, read_json,
    remove_if_exists, str_field, write_file_atomic,
};
use crate::EngineError;

/// A Claude Code / Codex subscription login on this device that the vault
/// doesn't hold yet (a real refresh token).
#[derive(Clone)]
pub(crate) struct LocalLogin {
    pub harness: HarnessId,
    /// Its saved slot (`None` for a live login no slot holds yet).
    pub slot_id: Option<String>,
    pub email: String,
    pub display_name: Option<String>,
    pub organization: Option<String>,
    pub plan: Option<String>,
    /// Whether it is the CLI's live login.
    pub live: bool,
    /// Claude: the `claudeAiOauth` object. Codex: the whole `auth.json`.
    pub credential: serde_json::Value,
    /// Claude: `{oauthAccount, userID}` — identity merged into
    /// `~/.claude.json` when the login is installed from a grant.
    pub claude_config: Option<serde_json::Value>,
}

impl std::fmt::Debug for LocalLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalLogin")
            .field("harness", &self.harness)
            .field("slot_id", &self.slot_id)
            .field("email", &self.email)
            .field("live", &self.live)
            .finish_non_exhaustive()
    }
}

/// Whether a Claude `claudeAiOauth` object / Codex `auth.json` carries a
/// refresh token of its own (a login the CLI or a sign-in wrote, not a
/// vault grant).
pub(crate) fn has_refresh_token(harness: HarnessId, credential: &serde_json::Value) -> bool {
    let token = match harness {
        HarnessId::ClaudeCode => credential.get("refreshToken"),
        HarnessId::Codex => credential
            .get("tokens")
            .and_then(|t| t.get("refresh_token")),
        _ => None,
    };
    token
        .and_then(|v| v.as_str())
        .is_some_and(|t| !t.trim().is_empty())
}

/// The two agents whose logins sync.
pub(crate) const SYNCED_HARNESSES: [HarnessId; 2] = [HarnessId::ClaudeCode, HarnessId::Codex];

/// Exclusive hold on `harness`'s logins for a read-upload-replace sequence:
/// the accounts' operation lock and, for Claude, Claude Code's own refresh
/// lock (taken in that order, like a switch), so neither a list, a switch
/// nor the CLI itself rotates the tokens between the read and the install.
pub(crate) struct Handoff<'a> {
    harness: HarnessId,
    _ops: tokio::sync::MutexGuard<'a, ()>,
    _claude: Option<zeron_harness::claude::refresh_lock::RefreshLock>,
}

impl AgentAccounts {
    pub(crate) async fn handoff(&self, harness: HarnessId) -> Result<Handoff<'_>, EngineError> {
        let ops = self.inner.ops.lock().await;
        let claude = match harness {
            HarnessId::ClaudeCode => Some(self.claude_refresh_lock().await?),
            _ => None,
        };
        Ok(Handoff {
            harness,
            _ops: ops,
            _claude: claude,
        })
    }

    /// Every Claude Code / Codex subscription login on this device that has
    /// its own refresh token: the live one (snapshotted first) and every
    /// saved slot. API keys and grant-installed logins are left out.
    pub(crate) async fn syncable_logins(
        &self,
        hold: &Handoff<'_>,
    ) -> Result<Vec<LocalLogin>, EngineError> {
        let harness = hold.harness;
        // Fresh tokens, never a cached read: the CLI may have rotated them.
        *lock(&self.inner.claude_credentials) = None;
        // Snapshots the live login into its slot (unless it's older).
        self.list_locked(false).await?;
        let live_key = self.live_account_key(harness, None).await;
        let mut out = Vec::new();
        for slot in self.read_slots(harness) {
            if slot.profile.auth_kind != AgentAuthKind::Oauth {
                continue;
            }
            let credential = match harness {
                HarnessId::ClaudeCode => match slot.credentials.get("claudeAiOauth") {
                    Some(oauth) => oauth.clone(),
                    None => continue,
                },
                _ => slot.credentials.clone(),
            };
            if !has_refresh_token(harness, &credential) {
                continue;
            }
            out.push(LocalLogin {
                harness,
                live: live_key.as_deref() == Some(slot.account_key.as_str()),
                slot_id: Some(slot.id.clone()),
                email: slot.profile.email.clone(),
                display_name: slot.profile.display_name.clone(),
                organization: slot.profile.organization.clone(),
                plan: slot.profile.plan.clone(),
                credential,
                claude_config: slot.claude_config.clone(),
            });
        }
        Ok(out)
    }

    /// The live login's identity and whether it is a grant (no refresh
    /// token of its own): `(email, organization, refreshable)`.
    pub(crate) async fn live_login(
        &self,
        harness: HarnessId,
    ) -> Option<(String, Option<String>, bool)> {
        match harness {
            HarnessId::ClaudeCode => {
                let cfg = read_json(&self.inner.config.claude_config_file)?;
                let oauth = cfg.get("oauthAccount")?;
                let email = str_field(oauth, "emailAddress")?;
                let (creds, _) = self.read_claude_credentials().await;
                let refreshable = creds
                    .as_ref()
                    .and_then(|c| c.get("claudeAiOauth"))
                    .is_some_and(|o| has_refresh_token(harness, o));
                Some((email, str_field(oauth, "organizationName"), refreshable))
            }
            HarnessId::Codex => {
                let auth = read_json(&self.inner.config.codex_auth_file())?;
                let refreshable = has_refresh_token(harness, &auth);
                let detected = parse_codex_auth(auth)?;
                (detected.profile.auth_kind == AgentAuthKind::Oauth).then_some((
                    detected.profile.email,
                    None,
                    refreshable,
                ))
            }
            _ => None,
        }
    }

    /// The live login's access token and its expiry (Unix ms; `None` when
    /// it doesn't say).
    #[cfg(test)]
    pub(crate) async fn live_access_token(
        &self,
        harness: HarnessId,
    ) -> Option<(String, Option<i64>)> {
        match harness {
            HarnessId::ClaudeCode => {
                let (creds, _) = self.read_claude_credentials().await;
                let oauth = creds?.get("claudeAiOauth")?.clone();
                let token = str_field(&oauth, "accessToken")?;
                let expires = oauth
                    .get("expiresAt")
                    .and_then(|v| v.as_i64())
                    .filter(|ms| *ms > 0);
                Some((token, expires))
            }
            HarnessId::Codex => {
                let auth = read_json(&self.inner.config.codex_auth_file())?;
                let token = str_field(auth.get("tokens")?, "access_token")?;
                Some((token, None))
            }
            _ => None,
        }
    }

    /// Delete saved slots the vault now holds (a slot is a second refresher).
    pub(crate) fn drop_slots(&self, harness: HarnessId, ids: &[String]) -> Result<(), EngineError> {
        let dir = self.slots_dir(harness)?;
        for id in ids {
            // Ids come from `read_slots`, never from the wire; still refuse
            // anything that isn't a plain slot id.
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
                continue;
            }
            remove_if_exists(&dir.join(format!("{id}.json")))?;
        }
        Ok(())
    }

    /// Delete the slots auto-snapshotted from grant-installed logins (no
    /// refresh token): they can't refresh, so once sync is off they would
    /// only list logins that stop working.
    pub(crate) fn drop_grant_slots(&self, harness: HarnessId) -> Result<(), EngineError> {
        let stale: Vec<String> = self
            .read_slots(harness)
            .into_iter()
            .filter(|slot: &Slot| {
                slot.profile.auth_kind == AgentAuthKind::Oauth && {
                    let credential = match harness {
                        HarnessId::ClaudeCode => slot
                            .credentials
                            .get("claudeAiOauth")
                            .cloned()
                            .unwrap_or_default(),
                        _ => slot.credentials.clone(),
                    };
                    !has_refresh_token(harness, &credential)
                }
            })
            .map(|slot| slot.id)
            .collect();
        self.drop_slots(harness, &stale)
    }

    /// Make a vault grant Claude Code's live login: `claudeAiOauth` (empty
    /// refresh token) composed with the machine-shared MCP/plugin entries
    /// already there, and the account's identity merged into
    /// `~/.claude.json`.
    pub(crate) async fn install_claude_grant(
        &self,
        hold: &Handoff<'_>,
        oauth: serde_json::Value,
        claude_config: &serde_json::Value,
    ) -> Result<(), EngineError> {
        debug_assert_eq!(hold.harness, HarnessId::ClaudeCode);
        let (live, _) = self.read_claude_credentials().await;
        let target = serde_json::json!({ "claudeAiOauth": oauth });
        let credentials = compose_claude_credentials(&target, live.as_ref());
        // Nothing else the CLI keeps in the blob may go missing.
        let credentials = match (credentials, live) {
            (serde_json::Value::Object(mut composed), Some(serde_json::Value::Object(live))) => {
                for (key, value) in live {
                    composed.entry(key).or_insert(value);
                }
                serde_json::Value::Object(composed)
            }
            (composed, _) => composed,
        };
        self.write_claude_credentials(&credentials).await?;
        *lock(&self.inner.claude_credentials) = None;
        // Identity: only `oauthAccount` / `userID` change; a file that exists
        // but doesn't parse is never rewritten (it holds the user's config).
        let file = &self.inner.config.claude_config_file;
        let cfg = read_json(file);
        if cfg.is_none() && file.exists() {
            return Err(EngineError::Other(
                "~/.claude.json exists but could not be parsed — fix or remove it, then retry."
                    .into(),
            ));
        }
        // Claude Code rewrites this file all the time: touch it only when the
        // identity actually changes (a re-grant of the same login doesn't).
        let account = claude_config.get("oauthAccount");
        let user_id = claude_config.get("userID").filter(|v| v.is_string());
        let unchanged = cfg.as_ref().is_some_and(|cfg| {
            account.is_none_or(|a| cfg.get("oauthAccount") == Some(a))
                && user_id.is_none_or(|u| cfg.get("userID") == Some(u))
        });
        if unchanged {
            return Ok(());
        }
        let mut merged = cfg.unwrap_or_else(|| serde_json::json!({}));
        if let Some(map) = merged.as_object_mut() {
            if let Some(account) = account {
                map.insert("oauthAccount".into(), account.clone());
            }
            // An identity without a `userID` keeps the file's own.
            if let Some(user_id) = user_id {
                map.insert("userID".into(), user_id.clone());
            }
        }
        write_file_atomic(file, merged.to_string().as_bytes(), false)
    }

    /// Whether the live login carries a refresh token of its own (the user
    /// signed in by themselves; never overwrite it with a grant).
    pub(crate) async fn live_has_refresh_token(&self, hold: &Handoff<'_>) -> bool {
        match hold.harness {
            HarnessId::ClaudeCode => {
                *lock(&self.inner.claude_credentials) = None;
                let (creds, _) = self.read_claude_credentials().await;
                creds
                    .as_ref()
                    .and_then(|c| c.get("claudeAiOauth"))
                    .is_some_and(|o| has_refresh_token(hold.harness, o))
            }
            HarnessId::Codex => read_json(&self.inner.config.codex_auth_file())
                .is_some_and(|auth| has_refresh_token(hold.harness, &auth)),
            _ => false,
        }
    }

    /// Blank the live login's refresh token, keeping its access token: from
    /// here the CLI can't refresh (the vault is about to), yet keeps working
    /// until the grant replaces it. Done BEFORE the upload, so no failure
    /// after it can leave two refreshers.
    pub(crate) async fn neuter_live(&self, hold: &Handoff<'_>) -> Result<(), EngineError> {
        match hold.harness {
            HarnessId::ClaudeCode => {
                *lock(&self.inner.claude_credentials) = None;
                let (live, _) = self.read_claude_credentials().await;
                let Some(mut credentials) = live else { return Ok(()) };
                let Some(oauth) = credentials
                    .get_mut("claudeAiOauth")
                    .and_then(|o| o.as_object_mut())
                else {
                    return Ok(());
                };
                oauth.insert("refreshToken".into(), "".into());
                self.write_claude_credentials(&credentials).await?;
                *lock(&self.inner.claude_credentials) = None;
                Ok(())
            }
            HarnessId::Codex => {
                let file = self.inner.config.codex_auth_file();
                let Some(mut auth) = read_json(&file) else { return Ok(()) };
                let Some(tokens) = auth.get_mut("tokens").and_then(|t| t.as_object_mut()) else {
                    return Ok(());
                };
                tokens.insert("refresh_token".into(), "".into());
                let json = serde_json::to_string_pretty(&auth)
                    .map_err(|e| EngineError::Other(format!("serialize codex auth: {e}")))?;
                write_file_atomic(&file, json.as_bytes(), true)
            }
            _ => Ok(()),
        }
    }

    /// Undo [`Self::neuter_live`] after a failed upload: the login is the
    /// CLI's own again (`credential` as [`LocalLogin::credential`]).
    pub(crate) async fn restore_live(
        &self,
        hold: &Handoff<'_>,
        credential: &serde_json::Value,
    ) -> Result<(), EngineError> {
        match hold.harness {
            HarnessId::ClaudeCode => {
                let (live, _) = self.read_claude_credentials().await;
                let mut credentials = live.unwrap_or_else(|| serde_json::json!({}));
                if let Some(map) = credentials.as_object_mut() {
                    map.insert("claudeAiOauth".into(), credential.clone());
                }
                self.write_claude_credentials(&credentials).await?;
                *lock(&self.inner.claude_credentials) = None;
                Ok(())
            }
            HarnessId::Codex => {
                let json = serde_json::to_string_pretty(credential)
                    .map_err(|e| EngineError::Other(format!("serialize codex auth: {e}")))?;
                write_file_atomic(&self.inner.config.codex_auth_file(), json.as_bytes(), true)
            }
            _ => Ok(()),
        }
    }

    /// Whether a saved slot still holds a refresh token (a login not handed
    /// to the vault yet). Reads slot files only — cheap enough per tick.
    pub(crate) fn has_syncable_slots(&self, harness: HarnessId) -> bool {
        self.read_slots(harness).iter().any(|slot| {
            slot.profile.auth_kind == AgentAuthKind::Oauth && {
                let credential = match harness {
                    HarnessId::ClaudeCode => slot.credentials.get("claudeAiOauth"),
                    _ => Some(&slot.credentials),
                };
                credential.is_some_and(|c| has_refresh_token(harness, c))
            }
        })
    }

    /// Make a vault grant Codex's live login (`auth.json`, empty refresh
    /// token). Left alone when it already holds the same tokens.
    pub(crate) fn install_codex_grant(
        &self,
        hold: &Handoff<'_>,
        auth: &serde_json::Value,
    ) -> Result<(), EngineError> {
        debug_assert_eq!(hold.harness, HarnessId::Codex);
        let file = self.inner.config.codex_auth_file();
        if read_json(&file).is_some_and(|current| current.get("tokens") == auth.get("tokens")) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.inner.config.codex_home)?;
        let json = serde_json::to_string_pretty(auth)
            .map_err(|e| EngineError::Other(format!("serialize codex auth: {e}")))?;
        write_file_atomic(&file, json.as_bytes(), true)
    }

    /// Whether Codex's live login is the user's own API key (sync never
    /// replaces one with a subscription login unasked).
    pub(crate) fn live_codex_api_key(&self) -> bool {
        read_json(&self.inner.config.codex_auth_file())
            .is_some_and(|auth| str_field(&auth, "OPENAI_API_KEY").is_some())
    }

    /// `{oauthAccount}` for a Claude login installed from a grant, from
    /// Claude's profile endpoint (the identity Claude Code itself records at
    /// sign-in). `None` when the profile can't be read.
    pub(crate) async fn claude_identity(&self, access_token: &str) -> Option<serde_json::Value> {
        let profile: serde_json::Value = self
            .inner
            .http
            .get(&self.inner.endpoints.claude_profile)
            .bearer_auth(access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await
            .ok()
            .filter(|res| res.status().is_success())?
            .json()
            .await
            .ok()?;
        let empty = serde_json::json!({});
        let account = profile.get("account").unwrap_or(&empty);
        let org = profile.get("organization").unwrap_or(&empty);
        let email = str_field(account, "email_address")?;
        let mut oauth_account = serde_json::json!({
            "accountUuid": str_field(account, "uuid").unwrap_or_else(|| email.clone()),
            "emailAddress": email,
            "organizationUuid": str_field(org, "uuid"),
            "organizationName": str_field(org, "name"),
            "displayName": str_field(account, "display_name")
                .or_else(|| str_field(account, "full_name")),
        });
        if let Some(map) = oauth_account.as_object_mut() {
            if let Some(kind) = str_field(org, "organization_type") {
                map.insert("organizationType".into(), kind.into());
            }
            if let Some(tier) = str_field(org, "rate_limit_tier") {
                map.insert("organizationRateLimitTier".into(), tier.into());
            }
        }
        Some(serde_json::json!({ "oauthAccount": oauth_account }))
    }

    /// Sign the CLI out of a synced login that was removed from the vault.
    /// Only a grant-installed login (no refresh token) is touched — one the
    /// user signed in to by themselves since stays.
    pub(crate) async fn sign_out_grant(&self, harness: HarnessId) -> Result<(), EngineError> {
        let _hold = self.handoff(harness).await?;
        match harness {
            HarnessId::ClaudeCode => {
                let (live, _) = self.read_claude_credentials().await;
                let Some(mut credentials) = live else {
                    return Ok(());
                };
                let grant = credentials
                    .get("claudeAiOauth")
                    .is_some_and(|o| !has_refresh_token(harness, o));
                if !grant {
                    return Ok(());
                }
                if let Some(map) = credentials.as_object_mut() {
                    map.remove("claudeAiOauth");
                }
                self.write_claude_credentials(&credentials).await?;
                let file = &self.inner.config.claude_config_file;
                if let Some(mut cfg) = read_json(file)
                    && let Some(map) = cfg.as_object_mut()
                    && map.remove("oauthAccount").is_some()
                {
                    write_file_atomic(file, cfg.to_string().as_bytes(), false)?;
                }
                *lock(&self.inner.claude_credentials) = None;
                Ok(())
            }
            HarnessId::Codex => {
                let file = self.inner.config.codex_auth_file();
                if read_json(&file).is_some_and(|auth| {
                    auth.get("tokens").is_some() && !has_refresh_token(harness, &auth)
                }) {
                    remove_if_exists(&file)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_accounts::AgentAccountsConfig;

    fn accounts(root: &std::path::Path) -> AgentAccounts {
        AgentAccounts::new(AgentAccountsConfig::isolated(root))
    }

    fn jwt(claims: serde_json::Value) -> String {
        use base64::Engine as _;
        let enc = |v: &serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
        };
        format!(
            "{}.{}.sig",
            enc(&serde_json::json!({"alg": "none"})),
            enc(&claims)
        )
    }

    fn codex_auth(email: &str, refresh: &str) -> serde_json::Value {
        serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt(serde_json::json!({
                    "email": email,
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "ws-1",
                        "chatgpt_user_id": format!("user-{email}"),
                        "chatgpt_plan_type": "pro",
                    },
                })),
                "access_token": format!("at-{email}"),
                "refresh_token": refresh,
                "account_id": "ws-1",
            },
            "last_refresh": "2026-11-01T00:00:00Z",
        })
    }

    fn write(path: &std::path::Path, value: &serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    #[test]
    fn refresh_tokens_tell_own_logins_from_grants() {
        assert!(has_refresh_token(
            HarnessId::ClaudeCode,
            &serde_json::json!({"refreshToken": "rt"})
        ));
        assert!(!has_refresh_token(
            HarnessId::ClaudeCode,
            &serde_json::json!({"refreshToken": ""})
        ));
        assert!(has_refresh_token(
            HarnessId::Codex,
            &codex_auth("a@x.com", "rt")
        ));
        assert!(!has_refresh_token(
            HarnessId::Codex,
            &codex_auth("a@x.com", "")
        ));
        assert!(!has_refresh_token(
            HarnessId::Codex,
            &serde_json::json!({"OPENAI_API_KEY": "k"})
        ));
    }

    #[tokio::test]
    async fn codex_logins_with_refresh_tokens_are_syncable_and_grants_are_not() {
        let root = tempfile::tempdir().unwrap();
        let config = AgentAccountsConfig::isolated(root.path());
        let accounts = accounts(root.path());
        // The live login: the user's own sign-in.
        write(
            &config.codex_home.join("auth.json"),
            &codex_auth("live@x.com", "rt-live"),
        );
        let logins = {
            let hold = accounts.handoff(HarnessId::Codex).await.unwrap();
            accounts.syncable_logins(&hold).await.unwrap()
        };
        assert_eq!(logins.len(), 1);
        assert!(logins[0].live);
        assert_eq!(logins[0].email, "live@x.com");
        assert!(logins[0].slot_id.is_some());
        assert_eq!(
            accounts.live_login(HarnessId::Codex).await,
            Some(("live@x.com".into(), None, true))
        );

        // Uploaded: its slot goes, and a grant replaces the live login.
        {
            let hold = accounts.handoff(HarnessId::Codex).await.unwrap();
            accounts
                .drop_slots(HarnessId::Codex, &[logins[0].slot_id.clone().unwrap()])
                .unwrap();
            accounts
                .install_codex_grant(&hold, &codex_auth("live@x.com", ""))
                .unwrap();
        }
        assert!(!accounts.live_codex_api_key());
        assert_eq!(
            accounts.live_login(HarnessId::Codex).await,
            Some(("live@x.com".into(), None, false))
        );
        // The grant is snapshotted into a slot by the next list, but is
        // never offered for upload — and is swept by `drop_grant_slots`.
        {
            let hold = accounts.handoff(HarnessId::Codex).await.unwrap();
            assert!(accounts.syncable_logins(&hold).await.unwrap().is_empty());
        }
        assert!(!accounts.read_slots(HarnessId::Codex).is_empty());
        accounts.drop_grant_slots(HarnessId::Codex).unwrap();
        assert!(accounts.read_slots(HarnessId::Codex).is_empty());

        // Removing the synced login signs a grant out, never a real login.
        accounts.sign_out_grant(HarnessId::Codex).await.unwrap();
        assert!(!config.codex_home.join("auth.json").exists());
        write(
            &config.codex_home.join("auth.json"),
            &codex_auth("own@x.com", "rt"),
        );
        accounts.sign_out_grant(HarnessId::Codex).await.unwrap();
        assert!(config.codex_home.join("auth.json").exists());
    }

    #[tokio::test]
    async fn a_claude_grant_installs_beside_shared_mcp_tokens_with_its_identity() {
        let root = tempfile::tempdir().unwrap();
        let config = AgentAccountsConfig::isolated(root.path());
        let accounts = accounts(root.path());
        write(
            &config.claude_config_dir.join(".credentials.json"),
            &serde_json::json!({
                "claudeAiOauth": {"accessToken": "old", "refreshToken": "rt-old", "expiresAt": 1},
                "mcpOAuth": {"server": {"token": "keep"}},
            }),
        );
        write(
            &config.claude_config_file,
            &serde_json::json!({"theme": "dark", "oauthAccount": {"emailAddress": "old@x.com"}, "userID": "u-old"}),
        );
        let oauth = serde_json::json!({
            "accessToken": "granted",
            "refreshToken": "",
            "expiresAt": 4_102_444_800_000_i64,
            "scopes": ["user:inference"],
        });
        let identity = serde_json::json!({
            "oauthAccount": {"accountUuid": "acct-1", "emailAddress": "new@x.com"},
        });
        {
            let hold = accounts.handoff(HarnessId::ClaudeCode).await.unwrap();
            accounts
                .install_claude_grant(&hold, oauth, &identity)
                .await
                .unwrap();
        }
        let creds = read_json(&config.claude_config_dir.join(".credentials.json")).unwrap();
        assert_eq!(creds["claudeAiOauth"]["accessToken"], "granted");
        assert_eq!(creds["claudeAiOauth"]["refreshToken"], "");
        assert_eq!(creds["mcpOAuth"]["server"]["token"], "keep");
        let cfg = read_json(&config.claude_config_file).unwrap();
        assert_eq!(cfg["theme"], "dark");
        assert_eq!(cfg["oauthAccount"]["emailAddress"], "new@x.com");
        // An identity without a userID keeps the file's own.
        assert_eq!(cfg["userID"], "u-old");
        assert_eq!(
            accounts.live_login(HarnessId::ClaudeCode).await,
            Some(("new@x.com".into(), None, false))
        );
        assert_eq!(
            accounts.live_access_token(HarnessId::ClaudeCode).await,
            Some(("granted".into(), Some(4_102_444_800_000)))
        );
        accounts
            .sign_out_grant(HarnessId::ClaudeCode)
            .await
            .unwrap();
        let creds = read_json(&config.claude_config_dir.join(".credentials.json")).unwrap();
        assert!(creds.get("claudeAiOauth").is_none());
        assert_eq!(creds["mcpOAuth"]["server"]["token"], "keep");
        assert!(
            read_json(&config.claude_config_file)
                .unwrap()
                .get("oauthAccount")
                .is_none()
        );
    }
}
