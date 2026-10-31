//! Muse Code accounts, kept by zeron — any number of them, each its own Muse
//! login, nothing ever swapped.
//!
//! Muse keeps ONE login per config root (`$XDG_CONFIG_HOME/muse`), on macOS
//! in the Keychain by default. So each account zeron adds gets a config root
//! of its own under `{data_dir}/agent-accounts/muse-homes/<slot>/config`,
//! file-backed (`TBH_CREDENTIAL_BACKEND=file`, so no Keychain prompt can
//! stall a background engine), and a Muse run of that account is pointed at
//! it. Switching is which root the next run gets — the parked session is
//! replaced at its next turn through the run-credential fingerprint — never
//! a rewrite of a store another process might be refreshing.
//!
//! `XDG_CONFIG_HOME` reaches every tool Muse runs (git, gh, …), so the root
//! MIRRORS the user's real config: a link to every entry there, except
//! `muse/`, whose entries are linked in turn except `auth.json` — the one
//! file that is the account's own. Links are refreshed at every spawn.
//!
//! Sign-in, identity, usage and sign-out all go through Muse's own account
//! surface (verified live against Muse 1.4.12; usage stays empty from a
//! fresh host, see [`AgentAccounts::muse_usage`]) ([`zeron_harness::muse::account`]): Muse mints, refreshes and
//! locks its tokens itself; zeron never reads one. With no zeron account
//! selected, Muse runs on its own login (`muse login`), untouched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use zeron_harness::muse::account as msp;
use zeron_proto::{AgentLoginMode, AgentLoginStart, AgentUsageWindow, HarnessId};

use super::{
    AgentAccounts, EngineError, LoginFlow, Slot, SlotProfile, TaskLoginState, UsageSnapshot, lock,
    new_id, now_ms, private_dir, slot_id_for,
};

/// The selected zeron-kept account's slot id (absent: Muse's own login).
const ACTIVE_FILE: &str = "muse-active";
const HOMES_DIR: &str = "muse-homes";
/// The run-credential fingerprint of Muse's own login (no zeron account).
pub(crate) const OWN_LOGIN: &str = "muse-own-login";

/// Make `root` mirror `real` (see module docs): links to every entry of
/// `real` except `muse`, and `root/muse/` linking every entry of `real/muse`
/// except `auth.json`. Stale links (an entry since removed) are pruned; real
/// files in `root` (the account's `auth.json`, anything Muse wrote in place
/// of a link) are left alone.
#[cfg(unix)]
fn mirror_config(real: &Path, root: &Path) -> std::io::Result<()> {
    fn link_entries(from: &Path, into: &Path, skip: impl Fn(&str) -> bool) -> std::io::Result<()> {
        std::fs::create_dir_all(into)?;
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
                // A real entry Muse wrote here: the account's own now.
                Ok(_) => continue,
                Err(_) => {}
            }
            std::os::unix::fs::symlink(&target, &link)?;
        }
        Ok(())
    }
    link_entries(real, root, |name| name == "muse")?;
    let muse = root.join("muse");
    if std::fs::symlink_metadata(&muse).is_ok_and(|meta| meta.file_type().is_symlink()) {
        std::fs::remove_file(&muse)?;
    }
    // The credential store is the account's own: its file, Muse's lock on
    // it (shared, it would serialize every account and the user's own
    // login) and any temp file of an atomic write.
    link_entries(&real.join("muse"), &muse, |name| {
        name.starts_with("auth.json") || name.starts_with(".auth.json")
    })
}

#[cfg(not(unix))]
fn mirror_config(_real: &Path, _root: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Muse accounts kept by Zeron need macOS or Linux",
    ))
}

fn muse_error(err: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!("Muse Code: {err}"))
}

impl AgentAccounts {
    fn muse_cli(&self) -> Result<PathBuf, EngineError> {
        if let Some(path) = lock(&self.inner.cli_overrides).get(&HarnessId::Muse) {
            return Ok(path.clone());
        }
        zeron_harness::MuseHarness::new()
            .resolve_executable()
            .map_err(muse_error)
    }

