//! A chat's direct link from this phone to its host (docs/design/relay-layer.md):
//! the same WebRTC peers terminals use, carrying the same idempotent frames as the
//! room. It's only ever a shortcut. Nothing depends on it, and losing it costs the
//! time until the room takes over.
//!
//! A link is a radio that can't sleep: its probe runs every few seconds, and the
//! connection under it keeps ICE alive. So one exists only while someone has this chat
//! on screen with the app in front ([`LinkWanted`]), for at most [`LINK_IDLE`] of nothing
//! happening, and the connection goes too once nothing else is using it. Opening the
//! chat starts it, so the first send already has it.

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
/// After a refusal over a connection that's up: an idle host may still be opening the chat.
const REFUSED_RETRY: Duration = Duration::from_secs(2);
const REFUSED_QUICK_RETRIES: u32 = 4;
/// A link that lived this long earns a fresh retry curve, starting with a quick one.
const LINK_STABLE: Duration = Duration::from_secs(60);
/// After a stable link drops: whatever ended it was probably passing, and the room is
/// slower than the link it replaces.
const LINK_RETRY_AFTER_STABLE: Duration = Duration::from_secs(1);
/// A chat on screen doing nothing for this long gives up its link. A send brings it
/// straight back, and the room carries that send meanwhile.
pub(crate) const LINK_IDLE: Duration = Duration::from_secs(300);
/// How long a chat has to stay open before it pairs: someone flicking through chats
/// shouldn't pair each one on the way.
const OPEN_SETTLE: Duration = Duration::from_millis(300);
/// Leaving a chat keeps its link this long, so going straight back reuses it. Going to
/// the background, losing the network or the host doesn't wait.
const LEAVE_GRACE: Duration = Duration::from_secs(10);

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

    /// Unwanted for a reason that ends a link at once: the app went away, or the
    /// network, the host, or the relay generation did. Only leaving the chat or going
    /// quiet gets [`LEAVE_GRACE`].
    fn ends_now(self) -> bool {
        !(self.relayed && self.foreground && self.network && self.host_online)
    }
}

/// The first state `wanted` takes that `pred` accepts. None: its sender is gone, so the
/// room is. (The value is copied out: a `watch::Ref` can't be held across an await.)
async fn until(
    wanted: &mut watch::Receiver<LinkWanted>,
    pred: impl FnMut(&LinkWanted) -> bool,
) -> Option<LinkWanted> {
    wanted.wait_for(pred).await.ok().map(|w| *w)
}

/// Resolves when a link should end: as soon as the app, network or host goes, or
/// [`LEAVE_GRACE`] after the chat stops being wanted otherwise, unless it's wanted
/// again before that.
async fn released(wanted: &mut watch::Receiver<LinkWanted>) {
    loop {
        let Some(now) = until(wanted, |w| !w.wanted()).await else { return };
        if now.ends_now() {
            return;
        }
        tokio::select! {
            back = until(wanted, |w| w.wanted() || w.ends_now()) => match back {
                Some(w) if w.wanted() => continue,
                _ => return,
            },
            _ = tokio::time::sleep(LEAVE_GRACE) => return,
        }
    }
}

/// Resolves true once a newly wanted chat has stayed wanted for [`OPEN_SETTLE`], false
/// if it stopped being wanted first.
async fn settled(wanted: &mut watch::Receiver<LinkWanted>) -> bool {
    tokio::select! {
        _ = until(wanted, |w| !w.wanted()) => false,
        _ = tokio::time::sleep(OPEN_SETTLE) => true,
    }
}

