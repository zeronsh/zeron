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
use zeron_sync::attachment_store::{self, HeldSend, PutError};

use crate::client::ClientInner;
use crate::{lock, now_ms};

const BACKOFF_BASE: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct HeldSends {
    active: Mutex<HashSet<String>>,
    kick: Arc<Notify>,
    /// Releasing a send and removing one exclude each other: a removed send
    /// never goes out, and a send that went out reads as not removed.
    gate: Mutex<()>,
}

impl HeldSends {
    /// Relaunch: pick up what was still on its way. A failed send stays, however
    /// old, until its person retries or removes it.
    pub(crate) fn resume(&self, inner: &Arc<ClientInner>) {
        let Some(live) = inner.live() else { return };
        let sends = live.store.held_sends().unwrap_or_default();
        let mut started = HashSet::new();
        for send in sends {
            if send.failure.is_none() && started.insert(send.chat_id.clone()) {
                self.spawn(inner, &send.chat_id);
            }
        }
    }

    /// Its person removed a send, failed or still uploading: it never goes out,
    /// and its files go, here and at the edge. `false` when it already went.
    pub(crate) fn remove(&self, inner: &Arc<ClientInner>, id: &str) -> crate::error::Result<bool> {
        let Some(live) = inner.live() else {
            return Ok(false);
        };
        let send = {
            let _gate = lock(&self.gate);
            let send = live
                .store
                .held_send(id)
                .map_err(|err| crate::error::ClientError::Storage(err.to_string()))?;
            let Some(send) = send else { return Ok(false) };
            live.store
                .remove_held_send(id)
                .map_err(|err| crate::error::ClientError::Storage(err.to_string()))?;
            send
        };
        forget(inner, &send);
        Ok(true)
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
        // Never given up on: only its person removes it. Every try uploads all
        // its files again, so none ages out at the edge meanwhile.
        let outcome = store_uploads(&inner, &send).await;
        let cap = attachment_store::held_backoff_cap(
            now_ms().saturating_sub(send.created_at),
            BACKOFF_CAP,
        );
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
        backoff = (backoff * 2).min(cap);
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
    let Some(live) = inner.live() else {
        return Ok(());
    };
    {
        let _gate = lock(&live.held.gate);
        let still_held = live
            .store
            .held_send(&send.id)
            .map_err(|err| crate::error::ClientError::Storage(err.to_string()))?
            .is_some();
        if !still_held {
            // Removed while it uploaded: it never goes out.
            forget(inner, send);
            return Ok(());
        }
        core.release_held(send, &inner.config.device_id)?;
        live.store
            .remove_held_send(&send.id)
            .map_err(|err| crate::error::ClientError::Storage(err.to_string()))?;
    }
    // Through the store, the file is kept here until its message has run: a
    // retry after the message expired uploads it again. (SessionCore sweeps it.)
    if !store_used {
        for upload in &send.payload.uploads {
            live.escorts.send_by_peer(inner, &upload.upload_id);
        }
    }
    core.reload_held();
    inner.after_command(core, false);
    Ok(())
}

/// A send that won't go out: its files here, and best effort at the edge (the
/// lifecycle rule takes whatever this misses).
fn forget(inner: &Arc<ClientInner>, send: &HeldSend) {
    let Some(live) = inner.live() else { return };
    for upload in &send.payload.uploads {
        live.escorts.remove_stored(&upload.upload_id);
    }
    if let Some(core) = inner.session_core(&send.chat_id) {
        core.reload_held();
    }
    if send.payload.uploads.is_empty() {
        return;
    }
    let weak = Arc::downgrade(inner);
    let send = send.clone();
    crate::runtime::shared().spawn(async move {
        let Some(inner) = weak.upgrade() else { return };
        let Some(live) = inner.live() else { return };
        let edge = live.edge.clone();
        let Ok(token) = inner.tokens.bearer().await else {
            return;
        };
        for upload in &send.payload.uploads {
            let url = attachment_store::blob_url(&edge, &send.chat_id, &upload.upload_id);
            if let Err(err) = attachment_store::delete(crate::auth::http(), &url, &token).await {
                tracing::warn!(send = %send.id, %err, "removed send's file not deleted");
            }
        }
    });
}
