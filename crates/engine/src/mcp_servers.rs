//! Running copies and last-seen PSS for the MCP settings list.
//!
//! Read on request from this data dir's mcp-proxy markers. No polling, and
//! no walk of `/proc` beyond the groups those markers name. PSS is per copy:
//! the settings pane multiplies it by the running count.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use zeron_mcp_proxy::LiveGroup;

/// One server name's live copies under this data dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Runtime {
    pub running: u32,
    /// Average PSS of the copies we could measure, in KiB. `None` when
    /// nothing was measured, or on platforms without `smaps_rollup`.
    pub pss_kib: Option<u64>,
}

#[derive(Default)]
struct Copy {
    groups: BTreeSet<u32>,
    proxies: BTreeSet<u32>,
}

#[cfg(target_os = "linux")]
const TREE_CAP: usize = 64;

/// Live copies keyed by MCP server name. A run dir contributes one copy per
/// name, however many groups that copy recorded. Markers outside this data
/// dir are ignored.
pub(crate) fn by_server(data_dir: &Path) -> BTreeMap<String, Runtime> {
    let Ok(data_dir) = contained_dir(data_dir, data_dir) else {
        return BTreeMap::new();
    };
    let root = data_dir.join("run").join("mcp");
    let Ok(root) = contained_dir(&data_dir, &root) else {
        return BTreeMap::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return BTreeMap::new();
    };
    let mut per_name: BTreeMap<String, Vec<Copy>> = BTreeMap::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Ok(run) = contained_dir(&data_dir, &entry.path()) else {
            continue;
        };
        let mut copies: BTreeMap<String, Copy> = BTreeMap::new();
        for group in zeron_mcp_proxy::live_groups(&run) {
            let Some(name) = server_name(&group, &run) else {
                continue;
            };
            let copy = copies.entry(name.clone()).or_default();
            if let Some(proxy) = proxy_for(group.pgid, &name, &run) {
                copy.proxies.insert(proxy);
            }
            copy.groups.insert(group.pgid);
        }
        for (name, copy) in copies {
            if copy.groups.is_empty() {
                continue;
            }
            per_name.entry(name).or_default().push(copy);
        }
    }
    per_name
        .into_iter()
        .map(|(name, copies)| {
            (
                name,
                Runtime {
                    running: copies.len() as u32,
                    pss_kib: pss_average(&copies),
                },
            )
        })
        .collect()
}

/// `$CODEX_HOME/config.toml` or `~/.codex/config.toml` in production.
/// Unit tests pass `None` so a developer config is never opened.
pub(crate) fn codex_config() -> Option<PathBuf> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        zeron_harness::mcp_observed::default_codex_config()
    }
}

fn contained_dir(data_dir: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let data_dir = data_dir.canonicalize()?;
    let path = path.canonicalize()?;
    if path.starts_with(&data_dir) {
        Ok(path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "mcp run dir escapes the data dir",
        ))
    }
}

fn server_name(group: &LiveGroup, run_dir: &Path) -> Option<String> {
    if !group.server.is_empty() {
        return Some(group.server.clone());
    }
    server_from_parent(group.pgid, run_dir)
}

fn proxy_for(pgid: u32, server: &str, run_dir: &Path) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        proxy_pid(pgid, server, run_dir)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pgid, server, run_dir);
        None
    }
}

fn server_from_parent(pgid: u32, run_dir: &Path) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let ppid = ppid_of(pgid)?;
        let args = cmdline(ppid);
        let name = arg_value(&args, "--server")?;
        let run = arg_value(&args, "--run-dir")?;
        same_dir(Path::new(&run), run_dir).then_some(name)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pgid, run_dir);
        None
    }
}

fn pss_average(copies: &[Copy]) -> Option<u64> {
    let mut total = 0u64;
    let mut measured = 0u64;
    for copy in copies {
        if let Some(kib) = copy_pss(copy) {
            total = total.saturating_add(kib);
            measured += 1;
        }
    }
    (measured > 0).then_some(total / measured)
}

