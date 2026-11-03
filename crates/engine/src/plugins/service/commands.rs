//! A service's programs (`zeron.run`, "commands"): the ones its manifest
//! names, run as you on this device. The window's runner, engine side
//! (apps/zgx/src/plugins/commands.rs): no shell, so each argument reaches
//! the program as it is; the program is found on your login shell's PATH;
//! each run waits at most its timeout; what it prints is kept up to a cap.
//!
//!   { program, args = { … }, cwd?, stdin?, timeout?, env? }
//!     → { status, stdout, stderr }   (status -1: stopped at its timeout)

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

use crate::plugins::manifest::valid_command;

/// What a run prints, kept at most (each of stdout and stderr).
const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(windows)]
const SEPARATOR: char = ';';
#[cfg(not(windows))]
const SEPARATOR: char = ':';

/// Your login shell's PATH, read once, then the process's and the usual
/// places programs are installed.
fn user_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut dirs: Vec<String> = Vec::new();
        #[cfg(not(windows))]
        {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
            if let Ok(out) = Command::new(&shell)
                .args(["-l", "-c", "printf %s \"$PATH\""])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                && out.status.success()
            {
                dirs.extend(
                    String::from_utf8_lossy(&out.stdout)
                        .split(SEPARATOR)
                        .filter(|d| !d.is_empty())
                        .map(str::to_string),
                );
            }
        }
        let usual: &[&str] = if cfg!(windows) {
            &[]
        } else {
            &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        };
        let process = std::env::var("PATH").unwrap_or_default();
        for extra in process
            .split(SEPARATOR)
            .map(str::to_string)
            .chain(usual.iter().map(|d| d.to_string()))
        {
            if !extra.is_empty() && !dirs.contains(&extra) {
                dirs.push(extra);
            }
        }
        dirs.join(&SEPARATOR.to_string())
    })
}

#[cfg(not(windows))]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn executable(path: &Path) -> bool {
    path.is_file()
}

/// Where `name` is on `path`: the first executable file of that name.
fn find(name: &str, path: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        ["", ".exe", ".cmd", ".bat"].iter().map(|ext| format!("{name}{ext}")).collect()
    } else {
        vec![name.to_owned()]
    };
    path.split(SEPARATOR)
        .flat_map(|dir| names.iter().map(move |n| Path::new(dir).join(n)))
        .find(|p| executable(p))
}

/// A run's own environment variables (`env = { LC_ALL = "C" }`), set over
/// yours: a name of capitals, digits and `_`, and text. Never one that
/// changes which program runs or what loads into it (`PATH`, `DYLD_…`,
/// `LD_…`): the manifest's `commands` say what runs.
pub(crate) fn env_of(asked: &Json) -> Result<Vec<(String, String)>, String> {
    let Some(env) = asked.get("env").filter(|e| !e.is_null()) else {
        return Ok(Vec::new());
    };
    let Some(env) = env.as_object() else {
        return Err("A run's env is a table of names and their text".into());
    };
    let mut out = Vec::with_capacity(env.len());
    for (key, value) in env {
        let named = !key.is_empty()
            && key.len() <= 64
            && !key.as_bytes()[0].is_ascii_digit()
            && key.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !named {
            return Err(format!("\"{key}\" isn't an environment variable's name (capitals, digits and _)"));
        }
        if key == "PATH" || key.starts_with("DYLD_") || key.starts_with("LD_") {
            return Err(format!("A run can't set {key}: it would change what runs"));
        }
        let Some(value) = value.as_str() else {
            return Err(format!("{key}'s value is text"));
        };
        if value.len() > 4096 || value.contains('\0') {
            return Err(format!("{key}'s value is too long, or holds a NUL"));
        }
        out.push((key.clone(), value.to_owned()));
    }
    Ok(out)
}

/// Run what `asked` says, if `allowed` names its program: the answer, or why
/// it wasn't run. Blocks: call it off the async runtime.
pub fn run(asked: &Json, allowed: &[String]) -> Result<Json, String> {
    run_on(asked, allowed, user_path())
}

