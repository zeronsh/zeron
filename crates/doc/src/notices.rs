//! The notices on a session doc: what the chats this one started have
//! reported (see `zeron_proto::chat_events`), waiting for (or already handed
//! to) this chat's agent.
//!
//! The parent's host writes one when a turn it delegated to a side chat (or a
//! task agent) ends, and when a task's coordinator reports. It is also the
//! only one that hands them to its agent (see `DocHost::drain_notices`),
//! stamping `deliveredAt` instead of removing them, so viewers can still show
//! each chat's latest word.
//!
//! Ids are deterministic (`finished:{chat}:{turn}` and the like), so a notice
//! is written once however many times its turn end is seen, and an agent that
//! already read the reply through `wait_for_turn` can mark it delivered before
//! it even lands: an id-only entry is that acknowledgement.
//!
//! `reported` remembers, per chat, the last turn reported for it: a parent
//! catching up on its side chats after a restart reports each turn once, even
//! after its notice was pruned.
//!
//! Writers: any device, like `queue`. Old builds never read the containers.

use loro::ToJson;
use serde::{Deserialize, Serialize};
use zeron_proto::chat_events::EventKind;

use crate::schema::{DocError, SessionDoc};

const CONTAINER: &str = "notices";
const REPORTED: &str = "noticesReported";

/// Delivered notices kept for viewers; older ones are pruned.
pub const DELIVERED_NOTICES_KEPT: usize = 40;

/// One chat's report to the chat that started it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatNotice {
    pub id: String,
    pub kind: EventKind,
    /// The chat that reported.
    pub chat_id: String,
    /// Its title when it reported.
    pub title: String,
    /// The final reply, error, question, or the coordinator's report.
    pub text: String,
    /// Device that wrote it.
    pub issued_by: String,
    /// Epoch millis.
    pub issued_at: i64,
    /// Epoch millis it reached the agent (or was acknowledged). `None` while
    /// it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<i64>,
}

impl ChatNotice {
    pub fn pending(&self) -> bool {
        self.delivered_at.is_none()
    }
}

/// The id of the notice for chat `chat_id`'s `kind` event, keyed by what
/// makes it unique: the turn's message id, the question's request id, the
/// report's own id.
pub fn notice_id(kind: EventKind, chat_id: &str, key: &str) -> String {
    let kind = match kind {
        EventKind::Finished => "finished",
        EventKind::Failed => "failed",
        EventKind::NeedsInput => "input",
        EventKind::TaskDone => "task-done",
        EventKind::TaskQuestion => "task-question",
        EventKind::TaskBlocked => "task-blocked",
    };
    format!("{kind}:{chat_id}:{key}")
}

impl SessionDoc {
    /// The notices in arrival order, one per id: delivered when any copy is
    /// (an acknowledgement or a host's stamp). Malformed rows skip.
    pub fn read_notices(&self) -> Result<Vec<ChatNotice>, DocError> {
        let raw = self
            .doc()
            .get_list(CONTAINER)
            .get_deep_value()
            .to_json_value();
        let rows: Vec<serde_json::Value> = serde_json::from_value(raw).unwrap_or_default();
        let mut notices: Vec<ChatNotice> = Vec::new();
        let mut delivered: Vec<(String, i64)> = Vec::new();
        for row in rows {
            let Some(id) = row.get("id").and_then(|v| v.as_str()).map(str::to_owned) else {
                continue;
            };
            if let Some(at) = row.get("deliveredAt").and_then(|v| v.as_i64()) {
                delivered.push((id.clone(), at));
            }
            if notices.iter().any(|n| n.id == id) {
                continue;
            }
            if let Ok(notice) = serde_json::from_value::<ChatNotice>(row) {
                notices.push(notice);
            }
        }
        for notice in &mut notices {
            if let Some((_, at)) = delivered.iter().find(|(id, _)| *id == notice.id) {
                notice.delivered_at = Some(notice.delivered_at.unwrap_or(*at).min(*at));
            }
        }
        Ok(notices)
    }

