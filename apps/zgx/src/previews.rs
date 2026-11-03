//! The dev servers running in a session's project, for the panel's browser
//! start page (ui/screens/panel/browser.luau): the engine's `WatchPreviews`
//! stream for a chat (crates/engine rpc.rs; its proxy gives each a
//! `*.localhost` address), published as the shared state `previews`:
//!
//!   { chat, ready, error?, services = { { id, name, port, url, project,
//!     device, cwd, pid, started, process, command } } }
//!
//! `process` names what listens, short (`cargo test zeron_engine`, `node
//! server.js`, `python3 -m http.server`): the engine's `process`, else made
//! here from the process's arguments where it runs on this machine (the
//! same pid in the same folder). `command` is that whole command line,
//! read only here (it never leaves the device: the engine advertises the
//! short name alone), values that look like credentials as `…`; else empty.
//!
//! One chat is watched at a time (the panel shows one session):
//!
//!   previews.watch { chatId }   watch this chat's (the last one's stream ends)
//!   previews.unwatch            none; `previews` is nil
//!
//! The watch is made again as the engine comes back. The engine answers a
//! failed subscription with no item (its error ends the stream unseen
//! here), so `ready` stays false until the first frame: the scripts give up
//! waiting on their own clock.

use serde_json::{Value as Json, json};
use std::{cell::RefCell, rc::Rc};
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{
    self,
    rpc::{Rpc, Watch},
};

#[derive(Default)]
struct Previews {
    /// Each window's copy of the shared state.
    shared: RefCell<Vec<Shared>>,
    chat: RefCell<Option<String>>,
    watch: RefCell<Option<Watch>>,
    listening: std::cell::Cell<bool>,
}

thread_local! {
    static PREVIEWS: Rc<Previews> = Rc::new(Previews::default());
}

pub fn install(host: &ScriptHost) {
    let p = PREVIEWS.with(Rc::clone);
    p.shared.borrow_mut().push(host.shared("previews", Value::Nil));
    if !p.listening.replace(true) {
        let weak = Rc::downgrade(&p);
        engine::get().on_connect(move |rpc| {
            let Some(p) = weak.upgrade() else { return };
            let chat = p.chat.borrow().clone();
            if let Some(chat) = chat {
                p.start(rpc, chat);
            }
        });
    }
    let me = p.clone();
    host.action("previews.watch", move |arg| {
        let chat = arg.field("chatId").map(crate::auth::text).unwrap_or_default();
        if chat.is_empty() {
            return;
        }
        if me.chat.borrow().as_deref() == Some(chat.as_str()) && me.watch.borrow().is_some() {
            return;
        }
        *me.chat.borrow_mut() = Some(chat.clone());
        match engine::get().current() {
            Some(rpc) => me.start(&rpc, chat),
            None => me.publish(waiting(&chat)),
        }
    });
    let me = p.clone();
    host.action("previews.unwatch", move |_| {
        *me.chat.borrow_mut() = None;
        *me.watch.borrow_mut() = None;
        me.publish(Value::Nil);
    });
}

impl Previews {
    fn start(&self, rpc: &Rpc, chat: String) {
        self.publish(waiting(&chat));
        let weak = PREVIEWS.with(Rc::downgrade);
        let watched = chat.clone();
        let watch = rpc.watch("WatchPreviews", json!({ "chatId": chat }), move |item| {
            if let Some(p) = weak.upgrade() {
                // (A frame from a watch that was replaced: dropped with it.)
                if p.chat.borrow().as_deref() == Some(watched.as_str()) {
                    p.publish(snapshot(&watched, &item));
                }
            }
        });
        *self.watch.borrow_mut() = Some(watch);
    }

    fn publish(&self, value: Value) {
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }
}

fn waiting(chat: &str) -> Value {
    Value::record([
        ("chat", Value::from(chat)),
        ("ready", Value::Bool(false)),
        ("services", Value::List(Vec::<Value>::new().into())),
    ])
}

