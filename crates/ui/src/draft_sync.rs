//! Live-synced composer drafts (`docs/draft-sync.md` §4).
//!
//! `AppState` keeps one [`DraftSync`] for the selected chat while its composer is showing: a
//! `DraftDoc` replica seeded from the first `WatchDraft` frame, an inbox of frames the composer
//! has not applied yet, and an outbox that coalesces the replica's local commits into `EditDraft`
//! calls. The composer is the only reader: it drains the inbox with its caret and selection
//! offsets ([`AppState::draft_take_remote`]) so Loro carries them through remote edits, and it is
//! the only writer ([`AppState::draft_local_edit`], [`AppState::draft_clear`]).
//!
//! Nothing here touches the text input, so it is testable with a fake engine alone.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gpui::{Context, Task};
use loro::{Subscription, VersionVector};
use tokio::sync::Notify;
use zeron_doc::{DraftDoc, diff_splice};
use zeron_proto::capabilities;
use zeron_proto::draft::{DraftFrame, DraftTarget, EditDraft};
use zeron_rpc::{RpcError, methods};

use crate::state::{AppState, EngineHandle};

/// Trailing debounce before local commits are pushed to the engine. A burst of typing becomes
/// one `EditDraft`; while a push is in flight further commits just mark the outbox dirty.
pub(crate) const PUSH_DEBOUNCE: Duration = Duration::from_millis(60);
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// What the replica has to tell the engine. The flags are level-triggered and `wake` holds at
/// most one permit, so any number of commits costs one wakeup and no queue.
#[derive(Default)]
pub(crate) struct Outbox {
    dirty: AtomicBool,
    clear: AtomicBool,
    wake: Notify,
}

impl Outbox {
    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
        self.wake.notify_one();
    }

    fn request_clear(&self) {
        self.clear.store(true, Ordering::Release);
        self.wake.notify_one();
    }
}

/// One decoded `DraftFrame`, waiting for the composer.
struct Incoming {
    epoch: u64,
    reset: bool,
    bytes: Vec<u8>,
}

/// The composer's view of its own box when it drains the inbox.
pub(crate) struct LocalDraft<'a> {
    /// What the text input holds right now.
    pub text: &'a str,
    /// What the box held when the draft watch began (the fallback draft loaded on navigation).
    /// Only used to merge typing that happened before the first frame arrived.
    pub base: &'a str,
}

/// Work for the push task, in the order the engine must see it.
pub(crate) enum Outgoing {
    /// The draft was sent: discard it. Always ahead of edits so a stale edit can never land in
    /// the next epoch.
    Clear,
    Edit {
        update: Vec<u8>,
        version: VersionVector,
        generation: u64,
    },
}

pub(crate) struct DraftSync {
    pub(crate) chat_id: String,
    /// `None` until the first `reset` frame.
    doc: Option<DraftDoc>,
    /// Bumped whenever `doc` is replaced, so a stale push ack is ignored.
    generation: u64,
    _local_updates: Option<Subscription>,
    epoch: Option<u64>,
    inbox: VecDeque<Incoming>,
    /// The engine's version as far as this window knows it (snapshot, then our own acks).
    sent: VersionVector,
    outbox: Arc<Outbox>,
    /// A `ClearDraft` is in flight: updates still on the wire belong to the discarded draft and
    /// must not resurrect it in the fresh replica. Ends at the next `reset` frame or the ack.
    clearing: bool,
    tasks: Vec<Task<()>>,
}

impl DraftSync {
    pub(crate) fn new(chat_id: &str, outbox: Arc<Outbox>) -> Self {
        Self {
            chat_id: chat_id.to_owned(),
            doc: None,
            generation: 0,
            _local_updates: None,
            epoch: None,
            inbox: VecDeque::new(),
            sent: VersionVector::default(),
            outbox,
            clearing: false,
            tasks: Vec::new(),
        }
    }

    pub(crate) fn text(&self) -> Option<String> {
        self.doc.as_ref().map(DraftDoc::text)
    }

