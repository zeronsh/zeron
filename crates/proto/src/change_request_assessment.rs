//! One assessment for board rows, inspection, and agent handoffs. Missing
//! metadata never implies that a pull request is ready or its checks passed.
use serde::{Deserialize, Serialize};

use crate::{
    ChangeRequestCheck, ChangeRequestDetail, ChangeRequestListItem, ChangeRequestMergeability,
    ChangeRequestReviewDecision, ChangeRequestState,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CiState {
    NoChecks,
    Pending,
    Passed,
    Skipped,
    Failed,
    #[default]
    #[serde(other)]
    Unknown,
}

impl CiState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "CI unavailable",
            Self::NoChecks => "No checks reported",
            Self::Pending => "CI pending",
            Self::Passed => "CI passed",
            Self::Skipped => "CI skipped",
            Self::Failed => "CI failing",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChangeRequestCi {
    pub state: CiState,
    pub total_count: u64,
}

pub fn valid_head_oid(head: &str) -> bool {
    head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn check_status(check: &ChangeRequestCheck) -> String {
    [&check.conclusion, &check.state, &check.status]
        .into_iter()
        .find(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_default()
}

pub fn check_failed(status: &str) -> bool {
    matches!(
        status,
        "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "CANCELLED" | "STARTUP_FAILURE"
    )
}

impl ChangeRequestCi {
    pub fn from_checks(checks: &[ChangeRequestCheck]) -> Self {
        let statuses: Vec<_> = checks.iter().map(check_status).collect();
        let state = if checks.is_empty() {
            CiState::NoChecks
        } else if statuses.iter().any(|value| check_failed(value)) {
            CiState::Failed
        } else if statuses
            .iter()
            .any(|value| !matches!(value.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED"))
        {
            CiState::Pending
        } else if statuses
            .iter()
            .all(|value| matches!(value.as_str(), "NEUTRAL" | "SKIPPED"))
        {
            CiState::Skipped
        } else {
            CiState::Passed
        };
        Self {
            state,
            total_count: checks.len() as u64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    FailingCi,
    ChangesRequested,
    MergeConflicts,
}

impl Blocker {
    pub fn label(self) -> &'static str {
        match self {
            Self::FailingCi => "CI failing",
            Self::ChangesRequested => "Changes requested",
            Self::MergeConflicts => "Merge conflicts",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingInformation {
    HeadCommit,
    Ci,
    Mergeability,
    ReviewDecision,
    ViewerRelationship,
    CheckDetails,
}

impl MissingInformation {
    pub fn label(self) -> &'static str {
        match self {
            Self::HeadCommit => "Current commit unavailable",
            Self::Ci => "CI unavailable",
            Self::Mergeability => "Mergeability unknown",
            Self::ReviewDecision => "No review decision reported",
            Self::ViewerRelationship => "Your relationship to this PR is unavailable",
            Self::CheckDetails => "Some check details are unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestedAction {
    FixChecks,
    AddressFeedback,
    ResolveConflicts,
    Review,
    RefreshMetadata,
}

impl SuggestedAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::FixChecks => "Fix failing checks",
            Self::AddressFeedback => "Address requested changes",
            Self::ResolveConflicts => "Resolve merge conflicts",
            Self::Review => "Review pull request",
            Self::RefreshMetadata => "Refresh metadata",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionReason {
    ReviewRequested,
    FeedbackOnAuthored,
    FailingCi,
}

impl AttentionReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReviewRequested => "Your review requested",
            Self::FeedbackOnAuthored => "Your PR needs changes",
            Self::FailingCi => "Your CI needs attention",
        }
    }
}

pub struct Assessment {
    pub blockers: Vec<Blocker>,
    pub missing: Vec<MissingInformation>,
    pub actions: Vec<SuggestedAction>,
    pub attention: Vec<AttentionReason>,
    pub ci: ChangeRequestCi,
}

/// Shared facts, whether supplied by a batch summary or opened PR detail.
pub struct Facts<'a> {
    pub head: &'a str,
    pub ci: ChangeRequestCi,
    pub open: bool,
    pub draft: bool,
    pub mergeability: ChangeRequestMergeability,
    pub review: ChangeRequestReviewDecision,
    pub authored: Option<bool>,
    pub requested: Option<bool>,
    /// Rows intentionally have summaries only; detail may report a partial check list.
    pub check_details_missing: bool,
}

impl<'a> Facts<'a> {
    pub fn from_item(item: &'a ChangeRequestListItem) -> Self {
        Self {
            head: &item.head_ref_oid,
            ci: item.ci,
            open: item.state == ChangeRequestState::Open,
            draft: item.is_draft,
            mergeability: item.mergeability,
            review: item.review_decision,
            authored: item.viewer_did_author,
            requested: item.viewer_review_requested,
            check_details_missing: false,
        }
    }

    pub fn from_detail(detail: &'a ChangeRequestDetail) -> Self {
        Self {
            head: &detail.head_ref_oid,
            ci: if detail.ci.state == CiState::Unknown && !detail.status_check_rollup.is_empty() {
                ChangeRequestCi::from_checks(&detail.status_check_rollup)
            } else {
                detail.ci
            },
            open: !matches!(
                detail.state.to_ascii_uppercase().as_str(),
                "CLOSED" | "MERGED"
            ),
            draft: detail.is_draft,
            mergeability: match detail.mergeable.to_ascii_uppercase().as_str() {
                "MERGEABLE" => ChangeRequestMergeability::Mergeable,
                "CONFLICTING" => ChangeRequestMergeability::Conflicting,
                _ => ChangeRequestMergeability::Unknown,
            },
            review: match detail.review_decision.to_ascii_uppercase().as_str() {
                "APPROVED" => ChangeRequestReviewDecision::Approved,
                "CHANGES_REQUESTED" => ChangeRequestReviewDecision::ChangesRequested,
                "REVIEW_REQUIRED" => ChangeRequestReviewDecision::ReviewRequired,
                _ => ChangeRequestReviewDecision::Unknown,
            },
            authored: detail.viewer_did_author,
            requested: detail.viewer_review_requested,
            check_details_missing: detail.ci.total_count > detail.status_check_rollup.len() as u64
                || (detail.ci.state == CiState::Failed
                    && !detail
                        .status_check_rollup
                        .iter()
                        .any(|check| check_failed(&check_status(check)))),
        }
    }

    pub fn assess(self) -> Assessment {
        let mut result = Assessment {
            blockers: Vec::new(),
            missing: Vec::new(),
            actions: Vec::new(),
            attention: Vec::new(),
            ci: self.ci,
        };
        if !valid_head_oid(self.head) {
            result.missing.push(MissingInformation::HeadCommit);
        }
        if self.ci.state == CiState::Unknown {
            result.missing.push(MissingInformation::Ci);
        }
        if self.mergeability == ChangeRequestMergeability::Unknown {
            result.missing.push(MissingInformation::Mergeability);
        }
        if self.review == ChangeRequestReviewDecision::Unknown {
            result.missing.push(MissingInformation::ReviewDecision);
        }
        if self.authored.is_none() || self.requested.is_none() {
            result.missing.push(MissingInformation::ViewerRelationship);
        }
        if self.check_details_missing {
            result.missing.push(MissingInformation::CheckDetails);
        }
        if self.open {
            if self.ci.state == CiState::Failed {
                result.blockers.push(Blocker::FailingCi);
                result.actions.push(SuggestedAction::FixChecks);
                if self.authored == Some(true) || self.requested == Some(true) {
                    result.attention.push(AttentionReason::FailingCi);
                }
            }
            if self.review == ChangeRequestReviewDecision::ChangesRequested {
                result.blockers.push(Blocker::ChangesRequested);
                result.actions.push(SuggestedAction::AddressFeedback);
                if self.authored == Some(true) {
                    result.attention.push(AttentionReason::FeedbackOnAuthored);
                }
            }
            if self.mergeability == ChangeRequestMergeability::Conflicting {
                result.blockers.push(Blocker::MergeConflicts);
                result.actions.push(SuggestedAction::ResolveConflicts);
            }
            if self.requested == Some(true) && !self.draft {
                result.attention.insert(0, AttentionReason::ReviewRequested);
            }
        }
        if !result.missing.is_empty() {
            result.actions.push(SuggestedAction::RefreshMetadata);
        }
        if self.open && !self.draft {
            result.actions.push(SuggestedAction::Review);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assessment_keeps_unknown_separate_and_targets_personal_actions() {
        let head = "a".repeat(40);
        let facts = |ci, review, authored, requested| Facts {
            head: &head,
            ci: ChangeRequestCi {
                state: ci,
                total_count: 3,
            },
            open: true,
            draft: false,
            mergeability: ChangeRequestMergeability::Mergeable,
            review,
            authored,
            requested,
            check_details_missing: false,
        };
        let unknown = facts(
            CiState::Unknown,
            ChangeRequestReviewDecision::Unknown,
            None,
            None,
        )
        .assess();
        assert!(unknown.blockers.is_empty());
        assert!(unknown.attention.is_empty());
        assert!(unknown.missing.contains(&MissingInformation::Ci));
        assert!(
            unknown
                .missing
                .contains(&MissingInformation::ViewerRelationship)
        );
        assert!(unknown.actions.contains(&SuggestedAction::RefreshMetadata));
        let mine = facts(
            CiState::Failed,
            ChangeRequestReviewDecision::ChangesRequested,
            Some(true),
            Some(false),
        )
        .assess();
        assert_eq!(
            mine.blockers,
            [Blocker::FailingCi, Blocker::ChangesRequested]
        );
        assert_eq!(
            mine.attention,
            [
                AttentionReason::FailingCi,
                AttentionReason::FeedbackOnAuthored
            ]
        );
        assert_eq!(
            mine.actions,
            [
                SuggestedAction::FixChecks,
                SuggestedAction::AddressFeedback,
                SuggestedAction::Review
            ]
        );
        let theirs = facts(
            CiState::Failed,
            ChangeRequestReviewDecision::ChangesRequested,
            Some(false),
            Some(false),
        )
        .assess();
        assert_eq!(theirs.blockers, mine.blockers);
        assert!(theirs.attention.is_empty());
        let requested = facts(
            CiState::Passed,
            ChangeRequestReviewDecision::ReviewRequired,
            Some(false),
            Some(true),
        )
        .assess();
        assert_eq!(requested.attention, [AttentionReason::ReviewRequested]);
        let mut closed = facts(
            CiState::Failed,
            ChangeRequestReviewDecision::ChangesRequested,
            Some(true),
            Some(true),
        );
        closed.open = false;
        assert!(closed.assess().actions.is_empty());
        let mut draft = facts(
            CiState::NoChecks,
            ChangeRequestReviewDecision::ReviewRequired,
            Some(false),
            Some(true),
        );
        draft.draft = true;
        assert!(draft.assess().attention.is_empty());
        // A detail and row with the same known facts produce the same rules.
        let detail = ChangeRequestDetail {
            head_ref_oid: head,
            mergeable: "MERGEABLE".into(),
            review_decision: "CHANGES_REQUESTED".into(),
            viewer_did_author: Some(true),
            viewer_review_requested: Some(false),
            ci: mine.ci,
            status_check_rollup: ["FAILURE", "SUCCESS", "SKIPPED"]
                .into_iter()
                .map(|conclusion| ChangeRequestCheck {
                    name: "build".into(),
                    conclusion: conclusion.into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let inspected = Facts::from_detail(&detail).assess();
        assert_eq!(inspected.blockers, mine.blockers);
        assert_eq!(inspected.actions, mine.actions);
        assert_eq!(inspected.attention, mine.attention);
        let mut old = serde_json::to_value(&detail).unwrap();
        for field in [
            "headRefOid",
            "ci",
            "viewerDidAuthor",
            "viewerReviewRequested",
            "statusCheckRollup",
        ] {
            old.as_object_mut().unwrap().remove(field);
        }
        let old: ChangeRequestDetail = serde_json::from_value(old).unwrap();
        let assessment = Facts::from_detail(&old).assess();
        assert!(assessment.missing.contains(&MissingInformation::HeadCommit));
        assert_eq!(assessment.ci.state, CiState::Unknown);
        assert_eq!(
            serde_json::from_str::<CiState>("\"futureState\"").unwrap(),
            CiState::Unknown
        );
    }
}
