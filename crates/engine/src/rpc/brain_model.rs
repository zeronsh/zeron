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
    /// The person's pick for the Brain dump `id`, checked against this
    /// host's catalog as a chat send's is, set on its chat (every client
    /// shows it) when it changes anything.
    pub(crate) async fn brain_dump_pick(
        &self,
        id: &str,
        pick: &zeron_proto::orchestration::DumpConfig,
    ) -> Result<(), zeron_rpc::RpcError> {
        use zeron_rpc::RpcError;
        let chat = self
            .workspace
            .chat(id)
            .map_err(|e| RpcError::Failed(e.to_string()))?
            .ok_or_else(|| RpcError::Failed("The Brain dump isn't here".into()))?;
        let mut config = chat.config.clone().unwrap_or(zeron_proto::ChatConfig {
            harness: self.doc_host.default_harness(),
            model: None,
            reasoning: None,
            model_options: Default::default(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            access: Default::default(),
            plan: false,
        });
        let before = config.clone();
        if let Some(harness) = pick.harness
            && harness != config.harness
        {
            config.harness = harness;
            // Another agent's model and options don't carry over.
            config.model = None;
            config.reasoning = None;
            config.model_options = Default::default();
        }
        let harness = self
            .registry
            .resolve(config.harness)
            .map_err(|_| RpcError::BadParams(format!("{:?} isn't available on this device", config.harness)))?;
        if pick.model.is_some() || pick.reasoning.is_some() {
            let listed = tokio::time::timeout(
                CATALOG_WAIT,
                crate::model_catalogs::list_with_lease(self.repos.data_dir(), harness, false, None),
            )
            .await;
            // (A catalog this host can't read now checks nothing.)
            if let Ok(Ok(models)) = listed {
                let model_id = pick.model.clone().or_else(|| config.model.clone());
                let model = model_id.as_deref().and_then(|m| models.iter().find(|x| x.id == m));
                if let Some(wanted) = pick.model.as_deref()
                    && model.is_none()
                {
                    return Err(RpcError::BadParams(format!(
                        "{wanted:?} isn't offered by {:?} here; available: {}",
                        config.harness,
                        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", ")
                    )));
                }
                if let (Some(level), Some(model)) = (pick.reasoning, model)
                    && !model.reasoning_levels.is_empty()
                    && !model.reasoning_levels.contains(&level)
                {
                    return Err(RpcError::BadParams(format!("{} has no {level:?} reasoning", model.id)));
                }
            }
        }
        if let Some(model) = &pick.model {
            config.model = Some(model.clone());
        }
        if let Some(level) = pick.reasoning {
            config.reasoning = Some(level);
        }
        if let Some(options) = &pick.model_options {
            config.model_options = options.clone();
        }
        if config != before {
            self.workspace
                .set_chat_config(id, &config)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
        }
        Ok(())
    }

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
            provider: None,
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
