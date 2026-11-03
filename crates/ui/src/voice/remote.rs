use crate::state::EngineHandle;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use zeron_proto::voice::{remote::Sdp, *};
use zeron_voice_session::VoiceMediaEndpoint;

struct Media(zeron_voice_media::DesktopMedia);
#[async_trait::async_trait]
impl VoiceMediaEndpoint for Media {
    async fn prepare(&self) -> Result<(), VoiceRejection> {
        super::permissions::microphone().await?;
        self.0.prepare().await
    }
    async fn offer(&self) -> Result<Sdp, VoiceRejection> {
        self.0.offer().await
    }
    async fn apply_answer(&self, answer: Sdp) -> Result<(), VoiceRejection> {
        self.0.apply_answer(answer).await
    }
    async fn set_muted(&self, muted: bool) -> Result<(), VoiceRejection> {
        self.0.set_muted(muted).await
    }
    async fn levels(&self) -> Result<(u16, u16), VoiceRejection> {
        self.0.levels().await
    }
    fn close(&self) {
        self.0.close();
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    engine: EngineHandle,
    host: String,
    config: zeron_proto::ChatConfig,
    voice: Option<String>,
    cancel: CancellationToken,
    events: mpsc::Sender<VoiceEvent>,
    mut controls: mpsc::Receiver<super::VoiceControl>,
) -> Result<(), VoiceRejection> {
    let client = Arc::new(
        engine
            .media_client()
            .await
            .map_err(|_| VoiceRejection::Protocol)?,
    );
    let control = Arc::new(zeron_voice_session::RpcTransport { client, host });
    // Audio runs here through this device's own standalone Codex helper; the
    // call's app-server runs on `host`. Resolution may consult the login shell.
    let codex = tokio::task::spawn_blocking(zeron_harness::codex::resolve_codex_executable)
        .await
        .ok()
        .flatten();
    let media = Arc::new(Media(zeron_voice_media::DesktopMedia::for_codex(
        codex.as_deref(),
    )?));
    let (mute_tx, muted) = watch::channel(false);
    let call =
        zeron_voice_session::run(control, media, config, voice, cancel.clone(), events, muted);
    tokio::pin!(call);
    loop {
        tokio::select! {biased;
            result=&mut call=>return result,
            command=controls.recv()=>match command {
                Some(super::VoiceControl::Mute(value))=>{mute_tx.send_replace(value);},
                None=>{cancel.cancel();return (&mut call).await;},
            }
        }
    }
}
