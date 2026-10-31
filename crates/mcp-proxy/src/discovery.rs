//! Client-independent server classification, shared by every proxy launcher.
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Discovery {
    NonChannel,
    Channel,
    Failed,
}
#[derive(Serialize, Deserialize)]
struct Evidence {
    identity: String,
    discovery: Discovery,
}
fn evidence_path(data: &Path, identity: &str) -> PathBuf {
    data.join("mcp-cache/class-v1")
        .join(format!("{identity}.json"))
}
pub fn discovery(data: &Path, identity: &str) -> io::Result<Option<Discovery>> {
    let raw = match std::fs::read(evidence_path(data, identity)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let evidence: Evidence = serde_json::from_slice(&raw)?;
    if evidence.identity != identity {
        return Err(io::Error::other("invalid launch identity"));
    }
    Ok(Some(evidence.discovery))
}
/// Run-local capability evidence for servers whose vendor status omits it.
/// Positive channel evidence is sticky until the runtime ends.
pub fn run_discovery(dir: &Path, server: &str) -> io::Result<Option<Discovery>> {
    let path = dir.join("capabilities").join(format!(
        "{}.json",
        crate::cache::hash(&serde_json::json!(server))
    ));
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let value: serde_json::Value = serde_json::from_slice(&raw)?;
    if value["server"] != server {
        return Err(io::Error::other("invalid run server identity"));
    }
    Ok(Some(serde_json::from_value(value["discovery"].clone())?))
}

pub fn record_run_discovery(config: &crate::Config, outcome: Discovery) -> io::Result<()> {
    let Some(dir) = &config.run_dir else {
        return Ok(());
    };
    let path = dir.join("capabilities").join(format!(
        "{}.json",
        crate::cache::hash(&serde_json::json!(config.server))
    ));
    std::fs::create_dir_all(path.parent().unwrap())?;
    let _lock = crate::cache::FileLock::new(&path.with_extension("lock"))?;
    let outcome = if run_discovery(dir, &config.server)? == Some(Discovery::Channel) {
        Discovery::Channel
    } else {
        outcome
    };
    if outcome == Discovery::Channel {
        crate::cache::atomic_write(&dir.join("channel-live"), b"channel")?;
    }
    crate::cache::atomic_write(
        &path,
        &serde_json::to_vec(&serde_json::json!({
            "server": config.server, "discovery": outcome
        }))?,
    )
}

pub fn record_discovery(config: &crate::Config, outcome: Discovery) -> io::Result<()> {
    let run_result = record_run_discovery(config, outcome);
    let Some(identity) = &config.launch_identity else {
        return run_result;
    };
    let path = evidence_path(&config.data_dir, identity);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let _lock = crate::cache::FileLock::new(&path.with_extension("lock"))?;
    // Positive channel evidence is never overwritten by a concurrent failed or
    // non-channel capture. Only a changed launch identity permits re-discovery.
    let outcome = if discovery(&config.data_dir, identity)? == Some(Discovery::Channel) {
        Discovery::Channel
    } else {
        outcome
    };
    crate::cache::atomic_write(
        &path,
        &serde_json::to_vec(&Evidence {
            identity: identity.clone(),
            discovery: outcome,
        })?,
    )?;
    run_result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hibernation_run_capabilities_are_scoped_and_channel_evidence_is_sticky() {
        let root = tempfile::tempdir().unwrap();
        let mut config = crate::Config::parse(
            ["--server", "fake-channel", "--", "node", "server.js"].map(str::to_owned),
            root.path().into(),
        )
        .unwrap();
        let run = root.path().join("run-one");
        config.run_dir = Some(run.clone());
        assert_eq!(run_discovery(&run, "fake-channel").unwrap(), None);
        record_run_discovery(&config, Discovery::NonChannel).unwrap();
        assert_eq!(
            run_discovery(&run, "fake-channel").unwrap(),
            Some(Discovery::NonChannel)
        );
        assert_eq!(
            run_discovery(&root.path().join("run-two"), "fake-channel").unwrap(),
            None
        );
        record_run_discovery(&config, Discovery::Channel).unwrap();
        record_run_discovery(&config, Discovery::NonChannel).unwrap();
        assert_eq!(
            run_discovery(&run, "fake-channel").unwrap(),
            Some(Discovery::Channel)
        );
        assert!(run.join("channel-live").exists());
    }
    #[cfg(unix)]
    #[test]
    fn channel_evidence_is_sticky_private_and_failed_or_corrupt_evidence_falls_back() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let mut config = crate::Config::parse(
            ["--server", "test", "--", "node", "srv.js"].map(str::to_owned),
            root.path().into(),
        )
        .unwrap();
        config.launch_identity = Some("digest".into());
        assert_eq!(discovery(root.path(), "digest").unwrap(), None);
        record_discovery(&config, Discovery::NonChannel).unwrap();
        assert_eq!(
            discovery(root.path(), "digest").unwrap(),
            Some(Discovery::NonChannel)
        );
        record_discovery(&config, Discovery::Failed).unwrap();
        assert_eq!(
            discovery(root.path(), "digest").unwrap(),
            Some(Discovery::Failed)
        );
        record_discovery(&config, Discovery::Channel).unwrap();
        record_discovery(&config, Discovery::NonChannel).unwrap();
        record_discovery(&config, Discovery::Failed).unwrap();
        assert_eq!(
            discovery(root.path(), "digest").unwrap(),
            Some(Discovery::Channel)
        );
        let path = evidence_path(root.path(), "digest");
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(path, "corrupt").unwrap();
        assert!(discovery(root.path(), "digest").is_err());
    }
}