pub(crate) fn run_on(asked: &Json, allowed: &[String], path: &str) -> Result<Json, String> {
    let program = asked["program"].as_str().unwrap_or_default();
    if !valid_command(program) || !allowed.iter().any(|a| a == program) {
        return Err(format!(
            "This plugin doesn't run \"{program}\": its manifest's \"commands\" names what it may run"
        ));
    }
    let mut args: Vec<String> = Vec::new();
    for arg in asked["args"].as_array().into_iter().flatten() {
        match arg {
            Json::String(s) => args.push(s.clone()),
            Json::Number(n) => args.push(n.to_string()),
            Json::Bool(b) => args.push(b.to_string()),
            _ => return Err("Each of a run's args is text".into()),
        }
    }
    let found = find(program, path)
        .ok_or_else(|| format!("\"{program}\" isn't installed on this device (or isn't on your PATH)"))?;
    let mut command = Command::new(found);
    command
        .args(&args)
        .env("PATH", path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env_of(asked)? {
        command.env(key, value);
    }
    if let Some(cwd) = asked["cwd"].as_str().filter(|c| !c.is_empty()) {
        let dir = Path::new(cwd);
        if !dir.is_absolute() || !dir.is_dir() {
            return Err(format!("There's no folder {cwd} on this device"));
        }
        command.current_dir(dir);
    }
    let stdin = asked["stdin"].as_str().map(str::to_string);
    command.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let timeout = asked["timeout"]
        .as_f64()
        .filter(|s| *s > 0.)
        .map(|s| Duration::from_secs_f64(s).min(MAX_TIMEOUT))
        .unwrap_or(DEFAULT_TIMEOUT);
    let mut child = command
        .spawn()
        .map_err(|e| format!("\"{program}\" didn't start: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // (Written beside the wait: a program reading slowly can't stall it.)
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(MAX_OUTPUT).read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status.code().unwrap_or(-1),
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break -1;
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    Ok(json!({
        "status": status,
        "stdout": out.join().unwrap_or_default(),
        "stderr": err.join().unwrap_or_default(),
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const PATH: &str = "/usr/bin:/bin";

    fn allowed(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn it_runs_only_what_the_manifest_names() {
        let ran = run_on(&json!({ "program": "echo", "args": ["hi", "$HOME", "a b"] }), &allowed(&["echo"]), PATH).unwrap();
        // (No shell: `$HOME` and the space reach it as they are.)
        assert_eq!((ran["status"].as_i64(), ran["stdout"].as_str()), (Some(0), Some("hi $HOME a b\n")));
        assert!(run_on(&json!({ "program": "ls" }), &allowed(&["echo"]), PATH).unwrap_err().contains("doesn't run"));
        for bad in ["/bin/echo", "../echo", "echo;ls", ""] {
            assert!(run_on(&json!({ "program": bad }), &allowed(&[bad]), PATH).is_err(), "{bad}");
        }
    }

    #[test]
    fn it_takes_stdin_a_folder_and_a_timeout() {
        let ran = run_on(&json!({ "program": "cat", "stdin": "piped in" }), &allowed(&["cat"]), PATH).unwrap();
        assert_eq!(ran["stdout"], "piped in");
        let ran = run_on(&json!({ "program": "pwd", "cwd": "/tmp" }), &allowed(&["pwd"]), PATH).unwrap();
        assert!(ran["stdout"].as_str().unwrap().trim_end().ends_with("tmp"), "{ran}");
        assert!(run_on(&json!({ "program": "pwd", "cwd": "relative" }), &allowed(&["pwd"]), PATH).is_err());
        let ran = run_on(&json!({ "program": "sleep", "args": ["5"], "timeout": 0.2 }), &allowed(&["sleep"]), PATH).unwrap();
        assert_eq!(ran["status"].as_i64(), Some(-1));
        assert!(run_on(&json!({ "program": "no-such-program-zeron" }), &allowed(&["no-such-program-zeron"]), PATH)
            .unwrap_err()
            .contains("isn't installed"));
    }

    #[test]
    fn it_sets_a_runs_own_environment_but_never_what_runs() {
        let ran = run_on(
            &json!({ "program": "printenv", "args": ["ZERON_TEST_WORD"], "env": { "ZERON_TEST_WORD": "hello" } }),
            &allowed(&["printenv"]),
            PATH,
        )
        .unwrap();
        assert_eq!(ran["stdout"], "hello\n");
        for bad in [
            json!({ "PATH": "/tmp" }),
            json!({ "DYLD_INSERT_LIBRARIES": "/tmp/x.dylib" }),
            json!({ "LD_PRELOAD": "/tmp/x.so" }),
            json!({ "lower": "x" }),
            json!({ "1X": "x" }),
            json!({ "X": 3 }),
            json!(["X"]),
        ] {
            assert!(
                run_on(&json!({ "program": "printenv", "env": bad }), &allowed(&["printenv"]), PATH).is_err(),
                "{bad}"
            );
        }
    }
}
