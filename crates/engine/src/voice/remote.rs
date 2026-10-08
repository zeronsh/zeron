//! Client-owned media: host state contains signaling and canonical text only.
use super::*;
use tokio::sync::watch;
use zeron_proto::voice::remote as wire;

type PreparedResult = Option<Result<wire::Prepared, VoiceRejection>>;
pub(crate) struct Attempt {
    request: Option<wire::Prepare>,
    created: Instant,
    pub cancel: CancellationToken,
    result: watch::Sender<PreparedResult>,
}
struct Negotiation {
    id: wire::AttemptKey,
    offer: wire::Sdp,
    result: watch::Sender<Option<Result<wire::Negotiated, VoiceRejection>>>,
}
pub(super) struct RemoteSlot {
    host: String,
    last_sequence: u64,
    pub last_report: Instant,
    confirm_deadline: Option<Instant>,
    negotiation: Option<Negotiation>,
    confirmed: bool,
    attempted: Arc<std::sync::atomic::AtomicBool>,
}

impl VoiceManager {
    /// Registers before spawning preparation, so cancel-before-prepare is terminal.
    pub(crate) fn begin_attempt(
        &self,
        request: &wire::Prepare,
    ) -> Result<(watch::Receiver<PreparedResult>, Option<CancellationToken>), VoiceRejection> {
        let mut attempts = self.inner.attempts.lock().unwrap();
        attempts.retain(|_, a| {
            a.created.elapsed() < Duration::from_secs(wire::ATTEMPT_TTL_SECS)
                || (!a.cancel.is_cancelled()
                    && a.result.borrow().as_ref().is_none_or(Result::is_ok))
        });
        if let Some(a) = attempts.get(&request.attempt_key) {
            if a.cancel.is_cancelled() {
                return Err(VoiceRejection::InvalidLease);
            }
            if a.request
                .as_ref()
                .is_none_or(|p| p.config != request.config || p.voice != request.voice)
            {
                return Err(VoiceRejection::Protocol);
            }
            return Ok((a.result.subscribe(), None));
        }
        if attempts.len() >= wire::MAX_ATTEMPTS {
            return Err(VoiceRejection::Overflow);
        }
        let (result, rx) = watch::channel(None);
        let cancel = CancellationToken::new();
        attempts.insert(
            request.attempt_key.clone(),
            Attempt {
                request: Some(request.clone()),
                created: Instant::now(),
                cancel: cancel.clone(),
                result,
            },
        );
        Ok((rx, Some(cancel)))
    }
    pub(crate) fn complete_attempt(
        &self,
        key: &wire::AttemptKey,
        result: Result<wire::Prepared, VoiceRejection>,
    ) {
        let mut attempts = self.inner.attempts.lock().unwrap();
        if let Some(a) = attempts.get_mut(key) {
            let result = if a.cancel.is_cancelled() {
                Err(VoiceRejection::InvalidLease)
            } else {
                result
            };
            a.result.send_replace(Some(result));
        }
    }
    pub(crate) fn cancel_attempt(&self, key: &wire::AttemptKey) -> Result<(), VoiceRejection> {
        let lease = {
            let mut attempts = self.inner.attempts.lock().unwrap();
            if let Some(a) = attempts.get_mut(key) {
                a.cancel.cancel();
                let lease = a
                    .result
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .map(|p| p.lease.voice.clone());
                a.result
                    .send_replace(Some(Err(VoiceRejection::InvalidLease)));
                lease
            } else {
                attempts.retain(|_, a| {
                    a.created.elapsed() < Duration::from_secs(wire::ATTEMPT_TTL_SECS)
                        || (!a.cancel.is_cancelled()
                            && a.result.borrow().as_ref().is_none_or(Result::is_ok))
                });
                if attempts.len() >= wire::MAX_ATTEMPTS {
                    return Err(VoiceRejection::Overflow);
                }
                let (result, _) = watch::channel(Some(Err(VoiceRejection::InvalidLease)));
                let cancel = CancellationToken::new();
                cancel.cancel();
                attempts.insert(
                    key.clone(),
                    Attempt {
                        request: None,
                        created: Instant::now(),
                        cancel,
                        result,
                    },
                );
                None
            }
        };
        if let Some(lease) = lease {
            let _ = self.stop(&lease);
        }
        Ok(())
    }
    pub(crate) fn busy(&self) -> bool {
        self.inner.slot.lock().unwrap().is_some()
    }
    fn remote_matches(s: &Slot, lease: &wire::Lease) -> bool {
        Self::matches(s, &lease.voice)
            && s.remote
                .as_ref()
                .is_some_and(|r| r.host == lease.host_device_id)
    }
    pub(crate) fn validate_remote(&self, lease: &wire::Lease) -> Result<(), VoiceRejection> {
        self.inner
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .filter(|s| Self::remote_matches(s, lease))
            .map(|_| ())
            .ok_or(VoiceRejection::InvalidLease)
    }
    pub(crate) fn own_remote(&self, lease: wire::Lease) -> Result<VoiceOwner, VoiceRejection> {
        self.validate_remote(&lease)?;
        self.own(lease.voice)
    }
    pub(crate) fn stop_remote(&self, lease: &wire::Lease) -> Result<(), VoiceRejection> {
        // An old authenticated lease is harmless once retired; never stop a successor.
        if self.validate_remote(lease).is_ok() {
            self.stop(&lease.voice)?;
        }
        Ok(())
    }
    pub(crate) fn report_remote(&self, p: &wire::Report) -> Result<wire::Ack, VoiceRejection> {
        let mut state = self.inner.slot.lock().unwrap();
        let s = state
            .as_mut()
            .filter(|s| Self::remote_matches(s, &p.lease) && s.owner_attached)
            .ok_or(VoiceRejection::InvalidLease)?;
        let r = s.remote.as_mut().unwrap();
        if p.sequence <= r.last_sequence {
            return Ok(wire::Ack {
                sequence: r.last_sequence,
            });
        }
        r.last_sequence = p.sequence;
        r.last_report = Instant::now();
        s.snapshot.muted = p.muted;
        let ack = wire::Ack {
            sequence: p.sequence,
        };
        drop(state);
        if p.state != wire::MediaState::Ready {
            self.stop(&p.lease.voice)?;
        }
        Ok(ack)
    }
    pub(crate) fn confirm_remote(
        &self,
        p: &wire::Confirm,
    ) -> Result<VoiceSnapshot, VoiceRejection> {
        let snapshot = {
            let mut state = self.inner.slot.lock().unwrap();
            let s = state
                .as_mut()
                .filter(|s| Self::remote_matches(s, &p.lease) && s.owner_attached)
                .ok_or(VoiceRejection::InvalidLease)?;
            let r = s.remote.as_mut().unwrap();
            let n = r
                .negotiation
                .as_ref()
                .filter(|n| n.id == p.negotiation_id)
                .ok_or(VoiceRejection::Protocol)?;
            if !matches!(&*n.result.borrow(), Some(Ok(_))) {
                return Err(VoiceRejection::Protocol);
            }
            if r.confirm_deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(VoiceRejection::InvalidLease);
            }
            if !r.confirmed {
                s.snapshot.muted = p.muted;
            }
            r.confirmed = true;
            r.confirm_deadline = None;
            s.snapshot.phase = VoicePhase::Active;
            s.snapshot.clone()
        };
        self.publish(
            &p.lease.voice,
            VoiceEvent::Snapshot {
                snapshot: snapshot.clone(),
            },
        )?;
        Ok(snapshot)
    }
    pub(crate) async fn negotiate_remote(
        &self,
        p: wire::Negotiate,
    ) -> Result<wire::Negotiated, VoiceRejection> {
        let mut rx = {
            let mut state = self.inner.slot.lock().unwrap();
            let s = state
                .as_mut()
                .filter(|s| Self::remote_matches(s, &p.lease) && s.owner_attached)
                .ok_or(VoiceRejection::InvalidLease)?;
            let r = s.remote.as_mut().unwrap();
            if let Some(n) = &r.negotiation {
                if n.id != p.negotiation_id || n.offer != p.offer {
                    return Err(VoiceRejection::Protocol);
                }
                n.result.subscribe()
            } else {
                let (result, rx) = watch::channel(None);
                r.negotiation = Some(Negotiation {
                    id: p.negotiation_id.clone(),
                    offer: p.offer.clone(),
                    result: result.clone(),
                });
                r.attempted.store(true, Ordering::Release);
                let bridge = s.provider.clone().ok_or(VoiceRejection::Protocol)?;
                let voice = s.snapshot.voice.clone();
                let cancel = s.cancel.clone();
                let manager = self.clone();
                let p = p.clone();
                tokio::spawn(async move {
                    let operation = async {
                        let (reply, answer) = tokio::sync::oneshot::channel();
                        bridge
                            .commands
                            .send(VoiceCommand::External {
                                voice,
                                session_id: p.lease.voice.session_id.clone(),
                                generation: p.lease.voice.generation,
                                offer: p.offer,
                                reply,
                            })
                            .await
                            .map_err(|_| VoiceRejection::Protocol)?;
                        let answer = answer.await.map_err(|_| VoiceRejection::Protocol)??;
                        manager.validate_remote(&p.lease)?;
                        {
                            let mut state = manager.inner.slot.lock().unwrap();
                            if let Some(r) = state
                                .as_mut()
                                .filter(|s| Self::remote_matches(s, &p.lease))
                                .and_then(|s| s.remote.as_mut())
                            {
                                r.confirm_deadline = Some(Instant::now() + Duration::from_secs(30));
                            }
                        }
                        Ok(wire::Negotiated {
                            negotiation_id: p.negotiation_id,
                            answer,
                        })
                    };
                    let outcome = tokio::select! {biased;
                        _=cancel.cancelled()=>Err(VoiceRejection::InvalidLease),
                        r=tokio::time::timeout(Duration::from_secs(90),operation)=>r.unwrap_or(Err(VoiceRejection::Protocol)),
                    };
                    if outcome.is_err() {
                        let _ = manager.finish(&p.lease.voice, outcome.as_ref().err().copied());
                    }
                    result.send_replace(Some(outcome));
                });
                rx
            }
        };
        loop {
            if let Some(result) = rx.borrow().clone() {
                return result;
            }
            rx.changed().await.map_err(|_| VoiceRejection::Protocol)?;
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn connect_remote(
        &self,
        host: String,
        chat: String,
        voice: Option<String>,
        voices: Vec<String>,
        bridge: RealtimeHandle,
        mut events: tokio::sync::broadcast::Receiver<VoiceEvent>,
        active: Arc<crate::sessions::VoiceActivity>,
        doc: Arc<crate::doc_host::ChatDocHandle>,
        sessions: crate::sessions::SessionsEngine,
        workspace: crate::workspace_host::WorkspaceHost,
        expected: zeron_proto::Chat,
        epoch: u64,
        attempt: CancellationToken,
    ) -> Result<wire::Prepared, VoiceRejection> {
        let lifecycle = self.inner.native_lifecycle.clone().lock_owned().await;
        if attempt.is_cancelled() {
            return Err(VoiceRejection::InvalidLease);
        }
        let lease = self.reserve_with_deadline(&chat, epoch, 15)?;
        let attempted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = {
            let mut state = self.inner.slot.lock().unwrap();
            let s = state
                .as_mut()
                .filter(|s| Self::matches(s, &lease))
                .ok_or(VoiceRejection::InvalidLease)?;
            s.remote = Some(RemoteSlot {
                host: host.clone(),
                last_sequence: 0,
                last_report: Instant::now(),
                confirm_deadline: None,
                negotiation: None,
                confirmed: false,
                attempted: attempted.clone(),
            });
            s.provider = Some(bridge.clone());
            s.snapshot.voice = voice;
            s.snapshot.voices = voices.clone();
            s.cancel.clone()
        };
        let prepared = wire::Prepared {
            lease: wire::Lease {
                host_device_id: host,
                voice: lease.clone(),
            },
            chat_id: chat.clone(),
            voices,
            heartbeat_seconds: wire::HEARTBEAT_SECS,
            lease_seconds: wire::LEASE_SECS,
        };
        let manager = self.clone();
        let call = active.call();
        tokio::spawn(async move {
            let _call = call;
            let _lifecycle = lifecycle;
            let mut ticker = tokio::time::interval(Duration::from_millis(250));
            let mut statuses = sessions.watch_sessions();
            let result=async {loop {tokio::select!{biased;
                _=cancel.cancelled()=>return Ok(()),
                _=attempt.cancelled()=>return Err(VoiceRejection::InvalidLease),
                _=ticker.tick()=>{
                    if bridge.invalidated(){return Err(VoiceRejection::InvalidLease);}
                    if workspace.chat(&chat).ok().flatten().is_none_or(|c|c.device_id!=expected.device_id || c.config!=expected.config || c.cwd!=expected.cwd){return Err(VoiceRejection::InvalidLease);}
                    let state=manager.inner.slot.lock().unwrap();
                    let s=state.as_ref().filter(|s|Self::matches(s,&lease)).ok_or(VoiceRejection::InvalidLease)?;
                    let r=s.remote.as_ref().unwrap();
                    if (s.owner_attached && r.last_report.elapsed()>=Duration::from_secs(wire::LEASE_SECS)) || r.confirm_deadline.is_some_and(|d|Instant::now()>=d){return Err(VoiceRejection::InvalidLease);}
                },
                changed=statuses.changed()=>{
                    changed.map_err(|_|VoiceRejection::Protocol)?;
                    let work=match sessions.session_status(&chat).map(|s|s.status){Some(SessionStatus::Working)=>VoiceWork::Working,Some(SessionStatus::AwaitingInput)=>VoiceWork::AwaitingInput,_=>VoiceWork::Idle};
                    manager.update(&lease,|s|s.work=work)?;
                },
                event=events.recv()=>{
                    let event=event.map_err(|_|VoiceRejection::Overflow)?;
                    match &event {
                        VoiceEvent::Final{transcript}=>{
                            if transcript.session_id!=lease.session_id{continue;}
                            if doc.commit_voice(transcript).map_err(|_|VoiceRejection::Protocol)?.is_some(){workspace.note_message(&chat,&transcript.text);}
                        },
                        VoiceEvent::Partial{generation,..} | VoiceEvent::InvalidatePlayout{generation,..} if *generation!=lease.generation=>continue,
                        VoiceEvent::Closed{generation,reason}=>{if *generation!=lease.generation{continue;}return reason.map_or(Ok(()),Err);},
                        VoiceEvent::Audio{..} | VoiceEvent::Levels{..}=>return Err(VoiceRejection::Protocol),
                        _=>{},
                    }
                    manager.publish(&lease,event)?;
                }
            }}}.await;
            bridge.abort_voice(lease.generation);
            let _ = bridge.stop().await;
            let _ = manager.finish(&lease, result.err());
            // A terminal attempt cannot replay a successful but dead lease.
            attempt.cancel();
            let empty = doc.watch_messages().borrow().entries.is_empty();
            if !attempted.load(Ordering::Acquire) && empty {
                let _ = sessions.interrupt(&chat).await;
                let _ = workspace.delete_chat(&chat);
            }
        });
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> wire::Prepare {
        wire::Prepare {
            attempt_key: wire::AttemptKey::new(),
            config: serde_json::from_value(
                serde_json::json!({"harness":"codex","sandbox":"danger-full-access"}),
            )
            .unwrap(),
            voice: None,
        }
    }
    #[tokio::test]
    async fn cancel_before_prepare_and_duplicate_payloads_are_terminal() {
        let m = VoiceManager::default();
        let p = request();
        m.cancel_attempt(&p.attempt_key).unwrap();
        assert!(matches!(
            m.begin_attempt(&p),
            Err(VoiceRejection::InvalidLease)
        ));
        let mut p = request();
        let (_, first) = m.begin_attempt(&p).unwrap();
        assert!(first.is_some());
        let (_, duplicate) = m.begin_attempt(&p).unwrap();
        assert!(duplicate.is_none());
        p.voice = Some("different".into());
        assert!(matches!(m.begin_attempt(&p), Err(VoiceRejection::Protocol)));
    }
    #[tokio::test]
    async fn bounded_attempt_registry_never_evicts_live_preparations() {
        let m = VoiceManager::default();
        for _ in 0..wire::MAX_ATTEMPTS {
            m.begin_attempt(&request()).unwrap();
        }
        assert!(matches!(
            m.begin_attempt(&request()),
            Err(VoiceRejection::Overflow)
        ));
    }
    #[tokio::test]
    async fn reports_require_owner_and_sequence_and_old_stop_is_harmless() {
        let m = VoiceManager::default();
        let lease = m.reserve("chat").unwrap();
        {
            let mut state = m.inner.slot.lock().unwrap();
            state.as_mut().unwrap().remote = Some(RemoteSlot {
                host: "host".into(),
                last_sequence: 0,
                last_report: Instant::now(),
                confirm_deadline: None,
                negotiation: None,
                confirmed: false,
                attempted: Arc::default(),
            });
        }
        let lease = wire::Lease {
            host_device_id: "host".into(),
            voice: lease,
        };
        let mut p = wire::Report {
            lease: lease.clone(),
            sequence: 2,
            muted: true,
            state: wire::MediaState::Ready,
        };
        assert_eq!(
            m.report_remote(&p).unwrap_err(),
            VoiceRejection::InvalidLease
        );
        let owner = m.own_remote(lease.clone()).unwrap();
        assert_eq!(m.report_remote(&p).unwrap().sequence, 2);
        p.sequence = 1;
        p.muted = false;
        assert_eq!(m.report_remote(&p).unwrap().sequence, 2);
        assert!(
            m.inner
                .slot
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .snapshot
                .muted
        );
        drop(owner);
        let successor = m.reserve("next").unwrap();
        m.stop_remote(&lease).unwrap();
        assert!(m.owns_chat("next"));
        m.stop(&successor).unwrap();
    }
}
