//! A chat's direct link from this phone to its host (docs/design/relay-layer.md):
//! the same WebRTC peers terminals use, carrying the same idempotent frames as the
//! room. It's only ever a shortcut. Nothing depends on it, and losing it costs the
//! time until the room takes over.
//!
//! A link is a radio that can't sleep: its probe runs every few seconds, and the
//! connection under it keeps ICE alive. So one exists only while someone is
//! watching this chat do something ([`LinkWanted`]), and the connection goes too once
//! nothing else is using it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use zeron_preview::viewer::ViewerPeers;
use zeron_proto::relay_gen::CHAT_LINK_PREFIX;
use zeron_sync::chat_relay::{RelayHandle, run_link};

use crate::lock;

/// Between attempts to open a link: pairing costs signaling and STUN on the radio,
/// and the room is carrying the chat meanwhile.
const LINK_RETRY_MIN: Duration = Duration::from_secs(15);
const LINK_RETRY_MAX: Duration = Duration::from_secs(300);
/// A link that lived this long earns a fresh retry curve.
const LINK_STABLE: Duration = Duration::from_secs(60);
/// A chat on screen doing nothing for this long gives up its link.
pub(crate) const LINK_IDLE: Duration = Duration::from_secs(60);

/// What decides whether a link should be open right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LinkWanted {
    /// The chat is on a relay-layer generation, so there's a stream to shortcut.
    pub relayed: bool,
    pub on_screen: bool,
    pub foreground: bool,
    pub network: bool,
    pub host_online: bool,
    /// A turn is running, a send is on its way, or one just finished.
    pub active: bool,
}

impl LinkWanted {
    pub(crate) fn wanted(self) -> bool {
        self.relayed && self.on_screen && self.foreground && self.network && self.host_online && self.active
    }
}

/// Resolves once `wanted` reads `state`. False: its sender is gone, so the room is.
async fn until(wanted: &mut watch::Receiver<bool>, state: bool) -> bool {
    wanted.wait_for(|on| *on == state).await.is_ok()
}

/// Keep one link up for as long as `wanted` says, until `cancel`.
pub(crate) fn spawn(
    relay: RelayHandle,
    direct: ViewerPeers,
    chat_id: String,
    host: Arc<Mutex<String>>,
    mut wanted: watch::Receiver<bool>,
    cancel: CancellationToken,
) {
    crate::runtime::shared().spawn(async move {
        let service = format!("{CHAT_LINK_PREFIX}{chat_id}");
        let mut retry = LINK_RETRY_MIN;
        loop {
            // Until someone wants one. Nothing runs here at rest.
            let woken = tokio::select! {
                _ = cancel.cancelled() => false,
                woken = until(&mut wanted, true) => woken,
            };
            if !woken {
                return;
            }
            let target = lock(&host).clone();
            let opened = tokio::select! {
                _ = cancel.cancelled() => return,
                _ = until(&mut wanted, false) => {
                    direct.release_if_idle(&target);
                    continue;
                }
                opened = direct.open(&target, &service) => opened,
            };
            match opened {
                Ok(stream) => {
                    tracing::info!(chat = %chat_id, device = %target, "chat link up");
                    let started = std::time::Instant::now();
                    let stop = CancellationToken::new();
                    let link = run_link(stream, target.clone(), relay.clone(), stop.clone());
                    tokio::pin!(link);
                    // Run it to its end either way: its last act tells the relay task
                    // the link is gone, and until then frames still go down it.
                    let ending = tokio::select! {
                        _ = &mut link => None,
                        _ = cancel.cancelled() => Some(true),
                        _ = until(&mut wanted, false) => Some(false),
                    };
                    if let Some(finished) = ending {
                        stop.cancel();
                        link.await;
                        direct.release_if_idle(&target);
                        if finished {
                            return;
                        }
                        continue;
                    }
                    tracing::info!(chat = %chat_id, device = %target, "chat link down");
                    if started.elapsed() >= LINK_STABLE {
                        retry = LINK_RETRY_MIN;
                    }
                }
                // The room carries this chat.
                Err(error) => tracing::debug!(chat = %chat_id, %error, "chat link unavailable"),
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                // Asked to stop between attempts: a half-open pairing shouldn't stay.
                _ = until(&mut wanted, false) => direct.release_if_idle(&target),
                _ = tokio::time::sleep(retry) => retry = (retry * 2).min(LINK_RETRY_MAX),
                // A network change or the app coming back tries now.
                _ = direct.changed() => retry = LINK_RETRY_MIN,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> LinkWanted {
        LinkWanted {
            relayed: true,
            on_screen: true,
            foreground: true,
            network: true,
            host_online: true,
            active: true,
        }
    }

    #[test]
    fn a_link_needs_every_condition() {
        assert!(all().wanted());
        for off in [
            LinkWanted { relayed: false, ..all() },
            LinkWanted { on_screen: false, ..all() },
            LinkWanted { foreground: false, ..all() },
            LinkWanted { network: false, ..all() },
            LinkWanted { host_online: false, ..all() },
            LinkWanted { active: false, ..all() },
        ] {
            assert!(!off.wanted(), "{off:?}");
        }
        assert!(!LinkWanted::default().wanted());
    }
}