/// The engine's snapshot (zeron_proto::PreviewSnapshot) as the scripts read it.
fn snapshot(chat: &str, item: &Json) -> Value {
    let proxy_port = item["proxyPort"].as_u64().unwrap_or(0);
    let services: Vec<Value> = item["services"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let hostname = s["hostname"].as_str().unwrap_or_default();
            let port = s["port"].as_u64().unwrap_or(0);
            // (The service's address through the local proxy, as the shipping
            // app opens it: PreviewService::url.)
            let url = if proxy_port > 0 && !hostname.is_empty() {
                format!("http://{hostname}:{proxy_port}")
            } else {
                format!("http://localhost:{port}")
            };
            let pid = s["pid"].as_u64().unwrap_or(0);
            let cwd = s["cwd"].as_str().unwrap_or_default();
            let args = local_args(pid as u32, cwd);
            let process = match s["process"].as_str() {
                Some(named) if !named.is_empty() => named.to_string(),
                _ => process_label(&args),
            };
            Value::record([
                ("id", Value::from(s["id"].as_str().unwrap_or_default())),
                ("name", Value::from(s["name"].as_str().unwrap_or_default())),
                ("port", Value::Num(port as f64)),
                ("url", Value::from(url.as_str())),
                (
                    "project",
                    Value::from(s["projectName"].as_str().unwrap_or_default()),
                ),
                (
                    "device",
                    Value::from(s["deviceName"].as_str().unwrap_or_default()),
                ),
                ("cwd", Value::from(cwd)),
                ("pid", Value::Num(pid as f64)),
                (
                    "started",
                    Value::Num(s["startedAt"].as_u64().unwrap_or(0) as f64),
                ),
                ("process", Value::from(process.as_str())),
                ("command", Value::from(command_line(&shown_command(&args)).as_str())),
            ])
        })
        .collect();
    let mut fields = vec![
        ("chat", Value::from(chat)),
        ("ready", Value::Bool(true)),
        (
            "remote",
            Value::Bool(item["remote"].as_bool().unwrap_or(false)),
        ),
        ("services", Value::List(services.into())),
    ];
    if let Some(error) = item["error"].as_str() {
        fields.push(("error", Value::from(error)));
    }
    if let Some(project) = item["projectName"].as_str() {
        fields.push(("project", Value::from(project)));
    }
    Value::record(fields)
}

