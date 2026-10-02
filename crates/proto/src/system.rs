//! Live resource usage of one device, streamed by `WatchSystemStats`.
//!
//! Every field a peer might not be able to measure is optional or a possibly
//! empty list, so older and newer engines (and platforms without a given
//! probe) still parse each other's frames.

use serde::{Deserialize, Serialize};

/// Params of `WatchSystemStats`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchSystemStatsRequest {
    /// Milliseconds between frames. The engine clamps this to a sane range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
}

/// One sampled frame. Rates are per second, measured over the interval since
/// the previous frame of the same subscription.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStats {
    /// Milliseconds since the Unix epoch on the sampled device.
    #[serde(default)]
    pub sampled_at_ms: u64,
    #[serde(default)]
    pub host: SystemHost,
    #[serde(default)]
    pub cpu: CpuStats,
    #[serde(default)]
    pub memory: MemoryStats,
    #[serde(default)]
    pub gpus: Vec<GpuStats>,
    #[serde(default)]
    pub network: NetworkStats,
    #[serde(default)]
    pub disks: DiskStats,
    /// The busiest processes, heaviest CPU first.
    #[serde(default)]
    pub processes: Vec<ProcessStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemHost {
    #[serde(default)]
    pub name: String,
    /// "macOS 15.2", "Ubuntu 24.04", …
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuStats {
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub physical_cores: Option<u32>,
    /// Whole-machine utilization, 0–100.
    #[serde(default)]
    pub total_percent: f32,
    /// One entry per logical core, each 0–100.
    #[serde(default)]
    pub per_core_percent: Vec<f32>,
    /// 1, 5 and 15 minute load averages. Absent on Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_average: Option<[f64; 3]>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub used_bytes: u64,
    #[serde(default)]
    pub swap_total_bytes: u64,
    #[serde(default)]
    pub swap_used_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    #[serde(default)]
    pub name: String,
    /// 0–100, when the driver reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_used_bytes: Option<u64>,
    /// Dedicated memory. Absent on unified-memory GPUs, which draw on RAM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_celsius: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStats {
    /// Sum over every non-loopback interface.
    #[serde(default)]
    pub received_per_sec: u64,
    #[serde(default)]
    pub transmitted_per_sec: u64,
    /// Interfaces that have moved traffic since boot, busiest first.
    #[serde(default)]
    pub interfaces: Vec<NetworkInterfaceStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterfaceStats {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub received_per_sec: u64,
    #[serde(default)]
    pub transmitted_per_sec: u64,
    #[serde(default)]
    pub total_received_bytes: u64,
    #[serde(default)]
    pub total_transmitted_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskStats {
    /// Sum over every physical disk. `None` when the platform can't report I/O.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_per_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_per_sec: Option<u64>,
    /// User-visible volumes; pseudo and duplicate mounts are dropped.
    #[serde(default)]
    pub volumes: Vec<DiskVolume>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskVolume {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mount_point: String,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessStats {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    /// Share of one core, so a busy multi-threaded process can exceed 100.
    #[serde(default)]
    pub cpu_percent: f32,
    #[serde(default)]
    pub memory_bytes: u64,
}
