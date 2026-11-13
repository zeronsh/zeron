//! `Orchestrate{op: report}`: a crew thread reports to its orchestrator.
//!
//! Here, on the crew thread's engine: the crew row's last report, the item's
//! progress and links, and a comms event. The orchestrator is always the
//! crew row's, never what the caller says. The report then goes to the
//! orchestrator's host as a `crewReport` command in its doc, which files it
//! in the transcript as "Crew updates" and wakes it when it should
//! (`doc_host/crew_inbox.rs`).

use chrono::Utc;
use serde_json::{Value, json};
use zeron_proto::orchestration::{CommKind, Report, ReportHead, ReportState, WorkLink};
use zeron_rpc::RpcError;

use crate::rpc::EngineRpc;
use zeron_doc::SessionCommandPayload;

/// The `report` op's fields (`OrchestrateOp::Report`).
#[derive(Debug, Clone, PartialEq)]
pub struct ReportParams {
    pub state: ReportState,
    pub summary: String,
    pub detail: String,
    pub item_id: Option<String>,
    pub links: Vec<WorkLink>,
    pub progress: Option<f32>,
}

/// A summary is one line of at most this many characters.
pub const MAX_SUMMARY_CHARS: usize = 140;

/// File `params` as `caller_chat`'s report. Replies `{id}` (the report's).
pub async fn file_report(
    ctx: &EngineRpc,
    caller_chat: &str,
    params: ReportParams,
) -> Result<Value, RpcError> {
    let orchestration = ctx.orchestration();
    let Some(member) = orchestration.crew_of(caller_chat) else {
        return Err(RpcError::Failed(
            "Only a crew thread reports: this chat isn't in an orchestrator's crew".into(),
        ));
    };
    let summary = super::excerpt(&params.summary, MAX_SUMMARY_CHARS);
    if summary.is_empty() {
        return Err(RpcError::BadParams("A report needs a summary".into()));
    }
    // An item it names counts only when it's on its own orchestrator's
    // board; otherwise its own item.
    let named = params.item_id.clone().filter(|id| {
        orchestration.item(id).is_some_and(|item| {
            item.orchestrator_id.as_deref() == Some(member.orchestrator_id.as_str())
        })
    });
    let report = Report {
        id: uuid::Uuid::new_v4().simple().to_string(),
        orchestrator_id: member.orchestrator_id.clone(),
        from_chat: caller_chat.to_owned(),
        item_id: named.or_else(|| member.item_id.clone()),
        state: params.state,
        summary,
        detail: params.detail,
        links: params.links,
        progress: params.progress.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0)),
        at: Utc::now(),
    };
    orchestration.set_last_report(
        caller_chat,
        ReportHead {
            id: report.id.clone(),
            state: report.state,
            summary: report.summary.clone(),
            at: report.at,
        },
    );
    if let Some(item_id) = report.item_id.as_deref() {
        let links = report.links.clone();
        let progress = report.progress;
        let chat = caller_chat.to_owned();
        orchestration.workspace().write_orchestration(|doc| {
            let Some(mut item) = doc.work_item(item_id) else {
                return;
            };
            if let Some(progress) = progress {
                item.progress = Some(progress);
            }
            super::merge_links(&mut item.links, links);
            if !item.threads.contains(&chat) {
                item.threads.push(chat);
            }
            item.updated_at = Utc::now();
            doc.upsert_work_item(&item);
        });
    }
    // A report is a step of the thread's own work (its host is this engine:
    // its agent's Zeron server dials it).
    orchestration
        .progress()
        .on_report(orchestration, caller_chat, report.state, report.at);
    orchestration.comms().say(
        CommKind::Report,
        caller_chat,
        &report.orchestrator_id,
        &report.summary,
    );
    let id = report.id.clone();
    let orchestrator = report.orchestrator_id.clone();
    if let Err(err) = ctx
        .doc_host_ref()
        .queue_command(&orchestrator, SessionCommandPayload::CrewReport { report })
    {
        tracing::warn!(orchestrator = %orchestrator, error = %err, "report: couldn't reach the orchestrator's transcript");
    }
    Ok(json!({ "id": id }))
}
