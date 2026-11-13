//! Release channels: `stable` (everyone) and `beta` (Zeron.app testers).
//!
//! The channel a Mac follows is kept in the data dir, `update-channel.json`,
//! where every updater reads it on each check: the app's own (whichever
//! interface is installed) and the engine's report. It is a file, not the
//! environment, so it survives the app replacing itself and an engine never
//! inherits a channel from whoever started it.
//!
//! Only macOS app installs follow it. Managed installs (headless daemons, the
//! Linux desktop), Windows and source builds are always on stable: beta
//! publishes the macOS app alone.
//!
//!   stable  `{edge}/releases/manifest.json` (and `latest.txt`), unchanged
//!   beta    `{edge}/releases/beta/manifest.json`
//!
//! Opting in: a "Get beta updates" choice in the app (`select`), or a beta
//! download whose first launch claims the channel (`claim_for_beta_build`).
//! A build that claimed it once never claims it again, so choosing Stable in
//! a beta build sticks until the app is replaced.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The file in the data dir that holds the channel.
pub const FILE: &str = "update-channel.json";

/// The oldest macOS a beta build runs on: 13.0, the floor of Ghostty, which
/// draws the zgx interface's terminals (its build targets macOS 13; zgx's own
/// newer-macOS calls fall back). The opt-in refuses older systems, and so
/// does the updater when a manifest says `minimumMacOS`.
pub const BETA_MINIMUM_MACOS: &str = "13.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "stable" => Some(Self::Stable),
            "beta" => Some(Self::Beta),
            _ => None,
        }
    }

    /// The channel a build of `version` belongs to: a `-beta.N` prerelease
    /// is beta's, anything else stable's.
    pub fn of_version(version: &str) -> Self {
        match version.trim().split_once('-') {
            Some((_, pre)) if pre.to_ascii_lowercase().starts_with("beta") => Self::Beta,
            _ => Self::Stable,
        }
    }

    /// The release feed's base for this channel, under `stable` (the edge's
    /// `/releases` or `ZERON_RELEASES_URL`).
    pub fn base(self, stable: &str) -> String {
        let stable = stable.trim_end_matches('/');
        match self {
            Self::Stable => stable.to_owned(),
            Self::Beta => format!("{stable}/beta"),
        }
    }
}

/// `update-channel.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRecord {
    #[serde(default)]
    pub channel: Channel,
    /// The beta build whose first launch claimed the channel, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
}

/// The record in `data_dir` (stable when it's missing or unreadable).
pub fn read(data_dir: &Path) -> ChannelRecord {
    std::fs::read(data_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Write the record, atomically (temp file + rename).
pub fn write(data_dir: &Path, record: &ChannelRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(FILE);
    let temp = data_dir.join(format!(".{FILE}.{}", std::process::id()));
    let json = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// The user's choice ("Get beta updates", About's switch). Keeps the claim.
pub fn select(data_dir: &Path, channel: Channel) -> std::io::Result<()> {
    let mut record = read(data_dir);
    record.channel = channel;
    write(data_dir, &record)
}

/// A beta build's first launch: claim the beta channel unless this build
/// claimed it before (and the user may have chosen Stable since). Whether it
/// changed anything.
pub fn claim_for_beta_build(data_dir: &Path, build: &str) -> std::io::Result<bool> {
    let record = read(data_dir);
    if record.claimed_by.as_deref() == Some(build) {
        return Ok(false);
    }
    write(
        data_dir,
        &ChannelRecord {
            channel: Channel::Beta,
            claimed_by: Some(build.to_owned()),
        },
    )?;
    Ok(true)
}

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where this process reads the channel from: the data dir, set once at
/// startup by the app and by the engine. Unset, it's stable.
pub fn set_data_dir(dir: &Path) {
    let _ = DATA_DIR.set(dir.to_path_buf());
}

/// The data dir [`set_data_dir`] named, if any.
pub fn data_dir() -> Option<&'static Path> {
    DATA_DIR.get().map(PathBuf::as_path)
}

/// The channel this process's updater follows: the record's on a macOS app
/// install, stable everywhere else.
pub fn selected() -> Channel {
    let crate::InstallKind::MacApp { bundle } = crate::detect_install() else {
        return Channel::Stable;
    };
    choice_for(
        data_dir().and_then(recorded),
        crate::bundle_app_version(&bundle).as_deref(),
    )
}

/// What's recorded, when there's a record to read.
fn recorded(dir: &Path) -> Option<ChannelRecord> {
    serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
}

/// A choice someone made stands. With none on record (the app hasn't run since it
/// was installed, say) an app stays on the channel it was built for: an update
/// asked from another device must not move a beta app off beta by default.
fn choice_for(record: Option<ChannelRecord>, app_version: Option<&str>) -> Channel {
    match record {
        Some(record) => record.channel,
        None => app_version.map(Channel::of_version).unwrap_or_default(),
    }
}

/// This Mac's macOS version (`sw_vers -productVersion`), once.
pub fn macos_version() -> Option<&'static str> {
    static VERSION: OnceLock<Option<String>> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            if std::env::consts::OS != "macos" {
                return None;
            }
            let out = std::process::Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            let version = String::from_utf8(out.stdout).ok()?.trim().to_owned();
            (out.status.success() && !version.is_empty()).then_some(version)
        })
        .as_deref()
}

