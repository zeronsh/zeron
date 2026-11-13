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

/// A dump whose every fragment has a place is routed; one with an `unsure`
/// fragment still waiting needs the person.
fn settled_state(dump: &Dump) -> DumpState {
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
    pub(crate) async fn dump(&self, text: &str) -> Result<String, RpcError> {
        let text = cut(text.trim(), MAX_DUMP_BYTES);
        if text.is_empty() {
            return Err(RpcError::BadParams("Nothing to dump".into()));
        }
        let brain = self.ensure_brain_dump()?;
        // A Brain dump that has never run starts on a fast model.
        self.brain_dump_fast_model(&brain).await;
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
        };
        self.orchestration()
            .workspace()
            .write_orchestration(|doc| doc.upsert_dump(&dump));
        let message = zeron_proto::orchestration::prompt_text::dump_message(&dump.id, &text);
        if let Err(error) = deliver(self, &brain, &message, None, When::Next) {
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

    /// Record one fragment's route and deliver it. The route's id.
    pub(crate) async fn route(
        &self,
        dump_id: &str,
        excerpt_text: String,
        target: RouteTarget,
        intent: RouteIntent,
        message: String,
        candidates: Vec<RouteTarget>,
    ) -> Result<String, RpcError> {
        let orch = self.orchestration().clone();
        let dump = orch
            .workspace()
            .read_orchestration_with(|doc| doc.dump(dump_id))
            .ok_or_else(|| failed(format!("No dump {dump_id}")))?;
        if target == RouteTarget::Other {
            return Err(RpcError::BadParams("Unknown route target".into()));
        }
        let mut route = Route {
            id: uuid::Uuid::new_v4().simple().to_string(),
            span: span_of(&dump.text, &excerpt_text),
            excerpt: excerpt_text,
            target,
            intent,
            message,
            state: RouteState::Pending,
            candidates,
            answer: None,
            item_id: None,
        };
        let brain = self.brain_of(&dump);
        self.deliver_route(&dump, &mut route, brain.as_deref())
            .await?;
        let to = target_chat(&route.target).unwrap_or("").to_owned();
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
        self.deliver_route(&dump, &mut route, brain.as_deref())
            .await?;
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
        Ok(route.id)
    }

    /// The Brain dump a dump was routed by: the one hosted where it was
    /// dumped, else any.
    fn brain_of(&self, dump: &Dump) -> Option<String> {
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
                deliver(self, &id, &message, brain, When::Now)?;
                route.state = RouteState::Delivered;
            }
            RouteTarget::Chat { id } => {
                deliver(self, &id, &message, brain, When::Now)?;
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
                        space_id: None,
                        device_id: None,
                        harness: None,
                        model: None,
                        crew: CrewPolicy::default(),
                    })
                    .await?
                };
                deliver(self, &id, &message, brain, When::Now)?;
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
                    self.create_chat_for(&id, None, None, None, None, None)?;
                    if !title.is_empty() {
                        self.workspace_host()
                            .rename_chat(&id, &title)
                            .map_err(|e| failed(e.to_string()))?;
                    }
                    id
                };
                deliver(self, &id, &message, brain, When::Now)?;
                route.target = RouteTarget::NewThread { id };
                route.state = RouteState::Delivered;
            }
            RouteTarget::Item { id } => {
                let existing = (!id.is_empty()).then(|| orch.item(&id)).flatten();
                let item_id = match existing {
                    Some(item) => item.id,
                    None => orch
                        .patch_item(WorkItemPatch {
                            title: Some(if title.is_empty() {
                                "Untitled".into()
                            } else {
                                title.clone()
                            }),
                            detail: Some(message.clone()),
                            state: Some(WorkState::Inbox),
                            source: Some(RouteRef {
                                dump_id: dump.id.clone(),
                                route_id: route.id.clone(),
                            }),
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
