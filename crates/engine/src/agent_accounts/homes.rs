//! Accounts per chat: a chat may name one of the saved Claude Code or Codex
//! logins (`ChatConfig::account`) and run on it, at the same time as other
//! chats run on theirs or on the machine's active login.
//!
//! Each saved account that a chat runs on gets ONE config dir of its own,
//! never one per chat: `{data_dir}/agent-homes/claude-code/<slot id>/` (the
//! run's `CLAUDE_CONFIG_DIR`) and `{data_dir}/agent-homes/codex/<slot id>/`
//! (its `CODEX_HOME`). Claude Code keeps that dir's login in its own Keychain
//! item (`Claude Code-credentials-<sha256(dir)[..8]>`) on macOS, or in the
//! dir's `.credentials.json`; Codex in the dir's `auth.json`.
//!
//! **One live copy of a login.** Refresh tokens are single-use: two stores
//! refreshing copies of one login log each other out at the first refresh.
//! So at any time exactly one store holds an account's login live:
//!
//! - The account is the machine's active login, and its dir holds none: its
//!   chats run on the machine's login, exactly as unpinned chats do.
//! - Otherwise its dir holds it: seeded once from the saved slot (which is
//!   only a backup, never refreshed by the engine while the dir holds the
//!   login), then refreshed by the CLI in place. Every list copies the dir's
//!   newest pair back into the slot.
//! - Making the account the machine's active login MOVES its login: the
//!   dir's newest pair is installed as the machine's login under both refresh
//!   locks, and the dir's copy is deleted. Its chats' credential fingerprint
//!   changes with it (`…@home` → `…@machine`), so a parked session is
//!   replaced before its next turn. A turn already running keeps its process
//!   until it ends.
//! - Making ANOTHER account the machine's login snapshots this one into its
//!   slot first (as every switch does); the dir is seeded from that slot at
//!   its next run.
//! - Removing the account deletes its dir (and its Keychain item); a chat
//!   still naming it is refused with a way out.
//!
//! **Shared configuration.** An account dir mirrors the user's own config
//! dir (`~/.claude`, `~/.codex`): a link to every entry there except the
//! login itself and its lock, refreshed at every spawn. Settings, CLAUDE.md,
//! skills, plugins, agents and the session history (`projects/`,
//! `sessions/`) are the user's own, so an agent behaves the same whichever
//! account runs it, and a chat resumes its session after changing account.
//! Claude Code's `.claude.json` (user MCP servers, project settings, the
//! account's identity) is written per dir at every spawn: the user's own
//! file, with the account's identity and account-scoped caches kept. (What
//! Claude Code writes into a dir's `.claude.json` during a run is replaced by
//! the user's file at the next spawn, except those account keys.) MCP
//! sign-ins (`mcpOAuth`) are NOT copied into a dir: their refresh tokens
//! rotate too, so a pinned chat signs MCP servers in again on its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zeron_proto::{AgentUsageWindow, HarnessId};

use super::{
    AgentAccounts, CLAUDE_REFRESH_LOCK_WAIT, CLAUDE_SHARED_CREDENTIAL_KEYS, EngineError, Slot,
    UsageSnapshot, claude_login_is_older, lock, now_ms, parse_codex_auth, private_dir, read_json,
    remove_if_exists, str_field, with_claude_ai_oauth, write_file_atomic,
};

/// Prefix of a pinned run's credential fingerprint
/// (`agent-account:<slot id>@home|@machine`).
pub const PINNED_CREDENTIAL: &str = "agent-account:";
const HOMES_DIR: &str = "agent-homes";

/// What a chat naming a removed account is told.
const REMOVED: &str = "The account this chat uses was removed from this computer. Choose \
                       another account for this chat, or use the computer's own.";

/// Where a pinned account's login lives now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// Its dir (seeded or about to be).
    Home,
    /// The machine's active login.
    Machine,
}

impl Holder {
    fn fingerprint(self, id: &str) -> String {
        match self {
            Holder::Home => format!("{PINNED_CREDENTIAL}{id}@home"),
            Holder::Machine => format!("{PINNED_CREDENTIAL}{id}@machine"),
        }
    }
}

/// What an account dir holds of its login.
enum HomeLogin {
    None,
    Readable(serde_json::Value),
    /// A login is there but could not be read (the Keychain refused):
    /// treated as held, never seeded over.
    Unreadable,
}

fn is_slot_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn pinnable(harness: HarnessId) -> bool {
    matches!(harness, HarnessId::ClaudeCode | HarnessId::Codex)
}

