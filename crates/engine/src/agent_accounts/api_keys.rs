//! API keys for agents that keep a login per model provider — added in the
//! app instead of a CLI, stored exactly where and how each agent's own CLI
//! stores them (`zeron_proto::providers::api_key_options` says which):
//!
//! - **Pi** — `auth.json`'s provider entry `{"type": "api_key", "key"}`
//!   (pi-ai `envApiKeyAuth`'s login result; pi's `FileAuthStorageBackend`
//!   writes it 0600 under its proper-lockfile lock, as zeron does). Pi reads
//!   a key starting `!` as a shell command and expands `$VAR`s, so a key is
//!   only ever a plain token (see [`valid_key`]).
//! - **OpenCode 1.x** — `auth.json`'s entry `{"type": "api", "key"}` (its
//!   credential schema; 0600). **OpenCode 2.x** keeps every credential in
//!   its own database (`opencode.db`'s `credential` table; `auth.json` is
//!   imported once and never read again), so a key written to `auth.json`
//!   would do nothing: refused, with what to do instead.
//! - **Hermes** — its own credential pool, through its own CLI: `hermes
//!   auth add <provider> --type api-key`, the key on its STDIN (its
//!   `masked_secret_prompt` falls back to `getpass`, which reads stdin when
//!   the child has no terminal — it runs in a session of its own, so it
//!   never has one). Hermes writes and locks its pool itself.
//!
//! Each key is checked with its provider first (a models / key-info call
//! that costs nothing) and only saved once accepted. Like every login on
//! this device, a key added to Pi or OpenCode is also kept in its account
//! slot (owner-only, 0600) so switching away from it and back works.
//!
//! The key is a secret end to end: never logged, never in an error, never
//! in a list (a key row says `API key ····abcd`), never on a command line,
//! and never sent anywhere but its provider — `AddAgentApiKey` is answered
//! only over this device's own IPC (`zeron_rpc::local_only`), and a request
//! naming another device is refused rather than forwarded.

use zeron_proto::providers::api_key_option;

use super::*;

/// How a provider's key is checked: the cheapest authenticated call it has.
struct KeyCheck {
    vendor: &'static str,
    base: &'static str,
    path: &'static str,
    auth: KeyAuth,
}

enum KeyAuth {
    Bearer,
    /// Anthropic's `x-api-key` (+ `anthropic-version`).
    Anthropic,
    /// Google's `x-goog-api-key` (never the `?key=` query: urls get logged).
    Google,
}

fn key_check(provider: &str) -> Option<KeyCheck> {
    let check = |vendor, base, path, auth| KeyCheck {
        vendor,
        base,
        path,
        auth,
    };
    Some(match provider {
        "openai" | "openai-api" => check(
            "OpenAI",
            "https://api.openai.com",
            "/v1/models",
            KeyAuth::Bearer,
        ),
        "anthropic" => check(
            "Anthropic",
            "https://api.anthropic.com",
            "/v1/models",
            KeyAuth::Anthropic,
        ),
        "openrouter" => check(
            "OpenRouter",
            "https://openrouter.ai",
            "/api/v1/key",
            KeyAuth::Bearer,
        ),
        "google" | "gemini" => check(
            "Google",
            "https://generativelanguage.googleapis.com",
            "/v1beta/models",
            KeyAuth::Google,
        ),
        "xai" => check("xAI", "https://api.x.ai", "/v1/api-key", KeyAuth::Bearer),
        "groq" => check(
            "Groq",
            "https://api.groq.com",
            "/openai/v1/models",
            KeyAuth::Bearer,
        ),
        "deepseek" => check(
            "DeepSeek",
            "https://api.deepseek.com",
            "/models",
            KeyAuth::Bearer,
        ),
        "mistral" => check(
            "Mistral",
            "https://api.mistral.ai",
            "/v1/models",
            KeyAuth::Bearer,
        ),
        _ => return None,
    })
}

/// A key as zeron accepts it: one plain token of 8 to 512 characters from
/// the alphabet API keys use. Nothing a shell, Pi's `!command` / `$VAR`
/// expansion or a JSON / TOML store could read as anything but a literal.
pub(super) fn valid_key(key: &str) -> bool {
    (8..=512).contains(&key.len())
        && !key.starts_with(['!', '-'])
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:+/=~".contains(c))
}

/// A key as a list may show it: its last four characters.
pub(super) fn masked(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("···{tail}")
}

