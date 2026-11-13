//! A runtime's background processes, shared between its driver and the host
//! through [`crate::TurnControl`].
//!
//! The driver owns the list: it publishes the runtime's authoritative set
//! (Claude's `background_tasks_changed` shells, Codex's commands that outlived
//! their turn) and records where each one's output can be read. The host
//! watches the list, reads output on request, and asks for a process to be
//! stopped; the driver turns that into the agent's own stop request, and the
//! process leaves the list when the agent reports it gone.
//!
//! It also keeps what the agent WAITS on once its turn has ended: work that
//! will wake it when it ends. That is only true of a runtime that wakes its
//! agent on its own ([`BackgroundWork::set_wakes`]: Claude Code re-invokes
//! the model when a background task finishes, Muse starts a wake turn;
//! Codex leaves a finished background terminal unread, live-verified on
//! 0.167). On such a runtime its background agents always count (they
//! report back), and a background process counts only once the agent says
//! it waits on it ([`BackgroundWork::mark`]): a dev server left running is
//! no wait. A mark lasts as long as its process is listed.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;

use tokio::sync::{Notify, watch};
use zeron_proto::{BackgroundTask, BackgroundTaskOutput, WaitingKind, WaitingOn};

/// Output kept per process the driver buffers itself (Codex output deltas),
/// and read from the end of an output file at most.
pub const OUTPUT_TAIL_BYTES: usize = 16 * 1024;

/// Where a background process's output can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSource {
    /// A file the agent writes the output to (Claude's task output files).
    File(PathBuf),
    /// Output the driver received on the wire, kept as its last
    /// [`OUTPUT_TAIL_BYTES`].
    Buffer { text: String, truncated: bool },
}

/// A mark the runtime can't honour ([`BackgroundWork::mark`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkRefused {
    /// This runtime doesn't wake its agent when background work ends: the
    /// agent must wait for the work inside its turn.
    NoWake,
}

/// Which of the ids a mark named are background processes listed now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkOutcome {
    pub matched: Vec<String>,
    pub unmatched: Vec<String>,
}

#[derive(Default)]
struct Waits {
    /// The runtime wakes its agent when background work ends.
    wakes: bool,
    /// Its background agents now, oldest first.
    agents: Vec<WaitingOn>,
    /// Listed processes the agent said it waits on.
    marked: HashSet<String>,
}

pub(crate) struct BackgroundWork {
    tasks: watch::Sender<Vec<BackgroundTask>>,
    outputs: Mutex<HashMap<String, OutputSource>>,
    stops: Mutex<VecDeque<String>>,
    stop_requested: Notify,
    waits: Mutex<Waits>,
    waiting: watch::Sender<Vec<WaitingOn>>,
    woke: Notify,
}

