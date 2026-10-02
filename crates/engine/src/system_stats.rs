//! Live resource sampling for `WatchSystemStats`.
//!
//! Each subscription owns its own [`Sampler`], so nothing runs while nobody is
//! watching and there is no shared state to reset. CPU, memory, network and
//! process figures come from `sysinfo`; GPU and disk throughput have no
//! portable source, so they come from small per-platform probes whose parsers
//! are pure functions over the tools' text output.

use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::BoxStream;
use sysinfo::{
    CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate,
    RefreshKind, System,
};
use zeron_proto::{
    CpuStats, DiskStats, DiskVolume, GpuStats, MemoryStats, NetworkInterfaceStats, NetworkStats,
    ProcessStats, SystemHost, SystemStats,
};

pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);
const MIN_INTERVAL: Duration = Duration::from_millis(500);
const MAX_INTERVAL: Duration = Duration::from_secs(10);
/// Per ranking (CPU, memory); the frame carries their union.
const TOP_PROCESSES: usize = 10;
/// A probe that hasn't answered by now is treated as unavailable this tick.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub fn clamp_interval(requested: Option<u64>) -> Duration {
    requested
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_INTERVAL)
        .clamp(MIN_INTERVAL, MAX_INTERVAL)
}

/// A frame every `interval`, starting once CPU deltas exist. Dropping the
/// stream (the subscriber left, or its link died) stops all sampling.
pub fn watch(interval: Duration) -> BoxStream<'static, serde_json::Value> {
    futures::stream::unfold(None::<Sampler>, move |sampler| async move {
        let mut sampler = match sampler {
            Some(sampler) => {
                tokio::time::sleep(interval).await;
                sampler
            }
            None => {
                let sampler = tokio::task::spawn_blocking(Sampler::new).await.ok()?;
                // CPU and process usage are deltas between two refreshes.
                tokio::time::sleep(
                    sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)),
                )
                .await;
                sampler
            }
        };
        let (sampler, stats) = tokio::time::timeout(
            PROBE_TIMEOUT * 3,
            tokio::task::spawn_blocking(move || {
                let stats = sampler.sample();
                (sampler, stats)
            }),
        )
        .await
        .ok()?
        .ok()?;
        let value = serde_json::to_value(stats).ok()?;
        Some((value, Some(sampler)))
    })
    .boxed()
}

pub struct Sampler {
    system: System,
    networks: Networks,
    disks: Disks,
    last: Instant,
    /// Cumulative disk byte counters from the previous tick.
    disk_counters: Option<(u64, u64)>,
}