    pub(crate) fn has_incoming(&self) -> bool {
        !self.inbox.is_empty()
    }

    fn push_frame(&mut self, frame: Incoming) {
        if frame.reset {
            self.clearing = false;
        } else if self.clearing {
            return;
        }
        self.inbox.push_back(frame);
    }

    fn install(&mut self, doc: DraftDoc, sent: VersionVector, epoch: Option<u64>) {
        let outbox = self.outbox.clone();
        self._local_updates = Some(doc.subscribe_local_update(move |_| outbox.mark_dirty()));
        self.doc = Some(doc);
        self.sent = sent;
        self.generation = self.generation.wrapping_add(1);
        if epoch.is_some() {
            self.epoch = epoch;
        }
    }

    /// Apply every frame the composer has not seen. `offsets` are byte offsets into
    /// `local.text` (caret, selection edges) and are moved to where they belong in the returned
    /// text. Returns the new text when it differs from `local.text`.
    pub(crate) fn take_remote(
        &mut self,
        local: &LocalDraft<'_>,
        offsets: &mut [usize],
    ) -> Option<String> {
        if self.inbox.is_empty() {
            return None;
        }
        // The replica normally equals the box (every user edit is pushed into it); when a path
        // let them drift, what the user sees wins so typed text is never dropped.
        if let Some(doc) = &self.doc
            && doc.text() != local.text
            && let Err(error) = doc.set_text(local.text)
        {
            tracing::warn!(%error, "draft: could not adopt the composer text");
        }
        let mut current = local.text.to_owned();
        for frame in std::mem::take(&mut self.inbox) {
            let Incoming {
                epoch,
                reset,
                bytes,
            } = frame;
            // `replaced`: the replica was swapped rather than imported into, so its offsets
            // have no Loro cursors to ride and are carried by position instead.
            let replaced = match (reset, self.doc.is_some()) {
                (true, false) => {
                    self.seed(&bytes, epoch, local, &current);
                    true
                }
                (true, true) => match DraftDoc::from_snapshot(&bytes) {
                    // A snapshot that still contains everything this window has seen from the
                    // engine is a resubscribe (lag, reconnect): merge, keeping unsent edits.
                    // Anything else — the engine started a fresh doc because the draft was sent
                    // from any device or window — replaces the replica. Comparing histories
                    // rather than epochs is what makes a same-engine clear (whose epoch only
                    // moves once the room confirms the discard) replace instead of merge.
                    Ok(snapshot) if snapshot.version().includes_vv(&self.sent) => {
                        self.import(&bytes, offsets);
                        self.sent = snapshot.version();
                        self.epoch = Some(epoch);
                        false
                    }
                    Ok(doc) => {
                        tracing::debug!(from = ?self.epoch, to = epoch, "draft: replaced by a fresh snapshot");
                        let sent = doc.version();
                        self.install(doc, sent, Some(epoch));
                        true
                    }
                    Err(error) => {
                        tracing::warn!(%error, "draft: bad reset snapshot");
                        false
                    }
                },
                (false, true) => {
                    self.import(&bytes, offsets);
                    false
                }
                (false, false) => {
                    tracing::debug!("draft: update before the first reset frame; dropped");
                    false
                }
            };
            if let Some(doc) = &self.doc {
                let after = doc.text();
                if after != current {
                    if replaced {
                        carry_offsets(&current, &after, offsets);
                    }
                    current = after;
                }
            }
        }
        (current != local.text).then_some(current)
    }

    fn import(&self, bytes: &[u8], offsets: &mut [usize]) {
        if let Some(doc) = &self.doc
            && let Err(error) = doc.import_tracking(bytes, offsets)
        {
            tracing::warn!(%error, "draft: bad update");
        }
    }

