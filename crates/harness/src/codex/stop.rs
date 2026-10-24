//! Ending one command the agent started, or one subagent's work.
//!
//! The app-server has no request that kills a command it ran itself
//! (`command/exec/terminate` covers only a `command/exec` the client made),
//! so the command's process is found among the app-server's descendants and
//! signaled: SIGTERM to its process group, SIGKILL after a grace — what Ctrl+C
//! on that one process would do. The item then completes failed (exit 143)
//! and the turn goes on (live-verified, codex-cli 0.164.0). A match that is
//! not exactly one process is refused: never a guess.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use super::normalize::{item_type, notification_thread_id, turn_id};

/// How long a command has to exit on SIGTERM before it is killed.
const TERM_GRACE: Duration = Duration::from_secs(2);

/// What the app-server is running right now, from its notifications: the
/// commands (any thread's) that started and have not completed, and each
/// thread's current turn — what stopping a subagent needs to name.
#[derive(Default)]
pub(super) struct Running {
    /// Item id → (thread, the command line the item reported).
    commands: HashMap<String, (String, String)>,
    /// Thread → the turn it has begun and not ended.
    turns: HashMap<String, String>,
}

impl Running {
    pub(super) fn observe(&mut self, method: &str, params: &Value) {
        let Some(thread) = notification_thread_id(method, params).filter(|t| !t.is_empty()) else {
            return;
        };
        match method {
            "item/started" | "item/completed" => {
                let item = params.get("item").unwrap_or(&Value::Null);
                if !matches!(item_type(item), "commandExecution" | "command_execution") {
                    return;
                }
                let Some(id) = item.get("id").and_then(Value::as_str) else {
                    return;
                };
                if method == "item/started" {
                    let command = item.get("command").and_then(Value::as_str);
                    self.commands.insert(
                        id.to_owned(),
                        (thread, command.unwrap_or_default().to_owned()),
                    );
                } else {
                    self.commands.remove(id);
                }
            }
            "turn/started" => {
                let id = turn_id(params);
                if !id.is_empty() {
                    self.turns.insert(thread, id);
                }
            }
            "turn/completed" | "turn/failed" | "turn/aborted" => {
                // A late end of an earlier turn must not clear the current one.
                let id = turn_id(params);
                if self
                    .turns
                    .get(&thread)
                    .is_some_and(|t| id.is_empty() || *t == id)
                {
                    self.turns.remove(&thread);
                }
            }
            _ => {}
        }
    }

    /// The command line of the running command item `id`.
    pub(super) fn command(&self, id: &str) -> Option<&str> {
        self.commands.get(id).map(|(_, command)| command.as_str())
    }

    /// The turn `thread` is in the middle of.
    pub(super) fn turn(&self, thread: &str) -> Option<&str> {
        self.turns.get(thread).map(String::as_str)
    }

    /// The running commands `thread` started, as command lines.
    pub(super) fn commands_of(&self, thread: &str) -> Vec<String> {
        self.commands
            .values()
            .filter(|(owner, _)| owner == thread)
            .map(|(_, command)| command.clone())
            .collect()
    }
}

/// Split a command line the way a POSIX shell would (quotes and backslash
/// escapes, no expansion). `None` for an unterminated quote.
pub(super) fn shell_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            // Inside double quotes a backslash only escapes these.
                            c @ ('"' | '\\' | '$' | '`') => word.push(c),
                            '\n' => {}
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                started = true;
                match chars.next()? {
                    '\n' => {}
                    c => word.push(c),
                }
            }
            c => {
                started = true;
                word.push(c);
            }
        }
    }
    if started {
        words.push(word);
    }
    Some(words)
}

/// The argument vectors a command item's process can have.
///
/// Codex reports `<shell> -lc '<script>'` and runs `<shell> -c <script>`. A
/// shell given one simple command execs it in place, leaving the command's
/// own arguments; a command that needed no shell runs as the words it names.
pub(super) struct CommandForms {
    shell: Option<(String, String)>,
    script_words: Vec<String>,
    command_words: Vec<String>,
}

impl CommandForms {
    pub(super) fn parse(command: &str) -> Option<Self> {
        let command_words = shell_words(command).filter(|words| !words.is_empty())?;
        let shell = match command_words.as_slice() {
            [shell, flag, script] if matches!(flag.as_str(), "-c" | "-lc") => {
                Some((shell.clone(), script.clone()))
            }
            _ => None,
        };
        let script_words = shell
            .as_ref()
            .and_then(|(_, script)| shell_words(script))
            .unwrap_or_default();
        Some(Self {
            shell,
            script_words,
            command_words,
        })
    }

