//! A Grok voice call on this computer: prepare a chat, then speak through the
//! local Grok subscription. Tool calls and transcript commits share one media
//! connection. `RpcClient` is not `Clone`; `Zeron` keeps it in an `Arc` and
//! `call` takes `&self`, so the two overlap without a second dial.

use std::sync::Arc;

use crate::state::EngineHandle;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use zeron_proto::voice::{VoiceEvent, VoiceRejection, VoiceTranscript};

pub(super) fn tune_from(style: &super::GrokStyle) -> zeron_grok_voice::Tune {
    zeron_grok_voice::Tune {
        instructions: instructions(style.personality),
        speech: zeron_grok_voice::Speech {
            voice: style.voice.clone(),
            speed: style.speed,
        },
        input: style.input.clone(),
    }
}

pub(super) async fn run(
    engine: EngineHandle,
    config: zeron_proto::ChatConfig,
    cancel: CancellationToken,
    muted: watch::Receiver<bool>,
    tune: watch::Receiver<zeron_grok_voice::Tune>,
    events: mpsc::Sender<VoiceEvent>,
) -> Result<(), VoiceRejection> {
    super::permissions::microphone().await?;
    let prepared = engine
        .client()
        .call(
            zeron_rpc::methods::PREPARE_GROK_VOICE,
            serde_json::json!({ "config": config }),
        )
        .await
        .map_err(zeron_voice_session::rejection)?;
    let chat_id = serde_json::from_value::<zeron_proto::voice::PreparedGrokVoice>(prepared)
        .map_err(|_| VoiceRejection::Protocol)?
        .chat_id;
    if chat_id.is_empty() {
        return Err(VoiceRejection::Protocol);
    }

    let media = engine
        .media_client()
        .await
        .map_err(|_| VoiceRejection::Protocol)?;
    let zeron = Arc::new(zeron_mcp::Zeron::with_client(
        media,
        zeron_mcp::Origin {
            chat_id: Some(chat_id.clone()),
            device_id: Some(engine.engine_info().device_id.clone()),
            side_chats_only: false,
        },
    ));
    let tools = zeron_mcp::Tools::new(zeron.clone());
    let specs = tools
        .list()
        .into_iter()
        .map(|def| zeron_grok_voice::ToolSpec {
            name: def.name.to_owned(),
            description: def.description.to_owned(),
            parameters: def.input_schema,
        })
        .collect();
    let initial = tune.borrow().clone();
    let mut duplex = zeron_grok_voice::DesktopDuplex::default();
    duplex.prefer_input(initial.input.clone());
    zeron_grok_voice::run(
        zeron_grok_voice::GrokBotCredentials,
        duplex,
        GrokTools { tools },
        GrokSink {
            zeron,
            chat_id: chat_id.clone(),
        },
        zeron_grok_voice::Call {
            instructions: initial.instructions,
            tools: specs,
            chat_id,
            speech: initial.speech,
        },
        cancel,
        muted,
        tune,
        events,
    )
    .await
}

fn instructions(personality: crate::settings::GrokPersonality) -> String {
    let base = format!(
        "{}\n\nThis is a spoken call. Keep replies short. Tool results are text.",
        zeron_harness::codex::realtime::ORCHESTRATOR_INSTRUCTIONS
    );
    let Some(direction) = personality.direction() else {
        return base;
    };
    format!(
        "{base}\n\nSpeaking style: {direction} Still use tools for real work, and keep each spoken reply short."
    )
}

struct GrokTools {
    tools: zeron_mcp::Tools,
}

struct GrokSink {
    zeron: Arc<zeron_mcp::Zeron>,
    chat_id: String,
}

#[async_trait::async_trait]
impl zeron_grok_voice::ToolExec for GrokTools {
    async fn call(&self, name: &str, arguments: serde_json::Value) -> String {
        match self.tools.call(name, arguments).await {
            Ok(value) => value.to_string(),
            Err(error) => error,
        }
    }
}

#[async_trait::async_trait]
impl zeron_grok_voice::TranscriptSink for GrokSink {
    async fn commit(&self, transcript: &VoiceTranscript) {
        let Ok(params) = serde_json::to_value(zeron_proto::voice::AppendGrokVoiceTranscript {
            chat_id: self.chat_id.clone(),
            transcript: transcript.clone(),
        }) else {
            tracing::warn!("grok voice transcript was not saved");
            return;
        };
        let method = zeron_rpc::methods::APPEND_GROK_VOICE_TRANSCRIPT;
        if self.zeron.call(method, params.clone()).await.is_err()
            && self.zeron.call(method, params).await.is_err()
        {
            tracing::warn!("grok voice transcript was not saved");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::instructions;
    use crate::settings::GrokPersonality;

    #[test]
    fn personality_is_added_on_top_of_the_orchestrator_prompt() {
        let assistant = instructions(GrokPersonality::Assistant);
        assert!(assistant.contains("Keep replies short"));
        assert!(!assistant.contains("Speaking style:"));
        let story = instructions(GrokPersonality::Storyteller);
        assert!(story.starts_with(&assistant));
        assert!(story.contains("Speaking style:"));
        assert!(story.contains("Still use tools for real work"));
    }
}