/// How a key's row reads — its last four characters said exactly once:
/// named, "Work" over "OpenRouter · API key"; unnamed, "API key ···a1b2"
/// over "OpenRouter"; seeded from an environment variable (Hermes), never
/// that variable's name: "API key ···a1b2" over "OpenRouter · from
/// environment". `key`: the key, when it's a plain one (none: a command or
/// variable Pi resolves, of which nothing is shown).
pub(super) fn key_rows(
    provider: &str,
    key: Option<&str>,
    name: Option<&str>,
    from_env: bool,
) -> (String, String) {
    let unnamed = match key {
        Some(key) => format!("API key {}", masked(key)),
        None => "API key".to_string(),
    };
    match (name, from_env) {
        (_, true) => (unnamed, format!("{provider} · from environment")),
        (Some(name), false) => (name.to_string(), format!("{provider} · API key")),
        (None, false) => (unnamed, provider.to_string()),
    }
}

/// `profile` as a key's, named `name` (none: unnamed). The name, when set,
/// is also kept apart (`display_name`), so it's never mistaken for the
/// derived title.
pub(super) fn name_key_profile(
    profile: &mut SlotProfile,
    provider: &str,
    key: Option<&str>,
    name: Option<&str>,
) {
    let (title, line) = key_rows(provider, key, name, false);
    profile.email = title;
    profile.plan = Some(line);
    profile.display_name = name.map(str::to_string);
}

/// A name for a key as given, or why not: one line of at most 48
/// characters that doesn't look like a key itself. The check never looks at
/// the key: a name is renamed from other devices too, and a refusal that
/// depended on the key's contents would let a peer probe it.
pub(super) fn key_name(name: &str) -> Result<Option<String>, EngineError> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > 48 || name.chars().any(char::is_control) {
        return Err(EngineError::Other(
            "A name is one short line — 48 characters at most.".into(),
        ));
    }
    if looks_like_a_key(name) {
        return Err(EngineError::Other(
            "That looks like a key — give it a name instead.".into(),
        ));
    }
    Ok(Some(name.to_string()))
}

/// Whether any word of `text` reads as a secret: a key's own prefix
/// (`sk-`, `sk-ant-`, `sk-or-`, `xai-`, `AIza`, GitHub's `gh?_`), or a run of
/// 16 or more key characters with no space in it.
fn looks_like_a_key(text: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-",
        "sk_",
        "xai-",
        "AIza",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
    ];
    text.split_whitespace().any(|word| {
        let lower = word.to_ascii_lowercase();
        PREFIXES
            .iter()
            .any(|p| word.starts_with(p) || lower.starts_with(&p.to_ascii_lowercase()))
            || word
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .any(|run| run.chars().count() >= 16)
    })
}

/// The entry's key, if it's an API-key entry in `harness`'s own shape (Pi
/// `api_key`, OpenCode `api`).
pub(super) fn key_entry_secret(harness: HarnessId, entry: &serde_json::Value) -> Option<String> {
    let kind = match harness {
        HarnessId::Pi => "api_key",
        HarnessId::Opencode => "api",
        _ => return None,
    };
    (entry.get("type").and_then(|t| t.as_str()) == Some(kind))
        .then(|| str_field(entry, "key"))
        .flatten()
}

/// An API-key entry of Pi's / OpenCode's store as a login: keyed by a
/// digest of the key (never the key), shown by its last four characters.
/// A Pi key that is really a command or variable (`!…`, `$…`) shows none.
pub(super) fn key_detected(
    harness: HarnessId,
    store_key: &str,
    entry: &serde_json::Value,
) -> Option<Detected> {
    let key = key_entry_secret(harness, entry)?;
    let provider = zeron_proto::providers::account_group_label(harness, store_key);
    // (A Pi key that's really a command or a variable shows none of it.)
    let plain = valid_key(&key).then_some(key.as_str());
    let digest = crate::repos::hex(&Sha256::digest(key.as_bytes()))[..16].to_string();
    // Unnamed until it's given a name ([`AgentAccounts::rename_account`]).
    let mut profile = SlotProfile {
        email: String::new(),
        display_name: None,
        organization: None,
        plan: None,
        auth_kind: AgentAuthKind::ApiKey,
    };
    name_key_profile(&mut profile, &provider, plain, None);
    let mut detected = Detected::known(format!("{store_key}:key:{digest}"), profile, entry.clone())
        .keyed(store_key);
    // A key carries no name of its own: the one its slot keeps (given when
    // it was added, or since) wins over the default.
    detected.identity_known = false;
    Some(detected)
}

/// The entry `harness`'s own CLI writes for a key.
fn key_entry(harness: HarnessId, key: &str) -> serde_json::Value {
    match harness {
        HarnessId::Opencode => serde_json::json!({ "type": "api", "key": key }),
        _ => serde_json::json!({ "type": "api_key", "key": key }),
    }
}

