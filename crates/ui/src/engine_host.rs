//! Where the headed app's engine lives.
//!
//! On Unix the window runs no engine of its own: it attaches to an **engine
//! host**, a `zeron headless --host` process that outlives window restarts and
//! can replace itself in place across an update (see `docs/live-update.md`).
//! If nothing answers on the IPC port, [`ensure_engine_host`] starts one — the
//! installed service if there is one, else a detached process — and the window
//! attaches to it. When no host can be started the window still embeds an
//! engine as before, so a broken host never bricks the app.

#[cfg(unix)]
use std::time::Duration;

/// Whether the window starts (and attaches to) an engine host, or embeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPolicy {
    /// Attach to a running engine or embed one in this process (Windows, tests,
    /// fixtures, `ZERON_EMBED_ENGINE=1`).
    Embed,
    /// Attach; if nothing answers, start an engine host and attach to that.
    SpawnOrAttach,
}

impl HostPolicy {
    /// The policy of the real app: hosted on Unix unless `ZERON_EMBED_ENGINE`
    /// asks for the embedded engine (a debugging escape hatch).
    pub fn from_env() -> Self {
        Self::from_parts(
            cfg!(unix),
            std::env::var("ZERON_EMBED_ENGINE").ok().as_deref(),
            // A source build or hand-copied binary never updates itself, and a
            // host that outlives the window would keep running stale code after
            // a rebuild: it embeds, as it always did.
            !matches!(
                zeron_update::detect_install(),
                zeron_update::InstallKind::Unmanaged
            ),
        )
    }

    fn from_parts(unix: bool, embed: Option<&str>, installed: bool) -> Self {
        let embed = embed.is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        });
        if unix && installed && !embed {
            Self::SpawnOrAttach
        } else {
            Self::Embed
        }
    }
}

static POLICY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called once by the real app before it boots: from then on
/// [`crate::state::EngineHandle::bootstrap`] follows `policy`. Never called by
/// tests or fixtures, which therefore always embed (a hosted engine would
/// exec the test binary).
pub fn set_policy(policy: HostPolicy) {
    POLICY.store(
        policy == HostPolicy::SpawnOrAttach,
        std::sync::atomic::Ordering::SeqCst,
    );
}

/// The policy in force (see [`set_policy`]).
pub fn policy() -> HostPolicy {
    if POLICY.load(std::sync::atomic::Ordering::SeqCst) {
        HostPolicy::SpawnOrAttach
    } else {
        HostPolicy::Embed
    }
}

/// This window was started by an update swap (`ZERON_ATTACH_ONLY=1`): it must
/// attach to the engine host that is already running and never embed one.
pub fn attach_only() -> bool {
    crate::app_update::started_by_swap()
}

/// How a missing engine host is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLaunch {
    /// `systemctl --user start zeron.service`
    Systemd,
    /// `launchctl kickstart` of the installed agent
    Launchd,
    /// A detached `zeron headless --host` in its own session
    Detached,
}

/// What decides [`choose_launch`].
#[derive(Debug, Clone, Copy)]
pub struct HostEnv {
    pub os: &'static str,
    pub systemd_unit: bool,
    pub launchd_plist: bool,
    /// The app was pointed at a specific engine (`ZERON_IPC_PORT` or
    /// `ZERON_DATA_DIR`): the installed service would serve another one.
    pub custom_engine: bool,
}

impl HostEnv {
    pub fn detect() -> Self {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        Self {
            os: std::env::consts::OS,
            systemd_unit: config
                .is_some_and(|config| config.join("systemd/user/zeron.service").is_file()),
            launchd_plist: home.is_some_and(|home| {
                home.join("Library/LaunchAgents/sh.zeron.app.plist")
                    .is_file()
            }),
            custom_engine: std::env::var_os("ZERON_IPC_PORT").is_some()
                || std::env::var_os("ZERON_DATA_DIR").is_some(),
        }
    }
}

/// Prefer the installed service (it is what the user set up to keep the engine
/// running), else detach a process of our own.
pub fn choose_launch(env: &HostEnv) -> HostLaunch {
    if env.custom_engine {
        return HostLaunch::Detached;
    }
    match (env.os, env.systemd_unit, env.launchd_plist) {
        ("linux", true, _) => HostLaunch::Systemd,
        ("macos", _, true) => HostLaunch::Launchd,
        _ => HostLaunch::Detached,
    }
}

