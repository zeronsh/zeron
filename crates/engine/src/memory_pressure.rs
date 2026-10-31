//! Host memory pressure and Linux run attribution. No privileged setup or tuning.
#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
use std::{path::Path, time::Duration};

pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const IDLE_FLOOR: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(crate) struct Pressure {
    consecutive: u8,
}
impl Pressure {
    pub(crate) fn observe(&mut self, high: bool) -> bool {
        self.consecutive = if high {
            self.consecutive.saturating_add(1)
        } else {
            0
        };
        self.consecutive >= 2
    }
}

fn low_memory(available: u64, total: u64) -> bool {
    available < (2 * 1024 * 1024 * 1024).max(total / 10)
}

#[cfg(target_os = "linux")]
fn linux_sample(meminfo: &str, psi: &str) -> Option<bool> {
    let kib = |key: &str| {
        meminfo.lines().find_map(|line| {
            let value = line
                .strip_prefix(key)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()?;
            value.checked_mul(1024)
        })
    };
    let memory = kib("MemAvailable:")
        .zip(kib("MemTotal:"))
        .map(|(available, total)| low_memory(available, total));
    let pressure = psi
        .lines()
        .find_map(|line| {
            let line = line.strip_prefix("some ")?;
            line.split_whitespace()
                .find_map(|v| v.strip_prefix("avg10=")?.parse::<f64>().ok())
        })
        .map(|avg| avg > 10.0);
    match (memory, pressure) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(false) || b.unwrap_or(false)),
    }
}

pub(crate) fn sample() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        linux_sample(
            &std::fs::read_to_string("/proc/meminfo").unwrap_or_default(),
            &std::fs::read_to_string("/proc/pressure/memory").unwrap_or_default(),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let mut level: libc::c_int = 0;
        let mut len = std::mem::size_of_val(&level);
        // Read-only kernel pressure level: 1 normal, 2 warning, 4 critical.
        let result = unsafe {
            libc::sysctlbyname(
                c"kern.memorystatus_vm_pressure_level".as_ptr(),
                (&mut level as *mut libc::c_int).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0).then_some(level > 1)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of_val(&status) as u32;
        (unsafe { GlobalMemoryStatusEx(&mut status) } != 0)
            .then(|| low_memory(status.ullAvailPhys, status.ullTotalPhys))
    }
    // Unsupported hosts, and failed/unavailable probes, do not trigger pressure.
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

/// Candidates have already passed the reaper's guards. Oldest first, one per tick.
pub(crate) fn oldest(candidates: impl IntoIterator<Item = (String, Duration)>) -> Option<String> {
    candidates
        .into_iter()
        .filter(|(_, idle)| *idle >= IDLE_FLOOR)
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(chat, _)| chat)
}

