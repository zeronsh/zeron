//! Whether this device's agents delegate in the background (Settings →
//! General): a chat hands work to child chats, ends its turn into Waiting,
//! and Zeron wakes it with the results ([`crate::delegation`]). Off, an agent
//! waits for a child inside its turn, as before this existed.
//!
//! It changes how every agent works with others, so it's chosen for nobody
//! who already had Zeron set up: an installation that predates it stays as it
//! was until the person turns it on. A new one has it on from the start,
//! unless it joins an account whose other devices don't — a new computer of
//! someone who hasn't opted in isn't a new user.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use zeron_proto::capabilities::ASYNC_DELEGATION_V1;

use crate::workspace_host::WorkspaceHost;

/// `GetAsyncDelegation` and `SetAsyncDelegation` (`{enabled}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncDelegationStatus {
    pub enabled: bool,
    /// The person chose it here (else it's this installation's default).
    pub chosen: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Stored {
    /// The person's own choice.
    chosen: Option<bool>,
    /// Zeron was first set up here with this feature in it.
    new_install: bool,
}

pub struct AsyncDelegation {
    path: PathBuf,
    stored: Mutex<Stored>,
    workspace: OnceLock<WorkspaceHost>,
}

/// Whether nothing of Zeron's chats has been kept under `data_dir` yet. Asked
/// before this run's store is opened.
fn is_new_install(data_dir: &Path) -> bool {
    // (The device id alone is made before an engine first opens its store.)
    !data_dir.join("orgs").exists()
        && !data_dir.join("profiles").join("local").join("docs.sqlite3").exists()
        && !data_dir.join("docs.sqlite3").exists()
        && !data_dir.join("docs").exists()
}

impl AsyncDelegation {
    /// Opened before the docs store is, so a first run reads as one.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("async-delegation.json");
        let stored = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let this = Self {
            path,
            stored: Mutex::new(stored.clone().unwrap_or_else(|| Stored {
                chosen: None,
                new_install: is_new_install(data_dir),
            })),
            workspace: OnceLock::new(),
        };
        // (Recorded now: from the next boot on there is a store.)
        if stored.is_none() {
            this.persist();
        }
        this
    }

    pub fn set_workspace(&self, workspace: WorkspaceHost) {
        let _ = self.workspace.set(workspace);
        self.advertise(self.enabled());
    }

    pub fn status(&self) -> AsyncDelegationStatus {
        AsyncDelegationStatus {
            enabled: self.enabled(),
            chosen: self.stored().chosen.is_some(),
        }
    }

    pub fn enabled(&self) -> bool {
        let stored = self.stored().clone();
        let enabled = stored
            .chosen
            .unwrap_or_else(|| stored.new_install && self.account_allows());
        self.advertise(enabled);
        enabled
    }

    pub fn set(&self, enabled: bool) {
        self.stored().chosen = Some(enabled);
        self.persist();
        self.advertise(enabled);
    }

    /// The account's other computers: none yet (this is its first), or one
    /// that has it on.
    fn account_allows(&self) -> bool {
        let Some(workspace) = self.workspace.get() else {
            return true;
        };
        let devices = workspace.read_devices().unwrap_or_default();
        let mut others = devices
            .iter()
            .filter(|d| d.id != workspace.device_id())
            .filter(|d| !zeron_proto::is_cloud_device(&d.id, &d.platform))
            // (Phones run no agents, and say nothing of this.)
            .filter(|d| !matches!(d.platform.as_str(), "ios" | "android" | "ipados"))
            .peekable();
        others.peek().is_none() || others.any(|d| d.supports(ASYNC_DELEGATION_V1))
    }

    /// Say so on this device's row, for the account's next device.
    fn advertise(&self, enabled: bool) {
        let Some(workspace) = self.workspace.get() else {
            return;
        };
        let Some(mut device) = workspace
            .read_devices()
            .unwrap_or_default()
            .into_iter()
            .find(|d| d.id == workspace.device_id())
        else {
            return;
        };
        if device.supports(ASYNC_DELEGATION_V1) == enabled {
            return;
        }
        device.capabilities.retain(|c| c != ASYNC_DELEGATION_V1);
        if enabled {
            device.capabilities.push(ASYNC_DELEGATION_V1.to_owned());
        }
        workspace.upsert_device_row(&device);
    }

    fn stored(&self) -> std::sync::MutexGuard<'_, Stored> {
        self.stored.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn persist(&self) {
        let stored = self.stored().clone();
        let written = serde_json::to_vec_pretty(&stored)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                let tmp = self.path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &self.path)
            });
        if let Err(error) = written {
            tracing::warn!(%error, "async delegation: the setting wasn't saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_install_has_it_on_and_keeps_that_once_it_has_a_store() {
        let dir = tempfile::tempdir().unwrap();
        // (The device id and the local profile's id come before any chat.)
        std::fs::write(dir.path().join("device-id"), "d").unwrap();
        assert!(AsyncDelegation::open(dir.path()).enabled());
        std::fs::create_dir_all(dir.path().join("profiles/local")).unwrap();
        std::fs::write(dir.path().join("profiles/local/docs.sqlite3"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("orgs/o/u")).unwrap();
        assert!(AsyncDelegation::open(dir.path()).enabled());
    }

    #[test]
    fn an_install_from_before_stays_off_until_the_person_turns_it_on() {
        for existing in ["profiles/local/docs.sqlite3", "orgs/o/u/docs.sqlite3", "docs.sqlite3"] {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join(existing);
            std::fs::create_dir_all(store.parent().unwrap()).unwrap();
            std::fs::write(store, "").unwrap();
            let setting = AsyncDelegation::open(dir.path());
            assert_eq!(
                setting.status(),
                AsyncDelegationStatus { enabled: false, chosen: false },
                "{existing}"
            );
            setting.set(true);
            assert_eq!(
                AsyncDelegation::open(dir.path()).status(),
                AsyncDelegationStatus { enabled: true, chosen: true }
            );
        }
    }

    #[tokio::test]
    async fn a_new_device_of_an_account_follows_its_other_computers() {
        for (other_on, expected) in [(false, false), (true, true)] {
            let dir = tempfile::tempdir().unwrap();
            let core = crate::EngineCore::assemble(
                dir.path(),
                std::sync::Arc::new(crate::HarnessRegistry::new()),
                zeron_proto::HarnessId::Mock,
                None,
            )
            .unwrap();
            assert!(core.sessions.async_delegation().enabled(), "its first device");
            let mut other = core
                .workspace
                .read_devices()
                .unwrap()
                .into_iter()
                .find(|d| d.id == core.device_id)
                .unwrap();
            assert!(other.supports(ASYNC_DELEGATION_V1), "advertised");
            other.id = "other".into();
            other.platform = "macos".into();
            other.capabilities.retain(|c| c != ASYNC_DELEGATION_V1);
            if other_on {
                other.capabilities.push(ASYNC_DELEGATION_V1.into());
            }
            core.workspace.upsert_device_row(&other);
            assert_eq!(core.sessions.async_delegation().enabled(), expected);
            // A phone says nothing either way.
            let mut phone = other.clone();
            phone.id = "phone".into();
            phone.platform = "ios".into();
            core.workspace.upsert_device_row(&phone);
            assert_eq!(core.sessions.async_delegation().enabled(), expected);
            core.sessions.async_delegation().set(true);
            assert!(core.sessions.async_delegation().enabled(), "the person's choice wins");
        }
    }
}