fn copy_pss(copy: &Copy) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux_pss(copy)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = copy;
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_pss(copy: &Copy) -> Option<u64> {
    let mut walked = BTreeSet::new();
    for leader in &copy.groups {
        if walked.len() >= TREE_CAP {
            break;
        }
        collect_tree(*leader, &mut walked, TREE_CAP);
    }
    let mut pids: BTreeSet<u32> = walked
        .into_iter()
        .filter(|pid| pgrp_of(*pid).is_some_and(|group| copy.groups.contains(&group)))
        .collect();
    pids.extend(copy.proxies.iter().copied());
    let mut sum = 0u64;
    let mut any = false;
    for pid in pids {
        if let Some(kib) = pss_kib(pid) {
            sum = sum.saturating_add(kib);
            any = true;
        }
    }
    any.then_some(sum)
}

#[cfg(target_os = "linux")]
fn collect_tree(root: u32, out: &mut BTreeSet<u32>, cap: usize) {
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        if out.len() >= cap || !out.insert(pid) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")) else {
            continue;
        };
        for token in text.split_whitespace() {
            if let Ok(child) = token.parse::<u32>() {
                stack.push(child);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn pss_kib(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    text.lines().find_map(|line| {
        line.strip_prefix("Pss:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

#[cfg(target_os = "linux")]
fn proxy_pid(pgid: u32, server: &str, run_dir: &Path) -> Option<u32> {
    let ppid = ppid_of(pgid)?;
    let args = cmdline(ppid);
    if arg_value(&args, "--server").as_deref() != Some(server) {
        return None;
    }
    let run = arg_value(&args, "--run-dir")?;
    same_dir(Path::new(&run), run_dir).then_some(ppid)
}

#[cfg(target_os = "linux")]
fn ppid_of(pid: u32) -> Option<u32> {
    stat_field(pid, 1)
}

#[cfg(target_os = "linux")]
fn pgrp_of(pid: u32) -> Option<u32> {
    stat_field(pid, 2)
}

#[cfg(target_os = "linux")]
fn stat_field(pid: u32, index: usize) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(index)?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn cmdline(pid: u32) -> Vec<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    bytes
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

#[cfg(target_os = "linux")]
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let prefixed = format!("{flag}=");
    for (index, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix(&prefixed) {
            return Some(value.to_owned());
        }
        if arg == flag {
            return args.get(index + 1).cloned();
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn same_dir(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{DiscoveredMcpServer, HarnessRegistry};
    use std::collections::BTreeMap;
    use zeron_harness::claude::mcp_capture::{ClaudeMcpServer, ClaudeMcpSnapshot, SnapshotStore};
    use zeron_harness::mcp_policy::McpClass;
    use zeron_proto::HarnessId;

    fn claude_server(
        name: &str,
        transport: &str,
        command: Option<&str>,
        scope: Option<&str>,
    ) -> ClaudeMcpServer {
        let mut config = serde_json::Map::new();
        if let Some(command) = command {
            config.insert("command".into(), command.into());
        }
        if transport == "http" {
            config.insert("url".into(), "https://example.test".into());
        }
        ClaudeMcpServer {
            name: name.into(),
            status: "connected".into(),
            config: (!config.is_empty()).then(|| serde_json::Value::Object(config)),
            scope: scope.map(str::to_owned),
            source: None,
            transport: Some(transport.into()),
            tools: None,
            extra: BTreeMap::new(),
        }
    }

    fn seed(data_dir: &Path, project_name: &str, servers: Vec<ClaudeMcpServer>) {
        let config_root = data_dir.join(format!("{project_name}-config"));
        let project = data_dir.join(project_name);
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        std::fs::write(config_root.join(".claude.json"), "{}").unwrap();
        let key =
            zeron_harness::claude::mcp_capture::fresh_capture_key(&config_root, &project, "cli")
                .unwrap();
        SnapshotStore::new(data_dir)
            .save(&ClaudeMcpSnapshot::new(key, servers))
            .unwrap();
    }

    fn by_name(servers: &[DiscoveredMcpServer]) -> BTreeMap<&str, &DiscoveredMcpServer> {
        servers
            .iter()
            .map(|server| (server.name.as_str(), server))
            .collect()
    }

    #[cfg(unix)]
    struct Reap(Option<std::process::Child>);
    #[cfg(unix)]
    impl Drop for Reap {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    #[cfg(unix)]
    fn sleep_group() -> Reap {
        use std::os::unix::process::CommandExt;
        Reap(Some(
            std::process::Command::new("sleep")
                .arg("60")
                .process_group(0)
                .spawn()
                .unwrap(),
        ))
    }

    #[cfg(unix)]
    #[test]
    fn mcp_settings_lists_seeded_servers_with_class_and_running_count() {
        let data = tempfile::tempdir().unwrap();
        seed(
            data.path(),
            "project",
            vec![
                claude_server("playwright", "stdio", Some("npx"), Some("user")),
                claude_server("reins", "stdio", Some("reins"), None),
                claude_server("docs", "http", None, Some("user")),
            ],
        );
        let run = data.path().join("run").join("mcp").join("run-1");
        let playwright = sleep_group();
        let playwright_again = sleep_group();
        zeron_mcp_proxy::record_group_marker(
            &run,
            playwright.0.as_ref().unwrap().id(),
            "playwright",
        )
        .unwrap();
        zeron_mcp_proxy::record_group_marker(
            &run,
            playwright_again.0.as_ref().unwrap().id(),
            "playwright",
        )
        .unwrap();
        std::fs::write(
            run.join("99"),
            r#"{"pgid":99,"start":"1","boot":"nope","server":"reins"}"#,
        )
        .unwrap();

        let other = tempfile::tempdir().unwrap();
        seed(
            other.path(),
            "foreign-project",
            vec![claude_server("foreign", "stdio", Some("reins"), None)],
        );
        let foreign_run = other.path().join("run").join("mcp").join("run-9");
        let foreign_playwright = sleep_group();
        let foreign_reins = sleep_group();
        zeron_mcp_proxy::record_group_marker(
            &foreign_run,
            foreign_playwright.0.as_ref().unwrap().id(),
            "playwright",
        )
        .unwrap();
        zeron_mcp_proxy::record_group_marker(
            &foreign_run,
            foreign_reins.0.as_ref().unwrap().id(),
            "reins",
        )
        .unwrap();

        let registry = HarnessRegistry::new();
        registry.load_prefs(data.path());
        let settings = registry.mcp_settings();
        assert!(settings.prefs.lazy);
        assert_eq!(settings.servers.len(), 3);
        let rows = by_name(&settings.servers);
        assert!(rows.get("foreign").is_none());
        let playwright = rows["playwright"];
        assert_eq!(playwright.harness, HarnessId::ClaudeCode);
        assert_eq!(playwright.class, McpClass::HeavyRuntime);
        assert_eq!(playwright.scope.as_deref(), Some("user"));
        assert_eq!(
            playwright.running, 1,
            "two groups in one run dir are one copy"
        );
        #[cfg(target_os = "linux")]
        assert!(playwright.last_seen_pss_kib.is_some_and(|kib| kib > 0));
        #[cfg(not(target_os = "linux"))]
        assert!(playwright.last_seen_pss_kib.is_none());
        let reins = rows["reins"];
        assert_eq!(reins.class, McpClass::NativeSmall);
        assert_eq!(reins.running, 0);
        assert!(reins.scope.is_none());
        assert!(reins.last_seen_pss_kib.is_none());
        let docs = rows["docs"];
        assert_eq!(docs.class, McpClass::Remote);
        assert_eq!(docs.running, 0);
        assert!(docs.last_seen_pss_kib.is_none());
    }

    #[test]
    fn mcp_settings_is_empty_without_snapshots() {
        let registry = HarnessRegistry::new();
        assert!(registry.mcp_settings().servers.is_empty());
        let data = tempfile::tempdir().unwrap();
        registry.load_prefs(data.path());
        let settings = registry.mcp_settings();
        assert!(settings.servers.is_empty());
        assert!(settings.prefs.lazy);
    }

    #[test]
    fn mcp_settings_ignores_another_data_dirs_servers() {
        let ours = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        seed(
            other.path(),
            "foreign-project",
            vec![claude_server("foreign", "stdio", Some("npx"), Some("user"))],
        );
        let registry = HarnessRegistry::new();
        registry.load_prefs(ours.path());
        assert!(registry.mcp_settings().servers.is_empty());
        let other_registry = HarnessRegistry::new();
        other_registry.load_prefs(other.path());
        let names: Vec<_> = other_registry
            .mcp_settings()
            .servers
            .iter()
            .map(|server| server.name.clone())
            .collect();
        assert_eq!(names, vec!["foreign"]);
    }
}
