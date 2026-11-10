//! Model catalog + effort mapping for Claude Code, ported from zeron's
//! `packages/harness/src/claude.ts` (which itself mirrors Claude Code's own
//! picker via t3code's catalog).
//!
//! Runtime initialize discovery appends concrete model ids to this curated list
//! and fills ID-only labels for configured models with discovered display names.
//! Curated rows retain their labels, effort ladders, and option sets because the
//! CLI can under-report supported modes. The shared initialize probe also supplies
//! slash commands and is cached by credential and binary context.

use zeron_proto::{Model, ModelOption, ModelOptionChoice, ReasoningLevel};

/// The ultrathink directive rides every user message as a prompt prefix — that
/// is how the mode actually works in Claude Code (a prompt convention, not an
/// effort flag). Applied to the initial prompt AND every steer.
pub(crate) const ULTRATHINK_PREFIX: &str = "Ultrathink:\n";

pub(crate) fn apply_ultrathink(reasoning: Option<ReasoningLevel>, text: &str) -> String {
    if reasoning == Some(ReasoningLevel::Ultrathink)
        && zeron_proto::invocation::leading_command(text).is_none()
    {
        format!("{ULTRATHINK_PREFIX}{text}")
    } else {
        text.to_owned()
    }
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Models whose CLI accepts `xhigh` natively; elsewhere it clamps to `max`
/// (mirroring Claude Code's own normalization). Substring port of claude.ts's
/// `/fable-5|opus-4-[7-9]|opus-[5-9]|sonnet-[5-9]/`.
pub(crate) fn supports_xhigh(model: &str) -> bool {
    contains_any(
        model,
        &[
            "fable-5", "opus-4-7", "opus-4-8", "opus-4-9", "opus-5", "opus-6", "opus-7", "opus-8",
            "opus-9", "sonnet-5", "sonnet-6", "sonnet-7", "sonnet-8", "sonnet-9",
        ],
    )
}

/// Map the unified level to the `--effort` flag value the CLI accepts for this
/// model. The special modes don't translate directly: `ultrathink` is a prompt
/// prefix (no flag), `ultracode` runs as `xhigh` plus the ultracode setting,
/// and `ultra` is a Codex-only tier (Claude tops out at `max`).
pub(crate) fn to_effort(
    reasoning: Option<ReasoningLevel>,
    model: Option<&str>,
) -> Option<&'static str> {
    let base = match reasoning? {
        ReasoningLevel::Ultrathink => return None,
        ReasoningLevel::Minimal | ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::XHigh | ReasoningLevel::Ultracode => "xhigh",
        ReasoningLevel::Max | ReasoningLevel::Ultra => "max",
    };
    if base == "xhigh" && !model.is_some_and(supports_xhigh) {
        return Some("max");
    }
    Some(base)
}

/// A boolean toggle rendered as an off/on select (the Rust `ModelOption` wire
/// type has no dedicated boolean kind).
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

/// The 200K/1M context-window select carried by the long-context models. The
/// 1M window is selected via a model-id suffix (`<model>[1m]`), exactly how the
/// CLI itself does it.
pub(crate) fn context_window() -> ModelOption {
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

const FULL_LADDER: &[ReasoningLevel] = &[
    ReasoningLevel::Low,
    ReasoningLevel::Medium,
    ReasoningLevel::High,
    ReasoningLevel::XHigh,
    ReasoningLevel::Max,
    ReasoningLevel::Ultracode,
    ReasoningLevel::Ultrathink,
];

/// opus-4-7 / sonnet-5+ tier (claude.ts `claudeEffortsFor`): xhigh native,
/// no ultracode.
const XHIGH_LADDER: &[ReasoningLevel] = &[
    ReasoningLevel::Low,
    ReasoningLevel::Medium,
    ReasoningLevel::High,
    ReasoningLevel::XHigh,
    ReasoningLevel::Max,
    ReasoningLevel::Ultrathink,
];

fn model(
    id: &str,
    label: &str,
    description: &str,
    ladder: &[ReasoningLevel],
    options: Vec<ModelOption>,
) -> Model {
    Model {
        default: false,
        id: id.into(),
        label: label.into(),
        description: (!description.is_empty()).then(|| description.into()),
        reasoning_levels: ladder.to_vec(),
        options,
    }
}

/// The curated model list, mirroring claude.ts's `claudeEffortsFor` /
/// `claudeOptionsFor` ladders: full ladder (through ultracode/ultrathink) on
/// Fable 5, `max`-topped ladders on Opus/Sonnet, no efforts but a thinking
/// toggle on Haiku; context-window select on the long-context families and
/// fast mode on Opus 4.5+.
///
/// `pub`: besides the discovery-side enrichment here, the UI's display-side
/// normalization borrows these labels so alias rows served by older engines
/// still read with their version numbers ("Opus 5.5", not "Opus").
pub(crate) fn configured_models() -> Vec<Model> {
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::executable::home_or_current_dir().join(".claude"));
    models_with_settings(&root.join("settings.json"))
}

