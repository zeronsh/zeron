//! OpenCode's sessions as Zeron chats, kept current as they change in any
//! client. OpenCode 2.x keeps every session on one background service that
//! its TUI, its desktop app and Zeron's runs share ([`zeron_harness`]'s
//! OpenCode driver attaches to it): this host reads them from there and
//! keeps each chat's transcript the session's, so a session can move from
//! Zeron to the TUI and back mid-way with nothing lost.
//!
//! - A session a Zeron chat runs on (its metadata, or `harness_session_id`)
//!   is always followed. While a turn of Zeron's own streams into it, that
//!   run writes the transcript; once it ends, what the run wrote is replaced
//!   by the session's messages — with anything sent from another client
//!   meanwhile. A turn running in another client streams in live.
//! - With "Sync sessions" on (Settings → Providers → OpenCode settings), any
//!   other session becomes a chat of its own, under the project of its
//!   folder (made for it with "Add projects" on), its id derived from the
//!   session's so it is made once. A chat deleted in Zeron is not brought
//!   back. A fork of a session Zeron shows is always brought in.
//! - Messages keep OpenCode's ids (`oc:<id>`), so one read again as it grows
//!   replaces its entry. Above where a chat moved to OpenCode from another
//!   agent, its transcript is its own and stays.
//!
//! Runs on the host device only, and only while OpenCode is enabled here and
//! has a background service; a Cloud machine has neither. `ZERON_OPENCODE_SYNC=0`
//! switches it off.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_harness::{
    CompactionStatus, NativeChange, NativeMessage, NativeMessageKind, NativeQuestion, NativeSession, OpencodeHarness,
};
use zeron_proto::{Chat, ChatConfig, HarnessId};

use crate::registry::OpencodeSettings;

use crate::doc_host::DocHost;
use crate::registry::HarnessRegistry;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

/// Entry ids of messages read from an OpenCode session.
const ENTRY_PREFIX: &str = "oc:";

/// The text part carrying a compaction's summary, under its note.
const SUMMARY_PART_ID: &str = "s0";

/// The entry of an undo staged in the session (one at most).
const REVERT_ENTRY: &str = "oc:revert";

/// Namespace of the chat ids derived from session ids.
const CHAT_NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0x7a1c_42e5_0b4f_4d8e_9c1a_5e0f_2d3b_6c71);

/// How often changed (and running) sessions are read again: a turn running
/// in another client shows at this pace.
const TICK: Duration = Duration::from_millis(750);

/// How long to wait before trying again when OpenCode is off or has no
/// service.
const IDLE_RETRY: Duration = Duration::from_secs(30);

/// Set to `0` to leave OpenCode's sessions out of Zeron.
const SYNC_ENV: &str = "ZERON_OPENCODE_SYNC";

/// Start following OpenCode's sessions, for the engine's life: the task, so
/// the engine can end it (it holds the doc host).
pub(crate) fn spawn(
    workspace: WorkspaceHost,
    doc_host: DocHost,
    sessions: SessionsEngine,
    registry: Arc<HarnessRegistry>,
) -> Option<tokio::task::JoinHandle<()>> {
    if std::env::var(SYNC_ENV).is_ok_and(|v| matches!(v.trim(), "0" | "false" | "no"))
        || crate::runner::is_cloud_platform()
    {
        return None;
    }
    let mirror = Mirror {
        harness: OpencodeHarness::new(),
        workspace,
        doc_host,
        sessions,
        registry,
        read: HashMap::new(),
        working: HashMap::new(),
        windows: HashMap::new(),
        summaries: HashMap::new(),
    };
    Some(tokio::spawn(mirror.run()))
}

struct Mirror {
    harness: OpencodeHarness,
    workspace: WorkspaceHost,
    doc_host: DocHost,
    sessions: SessionsEngine,
    registry: Arc<HarnessRegistry>,
    /// How each session was when last read (its `updated` time and staged
    /// undo): one unchanged since is skipped.
    read: HashMap<String, (i64, Option<String>)>,
    /// Chats shown working for a turn in another client, and when that was
    /// last stamped.
    working: HashMap<String, tokio::time::Instant>,
    /// Each model's context window, as read once.
    windows: HashMap<String, Option<u64>>,
    /// The words of the summary a session's compaction is writing, as they
    /// came on the feed: the session's own copy has none until it is done.
    summaries: HashMap<String, String>,
}

/// How often a turn running in another client re-stamps its chat's status:
/// well inside the window after which a status reads as stale.
const WORKING_REFRESH: Duration = Duration::from_secs(10);

