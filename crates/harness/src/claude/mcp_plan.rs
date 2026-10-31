//! Launch interception leaves Claude's merge, plugin identity and permissions intact.
use super::mcp_capture::{ClaudeMcpSnapshot, FingerprintInputs, SnapshotStore};
use crate::mcp_policy::{self, McpClass, McpServerFact};
use crate::process::Command;
use std::{
    io,
    path::{Path, PathBuf},
};
use zeron_mcp_proxy::shim::{self, Launch, Server};
use zeron_proto::{McpMode, McpPlan, RunRequest};

/// A guard belongs to the launched runtime, including failed spawns and relaunches.
/// Numeric server-group markers live beside (never inside) the shim directory.
pub(super) struct ShimRun {
    dir: PathBuf,
    run_dir: PathBuf,
    cleaned: bool,
}
impl ShimRun {
    pub async fn cleanup(mut self) {
        let _ = zeron_mcp_proxy::cleanup_run_dir(&self.run_dir).await;
        let _ = std::fs::remove_dir_all(&self.dir);
        self.cleaned = true;
    }
}
impl Drop for ShimRun {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        // Also cover cancellation/panic before the normal awaited teardown.
        if !self.cleaned
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let dir = self.run_dir.clone();
            runtime.spawn(async move {
                let _ = zeron_mcp_proxy::cleanup_run_dir(&dir).await;
            });
        }
    }
}

fn servers(snapshot: &ClaudeMcpSnapshot, plan: &McpPlan, path: &std::ffi::OsStr) -> Vec<Server> {
    snapshot
        .servers
        .iter()
        .filter_map(|row| {
            let config = row.config.as_ref()?;
            if row.transport.as_deref() != Some("stdio") || row.status != "connected" {
                return None;
            }
            let command = config.get("command")?.as_str()?;
            // Relative paths with a slash and absolute commands bypass PATH lookup.
            if command.is_empty() || command.contains(['/', '\\']) {
                return None;
            }
            let args = config
                .get("args")?
                .as_array()?
                .iter()
                .map(|a| a.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?;
            let channel = row.extra.iter().any(|(k, v)| {
                (k == "channelCapabilities" || k == "capabilities") && declares_channel(v)
            });
            let fact = McpServerFact {
                name: row.name.clone(),
                transport: "stdio".into(),
                command: command.into(),
                npx_resolved: false,
                channel,
                scope: row.scope.clone().unwrap_or_default(),
            };
            let class = mcp_policy::classify(&fact);
            let mode = if class == McpClass::HeavyRuntime {
                plan.modes
                    .get(&row.name)
                    .copied()
                    .unwrap_or(plan.default_mode)
            } else {
                McpMode::Eager
            };
            let mode = match mode {
                McpMode::Lazy => "lazy",
                McpMode::LazyKeepAlive => "lazy-keep-alive",
                McpMode::Eager => "eager",
                McpMode::Off => "off",
            };
            Some(Server {
                name: row.name.clone(),
                command: command.into(),
                resolved_command: zeron_mcp_proxy::cache::executable(command, Some(path)),
                args,
                mode: mode.into(),
                channel,
                owner_legacy: channel,
            })
        })
        .collect()
}
fn declares_channel(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => {
            m.keys().any(|k| mcp_policy::declares_channel_capability(k))
                || m.values().any(declares_channel)
        }
        _ => false,
    }
}