impl Sampler {
    pub fn new() -> Self {
        let mut system = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            ProcessRefreshKind::new().with_cpu().with_memory(),
        );
        Self {
            system,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last: Instant::now(),
            disk_counters: read_disk_counters(),
        }
    }

    /// Take one frame; rates cover the time since the previous one.
    pub fn sample(&mut self) -> SystemStats {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64().max(0.001);
        self.last = now;

        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            ProcessRefreshKind::new().with_cpu().with_memory(),
        );
        self.networks.refresh_list();
        self.disks.refresh_list();

        let load = System::load_average();
        let cpus = self.system.cpus();
        let cpu = CpuStats {
            brand: cpus
                .first()
                .map(|c| c.brand().trim().to_string())
                .unwrap_or_default(),
            physical_cores: self.system.physical_core_count().map(|n| n as u32),
            total_percent: self.system.global_cpu_usage().clamp(0.0, 100.0),
            per_core_percent: cpus
                .iter()
                .map(|c| c.cpu_usage().clamp(0.0, 100.0))
                .collect(),
            load_average: (cfg!(unix)).then_some([load.one, load.five, load.fifteen]),
        };

        let memory = MemoryStats {
            total_bytes: self.system.total_memory(),
            used_bytes: self.system.used_memory(),
            swap_total_bytes: self.system.total_swap(),
            swap_used_bytes: self.system.used_swap(),
        };

        let mut interfaces: Vec<NetworkInterfaceStats> = self
            .networks
            .list()
            .iter()
            .filter(|(name, _)| !is_loopback(name))
            .map(|(name, data)| NetworkInterfaceStats {
                name: name.clone(),
                received_per_sec: per_sec(data.received(), elapsed),
                transmitted_per_sec: per_sec(data.transmitted(), elapsed),
                total_received_bytes: data.total_received(),
                total_transmitted_bytes: data.total_transmitted(),
            })
            .filter(|i| i.total_received_bytes + i.total_transmitted_bytes > 0)
            .collect();
        interfaces.sort_by(|a, b| {
            (b.received_per_sec + b.transmitted_per_sec)
                .cmp(&(a.received_per_sec + a.transmitted_per_sec))
                .then_with(|| a.name.cmp(&b.name))
        });
        let network = NetworkStats {
            received_per_sec: interfaces.iter().map(|i| i.received_per_sec).sum(),
            transmitted_per_sec: interfaces.iter().map(|i| i.transmitted_per_sec).sum(),
            interfaces,
        };

        let counters = read_disk_counters();
        let (read_per_sec, write_per_sec) = match (self.disk_counters, counters) {
            (Some(prev), Some(now)) => (
                Some(per_sec(now.0.saturating_sub(prev.0), elapsed)),
                Some(per_sec(now.1.saturating_sub(prev.1), elapsed)),
            ),
            _ => (None, None),
        };
        self.disk_counters = counters;
        let disks = DiskStats {
            read_per_sec,
            write_per_sec,
            volumes: volumes(self.disks.list().iter().map(|disk| RawVolume {
                name: disk.name().to_string_lossy().into_owned(),
                mount_point: disk.mount_point().to_string_lossy().into_owned(),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                total: disk.total_space(),
                available: disk.available_space(),
            })),
        };

        let processes = rank_processes(
            self.system
                .processes()
                .iter()
                .map(|(pid, process)| ProcessStats {
                    pid: pid.as_u32(),
                    name: process.name().to_string_lossy().into_owned(),
                    cpu_percent: process.cpu_usage(),
                    memory_bytes: process.memory(),
                })
                .collect(),
        );

        SystemStats {
            sampled_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            host: SystemHost {
                name: System::host_name().unwrap_or_default(),
                os: System::long_os_version()
                    .map(|os| display_os(&os))
                    .unwrap_or_default(),
                uptime_secs: System::uptime(),
            },
            cpu,
            memory,
            gpus: read_gpus(),
            network,
            disks,
            processes,
        }
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

/// `sysinfo` capitalizes the macOS name as "MacOS" and can leave a trailing space.
fn display_os(raw: &str) -> String {
    let raw = raw.trim();
    match raw.strip_prefix("MacOS") {
        Some(rest) => format!("macOS{rest}"),
        None => raw.to_string(),
    }
}

fn per_sec(bytes: u64, elapsed_secs: f64) -> u64 {
    (bytes as f64 / elapsed_secs).round() as u64
}

fn is_loopback(name: &str) -> bool {
    name == "lo" || name.starts_with("lo0") || name.eq_ignore_ascii_case("loopback")
}

/// The heaviest processes by CPU and by memory, CPU first, without duplicates.
pub fn rank_processes(mut all: Vec<ProcessStats>) -> Vec<ProcessStats> {
    // pid 0 is the kernel's idle/task pseudo-process on most platforms.
    all.retain(|p| p.pid != 0);
    all.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.memory_bytes.cmp(&a.memory_bytes))
            .then(a.pid.cmp(&b.pid))
    });
    let mut picked: Vec<ProcessStats> = all.iter().take(TOP_PROCESSES).cloned().collect();
    let mut by_memory: Vec<&ProcessStats> = all.iter().collect();
    by_memory.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes).then(a.pid.cmp(&b.pid)));
    for process in by_memory.into_iter().take(TOP_PROCESSES) {
        if !picked.iter().any(|p| p.pid == process.pid) {
            picked.push(process.clone());
        }
    }
    picked
}

pub struct RawVolume {
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total: u64,
    pub available: u64,
}