/// Whether `system` is at least `minimum` (dotted numbers; a version that
/// doesn't parse is not).
pub fn macos_at_least(system: &str, minimum: &str) -> bool {
    let parts = |v: &str| -> Option<Vec<u64>> { v.trim().split('.').map(|p| p.parse().ok()).collect() };
    match (parts(system), parts(minimum)) {
        (Some(mut have), Some(mut need)) => {
            let len = have.len().max(need.len());
            have.resize(len, 0);
            need.resize(len, 0);
            have >= need
        }
        _ => false,
    }
}

/// Why this Mac can't take beta updates, in words for the person: not a
/// macOS app install, or macOS older than [`BETA_MINIMUM_MACOS`].
pub fn beta_unavailable() -> Option<String> {
    if !matches!(crate::detect_install(), crate::InstallKind::MacApp { .. }) {
        return Some("Beta updates are for the Zeron app on a Mac.".into());
    }
    match macos_version() {
        Some(version) if macos_at_least(version, BETA_MINIMUM_MACOS) => None,
        Some(version) => Some(format!(
            "Zeron beta needs macOS {BETA_MINIMUM_MACOS} or later. This Mac has macOS {version}."
        )),
        None => Some(format!("Zeron beta needs macOS {BETA_MINIMUM_MACOS} or later.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_s_channel_follows_its_version() {
        assert_eq!(Channel::of_version("0.2.136-beta.1"), Channel::Beta);
        assert_eq!(Channel::of_version("0.2.136"), Channel::Stable);
        assert_eq!(Channel::of_version("0.2.136-rc.1"), Channel::Stable);
    }

    #[test]
    fn beta_reads_its_own_folder() {
        assert_eq!(Channel::Stable.base("https://edge.zeron.sh/releases/"), "https://edge.zeron.sh/releases");
        assert_eq!(Channel::Beta.base("https://edge.zeron.sh/releases"), "https://edge.zeron.sh/releases/beta");
    }

    #[test]
    fn a_choice_stands_and_without_one_an_app_stays_on_its_own_channel() {
        let chose = |channel| Some(ChannelRecord { channel, claimed_by: None });
        // Someone turned beta off at that Mac: their choice, on a beta build.
        assert_eq!(choice_for(chose(Channel::Stable), Some("0.2.136-beta.1")), Channel::Stable);
        assert_eq!(choice_for(chose(Channel::Beta), Some("0.2.136")), Channel::Beta);
        assert_eq!(choice_for(None, Some("0.2.136-beta.1")), Channel::Beta);
        assert_eq!(choice_for(None, Some("0.2.136")), Channel::Stable);
        assert_eq!(choice_for(None, None), Channel::Stable);
    }

    #[test]
    fn the_record_defaults_to_stable_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()), ChannelRecord::default());
        std::fs::write(dir.path().join(FILE), b"not json").unwrap();
        assert_eq!(read(dir.path()).channel, Channel::Stable);
        select(dir.path(), Channel::Beta).unwrap();
        assert_eq!(read(dir.path()).channel, Channel::Beta);
    }

    #[test]
    fn a_beta_build_claims_once_and_respects_a_later_choice() {
        let dir = tempfile::tempdir().unwrap();
        assert!(claim_for_beta_build(dir.path(), "0.2.136-beta.1").unwrap());
        assert_eq!(read(dir.path()).channel, Channel::Beta);
        // The person picks Stable in that build: its next launch leaves it.
        select(dir.path(), Channel::Stable).unwrap();
        assert!(!claim_for_beta_build(dir.path(), "0.2.136-beta.1").unwrap());
        assert_eq!(read(dir.path()).channel, Channel::Stable);
        // A beta downloaded afresh claims again.
        assert!(claim_for_beta_build(dir.path(), "0.2.136-beta.2").unwrap());
        assert_eq!(read(dir.path()).channel, Channel::Beta);
    }

    #[test]
    fn macos_versions_compare_numerically() {
        assert!(macos_at_least("14.0", "14.0"));
        assert!(macos_at_least("26.1", "14.0"));
        assert!(macos_at_least("14", "14.0"));
        assert!(!macos_at_least("13.6.1", "14.0"));
        assert!(!macos_at_least("garbage", "14.0"));
    }
}