    fn seed(&mut self, bytes: &[u8], epoch: u64, local: &LocalDraft<'_>, current: &str) {
        let doc = match DraftDoc::from_snapshot(bytes) {
            Ok(doc) => doc,
            Err(error) => {
                tracing::warn!(%error, "draft: bad first snapshot");
                return;
            }
        };
        let sent = doc.version();
        let remote = doc.text();
        self.install(doc, sent, Some(epoch));
        let Some(doc) = &self.doc else { return };
        let merged = match merge_seed(local.base, current, &remote) {
            SeedMerge::Remote => return,
            SeedMerge::Local => current.to_owned(),
            SeedMerge::Text(text) => text,
        };
        if let Err(error) = doc.set_text(&merged) {
            tracing::warn!(%error, "draft: could not seed the draft with the typed text");
        }
    }

    /// The user edited the box: mirror it into the replica (one splice; the replica's local
    /// update hook wakes the push task).
    pub(crate) fn local_edit(&mut self, text: &str) {
        if let Some(doc) = &self.doc
            && let Err(error) = doc.set_text(text)
        {
            tracing::warn!(%error, "draft: local edit failed");
        }
    }

    /// The draft was sent: start over with an empty replica and tell the engine to discard.
    pub(crate) fn clear_local(&mut self) {
        self.inbox.clear();
        self.clearing = true;
        if self.doc.is_some() {
            self.install(DraftDoc::new(), VersionVector::default(), None);
        }
        // Edits typed before the send are gone with the old replica.
        self.outbox.dirty.store(false, Ordering::Release);
        self.outbox.request_clear();
    }

    pub(crate) fn next_outgoing(&mut self) -> Option<Outgoing> {
        if self.outbox.clear.swap(false, Ordering::AcqRel) {
            return Some(Outgoing::Clear);
        }
        if !self.outbox.dirty.swap(false, Ordering::AcqRel) {
            return None;
        }
        let doc = self.doc.as_ref()?;
        match doc.export_since(&self.sent) {
            Ok(update) => Some(Outgoing::Edit {
                update,
                version: doc.version(),
                generation: self.generation,
            }),
            Err(error) => {
                tracing::warn!(%error, "draft: export failed");
                None
            }
        }
    }

    fn outgoing_done(&mut self, sent: &Outgoing) {
        match sent {
            Outgoing::Clear => self.clearing = false,
            Outgoing::Edit {
                version,
                generation,
                ..
            } => {
                if *generation == self.generation {
                    self.sent = version.clone();
                }
            }
        }
    }

    fn outgoing_failed(&mut self, failed: &Outgoing) {
        match failed {
            Outgoing::Clear => self.outbox.request_clear(),
            Outgoing::Edit { generation, .. } => {
                if *generation == self.generation {
                    self.outbox.mark_dirty();
                }
            }
        }
    }
}

/// How the first snapshot combines with what is already in the box.
#[derive(Debug, PartialEq, Eq)]
enum SeedMerge {
    /// Adopt the engine's text.
    Remote,
    /// Push the box's text into the replica.
    Local,
    /// Both changed independently: this text, on top of the engine's.
    Text(String),
}

/// `base` is what the box held when the watch began, `local` what it holds now and `remote` the
/// engine's snapshot. Typed text is never dropped; deletions made before the first frame may be.
fn merge_seed(base: &str, local: &str, remote: &str) -> SeedMerge {
    if local == remote {
        SeedMerge::Remote
    } else if remote.is_empty() {
        SeedMerge::Local
    } else if local == base {
        SeedMerge::Remote
    } else if remote == base {
        SeedMerge::Local
    } else if local.is_empty() {
        SeedMerge::Remote
    } else {
        match diff_splice(base, local) {
            Some((_, _, inserted)) if !inserted.is_empty() => {
                let mut merged = remote.to_owned();
                if !merged.ends_with('\n') {
                    merged.push('\n');
                }
                merged.push_str(inserted);
                SeedMerge::Text(merged)
            }
            _ => SeedMerge::Remote,
        }
    }
}

