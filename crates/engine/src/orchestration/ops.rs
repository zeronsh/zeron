//! The `Orchestrate` RPC: one [`OrchestrateOp`] per call.
//!
//! The person (the app, the CLI, another of their devices) may do every op.
//! An agent (its Zeron MCP server's token) does only what the contract marks
//! `(agent)`, scoped to its own chat as the engine knows it: an orchestrator
//! to its own crew, board and notes; a crew thread reports and asks; the
//! Brain dump creates orchestrators, items and routes. The calling chat is
//! the one its MCP server names (`Caller::Agent { chat }`), never one the
//! params name.

use chrono::Utc;
use serde_json::{Value, json};
use zeron_proto::orchestration::{
    AnswerVia, Ask, AskOrigin, AskState, CommKind, CrewMember, CrewPolicy, OrchestrateOp, Orchestrator, OrchestratorKind,
    WorkItemPatch, seed_hue,
};
use zeron_proto::{ChatConfig, ChildKind, SandboxLevel, SessionStatus, agent_messages};
use zeron_rpc::{Caller, RpcError};

use super::comms::YOU;
use super::ChatRole;
use crate::rpc::EngineRpc;

fn failed(message: impl Into<String>) -> RpcError {
    RpcError::Failed(message.into())
}

fn engine_failed(error: crate::EngineError) -> RpcError {
    RpcError::Failed(error.to_string())
}

/// Who is calling, as the ops see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Who {
    /// The person (every op).
    Person,
    /// An agent in `chat`, which is `role`.
    Agent { chat: String, role: ChatRole },
}

impl Who {
    fn chat(&self) -> Option<&str> {
        match self {
            Who::Person => None,
            Who::Agent { chat, .. } => Some(chat),
        }
    }
}

/// How a message goes to a chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum When {
    /// A turn if idle, into the running turn if working.
    Now,
    /// A turn if idle, else after the running one.
    Next,
}

/// What [`deliver`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivered {
    Run,
    Steer,
    Queued,
}

impl EngineRpc {
    /// Who `zeron_rpc::caller()` is, its role from the rows.
    pub(crate) fn current_who(&self) -> Result<Who, RpcError> {
        Ok(match zeron_rpc::caller() {
            Caller::Agent { chat: Some(chat) } => {
                let role = self.orchestration().role_of(&chat);
                Who::Agent { chat, role }
            }
            Caller::Agent { chat: None } => {
                return Err(failed(
                    "Only an agent running in a Zeron chat takes part in orchestration",
                ));
            }
            Caller::Local | Caller::Peer { .. } => Who::Person,
        })
    }

    /// One op, from whoever `zeron_rpc::caller()` is.
    pub(crate) async fn orchestrate(&self, params: Value) -> Result<Value, RpcError> {
        let op: OrchestrateOp = serde_json::from_value(params)
            .map_err(|e| RpcError::BadParams(format!("Orchestrate: {e}")))?;
        let who = self.current_who()?;
        permitted(&who, &op, self)?;
        self.run_op(&who, op).await
    }

