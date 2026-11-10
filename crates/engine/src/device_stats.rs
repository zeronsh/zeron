//! What this device's machine says about itself, for `DeviceStats`: the OS,
//! CPU, memory, storage, GPU, load, battery and Zeron's own version.
//!
//! Everything comes from OS APIs (through `sysinfo` and a few small
//! per-platform readers), never a child process, and a report is reused for
//! [`CACHE_FOR`] so an agent asking in a loop costs one collection.
//!
//! Every field is optional: a machine that can't say something reports
//! `null` for it, and nothing here fails the whole report.
//!
//! Privacy: a report names volumes by their mount points only. It never
//! carries the data dir's or the home folder's own path, a user name, the
//! machine's hostname (it often holds a person's name; the device's Zeron
//! name says which machine it is), or a serial number.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How long one collection is reused.
pub const CACHE_FOR: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStats {
    /// When this report was collected (it may be up to [`CACHE_FOR`] old).
    pub collected_at: Option<chrono::DateTime<chrono::Utc>>,
    pub os: OsStats,
    pub cpu: CpuStats,
    pub memory: MemoryStats,
    pub storage: StorageStats,
    /// GPU names, where the platform says them cheaply; `null` when unknown.
    pub gpus: Option<Vec<GpuStats>>,
    pub uptime_seconds: Option<u64>,
    /// 1, 5 and 15 minute load averages (Unix only).
    pub load_average: Option<LoadAverage>,
    /// `null` on a machine with no battery.
    pub battery: Option<BatteryStats>,
    pub zeron: ZeronStats,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OsStats {
    /// e.g. "Darwin", "Ubuntu", "Windows".
    pub name: Option<String>,
    /// e.g. "15.1", "24.04", "11 (26100)".
    pub version: Option<String>,
    /// e.g. "macOS 15.1 Sequoia".
    pub long_version: Option<String>,
    pub kernel: Option<String>,
    /// e.g. "arm64", "x86_64".
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuStats {
    pub model: Option<String>,
    pub physical_cores: Option<usize>,
    pub logical_cores: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    /// The volume that holds Zeron's data folder.
    pub zeron_data: Option<VolumeStats>,
    /// The volume that holds the user's home folder.
    pub home: Option<VolumeStats>,
    /// Every mounted local volume (system-internal ones left out).
    pub volumes: Vec<VolumeStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeStats {
    pub mount_point: String,
    /// The volume's label (e.g. "Macintosh HD"), never a device path.
    pub name: Option<String>,
    /// The filesystem, e.g. "apfs", "ext4", "NTFS".
    pub file_system: Option<String>,
    /// "ssd", "hdd" or null.
    pub kind: Option<String>,
    pub removable: bool,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryStats {
    /// 0–100.
    pub percent: Option<f64>,
    pub charging: Option<bool>,
    /// Plugged in (charging or full).
    pub on_ac_power: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZeronStats {
    pub engine_version: String,
    /// The engine build (commit and build time) when known.
    pub build: Option<String>,
}

/// Where this engine keeps its data, for the "which volume" answers.
#[derive(Debug, Clone)]
pub struct Places {
    pub data_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub build: Option<String>,
}

static CACHE: Mutex<Option<(Instant, DeviceStats)>> = Mutex::new(None);

/// This machine's report: a cached one under [`CACHE_FOR`] old, a fresh one
/// otherwise (collected off the async runtime).
pub async fn current(places: Places) -> DeviceStats {
    if let Some(cached) = cached() {
        return cached;
    }
    let fresh = tokio::task::spawn_blocking(move || collect(&places))
        .await
        .unwrap_or_else(|_| DeviceStats {
            zeron: zeron(None),
            ..DeviceStats::default()
        });
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), fresh.clone()));
    fresh
}

fn cached() -> Option<DeviceStats> {
    let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .as_ref()
        .filter(|(at, _)| at.elapsed() < CACHE_FOR)
        .map(|(_, stats)| stats.clone())
}

fn zeron(build: Option<String>) -> ZeronStats {
    ZeronStats {
        engine_version: env!("CARGO_PKG_VERSION").to_string(),
        build,
    }
}

/// One collection, synchronously. Each part stands alone: a part the
/// platform can't answer comes back empty without touching the others.
pub fn collect(places: &Places) -> DeviceStats {
    use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

    let system = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::new())
            .with_memory(MemoryRefreshKind::new().with_ram()),
    );
    let supported = sysinfo::IS_SUPPORTED_SYSTEM;
    let model = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim().to_string())
        .filter(|brand| !brand.is_empty());
    let logical = Some(system.cpus().len()).filter(|n| *n > 0);
    let cpu = CpuStats {
        model: model.clone(),
        physical_cores: system.physical_core_count().filter(|n| *n > 0),
        logical_cores: logical
            .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get())),
    };
    // macOS: sysinfo subtracts the compressor's pages from free + inactive
    // + purgeable, which floors at 0 on a busy Mac (more compressed than
    // reclaimable); this asks the kernel the way psutil does instead. A 0
    // anywhere else is no answer either: null, never a false "nothing left".
    #[cfg(target_os = "macos")]
    let available = mac_memory::available_bytes();
    #[cfg(not(target_os = "macos"))]
    let available = Some(system.available_memory());
    let memory = MemoryStats {
        total_bytes: Some(system.total_memory()).filter(|n| supported && *n > 0),
        available_bytes: available
            .filter(|n| supported && *n > 0 && system.total_memory() > 0)
            .map(|n| n.min(system.total_memory())),
    };
    let tidy = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let os = OsStats {
        name: tidy(System::name()),
        version: tidy(System::os_version()),
        long_version: tidy(System::long_os_version()),
        kernel: tidy(System::kernel_version()),
        arch: tidy(System::cpu_arch()),
    };
    let uptime_seconds = Some(System::uptime()).filter(|s| supported && *s > 0);
    let load_average = if cfg!(unix) && supported {
        let load = System::load_average();
        Some(LoadAverage {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        })
    } else {
        None
    };

    let disks = Disks::new_with_refreshed_list();
    let mounted: Vec<Mounted> = disks
        .list()
        .iter()
        .map(|disk| Mounted {
            mount_point: disk.mount_point().to_path_buf(),
            name: disk.name().to_string_lossy().into_owned(),
            file_system: disk.file_system().to_string_lossy().into_owned(),
            kind: match disk.kind() {
                sysinfo::DiskKind::SSD => Some("ssd"),
                sysinfo::DiskKind::HDD => Some("hdd"),
                sysinfo::DiskKind::Unknown(_) => None,
            },
            removable: disk.is_removable(),
            total: disk.total_space(),
            available: disk.available_space(),
        })
        .collect();
    let storage = storage(&mounted, places);

    DeviceStats {
        collected_at: Some(chrono::Utc::now()),
        os,
        cpu,
        memory,
        storage,
        gpus: gpus(model.as_deref()),
        uptime_seconds,
        load_average,
        battery: battery(),
        zeron: zeron(places.build.clone()),
    }
}

