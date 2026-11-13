//! Where the Brain dump's new work goes, and what it asks when it can't tell.
//!
//! Bias to action: a task with no owner starts a thread in the project the
//! text names (else the one most recently worked in, this device's first),
//! on that project's device. `unsure` is only for two or more existing
//! places that fit; anything less starts the thread. An `unsure` route
//! raises an ask ("Where should this go?") whose options are its candidates
//! and a new thread in the best project; answering it moves the route there.

use chrono::{DateTime, Utc};
use zeron_proto::orchestration::{
    Ask, AskKind, AskOption, AskState, CommKind, Dump, OrchestratorKind, Route, RouteState,
    RouteTarget, prompt_text::one_line,
};
use zeron_proto::{Chat, Space};
use zeron_rpc::RpcError;

use crate::rpc::EngineRpc;

/// The fewest existing places an `unsure` route names.
pub(crate) const UNSURE_MIN: usize = 2;

/// A project, as placement weighs it.
#[derive(Debug, Clone)]
pub(crate) struct Project {
    pub space: Space,
    /// When a chat in it last had a message.
    pub last_active: Option<DateTime<Utc>>,
    /// It lives on this device.
    pub here: bool,
}

/// Every project, most recently active first (this device's first among
/// equals).
pub(crate) fn projects(spaces: &[Space], chats: &[Chat], here: &str) -> Vec<Project> {
    let mut out: Vec<Project> = spaces
        .iter()
        .map(|space| Project {
            last_active: chats
                .iter()
                .filter(|c| c.space_id.as_deref() == Some(space.id.as_str()))
                .filter_map(|c| c.last_message_at)
                .max(),
            here: space.device_id == here,
            space: space.clone(),
        })
        .collect();
    out.sort_by(|a, b| b.last_active.cmp(&a.last_active).then(b.here.cmp(&a.here)));
    out
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The project `text` is about: one it names (its name or folder), here
/// first; else the one most recently worked in.
pub(crate) fn best_project<'a>(text: &str, projects: &'a [Project]) -> Option<&'a Project> {
    let said = words(text);
    let named = |p: &&Project| {
        let name = p.space.display_name().to_lowercase();
        let folder = p.space.path.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or("").to_lowercase();
        [name, folder].iter().filter(|n| !n.is_empty()).any(|n| {
            let parts = words(n);
            !parts.is_empty() && said.windows(parts.len()).any(|w| w == parts.as_slice())
        })
    };
    let mut named_ones: Vec<&Project> = projects.iter().filter(named).collect();
    named_ones.sort_by(|a, b| b.here.cmp(&a.here).then(b.last_active.cmp(&a.last_active)));
    named_ones.first().copied().or_else(|| projects.first())
}

/// The most files a dump carries.
pub(crate) const DUMP_FILES_MAX: usize = 20;

/// A dump's files, checked as a send's are: a `pending://` ref, or an
/// absolute path on this device (where the Brain dump runs).
pub(crate) fn dump_files(paths: Vec<String>) -> Result<Vec<zeron_proto::orchestration::DumpAttachment>, RpcError> {
    if paths.len() > DUMP_FILES_MAX {
        return Err(RpcError::BadParams(format!("At most {DUMP_FILES_MAX} files in a dump")));
    }
    paths
        .into_iter()
        .map(|path| {
            let path = path.trim().to_owned();
            let name = if crate::uploads::is_pending_ref(&path) {
                crate::uploads::parse_pending_ref(&path)
                    .map(|(_, name)| name.to_owned())
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| RpcError::BadParams(format!("Not an upload: {path}")))?
            } else if std::path::Path::new(&path).is_absolute() {
                std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .ok_or_else(|| RpcError::BadParams(format!("Not a file: {path}")))?
            } else {
                return Err(RpcError::BadParams(format!("A dumped file is an upload or an absolute path: {path}")));
            };
            Ok(zeron_proto::orchestration::DumpAttachment { path, name })
        })
        .collect()
}

/// The dump's files a route forwards: the ones it names (by path or name;
/// each must be the dump's), else all of them for a task or info.
pub(crate) fn route_files(
    dump: &Dump,
    intent: zeron_proto::orchestration::RouteIntent,
    named: Option<Vec<String>>,
) -> Result<Vec<String>, RpcError> {
    use zeron_proto::orchestration::RouteIntent;
    match named {
        None if matches!(intent, RouteIntent::Task | RouteIntent::Info) => {
            Ok(dump.attachments.iter().map(|a| a.path.clone()).collect())
        }
        None => Ok(Vec::new()),
        Some(named) => named
            .iter()
            .map(|n| {
                dump.attachments
                    .iter()
                    .find(|a| a.path == *n || a.name == *n)
                    .map(|a| a.path.clone())
                    .ok_or_else(|| RpcError::BadParams(format!("The dump has no file {n}")))
            })
            .collect(),
    }
}