/// Sum PSS over the harness group(s) identified by the inherited run stamp,
/// plus numeric MCP group markers. Include group peers without the run env.
#[cfg(target_os = "linux")]
pub(crate) fn run_pss_mib(run_dir: &Path) -> Option<f64> {
    let stamp = format!("ZERON_RUN_ID={}", run_dir.file_name()?.to_str()?);
    let mut groups: BTreeSet<u32> = std::fs::read_dir(run_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let group = e.file_name().to_str()?.parse::<u32>().ok()?;
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            (value["pgid"] == group).then_some(group)
        })
        .collect();
    let processes: Vec<_> = std::fs::read_dir("/proc")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let pid = e.file_name().to_str()?.parse::<u32>().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            // Fields after comm: state, ppid, pgrp. comm may contain spaces/parentheses.
            let group = stat
                .rsplit_once(") ")?
                .1
                .split_whitespace()
                .nth(2)?
                .parse::<u32>()
                .ok()?;
            if std::fs::read(e.path().join("environ"))
                .ok()
                .is_some_and(|env| env.split(|b| *b == 0).any(|v| v == stamp.as_bytes()))
            {
                groups.insert(group);
            }
            Some((pid, group))
        })
        .collect();
    let mut kib = 0_u64;
    let mut measured = false;
    for (pid, group) in processes {
        if !groups.contains(&group) {
            continue;
        }
        if let Ok(rollup) = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            && let Some(pss) = rollup.lines().find_map(|l| {
                l.strip_prefix("Pss:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        {
            kib += pss;
            measured = true;
        }
    }
    measured.then_some(kib as f64 / 1024.0)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn run_pss_mib(_run_dir: &Path) -> Option<f64> {
    None
}

pub(crate) async fn cleanup_boot(root: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            zeron_mcp_proxy::cleanup_run_dir(&entry.path()).await?;
            // No engine is running (the instance lock), so every run directory
            // found here is a dead run's leftover.
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

/// Incremental, bounded intake of the proxy's existing lifecycle journal.
#[derive(Default)]
pub(crate) struct ProxyTelemetry {
    offset: u64,
}
impl ProxyTelemetry {
    pub(crate) fn emit(&mut self, path: &Path) {
        use std::io::{BufRead, BufReader, Seek, SeekFrom};
        let Ok(mut file) = std::fs::File::open(path) else {
            return;
        };
        let Ok(metadata) = file.metadata() else {
            return;
        };
        if metadata.len() < self.offset {
            self.offset = 0;
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut reader = BufReader::new(file);
        let mut consumed = 0;
        let mut line = String::new();
        while consumed < 256 * 1024 {
            line.clear();
            let Ok(n) = reader.read_line(&mut line) else {
                break;
            };
            // A concurrent append may leave an incomplete final record.
            if n == 0 || !line.ends_with('\n') {
                break;
            }
            self.offset += n as u64;
            consumed += n;
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            tracing::info!(
                event = value["event"].as_str().unwrap_or("unknown"),
                server_class = value["server_class"].as_str().unwrap_or("unknown"),
                startup_ms = value["startup_ms"]
                    .as_u64()
                    .or_else(|| value["ms"].as_u64()),
                server = value["server"].as_str().unwrap_or("unknown"),
                "mcp_proxy"
            );
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) fn fake_group(dir: &Path) -> tokio::process::Child {
    std::fs::create_dir_all(dir).unwrap();
    let mut child = tokio::process::Command::new("sleep");
    child
        .arg("60")
        .process_group(0)
        .kill_on_drop(true)
        .env("ZERON_RUN_ID", dir.file_name().unwrap());
    let child = child.spawn().unwrap();
    let pid = child.id().unwrap();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let start = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap();
    std::fs::write(
        dir.join(pid.to_string()),
        serde_json::json!({"pgid":pid,"start":start}).to_string(),
    )
    .unwrap();
    child
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn memory_pressure_boot_sweep_reaps_orphan_groups_and_measures_pss() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("orphan");
        let mut child = fake_group(&dir);
        // Ensure exec has finished before reading the inherited run stamp.
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(run_pss_mib(&dir).is_some_and(|pss| pss > 0.0));
        let marker = dir.join(child.id().unwrap().to_string());
        std::fs::create_dir_all(dir.join("capabilities")).unwrap();
        std::fs::write(dir.join("channel-live"), "channel").unwrap();
        cleanup_boot(root.path()).await.unwrap();
        assert!(!child.wait().await.unwrap().success());
        assert!(!marker.exists());
        assert!(!dir.exists(), "markers, evidence and the directory itself");
        assert!(run_pss_mib(&dir).is_none());
    }
    #[test]
    fn memory_pressure_proxy_telemetry_waits_for_complete_records() {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("mcp-proxy.jsonl");
        std::fs::write(&log, "{\"event\":\"start\"}").unwrap();
        let mut reader = ProxyTelemetry::default();
        reader.emit(&log);
        assert_eq!(reader.offset, 0);
        std::fs::write(&log, "{\"event\":\"start\"}\n").unwrap();
        reader.emit(&log);
        assert_eq!(reader.offset, std::fs::metadata(&log).unwrap().len());
        reader.emit(&log);
        assert_eq!(reader.offset, std::fs::metadata(&log).unwrap().len());
    }
    #[test]
    fn memory_pressure_requires_two_consecutive_samples_and_clears() {
        let mut pressure = Pressure::default();
        assert!(!pressure.observe(true));
        assert!(pressure.observe(true));
        assert!(!pressure.observe(false));
        assert!(!pressure.observe(true));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn memory_pressure_linux_thresholds_and_missing_probes() {
        let plenty = "MemTotal: 40000000 kB\nMemAvailable: 12000000 kB";
        assert_eq!(
            linux_sample(plenty, "some avg10=10.00 avg60=0"),
            Some(false)
        );
        assert_eq!(linux_sample(plenty, "some avg10=10.01 avg60=0"), Some(true));
        assert_eq!(
            linux_sample("MemTotal: 40000000 kB\nMemAvailable: 3000000 kB", ""),
            Some(true)
        );
        assert_eq!(
            linux_sample("MemTotal: 4000000 kB\nMemAvailable: 2000000 kB", ""),
            Some(true)
        );
        assert_eq!(linux_sample("", ""), None);
    }
    #[test]
    fn memory_pressure_picker_lru_and_floor() {
        assert_eq!(oldest([("new".into(), Duration::from_secs(119))]), None);
        assert_eq!(
            oldest([
                ("new".into(), Duration::from_secs(120)),
                ("old".into(), Duration::from_secs(300))
            ]),
            Some("old".into())
        );
    }
}
