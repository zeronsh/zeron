//! Sends held for the edge's attachment store (docs/design/cloud-attachments.md).
//!
//! A phone's send to a Cloud-hosted chat stays out of the chat's doc until its
//! bytes are stored: the machine may sleep for hours, and the phone may be gone
//! by then. The row and the stash both survive a relaunch, so this resumes.
//! Same table and rules as the desktop engine (`doc_host_attachments.rs`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use zeron_sync::attachment_store::{
    self, HELD_SEND_AUTO_RETRY_MS, HELD_SEND_FAILED_RETENTION_MS, HeldSend, PutError,
};

use crate::client::ClientInner;
use crate::{lock, now_ms};

const BACKOFF_BASE: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct HeldSends {
    active: Mutex<HashSet<String>>,
    kick: Arc<Notify>,
}

impl HeldSends {
    /// Relaunch: let go of failures nobody retried in a week, resume the rest.
    pub(crate) fn resume(&self, inner: &Arc<ClientInner>) {
        let Some(live) = inner.live() else { return };
        match live
            .store
            .purge_failed_held_sends(now_ms() - HELD_SEND_FAILED_RETENTION_MS)
        {
            Ok(purged) => {
                for send in purged {
                    tracing::warn!(chat = %send.chat_id, send = %send.id,
                        "dropping a failed send nobody retried");
                    for upload in &send.payload.uploads {
                        live.escorts.remove(&upload.upload_id);
                    }
                }
            }
            Err(err) => tracing::warn!(%err, "held-send purge failed"),
        }
        let sends = live.store.held_sends().unwrap_or_default();
        let mut started = HashSet::new();
        for send in sends {
            if send.failure.is_none() && started.insert(send.chat_id.clone()) {
                self.spawn(inner, &send.chat_id);
            }
        }
    }

    /// The user's retry: anything backing off tries now.
    pub(crate) fn kick(&self) {
        self.kick.notify_waiters();
    }

    pub(crate) fn spawn(&self, inner: &Arc<ClientInner>, chat_id: &str) {
        if !lock(&self.active).insert(chat_id.to_owned()) {
            return;
        }
        let weak = Arc::downgrade(inner);
        let chat = chat_id.to_owned();
        let cancel = inner.cancel.clone();
        let kick = self.kick.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                run(weak.clone(), &chat, &cancel, &kick).await;
                let Some(inner) = weak.upgrade() else { return };
                let Some(live) = inner.live() else { return };
                lock(&live.held.active).remove(&chat);
                // A send held after the last look left the work to this task.
                let more = live
                    .store
                    .next_held_send(&chat)
                    .is_ok_and(|next| next.is_some());
                if cancel.is_cancelled() || !more || !lock(&live.held.active).insert(chat.clone()) {
                    return;
                }
            }
        });
    }
}

async fn run(weak: Weak<ClientInner>, chat: &str, cancel: &CancellationToken, kick: &Notify) {
    let mut backoff = BACKOFF_BASE;
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let Some(live) = inner.live() else { return };
        let send = match live.store.next_held_send(chat) {
            Ok(Some(send)) => send,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(%chat, %err, "held sends unreadable");
                return;
            }
        };
        let outcome = if now_ms().saturating_sub(send.created_at) > HELD_SEND_AUTO_RETRY_MS {
            Err(PutError::Permanent(
                "it couldn't be uploaded in time".to_string(),
            ))
        } else {
            store_uploads(&inner, &send).await
        };
        match outcome {
            Ok(store_used) => match release(&inner, &send, store_used) {
                Ok(()) => {
                    backoff = BACKOFF_BASE;
                    continue;
                }
                Err(err) => {
                    tracing::warn!(%chat, send = %send.id, %err, "held send not released");
                }
            },
            Err(PutError::Permanent(why)) => {
                tracing::warn!(%chat, send = %send.id, %why, "attachment upload failed permanently");
                if let Err(err) = live.store.fail_held_send(&send.id, &why) {
                    tracing::warn!(%chat, %err, "held send failure not recorded");
                }
                if let Some(core) = inner.session_core(chat) {
                    core.reload_held();
                }
                continue;
            }
            Err(PutError::Unsupported) => {}
            Err(PutError::Transient(why)) => {
                tracing::warn!(%chat, send = %send.id, %why, "attachment upload retrying");
            }
        }
        drop(inner);
        let kicked = kick.notified();
        tokio::select! {
            alive = super::wait_backoff(cancel, backoff) => {
                if !alive {
                    return;
                }
            }
            _ = kicked => {}
        }
        backoff = (backoff * 2).min(BACKOFF_CAP);
    }
}

/// Store the send's files. `Ok(false)`: this edge has no store, so the bytes
/// go the peer way once the message is out.
async fn store_uploads(inner: &Arc<ClientInner>, send: &HeldSend) -> Result<bool, PutError> {
    let Some(live) = inner.live() else {
        return Err(PutError::Transient("signed out".into()));
    };
    let edge = live.edge.clone();
    let count = send.payload.uploads.len().max(1) as f64;
    for (ix, upload) in send.payload.uploads.iter().enumerate() {
        let Some((_, bytes)) = live.escorts.load(&upload.upload_id) else {
            return Err(PutError::Permanent(
                "the file is no longer on this device".to_string(),
            ));
        };
        let token = inner
            .tokens
            .bearer()
            .await
            .map_err(|err| PutError::Transient(format!("no token: {err}")))?;
        let url = attachment_store::blob_url(&edge, &send.chat_id, &upload.upload_id);
        let core = inner.session_core(&send.chat_id);
        let progress_core = core.as_ref().map(Arc::downgrade);
        let total = bytes.len().max(1) as f64;
        let progress = move |sent: u64| {
            if let Some(core) = progress_core.as_ref().and_then(Weak::upgrade) {
                core.set_transfer_progress(Some((ix as f64 + sent as f64 / total) / count));
            }
        };
        let stored =
            attachment_store::put(crate::auth::http(), &url, &token, bytes, progress).await;
        if let Some(core) = &core {
            core.set_transfer_progress(None);
        }
        match stored {
            Ok(()) => {
                tracing::info!(chat = %send.chat_id, upload = %upload.upload_id,
                    "attachment stored for its Cloud machine");
            }
            Err(PutError::Unsupported) => {
                tracing::info!(chat = %send.chat_id, "this edge has no attachment store");
                return Ok(false);
            }
            Err(err) => return Err(err),
        }
    }
    Ok(true)
}

/// The bytes are safe: the message enters the chat. A crash after the doc write
/// repeats this harmlessly; the write checks for its own id.
fn release(
    inner: &Arc<ClientInner>,
    send: &HeldSend,
    store_used: bool,
) -> crate::error::Result<()> {
    let session = inner.resume_delivery(&send.chat_id)?;
    let core = &session.core;
    core.release_held(send, &inner.config.device_id)?;
    let Some(live) = inner.live() else {
        return Ok(());
    };
    live.store
        .remove_held_send(&send.id)
        .map_err(|err| crate::error::ClientError::Storage(err.to_string()))?;
    for upload in &send.payload.uploads {
        if store_used {
            live.escorts.remove(&upload.upload_id);
        } else {
            live.escorts.send_by_peer(inner, &upload.upload_id);
        }
    }
    core.reload_held();
    inner.after_command(core, false);
    Ok(())
}
