//! Multi-call spawn counter. The launcher symlinks this binary into the
//! trial's shims directory as `git`, `sh`, `npm`, and the other names the
//! bench counts. That directory is first on `PATH`.
//!
//! Each invocation sends one datagram,
//! `{"k":"spawn","prog":…,"argv":[…],"ppid":…,"t":…}`, to `ZB_PROBE_SOCK`
//! (the trial's probe socket) and then `exec`s the next real binary. `t` is
//! `CLOCK_MONOTONIC` nanoseconds, the same clock as the launcher. A missing
//! socket, a full receive queue, or any other send error is ignored: the
//! counted command still runs. The search skips this process's own directory
//! and any later `PATH` entry that is the same file, so a shims directory
//! listed twice cannot exec the shim again.

use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

/// `argv` entries included in the datagram. Matches the replay harness.
const ARGV_CAP: usize = 8;
/// Bytes of each argument kept in the datagram, on a char boundary.
const ARG_MAX_BYTES: usize = 256;

fn main() {
    #[cfg(not(unix))]
    {
        let _ = writeln!(io::stderr(), "zb-shim only runs on unix");
        process::exit(1);
    }
    #[cfg(unix)]
    unix_main();
}

#[cfg(unix)]
fn unix_main() -> ! {
    let args: Vec<OsString> = env::args_os().collect();
    let argv0 = args
        .first()
        .cloned()
        .unwrap_or_else(|| OsString::from("zb-shim"));
    let prog = basename(&argv0);
    report_spawn(&prog, &args);
    let own_exe = env::current_exe().unwrap_or_else(|_| PathBuf::from(&argv0));
    let own_dir = invoked_dir(&prog, &argv0, &own_exe);
    let path = env::var_os("PATH").unwrap_or_default();
    match next_binary(&prog, &path, &own_exe, &own_dir) {
        Some(bin) if !same_file(&bin, &own_exe) => exec_real(&bin, &args),
        Some(_) | None => {
            let _ = writeln!(
                io::stderr(),
                "zb-shim: {}: command not found",
                prog.to_string_lossy()
            );
            process::exit(127);
        }
    }
}

#[cfg(unix)]
fn exec_real(bin: &Path, args: &[OsString]) -> ! {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(bin);
    if let Some(argv0) = args.first() {
        cmd.arg0(argv0);
    }
    if args.len() > 1 {
        cmd.args(&args[1..]);
    }
    let err = cmd.exec();
    let _ = writeln!(io::stderr(), "zb-shim: exec {}: {err}", bin.display());
    process::exit(127);
}

/// Directory of the symlink that was executed. Later `PATH` entries equal to
/// it are not candidates, which is what stops the shim from running itself.
#[cfg(unix)]
fn invoked_dir(prog: &OsStr, argv0: &OsStr, own_exe: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let invoked = Path::new(argv0);
    if argv0.as_bytes().contains(&b'/') {
        let parent = invoked.parent().filter(|p| !p.as_os_str().is_empty());
        return canonical_dir(parent.unwrap_or_else(|| Path::new(".")));
    }
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            let dir = path_dir(dir);
            if same_file(&dir.join(prog), own_exe) {
                return canonical_dir(&dir);
            }
        }
    }
    own_exe
        .parent()
        .map(canonical_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The first executable named `prog` on `path_env` that is neither inside
/// `own_dir` nor the same file as `own_exe`.
#[cfg(unix)]
fn next_binary(prog: &OsStr, path_env: &OsStr, own_exe: &Path, own_dir: &Path) -> Option<PathBuf> {
    let own_dir = canonical_dir(own_dir);
    let own_meta = std::fs::metadata(own_exe).ok();
    for dir in env::split_paths(path_env) {
        let dir = path_dir(dir);
        if dirs_match(&dir, &own_dir) {
            continue;
        }
        let candidate = dir.join(prog);
        if !is_executable(&candidate) {
            continue;
        }
        if same_file_meta(&candidate, own_meta.as_ref()) {
            continue;
        }
        return Some(candidate);
    }
    None
}

#[cfg(unix)]
fn path_dir(dir: PathBuf) -> PathBuf {
    if dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        dir
    }
}

#[cfg(unix)]
fn dirs_match(dir: &Path, own_dir: &Path) -> bool {
    if dir == own_dir {
        return true;
    }
    match (std::fs::canonicalize(dir), std::fs::canonicalize(own_dir)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(unix)]
fn canonical_dir(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

#[cfg(unix)]
fn same_file(path: &Path, own_exe: &Path) -> bool {
    same_file_meta(path, std::fs::metadata(own_exe).ok().as_ref())
}

#[cfg(unix)]
fn same_file_meta(path: &Path, own: Option<&std::fs::Metadata>) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Some(own), Ok(meta)) = (own, std::fs::metadata(path)) else {
        return false;
    };
    meta.dev() == own.dev() && meta.ino() == own.ino()
}

#[cfg(unix)]
fn basename(argv0: &OsStr) -> OsString {
    Path::new(argv0).file_name().unwrap_or(argv0).to_os_string()
}

#[cfg(unix)]
fn report_spawn(prog: &OsStr, args: &[OsString]) {
    use std::os::unix::net::UnixDatagram;
    let Some(path) = env::var_os("ZB_PROBE_SOCK").filter(|path| !path.is_empty()) else {
        return;
    };
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    if socket.set_nonblocking(true).is_err() {
        return;
    }
    // `connect` fails when nothing is bound (the probe unit is absent). That
    // is a skip, not a failed command. Nonblocking so a full queue cannot
    // stall the spawn.
    if socket.connect(Path::new(&path)).is_err() {
        return;
    }
    let bytes = format_spawn(prog, args, ppid(), mono_now());
    let _ = socket.send(&bytes);
}

