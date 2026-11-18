//! No two of an account's devices share a name: when this device's name is
//! one an earlier device already has, this device takes the lowest free
//! `<name>-1`, `<name>-2`, … (like a copied file). The earlier device keeps
//! the plain name.
//!
//! Only a device's own engine ever renames it, and only when it is the newer
//! of the two, so two engines never trade renames and an older engine that
//! doesn't do this is never fought. "Earlier" is the row's `createdAt`
//! (first registration); a row without one predates it and counts as
//! earliest. On a tie the larger device id yields. Names compare trimmed and
//! case-insensitively. Cloud machines keep their own names.

use zeron_proto::Device;

fn norm(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether `other` keeps the name over `me`.
fn precedes(other: &Device, me: &Device) -> bool {
    match (other.created_at, me.created_at) {
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (a, b) if a != b => a < b,
        _ => other.id < me.id,
    }
}

fn is_cloud(device: &Device) -> bool {
    device.platform == zeron_proto::CLOUD_PLATFORM
}

/// The name `me` should take among `devices` (every row of the account,
/// online or not), or `None` when its own is fine.
pub(crate) fn deduped_name(devices: &[Device], me: &str) -> Option<String> {
    let mine = devices.iter().find(|d| d.id == me)?;
    if is_cloud(mine) {
        return None;
    }
    let others: Vec<&Device> = devices
        .iter()
        .filter(|d| d.id != me && !is_cloud(d))
        .collect();
    let name = mine.name.trim();
    if name.is_empty() {
        return None;
    }
    let taken = |candidate: &str| others.iter().any(|d| norm(&d.name) == norm(candidate));
    let yields = others
        .iter()
        .any(|d| norm(&d.name) == norm(name) && precedes(d, mine));
    if !yields {
        return None;
    }
    // "honeypot-1" colliding again continues the series of "honeypot", not
    // a new one of its own ("honeypot-1-1").
    let base = match name.rsplit_once('-') {
        Some((base, n))
            if !base.trim().is_empty()
                && !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && taken(base) =>
        {
            base.trim()
        }
        _ => name,
    };
    (1u32..)
        .map(|k| format!("{base}-{k}"))
        .find(|candidate| !taken(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn at(ms: i64) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp_millis(ms)
    }

    fn device(id: &str, name: &str, created: Option<DateTime<Utc>>) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            platform: "linux".into(),
            last_seen_at: None,
            created_at: created,
            version: None,
            cursor_sdk_version: None,
            capabilities: vec![],
        }
    }

    /// Each device's engine renames only itself, in turn, until none does.
    fn settle(mut devices: Vec<Device>) -> Vec<String> {
        for _ in 0..10 {
            let mut changed = false;
            for i in 0..devices.len() {
                let me = devices[i].id.clone();
                if let Some(name) = deduped_name(&devices, &me) {
                    devices[i].name = name;
                    changed = true;
                }
            }
            if !changed {
                return devices.into_iter().map(|d| d.name).collect();
            }
        }
        panic!("names never settled: {devices:?}");
    }

    #[test]
    fn the_newer_of_two_takes_dash_one() {
        let devices = vec![
            device("a", "honeypot", at(1)),
            device("b", "Honeypot ", at(2)),
        ];
        assert_eq!(deduped_name(&devices, "a"), None);
        assert_eq!(deduped_name(&devices, "b").as_deref(), Some("Honeypot-1"));
        assert_eq!(settle(devices), vec!["honeypot", "Honeypot-1"]);
    }

    #[test]
    fn three_take_dash_one_and_dash_two_even_when_they_race() {
        let devices = vec![
            device("c", "honeypot", at(3)),
            device("a", "honeypot", at(1)),
            device("b", "honeypot", at(2)),
        ];
        // b and c both pick -1 first (as engines booting at once would); c,
        // newer, moves on to -2.
        assert_eq!(deduped_name(&devices, "b").as_deref(), Some("honeypot-1"));
        assert_eq!(deduped_name(&devices, "c").as_deref(), Some("honeypot-1"));
        let mut raced = devices.clone();
        raced[0].name = "honeypot-1".into();
        raced[2].name = "honeypot-1".into();
        assert_eq!(deduped_name(&raced, "b"), None);
        assert_eq!(deduped_name(&raced, "c").as_deref(), Some("honeypot-2"));
        // In whatever order the engines get to it: the oldest keeps the
        // name, the others -1 and -2.
        let settled = settle(devices);
        assert_eq!(settled[1], "honeypot");
        let mut names = settled.clone();
        names.sort();
        assert_eq!(names, vec!["honeypot", "honeypot-1", "honeypot-2"]);
    }

    #[test]
    fn a_tie_goes_to_the_smaller_id_and_settles() {
        let devices = vec![device("zz", "box", at(5)), device("aa", "box", at(5))];
        assert_eq!(deduped_name(&devices, "aa"), None);
        assert_eq!(deduped_name(&devices, "zz").as_deref(), Some("box-1"));
        assert_eq!(settle(devices.clone()), vec!["box-1", "box"]);
        // A row that predates createdAt counts as earliest.
        let devices = vec![device("a", "box", at(1)), device("b", "box", None)];
        assert_eq!(deduped_name(&devices, "b"), None);
        assert_eq!(deduped_name(&devices, "a").as_deref(), Some("box-1"));
    }

    #[test]
    fn a_unique_name_stays_even_one_that_looks_numbered() {
        let devices = vec![
            device("a", "honeypot", at(1)),
            device("b", "studio", at(2)),
            device("c", "server-2", at(3)),
        ];
        for id in ["a", "b", "c"] {
            assert_eq!(deduped_name(&devices, id), None, "{id}");
        }
        // Taken suffixes are skipped, offline rows count.
        let devices = vec![
            device("a", "h", at(1)),
            device("old", "h-1", at(0)),
            device("b", "h", at(2)),
        ];
        assert_eq!(deduped_name(&devices, "b").as_deref(), Some("h-2"));
    }

    #[test]
    fn a_rename_to_a_taken_name_is_numbered_on_the_newer() {
        // The person names the newer device as the older one is called.
        let devices = vec![device("a", "studio", at(1)), device("b", "Studio", at(2))];
        assert_eq!(settle(devices), vec!["studio", "Studio-1"]);
        // Or the older one as the newer is called: the newer still yields.
        let devices = vec![device("a", "laptop", at(1)), device("b", "laptop", at(2))];
        assert_eq!(deduped_name(&devices, "a"), None);
        assert_eq!(deduped_name(&devices, "b").as_deref(), Some("laptop-1"));
    }

    #[test]
    fn cloud_machines_keep_their_names() {
        let mut cloud = device("cloud-1", "Cloud", at(2));
        cloud.platform = zeron_proto::CLOUD_PLATFORM.into();
        let devices = vec![device("a", "Cloud", at(1)), cloud];
        assert_eq!(deduped_name(&devices, "cloud-1"), None);
        assert_eq!(deduped_name(&devices, "a"), None);
    }
}
