//! The engine's side of an update ([`zeron_update::transaction`]): settle what an
//! earlier start left before anything opens the data directory, and as a new
//! version on trial, prove it's ready before it's allowed to stay.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zeron_proto::WorkspaceScope;
use zeron_update::transaction;

use crate::{EngineConfig, EngineCore, InstanceLock};

/// How long a predecessor gets to let go of the data directory.
const HANDOFF: Duration = Duration::from_secs(60);
/// How long a new version gets to reach Zeron's servers. A box nobody can reach
/// is as stranded as one that doesn't start, and there's no point sitting
/// through the whole deadline to find out.
const EDGE_PROOF: Duration = Duration::from_secs(3 * 60);
/// Still running a moment after being ready: catches a failure just after
/// startup, before it can't be undone.
const SETTLE: Duration = Duration::from_secs(5);
/// Long enough for the old version's watchdog to have gone.
const TIDY_AFTER: Duration = Duration::from_secs(2);
/// A trial that gives up. Not zero: a service restarted only on failure has to
/// start the old version.
const TRIAL_FAILED_EXIT: i32 = 75;

/// Before anything else in an engine's start. May not return: it's where the old
/// version hands this process over to the new one. An error means the databases
/// aren't safe to open yet.
pub(crate) fn boot(data_dir: &Path) -> anyhow::Result<()> {
    transaction::boot(data_dir, &transaction::BootOptions::for_this_process(), || {
        let deadline = std::time::Instant::now() + HANDOFF;
        loop {
            match InstanceLock::acquire(data_dir) {
                Ok(lock) => return Ok(lock),
                Err(err) if std::time::Instant::now() >= deadline => return Err(err.into()),
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        }
    })
}

/// An engine that didn't come through [`boot`] (the app embedding one when it
/// can't start its helper) mustn't open databases a trial has changed or a restore
/// is part way through. [`boot`]'s own starts never get here in those states, apart
/// from the trial itself.
pub(crate) fn refuse_mid_update(data_dir: &Path) -> Result<(), crate::EngineError> {
    use transaction::Phase;
    let mid_update = transaction::read(data_dir)
        .is_some_and(|update| matches!(update.phase, Phase::Trial | Phase::Restoring));
    if mid_update && !transaction::in_trial() {
        return Err(crate::EngineError::Other(
            "Zeron is finishing an update. Try again in a moment.".into(),
        ));
    }
    Ok(())
}

/// Give up on this trial: the old version takes it from here.
fn fail(data_dir: &Path, why: &str) -> ! {
    tracing::error!(%why, "update trial failed");
    transaction::note_trial_failure(data_dir, why);
    std::process::exit(TRIAL_FAILED_EXIT)
}

/// Answering on the port with the data calls open, through the same front door
/// clients use.
async fn answers_on_its_port(port: u16, data_dir: &Path) -> Result<(), String> {
    let client = zeron_rpc::connect_ipc(port, Some(data_dir))
        .await
        .map_err(|err| format!("it isn't answering on its port ({err})"))?;
    for method in [zeron_rpc::methods::ENGINE_INFO, zeron_rpc::methods::ENGINE_READY] {
        client
            .call(method, serde_json::json!({}))
            .await
            .map_err(|err| format!("it isn't answering requests ({err})"))?;
    }
    Ok(())
}

/// Whether the servers answer at all. If they don't, the network is the problem
/// and a new version can't be blamed for not reaching them.
async fn edge_answers(edge_url: &str) -> bool {
    let url = format!("{}/releases/latest.txt", edge_url.trim_end_matches('/'));
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    else {
        return false;
    };
    client.get(url).send().await.is_ok_and(|reply| reply.status().is_success())
}

struct Proof {
    workspace: crate::workspace_host::WorkspaceHost,
    auth: crate::Auth,
    scope: WorkspaceScope,
    port: u16,
    data_dir: PathBuf,
    edge_url: String,
}

impl Proof {
    /// Its databases open and read (migrations included: assembly ran them), it
    /// serves clients, and a signed-in device is back on Zeron's servers, so
    /// other devices can reach it. Sessions wait until after this.
    async fn run(&self) -> Result<(), String> {
        self.workspace
            .read_chats()
            .map_err(|err| format!("it couldn't read its data ({err})"))?;
        answers_on_its_port(self.port, &self.data_dir).await?;
        if self.scope != WorkspaceScope::Synced {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + EDGE_PROOF;
        loop {
            if !self.auth.state().is_signed_in()
                || self.workspace.sync_status().is_some_and(|room| room.connected)
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if edge_answers(&self.edge_url).await {
            return Err("it couldn't connect to Zeron's servers".into());
        }
        tracing::warn!("no network during the update trial; not holding that against it");
        Ok(())
    }
}

/// On a new version's trial, once it's assembled and serving: prove it's ready,
/// commit, and only then start what can't be undone. Does nothing otherwise.
pub(crate) fn prove_and_commit(core: &EngineCore, config: &EngineConfig) {
    let Some(trial) = transaction::current_trial() else {
        return;
    };
    let proof = Proof {
        workspace: core.workspace.clone(),
        auth: core.auth(),
        scope: core.workspace_scope(),
        port: config.ipc_port,
        data_dir: config.data_dir.clone(),
        edge_url: config.edge_url.clone(),
    };
    let doc_host = core.doc_host.clone();
    let sessions = core.sessions.clone();
    let updater = core.updater();
    tokio::spawn(async move {
        tracing::info!(id = %trial.id, version = %trial.target, left = ?trial.remaining(), "proving this update is ready");
        if let Err(why) = proof.run().await {
            fail(&proof.data_dir, &format!("The new version started, but {why}."));
        }
        tokio::time::sleep(SETTLE).await;
        if let Err(err) = transaction::commit_trial() {
            tracing::error!(error = %format!("{err:#}"), "couldn't commit the update");
            fail(&proof.data_dir, "The new version couldn't be saved as the current one.");
        }
        tracing::info!(version = %trial.target, "update committed");
        if let Some(updater) = &updater {
            updater.refresh_update_report();
            // What's on offer has changed: this is the version it was comparing with.
            updater.check_now();
        }
        // What every other start does during assembly.
        match tokio::task::spawn_blocking(move || sessions.recover_stale()).await {
            Ok(Ok(0)) => {}
            Ok(Ok(recovered)) => tracing::info!(recovered, "stale sessions recovered after the update"),
            Ok(Err(err)) => tracing::error!(error = %err, "stale-session recovery failed"),
            Err(err) => tracing::error!(error = %err, "stale-session recovery panicked"),
        }
        doc_host.lift_quiesce();
        tokio::time::sleep(TIDY_AFTER).await;
        transaction::tidy_latest(&proof.data_dir);
    });
}

/// A trial with no profile to open: answering on its port is all it can prove.
pub(crate) fn commit_without_a_profile(config: &EngineConfig) {
    if !transaction::in_trial() {
        return;
    }
    let (port, data_dir) = (config.ipc_port, config.data_dir.clone());
    tokio::spawn(async move {
        let client = zeron_rpc::connect_ipc(port, Some(&data_dir)).await;
        let answered = match client {
            Ok(client) => client
                .call(zeron_rpc::methods::ENGINE_INFO, serde_json::json!({}))
                .await
                .map(drop)
                .map_err(|err| err.to_string()),
            Err(err) => Err(err.to_string()),
        };
        if let Err(why) = answered {
            fail(&data_dir, &format!("The new version started, but it isn't answering on its port ({why})."));
        }
        tokio::time::sleep(SETTLE).await;
        if let Err(err) = transaction::commit_trial() {
            tracing::error!(error = %format!("{err:#}"), "couldn't commit the update");
            fail(&data_dir, "The new version couldn't be saved as the current one.");
        }
        tokio::time::sleep(TIDY_AFTER).await;
        transaction::tidy_latest(&data_dir);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(dir: &Path, phase: &str) {
        std::fs::write(
            dir.join("update-transaction.json"),
            serde_json::json!({
                "id": "abc", "from": "1.0.0", "target": "2.0.0",
                "layout": {"versioned": {"app_root": "/nowhere"}},
                "phase": phase,
                "restart": "relaunch",
                "requestedAt": 1, "deadlineSecs": 60,
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn an_embedded_engine_keeps_out_of_databases_mid_update() {
        let dir = tempfile::tempdir().unwrap();
        assert!(refuse_mid_update(dir.path()).is_ok(), "no update");
        for (phase, refused) in [
            ("requested", false),
            ("trial", true),
            ("restoring", true),
            ("updated", false),
            ("rolledBack", false),
            ("failed", false),
        ] {
            record(dir.path(), phase);
            assert_eq!(refuse_mid_update(dir.path()).is_err(), refused, "{phase}");
        }
    }
}
