//! Dictation on a host: a phone streams what it hears to the device running
//! its chat, and that device's speech model (the desktop dictation's) writes
//! it down. Everything here is ephemeral control traffic: audio is held in
//! memory on the host for the length of the dictation and never persisted,
//! logged, synced or queued.
use serde::{Deserialize, Serialize};

/// Hosts advertising this capability take dictations.
pub const CAPABILITY: &str = "dictation-host-v1";
/// The audio a phone sends: mono PCM16 at the model's own rate, so the host
/// resamples nothing.
pub const SAMPLE_RATE: u32 = 16_000;
/// The most audio one chunk carries: half a second.
pub const MAX_CHUNK_BYTES: usize = 16_384;
/// A host drops a dictation nobody finished after this long.
pub const IDLE_SECS: u64 = 90;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DictationRejection {
    /// The host has no speech model: download it in its Settings → Voice.
    ModelMissing,
    /// The host is already taking a dictation.
    Busy,
    RemoteHost,
    Overflow,
    Protocol,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Start {
    /// Chosen by the phone, a random UUID v4: every later message names it.
    pub session_id: String,
}

/// Base64 PCM16LE, mono, [`SAMPLE_RATE`]. No Debug: audio is never logged.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub session_id: String,
    pub sequence: u64,
    pub data: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub session_id: String,
}

/// What the host says of a dictation, in order; `Final` or `Failed` ends it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DictationEvent {
    /// The host takes audio from now on.
    Listening,
    /// Everything heard so far, read again while the phone still sends.
    Partial {
        text: String,
    },
    /// The host closed the capture itself (the limit): the final follows.
    Finalizing,
    Final {
        text: String,
    },
    Failed {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_rejections_round_trip_and_chunks_stay_out_of_debug() {
        for event in [
            DictationEvent::Listening,
            DictationEvent::Partial {
                text: "so far".into(),
            },
            DictationEvent::Finalizing,
            DictationEvent::Final {
                text: "done".into(),
            },
            DictationEvent::Failed {
                message: "no".into(),
            },
        ] {
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(
                serde_json::from_value::<DictationEvent>(json).unwrap(),
                event
            );
        }
        assert_eq!(
            serde_json::to_string(&DictationRejection::ModelMissing).unwrap(),
            "\"modelMissing\""
        );
        let chunk = Chunk {
            session_id: "s".into(),
            sequence: 1,
            data: "AAAA".into(),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"sequence\":1"));
    }
}