    /// How closely `argv` is this command's process, best first: 0 the shell
    /// running the script, 1 the script's lone command after the shell
    /// exec'd it, 2 the command run directly.
    fn tier(&self, argv: &[String]) -> Option<u8> {
        if let Some((shell, script)) = &self.shell
            && let [program, flag, arg] = argv
            && matches!(flag.as_str(), "-c" | "-lc")
            && arg == script
            && base_name(program) == base_name(shell)
        {
            return Some(0);
        }
        if self.shell.is_some() && !self.script_words.is_empty() && argv == self.script_words {
            return Some(1);
        }
        (argv == self.command_words).then_some(2)
    }

    /// The one process that is this command, among `processes`. Several
    /// candidates at the best tier that has any, or none at all, is an error.
    pub(super) fn pick<T: Copy>(&self, processes: &[(T, Vec<String>)]) -> Result<T, String> {
        let tiered: Vec<(u8, T)> = processes
            .iter()
            .filter_map(|(process, argv)| Some((self.tier(argv)?, *process)))
            .collect();
        let Some(best) = tiered.iter().map(|(tier, _)| *tier).min() else {
            return Err("that command's process was not found".into());
        };
        let mut found = tiered.iter().filter(|(tier, _)| *tier == best);
        match (found.next(), found.next()) {
            (Some((_, process)), None) => Ok(*process),
            _ => Err("several running commands look the same; not guessing".into()),
        }
    }
}