fn models_with_settings(path: &std::path::Path) -> Vec<Model> {
    let mut models = static_models();
    let settings = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_default();
    let ids = std::iter::once(settings.get("model")).chain(
        [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        ]
        .into_iter()
        .map(|key| settings.get("env").and_then(|env| env.get(key))),
    );
    for id in ids
        .flatten()
        .filter_map(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        // `"model": "sonnet"` names whatever the CLI resolves that alias to;
        // the alias itself is no row (discovery drops it the same way).
        .filter(|id| !is_alias(id))
    {
        if !models.iter().any(|m| m.id == id) {
            models.push(Model {
                default: false,
                id: id.into(),
                label: id.into(),
                description: None,
                reasoning_levels: FULL_LADDER.to_vec(),
                options: vec![],
            });
        }
    }
    sort_best_first(&mut models);
    models
}

/// The CLI's bare aliases (`sonnet`, `opus[1m]`, …): they resolve per account
/// and per CLI release, so they can't be a row or a persisted selection.
fn is_alias(id: &str) -> bool {
    matches!(
        id.strip_suffix("[1m]").unwrap_or(id),
        "default" | "opus" | "sonnet" | "haiku" | "fable"
    )
}

/// Families best-first; ids of any other family sort after them.
const FAMILIES: &[&str] = &["fable", "opus", "sonnet", "haiku"];

/// `claude-fable-5-2[1m]` -> (0, [5, 2]); `claude-haiku-4-5-20251001` -> (3, [4, 5]).
/// The family is any `-`-separated token, so the older `claude-3-5-sonnet`
/// spelling ranks too; dated snapshot suffixes are not version parts.
fn family_and_version(id: &str) -> (usize, Vec<u32>) {
    let id = id.strip_suffix("[1m]").unwrap_or(id);
    let mut family = FAMILIES.len();
    let mut version = Vec::new();
    for token in id.split('-') {
        if let Some(rank) = FAMILIES.iter().position(|f| *f == token) {
            family = family.min(rank);
        } else if token.len() < 8
            && let Ok(part) = token.parse::<u32>()
        {
            version.push(part);
        }
    }
    if family == FAMILIES.len() {
        version.clear();
    }
    (family, version)
}

/// Best first: by family (Fable, Opus, Sonnet, Haiku, then the rest), newest
/// version first within one. Stable, so other ids keep their order and a
/// `[1m]` row stays next to its base.
fn sort_best_first(models: &mut [Model]) {
    models.sort_by_cached_key(|model| {
        let (family, version) = family_and_version(&model.id);
        (family, std::cmp::Reverse(version))
    });
}

#[cfg(test)]
#[test]
fn settings_models_keep_manifest_and_full_ladder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        serde_json::json!({"model":"gateway/model", "env": {
            "ANTHROPIC_MODEL":"gateway/model", "ANTHROPIC_SMALL_FAST_MODEL":"fast",
            "ANTHROPIC_DEFAULT_FABLE_MODEL":"gateway/astra",
            "ANTHROPIC_DEFAULT_OPUS_MODEL":"opus", "ANTHROPIC_DEFAULT_SONNET_MODEL":"sonnet",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL":"haiku"
        }})
        .to_string(),
    )
    .unwrap();
    let models = models_with_settings(&path);
    assert_eq!(&models[..static_models().len()], static_models());
    // gateway/model, fast and gateway/astra; the opus/sonnet/haiku aliases are no rows.
    assert_eq!(models.len(), static_models().len() + 3);
    assert!(models.iter().any(|model| model.id == "gateway/astra"));
    for alias in ["opus", "sonnet", "haiku"] {
        assert!(models.iter().all(|model| model.id != alias));
    }
    for model in &models[static_models().len()..] {
        assert_eq!(model.label, model.id);
        assert_eq!(model.reasoning_levels, FULL_LADDER);
    }
    std::fs::write(&path, "invalid").unwrap();
    assert_eq!(models_with_settings(&path), static_models());
}