/// User-visible volumes. Pseudo filesystems, system-internal mounts and the
/// APFS siblings that share one container's capacity are dropped, so a disk
/// is listed once with the numbers a person expects.
pub fn volumes(raw: impl Iterator<Item = RawVolume>) -> Vec<DiskVolume> {
    const PSEUDO: &[&str] = &[
        "tmpfs",
        "devtmpfs",
        "devfs",
        "squashfs",
        "overlay",
        "proc",
        "sysfs",
        "cgroup",
        "cgroup2",
        "efivarfs",
        "autofs",
        "fuse.portal",
        "ramfs",
        "nsfs",
    ];
    let mut out: Vec<DiskVolume> = Vec::new();
    for volume in raw {
        let fs = volume.file_system.to_ascii_lowercase();
        let mount = volume.mount_point.as_str();
        if volume.total == 0
            || PSEUDO.contains(&fs.as_str())
            || mount.starts_with("/snap/")
            || mount.starts_with("/boot")
            || mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data"
            || mount.starts_with("/private/var/")
            || mount.starts_with("/run/")
            || mount.starts_with("/dev")
        {
            continue;
        }
        if out.iter().any(|v| v.mount_point == volume.mount_point) {
            continue;
        }
        // macOS: the sealed system volume at `/` and the user's Data volume
        // share one container; show the Data volume, named for the machine's disk.
        if mount == "/" && cfg!(target_os = "macos") {
            continue;
        }
        out.push(DiskVolume {
            name: display_volume_name(&volume),
            mount_point: volume.mount_point,
            total_bytes: volume.total,
            available_bytes: volume.available,
        });
    }
    out
}

fn display_volume_name(volume: &RawVolume) -> String {
    if volume.mount_point == "/System/Volumes/Data" {
        return "Macintosh HD".into();
    }
    if !volume.name.is_empty() && !volume.name.starts_with("/dev/") {
        return volume.name.clone();
    }
    volume
        .mount_point
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("/")
        .to_string()
}

// ─── platform probes ────────────────────────────────────────────────────────

/// Run a probe binary, giving up (and killing it) after [`PROBE_TIMEOUT`].
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_probe(program: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Drain on a thread so a chatty probe can't fill the pipe and stall.
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).ok().map(|_| text)
    });
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return reader.join().ok().flatten(),
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[cfg(target_os = "macos")]
fn read_gpus() -> Vec<GpuStats> {
    run_probe(
        "/usr/sbin/ioreg",
        &["-r", "-d", "1", "-w0", "-c", "IOAccelerator"],
    )
    .map(|text| parse_ioreg_gpus(&text))
    .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn read_gpus() -> Vec<GpuStats> {
    let mut gpus = run_probe(
        "nvidia-smi",
        &[
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
    )
    .map(|text| parse_nvidia_smi(&text))
    .unwrap_or_default();
    gpus.extend(read_amdgpu_sysfs());
    gpus
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_gpus() -> Vec<GpuStats> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn read_disk_counters() -> Option<(u64, u64)> {
    run_probe(
        "/usr/sbin/ioreg",
        &["-r", "-c", "IOBlockStorageDriver", "-w0"],
    )
    .and_then(|text| parse_ioreg_disk_counters(&text))
}

#[cfg(target_os = "linux")]
fn read_disk_counters() -> Option<(u64, u64)> {
    parse_proc_diskstats(&std::fs::read_to_string("/proc/diskstats").ok()?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_disk_counters() -> Option<(u64, u64)> {
    None
}

#[cfg(target_os = "linux")]
fn read_amdgpu_sysfs() -> Vec<GpuStats> {
    let Ok(cards) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut names: Vec<_> = cards
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        // `card0`, not `card0-DP-1` connector nodes.
        .filter(|n| n.starts_with("card") && n[4..].bytes().all(|b| b.is_ascii_digit()))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|card| {
            let dir = std::path::Path::new("/sys/class/drm")
                .join(card)
                .join("device");
            let read = |file: &str| std::fs::read_to_string(dir.join(file)).ok();
            let busy: f32 = read("gpu_busy_percent")?.trim().parse().ok()?;
            let number = |file: &str| read(file).and_then(|t| t.trim().parse::<u64>().ok());
            Some(GpuStats {
                name: read("product_name")
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "AMD GPU".into()),
                utilization_percent: Some(busy.clamp(0.0, 100.0)),
                memory_used_bytes: number("mem_info_vram_used"),
                memory_total_bytes: number("mem_info_vram_total"),
                temperature_celsius: None,
            })
        })
        .collect()
}

// ─── probe parsers (pure) ───────────────────────────────────────────────────

/// The integer following `"key"=` inside an ioreg dictionary.
fn ioreg_number(text: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"=");
    let start = text.find(&needle)? + needle.len();
    let digits: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// The quoted string following `"key" = ` on an ioreg property line.
fn ioreg_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\" = \"");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..].find('"')?;
    Some(text[start..start + end].to_string())
}

