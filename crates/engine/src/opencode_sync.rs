//! OpenCode's sessions as Zeron chats, kept current as they change in any
//! client. OpenCode 2.x keeps every session on one background service that
//! its TUI, its desktop app and Zeron's runs share ([`zeron_harness`]'s
//! OpenCode driver attaches to it): this host reads them all from there and
//! shows each as a chat under the project of its directory.
//!
//! - A session a Zeron chat already runs on (`harness_session_id`, or the
//!   chat named in its metadata) stays that chat's: only what happened in it
//!   since Zeron last wrote — a turn continued in the TUI — is added.
//! - Any other session becomes a chat of its own, its id derived from the
//!   session's so it is made once, whichever device-side restart reads it.
//!   A chat deleted in Zeron is not brought back.
//! - Its messages keep OpenCode's ids (`oc:<id>`), so a message read again
//!   as it grows replaces its entry instead of adding one.
//! - A chat with a run of Zeron's on this device is left to that run.
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
    NativeChange, NativeMessage, NativeMessageKind, NativeQuestion, NativeSession, OpencodeHarness,
};
use zeron_proto::{Chat, ChatConfig, HarnessId};

use crate::doc_host::DocHost;
use crate::registry::HarnessRegistry;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

/// Entry ids of messages read from an OpenCode session.
const ENTRY_PREFIX: &str = "oc:";

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

