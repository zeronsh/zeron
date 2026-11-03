//! Explicit Namespace/Boat control only. Registry observation never starts a machine.
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use zeron_harness::CancellationToken;
use zeron_proto::{Device, DeviceEnvironment};
use zeron_rpc::{LinkCache, PeerLiveness};

const OUTPUT_LIMIT: usize = 64 * 1024;
const CLI_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(180);
const WAKE_TIMEOUT: Duration = Duration::from_secs(180);
// ponytail: one explicit wake at a time; use per-device gates if parallel wakes are needed.
static GATE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The `crew-devbox-autostart` channel wake starts on the machine. A controller
/// lists only devices in its own deployment, so the machine runs the channel
/// that the devbox skill (ashler-platform `skills/devbox`) configures for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostChannel {
    Production,
    Staging,
    Canary,
}

impl HostChannel {
    pub(crate) fn for_controller(scope: &zeron_proto::CollaborationScope) -> Result<Self, String> {
        match (scope.project_id.as_str(), scope.deployment_id.as_deref()) {
            // Production leaves deployment selection to device bootstrap.
            ("ashler-production", _) => Ok(Self::Production),
            ("ashler-staging", Some("ashler-staging")) => Ok(Self::Staging),
            ("crew-upstream-canary", Some("crew-upstream-canary")) => Ok(Self::Canary),
            _ => Err("This Crew deployment has no devbox setup. Wake devboxes from Crew, Crew Staging or Crew Canary".into()),
        }
    }

    fn start_helper(self) -> &'static str {
        match self {
            Self::Production => "exec \"$HOME/.local/bin/crew-devbox-autostart\" production",
            Self::Staging => "exec \"$HOME/.local/bin/crew-devbox-autostart\" staging",
            Self::Canary => "exec \"$HOME/.local/bin/crew-devbox-autostart\" canary",
        }
    }
}

pub fn namespace_host() -> bool {
    cfg!(target_os = "linux") && Path::new("/.namespace/tasks").is_dir()
}

pub fn host_binding() -> Result<Option<(DeviceEnvironment, String)>, String> {
    resolve_host_binding(
        cfg!(target_os = "linux"),
        namespace_host(),
        std::env::var("BOAT_ID").ok().as_deref(),
        std::env::var("COMET_DEVBOX_PROVIDER").ok().as_deref(),
        std::env::var("COMET_DEVBOX_ID").ok().as_deref(),
    )
}

pub(crate) fn boat_binding() -> Result<Option<String>, String> {
    Ok(host_binding()?
        .and_then(|(provider, id)| (provider == DeviceEnvironment::Boat).then_some(id)))
}

fn resolve_host_binding(
    linux: bool,
    namespace: bool,
    native: Option<&str>,
    provider: Option<&str>,
    configured: Option<&str>,
) -> Result<Option<(DeviceEnvironment, String)>, String> {
    if native.is_none() && !namespace && provider.is_none() && configured.is_none() {
        return Ok(None);
    }
    let kind = match provider {
        Some("boat") if linux && !namespace && native == configured => DeviceEnvironment::Boat,
        Some("namespace") if linux && namespace && native.is_none() => DeviceEnvironment::Namespace,
        _ => return Err("Devbox identity is unverified: configure COMET_DEVBOX_PROVIDER and COMET_DEVBOX_ID on the matching native Linux host (Boat must match BOAT_ID)".into()),
    };
    let id = configured
        .filter(|id| kind.valid_devbox_id(id))
        .ok_or("Invalid immutable Devbox ID")?;
    Ok(Some((kind, id.to_owned())))
}

fn executable(provider: DeviceEnvironment) -> Result<PathBuf, String> {
    let name = if provider == DeviceEnvironment::Boat {
        "boat"
    } else {
        "devbox"
    };
    let mut candidates = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin").join(name));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
    ]);
    candidates
        .into_iter()
        .find(|path| {
            path.metadata().is_ok_and(|metadata| {
                if !metadata.is_file() {
                    return false;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    true
                }
            })
        })
        .ok_or_else(|| {
            format!("{name} CLI not found. Install the official CLI and sign in on this controller")
        })
}

#[derive(Debug, PartialEq, Eq)]
enum WakeAction {
    Resume,
    Connect,
}

