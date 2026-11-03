//! A plugin's programs ("commands"): the ones its manifest names
//! (`"commands": ["gh"]`), run as you on this Mac when it asks
//! (`zeron.run(program, args, { cwd, stdin, timeout })`). No shell: each
//! argument reaches the program as it is, so nothing in one is expanded or
//! split. The program is found on your login shell's PATH (an app opened
//! from the Dock has only the system's), each run waits at most its
//! timeout, and what it prints is kept up to a cap.
//!
//!   { program, args = { … }, cwd?, stdin?, timeout?, env? }
//!     → { status, stdout, stderr }   (status -1: stopped at its timeout)

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

use serde_json::{Value as Json, json};

/// What a run prints, kept at most (each of stdout and stderr).
const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
/// A run's longest wait unless it asks less: and the most it may ask.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_TIMEOUT: Duration = Duration::from_secs(300);

/// A program's name as a manifest gives it (the engine's `valid_command`):
/// its name alone, never a path.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// Your login shell's PATH, read once (then the system's dirs, and the
/// usual places programs are installed, after it).
fn user_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut dirs: Vec<String> = Vec::new();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        if let Ok(out) = Command::new(&shell).args(["-l", "-c", "printf %s \"$PATH\""]).stdin(Stdio::null()).stderr(Stdio::null()).output()
            && out.status.success()
        {
            dirs.extend(String::from_utf8_lossy(&out.stdout).split(':').filter(|d| !d.is_empty()).map(str::to_string));
        }
        for extra in std::env::var("PATH").unwrap_or_default().split(':').map(str::to_string).chain(
            ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"].into_iter().map(str::to_string),
        ) {
            if !extra.is_empty() && !dirs.contains(&extra) {
                dirs.push(extra);
            }
        }
        dirs.join(":")
    })
}

/// Where `name` is on `path`: the first executable file of that name.
fn find(name: &str, path: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    path.split(':').map(|dir| Path::new(dir).join(name)).find(|p| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// A run's own environment variables (`env = { LC_ALL = "C" }`), set over
/// yours: a name of capitals, digits and `_`, and text. Never one that
/// changes which program runs or what loads into it (`PATH`, `DYLD_…`,
/// `LD_…`): the manifest's `commands` say what runs. (The engine's
/// service runner says the same: crates/engine/src/plugins/service/commands.rs.)
fn env_of(asked: &Json) -> Result<Vec<(String, String)>, String> {
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

/// Runs what `asked` says, if `allowed` names its program: the answer, or
/// why it wasn't run. Blocks: call it off the window's thread.
pub fn run(asked: &Json, allowed: &[String]) -> Result<Json, String> {
    run_on(asked, allowed, user_path())
}

fn run_on(asked: &Json, allowed: &[String], path: &str) -> Result<Json, String> {
    let program = asked["program"].as_str().unwrap_or_default();
    if !valid_name(program) || !allowed.iter().any(|a| a == program) {
        return Err(format!("This plugin doesn't run \"{program}\": its manifest's \"commands\" names what it may run"));
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
    let found = find(program, path).ok_or_else(|| format!("\"{program}\" isn't installed on this Mac (or isn't on your PATH)"))?;
    let mut command = Command::new(found);
    command.args(&args).env("PATH", path).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in env_of(asked)? {
        command.env(key, value);
    }
    if let Some(cwd) = asked["cwd"].as_str().filter(|c| !c.is_empty()) {
        let dir = Path::new(cwd);
        if !dir.is_absolute() || !dir.is_dir() {
            return Err(format!("There's no folder {cwd} on this Mac"));
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
    let mut child = command.spawn().map_err(|e| format!("\"{program}\" didn't start: {e}"))?;
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

#[cfg(test)]
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
        assert!(run_on(&json!({ "program": "ls", "args": [] }), &allowed(&["echo"]), PATH).unwrap_err().contains("doesn't run"));
        for bad in ["/bin/echo", "../echo", "echo;ls", ""] {
            assert!(run_on(&json!({ "program": bad }), &allowed(&[bad]), PATH).is_err(), "{bad}");
        }
    }

    #[test]
    fn it_takes_stdin_a_folder_and_says_how_it_ended() {
        let ran = run_on(&json!({ "program": "cat", "args": [], "stdin": "piped in" }), &allowed(&["cat"]), PATH).unwrap();
        assert_eq!(ran["stdout"], "piped in");
        let ran = run_on(&json!({ "program": "pwd", "args": [], "cwd": "/tmp" }), &allowed(&["pwd"]), PATH).unwrap();
        assert!(ran["stdout"].as_str().unwrap().trim_end().ends_with("tmp"), "{ran}");
        let ran = run_on(&json!({ "program": "ls", "args": ["/no/such/place"] }), &allowed(&["ls"]), PATH).unwrap();
        assert!(ran["status"].as_i64().unwrap() != 0 && !ran["stderr"].as_str().unwrap().is_empty(), "{ran}");
        assert!(run_on(&json!({ "program": "pwd", "cwd": "relative" }), &allowed(&["pwd"]), PATH).is_err());
        assert!(run_on(&json!({ "program": "no-such-program-zgx" }), &allowed(&["no-such-program-zgx"]), PATH).unwrap_err().contains("isn't installed"));
    }

    #[test]
    fn it_sets_a_runs_own_environment_but_never_what_runs() {
        let ran = run_on(&json!({ "program": "printenv", "args": ["ZERON_TEST_WORD"], "env": { "ZERON_TEST_WORD": "hi" } }), &allowed(&["printenv"]), PATH).unwrap();
        assert_eq!(ran["stdout"], "hi\n");
        for bad in [json!({ "PATH": "/tmp" }), json!({ "DYLD_INSERT_LIBRARIES": "/x" }), json!({ "LD_PRELOAD": "/x" }), json!({ "lower": "x" }), json!({ "X": 3 })] {
            assert!(run_on(&json!({ "program": "printenv", "env": bad }), &allowed(&["printenv"]), PATH).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_run_past_its_timeout_is_stopped() {
        let ran = run_on(&json!({ "program": "sleep", "args": ["5"], "timeout": 0.2 }), &allowed(&["sleep"]), PATH).unwrap();
        assert_eq!(ran["status"].as_i64(), Some(-1));
    }
}
