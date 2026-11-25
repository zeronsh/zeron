//! Codex on a saved account, for one chat: the chat's `codex app-server`
//! gets `CODEX_HOME` = that account's own dir, while the machine's login
//! (`~/.codex/auth.json`) stays as it is for every other chat and the
//! user's own terminals.
//!
//! One dir per saved account, never per chat:
//! `{data_dir}/agent-accounts/homes/codex/<slot id>/`, private like the rest
//! of the accounts root. It mirrors the user's own Codex home (a link to
//! every entry there: `config.toml`, `AGENTS.md`, skills, `sessions/` and
//! the rest) except the login, so a chat resumes its thread whichever
//! account runs it.
//!
//! **One live copy of a login.** Codex rotates its refresh token at each
//! refresh, so two stores refreshing copies of one login log each other
//! out. At any time one store holds an account's login live:
//!
//! - the account's dir, once it holds a login: seeded once from the saved
//!   slot, then refreshed there by the CLI. Each list copies the dir's newer
//!   login back into the slot (the engine never refreshes a Codex slot);
//! - else, when the account is the machine's login, the machine: the chat
//!   runs exactly as an unpinned one does;
//! - else the dir, seeded now.
//!
//! Making the account the machine's login moves its login out of the dir
//! ([`AgentAccounts::retire_codex_home_login`] after the switch installed the
//! dir's newest copy); removing the account deletes its dir.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zeron_proto::HarnessId;

use super::pins::PinRun;
use super::{AgentAccounts, EngineError, now_ms, parse_codex_auth, private_dir, read_json, write_file_atomic};

/// The variable the chat's Codex process reads its home from.
pub(super) const CODEX_HOME_VAR: &str = "CODEX_HOME";
/// Prefix of a run credential on an account's dir (`codex-home:<slot id>`).
const CREDENTIAL: &str = "codex-home:";

fn is_slot_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The entries of the user's Codex home that are an account's own.
fn entry_is_own(name: &str) -> bool {
    name.starts_with("auth.json") || name.starts_with(".auth.json")
}

/// A Codex `auth.json`'s `last_refresh`, as epoch millis.
fn refreshed_at(auth: &serde_json::Value) -> Option<i64> {
    let at = auth.get("last_refresh")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|at| at.timestamp_millis())
}

/// Whether login `a` is a later state of the login than `b`.
fn is_newer(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    if a == b {
        return false;
    }
    match (refreshed_at(a), refreshed_at(b)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        // Neither says: the dir's copy is the one the CLI keeps current.
        (None, None) => true,
    }
}

/// Make `into` mirror `from`: a link to every entry of `from` but the
/// login. Stale links are pruned; real entries in `into` (the account's own
/// login, anything the CLI wrote in place of a link) are left alone.
#[cfg(unix)]
fn mirror_dir(from: &Path, into: &Path) -> std::io::Result<()> {
    let wanted: std::collections::HashSet<std::ffi::OsString> = match std::fs::read_dir(from) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| !entry_is_own(&name.to_string_lossy()))
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
fn mirror_dir(_from: &Path, _into: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "a chat on its own Codex account needs macOS or Linux",
    ))
}

impl AgentAccounts {
    fn codex_homes_root(&self) -> PathBuf {
        self.inner.config.root_dir().join("homes").join(super::harness_slug(HarnessId::Codex))
    }

    /// The dir saved Codex account `id` runs from when a chat is pinned to
    /// it (made at that chat's first run). `None` for anything but a slot id:
    /// the id becomes a path.
    #[doc(hidden)]
    pub fn codex_home(&self, id: &str) -> Option<PathBuf> {
        is_slot_id(id).then(|| self.codex_homes_root().join(id))
    }

    fn codex_home_holds_login(&self, id: &str) -> bool {
        self.codex_home(id).is_some_and(|home| home.join("auth.json").is_file())
    }

