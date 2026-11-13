//! `DeviceStats {targetDeviceId?}`: this machine's report, or another of the
//! account's devices' through its relay ([`crate::device_stats`]).
//!
//! Only a device in this account's device list is asked (the relay itself
//! only lets the account's own devices in), and the whole ask is bounded by
//! [`REMOTE_DEADLINE`]: a device that's away says so instead of hanging.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use zeron_proto::Device;
use zeron_rpc::{RpcError, RpcReply};

use super::EngineRpc;
use crate::device_stats::{self, Places};

/// The most a remote report may take, dial included.
pub(crate) const REMOTE_DEADLINE: Duration = Duration::from_secs(5);

impl EngineRpc {
    pub(super) async fn device_stats(
        &self,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let target = params
            .get("targetDeviceId")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty() && *t != self.doc_host.device_id());
        let Some(target) = target else {
            return RpcReply::value(&device_stats::current(self.places()).await);
        };
        let devices = self.workspace.watch_devices().borrow().clone();
        let Some(device) = devices.iter().find(|d| d.id == target) else {
            return Err(RpcError::BadParams(format!(
                "{target} isn't one of this account's devices"
            )));
        };
        if self.links.is_none() {
            return Err(RpcError::Failed(format!(
                "Can't reach {}: this device isn't connected (signed out or offline)",
                device.name
            )));
        }
        let asked = tokio::time::timeout(
            REMOTE_DEADLINE,
            self.forward(target, zeron_rpc::methods::DEVICE_STATS, params.clone()),
        )
        .await;
        match asked {
            Ok(Ok(RpcReply::Value(value))) => Ok(RpcReply::Value(value)),
            Ok(Ok(RpcReply::Stream(_))) => Err(RpcError::Failed(format!(
                "{} answered DeviceStats with a stream",
                device.name
            ))),
            Ok(Err(error)) => Err(remote_error(device, error, Utc::now())),
            Err(_) => {
                // A link that doesn't answer may be a zombie: redial next time.
                if let Some(links) = &self.links {
                    links.invalidate(target);
                }
                Err(RpcError::Failed(format!(
                    "{} didn't answer within {} s ({}): it may be asleep or offline",
                    device.name,
                    REMOTE_DEADLINE.as_secs(),
                    last_seen(device, Utc::now()),
                )))
            }
        }
    }

    fn places(&self) -> Places {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        Places {
            data_dir: Some(self.uploads.dir().to_path_buf()),
            home,
            build: self.engine_info.build.clone(),
        }
    }
}

/// What a failed remote ask says, in the device's own name.
pub(crate) fn remote_error(device: &Device, error: RpcError, now: DateTime<Utc>) -> RpcError {
    let said = error.to_string();
    let lower = said.to_ascii_lowercase();
    if matches!(error, RpcError::UnknownMethod(_)) || lower.contains("unknown method") {
        let version = device
            .version
            .as_deref()
            .map(|v| format!(" ({v})"))
            .unwrap_or_default();
        return RpcError::Failed(format!(
            "{}'s Zeron{version} is too old to report stats. Update Zeron there.",
            device.name
        ));
    }
    let away = matches!(error, RpcError::Transport(_) | RpcError::Closed)
        || [
            "offline",
            "unreachable",
            "cannot reach",
            "host_offline",
            "host_closed",
        ]
        .iter()
        .any(|word| lower.contains(word));
    if away {
        return RpcError::Failed(format!(
            "{} is offline ({})",
            device.name,
            last_seen(device, now)
        ));
    }
    RpcError::Failed(format!("{}: {said}", device.name))
}

/// "last seen 2026-11-10T09:00:00Z, 3 h ago", or "never seen".
pub(crate) fn last_seen(device: &Device, now: DateTime<Utc>) -> String {
    let Some(at) = device.last_seen_at else {
        return "never seen".into();
    };
    let ago = (now - at).num_seconds().max(0);
    let ago = match ago {
        0..60 => "just now".to_string(),
        60..3600 => format!("{} min ago", ago / 60),
        3600..86_400 => format!("{} h ago", ago / 3600),
        86_400..172_800 => "1 day ago".to_string(),
        _ => format!("{} days ago", ago / 86_400),
    };
    format!(
        "last seen {}, {ago}",
        at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(version: Option<&str>, seen: Option<DateTime<Utc>>) -> Device {
        Device {
            id: "dev-b".into(),
            name: "devbox".into(),
            platform: "linux".into(),
            last_seen_at: seen,
            created_at: None,
            version: version.map(str::to_string),
            cursor_sdk_version: None,
            capabilities: Vec::new(),
        }
    }

    #[test]
    fn an_older_engine_is_named_too_old_with_its_version() {
        let now = Utc::now();
        for error in [
            RpcError::UnknownMethod("DeviceStats".into()),
            RpcError::Failed("unknown method: DeviceStats".into()),
        ] {
            let said = remote_error(&device(Some("0.2.100"), None), error, now).to_string();
            assert_eq!(
                said,
                "devbox's Zeron (0.2.100) is too old to report stats. Update Zeron there."
            );
        }
    }

    #[test]
    fn an_unreachable_device_is_offline_with_when_it_was_last_seen() {
        let now: DateTime<Utc> = "2026-11-10T12:00:00Z".parse().unwrap();
        let seen = Some("2026-11-10T09:00:00Z".parse().unwrap());
        for error in [
            RpcError::Transport("peer dev-b: device is offline (no recent presence)".into()),
            RpcError::Closed,
            RpcError::Failed(
                "cannot reach device dev-b: remote routing unavailable (offline)".into(),
            ),
        ] {
            let said = remote_error(&device(None, seen), error, now).to_string();
            assert_eq!(
                said,
                "devbox is offline (last seen 2026-11-10T09:00:00Z, 3 h ago)"
            );
        }
        let said = remote_error(&device(None, None), RpcError::Closed, now).to_string();
        assert_eq!(said, "devbox is offline (never seen)");
        // Anything else keeps its words, under the device's name.
        let said = remote_error(
            &device(None, None),
            RpcError::Failed("disk on fire".into()),
            now,
        )
        .to_string();
        assert_eq!(said, "devbox: disk on fire");
    }
}