/// Keys of Claude Code's `.claude.json` that belong to the signed-in
/// account, not to the user: kept per account dir, never taken from the
/// user's own file.
fn claude_json_key_is_own(key: &str) -> bool {
    matches!(
        key,
        "oauthAccount"
            | "userID"
            | "hasAvailableSubscription"
            | "subscriptionNoticeCount"
            | "claudeCodeFirstTokenDate"
            | "hasOpusPlanDefault"
            | "primaryApiKey"
            | "customApiKeyResponses"
    ) || key.starts_with("cached")
        || key.ends_with("Cache")
        || key.to_ascii_lowercase().contains("subscription")
}

/// An account dir's `.claude.json`: the user's own `real` file, with the
/// account's keys from the dir's `own` file kept, and the account's
/// identity from its slot when the dir has none yet.
fn claude_home_config(
    real: Option<serde_json::Value>,
    own: Option<serde_json::Value>,
    slot: &Slot,
) -> serde_json::Value {
    let mut merged = real
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let map = merged.as_object_mut().expect("an object");
    map.retain(|key, _| !claude_json_key_is_own(key));
    if let Some(own) = own.as_ref().and_then(|own| own.as_object()) {
        for (key, value) in own {
            if claude_json_key_is_own(key) {
                map.insert(key.clone(), value.clone());
            }
        }
    }
    if !map.contains_key("oauthAccount") {
        let identity = slot.claude_config.as_ref();
        map.insert(
            "oauthAccount".into(),
            identity
                .and_then(|c| c.get("oauthAccount").cloned())
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "accountUuid": slot.account_key,
                        "emailAddress": slot.profile.email,
                        "organizationName": slot.profile.organization,
                        "displayName": slot.profile.display_name,
                    })
                }),
        );
        if let Some(user_id) = identity
            .and_then(|c| c.get("userID").cloned())
            .filter(serde_json::Value::is_string)
        {
            map.insert("userID".into(), user_id);
        }
    }
    merged
}

/// Make `into` mirror `from`: a link to every entry of `from` that `skip`
/// doesn't keep out. Stale links are pruned; real entries in `into` (the
/// account's own login, anything the CLI wrote in place of a link) are left
/// alone.
#[cfg(unix)]
fn mirror_dir(from: &Path, into: &Path, skip: impl Fn(&str) -> bool) -> std::io::Result<()> {
    let wanted: std::collections::HashSet<std::ffi::OsString> = match std::fs::read_dir(from) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| !skip(&name.to_string_lossy()))
            .collect(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => return Err(err),
    };
    for entry in std::fs::read_dir(into)?.flatten() {
        let name = entry.file_name();
        let is_link = entry.file_type().is_ok_and(|kind| kind.is_symlink());
        if is_link && !wanted.contains(&name) {
            std::fs::remove_file(entry.path())?;
        }
    }
    for name in wanted {
        let link = into.join(&name);
        let target = from.join(&name);
        match std::fs::symlink_metadata(&link) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
                    continue;
                }
                std::fs::remove_file(&link)?;
            }
            Ok(_) => continue,
            Err(_) => {}
        }
        std::os::unix::fs::symlink(&target, &link)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn mirror_dir(_from: &Path, _into: &Path, _skip: impl Fn(&str) -> bool) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "accounts per chat need macOS or Linux",
    ))
}

/// The entries of the user's Claude config dir that are an account's own.
fn claude_entry_is_own(name: &str) -> bool {
    name.starts_with(".credentials.json")
        || name.starts_with(".oauth_refresh.lock")
        || name.starts_with(".claude.json")
}

/// The entries of the user's Codex home that are an account's own.
fn codex_entry_is_own(name: &str) -> bool {
    name.starts_with("auth.json") || name.starts_with(".auth.json")
}

fn engine_error(err: impl std::fmt::Display) -> EngineError {
    EngineError::Other(err.to_string())
}

impl AgentAccounts {
    /// The config dir saved account `id` of `harness` runs from when a chat
    /// names it (see module docs). Made only when a chat first runs on it.
    #[doc(hidden)]
    pub fn account_home(&self, harness: HarnessId, id: &str) -> Result<PathBuf, EngineError> {
        // `id` is a raw config string that becomes a path: slot ids only.
        if !is_slot_id(id) || !pinnable(harness) {
            return Err(EngineError::Other("Unknown account.".into()));
        }
        Ok(self
            .inner
            .config
            .data_dir
            .join(HOMES_DIR)
            .join(super::harness_slug(harness))
            .join(id))
    }