/// One mounted volume as the OS lists it (before privacy filtering).
#[derive(Debug, Clone)]
pub(crate) struct Mounted {
    pub mount_point: PathBuf,
    pub name: String,
    pub file_system: String,
    pub kind: Option<&'static str>,
    pub removable: bool,
    pub total: u64,
    pub available: u64,
}

impl Mounted {
    fn report(&self) -> VolumeStats {
        let text = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        VolumeStats {
            mount_point: self.mount_point.to_string_lossy().into_owned(),
            // A device node (`/dev/disk3s1`, `\\?\Volume{…}`) is a path, not
            // a label: left out.
            name: text(&self.name).filter(|n| !n.starts_with('/') && !n.starts_with('\\')),
            file_system: text(&self.file_system),
            kind: self.kind.map(str::to_string),
            removable: self.removable,
            total_bytes: self.total,
            available_bytes: self.available.min(self.total),
            used_bytes: self.total.saturating_sub(self.available),
        }
    }
}

pub(crate) fn storage(mounted: &[Mounted], places: &Places) -> StorageStats {
    let holding = |path: Option<&Path>| {
        path.and_then(|path| volume_of(mounted, path))
            .map(Mounted::report)
    };
    StorageStats {
        zeron_data: holding(places.data_dir.as_deref()),
        home: holding(places.home.as_deref()),
        volumes: mounted
            .iter()
            .filter(|m| listed(m))
            .map(Mounted::report)
            .collect(),
    }
}

