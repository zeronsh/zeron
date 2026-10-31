//! Parsers for the `/proc` text the sampler reads.
//!
//! The live sampler keeps these files open and re-reads them. Tests feed the
//! same parsers synthetic fixtures, so a formatting change shows up without a
//! child process.

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Resident and proportional set from `/proc/<pid>/smaps_rollup`, in KiB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SmapsRollup {
    pub rss_kib: u64,
    pub pss_kib: u64,
    pub pss_anon_kib: u64,
    pub pss_file_kib: u64,
    pub swap_kib: u64,
    pub swap_pss_kib: u64,
}

/// `/proc/<pid>/task/<tid>/schedstat`. Field 3 is the wakeup counter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Schedstat {
    pub run_ns: u64,
    pub wait_ns: u64,
    pub timeslices: u64,
}

/// `/proc/<pid>/io` counters. All of them are cumulative since exec.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcIo {
    pub rchar: u64,
    pub wchar: u64,
    pub syscr: u64,
    pub syscw: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub cancelled_write_bytes: u64,
}

/// The fields of `/proc/<pid>/stat` the sampler needs.
///
/// `utime` and the other times are clock ticks (`_SC_CLK_TCK`), not
/// microseconds. `comm` is the 15-byte task name and may contain spaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcStat {
    pub pid: u32,
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    pub pgrp: u32,
    pub utime: u64,
    pub stime: u64,
    pub cutime: u64,
    pub cstime: u64,
    pub num_threads: u32,
    pub starttime: u64,
}

/// Context-switch counters from `/proc/<pid>/task/<tid>/status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusSwitches {
    pub voluntary: u64,
    pub nonvoluntary: u64,
}

/// One `inotify` watch line from `/proc/<pid>/fdinfo/<fd>`.
///
/// `ino` and `sdev` identify the watched inode. Two lines with the same pair
/// are the same watch installed twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InotifyWatch {
    pub wd: u32,
    pub ino: u64,
    pub sdev: u64,
}

/// Totals across every inotify fdinfo in the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InotifyCount {
    pub watches: u32,
    /// `watches - unique (ino, sdev)`.
    pub dup: u32,
}

/// First line of `/proc/stat` (`cpu …`), in jiffies, summed across cores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MachineCpu {
    /// `user + nice + system + irq + softirq + steal`.
    /// Guest time is already inside `user` and is not added again.
    pub busy_jiffies: u64,
    pub total_jiffies: u64,
}

/// glibc `HEAP_MAX_SIZE` on 64-bit: non-main arenas are reserved in slots of
/// this size, aligned to the same value (`heap_for_ptr` masks it off).
pub const ARENA_HEAP_ALIGN: u64 = 64 * 1024 * 1024;

pub fn parse_smaps_rollup(text: &str) -> SmapsRollup {
    parse_smaps_rollup_bytes(text.as_bytes())
}

pub(crate) fn parse_smaps_rollup_bytes(buf: &[u8]) -> SmapsRollup {
    // `Pss:` does not match `Pss_Anon:`. The colon is part of the needle.
    SmapsRollup {
        rss_kib: labeled_u64(buf, b"Rss:"),
        pss_kib: labeled_u64(buf, b"Pss:"),
        pss_anon_kib: labeled_u64(buf, b"Pss_Anon:"),
        pss_file_kib: labeled_u64(buf, b"Pss_File:"),
        swap_kib: labeled_u64(buf, b"Swap:"),
        swap_pss_kib: labeled_u64(buf, b"SwapPss:"),
    }
}

pub fn parse_schedstat(text: &str) -> Option<Schedstat> {
    parse_schedstat_bytes(text.as_bytes())
}

pub(crate) fn parse_schedstat_bytes(buf: &[u8]) -> Option<Schedstat> {
    let mut rest = buf;
    Some(Schedstat {
        run_ns: take_ticks(&mut rest)?,
        wait_ns: take_ticks(&mut rest)?,
        timeslices: take_ticks(&mut rest)?,
    })
}

pub fn parse_io(text: &str) -> ProcIo {
    parse_io_bytes(text.as_bytes())
}

