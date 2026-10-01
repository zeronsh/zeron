//! Agent presets in the composer (docs/agent-presets.md): the pure half of the
//! new-session "Agent" picker, which sets the harness, model, reasoning and
//! permission policy together.
//!
//! Rendering lives with the other pickers (`pickers.rs`).

use zeron_proto::{AgentPreset, HarnessId, PresetSource};

/// The harness (and model) a preset runs on given what this device offers:
/// its own, else the first fallback that's offered. `None` when none is.
pub fn usable(preset: &AgentPreset, offered: &[HarnessId]) -> Option<(HarnessId, Option<String>)> {
    std::iter::once((preset.harness, preset.model.clone()))
        .chain(preset.fallbacks.iter().map(|f| (f.harness, f.model.clone())))
        .find(|(harness, _)| offered.contains(harness))
}

/// What the chip says: the preset's name, or the neutral label.
pub fn chip_label(preset: Option<&AgentPreset>) -> String {
    preset.map_or_else(|| "Default agent".to_string(), |p| p.name.clone())
}

/// A small label for where a preset came from; `None` for the user's own.
pub fn source_label(source: PresetSource) -> Option<&'static str> {
    match source {
        PresetSource::User => None,
        PresetSource::Project => Some("Project"),
        PresetSource::Imported => Some("Claude Code"),
    }
}

/// Why a preset can't be picked here; `None` when it can.
pub fn unavailable(preset: &AgentPreset, offered: &[HarnessId]) -> Option<String> {
    usable(preset, offered).is_none().then(|| {
        format!(
            "{:?} isn't available on this device{}",
            preset.harness,
            if preset.fallbacks.is_empty() {
                ""
            } else {
                ", and neither are its fallbacks"
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::PresetFallback;

    fn preset(harness: HarnessId, fallbacks: &[(HarnessId, Option<&str>)]) -> AgentPreset {
        let mut preset = AgentPreset::new("Reviewer", harness);
        preset.model = Some("gpt-5".into());
        preset.fallbacks = fallbacks
            .iter()
            .map(|(harness, model)| PresetFallback {
                harness: *harness,
                model: model.map(str::to_owned),
            })
            .collect();
        preset
    }

    #[test]
    fn a_preset_uses_its_own_harness_then_the_first_offered_fallback() {
        let p = preset(
            HarnessId::Codex,
            &[
                (HarnessId::Cursor, None),
                (HarnessId::ClaudeCode, Some("sonnet")),
            ],
        );
        assert_eq!(
            usable(&p, &[HarnessId::Codex, HarnessId::ClaudeCode]),
            Some((HarnessId::Codex, Some("gpt-5".into())))
        );
        assert_eq!(
            usable(&p, &[HarnessId::ClaudeCode]),
            Some((HarnessId::ClaudeCode, Some("sonnet".into()))),
            "the fallback brings its own model"
        );
        assert_eq!(usable(&p, &[HarnessId::Pi]), None);
        assert!(unavailable(&p, &[HarnessId::Pi]).unwrap().contains("fallbacks"));
        assert_eq!(unavailable(&p, &[HarnessId::Codex]), None);
    }

    #[test]
    fn labels_read_plainly() {
        assert_eq!(chip_label(None), "Default agent");
        assert_eq!(
            chip_label(Some(&AgentPreset::new("Fast fixer", HarnessId::ClaudeCode))),
            "Fast fixer"
        );
        assert_eq!(source_label(PresetSource::User), None);
        assert_eq!(source_label(PresetSource::Project), Some("Project"));
        assert_eq!(source_label(PresetSource::Imported), Some("Claude Code"));
    }
}
