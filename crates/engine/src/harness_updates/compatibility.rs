//! Agent CLI releases this zeron is known not to work with.
//!
//! A CLI release can break the protocol zeron speaks to it (a renamed
//! stream-json frame, a dropped flag). The policy lists those releases per
//! CLI, so an update never installs one, and an installed one says so with
//! the release to go back to. Sources, first match wins:
//!
//! 1. `{edge}/releases/harness-compatibility.json` — published on its own,
//!    so a release that broke this morning is held back today, without a
//!    zeron release. Fetched at most hourly; the last good copy is kept in
//!    `{data_dir}/harness-compatibility.json` for offline starts.
//! 2. The list built into this zeron (`compatibility.json`).
//!
//! ```json
//! { "policies": [ {
//!     "harness": "claude-code",
//!     "zeron": "<0.3.0",
//!     "recommended": "2.1.309",
//!     "ranges": [ { "range": ">=2.1.310, <2.1.312", "status": "broken",
//!                   "reason": "stream-json results lost their session id" } ]
//! } ] }
//! ```
//!
//! `zeron` (optional) scopes a policy to the zeron versions it was written
//! for, so a zeron that adapted is not held back by it. A range is a semver
//! requirement; `||` separates alternatives.

use serde::{Deserialize, Serialize};
use zeron_proto::{HarnessCompatibility, HarnessCompatibilityStatus, HarnessId};

