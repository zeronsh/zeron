//! Exclusive, local, ephemeral voice ownership. Native media remains in Codex's helper.
pub(crate) mod remote;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use zeron_harness::codex::realtime::{RealtimeHandle, VoiceCommand};
use zeron_proto::{SessionStatus, voice::*};

#[derive(Clone, Default)]
pub struct VoiceManager {
    inner: Arc<Inner>,
}
#[derive(Default)]
struct Inner {
    slot: Mutex<Option<Slot>>,
    attempts:
        Mutex<std::collections::HashMap<zeron_proto::voice::remote::AttemptKey, remote::Attempt>>,
    generation: AtomicU64,
    identity_epoch: AtomicU64,
    // A successor waits until the old native stop completes, even after its owner is dropped.
    preparation: tokio::sync::Mutex<()>,
    native_lifecycle: Arc<tokio::sync::Mutex<()>>,
}
struct Slot {
    lease: VoiceLease,
    snapshot: VoiceSnapshot,
    attach_deadline: Instant,
    owner_attached: bool,
    events: Option<mpsc::Receiver<VoiceEvent>>,
    sender: mpsc::Sender<VoiceEvent>,
    cancel: CancellationToken,
    provider: Option<RealtimeHandle>,
    remote: Option<remote::RemoteSlot>,
}
pub struct VoiceOwner {
    manager: VoiceManager,
    lease: VoiceLease,
    events: mpsc::Receiver<VoiceEvent>,
}
impl VoiceOwner {
    pub async fn next(&mut self) -> Option<VoiceEvent> {
        self.events.recv().await
    }
}
impl Drop for VoiceOwner {
    fn drop(&mut self) {
        let _ = self.manager.stop(&self.lease);
    }
}

