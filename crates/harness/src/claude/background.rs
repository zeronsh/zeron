//! Claude's background shells, from the frames that describe them.
//!
//! `background_tasks_changed` is the authority: every task the session holds
//! now, agents and shells alike (live-verified 2.1.312). Shells are
//! `task_type: "local_bash"` — `Bash` with `run_in_background`, a foreground
//! command moved to the background, and `Monitor` watches. The roster names a
//! task and its description only, so the rest is joined in from the frames
//! around it:
//!
//! - `task_started` pairs the task id with the `tool_use_id` that spawned it;
//! - that tool_use's `input.command` is what it runs;
//! - the tool_result says where its output goes ("Output is being written
//!   to: <path>"), a file in the session's `tasks/` folder named after the
//!   task id — so a task whose result never said it (one backgrounded later)
//!   is found in the same folder.
//!
//! Background agents are subagents: their chips show them, not this list.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::Value;
use zeron_proto::BackgroundTask;

use super::wire::{Frame, MessageFrame, SystemFrame};

/// Tool calls remembered for their command until a task claims them. A
/// session runs many foreground commands that never become tasks.
const MAX_COMMANDS: usize = 256;

#[derive(Default)]
pub(super) struct BackgroundShells {
    /// Shell-capable tool_use id → its command, oldest first.
    commands: Vec<(String, String)>,
    /// Task id → the tool_use that started it.
    spawned_by: HashMap<String, String>,
    /// Tool_use id → the output file its result named.
    output_files: HashMap<String, PathBuf>,
    /// The session's task-output folder, once any result named a file in it.
    tasks_dir: Option<PathBuf>,
    /// The roster's shells, in the order they appeared.
    listed: Vec<Listed>,
}

struct Listed {
    id: String,
    description: Option<String>,
    since: DateTime<Utc>,
}

impl BackgroundShells {
    /// Take one frame in. True when the listed shells changed.
    pub fn observe(&mut self, frame: &Frame) -> bool {
        match frame {
            Frame::Assistant(message) => {
                self.remember_commands(message);
                false
            }
            Frame::User(message) => {
                self.remember_output_files(message);
                false
            }
            Frame::System(system) => self.system(system),
            _ => false,
        }
    }

    fn remember_commands(&mut self, message: &MessageFrame) {
        for block in message.message.blocks() {
            if block.kind != "tool_use" || !matches!(block.name.as_str(), "Bash" | "Monitor") {
                continue;
            }
            let Some(command) = block.input.get("command").and_then(Value::as_str) else {
                continue;
            };
            if self.commands.len() >= MAX_COMMANDS {
                self.commands.remove(0);
            }
            self.commands
                .push((block.id.clone(), command.trim().to_owned()));
        }
    }