    /// Append `notice` unless one with its id (or its acknowledgement) is
    /// already here. `false` when it was.
    pub fn push_notice(&self, notice: &ChatNotice) -> Result<bool, DocError> {
        if notice.id.trim().is_empty() || notice.chat_id.trim().is_empty() {
            return Err(DocError::Schema("notice id and chat required".into()));
        }
        if self.notice_index(&notice.id).is_some() {
            return Ok(false);
        }
        let list = self.doc().get_list(CONTAINER);
        let map = list.push_container(loro::LoroMap::new())?;
        map.insert("id", notice.id.as_str())?;
        map.insert(
            "kind",
            serde_json::to_value(notice.kind)?
                .as_str()
                .ok_or_else(|| DocError::Schema("kind not a string".into()))?,
        )?;
        map.insert("chatId", notice.chat_id.as_str())?;
        map.insert("title", notice.title.as_str())?;
        map.insert("text", notice.text.as_str())?;
        map.insert("issuedBy", notice.issued_by.as_str())?;
        map.insert("issuedAt", notice.issued_at)?;
        if let Some(at) = notice.delivered_at {
            map.insert("deliveredAt", at)?;
        }
        self.doc().commit();
        Ok(true)
    }

    /// Stamp `ids` delivered at `at`. An id with no notice yet gets an
    /// id-only entry, so the notice it acknowledges arrives already delivered.
    pub fn mark_notices_delivered(&self, ids: &[String], at: i64) -> Result<(), DocError> {
        let list = self.doc().get_list(CONTAINER);
        for id in ids {
            match self.notice_index(id) {
                Some(index) => {
                    if let Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) =
                        list.get(index)
                        && map.get("deliveredAt").is_none()
                    {
                        map.insert("deliveredAt", at)?;
                    }
                }
                None => {
                    let map = list.push_container(loro::LoroMap::new())?;
                    map.insert("id", id.as_str())?;
                    map.insert("deliveredAt", at)?;
                }
            }
        }
        self.doc().commit();
        Ok(())
    }

    /// Put `ids` back to waiting: the turn that was to carry them never
    /// started.
    pub fn mark_notices_pending(&self, ids: &[String]) -> Result<(), DocError> {
        let list = self.doc().get_list(CONTAINER);
        for index in 0..list.len() {
            if let Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) =
                list.get(index)
                && let Some(id) = map_id(&map)
                && ids.contains(&id)
                && map.get("kind").is_some()
            {
                map.delete("deliveredAt")?;
            }
        }
        self.doc().commit();
        Ok(())
    }

    /// Drop delivered notices (and acknowledgements) past the newest `keep`.
    /// Pending ones always stay.
    pub fn prune_notices(&self, keep: usize) -> Result<(), DocError> {
        let list = self.doc().get_list(CONTAINER);
        let delivered: Vec<usize> = (0..list.len())
            .filter(|&i| {
                matches!(
                    list.get(i),
                    Some(loro::ValueOrContainer::Container(loro::Container::Map(map)))
                        if map.get("deliveredAt").is_some()
                )
            })
            .collect();
        let excess = delivered.len().saturating_sub(keep);
        if excess == 0 {
            return Ok(());
        }
        // Back to front, so earlier indexes stay put.
        for &index in delivered[..excess].iter().rev() {
            list.delete(index, 1)?;
        }
        self.doc().commit();
        Ok(())
    }

    /// The last turn reported for chat `chat_id` (see the module notes).
    pub fn last_reported(&self, chat_id: &str) -> Option<String> {
        match self.doc().get_map(REPORTED).get(chat_id) {
            Some(loro::ValueOrContainer::Value(loro::LoroValue::String(key))) => Some(key.to_string()),
            _ => None,
        }
    }

    /// Remember turn `key` as reported for chat `chat_id`.
    pub fn set_reported(&self, chat_id: &str, key: &str) -> Result<(), DocError> {
        self.doc().get_map(REPORTED).insert(chat_id, key)?;
        self.doc().commit();
        Ok(())
    }

    fn notice_index(&self, id: &str) -> Option<usize> {
        let list = self.doc().get_list(CONTAINER);
        (0..list.len()).find(|&i| {
            matches!(
                list.get(i),
                Some(loro::ValueOrContainer::Container(loro::Container::Map(map)))
                    if map_id(&map).as_deref() == Some(id)
            )
        })
    }
}