    fn muse_home(&self, name: &str) -> Result<PathBuf, EngineError> {
        let homes = self.inner.config.private_root()?.join(HOMES_DIR);
        private_dir(&homes)?;
        Ok(homes.join(name))
    }

    /// The environment that points Muse at account `name`'s config root,
    /// with its mirror refreshed.
    fn muse_env(&self, name: &str) -> Result<BTreeMap<String, String>, EngineError> {
        let home = self.muse_home(name)?;
        private_dir(&home)?;
        let root = home.join("config");
        mirror_config(&self.inner.config.muse_config_root.clone(), &root).map_err(muse_error)?;
        Ok(BTreeMap::from([
            (
                "XDG_CONFIG_HOME".into(),
                root.to_string_lossy().into_owned(),
            ),
            ("TBH_CREDENTIAL_BACKEND".into(), "file".into()),
        ]))
    }

    fn active_file(&self) -> Result<PathBuf, EngineError> {
        Ok(self.inner.config.private_root()?.join(ACTIVE_FILE))
    }

    /// The selected zeron-kept account, when there is one (and it still
    /// exists).
    pub(super) fn muse_active(&self) -> Option<Slot> {
        let id = std::fs::read_to_string(self.active_file().ok()?).ok()?;
        self.read_slot(HarnessId::Muse, id.trim())
    }

    pub(super) fn set_muse_active(&self, id: Option<&str>) -> Result<(), EngineError> {
        let file = self.active_file()?;
        match id {
            Some(id) => super::write_file_atomic(&file, id.as_bytes(), true),
            None => super::remove_if_exists(&file),
        }
    }

    /// What a Muse run uses now: the selected account's environment, and the
    /// fingerprint that replaces a parked session started with another.
    pub fn muse_run_env(&self) -> Result<(Option<BTreeMap<String, String>>, String), String> {
        match self.muse_active() {
            Some(slot) => Ok((
                Some(self.muse_env(&slot.id).map_err(|e| e.to_string())?),
                slot.id,
            )),
            None => Ok((None, OWN_LOGIN.to_string())),
        }
    }

