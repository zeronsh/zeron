//! Which engine this app works through, and keeping it running: the one
//! place that starts, restarts and refuses engines, so it can grow into the
//! engine's manager the shipping app has (crates/engine/src/managed.rs).
//!
//! For now it is a staging engine: the one at `ZERON_ZGX_ENGINE_PORT`
//! (27871) with its data in `ZERON_ZGX_ENGINE_DATA` (~/.zeron-staging),
//! scripts/staging-app.sh's. One already serving that port is used as it
//! is; otherwise this app starts `zeron-engine` (`ZERON_ZGX_ENGINE_BIN`, or
//! this checkout's `target/debug/zeron-engine`: `cargo build -p zeron
//! --bin zeron-engine` at the repository's root) against the same edge and
//! sign-in client as `src/auth.rs`'s, detached, so it outlives the app as
//! the shipping app's engine does.
//!
//!   ZERON_ZGX_ENGINE_PORT=<n>     the engine's IPC port (27871)
//!   ZERON_ZGX_ENGINE_DATA=<dir>   its data dir (~/.zeron-staging)
//!   ZERON_ZGX_ENGINE_BIN=<path>   the engine to start when none answers
//!   ZERON_ZGX_ENGINE_HOME=<dir>   a home of its own for the engine it starts
//!                                 (HOME, CLAUDE_CONFIG_DIR, CODEX_HOME), so
//!                                 the agents' logins it reads and writes are
//!                                 that dir's, not this Mac's: for trying
//!                                 sign-ins with throwaway state
//!   ZERON_ZGX_ENGINE_SPAWN=0      never start one; only use one that runs
//!
//! Never the production engine: port 27654 and ~/.comet-native are refused,
//! whatever the variables say.
//!
//! Zeron Preview (the `preview` feature: the app in its own bundle beside
//! Zeron.app, scripts/package-zgx-preview.sh) is production's only: its
//! edge and sign-in are production's whatever the variables say
//! (src/auth.rs), and staging's engine (port 27871, ~/.zeron-staging) is
//! refused as production's is. It has an engine of its own instead, the
//! engine variables above still moving it (port, data, binary, home):
//!   - the one in its bundle (`Contents/MacOS/zeron-engine`, beside this
//!     app's executable), on port 27664 with its data in ~/.zeron-preview;
//!   - named "<this Mac's name> · Preview" in the account
//!     (`ZERON_DEVICE_NAME`; `ZERON_ZGX_DEVICE_NAME` names it otherwise);
//!   - its updater reading the preview channel (`{edge}/releases/preview`,
//!     `ZERON_RELEASES_URL`), never the Zeron app's manifest: it only
//!     reports there anyway (an engine in an app bundle never swaps it), and
//!     About shows that channel's;
//!   - given its bundle as `ZERON_APP`, so its banners are Zeron Preview's
//!     and open it, and it restarts into a newer bundle copied over this one
//!     once nothing runs and the app is up (crates/engine/src/managed.rs).
//!
//! A signed-out engine opens a local workspace and keeps it; signing in
//! takes a new one (the shipping app's sync switch). So once the engine's
//! session is signed in while its workspace is still local, it is stopped
//! and started again ([`needs_restart`]); a synced engine stops by itself on
//! sign-out and is started again, signed out.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The production engine's port and data dir, refused outright.
const PRODUCTION_PORT: u16 = 27654;
const PRODUCTION_DATA: &str = ".comet-native";

const STAGING_PORT: u16 = 27871;
const STAGING_DATA: &str = ".zeron-staging";

/// Zeron Preview's engine: a port and data dir of its own, beside both.
const PREVIEW_PORT: u16 = 27664;
const PREVIEW_DATA: &str = ".zeron-preview";
/// The engine's executable in Zeron Preview's bundle, beside this app's
/// (a name crates/engine's managed.rs knows as an engine's, not the app's).
const PREVIEW_ENGINE: &str = "zeron-engine";

const PREVIEW: bool = cfg!(feature = "preview");

/// The soonest a second engine is started after the first.
const SPAWN_SPACING: Duration = Duration::from_secs(10);

