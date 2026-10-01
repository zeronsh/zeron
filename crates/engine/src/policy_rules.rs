//! Standing permission rules (docs/plans/2026-09-30-agent-mobility-and-policy.md,
//! Part 4): the device user's rules in `{data_dir}/policy-rules.json` and a
//! project's in `<workspace>/.zeron/policy.json`, both
//! `{"rules":[{kind?, pattern, effect}]}`.
//!
//! The host merges them into `RunRequest.policy.rules` at dispatch — project
//! rules first, then the user's, then whatever the request already carried —
//! and records the rule behind every "Always allow" answer in the user file.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use zeron_proto::{PolicyRule, RuleEffect};

pub const USER_RULES_FILE: &str = "policy-rules.json";
pub const PROJECT_RULES_FILE: &str = ".zeron/policy.json";
/// The user file stays small: past this, the oldest remembered "allow"
/// rules make room for new ones (hand-written deny/ask rules are kept).
pub const MAX_USER_RULES: usize = 500;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct RulesFile {
    rules: Vec<PolicyRule>,
}

/// Rules are read from disk on every dispatch (the files are tiny), so a
/// hand edit applies to the next run without a restart.
#[derive(Debug, Default)]
pub struct PolicyRules {
    /// `None` (bare tests) = no user rules and nothing persisted.
    user_path: Option<PathBuf>,
    /// Serializes read-modify-write of the user file.
    write: Mutex<()>,
}

impl PolicyRules {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            user_path: Some(data_dir.join(USER_RULES_FILE)),
            write: Mutex::new(()),
        }
    }

    pub fn user_rules(&self) -> Vec<PolicyRule> {
        self.user_path
            .as_deref()
            .map(read_rules)
            .unwrap_or_default()
    }

    /// The project's checked-in rules for a run working in `workspace`.
    pub fn project_rules(workspace: &Path) -> Vec<PolicyRule> {
        read_rules(&workspace.join(PROJECT_RULES_FILE))
    }

    /// `own` with the project's and then the user's rules in front of it,
    /// first occurrence kept (merging an already-merged list is a no-op).
    pub fn merged(&self, workspace: &Path, own: &[PolicyRule]) -> Vec<PolicyRule> {
        let mut out: Vec<PolicyRule> = Vec::new();
        for rule in Self::project_rules(workspace)
            .into_iter()
            .chain(self.user_rules())
            .chain(own.iter().cloned())
        {
            if !out.contains(&rule) {
                out.push(rule);
            }
        }
        out
    }

    /// Keep `rule` in the user file (an "Always allow" answer). Returns
    /// whether the file changed; a rule already present is not added twice.
    pub fn remember(&self, rule: PolicyRule) -> std::io::Result<bool> {
        let Some(path) = self.user_path.as_deref() else {
            return Ok(false);
        };
        let _guard = self.write.lock().unwrap_or_else(PoisonError::into_inner);
        let mut file = RulesFile {
            rules: read_rules(path),
        };
        if file.rules.contains(&rule) {
            return Ok(false);
        }
        file.rules.push(rule);
        while file.rules.len() > MAX_USER_RULES {
            match file
                .rules
                .iter()
                .position(|r| r.effect == RuleEffect::Allow)
            {
                Some(oldest) => {
                    file.rules.remove(oldest);
                }
                None => break,
            }
        }
        write_rules(path, &file)?;
        Ok(true)
    }
}

fn read_rules(path: &Path) -> Vec<PolicyRule> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<RulesFile>(&text) {
        Ok(file) => file.rules,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "policy rules unreadable; ignoring");
            Vec::new()
        }
    }
}

fn write_rules(path: &Path, file: &RulesFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, json)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::ActionKind;

    fn rule(kind: Option<ActionKind>, pattern: &str, effect: RuleEffect) -> PolicyRule {
        PolicyRule {
            kind,
            pattern: pattern.into(),
            effect,
        }
    }

    #[test]
    fn project_rules_come_first_then_user_then_own_deduplicated() {
        let data = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join(".zeron")).unwrap();
        std::fs::write(
            ws.path().join(PROJECT_RULES_FILE),
            r#"{"rules":[{"kind":"edit","pattern":"*/deploy/*","effect":"deny"},{"pattern":"cargo test*","effect":"allow"}]}"#,
        )
        .unwrap();
        let rules = PolicyRules::new(data.path());
        rules
            .remember(rule(Some(ActionKind::Exec), "make lint", RuleEffect::Allow))
            .unwrap();
        // A user rule repeating a project rule is listed once, where the
        // project put it.
        rules
            .remember(rule(None, "cargo test*", RuleEffect::Allow))
            .unwrap();
        let own = vec![
            rule(Some(ActionKind::Exec), "npm test", RuleEffect::Ask),
            rule(Some(ActionKind::Exec), "make lint", RuleEffect::Allow),
        ];
        let merged = rules.merged(ws.path(), &own);
        let patterns: Vec<&str> = merged.iter().map(|r| r.pattern.as_str()).collect();
        assert_eq!(
            patterns,
            ["*/deploy/*", "cargo test*", "make lint", "npm test"]
        );
        assert_eq!(merged[0].kind, Some(ActionKind::Edit));
        assert_eq!(merged[0].effect, RuleEffect::Deny);
        // Idempotent: merging the merged list changes nothing.
        assert_eq!(rules.merged(ws.path(), &merged), merged);
    }

    #[test]
    fn remembering_is_deduplicated_bounded_and_survives_garbage() {
        let data = tempfile::tempdir().unwrap();
        let rules = PolicyRules::new(data.path());
        let allow = rule(Some(ActionKind::Exec), "cargo build", RuleEffect::Allow);
        assert!(rules.remember(allow.clone()).unwrap());
        assert!(!rules.remember(allow.clone()).unwrap());
        assert_eq!(rules.user_rules(), vec![allow.clone()]);

        let deny = rule(None, "rm *", RuleEffect::Deny);
        std::fs::write(
            data.path().join(USER_RULES_FILE),
            serde_json::to_string(&RulesFile {
                rules: vec![deny.clone(), allow.clone()],
            })
            .unwrap(),
        )
        .unwrap();
        for i in 0..MAX_USER_RULES {
            rules
                .remember(rule(
                    Some(ActionKind::Exec),
                    &format!("cmd {i}"),
                    RuleEffect::Allow,
                ))
                .unwrap();
        }
        let kept = rules.user_rules();
        assert_eq!(kept.len(), MAX_USER_RULES);
        assert_eq!(kept[0], deny, "hand-written deny rules are never evicted");
        assert!(!kept.contains(&allow), "the oldest allow made room");
        assert_eq!(
            kept.last().unwrap().pattern,
            format!("cmd {}", MAX_USER_RULES - 1)
        );

        std::fs::write(data.path().join(USER_RULES_FILE), "not json").unwrap();
        assert!(rules.user_rules().is_empty());
        assert!(PolicyRules::default().user_rules().is_empty());
        assert!(!PolicyRules::default().remember(allow).unwrap());
    }
}
