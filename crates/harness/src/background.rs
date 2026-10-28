//! A runtime's background processes, shared between its driver and the host
//! through [`crate::TurnControl`].
//!
//! The driver owns the list: it publishes the runtime's authoritative set
//! (Claude's `background_tasks_changed` shells, Codex's commands that outlived
//! their turn) and records where each one's output can be read. The host
//! watches the list, reads output on request, and asks for a process to be
//! stopped; the driver turns that into the agent's own stop request, and the
//! process leaves the list when the agent reports it gone.

use std::collections::{HashMap, VecDeque};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;

use tokio::sync::{Notify, watch};
use zeron_proto::{BackgroundTask, BackgroundTaskOutput};

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

pub(crate) struct BackgroundWork {
    tasks: watch::Sender<Vec<BackgroundTask>>,
    outputs: Mutex<HashMap<String, OutputSource>>,
    stops: Mutex<VecDeque<String>>,
    stop_requested: Notify,
}

impl Default for BackgroundWork {
    fn default() -> Self {
        Self {
            tasks: watch::Sender::new(Vec::new()),
            outputs: Mutex::default(),
            stops: Mutex::default(),
            stop_requested: Notify::new(),
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
    }

    pub fn watch(&self) -> watch::Receiver<Vec<BackgroundTask>> {
        self.tasks.subscribe()
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
