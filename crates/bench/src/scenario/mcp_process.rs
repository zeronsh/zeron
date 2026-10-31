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
    let mut servers = 0;
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
            servers += 1;
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
        servers,
        roots: roots.into_iter().collect(),
        pss_kib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

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