#[cfg(unix)]
fn ppid() -> u32 {
    unsafe { libc::getppid() as u32 }
}

/// `CLOCK_MONOTONIC` nanoseconds, the same clock the launcher records.
/// This binary does not link the bench library.
#[cfg(unix)]
fn mono_now() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

/// One `ProbeEvent::Spawn` object: `{"k":"spawn","prog","argv","ppid","t"}`.
#[cfg(unix)]
fn format_spawn(prog: &OsStr, args: &[OsString], ppid: u32, t: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.extend_from_slice(br#"{"k":"spawn","prog":"#);
    push_json_str(&mut out, &clipped(prog, 64));
    out.extend_from_slice(br#","argv":["#);
    for (i, arg) in args.iter().take(ARGV_CAP).enumerate() {
        if i > 0 {
            out.push(b',');
        }
        push_json_str(&mut out, &clipped(arg, ARG_MAX_BYTES));
    }
    out.extend_from_slice(br#"],"ppid":"#);
    out.extend_from_slice(ppid.to_string().as_bytes());
    out.extend_from_slice(br#","t":"#);
    out.extend_from_slice(t.to_string().as_bytes());
    out.push(b'}');
    out
}

#[cfg(unix)]
fn clipped(raw: &OsStr, max_bytes: usize) -> String {
    let lossy = raw.to_string_lossy();
    let mut out = String::new();
    for ch in lossy.chars() {
        let next = out.len() + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(unix)]
fn push_json_str(out: &mut Vec<u8>, text: &str) {
    out.push(b'"');
    for ch in text.chars() {
        match ch {
            '"' => out.extend_from_slice(br#"\""#),
            '\\' => out.extend_from_slice(br#"\\"#),
            '\n' => out.extend_from_slice(br#"\n"#),
            '\r' => out.extend_from_slice(br#"\r"#),
            '\t' => out.extend_from_slice(br#"\t"#),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/home/user"));
        let dir = home
            .join("Documents/dev/tmp/zeron-perf/scratch/b12-spawn-shim/unit")
            .join(format!("{label}-{n}-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_exe(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn spawn_event_matches_the_probe_object() {
        let args = [
            OsString::from("git"),
            OsString::from("status"),
            OsString::from("a\"b\\c"),
        ];
        let msg = format_spawn(OsStr::new("git"), &args, 42, 7);
        assert_eq!(
            msg,
            br#"{"k":"spawn","prog":"git","argv":["git","status","a\"b\\c"],"ppid":42,"t":7}"#
        );
    }

    #[test]
    fn spawn_event_keeps_eight_arguments() {
        let args: Vec<_> = (0..12).map(|i| OsString::from(format!("a{i}"))).collect();
        let msg = String::from_utf8(format_spawn(OsStr::new("git"), &args, 1, 2)).unwrap();
        assert!(msg.contains("\"a7\""));
        assert!(!msg.contains("\"a8\""));
        assert!(msg.ends_with(r#","ppid":1,"t":2}"#));
    }

    #[test]
    fn next_binary_skips_its_own_dir_and_another_link_to_itself() {
        let root = scratch("resolve");
        let own = root.join("zb-shim");
        write_exe(&own, "#!/bin/sh\nexit 0\n");
        let shims = root.join("shims");
        let again = root.join("again");
        fs::create_dir_all(&shims).unwrap();
        fs::create_dir_all(&again).unwrap();
        std::os::unix::fs::symlink(&own, shims.join("git")).unwrap();
        std::os::unix::fs::symlink(&own, again.join("git")).unwrap();
        let real_dir = root.join("bin");
        write_exe(&real_dir.join("git"), "#!/bin/sh\necho real\n");
        let path = format!(
            "{}:{}:{}:{}",
            shims.display(),
            shims.display(),
            again.display(),
            real_dir.display()
        );
        let found = next_binary(OsStr::new("git"), OsStr::new(&path), &own, &shims).unwrap();
        assert_eq!(found, real_dir.join("git"));
    }

    #[test]
    fn next_binary_refuses_to_return_itself_when_nothing_else_exists() {
        let root = scratch("only-self");
        let own = root.join("zb-shim");
        write_exe(&own, "#!/bin/sh\nexit 0\n");
        let shims = root.join("shims");
        fs::create_dir_all(&shims).unwrap();
        std::os::unix::fs::symlink(&own, shims.join("git")).unwrap();
        let found = next_binary(
            OsStr::new("git"),
            OsStr::new(shims.as_os_str()),
            &own,
            &shims,
        );
        assert!(found.is_none());
    }

    #[test]
    fn a_symlinked_shim_dir_is_the_same_directory() {
        let root = scratch("link-dir");
        let own = root.join("zb-shim");
        write_exe(&own, "#!/bin/sh\nexit 0\n");
        let shims = root.join("shims");
        fs::create_dir_all(&shims).unwrap();
        std::os::unix::fs::symlink(&own, shims.join("git")).unwrap();
        let via = root.join("via");
        std::os::unix::fs::symlink(&shims, &via).unwrap();
        let real_dir = root.join("bin");
        write_exe(&real_dir.join("git"), "#!/bin/sh\necho real\n");
        let path = format!("{}:{}", via.display(), real_dir.display());
        let found = next_binary(OsStr::new("git"), OsStr::new(&path), &own, &shims).unwrap();
        assert_eq!(found, real_dir.join("git"));
    }
}