/// The volume a path lives on: the one whose mount point is the same
/// filesystem (Unix device id) — so macOS's `/Users` lands on the Data
/// volume — falling back to the longest mount-point prefix.
fn volume_of<'a>(mounted: &'a [Mounted], path: &Path) -> Option<&'a Mounted> {
    // The folder itself may not exist yet: ask about its nearest ancestor.
    let existing = path.ancestors().find(|p| p.exists());
    #[cfg(unix)]
    if let Some(existing) = existing {
        use std::os::unix::fs::MetadataExt;
        if let Ok(dev) = std::fs::metadata(existing).map(|m| m.dev()) {
            let same: Vec<&Mounted> = mounted
                .iter()
                .filter(|m| std::fs::metadata(&m.mount_point).is_ok_and(|md| md.dev() == dev))
                .collect();
            if let Some(best) = same.iter().max_by_key(|m| m.mount_point.as_os_str().len()) {
                return Some(*best);
            }
        }
    }
    let path = existing.unwrap_or(path);
    mounted
        .iter()
        .filter(|m| starts_with_ignoring_case(path, &m.mount_point))
        .max_by_key(|m| m.mount_point.as_os_str().len())
}

fn starts_with_ignoring_case(path: &Path, prefix: &Path) -> bool {
    if cfg!(windows) {
        let path = path.to_string_lossy().to_ascii_lowercase();
        let prefix = prefix.to_string_lossy().to_ascii_lowercase();
        path.starts_with(&prefix)
    } else {
        path.starts_with(prefix)
    }
}

/// Whether a volume belongs in the "every volume" list: not an empty or
/// pseudo filesystem, and not one of macOS's system-internal volumes.
fn listed(mounted: &Mounted) -> bool {
    if mounted.total == 0 {
        return false;
    }
    let fs = mounted.file_system.to_ascii_lowercase();
    if matches!(
        fs.as_str(),
        "squashfs" | "overlay" | "tmpfs" | "devfs" | "autofs" | "nullfs"
    ) {
        return false;
    }
    let mount = mounted.mount_point.to_string_lossy();
    if mount.starts_with("/System/Volumes/") && mount != "/System/Volumes/Data" {
        return false;
    }
    !(mount.starts_with("/private/var/vm")
        || mount.starts_with("/snap/")
        || mount.starts_with("/boot/efi"))
}

/// GPU names. macOS on Apple silicon: the GPU is part of the chip and Metal
/// names it the chip's name ("Apple M1 Pro"); asking Metal itself would load
/// the GPU driver into the engine, so that name is taken from the CPU brand.
/// Linux: NVIDIA's driver lists its GPUs in `/proc`. Elsewhere: unknown.
fn gpus(cpu_model: Option<&str>) -> Option<Vec<GpuStats>> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return cpu_model.filter(|m| m.starts_with("Apple")).map(|m| {
            vec![GpuStats {
                name: m.to_string(),
            }]
        });
    }
    #[cfg(target_os = "linux")]
    {
        let names = nvidia_gpus(Path::new("/proc/driver/nvidia/gpus"));
        if !names.is_empty() {
            return Some(names.into_iter().map(|name| GpuStats { name }).collect());
        }
    }
    None
}

/// `Model:` of each `/proc/driver/nvidia/gpus/<bus>/information`.
#[cfg(any(target_os = "linux", test))]
fn nvidia_gpus(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("information")).ok())
        .filter_map(|info| {
            info.lines().find_map(|line| {
                line.strip_prefix("Model:")
                    .map(|m| m.trim().to_string())
                    .filter(|m| !m.is_empty())
            })
        })
        .collect();
    names.sort();
    names
}

#[cfg(target_os = "macos")]
fn battery() -> Option<BatteryStats> {
    mac_battery::read()
}

#[cfg(target_os = "linux")]
fn battery() -> Option<BatteryStats> {
    linux_battery(Path::new("/sys/class/power_supply"))
}