    pub(crate) async fn run_op(&self, who: &Who, op: OrchestrateOp) -> Result<Value, RpcError> {
        let orch = self.orchestration().clone();
        let now = Utc::now();
        match op {
            OrchestrateOp::CreateOrchestrator {
                name,
                goal,
                rules,
                crew_rules,
                space_id,
                device_id,
                harness,
                model,
                crew,
                prompt,
                request_id,
            } => {
                // One request, one orchestrator: the same request id again
                // (a double submit, a retry) is answered with the first's.
                let request_id = request_id.filter(|r| !r.trim().is_empty());
                let mut created = request_id.as_ref().map(|_| orch.created());
                if let (Some(made), Some(request)) = (created.as_ref(), request_id.as_ref())
                    && let Some(id) = made.get(request)
                {
                    return Ok(json!({ "id": id }));
                }
                let id = self.create_orchestrator_chat(NewOrchestrator {
                    name,
                    goal,
                    rules,
                    crew_rules,
                    space_id,
                    device_id,
                    harness,
                    model,
                    crew,
                })?;
                if let (Some(made), Some(request)) = (created.as_mut(), request_id) {
                    made.put(request, id.clone());
                }
                drop(created);
                if let Some(prompt) = prompt.filter(|p| !p.trim().is_empty()) {
                    deliver(self, &id, &prompt, who.chat(), When::Now, None)?;
                }
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::PromoteChat { chat_id, name } => {
                let chat = self
                    .workspace_host()
                    .chat(&chat_id)
                    .map_err(engine_failed)?
                    .ok_or_else(|| failed(format!("No chat {chat_id}")))?;
                if chat.parent_chat_id.is_some() {
                    return Err(failed("Only a top-level chat becomes an orchestrator"));
                }
                if orch.orchestrator(&chat_id).is_some() {
                    return Err(failed("That chat is already an orchestrator"));
                }
                if orch.crew_of(&chat_id).is_some() {
                    return Err(failed("That chat is in a crew: release it first"));
                }
                let name = name.trim();
                let name = if name.is_empty() {
                    chat.title.clone().unwrap_or_else(|| "Orchestrator".into())
                } else {
                    name.to_owned()
                };
                let row = Orchestrator {
                    id: chat_id.clone(),
                    kind: OrchestratorKind::Standard,
                    name,
                    goal: String::new(),
                    rules: String::new(),
                    crew_rules: String::new(),
                    notes: String::new(),
                    hue: seed_hue(&chat_id),
                    crew: CrewPolicy::default(),
                    wake: None,
                    paused: false,
                    archived: false,
                    created_at: now,
                    updated_at: now,
                    workflow: None,
                    flow_run: None,
                };
                orch.workspace()
                    .write_orchestration(|doc| doc.upsert_orchestrator(&row));
                Ok(json!({ "id": chat_id }))
            }
            OrchestrateOp::UpdateOrchestrator {
                id,
                name,
                goal,
                rules,
                crew_rules,
                notes,
                crew,
                wake,
                paused,
                archived,
                workflow,
            } => {
                orch.workspace().write_orchestration(|doc| {
                    let mut row = doc
                        .orchestrator(&id)
                        .ok_or_else(|| failed(format!("No orchestrator {id}")))?;
                    if let Some(name) = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty())
                    {
                        row.name = name;
                    }
                    if let Some(goal) = goal {
                        row.goal = goal;
                    }
                    if let Some(rules) = rules {
                        row.rules = rules;
                    }
                    if let Some(crew_rules) = crew_rules {
                        row.crew_rules = crew_rules;
                    }
                    if let Some(notes) = notes {
                        row.notes = notes;
                    }
                    if let Some(crew) = crew {
                        row.crew = crew;
                    }
                    if let Some(wake) = wake {
                        row.wake = wake.map(super::wake::normalized);
                    }
                    if let Some(paused) = paused {
                        row.paused = paused;
                    }
                    if let Some(archived) = archived {
                        row.archived = archived;
                    }
                    if let Some(workflow) = workflow {
                        row.workflow = workflow;
                    }
                    row.updated_at = now;
                    doc.upsert_orchestrator(&row);
                    Ok::<_, RpcError>(())
                })?;
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::AddToCrew {
                orchestrator_id,
                chat_id,
                role,
                item_id,
            } => {
                self.add_to_crew(&orchestrator_id, &chat_id, &role, item_id)?;
                Ok(json!({ "id": chat_id }))
            }
            OrchestrateOp::UpdateSettings { crew_auto_archive, crew_idle_hours } => {
                use zeron_proto::orchestration::{CREW_IDLE_HOURS_MAX, CREW_IDLE_HOURS_MIN};
                if let Some(hours) = crew_idle_hours
                    && !(CREW_IDLE_HOURS_MIN..=CREW_IDLE_HOURS_MAX).contains(&hours)
                {
                    return Err(failed(format!(
                        "The idle window is {CREW_IDLE_HOURS_MIN} to {CREW_IDLE_HOURS_MAX} hours"
                    )));
                }
                let settings = orch.workspace().write_orchestration(|doc| {
                    let mut settings = doc.orchestration_settings();
                    if let Some(on) = crew_auto_archive {
                        settings.crew_auto_archive = on;
                    }
                    if let Some(hours) = crew_idle_hours {
                        settings.crew_idle_hours = hours;
                        settings.crew_idle_hours_set = true;
                    }
                    doc.upsert_orchestration_settings(&settings);
                    settings
                });
                Ok(serde_json::to_value(settings).unwrap_or(Value::Null))
            }
            OrchestrateOp::ApproveItem { id } => {
                self.approve_item(&id, now)?;
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::RequestChanges { id, text } => {
                let text = text.trim().to_owned();
                if text.is_empty() {
                    return Err(RpcError::BadParams("Say what to change".into()));
                }
                self.request_changes(&id, &text)?;
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::RemoveItem { id } => {
                self.remove_item(&id)?;
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::AddToBoard { chat_id } => {
                let id = self.add_to_board(&chat_id)?;
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::ReleaseFromCrew { chat_id } => {
                let released = orch.workspace().write_orchestration(|doc| {
                    let Some(mut member) = doc.crew_member(&chat_id).filter(|m| !m.released) else {
                        return false;
                    };
                    member.released = true;
                    doc.upsert_crew_member(&member);
                    true
                });
                if !released {
                    return Err(failed("That chat isn't in a crew"));
                }
                self.workspace_host()
                    .set_chat_parent(&chat_id, None)
                    .map_err(engine_failed)?;
                Ok(json!({ "id": chat_id }))
            }
            OrchestrateOp::UpsertItem { mut item, ask } => {
                // An orchestrator's new item is its own unless it says.
                if item.id.is_none()
                    && item.orchestrator_id.is_none()
                    && let Who::Agent {
                        role: ChatRole::Orchestrator { id },
                        ..
                    } = who
                {
                    item.orchestrator_id = Some(Some(id.clone()));
                }
                let current = item.id.as_deref().and_then(|id| orch.item(id));
                // The person's own hold: Blocked is Paused, and a paused card
                // moved out is resumed.
                if *who == Who::Person
                    && let Some(card) = current.as_ref()
                {
                    match item.state {
                        Some(zeron_proto::orchestration::WorkState::Blocked) if card.state != zeron_proto::orchestration::WorkState::Blocked => {
                            item.state = None;
                            let id = card.id.clone();
                            orch.patch_item(item).map_err(RpcError::Failed)?;
                            return self.pause_item(&id, now);
                        }
                        Some(to) if card.paused() && to != zeron_proto::orchestration::WorkState::Blocked => {
                            item.state = None;
                            let id = card.id.clone();
                            orch.patch_item(item).map_err(RpcError::Failed)?;
                            return self.resume_item(&id, Some(to), now).await;
                        }
                        _ => {}
                    }
                }
                // Into Now from Inbox or Next (or made there).
                let was = current.as_ref().map(|i| i.state);
                let to_now = item.state == Some(zeron_proto::orchestration::WorkState::Active)
                    && was.is_none_or(|s| {
                        matches!(s, zeron_proto::orchestration::WorkState::Inbox | zeron_proto::orchestration::WorkState::Planned)
                    });
                let by_hand = item.state.is_some();
                let id = orch.patch_item(item).map_err(RpcError::Failed)?;
                // Its column set by hand: what its route says no longer moves
                // it, until that route's state changes (board_cards::sync_card);
                // the board check's note on it is spent.
                if by_hand {
                    self.mark_moved(&id, now);
                    orch.workspace().write_orchestration(|doc| {
                        if let Some(mut c) = doc.work_item(&id)
                            && c.check.is_some()
                        {
                            c.check = None;
                            doc.upsert_work_item(&c);
                        }
                    });
                }
                // Its question, raised with the move (Blocked waits on it).
                let ask_id = match (ask, who) {
                    (Some(ask), Who::Agent { chat, .. }) => {
                        if ask.question.trim().is_empty() {
                            return Err(RpcError::BadParams("The ask needs a question".into()));
                        }
                        orch.item(&id).map(|card| self.raise_item_ask(chat, &card, ask, AskOrigin::Tool, now))
                    }
                    (Some(ask), Who::Person) => orch.item(&id).map(|card| {
                        let from = card.orchestrator_id.clone().or(card.threads.first().cloned()).unwrap_or_default();
                        self.raise_item_ask(&from, &card, ask, AskOrigin::Tool, now)
                    }),
                    (None, _) => None,
                };
                orch.board_poke().poke();
                // The person moving a card with no thread to Now starts it.
                if to_now
                    && *who == Who::Person
                    && orch.item(&id).is_some_and(|i| i.threads.is_empty())
                {
                    return self.start_item(&id, true);
                }
                Ok(match ask_id {
                    Some(ask_id) => json!({ "id": id, "askId": ask_id }),
                    None => json!({ "id": id }),
                })
            }
            OrchestrateOp::PauseItem { id } => self.pause_item(&id, now),
            OrchestrateOp::ResumeItem { id, state } => self.resume_item(&id, state, now).await,
            OrchestrateOp::StartItem { id } => self.start_item(&id, *who == Who::Person),
            OrchestrateOp::StartFlow { orchestrator_id, task } => self.start_flow(&orchestrator_id, &task),
            OrchestrateOp::StopFlow { orchestrator_id } => self.stop_flow(&orchestrator_id),
            OrchestrateOp::Report {
                state,
                summary,
                detail,
                item_id,
                links,
                progress,
                result,
            } => {
                let Some(chat) = who.chat() else {
                    return Err(failed("Only a crew thread reports"));
                };
                super::report::file_report(
                    self,
                    chat,
                    super::report::ReportParams {
                        state,
                        summary,
                        detail,
                        item_id,
                        links,
                        progress,
                        result,
                    },
                )
                .await
            }
            OrchestrateOp::Ask {
                kind,
                question,
                options,
                urgent,
                item_id,
                images,
                recommendation,
                links,
                context,
            } => {
                let Who::Agent { chat, role } = who else {
                    return Err(failed("An ask comes from an agent's chat"));
                };
                if question.trim().is_empty() {
                    return Err(RpcError::BadParams("An ask needs a question".into()));
                }
                let item_id = item_id.or_else(|| match role {
                    ChatRole::Crew { item_id, .. } => item_id.clone(),
                    _ => None,
                });
                if let Some(i) = recommendation.as_ref().and_then(|r| r.option)
                    && i >= options.len()
                {
                    return Err(RpcError::BadParams(format!("The recommendation names option {i}; there are {}", options.len())));
                }
                let mut ask = Ask {
                    id: uuid::Uuid::new_v4().simple().to_string(),
                    from_chat: chat.clone(),
                    orchestrator_id: role.orchestrator_id().map(str::to_owned),
                    item_id,
                    kind,
                    question,
                    options,
                    urgent,
                    state: AskState::Open,
                    answer: None,
                    created_at: now,
                    answered_at: None,
                    dump_id: None,
                    route_id: None,
                    quiet: Default::default(),
                    origin: Default::default(),
                    turn_id: None,
                    updated_at: None,
                    escalated_at: None,
                    answered_via: None,
                    images,
                    recommendation: recommendation.filter(|r| !r.is_empty()),
                    links,
                    context: context.filter(|c| !c.trim().is_empty()),
                    deferred_until: None,
                };
                // Its pictures, kept in the uploads store (off the runtime:
                // up to a few files of 15 MB).
                if super::ask_images::any(&ask) {
                    let cwd = super::ask_images::cwd_of(self, chat);
                    let uploads = self.uploads_ref().clone();
                    ask = tokio::task::spawn_blocking(move || {
                        super::ask_images::store_in(&uploads, cwd.as_deref(), &mut ask);
                        ask
                    })
                    .await
                    .map_err(|e| failed(e.to_string()))?;
                }
                orch.workspace()
                    .write_orchestration(|doc| doc.upsert_ask(&ask));
                // Its thread's card waits on the person.
                self.sync_cards_of(&ask.from_chat);
                // Its banner shows on every device as the row arrives
                // (orchestration/watchers.rs), this one too.
                orch.comms().say(CommKind::Ask, chat, YOU, &ask.question);
                Ok(json!({ "id": ask.id }))
            }
            OrchestrateOp::AnswerAsk { id, answer, option } => self.answer_ask(who, id, answer, option).await,
            OrchestrateOp::DeferAsk { id, until } => {
                if !matches!(who, Who::Person) {
                    return Err(failed("Only the person puts an ask off"));
                }
                let asker = orch.workspace().write_orchestration(|doc| {
                    let mut ask = doc.ask(&id).ok_or_else(|| failed(format!("No ask {id}")))?;
                    if ask.state != AskState::Open {
                        return Err(failed("That ask was already answered or dismissed"));
                    }
                    // A time already past is "back now".
                    ask.deferred_until = until.filter(|u| *u > now);
                    doc.upsert_ask(&ask);
                    Ok::<_, RpcError>(ask.from_chat)
                })?;
                self.sync_cards_of(&asker);
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::DismissAsk { id } => {
                let asker = orch.workspace().write_orchestration(|doc| {
                    let mut ask = doc.ask(&id).ok_or_else(|| failed(format!("No ask {id}")))?;
                    if ask.state == AskState::Open {
                        ask.state = AskState::Dismissed;
                        ask.answered_at = Some(now);
                        doc.upsert_ask(&ask);
                    }
                    Ok::<_, RpcError>(ask.from_chat)
                })?;
                self.sync_cards_of(&asker);
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::Dump { text, attachments, transfers, config, refs } => {
                let id = self.dump(&text, attachments, transfers, config, refs).await?;
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::Route {
                dump_id,
                excerpt,
                target,
                intent,
                message,
                title,
                candidates,
                progress,
                space_id,
                device_id,
                attachments,
                item_state,
                reference,
            } => {
                let id = self
                    .route(
                        &dump_id,
                        excerpt,
                        target,
                        intent,
                        message,
                        title,
                        candidates,
                        progress,
                        space_id,
                        device_id,
                        attachments,
                        item_state,
                        reference,
                    )
                    .await?;
                Ok(json!({ "id": id }))
            }
            OrchestrateOp::ProgressCheck { chat_id } => {
                super::progress::check_reply(self, &chat_id).await
            }
            OrchestrateOp::DumpDone { dump_id, summary } => {
                self.dump_done(&dump_id, summary)?;
                Ok(json!({ "id": dump_id }))
            }
            OrchestrateOp::Reroute {
                dump_id,
                route_id,
                target,
            } => {
                let id = self.reroute(&dump_id, &route_id, target).await?;
                Ok(json!({ "id": id }))
            }
        }
    }

    /// `who` answers ask `id`: it is marked answered and the answer goes to
    /// the asker (a Brain dump's "where does this go?" moves its route
    /// instead). The person's `answerAsk` op, and a dump's reply on an
    /// `@ask` reference (brain.rs `route`).
    pub(crate) async fn answer_ask(
        &self,
        who: &Who,
        id: String,
        answer: String,
        option: Option<usize>,
    ) -> Result<Value, RpcError> {
        let orch = self.orchestration().clone();
        let now = Utc::now();
        let text = answer.trim().to_owned();
        if text.is_empty() && option.is_none() {
            return Err(RpcError::BadParams("An answer can't be empty".into()));
        }
        // With an option: its label, for a route's ask; the full answer
        // names it and any words.
        let mut picked: Option<String> = None;
        let mut answer = text.clone();
        let ask = orch.workspace().write_orchestration(|doc| {
            let mut ask = doc.ask(&id).ok_or_else(|| failed(format!("No ask {id}")))?;
            if ask.state != AskState::Open {
                return Err(failed("That ask was already answered or dismissed"));
            }
            if let Some(i) = option {
                let label = ask
                    .options
                    .get(i)
                    .map(|o| o.label.clone())
                    .ok_or_else(|| RpcError::BadParams(format!("That ask has no option {i}")))?;
                answer = zeron_proto::orchestration::prompt_text::picked_answer(i, &label, &text);
                picked = Some(label);
            }
            if let Who::Agent { role: ChatRole::Orchestrator { id: mine }, .. } = who
                && !orchestrator_may_answer(&ask, mine, doc.crew_member(&ask.from_chat).as_ref())
            {
                return Err(failed("Only the person answers that ask"));
            }
            ask.state = AskState::Answered;
            ask.answer = Some(answer.clone());
            ask.answered_at = Some(now);
            ask.answered_via = Some(match who {
                Who::Agent { .. } => AnswerVia::Orchestrator,
                _ => AnswerVia::Person,
            });
            doc.upsert_ask(&ask);
            Ok(ask)
        })?;
        // Who answered: the person, or (its crew's ask) an orchestrator.
        let answerer = match who {
            Who::Agent { chat, .. } => chat.as_str(),
            _ => YOU,
        };
        // "Where should this go?" moves its route to the pick; "hasn't
        // answered" asks again or lets it be. A thread's own question
        // on a route goes back to it, below.
        let route_answer = picked.unwrap_or_else(|| answer.clone());
        // A flow's ask (a gate, a step's pass or fail, a return limit): the
        // answer leads its run on.
        if ask.item_id.as_deref().and_then(|i| orch.item(i)).is_some_and(|c| c.flow.is_some()) {
            zeron_rpc::with_caller(zeron_rpc::Caller::Local, async { self.flow_on_answer(&ask, &route_answer) }).await;
        }
        if self.answer_route_ask(&ask, &route_answer).await? {
            orch.comms().say(CommKind::Answer, YOU, &ask.from_chat, &answer);
            return Ok(json!({ "id": id }));
        }
        // One line each: an ask's text can't pass for another message.
        let text =
            zeron_proto::orchestration::prompt_text::answer_delivery(&ask.question, &answer);
        let text = match who {
            Who::Agent { .. } => text.replacen("[Answer from you]", "[Answer from your orchestrator]", 1),
            _ => text,
        };
        let said = zeron_doc::Injected::answer(
            &ask.id,
            &ask.from_chat,
            ask.turn_id.as_deref(),
            matches!(who, Who::Agent { .. }),
        );
        deliver(self, &ask.from_chat, &text, None, When::Now, Some(said))?;
        orch.comms()
            .say(CommKind::Answer, answerer, &ask.from_chat, &answer);
        self.sync_cards_of(&ask.from_chat);
        Ok(json!({ "id": id }))
    }

    /// Put `chat_id` in `orchestrator_id`'s crew (moving it from another),
    /// its chat row under the orchestrator's as kind crew, onto `item_id`'s
    /// threads when given.
    pub(crate) fn add_to_crew(
        &self,
        orchestrator_id: &str,
        chat_id: &str,
        role: &str,
        item_id: Option<String>,
    ) -> Result<(), RpcError> {
        let orch = self.orchestration();
        let orchestrator = orch
            .orchestrator(orchestrator_id)
            .ok_or_else(|| failed(format!("No orchestrator {orchestrator_id}")))?;
        if orchestrator.kind != OrchestratorKind::Standard {
            return Err(failed("Only a standard orchestrator has a crew"));
        }
        if chat_id == orchestrator_id {
            return Err(failed("An orchestrator isn't in its own crew"));
        }
        if zeron_proto::voice::is_orchestrator_chat(chat_id)
            || zeron_proto::orchestration::is_brain_dump_chat(chat_id)
        {
            return Err(failed("Zeron's own chats aren't in a crew"));
        }
        if !matches!(
            orch.role_of(chat_id),
            ChatRole::Plain | ChatRole::Crew { .. }
        ) {
            return Err(failed("An orchestrator isn't in a crew"));
        }
        let chat = self
            .workspace_host()
            .chat(chat_id)
            .map_err(engine_failed)?
            .ok_or_else(|| failed(format!("No chat {chat_id}")))?;
        if let Some(parent) = chat.parent_chat_id.as_deref()
            && parent != orchestrator_id
            && chat.child_kind != Some(ChildKind::Crew)
        {
            return Err(failed("A chat delegated by another chat can't join a crew"));
        }
        let now = Utc::now();
        orch.workspace().write_orchestration(|doc| {
            if let Some(item) = item_id.as_deref()
                && doc.work_item(item).filter(|i| !i.removed).is_none()
            {
                return Err(failed(format!("No board item {item}")));
            }
            let previous = doc.crew_member(chat_id);
            let same_crew = previous
                .as_ref()
                .is_some_and(|m| !m.released && m.orchestrator_id == orchestrator_id);
            let member = CrewMember {
                chat_id: chat_id.to_owned(),
                orchestrator_id: orchestrator_id.to_owned(),
                item_id: item_id.clone().or_else(|| {
                    previous
                        .as_ref()
                        .filter(|_| same_crew)
                        .and_then(|m| m.item_id.clone())
                }),
                role: if role.trim().is_empty() {
                    previous
                        .as_ref()
                        .filter(|_| same_crew)
                        .map(|m| m.role.clone())
                        .unwrap_or_default()
                } else {
                    role.trim().to_owned()
                },
                last_report: previous
                    .as_ref()
                    .filter(|_| same_crew)
                    .and_then(|m| m.last_report.clone()),
                released: false,
                joined_at: previous
                    .as_ref()
                    .filter(|_| same_crew)
                    .map_or(now, |m| m.joined_at),
                // (Its archive marks are its chat's, whatever crew it's in.)
                archived_seen_at: previous.as_ref().and_then(|m| m.archived_seen_at),
                revived_at: previous.as_ref().and_then(|m| m.revived_at),
            };
            doc.upsert_crew_member(&member);
            if let Some(item) = member.item_id.as_deref().and_then(|id| doc.work_item(id).filter(|i| !i.removed))
                && !item.threads.iter().any(|t| t == chat_id)
            {
                let mut item = item;
                item.threads.push(chat_id.to_owned());
                item.updated_at = now;
                doc.upsert_work_item(&item);
            }
            Ok(())
        })?;
        self.workspace_host()
            .set_chat_parent(chat_id, Some((orchestrator_id, ChildKind::Crew)))
            .map_err(engine_failed)?;
        orch.comms().say(
            CommKind::Spawn,
            orchestrator_id,
            chat_id,
            chat.title.as_deref().unwrap_or("joined the crew"),
        );
        Ok(())
    }

    /// A new orchestrator: its chat, created as `createChat` creates any,
    /// named, and its row. The id (the chat's).
    pub(crate) fn create_orchestrator_chat(
        &self,
        new: NewOrchestrator,
    ) -> Result<String, RpcError> {
        let name = new.name.trim().to_owned();
        if name.is_empty() {
            return Err(RpcError::BadParams("An orchestrator needs a name".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.create_chat_for(
            &id,
            new.space_id.as_deref(),
            new.device_id.as_deref(),
            new.harness,
            new.model,
            None,
        )?;
        self.workspace_host()
            .rename_chat(&id, &name)
            .map_err(engine_failed)?;
        let now = Utc::now();
        let row = Orchestrator {
            id: id.clone(),
            kind: OrchestratorKind::Standard,
            name,
            goal: new.goal,
            rules: new.rules,
            crew_rules: new.crew_rules,
            notes: String::new(),
            hue: seed_hue(&id),
            crew: new.crew,
            wake: None,
            paused: false,
            archived: false,
            created_at: now,
            updated_at: now,
            workflow: None,
            flow_run: None,
        };
        self.orchestration()
            .workspace()
            .write_orchestration(|doc| doc.upsert_orchestrator(&row));
        Ok(id)
    }

    /// A chat created through `createChat` (the agent gate caps an agent's),
    /// in `space_id`, else on `device_id`, else here. A crew thread runs
    /// with no more than its orchestrator's sandbox and access, whoever
    /// starts it.
    pub(crate) fn create_chat_for(
        &self,
        chat_id: &str,
        space_id: Option<&str>,
        device_id: Option<&str>,
        harness: Option<String>,
        model: Option<String>,
        parent: Option<(&str, ChildKind)>,
    ) -> Result<(), RpcError> {
        let harness = match harness.as_deref().filter(|h| !h.trim().is_empty()) {
            Some(slug) => zeron_proto::HarnessId::from_slug(slug)
                .ok_or_else(|| RpcError::BadParams(format!("Unknown harness {slug}")))?,
            None => self.doc_host_ref().default_harness(),
        };
        let (mut sandbox, mut access) = (SandboxLevel::WorkspaceWrite, Default::default());
        if let Some((orchestrator, ChildKind::Crew)) = parent
            && let Ok(Some(row)) = self.workspace_host().chat(orchestrator)
            && let Some(ceiling) = row.config
        {
            (sandbox, access, _) = zeron_proto::cap_access((sandbox, access), (ceiling.sandbox, ceiling.access));
        }
        let config = ChatConfig {
            harness,
            model: model.filter(|m| !m.trim().is_empty()),
            reasoning: None,
            model_options: Default::default(),
            sandbox,
            access,
            plan: false,
        };
        let mut params = json!({
            "op": "createChat",
            "chatId": chat_id,
            "config": config,
        });
        match (
            space_id.filter(|s| !s.is_empty()),
            device_id.filter(|d| !d.is_empty()),
        ) {
            (Some(space), device) => {
                params["spaceId"] = json!(space);
                if let Some(device) = device {
                    params["deviceId"] = json!(device);
                }
            }
            (None, device) => {
                params["deviceId"] = json!(device.unwrap_or(self.doc_host_ref().device_id()));
            }
        }
        if let Some((parent, kind)) = parent {
            params["parentChatId"] = json!(parent);
            params["childKind"] = json!(kind.as_str());
        }
        self.create_chat_gated(params)
    }
}

/// `createOrchestrator`'s fields.
pub(crate) struct NewOrchestrator {
    pub name: String,
    pub goal: String,
    pub rules: String,
    pub crew_rules: String,
    pub space_id: Option<String>,
    pub device_id: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub crew: CrewPolicy,
}

/// Whether `who` may do `op`. The person may do everything.
pub(crate) fn permitted(who: &Who, op: &OrchestrateOp, rpc: &EngineRpc) -> Result<(), RpcError> {
    let Who::Agent { chat, role } = who else {
        return Ok(());
    };
    let refuse = |why: &str| Err(failed(why.to_owned()));
    let orch = rpc.orchestration();
    match op {
        OrchestrateOp::CreateOrchestrator { .. } => match role {
            ChatRole::BrainDump { .. } => Ok(()),
            _ => refuse("Only the Brain dump creates orchestrators: ask the person"),
        },
        OrchestrateOp::PromoteChat { .. } => refuse("Only the person makes a chat an orchestrator"),
        OrchestrateOp::StartFlow { .. } | OrchestrateOp::StopFlow { .. } => refuse("Only the person runs or stops a flow"),
        OrchestrateOp::AddToBoard { .. } => refuse("Only the person puts a chat on the board"),
        OrchestrateOp::RemoveItem { .. } => refuse("Only the person takes a card off the board"),
        OrchestrateOp::UpdateSettings { .. } => refuse("Only the person changes orchestration settings"),
        OrchestrateOp::ApproveItem { .. } | OrchestrateOp::RequestChanges { .. } => {
            refuse("Only the person approves a card or asks for changes")
        }
        OrchestrateOp::UpdateOrchestrator {
            id,
            name,
            goal,
            rules,
            crew_rules,
            crew,
            wake,
            paused,
            archived,
            notes: _,
            workflow,
        } => {
            let own = matches!(role, ChatRole::Orchestrator { id: mine } | ChatRole::BrainDump { id: mine } if mine == id);
            let only_notes = name.is_none()
                && goal.is_none()
                && rules.is_none()
                && crew_rules.is_none()
                && crew.is_none()
                && wake.is_none()
                && paused.is_none()
                && archived.is_none()
                && workflow.is_none();
            if own && only_notes {
                Ok(())
            } else {
                refuse("An orchestrator changes only its own notes: the person sets the rest")
            }
        }
        OrchestrateOp::AddToCrew {
            orchestrator_id,
            chat_id,
            item_id,
            ..
        } => crew_join_permitted(who, orchestrator_id, chat_id, item_id.as_deref(), false, rpc),
        OrchestrateOp::ReleaseFromCrew { chat_id } => match (role, orch.crew_of(chat_id)) {
            (ChatRole::Orchestrator { id }, Some(member)) if member.orchestrator_id == *id => Ok(()),
            _ => refuse("Only an orchestrator releases threads, from its own crew"),
        },
        OrchestrateOp::UpsertItem { item, ask } => {
            upsert_permitted(role, item, rpc)?;
            let current = item.id.as_deref().and_then(|id| orch.item(id));
            let open_ask = current.as_ref().is_some_and(|c| rpc.card_has_open_ask(&c.id));
            let threads_changed = item.threads.as_ref().is_some_and(|t| current.as_ref().is_none_or(|c| &c.threads != t));
            super::board_checks::agent_move_allowed(current.as_ref(), item.state, threads_changed, open_ask, ask.is_some())
                .map_err(failed)
        }
        OrchestrateOp::PauseItem { .. } | OrchestrateOp::ResumeItem { .. } => {
            refuse("Only the person pauses or resumes a card")
        }
        OrchestrateOp::StartItem { id } => match (role, orch.item(id)) {
            (ChatRole::Orchestrator { .. }, Some(card)) if card.flow.is_some() => {
                refuse("That card is a step of its flow's run: the engine starts it when its turn comes")
            }
            (ChatRole::Orchestrator { id: mine }, Some(card)) if card.orchestrator_id.as_deref() == Some(mine.as_str()) => Ok(()),
            _ => refuse("An orchestrator starts only its own cards: the person starts the rest"),
        },
        OrchestrateOp::Report { .. } => match role {
            ChatRole::Crew { .. } => Ok(()),
            _ => refuse("Only a crew thread reports"),
        },
        OrchestrateOp::Ask { item_id, .. } => match (item_id.as_deref(), role.orchestrator_id()) {
            (None, _) => Ok(()),
            (Some(item), Some(owner)) if item_is_of(rpc, Some(item), owner) => Ok(()),
            _ => refuse("An ask names only a board item of your own orchestrator"),
        },
        // An orchestrator answers its own crew's open turn-end asks (a
        // [Crew ask]); an `ask` a thread raised itself is the person's.
        // (Checked again where the answer is written.)
        OrchestrateOp::AnswerAsk { id, .. } => {
            let ask = orch.workspace().read_orchestration_with(|doc| doc.ask(id));
            match (role, ask) {
                (ChatRole::Orchestrator { id: mine }, Some(ask))
                    if orchestrator_may_answer(&ask, mine, orch.crew_of(&ask.from_chat).as_ref()) =>
                {
                    Ok(())
                }
                _ => refuse("Only the person answers an ask (an orchestrator: its own crew's open turn-end ones)"),
            }
        }
        OrchestrateOp::DismissAsk { .. } => refuse("Only the person dismisses an ask"),
        OrchestrateOp::DeferAsk { .. } => refuse("Only the person puts an ask off"),
        OrchestrateOp::Dump { .. } => refuse("Only the person dumps"),
        OrchestrateOp::Route { .. } | OrchestrateOp::DumpDone { .. } => match role {
            ChatRole::BrainDump { .. } => Ok(()),
            _ => refuse("Only the Brain dump routes"),
        },
        OrchestrateOp::Reroute { .. } => refuse("Only the person moves a route"),
        OrchestrateOp::ProgressCheck { chat_id } => {
            if super::progress::may_check(rpc, who, chat_id) {
                Ok(())
            } else {
                refuse("You may check the progress of your own chat, the chats you started and your crew")
            }
        }
    }
    .map_err(|e: RpcError| {
        tracing::info!(chat = %chat, %e, "orchestrate: an agent's op refused");
        e
    })
}

/// Whether orchestrator `mine` may answer `ask`: open, raised from a turn's
/// end (not the `ask` tool), by a thread still in its crew (`member`).
pub(crate) fn orchestrator_may_answer(ask: &Ask, mine: &str, member: Option<&CrewMember>) -> bool {
    ask.state == AskState::Open
        && ask.origin == AskOrigin::TurnEnd
        && ask.orchestrator_id.as_deref() == Some(mine)
        && ask.from_chat != mine
        && member.is_some_and(|m| !m.released && m.orchestrator_id == mine)
}

/// Whether `who` may put `chat_id` in `orchestrator_id`'s crew. The person
/// may add any plain thread. An agent only its own orchestrator's, and
/// only threads it started: a chat whose parent is already the calling
/// orchestrator (`new_child`: one `createChat` is making with that parent
/// now). Joining sets the chat's parent, which would take the person's own
/// chat, or another chat's subagent, off where they keep it.
pub(crate) fn crew_join_permitted(
    who: &Who,
    orchestrator_id: &str,
    chat_id: &str,
    item_id: Option<&str>,
    new_child: bool,
    rpc: &EngineRpc,
) -> Result<(), RpcError> {
    let Who::Agent { chat: caller, role } = who else {
        return Ok(());
    };
    let refuse = |why: &str| Err(failed(why.to_owned()));
    let ChatRole::Orchestrator { id } = role else {
        return refuse("Only an orchestrator adds threads, to its own crew");
    };
    if id != orchestrator_id || caller != id {
        return refuse("Only an orchestrator adds threads, to its own crew");
    }
    if zeron_proto::voice::is_orchestrator_chat(chat_id)
        || zeron_proto::orchestration::is_brain_dump_chat(chat_id)
        || !matches!(
            rpc.orchestration().role_of(chat_id),
            ChatRole::Plain | ChatRole::Crew { .. }
        )
    {
        return refuse("An orchestrator or Zeron's own chat isn't in a crew");
    }
    if !new_child {
        let parent = rpc
            .workspace_host()
            .chat(chat_id)
            .ok()
            .flatten()
            .and_then(|c| c.parent_chat_id);
        if parent.as_deref() != Some(id.as_str()) {
            return refuse("An orchestrator's crew is threads it started: the person adds others");
        }
    }
    if rpc
        .orchestration()
        .crew_of(chat_id)
        .is_some_and(|m| m.orchestrator_id != *id)
    {
        return refuse("That thread is in another orchestrator's crew");
    }
    if !item_is_of(rpc, item_id, id) {
        return refuse("That board item isn't yours");
    }
    Ok(())
}

/// Whether `item` (when named) is on `orchestrator`'s board.
fn item_is_of(rpc: &EngineRpc, item: Option<&str>, orchestrator: &str) -> bool {
    item.is_none_or(|item| {
        rpc.orchestration()
            .item(item)
            .is_some_and(|i| i.orchestrator_id.as_deref() == Some(orchestrator))
    })
}

/// An orchestrator keeps its own board: its items, and loose ones it
/// claims, with its own crew as their threads. The Brain dump keeps any.
fn upsert_permitted(
    role: &ChatRole,
    item: &WorkItemPatch,
    rpc: &EngineRpc,
) -> Result<(), RpcError> {
    let refuse = || Err(failed("An orchestrator keeps only its own board"));
    match role {
        ChatRole::BrainDump { .. } => Ok(()),
        ChatRole::Orchestrator { id } => {
            let mine = Some(id.as_str());
            let claims = item.orchestrator_id.as_ref().map(|o| o.as_deref() == mine);
            let current = match item.id.as_deref() {
                Some(item_id) => match rpc.orchestration().item(item_id) {
                    Some(current) => Some(current),
                    None => return Err(failed(format!("No board item {item_id}"))),
                },
                None => None,
            };
            let allowed = match &current {
                // Its own: kept its own.
                Some(current) if current.orchestrator_id.as_deref() == mine => {
                    claims != Some(false)
                }
                // Loose: only by claiming it.
                Some(current) if current.orchestrator_id.is_none() => claims == Some(true),
                Some(_) => false,
                // New: its own (the default).
                None => claims != Some(false),
            };
            if !allowed {
                return refuse();
            }
            if let Some(threads) = &item.threads {
                let known = current
                    .as_ref()
                    .map(|c| c.threads.clone())
                    .unwrap_or_default();
                let all_crew = threads.iter().all(|t| {
                    known.contains(t)
                        || rpc
                            .orchestration()
                            .crew_of(t)
                            .is_some_and(|m| m.orchestrator_id == *id)
                });
                if !all_crew {
                    return Err(failed("An item's threads are its orchestrator's crew"));
                }
            }
            Ok(())
        }
        _ => Err(failed(
            "Only orchestrators and the Brain dump keep the board",
        )),
    }
}

/// What `chat_id`'s agent is doing, freshest first: this engine's own runs
/// (all there is for a chat hosted here), else its host's synced status row.
pub(crate) fn chat_status(rpc: &EngineRpc, chat_id: &str) -> SessionStatus {
    if let Some(session) = rpc
        .sessions_ref()
        .watch_sessions()
        .borrow()
        .iter()
        .find(|s| s.chat_id == chat_id)
    {
        return session.status;
    }
    // Hosted here: this engine's runs are the truth; a stored row is what an
    // earlier run of this engine left (a stop, a restart), not a turn now.
    let here = rpc.orchestration().device_id();
    if rpc.workspace_host().chat(chat_id).ok().flatten().is_some_and(|c| c.device_id == here) {
        return SessionStatus::Idle;
    }
    rpc.workspace_host()
        .read_sessions()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.chat_id == chat_id)
        .map_or(SessionStatus::Idle, |s| s.status)
}

/// Send `text` to `chat_id` as a message from `from` (a chat; `None`: the
/// person, or the engine), running as the chat is set. A turn when it's
/// idle; when it's working, into that turn ([`When::Now`]) or after it
/// ([`When::Next`]); after the question when it waits on one.
pub(crate) fn deliver(
    rpc: &EngineRpc,
    chat_id: &str,
    text: &str,
    from: Option<&str>,
    when: When,
    injected: Option<zeron_doc::Injected>,
) -> Result<Delivered, RpcError> {
    deliver_with(rpc, chat_id, text, from, when, &[], Vec::new(), injected)
}

/// [`deliver`] with files: `attachments` as a chat send carries them (paths
/// on the chat's host, or `pending://` refs whose bytes `transfers` push
/// there), named after the words and given to the run.
pub(crate) fn deliver_with(
    rpc: &EngineRpc,
    chat_id: &str,
    text: &str,
    from: Option<&str>,
    when: When,
    attachments: &[String],
    transfers: Vec<crate::uploads::AttachmentTransfer>,
    // The engine's message this is (a crew ask, an answer…), for the
    // transcript: carried beside `text`, which the agent gets unchanged.
    injected: Option<zeron_doc::Injected>,
) -> Result<Delivered, RpcError> {
    let workspace = rpc.workspace_host();
    let chat = workspace
        .chat(chat_id)
        .map_err(engine_failed)?
        .ok_or_else(|| failed(format!("No chat {chat_id}")))?;
    let prompt = match from {
        Some(from) => {
            let sender = workspace.chat(from).ok().flatten();
            let short: String = from.chars().take(8).collect();
            let label = match sender
                .as_ref()
                .and_then(|c| c.title.as_deref())
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                Some(title) => format!("{title} ({short})"),
                None => short,
            };
            agent_messages::attributed(Some((from, &label)), chat_id, text)
        }
        None => text.to_owned(),
    };
    let status = chat_status(rpc, chat_id);
    let doc_host = rpc.doc_host_ref();
    // A thread of a card the person paused: an agent's message waits in its
    // held queue until they resume the card.
    let paused = from.is_some() && super::board_checks::paused_thread(rpc.orchestration(), chat_id);
    if paused {
        doc_host.hold_queue(chat_id);
    }
    // (A queued row keeps its files apart; a run and a steer name them.)
    let with_files = crate::doc_host::queued_message_prompt(&prompt, attachments);
    let queued = |doc_host: &crate::DocHost| {
        doc_host
            .queue_injected(chat_id, &prompt, attachments.to_vec(), true, transfers.clone(), injected.clone())
            .map(|_| Delivered::Queued)
            .map_err(engine_failed)
    };
    match (status, when) {
        _ if paused => queued(doc_host),
        (SessionStatus::Working, When::Now) => doc_host
            .queue_command_with_transfers(
                chat_id,
                zeron_doc::SessionCommandPayload::Steer {
                    prompt: with_files.clone(),
                    message_id: Some(uuid::Uuid::new_v4().to_string()),
                    injected: injected.clone(),
                },
                transfers.clone(),
            )
            .map(|_| Delivered::Steer)
            .map_err(engine_failed),
        (SessionStatus::Working | SessionStatus::AwaitingInput, _) => queued(doc_host),
        (SessionStatus::Idle | SessionStatus::Errored, _) => {
            let config = chat.config.clone();
            // A project's chat with no folder of its own runs in the project's.
            let cwd = chat
                .cwd
                .clone()
                .or_else(|| {
                    chat.space_id
                        .as_deref()
                        .and_then(|id| workspace.space(id).ok().flatten())
                        .map(|space| space.path)
                })
                .unwrap_or_else(|| "~".into());
            let request = zeron_proto::RunRequest {
                mcp: None,
                prompt: with_files.clone(),
                harness: Some(
                    config
                        .as_ref()
                        .map_or_else(|| doc_host.default_harness(), |c| c.harness),
                ),
                model: config.as_ref().and_then(|c| c.model.clone()),
                reasoning: config.as_ref().and_then(|c| c.reasoning),
                model_options: config
                    .as_ref()
                    .map(|c| c.model_options.clone())
                    .unwrap_or_default(),
                cwd,
                sandbox: config
                    .as_ref()
                    .map_or(SandboxLevel::WorkspaceWrite, |c| c.sandbox),
                auto_approve: false,
                access: config.as_ref().map(|c| c.access).unwrap_or_default(),
                plan: config.as_ref().is_some_and(|c| c.plan),
                resume: None,
                attachments: attachments.to_vec(),
                worktree: None,
                env: Default::default(),
                extra_mcp: Vec::new(),
            };
            doc_host
                .queue_command_with_transfers(
                    chat_id,
                    zeron_doc::SessionCommandPayload::Run {
                        request,
                        message_id: uuid::Uuid::new_v4().to_string(),
                        injected,
                    },
                    transfers,
                )
                .map(|_| Delivered::Run)
                .map_err(engine_failed)
        }
    }
}
