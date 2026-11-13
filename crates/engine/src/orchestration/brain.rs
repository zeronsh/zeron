//! The Brain dump: the person dumps text, the Brain dump agent splits it and
//! routes each fragment (`route`), the engine delivers each at once.
//!
//! The Brain dump is an orchestrator of kind `brainDump`, hosted on the
//! device the person dumps on, created there on first use. Its chat id has
//! [`BRAIN_DUMP_CHAT_PREFIX`], so it never lists as a session (like the voice
//! orchestrator's) and the chats it creates are top-level.

use chrono::Utc;
use zeron_proto::orchestration::{
    BRAIN_DUMP_CHAT_PREFIX, BRAIN_DUMP_TITLE, CommKind, CrewPolicy, Dump, DumpState, Orchestrator,
    OrchestratorKind, Route, RouteIntent, RouteRef, RouteState, RouteTarget, WorkItemPatch,
    WorkState, new_brain_dump_chat_id, seed_hue,
};
use zeron_rpc::RpcError;

use super::excerpt;
use super::ops::{NewOrchestrator, When, deliver};
use crate::rpc::EngineRpc;

/// A dump is cut to this many bytes (a row is an index entry).
pub const MAX_DUMP_BYTES: usize = 8 * 1024;

fn failed(message: impl Into<String>) -> RpcError {
    RpcError::Failed(message.into())
}

