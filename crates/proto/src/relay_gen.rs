//! Who may use the relay layer (docs/design/relay-layer.md): the rule a chat's
//! host applies before moving the chat to room generation 3.

use chrono::{DateTime, Duration, Utc};

use crate::cloud::is_cloud_device;
use crate::entities::Device;

/// Advertised by an engine that runs the relay layer.
pub const CHAT_RELAY_CAPABILITY: &str = "chat-relay-v1";
/// Stream prefix of a chat's direct link: `chatlink/{chatId}` on the chat's host.
pub const CHAT_LINK_PREFIX: &str = "chatlink/";
/// Room generation of a chat whose host relays instead of storing.
pub const RELAY_ROOM_GEN: u32 = 3;
/// Generation a host returns a chat to when a device can't follow it.
pub const STORED_ROOM_GEN: u32 = 2;
/// A device unseen for this long no longer holds a chat back. If it returns, its
/// socket tells the host, and its device row moves chats back on its next boot.
pub const ACTIVE_WINDOW_DAYS: i64 = 14;

/// Can this chat leave stored rows? Every device that could read it and has been
/// seen recently must run the layer. Other Cloud machines never open this chat,
/// so they don't count, except the ones hosting it or its parent.
pub fn relay_allowed(
    devices: &[Device],
    host_device: &str,
    parent_host: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    let horizon = now - Duration::days(ACTIVE_WINDOW_DAYS);
    devices.iter().all(|device| {
        let involved = device.id == host_device
            || parent_host == Some(device.id.as_str())
            || !is_cloud_device(&device.id, &device.platform);
        // Never seen counts by its registration; with neither, it's a row
        // nothing has ever used.
        let seen = device.last_seen_at.or(device.created_at);
        let recent = seen.is_some_and(|at| at >= horizon);
        !involved || !recent || device.supports(CHAT_RELAY_CAPABILITY)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, days_ago: Option<i64>, relay: bool) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            platform: "macos".into(),
            last_seen_at: days_ago.map(|d| Utc::now() - Duration::days(d)),
            created_at: None,
            version: None,
            cursor_sdk_version: None,
            capabilities: if relay { vec![CHAT_RELAY_CAPABILITY.into()] } else { Vec::new() },
        }
    }

    #[test]
    fn every_recent_device_must_support_it() {
        let now = Utc::now();
        let new = [device("a", Some(0), true), device("b", Some(1), true)];
        assert!(relay_allowed(&new, "a", None, now));
        let mixed = [device("a", Some(0), true), device("old", Some(1), false)];
        assert!(!relay_allowed(&mixed, "a", None, now));
    }

    #[test]
    fn an_abandoned_old_device_stops_blocking() {
        let now = Utc::now();
        let edge = [device("a", Some(0), true), device("old", Some(ACTIVE_WINDOW_DAYS - 1), false)];
        assert!(!relay_allowed(&edge, "a", None, now));
        let gone = [device("a", Some(0), true), device("old", Some(ACTIVE_WINDOW_DAYS + 1), false)];
        assert!(relay_allowed(&gone, "a", None, now));
    }

    #[test]
    fn a_device_never_seen_counts_by_its_registration() {
        let now = Utc::now();
        let mut fresh = device("new-phone", None, false);
        fresh.created_at = Some(now - Duration::days(1));
        assert!(!relay_allowed(&[device("a", Some(0), true), fresh], "a", None, now));
        let unused = device("unused", None, false);
        assert!(relay_allowed(&[device("a", Some(0), true), unused], "a", None, now));
    }

    #[test]
    fn only_the_cloud_machines_involved_count() {
        let now = Utc::now();
        let cloud = format!("{}machine", crate::cloud::CLOUD_DEVICE_PREFIX);
        let devices = [device("a", Some(0), true), device(&cloud, Some(0), false)];
        assert!(relay_allowed(&devices, "a", None, now), "another chat's machine");
        assert!(!relay_allowed(&devices, &cloud, None, now), "this chat's host");
        assert!(!relay_allowed(&devices, "a", Some(&cloud), now), "its parent's host");
    }

    #[test]
    fn no_devices_means_nothing_blocks() {
        assert!(relay_allowed(&[], "a", None, Utc::now()));
    }
}