const BUILT_IN: &str = include_str!("compatibility.json");

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Feed {
    #[serde(default)]
    pub policies: Vec<Policy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Policy {
    pub harness: HarnessId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zeron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<String>,
    #[serde(default)]
    pub ranges: Vec<Range>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Range {
    pub range: String,
    pub status: HarnessCompatibilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Feed {
    pub(super) fn built_in() -> Self {
        Self::parse(BUILT_IN).unwrap_or_default()
    }

    /// A feed whose every requirement parses; anything else is rejected
    /// whole (a half-understood policy must not let a broken release in).
    pub(super) fn parse(json: &str) -> Result<Self, String> {
        let feed: Feed = serde_json::from_str(json).map_err(|error| error.to_string())?;
        for policy in &feed.policies {
            if let Some(zeron) = &policy.zeron {
                requirement(zeron)?;
            }
            for range in &policy.ranges {
                requirement(&range.range)?;
            }
        }
        Ok(feed)
    }

    /// What the policy says about `harness` at `version`, for this zeron.
    /// `None`: nothing (supported, or not listed).
    pub(super) fn verdict(
        &self,
        harness: HarnessId,
        version: &str,
    ) -> Option<HarnessCompatibility> {
        self.verdict_for(harness, version, env!("CARGO_PKG_VERSION"))
    }

    fn verdict_for(
        &self,
        harness: HarnessId,
        version: &str,
        zeron: &str,
    ) -> Option<HarnessCompatibility> {
        let zeron = lenient(zeron)?;
        let policy = self.policies.iter().find(|policy| {
            policy.harness == harness
                && policy
                    .zeron
                    .as_deref()
                    .is_none_or(|range| matches(range, &zeron))
        })?;
        let parsed = lenient(version)?;
        let range = policy
            .ranges
            .iter()
            .find(|range| matches(&range.range, &parsed))?;
        if range.status == HarnessCompatibilityStatus::Supported {
            return None;
        }
        let what = match range.status {
            HarnessCompatibilityStatus::Broken => "doesn't work with this version of Zeron",
            _ => "isn't supported by this version of Zeron",
        };
        let mut message = format!("{version} {what}");
        if let Some(reason) = &range.reason {
            message.push_str(&format!(" ({reason})"));
        }
        if let Some(recommended) = &policy.recommended {
            message.push_str(&format!(". Use {recommended}"));
        }
        Some(HarnessCompatibility {
            status: range.status,
            version: version.to_string(),
            message,
            recommended_version: policy.recommended.clone(),
        })
    }
}

/// `a, b || c` → alternatives of comma-joined comparators.
fn requirement(text: &str) -> Result<Vec<semver::VersionReq>, String> {
    text.split("||")
        .map(|alternative| {
            semver::VersionReq::parse(alternative.trim())
                .map_err(|error| format!("bad version range {text:?}: {error}"))
        })
        .collect()
}

fn matches(text: &str, version: &semver::Version) -> bool {
    requirement(text).is_ok_and(|alternatives| alternatives.iter().any(|req| req.matches(version)))
}

/// A CLI's version as semver: `v` prefixes, missing components and vendor
/// suffixes (`2.1.309 (Claude Code)`, `1.4.0-3f2a1c`) are tolerated; only
/// the numbers count.
fn lenient(version: &str) -> Option<semver::Version> {
    let numbers: Vec<u64> = version
        .trim()
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .next()?
        .split('.')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    let at = |index: usize| numbers.get(index).copied().unwrap_or(0);
    (!numbers.is_empty()).then(|| semver::Version::new(at(0), at(1), at(2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed() -> Feed {
        Feed::parse(
            r#"{ "policies": [
                { "harness": "claude-code", "zeron": "<0.3.0", "recommended": "2.1.309",
                  "ranges": [
                    { "range": ">=2.1.310, <2.1.312", "status": "broken",
                      "reason": "results lost their session id" },
                    { "range": "<2.0.0 || >=3.0.0", "status": "unsupported" }
                  ] },
                { "harness": "codex", "ranges": [ { "range": ">=0.1.0", "status": "supported" } ] }
            ] }"#,
        )
        .unwrap()
    }

    #[test]
    fn listed_releases_are_held_back_with_the_way_out() {
        let feed = feed();
        let broken = feed
            .verdict_for(HarnessId::ClaudeCode, "2.1.311", "0.2.120")
            .unwrap();
        assert_eq!(broken.status, HarnessCompatibilityStatus::Broken);
        assert_eq!(broken.recommended_version.as_deref(), Some("2.1.309"));
        assert_eq!(
            broken.message,
            "2.1.311 doesn't work with this version of Zeron (results lost their session id). \
             Use 2.1.309"
        );
        assert_eq!(
            feed.verdict_for(HarnessId::ClaudeCode, "1.9.0", "0.2.120")
                .unwrap()
                .status,
            HarnessCompatibilityStatus::Unsupported
        );
        assert!(
            feed.verdict_for(HarnessId::ClaudeCode, "v3.0.1", "0.2.120")
                .is_some()
        );
        // Outside the ranges, supported, or unlisted: nothing to say.
        assert!(
            feed.verdict_for(HarnessId::ClaudeCode, "2.1.309", "0.2.120")
                .is_none()
        );
        assert!(
            feed.verdict_for(HarnessId::ClaudeCode, "2.1.312", "0.2.120")
                .is_none()
        );
        assert!(
            feed.verdict_for(HarnessId::Codex, "0.163.0", "0.2.120")
                .is_none()
        );
        assert!(
            feed.verdict_for(HarnessId::Grok, "1.0.0", "0.2.120")
                .is_none()
        );
        // A zeron that adapted is not held back.
        assert!(
            feed.verdict_for(HarnessId::ClaudeCode, "2.1.311", "0.3.0")
                .is_none()
        );
    }

    #[test]
    fn vendor_version_strings_are_read_leniently() {
        assert_eq!(
            lenient("2.1.309 (Claude Code)"),
            Some(semver::Version::new(2, 1, 309))
        );
        assert_eq!(lenient("v0.163"), Some(semver::Version::new(0, 163, 0)));
        assert_eq!(lenient("1.4.0-3f2a1c"), Some(semver::Version::new(1, 4, 0)));
        assert_eq!(lenient("nightly"), None);
    }

    #[test]
    fn a_feed_with_any_unreadable_range_is_rejected_whole() {
        assert!(
            Feed::parse(
                r#"{"policies":[{"harness":"claude-code","ranges":[{"range":"soon","status":"broken"}]}]}"#
            )
            .is_err()
        );
        assert!(Feed::parse(r#"{"policies":[{"harness":"not-a-cli","ranges":[]}]}"#).is_err());
        assert!(Feed::parse("{}").unwrap().policies.is_empty());
    }

    #[test]
    fn the_built_in_list_parses() {
        Feed::parse(BUILT_IN).expect("compatibility.json");
    }
}