/// Keep one link up for as long as `wanted` says, until `cancel`.
pub(crate) fn spawn(
    relay: RelayHandle,
    direct: ViewerPeers,
    chat_id: String,
    host: Arc<Mutex<String>>,
    mut wanted: watch::Receiver<LinkWanted>,
    cancel: CancellationToken,
) {
    crate::runtime::shared().spawn(async move {
        let service = format!("{CHAT_LINK_PREFIX}{chat_id}");
        let mut retry = LINK_RETRY_MIN;
        let mut quick = false;
        let mut was_wanted = false;
        let mut quick_left = 0;
        loop {
            // Until someone wants one. Nothing runs here at rest.
            let woken = tokio::select! {
                _ = cancel.cancelled() => None,
                woken = until(&mut wanted, |w| w.wanted()) => woken,
            };
            if woken.is_none() {
                return;
            }
            if !was_wanted {
                // Just opened: pair once it's clear the chat is staying on screen.
                let stayed = tokio::select! {
                    _ = cancel.cancelled() => return,
                    stayed = settled(&mut wanted) => stayed,
                };
                if !stayed {
                    continue;
                }
                was_wanted = true;
                quick_left = REFUSED_QUICK_RETRIES;
            }
            let target = lock(&host).clone();
            tracing::debug!(chat = %chat_id, device = %target, "chat link pairing");
            let opened = tokio::select! {
                _ = cancel.cancelled() => return,
                _ = released(&mut wanted) => {
                    direct.release_if_idle(&target);
                    was_wanted = false;
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
                        _ = released(&mut wanted) => Some(false),
                    };
                    if let Some(finished) = ending {
                        stop.cancel();
                        link.await;
                        direct.release_if_idle(&target);
                        if finished {
                            return;
                        }
                        was_wanted = false;
                        continue;
                    }
                    tracing::info!(chat = %chat_id, device = %target, "chat link down");
                    if started.elapsed() >= LINK_STABLE {
                        retry = LINK_RETRY_MIN;
                        quick = true;
                    }
                }
                // The room carries this chat.
                Err(error) => tracing::debug!(chat = %chat_id, %error, "chat link unavailable"),
            }
            // The connection is up and the host said not yet: an idle host is still
            // opening the chat its wake asked for. That's cheap to ask again, a few times.
            let refused = quick_left > 0 && direct.ready(&target).await;
            let wait = if refused {
                quick_left -= 1;
                REFUSED_RETRY
            } else if quick {
                LINK_RETRY_AFTER_STABLE
            } else {
                retry
            };
            let doubles = !refused && !quick;
            quick = false;
            tokio::select! {
                _ = cancel.cancelled() => return,
                // Asked to stop between attempts: a half-open pairing shouldn't stay.
                _ = until(&mut wanted, |w| !w.wanted()) => {
                    direct.release_if_idle(&target);
                    was_wanted = false;
                }
                _ = tokio::time::sleep(wait) => {
                    if doubles {
                        retry = (retry * 2).min(LINK_RETRY_MAX);
                    }
                }
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

    /// How long `fut` takes on tokio's paused clock, or None if it's still waiting
    /// after a minute.
    async fn takes(fut: impl std::future::Future<Output = ()>) -> Option<Duration> {
        let start = tokio::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(60), fut)
            .await
            .ok()
            .map(|()| start.elapsed())
    }

    #[tokio::test(start_paused = true)]
    async fn leaving_a_chat_keeps_its_link_a_moment_and_the_background_ends_it_at_once() {
        // Left the chat and didn't come back: the link goes after the grace.
        let (tx, mut rx) = watch::channel(all());
        tx.send_replace(LinkWanted { on_screen: false, ..all() });
        assert_eq!(takes(released(&mut rx)).await, Some(LEAVE_GRACE));

        // Gone quiet on screen: the same.
        let (tx, mut rx) = watch::channel(all());
        tx.send_replace(LinkWanted { active: false, ..all() });
        assert_eq!(takes(released(&mut rx)).await, Some(LEAVE_GRACE));

        // The app going away, the network or the host: at once.
        for off in [
            LinkWanted { foreground: false, ..all() },
            LinkWanted { network: false, ..all() },
            LinkWanted { host_online: false, ..all() },
            LinkWanted { relayed: false, ..all() },
        ] {
            let (tx, mut rx) = watch::channel(all());
            tx.send_replace(off);
            assert_eq!(takes(released(&mut rx)).await, Some(Duration::ZERO), "{off:?}");
        }

        // Left, then went to the background during the grace: at that moment.
        let (tx, mut rx) = watch::channel(LinkWanted { on_screen: false, ..all() });
        let later = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            tx.send_replace(LinkWanted { on_screen: false, foreground: false, ..all() });
            tx
        });
        assert_eq!(takes(released(&mut rx)).await, Some(Duration::from_secs(3)));
        drop(later.await);

        // Left and came straight back: the link stays.
        let (tx, mut rx) = watch::channel(LinkWanted { on_screen: false, ..all() });
        let later = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            tx.send_replace(all());
            tx
        });
        assert_eq!(takes(released(&mut rx)).await, None, "the link should have stayed");
        drop(later.await);
    }

    #[tokio::test(start_paused = true)]
    async fn a_chat_flicked_past_doesnt_pair_and_one_that_stays_does_soon() {
        let (_tx, mut rx) = watch::channel(all());
        let start = tokio::time::Instant::now();
        assert!(settled(&mut rx).await);
        assert_eq!(start.elapsed(), OPEN_SETTLE);

        let (tx, mut rx) = watch::channel(all());
        let later = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.send_replace(LinkWanted { on_screen: false, ..all() });
            tx
        });
        assert!(!settled(&mut rx).await, "a chat open for 100 ms shouldn't pair");
        drop(later.await);
    }
}