    /// The Keychain item Claude Code keeps `home`'s login in (`None`: file
    /// only — tests, Linux, a Cloud device).
    fn home_keychain_service(&self, home: &Path) -> Option<String> {
        self.inner
            .config
            .claude_keychain_service
            .is_some()
            .then(|| super::claude_keychain_service(Some(home)))
    }

    async fn home_login(&self, harness: HarnessId, id: &str) -> HomeLogin {
        let Ok(home) = self.account_home(harness, id) else {
            return HomeLogin::None;
        };
        if !home.is_dir() {
            return HomeLogin::None;
        }
        let found = match harness {
            HarnessId::ClaudeCode => {
                #[cfg(target_os = "macos")]
                if let Some(service) = self.home_keychain_service(&home) {
                    let (creds, warning) = super::keychain::read_credentials(&service).await;
                    match (creds, warning) {
                        (Some(creds), _) => return HomeLogin::Readable(creds),
                        (None, Some(_)) => return HomeLogin::Unreadable,
                        (None, None) => {}
                    }
                }
                read_json(&home.join(".credentials.json"))
                    .filter(|creds| creds.get("claudeAiOauth").is_some())
            }
            HarnessId::Codex => read_json(&home.join("auth.json")),
            _ => None,
        };
        match found {
            Some(creds) => HomeLogin::Readable(creds),
            None => HomeLogin::None,
        }
    }

    /// Whether account `id`'s own dir holds its login now — then the dir is
    /// its only live holder, and the engine never refreshes the saved copy.
    /// Asked at every turn, so it never reads the secret: on macOS the
    /// Keychain item's presence (no prompt), or the dir's file.
    #[doc(hidden)]
    pub async fn holds_live_login(&self, harness: HarnessId, id: &str) -> bool {
        let Ok(home) = self.account_home(harness, id) else {
            return false;
        };
        if !home.is_dir() {
            return false;
        }
        match harness {
            HarnessId::ClaudeCode => {
                #[cfg(target_os = "macos")]
                if let Some(service) = self.home_keychain_service(&home) {
                    match super::keychain::lookup(&service).await {
                        super::keychain::Lookup::Absent => {}
                        // Unanswered: it may well be there; never seed over it.
                        _ => return true,
                    }
                }
                read_json(&home.join(".credentials.json"))
                    .is_some_and(|creds| creds.get("claudeAiOauth").is_some())
            }
            HarnessId::Codex => home.join("auth.json").is_file(),
            _ => false,
        }
    }

    /// The saved accounts of the agents a chat can name, by id: whose is it.
    fn pinned_slot(&self, harness: HarnessId, id: &str) -> Result<Option<Slot>, String> {
        if let Some(slot) = self.read_slot(harness, id) {
            return Ok(Some(slot));
        }
        // An account of another agent (the chat moved to another agent
        // since): it doesn't apply to this run.
        let elsewhere = [HarnessId::ClaudeCode, HarnessId::Codex]
            .into_iter()
            .filter(|other| *other != harness)
            .any(|other| self.read_slot(other, id).is_some());
        if elsewhere {
            Ok(None)
        } else {
            Err(REMOVED.into())
        }
    }

    /// Who holds account `slot`'s login for its next run (module docs).
    async fn holder(&self, harness: HarnessId, slot: &Slot) -> Holder {
        if self.holds_live_login(harness, &slot.id).await {
            return Holder::Home;
        }
        match self.live_account_key(harness, None).await {
            Some(live) if live == slot.account_key => Holder::Machine,
            _ => Holder::Home,
        }
    }

    /// The credential fingerprint a run of a chat naming `id` would use
    /// (`None`: the name doesn't apply to `harness`).
    async fn pinned_fingerprint(&self, harness: HarnessId, id: &str) -> Option<String> {
        match self.pinned_slot(harness, id) {
            Ok(Some(slot)) => Some(self.holder(harness, &slot).await.fingerprint(id)),
            Ok(None) => None,
            // Its next run is refused; any parked session goes first.
            Err(_) => Some(format!("{PINNED_CREDENTIAL}{id}@removed")),
        }
    }