    fn remember_output_files(&mut self, message: &MessageFrame) {
        let Some(blocks) = message.message.content.as_array() else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            if !self.commands.iter().any(|(tool, _)| tool == id) {
                continue;
            }
            let Some(path) = result_text(block.get("content")).and_then(|t| output_file(&t)) else {
                continue;
            };
            if let Some(dir) = path.parent() {
                self.tasks_dir = Some(dir.to_owned());
            }
            self.output_files.insert(id.to_owned(), path);
        }
    }

    fn system(&mut self, system: &SystemFrame) -> bool {
        match system.subtype.as_str() {
            "task_started" => {
                if let (Some(task), Some(tool)) = (
                    system.task_id.as_deref().filter(|t| !t.is_empty()),
                    system.tool_use_id.as_deref().filter(|t| !t.is_empty()),
                ) {
                    self.spawned_by.insert(task.to_owned(), tool.to_owned());
                    // The roster usually lists a task before its start
                    // arrives (live 2.1.312): only now is its command known.
                    return self.listed.iter().any(|l| l.id == task);
                }
                false
            }
            "background_tasks_changed" => {
                let now = Utc::now();
                let roster: Vec<(String, Option<String>)> = system
                    .tasks
                    .iter()
                    .flatten()
                    .filter(|task| {
                        task.get("task_type").and_then(Value::as_str) == Some("local_bash")
                    })
                    .filter_map(|task| {
                        let id = task.get("task_id").and_then(Value::as_str)?;
                        let description = task
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|d| !d.is_empty())
                            .map(str::to_owned);
                        Some((id.to_owned(), description))
                    })
                    .collect();
                let before: Vec<_> = self
                    .listed
                    .iter()
                    .map(|l| (l.id.clone(), l.description.clone()))
                    .collect();
                if before == roster {
                    return false;
                }
                let mut previous: HashMap<String, DateTime<Utc>> =
                    self.listed.drain(..).map(|l| (l.id, l.since)).collect();
                self.listed = roster
                    .into_iter()
                    .map(|(id, description)| Listed {
                        since: previous.remove(&id).unwrap_or(now),
                        id,
                        description,
                    })
                    .collect();
                true
            }
            // A task's end is final: forget what joined it.
            "task_notification" => {
                if let Some(tool) = system
                    .task_id
                    .as_deref()
                    .and_then(|task| self.spawned_by.remove(task))
                {
                    self.commands.retain(|(id, _)| *id != tool);
                    self.output_files.remove(&tool);
                }
                false
            }
            _ => false,
        }
    }

    /// The listed shells, oldest first.
    pub fn tasks(&self) -> Vec<BackgroundTask> {
        self.listed
            .iter()
            .map(|listed| {
                let command = self
                    .spawned_by
                    .get(&listed.id)
                    .and_then(|tool| self.commands.iter().find(|(id, _)| id == tool))
                    .map(|(_, command)| command.clone())
                    .filter(|c| !c.is_empty());
                BackgroundTask {
                    id: listed.id.clone(),
                    command: command
                        .clone()
                        .or_else(|| listed.description.clone())
                        .unwrap_or_else(|| "Background command".to_owned()),
                    // Only worth a line of its own beside the command.
                    description: listed.description.clone().filter(|_| command.is_some()),
                    started_at: listed.since,
                }
            })
            .collect()
    }

    /// Where each listed shell's output can be read, when known.
    pub fn output_files(&self) -> Vec<(String, PathBuf)> {
        self.listed
            .iter()
            .filter_map(|listed| {
                let named = self
                    .spawned_by
                    .get(&listed.id)
                    .and_then(|tool| self.output_files.get(tool))
                    .cloned();
                let path = named.or_else(|| {
                    self.tasks_dir
                        .as_ref()
                        .map(|dir| dir.join(format!("{}.output", listed.id)))
                })?;
                Some((listed.id.clone(), path))
            })
            .collect()
    }
}

