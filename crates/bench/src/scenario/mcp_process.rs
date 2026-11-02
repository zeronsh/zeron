//! Linux process accounting for synthetic MCP launch roots.
//! The roots are selected by executable identity and exact synthetic script
//! argument, not mutable process titles; their entire descendant PSS is summed.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Context, Result, ensure};

#[derive(Debug, PartialEq, Eq)]
pub struct McpProcesses {
    pub servers: usize,
    pub server_pids: Vec<u32>,
    pub roots: Vec<u32>,
    pub pss_kib: u64,
}

/// Count the real synthetic servers under Claude and sum their MCP subtrees,
/// including idle proxy roots. A disappearing process fails the sample.
pub fn measure(
    claude_pid: u32,
    script: &Path,
    runtime: &Path,
    proxy: Option<&Path>,
) -> Result<McpProcesses> {
    let mut parents = BTreeMap::new();
    for ent in fs::read_dir("/proc")? {
        let ent = ent?;
        let Ok(pid) = ent.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(ent.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            continue;
        };
        let mut fields = fields.split_whitespace();
        let Some(state) = fields.next() else { continue };
        let Some(ppid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if state != "Z" {
            parents.insert(pid, ppid);
        }
    }
    ensure!(parents.contains_key(&claude_pid), "Claude process exited");
    let is_descendant = |mut pid: u32| {
        for _ in 0..256 {
            let Some(&parent) = parents.get(&pid) else {
                return false;
            };
            if parent == claude_pid {
                return true;
            }
            if parent == 0 || parent == pid {
                return false;
            }
            pid = parent;
        }
        false
    };
    let canonical_proxy = proxy.map(fs::canonicalize).transpose()?;
    let canonical_runtime = fs::canonicalize(runtime)?;
    let mut seeds = BTreeSet::new();
    let mut server_pids = Vec::new();
    for &pid in parents
        .keys()
        .filter(|&&p| p != claude_pid && is_descendant(p))
    {
        let base = format!("/proc/{pid}");
        let Ok(args) = fs::read(format!("{base}/cmdline")) else {
            continue;
        };
        let exe = fs::read_link(format!("{base}/exe")).ok();
        let server = exe.as_deref() == Some(canonical_runtime.as_path())
            && args
                .split(|b| *b == 0)
                .any(|arg| arg == script.as_os_str().as_bytes());
        if server {
            server_pids.push(pid);
            seeds.insert(pid);
        }
        if let Some(proxy) = &canonical_proxy
            && exe.as_deref() == Some(proxy.as_path())
        {
            seeds.insert(pid);
        }
    }
    let mut roots = BTreeSet::new();
    for mut pid in seeds {
        loop {
            let parent = parents[&pid];
            if parent == claude_pid {
                roots.insert(pid);
                break;
            }
            pid = parent;
        }
    }
    let mut members = roots.clone();
    for &pid in parents.keys() {
        let mut at = pid;
        for _ in 0..256 {
            if roots.contains(&at) {
                members.insert(pid);
                break;
            }
            let Some(&parent) = parents.get(&at) else {
                break;
            };
            if parent == claude_pid || parent == 0 || parent == at {
                break;
            }
            at = parent;
        }
    }
    let mut pss_kib = 0;
    for pid in members {
        let text = fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            .with_context(|| format!("read MCP process {pid} PSS"))?;
        let value: u64 = text
            .lines()
            .find_map(|line| {
                line.strip_prefix("Pss:")
                    .and_then(|s| s.split_whitespace().next())
            })
            .context("missing Pss")?
            .parse()?;
        pss_kib += value;
    }
    Ok(McpProcesses {
        servers: server_pids.len(),
        server_pids,
        roots: roots.into_iter().collect(),
        pss_kib,
    })
}

/// Capture a live descendant set before teardown so reparented children can
/// still be checked after the engine exits.
pub fn descendant_pids(root: u32) -> Result<Vec<u32>> {
    let mut pending = vec![root];
    let mut found = BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if !found.insert(pid) {
            continue;
        }
        let tasks = match fs::read_dir(format!("/proc/{pid}/task")) {
            Ok(tasks) => tasks,
            Err(err) if pid != root && err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("read process {pid} tasks")),
        };
        for task in tasks {
            let task = task?;
            match fs::read_to_string(task.path().join("children")) {
                Ok(children) => pending.extend(
                    children
                        .split_whitespace()
                        .filter_map(|v| v.parse::<u32>().ok()),
                ),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    found.remove(&root);
    Ok(found.into_iter().collect())
}

/// Sum one live process and its descendants; never count a vanished process as zero.
pub fn tree_pss_kib(root: u32) -> Result<u64> {
    let mut pending = vec![root];
    let mut visited = BTreeSet::new();
    let mut pss = 0u64;
    while let Some(pid) = pending.pop() {
        if !visited.insert(pid) {
            continue;
        }
        let base = format!("/proc/{pid}");
        let rollup = fs::read_to_string(format!("{base}/smaps_rollup"))
            .with_context(|| format!("read process {pid} PSS"))?;
        let value: u64 = rollup
            .lines()
            .find_map(|line| {
                line.strip_prefix("Pss:")
                    .and_then(|s| s.split_whitespace().next())
            })
            .context("missing Pss")?
            .parse()?;
        pss += value;
        for task in fs::read_dir(format!("{base}/task"))? {
            let task = task?;
            match fs::read_to_string(task.path().join("children")) {
                Ok(children) => pending.extend(
                    children
                        .split_whitespace()
                        .filter_map(|v| v.parse::<u32>().ok()),
                ),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    Ok(pss)
}

/// Real Claude descendants of an engine, selected by executable identity.
pub fn claude_descendants(engine: u32, executable: &Path) -> Result<Vec<u32>> {
    let executable = fs::canonicalize(executable)?;
    let mut parents = BTreeMap::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            continue;
        };
        let mut fields = fields.split_whitespace();
        if fields.next() == Some("Z") {
            continue;
        }
        if let Some(ppid) = fields.next().and_then(|p| p.parse::<u32>().ok()) {
            parents.insert(pid, ppid);
        }
    }
    ensure!(parents.contains_key(&engine), "engine exited");
    let mut result = Vec::new();
    for &pid in parents.keys() {
        if fs::read_link(format!("/proc/{pid}/exe")).ok().as_deref() != Some(executable.as_path()) {
            continue;
        }
        let mut current = pid;
        for _ in 0..256 {
            let Some(&parent) = parents.get(&current) else {
                break;
            };
            if parent == engine {
                result.push(pid);
                break;
            }
            if parent == 0 || parent == current {
                break;
            }
            current = parent;
        }
    }
    Ok(result)
}

/// Approximate the kernel process start tick on CLOCK_MONOTONIC (one tick resolution).
pub fn started_mono_ns(pid: u32) -> Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields = stat.rsplit_once(')').context("malformed stat")?.1;
    let ticks: u64 = fields
        .split_whitespace()
        .nth(19)
        .context("missing start tick")?
        .parse()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    ensure!(hz > 0, "invalid kernel clock tick rate");
    let mut boot = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    ensure!(
        unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut boot) } == 0,
        "CLOCK_BOOTTIME unavailable"
    );
    let boot_now = (boot.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(boot.tv_nsec as u64);
    let offset = boot_now.saturating_sub(crate::mono_now());
    Ok(((ticks as u128 * 1_000_000_000) / hz as u128).min(u64::MAX as u128) as u64)
        .map(|t| t.saturating_sub(offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn process_start_is_in_the_monotonic_clock_domain() {
        let start = started_mono_ns(std::process::id()).unwrap();
        let now = crate::mono_now();
        assert!(start <= now && now - start < 60_000_000_000);
    }

    #[test]
    fn counts_synthetic_server_and_its_resident_memory() {
        let root =
            std::path::PathBuf::from(std::env::var_os("TMPDIR").expect("scratch TMPDIR")).join(
                format!("mcp-pss-{}-{}", std::process::id(), crate::mono_now()),
            );
        fs::create_dir(&root).unwrap();
        let home = crate::fixture::claude_home::setup(&root).unwrap();
        let script = home.join("synthetic-mcp.js");
        let mut child = Command::new("/usr/bin/node")
            .env_clear()
            .arg(&script)
            .arg("entry")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let sample = measure(
                std::process::id(),
                &script,
                Path::new("/usr/bin/node"),
                None,
            )
            .unwrap();
            if sample.servers == 1 && sample.pss_kib >= 150 * 1024 {
                assert_eq!(sample.roots, [child.id()]);
                assert!(sample.pss_kib < 250 * 1024);
                assert!(tree_pss_kib(child.id()).unwrap() >= 150 * 1024);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "synthetic server did not reach its expected footprint: {sample:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let stopped = measure(
            std::process::id(),
            &script,
            Path::new("/usr/bin/node"),
            None,
        )
        .unwrap();
        assert_eq!(stopped.servers, 0);
        assert_eq!(stopped.pss_kib, 0);
        let mut proxy = Command::new("/usr/bin/python3")
            .env_clear()
            .args(["-c", "import time; time.sleep(10)"])
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let proxy_sample = measure(
            std::process::id(),
            &script,
            Path::new("/usr/bin/node"),
            Some(Path::new("/usr/bin/python3")),
        )
        .unwrap();
        assert_eq!(
            proxy_sample.servers, 0,
            "a proxy with the script argument is not a real server"
        );
        assert_eq!(proxy_sample.roots, [proxy.id()]);
        proxy.kill().unwrap();
        proxy.wait().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