/// A string overwritten when dropped (the key's copy on its way to stdin).
struct Zeroized(String);

impl Drop for Zeroized {
    fn drop(&mut self) {
        // SAFETY: zeros are valid UTF-8; the bytes are dropped right after.
        unsafe { self.0.as_bytes_mut().fill(0) };
    }
}

fn zeroize_on_drop(text: String) -> Zeroized {
    Zeroized(text)
}

/// Every occurrence of `key` in `text`, blanked — for the agent's own
/// words, which never name a key, but are never trusted to.
fn without(text: &str, key: &str) -> String {
    if key.is_empty() {
        return text.to_string();
    }
    text.replace(key, "[key]")
}

impl AgentAccounts {
    /// Add `key` for `provider` of `harness` (an `api_key_options` pair),
    /// checked with the provider first. Pi / OpenCode: written to the
    /// agent's store when that provider has no live login there (else kept
    /// beside it, to switch to), Hermes: appended to its own pool.
    pub async fn add_api_key(
        &self,
        harness: HarnessId,
        provider: &str,
        key: &str,
        name: Option<&str>,
    ) -> Result<AgentAccountsSnapshot, EngineError> {
        let added = self
            .add_api_key_unpublished(harness, provider, key, name)
            .await;
        self.published(added)
    }

    async fn add_api_key_unpublished(
        &self,
        harness: HarnessId,
        provider: &str,
        key: &str,
        name: Option<&str>,
    ) -> Result<AgentAccountsSnapshot, EngineError> {
        let key = key.trim();
        let Some(option) = api_key_option(harness, provider) else {
            return Err(EngineError::Other(format!(
                "Zeron doesn't add {provider} keys for {}.",
                stores::cli_name(harness)
            )));
        };
        if !valid_key(key) {
            return Err(EngineError::Other(format!(
                "That doesn't look like a {} API key — paste the whole key, nothing else.",
                option.label
            )));
        }
        let name = key_name(name.unwrap_or_default())?;
        if harness == HarnessId::Opencode && self.opencode_keeps_its_own_store() {
            return Err(EngineError::Other(
                "This OpenCode keeps its keys in its own database — add it with \
                 `opencode auth login` there."
                    .into(),
            ));
        }
        self.check_api_key(provider, key).await?;
        match harness {
            HarnessId::Hermes => {
                self.add_hermes_key(provider, key, name.as_deref()).await?;
                let _ops = self.inner.ops.lock().await;
                self.list_locked(false).await
            }
            HarnessId::Pi | HarnessId::Opencode => {
                let entry = key_entry(harness, key);
                let mut detected = key_detected(harness, provider, &entry)
                    .ok_or_else(|| EngineError::Other("That key couldn't be stored.".into()))?;
                let group = zeron_proto::providers::account_group_label(harness, provider);
                name_key_profile(&mut detected.profile, &group, Some(key), name.as_deref());
                self.save_new_login(harness, &detected).await?;
                let _ops = self.inner.ops.lock().await;
                // (Added again under a new name: the new name.)
                if let Some(name) = name
                    && let Some(mut slot) =
                        self.read_slot(harness, &slot_id_for(harness, &detected.account_key))
                    && slot.profile.display_name.as_deref() != Some(name.as_str())
                {
                    name_key_profile(&mut slot.profile, &group, Some(key), Some(&name));
                    self.write_slot(&slot)?;
                }
                self.list_locked(false).await
            }
            _ => Err(EngineError::Other("No API keys for this agent.".into())),
        }
    }