/// After the replica was replaced (rather than imported into) the offsets have no cursors to
/// ride: keep a caret at the end at the end, clamp the rest.
fn carry_offsets(before: &str, after: &str, offsets: &mut [usize]) {
    for offset in offsets {
        *offset = if *offset >= before.len() {
            after.len()
        } else {
            floor_boundary(after, *offset)
        };
    }
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn decode_frame(value: serde_json::Value) -> Option<Incoming> {
    let frame: DraftFrame = match serde_json::from_value(value) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::warn!(%error, "dropping malformed draft frame");
            return None;
        }
    };
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    match engine.decode(frame.update.as_bytes()) {
        Ok(bytes) => Some(Incoming {
            epoch: frame.epoch,
            reset: frame.reset,
            bytes,
        }),
        Err(error) => {
            tracing::warn!(%error, "dropping draft frame with bad base64");
            None
        }
    }
}

impl AppState {
    /// Whether the selected chat's composer draft is live-synced (capability advertised and a
    /// watch is attached), even before the first frame has arrived.
    pub fn draft_active(&self, chat_id: &str) -> bool {
        !chat_id.is_empty()
            && self
                .draft
                .as_ref()
                .is_some_and(|draft| draft.chat_id == chat_id)
    }

    /// The replica's text once the first frame has seeded it.
    pub fn draft_text(&self, chat_id: &str) -> Option<String> {
        self.draft
            .as_ref()
            .filter(|draft| draft.chat_id == chat_id)
            .and_then(DraftSync::text)
    }

    pub fn draft_has_incoming(&self, chat_id: &str) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft.chat_id == chat_id && draft.has_incoming())
    }

    /// Apply the frames the watch has received. See [`DraftSync::take_remote`]. Deliberately
    /// takes no `Context`: it must not re-notify the observer that is calling it.
    pub(crate) fn draft_take_remote(
        &mut self,
        chat_id: &str,
        local: &LocalDraft<'_>,
        offsets: &mut [usize],
    ) -> Option<String> {
        self.draft
            .as_mut()
            .filter(|draft| draft.chat_id == chat_id)?
            .take_remote(local, offsets)
    }

    /// Mirror a user edit of the box into the replica.
    pub(crate) fn draft_local_edit(&mut self, chat_id: &str, text: &str) {
        if let Some(draft) = self.draft.as_mut().filter(|d| d.chat_id == chat_id) {
            draft.local_edit(text);
        }
    }

    /// The draft was sent (or abandoned): discard it everywhere.
    pub(crate) fn draft_clear(&mut self, chat_id: &str) {
        if let Some(draft) = self.draft.as_mut().filter(|d| d.chat_id == chat_id) {
            draft.clear_local();
        }
    }

    pub(crate) fn draft_next_outgoing(&mut self, chat_id: &str) -> Option<Outgoing> {
        self.draft
            .as_mut()
            .filter(|draft| draft.chat_id == chat_id)?
            .next_outgoing()
    }

    fn draft_outgoing_done(&mut self, chat_id: &str, sent: &Outgoing) {
        if let Some(draft) = self.draft.as_mut().filter(|d| d.chat_id == chat_id) {
            draft.outgoing_done(sent);
        }
    }

    fn draft_outgoing_failed(&mut self, chat_id: &str, failed: &Outgoing) {
        if let Some(draft) = self.draft.as_mut().filter(|d| d.chat_id == chat_id) {
            draft.outgoing_failed(failed);
        }
    }

    fn draft_push_frame(&mut self, chat_id: &str, frame: Incoming) -> bool {
        match self.draft.as_mut().filter(|d| d.chat_id == chat_id) {
            Some(draft) => {
                draft.push_frame(frame);
                true
            }
            None => false,
        }
    }

    /// Attach the draft watch and push task for `chat_id` (the selected chat) when the engine
    /// advertises `DRAFT_SYNC_V1`. Re-attaching the same chat (engine reconnect) keeps the
    /// replica, so edits typed while disconnected survive and merge on the next snapshot.
    pub(crate) fn start_draft_watch(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(handle) = self.engine().cloned() else {
            return;
        };
        if chat_id.is_empty() || !handle.engine_info().supports(capabilities::DRAFT_SYNC_V1) {
            self.draft = None;
            return;
        }
        let outbox = match self.draft.as_mut().filter(|d| d.chat_id == chat_id) {
            Some(existing) => {
                existing.tasks.clear();
                existing.outbox.clone()
            }
            None => {
                self.stop_draft_watch(cx);
                let outbox = Arc::new(Outbox::default());
                self.draft = Some(DraftSync::new(chat_id, outbox.clone()));
                outbox
            }
        };
        let tasks = vec![
            spawn_draft_watch(cx, handle.clone(), chat_id.to_owned()),
            spawn_draft_pump(cx, handle, chat_id.to_owned(), outbox),
        ];
        if let Some(draft) = self.draft.as_mut() {
            draft.tasks = tasks;
        }
    }

    /// Leave the chat: drop the watch, first handing the engine whatever it has not seen yet
    /// (a discard or the last keystrokes) so leaving right after typing loses nothing.
    pub(crate) fn stop_draft_watch(&mut self, cx: &mut Context<Self>) {
        let Some(mut draft) = self.draft.take() else {
            return;
        };
        draft.tasks.clear();
        let Some(handle) = self.engine().cloned() else {
            return;
        };
        let mut calls = Vec::new();
        while let Some(outgoing) = draft.next_outgoing() {
            calls.push(outgoing_call(&draft.chat_id, &outgoing));
        }
        if calls.is_empty() {
            return;
        }
        cx.spawn(async move |_, _| {
            for (method, params) in calls {
                if let Err(error) = handle.client().call(method, params).await {
                    tracing::debug!(%error, method, "draft: flush on leave failed");
                    break;
                }
            }
        })
        .detach();
    }
}