fn wake_action(output: &[u8], expected: &str) -> Result<WakeAction, String> {
    let value: serde_json::Value =
        serde_json::from_slice(output).map_err(|_| "Boat returned invalid machine status")?;
    let sandbox = value.get("sandbox").unwrap_or(&value);
    if value.get("ok") == Some(&serde_json::Value::Bool(false))
        || sandbox.get("id").and_then(serde_json::Value::as_str) != Some(expected)
    {
        return Err("Boat did not confirm the registered immutable sandbox ID; no machine operation was issued".into());
    }
    match sandbox.get("state").and_then(serde_json::Value::as_str) {
        Some("archived" | "stopped") => Ok(WakeAction::Resume),
        Some("ready" | "running" | "idle" | "restoring" | "provisioning" | "provisioned" | "cloning" | "init") => Ok(WakeAction::Connect),
        Some("archiving" | "stopping") => Err("Boat is still saving this machine. Wait for its snapshot to finish before retrying Wake".into()),
        _ => Err("Boat cannot start this machine in its current state. Crew will not recover, replace or create a machine".into()),
    }
}

fn provider_failure(stdout: &[u8], stderr: &[u8]) -> String {
    let contains = |word: &str| {
        [stdout, stderr].iter().any(|bytes| {
            bytes
                .windows(word.len())
                .any(|part| part.eq_ignore_ascii_case(word.as_bytes()))
        })
    };
    if [
        "unauthorized",
        "unauthenticated",
        "login",
        "credential",
        "permission denied",
    ]
    .iter()
    .any(|word| contains(word))
    {
        "Devbox authentication failed. Sign in on this controller and verify access to this exact devbox".into()
    } else if [
        "billing_required",
        "limit_reached",
        "rate_limited",
        "trial_machine_class_not_allowed",
        "org_suspended",
    ]
    .iter()
    .any(|word| contains(word))
    {
        "Devbox refused the operation because of billing, usage or permissions. No account change or replacement was attempted".into()
    } else {
        // Provider errors may contain auth URLs, environment values or remote logs.
        "Devbox command failed. Inspect this machine and its existing crew-devbox-autostart helper; no recovery or replacement was attempted".into()
    }
}

struct OwnedCommand {
    child: tokio::process::Child,
    #[cfg(unix)]
    group: Option<u32>,
}

impl OwnedCommand {
    fn spawn(command: &mut Command) -> std::io::Result<Self> {
        command.kill_on_drop(!cfg!(unix));
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        let child = command.spawn()?;
        Ok(Self {
            #[cfg(unix)]
            group: child.id(),
            child,
        })
    }
    fn kill_group(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // The unreaped owned leader reserves this group ID until cleanup.
            unsafe {
                libc::killpg(group as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    fn exited(&mut self) -> std::io::Result<bool> {
        #[cfg(unix)]
        {
            let Some(group) = self.group else {
                return Ok(true);
            };
            let mut status = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    group as libc::id_t,
                    &mut status,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    self.group = None;
                }
                return Err(error);
            }
            Ok(unsafe { status.si_pid() } != 0)
        }
        #[cfg(not(unix))]
        {
            self.child.try_wait().map(|status| status.is_some())
        }
    }
    async fn wait_unreaped(&mut self) -> std::io::Result<()> {
        while !self.exited()? {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok(())
    }
    async fn stop(&mut self) {
        self.kill_group();
        #[cfg(not(unix))]
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}
impl Drop for OwnedCommand {
    fn drop(&mut self) {
        self.kill_group();
    }
}

async fn capture(mut stream: impl AsyncRead + Unpin) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut excess = false;
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            return Ok((output, excess));
        }
        let keep = count.min(OUTPUT_LIMIT - output.len());
        output.extend_from_slice(&buffer[..keep]);
        excess |= keep < count;
    }
}

async fn run_cli(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), String> {
    if cancel.is_cancelled() { return Err("Devbox operation cancelled".into()); }
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process =
        OwnedCommand::spawn(&mut command).map_err(|_| "Could not start Devbox CLI")?;
    let stdout = process.child.stdout.take().expect("piped stdout");
    let stderr = process.child.stderr.take().expect("piped stderr");
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Devbox operation cancelled".to_owned()),
        result = tokio::time::timeout(timeout, async {
            tokio::try_join!(async {
                process.wait_unreaped().await?;
                process.kill_group();
                process.child.wait().await
            }, capture(stdout), capture(stderr))
        }) => match result {
            Ok(Ok((status, (out, out_excess), (err, err_excess)))) => {
                if out_excess || err_excess { Err("Devbox CLI output exceeded its 64 KiB stream limit".into()) }
                else { Ok((status, out, err)) }
            }
            Ok(Err(_)) => Err("Could not read Devbox CLI result".into()),
            Err(_) => Err("Devbox operation timed out; inspect its existing state before retrying".into()),
        }
    };
    if result.is_err() {
        process.stop().await;
    }
    result
}

