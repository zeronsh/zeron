//! Who may decide: one-time review capabilities for Zeron's consent window.
//!
//! Allowing a plugin (`DecidePlugin {allow: true}`), taking one as yours
//! (`SignPlugin {id}`) and allowing a template (`DecideTemplate {allow:
//! true}`) each need a `review`: a random 32-byte capability minted by
//! `ReviewPlugin {id, hash}` / `ReviewTemplate {pending, hash}` for exactly
//! that plugin or package as shown, used up by the decision it was minted
//! for, gone after [`TTL`]. Only the consent window's own host asks for one:
//! the methods are refused to agents' scoped token
//! ([`zeron_rpc::agent_allowed`]) and to other devices
//! ([`zeron_rpc::peer_refused`]), and the window's Luau never sees it.
//!
//! What this holds against: an agent through its MCP server, a plugin or a
//! template (which have no IPC token at all; they reach the engine only
//! through the app's host actions, none of which mints a review outside the
//! consent window), another device, and a stale or replayed decision for
//! something other than what was shown. Not against code running as the
//! user that reads the app's full token from the data dir: that code can
//! rewrite the data dir itself.

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

struct Review {
    kind: Kind,
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
    /// A fresh review for `subject` as shown (`hash`).
    pub fn mint(&mut self, kind: Kind, subject: &str, hash: &str) -> Result<String, RpcError> {
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
                subject: subject.to_owned(),
                hash: hash.to_owned(),
                at: Instant::now(),
            },
        );
        Ok(token)
    }

    /// Use up `token` for `subject` as shown (`hash`): good only once, only
    /// for what it was minted for, only within [`TTL`]. A token presented
    /// for something else is used up too (it was leaked or misused).
    pub fn redeem(
        &mut self,
        kind: Kind,
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
        if review.kind != kind || review.subject != subject || review.hash != hash {
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
        let token = reviews.mint(Kind::Plugin, "focus", "h1").unwrap();
        assert_eq!(token.len(), 64);
        assert!(reviews.redeem(Kind::Plugin, "focus", "h1", None).is_err());
        assert!(reviews.redeem(Kind::Plugin, "focus", "h1", Some("")).is_err());
        reviews.redeem(Kind::Plugin, "focus", "h1", Some(&token)).unwrap();
        assert!(
            reviews.redeem(Kind::Plugin, "focus", "h1", Some(&token)).is_err(),
            "used up"
        );

        // Another plugin, another hash, another kind: refused, and used up.
        for (kind, subject, hash) in [
            (Kind::Plugin, "other", "h1"),
            (Kind::Plugin, "focus", "h2"),
            (Kind::Template, "focus", "h1"),
        ] {
            let token = reviews.mint(Kind::Plugin, "focus", "h1").unwrap();
            assert!(reviews.redeem(kind, subject, hash, Some(&token)).is_err());
            assert!(reviews.redeem(Kind::Plugin, "focus", "h1", Some(&token)).is_err());
        }
    }

    #[test]
    fn outstanding_reviews_are_bounded() {
        let mut reviews = Reviews::default();
        let first = reviews.mint(Kind::Plugin, "a", "h").unwrap();
        for i in 0..MAX {
            reviews.mint(Kind::Plugin, &format!("p{i}"), "h").unwrap();
        }
        assert!(reviews.open.len() <= MAX);
        assert!(reviews.redeem(Kind::Plugin, "a", "h", Some(&first)).is_err());
    }
}