fn base_name(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// End the process running `command` under the app-server `app_server`.
#[cfg(unix)]
pub(super) async fn kill_command(app_server: u32, command: &str) -> Result<(), String> {
    let forms = CommandForms::parse(command).ok_or("the command line could not be read")?;
    let processes = crate::process::descendants_with_argv(app_server).await;
    let target = forms.pick(
        &processes
            .iter()
            .map(|(t, a)| (*t, a.clone()))
            .collect::<Vec<_>>(),
    )?;
    crate::process::terminate_tree(&[target], TERM_GRACE).await;
    Ok(())
}

#[cfg(not(unix))]
pub(super) async fn kill_command(_app_server: u32, _command: &str) -> Result<(), String> {
    Err("stopping a command is not supported on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn shell_words_follow_posix_quoting() {
        assert_eq!(
            shell_words(
                r#"/usr/bin/bash -lc 'python3 -c "import time; time.sleep(60)"; echo done'"#
            ),
            Some(argv(&[
                "/usr/bin/bash",
                "-lc",
                r#"python3 -c "import time; time.sleep(60)"; echo done"#
            ]))
        );
        // The `'\''` idiom for a quote inside single quotes.
        assert_eq!(
            shell_words(r"sh -c 'echo '\''hi'\'' there'"),
            Some(argv(&["sh", "-c", "echo 'hi' there"]))
        );
        assert_eq!(
            shell_words(r#"echo "a \"b\" $HOME \n" c\ d"#),
            Some(argv(&["echo", r#"a "b" $HOME \n"#, "c d"]))
        );
        assert_eq!(shell_words("a '' b"), Some(argv(&["a", "", "b"])));
        assert_eq!(shell_words("  "), Some(Vec::new()));
        assert_eq!(shell_words("echo 'open"), None);
    }

    #[test]
    fn the_shell_form_of_the_item_command_names_the_process() {
        let command = r#"/usr/bin/bash -lc 'python3 -c "import time; time.sleep(60)"; echo done'"#;
        let forms = CommandForms::parse(command).unwrap();
        let process = argv(&[
            "/usr/bin/bash",
            "-c",
            r#"python3 -c "import time; time.sleep(60)"; echo done"#,
        ]);
        assert_eq!(forms.tier(&process), Some(0));
        // The shell may be spelled by name; the script must match exactly.
        let mut by_name = process.clone();
        by_name[0] = "bash".into();
        assert_eq!(forms.tier(&by_name), Some(0));
        let mut other_script = process.clone();
        other_script[2].push(' ');
        assert_eq!(forms.tier(&other_script), None);
        let mut other_shell = process;
        other_shell[0] = "/bin/zsh".into();
        assert_eq!(forms.tier(&other_shell), None);
        // The command's own child is not the command.
        assert_eq!(
            forms.tier(&argv(&["python3", "-c", "import time; time.sleep(60)"])),
            None
        );
    }

    #[test]
    fn a_lone_command_the_shell_exec_ed_is_found_by_its_own_arguments() {
        let forms = CommandForms::parse("/bin/bash -lc 'sleep 60'").unwrap();
        assert_eq!(forms.tier(&argv(&["sleep", "60"])), Some(1));
        assert_eq!(forms.tier(&argv(&["sleep", "61"])), None);
        // A command that needed no shell at all.
        let direct = CommandForms::parse("sleep 60").unwrap();
        assert_eq!(direct.tier(&argv(&["sleep", "60"])), Some(2));
        assert_eq!(direct.tier(&argv(&["bash", "-c", "sleep 60"])), None);
    }

    #[test]
    fn exactly_one_process_is_ever_chosen() {
        let forms = CommandForms::parse("/bin/bash -lc 'sleep 60; echo done'").unwrap();
        let shell = argv(&["/bin/bash", "-c", "sleep 60; echo done"]);
        let noise = argv(&["sleep", "60"]);
        assert_eq!(
            forms.pick(&[(1, noise.clone()), (2, shell.clone()), (3, argv(&["x"]))]),
            Ok(2)
        );
        // Two of the same command running: refuse rather than pick one.
        assert!(forms.pick(&[(2, shell.clone()), (4, shell)]).is_err());
        assert!(forms.pick(&[(1, noise)]).is_err());
        assert!(forms.pick::<i32>(&[]).is_err());
        // The shell form wins over a same-looking exec'd command.
        let exec = CommandForms::parse("/bin/bash -lc 'sleep 60'").unwrap();
        assert_eq!(
            exec.pick(&[
                (1, argv(&["sleep", "60"])),
                (2, argv(&["/bin/bash", "-c", "sleep 60"]))
            ]),
            Ok(2)
        );
    }

    #[test]
    fn running_work_is_tracked_per_thread() {
        let mut running = Running::default();
        let started = |thread: &str, id: &str, command: &str| json!({"threadId": thread, "item": {"type": "commandExecution", "id": id, "command": command}});
        running.observe("item/started", &started("parent", "c1", "sleep 1"));
        running.observe("item/started", &started("child", "c2", "sleep 2"));
        running.observe(
            "item/started",
            &json!({"threadId": "child", "item": {"type": "agentMessage", "id": "m1"}}),
        );
        assert_eq!(running.command("c1"), Some("sleep 1"));
        assert_eq!(running.commands_of("child"), ["sleep 2"]);
        running.observe("item/completed", &started("parent", "c1", "sleep 1"));
        assert_eq!(running.command("c1"), None);

        running.observe(
            "turn/started",
            &json!({"threadId": "child", "turn": {"id": "t1"}}),
        );
        assert_eq!(running.turn("child"), Some("t1"));
        // A replaced turn's late end leaves the current one.
        running.observe(
            "turn/started",
            &json!({"threadId": "child", "turn": {"id": "t2"}}),
        );
        running.observe(
            "turn/completed",
            &json!({"threadId": "child", "turn": {"id": "t1"}}),
        );
        assert_eq!(running.turn("child"), Some("t2"));
        running.observe(
            "turn/completed",
            &json!({"threadId": "child", "turn": {"id": "t2"}}),
        );
        assert_eq!(running.turn("child"), None);
    }

    /// The real thing: a command in its own process group under a stand-in
    /// app-server, found by its item command and ended.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_running_command_is_ended_through_its_process_group() {
        // The app-server stand-in: starts the command in its own process
        // group, as the real one does, and waits for it.
        let mut server = tokio::process::Command::new("python3");
        server.args([
            "-c",
            "import subprocess; subprocess.Popen(['/bin/bash', '-c', 'sleep 45; echo done'], \
             start_new_session=True).wait()",
        ]);
        let mut server = server.spawn().unwrap();
        let pid = server.id().unwrap();
        // Wait for the shell to be there.
        let command = "/bin/bash -lc 'sleep 45; echo done'";
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let found = crate::process::descendants_with_argv(pid).await;
            if found
                .iter()
                .any(|(_, argv)| argv.last().is_some_and(|a| a == "sleep 45; echo done"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "command never started"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        kill_command(pid, command)
            .await
            .expect("the command is ended");
        // The command died of the signal and its waiting parent returned.
        tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .expect("the stand-in server saw its child die")
            .unwrap();
        assert!(kill_command(pid, command).await.is_err());
    }
}