    /// The environment a run of a chat naming account `id` gets: the
    /// account's dir, seeded and its shared config refreshed, or nothing
    /// when the machine's login is that account (module docs).
    async fn pinned_run(
        &self,
        harness: HarnessId,
        id: &str,
    ) -> Result<Option<BTreeMap<String, String>>, String> {
        let Some(slot) = self.pinned_slot(harness, id)? else {
            return Ok(None);
        };
        // One seeding at a time, and never across a switch or removal.
        let _homes = self.inner.homes.lock().await;
        if self.read_slot(harness, id).is_none() {
            return Err(REMOVED.into());
        }
        let home = self.account_home(harness, id).map_err(|e| e.to_string())?;
        match self.holder(harness, &slot).await {
            Holder::Machine => return Ok(None),
            Holder::Home => {}
        }
        let failed = |err: EngineError| {
            tracing::warn!(slot = %id, error = %err, "account dir could not be set up");
            format!("Couldn't set up this chat's account: {err}")
        };
        self.seed_home(harness, &slot, &home)
            .await
            .map_err(failed)?;
        self.share_config(harness, &slot, &home).map_err(failed)?;
        let var = match harness {
            HarnessId::ClaudeCode => "CLAUDE_CONFIG_DIR",
            _ => "CODEX_HOME",
        };
        Ok(Some(BTreeMap::from([(
            var.to_string(),
            home.to_string_lossy().into_owned(),
        )])))
    }

    /// The account's dir, made, with its login written from the slot when
    /// the dir holds none (the caller holds `homes`, and checked that the
    /// machine's login isn't this account).
    async fn seed_home(
        &self,
        harness: HarnessId,
        slot: &Slot,
        home: &Path,
    ) -> Result<(), EngineError> {
        if let Some(parent) = home.parent() {
            private_dir(&self.inner.config.data_dir.join(HOMES_DIR))?;
            private_dir(parent)?;
        }
        private_dir(home)?;
        if self.holds_live_login(harness, &slot.id).await {
            return Ok(());
        }
        tracing::info!(slot = %slot.id, harness = ?harness, "account dir takes its login");
        match harness {
            HarnessId::ClaudeCode => {
                // The account's own login only: the machine's MCP sign-ins
                // rotate their tokens too, and must not be copied.
                let mut creds = slot.credentials.clone();
                if let Some(map) = creds.as_object_mut() {
                    map.retain(|key, _| !CLAUDE_SHARED_CREDENTIAL_KEYS.contains(&key.as_str()));
                }
                write_file_atomic(
                    &home.join(".credentials.json"),
                    creds.to_string().as_bytes(),
                    true,
                )
            }
            _ => {
                let json = serde_json::to_string_pretty(&slot.credentials).map_err(engine_error)?;
                write_file_atomic(&home.join("auth.json"), json.as_bytes(), true)
            }
        }
    }

    /// Link the user's own config into the account's dir (module docs).
    fn share_config(
        &self,
        harness: HarnessId,
        slot: &Slot,
        home: &Path,
    ) -> Result<(), EngineError> {
        let config = &self.inner.config;
        match harness {
            HarnessId::ClaudeCode => {
                // History lives in the user's dir, whichever account wrote it.
                std::fs::create_dir_all(config.claude_config_dir.join("projects"))?;
                mirror_dir(&config.claude_config_dir, home, claude_entry_is_own)?;
                let file = home.join(".claude.json");
                let real = read_json(&config.claude_config_file);
                let merged = claude_home_config(real, read_json(&file), slot);
                if read_json(&file).as_ref() != Some(&merged) {
                    write_file_atomic(&file, merged.to_string().as_bytes(), true)?;
                }
                Ok(())
            }
            _ => {
                std::fs::create_dir_all(config.codex_home.join("sessions"))?;
                Ok(mirror_dir(&config.codex_home, home, codex_entry_is_own)?)
            }
        }
    }

