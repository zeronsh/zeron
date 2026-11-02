//! Optional package-energy reader.
//!
//! [`sample_package`] returns `None` when `energy_uj` cannot be read. It does
//! not change permissions and it does not explain how to make the file
//! readable. Power is not a bench metric; CPU time, instructions, and
//! wakeups are. The delta helpers exist so a later window can subtract two
//! samples if a trial directory supplies them.

use std::fs;
use std::path::Path;

/// One sample of a RAPL package energy counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaplSample {
    /// Microjoules from `energy_uj`.
    pub energy_uj: u64,
    /// Wrap modulus from `max_energy_range_uj`, or zero when that file is absent.
    pub max_energy_range_uj: u64,
}

/// Read `intel-rapl:0` under a `powercap` directory.
///
/// `powercap` is `/sys/class/powercap` or a fake tree with the same shape.
/// `None` when `energy_uj` is missing or not readable.
pub fn read_package_at(powercap: &Path) -> Option<RaplSample> {
    let domain = powercap.join("intel-rapl:0");
    let energy_uj = read_uj(&domain.join("energy_uj"))?;
    let max_energy_range_uj = read_uj(&domain.join("max_energy_range_uj")).unwrap_or(0);
    Some(RaplSample {
        energy_uj,
        max_energy_range_uj,
    })
}

/// The live package counter. `None` when the sysfs file cannot be read.
pub fn sample_package() -> Option<RaplSample> {
    read_package_at(Path::new("/sys/class/powercap"))
}

/// Microjoules added between two readings of the same counter.
///
/// When `end < start` the counter wrapped through `max_energy_range_uj`.
/// `None` when a wrap is required and the modulus is unknown.
pub fn energy_delta_uj(start: u64, end: u64, max_energy_range_uj: u64) -> Option<u64> {
    if end >= start {
        return Some(end - start);
    }
    if max_energy_range_uj < start {
        return None;
    }
    Some(max_energy_range_uj - start + end)
}

/// Joules above an idle baseline: `delta_uj / 1e6 - idle_watts * wall_secs`.
///
/// Pure arithmetic. This unit does not measure `idle_watts`.
pub fn energy_j_net(delta_uj: u64, idle_watts: f64, wall_secs: f64) -> f64 {
    (delta_uj as f64) / 1_000_000.0 - idle_watts * wall_secs
}

fn read_uj(path: &Path) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(name: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::path::PathBuf::from("/home/user")
            .join("Documents/dev/tmp/zeron-perf/scratch/b11-perf-counted/rapl")
            .join(format!("{name}-{n}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("intel-rapl:0")).unwrap();
        dir
    }

    fn write_counter(dir: &Path, energy: &str, max: Option<&str>) {
        let domain = dir.join("intel-rapl:0");
        fs::write(domain.join("energy_uj"), energy).unwrap();
        if let Some(max) = max {
            fs::write(domain.join("max_energy_range_uj"), max).unwrap();
        }
    }

    #[test]
    fn missing_sysfs_is_none() {
        let dir = std::path::PathBuf::from(
            "/home/user/Documents/dev/tmp/zeron-perf/scratch/b11-perf-counted/rapl-missing",
        );
        let _ = fs::remove_dir_all(&dir);
        assert!(read_package_at(&dir).is_none());
    }

    #[test]
    fn unreadable_energy_is_none() {
        let dir = scratch("unreadable");
        write_counter(&dir, "10\n", Some("100\n"));
        let energy = dir.join("intel-rapl:0/energy_uj");
        fs::set_permissions(&energy, fs::Permissions::from_mode(0o000)).unwrap();
        assert!(read_package_at(&dir).is_none());
        fs::set_permissions(&energy, fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn fake_sysfs_reads_energy_and_range() {
        let dir = scratch("readable");
        write_counter(&dir, "51500892464\n", Some("65532610987\n"));
        let sample = read_package_at(&dir).unwrap();
        assert_eq!(sample.energy_uj, 51_500_892_464);
        assert_eq!(sample.max_energy_range_uj, 65_532_610_987);
    }

    #[test]
    fn energy_without_a_range_still_reads() {
        let dir = scratch("no-max");
        write_counter(&dir, "42\n", None);
        let sample = read_package_at(&dir).unwrap();
        assert_eq!(sample.energy_uj, 42);
        assert_eq!(sample.max_energy_range_uj, 0);
    }

    #[test]
    fn garbage_energy_is_none() {
        let dir = scratch("garbage");
        write_counter(&dir, "nope\n", Some("10\n"));
        assert!(read_package_at(&dir).is_none());
    }

    #[test]
    fn delta_handles_wrap() {
        assert_eq!(energy_delta_uj(100, 250, 1_000), Some(150));
        assert_eq!(energy_delta_uj(9_000, 1_500, 10_000), Some(2_500));
        assert_eq!(energy_delta_uj(9_000, 1_500, 0), None);
        assert_eq!(energy_delta_uj(5, 5, 10), Some(0));
    }

    #[test]
    fn net_joules_subtract_the_idle_baseline() {
        let net = energy_j_net(2_000_000, 1.0, 0.5);
        assert!((net - 1.5).abs() < 1e-9, "{net}");
    }

    #[test]
    fn live_package_follows_whether_the_file_opens() {
        let path = Path::new("/sys/class/powercap/intel-rapl:0/energy_uj");
        let readable = fs::File::open(path).is_ok();
        assert_eq!(sample_package().is_some(), readable);
    }
}
