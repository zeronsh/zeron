//! The Brain dump: the person dumps text, the Brain dump agent splits it and
//! routes each fragment (`route`), the engine delivers each at once.
//!
//! The Brain dump is an orchestrator of kind `brainDump`, hosted on the
//! device the person dumps on, created there on first use. Its chat id has
//! [`BRAIN_DUMP_CHAT_PREFIX`], so it never lists as a session (like the voice
//! orchestrator's) and the chats it creates are top-level.

use chrono::Utc;
use zeron_proto::orchestration::{
    AskState, BRAIN_DUMP_CHAT_PREFIX, BRAIN_DUMP_TITLE, CommKind, CrewPolicy, Dump, DumpRef, DumpState,
    Orchestrator, OrchestratorKind, RefKind, Route, RouteIntent, RouteRef, RouteState, RouteTarget,
    WorkItemPatch, WorkState, new_brain_dump_chat_id, seed_hue,
};
use zeron_rpc::RpcError;

use super::excerpt;
use super::ops::{NewOrchestrator, When, Who, deliver};
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

/// The sentence (or line) of `text` that holds `span`: from the last
/// `.`, `!`, `?` or line break before it to the next one after it.
pub(crate) fn sentence_around(text: &str, span: (u32, u32)) -> String {
    let (from, to) = (span.0 as usize, span.1 as usize);
    let stop = |c: char| matches!(c, '.' | '!' | '?' | '\n' | ';');
    let start = text[..from].rfind(stop).map_or(0, |i| i + 1);
    let end = text[to..].find(stop).map_or(text.len(), |i| to + i);
    text[start..end].trim().to_owned()
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

/// Where a fragment about one of the dump's references goes (see
/// [`EngineRpc::aim_of`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Aim {
    /// As the Brain dump routed it (the reference is context; a device is
    /// where a new thread starts).
    Routed,
    /// Answers this open ask (its asker gets the answer).
    Answer { ask_id: String, asker: String },
    /// Straight to this place, not through a crew thread's owner. `card`:
    /// the card it is a comment on.
    Direct { target: RouteTarget, card: Option<String> },
    /// Kept on a card with no thread or owner yet (its detail).
    OnCard { item_id: String },
}