/// The arguments of `pid`, where it runs on this machine in `cwd` (a
/// remote device's pid names some other process here, or none).
#[cfg(target_os = "linux")]
fn local_args(pid: u32, cwd: &str) -> Vec<String> {
    if pid == 0 || cwd.is_empty() {
        return Vec::new();
    }
    let here = std::fs::read_link(format!("/proc/{pid}/cwd")).ok();
    if here.as_deref() != Some(std::path::Path::new(cwd)) {
        return Vec::new();
    }
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn local_args(_pid: u32, _cwd: &str) -> Vec<String> {
    Vec::new()
}

/// Values that look like credentials as `…` (`--token x`, `--password=x`,
/// `API_KEY=x`), and a URL's `user:pass@` and query left out: the command
/// line read here, for this machine's own tooltip.
fn shown_command(args: &[String]) -> Vec<String> {
    fn secret(name: &str) -> bool {
        let name = name.trim_start_matches('-').to_ascii_lowercase();
        ["token", "secret", "passw", "auth", "credential", "cookie", "key"]
            .iter()
            .any(|word| name.contains(word))
    }
    fn url(arg: &str) -> Option<String> {
        let (scheme, rest) = arg.split_once("://")?;
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        let rest = match rest.split_once('@') {
            Some((user, host)) if !user.contains('/') => format!("…@{host}"),
            _ => rest.to_string(),
        };
        Some(format!("{scheme}://{rest}"))
    }
    let mut hide_next = false;
    args.iter()
        .map(|arg| {
            if std::mem::take(&mut hide_next) && !arg.starts_with('-') {
                return "…".to_string();
            }
            if let Some((name, _)) = arg.split_once('=')
                && !name.contains('/')
                && secret(name)
            {
                return format!("{name}=…");
            }
            if arg.starts_with('-') && !arg.contains('=') && secret(arg) {
                hide_next = true;
            }
            url(arg).unwrap_or_else(|| arg.clone())
        })
        .collect()
}

/// Interpreters' switches that take no value (`python -u app.py`, `node
/// --inspect server.js`): any other flag without `=` is read as taking the
/// argument after it.
const SWITCHES: &[&str] = &[
    "-u", "-B", "-O", "-OO", "-E", "-s", "-S", "-I", "-q", "-b", "-bb", "-d", "-i", "-v",
    "--inspect", "--inspect-brk", "--watch", "--trace-warnings", "--no-warnings",
    "--enable-source-maps", "--hot", "--smol",
];

/// A flag whose name looks like a credential's (`--token`, `--api-key`,
/// `--password`).
fn secret_flag(flag: &str) -> bool {
    let name = flag.trim_start_matches('-').to_ascii_lowercase();
    ["token", "secret", "passw", "auth", "credential", "cookie", "key"]
        .iter()
        .any(|word| name.contains(word))
}

/// What a listener is, in a few words (zeron_preview::discovery::
/// process_label, its copy, for a process read here): its
/// program, and what a runner runs (`node server.js`, `python3 -m
/// http.server`, `vite`, `npm run dev`, `cargo test zeron_engine` for a
/// test binary). Never its other arguments, which may hold credentials:
/// only a script's or module's name, a package's, or a runner's
/// subcommand, each kept only while it looks like a plain name.
fn process_label(args: &[String]) -> String {
    fn base(path: &str) -> &str {
        path.rsplit('/').next().unwrap_or(path)
    }
    // A plain name: letters, digits and `._-` (`@`, `/` and `:` where it
    // allows them), short.
    fn plain(word: &str, extra: &str) -> bool {
        !word.is_empty()
            && word.len() <= 40
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c) || extra.contains(c))
    }
    let Some(first) = args.first() else {
        return String::new();
    };
    let program = base(first);
    if !plain(program, "") {
        return String::new();
    }
    // A Rust test binary: target/<profile>/deps/<crate>-<16 hex>.
    if first.contains("/deps/")
        && let Some((name, hash)) = program.rsplit_once('-')
        && hash.len() == 16
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return format!("cargo test {name}");
    }
    let rest = &args[1..];
    // (Code given inline, `python -c …`, `node -e …`: only the program.)
    let inline = rest
        .iter()
        .any(|a| matches!(a.as_str(), "-c" | "-e" | "--eval" | "-p" | "--print"));
    // Its arguments that are no flag nor a flag's value: a flag without
    // `=` takes the next argument (`--token abc`, `-p 3000`), unless it's
    // an interpreter's switch known to stand alone; one whose name looks
    // like a credential's always does.
    let words = || {
        let mut out: Vec<&String> = Vec::new();
        let mut value = false;
        for arg in rest {
            if std::mem::take(&mut value) {
                continue;
            }
            if arg == "--" {
                continue;
            }
            if arg.starts_with('-') {
                value = !arg.contains('=') && (secret_flag(arg) || !SWITCHES.contains(&arg.as_str()));
                continue;
            }
            out.push(arg);
        }
        out.into_iter()
    };
    let lower = program.to_ascii_lowercase();
    if lower.starts_with("python") {
        if let Some(i) = rest.iter().position(|a| a == "-m")
            && let Some(module) = rest.get(i + 1)
            && !module.starts_with('-')
            && plain(module, "")
        {
            return format!("{program} -m {module}");
        }
        if !inline && let Some(script) = words().next().map(|s| base(s)) && plain(script, "") {
            return format!("{program} {script}");
        }
        return program.to_string();
    }
    if matches!(lower.as_str(), "node" | "bun" | "deno" | "ruby" | "perl" | "php") {
        if inline {
            return program.to_string();
        }
        return match words().next() {
            // A package's own command, run through its bin script.
            Some(script) if script.contains("node_modules/") => {
                let after = script.rsplit("node_modules/").next().unwrap_or(script);
                let after = after.strip_prefix(".bin/").unwrap_or(after);
                let package = if after.starts_with('@') {
                    after.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
                } else {
                    after.split('/').next().unwrap_or(after).to_string()
                };
                if plain(&package, "@/") { package } else { program.to_string() }
            }
            Some(script) if plain(base(script), "") => format!("{program} {}", base(script)),
            _ => program.to_string(),
        };
    }
    if matches!(
        lower.as_str(),
        "npm" | "pnpm" | "yarn" | "npx" | "bunx" | "uv" | "uvx" | "cargo" | "go" | "dotnet"
    ) {
        let mut words = words();
        let Some(sub) = words.next() else {
            return program.to_string();
        };
        // npx/bunx/uvx run a package: its name.
        if matches!(lower.as_str(), "npx" | "bunx" | "uvx") {
            return if plain(sub, "@/") { format!("{program} {sub}") } else { program.to_string() };
        }
        if !matches!(
            sub.as_str(),
            "run" | "dev" | "start" | "serve" | "test" | "exec" | "watch" | "preview" | "x"
        ) {
            return program.to_string();
        }
        // `npm run dev`: a script's name from the package, after `run`.
        if matches!(sub.as_str(), "run" | "exec" | "x")
            && let Some(script) = words.next()
            && plain(script, ":")
        {
            return format!("{program} {sub} {script}");
        }
        return format!("{program} {sub}");
    }
    program.to_string()
}

