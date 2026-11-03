//! Device-local discovery metadata and the browser's live preview list.
use serde::{Deserialize, Serialize};

pub const PREVIEW_PROXY_PORT: u16 = 7331;

/// Stamped by the engine into the environment of every process it launches:
/// [`chat_origin`] for agent runs, [`terminal_origin`] for its terminals
/// (project actions run in one). Children inherit it, through `nohup`,
/// daemonizing and an engine restart alike, so preview discovery lists
/// exactly the servers Zeron started and knows which chat started them. One
/// variable, so the nearest Zeron launch always overwrites an outer one.
pub const ORIGIN_ENV: &str = "ZERON_ORIGIN";

pub fn chat_origin(chat_id: &str) -> String {
    format!("chat:{chat_id}")
}

pub fn terminal_origin(terminal_id: &str) -> String {
    format!("terminal:{terminal_id}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewService {
    /// Persisted identities; neither includes a listening port or process ID.
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub project_cwd: String,
    pub device_id: String,
    pub device_name: String,
    pub hostname: String,
    pub name: String,
    pub port: u16,
    pub pid: u32,
    pub cwd: String,
    /// Process creation time, milliseconds since the Unix epoch.
    pub started_at: u64,
    pub zeron_owned: bool,
    /// The chat whose agent or terminal started it (see [`ORIGIN_ENV`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    /// What listens, in a few words (`node server.js`, `python3 -m
    /// http.server`): its program and the script or module it runs, never
    /// its other arguments. Empty from engines that predate it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub process: String,
}

impl PreviewService {
    pub fn url(&self, proxy_port: u16) -> String {
        format!("http://{}:{proxy_port}", self.hostname)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSnapshot {
    pub services: Vec<PreviewService>,
    pub proxy_port: u16,
    pub error: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub remote: bool,
}

impl Default for PreviewSnapshot {
    fn default() -> Self {
        Self {
            services: Vec::new(),
            proxy_port: PREVIEW_PROXY_PORT,
            error: None,
            project_name: None,
            remote: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPreviewsParams {
    pub chat_id: String,
}

// What a listener is, in words: the label advertised with a service
// (`PreviewService::process`) and the command line shown on its own
// machine. One copy, for the engine that advertises and the app that shows.

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

/// What a listener is, in a few words, for clients to name it by: its
/// program, and what a runner runs (`node server.js`, `python3 -m
/// http.server`, `vite`, `npm run dev`, `cargo test zeron_engine` for a
/// test binary). Never its other arguments, which may hold credentials:
/// only a script's or module's name, a package's, or a runner's
/// subcommand, each kept only while it looks like a plain name.
pub fn process_label(args: &[String]) -> String {
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

/// A command line with values that look like credentials as `…`
/// (`--token x`, `--password=x`, `API_KEY=x`), and a URL's `user:pass@` and
/// query left out: for showing a process's command line on the machine it
/// runs on (it's never advertised).
pub fn shown_command(args: &[String]) -> Vec<String> {
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
                && secret_flag(name)
            {
                return format!("{name}=…");
            }
            if arg.starts_with('-') && !arg.contains('=') && secret_flag(arg) {
                hide_next = true;
            }
            url(arg).unwrap_or_else(|| arg.clone())
        })
        .collect()
}

#[cfg(test)]
mod process_tests {
    use super::*;

    #[test]
    fn a_process_is_named_by_its_program_and_what_it_runs_only() {
        let label = |a: &[&str]| process_label(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(label(&["/usr/bin/python3", "-m", "http.server", "8000"]), "python3 -m http.server");
        assert_eq!(label(&["python", "app.py", "--token=abc"]), "python app.py");
        assert_eq!(label(&["python", "-c", "print(1)"]), "python");
        assert_eq!(label(&["node", "/w/server.js", "--port", "3000"]), "node server.js");
        assert_eq!(label(&["node", "/w/node_modules/.bin/vite"]), "vite");
        assert_eq!(label(&["node", "/w/node_modules/@scope/cli/bin/run.js"]), "@scope/cli");
        assert_eq!(label(&["npm", "run", "dev"]), "npm run dev");
        assert_eq!(label(&["cargo", "test", "-p", "x"]), "cargo test");
        assert_eq!(label(&["/w/target/debug/deps/zeron_engine-0123456789abcdef", "--x"]), "cargo test zeron_engine");
        assert_eq!(label(&["/usr/bin/zeron-engine", "serve", "--token", "abc"]), "zeron-engine");
        assert_eq!(label(&[]), "");
        // A flag's value is never what runs: `--flag value` is a pair.
        assert_eq!(label(&["node", "-p", "3000", "server.js"]), "node");
        assert_eq!(label(&["node", "--port", "3000", "server.js"]), "node server.js");
        assert_eq!(label(&["node", "--inspect", "server.js"]), "node server.js");
        assert_eq!(label(&["python3", "-u", "app.py"]), "python3 app.py");
        assert_eq!(label(&["python3", "--token", "abc", "app.py"]), "python3 app.py");
        assert_eq!(label(&["node", "--api-key", "s3cr3t", "app.js"]), "node app.js");
        assert_eq!(label(&["node", "--api-key=s3cr3t", "app.js"]), "node app.js");
        assert_eq!(label(&["npx", "--token", "abc", "server"]), "npx server");
        assert_eq!(label(&["npx", "--token=abc", "server"]), "npx server");
        assert_eq!(label(&["npm", "--auth", "abc", "run", "dev"]), "npm run dev");
        assert_eq!(label(&["server", "-p", "3000"]), "server");
        // Nothing but plain names: a secret in a script's place, a URL, a
        // runner's argument, never.
        for args in [
            vec!["node", "--token=abc", "https://user:pass@host/x?key=1"],
            vec!["python3", "https://user:pass@host/x?key=1"],
            vec!["npx", "https://user:pass@host/x?key=1"],
            vec!["npm", "run", "--token=abc"],
            vec!["server", "--token=abc", "https://user:pass@host/x?key=1"],
            vec!["npx", "--token", "abc", "server"],
            vec!["node", "--api-key", "s3cr3t", "app.js"],
            vec!["python3", "--password", "abc", "-m", "http.server"],
            vec!["npm", "--auth", "abc"],
            vec!["cargo", "--token", "abc", "run"],
        ] {
            let l = label(&args);
            for secret in ["abc", "s3cr3t", "pass", "key=1", "host"] {
                assert!(!l.contains(secret), "{l:?} from {args:?}");
            }
        }
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
            shown_command(&args).join(" "),
            "serve --token … API_KEY=… --port 1 https://…@host/x"
        );
    }
}
