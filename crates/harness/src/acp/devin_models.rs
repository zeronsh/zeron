//! Devin's ACP session starts with a bundled catalog and refreshes it later.
//! `models list` waits for the account catalog before writing JSON, so use it
//! instead of racing `session/new` against `config_option_update` notifications.
//!
//! Since CLI 3000.11 the catalog's variant ids (`swe-2-medium`, …) are not
//! what the ACP session selects: it advertises ONE id per group of variants
//! and exposes effort as the `thought_level` option and fast mode as `speed`
//! (verified against 3000.11.3). The picker therefore lists one model per
//! group — a family plus any non-effort qualifier such as "1M" — with the
//! group's effort levels and a Fast option; a run selects whichever member the
//! session advertises and then sets effort and speed.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;

use zeron_proto::{Model, ModelOption, ModelOptionChoice, ReasoningLevel};

use crate::HarnessError;
use crate::jsonrpc::{Incoming, RpcClient};
use crate::process::{Command, Stdio};

/// A freshly discovered variant may also arrive after `session/new` in the
/// process that runs the prompt. Wait for that exact id; the generic ACP
/// family fallback could otherwise silently select a different GPT model.
pub(super) async fn wait_for_model(
    client: &RpcClient,
    incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
    session_id: &str,
    response: &mut serde_json::Value,
    model: &str,
    members: &[String],
) -> Result<String, HarnessError> {
    let wait = async {
        loop {
            let advertised = super::models_from_session(response, &[]);
            if let Some(id) = std::iter::once(model)
                .chain(members.iter().map(String::as_str))
                .find(|id| advertised.iter().any(|m| m.id == *id))
            {
                return Ok(id.to_owned());
            }
            match incoming.recv().await {
                Some(Incoming::Notification { method, params })
                    if method == "session/update"
                        && params.get("sessionId").and_then(serde_json::Value::as_str)
                            == Some(session_id)
                        && params["update"]["sessionUpdate"] == "config_option_update" =>
                {
                    if params["update"]["configOptions"].is_array() {
                        response["configOptions"] = params["update"]["configOptions"].clone();
                    }
                }
                Some(Incoming::Request { id, method, params }) => {
                    super::handle_server_request(client, id, &method, &params);
                }
                Some(_) => {}
                None => {
                    return Err(HarnessError::Protocol(
                        "Devin exited while refreshing models".into(),
                    ));
                }
            }
        }
    };
    tokio::time::timeout(super::DEFAULT_MODEL_DISCOVERY_TIMEOUT, wait)
        .await
        .map_err(|_| {
            HarnessError::Protocol(format!(
                "Devin did not advertise requested model {model} after refreshing"
            ))
        })?
}

/// The requested model resolved against the catalog.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Selection {
    /// Every variant id of the requested model's group; the session
    /// advertises exactly one of them.
    pub members: Vec<String>,
    /// Effort and fast mode baked into the requested id itself (an id saved
    /// before effort moved to its own option, e.g. `swe-2-medium`).
    pub effort: Option<ReasoningLevel>,
    pub fast: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Member {
    id: String,
    effort: Option<ReasoningLevel>,
    fast: bool,
}

#[derive(Default)]
pub(super) struct Catalog {
    // Only overlapping callers share a result. A later picker open always
    // probes again, including after errors, login changes, or model rollouts.
    latest: Mutex<Option<(Instant, Vec<Model>, Vec<Vec<Member>>)>>,
}