    /// Copy each account dir's newest login into its saved slot, so the
    /// slot stays a current backup (and a switch to it installs the newest
    /// pair). A dir whose login names someone else is left alone. Caller
    /// holds `ops`.
    pub(super) async fn sync_homes(&self) {
        for harness in [HarnessId::ClaudeCode, HarnessId::Codex] {
            let root = self
                .inner
                .config
                .data_dir
                .join(HOMES_DIR)
                .join(super::harness_slug(harness));
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let id = entry.file_name().to_string_lossy().into_owned();
                if is_slot_id(&id) {
                    self.sync_home(harness, &id).await;
                }
            }
        }
    }

    async fn sync_home(&self, harness: HarnessId, id: &str) {
        let Some(slot) = self.read_slot(harness, id) else {
            return;
        };
        let HomeLogin::Readable(creds) = self.home_login(harness, id).await else {
            return;
        };
        let credentials = match harness {
            HarnessId::ClaudeCode => {
                let Some(oauth) = creds.get("claudeAiOauth").cloned() else {
                    return;
                };
                // Signed in as someone else in that dir (`/login`): not this
                // account's login.
                let home = self.account_home(harness, id).ok();
                let named = home
                    .and_then(|home| read_json(&home.join(".claude.json")))
                    .and_then(|cfg| {
                        let oauth = cfg.get("oauthAccount")?.clone();
                        str_field(&oauth, "accountUuid")
                            .or_else(|| str_field(&oauth, "emailAddress"))
                    });
                if named.is_some_and(|key| key != slot.account_key) {
                    return;
                }
                let updated = with_claude_ai_oauth(&slot.credentials, oauth);
                if claude_login_is_older(&updated, &slot.credentials) {
                    return;
                }
                updated
            }
            _ => {
                if parse_codex_auth(creds.clone()).map(|d| d.account_key)
                    != Some(slot.account_key.clone())
                {
                    return;
                }
                creds
            }
        };
        if credentials == slot.credentials {
            return;
        }
        let mut synced = slot;
        synced.credentials = credentials;
        synced.saved_at = now_ms();
        if let Err(err) = self.write_slot(&synced) {
            tracing::warn!(slot = %id, error = %err, "account dir's login not saved");
        }
    }

    /// Account `slot` is about to become the machine's login: when its dir
    /// holds it, the dir's newest pair is what moves (returned as the slot to
    /// install), and the dir's copy is deleted by [`Self::finish_handover`]
    /// once installed. Caller holds `ops` and `homes`.
    pub(super) async fn begin_handover(
        &self,
        harness: HarnessId,
        slot: Slot,
    ) -> Result<(Slot, Option<HandoverLock>), EngineError> {
        if !pinnable(harness) || !self.holds_live_login(harness, &slot.id).await {
            return Ok((slot, None));
        }
        let home = self.account_home(harness, &slot.id)?;
        // No refresh in that dir until its copy is gone.
        let refresh_lock = match harness {
            HarnessId::ClaudeCode => Some(
                zeron_harness::claude::refresh_lock::RefreshLock::acquire(
                    &home,
                    CLAUDE_REFRESH_LOCK_WAIT,
                )
                .await
                .map_err(|err| {
                    EngineError::Other(format!(
                        "Claude Code is still saving this account's refreshed login — try again \
                         in a moment. ({err})"
                    ))
                })?,
            ),
            _ => None,
        };
        if let HomeLogin::Unreadable = self.home_login(harness, &slot.id).await {
            return Err(EngineError::Other(
                "macOS Keychain wouldn't give up this account's login from its chats' dir — \
                 approve the prompt and try again."
                    .into(),
            ));
        }
        self.sync_home(harness, &slot.id).await;
        let slot = self.read_slot(harness, &slot.id).unwrap_or(slot);
        Ok((
            slot,
            Some(HandoverLock {
                _refresh: refresh_lock,
            }),
        ))
    }

    /// Delete account `id`'s login from its dir (it moved to the machine, or
    /// the account is being removed).
    pub(super) async fn retire_home_login(
        &self,
        harness: HarnessId,
        id: &str,
    ) -> Result<(), EngineError> {
        let home = self.account_home(harness, id)?;
        match harness {
            HarnessId::ClaudeCode => {
                #[cfg(target_os = "macos")]
                if let Some(service) = self.home_keychain_service(&home) {
                    super::keychain::delete_credentials(&service).await?;
                }
                remove_if_exists(&home.join(".credentials.json"))
            }
            _ => remove_if_exists(&home.join("auth.json")),
        }
    }

    /// Account `id` removed: its dir goes too. Caller holds `ops`.
    pub(super) async fn remove_home(
        &self,
        harness: HarnessId,
        id: &str,
    ) -> Result<(), EngineError> {
        if !pinnable(harness) {
            return Ok(());
        }
        let _homes = self.inner.homes.lock().await;
        let home = self.account_home(harness, id)?;
        if !home.exists() {
            return Ok(());
        }
        self.retire_home_login(harness, id).await?;
        std::fs::remove_dir_all(&home)?;
        Ok(())
    }

    /// A pinned run reported its subscription's usage: it belongs to the
    /// account the chat named.
    fn record_pinned_usage(
        &self,
        harness: HarnessId,
        credential: &str,
        windows: Vec<AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        let Some(id) = credential
            .strip_prefix(PINNED_CREDENTIAL)
            .and_then(|rest| rest.split('@').next())
        else {
            return;
        };
        let Some(slot) = self.read_slot(harness, id) else {
            return;
        };
        let mut usage = lock(&self.inner.usage);
        usage
            .entry(super::usage_key(harness, &slot.account_key))
            .or_default()
            .record(
                Ok(UsageSnapshot {
                    windows,
                    plan_label,
                    codex_reset_credits: None,
                }),
                String::new(),
                now_ms(),
            );
        self.persist_usage(&usage);
    }
}

