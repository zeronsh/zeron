//! What a tap on a card control does. The platform paints `Action` widgets and
//! forwards their payload; this is the whole vocabulary, so a control never
//! needs platform code of its own (and Android gets it for free).
//!
//! * `goal.toggle|pause|resume|clear`
//! * `todo.toggle|earlier|later|dismiss`
//! * `wf.toggle|actors|reports|result:<run>`
//! * `wf.stop|resume:<run>`
//! * `wf.artifact|artifact-retry:<run>:<artifact>`
//!
//! (`chat:<id>` is navigation: `TranscriptView::act` returns it unhandled.)
//!
//! Commands travel the same command plane as the desktop's: a doc command the
//! chat's host executes wherever it runs. A refusal shows on the card.

use zeron_client::ClientError;
use zeron_proto::{GoalCommand, WorkflowCommand, todo_view::TodoSummary, workflow_view};

use super::Worker;
use super::status::{self, ArtifactView};

impl Worker {
    pub(super) fn act(&mut self, payload: &str) {
        let (verb, rest) = payload.split_once(':').unwrap_or((payload, ""));
        match verb {
            "goal.toggle" => self.flip("goal", false),
            "goal.pause" => self.command("goal", |h| h.goal_command(GoalCommand::Pause)),
            "goal.resume" => self.command("goal", |h| h.goal_command(GoalCommand::Resume)),
            "goal.clear" => self.command("goal", |h| h.goal_command(GoalCommand::Clear)),
            "todo.toggle" => {
                let finished = self.input.todo.as_deref().is_some_and(|t| TodoSummary::of(t).finished());
                self.builder.ui.todo.toggle(finished);
            }
            "todo.earlier" => self.builder.ui.todo.toggle_fold(zeron_proto::todo_view::FoldSide::Earlier),
            "todo.later" => self.builder.ui.todo.toggle_fold(zeron_proto::todo_view::FoldSide::Later),
            "todo.dismiss" => {
                if let Some(items) = self.input.todo.clone() {
                    self.builder.ui.todo.dismiss(&items);
                }
            }
            "wf.toggle" => {
                let default = status::default_open(self.is_newest(rest));
                self.flip(&format!("wf:{rest}"), default);
            }
            "wf.actors" => *self.builder.ui.actor_pages.entry(rest.to_owned()).or_insert(1) += 1,
            "wf.reports" => toggle(&mut self.builder.ui.reports_all, rest),
            "wf.result" => toggle(&mut self.builder.ui.result_open, rest),
            "wf.stop" => {
                let run_id = rest.to_owned();
                self.command(rest, move |h| h.workflow_command(WorkflowCommand::Stop { run_id, reason: Some("Stopped from the phone".into()) }))
            }
            "wf.resume" => {
                let run_id = rest.to_owned();
                self.command(rest, move |h| h.workflow_command(WorkflowCommand::Resume { run_id }))
            }
            "wf.artifact" | "wf.artifact-retry" => {
                let Some((run, artifact)) = rest.split_once(':') else { return };
                let retry = verb == "wf.artifact-retry";
                let ui = &mut self.builder.ui;
                if !retry && ui.artifact.get(run).map(String::as_str) == Some(artifact) {
                    // The open one again: close it.
                    ui.artifact.remove(run);
                    return;
                }
                ui.artifact.insert(run.to_owned(), artifact.to_owned());
                // A chip on a folded card opens the card on the document.
                ui.open.insert(format!("wf:{run}"), true);
                let ready = matches!(ui.artifacts.get(&(run.to_owned(), artifact.to_owned())), Some(ArtifactView::Ready { .. }));
                if retry || !ready {
                    ui.artifacts.insert((run.to_owned(), artifact.to_owned()), ArtifactView::Loading);
                    self.fetch_artifact(run, artifact);
                }
            }
            _ => {}
        }
    }

    fn is_newest(&self, run_id: &str) -> bool {
        self.input.workflows.runs.last().is_some_and(|r| r.header.run_id == run_id)
    }

    /// Flip an explicit open choice whose default is `default`.
    fn flip(&mut self, key: &str, default: bool) {
        let open = self.builder.ui.open.get(key).copied().unwrap_or(default);
        self.builder.ui.open.insert(key.to_owned(), !open);
    }

    /// Send a command to the attached session; remember a refusal under `key`
    /// so the card says so, and forget it after the next success.
    fn command(&mut self, key: &str, send: impl FnOnce(&zeron_client::SessionHandle) -> Result<(), ClientError>) {
        let handle = self.shared.handle.lock().unwrap().clone();
        let result = match handle {
            Some(handle) => send(&handle),
            None => Err(ClientError::Closed),
        };
        match result {
            Ok(()) => {
                self.builder.ui.failures.remove(key);
            }
            Err(err) => {
                self.builder.ui.failures.insert(key.to_owned(), err.to_string());
            }
        }
    }

    /// Read one artifact on the chat's host and report back through the
    /// worker's own queue as markdown for the card to draw.
    fn fetch_artifact(&mut self, run: &str, artifact: &str) {
        let handle = self.shared.handle.lock().unwrap().clone();
        let (Some(handle), Some(tx)) = (handle, self.tx.clone()) else {
            self.builder.ui.artifacts.insert((run.to_owned(), artifact.to_owned()), ArtifactView::Failed("Not connected to the chat.".into()));
            return;
        };
        let summary = self.input.workflows.run(run).and_then(|r| r.artifacts.iter().find(|a| a.id == artifact)).cloned();
        let (run, artifact) = (run.to_owned(), artifact.to_owned());
        zeron_client::runtime::shared().spawn(async move {
            let view = match handle.workflow_artifact(&run, &artifact).await {
                Ok(page) => preview(summary.as_ref().map(|s| s.kind), &page),
                Err(err) => ArtifactView::Failed(err.to_string()),
            };
            let _ = tx.send(super::Msg::Fetched { run, artifact, view });
        });
    }
}

fn toggle(set: &mut std::collections::HashSet<String>, key: &str) {
    if !set.remove(key) {
        set.insert(key.to_owned());
    }
}

/// An artifact page as the card shows it. A table or metrics page that was
/// cut mid-document cannot be parsed, so it says so instead of failing oddly.
pub(super) fn preview(kind: Option<zeron_proto::ArtifactKind>, page: &zeron_client::ArtifactPage) -> ArtifactView {
    use zeron_proto::ArtifactKind;
    let kind = kind.unwrap_or(ArtifactKind::File);
    let fetched = page.text.as_ref().map_or(0, |t| t.len() as u64);
    let truncated = page.text.is_some() && fetched < page.total;
    if truncated && matches!(kind, ArtifactKind::Table | ArtifactKind::Metrics) {
        return ArtifactView::Failed(format!("This {} is too large to preview on the phone ({}).", if kind == ArtifactKind::Table { "table" } else { "list" }, workflow_view::format_bytes(page.total)));
    }
    match zeron_proto::artifact_view::to_markdown(kind, &page.content_type, page.text.as_deref(), status::PREVIEW_LINES) {
        Ok(markdown) => ArtifactView::Ready { title: page.title.clone(), markdown: std::sync::Arc::new(markdown), total: page.total, truncated },
        Err(err) => ArtifactView::Failed(err),
    }
}