pub(crate) fn parse_io_bytes(buf: &[u8]) -> ProcIo {
    ProcIo {
        rchar: labeled_u64(buf, b"rchar:"),
        wchar: labeled_u64(buf, b"wchar:"),
        syscr: labeled_u64(buf, b"syscr:"),
        syscw: labeled_u64(buf, b"syscw:"),
        read_bytes: labeled_u64(buf, b"read_bytes:"),
        write_bytes: labeled_u64(buf, b"write_bytes:"),
        cancelled_write_bytes: labeled_u64(buf, b"cancelled_write_bytes:"),
    }
}

pub fn parse_stat(text: &str) -> Option<ProcStat> {
    let fields = parse_stat_view(text.as_bytes())?;
    let comm = std::str::from_utf8(fields.comm).ok()?.to_string();
    Some(ProcStat {
        pid: fields.pid,
        comm,
        state: fields.state,
        ppid: fields.ppid,
        pgrp: fields.pgrp,
        utime: fields.utime,
        stime: fields.stime,
        cutime: fields.cutime,
        cstime: fields.cstime,
        num_threads: fields.num_threads,
        starttime: fields.starttime,
    })
}

/// Numeric `/proc/<pid>/stat` fields plus the comm bytes inside `buf`.
pub(crate) struct BorrowedStat<'a> {
    pub pid: u32,
    pub state: char,
    pub ppid: u32,
    pub pgrp: u32,
    pub utime: u64,
    pub stime: u64,
    pub cutime: u64,
    pub cstime: u64,
    pub num_threads: u32,
    pub starttime: u64,
    pub comm: &'a [u8],
}

pub(crate) fn parse_stat_view(buf: &[u8]) -> Option<BorrowedStat<'_>> {
    let open = mem_chr(buf, b'(')?;
    let close = open + 1 + mem_rchr(&buf[open + 1..], b')')?;
    if close <= open {
        return None;
    }
    let pid = parse_ascii_u32(trim_ascii(&buf[..open]))?;
    let comm = &buf[open + 1..close];
    let mut rest = trim_ascii(&buf[close + 1..]);
    let state = rest.first().copied().unwrap_or(b'?') as char;
    rest = skip_field(rest)?;
    let ppid = take_u32(&mut rest)?;
    let pgrp = take_u32(&mut rest)?;
    // session tty_nr tpgid flags minflt cminflt majflt cmajflt, then the times.
    for _ in 0..8 {
        rest = skip_field(rest)?;
    }
    let utime = take_ticks(&mut rest)?;
    let stime = take_ticks(&mut rest)?;
    let cutime = take_ticks(&mut rest)?;
    let cstime = take_ticks(&mut rest)?;
    rest = skip_field(rest)?;
    rest = skip_field(rest)?;
    let num_threads = take_u32(&mut rest)?;
    rest = skip_field(rest)?;
    let starttime = take_ticks(&mut rest)?;
    Some(BorrowedStat {
        pid,
        state,
        ppid,
        pgrp,
        utime,
        stime,
        cutime,
        cstime,
        num_threads,
        starttime,
        comm,
    })
}

pub fn parse_status_switches(text: &str) -> StatusSwitches {
    parse_status_switches_bytes(text.as_bytes())
}

pub(crate) fn parse_status_switches_bytes(buf: &[u8]) -> StatusSwitches {
    // The newline keeps `voluntary_` from matching inside `nonvoluntary_`.
    StatusSwitches {
        voluntary: labeled_u64(buf, b"voluntary_ctxt_switches:"),
        nonvoluntary: labeled_u64(buf, b"nonvoluntary_ctxt_switches:"),
    }
}

/// Watch lines from one fdinfo file. Lines that are not `inotify` records
/// are ignored. `ino` and `sdev` are hex, as the kernel prints them.
pub fn parse_inotify_fdinfo(text: &str) -> Vec<InotifyWatch> {
    let mut watches = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("inotify ") {
            continue;
        }
        let mut wd = None;
        let mut ino = None;
        let mut sdev = None;
        for token in line.split_whitespace().skip(1) {
            let Some((key, value)) = token.split_once(':') else {
                continue;
            };
            match key {
                "wd" => wd = value.parse().ok(),
                "ino" => ino = u64::from_str_radix(value, 16).ok(),
                "sdev" => sdev = u64::from_str_radix(value, 16).ok(),
                _ => {}
            }
        }
        if let (Some(ino), Some(sdev)) = (ino, sdev) {
            watches.push(InotifyWatch {
                wd: wd.unwrap_or(0),
                ino,
                sdev,
            });
        }
    }
    watches
}