impl Catalog {
    pub(super) async fn refresh(
        &self,
        exe: &Path,
        timeout: Duration,
    ) -> Result<Vec<Model>, HarnessError> {
        let requested_at = Instant::now();
        let mut latest = self.latest.lock().await;
        if let Some((completed_at, models, _)) = &*latest
            && *completed_at >= requested_at
        {
            return Ok(models.clone());
        }
        let mut cmd = Command::new(exe);
        cmd.args(["models", "list", "--format", "json"]);
        crate::compose_child_path(&mut cmd, exe);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| HarnessError::Protocol("Devin model discovery timed out".into()))??;
        if !output.status.success() {
            return Err(HarnessError::Protocol(format!(
                "Devin models list failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let (models, groups) = parse_catalog(&output.stdout)?;
        *latest = Some((Instant::now(), models.clone(), groups));
        Ok(models)
    }

    /// Resolve `model` to its group, refreshing the catalog when this
    /// process has none yet. `None` when the catalog does not know the id.
    pub(super) async fn selection(
        &self,
        exe: &Path,
        timeout: Duration,
        model: &str,
    ) -> Option<Selection> {
        let known = self.latest.lock().await.is_some();
        if !known {
            self.refresh(exe, timeout).await.ok()?;
        }
        let latest = self.latest.lock().await;
        let (_, _, groups) = latest.as_ref()?;
        groups.iter().find_map(|group| {
            let hit = group.iter().find(|m| m.id == model)?;
            Some(Selection {
                members: group.iter().map(|m| m.id.clone()).collect(),
                effort: hit.effort,
                fast: hit.fast,
            })
        })
    }
}

#[derive(Deserialize)]
struct ModelList {
    families: Vec<Family>,
}

#[derive(Deserialize)]
struct Family {
    #[serde(default)]
    family_label: String,
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
struct Variant {
    model_uid: String,
    label: String,
    cost_summary: Option<String>,
}

/// Split a variant label into (qualifier, effort, fast) relative to its
/// family label: "GPT-6 Sol No Thinking Fast" → ("", Minimal, true),
/// "GLM-5.2 Max 1M" → ("1M", Max, false). Composite labels such as Fusion's
/// "(A Medium + B Medium)" are qualifiers as a whole.
fn parse_variant(family: &str, label: &str) -> (String, Option<ReasoningLevel>, bool) {
    let rest = label
        .strip_prefix(family)
        .filter(|r| r.is_empty() || r.starts_with(' '))
        .unwrap_or(label)
        .trim();
    if rest.starts_with('(') || rest.contains('+') {
        return (rest.to_owned(), None, false);
    }
    let mut effort = None;
    let mut fast = false;
    let mut qualifier = Vec::new();
    let mut words = rest.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let level = match word {
            // "No Thinking" is Devin's `none`; Zeron's lowest level stands in.
            "No" if words.peek() == Some(&"Thinking") => {
                words.next();
                Some(ReasoningLevel::Minimal)
            }
            "None" | "Minimal" => Some(ReasoningLevel::Minimal),
            "Low" => Some(ReasoningLevel::Low),
            "Medium" => Some(ReasoningLevel::Medium),
            "High" => Some(ReasoningLevel::High),
            "XHigh" | "X-High" => Some(ReasoningLevel::XHigh),
            "Max" => Some(ReasoningLevel::Max),
            _ => None,
        };
        match level {
            Some(level) if effort.is_none() => {
                effort = Some(level);
                if words.peek() == Some(&"Thinking") {
                    words.next();
                }
            }
            _ if word == "Fast" => fast = true,
            _ => qualifier.push(word),
        }
    }
    (qualifier.join(" "), effort, fast)
}

fn parse_catalog(bytes: &[u8]) -> Result<(Vec<Model>, Vec<Vec<Member>>), HarnessError> {
    let catalog: ModelList = serde_json::from_slice(bytes)
        .map_err(|error| HarnessError::Protocol(format!("invalid Devin model catalog: {error}")))?;
    let mut models: Vec<Model> = Vec::new();
    let mut groups: Vec<Vec<Member>> = Vec::new();
    for family in catalog.families {
        // (qualifier, label, description, members) in catalog order.
        let mut family_groups: Vec<(String, String, Option<String>, Vec<Member>)> = Vec::new();
        for variant in family.variants {
            if variant.model_uid.trim().is_empty() || variant.label.trim().is_empty() {
                return Err(HarnessError::Protocol(
                    "invalid empty Devin model id or label".into(),
                ));
            }
            if groups
                .iter()
                .chain(family_groups.iter().map(|g| &g.3))
                .flatten()
                .any(|m| m.id == variant.model_uid)
            {
                continue;
            }
            let (qualifier, effort, fast) = parse_variant(&family.family_label, &variant.label);
            let member = Member {
                id: variant.model_uid.clone(),
                effort,
                fast,
            };
            // A variant with neither effort nor speed is a model of its own;
            // so is one whose effort/speed pair its group already has.
            let group = family_groups.iter_mut().find(|g| {
                g.0 == qualifier
                    && (effort.is_some() || fast)
                    && g.3.iter().any(|m| m.effort.is_some() || m.fast)
                    && !g.3.iter().any(|m| m.effort == effort && m.fast == fast)
            });
            match group {
                Some(group) => group.3.push(member),
                None => {
                    let label = if effort.is_none() && !fast {
                        variant.label.clone()
                    } else if qualifier.is_empty() || family.family_label.is_empty() {
                        if family.family_label.is_empty() {
                            variant.label.clone()
                        } else {
                            family.family_label.clone()
                        }
                    } else {
                        format!("{} {qualifier}", family.family_label)
                    };
                    family_groups.push((qualifier, label, variant.cost_summary, vec![member]));
                }
            }
        }
        for (_, label, description, members) in family_groups {
            let mut reasoning_levels: Vec<ReasoningLevel> =
                members.iter().filter_map(|m| m.effort).collect();
            reasoning_levels.sort();
            reasoning_levels.dedup();
            let options = if members.iter().any(|m| m.fast) {
                vec![ModelOption {
                    id: "speed".into(),
                    label: "Speed".into(),
                    choices: vec![
                        ModelOptionChoice {
                            id: "standard".into(),
                            label: "Standard".into(),
                        },
                        ModelOptionChoice {
                            id: "fast".into(),
                            label: "Fast".into(),
                        },
                    ],
                    default_choice: "standard".into(),
                }]
            } else {
                Vec::new()
            };
            models.push(Model {
                id: members[0].id.clone(),
                label,
                description,
                reasoning_levels,
                options,
            });
            groups.push(members);
        }
    }
    if models.is_empty() {
        return Err(HarnessError::Protocol(
            "Devin returned an empty model catalog".into(),
        ));
    }
    Ok((models, groups))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shapes taken from `devin models list --format json` (CLI 3000.11.3).
    const LIVE_SHAPES: &str = r#"{"families":[
        {"family_uid":"swe-2","family_label":"SWE-2","aliases":["swe"],"variants":[
            {"model_uid":"swe-2-high","label":"SWE-2 High"},
            {"model_uid":"swe-2-medium","label":"SWE-2 Medium"},
            {"model_uid":"swe-2-max","label":"SWE-2 Max"}]},
        {"family_uid":"gpt-6-sol","family_label":"GPT-6 Sol","variants":[
            {"model_uid":"gpt-6-sol-medium","label":"GPT-6 Sol Medium Thinking","cost_summary":"$2 / 1M"},
            {"model_uid":"gpt-6-sol-none","label":"GPT-6 Sol No Thinking"},
            {"model_uid":"gpt-6-sol-high-priority","label":"GPT-6 Sol High Thinking Fast"}]},
        {"family_uid":"glm-5.2","family_label":"GLM-5.2","variants":[
            {"model_uid":"glm-5-2","label":"GLM-5.2 High"},
            {"model_uid":"glm-5-2-max","label":"GLM-5.2 Max"},
            {"model_uid":"glm-5-2-1m","label":"GLM-5.2 High 1M"},
            {"model_uid":"glm-5-2-max-1m","label":"GLM-5.2 Max 1M"}]},
        {"family_uid":"claude-opus-4.6","family_label":"Claude Opus 4.6","variants":[
            {"model_uid":"claude-opus-4-6","label":"Claude Opus 4.6"},
            {"model_uid":"claude-opus-4-6-thinking","label":"Claude Opus 4.6 Thinking"}]},
        {"family_uid":"fusion","family_label":"Fusion","variants":[
            {"model_uid":"fusion-a-medium-sidekick-b-medium","label":"Fusion (A Medium + B Medium)"},
            {"model_uid":"fusion-a-high-sidekick-b-medium","label":"Fusion (A High + B Medium)"}]},
        {"family_uid":"adaptive","family_label":"Adaptive","variants":[
            {"model_uid":"adaptive","label":"Adaptive"}]}
    ]}"#;

    #[test]
    fn groups_effort_and_speed_variants_into_one_picker_model() {
        let (models, groups) = parse_catalog(LIVE_SHAPES.as_bytes()).unwrap();
        let summary: Vec<_> = models
            .iter()
            .map(|m| {
                (
                    m.id.as_str(),
                    m.label.as_str(),
                    m.reasoning_levels.clone(),
                    m.options.len(),
                )
            })
            .collect();
        use ReasoningLevel::*;
        assert_eq!(
            summary,
            vec![
                ("swe-2-high", "SWE-2", vec![Medium, High, Max], 0),
                (
                    "gpt-6-sol-medium",
                    "GPT-6 Sol",
                    vec![Minimal, Medium, High],
                    1
                ),
                ("glm-5-2", "GLM-5.2", vec![High, Max], 0),
                ("glm-5-2-1m", "GLM-5.2 1M", vec![High, Max], 0),
                ("claude-opus-4-6", "Claude Opus 4.6", vec![], 0),
                (
                    "claude-opus-4-6-thinking",
                    "Claude Opus 4.6 Thinking",
                    vec![],
                    0
                ),
                (
                    "fusion-a-medium-sidekick-b-medium",
                    "Fusion (A Medium + B Medium)",
                    vec![],
                    0
                ),
                (
                    "fusion-a-high-sidekick-b-medium",
                    "Fusion (A High + B Medium)",
                    vec![],
                    0
                ),
                ("adaptive", "Adaptive", vec![], 0),
            ]
        );
        assert_eq!(models[1].options[0].id, "speed");
        assert_eq!(models[1].description.as_deref(), Some("$2 / 1M"));
        let sol = &groups[1];
        assert_eq!(
            sol.iter()
                .map(|m| (m.id.as_str(), m.effort, m.fast))
                .collect::<Vec<_>>(),
            vec![
                ("gpt-6-sol-medium", Some(Medium), false),
                ("gpt-6-sol-none", Some(Minimal), false),
                ("gpt-6-sol-high-priority", Some(High), true),
            ]
        );
    }

    #[tokio::test]
    async fn saved_variant_ids_resolve_to_their_group_with_effort_and_speed() {
        let catalog = Catalog::default();
        let (models, groups) = parse_catalog(LIVE_SHAPES.as_bytes()).unwrap();
        *catalog.latest.lock().await = Some((Instant::now(), models, groups));
        let exe = Path::new("/nonexistent/devin");
        let swe = catalog
            .selection(exe, Duration::from_secs(1), "swe-2-medium")
            .await
            .unwrap();
        assert_eq!(swe.members, ["swe-2-high", "swe-2-medium", "swe-2-max"]);
        assert_eq!(
            (swe.effort, swe.fast),
            (Some(ReasoningLevel::Medium), false)
        );
        let fast = catalog
            .selection(exe, Duration::from_secs(1), "gpt-6-sol-high-priority")
            .await
            .unwrap();
        assert_eq!((fast.effort, fast.fast), (Some(ReasoningLevel::High), true));
        assert!(
            catalog
                .selection(exe, Duration::from_secs(1), "unknown")
                .await
                .is_none()
        );
    }

    #[test]
    fn invalid_or_empty_catalogs_are_retryable_errors() {
        for bytes in [
            "not json",
            "{}",
            r#"{"families":[]}"#,
            r#"{"families":[{"variants":[{"label":"Missing id"}]}]}"#,
            r#"{"families":[{"variants":[{"model_uid":"","label":"Empty id"}]}]}"#,
        ] {
            assert!(parse_catalog(bytes.as_bytes()).is_err(), "{bytes}");
        }
    }
}