/// No shim on unsupported platforms, a disabled plan, or any freshness/setup error.
/// Small-helper absence in source/old packages keeps the normal eager launch.
#[cfg(target_os = "linux")]
pub(super) fn prepare(
    cmd: &mut Command,
    exe: &Path,
    request: &RunRequest,
    title: bool,
) -> Option<ShimRun> {
    if title
        || request.env.get("ZERON_MCP_LAZY").map(String::as_str) == Some("0")
        || std::env::var("ZERON_MCP_LAZY").as_deref() == Ok("0")
    {
        return None;
    }
    let plan = request.mcp_plan.as_ref()?;
    let inputs = FingerprintInputs::for_run(
        (!request.cwd.is_empty()).then(|| Path::new(&request.cwd)),
        &request.env,
    )
    .ok()?;
    let key = inputs.key(exe).ok()?;
    let run_dir = PathBuf::from(&plan.run_dir);
    // The plan's documented layout identifies storage even with a run-specific env.
    let data_dir = run_dir.parent()?.parent()?.parent()?.to_owned();
    if !run_dir.is_absolute() || plan.run_dir.is_empty() {
        return None;
    }
    let snapshot = SnapshotStore::new(&data_dir).load(&key)?;
    let path = cmd
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == "PATH")
        .and_then(|(_, v)| v.map(|v| v.to_owned()))
        .or_else(|| std::env::var_os("PATH"))?;
    let rows = servers(&snapshot, plan, &path);
    if !rows
        .iter()
        .any(|s| !s.channel && matches!(s.mode.as_str(), "lazy" | "lazy-keep-alive"))
    {
        return None;
    }
    let proxy = Path::new(&plan.proxy_command);
    let helper = if proxy.file_name()? == "zeron-mcp-proxy" {
        proxy.to_owned()
    } else {
        proxy.parent()?.join("zeron-mcp-proxy")
    }
    .canonicalize()
    .ok()?;
    // Old/source builds may leave an H1 helper beside a new app. Do not
    // install multi-call entries unless this exact helper supports them.
    let support = std::process::Command::new(&helper)
        .arg("--shim-version")
        .output()
        .ok()?;
    if !support.status.success() || support.stdout != b"1\n" {
        return None;
    }
    let dir = run_dir.join(format!("shim-{}", uuid::Uuid::new_v4()));
    let guard = ShimRun {
        dir: dir.clone(),
        run_dir: run_dir.clone(),
        cleaned: false,
    };
    let result = (|| -> io::Result<()> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir)?;
        let launch = Launch {
            version: 1,
            key: key.digest(),
            parent_exe: exe.canonicalize()?,
            parent_identity: zeron_mcp_proxy::cache::binary_id(&exe.canonicalize()?),
            data_dir,
            run_dir,
            idle_minutes: plan.idle_minutes,
            servers: rows,
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        serde_json::to_writer(options.open(dir.join(shim::MANIFEST))?, &launch)?;
        // Derive entries from policy-selected snapshot commands, including explicit
        // lazy overrides. Do not intercept every launcher in every session.
        for command in launch
            .servers
            .iter()
            .filter(|s| !s.channel && matches!(s.mode.as_str(), "lazy" | "lazy-keep-alive"))
            .map(|s| &s.command)
            .collect::<std::collections::BTreeSet<_>>()
        {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&helper, dir.join(command))?;
        }
        let composed =
            std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&path)))
                .map_err(io::Error::other)?;
        // Apply only after ALL fallible setup: a fallback must not inherit half a shim.
        cmd.env("PATH", composed)
            .env(shim::DIR_ENV, &dir)
            .env(shim::KEY_ENV, &launch.key)
            .env(shim::PARENT_ENV, &launch.parent_exe);
        Ok(())
    })();
    result.ok().map(|_| guard)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn prepare(
    _cmd: &mut Command,
    _exe: &Path,
    _request: &RunRequest,
    _title: bool,
) -> Option<ShimRun> {
    None
}