pub fn inotify_summary(watches: impl IntoIterator<Item = InotifyWatch>) -> InotifyCount {
    use std::collections::HashSet;
    let mut unique = HashSet::new();
    let mut total = 0u32;
    for watch in watches {
        total = total.saturating_add(1);
        unique.insert((watch.ino, watch.sdev));
    }
    let distinct = u32::try_from(unique.len()).unwrap_or(u32::MAX);
    InotifyCount {
        watches: total,
        dup: total.saturating_sub(distinct),
    }
}

/// Count glibc non-main arena reservations in `/proc/<pid>/maps`.
///
/// A reservation is an anonymous private mapping whose start is aligned to
/// [`ARENA_HEAP_ALIGN`] and whose length is at most one heap slot. The main
/// `[heap]` and every named or file-backed mapping are not arenas. A heap
/// that the kernel split into an `rw` prefix and a `---` tail counts once,
/// because only the aligned start matches.
pub fn count_arena_heaps(maps: &str) -> u32 {
    let mut count = 0u32;
    for line in maps.lines() {
        if is_arena_heap_line(line) {
            count = count.saturating_add(1);
        }
    }
    count
}

pub fn parse_machine_cpu(text: &str) -> Option<MachineCpu> {
    for line in text.lines() {
        let line = line.trim();
        // `cpu0` is one core. The aggregate line is `cpu `.
        if !line.starts_with("cpu ") {
            continue;
        }
        let nums: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .map(|field| field.parse::<u64>().unwrap_or(0))
            .collect();
        if nums.len() < 4 {
            return None;
        }
        let user = nums.first().copied().unwrap_or(0);
        let nice = nums.get(1).copied().unwrap_or(0);
        let system = nums.get(2).copied().unwrap_or(0);
        let idle = nums.get(3).copied().unwrap_or(0);
        let iowait = nums.get(4).copied().unwrap_or(0);
        let irq = nums.get(5).copied().unwrap_or(0);
        let softirq = nums.get(6).copied().unwrap_or(0);
        let steal = nums.get(7).copied().unwrap_or(0);
        let busy = user
            .saturating_add(nice)
            .saturating_add(system)
            .saturating_add(irq)
            .saturating_add(softirq)
            .saturating_add(steal);
        return Some(MachineCpu {
            busy_jiffies: busy,
            total_jiffies: busy.saturating_add(idle).saturating_add(iowait),
        });
    }
    None
}

pub(crate) fn parse_children(text: &str) -> Vec<u32> {
    text.split_whitespace()
        .filter_map(|field| field.parse().ok())
        .filter(|pid| *pid > 1)
        .collect()
}

fn is_arena_heap_line(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }
    let mut parts = line.split_whitespace();
    let Some(range) = parts.next() else {
        return false;
    };
    let Some(perms) = parts.next() else {
        return false;
    };
    let Some(_offset) = parts.next() else {
        return false;
    };
    let Some(_dev) = parts.next() else {
        return false;
    };
    let Some(inode) = parts.next() else {
        return false;
    };
    let path = parts.next().unwrap_or("");
    if inode != "0" || !perms.contains('p') || perms.contains('s') {
        return false;
    }
    if !path.is_empty() {
        return false;
    }
    let Some((start_hex, end_hex)) = range.split_once('-') else {
        return false;
    };
    let Ok(start) = u64::from_str_radix(start_hex, 16) else {
        return false;
    };
    let Ok(end) = u64::from_str_radix(end_hex, 16) else {
        return false;
    };
    let size = end.saturating_sub(start);
    size > 0 && size <= ARENA_HEAP_ALIGN && start % ARENA_HEAP_ALIGN == 0
}

/// Value of a `Label:` line. `label` includes the colon. `Pss:` does not match
/// `Pss_Anon:`, and a leading newline keeps `voluntary_` out of `nonvoluntary_`.
fn labeled_u64(buf: &[u8], label: &[u8]) -> u64 {
    let pos = if buf.starts_with(label) {
        0
    } else {
        let mut needle = [0u8; 48];
        let nlen = label.len() + 1;
        if nlen > needle.len() {
            return 0;
        }
        needle[0] = b'\n';
        needle[1..nlen].copy_from_slice(label);
        let Some(at) = mem_find(buf, &needle[..nlen]) else {
            return 0;
        };
        at + 1
    };
    let Some(rest) = buf.get(pos + label.len()..) else {
        return 0;
    };
    leading_u64(rest).unwrap_or(0)
}