fn outgoing_call(chat_id: &str, outgoing: &Outgoing) -> (&'static str, serde_json::Value) {
    match outgoing {
        Outgoing::Clear => (
            methods::CLEAR_DRAFT,
            serde_json::to_value(DraftTarget {
                chat_id: chat_id.to_owned(),
            })
            .unwrap_or_default(),
        ),
        Outgoing::Edit { update, .. } => (
            methods::EDIT_DRAFT,
            serde_json::to_value(EditDraft {
                chat_id: chat_id.to_owned(),
                update: BASE64.encode(update),
            })
            .unwrap_or_default(),
        ),
    }
}

/// `WatchDraft` for one chat, retried like the queue watch. Frames are decoded here and parked
/// in the inbox; the composer applies them so it can carry its selection through each one.
fn spawn_draft_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    chat_id: String,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let mut backoff = RETRY_MIN;
        loop {
            let params = serde_json::to_value(DraftTarget {
                chat_id: chat_id.clone(),
            })
            .unwrap_or_default();
            match handle
                .client()
                .subscribe_checked(methods::WATCH_DRAFT, params)
                .await
            {
                Ok(mut rx) => {
                    while let Some(value) = rx.recv().await {
                        backoff = RETRY_MIN;
                        let Some(frame) = decode_frame(value) else {
                            continue;
                        };
                        let alive = this.update(cx, |state, cx| {
                            if state.draft_push_frame(&chat_id, frame) {
                                cx.notify();
                            }
                        });
                        if alive.is_err() {
                            return;
                        }
                    }
                }
                Err(RpcError::UnknownMethod(_)) => {
                    tracing::warn!("engine advertises draft sync but not WatchDraft");
                    return;
                }
                Err(error) => {
                    tracing::debug!(%chat_id, %error, "draft watch failed; retrying");
                }
            }
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(backoff).await;
            backoff = (backoff * 2).min(RETRY_MAX);
        }
    })
}