/// Whether the host `ensure_engine_host` brought up is this app's to stop when
/// the window quits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostStart {
    /// The app started it (a detached process, or a service that was stopped):
    /// quitting stops it again.
    Ours,
    /// Something else was already bringing it up (a service that was starting
    /// on its own), or it is managed by a service manager the app does not
    /// track: it keeps running.
    NotOurs,
}

/// Start an engine host and wait until it answers on `port`.
#[cfg(unix)]
pub async fn ensure_engine_host(port: u16) -> anyhow::Result<HostStart> {
    let mut start = HostStart::Ours;
    match choose_launch(&HostEnv::detect()) {
        HostLaunch::Systemd => {
            // A service that is already active or activating (login autostart
            // racing this app) is not ours; only one we bring up from stopped is.
            let state = output_blocking("systemctl", &["--user", "is-active", "zeron.service"])
                .await
                .unwrap_or_default();
            if matches!(state.trim(), "active" | "activating" | "reloading") {
                start = HostStart::NotOurs;
            }
            run_blocking("systemctl", &["--user", "start", "zeron.service"]).await?
        }
        HostLaunch::Launchd => {
            let domain = format!("gui/{}", uid());
            let plist = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default()
                .join("Library/LaunchAgents/sh.zeron.app.plist");
            // `bootstrap` fails harmlessly when the job is already loaded;
            // `kickstart` then guarantees a running process either way.
            let _ = run_blocking(
                "launchctl",
                &["bootstrap", &domain, &plist.to_string_lossy()],
            )
            .await;
            run_blocking(
                "launchctl",
                &["kickstart", &format!("{domain}/sh.zeron.app")],
            )
            .await?;
            // The app does not track launchd's state: leave the agent running.
            start = HostStart::NotOurs;
        }
        HostLaunch::Detached => {
            start_detached(port).await?;
            return Ok(HostStart::Ours);
        }
    }
    wait_for_port(port, Duration::from_secs(20), None).await?;
    Ok(start)
}

#[cfg(not(unix))]
pub async fn ensure_engine_host(_port: u16) -> anyhow::Result<HostStart> {
    anyhow::bail!("an engine host is not supported on this platform")
}

/// Ownership of a host, as reported by the engine itself (the process the
/// window started carries `ZERON_ENGINE_HOST=app`): it survives a crashed
/// window and is what a window started by an update swap sees.
pub fn is_app_hosted(info: &zeron_proto::EngineInfo) -> bool {
    info.supports(zeron_proto::capabilities::APP_HOSTED)
}

#[cfg(unix)]
fn uid() -> u32 {
    // SAFETY: getuid has no failure mode and touches no memory.
    unsafe { libc::getuid() }
}

#[cfg(unix)]
async fn output_blocking(program: &'static str, args: &[&str]) -> anyhow::Result<String> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new(program).args(&args).output()?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    })
    .await?
}

