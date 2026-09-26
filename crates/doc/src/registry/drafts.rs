//! Small replicated draft indexes. Immutable revision rows preserve concurrent
//! edits: every unreferenced head is recoverable, even if another writer won
//! the current-revision register. Moving a row never writes content or liveness.
use super::*;
use zeron_proto::ordering::{order_key_between, valid_order_key};
use zeron_proto::{DraftChange, DraftTarget, PromptDraft, SaveDraft, valid_draft_id};
const DRAFTS: &str = "promptDrafts";
const REVISIONS: &str = "draftRevisions";

impl RegistryDoc {
    pub fn set_draft_order(&mut self, id: &str, key: &str) -> Result<(), DocError> {
        if !valid_draft_id(id) || !valid_order_key(key) {
            return Err(DocError::Schema("Invalid draft order".into()));
        }
        self.observe_sidebar_row(DRAFTS, id);
        self.write(
            DRAFTS,
            id,
            OpKind::Upsert,
            fields([("orderKey", json!(key))]),
        );
        Ok(())
    }

    pub fn draft_discarded(&self, id: &str) -> bool {
        self.overlay_row(DRAFTS, id)
            .is_some_and(|r| r.fields.get("closed").and_then(Value::as_bool) == Some(true))
    }
    pub fn draft_closed(&self, id: &str) -> bool {
        self.draft_discarded(id)
            || self
                .overlay_row(DRAFTS, id)
                .is_some_and(|r| r.fields.contains_key("sentRevision"))
    }
    pub fn publish_draft(&mut self, draft: &SaveDraft) -> Result<(), DocError> {
        if !valid_draft_id(&draft.id) || !valid_draft_id(&draft.revision) {
            return Err(DocError::Schema("Invalid draft ID".into()));
        }
        if self.draft_discarded(&draft.id) {
            return Ok(());
        }
        self.observe_sidebar_row(DRAFTS, &draft.id);
        if let Some(version) = self.overlay_row(REVISIONS, &draft.revision) {
            if version.fields.get("draftId").and_then(Value::as_str) == Some(draft.id.as_str())
                && !draft.deferred
                && !self
                    .overlay_row(DRAFTS, &draft.id)
                    .is_some_and(|r| r.fields.get("listed").and_then(Value::as_bool) == Some(true))
            {
                self.write(
                    DRAFTS,
                    &draft.id,
                    OpKind::Upsert,
                    fields([("listed", json!(true))]),
                );
            }
            return Ok(());
        }
        if let Some(base) = &draft.base_revision {
            self.observe_sidebar_row(REVISIONS, base);
        }
        let first = self.read_drafts().first().map(|d| d.order_key.clone());
        let stamp = self.next_hlc();
        let mut index = fields([("revision", json!(draft.revision))]);
        index.insert(
            if draft.deferred { "deferred" } else { "listed" }.into(),
            json!(true),
        );
        // A delayed recovery may publish an ancestor after its child.
        if self.overlay_rows(REVISIONS).iter().any(|r| {
            r.fields.get("baseRevision").and_then(Value::as_str) == Some(draft.revision.as_str())
        }) {
            index.remove("revision");
        }

        if !self.overlay_row(DRAFTS, &draft.id).is_some_and(|r| {
            r.fields
                .get("orderKey")
                .and_then(Value::as_str)
                .is_some_and(valid_order_key)
        }) {
            let order = order_key_between(None, first.as_deref(), &stamp)
                .map_err(|s| DocError::Schema(s.into()))?;
            index.insert("orderKey".into(), json!(order));
        }
        // One batch: no watcher can see an index pointing at a missing revision.
        self.enqueue_ops(vec![
            RowOp {
                kind: REVISIONS.into(),
                id: draft.revision.clone(),
                op: OpKind::Upsert,
                hlc: stamp.clone(),
                clocks: None,
                set: Some(fields([
                    ("draftId", json!(draft.id)),
                    ("baseRevision", json!(draft.base_revision)),
                    ("createdAt", json!(draft.created_at)),
                    ("preview", json!(draft.content.preview())),
                    ("target", json!(draft.content.target)),
                ])),
            },
            RowOp {
                kind: DRAFTS.into(),
                id: draft.id.clone(),
                op: OpKind::Upsert,
                hlc: stamp,
                clocks: None,
                set: Some(index),
            },
        ]);
        Ok(())
    }
    pub fn read_drafts(&self) -> Vec<PromptDraft> {
        let revisions = self.overlay_rows(REVISIONS);
        let parents: std::collections::HashSet<_> = revisions
            .iter()
            .filter_map(|r| r.fields.get("baseRevision").and_then(Value::as_str))
            .collect();
        // The LWW index can name an ancestor after out-of-order publication.
        // Select an actual head deterministically while preserving siblings.
        let mut heads = std::collections::HashMap::<&str, &str>::new();
        for row in &revisions {
            if !parents.contains(row.id.as_str()) {
                if let Some(root) = row.fields.get("draftId").and_then(Value::as_str) {
                    heads
                        .entry(root)
                        .and_modify(|id| *id = (*id).max(row.id.as_str()))
                        .or_insert(row.id.as_str());
                }
            }
        }
        let mut result = Vec::new();
        for row in &revisions {
            if parents.contains(row.id.as_str()) {
                continue;
            }
            let Some(root) = row.fields.get("draftId").and_then(Value::as_str) else {
                continue;
            };
            if self.draft_discarded(root) {
                continue;
            }
            let index = self.overlay_row(DRAFTS, root);
            if index.as_ref().is_some_and(|r| {
                r.fields.get("deferred").and_then(Value::as_bool) == Some(true)
                    && r.fields.get("listed").and_then(Value::as_bool) != Some(true)
            }) {
                continue;
            }

            let sent = index
                .as_ref()
                .and_then(|r| r.fields.get("sentRevision"))
                .and_then(Value::as_str);
            if sent == Some(row.id.as_str()) {
                continue;
            }
            let current = index
                .as_ref()
                .and_then(|r| r.fields.get("revision"))
                .and_then(Value::as_str)
                .filter(|id| !parents.contains(id))
                .or_else(|| heads.get(root).copied());
            let conflict = sent.is_some() || current != Some(row.id.as_str());
            let id = if conflict { row.id.as_str() } else { root };
            if self.draft_closed(id) {
                continue;
            }
            let order = self.overlay_row(DRAFTS, id).or(index).and_then(|r| {
                r.fields
                    .get("orderKey")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            let Some(order_key) = order.filter(|k| valid_order_key(k)) else {
                continue;
            };
            let Some(target) = row
                .fields
                .get("target")
                .and_then(|v| serde_json::from_value::<DraftTarget>(v.clone()).ok())
            else {
                continue;
            };
            result.push(PromptDraft {
                id: id.into(),
                revision: row.id.clone(),
                base_revision: row
                    .fields
                    .get("baseRevision")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                created_at: row
                    .fields
                    .get("createdAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                preview: row
                    .fields
                    .get("preview")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                target,
                order_key,
                conflict,
                pending: false,
            });
        }
        let keys: Vec<_> = result.iter().map(|d| d.order_key.clone()).collect();
        for row in &mut result {
            if row.conflict && self.overlay_row(DRAFTS, &row.id).is_none() {
                let upper = keys.iter().filter(|key| *key > &row.order_key).min();
                if let Ok(key) = order_key_between(
                    Some(&row.order_key),
                    upper.map(String::as_str),
                    &row.revision,
                ) {
                    row.order_key = key;
                }
            }
        }
        result.sort_by(|a, b| a.order_key.cmp(&b.order_key).then(a.id.cmp(&b.id)));
        result
    }
    pub fn change_draft(&mut self, change: &DraftChange) -> Result<(), DocError> {
        let id = match change {
            DraftChange::Move { id, .. }
            | DraftChange::Discard { id }
            | DraftChange::Consume { id, .. } => id,
        };
        if !valid_draft_id(id) {
            return Err(DocError::Schema("Invalid draft ID".into()));
        }
        self.observe_sidebar_row(DRAFTS, id);
        if let DraftChange::Consume { revision, .. } = change {
            self.write(
                DRAFTS,
                id,
                OpKind::Upsert,
                fields([("sentRevision", json!(revision))]),
            );
            return Ok(());
        }

        if let DraftChange::Discard { .. } = change {
            self.write(
                DRAFTS,
                id,
                OpKind::Upsert,
                fields([("closed", json!(true))]),
            );
            return Ok(());
        }
        if self.draft_closed(id) {
            return Ok(());
        }
        let DraftChange::Move { after, before, .. } = change else {
            unreachable!()
        };
        let mut rows = self.read_drafts();
        let Some(at) = rows.iter().position(|d| d.id == *id) else {
            return Ok(());
        };
        rows.remove(at);
        let at = before
            .as_ref()
            .and_then(|b| rows.iter().position(|d| &d.id == b))
            .or_else(|| {
                after
                    .as_ref()
                    .and_then(|a| rows.iter().position(|d| &d.id == a).map(|i| i + 1))
            })
            .unwrap_or(rows.len());
        let stamp = self.next_hlc();
        let key = order_key_between(
            at.checked_sub(1).map(|i| rows[i].order_key.as_str()),
            rows.get(at).map(|r| r.order_key.as_str()),
            &stamp,
        )
        .map_err(|e| DocError::Schema(e.into()))?;
        self.write(
            DRAFTS,
            id,
            OpKind::Upsert,
            fields([("orderKey", json!(key))]),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft(id: &str, revision: &str, base: Option<&str>) -> SaveDraft {
        SaveDraft {
            deferred: false,
            id: id.into(),
            revision: revision.into(),
            base_revision: base.map(str::to_owned),
            created_at: 1,
            content: zeron_proto::DraftContent {
                prompt: revision.into(),
                ..Default::default()
            },
            assets: vec![],
        }
    }
    #[test]
    fn active_canvas_stays_hidden_until_navigation_and_late_autosave_cannot_hide_it() {
        let mut doc = RegistryDoc::new("device");
        let mut save = draft("a", "v1", None);
        save.deferred = true;
        doc.publish_draft(&save).unwrap();
        assert!(doc.read_drafts().is_empty());
        assert!(doc.overlay_row(REVISIONS, "v1").is_some());
        save.revision = "v2".into();
        save.base_revision = Some("v1".into());
        doc.publish_draft(&save).unwrap();
        assert!(doc.read_drafts().is_empty());
        save.deferred = false;
        doc.publish_draft(&save).unwrap();
        let rows = doc.read_drafts();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].revision, "v2");
        let order = rows[0].order_key.clone();
        save.deferred = true;
        doc.publish_draft(&save).unwrap();
        save.revision = "v3".into();
        save.base_revision = Some("v2".into());
        doc.publish_draft(&save).unwrap();
        assert_eq!(doc.read_drafts()[0].revision, "v3");
        assert_eq!(doc.read_drafts()[0].order_key, order);
    }

    #[test]
    fn edits_keep_order_and_concurrent_heads_are_recoverable() {
        let mut doc = RegistryDoc::new("test");
        doc.publish_draft(&draft("a", "v1", None)).unwrap();
        doc.publish_draft(&draft("b", "v2", None)).unwrap();
        let key = doc
            .read_drafts()
            .iter()
            .find(|d| d.id == "a")
            .unwrap()
            .order_key
            .clone();
        doc.publish_draft(&draft("a", "v3", Some("v1"))).unwrap();
        doc.publish_draft(&draft("a", "v4", Some("v1"))).unwrap();
        let rows = doc.read_drafts();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().find(|d| d.id == "a").unwrap().order_key, key);
        assert!(rows.iter().any(|d| d.id == "v3" && d.conflict));
        doc.publish_draft(&draft("v3", "v5", Some("v3"))).unwrap();
        assert_eq!(doc.read_drafts().len(), 3);
    }
    #[test]
    fn recovery_published_out_of_order_keeps_the_head_under_its_draft_identity() {
        let mut doc = RegistryDoc::new("device");
        doc.publish_draft(&draft("a", "r3", Some("r2"))).unwrap();
        doc.publish_draft(&draft("a", "r1", None)).unwrap();
        doc.publish_draft(&draft("a", "r2", Some("r1"))).unwrap();
        let rows = doc.read_drafts();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a");
        assert_eq!(rows[0].revision, "r3");
        assert!(!rows[0].conflict);
    }

    #[test]
    fn delayed_move_or_save_cannot_revive_discarded_draft() {
        let mut doc = RegistryDoc::new("test");
        doc.publish_draft(&draft("a", "v1", None)).unwrap();
        doc.change_draft(&DraftChange::Discard { id: "a".into() })
            .unwrap();
        doc.publish_draft(&draft("a", "v2", Some("v1"))).unwrap();
        doc.change_draft(&DraftChange::Move {
            id: "a".into(),
            after: None,
            before: None,
        })
        .unwrap();
        assert!(doc.read_drafts().is_empty());
    }
    #[test]
    fn sending_one_head_keeps_a_concurrent_edit_recoverable() {
        let mut doc = RegistryDoc::new("a");
        doc.publish_draft(&draft("a", "base", None)).unwrap();
        doc.publish_draft(&draft("a", "left", Some("base")))
            .unwrap();
        doc.change_draft(&DraftChange::Consume {
            id: "a".into(),
            revision: "left".into(),
        })
        .unwrap();
        // A previously offline writer arrives after the send.
        doc.publish_draft(&draft("a", "right", Some("base")))
            .unwrap();
        let rows = doc.read_drafts();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "right");
        assert!(rows[0].conflict);
    }

    #[test]
    fn offline_moves_converge_without_losing_concurrent_content() {
        let mut a = RegistryDoc::new("a");
        for (id, rev) in [("a", "one"), ("b", "two"), ("c", "three")] {
            a.publish_draft(&draft(id, rev, None)).unwrap();
        }
        let mut b = RegistryDoc::new("b");
        let mut server: HashMap<(String, String), RegistryRow> = HashMap::new();
        fn exchange(doc: &mut RegistryDoc, server: &mut HashMap<(String, String), RegistryRow>) {
            for batch in doc.take_pushable() {
                for op in &batch.ops {
                    let key = (op.kind.clone(), op.id.clone());
                    if let Some(row) = apply_op(server.get(&key), op).0 {
                        server.insert(key, row);
                    }
                }
                doc.ack_batch(&batch.batch, 1);
            }
            doc.apply_rows(1, server.values().cloned().collect());
        }
        exchange(&mut a, &mut server);
        exchange(&mut b, &mut server);
        a.change_draft(&DraftChange::Move {
            id: "a".into(),
            before: Some("c".into()),
            after: None,
        })
        .unwrap();
        b.publish_draft(&draft("b", "edited", Some("two"))).unwrap();
        b.change_draft(&DraftChange::Move {
            id: "b".into(),
            before: Some("c".into()),
            after: None,
        })
        .unwrap();
        exchange(&mut a, &mut server);
        exchange(&mut b, &mut server);
        exchange(&mut a, &mut server);
        assert_eq!(a.read_drafts(), b.read_drafts());
        assert_eq!(
            a.read_drafts()
                .iter()
                .find(|d| d.id == "b")
                .unwrap()
                .revision,
            "edited"
        );
    }
}