/// Append new concrete model IDs and fill ID-only labels on existing rows;
/// curated metadata remains authoritative.
pub(super) fn with_discovered_models(
    mut models: Vec<Model>,
    response: &serde_json::Value,
) -> Result<Vec<Model>, crate::HarnessError> {
    fn entry_text<'a>(entry: &'a serde_json::Value, key: &str) -> Option<&'a str> {
        entry
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    }

    let entries = response
        .get("response")
        .and_then(|v| v.get("models"))
        .and_then(serde_json::Value::as_array)
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| {
            crate::HarnessError::Protocol("Claude returned an empty model catalog".into())
        })?;
    // Alias rows can precede the concrete row and carry generic names such as
    // "Default (recommended)". Prefer concrete names regardless of entry order.
    let mut concrete_labels = std::collections::HashMap::new();
    for entry in entries {
        if let Some(id) = entry_text(entry, "value")
            && entry_text(entry, "resolvedModel").is_none_or(|resolved| resolved == id)
            && let Some(label) = entry_text(entry, "displayName")
        {
            concrete_labels.entry(id).or_insert(label);
        }
    }
    let mut default = None;
    let mut valid = false;
    for entry in entries {
        let text = |key: &str| entry_text(entry, key);
        let Some(id) = text("resolvedModel").or_else(|| text("value")) else {
            continue;
        };
        let label = concrete_labels
            .get(id)
            .copied()
            .or_else(|| text("displayName"));
        // Bare aliases cannot become persisted selections. Prefer resolvedModel.
        if is_alias(id) {
            continue;
        }
        valid = true;
        // The CLI lists some models by dated snapshot ("claude-haiku-4-5-20251001");
        // fold those into the curated undated row instead of duplicating it.
        let id = snapshot_base(id)
            .filter(|base| models.iter().any(|model| model.id == *base))
            .unwrap_or(id);
        if text("value") == Some("default") {
            default = Some(id.to_owned());
        }
        if let Some(model) = models.iter_mut().find(|model| model.id == id) {
            if model.label == model.id
                && let Some(label) = label
            {
                model.label = label.into();
            }
            continue;
        }
        let mut ladder = Vec::new();
        for effort in entry
            .get("supportedEffortLevels")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
        {
            let level = match effort {
                "low" => ReasoningLevel::Low,
                "medium" => ReasoningLevel::Medium,
                "high" => ReasoningLevel::High,
                "xhigh" => ReasoningLevel::XHigh,
                "max" => ReasoningLevel::Max,
                _ => continue,
            };
            if !ladder.contains(&level) {
                ladder.push(level);
            }
        }
        if ladder.contains(&ReasoningLevel::XHigh) {
            ladder.extend([ReasoningLevel::Ultracode, ReasoningLevel::Ultrathink]);
        }
        models.push(Model {
            default: false,
            id: id.into(),
            label: label.unwrap_or(id).into(),
            description: text("description").map(str::to_owned),
            reasoning_levels: ladder,
            options: vec![],
        });
    }
    if !valid {
        return Err(crate::HarnessError::Protocol(
            "Claude returned an empty model catalog".into(),
        ));
    }
    // The CLI's default is marked, not moved: rows stay best-first and
    // clients preselect the marked row. Pickers fold a long-context row into
    // its listed base, so a `[1m]` default marks that base.
    let default = default.map(|id| match id.strip_suffix("[1m]") {
        Some(base) if models.iter().any(|m| m.id == base) => base.to_owned(),
        _ => id,
    });
    if let Some(default) = default
        && models.iter().any(|m| m.id == default)
    {
        for model in &mut models {
            model.default = model.id == default;
        }
    }
    sort_best_first(&mut models);
    Ok(models)
}

