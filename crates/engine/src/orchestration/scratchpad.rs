//! The `Scratchpad` RPC: the Control Room's Scratchpad
//! (`zeron_proto::scratchpad`), one [`ScratchpadOp`] per call.
//!
//! A place for the person's ideas, notes and drawings: nothing here routes,
//! makes an item or a route, or tells any chat anything. Each op only reads
//! or writes the Scratchpad's own rows (`zeron_doc` `registry/scratchpad.rs`).
//!
//! Who may do what (the calling chat as the engine knows it, never one the
//! params name):
//! - the person: every op;
//! - the Brain dump: `read`, `write` and `append` (marked "Brain dump" in
//!   history);
//! - an orchestrator and its crew: `read`;
//! - any other chat: nothing.

use chrono::Utc;
use serde_json::{Value, json};
use zeron_doc::ScratchpadEdit;
use zeron_proto::scratchpad::{
    Editor, ScratchpadOp, ScratchpadRead, append_paragraph, describe_shapes, replace_section,
};
use zeron_rpc::RpcError;

use super::ChatRole;
use super::ops::Who;
use crate::rpc::EngineRpc;

fn failed(message: impl Into<String>) -> RpcError {
    RpcError::Failed(message.into())
}

/// Whether `who` may run `op`.
pub(crate) fn permitted(who: &Who, op: &ScratchpadOp) -> Result<(), RpcError> {
    let Who::Agent { chat, role } = who else {
        return Ok(());
    };
    let reads = matches!(op, ScratchpadOp::Read);
    let writes = matches!(op, ScratchpadOp::Write { .. } | ScratchpadOp::Append { .. });
    let ok = match role {
        ChatRole::BrainDump { .. } => reads || writes,
        ChatRole::Orchestrator { .. } | ChatRole::Crew { .. } => reads,
        ChatRole::Plain => false,
    };
    if ok {
        return Ok(());
    }
    let why = match role {
        ChatRole::Plain => {
            "The Scratchpad is the person's: only the Brain dump, orchestrators and their crews read it"
        }
        ChatRole::BrainDump { .. } => {
            "The Brain dump reads, writes and appends to the Scratchpad: the person does the rest"
        }
        _ => {
            "An orchestrator and its crew only read the Scratchpad: the person and the Brain dump write it"
        }
    };
    tracing::info!(%chat, "scratchpad: an agent's op refused");
    Err(failed(why))
}

impl EngineRpc {
    /// One op, from whoever `zeron_rpc::caller()` is.
    pub(crate) async fn scratchpad(&self, params: Value) -> Result<Value, RpcError> {
        let op: ScratchpadOp = serde_json::from_value(params)
            .map_err(|e| RpcError::BadParams(format!("Scratchpad: {e}")))?;
        let who = self.current_who()?;
        permitted(&who, &op)?;
        self.run_scratchpad_op(&who, op)
    }

