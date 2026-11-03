//! Canonical final text reducer. Call from the engine's serialized document
//! owner; this never submits a command and never accepts PCM or partial text.
use crate::{DocError, MessagePart, MessageRole, MessageStatus, SessionDoc, SessionMessageEntry};
use zeron_proto::voice::{MAX_TRANSCRIPT_BYTES, VoiceRole, VoiceTranscript};

pub fn commit_voice_transcript(
    doc: &SessionDoc,
    final_text: &VoiceTranscript,
    device: &str,
    created_at: i64,
) -> Result<Option<String>, DocError> {
    if final_text.text.len() > MAX_TRANSCRIPT_BYTES
        || final_text.session_id.is_empty()
        || final_text.item_id.is_empty()
        || final_text.session_id.len() > 256
        || final_text.item_id.len() > 256
    {
        return Err(DocError::Schema(
            "invalid canonical voice transcript".into(),
        ));
    }
    if final_text.text.trim().is_empty() {
        return Ok(None);
    }
    // Length-prefixing prevents collisions if a provider id contains ':';
    // identity is unrelated to text, so two identical phrases stay two items.
    let id = format!(
        "voice:{}:{}:{}",
        final_text.session_id.len(),
        final_text.session_id,
        final_text.item_id
    );
    let entries = doc.read_entries()?;
    if entries.iter().any(|entry| entry.id == id) {
        return Ok(None);
    }
    if let Some(promoted) = &final_text.promoted_message_id {
        // Promotion must point to an existing native record. Never silently
        // create a duplicate when it arrives out of order; retry after routing.
        return if entries.iter().any(|entry| &entry.id == promoted) {
            Ok(None)
        } else {
            Err(DocError::Schema(
                "voice promotion precedes native message".into(),
            ))
        };
    }
    doc.push_message(&SessionMessageEntry {
        id: id.clone(),
        role: match final_text.role {
            VoiceRole::User => MessageRole::User,
            VoiceRole::Assistant => MessageRole::Assistant,
        },
        parts: vec![MessagePart::Text {
            id: "t0".into(),
            text: final_text.text.clone(),
        }],
        created_at,
        device_id: device.into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    })?;
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::ToJson;
    #[test]
    fn final_identity_survives_replay_and_snapshot_without_commands_or_audio() {
        let doc = SessionDoc::init("chat").unwrap();
        let mut text = VoiceTranscript {
            session_id: "session".into(),
            item_id: "one".into(),
            role: VoiceRole::User,
            text: "create a chat".into(),
            promoted_message_id: None,
        };
        assert!(
            commit_voice_transcript(&doc, &text, "device", 1)
                .unwrap()
                .is_some()
        );
        assert!(
            commit_voice_transcript(&doc, &text, "device", 2)
                .unwrap()
                .is_none()
        );
        text.item_id = "two".into();
        assert!(
            commit_voice_transcript(&doc, &text, "device", 3)
                .unwrap()
                .is_some()
        );
        let snapshot = doc.doc().export(loro::ExportMode::Snapshot).unwrap();
        let restored = loro::LoroDoc::new();
        restored.import(&snapshot).unwrap();
        let restored = SessionDoc::from_doc(restored);
        assert_eq!(restored.read_entries().unwrap().len(), 2);
        assert!(
            commit_voice_transcript(&restored, &text, "device", 4)
                .unwrap()
                .is_none()
        );
        let json = restored.doc().get_deep_value().to_json_value().to_string();
        assert!(!json.contains("leaseToken"));
        assert!(!json.contains("sampleRate"));
        assert!(!json.contains("Submit"));
    }
}