/// A tool_result's text, whether a plain string or text blocks.
fn result_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => Some(
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

/// The output file a background command's result names.
fn output_file(text: &str) -> Option<PathBuf> {
    const MARKER: &str = "Output is being written to: ";
    let rest = &text[text.find(MARKER)? + MARKER.len()..];
    let path = rest.lines().next()?;
    // The sentence ends after the path; a path itself ends `.output`.
    let path = match path.find(".output") {
        Some(end) => &path[..end + ".output".len()],
        None => path.split_whitespace().next()?.trim_end_matches('.'),
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::wire::parse_frame;

    /// Frames from a live 2.1.312 session: a background loop, a foreground
    /// command alongside it, then TaskStop.
    const LIVE: &[&str] = &[
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_bg","name":"Bash","input":{"command":"for i in 1 2 3 4 5 6; do echo tick $i; sleep 2; done","description":"Run six-tick loop in background","run_in_background":true}}]}}"#,
        r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bn8lafmo7","run_id":"0mvryljj2-ee0442c8","task_type":"local_bash","description":"Run six-tick loop in background"}]}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"bn8lafmo7","tool_use_id":"toolu_bg","description":"Run six-tick loop in background","is_backgrounded":true,"task_type":"local_bash"}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_bg","type":"tool_result","content":"Command running in background with ID: bn8lafmo7. Output is being written to: /tmp/claude-1000/-tmp-bgprobe/6c99/tasks/bn8lafmo7.output. You will be notified when it completes. To check interim output, use Read on that file path.","is_error":false}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_fg","name":"Bash","input":{"command":"sleep 4","description":"Sleep four seconds in foreground"}}]}}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"blsjn92tg","tool_use_id":"toolu_fg","description":"Sleep four seconds in foreground","is_backgrounded":false,"task_type":"local_bash"}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"blsjn92tg","tool_use_id":"toolu_fg","status":"completed","output_file":"","summary":"Sleep four seconds in foreground"}"#,
    ];

    fn feed(shells: &mut BackgroundShells, frames: &[&str]) -> bool {
        let mut changed = false;
        for raw in frames {
            changed |= shells.observe(&parse_frame(raw).expect("parses"));
        }
        changed
    }

    #[test]
    fn a_background_shell_is_listed_with_its_command_and_output_file() {
        let mut shells = BackgroundShells::default();
        assert!(feed(&mut shells, LIVE));
        let tasks = shells.tasks();
        assert_eq!(tasks.len(), 1, "the foreground command is not listed");
        assert_eq!(tasks[0].id, "bn8lafmo7");
        assert_eq!(
            tasks[0].command,
            "for i in 1 2 3 4 5 6; do echo tick $i; sleep 2; done"
        );
        assert_eq!(
            tasks[0].description.as_deref(),
            Some("Run six-tick loop in background")
        );
        assert_eq!(
            shells.output_files(),
            [(
                "bn8lafmo7".to_owned(),
                PathBuf::from("/tmp/claude-1000/-tmp-bgprobe/6c99/tasks/bn8lafmo7.output")
            )]
        );

        // Stopped: the roster empties, and so does the list.
        assert!(feed(
            &mut shells,
            &[r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#]
        ));
        assert!(shells.tasks().is_empty());
    }

    #[test]
    fn a_start_after_the_roster_republishes_with_the_command() {
        let mut shells = BackgroundShells::default();
        assert!(feed(&mut shells, &LIVE[..2]));
        assert_eq!(shells.tasks()[0].command, "Run six-tick loop in background");
        assert!(
            feed(&mut shells, &LIVE[2..3]),
            "the start names its command"
        );
        assert!(shells.tasks()[0].command.starts_with("for i in"));
    }

    #[test]
    fn agents_are_not_listed_and_an_unchanged_roster_is_no_change() {
        let mut shells = BackgroundShells::default();
        let agent = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a1","task_type":"local_agent","description":"scout"}]}"#;
        assert!(!feed(&mut shells, &[agent]));
        assert!(feed(&mut shells, &LIVE[..2]));
        let since = shells.tasks()[0].started_at;
        assert!(!feed(&mut shells, &LIVE[1..2]));
        assert_eq!(shells.tasks()[0].started_at, since, "first sight is kept");
    }

    #[test]
    fn a_shell_backgrounded_later_finds_its_output_in_the_tasks_folder() {
        let mut shells = BackgroundShells::default();
        feed(&mut shells, LIVE);
        feed(
            &mut shells,
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_dev","name":"Bash","input":{"command":"npm run dev"}}]}}"#,
                r#"{"type":"system","subtype":"task_started","task_id":"bdev","tool_use_id":"toolu_dev","is_backgrounded":false,"task_type":"local_bash"}"#,
                r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bn8lafmo7","task_type":"local_bash","description":"Run six-tick loop in background"},{"task_id":"bdev","task_type":"local_bash"}]}"#,
            ],
        );
        let tasks = shells.tasks();
        assert_eq!(tasks[1].command, "npm run dev");
        assert_eq!(tasks[1].description, None);
        assert_eq!(
            shells.output_files()[1].1,
            PathBuf::from("/tmp/claude-1000/-tmp-bgprobe/6c99/tasks/bdev.output")
        );
    }

    #[test]
    fn an_unknown_command_reads_as_its_description() {
        let mut shells = BackgroundShells::default();
        feed(
            &mut shells,
            &[
                r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash","description":"Watch the build"}]}"#,
            ],
        );
        let tasks = shells.tasks();
        assert_eq!(tasks[0].command, "Watch the build");
        assert_eq!(tasks[0].description, None);
    }
}