impl Default for BackgroundWork {
    fn default() -> Self {
        Self {
            tasks: watch::Sender::new(Vec::new()),
            outputs: Mutex::default(),
            stops: Mutex::default(),
            stop_requested: Notify::new(),
            waits: Mutex::default(),
            waiting: watch::Sender::new(Vec::new()),
            woke: Notify::new(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl BackgroundWork {
    /// Driver: the runtime's whole set now. Output of a process that leaves
    /// the list is dropped with it (output of one not yet listed — a
    /// foreground command that may outlive its turn — is kept).
    pub fn set(&self, tasks: Vec<BackgroundTask>) {
        {
            let current = self.tasks.borrow();
            let mut outputs = lock(&self.outputs);
            for gone in current
                .iter()
                .filter(|t| !tasks.iter().any(|n| n.id == t.id))
            {
                outputs.remove(&gone.id);
            }
        }
        self.tasks.send_if_modified(|current| {
            if *current == tasks {
                return false;
            }
            *current = tasks;
            true
        });
        // A mark ends with its process.
        {
            let listed = self.tasks.borrow();
            lock(&self.waits)
                .marked
                .retain(|id| listed.iter().any(|t| &t.id == id));
        }
        self.publish_waiting();
    }

    pub fn watch(&self) -> watch::Receiver<Vec<BackgroundTask>> {
        self.tasks.subscribe()
    }

    /// Driver: whether this runtime wakes its agent when background work
    /// ends. Off (the default), nothing is ever waited on.
    pub fn set_wakes(&self, wakes: bool) {
        lock(&self.waits).wakes = wakes;
        self.publish_waiting();
    }

    /// Driver: the runtime's background agents now, its whole set.
    pub fn set_agents(&self, agents: Vec<WaitingOn>) {
        lock(&self.waits).agents = agents;
        self.publish_waiting();
    }

    /// Host: the agent says it waits on these background processes (or, with
    /// `waiting` false, no longer). Only processes listed now match.
    pub fn mark(&self, ids: &[String], waiting: bool) -> Result<MarkOutcome, MarkRefused> {
        let mut outcome = MarkOutcome::default();
        {
            let listed = self.tasks.borrow();
            let mut waits = lock(&self.waits);
            if !waits.wakes {
                return Err(MarkRefused::NoWake);
            }
            for id in ids {
                let id = id.trim();
                if id.is_empty() || outcome.matched.iter().chain(&outcome.unmatched).any(|seen| seen == id) {
                    continue;
                }
                if listed.iter().any(|t| t.id == id) {
                    if waiting {
                        waits.marked.insert(id.to_owned());
                    } else {
                        waits.marked.remove(id);
                    }
                    outcome.matched.push(id.to_owned());
                } else {
                    outcome.unmatched.push(id.to_owned());
                }
            }
        }
        self.publish_waiting();
        Ok(outcome)
    }

    /// Host: the person says the agent isn't waiting on anything it marked
    /// (Stop waiting). Its background agents still count.
    pub fn clear_marks(&self) {
        lock(&self.waits).marked.clear();
        self.publish_waiting();
    }

    /// Host: what the agent waits on now, as it changes: its background
    /// agents, then the processes it marked, oldest first. Empty on a runtime
    /// that doesn't wake its agent.
    pub fn watch_waiting(&self) -> watch::Receiver<Vec<WaitingOn>> {
        self.waiting.subscribe()
    }

    /// Driver: the runtime started a turn of its own, to deliver background
    /// work that ended (a wake turn).
    pub fn note_wake(&self) {
        self.woke.notify_one();
    }

    /// Host: resolves once per [`Self::note_wake`].
    pub async fn woke(&self) {
        self.woke.notified().await;
    }

    fn publish_waiting(&self) {
        let now: Vec<WaitingOn> = {
            let listed = self.tasks.borrow();
            let waits = lock(&self.waits);
            if !waits.wakes {
                Vec::new()
            } else {
                waits
                    .agents
                    .iter()
                    .cloned()
                    .chain(
                        listed
                            .iter()
                            .filter(|t| waits.marked.contains(&t.id))
                            .map(|t| WaitingOn {
                                id: t.id.clone(),
                                kind: WaitingKind::Process,
                                label: t
                                    .description
                                    .clone()
                                    .filter(|d| !d.trim().is_empty())
                                    .unwrap_or_else(|| t.command.clone()),
                            }),
                    )
                    .collect()
            }
        };
        self.waiting.send_if_modified(|current| {
            if *current == now {
                return false;
            }
            *current = now;
            true
        });
    }

    pub fn set_output(&self, id: &str, source: OutputSource) {
        lock(&self.outputs).insert(id.to_owned(), source);
    }

    /// Driver: `id` ended without ever being listed.
    pub fn clear_output(&self, id: &str) {
        lock(&self.outputs).remove(id);
    }

    /// Driver: output the wire delivered for `id`, kept as its tail.
    pub fn append_output(&self, id: &str, chunk: &str) {
        let mut outputs = lock(&self.outputs);
        let source = outputs
            .entry(id.to_owned())
            .or_insert_with(|| OutputSource::Buffer {
                text: String::new(),
                truncated: false,
            });
        if let OutputSource::Buffer { text, truncated } = source {
            text.push_str(chunk);
            if text.len() > OUTPUT_TAIL_BYTES {
                let cut = ceil_char_boundary(text, text.len() - OUTPUT_TAIL_BYTES);
                text.drain(..cut);
                *truncated = true;
            }
        }
    }

    /// Host: the output of a listed process, or `None` when it is not listed.
    /// Reads a file source from disk: call it off the async runtime.
    pub fn output(&self, id: &str) -> Option<BackgroundTaskOutput> {
        if !self.tasks.borrow().iter().any(|t| t.id == id) {
            return None;
        }
        let source = lock(&self.outputs).get(id).cloned();
        Some(match source {
            None => BackgroundTaskOutput::default(),
            Some(OutputSource::Buffer { text, truncated }) => BackgroundTaskOutput {
                text: plain_text(&text),
                truncated,
            },
            Some(OutputSource::File(path)) => {
                let (text, truncated) = read_tail(&path, OUTPUT_TAIL_BYTES).unwrap_or_default();
                BackgroundTaskOutput {
                    text: plain_text(&text),
                    truncated,
                }
            }
        })
    }

    /// Host: ask the driver to stop `id`. False when it is not listed.
    pub fn request_stop(&self, id: &str) -> bool {
        if !self.tasks.borrow().iter().any(|t| t.id == id) {
            return false;
        }
        lock(&self.stops).push_back(id.to_owned());
        self.stop_requested.notify_one();
        true
    }

    /// Driver: the next process the host asked to stop.
    pub async fn stop_requested(&self) -> String {
        loop {
            // Registered before the queue is checked, so a request that
            // lands in between still wakes this wait.
            let notified = self.stop_requested.notified();
            if let Some(id) = lock(&self.stops).pop_front() {
                return id;
            }
            notified.await;
        }
    }
}

fn ceil_char_boundary(text: &str, mut at: usize) -> usize {
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// The last `max` bytes of `path`, from the first whole line inside them,
/// and whether anything before was cut.
fn read_tail(path: &std::path::Path, max: usize) -> std::io::Result<(String, bool)> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.take(max as u64).read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0
        && let Some(newline) = text.find('\n')
    {
        text.drain(..=newline);
    }
    Ok((text, start > 0))
}

/// Output as it reads on a terminal, without the terminal: escape sequences
/// dropped, carriage-return rewrites (progress bars) collapsed to what the
/// line finally said, and no trailing blank lines.
pub fn plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut line = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters and intermediates up to a final byte.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ST.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => line.clear(),
            '\n' => {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            '\t' => line.push_str("    "),
            c if c.is_control() => {}
            c => line.push(c),
        }
    }
    out.push_str(&line);
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> BackgroundTask {
        BackgroundTask {
            id: id.into(),
            command: format!("run {id}"),
            description: None,
            started_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn plain_text_drops_escapes_and_collapses_rewrites() {
        assert_eq!(
            plain_text(
                "\u{1b}[32mok\u{1b}[0m\r\nbuild 10%\rbuild 100%\n\u{1b}]0;title\u{7}done\n\n"
            ),
            "ok\nbuild 100%\ndone"
        );
    }

    #[test]
    fn buffered_output_keeps_its_tail() {
        let work = BackgroundWork::default();
        work.set(vec![task("a")]);
        work.append_output("a", &"x".repeat(OUTPUT_TAIL_BYTES));
        work.append_output("a", "\nlast line");
        let output = work.output("a").unwrap();
        assert!(output.truncated);
        assert!(output.text.ends_with("last line"));
        assert!(output.text.len() <= OUTPUT_TAIL_BYTES);
    }

    #[test]
    fn file_output_starts_at_a_whole_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.output");
        let mut body = String::new();
        for i in 0..4000 {
            body.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&path, &body).unwrap();
        let work = BackgroundWork::default();
        work.set(vec![task("a")]);
        work.set_output("a", OutputSource::File(path));
        let output = work.output("a").unwrap();
        assert!(output.truncated);
        assert!(output.text.starts_with("line "), "{}", &output.text[..20]);
        assert!(output.text.ends_with("line 3999"));
    }

    #[test]
    fn only_listed_processes_answer_and_stop() {
        let work = BackgroundWork::default();
        work.set(vec![task("a")]);
        work.append_output("a", "hello");
        assert!(work.output("b").is_none());
        assert!(!work.request_stop("b"));
        assert!(work.request_stop("a"));
        work.set(Vec::new());
        assert!(work.output("a").is_none(), "gone with the process");
    }

    fn agent(id: &str) -> WaitingOn {
        WaitingOn { id: id.into(), kind: WaitingKind::Agent, label: format!("agent {id}") }
    }

    #[test]
    fn nothing_is_waited_on_where_the_runtime_never_wakes_its_agent() {
        let work = BackgroundWork::default();
        work.set(vec![task("a")]);
        work.set_agents(vec![agent("x")]);
        assert_eq!(work.mark(&["a".into()], true), Err(MarkRefused::NoWake));
        assert!(work.watch_waiting().borrow().is_empty());
    }

    #[test]
    fn agents_always_count_and_processes_only_once_marked() {
        let work = BackgroundWork::default();
        work.set_wakes(true);
        work.set(vec![task("dev"), task("tests")]);
        let waiting = work.watch_waiting();
        assert!(waiting.borrow().is_empty(), "a process left running is no wait");
        work.set_agents(vec![agent("x")]);
        let outcome = work.mark(&["tests".into(), "gone".into(), "tests".into()], true).unwrap();
        assert_eq!(outcome, MarkOutcome { matched: vec!["tests".into()], unmatched: vec!["gone".into()] });
        let ids: Vec<String> = waiting.borrow().iter().map(|w| w.id.clone()).collect();
        assert_eq!(ids, ["x", "tests"]);
        assert_eq!(waiting.borrow()[1].label, "run tests");
        // The process ending takes its mark with it: listed again later
        // (a reused id), it isn't waited on unless marked again.
        work.set(vec![task("dev")]);
        work.set(vec![task("dev"), task("tests")]);
        let ids: Vec<String> = waiting.borrow().iter().map(|w| w.id.clone()).collect();
        assert_eq!(ids, ["x"]);
        // Unmarked, and the person's Stop waiting: agents still count.
        work.mark(&["dev".into()], true).unwrap();
        work.mark(&["dev".into()], false).unwrap();
        work.mark(&["tests".into()], true).unwrap();
        work.clear_marks();
        let ids: Vec<String> = waiting.borrow().iter().map(|w| w.id.clone()).collect();
        assert_eq!(ids, ["x"]);
        work.set_agents(Vec::new());
        assert!(waiting.borrow().is_empty());
    }

    #[tokio::test]
    async fn a_wake_reaches_the_host() {
        let work = std::sync::Arc::new(BackgroundWork::default());
        let host = work.clone();
        let waiter = tokio::spawn(async move { host.woke().await });
        tokio::task::yield_now().await;
        work.note_wake();
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn a_stop_request_reaches_the_driver() {
        let work = std::sync::Arc::new(BackgroundWork::default());
        work.set(vec![task("a")]);
        let driver = work.clone();
        let waiter = tokio::spawn(async move { driver.stop_requested().await });
        tokio::task::yield_now().await;
        assert!(work.request_stop("a"));
        assert_eq!(waiter.await.unwrap(), "a");
    }
}
