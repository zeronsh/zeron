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

/// A picker model's variants, plus the (lead, sidekick) choice ids when the
/// group is a Fusion pair.
#[derive(Debug, Clone, PartialEq)]
struct Group {
    members: Vec<Member>,
    pair: Option<(String, String)>,
}

/// The single picker entry standing for every Fusion pair; `lead` and
/// `sidekick` model options pick the pair.
pub(super) const FUSION: &str = "fusion";

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
    latest: Mutex<Option<(Instant, Vec<Model>, Vec<Group>)>>,
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
    /// process has none yet. `None` when the catalog does not know the id —
    /// or, for [`FUSION`], when it offers no pair for the chosen lead and
    /// sidekick (`options`, falling back to the catalog's first pair).
    pub(super) async fn selection(
        &self,
        exe: &Path,
        timeout: Duration,
        model: &str,
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Selection> {
        let known = self.latest.lock().await.is_some();
        if !known {
            self.refresh(exe, timeout).await.ok()?;
        }
        let latest = self.latest.lock().await;
        let (_, _, groups) = latest.as_ref()?;
        if model == FUSION {
            let (default_lead, default_sidekick) = groups.iter().find_map(|g| g.pair.clone())?;
            let pick = |key: &str, default: String| {
                options
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or(default)
            };
            let wanted = (
                pick("lead", default_lead),
                pick("sidekick", default_sidekick),
            );
            let group = groups.iter().find(|g| g.pair.as_ref() == Some(&wanted))?;
            return Some(Selection {
                members: group.members.iter().map(|m| m.id.clone()).collect(),
                effort: None,
                fast: false,
            });
        }
        groups.iter().find_map(|group| {
            let hit = group.members.iter().find(|m| m.id == model)?;
            Some(Selection {
                members: group.members.iter().map(|m| m.id.clone()).collect(),
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
/// "GLM-5.2 Max 1M" → ("1M", Max, false). A Fusion pair
/// "(Claude Opus 5.5 High Fast + SWE-2 Medium)" → ("(Claude Opus 5.5 + SWE-2
/// Medium)", High, true): the ACP session selects one id per primary model
/// and sidekick, and the effort it exposes is the primary's.
fn parse_variant(family: &str, label: &str) -> (String, Option<ReasoningLevel>, bool) {
    let rest = label
        .strip_prefix(family)
        .filter(|r| r.is_empty() || r.starts_with(' '))
        .unwrap_or(label)
        .trim();
    if let Some((primary, sidekick)) = rest
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(')'))
        .and_then(|r| r.split_once(" + "))
    {
        let (base, effort, fast) = split_effort(primary);
        let (sidekick, _, sidekick_fast) = split_effort_keep_level(sidekick);
        return (
            format!("({base} + {sidekick})"),
            effort,
            fast || sidekick_fast,
        );
    }
    if rest.starts_with('(') || rest.contains('+') {
        return (rest.to_owned(), None, false);
    }
    split_effort(rest)
}

fn effort_word(word: &str, next: Option<&&str>) -> Option<(ReasoningLevel, bool)> {
    Some(match word {
        // "No Thinking" is Devin's `none`; Zeron's lowest level stands in.
        "No" if next == Some(&"Thinking") => (ReasoningLevel::Minimal, true),
        "None" | "Minimal" => (ReasoningLevel::Minimal, false),
        "Low" => (ReasoningLevel::Low, false),
        "Medium" => (ReasoningLevel::Medium, false),
        "High" => (ReasoningLevel::High, false),
        "XHigh" | "X-High" => (ReasoningLevel::XHigh, false),
        "Max" => (ReasoningLevel::Max, false),
        _ => return None,
    })
}

/// "High Thinking Fast 1M" → ("1M", High, true): the words that are not
/// effort or speed, joined.
fn split_effort(text: &str) -> (String, Option<ReasoningLevel>, bool) {
    let mut effort = None;
    let mut fast = false;
    let mut rest = Vec::new();
    let mut words = text.split_whitespace().peekable();
    while let Some(word) = words.next() {
        match effort_word(word, words.peek()) {
            Some((level, consumes_next)) if effort.is_none() => {
                effort = Some(level);
                if consumes_next || words.peek() == Some(&"Thinking") {
                    words.next();
                }
            }
            _ if word == "Fast" => fast = true,
            _ => rest.push(word),
        }
    }
    (rest.join(" "), effort, fast)
}

/// A Fusion sidekick's effort is part of its identity ("SWE-2 Medium" and
/// "SWE-2 High" are different ids); only its speed is dropped.
fn split_effort_keep_level(text: &str) -> (String, Option<ReasoningLevel>, bool) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let fast = words.last() == Some(&"Fast");
    let kept = if fast {
        &words[..words.len() - 1]
    } else {
        &words[..]
    };
    (kept.join(" "), None, fast)
}

fn slug(label: &str) -> String {
    label
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn speed_option(label: &str) -> ModelOption {
    ModelOption {
        id: "speed".into(),
        label: label.into(),
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
    }
}

fn choice_option(id: &str, label: &str, choices: &[(String, String)]) -> ModelOption {
    ModelOption {
        id: id.into(),
        label: label.into(),
        choices: choices
            .iter()
            .map(|(id, label)| ModelOptionChoice {
                id: id.clone(),
                label: label.clone(),
            })
            .collect(),
        default_choice: choices[0].0.clone(),
    }
}

fn parse_catalog(bytes: &[u8]) -> Result<(Vec<Model>, Vec<Group>), HarnessError> {
    let catalog: ModelList = serde_json::from_slice(bytes)
        .map_err(|error| HarnessError::Protocol(format!("invalid Devin model catalog: {error}")))?;
    let mut models: Vec<Model> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    // Fusion: every pair folds into one picker model (see [`FUSION`]).
    let mut leads: Vec<(String, String)> = Vec::new();
    let mut sidekicks: Vec<(String, String)> = Vec::new();
    let mut fusion_levels: Vec<ReasoningLevel> = Vec::new();
    let mut fusion_fast = false;
    let mut fusion_label = None;
    let mut fusion_description = None;
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
                .map(|g| &g.members)
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
            // A Fusion pair is one model whatever its variants' speeds (the
            // sidekick's is not in the key); elsewhere a repeated
            // effort/speed pair means a genuinely different model.
            let pair = qualifier.contains(" + ");
            let group = family_groups.iter_mut().find(|g| {
                g.0 == qualifier
                    && (effort.is_some() || fast)
                    && g.3.iter().any(|m| m.effort.is_some() || m.fast)
                    && (pair || !g.3.iter().any(|m| m.effort == effort && m.fast == fast))
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
        for (qualifier, label, description, members) in family_groups {
            let mut reasoning_levels: Vec<ReasoningLevel> =
                members.iter().filter_map(|m| m.effort).collect();
            reasoning_levels.sort();
            reasoning_levels.dedup();
            if let Some((lead, sidekick)) = qualifier
                .strip_prefix('(')
                .and_then(|q| q.strip_suffix(')'))
                .and_then(|q| q.split_once(" + "))
            {
                // Devin shows the sidekick without "Thinking".
                let sidekick = sidekick
                    .split_whitespace()
                    .filter(|w| *w != "Thinking")
                    .collect::<Vec<_>>()
                    .join(" ");
                let pair = (slug(lead), slug(&sidekick));
                if !leads.iter().any(|(id, _)| *id == pair.0) {
                    leads.push((pair.0.clone(), lead.to_owned()));
                }
                if !sidekicks.iter().any(|(id, _)| *id == pair.1) {
                    sidekicks.push((pair.1.clone(), sidekick));
                }
                fusion_levels.extend(reasoning_levels);
                fusion_fast |= members.iter().any(|m| m.fast);
                fusion_label.get_or_insert_with(|| family.family_label.clone());
                if fusion_description.is_none() {
                    fusion_description = description;
                }
                groups.push(Group {
                    members,
                    pair: Some(pair),
                });
                continue;
            }
            let options = if members.iter().any(|m| m.fast) {
                vec![speed_option("Speed")]
            } else {
                Vec::new()
            };
            models.push(Model {
                id: members
                    .iter()
                    .find(|m| !m.fast)
                    .unwrap_or(&members[0])
                    .id
                    .clone(),
                label,
                description,
                reasoning_levels,
                options,
            });
            groups.push(Group {
                members,
                pair: None,
            });
        }
    }
    if !leads.is_empty() {
        fusion_levels.sort();
        fusion_levels.dedup();
        let mut options = vec![
            choice_option("lead", "Lead", &leads),
            choice_option("sidekick", "Sidekick", &sidekicks),
        ];
        // Offered where the lead has fast variants; the session exposes
        // `speed` for those leads and the run ignores it for the rest.
        if fusion_fast {
            options.push(speed_option("Fast Mode"));
        }
        let fusion = Model {
            id: FUSION.into(),
            label: fusion_label.unwrap_or_else(|| "Fusion".into()),
            description: fusion_description,
            reasoning_levels: fusion_levels,
            options,
        };
        // Directly under Adaptive, as Devin lists it.
        let at = models
            .iter()
            .position(|m| m.id == "adaptive")
            .map_or(0, |i| i + 1);
        models.insert(at, fusion);
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
            {"model_uid":"fusion-claude-opus-5-5-high-sidekick-swe-2-medium","label":"Fusion (Claude Opus 5.5 High + SWE-2 Medium)"},
            {"model_uid":"fusion-claude-opus-5-5-max-fast-sidekick-swe-2-medium","label":"Fusion (Claude Opus 5.5 Max Fast + SWE-2 Medium)"},
            {"model_uid":"fusion-claude-opus-5-5-low-sidekick-swe-2-medium","label":"Fusion (Claude Opus 5.5 Low + SWE-2 Medium)"},
            {"model_uid":"fusion-claude-opus-5-5-high-sidekick-swe-2-high","label":"Fusion (Claude Opus 5.5 High + SWE-2 High)"},
            {"model_uid":"fusion-gpt-6-sol-high-sidekick-gpt-6-luna-high-priority","label":"Fusion (GPT-6 Sol High Thinking + GPT-6 Luna High Thinking Fast)"},
            {"model_uid":"fusion-gpt-6-sol-high-sidekick-gpt-6-luna-high","label":"Fusion (GPT-6 Sol High Thinking + GPT-6 Luna High Thinking)"}]},
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
                ("adaptive", "Adaptive", vec![], 0),
                // Every Fusion pair folds into one entry right under Adaptive.
                ("fusion", "Fusion", vec![Low, High, Max], 3),
            ]
        );
        assert_eq!(models[1].options[0].id, "speed");
        assert_eq!(models[1].description.as_deref(), Some("$2 / 1M"));
        let sol = &groups[1].members;
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

    #[test]
    fn fusion_offers_lead_sidekick_and_fast_mode_choices() {
        let (models, _) = parse_catalog(LIVE_SHAPES.as_bytes()).unwrap();
        let fusion = models.iter().find(|m| m.id == FUSION).unwrap();
        let choices = |id: &str| {
            let option = fusion.options.iter().find(|o| o.id == id).unwrap();
            (
                option.label.clone(),
                option
                    .choices
                    .iter()
                    .map(|c| (c.id.clone(), c.label.clone()))
                    .collect::<Vec<_>>(),
                option.default_choice.clone(),
            )
        };
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            choices("lead"),
            (
                "Lead".into(),
                vec![
                    pair("claude-opus-5-5", "Claude Opus 5.5"),
                    pair("gpt-6-sol", "GPT-6 Sol")
                ],
                "claude-opus-5-5".into()
            )
        );
        assert_eq!(
            choices("sidekick"),
            (
                "Sidekick".into(),
                vec![
                    pair("swe-2-medium", "SWE-2 Medium"),
                    pair("swe-2-high", "SWE-2 High"),
                    pair("gpt-6-luna-high", "GPT-6 Luna High")
                ],
                "swe-2-medium".into()
            )
        );
        assert_eq!(choices("speed").0, "Fast Mode");
    }

    #[tokio::test]
    async fn fusion_resolves_the_chosen_lead_and_sidekick_pair() {
        let catalog = Catalog::default();
        let (models, groups) = parse_catalog(LIVE_SHAPES.as_bytes()).unwrap();
        *catalog.latest.lock().await = Some((Instant::now(), models, groups));
        let exe = Path::new("/nonexistent/devin");
        let options = |lead: &str, sidekick: &str| {
            let mut map = serde_json::Map::new();
            map.insert("lead".into(), lead.into());
            map.insert("sidekick".into(), sidekick.into());
            map
        };
        let pick = |opts| {
            let catalog = &catalog;
            async move {
                catalog
                    .selection(exe, Duration::from_secs(1), FUSION, &opts)
                    .await
            }
        };
        let high = pick(options("claude-opus-5-5", "swe-2-high"))
            .await
            .unwrap();
        assert_eq!(
            high.members,
            ["fusion-claude-opus-5-5-high-sidekick-swe-2-high"]
        );
        let default = pick(serde_json::Map::new()).await.unwrap();
        assert!(
            default
                .members
                .contains(&"fusion-claude-opus-5-5-high-sidekick-swe-2-medium".to_owned())
        );
        assert!(pick(options("gpt-6-sol", "swe-2-high")).await.is_none());
    }

    #[tokio::test]
    async fn saved_variant_ids_resolve_to_their_group_with_effort_and_speed() {
        let catalog = Catalog::default();
        let (models, groups) = parse_catalog(LIVE_SHAPES.as_bytes()).unwrap();
        *catalog.latest.lock().await = Some((Instant::now(), models, groups));
        let exe = Path::new("/nonexistent/devin");
        let swe = catalog
            .selection(
                exe,
                Duration::from_secs(1),
                "swe-2-medium",
                &Default::default(),
            )
            .await
            .unwrap();
        assert_eq!(swe.members, ["swe-2-high", "swe-2-medium", "swe-2-max"]);
        assert_eq!(
            (swe.effort, swe.fast),
            (Some(ReasoningLevel::Medium), false)
        );
        let fast = catalog
            .selection(
                exe,
                Duration::from_secs(1),
                "gpt-6-sol-high-priority",
                &Default::default(),
            )
            .await
            .unwrap();
        assert_eq!((fast.effort, fast.fast), (Some(ReasoningLevel::High), true));
        assert!(
            catalog
                .selection(exe, Duration::from_secs(1), "unknown", &Default::default())
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