pub(crate) async fn run_boat_extend(id: &str, cancel: &CancellationToken) -> Result<(), String> {
    if !DeviceEnvironment::Boat.valid_devbox_id(id) {
        return Err("Invalid Boat sandbox ID".into());
    }
    let (status, out, err) = run_cli(
        &executable(DeviceEnvironment::Boat)?,
        &["--no-update", "--json", "extend", id, "--ttl", "3600"],
        CLI_TIMEOUT,
        cancel,
    )
    .await?;
    if status.success() {
        Ok(())
    } else {
        Err(provider_failure(&out, &err))
    }
}

async fn prepare_host(
    executable: &Path,
    provider: DeviceEnvironment,
    id: &str,
    channel: HostChannel,
    cancel: &CancellationToken,
    authorize: &(impl Fn() -> Result<(), String> + Sync),
) -> Result<(), String> {
    authorize()?;
    if provider == DeviceEnvironment::Namespace {
        let (status, out, err) = run_cli(
            executable,
            &["exec", id, "--", "sh", "-c", channel.start_helper()],
            START_TIMEOUT,
            cancel,
        )
        .await?;
        return if status.success() {
            Ok(())
        } else {
            Err(provider_failure(&out, &err))
        };
    }
    if !DeviceEnvironment::Boat.valid_devbox_id(id) {
        return Err("Invalid Boat sandbox ID".into());
    }
    let (status, out, err) = run_cli(
        executable,
        &["--no-update", "--json", "info", id],
        CLI_TIMEOUT,
        cancel,
    )
    .await?;
    if !status.success() {
        return Err(provider_failure(&out, &err));
    }
    if wake_action(&out, id)? == WakeAction::Resume {
        authorize()?;
        let (status, out, err) = run_cli(
            executable,
            &["--no-update", "--json", "resume", id, "--ttl", "3600"],
            START_TIMEOUT,
            cancel,
        )
        .await?;
        if !status.success() {
            return Err(provider_failure(&out, &err));
        }
    }
    authorize()?;
    // A restarted Boat agent can drop BOAT_ID from exec sessions. Boat routed
    // this exec to `id`, so supply only that missing value; Boat's own wins.
    let start = format!("export BOAT_ID=\"${{BOAT_ID:-{id}}}\"; {}", channel.start_helper());
    let (status, out, err) = run_cli(
        executable,
        &["--no-update", "--json", "exec", id, "--timeout", "180", &start],
        START_TIMEOUT,
        cancel,
    )
    .await?;
    if status.success() {
        Ok(())
    } else {
        Err(provider_failure(&out, &err))
    }
}

fn verify_peer<'a>(
    value: &'a serde_json::Value,
    device_id: &str,
    provider: DeviceEnvironment,
    box_id: &str,
) -> Result<&'a [serde_json::Value], String> {
    if value.get("deviceId").and_then(serde_json::Value::as_str) != Some(device_id)
        || value
            .get("workspaceScope")
            .and_then(serde_json::Value::as_str)
            != Some("synced")
        || value.get("devboxProvider") != Some(&serde_json::json!(provider))
        || value.get("devboxId").and_then(serde_json::Value::as_str) != Some(box_id)
    {
        return Err(
            "The responding peer does not match the selected synced device and native Devbox"
                .into(),
        );
    }
    let capabilities = value
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .filter(|items| items.len() <= 128)
        .ok_or("Peer capabilities are unavailable")?;
    if capabilities
        .iter()
        .any(|item| item.as_str().is_none_or(|text| text.len() > 128))
    {
        return Err("Peer returned invalid capabilities".into());
    }
    Ok(capabilities)
}

async fn wait_for_readiness_change(
    devices: &mut tokio::sync::watch::Receiver<Vec<Device>>,
    online: &mut tokio::sync::broadcast::Receiver<()>,
) -> Result<(), String> {
    tokio::select! {
        changed = devices.changed() => changed.map_err(|_| "Device registry closed during wake".into()),
        event = online.recv() => match event {
            Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Ok(()),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => Err("Peer connectivity notifications closed".into()),
        },
    }
}

