//! One live owner per channel-capable MCP server.
//!
//! A channel server polls an external inbox. Two chats polling the same
//! server steal updates from each other, so the engine grants the server to
//! one non-worker chat. The identity is the server's name plus its config
//! hash ([`zeron_harness::mcp_policy::channel_identity`]), not a provider.

use std::collections::HashMap;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// Stand-in identity for Automatic mode before any channel server has been
/// named. Snapshots arrive in a later unit; until then the first non-worker
/// launch claims this so the second chat still drops channel servers.
pub(crate) const LAUNCH_LEASE: &str = "\0launch";

/// Settings → "channel delivers to".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelOwnerSetting {
    /// The first non-worker launch while the lease is free.
    Automatic,
    /// Nobody holds it. Every chat drops channel servers.
    Off,
    /// This chat, and only this chat. A worker id grants nobody.
    Chat(String),
}

impl Default for ChannelOwnerSetting {
    fn default() -> Self {
        Self::Automatic
    }
}

impl Serialize for ChannelOwnerSetting {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            Self::Automatic => "auto",
            Self::Off => "off",
            Self::Chat(id) => id.as_str(),
        };
        serializer.serialize_str(wire)
    }
}

impl<'de> Deserialize<'de> for ChannelOwnerSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "" | "auto" => Self::Automatic,
            "off" => Self::Off,
            other => Self::Chat(other.to_owned()),
        })
    }
}

/// `identity → chat id` for channel servers currently granted.
#[derive(Debug, Default, Clone)]
pub(crate) struct ChannelLeases {
    held: HashMap<String, String>,
}

impl ChannelLeases {
    pub(crate) fn holder(&self, identity: &str) -> Option<&str> {
        self.held.get(identity).map(String::as_str)
    }

    /// Whether this launch should omit channel servers.
    ///
    /// Grants `identity` to `chat_id` when the setting says this chat is the
    /// owner and it is not a worker. A worker never receives a grant, even
    /// when the setting pins that chat.
    pub(crate) fn drop_channels_for_launch(
        &mut self,
        identity: &str,
        chat_id: &str,
        worker: bool,
        setting: &ChannelOwnerSetting,
    ) -> bool {
        match setting {
            ChannelOwnerSetting::Off => true,
            ChannelOwnerSetting::Chat(owner) => {
                if worker || chat_id != owner {
                    return true;
                }
                match self.held.get(identity) {
                    Some(current) if current != chat_id => true,
                    Some(_) => false,
                    None => {
                        self.held.insert(identity.to_owned(), chat_id.to_owned());
                        false
                    }
                }
            }
            ChannelOwnerSetting::Automatic => {
                if worker {
                    return true;
                }
                match self.held.get(identity) {
                    Some(owner) if owner == chat_id => false,
                    Some(_) => true,
                    None => {
                        self.held.insert(identity.to_owned(), chat_id.to_owned());
                        false
                    }
                }
            }
        }
    }

    /// Drop every identity `chat_id` holds.
    ///
    /// Automatic hands each one to `successor` when that is some other
    /// non-empty chat. A pinned chat and Off do not hand off: the pin is
    /// still the owner when it next launches, and Off has no owner.
    pub(crate) fn release(
        &mut self,
        chat_id: &str,
        setting: &ChannelOwnerSetting,
        successor: Option<&str>,
    ) {
        let owned: Vec<String> = self
            .held
            .iter()
            .filter(|(_, owner)| owner.as_str() == chat_id)
            .map(|(identity, _)| identity.clone())
            .collect();
        for identity in owned {
            self.held.remove(&identity);
            if matches!(setting, ChannelOwnerSetting::Automatic)
                && let Some(next) = successor.filter(|next| !next.is_empty() && *next != chat_id)
            {
                self.held.insert(identity, next.to_owned());
            }
        }
    }

    /// A setting change cannot revoke a running process. Retain its lease
    /// until it exits, so a new owner cannot start a competing poller.
    pub(crate) fn apply_setting(&mut self, _setting: &ChannelOwnerSetting) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_harness::mcp_policy::channel_identity;

    fn fake() -> String {
        channel_identity("fake-channel", "hash")
    }

    #[test]
    fn mcp_policy_channel_lease_grant_and_release() {
        let id = fake();
        let mut leases = ChannelLeases::default();
        let auto = ChannelOwnerSetting::Automatic;

        assert!(leases.drop_channels_for_launch(&id, "worker", true, &auto));
        assert!(leases.holder(&id).is_none());

        assert!(!leases.drop_channels_for_launch(&id, "main-a", false, &auto));
        assert_eq!(leases.holder(&id), Some("main-a"));
        assert!(!leases.drop_channels_for_launch(&id, "main-a", false, &auto));
        assert!(leases.drop_channels_for_launch(&id, "main-b", false, &auto));
        assert_eq!(leases.holder(&id), Some("main-a"));

        leases.release("main-a", &auto, Some("main-b"));
        assert_eq!(leases.holder(&id), Some("main-b"));
        leases.release("main-b", &auto, None);
        assert!(leases.holder(&id).is_none());
        leases.drop_channels_for_launch(&id, "main-a", false, &auto);
        leases.release("main-a", &auto, Some(""));
        assert!(leases.holder(&id).is_none());
        leases.drop_channels_for_launch(&id, "main-a", false, &auto);
        leases.release("main-a", &auto, Some("main-a"));
        assert!(leases.holder(&id).is_none());

        let pin = ChannelOwnerSetting::Chat("pinned".into());
        assert!(leases.drop_channels_for_launch(&id, "other", false, &pin));
        assert!(leases.holder(&id).is_none());
        // The pin is a worker: nobody gets the poller.
        assert!(leases.drop_channels_for_launch(&id, "pinned", true, &pin));
        assert!(leases.holder(&id).is_none());
        assert!(!leases.drop_channels_for_launch(&id, "pinned", false, &pin));
        assert_eq!(leases.holder(&id), Some("pinned"));
        // A pinned owner does not hand the lease to whoever happens to be live.
        leases.release("pinned", &pin, Some("other"));
        assert!(leases.holder(&id).is_none());

        assert!(!leases.drop_channels_for_launch(&id, "pinned", false, &pin));
        leases.apply_setting(&ChannelOwnerSetting::Off);
        assert_eq!(leases.holder(&id), Some("pinned"));
        assert!(leases.drop_channels_for_launch(&id, "pinned", false, &ChannelOwnerSetting::Off));
        let other_pin = ChannelOwnerSetting::Chat("other".into());
        assert!(leases.drop_channels_for_launch(&id, "other", false, &other_pin));
        leases.release("pinned", &other_pin, None);
        assert!(!leases.drop_channels_for_launch(&id, "other", false, &other_pin));
        leases.release("other", &pin, None);

        assert!(!leases.drop_channels_for_launch(&id, "pinned", false, &pin));
        assert!(!leases.drop_channels_for_launch(LAUNCH_LEASE, "other", false, &auto));
        leases.apply_setting(&pin);
        assert_eq!(leases.holder(&id), Some("pinned"));
        assert_eq!(leases.holder(LAUNCH_LEASE), Some("other"));
        leases.apply_setting(&auto);
        assert_eq!(leases.holder(&id), Some("pinned"));
    }
}
