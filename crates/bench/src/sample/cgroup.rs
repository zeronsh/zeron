//! cgroup v2 files for one transient scope.
//!
//! The sampler reads these from outside the scope. `memory.peak` is reset at
//! the start of a window when the kernel allows the write (6.12 and later).
//! A failed write is ignored: the sampled high-water mark still works, it
//! just is not cleared back to the usage at the mark.

use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// A cgroup directory the sampler may read, and whose `memory.peak` it may reset.
///
/// The directory fd is held open so later reads cannot be pointed somewhere
/// else by swapping the path.
#[derive(Debug)]
pub struct Cgroup {
    path: PathBuf,
    dir: OwnedFd,
}

/// `cpu.stat` usage counters, microseconds, cumulative since the cgroup was created.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuStat {
    pub usage_usec: u64,
    pub user_usec: u64,
    pub system_usec: u64,
}

/// One PSI file (`cpu.pressure` or `memory.pressure`).
///
/// `some_total_us` / `full_total_us` are the `total=` fields, microseconds
/// since boot. `avg10` is the recent stall percent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pressure {
    pub some_avg10: f64,
    pub some_total_us: u64,
    pub full_avg10: f64,
    pub full_total_us: u64,
}

/// One sample of the scope, missing fields when a file cannot be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CgroupSnap {
    pub cpu: Option<CpuStat>,
    pub mem_current: Option<u64>,
    pub mem_peak: Option<u64>,
    pub anon: Option<u64>,
    pub cpu_pressure: Option<Pressure>,
    pub mem_pressure: Option<Pressure>,
    pub procs: Vec<u32>,
}

impl Cgroup {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let path = dir.into();
        if !path.is_dir() {
            bail!("{} is not a cgroup directory", path.display());
        }
        let c_path = CString::new(path.as_os_str().as_bytes())
            .with_context(|| format!("cgroup path {} contains NUL", path.display()))?;
        // SAFETY: `c_path` is NUL-terminated. `open` returns a new fd or -1.
        // `O_DIRECTORY` refuses a file. The fd stays owned by `Cgroup`.
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error())
                .with_context(|| format!("open {}", path.display()));
        }
        let dir = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Self { path, dir })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> CgroupSnap {
        let mut buf = Vec::with_capacity(4096);
        let cpu = self
            .read_text(c"cpu.stat", &mut buf)
            .map(|text| parse_cpu_stat(text));
        let mem_current = self
            .read_text(c"memory.current", &mut buf)
            .and_then(parse_memory_bytes);
        let mem_peak = self
            .read_text(c"memory.peak", &mut buf)
            .and_then(parse_memory_bytes);
        let anon = self
            .read_text(c"memory.stat", &mut buf)
            .and_then(parse_memory_stat_anon);
        let cpu_pressure = self
            .read_text(c"cpu.pressure", &mut buf)
            .and_then(parse_pressure);
        let mem_pressure = self
            .read_text(c"memory.pressure", &mut buf)
            .and_then(parse_pressure);
        let procs = self
            .read_text(c"cgroup.procs", &mut buf)
            .map(parse_cgroup_procs)
            .unwrap_or_default();
        CgroupSnap {
            cpu,
            mem_current,
            mem_peak,
            anon,
            cpu_pressure,
            mem_pressure,
            procs,
        }
    }

    /// Ask the kernel to set `memory.peak` back to the current usage.
    ///
    /// Returns whether the write happened. Kernels before 6.12, and a
    /// directory that is not a real cgroup, return false.
    pub fn reset_memory_peak(&self) -> bool {
        let fd = unsafe {
            libc::openat(
                self.dir.as_raw_fd(),
                c"memory.peak".as_ptr(),
                libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return false;
        }
        // SAFETY: `fd` is a new writable fd. It is closed when `file` drops.
        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        if file.write_all(b"0\n").is_err() {
            return false;
        }
        // cgroup files replace their contents on write. A plain file keeps
        // the unread tail (`999\n` would become `0\n9\n`), so drop it.
        let _ = file.set_len(2);
        true
    }

    fn read_text<'a>(&self, name: &std::ffi::CStr, buf: &'a mut Vec<u8>) -> Option<&'a str> {
        let file = open_at_read(self.dir.as_raw_fd(), name).ok()?;
        buf.clear();
        if super::proc::reread(file.as_raw_fd(), buf).is_err() {
            return None;
        }
        super::proc::text_of(buf)
    }
}

pub fn parse_cpu_stat(text: &str) -> CpuStat {
    let mut stat = CpuStat::default();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(key) = fields.next() else { continue };
        let Some(value) = fields.next().and_then(|v| v.parse().ok()) else {
            continue;
        };
        match key {
            "usage_usec" => stat.usage_usec = value,
            "user_usec" => stat.user_usec = value,
            "system_usec" => stat.system_usec = value,
            _ => {}
        }
    }
    stat
}

