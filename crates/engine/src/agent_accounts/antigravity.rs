//! Profile metadata only; Antigravity owns every token and refresh.
use super::*;

const DEFAULT_PROFILE: &str = "default";

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profiles {
    accounts: Vec<Profile>,
    active: Option<String>,
    #[serde(default)]
    chats: HashMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct Profile {
    id: String,
    name: String,
    saved_at: i64,
}

fn valid_id(id: &str) -> bool {
    id == DEFAULT_PROFILE
        || (id.len() == 16
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

impl AgentAccounts {
    fn antigravity_profiles_file(&self) -> PathBuf {
        self.inner
            .config
            .root_dir()
            .join("antigravity-profiles.json")
    }

    fn read_antigravity_profiles(&self) -> Result<Profiles, EngineError> {
        let raw = match std::fs::read(self.antigravity_profiles_file()) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Profiles::default());
            }
            Err(error) => return Err(error.into()),
        };
        let profiles: Profiles = serde_json::from_slice(&raw)
            .map_err(|error| EngineError::Other(format!("read Antigravity profiles: {error}")))?;
        let ids: std::collections::HashSet<_> =
            profiles.accounts.iter().map(|p| p.id.as_str()).collect();
        if ids.len() != profiles.accounts.len()
            || ids.iter().any(|id| !valid_id(id))
            || profiles
                .active
                .as_ref()
                .is_some_and(|id| !ids.contains(id.as_str()))
            || profiles
                .chats
                .values()
                .any(|id| id != DEFAULT_PROFILE && !ids.contains(id.as_str()))
        {
            return Err(EngineError::Other(
                "Invalid Antigravity profile metadata.".into(),
            ));
        }
        Ok(profiles)
    }

    fn write_antigravity_profiles(&self, profiles: &Profiles) -> Result<(), EngineError> {
        self.inner.config.private_root()?;
        let json = serde_json::to_vec_pretty(profiles).map_err(|error| {
            EngineError::Other(format!("serialize Antigravity profiles: {error}"))
        })?;
        write_file_atomic(&self.antigravity_profiles_file(), &json, true)
    }

    pub(super) fn antigravity_profile_home(&self, id: &str) -> Result<PathBuf, EngineError> {
        if !valid_id(id) {
            return Err(EngineError::Other("Unknown Antigravity account.".into()));
        }
        if id == DEFAULT_PROFILE {
            return self.inner.config.antigravity_home.clone().ok_or_else(|| {
                EngineError::Other("Cannot resolve the default Antigravity profile.".into())
            });
        }
        Ok(std::path::absolute(self.inner.config.root_dir())?
            .join("antigravity-profiles")
            .join(id))
    }

    pub(super) async fn antigravity_accounts(&self) -> Result<Vec<AgentAccount>, EngineError> {
        let legacy = self.detect_antigravity().await;
        let profiles = {
            let _ops = lock(&self.inner.antigravity_ops);
            let mut profiles = self.read_antigravity_profiles()?;
            if legacy.is_some() && !profiles.accounts.iter().any(|p| p.id == DEFAULT_PROFILE) {
                profiles.accounts.insert(
                    0,
                    Profile {
                        id: DEFAULT_PROFILE.into(),
                        name: format!(
                            "Default {}",
                            legacy.as_ref().unwrap().account().display_name.unwrap()
                        ),
                        saved_at: now_ms(),
                    },
                );
                if profiles.active.is_none() {
                    profiles.active = Some(DEFAULT_PROFILE.into());
                }
                self.write_antigravity_profiles(&profiles)?;
            }
            profiles
        };
        let mut rows = Vec::new();
        for profile in &profiles.accounts {
            let login = if profile.id == DEFAULT_PROFILE {
                legacy.clone()
            } else {
                detect_antigravity_login(&self.antigravity_profile_home(&profile.id)?, false).await
            };
            let mut row = login
                .unwrap_or(AntigravityLogin {
                    method: "oauth-personal".into(),
                })
                .account();
            row.id = profile.id.clone();
            row.display_name = Some(profile.name.clone());
            row.active = profiles.active.as_deref() == Some(profile.id.as_str());
            row.switchable = true;
            row.saved_at = Some(profile.saved_at);
            rows.push(row);
        }
        Ok(rows)
    }

    pub(super) fn activate_antigravity(&self, id: &str) -> Result<(), EngineError> {
        let _ops = lock(&self.inner.antigravity_ops);
        let mut profiles = self.read_antigravity_profiles()?;
        if !profiles.accounts.iter().any(|p| p.id == id) {
            return Err(EngineError::Other("Unknown Antigravity account.".into()));
        }
        profiles.active = Some(id.into());
        self.write_antigravity_profiles(&profiles)
    }

    pub(super) fn save_antigravity_login(&self, id: &str, name: String) -> Result<(), EngineError> {
        let _ops = lock(&self.inner.antigravity_ops);
        let mut profiles = self.read_antigravity_profiles()?;
        profiles.accounts.push(Profile {
            id: id.into(),
            name,
            saved_at: now_ms(),
        });
        if profiles.active.is_none() {
            profiles.active = Some(id.into());
        }
        self.write_antigravity_profiles(&profiles)
    }

    pub(super) fn cleanup_antigravity_login(&self, home: &Path) -> Result<(), EngineError> {
        let _ops = lock(&self.inner.antigravity_ops);
        let profiles = self.read_antigravity_profiles()?;
        let id = home
            .file_name()
            .and_then(|id| id.to_str())
            .filter(|id| *id != DEFAULT_PROFILE && valid_id(id))
            .ok_or_else(|| EngineError::Other("Invalid Antigravity login directory.".into()))?;
        if profiles.accounts.iter().any(|p| p.id == id) {
            return Ok(());
        }
        if home != self.antigravity_profile_home(id)? {
            return Err(EngineError::Other(
                "Invalid Antigravity login directory.".into(),
            ));
        }
        if home.exists() {
            let root = std::fs::canonicalize(self.inner.config.root_dir())?;
            let target = std::fs::canonicalize(home)?;
            if target == root || !target.starts_with(&root) {
                return Err(EngineError::Other(
                    "Antigravity login directory is outside the account directory.".into(),
                ));
            }
            std::fs::remove_dir_all(home)?;
        }
        Ok(())
    }

    /// A chat's first launch pins its profile on disk. Catalogs use the current default.
    pub(crate) fn antigravity_harness(
        &self,
        chat: Option<&str>,
        existing_session: bool,
    ) -> Result<Arc<zeron_harness::AcpHarness>, EngineError> {
        let _ops = lock(&self.inner.antigravity_ops);
        let mut profiles = self.read_antigravity_profiles()?;
        let id = chat
            .and_then(|chat| profiles.chats.get(chat))
            .cloned()
            .or_else(|| existing_session.then(|| DEFAULT_PROFILE.into()))
            .or_else(|| profiles.active.clone())
            .unwrap_or_else(|| DEFAULT_PROFILE.into());
        if let Some(chat) = chat
            && (profiles.active.is_some() || existing_session)
            && !profiles.chats.contains_key(chat)
        {
            profiles.chats.insert(chat.into(), id.clone());
            self.write_antigravity_profiles(&profiles)?;
        }
        let home = self.antigravity_profile_home(&id)?;
        let name = profiles
            .accounts
            .iter()
            .find(|profile| profile.id == id)
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| "Default Google account".into());
        let mut harnesses = lock(&self.inner.antigravity_harnesses);
        Ok(harnesses
            .entry(id.clone())
            .or_insert_with(|| {
                Arc::new(
                    self.acp_harness(HarnessId::Antigravity)
                        .expect("Antigravity harness")
                        .with_antigravity_profile(home, id != DEFAULT_PROFILE)
                        .with_antigravity_account_name(name),
                )
            })
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(accounts: &AgentAccounts, name: &str) -> String {
        let id = slot_id_for(HarnessId::Antigravity, &new_id());
        accounts.inner.config.private_root().unwrap();
        let home = accounts.antigravity_profile_home(&id).unwrap();
        private_dir(home.parent().unwrap()).unwrap();
        private_dir(&home).unwrap();
        let acp = home.join("antigravity-acp");
        private_dir(&acp).unwrap();
        std::fs::write(acp.join("acp_token.json"), "opaque-test-token").unwrap();
        accounts.save_antigravity_login(&id, name.into()).unwrap();
        id
    }

    #[tokio::test]
    async fn profiles_switch_without_changing_existing_chats_and_survive_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let config = AgentAccountsConfig::isolated(tmp.path());
        let accounts = AgentAccounts::new(config.clone());
        let first = saved(&accounts, "Personal");
        let second = saved(&accounts, "Work");
        let registry = crate::HarnessRegistry::new();
        registry.register(Arc::new(zeron_harness::AcpHarness::antigravity()));
        registry.set_agent_accounts(accounts.clone());
        let old_harness = registry
            .resolve_for_chat(HarnessId::Antigravity, Some("old-chat"), false)
            .unwrap();
        accounts
            .activate(HarnessId::Antigravity, &second)
            .await
            .unwrap();
        let new_harness = registry
            .resolve_for_chat(HarnessId::Antigravity, Some("new-chat"), false)
            .unwrap();
        assert!(!Arc::ptr_eq(&old_harness, &new_harness));
        assert!(Arc::ptr_eq(
            &old_harness,
            &registry
                .resolve_for_chat(HarnessId::Antigravity, Some("old-chat"), false)
                .unwrap()
        ));
        assert!(Arc::ptr_eq(
            &new_harness,
            &registry.resolve(HarnessId::Antigravity).unwrap()
        ));
        let rows = accounts.antigravity_accounts().await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display_name.as_deref(), Some("Personal"));
        assert!(!rows[0].active && rows[1].active && rows.iter().all(|r| r.switchable));
        drop(registry);
        drop(accounts);
        let accounts = AgentAccounts::new(config);
        accounts
            .antigravity_harness(Some("old-chat"), false)
            .unwrap();
        let profiles = accounts.read_antigravity_profiles().unwrap();
        assert_eq!(profiles.active.as_deref(), Some(second.as_str()));
        assert_eq!(profiles.chats["old-chat"], first);
        assert_eq!(profiles.chats["new-chat"], second);
        assert!(
            accounts
                .forget(HarnessId::Antigravity, &first)
                .await
                .is_err()
        );
        assert!(accounts.antigravity_profile_home(&first).unwrap().is_dir());
        let raw = std::fs::read_to_string(accounts.antigravity_profiles_file()).unwrap();
        assert!(!raw.contains("opaque-test-token"));
    }

    #[tokio::test]
    async fn a_failed_attempt_before_sign_in_does_not_pin_an_empty_default_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(AgentAccountsConfig::isolated(tmp.path()));
        accounts.antigravity_harness(Some("chat"), false).unwrap();
        assert!(
            accounts
                .read_antigravity_profiles()
                .unwrap()
                .chats
                .is_empty()
        );
        let first = saved(&accounts, "Personal");
        accounts.antigravity_harness(Some("chat"), false).unwrap();
        assert_eq!(
            accounts.read_antigravity_profiles().unwrap().chats["chat"],
            first
        );
    }

    #[tokio::test]
    async fn invalid_ids_and_metadata_never_touch_profiles() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(AgentAccountsConfig::isolated(tmp.path()));
        let first = saved(&accounts, "Personal");
        let second = saved(&accounts, "Work");
        assert!(accounts.antigravity_profile_home("../escape").is_err());
        assert!(
            accounts
                .forget(HarnessId::Antigravity, &first)
                .await
                .is_err()
        );
        assert!(accounts.antigravity_profile_home(&first).unwrap().is_dir());
        assert!(accounts.antigravity_profile_home(&second).unwrap().is_dir());
        std::fs::write(accounts.antigravity_profiles_file(), "{broken").unwrap();
        assert!(accounts.activate_antigravity(&second).is_err());
        assert!(accounts.antigravity_harness(Some("chat"), false).is_err());
        let snapshot = accounts.list(false).await.unwrap();
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.harness == HarnessId::Antigravity)
        );
        assert_eq!(
            std::fs::read_to_string(accounts.antigravity_profiles_file()).unwrap(),
            "{broken"
        );
    }

    #[tokio::test]
    async fn legacy_sessions_stay_on_the_default_profile_after_switching() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(AgentAccountsConfig::isolated(tmp.path()));
        let legacy = accounts
            .inner
            .config
            .antigravity_home
            .as_ref()
            .unwrap()
            .join("antigravity-acp");
        private_dir(&legacy).unwrap();
        std::fs::write(legacy.join("acp_token.json"), "legacy-token").unwrap();
        let rows = accounts.antigravity_accounts().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].active);
        let second = saved(&accounts, "Work");
        accounts.activate_antigravity(&second).unwrap();
        accounts
            .antigravity_harness(Some("pre-upgrade-chat"), true)
            .unwrap();
        accounts
            .antigravity_harness(Some("fresh-chat"), false)
            .unwrap();
        let profiles = accounts.read_antigravity_profiles().unwrap();
        assert_eq!(profiles.chats["pre-upgrade-chat"], DEFAULT_PROFILE);
        assert_eq!(profiles.chats["fresh-chat"], second);
        assert_eq!(
            std::fs::read_to_string(legacy.join("acp_token.json")).unwrap(),
            "legacy-token"
        );
    }

    #[tokio::test]
    async fn login_cleanup_keeps_completed_profiles_and_reclaims_cancelled_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(AgentAccountsConfig::isolated(tmp.path()));
        let id = saved(&accounts, "Personal");
        let home = accounts.antigravity_profile_home(&id).unwrap();
        accounts.cleanup_antigravity_login(&home).unwrap();
        assert!(home.is_dir());
        let cancelled = accounts
            .antigravity_profile_home("0123456789abcdef")
            .unwrap();
        private_dir(&cancelled).unwrap();
        accounts.cleanup_antigravity_login(&cancelled).unwrap();
        assert!(!cancelled.exists());
        assert!(accounts.cleanup_antigravity_login(tmp.path()).is_err());
        assert!(home.is_dir());
        for name in ["line\nbreak".to_string(), "x".repeat(101)] {
            assert!(
                accounts
                    .start_login_named(HarnessId::Antigravity, None, None, Some(&name))
                    .await
                    .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_login_cleanup_rejects_a_profile_replaced_with_an_external_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(AgentAccountsConfig::isolated(tmp.path()));
        accounts.inner.config.private_root().unwrap();
        let home = accounts
            .antigravity_profile_home("0123456789abcdef")
            .unwrap();
        private_dir(home.parent().unwrap()).unwrap();
        std::fs::write(outside.path().join("keep"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), &home).unwrap();
        assert!(accounts.cleanup_antigravity_login(&home).is_err());
        assert!(outside.path().join("keep").is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adding_two_accounts_runs_oauth_in_fresh_profiles() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(AgentAccountsConfig::isolated(tmp.path()));
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../harness/tests/fixtures/fake-antigravity-profiles.sh");
        std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755)).unwrap();
        accounts.override_cli(HarnessId::Antigravity, fixture);
        for name in ["Personal", "Work"] {
            let start = accounts
                .start_login_named(HarnessId::Antigravity, None, None, Some(name))
                .await
                .unwrap();
            let poll = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let poll = accounts.poll_login(&start.login_id).await.unwrap();
                    if poll.status != AgentLoginStatus::Pending {
                        break poll;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(poll.status, AgentLoginStatus::Done, "{:?}", poll.message);
        }
        let rows = accounts.antigravity_accounts().await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display_name.as_deref(), Some("Personal"));
        assert_eq!(rows[1].display_name.as_deref(), Some("Work"));
        assert!(rows[0].active && !rows[1].active);
        assert_ne!(rows[0].id, rows[1].id);
        for row in rows {
            assert!(
                accounts
                    .antigravity_profile_home(&row.id)
                    .unwrap()
                    .join("antigravity-acp/acp_token.json")
                    .is_file()
            );
        }
    }
}
