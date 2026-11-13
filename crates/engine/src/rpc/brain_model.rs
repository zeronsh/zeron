//! The Brain dump runs on a fast model unless the person picks another: a
//! new Brain dump on Claude Code gets the catalog's fastest Claude (Haiku);
//! any other harness keeps its default.

use std::time::Duration;

use zeron_proto::{HarnessId, Model};

use super::EngineRpc;

/// How long the catalog may take before the Brain dump starts on the
/// harness default instead.
const CATALOG_WAIT: Duration = Duration::from_secs(5);

/// The fast model for `harness` from its catalog (listed best-first).
pub(crate) fn fast_model(harness: HarnessId, models: &[Model]) -> Option<String> {
    if harness != HarnessId::ClaudeCode {
        return None;
    }
    models
        .iter()
        .find(|m| m.id.to_ascii_lowercase().contains("haiku") || m.label.to_ascii_lowercase().contains("haiku"))
        .map(|m| m.id.clone())
}

impl EngineRpc {
    /// Give the Brain dump `id` its fast model, while it has none and has
    /// never run (after that, its model is the person's to choose).
    pub(crate) async fn brain_dump_fast_model(&self, id: &str) {
        let Ok(Some(chat)) = self.workspace.chat(id) else { return };
        let Some(mut config) = chat.config.clone() else { return };
        if config.model.is_some() || chat.last_message_at.is_some() {
            return;
        }
        let Ok(harness) = self.registry.resolve(config.harness) else { return };
        let listed = tokio::time::timeout(
            CATALOG_WAIT,
            crate::model_catalogs::list_with_lease(self.repos.data_dir(), harness, false, None),
        )
        .await;
        let Ok(Ok(models)) = listed else { return };
        let Some(model) = fast_model(config.harness, &models) else { return };
        config.model = Some(model);
        if let Err(error) = self.workspace.set_chat_config(id, &config) {
            tracing::warn!(%error, "brain dump: fast model not set");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, label: &str) -> Model {
        Model {
            id: id.into(),
            label: label.into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
            default: false,
        }
    }

    #[test]
    fn claude_code_dumps_route_on_haiku_and_others_on_their_default() {
        let models = vec![model("default", "Opus 5.5"), model("sonnet", "Sonnet 5"), model("haiku", "Haiku 5.5")];
        assert_eq!(fast_model(HarnessId::ClaudeCode, &models).as_deref(), Some("haiku"));
        assert_eq!(fast_model(HarnessId::ClaudeCode, &models[..2]), None);
        assert_eq!(fast_model(HarnessId::Codex, &models), None);
    }
}