pub fn parse_memory_bytes(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

/// `anon` from `memory.stat`. This is the anonymous footprint; `memory.current`
/// also counts page cache.
pub fn parse_memory_stat_anon(text: &str) -> Option<u64> {
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() == Some("anon") {
            return fields.next()?.parse().ok();
        }
    }
    None
}

pub fn parse_pressure(text: &str) -> Option<Pressure> {
    let mut pressure = Pressure::default();
    let mut saw = false;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(kind) = fields.next() else { continue };
        let mut avg10 = None;
        let mut total = None;
        for field in fields {
            if let Some(value) = field.strip_prefix("avg10=") {
                avg10 = value.parse().ok();
            } else if let Some(value) = field.strip_prefix("total=") {
                total = value.parse().ok();
            }
        }
        let (Some(avg10), Some(total)) = (avg10, total) else {
            continue;
        };
        saw = true;
        match kind {
            "some" => {
                pressure.some_avg10 = avg10;
                pressure.some_total_us = total;
            }
            "full" => {
                pressure.full_avg10 = avg10;
                pressure.full_total_us = total;
            }
            _ => {}
        }
    }
    saw.then_some(pressure)
}

pub fn parse_cgroup_procs(text: &str) -> Vec<u32> {
    text.lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid| *pid > 1)
        .collect()
}

fn open_at_read(dir: std::os::fd::RawFd, name: &std::ffi::CStr) -> io::Result<OwnedFd> {
    let fd = unsafe {
        libc::openat(
            dir,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_stat_fixture_reads_usage_user_and_system() {
        let text = "\
usage_usec 5000000
user_usec 4000000
system_usec 1000000
nice_usec 0
";
        let got = parse_cpu_stat(text);
        assert_eq!(got.usage_usec, 5_000_000);
        assert_eq!(got.user_usec, 4_000_000);
        assert_eq!(got.system_usec, 1_000_000);
    }

    #[test]
    fn memory_stat_fixture_reads_anon_not_file() {
        let text = "\
anon 1048576
file 4096
kernel 8192
";
        assert_eq!(parse_memory_stat_anon(text), Some(1_048_576));
        assert_eq!(parse_memory_bytes("2097152\n"), Some(2_097_152));
    }

    #[test]
    fn pressure_fixture_reads_some_and_full() {
        let text = "\
some avg10=1.50 avg60=0.10 avg300=0.01 total=1000
full avg10=0.25 avg60=0.00 avg300=0.00 total=40
";
        let got = parse_pressure(text).unwrap();
        assert_eq!(got.some_avg10, 1.5);
        assert_eq!(got.some_total_us, 1000);
        assert_eq!(got.full_avg10, 0.25);
        assert_eq!(got.full_total_us, 40);
    }

    #[test]
    fn procs_fixture_skips_the_header_and_pid_one() {
        // cgroup.procs is one pid per line. A blank and a 1 are ignored.
        let text = "1\n42\n\n43\n";
        assert_eq!(parse_cgroup_procs(text), vec![42, 43]);
    }

    #[test]
    fn reset_memory_peak_writes_the_file_in_a_scratch_dir() {
        let dir = scratch_cgroup_dir("reset-peak");
        fs::write(dir.join("memory.peak"), b"999\n").unwrap();
        let cgroup = Cgroup::open(&dir).unwrap();
        assert!(cgroup.reset_memory_peak());
        let text = fs::read_to_string(dir.join("memory.peak")).unwrap();
        assert_eq!(text, "0\n");
        fs::write(
            dir.join("cpu.stat"),
            b"usage_usec 10\nuser_usec 8\nsystem_usec 2\n",
        )
        .unwrap();
        fs::write(dir.join("memory.current"), b"100\n").unwrap();
        fs::write(dir.join("memory.peak"), b"250\n").unwrap();
        fs::write(dir.join("memory.stat"), b"anon 80\nfile 20\n").unwrap();
        fs::write(
            dir.join("cpu.pressure"),
            b"some avg10=0.00 avg60=0.00 avg300=0.00 total=5\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=1\n",
        )
        .unwrap();
        fs::write(dir.join("cgroup.procs"), b"42\n").unwrap();
        let snap = cgroup.snapshot();
        assert_eq!(snap.cpu.unwrap().usage_usec, 10);
        assert_eq!(snap.mem_current, Some(100));
        assert_eq!(snap.mem_peak, Some(250));
        assert_eq!(snap.anon, Some(80));
        assert_eq!(snap.procs, vec![42]);
        assert_eq!(snap.cpu_pressure.unwrap().some_total_us, 5);
        let _ = fs::remove_dir_all(&dir);
    }

    fn scratch_cgroup_dir(name: &str) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".into());
        let dir = PathBuf::from(home)
            .join("Documents/dev/tmp/zeron-perf/scratch/b2-sampler/cgroup")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
