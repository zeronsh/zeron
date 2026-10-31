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
    #[serde(default)]
    cached_at: u64,
}
fn evidence_path(data: &Path, identity: &str) -> PathBuf {
    data.join("mcp-cache/class-v1")
        .join(format!("{identity}.json"))
}
pub fn discovery(data: &Path, identity: &str) -> io::Result<Option<Discovery>> {
    discovery_at(data, identity, crate::cache::now_ms())
}
fn discovery_at(data: &Path, identity: &str, now: u64) -> io::Result<Option<Discovery>> {
    let raw = match std::fs::read(evidence_path(data, identity)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let evidence: Evidence = serde_json::from_slice(&raw)?;
    if evidence.identity != identity {
        return Err(io::Error::other("invalid launch identity"));
    }
    if evidence.discovery == Discovery::Failed
        && !crate::cache::discovery_failure_active(evidence.cached_at, now)
    {
        return Ok(None);
    }
    Ok(Some(evidence.discovery))
}
pub fn record_discovery(config: &crate::Config, outcome: Discovery) -> io::Result<()> {
    record_discovery_at(config, outcome, crate::cache::now_ms())
}
fn record_discovery_at(config: &crate::Config, outcome: Discovery, now: u64) -> io::Result<()> {
    let Some(identity) = &config.launch_identity else {
        return Ok(());
    };
    let path = evidence_path(&config.data_dir, identity);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let _lock = crate::cache::FileLock::new(&path.with_extension("lock"))?;
    // Positive channel evidence is never overwritten by a concurrent failed or
    // non-channel capture. Only a changed launch identity permits re-discovery.
    let outcome = if discovery_at(&config.data_dir, identity, now)? == Some(Discovery::Channel) {
        Discovery::Channel
    } else {
        outcome
    };
    crate::cache::atomic_write(
        &path,
        &serde_json::to_vec(&Evidence {
            identity: identity.clone(),
            discovery: outcome,
            cached_at: now,
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_evidence_expires_at_24_hours_without_expiring_channel_evidence() {
        use crate::cache::DISCOVERY_RETRY_MS;
        let root = tempfile::tempdir().unwrap();
        let mut config = crate::Config::parse(
            ["--server", "test", "--", "node", "srv.js"].map(str::to_owned),
            root.path().into(),
        )
        .unwrap();
        config.launch_identity = Some("digest".into());
        let now = DISCOVERY_RETRY_MS * 2;
        record_discovery_at(&config, Discovery::Failed, now).unwrap();
        assert_eq!(
            discovery_at(root.path(), "digest", now - 1).unwrap(),
            Some(Discovery::Failed)
        );
        assert_eq!(
            discovery_at(root.path(), "digest", now + DISCOVERY_RETRY_MS - 1).unwrap(),
            Some(Discovery::Failed)
        );
        assert_eq!(
            discovery_at(root.path(), "digest", now + DISCOVERY_RETRY_MS).unwrap(),
            None
        );
        assert_eq!(
            discovery_at(root.path(), "digest", now + DISCOVERY_RETRY_MS + 1).unwrap(),
            None
        );
        // An old writer's failure has no timestamp; retry it on the next launch.
        std::fs::write(
            evidence_path(root.path(), "digest"),
            br#"{"identity":"digest","discovery":"failed"}"#,
        )
        .unwrap();
        assert_eq!(discovery_at(root.path(), "digest", now).unwrap(), None);
        for outcome in [Discovery::NonChannel, Discovery::Channel] {
            record_discovery_at(&config, outcome, now).unwrap();
            assert_eq!(
                discovery_at(root.path(), "digest", now + 10 * DISCOVERY_RETRY_MS).unwrap(),
                Some(outcome)
            );
        }
        record_discovery_at(&config, Discovery::Failed, now + 10 * DISCOVERY_RETRY_MS).unwrap();
        assert_eq!(
            discovery_at(root.path(), "digest", now + 11 * DISCOVERY_RETRY_MS).unwrap(),
            Some(Discovery::Channel)
        );
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
