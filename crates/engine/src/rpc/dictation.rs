use super::*;
use zeron_proto::voice::dictation::{self as wire, DictationRejection};
use zeron_proto::voice::remote::Envelope;

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::START_DICTATION_V1
            | methods::FEED_DICTATION_V1
            | methods::FINISH_DICTATION_V1
            | methods::CANCEL_DICTATION_V1
            | methods::OWN_DICTATION_V1
    )
}

fn failure(reason: DictationRejection) -> RpcError {
    RpcError::Failed(format!("dictation unavailable: {reason:?}"))
}

impl EngineRpc {
    pub(super) async fn dictation(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let envelope: Envelope<serde_json::Value> = parse_params(params)?;
        if envelope.target_device_id != self.engine_info.device_id {
            return Err(failure(DictationRejection::RemoteHost));
        }
        let p = envelope.payload;
        let ok = || RpcReply::value(&serde_json::json!({"ok":true}));
        match method {
            methods::START_DICTATION_V1 => {
                let start: wire::Start = parse_params(p)?;
                self.dictation.start(&start.session_id).map_err(failure)?;
                ok()
            }
            methods::FEED_DICTATION_V1 => {
                let chunk: wire::Chunk = parse_params(p)?;
                self.dictation.feed(&chunk).map_err(failure)?;
                ok()
            }
            methods::FINISH_DICTATION_V1 => {
                let session: wire::Session = parse_params(p)?;
                self.dictation
                    .finish(&session.session_id)
                    .map_err(failure)?;
                ok()
            }
            methods::CANCEL_DICTATION_V1 => {
                let session: wire::Session = parse_params(p)?;
                self.dictation
                    .cancel(&session.session_id)
                    .map_err(failure)?;
                ok()
            }
            methods::OWN_DICTATION_V1 => {
                let session: wire::Session = parse_params(p)?;
                let rx = self.dictation.own(&session.session_id).map_err(failure)?;
                Ok(RpcReply::Stream(
                    futures::stream::unfold(rx, |mut rx| async move {
                        rx.recv()
                            .await
                            .map(|e| (serde_json::to_value(e).unwrap(), rx))
                    })
                    .boxed(),
                ))
            }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
}