    /// Sign a new Muse account in: Muse's own device-code flow, run against
    /// a fresh config root, adopted as a slot once approved. Re-signing an
    /// account already kept replaces its login in place.
    pub(super) async fn start_muse_login(&self) -> Result<AgentLoginStart, EngineError> {
        // A sign-in already waiting for approval is the answer: replacing it
        // would kill the Muse holding its device code, and the page already
        // open for that code would then say it is invalid.
        if let Some(start) = self.pending_muse_login() {
            return Ok(start);
        }
        self.reap_spawned_flows(HarnessId::Muse);
        self.sweep_muse_pending();
        let cli = self.muse_cli()?;
        let pending = format!("pending-{}", new_id());
        let env = self.muse_env(&pending)?;
        let pending_home = self.muse_home(&pending)?;
        let login = match msp::start_device_login(&cli, &env).await {
            Ok(login) => login,
            Err(err) => {
                let _ = std::fs::remove_dir_all(self.muse_home(&pending)?);
                return Err(muse_error(err));
            }
        };
        let url = login.verification_url.clone();
        tracing::info!(%pending, %url, "Muse sign-in started");
        let state = std::sync::Arc::new(std::sync::Mutex::new(TaskLoginState {
            url: Some(url.clone()),
            message: Some(format!("Enter the code {} on Meta.", login.user_code)),
            ..Default::default()
        }));
        let this = self.clone();
        let task_state = state.clone();
        let handle = tokio::spawn(async move {
            let outcome = match login.finish().await {
                Ok(account) => this.adopt_muse_login(&pending, account).await,
                Err(reason) => Err(EngineError::Other(reason)),
            };
            match &outcome {
                Ok(()) => tracing::info!(%pending, "Muse sign-in adopted"),
                Err(error) => tracing::warn!(%pending, %error, "Muse sign-in failed"),
            }
            if outcome.is_err()
                && let Ok(home) = this.muse_home(&pending)
            {
                let _ = std::fs::remove_dir_all(home);
            }
            lock(&task_state).outcome = Some(outcome.map_err(|e| e.to_string()));
        });
        let login_id = new_id();
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness: HarnessId::Muse,
                started_at: std::time::Instant::now(),
                state,
                handle,
                // Cancelled (dialog closed, superseded): the root goes too.
                home: Some(pending_home),
                port: None,
            },
        );
        Ok(AgentLoginStart {
            login_id,
            url,
            mode: AgentLoginMode::Browser,
            callback_port: None,
        })
    }

    /// The Muse sign-in still waiting for approval, if any.
    fn pending_muse_login(&self) -> Option<AgentLoginStart> {
        let flows = lock(&self.inner.flows);
        flows.iter().find_map(|(login_id, flow)| match flow {
            LoginFlow::Task {
                harness: HarnessId::Muse,
                state,
                handle,
                ..
            } if !handle.is_finished() && lock(state).outcome.is_none() => Some(AgentLoginStart {
                login_id: login_id.clone(),
                url: lock(state).url.clone().unwrap_or_default(),
                mode: AgentLoginMode::Browser,
                callback_port: None,
            }),
            _ => None,
        })
    }

    /// Roots of sign-ins that never finished (an engine stopped mid-flow),
    /// once older than any flow can live.
    fn sweep_muse_pending(&self) {
        let Ok(homes) = self
            .inner
            .config
            .private_root()
            .map(|root| root.join(HOMES_DIR))
        else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(homes) else {
            return;
        };
        for entry in entries.flatten() {
            let stale = entry.file_name().to_string_lossy().starts_with("pending-")
                && entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .is_ok_and(|at| at.elapsed().unwrap_or_default() > super::FLOW_TTL);
            if stale {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    /// An approved sign-in in `pending`'s root becomes a kept account.
    async fn adopt_muse_login(
        &self,
        pending: &str,
        account: msp::MuseAccount,
    ) -> Result<(), EngineError> {
        if !account.signed_in() {
            return Err(EngineError::Other(
                "Muse finished the sign-in but reports no login.".into(),
            ));
        }
        let _ops = self.inner.ops.lock().await;
        let account_key = account
            .label
            .clone()
            .unwrap_or_else(|| format!("muse:{pending}"));
        let id = slot_id_for(HarnessId::Muse, &account_key);
        let from = self.muse_home(pending)?;
        let to = self.muse_home(&id)?;
        if to.exists() {
            // A re-login of a kept account: its new login replaces the old.
            let auth = |home: &Path| home.join("config").join("muse").join("auth.json");
            std::fs::rename(auth(&from), auth(&to))?;
            let _ = std::fs::remove_dir_all(&from);
        } else {
            std::fs::rename(&from, &to)?;
        }
        self.write_slot(&Slot {
            id: id.clone(),
            harness: HarnessId::Muse,
            account_key,
            profile: SlotProfile {
                email: account
                    .label
                    .clone()
                    .unwrap_or_else(|| "Meta account".into()),
                display_name: None,
                organization: None,
                plan: None,
                auth_kind: zeron_proto::AgentAuthKind::Oauth,
            },
            credentials: serde_json::json!({}),
            claude_config: None,
            saved_at: now_ms(),
            created_at: None,
            store_key: None,
        })?;
        if self.muse_active().is_none() {
            self.set_muse_active(Some(&id))?;
        }
        Ok(())
    }

    /// A run of kept account `slot_id` reported the subscription's usage:
    /// it becomes that account's meters (Muse observes usage only on the
    /// host doing the work, so a probe would never see any).
    pub(super) fn record_muse_usage(
        &self,
        slot_id: &str,
        windows: Vec<AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        let Some(slot) = self.read_slot(HarnessId::Muse, slot_id) else {
            return;
        };
        let mut usage = lock(&self.inner.usage);
        usage
            .entry(super::usage_key(HarnessId::Muse, &slot.account_key))
            .or_default()
            .record(
                Ok(UsageSnapshot {
                    windows,
                    plan_label,
                }),
                String::new(),
                now_ms(),
            );
        self.persist_usage(&usage);
    }

    /// Remove a kept account: Muse signs it out (revoking the session,
    /// best-effort), then its root goes. The selection falls back to Muse's
    /// own login.
    pub(super) async fn forget_muse(&self, id: &str) -> Result<(), EngineError> {
        if let (Ok(cli), Ok(env)) = (self.muse_cli(), self.muse_env(id)) {
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(20), msp::logout(&cli, &env))
                    .await;
        }
        if self.muse_active().is_some_and(|slot| slot.id == id) {
            self.set_muse_active(None)?;
        }
        let home = self.muse_home(id)?;
        if home.exists() {
            std::fs::remove_dir_all(home)?;
        }
        super::remove_if_exists(&self.slots_dir(HarnessId::Muse)?.join(format!("{id}.json")))
    }
}

/// Points Muse runs at the selected kept account (see module docs).
pub struct MuseRunEnvironment(pub AgentAccounts);

#[async_trait::async_trait]
impl crate::registry::RunEnvironment for MuseRunEnvironment {
    async fn prepare(
        &self,
        harness: HarnessId,
        request: &mut zeron_proto::RunRequest,
    ) -> Result<(), String> {
        if harness != HarnessId::Muse {
            return Ok(());
        }
        if let (Some(env), _) = self.0.muse_run_env()? {
            for (key, value) in env {
                request.env.entry(key).or_insert(value);
            }
        }
        Ok(())
    }

    async fn credential(&self, harness: HarnessId) -> Option<String> {
        (harness == HarnessId::Muse)
            .then(|| {
                self.0
                    .muse_run_env()
                    .ok()
                    .map(|(_, fingerprint)| fingerprint)
            })
            .flatten()
    }

    async fn record_usage(
        &self,
        harness: HarnessId,
        credential: &str,
        windows: Vec<AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        // Muse's own login (no kept account) has no row to show it on.
        if harness == HarnessId::Muse && credential != OWN_LOGIN {
            self.0.record_muse_usage(credential, windows, plan_label);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_root_mirrors_the_real_config_except_the_accounts_own_login() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("git")).unwrap();
        std::fs::create_dir_all(real.join("muse/skills")).unwrap();
        std::fs::write(real.join("muse/settings.json"), "{}").unwrap();
        std::fs::write(real.join("muse/auth.json"), "the user's own login").unwrap();
        std::fs::write(real.join("muse/.auth.json.lock"), "").unwrap();
        std::fs::write(real.join("muse/auth.json.tmp-1"), "").unwrap();
        let root = tmp.path().join("account/config");

        mirror_config(&real, &root).unwrap();
        assert_eq!(
            std::fs::read_link(root.join("git")).unwrap(),
            real.join("git")
        );
        assert_eq!(
            std::fs::read_link(root.join("muse/settings.json")).unwrap(),
            real.join("muse/settings.json")
        );
        assert!(root.join("muse/skills").is_dir());
        assert!(
            !root.join("muse/auth.json").exists(),
            "never the user's login"
        );
        assert!(
            std::fs::symlink_metadata(root.join("muse/.auth.json.lock")).is_err(),
            "never the user's credential lock"
        );
        assert!(std::fs::symlink_metadata(root.join("muse/auth.json.tmp-1")).is_err());

        // The account's own login, an entry added and one removed later.
        std::fs::write(root.join("muse/auth.json"), "the account's login").unwrap();
        std::fs::create_dir_all(real.join("gh")).unwrap();
        std::fs::remove_dir_all(real.join("git")).unwrap();
        mirror_config(&real, &root).unwrap();
        assert!(root.join("gh").exists());
        assert!(std::fs::symlink_metadata(root.join("git")).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("muse/auth.json")).unwrap(),
            "the account's login"
        );
    }

    /// A `muse serve` speaking just the account surface: the login lives in
    /// `$XDG_CONFIG_HOME/muse/auth.json`, the account named by `$FAKE_EMAILS`'
    /// next line (one per sign-in, counted in `$FAKE_DIR/n`).
    const FAKE_MUSE: &str = r#"#!/usr/bin/env python3
import json, os, sys, time
root = os.environ["XDG_CONFIG_HOME"]; auth = os.path.join(root, "muse", "auth.json")
fake = os.environ["FAKE_DIR"]
assert os.environ.get("TBH_CREDENTIAL_BACKEND") == "file"
def send(obj): sys.stdout.write(json.dumps(obj) + "\n"); sys.stdout.flush()
def state():
    if not os.path.exists(auth): return {"state": "loggedOut", "credentialRequired": True}
    return {"state": "accountLogin", "label": json.load(open(auth))["email"], "credentialRequired": True}
for line in sys.stdin:
    msg = json.loads(line); method = msg.get("method"); rid = msg.get("id")
    if rid is None: continue
    if method == "initialize":
        assert msg["params"]["capabilities"]["experimentalApi"] is True
        send({"jsonrpc": "2.0", "id": rid, "result": {"schema": {"version": 1}, "grantedCapabilities": []}})
    elif method == "account/read": send({"jsonrpc": "2.0", "id": rid, "result": state()})
    elif method == "account/loginStart":
        send({"jsonrpc": "2.0", "id": rid, "result": {"verificationUrl": "https://auth.example/device", "userCode": "ABCD-EFGH"}})
        if os.environ.get("FAKE_HOLD"): continue  # never approved
        n = int(open(os.path.join(fake, "n")).read()) if os.path.exists(os.path.join(fake, "n")) else 0
        open(os.path.join(fake, "n"), "w").write(str(n + 1))
        email = os.environ["FAKE_EMAILS"].split(",")[n]
        time.sleep(0.2)
        os.makedirs(os.path.dirname(auth), exist_ok=True)
        json.dump({"email": email}, open(auth, "w"))
        send({"jsonrpc": "2.0", "method": "account/loginCompleted", "params": {"outcome": "granted"}})
    elif method == "usage/read":
        open(os.path.join(fake, "usage-probed"), "w").write("1")
        send({"jsonrpc": "2.0", "id": rid, "result": {"usage": {"observedAtMs": 1, "tier": "pro",
            "window": {"usedPercent": 25, "resetsAtMs": 1000, "windowDurationMins": 300},
            "weekly": {"usedPercent": 50, "resetsAtMs": 2000}}}})
    elif method == "account/logout":
        if os.path.exists(auth): os.remove(auth)
        open(os.path.join(fake, "logouts"), "a").write("1")
        send({"jsonrpc": "2.0", "id": rid, "result": state()})
    else: send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "nope"}})