    pub(crate) fn run_scratchpad_op(&self, who: &Who, op: ScratchpadOp) -> Result<Value, RpcError> {
        let by = match who {
            Who::Person => Editor::Person,
            Who::Agent {
                role: ChatRole::BrainDump { .. },
                ..
            } => Editor::BrainDump,
            Who::Agent { .. } => Editor::Other,
        };
        let workspace = self.orchestration().workspace();
        let now = Utc::now();
        let edit = |text: Option<String>, shapes, restored_from| ScratchpadEdit {
            text,
            shapes,
            by,
            now,
            restored_from,
        };
        let rev = match op {
            ScratchpadOp::Read => {
                let pad = workspace.read_orchestration_with(|doc| doc.scratchpad());
                let read = ScratchpadRead {
                    drawing: describe_shapes(&pad.shapes),
                    text: pad.text,
                    shapes: pad.shapes,
                    rev: pad.rev,
                    updated_at: pad.updated_at,
                    updated_by: pad.updated_by,
                };
                return serde_json::to_value(read).map_err(|e| failed(e.to_string()));
            }
            ScratchpadOp::Revision { id } => {
                let revision = workspace
                    .read_orchestration_with(|doc| doc.scratchpad_revision(&id))
                    .ok_or_else(|| failed(format!("No earlier version {id}")))?;
                return serde_json::to_value(revision).map_err(|e| failed(e.to_string()));
            }
            ScratchpadOp::SetText { text } => workspace
                .write_orchestration(|doc| doc.write_scratchpad(edit(Some(text), None, None))),
            ScratchpadOp::SetShapes { shapes } => workspace
                .write_orchestration(|doc| doc.write_scratchpad(edit(None, Some(shapes), None))),
            ScratchpadOp::Write { text, heading } => workspace.write_orchestration(|doc| {
                let next = match heading
                    .as_deref()
                    .map(str::trim)
                    .filter(|h| !h.trim_start_matches('#').trim().is_empty())
                {
                    Some(heading) => replace_section(&doc.scratchpad().text, heading, &text),
                    None => text,
                };
                doc.write_scratchpad(edit(Some(next), None, None))
            }),
            ScratchpadOp::Append { text } => {
                if text.trim().is_empty() {
                    return Err(failed("Nothing to append"));
                }
                workspace.write_orchestration(|doc| {
                    let next = append_paragraph(&doc.scratchpad().text, &text);
                    doc.write_scratchpad(edit(Some(next), None, None))
                })
            }
            ScratchpadOp::Restore { revision_id } => workspace.write_orchestration(|doc| {
                let revision = doc
                    .scratchpad_revision(&revision_id)
                    .ok_or_else(|| failed(format!("No earlier version {revision_id}")))?;
                Ok::<_, RpcError>(doc.write_scratchpad(edit(
                    Some(revision.text),
                    Some(revision.shapes),
                    Some(revision.id),
                )))
            })?,
        };
        Ok(json!({ "rev": rev }))
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use serde_json::{Value, json};
    use zeron_proto::HarnessId;
    use zeron_proto::orchestration::OrchestrationState;
    use zeron_proto::scratchpad::{Editor, ScratchpadState};
    use zeron_rpc::{Caller, RpcReply, RpcService, methods, with_caller};

    use crate::registry::HarnessRegistry;

    fn assemble(dir: &std::path::Path) -> crate::EngineCore {
        crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(dir).unwrap(),
            std::sync::Arc::new(HarnessRegistry::new()),
            HarnessId::Codex,
            None,
        )
        .unwrap()
    }

    fn agent(chat: &str) -> Caller {
        Caller::Agent {
            chat: Some(chat.into()),
        }
    }