fn map_id(map: &loro::LoroMap) -> Option<String> {
    match map.get("id") {
        Some(loro::ValueOrContainer::Value(loro::LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> SessionDoc {
        SessionDoc::init("chat-1").unwrap()
    }

    fn notice(id: &str, kind: EventKind) -> ChatNotice {
        ChatNotice {
            id: id.into(),
            kind,
            chat_id: "side".into(),
            title: "Fix tests".into(),
            text: "All green.".into(),
            issued_by: "dev".into(),
            issued_at: 10,
            delivered_at: None,
        }
    }

    #[test]
    fn a_notice_is_written_once() {
        let doc = doc();
        assert!(doc.push_notice(&notice("a", EventKind::Finished)).unwrap());
        assert!(!doc.push_notice(&notice("a", EventKind::Finished)).unwrap());
        let read = doc.read_notices().unwrap();
        assert_eq!(read.len(), 1);
        assert!(read[0].pending());
        assert_eq!(read[0].kind, EventKind::Finished);
    }

    #[test]
    fn an_acknowledgement_before_the_notice_keeps_it_delivered() {
        let doc = doc();
        doc.mark_notices_delivered(&["a".into()], 5).unwrap();
        assert!(doc.read_notices().unwrap().is_empty());
        assert!(!doc.push_notice(&notice("a", EventKind::Finished)).unwrap());
        doc.push_notice(&notice("b", EventKind::TaskDone)).unwrap();
        doc.mark_notices_delivered(&["b".into()], 7).unwrap();
        let read = doc.read_notices().unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].delivered_at, Some(7));
        doc.mark_notices_pending(&["b".into()]).unwrap();
        assert!(doc.read_notices().unwrap()[0].pending());
    }

    #[test]
    fn concurrent_copies_merge_as_delivered() {
        let host = doc();
        let side = doc();
        host.mark_notices_delivered(&["a".into()], 3).unwrap();
        side.push_notice(&notice("a", EventKind::Finished))
            .unwrap();
        let update = side.doc().export(loro::ExportMode::all_updates()).unwrap();
        host.doc().import(&update).unwrap();
        let read = host.read_notices().unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].delivered_at, Some(3));
    }

    #[test]
    fn pruning_keeps_pending_and_the_newest_delivered() {
        let doc = doc();
        for i in 0..5 {
            doc.push_notice(&notice(&format!("n{i}"), EventKind::TaskDone))
                .unwrap();
        }
        doc.mark_notices_delivered(&["n0".into(), "n1".into(), "n2".into()], 9)
            .unwrap();
        doc.prune_notices(1).unwrap();
        let ids: Vec<String> = doc
            .read_notices()
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(ids, ["n2", "n3", "n4"]);
    }

    #[test]
    fn a_reported_turn_is_remembered_per_chat() {
        let doc = doc();
        assert_eq!(doc.last_reported("side"), None);
        doc.set_reported("side", "u1").unwrap();
        doc.set_reported("side", "u2").unwrap();
        assert_eq!(doc.last_reported("side").as_deref(), Some("u2"));
        assert_eq!(doc.last_reported("other"), None);
    }

    #[test]
    fn ids_name_the_update() {
        assert_eq!(notice_id(EventKind::NeedsInput, "c", "r1"), "input:c:r1");
    }
}