"#;

    fn run_request() -> zeron_proto::RunRequest {
        serde_json::from_str(
            r#"{"prompt":"p","model":null,"reasoning":null,"cwd":".","sandbox":"workspace-write","resume":null}"#,
        )
        .unwrap()
    }

    async fn poll_done(accounts: &AgentAccounts, login_id: &str) {
        for _ in 0..200 {
            let poll = accounts.poll_login(login_id).await.unwrap();
            match poll.status {
                zeron_proto::AgentLoginStatus::Pending => {}
                zeron_proto::AgentLoginStatus::Done => return,
                zeron_proto::AgentLoginStatus::Error => panic!("{:?}", poll.message),
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("sign-in never finished");
    }

    #[tokio::test]
    async fn a_second_start_reuses_the_waiting_sign_in_and_closing_it_cleans_up() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("fake");
        std::fs::create_dir_all(&fake).unwrap();
        let cli = fake.join("muse");
        std::fs::write(&cli, FAKE_MUSE).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = super::super::AgentAccountsConfig::isolated(&tmp.path().join("root"));
        let accounts = AgentAccounts::new(config.clone());
        // A wrapper holding every sign-in open (never approved).
        let hold = fake.join("muse-hold");
        std::fs::write(
            &hold,
            format!(
                "#!/bin/sh\nFAKE_HOLD=1 FAKE_DIR='{}' FAKE_EMAILS=x exec '{}' \"$@\"\n",
                fake.display(),
                cli.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hold, std::fs::Permissions::from_mode(0o755)).unwrap();
        accounts.override_cli(HarnessId::Muse, &hold);

        let first = accounts.start_login(HarnessId::Muse).await.unwrap();
        let second = accounts.start_login(HarnessId::Muse).await.unwrap();
        assert_eq!(
            second.login_id, first.login_id,
            "the open page's code stays valid"
        );
        assert_eq!(second.url, first.url);
        let homes = config.private_root().unwrap().join(HOMES_DIR);
        let pending = || {
            std::fs::read_dir(&homes)
                .map(|entries| entries.flatten().count())
                .unwrap_or(0)
        };
        assert_eq!(pending(), 1);
        accounts.cancel_login(&first.login_id);
        assert_eq!(pending(), 0, "a closed sign-in leaves no root behind");
    }

    #[tokio::test]
    async fn kept_muse_accounts_sign_in_switch_report_usage_and_go() {
        use crate::registry::RunEnvironment;
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("fake");
        std::fs::create_dir_all(&fake).unwrap();
        let cli = fake.join("muse");
        std::fs::write(&cli, FAKE_MUSE).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: read only by the fake child processes this test spawns.
        unsafe {
            std::env::set_var("FAKE_DIR", &fake);
            std::env::set_var("FAKE_EMAILS", "a@example.com,b@example.com");
        }
        let config = super::super::AgentAccountsConfig::isolated(&tmp.path().join("root"));
        std::fs::create_dir_all(config.muse_config_root.join("muse")).unwrap();
        std::fs::write(config.muse_config_root.join("muse/auth.json"), "own login").unwrap();
        let accounts = AgentAccounts::new(config.clone());
        accounts.override_cli(HarnessId::Muse, &cli);
        let env = MuseRunEnvironment(accounts.clone());

        // None kept yet: Muse runs on its own login.
        assert_eq!(
            env.credential(HarnessId::Muse).await.as_deref(),
            Some(OWN_LOGIN)
        );

        let start = accounts.start_login(HarnessId::Muse).await.unwrap();
        assert_eq!(start.url, "https://auth.example/device");
        let poll = accounts.poll_login(&start.login_id).await.unwrap();
        assert!(poll.message.unwrap_or_default().contains("ABCD-EFGH"));
        poll_done(&accounts, &start.login_id).await;
        let start = accounts.start_login(HarnessId::Muse).await.unwrap();
        poll_done(&accounts, &start.login_id).await;

        let rows = |snapshot: zeron_proto::AgentAccountsSnapshot| {
            snapshot
                .accounts
                .into_iter()
                .filter(|a| a.harness == HarnessId::Muse)
                .map(|a| (a.email.unwrap_or_default(), a.active, a.id))
                .collect::<Vec<_>>()
        };
        let listed = rows(accounts.list(true).await.unwrap());
        assert_eq!(listed.len(), 2);
        // The first one signed in is selected; the second is kept beside it.
        assert_eq!((listed[0].0.as_str(), listed[0].1), ("a@example.com", true));
        assert_eq!(
            (listed[1].0.as_str(), listed[1].1),
            ("b@example.com", false)
        );
        // Usage arrives from a run on that account (never from a probe).
        let window = |label: &str| AgentUsageWindow {
            label: label.into(),
            used_fraction: 0.25,
            resets_at: None,
        };
        env.record_usage(
            HarnessId::Muse,
            &listed[0].2,
            vec![window("Session"), window("Week")],
            Some("pro".into()),
        )
        .await;
        let usage = accounts.list(false).await.unwrap();
        let a = usage.accounts.iter().find(|a| a.id == listed[0].2).unwrap();
        assert_eq!(a.usage_windows.len(), 2);
        assert_eq!(a.plan_label.as_deref(), Some("pro"));
        let b = usage.accounts.iter().find(|a| a.id == listed[1].2).unwrap();
        assert!(b.usage_windows.is_empty(), "only the account that ran");
        assert!(!fake.join("usage-probed").exists());

        // A run gets the selected account's root; switching changes it.
        let mut request = run_request();
        env.prepare(HarnessId::Muse, &mut request).await.unwrap();
        let root_a = request.env["XDG_CONFIG_HOME"].clone();
        assert_eq!(request.env["TBH_CREDENTIAL_BACKEND"], "file");
        assert_eq!(
            std::fs::read_to_string(Path::new(&root_a).join("muse/auth.json")).unwrap(),
            r#"{"email": "a@example.com"}"#
        );
        let before = env.credential(HarnessId::Muse).await;
        let listed = rows(
            accounts
                .activate(HarnessId::Muse, &listed[1].2)
                .await
                .unwrap(),
        );
        assert!(listed[1].1 && !listed[0].1);
        assert_ne!(env.credential(HarnessId::Muse).await, before);
        let mut request = run_request();
        env.prepare(HarnessId::Muse, &mut request).await.unwrap();
        assert_ne!(request.env["XDG_CONFIG_HOME"], root_a);
        // Other agents are untouched; the user's own login never moved.
        let mut other = run_request();
        env.prepare(HarnessId::ClaudeCode, &mut other)
            .await
            .unwrap();
        assert!(other.env.is_empty());
        assert_eq!(
            std::fs::read_to_string(config.muse_config_root.join("muse/auth.json")).unwrap(),
            "own login"
        );

        // Removing the selected account signs it out and falls back.
        let listed = rows(
            accounts
                .forget(HarnessId::Muse, &listed[1].2)
                .await
                .unwrap(),
        );
        assert_eq!(listed.len(), 1);
        assert_eq!(std::fs::read_to_string(fake.join("logouts")).unwrap(), "1");
        assert_eq!(
            env.credential(HarnessId::Muse).await.as_deref(),
            Some(OWN_LOGIN)
        );
    }

    /// Against a real `muse` (opt-in, never approves anything):
    /// `MUSE_LIVE_BIN=~/.local/bin/muse cargo test -p zeron-engine --lib
    /// live_muse_sign_in_stays_pending -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_muse_sign_in_stays_pending_until_approved() {
        let Some(bin) = std::env::var_os("MUSE_LIVE_BIN") else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let config = super::super::AgentAccountsConfig::isolated(&tmp.path().join("root"));
        std::fs::create_dir_all(config.muse_config_root.join("muse")).unwrap();
        std::fs::write(config.muse_config_root.join("muse/.auth.json.lock"), "").unwrap();
        let accounts = AgentAccounts::new(config);
        accounts.override_cli(HarnessId::Muse, PathBuf::from(bin));
        let start = accounts.start_login(HarnessId::Muse).await.unwrap();
        println!("url {}", start.url);
        for second in 0..20 {
            let poll = accounts.poll_login(&start.login_id).await.unwrap();
            println!("{second}s: {:?} {:?}", poll.status, poll.message);
            assert_eq!(poll.status, zeron_proto::AgentLoginStatus::Pending);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        accounts.cancel_login(&start.login_id);
    }

    /// Signs in for real through the engine (approve the printed code):
    /// `MUSE_LIVE_BIN=~/.local/bin/muse cargo test -p zeron-engine --lib
    /// live_muse_sign_in_is_adopted -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_muse_sign_in_is_adopted_once_approved() {
        let Some(bin) = std::env::var_os("MUSE_LIVE_BIN") else {
            return;
        };
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let tmp = tempfile::tempdir().unwrap();
        let config = super::super::AgentAccountsConfig::isolated(&tmp.path().join("root"));
        let accounts = AgentAccounts::new(config);
        accounts.override_cli(HarnessId::Muse, PathBuf::from(bin));
        let start = accounts.start_login(HarnessId::Muse).await.unwrap();
        println!("\n>>> Open {}\n", start.url);
        for _ in 0..900 {
            let poll = accounts.poll_login(&start.login_id).await.unwrap();
            if poll.status != zeron_proto::AgentLoginStatus::Pending {
                println!("finished: {:?} {:?}", poll.status, poll.message);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        let rows: Vec<_> = accounts
            .list(false)
            .await
            .unwrap()
            .accounts
            .into_iter()
            .filter(|a| a.harness == HarnessId::Muse)
            .collect();
        println!("accounts: {rows:?}");
        for row in &rows {
            accounts.forget(HarnessId::Muse, &row.id).await.unwrap();
        }
        assert_eq!(rows.len(), 1);
    }
}