fn mem_find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    // SAFETY: both slices are valid for their lengths. `memmem` returns a
    // pointer into `hay` or NULL.
    let found = unsafe {
        libc::memmem(
            hay.as_ptr().cast::<libc::c_void>(),
            hay.len(),
            needle.as_ptr().cast::<libc::c_void>(),
            needle.len(),
        )
    };
    if found.is_null() {
        None
    } else {
        Some(unsafe { found.cast::<u8>().offset_from(hay.as_ptr()) as usize })
    }
}

fn mem_chr(hay: &[u8], byte: u8) -> Option<usize> {
    if hay.is_empty() {
        return None;
    }
    // SAFETY: `hay` is valid for its length. `memchr` returns a pointer into it or NULL.
    let found = unsafe {
        libc::memchr(
            hay.as_ptr().cast::<libc::c_void>(),
            byte as libc::c_int,
            hay.len(),
        )
    };
    if found.is_null() {
        None
    } else {
        Some(unsafe { found.cast::<u8>().offset_from(hay.as_ptr()) as usize })
    }
}

fn mem_rchr(hay: &[u8], byte: u8) -> Option<usize> {
    if hay.is_empty() {
        return None;
    }
    // SAFETY: `hay` is valid for its length. `memrchr` returns a pointer into it or NULL.
    let found = unsafe {
        libc::memrchr(
            hay.as_ptr().cast::<libc::c_void>(),
            byte as libc::c_int,
            hay.len(),
        )
    };
    if found.is_null() {
        None
    } else {
        Some(unsafe { found.cast::<u8>().offset_from(hay.as_ptr()) as usize })
    }
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = bytes {
        bytes = rest;
    }
    while let [rest @ .., b' ' | b'\t' | b'\n' | b'\r'] = bytes {
        bytes = rest;
    }
    bytes
}

fn leading_u64(bytes: &[u8]) -> Option<u64> {
    let bytes = trim_ascii(bytes);
    let mut n = 0u64;
    let mut saw = false;
    for &byte in bytes {
        if byte.is_ascii_digit() {
            saw = true;
            n = n.saturating_mul(10).saturating_add(u64::from(byte - b'0'));
        } else {
            break;
        }
    }
    saw.then_some(n)
}

fn parse_ascii_u32(bytes: &[u8]) -> Option<u32> {
    let n = leading_u64(bytes)?;
    u32::try_from(n).ok()
}

fn skip_field(bytes: &[u8]) -> Option<&[u8]> {
    let bytes = trim_ascii(bytes);
    let mut i = 0;
    if i >= bytes.len() {
        return None;
    }
    while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' && bytes[i] != b'\n' {
        i += 1;
    }
    Some(trim_ascii(&bytes[i..]))
}

fn take_u32(rest: &mut &[u8]) -> Option<u32> {
    let bytes = trim_ascii(rest);
    let mut i = 0;
    if i >= bytes.len() || !bytes[0].is_ascii_digit() {
        return None;
    }
    let mut n = 0u32;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        n = n
            .saturating_mul(10)
            .saturating_add(u32::from(bytes[i] - b'0'));
        i += 1;
    }
    *rest = trim_ascii(&bytes[i..]);
    Some(n)
}

fn take_ticks(rest: &mut &[u8]) -> Option<u64> {
    let bytes = trim_ascii(rest);
    let mut i = 0;
    if i >= bytes.len() {
        return None;
    }
    let neg = bytes[0] == b'-';
    if neg {
        i = 1;
    }
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }
    let mut n = 0u64;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        n = n
            .saturating_mul(10)
            .saturating_add(u64::from(bytes[i] - b'0'));
        i += 1;
    }
    *rest = trim_ascii(&bytes[i..]);
    if neg { Some(0) } else { Some(n) }
}

pub(crate) fn reread(fd: std::os::fd::RawFd, buf: &mut Vec<u8>) -> io::Result<()> {
    buf.clear();
    if buf.capacity() < 8192 {
        buf.reserve(8192);
    }
    // One pread at offset 0 restarts a seq_file. A short read is the whole
    // file, so the hot path is a single syscall. A full buffer falls back to
    // lseek+read; continuing pread at a non-zero offset is not reliable.
    let spare = buf.capacity();
    let n = pread_append(fd, buf, spare, 0)?;
    if n < spare {
        return Ok(());
    }
    buf.clear();
    read_seq(fd, buf)
}