    async fn call(
        core: &crate::EngineCore,
        caller: Caller,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let rpc = core.rpc_service();
        match with_caller(caller, rpc.handle(method, params)).await {
            Ok(RpcReply::Value(value)) => Ok(value),
            Ok(RpcReply::Stream(mut stream)) => Ok(stream.next().await.unwrap_or(Value::Null)),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn pad(core: &crate::EngineCore, caller: Caller, params: Value) -> Result<Value, String> {
        call(core, caller, methods::SCRATCHPAD, params).await
    }

    /// What `WatchScratchpad` sends once it has caught up with the rows
    /// (it publishes after the write, off the caller's path).
    async fn watched(core: &crate::EngineCore) -> ScratchpadState {
        let rows = core
            .workspace
            .read_orchestration_with(|doc| doc.read_scratchpad());
        let mut live = core.workspace.watch_scratchpad();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            live.wait_for(|s| *s == rows),
        )
        .await
        .expect("WatchScratchpad caught up")
        .unwrap();
        let sent: ScratchpadState = serde_json::from_value(
            call(core, Caller::Local, methods::WATCH_SCRATCHPAD, json!({}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(sent, rows);
        sent
    }

    /// The person's orchestrator (hosted elsewhere, so nothing runs), a
    /// crew thread of it, a plain chat and this device's Brain dump.
    async fn cast(core: &crate::EngineCore) -> (String, String, String, String) {
        let orch = call(
            core,
            Caller::Local,
            methods::ORCHESTRATE,
            json!({"op":"createOrchestrator","name":"Auth","goal":"g","deviceId":"dev-remote"}),
        )
        .await
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        for id in ["crew-chat", "plain-chat"] {
            call(
                core,
                Caller::Local,
                methods::MUTATE,
                json!({"op":"createChat","chatId":id,"deviceId":"dev-remote",
                "config":{"harness":"codex","sandbox":"workspace-write"}}),
            )
            .await
            .unwrap();
        }
        call(
            core,
            Caller::Local,
            methods::ORCHESTRATE,
            json!({"op":"addToCrew","orchestratorId":orch,"chatId":"crew-chat"}),
        )
        .await
        .unwrap();
        let brain = core.rpc_service().ensure_brain_dump().unwrap();
        (orch, "crew-chat".into(), "plain-chat".into(), brain)
    }

    /// Everything that is work: items, routes, dumps, asks.
    fn work(state: &OrchestrationState) -> (usize, usize, usize, usize) {
        (
            state.items.len(),
            state.dumps.iter().map(|d| d.routes.len()).sum(),
            state.dumps.len(),
            state.asks.len(),
        )
    }

    #[tokio::test]
    async fn the_scratchpad_survives_an_engine_restart() {
        let temp = tempfile::tempdir().unwrap();
        let core = assemble(temp.path());
        pad(
            &core,
            Caller::Local,
            json!({"op":"setText","text":"# Ideas\n\nsync later"}),
        )
        .await
        .unwrap();
        pad(&core, Caller::Local, json!({"op":"setShapes","shapes":[{"id":"a","kind":"rect","x":1,"y":2,"w":3,"h":4,"color":"accent","z":1}]}))
            .await
            .unwrap();
        let before = watched(&core).await;
        core.shutdown().await;
        drop(core);

        let core = assemble(temp.path());
        let after = watched(&core).await;
        assert_eq!(after, before);
        assert_eq!(after.pad.text, "# Ideas\n\nsync later");
        assert_eq!(after.pad.shapes[0].color, "accent");
        assert!(!after.history.is_empty());
        core.shutdown().await;
    }

    #[tokio::test]
    async fn the_brain_dump_reads_writes_and_appends_and_its_edits_are_marked() {
        let temp = tempfile::tempdir().unwrap();
        let core = assemble(temp.path());
        let (_, _, _, brain) = cast(&core).await;
        pad(
            &core,
            Caller::Local,
            json!({"op":"setText","text":"# Ideas\n\n## Sync\n\nold\n\n## Map\n\nislands\n"}),
        )
        .await
        .unwrap();
        pad(&core, Caller::Local, json!({"op":"setShapes","shapes":[{"id":"a","kind":"rect","x":0,"y":0,"w":10,"h":10,"text":"API"}]}))
            .await
            .unwrap();
        let mut live = core.workspace.watch_scratchpad();
        live.mark_unchanged();

        let read = pad(&core, agent(&brain), json!({"op":"read"}))
            .await
            .unwrap();
        assert!(read["text"].as_str().unwrap().contains("## Sync"));
        assert!(
            read["drawing"]
                .as_str()
                .unwrap()
                .contains("1 box labelled 'API'"),
            "{read}"
        );

        pad(
            &core,
            agent(&brain),
            json!({"op":"write","text":"new sync plan","heading":"Sync"}),
        )
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), live.changed())
            .await
            .expect("the edit shows live")
            .unwrap();
        let text = watched(&core).await.pad.text;
        assert_eq!(
            text,
            "# Ideas\n\n## Sync\n\nnew sync plan\n\n## Map\n\nislands\n"
        );
        pad(
            &core,
            agent(&brain),
            json!({"op":"append","text":"- one more idea"}),
        )
        .await
        .unwrap();
        let state = watched(&core).await;
        assert!(
            state.pad.text.ends_with("islands\n\n- one more idea\n"),
            "{}",
            state.pad.text
        );
        assert_eq!(state.pad.updated_by, Editor::BrainDump);
        assert_eq!(state.history[0].by, Editor::BrainDump);
        assert_eq!(state.history[1].by, Editor::Person);
        // The drawing is the person's to change.
        pad(&core, agent(&brain), json!({"op":"write","text":"all new"}))
            .await
            .unwrap();
        assert_eq!(watched(&core).await.pad.text, "all new");
        assert_eq!(watched(&core).await.pad.shapes.len(), 1);
        for refused in [
            json!({"op":"setText","text":"x"}),
            json!({"op":"setShapes","shapes":[]}),
            json!({"op":"restore","revisionId":state.history[1].id}),
        ] {
            assert!(
                pad(&core, agent(&brain), refused.clone()).await.is_err(),
                "{refused}"
            );
        }
        core.shutdown().await;
    }

    #[tokio::test]
    async fn orchestrators_and_crew_only_read_and_other_chats_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let core = assemble(temp.path());
        let (orch, crew, plain, _) = cast(&core).await;
        pad(
            &core,
            Caller::Local,
            json!({"op":"setText","text":"the person's idea"}),
        )
        .await
        .unwrap();
        for reader in [&orch, &crew] {
            let read = pad(&core, agent(reader), json!({"op":"read"}))
                .await
                .unwrap();
            assert_eq!(read["text"], "the person's idea");
            for write in [
                json!({"op":"write","text":"mine now"}),
                json!({"op":"append","text":"more"}),
                json!({"op":"setText","text":"x"}),
            ] {
                let error = pad(&core, agent(reader), write).await.unwrap_err();
                assert!(error.contains("only read"), "{error}");
            }
        }
        assert!(
            pad(&core, agent(&plain), json!({"op":"read"}))
                .await
                .is_err()
        );
        assert!(
            pad(&core, agent(&plain), json!({"op":"append","text":"x"}))
                .await
                .is_err()
        );
        // Agents don't watch it.
        assert!(
            call(&core, agent(&orch), methods::WATCH_SCRATCHPAD, json!({}))
                .await
                .is_err()
        );
        assert_eq!(watched(&core).await.pad.text, "the person's idea");
        core.shutdown().await;
    }

    #[tokio::test]
    async fn history_undo_and_restore() {
        let temp = tempfile::tempdir().unwrap();
        let core = assemble(temp.path());
        let (_, _, _, brain) = cast(&core).await;
        pad(&core, Caller::Local, json!({"op":"setText","text":"v1"}))
            .await
            .unwrap();
        pad(
            &core,
            agent(&brain),
            json!({"op":"append","text":"from the dump"}),
        )
        .await
        .unwrap();
        let state = watched(&core).await;
        assert_eq!(state.history.len(), 2);
        // Undo: the version before the current one.
        let before = &state.history[1];
        let revision = pad(
            &core,
            Caller::Local,
            json!({"op":"revision","id":before.id}),
        )
        .await
        .unwrap();
        assert_eq!(revision["text"], "v1");
        pad(
            &core,
            Caller::Local,
            json!({"op":"restore","revisionId":before.id}),
        )
        .await
        .unwrap();
        let state = watched(&core).await;
        assert_eq!(state.pad.text, "v1");
        assert_eq!(state.history.len(), 3, "a restore loses nothing");
        assert_eq!(
            state.history[0].restored_from.as_deref(),
            Some(before.id.as_str())
        );
        assert_eq!(state.history[1].by, Editor::BrainDump);
        assert!(
            pad(
                &core,
                Caller::Local,
                json!({"op":"restore","revisionId":"nope"})
            )
            .await
            .is_err()
        );
        core.shutdown().await;
    }

    #[tokio::test]
    async fn writing_never_routes_or_makes_work() {
        let temp = tempfile::tempdir().unwrap();
        let core = assemble(temp.path());
        let (orch, _, _, brain) = cast(&core).await;
        let before = core.orchestration.state();
        let comms_before = core.orchestration.comms().recent().len();
        let text = "fix the login redirect. buy milk. @Auth ship it now";
        pad(&core, Caller::Local, json!({"op":"setText","text":text}))
            .await
            .unwrap();
        pad(
            &core,
            agent(&brain),
            json!({"op":"append","text":"route this to Auth: deploy"}),
        )
        .await
        .unwrap();
        pad(
            &core,
            agent(&brain),
            json!({"op":"write","text":"do it","heading":"Tasks"}),
        )
        .await
        .unwrap();
        let after = core.orchestration.state();
        assert_eq!(work(&after), work(&before));
        assert_eq!(work(&after), (0, 0, 0, 0));
        assert_eq!(after, before, "no orchestration row changed");
        assert_eq!(core.orchestration.comms().recent().len(), comms_before);
        // Nothing was sent to any chat.
        for chat in [&orch, &brain] {
            let handle = core.doc_host.open(chat).unwrap();
            assert!(handle.doc().read_commands().unwrap().is_empty(), "{chat}");
        }
        core.shutdown().await;
    }
}