/// `claude-haiku-4-5-20251001` -> `claude-haiku-4-5`.
fn snapshot_base(id: &str) -> Option<&str> {
    let (base, date) = id.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit())).then_some(base)
}

/// Written best-first (`sort_best_first` keeps it so). Options follow what
/// the CLI's initialize reports per model (`supportsFastMode`,
/// `supportsThinkingOff`); it reports no 1M flag, so only the rows that
/// already carried the context-window select keep it.
pub fn static_models() -> Vec<Model> {
    let mut models = vec![
        model(
            "claude-fable-5-2",
            "Fable 5.2",
            "For your toughest challenges",
            FULL_LADDER,
            vec![toggle("fastMode", "Fast Mode")],
        ),
        model(
            "claude-fable-5-1",
            "Fable 5.1",
            "Previous generation Fable",
            FULL_LADDER,
            vec![context_window()],
        ),
        model(
            "claude-fable-5",
            "Fable 5",
            "Older generation Fable",
            FULL_LADDER,
            vec![context_window()],
        ),
        model(
            "claude-opus-5-5",
            "Opus 5.5",
            "Best for everyday, complex tasks",
            FULL_LADDER,
            vec![context_window(), toggle("fastMode", "Fast Mode")],
        ),
        model(
            "claude-opus-4-8",
            "Opus 4.8",
            "Previous generation Opus",
            FULL_LADDER,
            vec![toggle("fastMode", "Fast Mode")],
        ),
        model(
            "claude-opus-4-7",
            "Opus 4.7",
            "Older generation Opus",
            XHIGH_LADDER,
            vec![toggle("fastMode", "Fast Mode")],
        ),
        model(
            "claude-sonnet-5-5",
            "Sonnet 5.5",
            "Most efficient for simpler tasks",
            XHIGH_LADDER,
            vec![],
        ),
        model(
            "claude-sonnet-5",
            "Sonnet 5",
            "Previous generation Sonnet",
            XHIGH_LADDER,
            vec![context_window()],
        ),
        model(
            "claude-haiku-5-5",
            "Haiku 5.5",
            "Fastest for quick answers",
            XHIGH_LADDER,
            vec![],
        ),
        model(
            "claude-haiku-4-5",
            "Haiku 4.5",
            "Previous generation Haiku",
            &[],
            vec![toggle("thinking", "Thinking")],
        ),
    ];
    // The CLI's default as of 2.1.322, for when discovery is unavailable;
    // a live catalog marks the CLI's own default instead.
    for model in &mut models {
        model.default = model.id == "claude-opus-5-5";
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_names_complete_configured_models_without_replacing_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::json!({"model":"gateway/sol", "env": {
                "ANTHROPIC_DEFAULT_FABLE_MODEL":"gateway/astra",
                "ANTHROPIC_DEFAULT_OPUS_MODEL":"gateway/sol",
                "ANTHROPIC_DEFAULT_SONNET_MODEL":"gateway/terra",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL":"gateway/luna"
            }})
            .to_string(),
        )
        .unwrap();
        let configured = models_with_settings(&path);
        let response = serde_json::json!({"response":{"models":[
            {"value":"default", "resolvedModel":"gateway/sol", "displayName":"Default (recommended)", "supportedEffortLevels":["low"]},
            {"value":"gateway/sol", "displayName":"  GPT Sol  "},
            {"value":"gateway/sol", "displayName":"Another Sol label"},
            {"value":"gateway/terra", "displayName":"GPT Terra"},
            {"value":"gateway/luna", "displayName":"GPT Luna"},
            {"value":"gateway/astra", "displayName":"GPT Astra"},
            {"value":"gateway/new", "displayName":"New gateway model"},
            {"value":"claude-opus-5-5", "displayName":"Opus", "supportedEffortLevels":["low"]}
        ]}});
        let models = with_discovered_models(configured.clone(), &response).unwrap();
        for (id, label) in [
            ("gateway/sol", "GPT Sol"),
            ("gateway/terra", "GPT Terra"),
            ("gateway/luna", "GPT Luna"),
            ("gateway/astra", "GPT Astra"),
        ] {
            let mut expected = configured.iter().find(|m| m.id == id).unwrap().clone();
            expected.label = label.into();
            expected.default = id == "gateway/sol";
            assert_eq!(models.iter().find(|m| m.id == id), Some(&expected));
            assert_eq!(models.iter().filter(|m| m.id == id).count(), 1);
        }
        let defaults: Vec<_> = models
            .iter()
            .filter(|m| m.default)
            .map(|m| &*m.id)
            .collect();
        assert_eq!(defaults, ["gateway/sol"]);
        assert_eq!(
            models.iter().find(|m| m.id == "gateway/new").unwrap().label,
            "New gateway model"
        );
        for mut curated in static_models() {
            curated.default = false;
            assert_eq!(models.iter().find(|m| m.id == curated.id), Some(&curated));
        }
    }

    #[test]
    fn concrete_names_take_priority_over_alias_names_in_either_order() {
        let alias = serde_json::json!({
            "value":"default", "resolvedModel":"gateway/sol",
            "displayName":"Default (recommended)"
        });
        let concrete = serde_json::json!({
            "value":"gateway/sol", "resolvedModel":"gateway/sol",
            "displayName":"GPT Sol"
        });
        for entries in [
            vec![alias.clone(), concrete.clone()],
            vec![concrete, alias.clone()],
        ] {
            let response = serde_json::json!({"response":{"models":entries}});
            for configured in [
                vec![],
                vec![model("gateway/sol", "gateway/sol", "", FULL_LADDER, vec![])],
            ] {
                let models = with_discovered_models(configured, &response).unwrap();
                assert_eq!(models.len(), 1);
                assert_eq!(models[0].id, "gateway/sol");
                assert_eq!(models[0].label, "GPT Sol");
            }
        }
        // An alias still provides a fallback name when no concrete name exists.
        let response = serde_json::json!({"response":{"models":[
            alias,
            {"value":"gateway/sol", "displayName":"  "}
        ]}});
        let models = with_discovered_models(vec![], &response).unwrap();
        assert_eq!(models[0].label, "Default (recommended)");
    }

    #[test]
    fn missing_discovered_names_preserve_the_configured_id_label() {
        let configured = vec![model(
            "gateway/model",
            "gateway/model",
            "",
            FULL_LADDER,
            vec![],
        )];
        for entry in [
            serde_json::json!({"value":"gateway/model"}),
            serde_json::json!({"value":"gateway/model", "displayName":"  "}),
            serde_json::json!({"value":"gateway/model", "displayName":null}),
        ] {
            let response = serde_json::json!({"response":{"models":[entry]}});
            assert_eq!(
                with_discovered_models(configured.clone(), &response).unwrap(),
                configured
            );
        }
    }

    /// The models of a real `initialize` answer (CLI 2.1.322, Nov 2026).
    fn initialize_2_1_322() -> serde_json::Value {
        serde_json::from_str(include_str!("testdata/initialize-models-2.1.322.json")).unwrap()
    }

    fn ids(models: &[Model]) -> Vec<&str> {
        models.iter().map(|m| &*m.id).collect()
    }

    #[test]
    fn a_real_catalog_lists_best_first_and_marks_the_cli_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"model":"sonnet"}"#).unwrap();
        let models =
            with_discovered_models(models_with_settings(&path), &initialize_2_1_322()).unwrap();
        assert_eq!(
            ids(&models),
            [
                "claude-fable-5-2",
                "claude-fable-5-1",
                "claude-fable-5",
                "claude-opus-5-5",
                "claude-opus-5",
                "claude-opus-4-8",
                "claude-opus-4-7",
                "claude-opus-4-6",
                "claude-sonnet-5-5",
                "claude-sonnet-5",
                "claude-sonnet-4-6",
                "claude-haiku-5-5",
                "claude-haiku-4-5",
            ]
        );
        let defaults: Vec<_> = models
            .iter()
            .filter(|m| m.default)
            .map(|m| &*m.id)
            .collect();
        assert_eq!(defaults, ["claude-opus-5-5"]);
        // Curated rows keep their curated metadata.
        let fable = &models[0];
        assert_eq!(fable.label, "Fable 5.2");
        assert_eq!(fable.reasoning_levels, FULL_LADDER);
        assert_eq!(ids_of_options(fable), ["fastMode"]);
        assert_eq!(
            models[1].description.as_deref(),
            Some("Previous generation Fable")
        );
    }

    fn ids_of_options(model: &Model) -> Vec<&str> {
        model.options.iter().map(|o| &*o.id).collect()
    }

    #[test]
    fn the_curated_list_is_already_best_first_with_one_default() {
        let models = static_models();
        let mut sorted = models.clone();
        sort_best_first(&mut sorted);
        assert_eq!(sorted, models);
        assert_eq!(models.iter().filter(|m| m.default).count(), 1);
    }

    #[test]
    fn order_is_by_family_then_newest_version_with_others_after() {
        let mut models: Vec<_> = [
            "gateway/sol",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-5",
            "claude-opus-5-5[1m]",
            "claude-fable-5-2",
            "claude-opus-5-5",
            "claude-3-5-sonnet-20241022",
            "claude-fable-5-3",
            "claude-opus-10",
            "claude-fable-5",
            "custom-model",
            "claude-sonnet-5-5",
        ]
        .into_iter()
        .map(|id| model(id, id, "", &[], vec![]))
        .collect();
        sort_best_first(&mut models);
        assert_eq!(
            ids(&models),
            [
                // A future release sorts first with no curated row.
                "claude-fable-5-3",
                "claude-fable-5-2",
                "claude-fable-5",
                "claude-opus-10",
                // A long-context row stays beside its base, in list order.
                "claude-opus-5-5[1m]",
                "claude-opus-5-5",
                "claude-sonnet-5-5",
                "claude-sonnet-5",
                "claude-3-5-sonnet-20241022",
                "claude-haiku-4-5-20251001",
                // Unknown families keep their relative order, last.
                "gateway/sol",
                "custom-model",
            ]
        );
    }

    #[test]
    fn settings_aliases_are_no_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for alias in [
            "sonnet", "opus", "haiku", "fable", "default", "opus[1m]", " sonnet ",
        ] {
            std::fs::write(
                &path,
                serde_json::json!({"model": alias, "env": {"ANTHROPIC_MODEL": alias}}).to_string(),
            )
            .unwrap();
            assert_eq!(models_with_settings(&path), static_models(), "{alias}");
        }
        // A concrete id from settings is a row, in its best-first place.
        std::fs::write(&path, r#"{"model":"claude-opus-4-1"}"#).unwrap();
        let models = models_with_settings(&path);
        let at = |id: &str| models.iter().position(|m| m.id == id).unwrap();
        assert!(at("claude-opus-4-7") < at("claude-opus-4-1"));
        assert!(at("claude-opus-4-1") < at("claude-sonnet-5-5"));
    }

    #[test]
    fn a_long_context_default_marks_its_listed_base() {
        let response = serde_json::json!({"response":{"models":[
            {"value":"default", "resolvedModel":"claude-opus-5-5[1m]", "displayName":"Default"},
            {"value":"claude-fable-5-2", "displayName":"Fable 5.2"},
        ]}});
        let models = with_discovered_models(static_models(), &response).unwrap();
        assert_eq!(models[0].id, "claude-fable-5-2");
        let defaults: Vec<_> = models
            .iter()
            .filter(|m| m.default)
            .map(|m| &*m.id)
            .collect();
        assert_eq!(defaults, ["claude-opus-5-5"]);
        let at = |id: &str| models.iter().position(|m| m.id == id).unwrap();
        assert_eq!(at("claude-opus-5-5") + 1, at("claude-opus-5-5[1m]"));
        // With no base listed, the long-context row itself is the default.
        let models = with_discovered_models(vec![], &response).unwrap();
        let defaults: Vec<_> = models
            .iter()
            .filter(|m| m.default)
            .map(|m| &*m.id)
            .collect();
        assert_eq!(defaults, ["claude-opus-5-5[1m]"]);
    }

    #[test]
    fn effort_maps_special_modes() {
        assert_eq!(to_effort(None, None), None);
        assert_eq!(to_effort(Some(ReasoningLevel::Ultrathink), None), None);
        assert_eq!(
            to_effort(Some(ReasoningLevel::Minimal), Some("claude-fable-5")),
            Some("low")
        );
        assert_eq!(
            to_effort(Some(ReasoningLevel::Ultra), Some("claude-fable-5")),
            Some("max")
        );
        // ultracode -> xhigh where supported…
        assert_eq!(
            to_effort(Some(ReasoningLevel::Ultracode), Some("claude-fable-5")),
            Some("xhigh")
        );
        // …and xhigh clamps to max elsewhere.
        assert_eq!(
            to_effort(Some(ReasoningLevel::XHigh), Some("claude-opus-4-5")),
            Some("max")
        );
        assert_eq!(to_effort(Some(ReasoningLevel::XHigh), None), Some("max"));
    }

    #[test]
    fn dated_snapshots_fold_into_curated_rows() {
        let response = serde_json::json!({"response": {"models": [
            {"value": "claude-haiku-4-5-20251001", "resolvedModel": "claude-haiku-4-5-20251001",
             "displayName": "Haiku 4.5", "description": "Fastest for quick answers"},
            {"value": "claude-haiku-9-9-20990101", "displayName": "Haiku 9.9"},
        ]}});
        let models = with_discovered_models(static_models(), &response).unwrap();
        assert_eq!(models.iter().filter(|m| m.label == "Haiku 4.5").count(), 1);
        // Snapshots with no curated base still surface as-is.
        assert!(models.iter().any(|m| m.id == "claude-haiku-9-9-20990101"));
    }

    #[test]
    fn xhigh_family_matching() {
        assert!(supports_xhigh("claude-fable-5"));
        assert!(supports_xhigh("claude-fable-5-1"));
        assert!(supports_xhigh("claude-opus-5"));
        assert!(supports_xhigh("claude-opus-5-5"));
        assert!(supports_xhigh("claude-opus-5-5[1m]"));
        assert!(supports_xhigh("claude-opus-4-7-20260101"));
        assert!(!supports_xhigh("claude-opus-4-5"));
        assert!(!supports_xhigh("claude-sonnet-4-5"));
    }

    #[test]
    fn ultrathink_preserves_leading_commands_and_arguments() {
        for command in ["/compact", "/review focus on tests"] {
            for prefix in ["", " ", "   ", "\n", "\r\n  "] {
                let text = format!("{prefix}{command}");
                assert_eq!(
                    apply_ultrathink(Some(ReasoningLevel::Ultrathink), &text),
                    text
                );
            }
        }
        for literal in [
            "    /compact",
            "\t/compact",
            "\n    /compact",
            "\u{a0}/compact",
        ] {
            assert_eq!(
                apply_ultrathink(Some(ReasoningLevel::Ultrathink), literal),
                format!("{ULTRATHINK_PREFIX}{literal}")
            );
        }
    }

    #[test]
    fn ultrathink_prefixes_prompt() {
        assert_eq!(
            apply_ultrathink(Some(ReasoningLevel::Ultrathink), "do it"),
            "Ultrathink:\ndo it"
        );
        assert_eq!(
            apply_ultrathink(Some(ReasoningLevel::Max), "do it"),
            "do it"
        );
    }
}
