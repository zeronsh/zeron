use super::*;
use zeron_proto::voice::{self, VoiceRejection};

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::PREPARE_GROK_VOICE | methods::APPEND_GROK_VOICE_TRANSCRIPT
    )
}
fn failure(reason: VoiceRejection) -> RpcError {
    RpcError::Failed(format!("voice unavailable: {reason:?}"))
}

impl EngineRpc {
    /// Not device-forwardable: the viewport mints from this computer's Grok Bot login.
    pub(super) async fn grok_voice(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        match method {
            methods::PREPARE_GROK_VOICE => {
                let request: voice::PrepareGrokVoice = parse_params(params)?;
                if request.config.harness != HarnessId::Grok {
                    return Err(failure(VoiceRejection::WrongHarness));
                }
                let chat_id = self.voice_chat(request.config).await.map_err(failure)?;
                RpcReply::value(&voice::PreparedGrokVoice { chat_id })
            }
            methods::APPEND_GROK_VOICE_TRANSCRIPT => {
                let request: voice::AppendGrokVoiceTranscript = parse_params(params)?;
                let Some(chat) = self
                    .workspace
                    .chat(&request.chat_id)
                    .map_err(|_| failure(VoiceRejection::Protocol))?
                    .filter(|chat| {
                        voice::is_orchestrator_chat(&chat.id)
                            && chat.device_id == self.engine_info.device_id
                            && chat
                                .config
                                .as_ref()
                                .is_some_and(|cfg| cfg.harness == HarnessId::Grok)
                    })
                else {
                    return Err(failure(VoiceRejection::InvalidLease));
                };
                let doc = self
                    .doc_host
                    .open(&chat.id)
                    .map_err(|_| failure(VoiceRejection::Protocol))?;
                if doc
                    .commit_voice(&request.transcript)
                    .map_err(|_| failure(VoiceRejection::Protocol))?
                    .is_some()
                {
                    self.workspace
                        .note_message(&chat.id, &request.transcript.text);
                }
                RpcReply::value(&serde_json::json!({"ok":true}))
            }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
}