/// The command line whole, an argument with a space quoted.
fn command_line(args: &[String]) -> String {
    let line = args
        .iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    clip(&line, 600)
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_names_each_service_through_the_proxy() {
        let item = json!({
            "proxyPort": 27800,
            "services": [{ "id": "a", "name": "web", "port": 3000, "hostname": "web.zeron.localhost", "projectName": "site", "deviceName": "mac", "cwd": "/w" }],
            "remote": false
        });
        let v = snapshot("c1", &item);
        let Value::List(list) = v.field("services").unwrap() else { panic!("services") };
        assert_eq!(list.len(), 1);
        assert_eq!(crate::auth::text(&list[0].field("url").unwrap()), "http://web.zeron.localhost:27800");
        assert!(v.field("ready").unwrap().truthy());
        // (An older engine sends no command, and pid 123 isn't in /w here.)
        assert_eq!(crate::auth::text(&list[0].field("process").unwrap()), "");
    }

    #[test]
    fn a_service_is_named_as_the_engine_names_it() {
        let item = json!({
            "proxyPort": 27800,
            "services": [{ "id": "a", "name": "HTTP server", "port": 8000, "hostname": "h.localhost", "pid": 9, "startedAt": 5,
                "cwd": "/nowhere", "process": "python3 -m http.server" }],
        });
        let v = snapshot("c1", &item);
        let Value::List(list) = v.field("services").unwrap() else {
            panic!("services")
        };
        assert_eq!(
            crate::auth::text(&list[0].field("process").unwrap()),
            "python3 -m http.server"
        );
        // (Its whole command line only from a process here: none.)
        assert_eq!(crate::auth::text(&list[0].field("command").unwrap()), "");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_service_on_this_machine_is_named_from_its_process() {
        let pid = std::process::id();
        let cwd = std::env::current_dir().unwrap();
        let item = json!({
            "services": [{ "id": "a", "name": "HTTP server", "port": 1, "hostname": "h.localhost", "pid": pid, "cwd": cwd.to_string_lossy() }],
        });
        let v = snapshot("c1", &item);
        let Value::List(list) = v.field("services").unwrap() else {
            panic!("services")
        };
        assert!(!crate::auth::text(&list[0].field("process").unwrap()).is_empty());
        // Another folder: some other process with that pid, unnamed.
        let item = json!({ "services": [{ "id": "a", "pid": pid, "cwd": "/nowhere" }] });
        let v = snapshot("c1", &item);
        let Value::List(list) = v.field("services").unwrap() else {
            panic!("services")
        };
        assert_eq!(crate::auth::text(&list[0].field("process").unwrap()), "");
    }

    #[test]
    fn a_process_is_named_in_a_few_words() {
        let label =
            |a: &[&str]| process_label(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            label(&[
                "/w/target/debug/deps/zeron_engine-0123456789abcdef",
                "--test-threads",
                "4"
            ]),
            "cargo test zeron_engine"
        );
        assert_eq!(label(&["/usr/bin/zeron-engine", "serve"]), "zeron-engine");
        assert_eq!(label(&["node", "/w/server.js"]), "node server.js");
        assert_eq!(
            label(&["node", "/w/node_modules/.bin/vite", "--port", "5173"]),
            "vite"
        );
        assert_eq!(
            label(&["node", "/w/node_modules/@scope/cli/bin/run.js"]),
            "@scope/cli"
        );
        assert_eq!(
            label(&["/usr/bin/python3", "-m", "http.server", "8000"]),
            "python3 -m http.server"
        );
        assert_eq!(label(&["python", "app.py"]), "python app.py");
        assert_eq!(label(&["npm", "run", "dev"]), "npm run dev");
        assert_eq!(label(&["cargo", "test", "-p", "x"]), "cargo test");
        assert_eq!(label(&["npx", "--token", "abc", "server"]), "npx server");
        assert_eq!(label(&["node", "--api-key", "s3cr3t", "app.js"]), "node app.js");
        assert_eq!(label(&[]), "");
    }

    #[test]
    fn a_command_hides_what_looks_like_credentials() {
        let args: Vec<String> = [
            "serve",
            "--token",
            "abc",
            "API_KEY=x",
            "--port",
            "1",
            "https://user:pass@host/x?key=1",
        ]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            command_line(&shown_command(&args)),
            "serve --token … API_KEY=… --port 1 https://…@host/x"
        );
    }
}
