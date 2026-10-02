//! Workflow tools: `workflow_guide`, `start_workflow`, `get_workflow_run`,
//! `list_workflow_runs`, `stop_workflow_run`, `resume_workflow_run`,
//! `resolve_workflow_question` (see docs/workflows.md).
//!
//! A workflow runs on the device that hosts the chat; these tools talk to the
//! local engine's RPC. An agent starts workflows for **its own chat** only,
//! and may only stop, resume or answer runs of its own chat — the same
//! "act on your own work, not on a sibling's" rule as the goal tools. The
//! actor-side tools (`submit_result`, `escalate`) live in the ask profile and
//! are never offered here.

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::methods;

use crate::tools::{ToolDef, Tools};

/// The authoring guide, embedded so `workflow_guide` needs no file access.
pub(crate) const GUIDE: &str = include_str!("../../../docs/workflow-guide.md");

pub(crate) fn catalog() -> Vec<ToolDef> {
    let run_id =
        json!({ "type": "string", "description": "The run id returned by start_workflow." });
    vec![
        ToolDef {
            name: "workflow_guide",
            description: "The authoring guide for dynamic workflows: the Starlark API with exact signatures, a complete worked example, patterns that make reviews trustworthy, and common mistakes. Read it BEFORE writing a workflow script.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "start_workflow",
            description: "Start a dynamic workflow: a Starlark script that orchestrates many agent chats in the background (phases, parallel fan-out, typed results, shell gates, reports, artifacts). Use it ONLY when the user explicitly asks for a workflow, or for work that genuinely needs many parallel independent agents (broad reviews, audits, migrations over many files); otherwise do the work yourself or use create_chats. Call workflow_guide first and write the script to it. The user must approve the run (the graph, commands and limits are shown to them); this call returns the run id as soon as they do, or an error if they deny it or the script has problems (listed as path:line:col). Do NOT poll: when the run finishes, its result is delivered to you as a message; you can stop it with stop_workflow_run. Give either `script` (the source) or `path` (a script file inside the project).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "A short title shown to the user." },
                    "script": { "type": "string", "description": "The Starlark source: `def main(args): ...`. At most 256 KB." },
                    "path": { "type": "string", "description": "A script file inside the project (relative path), instead of `script`." },
                    "args": { "type": "object", "description": "Passed to main(args) as a frozen dict. Keep tunable values here, not in ask text." },
                    "max_concurrency": { "type": "integer", "minimum": 1, "maximum": 32, "description": "Most agents running at once (default: cores - 2, capped at 16)." },
                    "harness": { "type": "string", "description": "Default harness for agents that do not pick one (see list_harnesses). Default: this chat's." },
                    "model": { "type": "string", "description": "Default model for agents that do not pick one (see list_models)." },
                    "reasoning": { "type": "string", "description": "Default reasoning level." },
                    "max_asks": { "type": "integer", "minimum": 1, "description": "Budget: asks the run may start (default 500)." },
                    "max_tokens": { "type": "integer", "minimum": 1, "description": "Budget: total tokens across all agents." },
                    "max_runtime_seconds": { "type": "integer", "minimum": 1, "description": "Budget: wall-clock seconds." }
                }
            }),
        },
        ToolDef {
            name: "get_workflow_run",
            description: "A workflow run's state: status, phase, usage, agents, artifacts, pending questions, and why it stopped. Add `include` for the node list, every reported item, or the script's full result. Do not poll a running workflow: its result is delivered to you when it finishes.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "run_id": run_id,
                    "include": { "type": "array", "items": { "type": "string", "enum": ["nodes", "reports", "result"] } }
                },
                "required": ["run_id"]
            }),
        },
        ToolDef {
            name: "list_workflow_runs",
            description: "Workflow runs of this chat (or of the chat you name, or all with chat=\"all\"), newest last.",
            input_schema: json!({
                "type": "object",
                "properties": { "chat": { "type": "string", "description": "Chat id, unique prefix or exact title; \"all\" for every chat. Default: your own chat." } }
            }),
        },
        ToolDef {
            name: "stop_workflow_run",
            description: "Stop a running workflow of your own chat: pending asks are cancelled and its agents are interrupted. The run stays resumable.",
            input_schema: json!({
                "type": "object",
                "properties": { "run_id": run_id, "reason": { "type": "string" } },
                "required": ["run_id"]
            }),
        },
        ToolDef {
            name: "resume_workflow_run",
            description: "Continue a stopped workflow of your own chat from its journal: work already done is replayed, not asked again. Needs the user's approval like a start. Only runs stopped by an interruption, a provider error, a budget or a stop can be resumed.",
            input_schema: json!({
                "type": "object",
                "properties": { "run_id": run_id },
                "required": ["run_id"]
            }),
        },
        ToolDef {
            name: "resolve_workflow_question",
            description: "Answer a question one of your workflow's agents escalated (you were told its run_id and qid). Decide from what you know; ask the user only if you truly cannot. Only that agent's current task was waiting for it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "run_id": run_id,
                    "qid": { "type": "string" },
                    "answer": { "type": "string" }
                },
                "required": ["run_id", "qid", "answer"]
            }),
        },
    ]
}