async fn await_peer(
    workspace: &crate::WorkspaceHost,
    links: &Arc<LinkCache>,
    device_id: &str,
    provider: DeviceEnvironment,
    box_id: &str,
) -> Result<Device, String> {
    let mut devices = workspace.watch_devices();
    let mut online = zeron_sync::wake::subscribe_online();
    tokio::time::timeout(WAKE_TIMEOUT, async {
        loop {
            {
                let rows = devices.borrow_and_update();
                let device = rows.iter().find(|device| device.id == device_id)
                    .ok_or("The selected device is no longer registered")?;
                if device.devbox_binding() != Some((provider, box_id)) { return Err("The selected Devbox binding changed during wake".into()); }
            }
            if workspace.peer_liveness(device_id) != PeerLiveness::Dark {
                match links.client(device_id).await {
                    Ok(client) => {
                        match tokio::time::timeout(Duration::from_secs(15), client.call(zeron_rpc::methods::ENGINE_INFO, serde_json::json!({}))).await {
                            Ok(Ok(info)) => {
                                let capabilities = verify_peer(&info, device_id, provider, box_id)?;
                                let current = devices.borrow_and_update().iter().find(|row| row.id == device_id).cloned()
                                    .ok_or("The selected device disappeared during identity verification")?;
                                if current.devbox_binding() != Some((provider, box_id)) {
                                    return Err("The selected Devbox binding changed during identity verification".into());
                                }
                                if current.capabilities.len() == capabilities.len() && current.capabilities.iter().zip(capabilities)
                                    .all(|(local, remote)| remote.as_str() == Some(local.as_str())) { return Ok(current); }
                            }
                            _ => {
                                links.invalidate(device_id);
                                tracing::debug!(device = %device_id, "Devbox identity transport unavailable; waiting for presence change");
                            }
                        }
                    }
                    Err(error) => tracing::debug!(device = %device_id, %error, "Devbox peer not ready; waiting for registry/presence change"),
                }
            }
            // The normal registry/presence stream drives readiness; never poll the provider.
            wait_for_readiness_change(&mut devices, &mut online).await?;
        }
    }).await.map_err(|_| "Devbox started but its exact Crew peer did not become ready; the original draft was not submitted".to_owned())?
}

pub(crate) async fn wake(
    workspace: &crate::WorkspaceHost,
    links: &Arc<LinkCache>,
    device_id: &str,
    channel: HostChannel,
    authorize: &(impl Fn() -> Result<(), String> + Sync),
) -> Result<Device, String> {
    // This deadline includes gate contention, provider operations and exact-peer
    // readiness. Dropping an in-flight provider future kills only its owned group.
    tokio::time::timeout(WAKE_TIMEOUT, wake_inner(workspace, links, device_id, channel, authorize)).await
        .map_err(|_| "Devbox wake timed out; inspect its existing state before retrying. The original draft was not submitted".to_owned())?
}

async fn wake_inner(
    workspace: &crate::WorkspaceHost,
    links: &Arc<LinkCache>,
    device_id: &str,
    channel: HostChannel,
    authorize: &(impl Fn() -> Result<(), String> + Sync),
) -> Result<Device, String> {
    if !cfg!(unix) {
        return Err("Devbox wake requires a macOS or Linux controller".into());
    }
    let device = workspace
        .read_devices()
        .map_err(|_| "Could not read the device registry")?
        .into_iter()
        .find(|device| device.id == device_id)
        .ok_or("Unknown selected device")?;
    let (provider, id) = device
        .devbox_binding()
        .ok_or("The selected device has no valid immutable Devbox binding")?;
    let _guard = GATE.lock().await;
    // Consent belongs to the original binding and live controller authority,
    // revalidated after gate contention and before every provider command.
    let authorize_host = || {
        authorize()?;
        let current = workspace.read_devices().map_err(|_| "Could not read the device registry")?;
        if current.iter().find(|row| row.id == device_id).and_then(Device::devbox_binding)
            != Some((provider, id)) {
            return Err("The selected Devbox binding changed during wake".into());
        }
        Ok(())
    };
    authorize_host()?;
    let stop = CancellationToken::new();
    prepare_host(&executable(provider)?, provider, id, channel, &stop, &authorize_host).await?;
    links.invalidate(device_id);
    await_peer(workspace, links, device_id, provider, id).await
}

#[cfg(test)]
#[path = "devbox_tests.rs"]
mod tests;