/// Start following OpenCode's sessions, for the engine's life.
pub(crate) fn spawn(
    workspace: WorkspaceHost,
    doc_host: DocHost,
    sessions: SessionsEngine,
    registry: Arc<HarnessRegistry>,
) {
    if std::env::var(SYNC_ENV).is_ok_and(|v| matches!(v.trim(), "0" | "false" | "no"))
        || crate::runner::is_cloud_platform()
    {
        return;
    }
    let mirror = Mirror {
        harness: OpencodeHarness::new(),
        workspace,
        doc_host,
        sessions,
        registry,
        read: HashMap::new(),
        working: HashMap::new(),
    };
    tokio::spawn(mirror.run());
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
                        Some(NativeChange::Deleted(id)) => self.deleted(&id),
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
        // (A run of Zeron's is writing this chat: it has the session live.)
        if let Some(chat) = &known
            && self.sessions.has_run(&chat.id)
        {
            return;
        }
        let unchanged = self.read.get(&session.id)
            == Some(&(session.updated_at_ms, session.reverted_from.clone()));
        if unchanged && known.is_some() && !session.busy {
            return;
        }
        let history = match self.harness.native_history(session).await {
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
        let result = match known {
            Some(chat) => self.update(&chat, session, &history, &questions),
            None => self.import(session, &history, &questions),
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
    /// its directory. Its history is saved before its row is published, so a
    /// crash never leaves an empty chat in the sidebar.
    fn import(
        &self,
        session: &NativeSession,
        history: &[NativeMessage],
        questions: &[NativeQuestion],
    ) -> Result<(), String> {
        // A session Zeron made for a chat that is gone went with that chat.
        if session.zeron_chat.is_some() {
            return Ok(());
        }
        let chat_id = imported_chat_id(&session.id);
        if self.workspace.chat_deleted(&chat_id) {
            return Ok(());
        }
        let space_id = self
            .workspace
            .space_for_path(&session.directory)
            .map_err(|e| e.to_string())?;
        let doc = self.doc_host.open(&chat_id).map_err(|e| e.to_string())?;
        let written = write_history(doc.doc(), history, session, None, self.doc_host.device_id())
            .map_err(|e| e.to_string())?;
        write_questions(doc.doc(), questions, self.doc_host.device_id())
            .map_err(|e| e.to_string())?;
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

    /// A chat that shows `session` already: what's new in the session, and
    /// for an imported chat its title and archived state too.
    fn update(
        &self,
        chat: &Chat,
        session: &NativeSession,
        history: &[NativeMessage],
        questions: &[NativeQuestion],
    ) -> Result<(), String> {
        let imported = chat.id == imported_chat_id(&session.id);
        let doc = self.doc_host.open(&chat.id).map_err(|e| e.to_string())?;
        // Zeron's own entries end where its last run did; only what came
        // after (a turn in another client) is the session's to add.
        let since = (!imported).then(|| zeron_written_until(doc.doc()));
        let written = write_history(
            doc.doc(),
            history,
            session,
            since,
            self.doc_host.device_id(),
        )
        .map_err(|e| e.to_string())?;
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

/// Where Zeron's own entries end: the last one's end (its start, plus how
/// long it streamed). Entries read from the session are not Zeron's.
fn zeron_written_until(doc: &zeron_doc::SessionDoc) -> i64 {
    doc.read_entries()
        .unwrap_or_default()
        .iter()
        .filter(|entry| !entry.id.starts_with(ENTRY_PREFIX))
        .map(|entry| entry.created_at + entry.duration_ms.unwrap_or(0))
        .max()
        .unwrap_or(i64::MIN)
}

/// What [`write_history`] did.
#[derive(Debug, Default)]
struct Written {
    /// An entry was added or rewritten.
    changed: bool,
    /// The last message's text, for the sidebar.
    preview: Option<String>,
}

/// Write `history` into `doc`: a message not there yet is added, one that
/// grew is rewritten in place, one that's the same is left alone. Only
/// messages after `since` count, when given (a chat Zeron also writes).
fn write_history(
    doc: &zeron_doc::SessionDoc,
    history: &[NativeMessage],
    session: &NativeSession,
    since: Option<i64>,
    device_id: &str,
) -> Result<Written, zeron_doc::DocError> {
    // An undo staged in the session takes its message and all after it back,
    // as OpenCode's own clients show it; a redo brings them in again.
    let cut = session
        .reverted_from
        .as_ref()
        .and_then(|id| history.iter().position(|message| &message.id == id))
        .unwrap_or(history.len());
    let history = &history[..cut];
    // Entries read from the session whose message it no longer has (undone,
    // or an undo that a new prompt made final) go too.
    let kept: HashSet<String> = history
        .iter()
        .map(|message| format!("{ENTRY_PREFIX}{}", message.id))
        .collect();
    let mut written = Written::default();
    let gone: Vec<usize> = doc
        .read_entries()?
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.id.starts_with(ENTRY_PREFIX) && !kept.contains(&entry.id))
        .map(|(index, _)| index)
        .collect();
    for index in gone.into_iter().rev() {
        doc.remove_message(index)?;
        written.changed = true;
    }
    let existing = doc.read_entries()?;
    let at: HashMap<&str, usize> = existing
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.as_str(), index))
        .collect();
    let last = history.len().saturating_sub(1);
    for (index, message) in history.iter().enumerate() {
        if since.is_some_and(|since| message.created_at_ms <= since) {
            continue;
        }
        // (The last reply of a running session is still being written.)
        let streaming = session.busy && index == last;
        let entry = entry_of(message, streaming, device_id);
        if let Some(text) = preview_of(&entry) {
            written.preview = Some(text);
        }
        match at.get(entry.id.as_str()) {
            Some(&index) => {
                let current = &existing[index];
                if current.parts != entry.parts || current.status != entry.status {
                    doc.replace_message(index, &entry)?;
                    written.changed = true;
                }
            }
            None => {
                doc.push_message(&entry)?;
                written.changed = true;
            }
        }
    }
    Ok(written)
}

/// One of OpenCode's TUI actions, as typed in Zeron's composer for an
/// OpenCode chat. The driver does them (`zeron_harness`'s OpenCode actions);
/// the transcript shows what they did, never the typed command as a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiAction {
    Compact,
    Undo,
    Redo,
    Fork,
    /// `!command`, the TUI's shell mode: its command chip says what ran.
    Shell,
}

/// The action `prompt` is, if it is one.
pub(crate) fn tui_action(prompt: &str) -> Option<TuiAction> {
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

/// What an undo took out of a chat's transcript, and where it was: a redo
/// puts it back there. Per chat, newest last; kept for the engine's life
/// (after a restart a redo still restores OpenCode's session, and an
/// imported chat's entries with it, through the sync).
struct Undone {
    at: usize,
    entries: Vec<SessionMessageEntry>,
}

static UNDONE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, Vec<Undone>>>> =
    std::sync::LazyLock::new(Default::default);

fn undone() -> std::sync::MutexGuard<'static, HashMap<String, Vec<Undone>>> {
    UNDONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `/undo` in the transcript, as OpenCode's session takes it back: the last
/// prompt and everything after it go, kept for a redo.
pub(crate) fn undo_in_transcript(
    chat_id: &str,
    doc: &zeron_doc::SessionDoc,
) -> Result<(), zeron_doc::DocError> {
    let entries = doc.read_entries()?;
    let Some(at) = entries
        .iter()
        .rposition(|entry| entry.role == MessageRole::User)
    else {
        return Ok(());
    };
    for index in (at..entries.len()).rev() {
        doc.remove_message(index)?;
    }
    undone()
        .entry(chat_id.to_owned())
        .or_default()
        .push(Undone {
            at,
            entries: entries[at..].to_vec(),
        });
    Ok(())
}

/// `/redo` in the transcript: what the last undo took goes back where it
/// was (entries back already, by the sync, are left as they are).
pub(crate) fn redo_in_transcript(
    chat_id: &str,
    doc: &zeron_doc::SessionDoc,
) -> Result<(), zeron_doc::DocError> {
    let Some(undo) = undone().get_mut(chat_id).and_then(Vec::pop) else {
        return Ok(());
    };
    let present: HashSet<String> = doc.read_entries()?.into_iter().map(|e| e.id).collect();
    let mut at = undo.at;
    for entry in undo.entries {
        if present.contains(&entry.id) {
            continue;
        }
        doc.insert_message(at, &entry)?;
        at += 1;
    }
    Ok(())
}

/// A new prompt makes the chat's undos final, as it does in OpenCode.
pub(crate) fn forget_undone(chat_id: &str) {
    undone().remove(chat_id);
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
        NativeMessageKind::Assistant { events } => {
            let mut parts = Vec::new();
            for event in events {
                zeron_doc::fold_event_into_parts(&mut parts, event);
            }
            (MessageRole::Assistant, parts)
        }
    };
    SessionMessageEntry {
        duration_ms: None,
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
            reverted_from: None,
        }
    }

    fn user(id: &str, at: i64, text: &str) -> NativeMessage {
        NativeMessage {
            id: id.into(),
            created_at_ms: at,
            kind: NativeMessageKind::User { text: text.into() },
        }
    }

    fn reply(id: &str, at: i64, text: &str) -> NativeMessage {
        NativeMessage {
            id: id.into(),
            created_at_ms: at,
            kind: NativeMessageKind::Assistant {
                events: vec![AgentEvent::TextDelta { text: text.into() }],
            },
        }
    }

    #[test]
    fn a_session_reads_in_once_and_grows_in_place() {
        let doc = zeron_doc::SessionDoc::init("sync").unwrap();
        let history = vec![user("m1", 10, "hi"), reply("m2", 11, "hel")];
        let written = write_history(&doc, &history, &session(true), None, "dev").unwrap();
        assert!(written.changed);
        assert_eq!(written.preview.as_deref(), Some("hel"));
        let entries = doc.read_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "oc:m1");
        assert_eq!(entries[1].status, Some(MessageStatus::Streaming));

        // The reply grows and the turn ends: the same entry, rewritten.
        let history = vec![user("m1", 10, "hi"), reply("m2", 11, "hello")];
        write_history(&doc, &history, &session(false), None, "dev").unwrap();
        let entries = doc.read_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].status, Some(MessageStatus::Complete));
        assert_eq!(preview_of(&entries[1]).as_deref(), Some("hello"));

        // Read again unchanged: nothing written.
        let again = write_history(&doc, &history, &session(false), None, "dev").unwrap();
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
        write_history(&doc, &history, &session(false), None, "dev").unwrap();
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
            write_history(&doc, &history, &undone, None, "dev")
                .unwrap()
                .changed
        );
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2"]);
        // /redo: the same messages, back in their place.
        write_history(&doc, &history, &session(false), None, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", "oc:m3", "oc:m4"]);
        // The undo made final by a new prompt: the session no longer has them.
        let after = vec![
            user("m1", 10, "one"),
            reply("m2", 11, "1"),
            user("m5", 20, "three"),
        ];
        write_history(&doc, &after, &session(false), None, "dev").unwrap();
        assert_eq!(ids(&doc), ["oc:m1", "oc:m2", "oc:m5"]);
    }

    #[test]
    fn a_chat_zeron_writes_only_gains_what_came_after_its_runs() {
        let doc = zeron_doc::SessionDoc::init("own").unwrap();
        doc.push_message(&SessionMessageEntry {
            duration_ms: Some(5),
            id: "zeron-reply".into(),
            role: MessageRole::Assistant,
            parts: Vec::new(),
            created_at: 100,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        })
        .unwrap();
        let since = zeron_written_until(&doc);
        assert_eq!(since, 105);
        // The session holds Zeron's own turn too (before 105) and one the TUI
        // added after it.
        let history = vec![
            user("m1", 99, "from zeron"),
            reply("m2", 104, "zeron's answer"),
            user("m3", 200, "from the tui"),
        ];
        write_history(&doc, &history, &session(false), Some(since), "dev").unwrap();
        let ids: Vec<String> = doc
            .read_entries()
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, ["zeron-reply", "oc:m3"]);
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
    }

    #[test]
    fn undo_takes_the_last_turn_out_and_redo_puts_it_back_in_place() {
        let doc = zeron_doc::SessionDoc::init("undo-visual").unwrap();
        let entry = |id: &str, role: MessageRole| SessionMessageEntry {
            duration_ms: None,
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: id.into(),
            }],
            created_at: 0,
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        };
        for (id, role) in [
            ("p1", MessageRole::User),
            ("r1", MessageRole::Assistant),
            ("p2", MessageRole::User),
            ("r2", MessageRole::Assistant),
        ] {
            doc.push_message(&entry(id, role)).unwrap();
        }
        let ids = |doc: &zeron_doc::SessionDoc| -> Vec<String> {
            doc.read_entries()
                .unwrap()
                .into_iter()
                .map(|e| e.id)
                .collect()
        };
        let chat = "undo-visual-chat";
        undo_in_transcript(chat, &doc).unwrap();
        assert_eq!(ids(&doc), ["p1", "r1"]);
        // The action's own chip lands after it.
        doc.push_message(&entry("undo-chip", MessageRole::Assistant))
            .unwrap();
        redo_in_transcript(chat, &doc).unwrap();
        assert_eq!(ids(&doc), ["p1", "r1", "p2", "r2", "undo-chip"]);
        // Nothing left to redo: nothing changes.
        redo_in_transcript(chat, &doc).unwrap();
        assert_eq!(ids(&doc).len(), 5);
        // An undo made final by a new prompt can't be redone.
        undo_in_transcript(chat, &doc).unwrap();
        forget_undone(chat);
        redo_in_transcript(chat, &doc).unwrap();
        assert_eq!(ids(&doc), ["p1", "r1"]);
    }

    #[test]
    fn an_imported_chat_id_is_the_same_every_time() {
        assert_eq!(imported_chat_id("ses_a"), imported_chat_id("ses_a"));
        assert_ne!(imported_chat_id("ses_a"), imported_chat_id("ses_b"));
        assert!(uuid::Uuid::parse_str(&imported_chat_id("ses_a")).is_ok());
    }
}
