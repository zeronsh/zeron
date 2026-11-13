//! Who may decide: one-time review capabilities for Zeron's consent window.
//!
//! Allowing a plugin (`DecidePlugin {allow: true}`), taking one as yours
//! (`SignPlugin {id}`) and allowing a template (`DecideTemplate {allow:
//! true}`) each need a `review`: a random 32-byte capability minted by
//! `ReviewPlugin {id, hash}` / `ReviewTemplate {pending, hash}` for exactly
//! that plugin or package as shown, used up by the decision it was minted
//! for, gone after [`TTL`]. Only a consent window's own host asks for one:
//! the methods are refused to agents' scoped token
//! ([`zeron_rpc::agent_allowed`]), and the window's Luau never sees it.
//!
//! A plugin's may also be asked for by another of the account's devices
//! over the device link (Settings → Plugins' device switcher: that device's
//! consent window shows this device's plugin as this engine lists it). A
//! review is bound to who asked for it ([`Origin`]): one minted for this
//! device's app is good only to this device's app, one minted for a peer
//! only to that same peer, as the edge verified it (`fromDevice`). A dial
//! the edge didn't verify gets none. Templates' stay this device's.
//!
//! What this holds against: an agent through its MCP server (here or on
//! another device: its token can't mint, and its engine forwards a mint
//! only for its own app), a plugin or a template (which have no IPC token
//! at all; they reach the engine only through the app's host actions, none
//! of which mints a review outside the consent window), another account,
//! and a stale or replayed decision for something other than what was
//! shown. Not against code running as the user that reads the app's full
//! token from the data dir: that code can rewrite the data dir itself.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use zeron_rpc::RpcError;

/// How long a review stays good: a decision is made while the sheet is up.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// At most this many outstanding at once; the oldest goes first.
const MAX: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `DecidePlugin {allow: true}` or `SignPlugin {id}` on plugin `id`.
    Plugin,
    /// `DecideTemplate {allow: true}` on waiting package `pending`.
    Template,
}

/// Who a review was minted for, and so the only one who may use it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// This device's own app (full-token local IPC).
    Local,
    /// Another of the account's devices, as the edge verified the link.
    Peer(String),
}

impl Origin {
    /// The origin of the call being served, if it may hold a review at all:
    /// this device's app, or a verified peer. An agent's token or an
    /// unverified dial may not.
    pub fn of(caller: &zeron_rpc::Caller) -> Option<Origin> {
        match caller {
            zeron_rpc::Caller::Local => Some(Origin::Local),
            zeron_rpc::Caller::Peer { device: Some(device) } if !device.is_empty() => Some(Origin::Peer(device.clone())),
            zeron_rpc::Caller::Peer { device: _ } | zeron_rpc::Caller::Agent { .. } => None,
        }
    }
}

struct Review {
    kind: Kind,
    origin: Origin,
    subject: String,
    hash: String,
    at: Instant,
}

#[derive(Default)]
pub struct Reviews {
    open: HashMap<String, Review>,
}

/// The error a missing, stale or mismatched review gets: the app's sheet
/// mints a fresh one and tries again.
pub fn needed(what: &str) -> RpcError {
    RpcError::Failed(format!(
        "review: {what} needs Zeron's own consent window (open it there and decide again)"
    ))
}

impl Reviews {
    /// A fresh review for `subject` as shown (`hash`), good only to `origin`.
    pub fn mint(&mut self, kind: Kind, origin: Origin, subject: &str, hash: &str) -> Result<String, RpcError> {
        self.prune();
        while self.open.len() >= MAX {
            let Some(oldest) = self
                .open
                .iter()
                .min_by_key(|(_, r)| r.at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.open.remove(&oldest);
        }
        let token = zeron_rpc::ipc_auth::generate_token()
            .map_err(|e| RpcError::Failed(format!("Couldn't open a review: {e}")))?;
        self.open.insert(
            token.clone(),
            Review {
                kind,
                origin,
                subject: subject.to_owned(),
                hash: hash.to_owned(),
                at: Instant::now(),
            },
        );
        Ok(token)
    }

    /// Use up `token` for `subject` as shown (`hash`): good only once, only
    /// for what it was minted for, only to whom it was minted for
    /// (`origin`), only within [`TTL`]. A token presented for something
    /// else, or by someone else, is used up too (it was leaked or misused).
    pub fn redeem(
        &mut self,
        kind: Kind,
        origin: &Origin,
        subject: &str,
        hash: &str,
        token: Option<&str>,
    ) -> Result<(), RpcError> {
        self.prune();
        let what = match kind {
            Kind::Plugin => "Allowing a plugin",
            Kind::Template => "Allowing a template",
        };
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return Err(needed(what));
        };
        let Some(review) = self.open.remove(token) else {
            return Err(needed(what));
        };
        if review.kind != kind || review.origin != *origin || review.subject != subject || review.hash != hash {
            return Err(needed(what));
        }
        Ok(())
    }