pub(super) fn same_plan(a: &Option<McpPlan>, b: &Option<McpPlan>) -> bool {
    // A new run directory does not change policy; modes and ownership do.
    match (a, b) {
        (Some(a), Some(b)) => {
            a.proxy_command == b.proxy_command
                && a.idle_minutes == b.idle_minutes
                && a.default_mode == b.default_mode
                && a.modes == b.modes
                && a.drop_channels == b.drop_channels
                && a.worker == b.worker
        }
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::mcp_capture::{CaptureKey, parse_status};
    use super::*;
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn prepare_requires_fresh_snapshot_enabled_plan_and_helper_and_cleans_run() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let config = root.path().join("config");
        let data = root.path().join("data");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        let exe = root.path().join("claude");
        let helper = root.path().join("zeron-mcp-proxy");
        std::fs::write(&exe, "fixture").unwrap();
        std::fs::write(&helper, "#!/bin/sh\necho 1\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut request: RunRequest = serde_json::from_value(serde_json::json!({
            "prompt":"", "cwd":project, "sandbox":"workspace-write"
        }))
        .unwrap();
        request.env.insert(
            "CLAUDE_CONFIG_DIR".into(),
            config.to_string_lossy().into_owned(),
        );
        request
            .env
            .insert("HOME".into(), root.path().to_string_lossy().into_owned());
        let mut plan = McpPlan {
            proxy_command: helper.to_string_lossy().into_owned(),
            run_dir: data.join("run/mcp/test").to_string_lossy().into_owned(),
            ..McpPlan::default()
        };
        request.mcp_plan = Some(plan.clone());
        let command = || {
            let mut c = Command::new(&exe);
            c.env("PATH", "/usr/bin:/bin");
            c
        };
        let mut cmd = command();
        assert!(prepare(&mut cmd, &exe, &request, false).is_none());
        let frame = serde_json::json!({"type":"control_response","response":{"subtype":"success","response":{"mcpServers":[
            {"name":"srv","status":"connected","transport":"stdio","config":{"command":"node","args":["srv.js"]}}
        ]}}});
        let key = FingerprintInputs::for_run(Some(&project), &request.env)
            .unwrap()
            .key(&exe)
            .unwrap();
        SnapshotStore::new(&data)
            .save(&ClaudeMcpSnapshot::new(key, parse_status(&frame).unwrap()))
            .unwrap();
        let guard = prepare(&mut cmd, &exe, &request, false).expect("fresh snapshot enables shim");
        let dir = guard.dir.clone();
        assert!(dir.join("node").is_symlink());
        assert_eq!(
            std::fs::metadata(dir.join(shim::MANIFEST))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let path = cmd
            .as_std()
            .get_envs()
            .find(|(k, _)| *k == "PATH")
            .unwrap()
            .1
            .unwrap();
        assert_eq!(std::env::split_paths(path).next().unwrap(), dir);
        guard.cleanup().await;
        assert!(!dir.exists());
        assert!(prepare(&mut command(), &exe, &request, true).is_none());
        request.env.insert("ZERON_MCP_LAZY".into(), "0".into());
        assert!(prepare(&mut command(), &exe, &request, false).is_none());
        request.env.remove("ZERON_MCP_LAZY");
        request.mcp_plan = None;
        assert!(prepare(&mut command(), &exe, &request, false).is_none());
        plan.proxy_command = root
            .path()
            .join("missing/zeron")
            .to_string_lossy()
            .into_owned();
        request.mcp_plan = Some(plan.clone());
        assert!(prepare(&mut command(), &exe, &request, false).is_none());
        plan.proxy_command = helper.to_string_lossy().into_owned();
        request.mcp_plan = Some(plan);
        std::fs::write(&helper, "#!/bin/sh\nexit 0\n").unwrap();
        assert!(prepare(&mut command(), &exe, &request, false).is_none());
        std::fs::write(&helper, "#!/bin/sh\necho 1\n").unwrap();
        std::fs::write(
            config.join("settings.json"),
            r#"{"enabledPlugins":{"changed":true}}"#,
        )
        .unwrap();
        assert!(prepare(&mut command(), &exe, &request, false).is_none());
    }

    #[test]
    fn snapshot_modes_keep_absolute_remote_channels_and_owners_eager() {
        let frame = serde_json::json!({"type":"control_response", "response":{"subtype":"success", "response":{"mcpServers":[
            {"name":"lazy", "status":"connected","transport":"stdio","config":{"command":"node","args":["srv.js"]}},
            {"name":"absolute", "status":"connected","transport":"stdio","config":{"command":"/usr/bin/node","args":[]}},
            {"name":"remote", "status":"connected","transport":"http","config":{"url":"http://localhost"}},
            {"name":"channel", "status":"connected","transport":"stdio","channelCapabilities":{"claude/channel":{}},"config":{"command":"node","args":["channel.js"]}}
        ]}}});
        let snapshot = ClaudeMcpSnapshot::new(
            CaptureKey {
                config_root: "c".into(),
                project_dir: "p".into(),
                fingerprint: "f".into(),
                cli_binary_identity: "i".into(),
            },
            parse_status(&frame).unwrap(),
        );
        let mut plan = McpPlan::default();
        plan.modes.insert("channel".into(), McpMode::Lazy);
        let rows = servers(&snapshot, &plan, std::ffi::OsStr::new("/usr/bin"));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].mode, "lazy");
        assert!(rows[1].channel && rows[1].owner_legacy);
        plan.modes.insert("lazy".into(), McpMode::Eager);
        assert_eq!(
            servers(&snapshot, &plan, std::ffi::OsStr::new("/usr/bin"))[0].mode,
            "eager"
        );
    }
    #[test]
    fn plan_changes_require_relaunch_but_run_directory_does_not() {
        let a = Some(McpPlan::default());
        let mut b = a.clone();
        b.as_mut().unwrap().run_dir = "another".into();
        assert!(same_plan(&a, &b));
        b.as_mut().unwrap().drop_channels = true;
        assert!(!same_plan(&a, &b));
        assert!(!same_plan(&a, &None));
        b = a.clone();
        b.as_mut()
            .unwrap()
            .modes
            .insert("srv".into(), McpMode::Eager);
        assert!(!same_plan(&a, &b));
    }
}