#[cfg(unix)]
async fn run_blocking(program: &'static str, args: &[&str]) -> anyhow::Result<()> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new(program).args(&args).output()?;
        if !output.status.success() {
            anyhow::bail!(
                "{program} {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    })
    .await?
}

/// A host in a session and, where systemd is available, a cgroup scope of its
/// own: closing the window, its terminal or the app's scope must not take the
/// engine down with it. Falls back to a plain detached process when the scope
/// cannot be created.
#[cfg(unix)]
async fn start_detached(port: u16) -> anyhow::Result<()> {
    if cfg!(target_os = "linux")
        && std::env::var_os("XDG_RUNTIME_DIR").is_some()
        && which("systemd-run")
    {
        let mut child = spawn_detached(true)?;
        match wait_for_port(port, Duration::from_secs(20), Some(&mut child)).await {
            Ok(()) => {
                reap_in_background(child);
                return Ok(());
            }
            // systemd-run itself failed (no user manager, refused scope):
            // start the host directly instead of waiting out the timeout.
            Err(error) if matches!(child.try_wait(), Ok(Some(_))) => {
                tracing::warn!(%error, "could not start the engine host in its own scope; starting it directly");
            }
            Err(error) => return Err(error),
        }
    }
    let mut child = spawn_detached(false)?;
    wait_for_port(port, Duration::from_secs(20), Some(&mut child)).await?;
    reap_in_background(child);
    Ok(())
}

#[cfg(unix)]
fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[cfg(unix)]
fn spawn_detached(scoped: bool) -> anyhow::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let exe = zeron_update::stable_exe()?;
    let mut command = if scoped {
        let mut command = std::process::Command::new("systemd-run");
        command
            .args(["--user", "--scope", "--quiet", "--collect"])
            .arg("--")
            .arg(&exe);
        command
    } else {
        std::process::Command::new(&exe)
    };
    command
        .args(["headless", "--host"])
        // This host is the app's engine: the app downloads and installs
        // updates, and the host follows by handing itself over.
        .env("ZERON_ENGINE_HOST", "app")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // A session of its own (which is also a new process group): no controlling
    // terminal, so an agent that opens /dev/tty is refused instead of stopped
    // by SIGTTIN, and closing the window's terminal cannot hang it up.
    // SAFETY: setsid is async-signal-safe and touches no shared state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command.spawn()?)
}

/// Reap the host if it ever exits while the window is still running.
#[cfg(unix)]
fn reap_in_background(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Wait until something listens on `port`. With a `child`, give up at once if
/// it exits first (a host that fails at boot must not cost the whole timeout).
#[cfg(unix)]
async fn wait_for_port(
    port: u16,
    timeout: Duration,
    mut child: Option<&mut std::process::Child>,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return Ok(());
        }
        if let Some(child) = child.as_deref_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            anyhow::bail!("the engine host exited before listening on port {port} ({status})");
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "the engine host did not start listening on port {port} within {} seconds",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_prefers_the_installed_service_then_detaches() {
        let base = HostEnv {
            os: "linux",
            systemd_unit: true,
            launchd_plist: false,
            custom_engine: false,
        };
        assert_eq!(choose_launch(&base), HostLaunch::Systemd);
        // Pointed at a specific engine (custom port/data dir): never the
        // installed service, which would serve a different one.
        assert_eq!(
            choose_launch(&HostEnv {
                custom_engine: true,
                ..base
            }),
            HostLaunch::Detached
        );
        assert_eq!(
            choose_launch(&HostEnv {
                systemd_unit: false,
                ..base
            }),
            HostLaunch::Detached
        );
        let mac = HostEnv {
            os: "macos",
            systemd_unit: false,
            launchd_plist: true,
            custom_engine: false,
        };
        assert_eq!(choose_launch(&mac), HostLaunch::Launchd);
        // A systemd unit means nothing on macOS, and a plist nothing on Linux.
        assert_eq!(
            choose_launch(&HostEnv {
                systemd_unit: true,
                launchd_plist: false,
                ..mac
            }),
            HostLaunch::Detached
        );
        assert_eq!(
            choose_launch(&HostEnv {
                launchd_plist: true,
                systemd_unit: false,
                ..base
            }),
            HostLaunch::Detached
        );
    }

    #[test]
    fn the_app_hosts_its_engine_on_unix_unless_told_to_embed() {
        assert_eq!(
            HostPolicy::from_parts(true, None, true),
            HostPolicy::SpawnOrAttach
        );
        assert_eq!(
            HostPolicy::from_parts(true, Some("1"), true),
            HostPolicy::Embed
        );
        assert_eq!(
            HostPolicy::from_parts(true, Some("no"), true),
            HostPolicy::SpawnOrAttach
        );
        assert_eq!(HostPolicy::from_parts(false, None, true), HostPolicy::Embed);
        // A source build (unmanaged install) embeds: a host would outlive
        // rebuilds and keep running stale code.
        assert_eq!(HostPolicy::from_parts(true, None, false), HostPolicy::Embed);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn waiting_for_a_port_succeeds_once_something_listens_and_times_out_otherwise() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        wait_for_port(port, Duration::from_secs(2), None)
            .await
            .unwrap();
        drop(listener);
        let error = wait_for_port(port, Duration::from_millis(300), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("did not start listening"));
        // A host that dies at boot is reported at once, not after the timeout.
        let mut dead = std::process::Command::new("true").spawn().unwrap();
        let started = std::time::Instant::now();
        let error = wait_for_port(port, Duration::from_secs(20), Some(&mut dead))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exited before listening"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