pub struct Config {
    pub port: u16,
    pub data_dir: PathBuf,
    bin: Option<PathBuf>,
    home: Option<PathBuf>,
    spawn: bool,
    /// What the engine signs in against: `src/auth.rs`'s.
    edge: String,
    client: String,
    callback_port: u16,
    /// Zeron Preview's: its device's name in the account (none: the
    /// engine's own, this Mac's name).
    device_name: Option<String>,
    /// Zeron Preview's: the bundle its banners open (`ZERON_APP`).
    app: Option<PathBuf>,
}

/// Why no engine can be used.
pub type Refusal = String;

/// The file in an engine's data dir holding the build of the engine this
/// app started there last (`zeron-engine --build-id`): an engine reporting
/// that build is one this app started.
const STARTED: &str = "zgx-started-build";

impl Config {
    /// The engine this app would start now, if it may start one.
    pub fn startable(&self) -> Option<&Path> {
        if self.spawn { self.bin.as_deref() } else { None }
    }
}

/// Which build `bin` is (`zeron-engine --build-id`), if it says; an engine
/// from before build ids doesn't.
pub fn build_of(bin: &Path) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--build-id")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let id = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !id.is_empty() && !id.contains(' ')).then_some(id)
}

/// The build of the engine this app last started with `config`'s data.
pub fn started_build(config: &Config) -> Option<String> {
    let id = std::fs::read_to_string(config.data_dir.join(STARTED)).ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

impl Config {
    /// From the environment, for the account's edge, sign-in client and
    /// callback port.
    pub fn from_env(edge: &str, client: &str, callback_port: u16) -> Result<Self, Refusal> {
        let var = |key: &str| std::env::var(key).ok().filter(|v: &String| !v.trim().is_empty());
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        Self::from_vars(var, &home, edge, client, callback_port)
    }

    fn from_vars(
        var: impl Fn(&str) -> Option<String>,
        home: &Path,
        edge: &str,
        client: &str,
        callback_port: u16,
    ) -> Result<Self, Refusal> {
        let port = match var("ZERON_ZGX_ENGINE_PORT") {
            Some(raw) => raw
                .trim()
                .parse()
                .map_err(|_| format!("ZERON_ZGX_ENGINE_PORT `{raw}` isn't a port"))?,
            None if PREVIEW => PREVIEW_PORT,
            None => STAGING_PORT,
        };
        let data_dir = var("ZERON_ZGX_ENGINE_DATA")
            .map(|dir| expand(&dir, home))
            .unwrap_or_else(|| home.join(if PREVIEW { PREVIEW_DATA } else { STAGING_DATA }));
        if port == PRODUCTION_PORT {
            return Err(format!("port {PRODUCTION_PORT} is the production engine's"));
        }
        if same_dir(&data_dir, &home.join(PRODUCTION_DATA)) {
            return Err(format!("~/{PRODUCTION_DATA} is the production engine's"));
        }
        // Zeron Preview is production's only: never staging's engine (signed
        // into staging), nor any edge but production's.
        if PREVIEW {
            if port == STAGING_PORT {
                return Err(format!("port {STAGING_PORT} is the staging engine's"));
            }
            if same_dir(&data_dir, &home.join(STAGING_DATA)) {
                return Err(format!("~/{STAGING_DATA} is the staging engine's"));
            }
            let production = crate::auth::production();
            if (edge, client, callback_port) != (production.edge.as_str(), production.client.as_str(), production.port) {
                return Err(format!("{edge} isn't production's edge and sign-in"));
            }
        }
        let exe = std::env::current_exe().ok();
        let bin = var("ZERON_ZGX_ENGINE_BIN").map(|p| expand(&p, home)).or_else(|| {
            let built = if PREVIEW {
                // The bundle's, beside this app's executable.
                exe.as_deref()?.parent()?.join(PREVIEW_ENGINE)
            } else {
                // This checkout's: apps/zgx is two below the repository's root.
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/zeron-engine")
            };
            built.exists().then_some(built)
        });
        let device_name = var("ZERON_ZGX_DEVICE_NAME").or_else(|| PREVIEW.then(|| preview_device_name(&computer_name())));
        let app = PREVIEW
            .then(|| exe.as_deref()?.ancestors().find(|dir| dir.extension().is_some_and(|ext| ext == "app")).map(Path::to_path_buf))
            .flatten();
        Ok(Self {
            port,
            data_dir,
            bin,
            home: var("ZERON_ZGX_ENGINE_HOME").map(|dir| expand(&dir, home)),
            spawn: var("ZERON_ZGX_ENGINE_SPAWN").as_deref() != Some("0"),
            edge: edge.into(),
            client: client.into(),
            callback_port,
            device_name,
            app,
        })
    }

    pub fn describe(&self) -> String {
        format!("port {}, data {}", self.port, self.data_dir.display())
    }

    /// Where the engine's updater looks for releases: Zeron Preview's own
    /// channel (none: the edge's, the Zeron app's).
    fn releases(&self) -> Option<String> {
        PREVIEW.then(|| format!("{}/releases/preview", self.edge.trim_end_matches('/')))
    }
}

/// This Mac's name as the account shows it (System Settings → General →
/// About → Name), or its host name.
fn computer_name() -> String {
    let named = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    named.unwrap_or_else(|| {
        std::process::Command::new("/bin/hostname")
            .arg("-s")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Mac".into())
    })
}

/// Zeron Preview's device, told apart from the Zeron app's on the same Mac.
fn preview_device_name(computer: &str) -> String {
    format!("{} · Preview", computer.trim())
}

fn expand(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// Starts engines for `config`, no more often than [`SPAWN_SPACING`].
pub struct Spawner {
    last: Option<Instant>,
    said_missing: bool,
}

impl Spawner {
    pub fn new() -> Self {
        Self { last: None, said_missing: false }
    }

    /// Nothing answers on the port: start an engine, if this app may and
    /// it's been long enough since the last. Whether one was started.
    pub fn start_if_due(&mut self, config: &Config) -> bool {
        if !config.spawn || self.last.is_some_and(|at| at.elapsed() < SPAWN_SPACING) {
            return false;
        }
        let Some(bin) = &config.bin else {
            if !self.said_missing {
                self.said_missing = true;
                eprintln!(
                    "zeron-zgx: engine: nothing answers on {} and there's no engine to start: \
                     build one (`cargo build -p zeron --bin zeron-engine` at the repository's root) \
                     or set ZERON_ZGX_ENGINE_BIN",
                    config.describe()
                );
            }
            return false;
        };
        self.last = Some(Instant::now());
        match spawn(config, bin) {
            Ok(()) => {
                eprintln!("zeron-zgx: engine: started {} ({})", bin.display(), config.describe());
                true
            }
            Err(error) => {
                eprintln!("zeron-zgx: engine: {} didn't start: {error}", bin.display());
                false
            }
        }
    }
}

/// The engine, detached (its own process group, no inherited pipes), so
/// this app closing leaves it running; it logs to its data dir.
fn spawn(config: &Config, bin: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(&config.data_dir)?;
    // Which build it is, kept (an engine that reports it is this app's), as
    // its reaper asks below: off this thread, since asking runs the binary.
    // Until then none is kept, so a check meanwhile reads it as not ours.
    let started = config.data_dir.join(STARTED);
    let _ = std::fs::remove_file(&started);
    let mut command = std::process::Command::new(bin);
    command
        .env("ZERON_DATA_DIR", &config.data_dir)
        .env("ZERON_IPC_PORT", config.port.to_string())
        .env("ZERON_EDGE_URL", &config.edge)
        .env("ZERON_WORKOS_CLIENT_ID", &config.client)
        .env("ZERON_CALLBACK_PORT", config.callback_port.to_string())
        // Not the shipping app's: no app to open from a banner.
        .env_remove("ZERON_APP")
        // Nor a chat's (this app run from a Zeron terminal): an engine that
        // carries a chat's origin marker lends it to everything it runs, so
        // the shipping engine's preview scan would probe its agents'
        // sign-in callbacks (Gemini CLI's OAuth ends on the first stray
        // request).
        .env_remove("ZERON_ORIGIN")
        .env_remove("ZERON_EDGE_TOKEN")
        .env_remove("ZERON_IPC_TOKEN")
        .env_remove("ZERON_IPC_TOKEN_FILE")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Zeron Preview's: its device's name, its release channel and its bundle
    // (set after the removals above, which `ZERON_APP` would otherwise lose).
    if let Some(name) = &config.device_name {
        command.env("ZERON_DEVICE_NAME", name);
    }
    if let Some(releases) = config.releases() {
        command.env("ZERON_RELEASES_URL", releases);
    }
    if let Some(app) = &config.app {
        command.env("ZERON_APP", app);
    }
    if let Some(home) = &config.home {
        let (claude, codex) = (home.join(".claude"), home.join(".codex"));
        std::fs::create_dir_all(&claude)?;
        std::fs::create_dir_all(&codex)?;
        command
            .env("HOME", home)
            .env("CLAUDE_CONFIG_DIR", claude)
            .env("CODEX_HOME", codex);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn()?;
    // Its build kept, then it reaped when it exits, whenever that is.
    let bin = bin.to_path_buf();
    std::thread::spawn(move || {
        if let Some(id) = build_of(&bin) {
            let _ = std::fs::write(&started, id);
        }
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

/// Whether the engine has to start again to follow its session: signed in
/// (or choosing an organization) while its workspace is still the local
/// one it opened signed out.
pub fn needs_restart(scope: Option<&str>, signed_in: bool) -> bool {
    signed_in && scope == Some("local")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_in_session_on_a_local_workspace_restarts() {
        assert!(needs_restart(Some("local"), true));
        assert!(!needs_restart(Some("local"), false));
        assert!(!needs_restart(Some("synced"), true));
        assert!(!needs_restart(Some("development"), true));
        assert!(!needs_restart(None, true));
    }

    #[test]
    fn a_preview_device_is_marked_as_one() {
        assert_eq!(preview_device_name("Wing's MacBook Pro\n"), "Wing's MacBook Pro · Preview");
    }

    #[test]
    fn preview_has_an_engine_of_its_own() {
        for port in [PRODUCTION_PORT, STAGING_PORT] {
            assert_ne!(PREVIEW_PORT, port);
        }
        for data in [PRODUCTION_DATA, STAGING_DATA] {
            assert_ne!(PREVIEW_DATA, data);
        }
    }

    /// A preview build's engine: production's edge, sign-in and release
    /// channel, Preview's port and data, whatever the variables say; and
    /// staging's engine refused outright.
    #[cfg(feature = "preview")]
    #[test]
    fn a_preview_build_resolves_to_production_only() {
        let home = Path::new("/Users/someone");
        let prod = crate::auth::endpoint();
        let config = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            Config::from_vars(
                move |key| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
                home,
                &prod.edge,
                &prod.client,
                prod.port,
            )
        };
        let plain = config(&[]).unwrap();
        assert_eq!((plain.edge.as_str(), plain.client.as_str()), ("https://edge.zeron.sh", "client_01KWD0EAKZKD50YCQJNYSRE4BY"));
        assert_eq!(plain.callback_port, 27641);
        assert_eq!((plain.port, plain.data_dir.clone()), (PREVIEW_PORT, home.join(PREVIEW_DATA)));
        assert_eq!(plain.releases().as_deref(), Some("https://edge.zeron.sh/releases/preview"));
        assert!(plain.device_name.as_deref().is_some_and(|n| n.ends_with(" · Preview")));
        // The edge-picking variables do nothing (src/auth.rs); staging's
        // engine is refused.
        let staged = config(&[("ZERON_ZGX_ENV", "staging"), ("ZERON_ZGX_EDGE", "https://staging.zeron.sh")]).unwrap();
        assert_eq!(staged.edge, "https://edge.zeron.sh");
        assert!(config(&[("ZERON_ZGX_ENGINE_PORT", "27871")]).is_err());
        assert!(config(&[("ZERON_ZGX_ENGINE_DATA", "~/.zeron-staging")]).is_err());
        assert!(config(&[("ZERON_ZGX_ENGINE_PORT", "27654")]).is_err());
        // Nor any other edge, should one be handed in.
        assert!(Config::from_vars(|_| None, home, "https://staging.zeron.sh", &prod.client, prod.port).is_err());
    }

    #[test]
    fn home_relative_paths_expand() {
        let home = Path::new("/Users/someone");
        assert_eq!(expand("~/.zeron-staging", home), home.join(".zeron-staging"));
        assert_eq!(expand("/tmp/x", home), PathBuf::from("/tmp/x"));
    }
}