#[cfg(windows)]
fn battery() -> Option<BatteryStats> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: a plain out-parameter struct the call fills.
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return None;
    }
    // BatteryFlag 128 = no system battery, 255 = unknown.
    if status.BatteryFlag == 128 || status.BatteryFlag == 255 {
        return None;
    }
    Some(BatteryStats {
        percent: (status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent as f64),
        charging: Some(status.BatteryFlag & 8 != 0),
        on_ac_power: match status.ACLineStatus {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn battery() -> Option<BatteryStats> {
    None
}

/// The first `type = Battery` supply under `/sys/class/power_supply`.
#[cfg(any(target_os = "linux", test))]
fn linux_battery(root: &Path) -> Option<BatteryStats> {
    let read = |dir: &Path, name: &str| {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .map(|s| s.trim().to_string())
    };
    let mut supplies: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    supplies.sort();
    let on_ac_power = supplies
        .iter()
        .filter(|dir| read(dir, "type").as_deref() == Some("Mains"))
        .find_map(|dir| read(dir, "online"))
        .map(|online| online == "1");
    let battery = supplies
        .iter()
        .find(|dir| read(dir, "type").as_deref() == Some("Battery"))?;
    let status = read(battery, "status");
    Some(BatteryStats {
        percent: read(battery, "capacity")
            .and_then(|c| c.parse::<f64>().ok())
            .map(|c| c.clamp(0.0, 100.0)),
        charging: status.as_deref().map(|s| s == "Charging"),
        on_ac_power: on_ac_power.or(match status.as_deref() {
            Some("Charging" | "Full") => Some(true),
            Some("Discharging") => Some(false),
            _ => None,
        }),
    })
}

/// The kernel's VM statistics: memory that can be handed out without
/// swapping, i.e. free (speculative included) + inactive pages, as psutil
/// counts it on macOS.
#[cfg(target_os = "macos")]
mod mac_memory {
    use std::sync::OnceLock;

    unsafe extern "C" {
        // Declared here: libc marks its binding deprecated in favour of the
        // mach2 crate, for this one call.
        fn mach_host_self() -> libc::mach_port_t;
    }

    /// One send right to the host port for the engine's lifetime, not one
    /// per report.
    fn host() -> libc::mach_port_t {
        static HOST: OnceLock<libc::mach_port_t> = OnceLock::new();
        // SAFETY: no arguments; returns the caller's host port name.
        *HOST.get_or_init(|| unsafe { mach_host_self() })
    }

    pub fn available_bytes() -> Option<u64> {
        // SAFETY: `stat` is plain data the call fills; `count` says its size
        // in natural-sized words, as the call requires.
        unsafe {
            let mut stat: libc::vm_statistics64 = std::mem::zeroed();
            let mut count = libc::HOST_VM_INFO64_COUNT;
            let status = libc::host_statistics64(
                host(),
                libc::HOST_VM_INFO64,
                &mut stat as *mut libc::vm_statistics64 as *mut libc::integer_t,
                &mut count,
            );
            if status != libc::KERN_SUCCESS {
                return None;
            }
            let page = libc::sysconf(libc::_SC_PAGESIZE);
            if page <= 0 {
                return None;
            }
            let pages = u64::from(stat.free_count) + u64::from(stat.inactive_count);
            Some(pages.saturating_mul(page as u64))
        }
    }
}

/// IOKit's power sources: the internal battery's capacity and state.
#[cfg(target_os = "macos")]
mod mac_battery {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    use super::BatteryStats;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> core_foundation::array::CFArrayRef;
        fn IOPSGetPowerSourceDescription(blob: CFTypeRef, source: CFTypeRef) -> CFDictionaryRef;
    }

    pub fn read() -> Option<BatteryStats> {
        // SAFETY: Copy-rule results are owned and released by their
        // wrappers; the description is Get-rule and retained by its wrapper
        // while `info` is alive.
        unsafe {
            let info = IOPSCopyPowerSourcesInfo();
            if info.is_null() {
                return None;
            }
            let info = CFType::wrap_under_create_rule(info);
            let list = IOPSCopyPowerSourcesList(info.as_CFTypeRef());
            if list.is_null() {
                return None;
            }
            let list: CFArray<CFType> = CFArray::wrap_under_create_rule(list);
            for source in list.iter() {
                let description =
                    IOPSGetPowerSourceDescription(info.as_CFTypeRef(), source.as_CFTypeRef());
                if description.is_null() {
                    continue;
                }
                let description: CFDictionary<CFString, CFType> =
                    CFDictionary::wrap_under_get_rule(description);
                let get = |key: &'static str| {
                    description
                        .find(&CFString::from_static_string(key))
                        .map(|v| (*v).clone())
                };
                let text = |key| {
                    get(key)
                        .and_then(|v| v.downcast::<CFString>())
                        .map(|s| s.to_string())
                };
                let number = |key| {
                    get(key)
                        .and_then(|v| v.downcast::<CFNumber>())
                        .and_then(|n| n.to_f64())
                };
                let flag = |key| {
                    get(key)
                        .and_then(|v| v.downcast::<CFBoolean>())
                        .map(bool::from)
                };
                if text("Type").as_deref() != Some("InternalBattery") {
                    continue;
                }
                let percent = match (number("Current Capacity"), number("Max Capacity")) {
                    (Some(current), Some(max)) if max > 0.0 => {
                        Some((current / max * 100.0).clamp(0.0, 100.0).round())
                    }
                    (Some(current), _) => Some(current.clamp(0.0, 100.0)),
                    _ => None,
                };
                return Some(BatteryStats {
                    percent,
                    charging: flag("Is Charging"),
                    on_ac_power: text("Power Source State").map(|s| s == "AC Power"),
                });
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(mount: &str, fs: &str, total: u64, available: u64) -> Mounted {
        Mounted {
            mount_point: PathBuf::from(mount),
            name: "/dev/disk3s1".into(),
            file_system: fs.into(),
            kind: None,
            removable: false,
            total,
            available,
        }
    }

    #[test]
    fn collecting_on_this_machine_never_fails_and_is_fast() {
        let dir = tempfile::tempdir().unwrap();
        let places = Places {
            data_dir: Some(dir.path().join("not-made-yet")),
            home: std::env::var_os("HOME").map(PathBuf::from),
            build: None,
        };
        let started = Instant::now();
        let stats = collect(&places);
        let took = started.elapsed();
        assert!(
            took < Duration::from_millis(1500),
            "collection took {took:?}"
        );
        assert_eq!(stats.zeron.engine_version, env!("CARGO_PKG_VERSION"));
        assert!(stats.collected_at.is_some());
        let os = serde_json::to_value(&stats.os).unwrap();
        assert!(os.get("hostname").is_none(), "no hostname: {os}");
        if sysinfo::IS_SUPPORTED_SYSTEM {
            assert!(stats.memory.total_bytes.is_some_and(|t| t > 0), "{stats:?}");
            // Some memory is always free or reclaimable on a running machine,
            // however busy (a Mac whose compressor holds more than that read 0
            // through sysinfo).
            let total = stats.memory.total_bytes.unwrap_or_default();
            assert!(
                stats
                    .memory
                    .available_bytes
                    .is_some_and(|a| a > 0 && a <= total),
                "available memory: {:?} of {total}",
                stats.memory.available_bytes
            );
            assert!(stats.cpu.logical_cores.is_some_and(|n| n > 0), "{stats:?}");
            let data = stats
                .storage
                .zeron_data
                .as_ref()
                .expect("the temp dir's volume");
            assert!(data.total_bytes >= data.available_bytes);
        }
        // Mount points only: the folders asked about never show up.
        let json = serde_json::to_string(&stats).unwrap();
        assert!(!json.contains("not-made-yet"), "{json}");
        if let Some(home) = places.home.as_ref().filter(|h| h.components().count() > 2) {
            assert!(!json.contains(&*home.to_string_lossy()), "{json}");
        }
    }

    #[test]
    fn an_empty_report_serializes_every_field_as_null() {
        let value = serde_json::to_value(DeviceStats::default()).unwrap();
        assert!(value["os"]["name"].is_null());
        assert!(value["cpu"]["model"].is_null());
        assert!(value["memory"]["totalBytes"].is_null());
        assert!(value["storage"]["zeronData"].is_null());
        assert_eq!(value["storage"]["volumes"], serde_json::json!([]));
        assert!(value["gpus"].is_null());
        assert!(value["battery"].is_null());
        assert!(value["loadAverage"].is_null());
        // And it reads back from a report with fields missing (an older or
        // newer engine's).
        let back: DeviceStats = serde_json::from_value(serde_json::json!({
            "os": {}, "cpu": {}, "memory": {}, "storage": { "volumes": [] }, "zeron": { "engineVersion": "0.1.0" }
        }))
        .unwrap();
        assert_eq!(back.zeron.engine_version, "0.1.0");
    }

    #[test]
    fn volumes_report_mount_points_and_labels_but_never_device_paths() {
        let mut labelled = volume("/Volumes/Backup", "apfs", 100, 40);
        labelled.name = "Backup".into();
        let report = labelled.report();
        assert_eq!(report.name.as_deref(), Some("Backup"));
        assert_eq!(report.used_bytes, 60);
        let report = volume("/", "apfs", 100, 40).report();
        assert_eq!(report.name, None, "a /dev path is not a label");
        let mut windows = volume("C:\\", "NTFS", 100, 40);
        windows.name = "\\\\?\\Volume{1234}".into();
        assert_eq!(windows.report().name, None);
        // Available above total (a filesystem quirk) never makes used negative.
        let odd = volume("/", "apfs", 10, 20).report();
        assert_eq!((odd.available_bytes, odd.used_bytes), (10, 0));
    }

    #[test]
    fn system_internal_and_pseudo_volumes_are_left_out_of_the_list() {
        let mounted = [
            volume("/", "apfs", 100, 50),
            volume("/System/Volumes/Data", "apfs", 100, 50),
            volume("/System/Volumes/VM", "apfs", 100, 50),
            volume("/System/Volumes/Preboot", "apfs", 100, 50),
            volume("/snap/core/1", "squashfs", 100, 0),
            volume("/run/user", "tmpfs", 100, 100),
            volume("/empty", "apfs", 0, 0),
            volume("/Volumes/Backup", "exfat", 100, 50),
        ];
        let places = Places {
            data_dir: None,
            home: None,
            build: None,
        };
        let listed: Vec<String> = storage(&mounted, &places)
            .volumes
            .into_iter()
            .map(|v| v.mount_point)
            .collect();
        assert_eq!(listed, ["/", "/System/Volumes/Data", "/Volumes/Backup"]);
    }

    #[test]
    fn a_folder_lands_on_the_longest_matching_mount_point() {
        // Paths that don't exist here, so only the prefix rule can answer.
        let mounted = [
            volume("/", "apfs", 100, 50),
            volume("/zeron-test-mount", "ext4", 10, 5),
        ];
        let places = Places {
            data_dir: Some("/zeron-test-mount/data/zeron".into()),
            home: None,
            build: None,
        };
        let storage = storage(&mounted, &places);
        // On Unix the nearest existing ancestor is `/`, the same filesystem
        // as the `/` mount: that answer wins.
        let data = storage.zeron_data.expect("a volume");
        assert!(["/", "/zeron-test-mount"].contains(&data.mount_point.as_str()));
        assert!(storage.home.is_none(), "no home asked about, none answered");
        // No volumes listed at all: nothing to answer with, not a failure.
        let none = super::storage(&[], &places);
        assert!(none.zeron_data.is_none() && none.volumes.is_empty());
    }

    #[test]
    fn linux_batteries_and_nvidia_gpus_read_from_sysfs_and_proc() {
        let root = tempfile::tempdir().unwrap();
        let supply = |name: &str, files: &[(&str, &str)]| {
            let dir = root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for (file, text) in files {
                std::fs::write(dir.join(file), format!("{text}\n")).unwrap();
            }
        };
        assert_eq!(linux_battery(root.path()), None, "no supplies, no battery");
        supply("AC", &[("type", "Mains"), ("online", "1")]);
        assert_eq!(linux_battery(root.path()), None, "mains only, no battery");
        supply(
            "BAT0",
            &[
                ("type", "Battery"),
                ("capacity", "87"),
                ("status", "Charging"),
            ],
        );
        assert_eq!(
            linux_battery(root.path()),
            Some(BatteryStats {
                percent: Some(87.0),
                charging: Some(true),
                on_ac_power: Some(true)
            })
        );
        // A battery missing every optional file still reports, all null.
        let bare = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(bare.path().join("BAT1")).unwrap();
        std::fs::write(bare.path().join("BAT1/type"), "Battery").unwrap();
        assert_eq!(linux_battery(bare.path()), Some(BatteryStats::default()));

        let gpus = tempfile::tempdir().unwrap();
        assert!(nvidia_gpus(&gpus.path().join("missing")).is_empty());
        for (bus, model) in [
            ("0000:01:00.0", "NVIDIA GeForce RTX 4090"),
            ("0000:02:00.0", "NVIDIA A100"),
        ] {
            let dir = gpus.path().join(bus);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("information"),
                format!("Model: \t\t {model}\nIRQ: 1\n"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(gpus.path().join("broken")).unwrap();
        assert_eq!(
            nvidia_gpus(gpus.path()),
            ["NVIDIA A100", "NVIDIA GeForce RTX 4090"]
        );
    }

    #[tokio::test]
    async fn a_report_is_reused_for_a_few_seconds() {
        let places = Places {
            data_dir: None,
            home: None,
            build: Some("abc".into()),
        };
        let first = current(places.clone()).await;
        let second = current(places).await;
        assert_eq!(
            first.collected_at, second.collected_at,
            "the second call reuses the first"
        );
    }
}