impl Mirror {
    async fn run(mut self) {
        loop {
            if !self.registry.enabled_set().contains(&HarnessId::Opencode) {
                tokio::time::sleep(IDLE_RETRY).await;
                continue;
            }
            // (The server followed: this computer's service, or the remote
            // one Settings name. Another named, the feed follows it instead.)
            let server = self.registry.opencode_settings().remote_server();
            let mut feed = match self.harness.follow_native().await {
                Ok(feed) => feed,
                Err(error) => {
                    tracing::debug!(%error, "opencode sync: no service to follow");
                    tokio::time::sleep(IDLE_RETRY).await;
                    continue;
                }
            };
            let mut changed: HashSet<String> = HashSet::new();
            let mut running: HashSet<String> = HashSet::new();
            let mut tick = tokio::time::interval(TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    change = feed.recv() => match change {
                        None => break,
                        Some(NativeChange::Resync) => self.read_all(&mut running).await,
                        Some(NativeChange::Session(id)) => {
                            changed.insert(id);
                        }
                        Some(NativeChange::Deleted(id)) => {
                            self.summaries.remove(&id);
                            self.deleted(&id);
                        }
                        Some(NativeChange::CompactionText { session, text }) => {
                            self.summaries.entry(session.clone()).or_default().push_str(&text);
                            // Its note takes the words as they come; the
                            // first one finds no note yet, so the session is
                            // read now (the tick would be up to a second
                            // away, and the whole summary with it).
                            if !self.stream_summary(&session, &text) {
                                self.read_one(&session, &mut running).await;
                            }
                            changed.insert(session);
                        }
                        // (The harness's catalogs follow these themselves.)
                        Some(NativeChange::Catalog) => {}
                    },
                    _ = tick.tick() => {
                        let due: HashSet<String> = changed.drain().chain(running.iter().cloned()).collect();
                        for id in due {
                            self.read_one(&id, &mut running).await;
                        }
                    }
                }
                if !self.registry.enabled_set().contains(&HarnessId::Opencode) {
                    break;
                }
                if self.registry.opencode_settings().remote_server() != server {
                    self.read.clear();
                    self.summaries.clear();
                    break;
                }
            }
        }
    }

    /// Every session, after the feed (re)connected: what changed while it
    /// was down is only found this way.
    async fn read_all(&mut self, running: &mut HashSet<String>) {
        let listed = match self.harness.native_sessions().await {
            Ok(listed) => listed,
            Err(error) => {
                tracing::debug!(%error, "opencode sync: sessions unreadable");
                return;
            }
        };
        for session in listed {
            self.mirror(&session, running).await;
        }
    }

    async fn read_one(&mut self, id: &str, running: &mut HashSet<String>) {
        match self.harness.native_session(id).await {
            Ok(Some(session)) => self.mirror(&session, running).await,
            Ok(None) => {
                running.remove(id);
            }
            Err(error) => {
                tracing::debug!(%error, session = %id, "opencode sync: session unreadable")
            }
        }
    }

    /// Bring the session's chat up to date with it.
    async fn mirror(&mut self, session: &NativeSession, running: &mut HashSet<String>) {
        if session.busy {
            running.insert(session.id.clone());
        } else {
            running.remove(&session.id);
        }
        let known = self.chat_for(session);
        let settings = self.registry.opencode_settings();
        let own = known
            .as_ref()
            .is_some_and(|chat| chat.id != imported_chat_id(&session.id));
        let followed = if own {
            // (A run on a private server isn't on the service's feed.)
            settings.background_service
        } else {
            settings.sync_sessions || self.fork_source(session).is_some()
        };
        if !followed {
            return;
        }
        // A turn of Zeron's own is streaming into it: that run writes the
        // transcript until it ends. Read again after, whatever changes then.
        if let Some(chat) = &known
            && self.sessions.turn_in_flight(&chat.id)
            && !acting(&chat.id)
        {
            running.insert(session.id.clone());
            self.read.remove(&session.id);
            return;
        }
        let unchanged = self.read.get(&session.id)
            == Some(&(session.updated_at_ms, session.reverted_from.clone()));
        let summarizing = self.summaries.contains_key(&session.id);
        if unchanged && known.is_some() && !session.busy && !summarizing {
            return;
        }
        let mut history = match self.harness.native_history(session).await {
            Ok(history) => history,
            Err(error) => {
                tracing::debug!(%error, session = %session.id, "opencode sync: history unreadable");
                return;
            }
        };
        let questions = match self.harness.native_questions(session).await {
            Ok(questions) => questions,
            Err(error) => {
                tracing::debug!(%error, session = %session.id, "opencode sync: questions unreadable");
                Vec::new()
            }
        };
        self.fill_summary(&session.id, &mut history);
        let window = self.window(session).await;
        let result = match known {
            Some(chat) => self.update(&chat, session, &history, &questions, window),
            None => self.import(session, &history, &questions, &settings, window),
        };
        match result {
            Ok(()) => {
                self.read.insert(
                    session.id.clone(),
                    (session.updated_at_ms, session.reverted_from.clone()),
                );
                self.show_working(session);
            }
            Err(error) => {
                tracing::warn!(%error, session = %session.id, "opencode sync: couldn't mirror a session");
            }
        }
    }

    /// The running compaction's summary so far, from the feed's words; once
    /// none runs, the session's own summary is the whole of it.
    fn fill_summary(&mut self, session: &str, history: &mut [NativeMessage]) {
        let running = history.iter_mut().rev().find_map(|message| match &mut message.kind {
            NativeMessageKind::Compaction {
                status: CompactionStatus::Running,
                summary,
                ..
            } => Some(summary),
            _ => None,
        });
        match running {
            Some(summary) => {
                if summary.is_empty()
                    && let Some(so_far) = self.summaries.get(session)
                {
                    summary.clone_from(so_far);
                }
            }
            None => {
                self.summaries.remove(session);
            }
        }
    }

    /// The chat showing the session `session` was forked from, if any.
    fn fork_source(&self, session: &NativeSession) -> Option<Chat> {
        let source = session.forked_from.as_deref()?;
        let chats = self.workspace.watch_chats();
        let chats = chats.borrow();
        let derived = imported_chat_id(source);
        chats
            .iter()
            .find(|chat| {
                chat.id == derived || chat.harness_session_id.as_deref() == Some(source)
            })
            .cloned()
    }

    /// The words of a running compaction's summary, into its note's text part
    /// as they come: the note streams them in, as a reply's words stream in.
    /// `false` when there is no note of a running compaction to take them
    /// yet (they are kept in `summaries`, and written with the note then).
    fn stream_summary(&mut self, session_id: &str, words: &str) -> bool {
        let Some(chat) = self.chat_for_session_id(session_id) else {
            return false;
        };
        let Ok(doc) = self.doc_host.open(&chat) else {
            return false;
        };
        match doc.doc().append_running_compaction_summary(words) {
            Ok(appended) => appended,
            Err(error) => {
                tracing::debug!(%error, session = %session_id, "opencode sync: summary not appended");
                false
            }
        }
    }

    /// The chat that shows the session `session_id`: one a run of Zeron's
    /// made, else the one an earlier import made.
    fn chat_for_session_id(&self, session_id: &str) -> Option<String> {
        let chats = self.workspace.watch_chats();
        let chats = chats.borrow();
        let derived = imported_chat_id(session_id);
        chats
            .iter()
            .find(|chat| chat.harness_session_id.as_deref() == Some(session_id))
            .or_else(|| chats.iter().find(|chat| chat.id == derived))
            .map(|chat| chat.id.clone())
    }

    /// The context window of the session's model, read once per model.
    async fn window(&mut self, session: &NativeSession) -> Option<u64> {
        let model = session.model.clone()?;
        if let Some(window) = self.windows.get(&model) {
            return *window;
        }
        let window = self
            .harness
            .native_context_window(&model)
            .await
            .ok()
            .flatten();
        self.windows.insert(model, window);
        window
    }

    /// Show the session's chat working while a turn runs in it in another
    /// client, and settle it when that turn ends.
    fn show_working(&mut self, session: &NativeSession) {
        let Some(chat) = self.chat_for(session) else {
            return;
        };
        if session.busy {
            let due = self
                .working
                .get(&chat.id)
                .is_none_or(|stamped| stamped.elapsed() >= WORKING_REFRESH);
            if due {
                self.sessions.set_external_turn(&chat.id, true);
                self.working.insert(chat.id, tokio::time::Instant::now());
            }
        } else if self.working.remove(&chat.id).is_some() {
            self.sessions.set_external_turn(&chat.id, false);
        }
    }

    /// The chat that already shows `session`: one a run of Zeron's made it
    /// for, else the one an earlier import made.
    fn chat_for(&self, session: &NativeSession) -> Option<Chat> {
        let chats = self.workspace.watch_chats();
        let chats = chats.borrow();
        let derived = imported_chat_id(&session.id);
        chats
            .iter()
            .find(|chat| Some(&chat.id) == session.zeron_chat.as_ref())
            .or_else(|| {
                chats.iter().find(|chat| {
                    chat.harness_session_id.as_deref() == Some(session.id.as_str())
                        && chat.harness_session_harness.as_deref().map_or_else(
                            || {
                                chat.config
                                    .as_ref()
                                    .is_some_and(|c| c.harness == HarnessId::Opencode)
                            },
                            |harness| harness == HarnessId::Opencode.as_str(),
                        )
                })
            })
            .or_else(|| chats.iter().find(|chat| chat.id == derived))
            .cloned()
    }

    /// A session no chat shows yet: a chat of its own, under the project of
    /// its folder (or its fork source's). Its history is saved before its
    /// row is published, so a crash never leaves an empty chat in the
    /// sidebar.
    fn import(
        &self,
        session: &NativeSession,
        history: &[NativeMessage],
        questions: &[NativeQuestion],
        settings: &OpencodeSettings,
        window: Option<u64>,
    ) -> Result<(), String> {
        // A session Zeron made for a chat that is gone went with that chat.
        if session.zeron_chat.is_some() {
            return Ok(());
        }
        let chat_id = imported_chat_id(&session.id);
        if self.workspace.chat_deleted(&chat_id) {
            return Ok(());
        }
        let existing = self
            .workspace
            .existing_space_for_path(&session.directory)
            .map_err(|e| e.to_string())?;
        let space_id = match existing {
            Some(space) => space,
            None => match self.fork_source(session).and_then(|chat| chat.space_id) {
                Some(space) => space,
                None if settings.add_projects => self
                    .workspace
                    .space_for_path(&session.directory)
                    .map_err(|e| e.to_string())?,
                // (Not a project of Zeron's, and none is to be made.)
                None => return Ok(()),
            },
        };
        let doc = self.doc_host.open(&chat_id).map_err(|e| e.to_string())?;
        let written = write_history(
            doc.doc(),
            history,
            session,
            Start::ALL,
            self.doc_host.device_id(),
        )
        .map_err(|e| e.to_string())?;
        write_questions(doc.doc(), questions, self.doc_host.device_id())
            .map_err(|e| e.to_string())?;
        if let Some(tokens) = context_tokens(history, session, Start::ALL) {
            doc.doc()
                .update_context_usage(Some(tokens), window)
                .map_err(|e| e.to_string())?;
        }
        self.doc_host
            .persist_fork(&doc)
            .map_err(|e| e.to_string())?;
        let updated = millis(session.updated_at_ms);
        let chat = Chat {
            id: chat_id,
            device_id: self.doc_host.device_id().to_owned(),
            title: session.title.clone(),
            archived: session.archived,
            cwd: Some(session.directory.clone()),
            branch: None,
            checkout_id: None,
            source_context: None,
            config: Some(ChatConfig {
                harness: HarnessId::Opencode,
                model: session.model.clone(),
                reasoning: None,
                model_options: Default::default(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                access: Default::default(),
                plan: false,
            }),
            last_message_preview: written.preview,
            last_message_at: Some(updated),
            created_at: millis(session.created_at_ms),
            // (So the chat's next message continues the same session.)
            harness_session_id: Some(session.id.clone()),
            harness_session_cwd: Some(session.directory.clone()),
            harness_session_harness: Some(HarnessId::Opencode.as_str().to_owned()),
            space_id: Some(space_id),
            // (Seen as of now: a history read in is not news.)
            last_seen_at: Some(updated),
            room_gen: Some(2),
            parent_chat_id: None,
            child_kind: None,
            sleep: None,
        };
        self.workspace
            .import_chat_row(&chat)
            .map_err(|e| e.to_string())
    }

    /// A chat that shows `session` already: its transcript, from where it
    /// moved to OpenCode, made the session's; for an imported chat its
    /// title and archived state too.
    fn update(
        &self,
        chat: &Chat,
        session: &NativeSession,
        history: &[NativeMessage],
        questions: &[NativeQuestion],
        window: Option<u64>,
    ) -> Result<(), String> {
        let imported = chat.id == imported_chat_id(&session.id);
        let doc = self.doc_host.open(&chat.id).map_err(|e| e.to_string())?;
        let start = if imported {
            Some(Start::ALL)
        } else {
            Start::of(&doc.doc().read_entries().map_err(|e| e.to_string())?)
        };
        // (Moved on to another agent since: the session is no longer its.)
        let Some(start) = start else {
            return Ok(());
        };
        let written = write_history(
            doc.doc(),
            history,
            session,
            start,
            self.doc_host.device_id(),
        )
        .map_err(|e| e.to_string())?;
        if let Some(tokens) = context_tokens(history, session, start) {
            doc.doc()
                .update_context_usage(Some(tokens), window)
                .map_err(|e| e.to_string())?;
        }
        let asked = write_questions(doc.doc(), questions, self.doc_host.device_id())
            .map_err(|e| e.to_string())?;
        if asked {
            self.workspace
                .set_chat_activity(&chat.id, Some(Utc::now().timestamp_millis()), None)
                .map_err(|e| e.to_string())?;
        }
        if written.changed {
            if let Some(preview) = &written.preview {
                self.workspace.note_message(&chat.id, preview);
            }
            self.workspace
                .set_chat_activity(&chat.id, Some(session.updated_at_ms), None)
                .map_err(|e| e.to_string())?;
        }
        if imported {
            if let Some(title) = &session.title
                && chat.title.as_ref() != Some(title)
            {
                self.workspace
                    .rename_chat(&chat.id, title)
                    .map_err(|e| e.to_string())?;
            }
            if chat.archived != session.archived {
                self.workspace
                    .set_chat_archived(&chat.id, session.archived)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// A session deleted in OpenCode: its imported chat is archived rather
    /// than deleted — the person may still want what it said.
    fn deleted(&mut self, session_id: &str) {
        self.read.remove(session_id);
        let chat_id = imported_chat_id(session_id);
        if self.workspace.chat(&chat_id).ok().flatten().is_some()
            && let Err(error) = self.workspace.set_chat_archived(&chat_id, true)
        {
            tracing::warn!(%error, session = %session_id, "opencode sync: couldn't archive a deleted session's chat");
        }
    }
}

/// The chat id an imported session gets: the same on every read.
fn imported_chat_id(session_id: &str) -> String {
    uuid::Uuid::new_v5(&CHAT_NAMESPACE, session_id.as_bytes()).to_string()
}

fn millis(ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Where the session's part of a chat's transcript starts. Above it, the
/// chat's own: turns another agent ran before the chat moved to OpenCode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Start {
    /// The first entry that is the session's to write.
    index: usize,
    /// Messages from before this (a stint on OpenCode before the chat moved
    /// away and back) are not shown again.
    at: i64,
}

impl Start {
    /// All of it: a chat that has only ever been the session.
    const ALL: Start = Start {
        index: 0,
        at: i64::MIN,
    };

    /// For a chat Zeron runs: below its last move to OpenCode, or all of it
    /// when it never moved. `None` when it has moved on to another agent.
    fn of(entries: &[SessionMessageEntry]) -> Option<Start> {
        let seam = entries.iter().enumerate().rev().find_map(|(index, entry)| {
            entry.parts.iter().find_map(|part| match part {
                MessagePart::Switch { to_harness, .. } => {
                    Some((index, entry.created_at, to_harness.as_deref()))
                }
                _ => None,
            })
        });
        match seam {
            None => Some(Start::ALL),
            Some((index, at, None | Some("opencode"))) => Some(Start {
                index: index + 1,
                at,
            }),
            Some(_) => None,
        }
    }
}

/// What [`write_history`] did.
#[derive(Debug, Default)]
struct Written {
    /// An entry was added, rewritten or taken out.
    changed: bool,
    /// The last message's text, for the sidebar.
    preview: Option<String>,
}

/// An entry a run of Zeron's wrote for a turn on the session (a prompt, a
/// reply): the session has it too, once the turn is over. Errors stay: the
/// session may never have heard of what failed.
fn zeron_written(entry: &SessionMessageEntry) -> bool {
    !entry.id.starts_with(ENTRY_PREFIX)
        && !entry.id.starts_with(QUESTION_PREFIX)
        && matches!(entry.role, MessageRole::User | MessageRole::Assistant)
        && !entry
            .parts
            .iter()
            .any(|part| matches!(part, MessagePart::Error { .. }))
}

/// `current` grown into `next` by words appended to one of its text parts:
/// that part's id and the words — a summary being written, or a reply. `None`
/// for any other change (a part rewritten, taken out or added).
fn grown_text(current: &SessionMessageEntry, next: &SessionMessageEntry) -> Option<(String, String)> {
    if current.parts.len() != next.parts.len() {
        return None;
    }
    let mut grown = None;
    for (was, now) in current.parts.iter().zip(&next.parts) {
        if was == now {
            continue;
        }
        let (id, before, after) = match (was, now) {
            (
                MessagePart::Text {
                    id: was_id,
                    text: before,
                },
                MessagePart::Text {
                    id: now_id,
                    text: after,
                },
            )
            | (
                MessagePart::Reasoning {
                    id: was_id,
                    text: before,
                },
                MessagePart::Reasoning {
                    id: now_id,
                    text: after,
                },
            ) if was_id == now_id && after.starts_with(before.as_str()) => {
                (now_id, before, after)
            }
            _ => return None,
        };
        if grown.is_some() {
            return None;
        }
        grown = Some((id.clone(), after[before.len()..].to_owned()));
    }
    grown
}

/// Make `doc`'s transcript from `start` on the session's `history`: a
/// message not there yet goes in where it belongs, one that grew is
/// rewritten in place, one that's the same is left alone; one the session no
/// longer has (undone) goes, and so do the entries a run of Zeron's wrote for
/// turns the session now holds.
fn write_history(
    doc: &zeron_doc::SessionDoc,
    history: &[NativeMessage],
    session: &NativeSession,
    start: Start,
    device_id: &str,
) -> Result<Written, zeron_doc::DocError> {
    // An undo staged in the session takes its message and all after it back,
    // as OpenCode's own clients show it; a redo brings them in again.
    let cut = session
        .reverted_from
        .as_ref()
        .and_then(|id| history.iter().position(|message| &message.id == id))
        .unwrap_or(history.len());
    // Zeron's writes up to here are the session's to show now — or, undone,
    // not to show: the session's messages past an undo still hold them.
    let held_until = history
        .iter()
        .filter(|message| message.created_at_ms >= start.at)
        .map(|message| message.completed_at_ms.unwrap_or(0).max(message.created_at_ms))
        .max();
    let all = history;
    let history: Vec<&NativeMessage> = history[..cut]
        .iter()
        .filter(|message| message.created_at_ms >= start.at)
        .collect();
    // The undo itself stands where the turns it took back were, as OpenCode's
    // own clients show it: how many of the person's messages went. A turn
    // that has none of its own (a `!command` run took back) still reads as
    // the one message it reverted, never as none.
    let revert = (cut < all.len()).then(|| SessionMessageEntry {
        duration_ms: None,
        id: REVERT_ENTRY.to_owned(),
        role: MessageRole::System,
        parts: vec![MessagePart::Revert {
            id: "r0".into(),
            messages: all[cut..]
                .iter()
                .filter(|message| message.created_at_ms >= start.at)
                .filter(|message| matches!(message.kind, NativeMessageKind::User { .. }))
                .count()
                .max(1) as u64,
            files: session
                .reverted_files
                .iter()
                .map(|file| zeron_doc::RevertedFile {
                    path: file.path.clone(),
                    status: file.status.clone(),
                    additions: file.additions,
                    deletions: file.deletions,
                })
                .collect(),
        }],
        created_at: all[cut].created_at_ms,
        device_id: device_id.to_owned(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
    });
    let kept: HashSet<String> = history
        .iter()
        .map(|message| format!("{ENTRY_PREFIX}{}", message.id))
        .chain(revert.as_ref().map(|entry| entry.id.clone()))
        .collect();
    let mut written = Written::default();
    let mut ids: Vec<String> = Vec::new();
    let entries = doc.read_entries()?;
    let mut gone = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let replaced = index >= start.index
            && ((entry.id.starts_with(ENTRY_PREFIX) && !kept.contains(&entry.id))
                || (zeron_written(entry) && held_until.is_some_and(|t| entry.created_at <= t)));
        if replaced {
            gone.push(index);
        } else {
            ids.push(entry.id.clone());
        }
    }
    for index in gone.into_iter().rev() {
        doc.remove_message(index)?;
        written.changed = true;
    }
    let existing: HashMap<String, SessionMessageEntry> = doc
        .read_entries()?
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    let last = history.len().saturating_sub(1);
    for (index, message) in history.iter().enumerate() {
        // (The last message of a running session is still being written.)
        let streaming = session.busy && index == last;
        let entry = entry_of(message, streaming, device_id);
        if let Some(text) = preview_of(&entry) {
            written.preview = Some(text);
        }
        match ids.iter().position(|id| *id == entry.id) {
            Some(at) => {
                let current = &existing[&entry.id];
                if current.parts != entry.parts
                    || current.status != entry.status
                    || current.duration_ms != entry.duration_ms
                {
                    // A summary (or a reply) still being written: its words
                    // appended into the part, so a reader streams them in
                    // rather than re-making the row for each of them. Any
                    // other change rewrites the entry whole.
                    let grown = (current.status == entry.status
                        && current.duration_ms == entry.duration_ms)
                        .then(|| grown_text(current, &entry))
                        .flatten();
                    match grown {
                        Some((part_id, delta)) => {
                            doc.append_message_part_text(&entry.id, &part_id, &delta)?;
                        }
                        None => doc.replace_message(at, &entry)?,
                    }
                    written.changed = true;
                }
            }
            None => {
                // Before the next of the session's messages already shown,
                // else last.
                let at = history[index + 1..]
                    .iter()
                    .find_map(|later| {
                        let later = format!("{ENTRY_PREFIX}{}", later.id);
                        ids.iter().position(|id| *id == later)
                    })
                    .unwrap_or(ids.len());
                doc.insert_message(at, &entry)?;
                ids.insert(at, entry.id.clone());
                written.changed = true;
            }
        }
    }
    if let Some(entry) = revert {
        // After the last of the session's messages still shown.
        let after = history
            .last()
            .and_then(|last| {
                let last = format!("{ENTRY_PREFIX}{}", last.id);
                ids.iter().position(|id| *id == last)
            })
            .map_or(start.index.min(ids.len()), |at| at + 1);
        match ids.iter().position(|id| *id == entry.id) {
            Some(at) if at == after => {
                if existing.get(&entry.id).map(|current| &current.parts) != Some(&entry.parts) {
                    doc.replace_message(at, &entry)?;
                    written.changed = true;
                }
            }
            Some(at) => {
                doc.remove_message(at)?;
                ids.remove(at);
                let after = if at < after { after - 1 } else { after };
                doc.insert_message(after, &entry)?;
                ids.insert(after, entry.id.clone());
                written.changed = true;
            }
            None => {
                doc.insert_message(after, &entry)?;
                ids.insert(after, entry.id.clone());
                written.changed = true;
            }
        }
    }
    Ok(written)
}

/// What the session's context holds now: its newest reply's tokens, or a
/// compaction's summary when that came after.
fn context_tokens(history: &[NativeMessage], session: &NativeSession, start: Start) -> Option<u64> {
    let cut = session
        .reverted_from
        .as_ref()
        .and_then(|id| history.iter().position(|message| &message.id == id))
        .unwrap_or(history.len());
    history[..cut]
        .iter()
        .rev()
        .take_while(|message| message.created_at_ms >= start.at)
        .find_map(NativeMessage::context_tokens)
}

/// One of OpenCode's TUI actions, as sent from Zeron's composer for an
/// OpenCode chat — typed, or picked from its `/` list (a link the agent gets
/// as the command). The driver does them (`zeron_harness`'s OpenCode
/// actions); the session shows what they did, never the command as a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiAction {
    Compact,
    Undo,
    Redo,
    Fork,
    /// `!command`, the TUI's shell mode.
    Shell,
}

/// The action `prompt` is, if it is one.
pub(crate) fn tui_action(prompt: &str) -> Option<TuiAction> {
    let prompt = zeron_proto::invocation::harness_prompt(prompt, HarnessId::Opencode);
    let prompt = prompt.trim();
    match prompt {
        "/compact" => Some(TuiAction::Compact),
        "/undo" => Some(TuiAction::Undo),
        "/redo" => Some(TuiAction::Redo),
        "/fork" => Some(TuiAction::Fork),
        _ => prompt
            .strip_prefix('!')
            .is_some_and(|command| !command.trim().is_empty())
            .then_some(TuiAction::Shell),
    }
}

/// Chats whose turn in flight is one of those actions: it writes nothing of
/// its own, so the session's changes show as they happen (a compaction's
/// progress, an undo).
static ACTING: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

fn acting_set() -> std::sync::MutexGuard<'static, HashSet<String>> {
    ACTING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn acting(chat_id: &str) -> bool {
    acting_set().contains(chat_id)
}

/// A prompt sent to an OpenCode chat: whether it is one of the TUI's
/// actions (no message of the person's), noted for the sync either way.
pub(crate) fn note_prompt(chat_id: &str, prompt: &str) -> bool {
    let action = tui_action(prompt).is_some();
    if action {
        acting_set().insert(chat_id.to_owned());
    } else {
        acting_set().remove(chat_id);
    }
    action
}

/// Entry ids of the questions a session is waiting on.
const QUESTION_PREFIX: &str = "oc-form:";

/// The request ids of OpenCode's question forms (`frm_…`): an answer to one
/// goes back to OpenCode ([`answer`]).
pub(crate) const FORM_REQUEST_PREFIX: &str = "frm_";

/// Show the questions the session is waiting on, each as an open request of
/// its own, and settle those it no longer waits on (answered in another
/// client, or dismissed). Returns whether one was newly asked.
fn write_questions(
    doc: &zeron_doc::SessionDoc,
    questions: &[NativeQuestion],
    device_id: &str,
) -> Result<bool, zeron_doc::DocError> {
    let existing = doc.read_entries()?;
    let mut asked = false;
    for question in questions {
        let id = format!("{QUESTION_PREFIX}{}", question.form_id);
        if existing.iter().any(|entry| entry.id == id) {
            continue;
        }
        doc.push_message(&SessionMessageEntry {
            duration_ms: None,
            id: id.clone(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Input {
                id,
                request_id: question.form_id.clone(),
                questions: question.questions.clone(),
                resolved: false,
                answers: None,
            }],
            created_at: Utc::now().timestamp_millis(),
            device_id: device_id.to_owned(),
            // (Settled, so an answer with no run to take it still finds it.)
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        })?;
        asked = true;
    }
    for (index, entry) in existing.iter().enumerate() {
        let Some(form_id) = entry.id.strip_prefix(QUESTION_PREFIX) else {
            continue;
        };
        if questions.iter().any(|question| question.form_id == form_id) {
            continue;
        }
        let open = entry.parts.iter().any(|part| {
            matches!(
                part,
                MessagePart::Input {
                    resolved: false,
                    ..
                }
            )
        });
        if !open {
            continue;
        }
        // Settled elsewhere: closed, with what was answered unknown here.
        let mut settled = entry.clone();
        for part in &mut settled.parts {
            if let MessagePart::Input { resolved, .. } = part {
                *resolved = true;
            }
        }
        doc.replace_message(index, &settled)?;
    }
    Ok(asked)
}

/// Send the person's answer to a question an OpenCode session is waiting on
/// (no run of Zeron's holds it: it was asked in another client, or the run
/// ended). No answers closes it unanswered.
pub(crate) async fn answer(
    workspace: &WorkspaceHost,
    chat_id: &str,
    form_id: &str,
    questions: &[zeron_proto::UserInputQuestion],
    answers: &[zeron_proto::UserInputAnswer],
) -> Result<(), String> {
    let chat = workspace
        .chat(chat_id)
        .map_err(|e| e.to_string())?
        .ok_or("the chat is gone")?;
    let session = chat
        .harness_session_id
        .filter(|session| !session.is_empty())
        .ok_or("the chat has no OpenCode session")?;
    let directory = chat
        .harness_session_cwd
        .or(chat.cwd)
        .ok_or("the chat has no folder")?;
    OpencodeHarness::new()
        .answer_native_question(&session, &directory, form_id, questions, answers)
        .await
        .map_err(|e| e.to_string())
}

/// A session message as a transcript entry, built the way a run's would be.
fn entry_of(message: &NativeMessage, streaming: bool, device_id: &str) -> SessionMessageEntry {
    let (role, parts) = match &message.kind {
        NativeMessageKind::User { text } => (
            MessageRole::User,
            vec![MessagePart::Text {
                id: "t0".into(),
                text: text.clone(),
            }],
        ),
        NativeMessageKind::Assistant { events, .. } => {
            let mut parts = Vec::new();
            for event in events {
                zeron_doc::fold_event_into_parts(&mut parts, event);
            }
            (MessageRole::Assistant, parts)
        }
        // Its summary under it, as OpenCode's clients show it: written as it
        // runs, then the session's own. (Even empty while it runs, so the
        // words appended into it as they come land in it.)
        NativeMessageKind::Compaction {
            status,
            tokens_in,
            tokens_out,
            summary,
        } => (
            MessageRole::System,
            std::iter::once(MessagePart::Compaction {
                id: "c0".into(),
                status: status.as_str().to_owned(),
                tokens_in: *tokens_in,
                tokens_out: *tokens_out,
            })
            .chain(
                (!summary.trim().is_empty() || *status == CompactionStatus::Running).then(|| {
                    MessagePart::Text {
                        id: SUMMARY_PART_ID.into(),
                        text: summary.clone(),
                    }
                }),
            )
            .collect(),
        ),
    };
    let duration_ms = matches!(message.kind, NativeMessageKind::Assistant { .. })
        .then_some(message.completed_at_ms)
        .flatten()
        .map(|done| done - message.created_at_ms)
        .filter(|ms| *ms > 0);
    SessionMessageEntry {
        duration_ms,
        id: format!("{ENTRY_PREFIX}{}", message.id),
        role,
        parts,
        created_at: message.created_at_ms,
        device_id: device_id.to_owned(),
        status: Some(if streaming {
            MessageStatus::Streaming
        } else {
            MessageStatus::Complete
        }),
        continuation_of: None,
    }
}

/// The entry's last text, if it has any.
fn preview_of(entry: &SessionMessageEntry) -> Option<String> {
    entry.parts.iter().rev().find_map(|part| match part {
        MessagePart::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::AgentEvent;

    fn session(busy: bool) -> NativeSession {
        NativeSession {
            id: "ses_a".into(),
            title: Some("Fix it".into()),
            directory: "/work".into(),
            created_at_ms: 1,
            updated_at_ms: 2,
            archived: false,
            busy,
            model: None,
            zeron_chat: None,
            forked_from: None,
            reverted_from: None,
            reverted_files: Vec::new(),
        }
    }

    fn user(id: &str, at: i64, text: &str) -> NativeMessage {
        NativeMessage {
            id: id.into(),
            created_at_ms: at,
            completed_at_ms: None,
            kind: NativeMessageKind::User { text: text.into() },
        }
    }

    fn reply(id: &str, at: i64, text: &str) -> NativeMessage {
        NativeMessage {
            id: id.into(),
            created_at_ms: at,
            completed_at_ms: Some(at + 1),
            kind: NativeMessageKind::Assistant {
                events: vec![AgentEvent::TextDelta { text: text.into() }],
                context: Some(at as u64 * 10),
            },
        }
    }

    #[test]
    fn a_session_reads_in_once_and_grows_in_place() {
        let doc = zeron_doc::SessionDoc::init("sync").unwrap();
        let history = vec![user("m1", 10, "hi"), reply("m2", 11, "hel")];
        let written = write_history(&doc, &history, &session(true), Start::ALL, "dev").unwrap();
        assert!(written.changed);
        assert_eq!(written.preview.as_deref(), Some("hel"));
        let entries = doc.read_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "oc:m1");
        assert_eq!(entries[1].status, Some(MessageStatus::Streaming));

        // The reply grows and the turn ends: the same entry, rewritten.
        let history = vec![user("m1", 10, "hi"), reply("m2", 11, "hello")];
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        let entries = doc.read_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].status, Some(MessageStatus::Complete));
        assert_eq!(preview_of(&entries[1]).as_deref(), Some("hello"));

        // Read again unchanged: nothing written.
        let again = write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        assert!(!again.changed);
    }

    #[test]
    fn an_undo_takes_its_messages_back_and_a_redo_brings_them_again() {
        let doc = zeron_doc::SessionDoc::init("undo").unwrap();
        let history = vec![
            user("m1", 10, "one"),
            reply("m2", 11, "1"),
            user("m3", 12, "two"),
            reply("m4", 13, "2"),
        ];
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        let ids = |doc: &zeron_doc::SessionDoc| -> Vec<String> {
            doc.read_entries()
                .unwrap()
                .into_iter()
                .map(|e| e.id)
                .collect()
        };
        // /undo in the TUI: staged at the second prompt.
        let mut undone = session(false);
        undone.reverted_from = Some("m3".into());
        assert!(
            write_history(&doc, &history, &undone, Start::ALL, "dev")
                .unwrap()
                .changed
        );
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", REVERT_ENTRY]);
        // /redo: the same messages, back in their place.
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", "oc:m3", "oc:m4"]);
        // The undo made final by a new prompt: the session no longer has them.
        let after = vec![
            user("m1", 10, "one"),
            reply("m2", 11, "1"),
            user("m5", 20, "three"),
        ];
        write_history(&doc, &after, &session(false), Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", "oc:m5"]);
    }

    fn zeron_entry(id: &str, role: MessageRole, at: i64, parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            duration_ms: None,
            id: id.into(),
            role,
            parts,
            created_at: at,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        }
    }

    fn ids(doc: &zeron_doc::SessionDoc) -> Vec<String> {
        doc.read_entries()
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect()
    }

    #[test]
    fn once_zerons_turn_is_over_the_transcript_is_the_sessions() {
        let doc = zeron_doc::SessionDoc::init("own").unwrap();
        // Claude ran the chat first; then it moved to OpenCode.
        doc.push_message(&zeron_entry("claude-q", MessageRole::User, 1, Vec::new())).unwrap();
        doc.push_message(&zeron_entry(
            "seam",
            MessageRole::System,
            5,
            vec![MessagePart::Switch {
                id: "s".into(),
                from: "Claude Code".into(),
                to: "OpenCode".into(),
                from_harness: Some("claude-code".into()),
                to_harness: Some("opencode".into()),
            }],
        ))
        .unwrap();
        // Zeron's run wrote its turn; a failure of its own stays.
        doc.push_message(&zeron_entry("prompt", MessageRole::User, 9, Vec::new())).unwrap();
        doc.push_message(&zeron_entry("reply", MessageRole::Assistant, 10, Vec::new())).unwrap();
        doc.push_message(&zeron_entry(
            "failed",
            MessageRole::Assistant,
            11,
            vec![MessagePart::Error {
                id: "e".into(),
                message: "boom".into(),
            }],
        ))
        .unwrap();
        let start = Start::of(&doc.read_entries().unwrap()).unwrap();
        assert_eq!(start, Start { index: 2, at: 5 });
        // The session has Zeron's turn, and one sent from the TUI after it.
        let history = vec![
            user("m1", 10, "from zeron"),
            reply("m2", 11, "zeron's answer"),
            user("m3", 20, "from the tui"),
            reply("m4", 21, "the tui's answer"),
        ];
        write_history(&doc, &history, &session(false), start, "dev").unwrap();
        assert_eq!(
            ids(&doc),
            ["claude-q", "seam", "failed", "oc:m1", "oc:m2", "oc:m3", "oc:m4"]
        );
        // A prompt Zeron wrote that the session hasn't taken yet stays.
        doc.push_message(&zeron_entry("next", MessageRole::User, 500, Vec::new())).unwrap();
        write_history(&doc, &history, &session(true), start, "dev").unwrap();
        assert_eq!(ids(&doc).last().map(String::as_str), Some("next"));
        // Moved on to another agent: no longer the session's to write.
        doc.push_message(&zeron_entry(
            "seam2",
            MessageRole::System,
            600,
            vec![MessagePart::Switch {
                id: "s2".into(),
                from: "OpenCode".into(),
                to: "Codex".into(),
                from_harness: Some("opencode".into()),
                to_harness: Some("codex".into()),
            }],
        ))
        .unwrap();
        assert_eq!(Start::of(&doc.read_entries().unwrap()), None);
    }

    #[test]
    fn an_undo_takes_back_the_turn_zerons_run_wrote() {
        let doc = zeron_doc::SessionDoc::init("undo-own").unwrap();
        let history = vec![
            user("m1", 10, "one"),
            reply("m2", 11, "1"),
            user("m3", 20, "two"),
            reply("m4", 21, "2"),
        ];
        write_history(&doc, &history[..2], &session(false), Start::ALL, "dev").unwrap();
        // Zeron's run wrote the second turn; it was undone before the
        // transcript was made the session's again.
        doc.push_message(&zeron_entry("prompt", MessageRole::User, 19, Vec::new())).unwrap();
        doc.push_message(&zeron_entry("reply", MessageRole::Assistant, 20, Vec::new())).unwrap();
        let mut undone = session(false);
        undone.reverted_from = Some("m3".into());
        write_history(&doc, &history, &undone, Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", REVERT_ENTRY]);
    }

    #[test]
    fn a_message_brought_back_goes_in_where_it_was() {
        let doc = zeron_doc::SessionDoc::init("order").unwrap();
        let history = vec![user("m1", 10, "one"), reply("m2", 11, "1"), user("m3", 12, "two")];
        write_history(&doc, &[history[0].clone(), history[2].clone()], &session(false), Start::ALL, "dev")
            .unwrap();
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", "oc:m3"]);
    }

    #[test]
    fn a_compaction_shows_as_it_runs_and_resets_the_context() {
        let doc = zeron_doc::SessionDoc::init("compact").unwrap();
        let compaction = |status: CompactionStatus, tokens: Option<(u64, u64)>, summary: &str| NativeMessage {
            id: "mc".into(),
            created_at_ms: 30,
            completed_at_ms: None,
            kind: NativeMessageKind::Compaction {
                status,
                tokens_in: tokens.map(|t| t.0),
                tokens_out: tokens.map(|t| t.1),
                summary: summary.into(),
            },
        };
        let mut history = vec![user("m1", 10, "one"), reply("m2", 11, "1")];
        assert_eq!(context_tokens(&history, &session(false), Start::ALL), Some(110));
        history.push(compaction(CompactionStatus::Running, None, "## Goal"));
        write_history(&doc, &history, &session(true), Start::ALL, "dev").unwrap();
        let entry = doc.read_entries().unwrap().pop().unwrap();
        assert_eq!(entry.role, MessageRole::System);
        assert!(matches!(&entry.parts[0], MessagePart::Compaction { status, .. } if status == "running"));
        // (Its summary so far, under it.)
        assert!(matches!(&entry.parts[1], MessagePart::Text { text, .. } if text == "## Goal"));
        // (Still running: the context is what it was.)
        assert_eq!(context_tokens(&history, &session(true), Start::ALL), Some(110));
        history[2] = compaction(CompactionStatus::Completed, Some((648_371, 2_812)), "## Goal\n- tides");
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        let entry = doc.read_entries().unwrap().pop().unwrap();
        assert!(matches!(
            &entry.parts[0],
            MessagePart::Compaction { status, tokens_in: Some(648_371), tokens_out: Some(2_812), .. }
                if status == "completed"
        ));
        assert_eq!(context_tokens(&history, &session(false), Start::ALL), Some(2_812));
        let entry = doc.read_entries().unwrap().pop().unwrap();
        assert!(matches!(&entry.parts[1], MessagePart::Text { text, .. } if text == "## Goal\n- tides"));
    }

    #[test]
    fn a_running_summary_is_appended_word_by_word() {
        let doc = zeron_doc::SessionDoc::init("compact-stream").unwrap();
        let compaction = |summary: &str| NativeMessage {
            id: "mc".into(),
            created_at_ms: 30,
            completed_at_ms: None,
            kind: NativeMessageKind::Compaction {
                status: CompactionStatus::Running,
                tokens_in: None,
                tokens_out: None,
                summary: summary.into(),
            },
        };
        let mut history = vec![user("m1", 10, "one"), reply("m2", 11, "1")];
        // (A running compaction holds its summary's part from the first read,
        // empty: the words appended into it need it to be there.)
        history.push(compaction(""));
        write_history(&doc, &history, &session(true), Start::ALL, "dev").unwrap();
        let entry = doc.read_entries().unwrap().pop().unwrap();
        assert!(matches!(&entry.parts[1], MessagePart::Text { text, .. } if text.is_empty()));
        // Its words arrive: appended into the part, as a reply's do.
        for so_far in ["## Goal", "## Goal\n- tid", "## Goal\n- tides"] {
            history[2] = compaction(so_far);
            write_history(&doc, &history, &session(true), Start::ALL, "dev").unwrap();
            let entry = doc.read_entries().unwrap().pop().unwrap();
            assert!(matches!(&entry.parts[1], MessagePart::Text { text, .. } if text == so_far));
        }
    }

    #[test]
    fn an_undo_of_a_shell_turn_counts_the_message_it_took_back() {
        let doc = zeron_doc::SessionDoc::init("undo-shell").unwrap();
        // (A `!command` run in the session: a command chip, no prompt of the
        // person's.)
        let shell = NativeMessage {
            id: "m1".into(),
            created_at_ms: 10,
            completed_at_ms: Some(11),
            kind: NativeMessageKind::Assistant {
                events: Vec::new(),
                context: None,
            },
        };
        let history = vec![shell];
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        let mut undone = session(false);
        undone.reverted_from = Some("m1".into());
        write_history(&doc, &history, &undone, Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), [REVERT_ENTRY]);
        assert!(matches!(
            &doc.read_entries().unwrap()[0].parts[0],
            MessagePart::Revert { messages: 1, .. }
        ));
    }

    #[test]
    fn an_undo_stands_where_its_turns_were_until_a_redo() {
        let doc = zeron_doc::SessionDoc::init("revert-note").unwrap();
        let history = vec![
            user("m1", 10, "one"),
            reply("m2", 11, "1"),
            user("m3", 20, "two"),
            reply("m4", 21, "2"),
            user("m5", 30, "three"),
            reply("m6", 31, "3"),
        ];
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        let mut undone = session(false);
        undone.reverted_from = Some("m3".into());
        undone.reverted_files = vec![zeron_harness::OpencodeRevertedFile {
            path: "a.rs".into(),
            status: "modified".into(),
            additions: 2,
            deletions: 1,
        }];
        write_history(&doc, &history, &undone, Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", REVERT_ENTRY]);
        let note = doc.read_entries().unwrap().pop().unwrap();
        assert!(matches!(
            &note.parts[0],
            MessagePart::Revert { messages: 2, files, .. } if files.len() == 1 && files[0].additions == 2
        ));
        // Undone further: the same note, counting more.
        undone.reverted_from = Some("m1".into());
        write_history(&doc, &history, &undone, Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), [REVERT_ENTRY]);
        assert!(matches!(&doc.read_entries().unwrap()[0].parts[0], MessagePart::Revert { messages: 3, .. }));
        // Redone: the turns back, the note gone.
        write_history(&doc, &history, &session(false), Start::ALL, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", "oc:m3", "oc:m4", "oc:m5", "oc:m6"]);
    }

    #[test]
    fn a_question_is_asked_once_and_settles_when_answered_elsewhere() {
        let doc = zeron_doc::SessionDoc::init("ask").unwrap();
        let question = NativeQuestion {
            form_id: "frm_1".into(),
            questions: vec![zeron_proto::UserInputQuestion {
                id: "frm_1:q0".into(),
                header: "Scope".into(),
                question: "Which part?".into(),
                options: vec!["Engine".into()],
                multi_select: false,
                prefill: None,
                multiline: false,
            }],
        };
        assert!(write_questions(&doc, std::slice::from_ref(&question), "dev").unwrap());
        // Read again while still waiting: nothing new.
        assert!(!write_questions(&doc, std::slice::from_ref(&question), "dev").unwrap());
        let open = |doc: &zeron_doc::SessionDoc| {
            doc.read_entries().unwrap().iter().any(|entry| {
                entry.parts.iter().any(|part| {
                    matches!(part, MessagePart::Input { request_id, resolved: false, .. } if request_id == "frm_1")
                })
            })
        };
        assert!(open(&doc));
        // Answered in the TUI: it no longer waits, so it settles here too.
        write_questions(&doc, &[], "dev").unwrap();
        assert!(!open(&doc));
        assert_eq!(doc.read_entries().unwrap().len(), 1);
    }

    #[test]
    fn the_tui_actions_are_known_by_what_is_typed() {
        assert_eq!(tui_action(" /compact "), Some(TuiAction::Compact));
        assert_eq!(tui_action("/undo"), Some(TuiAction::Undo));
        assert_eq!(tui_action("/redo"), Some(TuiAction::Redo));
        assert_eq!(tui_action("/fork"), Some(TuiAction::Fork));
        assert_eq!(tui_action("!git status"), Some(TuiAction::Shell));
        assert_eq!(tui_action("!"), None);
        assert_eq!(tui_action("/undo this please"), None);
        assert_eq!(tui_action("please /compact"), None);
        // Picked from the composer's list: a link the agent gets as the command.
        let picked = zeron_proto::invocation::Invocation::Command {
            name: "undo".into(),
        };
        assert_eq!(tui_action(&picked.link()), Some(TuiAction::Undo));
        assert!(note_prompt("acting-chat", "/compact"));
        assert!(acting("acting-chat"));
        assert!(!note_prompt("acting-chat", "hello"));
        assert!(!acting("acting-chat"));
    }

    #[test]
    fn an_imported_chat_id_is_the_same_every_time() {
        assert_eq!(imported_chat_id("ses_a"), imported_chat_id("ses_a"));
        assert_ne!(imported_chat_id("ses_a"), imported_chat_id("ses_b"));
        assert!(uuid::Uuid::parse_str(&imported_chat_id("ses_a")).is_ok());
    }
}
