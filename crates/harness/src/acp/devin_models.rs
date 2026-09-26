//! Devin's ACP session starts with a bundled catalog and refreshes it later.
//! `models list` waits for the account catalog before writing JSON, so use it
//! instead of racing `session/new` against `config_option_update` notifications.
//!
//! The CLI reports every variant — model × effort × fast × context, plus every
//! Fusion lead/effort/sidekick combination — as a separate model, which reads
//! as an unreadable wall of near-duplicates in the picker. [`parse_catalog`]
//! folds each family into ONE model carrying a reasoning ladder and the
//! option selects the variant encodes (fast mode, 1M context, thinking toggle;
//! Fusion's lead/sidekick/fast-mode), keeping the parsed variants alongside so
//! [`Catalog::resolve`] can turn a run's model + traits back into the exact
//! `model_uid` the session expects.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
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
) -> Result<(), HarnessError> {
    let wait = async {
        loop {
            if super::models_from_session(response, &[])
                .iter()
                .any(|m| m.id == model)
            {
                return Ok(());
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

#[derive(Default)]
pub(super) struct Catalog {
    // Only overlapping callers share a result. A later picker open always
    // probes again, including after errors, login changes, or model rollouts.
    latest: Mutex<Option<(Instant, Arc<ParsedCatalog>)>>,
}

/// The run's picks turned back into a concrete `model_uid`.
pub(super) enum Resolution {
    /// `request.model` is not a folded catalog id (a legacy variant uid or an
    /// unknown id) — send it to the session untouched.
    PassThrough,
    /// `request.model` + traits resolved to this variant uid.
    Variant(String),
}

impl Catalog {
    pub(super) async fn refresh(
        &self,
        exe: &Path,
        timeout: Duration,
    ) -> Result<Vec<Model>, HarnessError> {
        let requested_at = Instant::now();
        let mut latest = self.latest.lock().await;
        if let Some((completed_at, parsed)) = &*latest
            && *completed_at >= requested_at
        {
            return Ok(parsed.models.clone());
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
        let parsed = parse_catalog(&output.stdout)?;
        let models = parsed.models.clone();
        *latest = Some((Instant::now(), Arc::new(parsed)));
        Ok(models)
    }

    /// The parsed catalog, reusing the latest probe and fetching only when
    /// nothing has landed yet (a run can precede the first picker open).
    async fn parsed(
        &self,
        exe: &Path,
        timeout: Duration,
    ) -> Result<Arc<ParsedCatalog>, HarnessError> {
        {
            let latest = self.latest.lock().await;
            if let Some((_, parsed)) = &*latest {
                return Ok(parsed.clone());
            }
        }
        self.refresh(exe, timeout).await?;
        let latest = self.latest.lock().await;
        Ok(latest
            .as_ref()
            .map(|(_, parsed)| parsed.clone())
            .expect("refresh stores the parsed catalog"))
    }

    /// Map a run's folded model id + reasoning + option picks to the variant
    /// `model_uid` Devin's session config expects. `Err` only when the model
    /// IS a catalog id but the picked combination doesn't exist (Fusion can't
    /// pair a lead with itself); a failed catalog fetch falls back to
    /// [`Resolution::PassThrough`] so a legacy variant id still runs.
    pub(super) async fn resolve(
        &self,
        exe: &Path,
        timeout: Duration,
        model: &str,
        reasoning: Option<ReasoningLevel>,
        options: &serde_json::Map<String, Value>,
    ) -> Result<Resolution, HarnessError> {
        let Ok(parsed) = self.parsed(exe, timeout).await else {
            return Ok(Resolution::PassThrough);
        };
        resolve(&parsed, model, reasoning, options)
    }
}

/// The catalog as the picker sees it (`models`) plus the per-model variant
/// matrix resolution needs (`variants`, keyed by the emitted model id).
struct ParsedCatalog {
    models: Vec<Model>,
    variants: HashMap<String, Vec<ParsedVariant>>,
}

/// One selectable variant with the traits its label/uid encode. `lead` /
/// `sidekick` are set only on Fusion combos (the uid's `fusion-<lead>-…
/// -sidekick-<sidekick>` shape); `sidekick` is normalized so the `-priority`
/// serving tier rides the variant's `fast` flag instead of its id.
#[derive(Clone)]
struct ParsedVariant {
    uid: String,
    effort: Option<ReasoningLevel>,
    thinking: bool,
    fast: bool,
    context_1m: bool,
    lead: Option<String>,
    sidekick: Option<String>,
}

#[derive(Deserialize)]
struct ModelList {
    families: Vec<Family>,
}

#[derive(Deserialize)]
struct Family {
    family_uid: Option<String>,
    family_label: Option<String>,
    #[serde(default)]
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
struct Variant {
    model_uid: String,
    label: String,
    description: Option<String>,
    cost_summary: Option<String>,
}

/// Lowercase alnum-only — ids and labels disagree on separators and case
/// (`gpt-5.6-sol` vs `gpt-5-6-sol-*`, `MODEL_GPT_5_2_LOW` vs "GPT-5.2 Low
/// Thinking"), so traits are parsed off the normalized label instead.
fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The effort a normalized label suffix names; "no thinking"/"none" collapse
/// into Minimal (the ladder has no explicit Off level).
fn effort_word(word: &str) -> Option<ReasoningLevel> {
    Some(match word {
        "no" | "none" | "minimal" => ReasoningLevel::Minimal,
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        "ultra" => ReasoningLevel::Ultra,
        _ => return None,
    })
}

/// Traits a variant label carries past the family label: trailing `1M`
/// context, trailing `Fast` (Devin's priority serving tier), a bare
/// `Thinking` toggle, or an effort word optionally qualified by `Thinking`
/// ("Low Thinking" → Low, "No Thinking" → Minimal). `1M`/`Fast` stack in
/// either order, so peel both until neither strips.
fn variant_traits(family_label: &str, label: &str) -> (Option<ReasoningLevel>, bool, bool, bool) {
    let normalized = norm(label);
    let mut rest = normalized
        .strip_prefix(&norm(family_label))
        .unwrap_or(normalized.as_str());
    let mut context_1m = false;
    let mut fast = false;
    loop {
        if let Some(r) = rest.strip_suffix("1m") {
            rest = r;
            context_1m = true;
        } else if let Some(r) = rest.strip_suffix("fast") {
            rest = r;
            fast = true;
        } else {
            break;
        }
    }
    if rest == "thinking" {
        return (None, true, fast, context_1m);
    }
    let effort = effort_word(rest.strip_suffix("thinking").unwrap_or(rest));
    (effort, false, fast, context_1m)
}

/// `fusion-{lead}-{effort}[-fast]-sidekick-{sidekick}[-priority]`. The GPT
/// sidekicks spell their fast tier `-priority`; it only exists under fast
/// leads, so folding it into `fast` keeps one sidekick id per model.
fn parse_fusion_uid(uid: &str) -> Option<ParsedVariant> {
    let (lead_part, sidekick) = uid.strip_prefix("fusion-")?.split_once("-sidekick-")?;
    let (lead_part, fast) = match lead_part.strip_suffix("-fast") {
        Some(lead) => (lead, true),
        None => (lead_part, false),
    };
    let (lead, effort) = lead_part.rsplit_once('-')?;
    let effort = effort_word(effort)?;
    if lead.is_empty() {
        return None;
    }
    Some(ParsedVariant {
        uid: uid.to_owned(),
        effort: Some(effort),
        thinking: false,
        fast,
        context_1m: false,
        lead: Some(lead.to_owned()),
        sidekick: Some(
            sidekick
                .strip_suffix("-priority")
                .unwrap_or(sidekick)
                .to_owned(),
        ),
    })
}

/// The "+ X" half of a `Fusion (L + S)` label, minus the Fast tag the
/// `-priority` sidekick spelling adds.
fn fusion_sidekick_label(label: &str) -> Option<String> {
    let inner = label.strip_prefix("Fusion (")?.strip_suffix(')')?;
    let side = inner.rsplit_once(" + ")?.1;
    Some(
        side.strip_suffix(" Fast")
            .unwrap_or(side)
            .trim_end()
            .to_owned(),
    )
}

/// `fastMode`-style truthiness shared by every harness's option encoding.
fn option_on(options: &serde_json::Map<String, Value>, id: &str) -> bool {
    match options.get(id) {
        Some(Value::Bool(on)) => *on,
        Some(Value::String(choice)) => {
            matches!(choice.as_str(), "on" | "true" | "fast" | "priority")
        }
        _ => false,
    }
}

fn toggle(id: &str, label: &str) -> ModelOption {
    ModelOption {
        id: id.into(),
        label: label.into(),
        choices: vec![
            ModelOptionChoice {
                id: "off".into(),
                label: "Off".into(),
            },
            ModelOptionChoice {
                id: "on".into(),
                label: "On".into(),
            },
        ],
        default_choice: "off".into(),
    }
}

fn context_window() -> ModelOption {
    ModelOption {
        id: "contextWindow".into(),
        label: "Context Window".into(),
        choices: vec![
            ModelOptionChoice {
                id: "200k".into(),
                label: "200K".into(),
            },
            ModelOptionChoice {
                id: "1m".into(),
                label: "1M".into(),
            },
        ],
        default_choice: "200k".into(),
    }
}

/// Whether the variant matrix actually offers the trait on BOTH settings —
/// only then is the option real (and the request bit a hard constraint).
fn both_ways(variants: &[ParsedVariant], trait_of: fn(&ParsedVariant) -> bool) -> bool {
    variants.iter().any(&trait_of) && variants.iter().any(|v| !trait_of(v))
}

/// Pick the run's variant: hard-filter the selects and offered toggles, then
/// the requested effort. `reasoning: None` (traits tray untouched on an
/// effort-less model, or a drive-by request) prefers Medium then High, the
/// sane middle of Devin's ladders.
fn pick_variant(
    model_id: &str,
    model: &Model,
    variants: &[ParsedVariant],
    reasoning: Option<ReasoningLevel>,
    options: &serde_json::Map<String, Value>,
) -> Result<String, HarnessError> {
    let choice = |id: &str| -> Option<&str> {
        options.get(id).and_then(Value::as_str).or_else(|| {
            model
                .options
                .iter()
                .find(|option| option.id == id)
                .map(|option| option.default_choice.as_str())
        })
    };
    let lead = choice("lead");
    let sidekick = choice("sidekick");
    let fast = option_on(options, "fastMode");
    let context_1m = options
        .get("contextWindow")
        .and_then(Value::as_str)
        .is_some_and(|w| w.eq_ignore_ascii_case("1m"));
    let thinking = option_on(options, "thinking");

    let offered_fast = both_ways(variants, |v| v.fast);
    let offered_context = both_ways(variants, |v| v.context_1m);
    let offered_thinking = both_ways(variants, |v| v.thinking);
    let candidates: Vec<&ParsedVariant> = variants
        .iter()
        .filter(|v| {
            lead.is_none_or(|l| v.lead.as_deref() == Some(l))
                && sidekick.is_none_or(|s| v.sidekick.as_deref() == Some(s))
                && (!offered_fast || v.fast == fast)
                && (!offered_context || v.context_1m == context_1m)
                && (!offered_thinking || v.thinking == thinking)
        })
        .collect();

    // A stale reasoning pick must not sink a run on a model with no effort
    // axis at all (the UI clamps to the ladder only when one exists).
    let reasoning = reasoning.filter(|_| variants.iter().any(|v| v.effort.is_some()));
    let resolved = match reasoning {
        Some(level) => candidates.iter().find(|v| v.effort == Some(level)).copied(),
        None => [ReasoningLevel::Medium, ReasoningLevel::High]
            .into_iter()
            .find_map(|level| candidates.iter().find(|v| v.effort == Some(level)).copied())
            .or_else(|| candidates.first().copied()),
    };
    resolved.map(|v| v.uid.clone()).ok_or_else(|| {
        HarnessError::Protocol(format!(
            "Devin doesn't offer {model_id} with the requested configuration"
        ))
    })
}

fn resolve(
    parsed: &ParsedCatalog,
    model: &str,
    reasoning: Option<ReasoningLevel>,
    options: &serde_json::Map<String, Value>,
) -> Result<Resolution, HarnessError> {
    let Some(variants) = parsed.variants.get(model) else {
        return Ok(Resolution::PassThrough);
    };
    let Some(entry) = parsed.models.iter().find(|m| m.id == model) else {
        return Ok(Resolution::PassThrough);
    };
    Ok(Resolution::Variant(pick_variant(
        model, entry, variants, reasoning, options,
    )?))
}

fn parse_catalog(bytes: &[u8]) -> Result<ParsedCatalog, HarnessError> {
    let catalog: ModelList = serde_json::from_slice(bytes)
        .map_err(|error| HarnessError::Protocol(format!("invalid Devin model catalog: {error}")))?;
    let mut models: Vec<Model> = Vec::new();
    let mut parsed_variants: HashMap<String, Vec<ParsedVariant>> = HashMap::new();
    // family label by normalized family uid — resolves Fusion lead keys
    // (uid-style "gpt-5-6-sol") to display labels ("GPT-5.6 Sol").
    let family_labels: HashMap<String, String> = catalog
        .families
        .iter()
        .filter_map(|f| {
            let uid = f
                .family_uid
                .clone()
                .or_else(|| f.variants.first().map(|v| v.model_uid.clone()))?;
            let label = f.family_label.clone().unwrap_or_else(|| uid.clone());
            Some((norm(&uid), label))
        })
        .collect();

    for family in catalog.families {
        if family.variants.is_empty() {
            continue;
        }
        let id = family
            .family_uid
            .clone()
            .filter(|uid| !uid.trim().is_empty())
            .unwrap_or_else(|| family.variants[0].model_uid.clone());
        let label = family
            .family_label
            .clone()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| id.clone());
        if models.iter().any(|m| m.id == id) {
            continue;
        }
        for variant in &family.variants {
            if variant.model_uid.trim().is_empty() || variant.label.trim().is_empty() {
                return Err(HarnessError::Protocol(
                    "invalid empty Devin model id or label".into(),
                ));
            }
        }

        let is_fusion = norm(&id) == "fusion" || id.starts_with("fusion-");
        let rows: Vec<(ParsedVariant, &Variant)> = if is_fusion {
            family
                .variants
                .iter()
                .filter_map(|v| parse_fusion_uid(&v.model_uid).map(|p| (p, v)))
                .collect()
        } else {
            family
                .variants
                .iter()
                .map(|v| {
                    let (effort, thinking, fast, context_1m) = variant_traits(&label, &v.label);
                    (
                        ParsedVariant {
                            uid: v.model_uid.clone(),
                            effort,
                            thinking,
                            fast,
                            context_1m,
                            lead: None,
                            sidekick: None,
                        },
                        v,
                    )
                })
                .collect()
        };
        if rows.is_empty() {
            continue;
        }
        let variants: Vec<ParsedVariant> = rows.iter().map(|(p, _)| p.clone()).collect();

        let mut ladder: Vec<ReasoningLevel> = variants.iter().filter_map(|v| v.effort).collect();
        ladder.sort();
        ladder.dedup();
        let ladder = if ladder.len() > 1 { ladder } else { Vec::new() };

        let mut options = Vec::new();
        if is_fusion {
            let mut leads: Vec<String> = Vec::new();
            let mut sidekicks: Vec<(String, String)> = Vec::new();
            for (variant, raw) in &rows {
                if let Some(lead) = &variant.lead
                    && !leads.contains(lead)
                {
                    leads.push(lead.clone());
                }
                if let Some(sidekick) = &variant.sidekick
                    && !sidekicks.iter().any(|(id, _)| id == sidekick)
                {
                    let label =
                        fusion_sidekick_label(&raw.label).unwrap_or_else(|| sidekick.clone());
                    sidekicks.push((sidekick.clone(), label));
                }
            }
            options.push(ModelOption {
                id: "lead".into(),
                label: "Lead".into(),
                default_choice: leads
                    .iter()
                    .find(|l| l.as_str() == "claude-fable-5-1")
                    .or_else(|| leads.first())
                    .cloned()
                    .unwrap_or_default(),
                choices: leads
                    .iter()
                    .map(|lead| ModelOptionChoice {
                        id: lead.clone(),
                        label: family_labels
                            .get(&norm(lead))
                            .cloned()
                            .unwrap_or_else(|| lead.clone()),
                    })
                    .collect(),
            });
            options.push(ModelOption {
                id: "sidekick".into(),
                label: "Sidekick".into(),
                default_choice: sidekicks
                    .iter()
                    .find(|(id, _)| id == "swe-2-high")
                    .or_else(|| sidekicks.first())
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default(),
                choices: sidekicks
                    .into_iter()
                    .map(|(id, label)| ModelOptionChoice { id, label })
                    .collect(),
            });
        }
        if both_ways(&variants, |v| v.context_1m) {
            options.push(context_window());
        }
        if both_ways(&variants, |v| v.thinking) {
            options.push(toggle("thinking", "Thinking"));
        }
        if both_ways(&variants, |v| v.fast) {
            options.push(toggle("fastMode", "Fast Mode"));
        }

        // The description rides the variant the defaults would pick so the
        // picker's tagline prices the configuration it names.
        let defaults = serde_json::Map::new();
        let representative = pick_variant(
            &id,
            &Model {
                id: id.clone(),
                label: label.clone(),
                description: None,
                reasoning_levels: ladder.clone(),
                options: options.clone(),
            },
            &variants,
            None,
            &defaults,
        )
        .ok()
        .and_then(|uid| family.variants.iter().find(|v| v.model_uid == uid))
        .or_else(|| family.variants.first());
        let description = representative.and_then(|v| {
            v.description
                .clone()
                .filter(|d| !d.trim().is_empty())
                .or_else(|| v.cost_summary.clone())
        });

        models.push(Model {
            id: id.clone(),
            label,
            description,
            reasoning_levels: ladder,
            options,
        });
        parsed_variants.insert(id, variants);
    }
    if models.is_empty() {
        return Err(HarnessError::Protocol(
            "Devin returned an empty model catalog".into(),
        ));
    }
    Ok(ParsedCatalog {
        models,
        variants: parsed_variants,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> ParsedCatalog {
        parse_catalog(bytes).unwrap()
    }

    fn no_opts() -> serde_json::Map<String, Value> {
        serde_json::Map::new()
    }

    #[test]
    fn folds_effort_and_fast_variants_into_one_model() {
        let parsed = parse(br#"{"families":[{
            "family_uid":"claude-opus-5", "family_label":"Claude Opus 5", "variants":[
                {"model_uid":"claude-opus-5-medium","label":"Claude Opus 5 Medium","cost_summary":"$5 / 1M Input"},
                {"model_uid":"claude-opus-5-low","label":"Claude Opus 5 Low"},
                {"model_uid":"claude-opus-5-high","label":"Claude Opus 5 High"},
                {"model_uid":"claude-opus-5-xhigh","label":"Claude Opus 5 XHigh"},
                {"model_uid":"claude-opus-5-max","label":"Claude Opus 5 Max"},
                {"model_uid":"claude-opus-5-low-fast","label":"Claude Opus 5 Low Fast"},
                {"model_uid":"claude-opus-5-high-fast","label":"Claude Opus 5 High Fast","cost_summary":"$10 / 1M Input"}
            ]
        }]}"#);
        assert_eq!(parsed.models.len(), 1);
        let model = &parsed.models[0];
        assert_eq!(model.id, "claude-opus-5");
        assert_eq!(model.label, "Claude Opus 5");
        assert_eq!(
            model.reasoning_levels,
            vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max,
            ]
        );
        assert_eq!(model.options.len(), 1);
        assert_eq!(model.options[0].id, "fastMode");
        assert_eq!(model.description.as_deref(), Some("$5 / 1M Input"));
        assert_eq!(parsed.variants["claude-opus-5"].len(), 7);
    }

    #[test]
    fn parses_thinking_toggle_and_context_variants() {
        let parsed = parse(
            br#"{"families":[{
            "family_uid":"claude-opus-4.6", "family_label":"Claude Opus 4.6", "variants":[
                {"model_uid":"claude-opus-4-6","label":"Claude Opus 4.6"},
                {"model_uid":"claude-opus-4-6-thinking","label":"Claude Opus 4.6 Thinking"},
                {"model_uid":"claude-opus-4-6-1m","label":"Claude Opus 4.6 1M"},
                {"model_uid":"claude-opus-4-6-thinking-1m","label":"Claude Opus 4.6 Thinking 1M"}
            ]
        }]}"#,
        );
        let model = &parsed.models[0];
        assert!(model.reasoning_levels.is_empty());
        let ids: Vec<&str> = model.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["contextWindow", "thinking"]);

        let mut opts = no_opts();
        opts.insert("thinking".into(), Value::String("on".into()));
        opts.insert("contextWindow".into(), Value::String("1m".into()));
        match resolve(&parsed, "claude-opus-4.6", None, &opts).unwrap() {
            Resolution::Variant(uid) => assert_eq!(uid, "claude-opus-4-6-thinking-1m"),
            _ => panic!("resolved"),
        }
        match resolve(&parsed, "claude-opus-4.6", None, &no_opts()).unwrap() {
            Resolution::Variant(uid) => assert_eq!(uid, "claude-opus-4-6"),
            _ => panic!("resolved"),
        }
    }

    #[test]
    fn gpt_style_ids_parse_off_labels() {
        let parsed = parse(
            br#"{"families":[{
            "family_uid":"gpt-5.2", "family_label":"GPT-5.2", "variants":[
                {"model_uid":"MODEL_GPT_5_2_NONE","label":"GPT-5.2 No Thinking"},
                {"model_uid":"MODEL_GPT_5_2_LOW","label":"GPT-5.2 Low Thinking"},
                {"model_uid":"MODEL_GPT_5_2_HIGH","label":"GPT-5.2 High Thinking"}
            ]
        }]}"#,
        );
        let model = &parsed.models[0];
        assert_eq!(
            model.reasoning_levels,
            vec![
                ReasoningLevel::Minimal,
                ReasoningLevel::Low,
                ReasoningLevel::High
            ]
        );
        match resolve(
            &parsed,
            "gpt-5.2",
            Some(ReasoningLevel::Minimal),
            &no_opts(),
        )
        .unwrap()
        {
            Resolution::Variant(uid) => assert_eq!(uid, "MODEL_GPT_5_2_NONE"),
            _ => panic!("resolved"),
        }
    }

    #[test]
    fn fusion_folds_into_one_configurable_model() {
        let parsed = parse(br#"{"families":[
            {"family_uid":"claude-fable-5-1","family_label":"Claude Fable 5.1","variants":[
                {"model_uid":"claude-fable-5-1-high","label":"Claude Fable 5.1 High"}]},
            {"family_uid":"swe-2","family_label":"SWE-2","variants":[
                {"model_uid":"swe-2-high","label":"SWE-2 High"}]},
            {"family_uid":"fusion","family_label":"Fusion","variants":[
                {"model_uid":"fusion-claude-fable-5-1-medium-sidekick-swe-2-medium","label":"Fusion (Claude Fable 5.1 Medium + SWE-2 Medium)","cost_summary":"$10 / 1M Input"},
                {"model_uid":"fusion-claude-fable-5-1-high-sidekick-swe-2-high","label":"Fusion (Claude Fable 5.1 High + SWE-2 High)"},
                {"model_uid":"fusion-claude-fable-5-1-high-fast-sidekick-swe-2-high","label":"Fusion (Claude Fable 5.1 High Fast + SWE-2 High)"},
                {"model_uid":"fusion-claude-fable-5-1-high-fast-sidekick-gpt-5-6-luna-high-priority","label":"Fusion (Claude Fable 5.1 High Fast + GPT-5.6 Luna High Thinking Fast)"},
                {"model_uid":"fusion-claude-fable-5-1-high-sidekick-gpt-5-6-luna-high","label":"Fusion (Claude Fable 5.1 High + GPT-5.6 Luna High Thinking)"}
            ]}
        ]}"#);
        let model = parsed
            .models
            .iter()
            .find(|m| m.id == "fusion")
            .expect("fusion model");
        assert_eq!(model.label, "Fusion");
        assert_eq!(
            model.reasoning_levels,
            vec![ReasoningLevel::Medium, ReasoningLevel::High]
        );
        let ids: Vec<&str> = model.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["lead", "sidekick", "fastMode"]);
        let lead = &model.options[0];
        assert_eq!(lead.default_choice, "claude-fable-5-1");
        assert_eq!(lead.choices[0].label, "Claude Fable 5.1");
        let sidekick = &model.options[1];
        assert_eq!(sidekick.default_choice, "swe-2-high");
        assert_eq!(
            sidekick
                .choices
                .iter()
                .map(|c| (c.id.as_str(), c.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("swe-2-medium", "SWE-2 Medium"),
                ("swe-2-high", "SWE-2 High"),
                ("gpt-5-6-luna-high", "GPT-5.6 Luna High Thinking"),
            ]
        );
    }

    #[test]
    fn resolve_maps_traits_to_the_exact_variant() {
        let parsed = parse(
            br#"{"families":[{
            "family_uid":"claude-opus-5", "family_label":"Claude Opus 5", "variants":[
                {"model_uid":"claude-opus-5-low","label":"Claude Opus 5 Low"},
                {"model_uid":"claude-opus-5-medium","label":"Claude Opus 5 Medium"},
                {"model_uid":"claude-opus-5-high","label":"Claude Opus 5 High"},
                {"model_uid":"claude-opus-5-high-fast","label":"Claude Opus 5 High Fast"}
            ]
        }]}"#,
        );
        // Default traits → default effort, non-fast.
        match resolve(
            &parsed,
            "claude-opus-5",
            Some(ReasoningLevel::High),
            &no_opts(),
        )
        .unwrap()
        {
            Resolution::Variant(uid) => assert_eq!(uid, "claude-opus-5-high"),
            _ => panic!("resolved"),
        }
        // Fast mode picks the priority variant of the same effort.
        let mut opts = no_opts();
        opts.insert("fastMode".into(), Value::String("on".into()));
        match resolve(&parsed, "claude-opus-5", Some(ReasoningLevel::High), &opts).unwrap() {
            Resolution::Variant(uid) => assert_eq!(uid, "claude-opus-5-high-fast"),
            _ => panic!("resolved"),
        }
        // Missing effort errors instead of silently picking another model.
        assert!(
            resolve(
                &parsed,
                "claude-opus-5",
                Some(ReasoningLevel::Ultra),
                &no_opts()
            )
            .is_err()
        );
        // Legacy variant ids pass through untouched.
        assert!(matches!(
            resolve(
                &parsed,
                "claude-opus-5-high",
                Some(ReasoningLevel::High),
                &no_opts()
            )
            .unwrap(),
            Resolution::PassThrough
        ));
        // Unknown ids pass through too (session-side validation owns them).
        assert!(matches!(
            resolve(&parsed, "mystery", None, &no_opts()).unwrap(),
            Resolution::PassThrough
        ));
    }

    #[test]
    fn resolve_fusion_composes_lead_effort_sidekick_and_fast() {
        let parsed = parse(br#"{"families":[
            {"family_uid":"fusion","family_label":"Fusion","variants":[
                {"model_uid":"fusion-claude-fable-5-1-medium-sidekick-swe-2-high","label":"Fusion (Claude Fable 5.1 Medium + SWE-2 High)"},
                {"model_uid":"fusion-claude-fable-5-1-medium-fast-sidekick-swe-2-high","label":"Fusion (Claude Fable 5.1 Medium Fast + SWE-2 High)"},
                {"model_uid":"fusion-claude-fable-5-1-high-fast-sidekick-gpt-5-6-luna-high-priority","label":"Fusion (Claude Fable 5.1 High Fast + GPT-5.6 Luna High Thinking Fast)"},
                {"model_uid":"fusion-claude-fable-5-1-high-sidekick-gpt-5-6-luna-high","label":"Fusion (Claude Fable 5.1 High + GPT-5.6 Luna High Thinking)"}
            ]}
        ]}"#);
        // Defaults: fable lead + swe-2-high sidekick + medium effort.
        match resolve(&parsed, "fusion", Some(ReasoningLevel::Medium), &no_opts()).unwrap() {
            Resolution::Variant(uid) => {
                assert_eq!(uid, "fusion-claude-fable-5-1-medium-sidekick-swe-2-high")
            }
            _ => panic!("resolved"),
        }
        let mut opts = no_opts();
        opts.insert("fastMode".into(), Value::String("on".into()));
        match resolve(&parsed, "fusion", Some(ReasoningLevel::Medium), &opts).unwrap() {
            Resolution::Variant(uid) => assert_eq!(
                uid,
                "fusion-claude-fable-5-1-medium-fast-sidekick-swe-2-high"
            ),
            _ => panic!("resolved"),
        }
        // GPT sidekicks spell their fast tier -priority.
        opts.insert("sidekick".into(), Value::String("gpt-5-6-luna-high".into()));
        match resolve(&parsed, "fusion", Some(ReasoningLevel::High), &opts).unwrap() {
            Resolution::Variant(uid) => assert_eq!(
                uid,
                "fusion-claude-fable-5-1-high-fast-sidekick-gpt-5-6-luna-high-priority"
            ),
            _ => panic!("resolved"),
        }
        opts.insert("fastMode".into(), Value::String("off".into()));
        match resolve(&parsed, "fusion", Some(ReasoningLevel::High), &opts).unwrap() {
            Resolution::Variant(uid) => assert_eq!(
                uid,
                "fusion-claude-fable-5-1-high-sidekick-gpt-5-6-luna-high"
            ),
            _ => panic!("resolved"),
        }
        // A combination Devin doesn't offer fails loudly.
        opts.insert("lead".into(), Value::String("gpt-5-6-sol".into()));
        assert!(resolve(&parsed, "fusion", Some(ReasoningLevel::High), &opts).is_err());
    }

    #[test]
    fn trait_suffixes_parse_in_either_order() {
        let parsed = parse(
            br#"{"families":[{
            "family_uid":"claude-opus-5", "family_label":"Claude Opus 5", "variants":[
                {"model_uid":"claude-opus-5","label":"Claude Opus 5"},
                {"model_uid":"claude-opus-5-fast-1m","label":"Claude Opus 5 Fast 1M"},
                {"model_uid":"claude-opus-5-1m-fast","label":"Claude Opus 5 1M Fast"}
            ]
        }]}"#,
        );
        let ids: Vec<&str> = parsed.models[0]
            .options
            .iter()
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(ids, ["contextWindow", "fastMode"]);
        let mut opts = no_opts();
        opts.insert("contextWindow".into(), Value::String("1m".into()));
        opts.insert("fastMode".into(), Value::String("on".into()));
        // Both spellings carry both flags; the first matching variant wins.
        assert!(matches!(
            resolve(&parsed, "claude-opus-5", None, &opts).unwrap(),
            Resolution::Variant(uid) if uid == "claude-opus-5-fast-1m" || uid == "claude-opus-5-1m-fast"
        ));
    }

    #[test]
    fn stale_reasoning_ignored_when_the_model_has_no_effort_axis() {
        let parsed = parse(
            br#"{"families":[{
            "family_uid":"claude-opus-4.6", "family_label":"Claude Opus 4.6", "variants":[
                {"model_uid":"claude-opus-4-6","label":"Claude Opus 4.6"},
                {"model_uid":"claude-opus-4-6-thinking","label":"Claude Opus 4.6 Thinking"}
            ]
        }]}"#,
        );
        match resolve(
            &parsed,
            "claude-opus-4.6",
            Some(ReasoningLevel::High),
            &no_opts(),
        )
        .unwrap()
        {
            Resolution::Variant(uid) => assert_eq!(uid, "claude-opus-4-6"),
            _ => panic!("resolved"),
        }
    }

    #[test]
    fn bare_variants_labeled_with_an_effort_fold_into_the_ladder() {
        // swe-1.7's bare uid is its Max tier; glm-5-2's is High.
        let parsed = parse(
            br#"{"families":[{
            "family_uid":"swe-1.7", "family_label":"SWE-1.7", "variants":[
                {"model_uid":"swe-1-7","label":"SWE-1.7 Max"},
                {"model_uid":"swe-1-7-medium","label":"SWE-1.7 Medium"}
            ]
        }]}"#,
        );
        assert_eq!(
            parsed.models[0].reasoning_levels,
            vec![ReasoningLevel::Medium, ReasoningLevel::Max]
        );
        match resolve(&parsed, "swe-1.7", Some(ReasoningLevel::Max), &no_opts()).unwrap() {
            Resolution::Variant(uid) => assert_eq!(uid, "swe-1-7"),
            _ => panic!("resolved"),
        }
    }

    #[test]
    fn invalid_or_empty_catalogs_are_retryable_errors() {
        for bytes in [
            "not json",
            "{}",
            r#"{"families":[]}"#,
            r#"{"families":[{"variants":[]}]}"#,
            r#"{"families":[{"variants":[{"label":"Missing id"}]}]}"#,
            r#"{"families":[{"variants":[{"model_uid":"","label":"Empty id"}]}]}"#,
        ] {
            assert!(parse_catalog(bytes.as_bytes()).is_err(), "{bytes}");
        }
    }
}
