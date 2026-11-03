//! A typed fork's transcript, read off disk while the CLI holds it back.
//!
//! The CLI replays a typed fork's transcript only once the fork is over. Where
//! the transcript lives is documented: "`agent_transcript_path` is the
//! subagent's own transcript stored in a nested `subagents/` folder"
//! (https://code.claude.com/docs/en/hooks.md#subagentstop-input), here
//! `<project>/<session>/subagents/agent-<task>.jsonl`. Live-verified 2.1.310:
//! the file appears with the fork's start and grows as the fork works, while
//! stdout is silent. The normalizer shows each record once
//! (`Normalizer::fork_record`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use crate::jsonl_tail::TailReader;

/// How often the transcript is read.
const POLL: Duration = Duration::from_millis(250);
/// How long the file may take to appear. It never does under
/// `CLAUDE_CODE_SKIP_PROMPT_HISTORY`, `--no-session-persistence` or a
/// wrapper's own config dir: the tab then fills from the replay.
const LOCATE_FOR: Duration = Duration::from_secs(10);

pub(super) struct ForkTail {
    /// The fork's chip.
    pub chip: String,
    /// `<config root>/projects`.
    projects: PathBuf,
    /// `<session>/subagents/agent-<task>.jsonl`, under one of them.
    file: PathBuf,
    /// The transcript, once found.
    reader: Option<TailReader>,
    give_up: Instant,
    next: Instant,
}

impl ForkTail {
    pub fn new(claude_root: &Path, (chip, session, task): (&str, &str, &str)) -> Self {
        let now = Instant::now();
        Self {
            chip: chip.to_owned(),
            projects: claude_root.join("projects"),
            file: Path::new(session)
                .join("subagents")
                .join(format!("agent-{task}.jsonl")),
            reader: None,
            give_up: now + LOCATE_FOR,
            next: now,
        }
    }

    /// When to read next; `None` once the file failed to appear in time.
    pub fn next_read(&self) -> Option<Instant> {
        (self.reader.is_some() || self.next <= self.give_up).then_some(self.next)
    }

    /// The records written since the last read.
    pub fn read(&mut self) -> Vec<Value> {
        self.next = Instant::now() + POLL;
        if self.reader.is_none() {
            self.reader = self.locate().map(TailReader::new);
            if self.reader.is_none() && self.next > self.give_up {
                tracing::info!(
                    target: "zeron_harness::claude",
                    chip = %self.chip,
                    "no transcript on disk for the typed fork: its tab fills when it ends"
                );
            }
        }
        self.reader
            .as_mut()
            .map_or_else(Vec::new, TailReader::read_new)
    }

    /// The fork's file, looked for ONE level under `projects/` as Grok's
    /// driver looks for a subagent's history: scanning dodges the CLI's cwd
    /// encoding (long paths are hashed, `CLAUDE_CODE_PROJECT_DIR_NAME` names
    /// the directory outright), and no other fork's file has this name.
    fn locate(&self) -> Option<PathBuf> {
        std::fs::read_dir(&self.projects)
            .ok()?
            .flatten()
            .map(|project| project.path().join(&self.file))
            .find(|path| path.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_forks_transcript_is_found_under_any_project_until_the_deadline() {
        let root = tempfile::tempdir().unwrap();
        let mut tail = ForkTail::new(root.path(), ("chip", "s-1", "t1"));
        assert!(tail.read().is_empty(), "no history yet");
        // A hashed long path or `CLAUDE_CODE_PROJECT_DIR_NAME`: not what the
        // cwd would name.
        let project = root.path().join("projects/work");
        std::fs::create_dir_all(project.join("s-1/subagents")).unwrap();
        std::fs::write(project.join("s-1/subagents/agent-t1.jsonl"), "{\"n\":1}\n").unwrap();
        tokio::time::advance(POLL).await;
        assert_eq!(tail.next_read(), Some(Instant::now()));
        assert_eq!(tail.read(), [serde_json::json!({"n": 1})]);
        // Found, it is read for as long as the fork runs.
        tokio::time::advance(LOCATE_FOR).await;
        assert!(tail.next_read().is_some());

        // Another task's transcript never appears: given up at the deadline.
        let mut other = ForkTail::new(root.path(), ("chip", "s-1", "t2"));
        let start = Instant::now();
        for _ in 0..100 {
            let Some(at) = other.next_read() else { break };
            tokio::time::advance(at - Instant::now()).await;
            assert!(other.read().is_empty());
        }
        assert_eq!(other.next_read(), None, "still looking");
        let looked = Instant::now() - start;
        assert!(
            looked > LOCATE_FOR - POLL && looked <= LOCATE_FOR,
            "{looked:?}"
        );
    }
}