/// Serialises pushes: one call in flight, everything typed meanwhile folds into the next.
fn spawn_draft_pump(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    chat_id: String,
    outbox: Arc<Outbox>,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let mut backoff = RETRY_MIN;
        loop {
            outbox.wake.notified().await;
            cx.background_executor().timer(PUSH_DEBOUNCE).await;
            loop {
                let next = match this.update(cx, |state, _| state.draft_next_outgoing(&chat_id)) {
                    Ok(next) => next,
                    Err(_) => return,
                };
                let Some(next) = next else { break };
                let (method, params) = outgoing_call(&chat_id, &next);
                match handle.client().call(method, params).await {
                    Ok(_) => {
                        backoff = RETRY_MIN;
                        if this
                            .update(cx, |state, _| state.draft_outgoing_done(&chat_id, &next))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(RpcError::UnknownMethod(_)) => {
                        tracing::warn!(method, "engine advertises draft sync but not {method}");
                        this.update(cx, |state, _| state.draft_outgoing_done(&chat_id, &next))
                            .ok();
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(%error, method, "draft push failed; retrying");
                        if this
                            .update(cx, |state, _| state.draft_outgoing_failed(&chat_id, &next))
                            .is_err()
                        {
                            return;
                        }
                        cx.background_executor().timer(backoff).await;
                        backoff = (backoff * 2).min(RETRY_MAX);
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(reset: bool, epoch: u64, doc: &DraftDoc) -> Incoming {
        Incoming {
            epoch,
            reset,
            bytes: doc.snapshot(),
        }
    }

    fn sync() -> DraftSync {
        DraftSync::new("c", Arc::new(Outbox::default()))
    }

    #[test]
    fn seed_merge_never_drops_typed_text() {
        use SeedMerge::*;
        assert_eq!(merge_seed("", "", ""), Remote);
        assert_eq!(merge_seed("", "abc", "abc"), Remote);
        assert_eq!(merge_seed("", "abc", ""), Local);
        // Nothing typed since the fallback draft loaded: the engine's newer text wins.
        assert_eq!(merge_seed("old", "old", "newer"), Remote);
        // Typed before the frame, engine unchanged since.
        assert_eq!(merge_seed("old", "old more", "old"), Local);
        // Both changed: the typed insertion rides on top of the engine's draft.
        assert_eq!(
            merge_seed("hello", "hello there", "hello world"),
            Text("hello world\n there".into())
        );
        assert_eq!(
            merge_seed("", "typed", "theirs\n"),
            Text("theirs\ntyped".into())
        );
    }

    #[test]
    fn first_frame_seeds_and_later_updates_carry_offsets() {
        let engine = DraftDoc::new();
        engine.set_text("hello world").unwrap();
        let mut sync = sync();
        sync.push_frame(frame(true, 1, &engine));
        let mut offsets = [0usize, 0];
        let text = sync.take_remote(&LocalDraft { text: "", base: "" }, &mut offsets);
        assert_eq!(text.as_deref(), Some("hello world"));
        assert_eq!(sync.text().as_deref(), Some("hello world"));
        assert!(sync.text().is_some());

        // A remote edit before the caret shifts it; the engine doc is a peer of the replica.
        let before = engine.version();
        engine.set_text("well hello world").unwrap();
        let update = engine.export_since(&before).unwrap();
        sync.push_frame(Incoming {
            epoch: 1,
            reset: false,
            bytes: update,
        });
        let mut caret = [6usize];
        let text = sync.take_remote(
            &LocalDraft {
                text: "hello world",
                base: "",
            },
            &mut caret,
        );
        assert_eq!(text.as_deref(), Some("well hello world"));
        assert_eq!(caret, [11]);
    }

    #[test]
    fn later_reset_replaces_and_resubscribe_merges() {
        let engine = DraftDoc::new();
        engine.set_text("abc").unwrap();
        let mut sync = sync();
        sync.push_frame(frame(true, 1, &engine));
        sync.take_remote(&LocalDraft { text: "", base: "" }, &mut []);

        // Typed while the stream was down, then the stream reconnects at the same epoch.
        sync.local_edit("abc!");
        sync.push_frame(frame(true, 1, &engine));
        let text = sync.take_remote(
            &LocalDraft {
                text: "abc!",
                base: "",
            },
            &mut [],
        );
        assert_eq!(
            text, None,
            "unsent local edits survive a same-epoch snapshot"
        );
        assert_eq!(sync.text().as_deref(), Some("abc!"));

        // The draft was sent from another device: next epoch, empty snapshot.
        sync.push_frame(frame(true, 2, &DraftDoc::new()));
        let mut caret = [4usize];
        let text = sync.take_remote(
            &LocalDraft {
                text: "abc!",
                base: "",
            },
            &mut caret,
        );
        assert_eq!(text.as_deref(), Some(""));
        assert_eq!(caret, [0]);
        assert_eq!(sync.text().as_deref(), Some(""));
    }

    /// The engine's own clear starts a fresh doc but keeps the OLD epoch until the room confirms
    /// the discard, so another window on the same engine sees a same-epoch, empty reset. It must
    /// replace (not merge into) its replica: otherwise its next keystroke would export the whole
    /// old history and resurrect the sent draft everywhere.
    #[test]
    fn a_same_epoch_clear_from_another_window_replaces_and_does_not_resurrect() {
        let engine = DraftDoc::new();
        engine.set_text("sent prompt").unwrap();
        let mut sync = sync();
        sync.push_frame(frame(true, 1, &engine));
        sync.take_remote(&LocalDraft { text: "", base: "" }, &mut []);
        assert_eq!(sync.text().as_deref(), Some("sent prompt"));

        // Another window sends: the engine swaps in a fresh doc, still at epoch 1.
        sync.push_frame(frame(true, 1, &DraftDoc::new()));
        let mut caret = [11usize];
        let text = sync.take_remote(
            &LocalDraft {
                text: "sent prompt",
                base: "",
            },
            &mut caret,
        );
        assert_eq!(text.as_deref(), Some(""));
        assert_eq!(caret, [0]);

        // The next keystroke carries only new history.
        sync.local_edit("n");
        let Some(Outgoing::Edit { update, .. }) = sync.next_outgoing() else {
            panic!("the keystroke is pushed");
        };
        let receiver = DraftDoc::new();
        receiver.import(&update).unwrap();
        assert_eq!(
            receiver.text(),
            "n",
            "the sent draft's history is not re-exported"
        );
    }

    #[test]
    fn clearing_ignores_stale_updates_until_the_reset() {
        let engine = DraftDoc::new();
        engine.set_text("abc").unwrap();
        let mut sync = sync();
        sync.push_frame(frame(true, 1, &engine));
        sync.take_remote(&LocalDraft { text: "", base: "" }, &mut []);

        sync.clear_local();
        assert_eq!(sync.text().as_deref(), Some(""));
        let before = engine.version();
        engine.set_text("abcd").unwrap();
        sync.push_frame(Incoming {
            epoch: 1,
            reset: false,
            bytes: engine.export_since(&before).unwrap(),
        });
        assert!(
            !sync.has_incoming(),
            "stale echo of the sent draft is dropped"
        );
        assert!(matches!(sync.next_outgoing(), Some(Outgoing::Clear)));
        sync.push_frame(frame(true, 2, &DraftDoc::new()));
        assert!(sync.has_incoming());
    }

    #[test]
    fn drift_between_box_and_replica_keeps_what_the_user_sees() {
        let engine = DraftDoc::new();
        engine.set_text("abc").unwrap();
        let mut sync = sync();
        sync.push_frame(frame(true, 1, &engine));
        sync.take_remote(&LocalDraft { text: "", base: "" }, &mut []);
        let before = engine.version();
        engine.set_text("abc def").unwrap();
        sync.push_frame(Incoming {
            epoch: 1,
            reset: false,
            bytes: engine.export_since(&before).unwrap(),
        });
        // The box holds text the replica never heard about.
        let text = sync.take_remote(
            &LocalDraft {
                text: "abc typed",
                base: "",
            },
            &mut [],
        );
        let text = text.expect("merged");
        assert!(text.contains("typed") && text.contains("def"), "{text}");
    }
}