/// Byte offsets of `excerpt` in `text` (end exclusive), when it's there
/// verbatim.
pub(crate) fn span_of(text: &str, excerpt: &str) -> Option<(u32, u32)> {
    let excerpt = excerpt.trim();
    if excerpt.is_empty() {
        return None;
    }
    let from = text.find(excerpt)?;
    Some((from as u32, (from + excerpt.len()) as u32))
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The chat a route target delivers to, if it delivers to one.
fn target_chat(target: &RouteTarget) -> Option<&str> {
    match target {
        RouteTarget::Orchestrator { id }
        | RouteTarget::Chat { id }
        | RouteTarget::NewOrchestrator { id }
        | RouteTarget::NewThread { id } => Some(id).filter(|id| !id.is_empty()).map(String::as_str),
        _ => None,
    }
}

/// The column a route's new card goes in, when it makes one: an `item`
/// with no id (as asked, else as its wording says, else Inbox), or a task
/// for a new thread whose wording says it waits (`queue_of`). `None`: not a
/// new card.
pub(crate) fn waits(target: &RouteTarget, intent: RouteIntent, asked: Option<WorkState>, text: &str) -> Option<WorkState> {
    let asked = asked.map(|s| if s == WorkState::Planned { WorkState::Planned } else { WorkState::Inbox });
    match target {
        RouteTarget::Item { id } if id.is_empty() => {
            Some(asked.or_else(|| zeron_proto::orchestration::queue_of(text)).unwrap_or(WorkState::Inbox))
        }
        RouteTarget::NewThread { id } if id.is_empty() && intent == RouteIntent::Task => {
            asked.or_else(|| zeron_proto::orchestration::queue_of(text))
        }
        _ => None,
    }
}

/// Intents the owner of a crew thread decides on (it keeps its board).
pub(crate) fn owner_decides(intent: RouteIntent) -> bool {
    matches!(intent, RouteIntent::Task | RouteIntent::Priority | RouteIntent::Cancel)
}

/// A dump whose every fragment has a place is routed; one with an `unsure`
/// fragment still waiting needs the person.
pub(crate) fn settled_state(dump: &Dump) -> DumpState {
    if dump
        .routes
        .iter()
        .any(|r| r.target == RouteTarget::Unsure && r.state == RouteState::Pending)
    {
        DumpState::NeedsYou
    } else {
        DumpState::Routed
    }
}

impl EngineRpc {
    /// This device's Brain dump, created here on first use. Its id.
    pub(crate) fn ensure_brain_dump(&self) -> Result<String, RpcError> {
        let device = self.doc_host_ref().device_id().to_owned();
        let chats = self.workspace_host().read_chats().unwrap_or_default();
        let here = |id: &str| chats.iter().any(|c| c.id == id && c.device_id == device);
        let existing = self
            .orchestration()
            .state()
            .orchestrators
            .into_iter()
            .find(|o| o.kind == OrchestratorKind::BrainDump && !o.archived && here(&o.id));
        if let Some(existing) = existing {
            return Ok(existing.id);
        }
        let id = new_brain_dump_chat_id();
        debug_assert!(id.starts_with(BRAIN_DUMP_CHAT_PREFIX));
        self.create_chat_for(&id, None, Some(&device), None, None, None)?;
        self.workspace_host()
            .rename_chat(&id, BRAIN_DUMP_TITLE)
            .map_err(|e| failed(e.to_string()))?;
        let now = Utc::now();
        let row = Orchestrator {
            id: id.clone(),
            kind: OrchestratorKind::BrainDump,
            name: BRAIN_DUMP_TITLE.into(),
            goal: "Route the person's brain dumps".into(),
            rules: String::new(),
            crew_rules: String::new(),
            notes: String::new(),
            hue: seed_hue(&id),
            crew: CrewPolicy::default(),
            wake: None,
            paused: false,
            archived: false,
            created_at: now,
            updated_at: now,
        };
        self.orchestration()
            .workspace()
            .write_orchestration(|doc| doc.upsert_orchestrator(&row));
        Ok(id)
    }

    /// The person dumps `text`: recorded, and sent to the Brain dump as a
    /// run (after its current turn, if it's routing another). Its id.
    pub(crate) async fn dump(
        &self,
        text: &str,
        attachments: Vec<String>,
        transfers: Vec<zeron_proto::orchestration::UploadTransfer>,
        config: Option<zeron_proto::orchestration::DumpConfig>,
    ) -> Result<String, RpcError> {
        let text = cut(text.trim(), MAX_DUMP_BYTES);
        let files = super::placement::dump_files(attachments)?;
        let text = if text.is_empty() && !files.is_empty() { "(files)".to_owned() } else { text };
        if text.is_empty() {
            return Err(RpcError::BadParams("Nothing to dump".into()));
        }
        let brain = self.ensure_brain_dump()?;
        match &config {
            // The person's pick wins, and is kept on the chat.
            Some(pick) => self.brain_dump_pick(&brain, pick).await?,
            // Otherwise a Brain dump that has never run starts on a fast model.
            None => self.brain_dump_fast_model(&brain).await,
        }
        let now = Utc::now();
        let dump = Dump {
            id: uuid::Uuid::new_v4().simple().to_string(),
            text: text.clone(),
            state: DumpState::Routing,
            routes: Vec::new(),
            summary: None,
            device_id: self.doc_host_ref().device_id().to_owned(),
            created_at: now,
            updated_at: now,
            attachments: files.clone(),
        };
        self.orchestration()
            .workspace()
            .write_orchestration(|doc| doc.upsert_dump(&dump));
        self.orchestration()
            .comms()
            .say(CommKind::Message, super::comms::YOU, &brain, &text);
        let message = zeron_proto::orchestration::prompt_text::dump_message(&dump.id, &text);
        // The Brain dump sees the files, as a chat send carries them.
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let transfers = transfers
            .into_iter()
            .map(|t| crate::uploads::AttachmentTransfer { upload_id: t.upload_id, file_name: t.file_name })
            .collect();
        if let Err(error) = super::ops::deliver_with(self, &brain, &message, None, When::Next, &paths, transfers) {
            self.orchestration().workspace().write_orchestration(|doc| {
                if let Some(mut d) = doc.dump(&dump.id) {
                    d.state = DumpState::Failed;
                    d.updated_at = Utc::now();
                    doc.upsert_dump(&d);
                }
            });
            return Err(error);
        }
        Ok(dump.id)
    }

    /// Record one fragment's route and deliver it. The route's id. A
    /// question about a working thread's progress (`progress`) is answered
    /// from its [`Progress`](zeron_proto::orchestration::Progress) instead:
    /// asking it would interrupt its work.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn route(
        &self,
        dump_id: &str,
        excerpt_text: String,
        target: RouteTarget,
        intent: RouteIntent,
        message: String,
        title: Option<String>,
        candidates: Vec<RouteTarget>,
        progress: bool,
        space_id: Option<String>,
        device_id: Option<String>,
        attachments: Option<Vec<String>>,
        item_state: Option<WorkState>,
    ) -> Result<String, RpcError> {
        let orch = self.orchestration().clone();
        let dump = orch
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .ok_or_else(|| failed(format!("No dump {dump_id}")))?;
        if target == RouteTarget::Other {
            return Err(RpcError::BadParams("Unknown route target".into()));
        }
        // A "question" that asks for work is a task; a rhetorical one, info
        // (the person's words, checked: zeron_proto::direct_question).
        let intent = zeron_proto::direct_question::settle_intent(intent, &excerpt_text);
        // Work for a crew thread goes through its orchestrator, so its board
        // sees it; info and questions may go to the thread itself.
        let (target, message) = self.through_owner(target, intent, message);
        // Unsure only between existing places: with fewer than two it does
        // the work instead, in a new thread in the best project.
        let candidates = self.existing_candidates(candidates);
        let target = if target == RouteTarget::Unsure && candidates.len() < super::placement::UNSURE_MIN {
            RouteTarget::NewThread { id: String::new() }
        } else {
            target
        };
        let candidates = if target == RouteTarget::Unsure { candidates } else { Vec::new() };
        // What the person says should wait is a card, not a thread: Inbox
        // for later, Next for after something else.
        let card = waits(&target, intent, item_state, &format!("{excerpt_text} {message}"));
        let target = if card.is_some() { RouteTarget::Item { id: String::new() } } else { target };
        // Where a new thread or orchestrator starts (and where a new card's
        // will): as named, else placed.
        let (space_id, device_id) = match &target {
            RouteTarget::NewThread { id } | RouteTarget::NewOrchestrator { id } | RouteTarget::Item { id }
                if id.is_empty() =>
            {
                self.place(&format!("{excerpt_text} {message}"), space_id, device_id)?
            }
            _ => (None, None),
        };
        let mut route = Route {
            id: uuid::Uuid::new_v4().simple().to_string(),
            span: span_of(&dump.text, &excerpt_text),
            excerpt: excerpt_text,
            target,
            intent,
            message,
            title: title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
            state: RouteState::Pending,
            candidates,
            answer: None,
            answer_full: None,
            answered_at: None,
            item_id: None,
            space_id,
            device_id,
            attachments: super::placement::route_files(&dump, intent, attachments)?,
        };
        let brain = self.brain_of(&dump);
        let answered = match (&route.target, intent == RouteIntent::Question && progress) {
            (RouteTarget::Chat { id } | RouteTarget::Orchestrator { id }, true) => self.progress_answer(id).await,
            _ => None,
        };
        let to = target_chat(&route.target).unwrap_or("").to_owned();
        match answered {
            Some(answer) => {
                route.state = RouteState::Answered;
                route.answer = Some(answer.clone());
                route.answered_at = Some(Utc::now());
                orch.comms().say(CommKind::Answer, &to, super::comms::YOU, &answer);
            }
            None => self.deliver_route(&dump, &mut route, brain.as_deref(), card).await?,
        }
        // Not sure between places: the person picks, from Needs you.
        if route.target == RouteTarget::Unsure {
            self.ask_where(&dump, &mut route, brain.as_deref());
        }
        // A task it hands a thread lands on the board.
        self.card_for_route(&dump, &mut route);
        orch.workspace().write_orchestration(|doc| {
            let Some(mut current) = doc.dump(dump_id) else {
                return;
            };
            current.routes.push(route.clone());
            if route.target == RouteTarget::Unsure {
                current.state = DumpState::NeedsYou;
            }
            current.updated_at = Utc::now();
            doc.upsert_dump(&current);
        });
        orch.comms().say(
            CommKind::Route,
            brain.as_deref().unwrap_or(super::comms::YOU),
            &to,
            &route.excerpt,
        );
        Ok(route.id)
    }

    /// The answer to "how's it going?" for `chat`, from its progress, when
    /// it is working (or waiting to go on); `None` when it isn't, so the
    /// question goes to it as any would.
    async fn progress_answer(&self, chat: &str) -> Option<String> {
        let working = super::ops::chat_status(self, chat) == zeron_proto::SessionStatus::Working;
        let (progress, _) = super::progress::check(self, chat).await.ok()?;
        (working || progress.state.active()).then(|| progress.answer(Utc::now()))
    }

    /// `target` and `message` as a route delivers them: a task, priority or
    /// cancel aimed at a crew thread (or the thread a new-thread route names)
    /// goes to that thread's orchestrator instead, naming the thread.
    fn through_owner(&self, target: RouteTarget, intent: RouteIntent, message: String) -> (RouteTarget, String) {
        if !owner_decides(intent) {
            return (target, message);
        }
        let thread = match &target {
            RouteTarget::Chat { id } | RouteTarget::NewThread { id } if !id.is_empty() => id.clone(),
            _ => return (target, message),
        };
        let Some(member) = self.orchestration().crew_of(&thread) else {
            return (target, message);
        };
        let title = self
            .workspace_host()
            .chat(&thread)
            .ok()
            .flatten()
            .and_then(|c| c.title)
            .unwrap_or_else(|| thread.chars().take(8).collect());
        let message = zeron_proto::orchestration::role_prompts::for_crew_thread(&title, &thread, &message);
        (RouteTarget::Orchestrator { id: member.orchestrator_id }, message)
    }

    /// The Brain dump: done routing `dump_id`.
    pub(crate) fn dump_done(&self, dump_id: &str, summary: String) -> Result<(), RpcError> {
        self.orchestration().workspace().write_orchestration(|doc| {
            let mut dump = doc
                .dump(dump_id)
                .ok_or_else(|| failed(format!("No dump {dump_id}")))?;
            let summary = excerpt(&summary, 200);
            dump.summary = (!summary.is_empty()).then_some(summary);
            dump.state = settled_state(&dump);
            dump.updated_at = Utc::now();
            doc.upsert_dump(&dump);
            Ok(())
        })
    }

    /// The person moves a route: a retraction to where it went, the message
    /// to where it goes now.
    pub(crate) async fn reroute(
        &self,
        dump_id: &str,
        route_id: &str,
        target: RouteTarget,
    ) -> Result<String, RpcError> {
        let orch = self.orchestration().clone();
        let dump = orch
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .ok_or_else(|| failed(format!("No dump {dump_id}")))?;
        let mut route = dump
            .routes
            .iter()
            .find(|r| r.id == route_id)
            .cloned()
            .ok_or_else(|| failed(format!("No route {route_id} in that dump")))?;
        if matches!(target, RouteTarget::Other | RouteTarget::Unsure) {
            return Err(RpcError::BadParams("Move it to a place".into()));
        }
        if route.target == target {
            return Ok(route.id);
        }
        let brain = self.brain_of(&dump);
        let delivered = matches!(route.state, RouteState::Delivered | RouteState::Answered);
        if delivered && let Some(old) = target_chat(&route.target) {
            let retraction = zeron_proto::orchestration::prompt_text::retraction(&route.excerpt);
            if let Err(error) = deliver(self, old, &retraction, brain.as_deref(), When::Now) {
                tracing::warn!(chat = %old, %error, "brain dump: retraction not delivered");
            }
        }
        route.target = target;
        route.state = RouteState::Pending;
        route.answer = None;
        route.answer_full = None;
        // (The person's move to a new thread starts it, whatever it says.)
        let card = match route.target {
            RouteTarget::Item { .. } => waits(&route.target, route.intent, None, &format!("{} {}", route.excerpt, route.message)),
            _ => None,
        };
        self.deliver_route(&dump, &mut route, brain.as_deref(), card)
            .await?;
        self.card_for_route(&dump, &mut route);
        let to = target_chat(&route.target).unwrap_or("").to_owned();
        orch.workspace().write_orchestration(|doc| {
            let Some(mut current) = doc.dump(dump_id) else {
                return;
            };
            if let Some(slot) = current.routes.iter_mut().find(|r| r.id == route.id) {
                *slot = route.clone();
            }
            if current.state != DumpState::Routing {
                current.state = settled_state(&current);
            }
            current.updated_at = Utc::now();
            doc.upsert_dump(&current);
        });
        orch.comms()
            .say(CommKind::Route, super::comms::YOU, &to, &route.excerpt);
        // Its "Where should this go?" is settled by the move.
        orch.workspace().write_orchestration(|doc| {
            for mut ask in doc.read_asks() {
                if ask.state == zeron_proto::orchestration::AskState::Open
                    && ask.dump_id.as_deref() == Some(dump_id)
                    && ask.route_id.as_deref() == Some(route_id)
                {
                    ask.state = zeron_proto::orchestration::AskState::Answered;
                    ask.answer.get_or_insert_with(|| to.clone());
                    ask.answered_at = Some(Utc::now());
                    doc.upsert_ask(&ask);
                }
            }
        });
        Ok(route.id)
    }

    /// The Brain dump a dump was routed by: the one hosted where it was
    /// dumped, else any.
    pub(crate) fn brain_of(&self, dump: &Dump) -> Option<String> {
        let chats = self.workspace_host().read_chats().unwrap_or_default();
        let brains: Vec<Orchestrator> = self
            .orchestration()
            .state()
            .orchestrators
            .into_iter()
            .filter(|o| o.kind == OrchestratorKind::BrainDump)
            .collect();
        brains
            .iter()
            .find(|o| {
                chats
                    .iter()
                    .any(|c| c.id == o.id && c.device_id == dump.device_id)
            })
            .or(brains.first())
            .map(|o| o.id.clone())
    }

    /// Deliver `route` as its target says, creating what it names first;
    /// its target, state and item updated to what happened.
    async fn deliver_route(
        &self,
        dump: &Dump,
        route: &mut Route,
        brain: Option<&str>,
        card: Option<WorkState>,
    ) -> Result<(), RpcError> {
        let orch = self.orchestration().clone();
        let message = if route.message.trim().is_empty() {
            route.excerpt.clone()
        } else {
            route.message.clone()
        };
        let title = excerpt(&route.excerpt, 60);
        match route.target.clone() {
            RouteTarget::Orchestrator { id } => {
                if orch.orchestrator(&id).is_none() {
                    return Err(failed(format!("No orchestrator {id}")));
                }
                self.deliver_forwarding(&id, &message, brain, &route.attachments)?;
                route.state = RouteState::Delivered;
            }
            RouteTarget::Chat { id } => {
                self.deliver_forwarding(&id, &message, brain, &route.attachments)?;
                route.state = RouteState::Delivered;
            }
            RouteTarget::NewOrchestrator { id } => {
                let id = if !id.is_empty() && orch.orchestrator(&id).is_some() {
                    id
                } else {
                    self.create_orchestrator_chat(NewOrchestrator {
                        name: if title.is_empty() {
                            "New orchestrator".into()
                        } else {
                            title.clone()
                        },
                        goal: message.clone(),
                        rules: String::new(),
                        crew_rules: String::new(),
                        space_id: route.space_id.clone(),
                        device_id: route.device_id.clone(),
                        harness: None,
                        model: None,
                        crew: CrewPolicy::default(),
                    })
                    .await?
                };
                if let Some(brain) = brain {
                    self.mark_started_by(&id, brain, Some(&dump.id), Some(&route.id));
                }
                self.deliver_forwarding(&id, &message, brain, &route.attachments)?;
                route.target = RouteTarget::NewOrchestrator { id };
                route.state = RouteState::Delivered;
            }
            RouteTarget::NewThread { id } => {
                let exists =
                    !id.is_empty() && self.workspace_host().chat(&id).ok().flatten().is_some();
                let id = if exists {
                    id
                } else {
                    let id = uuid::Uuid::new_v4().to_string();
                    if route.space_id.is_none() && route.device_id.is_none() {
                        let (space, device) =
                            self.place(&format!("{} {}", route.excerpt, message), None, None)?;
                        route.space_id = space;
                        route.device_id = device;
                    }
                    self.create_chat_for(
                        &id,
                        route.space_id.as_deref(),
                        route.device_id.as_deref(),
                        None,
                        None,
                        None,
                    )?;
                    if let Some(brain) = brain {
                        self.mark_started_by(&id, brain, Some(&dump.id), Some(&route.id));
                    }
                    if !title.is_empty() {
                        self.workspace_host()
                            .rename_chat(&id, &title)
                            .map_err(|e| failed(e.to_string()))?;
                    }
                    id
                };
                self.deliver_forwarding(&id, &message, brain, &route.attachments)?;
                route.target = RouteTarget::NewThread { id };
                route.state = RouteState::Delivered;
            }
            RouteTarget::Item { id } => {
                let existing = (!id.is_empty()).then(|| orch.item(&id)).flatten();
                let item_id = match existing {
                    Some(item) => item.id,
                    None => orch
                        .patch_item(WorkItemPatch {
                            title: Some(super::board_cards::route_card_title(route, &message)),
                            detail: Some(message.clone()),
                            state: Some(card.unwrap_or(WorkState::Inbox)),
                            priority: Some(super::board_cards::priority_of(&format!("{} {message}", route.excerpt))),
                            source: Some(RouteRef {
                                dump_id: dump.id.clone(),
                                route_id: route.id.clone(),
                            }),
                            space_id: Some(route.space_id.clone()),
                            device_id: Some(route.device_id.clone()),
                            ..Default::default()
                        })
                        .map_err(RpcError::Failed)?,
                };
                route.item_id = Some(item_id.clone());
                route.target = RouteTarget::Item { id: item_id };
                route.state = RouteState::Delivered;
            }
            RouteTarget::Note => {
                let Some(brain) = brain else {
                    return Err(failed("No Brain dump to keep the note"));
                };
                orch.workspace().write_orchestration(|doc| {
                    if let Some(mut o) = doc.orchestrator(brain) {
                        if !o.notes.is_empty() && !o.notes.ends_with('\n') {
                            o.notes.push('\n');
                        }
                        o.notes.push_str(&format!("- {message}\n"));
                        o.updated_at = Utc::now();
                        doc.upsert_orchestrator(&o);
                    }
                });
                route.state = RouteState::Delivered;
            }
            RouteTarget::Unsure => {
                route.state = RouteState::Pending;
            }
            RouteTarget::Other => return Err(RpcError::BadParams("Unknown route target".into())),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_byte_offsets_of_the_verbatim_excerpt() {
        let text = "café first. then the auth bug";
        assert_eq!(span_of(text, "the auth bug"), Some((18, 30)));
        assert_eq!(&text[18..30], "the auth bug");
        assert_eq!(span_of(text, "not there"), None);
        assert_eq!(span_of(text, "  "), None);
    }
}
