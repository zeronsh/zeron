//! Release channels: `stable` (everyone) and `beta` (Zeron.app testers).
//!
//! The channel an install follows is kept in the data dir,
//! `update-channel.json`, where every updater reads it on each check: the
//! app's own (whichever interface is installed) and the engine's report. It is
//! a file, not the environment, so it survives the app replacing itself and an
//! engine never inherits a channel from whoever started it.
//!
//! macOS app installs follow it. On Windows and the Linux desktop only a beta
//! build does (`InstallKind::follows_channels`), which is how a tester goes
//! back to Stable. Stable builds there, headless daemons and source builds are
//! always on stable.
//!
//!   stable  `{edge}/releases/manifest.json` (and `latest.txt`), unchanged
//!   beta    `{edge}/releases/beta/manifest.json`
//!
//! Opting in: a "Get beta updates" choice in the app (`select`), or a beta
//! download whose first launch claims the channel (`claim_for_beta_build`).
//! A build that claimed it once never claims it again, so choosing Stable in
//! a beta build sticks until the app is replaced.
//!
//! A claim lasts until a stable release reaches the install (the beta
//! channel carries every release): that stable build's first launch moves an
//! install that's on beta only by a claim to stable
//! (`release_claim_for_stable_build`). One whose person chose beta stays.
//! (A choice made in a beta build from before `chosen` was recorded reads as
//! a claim.)

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
    /// The channel is the person's choice (`select`), not a download's claim.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
    /// Stable because a stable release ended a claim, not by choice
    /// ([`release_claim_for_stable_build`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub released: bool,
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

/// The user's choice ("Get beta updates", About's switch). Keeps the claim,
/// so the build that made it doesn't claim again.
pub fn select(data_dir: &Path, channel: Channel) -> std::io::Result<()> {
    let mut record = read(data_dir);
    record.channel = channel;
    record.chosen = true;
    record.released = false;
    write(data_dir, &record)
}

/// A beta build's first launch: claim the beta channel unless this build
/// claimed it before (and the user may have chosen Stable since). Whether it
/// changed anything.
pub fn claim_for_beta_build(data_dir: &Path, build: &str) -> std::io::Result<bool> {
    let record = recorded(data_dir);
    if let Some(record) = &record {
        // Beta with no claim was chosen: only the switch ever wrote that,
        // the old app's included, before `chosen` was recorded. Kept, and
        // said so now.
        if record.channel == Channel::Beta && record.claimed_by.is_none() && !record.chosen {
            write(data_dir, &ChannelRecord { chosen: true, ..record.clone() })?;
            return Ok(false);
        }
        // (On beta by choice already: a claim would only make it look like one.)
        if record.claimed_by.as_deref() == Some(build) || (record.chosen && record.channel == Channel::Beta) {
            return Ok(false);
        }
    }
    write(
        data_dir,
        &ChannelRecord {
            channel: Channel::Beta,
            claimed_by: Some(build.to_owned()),
            ..ChannelRecord::default()
        },
    )?;
    Ok(true)
}