    fn prune(&mut self) {
        self.open.retain(|_, r| r.at.elapsed() < TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_is_good_once_and_only_for_what_was_shown() {
        let mut reviews = Reviews::default();
        let here = Origin::Local;
        let token = reviews.mint(Kind::Plugin, here.clone(), "focus", "h1").unwrap();
        assert_eq!(token.len(), 64);
        assert!(reviews.redeem(Kind::Plugin, &here, "focus", "h1", None).is_err());
        assert!(reviews.redeem(Kind::Plugin, &here, "focus", "h1", Some("")).is_err());
        reviews.redeem(Kind::Plugin, &here, "focus", "h1", Some(&token)).unwrap();
        assert!(
            reviews.redeem(Kind::Plugin, &here, "focus", "h1", Some(&token)).is_err(),
            "used up"
        );

        // Another plugin, another hash, another kind: refused, and used up.
        for (kind, subject, hash) in [
            (Kind::Plugin, "other", "h1"),
            (Kind::Plugin, "focus", "h2"),
            (Kind::Template, "focus", "h1"),
        ] {
            let token = reviews.mint(Kind::Plugin, here.clone(), "focus", "h1").unwrap();
            assert!(reviews.redeem(kind, &here, subject, hash, Some(&token)).is_err());
            assert!(reviews.redeem(Kind::Plugin, &here, "focus", "h1", Some(&token)).is_err());
        }
    }

    /// A review is good only to whom it was minted for: this device's app,
    /// or the very device that asked over the link.
    #[test]
    fn a_review_is_good_only_to_whom_it_was_minted_for() {
        let mut reviews = Reviews::default();
        let laptop = Origin::Peer("laptop".into());
        let desk = Origin::Peer("desk".into());
        for (minted, presented) in [(&laptop, &Origin::Local), (&laptop, &desk), (&Origin::Local, &laptop)] {
            let token = reviews.mint(Kind::Plugin, minted.clone(), "focus", "h1").unwrap();
            assert!(reviews.redeem(Kind::Plugin, presented, "focus", "h1", Some(&token)).is_err());
            assert!(reviews.redeem(Kind::Plugin, minted, "focus", "h1", Some(&token)).is_err(), "used up");
        }
        let token = reviews.mint(Kind::Plugin, laptop.clone(), "focus", "h1").unwrap();
        reviews.redeem(Kind::Plugin, &laptop, "focus", "h1", Some(&token)).unwrap();
    }

    #[test]
    fn only_the_app_and_verified_devices_may_hold_one() {
        use zeron_rpc::Caller;
        assert_eq!(Origin::of(&Caller::Local), Some(Origin::Local));
        assert_eq!(Origin::of(&Caller::Peer { device: Some("laptop".into()) }), Some(Origin::Peer("laptop".into())));
        assert_eq!(Origin::of(&Caller::Peer { device: None }), None);
        assert_eq!(Origin::of(&Caller::Peer { device: Some(String::new()) }), None);
        assert_eq!(Origin::of(&Caller::Agent { chat: Some("c".into()) }), None);
    }

    #[test]
    fn outstanding_reviews_are_bounded() {
        let mut reviews = Reviews::default();
        let first = reviews.mint(Kind::Plugin, Origin::Local, "a", "h").unwrap();
        for i in 0..MAX {
            reviews.mint(Kind::Plugin, Origin::Local, &format!("p{i}"), "h").unwrap();
        }
        assert!(reviews.open.len() <= MAX);
        assert!(reviews.redeem(Kind::Plugin, &Origin::Local, "a", "h", Some(&first)).is_err());
    }
}