/// `ioreg -r -d 1 -w0 -c IOAccelerator`: one object per GPU, each with a
/// `PerformanceStatistics` dictionary. Apple GPUs share system memory, so no
/// dedicated total is reported.
pub fn parse_ioreg_gpus(text: &str) -> Vec<GpuStats> {
    text.split("+-o ")
        .skip(1)
        .filter_map(|block| {
            let stats_line = block
                .lines()
                .find(|line| line.contains("\"PerformanceStatistics\""))?;
            let utilization = ioreg_number(stats_line, "Device Utilization %")?;
            Some(GpuStats {
                name: ioreg_string(block, "model").unwrap_or_else(|| "GPU".into()),
                utilization_percent: Some((utilization as f32).clamp(0.0, 100.0)),
                memory_used_bytes: ioreg_number(stats_line, "In use system memory"),
                memory_total_bytes: None,
                temperature_celsius: None,
            })
        })
        .collect()
}

/// `ioreg -r -c IOBlockStorageDriver -w0`: cumulative bytes read and written
/// summed over every storage driver.
pub fn parse_ioreg_disk_counters(text: &str) -> Option<(u64, u64)> {
    let mut found = false;
    let (mut read, mut written) = (0u64, 0u64);
    for line in text.lines().filter(|l| l.contains("\"Statistics\"")) {
        if let (Some(r), Some(w)) = (
            ioreg_number(line, "Bytes (Read)"),
            ioreg_number(line, "Bytes (Write)"),
        ) {
            found = true;
            read = read.saturating_add(r);
            written = written.saturating_add(w);
        }
    }
    found.then_some((read, written))
}

/// `nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,
/// temperature.gpu --format=csv,noheader,nounits`. Memory is in MiB. Fields a
/// GPU can't report print `[N/A]`.
pub fn parse_nvidia_smi(text: &str) -> Vec<GpuStats> {
    const MIB: u64 = 1024 * 1024;
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let [name, util, used, total, temp] = fields.as_slice() else {
                return None;
            };
            let mib = |s: &str| s.parse::<u64>().ok().map(|v| v * MIB);
            Some(GpuStats {
                name: (*name).to_string(),
                utilization_percent: util.parse::<f32>().ok().map(|v| v.clamp(0.0, 100.0)),
                memory_used_bytes: mib(used),
                memory_total_bytes: mib(total),
                temperature_celsius: temp.parse().ok(),
            })
        })
        .collect()
}

/// `/proc/diskstats`: cumulative sectors (512 bytes) read and written over
/// whole physical disks. Partitions, loop, ram and device-mapper devices are
/// skipped so nothing counts twice.
pub fn parse_proc_diskstats(text: &str) -> Option<(u64, u64)> {
    let mut found = false;
    let (mut read, mut written) = (0u64, 0u64);
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 || !is_whole_disk(fields[2]) {
            continue;
        }
        if let (Ok(r), Ok(w)) = (fields[5].parse::<u64>(), fields[9].parse::<u64>()) {
            found = true;
            read = read.saturating_add(r.saturating_mul(512));
            written = written.saturating_add(w.saturating_mul(512));
        }
    }
    found.then_some((read, written))
}