/// A stable build's first launch: an install on beta only because a beta
/// download claimed it moves to stable, now that a stable release has reached
/// it. One whose person chose beta stays. Whether it changed anything.
pub fn release_claim_for_stable_build(data_dir: &Path) -> std::io::Result<bool> {
    match recorded(data_dir) {
        Some(record) if is_claim_only(&record) => {
            write(data_dir, &ChannelRecord { released: true, ..ChannelRecord::default() })?;
            Ok(true)
        }
        _ => Ok(false),
    }
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

/// The channel this process's updater follows: the record's on an install
/// that follows channels, stable everywhere else.
pub fn selected() -> Channel {
    let install = crate::detect_install();
    if !install.follows_channels() {
        return Channel::Stable;
    }
    let app_version = match &install {
        crate::InstallKind::MacApp { bundle } => crate::bundle_app_version(bundle),
        _ => Some(crate::app_version().to_owned()),
    };
    choice_for(data_dir().and_then(recorded), app_version.as_deref())
}

/// Whether Stable is a choice on record: the only thing that takes a beta
/// build back to an older stable (crates/update `offer`). A record saying
/// stable is one, unless a stable release wrote it ending a claim.
pub fn stable_on_record() -> bool {
    data_dir().is_some_and(stable_on_record_in)
}

fn stable_on_record_in(dir: &Path) -> bool {
    recorded(dir).is_some_and(|record| record.channel == Channel::Stable && !record.released)
}

/// What's recorded, when there's a record to read.
fn recorded(dir: &Path) -> Option<ChannelRecord> {
    serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
}

/// A choice someone made stands. With none on record (the app hasn't run since it
/// was installed, say) an app stays on the channel it was built for: an update
/// asked from another device must not move a beta app off beta by default.
///
/// A download's claim ends where a stable build is installed, as
/// [`release_claim_for_stable_build`] records at its first launch: read so
/// before that too, so that an update asked from another device (the engine
/// reads this, never launching the app) keeps the same rule.
fn choice_for(record: Option<ChannelRecord>, app_version: Option<&str>) -> Channel {
    let stable_build = app_version.map(Channel::of_version) == Some(Channel::Stable);
    match record {
        Some(record) if is_claim_only(&record) && stable_build => Channel::Stable,
        Some(record) => record.channel,
        None => app_version.map(Channel::of_version).unwrap_or_default(),
    }
}

/// On beta only because a beta download claimed it.
fn is_claim_only(record: &ChannelRecord) -> bool {
    record.channel == Channel::Beta && record.claimed_by.is_some() && !record.chosen
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

/// Why this install can't take beta updates, in words for the person: it
/// doesn't follow channels, or it's a Mac on macOS older than
/// [`BETA_MINIMUM_MACOS`] (the floor is macOS's alone).
pub fn beta_unavailable() -> Option<String> {
    let install = crate::detect_install();
    if !install.follows_channels() {
        return Some("Beta updates are for the Zeron app on a Mac.".into());
    }
    if !matches!(install, crate::InstallKind::MacApp { .. }) {
        return None;
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
        let chose = |channel| Some(ChannelRecord { channel, chosen: true, ..ChannelRecord::default() });
        // Someone turned beta off at that Mac: their choice, on a beta build.
        assert_eq!(choice_for(chose(Channel::Stable), Some("0.2.136-beta.1")), Channel::Stable);
        assert_eq!(choice_for(chose(Channel::Beta), Some("0.2.136")), Channel::Beta);
        assert_eq!(choice_for(None, Some("0.2.136-beta.1")), Channel::Beta);
        assert_eq!(choice_for(None, Some("0.2.136")), Channel::Stable);
        assert_eq!(choice_for(None, None), Channel::Stable);
        // A download's claim holds on its beta build and ends on a stable one,
        // before that build's first launch records it.
        let claimed = || Some(ChannelRecord { channel: Channel::Beta, claimed_by: Some("0.2.139-beta.6".into()), ..ChannelRecord::default() });
        assert_eq!(choice_for(claimed(), Some("0.2.139-beta.6")), Channel::Beta);
        assert_eq!(choice_for(claimed(), Some("0.2.139")), Channel::Stable);
        let chosen = Some(ChannelRecord { channel: Channel::Beta, claimed_by: Some("0.2.139-beta.6".into()), chosen: true, ..ChannelRecord::default() });
        assert_eq!(choice_for(chosen, Some("0.2.139")), Channel::Beta);
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

    /// The website's downloads are betas: when a stable release reaches one,
    /// it moves to stable unless its person chose beta.
    #[test]
    fn a_stable_build_ends_a_claim_but_not_a_choice() {
        let dir = tempfile::tempdir().unwrap();
        // No record: nothing to do.
        assert!(!release_claim_for_stable_build(dir.path()).unwrap());
        // A beta download's claim.
        claim_for_beta_build(dir.path(), "0.2.139-beta.6").unwrap();
        assert!(release_claim_for_stable_build(dir.path()).unwrap());
        assert_eq!(read(dir.path()).channel, Channel::Stable);
        assert!(!stable_on_record_in(dir.path()), "a released claim isn't a choice of Stable");
        assert!(!release_claim_for_stable_build(dir.path()).unwrap());
        // Beta chosen in the app, on a stable build (the old app's records
        // have no claim, and no `chosen` either): the beta it installs keeps
        // it a choice.
        std::fs::write(dir.path().join(FILE), br#"{"channel":"beta"}"#).unwrap();
        assert!(!release_claim_for_stable_build(dir.path()).unwrap());
        assert!(!claim_for_beta_build(dir.path(), "0.2.139-beta.7").unwrap());
        assert!(read(dir.path()).chosen);
        assert!(!release_claim_for_stable_build(dir.path()).unwrap());
        assert_eq!(read(dir.path()).channel, Channel::Beta);
        // Beta chosen after a claim: it stays, through later betas' claims too.
        claim_for_beta_build(dir.path(), "0.2.139-beta.6").unwrap();
        select(dir.path(), Channel::Stable).unwrap();
        select(dir.path(), Channel::Beta).unwrap();
        assert!(!claim_for_beta_build(dir.path(), "0.2.140-beta.1").unwrap());
        assert!(!release_claim_for_stable_build(dir.path()).unwrap());
        assert_eq!(read(dir.path()).channel, Channel::Beta);
        // Stable chosen in a beta build: left as it is.
        claim_for_beta_build(dir.path(), "0.2.140-beta.2").unwrap();
        select(dir.path(), Channel::Stable).unwrap();
        assert!(!release_claim_for_stable_build(dir.path()).unwrap());
        assert_eq!(read(dir.path()).channel, Channel::Stable);
    }

    #[test]
    fn only_a_record_saying_stable_is_a_choice_of_it() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!stable_on_record_in(dir.path()), "no record");
        claim_for_beta_build(dir.path(), "0.2.139-beta.6").unwrap();
        assert!(!stable_on_record_in(dir.path()), "a claim");
        select(dir.path(), Channel::Stable).unwrap();
        assert!(stable_on_record_in(dir.path()), "turned off in a beta build");
        std::fs::write(dir.path().join(FILE), br#"{"channel":"stable"}"#).unwrap();
        assert!(stable_on_record_in(dir.path()), "the old app's choice");
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