/// Whether a reply reads as approving a card in Review ("approve",
/// "looks good", "lgtm", "ship it").
pub(crate) fn reads_as_approval(text: &str) -> bool {
    let text = text.to_lowercase();
    let negated = ["not ", "n't", "don't", "dont", "no "].iter().any(|n| text.contains(n));
    !negated
        && [
            "approve", "approved", "lgtm", "looks good", "look good", "ship it", "sgtm", "good to go",
            "merge it", "go ahead",
        ]
        .iter()
        .any(|p| text.contains(p))
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
        refs: Vec<DumpRef>,
    ) -> Result<String, RpcError> {
        let text = cut(text.trim(), MAX_DUMP_BYTES);
        let files = super::placement::dump_files(attachments)?;
        let mut refs = zeron_proto::orchestration::clean_refs(refs);
        // A card's state as the engine has it when the person dumps (never the
        // window's word for it): a reply only counts as a verdict on a card
        // that was in Review as they wrote it.
        for r in refs.iter_mut().filter(|r| r.kind == zeron_proto::orchestration::RefKind::Card) {
            r.context.remove("state");
            if let Some(card) = self.orchestration().item(&r.id) {
                r.context.insert("state".into(), if card.state == WorkState::Review { "review" } else { "other" }.into());
            }
        }
        let text = match () {
            _ if !text.is_empty() => text,
            _ if !files.is_empty() => "(files)".to_owned(),
            _ if !refs.is_empty() => "(references)".to_owned(),
            _ => text,
        };
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
            refs,
        };
        self.orchestration()
            .workspace()
            .write_orchestration(|doc| doc.upsert_dump(&dump));
        self.orchestration()
            .comms()
            .say(CommKind::Message, super::comms::YOU, &brain, &text);
        let message = zeron_proto::orchestration::prompt_text::dump_message_with_refs(&dump.id, &text, &dump.refs);
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
        reference: Option<String>,
    ) -> Result<String, RpcError> {
        let orch = self.orchestration().clone();
        let dump = orch
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .ok_or_else(|| failed(format!("No dump {dump_id}")))?;
        if target == RouteTarget::Other {
            return Err(RpcError::BadParams("Unknown route target".into()));
        }
        // The element the fragment is about: one the person attached.
        let reference = reference.map(|r| r.trim().to_owned()).filter(|r| !r.is_empty());
        let referenced = match &reference {
            None => None,
            Some(id) => Some(dump.refs.iter().find(|r| r.id == *id).cloned().ok_or_else(|| {
                RpcError::BadParams(format!("{id} is not one of dump {dump_id}'s references"))
            })?),
        };
        let aim = referenced.as_ref().map_or(Aim::Routed, |r| self.aim_of(r));
        // A "question" that asks for work is a task; a rhetorical one, info
        // (the person's words, checked: zeron_proto::direct_question).
        let intent = zeron_proto::direct_question::settle_intent(intent, &excerpt_text);
        // A reply to an ask is what the person tells the asker.
        let intent = match aim {
            Aim::Answer { .. } if intent != RouteIntent::Question => RouteIntent::Info,
            _ => intent,
        };
        // A reply on a Review card is the person's verdict on it: approval
        // approves it, anything else asks for changes. Only in the person's
        // own words (the excerpt, verbatim in the dump), never the Brain
        // dump's rewording: these are the person's ops.
        // Guarded, as these are the person's ops: only while the dump is
        // being routed, on a card in Review both when the person dumped and
        // now, once per card per dump, and judged on the whole sentence the
        // excerpt sits in (a "looks good" cut from "doesn't look good" isn't
        // one). Otherwise the reply is a comment to the card's thread.
        let verdict: Option<(String, bool, String)> = match &aim {
            Aim::Direct { card: Some(id), .. } if dump.state == DumpState::Routing => {
                let was_review = dump.refs.iter().any(|r| {
                    r.kind == zeron_proto::orchestration::RefKind::Card
                        && &r.id == id
                        && r.context.get("state").map(String::as_str) == Some("review")
                });
                let judged = dump.routes.iter().any(|r| r.reference.as_deref() == Some(id.as_str()));
                let sentence = span_of(&dump.text, &excerpt_text).map(|span| sentence_around(&dump.text, span));
                match (was_review && !judged, sentence) {
                    (true, Some(sentence)) => orch
                        .item(id)
                        .filter(|c| c.state == WorkState::Review)
                        .map(|c| (c.id, reads_as_approval(&sentence), sentence)),
                    _ => None,
                }
            }
            _ => None,
        };
        let (target, message) = match &aim {
            Aim::Answer { asker, .. } => (RouteTarget::Chat { id: asker.clone() }, message),
            Aim::OnCard { item_id } => (RouteTarget::Item { id: item_id.clone() }, message),
            // To the very place the person pointed at.
            Aim::Direct { target, card } => {
                let message = match card.as_deref().and_then(|id| orch.item(id)) {
                    Some(card) => {
                        format!(
                            "[On your card \"{}\"] {}",
                            zeron_proto::orchestration::prompt_text::one_line(&card.title, zeron_proto::orchestration::prompt_text::LABEL_MAX),
                            if message.trim().is_empty() { excerpt_text.trim() } else { message.trim() }
                        )
                    }
                    None => message,
                };
                (target.clone(), message)
            }
            // Work for a crew thread goes through its orchestrator, so its
            // board sees it; info and questions may go to the thread itself.
            Aim::Routed => self.through_owner(target, intent, message),
        };
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
        // A device the person attached (one Zeron knows) is where a new one
        // starts, when the Brain dump named no place.
        let device_id = match referenced.as_ref() {
            Some(r) if r.kind == RefKind::Device && space_id.is_none() && device_id.is_none() => {
                let known = r.id == self.doc_host_ref().device_id()
                    || self.workspace_host().read_devices().unwrap_or_default().iter().any(|d| d.id == r.id);
                known.then(|| r.id.clone())
            }
            _ => device_id,
        };
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
            review: None,
            answered_at: None,
            item_id: None,
            space_id,
            device_id,
            attachments: super::placement::route_files(&dump, intent, attachments)?,
            delivered_at: Default::default(),
            reference,
        };
        let brain = self.brain_of(&dump);
        let progress = progress && !matches!(aim, Aim::Answer { .. });
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
            None => match &aim {
                Aim::Answer { ask_id, .. } => {
                    if dump.state != DumpState::Routing {
                        return Err(RpcError::BadParams("This dump is already routed".into()));
                    }
                    // LATER(decision-ui): use the decision op with {option?, text?}: pass
                    // the option `answer_text` matched as AnswerAsk's `option`, else the text.
                    let answer = self.answer_text(ask_id, &route).ok_or_else(|| {
                        RpcError::BadParams(
                            "A reply to an @ask is the person's words: quote it verbatim from the dump as `excerpt`".into(),
                        )
                    })?;
                    self.answer_ask(&Who::Person, ask_id.clone(), answer).await?;
                    route.state = RouteState::Delivered;
                }
                Aim::OnCard { item_id } => {
                    self.note_on_card(item_id, &route)?;
                    route.state = RouteState::Delivered;
                }
                // (Review card: approveItem / requestChanges, as the person's
                // buttons on it do; requestChanges tells its thread itself.)
                _ if verdict.is_some() => {
                    let (card_id, approve, sentence) = verdict.clone().expect("checked");
                    if approve {
                        self.approve_item(&card_id, Utc::now())?;
                    } else {
                        self.request_changes(&card_id, &sentence)?;
                    }
                    route.state = RouteState::Delivered;
                }
                _ => self.deliver_route(&dump, &mut route, brain.as_deref(), card).await?,
            },
        }
        // Not sure between places: the person picks, from Needs you.
        if route.target == RouteTarget::Unsure {
            self.ask_where(&dump, &mut route, brain.as_deref());
        }
        // A task it hands a thread lands on the board.
        if route.state == RouteState::Delivered {
            route.delivered_at = Some(Utc::now());
        }
        // (A reply on an ask or a card is about a card that is already there.)
        let on_a_card = matches!(aim, Aim::Answer { .. } | Aim::OnCard { .. } | Aim::Direct { card: Some(_), .. });
        if !on_a_card {
            self.card_for_route(&dump, &mut route);
        }
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

    /// Where a fragment about `r` goes, from the rows (an id the person
    /// attached, never trusted beyond what the rows say): an open ask is
    /// answered; a card's comment goes to its first thread, else its
    /// orchestrator, else stays on the card; a chat gets it directly; a Q&A
    /// entry's follow-up goes to the chat that answered it. The rest is
    /// context for routing.
    pub(crate) fn aim_of(&self, r: &DumpRef) -> Aim {
        let orch = self.orchestration();
        let chat_exists = |id: &str| self.workspace_host().chat(id).ok().flatten().is_some();
        let direct = |id: &str| Aim::Direct { target: RouteTarget::Chat { id: id.to_owned() }, card: None };
        match r.kind {
            RefKind::Ask => match orch.workspace().read_orchestration_with(|doc| doc.ask(&r.id)) {
                Some(ask) if ask.state == AskState::Open => Aim::Answer { ask_id: ask.id, asker: ask.from_chat },
                // Answered already: a follow-up for the asker.
                Some(ask) => direct(&ask.from_chat),
                None => r.context("chat").filter(|c| chat_exists(c)).map_or(Aim::Routed, direct),
            },
            RefKind::Card => match orch.item(&r.id) {
                Some(card) => {
                    let target = match (card.threads.first(), card.orchestrator_id.as_deref()) {
                        (Some(thread), _) => RouteTarget::Chat { id: thread.clone() },
                        (None, Some(owner)) if orch.orchestrator(owner).is_some() => {
                            RouteTarget::Orchestrator { id: owner.to_owned() }
                        }
                        _ => return Aim::OnCard { item_id: card.id },
                    };
                    Aim::Direct { target, card: Some(card.id) }
                }
                None => Aim::Routed,
            },
            RefKind::Chat if chat_exists(&r.id) => direct(&r.id),
            RefKind::Qa => {
                let answered_by = r.context("dump").and_then(|dump| {
                    orch.workspace()
                        .read_orchestration_with(|doc| doc.dump(dump))
                        .and_then(|d| d.routes.into_iter().find(|x| x.id == r.id))
                        .and_then(|x| target_chat(&x.target).map(str::to_owned))
                });
                answered_by
                    .or_else(|| r.context("chat").map(str::to_owned))
                    .filter(|c| chat_exists(c))
                    .map_or(Aim::Routed, |c| direct(&c))
            }
            _ => Aim::Routed,
        }
    }

    /// What a route answers ask `ask_id` with: one of its options when the
    /// reply is just that option, else the reply.
    ///
    /// Always the person's own words: the fragment quoted verbatim from the
    /// dump (never the Brain dump's rewording, which would answer as the
    /// person in words they didn't write). `None` when the excerpt isn't
    /// found in the dump's text.
    fn answer_text(&self, ask_id: &str, route: &Route) -> Option<String> {
        route.span?;
        let reply = route.excerpt.trim();
        if reply.is_empty() {
            return None;
        }
        let bare = reply.trim_end_matches(['.', '!']).trim().to_lowercase();
        let ask = self.orchestration().workspace().read_orchestration_with(|doc| doc.ask(ask_id));
        ask.and_then(|a| a.options.into_iter().find(|o| o.label.trim().to_lowercase() == bare))
            .map_or_else(|| reply.to_owned(), |o| o.label)
            .into()
    }

    /// A comment kept on card `item_id` (no thread or owner to tell yet):
    /// added to its detail.
    fn note_on_card(&self, item_id: &str, route: &Route) -> Result<(), RpcError> {
        let orch = self.orchestration();
        let card = orch.item(item_id).ok_or_else(|| failed(format!("No card {item_id}")))?;
        let comment = if route.message.trim().is_empty() { route.excerpt.trim() } else { route.message.trim() };
        let detail = if card.detail.trim().is_empty() {
            comment.to_owned()
        } else {
            format!("{}\n\n{comment}", card.detail.trim_end())
        };
        orch.patch_item(WorkItemPatch { id: Some(card.id), detail: Some(detail), ..Default::default() })
            .map(|_| ())
            .map_err(RpcError::Failed)
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
        if route.state == RouteState::Delivered {
            route.delivered_at = Some(Utc::now());
        }
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