fn is_whole_disk(name: &str) -> bool {
    let digits_tail = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
    };
    // nvme0n1, mmcblk0 (partitions carry a `p<N>` suffix).
    if let Some(rest) = name.strip_prefix("nvme") {
        return rest.split_once('n').is_some_and(|(ctrl, ns)| {
            !ctrl.is_empty()
                && !ns.is_empty()
                && ctrl.bytes().all(|b| b.is_ascii_digit())
                && ns.bytes().all(|b| b.is_ascii_digit())
        });
    }
    if digits_tail("mmcblk") {
        return true;
    }
    // sda, vdb, xvdc, hdd — letters only (partitions end in a digit).
    ["sd", "vd", "xvd", "hd"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_lowercase()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IOREG_GPU: &str = r#"+-o AGXAcceleratorG14G  <class AGXAcceleratorG14G, id 0x10000054f, registered, matched, active, busy 0 (204 ms), retain 95>
    {
      "PerformanceStatistics" = {"In use system memory (driver)"=0,"Alloc system memory"=2930081792,"Tiler Utilization %"=54,"recoveryCount"=0,"Renderer Utilization %"=54,"Device Utilization %"=54,"In use system memory"=855785472}
      "model" = "Apple M2"
      "gpu-core-count" = 10
    }
"#;

    #[test]
    fn parses_apple_gpu_utilization_and_memory() {
        let gpus = parse_ioreg_gpus(IOREG_GPU);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "Apple M2");
        assert_eq!(gpus[0].utilization_percent, Some(54.0));
        // The plain key, not the "(driver)" one that shares its prefix.
        assert_eq!(gpus[0].memory_used_bytes, Some(855_785_472));
        assert_eq!(gpus[0].memory_total_bytes, None);
    }

    #[test]
    fn gpu_without_statistics_is_skipped() {
        assert!(parse_ioreg_gpus("+-o Foo\n    {\n      \"model\" = \"x\"\n    }\n").is_empty());
        assert!(parse_ioreg_gpus("").is_empty());
    }

    #[test]
    fn sums_ioreg_disk_counters_across_drivers() {
        let text = concat!(
            "  |   \"Statistics\" = {\"Bytes (Read)\"=100,\"Bytes (Write)\"=40,\"Operations (Read)\"=3}\n",
            "  |   \"Statistics\" = {\"Bytes (Read)\"=5,\"Bytes (Write)\"=0}\n",
            "  |   \"Other\" = {\"Bytes (Read)\"=999}\n",
        );
        assert_eq!(parse_ioreg_disk_counters(text), Some((105, 40)));
        assert_eq!(parse_ioreg_disk_counters("nothing here"), None);
    }

    #[test]
    fn parses_nvidia_smi_rows_and_missing_fields() {
        let text = "NVIDIA GeForce RTX 4090, 37, 2048, 24564, 51\nTesla T4, 0, 0, 15360, [N/A]\n";
        let gpus = parse_nvidia_smi(text);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(gpus[0].utilization_percent, Some(37.0));
        assert_eq!(gpus[0].memory_used_bytes, Some(2048 * 1024 * 1024));
        assert_eq!(gpus[0].memory_total_bytes, Some(24564 * 1024 * 1024));
        assert_eq!(gpus[0].temperature_celsius, Some(51.0));
        assert_eq!(gpus[1].temperature_celsius, None);
        assert!(parse_nvidia_smi("garbage line").is_empty());
    }

    #[test]
    fn diskstats_count_whole_disks_only() {
        let text = "\
   8       0 sda 100 0 2000 0 50 0 800 0 0 0 0
   8       1 sda1 90 0 1900 0 40 0 700 0 0 0 0
 259       0 nvme0n1 10 0 300 0 5 0 60 0 0 0 0
 259       1 nvme0n1p1 9 0 290 0 4 0 50 0 0 0 0
   7       0 loop0 5 0 999 0 0 0 0 0 0 0 0
 253       0 dm-0 5 0 999 0 0 0 0 0 0 0 0
";
        // (2000 + 300) sectors read, (800 + 60) written, × 512.
        assert_eq!(parse_proc_diskstats(text), Some((2300 * 512, 860 * 512)));
        assert_eq!(parse_proc_diskstats(""), None);
    }

    #[test]
    fn whole_disk_names() {
        for name in [
            "sda", "vdb", "xvdc", "nvme0n1", "nvme12n2", "mmcblk0", "hda",
        ] {
            assert!(is_whole_disk(name), "{name}");
        }
        for name in [
            "sda1",
            "nvme0n1p1",
            "mmcblk0p2",
            "loop0",
            "dm-0",
            "sr0",
            "zram0",
            "sd",
            "",
        ] {
            assert!(!is_whole_disk(name), "{name}");
        }
    }

    fn process(pid: u32, name: &str, cpu: f32, memory: u64) -> ProcessStats {
        ProcessStats {
            pid,
            name: name.into(),
            cpu_percent: cpu,
            memory_bytes: memory,
        }
    }

    #[test]
    fn ranking_leads_with_cpu_and_adds_memory_hogs_once() {
        let mut all: Vec<ProcessStats> = (1..=30)
            .map(|pid| process(pid, &format!("p{pid}"), pid as f32, 1))
            .collect();
        // Idle but enormous: only the memory ranking surfaces it.
        all.push(process(100, "hog", 0.0, 9_000_000));
        all.push(process(0, "kernel_task", 500.0, 1));
        let ranked = rank_processes(all);
        assert_eq!(
            ranked[0].pid, 30,
            "highest CPU first, kernel pseudo-process dropped"
        );
        assert_eq!(ranked.iter().filter(|p| p.pid == 100).count(), 1);
        assert!(ranked.iter().all(|p| p.pid != 0));
        assert!(ranked.len() <= 2 * TOP_PROCESSES);
        let mut pids: Vec<_> = ranked.iter().map(|p| p.pid).collect();
        pids.sort();
        pids.dedup();
        assert_eq!(pids.len(), ranked.len());
    }

    #[test]
    fn volumes_drop_pseudo_and_duplicate_mounts() {
        let raw = |name: &str, mount: &str, fs: &str, total: u64| RawVolume {
            name: name.into(),
            mount_point: mount.into(),
            file_system: fs.into(),
            total,
            available: total / 2,
        };
        let listed = volumes(
            vec![
                raw("/dev/disk3s1s1", "/", "apfs", 500),
                raw("/dev/disk3s5", "/System/Volumes/Data", "apfs", 500),
                raw("/dev/disk3s6", "/System/Volumes/VM", "apfs", 500),
                raw("tmpfs", "/run/user/1000", "tmpfs", 10),
                raw("Backup", "/Volumes/Backup", "exfat", 2000),
                raw("Backup", "/Volumes/Backup", "exfat", 2000),
                raw("empty", "/Volumes/Empty", "apfs", 0),
            ]
            .into_iter(),
        );
        let mounts: Vec<_> = listed.iter().map(|v| v.mount_point.as_str()).collect();
        if cfg!(target_os = "macos") {
            assert_eq!(mounts, ["/System/Volumes/Data", "/Volumes/Backup"]);
            assert_eq!(listed[0].name, "Macintosh HD");
        } else {
            assert_eq!(mounts, ["/", "/System/Volumes/Data", "/Volumes/Backup"]);
        }
    }

    #[test]
    fn frames_parse_from_older_or_partial_peers() {
        // A peer that reports nothing new still yields a usable frame.
        let empty: SystemStats = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(empty.cpu.per_core_percent.is_empty());
        assert!(empty.gpus.is_empty());
        // Unknown fields from a newer peer are ignored.
        let newer: SystemStats = serde_json::from_value(serde_json::json!({
            "cpu": { "totalPercent": 12.5, "fanRpm": 3000 },
            "quantumCores": 4
        }))
        .unwrap();
        assert_eq!(newer.cpu.total_percent, 12.5);
    }

    #[tokio::test]
    async fn watch_streams_frames_until_dropped() {
        let mut stream = watch(Duration::from_millis(500));
        let first = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("first frame")
            .expect("stream open");
        let first: SystemStats = serde_json::from_value(first).unwrap();
        assert!(!first.cpu.per_core_percent.is_empty());
        let second = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("second frame")
            .expect("stream open");
        let second: SystemStats = serde_json::from_value(second).unwrap();
        assert!(second.sampled_at_ms >= first.sampled_at_ms);
        drop(stream);
    }

    #[test]
    fn os_name_is_tidied() {
        assert_eq!(display_os("MacOS 27.0.1 "), "macOS 27.0.1");
        assert_eq!(display_os("Ubuntu 24.04"), "Ubuntu 24.04");
        assert_eq!(display_os(""), "");
    }

    #[test]
    fn interval_is_clamped() {
        assert_eq!(clamp_interval(None), DEFAULT_INTERVAL);
        assert_eq!(clamp_interval(Some(5)), MIN_INTERVAL);
        assert_eq!(clamp_interval(Some(2000)), Duration::from_secs(2));
        assert_eq!(clamp_interval(Some(u64::MAX)), MAX_INTERVAL);
    }

    #[test]
    fn sampler_reports_this_machine() {
        let mut sampler = Sampler::new();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let stats = sampler.sample();
        assert!(!stats.cpu.per_core_percent.is_empty());
        assert!(
            stats
                .cpu
                .per_core_percent
                .iter()
                .all(|c| (0.0..=100.0).contains(c))
        );
        assert!(stats.memory.total_bytes > 0);
        assert!(stats.memory.used_bytes <= stats.memory.total_bytes);
        assert!(!stats.processes.is_empty());
        // The whole frame survives the wire.
        let back: SystemStats =
            serde_json::from_value(serde_json::to_value(&stats).unwrap()).unwrap();
        assert_eq!(
            back.cpu.per_core_percent.len(),
            stats.cpu.per_core_percent.len()
        );
    }
}