#[derive(Deserialize)]
pub(crate) struct StartArgs {
    name: Option<String>,
    script: Option<String>,
    path: Option<String>,
    args: Option<Value>,
    max_concurrency: Option<u32>,
    harness: Option<String>,
    model: Option<String>,
    reasoning: Option<String>,
    max_asks: Option<u32>,
    max_tokens: Option<u64>,
    max_runtime_seconds: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct RunArgs {
    run_id: String,
    #[serde(default)]
    include: Vec<String>,
    reason: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct ListArgs {
    chat: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct AnswerArgs {
    run_id: String,
    qid: String,
    answer: String,
}

impl Tools {
    pub(crate) async fn workflow_guide(&self) -> anyhow::Result<Value> {
        Ok(json!({ "guide": GUIDE }))
    }

    pub(crate) async fn start_workflow(&self, args: StartArgs) -> anyhow::Result<Value> {
        let Some(chat_id) = self.zeron.origin().chat_id.clone() else {
            anyhow::bail!(
                "start_workflow must be called from inside a Zeron chat: the workflow belongs to that chat and its result is delivered there"
            );
        };
        let result = self
            .zeron
            .call(
                methods::WORKFLOW_START,
                json!({
                    "chatId": chat_id,
                    "name": args.name,
                    "script": args.script,
                    "path": args.path,
                    "args": args.args,
                    "maxConcurrency": args.max_concurrency,
                    "harness": args.harness,
                    "model": args.model,
                    "reasoning": args.reasoning,
                    "maxAsks": args.max_asks,
                    "maxTokens": args.max_tokens,
                    "maxRuntimeSeconds": args.max_runtime_seconds,
                }),
            )
            .await
            .map_err(strip_method)?;
        Ok(json!({
            "runId": result["runId"],
            "name": result["name"],
            "status": "running",
            "phases": result["graph"]["phases"].as_array().map(|p| p.iter().map(|x| x["name"].clone()).collect::<Vec<_>>()),
            "agents": result["graph"]["actors"].as_array().map_or(0, Vec::len),
            "commands": result["graph"]["commands"].as_array().map(|c| c.iter().map(|x| x["command"].clone()).collect::<Vec<_>>()),
            "maxConcurrency": result["maxConcurrency"],
            "draftPath": result["draftPath"],
            "warnings": result["warnings"],
            "note": "Running in the background. Do not poll: when it finishes (or stops) its result is delivered to you as a message. Tell the user it has started and carry on, or end your turn.",
        }))
    }

    pub(crate) async fn get_workflow_run(&self, args: RunArgs) -> anyhow::Result<Value> {
        for what in &args.include {
            anyhow::ensure!(
                matches!(what.as_str(), "nodes" | "reports" | "result"),
                "include may contain nodes, reports, result — not {what:?}"
            );
        }
        self.zeron
            .call(
                methods::WORKFLOW_GET,
                json!({ "runId": args.run_id, "include": args.include }),
            )
            .await
            .map_err(strip_method)
    }

    pub(crate) async fn list_workflow_runs(&self, args: ListArgs) -> anyhow::Result<Value> {
        let chat_id = match args
            .chat
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            Some("all") => None,
            Some(key) => Some(self.zeron.resolve_chat(key).await?.id),
            None => self.zeron.origin().chat_id.clone(),
        };
        let runs = self
            .zeron
            .call(methods::WORKFLOW_LIST, json!({ "chatId": chat_id }))
            .await
            .map_err(strip_method)?;
        Ok(json!({ "runs": runs }))
    }

    /// The run's chat, and whether this server may act on it.
    async fn own_run(&self, run_id: &str, verb: &str) -> anyhow::Result<()> {
        let got = self
            .zeron
            .call(
                methods::WORKFLOW_GET,
                json!({ "runId": run_id, "include": [] }),
            )
            .await
            .map_err(strip_method)?;
        let owner = got["run"]["chatId"].as_str().unwrap_or_default();
        if let Some(own) = &self.zeron.origin().chat_id
            && own != owner
        {
            anyhow::bail!(
                "you can only {verb} workflows of your own chat; run {run_id} belongs to another chat"
            );
        }
        Ok(())
    }

    pub(crate) async fn stop_workflow_run(&self, args: RunArgs) -> anyhow::Result<Value> {
        self.own_run(&args.run_id, "stop").await?;
        let out = self
            .zeron
            .call(
                methods::WORKFLOW_STOP,
                json!({ "runId": args.run_id, "reason": args.reason }),
            )
            .await
            .map_err(strip_method)?;
        Ok(json!({
            "runId": args.run_id,
            "stopped": out["stopped"],
            "note": "The run settles shortly; you will receive its final message.",
        }))
    }

    pub(crate) async fn resume_workflow_run(&self, args: RunArgs) -> anyhow::Result<Value> {
        self.own_run(&args.run_id, "resume").await?;
        let out = self
            .zeron
            .call(methods::WORKFLOW_RESUME, json!({ "runId": args.run_id }))
            .await
            .map_err(strip_method)?;
        Ok(json!({
            "runId": out["runId"],
            "resumedFrom": args.run_id,
            "status": "running",
            "note": "Work already done is replayed. Its result is delivered to you when it finishes; do not poll.",
        }))
    }

    pub(crate) async fn resolve_workflow_question(
        &self,
        args: AnswerArgs,
    ) -> anyhow::Result<Value> {
        self.own_run(&args.run_id, "answer questions of").await?;
        self.zeron
            .call(
                methods::WORKFLOW_ANSWER,
                json!({ "runId": args.run_id, "qid": args.qid, "answer": args.answer }),
            )
            .await
            .map_err(strip_method)?;
        Ok(json!({ "answered": true, "runId": args.run_id, "qid": args.qid }))
    }
}

/// `WorkflowStart: message` → `message`: the method name is plumbing.
fn strip_method(e: anyhow::Error) -> anyhow::Error {
    let text = e.to_string();
    match text.split_once(": ") {
        Some((head, rest)) if head.starts_with("Workflow") && !head.contains(' ') => {
            anyhow::anyhow!("{rest}")
        }
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_is_embedded_and_names_the_api() {
        assert!(GUIDE.contains("def main(args)"));
        for needle in [
            "phase(",
            "agent(",
            ".ask(",
            "pmap(",
            "run(",
            "artifact.markdown",
            "schema.obj",
        ] {
            assert!(GUIDE.contains(needle), "{needle}");
        }
    }

    #[test]
    fn the_catalog_steers_and_never_offers_actor_tools() {
        let defs = catalog();
        let names: Vec<&str> = defs.iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            [
                "workflow_guide",
                "start_workflow",
                "get_workflow_run",
                "list_workflow_runs",
                "stop_workflow_run",
                "resume_workflow_run",
                "resolve_workflow_question"
            ]
        );
        let start = defs.iter().find(|d| d.name == "start_workflow").unwrap();
        assert!(
            start
                .description
                .contains("ONLY when the user explicitly asks")
        );
        assert!(start.description.contains("workflow_guide"));
        assert!(start.description.contains("Do NOT poll"));
        for d in &defs {
            assert_eq!(d.input_schema["type"], "object");
            assert!(d.name != "submit_result" && d.name != "escalate");
        }
    }

    #[test]
    fn method_prefixes_are_stripped_from_errors() {
        let e = strip_method(anyhow::anyhow!(
            "WorkflowStart: wf.star:2:5 phase \"x\" contains no ask"
        ));
        assert!(e.to_string().starts_with("wf.star:2:5"), "{e}");
        let e = strip_method(anyhow::anyhow!("something: else"));
        assert_eq!(e.to_string(), "something: else");
    }
}