fn pread_append(fd: RawFd, buf: &mut Vec<u8>, spare: usize, off: i64) -> io::Result<usize> {
    loop {
        // SAFETY: `buf` has `spare` bytes of capacity after `len`. `pread`
        // writes at most that many bytes and does not move the fd offset.
        let n = unsafe {
            libc::pread(
                fd,
                buf.as_mut_ptr().add(buf.len()).cast::<libc::c_void>(),
                spare,
                off,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        // SAFETY: `pread` wrote `n` bytes into the spare capacity.
        unsafe { buf.set_len(buf.len() + n as usize) };
        return Ok(n as usize);
    }
}

fn read_seq(fd: RawFd, buf: &mut Vec<u8>) -> io::Result<()> {
    // SAFETY: `fd` is an open proc file. `lseek` to 0 restarts the seq_file.
    let seek = unsafe { libc::lseek(fd, 0, libc::SEEK_SET) };
    if seek < 0 {
        return Err(io::Error::last_os_error());
    }
    loop {
        if buf.len() + 4096 > buf.capacity() {
            buf.reserve(8192);
        }
        let spare = buf.capacity() - buf.len();
        // SAFETY: `buf` has `spare` bytes of capacity after `len`. `read`
        // writes at most that many bytes.
        let n = unsafe {
            libc::read(
                fd,
                buf.as_mut_ptr().add(buf.len()).cast::<libc::c_void>(),
                spare,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 || buf.len() + (n as usize) > 32 * 1024 * 1024 {
            if n > 0 {
                // SAFETY: `read` wrote `n` bytes into the spare capacity.
                unsafe { buf.set_len(buf.len() + n as usize) };
            }
            return Ok(());
        }
        // SAFETY: `read` wrote `n` bytes into the spare capacity.
        unsafe { buf.set_len(buf.len() + n as usize) };
        if (n as usize) < spare {
            return Ok(());
        }
    }
}

pub(crate) fn open_read(path: &Path) -> io::Result<OwnedFd> {
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    // SAFETY: `bytes` is a NUL-terminated path. `open` returns a new fd or -1.
    let fd = unsafe {
        libc::open(
            bytes.as_ptr().cast::<libc::c_char>(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is open and not owned anywhere else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(crate) fn open_at(dir: RawFd, name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: `dir` is a directory fd. `name` is a NUL-terminated relative
    // path. `O_NOFOLLOW` refuses a symlink at the final component.
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

/// File descriptors of one process, plus any inotify watches behind them.
#[derive(Clone, Debug, Default)]
pub(crate) struct FdScan {
    pub count: u32,
    pub watches: Vec<InotifyWatch>,
}

/// `fd_dir` is `/proc/<pid>/fd`. Inotify fds are detected by readlink
/// (`anon_inode:inotify`); only those fdinfo files are parsed.
pub(crate) fn scan_fds(fd_dir: RawFd) -> FdScan {
    let mut out = FdScan::default();
    // SAFETY: `fd_dir` is an open directory. Rewinding it is local to this fd.
    unsafe { libc::lseek(fd_dir, 0, libc::SEEK_SET) };
    let mut dents = [0u8; 8192];
    loop {
        // SAFETY: `dents` is a writable buffer of that length. `getdents64`
        // writes directory records and returns the byte count, 0 at the end,
        // or -1.
        let n = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                fd_dir,
                dents.as_mut_ptr(),
                dents.len(),
            )
        };
        if n <= 0 {
            break;
        }
        let mut off = 0usize;
        let n = n as usize;
        while off + 19 <= n {
            let reclen = u16::from_ne_bytes([dents[off + 16], dents[off + 17]]) as usize;
            if reclen < 19 || off + reclen > n {
                break;
            }
            let name = &dents[off + 19..off + reclen];
            let nul = name
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(name.len());
            let name = &name[..nul];
            off += reclen;
            if name.is_empty() || name == b"." || name == b".." {
                continue;
            }
            out.count = out.count.saturating_add(1);
            if !link_contains_inotify(fd_dir, name) {
                continue;
            }
            if let Some(text) = read_fdinfo(fd_dir, name) {
                out.watches.extend(parse_inotify_fdinfo(&text));
            }
        }
    }
    out
}

fn link_contains_inotify(fd_dir: RawFd, name: &[u8]) -> bool {
    let mut name_buf = [0u8; 32];
    if name.len() + 1 > name_buf.len() {
        return false;
    }
    name_buf[..name.len()].copy_from_slice(name);
    let mut link = [0u8; 64];
    // SAFETY: `name_buf` is NUL-terminated. `link` receives the symlink text.
    let n = unsafe {
        libc::readlinkat(
            fd_dir,
            name_buf.as_ptr().cast::<libc::c_char>(),
            link.as_mut_ptr().cast::<libc::c_char>(),
            link.len(),
        )
    };
    if n <= 0 {
        return false;
    }
    let n = (n as usize).min(link.len());
    link[..n].windows(7).any(|window| window == b"inotify")
}

fn read_fdinfo(fd_dir: RawFd, name: &[u8]) -> Option<String> {
    let mut path = Vec::with_capacity(12 + name.len());
    path.extend_from_slice(b"../fdinfo/");
    path.extend_from_slice(name);
    path.push(0);
    // SAFETY: `path` is a relative NUL-terminated path under the same
    // `/proc/<pid>` directory the fd dir lives in.
    let fd = unsafe {
        libc::openat(
            fd_dir,
            path.as_ptr().cast::<libc::c_char>(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return None;
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut buf = Vec::new();
    reread(owned.as_raw_fd(), &mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) fn text_of(buf: &[u8]) -> Option<&str> {
    std::str::from_utf8(buf).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMAPS: &str = "\
560000000000-7fffffffffff ---p 00000000 00:00 0                          [rollup]
Rss:               10240 kB
Pss:                5120 kB
Pss_Dirty:          4096 kB
Pss_Anon:           4096 kB
Pss_File:           1024 kB
Pss_Shmem:             0 kB
Shared_Clean:       8192 kB
Private_Dirty:      2048 kB
Anonymous:          4096 kB
Swap:                256 kB
SwapPss:             128 kB
Locked:                0 kB
";

    #[test]
    fn smaps_rollup_fixture_splits_anon_file_and_swap() {
        let got = parse_smaps_rollup(SMAPS);
        assert_eq!(got.rss_kib, 10_240);
        assert_eq!(got.pss_kib, 5_120);
        assert_eq!(got.pss_anon_kib, 4_096);
        assert_eq!(got.pss_file_kib, 1_024);
        assert_eq!(got.swap_kib, 256);
        assert_eq!(got.swap_pss_kib, 128);
        // `Pss` must not pick up `Pss_Anon`.
        assert_ne!(got.pss_kib, got.pss_anon_kib);
    }

    #[test]
    fn schedstat_fixture_reads_the_third_field_as_timeslices() {
        let got = parse_schedstat("12345 678 42\n").unwrap();
        assert_eq!(got.run_ns, 12_345);
        assert_eq!(got.wait_ns, 678);
        assert_eq!(got.timeslices, 42);
        assert!(parse_schedstat("1 2").is_none());
    }

    #[test]
    fn io_fixture_keeps_write_bytes_apart_from_wchar() {
        let text = "\
rchar: 10
wchar: 20
syscr: 1
syscw: 4
read_bytes: 100
write_bytes: 200
cancelled_write_bytes: 7
";
        let got = parse_io(text);
        assert_eq!(got.rchar, 10);
        assert_eq!(got.wchar, 20);
        assert_eq!(got.syscr, 1);
        assert_eq!(got.syscw, 4);
        assert_eq!(got.read_bytes, 100);
        assert_eq!(got.write_bytes, 200);
        assert_eq!(got.cancelled_write_bytes, 7);
    }

    #[test]
    fn stat_fixture_allows_spaces_and_parens_inside_comm() {
        // comm is `a) b`. The closer is the last `)`.
        // After the closing paren the kernel order is state, ppid, pgrp,
        // session, tty_nr, tpgid, flags, minflt, cminflt, majflt, cmajflt,
        // then utime. The numbers below land 11/12/13/20 on those times.
        let text = "4242 (a) b) R 7 99 1 2 3 4 5 6 7 8 11 12 13 20 30 40 3 0 555 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        let got = parse_stat(text).unwrap();
        assert_eq!(got.pid, 4242);
        assert_eq!(got.comm, "a) b");
        assert_eq!(got.state, 'R');
        assert_eq!(got.ppid, 7);
        assert_eq!(got.pgrp, 99);
        assert_eq!(got.utime, 11);
        assert_eq!(got.stime, 12);
        assert_eq!(got.cutime, 13);
        assert_eq!(got.cstime, 20);
        assert_eq!(got.num_threads, 3);
        assert_eq!(got.starttime, 555);
    }

    #[test]
    fn status_fixture_reads_both_switch_counters() {
        let text = "\
Name:\tsleeper
Threads:\t4
voluntary_ctxt_switches:\t15
nonvoluntary_ctxt_switches:\t2
";
        let got = parse_status_switches(text);
        assert_eq!(got.voluntary, 15);
        assert_eq!(got.nonvoluntary, 2);
    }

    #[test]
    fn fdinfo_fixture_counts_duplicate_inotify_inodes() {
        let one = "\
pos:\t0
flags:\t02000002
mnt_id:\t11
ino:\t99
inotify wd:1 ino:1e0a sdev:800001 mask:fff ignored_mask:0 fhandle-bytes:8
inotify wd:2 ino:1e0a sdev:800001 mask:fff ignored_mask:0
inotify wd:3 ino:22 sdev:800001 mask:fff ignored_mask:0
";
        let two = "inotify wd:1 ino:22 sdev:800001 mask:fff\n";
        let mut watches = parse_inotify_fdinfo(one);
        assert_eq!(watches.len(), 3);
        assert_eq!(watches[0].ino, 0x1e0a);
        assert_eq!(watches[0].sdev, 0x800001);
        watches.extend(parse_inotify_fdinfo(two));
        let summary = inotify_summary(watches);
        assert_eq!(summary.watches, 4);
        // (1e0a, 800001) twice and (22, 800001) twice → 4 watches, 2 unique.
        assert_eq!(summary.dup, 2);
        assert!(parse_inotify_fdinfo("pos:\t0\nflags:\t00\n").is_empty());
    }

    #[test]
    fn maps_fixture_counts_only_aligned_anonymous_heap_slots() {
        let align = ARENA_HEAP_ALIGN;
        let slot = format!("{align:x}");
        let text = format!(
            "\
00400000-00401000 r-xp 00000000 103:03 1234 /usr/bin/zb-replay
{base:x}-{base_end:x} rw-p 00000000 00:00 0
{heap:x}-{heap_end:x} rw-p 00000000 00:00 0 [heap]
{small:x}-{small_end:x} rw-p 00000000 00:00 0
{huge:x}-{huge_end:x} rw-p 00000000 00:00 0
{shared:x}-{shared_end:x} rw-s 00000000 00:00 0
{odd:x}-{odd_end:x} rw-p 00000000 00:00 0
",
            base = 0x7f0000000000u64,
            base_end = 0x7f0000000000u64 + align,
            heap = 0x7f0000000000u64 + align,
            heap_end = 0x7f0000000000u64 + align + 0x201000,
            small = 0x7f0000000000u64 + 2 * align,
            small_end = 0x7f0000000000u64 + 2 * align + 0x100000,
            huge = 0x7f0000000000u64 + 4 * align,
            huge_end = 0x7f0000000000u64 + 4 * align + align + 1,
            shared = 0x7f0000000000u64 + 5 * align,
            shared_end = 0x7f0000000000u64 + 5 * align + 4096,
            odd = 0x7f0000001000u64,
            odd_end = 0x7f0000001000u64 + 8192,
        );
        // Keep `slot` used so the alignment constant is what the fixture uses.
        assert_eq!(u64::from_str_radix(&slot, 16).unwrap(), align);
        // Full 64 MiB anon slot, plus the 1 MiB aligned anon. Not [heap],
        // not the 128 MiB mapping, not the shared mapping, not the file,
        // not the unaligned page.
        assert_eq!(count_arena_heaps(&text), 2);
    }

    #[test]
    fn machine_cpu_fixture_does_not_add_guest_twice() {
        // user nice system idle iowait irq softirq steal guest guest_nice
        let text = "cpu  10 1 2 100 4 1 1 1 3 0\ncpu0 10 1 2 100 4 1 1 1 3 0\n";
        let got = parse_machine_cpu(text).unwrap();
        // busy = 10+1+2+1+1+1 = 16. Guest (3) stays inside user.
        assert_eq!(got.busy_jiffies, 16);
        assert_eq!(got.total_jiffies, 16 + 100 + 4);
    }
}
