use super::*;
use zeron_proto::voice::{VoiceRejection, remote as wire};

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::VOICE_CAPABILITIES_V2
            | methods::PREPARE_VOICE_V2
            | methods::OWN_VOICE_V2
            | methods::NEGOTIATE_VOICE_V2
            | methods::CONFIRM_VOICE_MEDIA_V2
            | methods::REPORT_VOICE_MEDIA_V2
            | methods::STOP_VOICE_V2
            | methods::CANCEL_VOICE_ATTEMPT_V2
    )
}
fn failure(reason: VoiceRejection) -> RpcError {
    RpcError::Failed(format!("voice unavailable: {reason:?}"))
}

impl EngineRpc {
    pub(super) async fn voice_remote(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let envelope: wire::Envelope<serde_json::Value> = parse_params(params)?;
        if envelope.target_device_id != self.engine_info.device_id {
            return Err(failure(VoiceRejection::RemoteHost));
        }
        let p = envelope.payload;
        match method {
            methods::VOICE_CAPABILITIES_V2 => RpcReply::value(&wire::Capabilities {
                protocol: wire::CAPABILITY.into(),
                client_webrtc: true,
                voices: vec![],
            }),
            methods::PREPARE_VOICE_V2 => {
                let request: wire::Prepare = parse_params(p)?;
                if request.config.harness != HarnessId::Codex {
                    return Err(failure(VoiceRejection::WrongHarness));
                }
                if request.voice.as_ref().is_some_and(|v| v.len() > 128) {
                    return Err(failure(VoiceRejection::Protocol));
                }
                let (mut rx, start) = self.voice.begin_attempt(&request).map_err(failure)?;
                if let Some(cancel) = start {
                    let engine = self.clone();
                    tokio::spawn(async move {
                        let key = request.attempt_key.clone();
                        let mut chat = None;
                        let result = tokio::select! {biased;
                            _=cancel.cancelled()=>Err(VoiceRejection::InvalidLease),
                            r=tokio::time::timeout(Duration::from_secs(60),engine.prepare_remote_voice(request,&mut chat,cancel.clone()))=>r.unwrap_or(Err(VoiceRejection::Protocol)),
                        };
                        if let Some(chat) = chat.filter(|chat| {
                            result.is_err() && engine.workspace.chat(chat).ok().flatten().is_some()
                        }) {
                            engine.voice.retire_chat(&chat);
                            // The chat outlives calls: delegated work it still
                            // runs survives this failed start.
                            if !engine.sessions.turn_in_flight(&chat) {
                                let _ = engine.sessions.interrupt(&chat).await;
                            }
                            // Preserve any actual activity; remove only this empty preparation.
                            if engine
                                .doc_host
                                .open(&chat)
                                .ok()
                                .is_some_and(|d| d.watch_messages().borrow().entries.is_empty())
                            {
                                let _ = engine.workspace.delete_chat(&chat);
                            }
                        }
                        engine.voice.complete_attempt(&key, result);
                    });
                }
                loop {
                    if let Some(result) = rx.borrow().clone() {
                        let prepared = result.map_err(failure)?;
                        self.voice
                            .validate_remote(&prepared.lease)
                            .map_err(failure)?;
                        return RpcReply::value(&prepared);
                    }
                    rx.changed()
                        .await
                        .map_err(|_| failure(VoiceRejection::Protocol))?;
                }
            }
            methods::OWN_VOICE_V2 => {
                let lease: wire::Lease = parse_params(p)?;
                let owner = self.voice.own_remote(lease).map_err(failure)?;
                Ok(RpcReply::Stream(
                    futures::stream::unfold(owner, |mut owner| async move {
                        owner
                            .next()
                            .await
                            .map(|e| (serde_json::to_value(e).unwrap(), owner))
                    })
                    .boxed(),
                ))
            }
            methods::NEGOTIATE_VOICE_V2 => RpcReply::value(
                &self
                    .voice
                    .negotiate_remote(parse_params(p)?)
                    .await
                    .map_err(failure)?,
            ),
            methods::CONFIRM_VOICE_MEDIA_V2 => RpcReply::value(
                &self
                    .voice
                    .confirm_remote(&parse_params(p)?)
                    .map_err(failure)?,
            ),
            methods::REPORT_VOICE_MEDIA_V2 => RpcReply::value(
                &self
                    .voice
                    .report_remote(&parse_params(p)?)
                    .map_err(failure)?,
            ),
            methods::STOP_VOICE_V2 => {
                self.voice.stop_remote(&parse_params(p)?).map_err(failure)?;
                RpcReply::value(&serde_json::json!({"ok":true}))
            }
            methods::CANCEL_VOICE_ATTEMPT_V2 => {
                let p: wire::Cancel = parse_params(p)?;
                self.voice.cancel_attempt(&p.attempt_key).map_err(failure)?;
                RpcReply::value(&serde_json::json!({"ok":true}))
            }
            _ => Err(RpcError::UnknownMethod(method.into())),
        }
    }
    async fn prepare_remote_voice(
        &self,
        p: wire::Prepare,
        resolved: &mut Option<String>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<wire::Prepared, VoiceRejection> {
        let _preparation = self.voice.preparation().await;
        let epoch = self.voice.identity_epoch();
        if self.voice.busy() {
            return Err(VoiceRejection::Busy);
        }
        if cancel.is_cancelled() {
            return Err(VoiceRejection::InvalidLease);
        }
        let chat = self.voice_chat(p.config).await?;
        *resolved = Some(chat.clone());
        let (bridge, events, active) = self.prepare_voice_bridge(&chat).await?;
        let eligibility = bridge.probe_external().await?;
        if p.voice
            .as_ref()
            .is_some_and(|v| !eligibility.voices.contains(v))
        {
            return Err(VoiceRejection::Unsupported);
        }
        if cancel.is_cancelled() {
            return Err(VoiceRejection::InvalidLease);
        }
        let expected = self
            .workspace
            .chat(&chat)
            .map_err(|_| VoiceRejection::Protocol)?
            .ok_or(VoiceRejection::InvalidLease)?;
        let doc = self
            .doc_host
            .open(&chat)
            .map_err(|_| VoiceRejection::Protocol)?;
        self.voice
            .connect_remote(
                self.engine_info.device_id.clone(),
                chat,
                p.voice,
                eligibility.voices,
                bridge,
                events,
                active,
                doc,
                self.sessions.clone(),
                self.workspace.clone(),
                expected,
                epoch,
                cancel,
            )
            .await
    }
}