    /// Ask the provider whether it takes `key` — the one request the key is
    /// ever sent in by zeron, over https to the provider's own host.
    async fn check_api_key(&self, provider: &str, key: &str) -> Result<(), EngineError> {
        let Some(check) = key_check(provider) else {
            return Err(EngineError::Other(
                "Zeron can't check that provider's keys yet.".into(),
            ));
        };
        let base = self
            .inner
            .endpoints
            .api_key_check
            .as_deref()
            .unwrap_or(check.base)
            .trim_end_matches('/');
        let request = self
            .inner
            .http
            .get(format!("{base}{}", check.path))
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(10));
        let request = match check.auth {
            KeyAuth::Bearer => request.bearer_auth(key),
            KeyAuth::Anthropic => request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
            KeyAuth::Google => request.header("x-goog-api-key", key),
        };
        let vendor = check.vendor;
        // (The error is reqwest's, which names the url, never a header.)
        let response = request.send().await.map_err(|_| {
            EngineError::Other(format!(
                "Couldn't reach {vendor} to check the key — try again."
            ))
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        // Never the body: some providers echo what they were sent.
        Err(EngineError::Other(match status.as_u16() {
            400 | 401 | 403 => format!("{vendor} didn't accept that key — check it and try again."),
            402 => format!("{vendor} accepted the key, but its account has no credit."),
            429 => format!("{vendor} is rate limiting right now — try the key again in a minute."),
            _ => format!("{vendor} couldn't check the key ({status}) — try again."),
        }))
    }

    /// `hermes auth add <provider> --type api-key`, the key written to its
    /// stdin (never argv, never its environment), in a session of its own
    /// so `getpass` has no terminal to prompt on and reads stdin instead.
    async fn add_hermes_key(
        &self,
        provider: &str,
        key: &str,
        name: Option<&str>,
    ) -> Result<(), EngineError> {
        // (Its name is no secret — checked to hold none of the key — so it
        // may ride on the command line, as Hermes takes it.)
        // (`--label=…` in one argument: a name starting with `-` is never
        // read as a flag.)
        let label = name.map(|name| format!("--label={name}"));
        let mut args = vec!["auth", "add", provider, "--type", "api-key"];
        if let Some(label) = &label {
            args.push(label);
        }
        let mut command = self.cli_command(HarnessId::Hermes, &args).await?;
        // A sign-in abandoned (the engine stopping, the wait timing out)
        // takes Hermes with it.
        command.kill_on_drop(true);
        command
            .env("HERMES_HOME", &self.inner.config.hermes_home)
            .stdin(zeron_harness::process::Stdio::piped())
            .stdout(zeron_harness::process::Stdio::piped())
            .stderr(zeron_harness::process::Stdio::piped());
        #[cfg(unix)]
        // SAFETY: `setsid` is async-signal-safe; nothing else runs between
        // fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|e| EngineError::Other(format!("Couldn't run hermes: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt as _;
            let line = zeroize_on_drop(format!("{key}\n"));
            let _ = stdin.write_all(line.0.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .map_err(|_| EngineError::Other("hermes didn't finish adding the key.".into()))?
            .map_err(|e| EngineError::Other(format!("hermes failed: {e}")))?;
        if output.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&output.stderr);
        let said = without(said.trim(), key);
        let last = said
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty() && !l.contains("GetPassWarning"))
            .unwrap_or("hermes refused the key");
        Err(EngineError::Other(zeron_harness::redact::redact_output(
            &without(last.trim(), key),
        )))
    }

    /// OpenCode 2.x: its credentials live in `opencode.db`'s `credential`
    /// table, beside the `auth.json` 1.x read (imported once, then unread).
    pub(super) fn opencode_keeps_its_own_store(&self) -> bool {
        let Some(dir) = self.inner.config.opencode_auth_file.parent() else {
            return false;
        };
        let db = dir.join("opencode.db");
        if !db.exists() {
            return false;
        }
        let Ok(conn) = rusqlite::Connection::open_with_flags(
            &db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return false;
        };
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'credential'",
            [],
            |_| Ok(()),
        )
        .is_ok()
    }
}

impl AgentAccounts {
    /// A key's name, changed (no key asked again): kept with its slot's
    /// other details, so it shows wherever the row does. Keys alone are
    /// named (a login goes by its own address); Hermes' pool is Hermes' to
    /// label.
    pub async fn rename_account(
        &self,
        harness: HarnessId,
        account_id: &str,
        name: &str,
    ) -> Result<AgentAccountsSnapshot, EngineError> {
        let renamed = self
            .rename_account_unpublished(harness, account_id, name)
            .await;
        self.published(renamed)
    }

    async fn rename_account_unpublished(
        &self,
        harness: HarnessId,
        account_id: &str,
        name: &str,
    ) -> Result<AgentAccountsSnapshot, EngineError> {
        if account_id.len() != 16
            || !account_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(EngineError::Other("Unknown account.".into()));
        }
        if harness == HarnessId::Hermes {
            return Err(EngineError::Other(
                "Hermes names its own keys — rename one with `hermes auth`.".into(),
            ));
        }
        let _ops = self.inner.ops.lock().await;
        let mut slot = self
            .read_slot(harness, account_id)
            .filter(|slot| slot.profile.auth_kind == AgentAuthKind::ApiKey)
            .ok_or_else(|| EngineError::Other("Only an API key takes a name here.".into()))?;
        let key = key_entry_secret(harness, &slot.credentials);
        let provider = zeron_proto::providers::account_group_label(
            harness,
            slot.store_key.as_deref().unwrap_or_default(),
        );
        let name = key_name(name)?;
        let plain = key.as_deref().filter(|k| valid_key(k));
        name_key_profile(&mut slot.profile, &provider, plain, name.as_deref());
        self.write_slot(&slot)?;
        self.list_locked(false).await
    }
}