impl EngineRpc {
    /// Deliver a route's message with the dump's files it forwards, carried
    /// to wherever `chat` runs: as they are on this device, or staged as
    /// uploads whose bytes are pushed to its host.
    pub(crate) fn deliver_forwarding(
        &self,
        chat: &str,
        message: &str,
        brain: Option<&str>,
        files: &[String],
    ) -> Result<(), RpcError> {
        let host = self.workspace_host().chat(chat).ok().flatten().map(|c| c.device_id);
        let (paths, transfers) = self.forwardable(files, host.as_deref());
        super::ops::deliver_with(self, chat, message, brain, super::ops::When::Now, &paths, transfers).map(|_| ())
    }

    /// `files` as a chat on device `host` can get them.
    pub(crate) fn forwardable(
        &self,
        files: &[String],
        host: Option<&str>,
    ) -> (Vec<String>, Vec<crate::uploads::AttachmentTransfer>) {
        let uploads = self.uploads_ref();
        let here = host.is_none_or(|h| h == self.doc_host_ref().device_id());
        let mut paths = Vec::new();
        let mut transfers = Vec::new();
        for file in files {
            if here {
                paths.push(uploads.resolve_pending(file).unwrap_or_else(|| file.clone()));
                continue;
            }
            if let Some((upload_id, file_name)) = crate::uploads::parse_pending_ref(file) {
                transfers.push(crate::uploads::AttachmentTransfer {
                    upload_id: upload_id.to_owned(),
                    file_name: file_name.to_owned(),
                });
                paths.push(file.clone());
                continue;
            }
            // A file here, for another device: staged as an upload of its own.
            let source = std::path::Path::new(file);
            let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let upload_id = uuid::Uuid::new_v4().simple().to_string();
            let target = uploads.pending_target(&upload_id, &name);
            let staged = target.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::copy(source, &target));
            match staged {
                Ok(_) => {
                    paths.push(crate::uploads::pending_ref(&upload_id, &name));
                    transfers.push(crate::uploads::AttachmentTransfer { upload_id, file_name: name });
                }
                Err(error) => tracing::warn!(file = %file, %error, "brain dump: a file couldn't be forwarded"),
            }
        }
        (paths, transfers)
    }

    pub(crate) fn placement_projects(&self) -> Vec<Project> {
        let spaces = self.workspace_host().read_spaces().unwrap_or_default();
        let chats = self.workspace_host().read_chats().unwrap_or_default();
        projects(&spaces, &chats, self.doc_host_ref().device_id())
    }

    /// The project and device for a new thread or orchestrator: what the
    /// route named (checked), else the best project for `text`. `(None,
    /// None)` when there are no projects (it runs here, projectless).
    pub(crate) fn place(
        &self,
        text: &str,
        space_id: Option<String>,
        device_id: Option<String>,
    ) -> Result<(Option<String>, Option<String>), RpcError> {
        let space_id = space_id.filter(|s| !s.trim().is_empty());
        let device_id = device_id.filter(|d| !d.trim().is_empty());
        if let Some(space_id) = space_id {
            let space = self
                .workspace_host()
                .space(&space_id)
                .ok()
                .flatten()
                .ok_or_else(|| RpcError::BadParams(format!("No project {space_id}")))?;
            if device_id.as_deref().is_some_and(|d| d != space.device_id) {
                return Err(RpcError::BadParams(format!(
                    "Project {} lives on another device ({})",
                    space.display_name(),
                    space.device_id
                )));
            }
            return Ok((Some(space.id), Some(space.device_id)));
        }
        if let Some(device_id) = device_id {
            let known = self
                .workspace_host()
                .read_devices()
                .unwrap_or_default()
                .iter()
                .any(|d| d.id == device_id);
            if !known && device_id != self.doc_host_ref().device_id() {
                return Err(RpcError::BadParams(format!("No device {device_id}")));
            }
            return Ok((None, Some(device_id)));
        }
        let projects = self.placement_projects();
        Ok(match best_project(text, &projects) {
            Some(p) => (Some(p.space.id.clone()), Some(p.space.device_id.clone())),
            None => (None, None),
        })
    }

    /// Record that `by` started `chat` for the person (provenance, not
    /// parenthood: [`zeron_proto::Chat::started_by`]).
    pub(crate) fn mark_started_by(&self, chat: &str, by: &str, dump_id: Option<&str>, route_id: Option<&str>) {
        let started = zeron_proto::StartedBy {
            chat_id: by.to_owned(),
            dump_id: dump_id.map(str::to_owned),
            route_id: route_id.map(str::to_owned),
        };
        let marked = self
            .orchestration()
            .workspace()
            .write_orchestration(|doc| doc.set_chat_started_by(chat, &started));
        if !matches!(marked, Ok(true)) {
            tracing::warn!(chat = %chat, "started-by not recorded");
        }
    }

    /// The existing places among `candidates` (orchestrators, live chats,
    /// board items), each once.
    pub(crate) fn existing_candidates(&self, candidates: Vec<RouteTarget>) -> Vec<RouteTarget> {
        let orch = self.orchestration();
        let chats = self.workspace_host().read_chats().unwrap_or_default();
        let mut out: Vec<RouteTarget> = Vec::new();
        for c in candidates {
            let exists = match &c {
                RouteTarget::Orchestrator { id } | RouteTarget::NewOrchestrator { id } => orch
                    .orchestrator(id)
                    .is_some_and(|o| o.kind == OrchestratorKind::Standard && !o.archived),
                RouteTarget::Chat { id } | RouteTarget::NewThread { id } => {
                    chats.iter().any(|chat| &chat.id == id && !chat.archived)
                }
                RouteTarget::Item { id } => orch.item(id).is_some(),
                _ => false,
            };
            if exists && !out.contains(&c) {
                out.push(c);
            }
        }
        out
    }

    /// How a place reads as an option.
    fn place_label(&self, target: &RouteTarget) -> String {
        let orch = self.orchestration();
        let label = match target {
            RouteTarget::Orchestrator { id } | RouteTarget::NewOrchestrator { id } => {
                orch.orchestrator(id).map(|o| o.name).unwrap_or_else(|| id.clone())
            }
            RouteTarget::Chat { id } | RouteTarget::NewThread { id } => self
                .workspace_host()
                .chat(id)
                .ok()
                .flatten()
                .and_then(|c| c.title)
                .unwrap_or_else(|| id.chars().take(8).collect()),
            RouteTarget::Item { id } => orch.item(id).map(|i| i.title).unwrap_or_else(|| id.clone()),
            _ => "Somewhere".into(),
        };
        one_line(&label, 60)
    }

    /// The label of the new-thread option, for a route placed in `space`.
    fn new_thread_label(&self, space: Option<&str>) -> String {
        match space.and_then(|id| self.workspace_host().space(id).ok().flatten()) {
            Some(space) => format!("New thread in {}", one_line(&space.display_name(), 60)),
            None => "New thread".into(),
        }
    }

    /// Raise "Where should this go?" for the unsure `route` of `dump`: its
    /// candidates, then a new thread in its project. Placed (project) and
    /// stored on the route first. The ask's id.
    pub(crate) fn ask_where(&self, dump: &Dump, route: &mut Route, brain: Option<&str>) -> String {
        if route.space_id.is_none() && route.device_id.is_none() {
            let text = format!("{} {}", route.excerpt, route.message);
            if let Ok((space, device)) = self.place(&text, None, None) {
                route.space_id = space;
                route.device_id = device;
            }
        }
        let mut labels: Vec<String> = Vec::new();
        for c in &route.candidates {
            let mut label = self.place_label(c);
            // Labels pick the answer: each must be its own.
            let mut n = 2;
            while labels.contains(&label) {
                label = format!("{} ({n})", self.place_label(c));
                n += 1;
            }
            labels.push(label);
        }
        labels.push(self.new_thread_label(route.space_id.as_deref()));
        let now = Utc::now();
        let ask = Ask {
            id: uuid::Uuid::new_v4().simple().to_string(),
            from_chat: brain.unwrap_or(super::comms::YOU).to_owned(),
            orchestrator_id: brain.map(str::to_owned),
            item_id: None,
            kind: AskKind::Decision,
            question: format!("Where should this go?\n\n> {}", one_line(&route.excerpt, 400)),
            options: labels.into_iter().map(|label| AskOption { label, detail: String::new() }).collect(),
            urgent: false,
            state: AskState::Open,
            answer: None,
            created_at: now,
            answered_at: None,
            dump_id: Some(dump.id.clone()),
            route_id: Some(route.id.clone()),
            quiet: Default::default(),
        };
        self.orchestration().workspace().write_orchestration(|doc| doc.upsert_ask(&ask));
        self.orchestration().comms().say(
            CommKind::Ask,
            brain.unwrap_or(super::comms::YOU),
            super::comms::YOU,
            &format!("Where should this go? {}", one_line(&route.excerpt, 100)),
        );
        ask.id
    }

    /// The person answered a "Where should this go?" ask: move its route
    /// to the place picked (a new thread in its project for the last option
    /// or any other answer).
    pub(crate) async fn answer_where(&self, ask: &Ask, answer: &str) -> Result<(), RpcError> {
        let (Some(dump_id), Some(route_id)) = (ask.dump_id.as_deref(), ask.route_id.as_deref()) else {
            return Ok(());
        };
        let route = self
            .orchestration()
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .and_then(|d| d.routes.into_iter().find(|r| r.id == route_id))
            .ok_or_else(|| RpcError::Failed(format!("No route {route_id} in dump {dump_id}")))?;
        if route.state != RouteState::Pending || route.target != RouteTarget::Unsure {
            return Ok(()); // moved already (from the receipt)
        }
        let picked = ask
            .options
            .iter()
            .position(|o| o.label.trim().eq_ignore_ascii_case(answer.trim()));
        let target = match picked {
            Some(i) if i < route.candidates.len() => route.candidates[i].clone(),
            _ => RouteTarget::NewThread { id: String::new() },
        };
        self.reroute(dump_id, route_id, target).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(id: &str, name: &str, path: &str, device: &str) -> Space {
        serde_json::from_value(serde_json::json!({
            "id": id, "deviceId": device, "path": path, "name": name,
            "gitDetected": true, "createdAt": "2026-11-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn project(id: &str, name: &str, path: &str, device: &str, here: bool, active: i64) -> Project {
        Project {
            space: space(id, name, path, device),
            last_active: DateTime::from_timestamp(active, 0),
            here,
        }
    }

    #[test]
    fn the_project_the_text_names_wins_this_devices_first() {
        let projects = vec![
            project("s1", "tally", "/home/u/tally", "d1", true, 300),
            project("s2", "zeron", "/srv/zeron", "honeypot", false, 200),
            project("s3", "zeron", "/home/u/zeron", "d1", true, 100),
        ];
        let text = "make it so that there cant be 2 devices with same name on zeron";
        assert_eq!(best_project(text, &projects).unwrap().space.id, "s3", "named, and here");
        let only_remote = &projects[..2];
        assert_eq!(best_project(text, only_remote).unwrap().space.id, "s2", "named, on its device");
        assert_eq!(best_project("buy milk", &projects).unwrap().space.id, "s1", "most recently active");
        assert!(best_project("anything", &[]).is_none());
        // A folder name counts; a word inside another doesn't.
        let p = vec![project("s4", "", "/code/my-app", "d1", true, 1)];
        assert_eq!(best_project("fix my-app login", &p).unwrap().space.id, "s4");
        let q = vec![project("s6", "web", "/x/web", "d1", true, 9), project("s5", "app", "/x/app", "d1", true, 1)];
        assert_eq!(best_project("the happy path", &q).unwrap().space.id, "s6");
    }

    /// A file here, for a thread on another device: staged as an upload of
    /// its own, its bytes pushed there; for one here, the path as it is.
    #[tokio::test]
    async fn files_go_to_another_device_as_uploads() {
        let temp = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(temp.path()).unwrap(),
            std::sync::Arc::new(crate::registry::HarnessRegistry::new()),
            zeron_proto::HarnessId::Codex,
            None,
        )
        .unwrap();
        let rpc = core.rpc_service();
        let file = temp.path().join("shot.png");
        std::fs::write(&file, b"png").unwrap();
        let file = file.to_string_lossy().into_owned();
        let (paths, transfers) = rpc.forwardable(std::slice::from_ref(&file), Some(core.device_id.as_str()));
        assert_eq!((paths, transfers.len()), (vec![file.clone()], 0));
        let (paths, transfers) = rpc.forwardable(std::slice::from_ref(&file), Some("honeypot"));
        assert_eq!(transfers.len(), 1);
        assert_eq!(paths[0], crate::uploads::pending_ref(&transfers[0].upload_id, "shot.png"));
        let staged = rpc.uploads_ref().resolve_pending(&paths[0]).expect("staged here");
        assert_eq!(std::fs::read(staged).unwrap(), b"png");
    }

    #[test]
    fn projects_sort_by_recent_work() {
        let spaces = vec![space("a", "a", "/a", "d1"), space("b", "b", "/b", "d2")];
        let chat = |space: &str, at: i64| -> Chat {
            serde_json::from_value(serde_json::json!({
                "id": format!("c-{space}-{at}"), "deviceId": "d1", "spaceId": space,
                "lastMessageAt": DateTime::from_timestamp(at, 0).unwrap(), "createdAt": "2026-11-01T00:00:00Z", "archived": false
            }))
            .unwrap()
        };
        let listed = projects(&spaces, &[chat("a", 10), chat("b", 50)], "d1");
        assert_eq!(listed.iter().map(|p| p.space.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert!(listed[1].here && !listed[0].here);
    }
}