/// Held while an account's login moves from its dir to the machine.
pub(super) struct HandoverLock {
    _refresh: Option<zeron_harness::claude::refresh_lock::RefreshLock>,
}

/// The spawn-time environment of the accounts zeron keeps: Muse's selected
/// account, and the Claude Code / Codex account a chat names (module docs).
pub struct AccountsRunEnvironment(pub AgentAccounts);

#[async_trait::async_trait]
impl crate::registry::RunEnvironment for AccountsRunEnvironment {
    async fn prepare(
        &self,
        harness: HarnessId,
        request: &mut zeron_proto::RunRequest,
    ) -> Result<(), String> {
        self.prepare_for(harness, None, request).await
    }

    async fn credential(&self, harness: HarnessId) -> Option<String> {
        self.credential_for(harness, None).await
    }

    async fn prepare_for(
        &self,
        harness: HarnessId,
        account: Option<&str>,
        request: &mut zeron_proto::RunRequest,
    ) -> Result<(), String> {
        if harness == HarnessId::Muse {
            return super::MuseRunEnvironment(self.0.clone())
                .prepare(harness, request)
                .await;
        }
        let Some(id) = account.filter(|_| pinnable(harness)) else {
            return Ok(());
        };
        if let Some(env) = self.0.pinned_run(harness, id).await? {
            for (key, value) in env {
                request.env.entry(key).or_insert(value);
            }
        }
        Ok(())
    }

    async fn credential_for(&self, harness: HarnessId, account: Option<&str>) -> Option<String> {
        if harness == HarnessId::Muse {
            return super::MuseRunEnvironment(self.0.clone())
                .credential(harness)
                .await;
        }
        let id = account.filter(|_| pinnable(harness))?;
        self.0.pinned_fingerprint(harness, id).await
    }

    async fn record_usage(
        &self,
        harness: HarnessId,
        credential: &str,
        windows: Vec<AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        if credential.starts_with(PINNED_CREDENTIAL) {
            self.0
                .record_pinned_usage(harness, credential, windows, plan_label);
        } else {
            super::MuseRunEnvironment(self.0.clone())
                .record_usage(harness, credential, windows, plan_label)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_home_config_takes_the_users_settings_and_keeps_the_account() {
        let slot: Slot = serde_json::from_value(serde_json::json!({
            "id": "0123456789abcdef",
            "harness": "claude-code",
            "accountKey": "uuid-a",
            "profile": { "email": "a@x.test", "authKind": "oauth" },
            "credentials": {},
            "claudeConfig": { "oauthAccount": { "accountUuid": "uuid-a" }, "userID": "u-a" },
            "savedAt": 0,
        }))
        .unwrap();
        let real = serde_json::json!({
            "oauthAccount": { "accountUuid": "uuid-machine" },
            "userID": "u-machine",
            "mcpServers": { "docs": {} },
            "cachedGrowthBookFeatures": { "machine": true },
        });
        // First spawn: identity from the slot, settings from the user.
        let first = claude_home_config(Some(real.clone()), None, &slot);
        assert_eq!(first["oauthAccount"]["accountUuid"], "uuid-a");
        assert_eq!(first["userID"], "u-a");
        assert!(first["mcpServers"]["docs"].is_object());
        assert!(first.get("cachedGrowthBookFeatures").is_none());
        // Later: what the account wrote of its own stays.
        let mut own = first.clone();
        own["cachedGrowthBookFeatures"] = serde_json::json!({ "account": true });
        own["numStartups"] = serde_json::json!(9);
        let next = claude_home_config(Some(real), Some(own), &slot);
        assert_eq!(next["cachedGrowthBookFeatures"]["account"], true);
        assert!(next.get("numStartups").is_none());
        assert_eq!(next["oauthAccount"]["accountUuid"], "uuid-a");
    }
}
