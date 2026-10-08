//! Ephemeral client-media control. Never persist these payloads in sync/queues.
use super::{VoiceLease, VoiceRejection};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

pub const CAPABILITY: &str = "voice-client-media-v1";
pub const MAX_SDP_BYTES: usize = 64 * 1024;
pub const HEARTBEAT_SECS: u64 = 5;
pub const LEASE_SECS: u64 = 15;
pub const ATTEMPT_TTL_SECS: u64 = 300;
pub const MAX_ATTEMPTS: usize = 256;

/// Random capability, not a timestamp or a user-chosen label.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AttemptKey(String);
impl AttemptKey {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl Default for AttemptKey {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Debug for AttemptKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AttemptKey([redacted])")
    }
}
impl<'de> Deserialize<'de> for AttemptKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let id = uuid::Uuid::parse_str(&s)
            .map_err(|_| serde::de::Error::custom("invalid attempt key"))?;
        if id.get_version_num() != 4 {
            return Err(serde::de::Error::custom(
                "attempt key must be random UUID v4",
            ));
        }
        Ok(Self(id.to_string()))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Sdp(String);
impl Sdp {
    pub fn new(value: String) -> Result<Self, VoiceRejection> {
        if value.is_empty() || value.len() > MAX_SDP_BYTES {
            return Err(VoiceRejection::Protocol);
        }
        Ok(Self(value))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Sdp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sdp([redacted])")
    }
}
impl<'de> Deserialize<'de> for Sdp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?)
            .map_err(|_| serde::de::Error::custom("invalid SDP length"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Envelope<T> {
    pub target_device_id: String,
    pub payload: T,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lease {
    pub host_device_id: String,
    pub voice: VoiceLease,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub protocol: String,
    pub client_webrtc: bool,
    pub voices: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prepare {
    pub attempt_key: AttemptKey,
    pub config: crate::ChatConfig,
    #[serde(default)]
    pub voice: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prepared {
    pub lease: Lease,
    pub chat_id: String,
    pub voices: Vec<String>,
    pub heartbeat_seconds: u64,
    pub lease_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Negotiate {
    pub lease: Lease,
    pub negotiation_id: AttemptKey,
    pub offer: Sdp,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Negotiated {
    pub negotiation_id: AttemptKey,
    pub answer: Sdp,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Confirm {
    pub lease: Lease,
    pub negotiation_id: AttemptKey,
    pub muted: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaState {
    Ready,
    Failed,
    Closed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    pub lease: Lease,
    pub sequence: u64,
    pub muted: bool,
    pub state: MediaState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ack {
    pub sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancel {
    pub attempt_key: AttemptKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_redacted_signaling_and_attempts() {
        assert!(Sdp::new("é".repeat(MAX_SDP_BYTES / 2 + 1)).is_err());
        assert!(serde_json::from_str::<Sdp>("\"\"").is_err());
        let s = Sdp::new("SECRET SDP".into()).unwrap();
        assert!(!format!("{s:?}").contains("SECRET"));
        let key = AttemptKey::new();
        assert!(!format!("{key:?}").contains(key.expose()));
        assert_eq!(
            serde_json::from_str::<AttemptKey>(&serde_json::to_string(&key).unwrap()).unwrap(),
            key
        );
        assert!(serde_json::from_str::<AttemptKey>("\"predictable\"").is_err());
        assert!(
            serde_json::from_str::<Cancel>(&format!(
                r#"{{"attemptKey":"{}","extra":1}}"#,
                key.expose()
            ))
            .is_err()
        );
    }
    #[test]
    fn routed_payload_stays_strict() {
        let value =
            serde_json::json!({"targetDeviceId":"host","payload":{"attemptKey":AttemptKey::new()}});
        assert!(serde_json::from_value::<Envelope<Cancel>>(value.clone()).is_ok());
        assert!(serde_json::from_value::<Cancel>(value).is_err());
    }
}