    /// What a run pinned to saved Codex account `id` gets (module docs):
    /// `None` when the machine's login is that account, or the slot is gone.
    pub(super) async fn codex_pin_run(&self, id: &str) -> Result<Option<PinRun>, String> {
        let Some(slot) = self.read_slot(HarnessId::Codex, id) else { return Ok(None) };
        let home = self.codex_home(id).ok_or("Unknown account.")?;
        // One seeding at a time, and never across a switch or a removal.
        let _homes = self.inner.homes.lock().await;
        if !self.codex_home_holds_login(id) {
            if self.live_account_key(HarnessId::Codex, None).await.as_deref() == Some(slot.account_key.as_str()) {
                return Ok(None);
            }
            if slot.credentials.get("tokens").is_none() {
                return Err(format!(
                    "{} has no ChatGPT login saved: sign in to it again (Settings → Accounts)",
                    slot.profile.email
                ));
            }
            let failed = |err: EngineError| {
                tracing::warn!(slot = %id, error = %err, "Codex account dir could not be set up");
                format!("Couldn't set up this chat's Codex account: {err}")
            };
            private_dir(&home).map_err(|e| failed(e.into()))?;
            let json = serde_json::to_string_pretty(&slot.credentials).map_err(|e| failed(EngineError::Other(e.to_string())))?;
            write_file_atomic(&home.join("auth.json"), json.as_bytes(), true).map_err(failed)?;
            tracing::info!(slot = %id, "Codex account dir takes its login");
        }
        let user_home = &self.inner.config.codex_home;
        // History (and the images the engine collects) live in the user's
        // home, whichever account wrote them.
        std::fs::create_dir_all(user_home.join("sessions"))
            .and_then(|()| std::fs::create_dir_all(user_home.join("generated_images")))
            .and_then(|()| mirror_dir(user_home, &home))
            .map_err(|err| format!("Couldn't set up this chat's Codex account: {err}"))?;
        Ok(Some(PinRun {
            env: BTreeMap::from([(CODEX_HOME_VAR.to_owned(), home.to_string_lossy().into_owned())]),
            credential: format!("{CREDENTIAL}{id}"),
        }))
    }

    /// Copy each account dir's newer login back into its slot, so the slot
    /// stays a current backup (usage probes read it; a switch installs it).
    /// A dir signed in as someone else is left alone. Caller holds `ops`.
    pub(super) fn sync_codex_homes(&self) {
        let Ok(entries) = std::fs::read_dir(self.codex_homes_root()) else { return };
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if is_slot_id(&id) {
                self.sync_codex_home(&id);
            }
        }
    }

    fn sync_codex_home(&self, id: &str) {
        let Some(slot) = self.read_slot(HarnessId::Codex, id) else { return };
        let Some(home) = self.codex_home(id) else { return };
        let Some(auth) = read_json(&home.join("auth.json")) else { return };
        if parse_codex_auth(auth.clone()).map(|d| d.account_key) != Some(slot.account_key.clone()) {
            return;
        }
        if !is_newer(&auth, &slot.credentials) {
            return;
        }
        let mut synced = slot;
        synced.credentials = auth;
        synced.saved_at = now_ms();
        if let Err(err) = self.write_slot(&synced) {
            tracing::warn!(slot = %id, error = %err, "Codex account dir's login not saved");
        }
    }

    /// Account `id` just became the machine's login (installed from its
    /// slot, which [`Self::sync_codex_homes`] brought up to date): its dir
    /// gives its copy up, so only the machine refreshes it from now on.
    /// Its chats' credential changes with it, so a parked session is
    /// replaced before its next turn.
    pub(super) async fn retire_codex_home_login(&self, id: &str) {
        let _homes = self.inner.homes.lock().await;
        let Some(home) = self.codex_home(id) else { return };
        if let Err(err) = super::remove_if_exists(&home.join("auth.json")) {
            tracing::warn!(slot = %id, error = %err, "Codex account dir kept its login");
        }
    }

    /// Account `id` was removed: its dir goes too.
    pub(super) async fn remove_codex_home(&self, id: &str) {
        let _homes = self.inner.homes.lock().await;
        let Some(home) = self.codex_home(id).filter(|home| home.exists()) else { return };
        // Unlink the mirrored entries first: shredding must never follow a
        // link into the user's own home.
        if let Ok(entries) = std::fs::read_dir(&home) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        super::shred_dir(&home);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_refresh_wins_and_an_equal_login_is_not_newer() {
        let at = |t: &str| serde_json::json!({ "tokens": { "refresh_token": t }, "last_refresh": t });
        assert!(is_newer(&at("2026-11-17T12:00:00Z"), &at("2026-11-17T11:00:00Z")));
        assert!(!is_newer(&at("2026-11-17T11:00:00Z"), &at("2026-11-17T12:00:00Z")));
        assert!(!is_newer(&at("2026-11-17T11:00:00Z"), &at("2026-11-17T11:00:00Z")));
    }

    #[test]
    fn only_slot_ids_name_a_dir() {
        assert!(is_slot_id("0123456789abcdef"));
        assert!(!is_slot_id("../../etc/passwd"));
        assert!(!is_slot_id("0123456789ABCDEF"));
    }
}