impl VoiceManager {
    pub(crate) async fn preparation(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.preparation.lock().await
    }
    pub(crate) fn identity_epoch(&self) -> u64 {
        self.inner.identity_epoch.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(crate) fn reserve(&self, chat: &str) -> Result<VoiceLease, VoiceRejection> {
        self.reserve_with_deadline(chat, self.identity_epoch(), 5)
    }
    fn reserve_with_deadline(
        &self,
        chat: &str,
        epoch: u64,
        seconds: u64,
    ) -> Result<VoiceLease, VoiceRejection> {
        let mut state = self.inner.slot.lock().unwrap();
        if self.identity_epoch() != epoch {
            return Err(VoiceRejection::InvalidLease);
        }
        if state.is_some() {
            return Err(VoiceRejection::Busy);
        }
        let generation = self.inner.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let lease = VoiceLease {
            session_id: uuid::Uuid::new_v4().to_string(),
            generation,
            token: uuid::Uuid::new_v4().to_string(),
        };
        let snapshot = VoiceSnapshot {
            session_id: lease.session_id.clone(),
            chat_id: chat.into(),
            generation,
            phase: VoicePhase::Starting,
            muted: false,
            playing: false,
            work: VoiceWork::Idle,
            reason: None,
            voice: None,
            voices: Vec::new(),
        };
        let (sender, events) = mpsc::channel(32);
        let _ = sender.try_send(VoiceEvent::Snapshot {
            snapshot: snapshot.clone(),
        });
        *state = Some(Slot {
            lease: lease.clone(),
            snapshot,
            attach_deadline: Instant::now() + Duration::from_secs(seconds),
            owner_attached: false,
            events: Some(events),
            sender,
            cancel: CancellationToken::new(),
            provider: None,
            remote: None,
        });
        drop(state);
        let manager = self.clone();
        let pending = lease.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            manager.expire_unattached(&pending);
        });
        Ok(lease)
    }
    fn expire_unattached(&self, lease: &VoiceLease) {
        let expired = self.inner.slot.lock().unwrap().as_ref().is_some_and(|s| {
            Self::matches(s, lease) && !s.owner_attached && Instant::now() >= s.attach_deadline
        });
        if expired {
            let _ = self.stop(lease);
        }
    }
    fn matches(s: &Slot, l: &VoiceLease) -> bool {
        s.lease.session_id == l.session_id
            && s.lease.generation == l.generation
            && s.lease.token == l.token
    }
    pub fn own(&self, lease: VoiceLease) -> Result<VoiceOwner, VoiceRejection> {
        let mut state = self.inner.slot.lock().unwrap();
        let slot = state
            .as_mut()
            .filter(|s| Self::matches(s, &lease))
            .ok_or(VoiceRejection::InvalidLease)?;
        if slot.owner_attached || Instant::now() >= slot.attach_deadline {
            return Err(VoiceRejection::InvalidLease);
        }
        let events = slot.events.take().ok_or(VoiceRejection::InvalidLease)?;
        slot.owner_attached = true;
        if let Some(remote) = &mut slot.remote {
            remote.last_report = Instant::now();
        }
        Ok(VoiceOwner {
            manager: self.clone(),
            lease,
            events,
        })
    }
    fn publish(&self, lease: &VoiceLease, event: VoiceEvent) -> Result<(), VoiceRejection> {
        let result = {
            let state = self.inner.slot.lock().unwrap();
            let slot = state
                .as_ref()
                .filter(|s| Self::matches(s, lease))
                .ok_or(VoiceRejection::InvalidLease)?;
            slot.sender
                .try_send(event)
                .map_err(|_| VoiceRejection::Overflow)
        };
        if result.is_err() {
            let _ = self.stop(lease);
        }
        result
    }
    fn update(
        &self,
        lease: &VoiceLease,
        f: impl FnOnce(&mut VoiceSnapshot),
    ) -> Result<(), VoiceRejection> {
        let snapshot = {
            let mut state = self.inner.slot.lock().unwrap();
            let slot = state
                .as_mut()
                .filter(|s| Self::matches(s, lease))
                .ok_or(VoiceRejection::InvalidLease)?;
            f(&mut slot.snapshot);
            slot.snapshot.clone()
        };
        self.publish(lease, VoiceEvent::Snapshot { snapshot })
    }
    pub fn stop(&self, lease: &VoiceLease) -> Result<(), VoiceRejection> {
        self.finish(lease, None)
    }
    fn finish(
        &self,
        lease: &VoiceLease,
        reason: Option<VoiceRejection>,
    ) -> Result<(), VoiceRejection> {
        let mut state = self.inner.slot.lock().unwrap();
        let Some(s) = state.as_ref() else {
            return Ok(());
        };
        if !Self::matches(s, lease) {
            return Err(VoiceRejection::InvalidLease);
        }
        let s = state.take().unwrap();
        drop(state);
        Self::close_slot(s, reason);
        Ok(())
    }
    fn close_slot(s: Slot, reason: Option<VoiceRejection>) {
        s.cancel.cancel();
        if let Some(provider) = s.provider.as_ref() {
            provider.abort_voice(s.lease.generation);
        }
        let _ = s.sender.try_send(VoiceEvent::Closed {
            generation: s.lease.generation,
            reason,
        });
    }
    #[cfg(test)]
    pub(crate) fn owns_chat(&self, chat: &str) -> bool {
        self.inner
            .slot
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s.snapshot.chat_id == chat)
    }
    pub fn retire(&self) {
        let mut state = self.inner.slot.lock().unwrap();
        // The same lock covers reservation and identity changes: a pending probe
        // may never create a lease after account/profile retirement.
        self.inner.identity_epoch.fetch_add(1, Ordering::AcqRel);
        for attempt in self.inner.attempts.lock().unwrap().values() {
            attempt.cancel.cancel();
        }
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        let old = state.take();
        drop(state);
        if let Some(slot) = old {
            Self::close_slot(slot, None);
        }
    }
    pub(crate) fn retire_chat(&self, chat: &str) {
        let mut state = self.inner.slot.lock().unwrap();
        self.inner.identity_epoch.fetch_add(1, Ordering::AcqRel);
        let old = if state.as_ref().is_some_and(|s| s.snapshot.chat_id == chat) {
            state.take()
        } else {
            None
        };
        drop(state);
        if let Some(slot) = old {
            Self::close_slot(slot, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exclusive_owner_drop_and_stale_guards() {
        let manager = VoiceManager::default();
        let lease = manager.reserve("chat").unwrap();
        assert_eq!(manager.reserve("chat").unwrap_err(), VoiceRejection::Busy);
        let mut bad = lease.clone();
        bad.token = "bad".into();
        assert!(manager.own(bad).is_err());
        let owner = manager.own(lease.clone()).unwrap();
        assert!(manager.own(lease.clone()).is_err());
        drop(owner);
        let next = manager.reserve("chat").unwrap();
        assert_eq!(manager.stop(&lease), Err(VoiceRejection::InvalidLease));
        assert!(manager.own(next).is_ok());
    }
    #[tokio::test(start_paused = true)]
    async fn unattached_owner_watchdog_releases_reservation() {
        let manager = VoiceManager::default();
        let old = manager.reserve("chat").unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        assert!(manager.own(old.clone()).is_err());
        let next = manager.reserve("next").unwrap();
        assert!(next.generation > old.generation);
        let owner = manager.own(next).unwrap();
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        assert!(manager.owns_chat("next"));
        drop(owner);
        assert!(!manager.owns_chat("next"));
    }

    #[tokio::test]
    async fn retirement_rejects_a_start_prepared_under_the_old_identity() {
        let manager = VoiceManager::default();
        let epoch = manager.identity_epoch();
        manager.retire();
        assert_eq!(
            manager.reserve_with_deadline("chat", epoch, 5).unwrap_err(),
            VoiceRejection::InvalidLease
        );
        assert!(manager.reserve("chat").is_ok());
    }

    #[tokio::test]
    async fn account_retirement_invalidates_lease() {
        let manager = VoiceManager::default();
        let lease = manager.reserve("chat").unwrap();
        manager.retire();
        assert!(manager.own(lease).is_err());
    }
}
