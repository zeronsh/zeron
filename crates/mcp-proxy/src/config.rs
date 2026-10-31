use std::{io, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Lazy,
    LazyKeepAlive,
    KeepAlive,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub server: String,
    pub mode: Mode,
    pub idle: Duration,
    pub identity_env: Vec<String>,
    /// Shim-only PATH identity, with its per-run interception directory removed.
    pub identity_path: Option<String>,
    pub plugin_root: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub run_dir: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub no_npx_resolve: bool,
    pub command: String,
    pub args: Vec<String>,
    pub selftest: bool,
    pub backoff: Duration,
    pub start_timeout: Duration,
    pub check_interval: Duration,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn millis(name: &str, default: u64) -> Duration {
    Duration::from_millis(
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
            .max(1),
    )
}
impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>, data_dir: PathBuf) -> io::Result<Self> {
        let mut c = Self {
            server: String::new(),
            mode: Mode::Lazy,
            idle: Duration::from_secs(600),
            identity_env: Vec::new(),
            identity_path: None,
            plugin_root: None,
            cwd: None,
            run_dir: None,
            data_dir,
            no_npx_resolve: false,
            command: String::new(),
            args: Vec::new(),
            selftest: false,
            backoff: millis("ZERON_MCP_PROXY_BACKOFF_MS", 60_000),
            start_timeout: millis("ZERON_MCP_PROXY_START_TIMEOUT_MS", 60_000),
            check_interval: millis("ZERON_MCP_PROXY_CHECK_MS", 30_000),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                c.command = args
                    .next()
                    .ok_or_else(|| invalid("missing command after --"))?;
                c.args = args.collect();
                break;
            }
            if arg == "--selftest" {
                c.selftest = true;
                continue;
            }
            if arg == "--no-npx-resolve" {
                c.no_npx_resolve = true;
                continue;
            }
            let value = args
                .next()
                .ok_or_else(|| invalid("option requires a value"))?;
            match arg.as_str() {
                "--server" => c.server = value,
                "--mode" => {
                    c.mode = match value.as_str() {
                        "lazy" => Mode::Lazy,
                        "lazy-keep-alive" => Mode::LazyKeepAlive,
                        "keep-alive" => Mode::KeepAlive,
                        _ => return Err(invalid("unknown lifecycle mode")),
                    }
                }
                "--idle-min" => {
                    let minutes: f64 = value.parse().map_err(|_| invalid("invalid idle-min"))?;
                    if !minutes.is_finite() || minutes < 0.0 || minutes > 525600.0 {
                        return Err(invalid("idle-min out of range"));
                    }
                    c.idle = Duration::from_secs_f64(minutes * 60.0);
                }
                "--identity-env" => {
                    c.identity_env = value
                        .split(',')
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned)
                        .collect()
                }
                "--plugin-root" => c.plugin_root = Some(value.into()),
                "--cwd" => c.cwd = Some(value.into()),
                "--run-dir" => c.run_dir = Some(value.into()),
                _ => return Err(invalid("unknown mcp-proxy option")),
            }
        }
        if let Ok(ms) = std::env::var("ZERON_MCP_PROXY_IDLE_MS") {
            c.idle = Duration::from_millis(ms.parse().map_err(|_| invalid("invalid idle ms"))?);
        }
        if !c.selftest && (c.command.is_empty() || c.server.is_empty()) {
            return Err(invalid(
                "mcp-proxy requires --server NAME -- COMMAND ARGS; or --selftest",
            ));
        }
        Ok(c)
    }
}
