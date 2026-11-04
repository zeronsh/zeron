//! Processes: the user's shell, and a process's name as it presents itself.

/// The user's login shell: `$SHELL`, else the password database's, else
/// `/bin/sh`.
#[cfg(unix)]
pub(crate) fn login_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL")
        && !shell.is_empty()
    {
        return shell;
    }
    let mut buffer = vec![0 as libc::c_char; 4096];
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: `entry` and `buffer` (of the length given) are ours to fill;
    // `found` is set to `entry` or null. Read only when non-null, while
    // `buffer` (which its strings point into) lives.
    unsafe {
        if libc::getpwuid_r(
            libc::getuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        ) == 0
            && !found.is_null()
            && !(*found).pw_shell.is_null()
        {
            let shell = std::ffi::CStr::from_ptr((*found).pw_shell).to_string_lossy();
            if !shell.is_empty() {
                return shell.into_owned();
            }
        }
    }
    "/bin/sh".into()
}

/// A process's name as it presents itself: its `argv[0]` (which programs
/// like Claude Code set to their own name) without its directory or a login
/// shell's `-`, else its executable's; empty if it can't be read.
pub fn process_name(pid: u32) -> String {
    let name = argv0(pid).or_else(|| executable(pid)).unwrap_or_default();
    let name = name.rsplit(['/', '\\']).next().unwrap_or(&name);
    let name = name.strip_prefix('-').unwrap_or(name);
    #[cfg(windows)]
    let name = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    name.to_owned()
}

/// The first word of a command line some programs rewrite argv[0] into.
#[cfg(unix)]
fn first_word(arg: &str) -> Option<String> {
    let first = arg.trim().split(' ').next().unwrap_or("");
    (!first.is_empty()).then(|| first.to_owned())
}

#[cfg(target_os = "linux")]
fn argv0(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let end = cmdline
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(cmdline.len());
    first_word(&String::from_utf8_lossy(&cmdline[..end]))
}
#[cfg(target_os = "linux")]
fn executable(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    first_word(&comm)
}

#[cfg(target_os = "macos")]
fn argv0(pid: u32) -> Option<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut size: libc::size_t = 0;
    // SAFETY: `mib` holds the 3 names passed; the first call (no buffer)
    // only writes the size needed into `size`, the second writes at most
    // `size` bytes into a buffer that long and updates `size` to what it
    // wrote. No new value is set (null, 0).
    let buffer = unsafe {
        if libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        let mut buffer = vec![0u8; size];
        if libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buffer.truncate(size);
        buffer
    };
    // argc, the executable path, NUL padding, then argv.
    let rest = buffer.get(4..)?;
    let exe_end = rest.iter().position(|b| *b == 0)?;
    let after_exe = &rest[exe_end..];
    let start = after_exe.iter().position(|b| *b != 0)?;
    let argv = &after_exe[start..];
    let end = argv.iter().position(|b| *b == 0).unwrap_or(argv.len());
    first_word(&String::from_utf8_lossy(&argv[..end]))
}
#[cfg(target_os = "macos")]
fn executable(pid: u32) -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: writes at most `buffer.len()` bytes into `buffer`, returning
    // how many.
    let len =
        unsafe { libc::proc_name(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    (len > 0).then(|| String::from_utf8_lossy(&buffer[..len as usize]).into_owned())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn argv0(_pid: u32) -> Option<String> {
    None
}
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn executable(_pid: u32) -> Option<String> {
    None
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };

    /// Every process: (pid, parent pid, executable name).
    pub(super) fn processes() -> Vec<(u32, u32, String)> {
        let mut all = Vec::new();
        // SAFETY: a snapshot handle, closed below; each entry is a
        // `PROCESSENTRY32W` with its size set, as the calls require, and its
        // name a NUL-terminated UTF-16 array inside it.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return all;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let name = &entry.szExeFile;
                let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
                all.push((
                    entry.th32ProcessID,
                    entry.th32ParentProcessID,
                    String::from_utf16_lossy(&name[..end]),
                ));
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        all
    }
}

#[cfg(windows)]
fn argv0(_pid: u32) -> Option<String> {
    None
}
#[cfg(windows)]
fn executable(pid: u32) -> Option<String> {
    windows::processes()
        .into_iter()
        .find(|(id, _, _)| *id == pid)
        .map(|(_, _, name)| name)
}

/// The deepest process under `root`, the latest started where several are
/// as deep (the snapshot lists processes in creation order): what a shell
/// runs in front. `None` when it runs nothing.
#[cfg(windows)]
pub(crate) fn newest_descendant(root: u32) -> Option<u32> {
    let all = windows::processes();
    let mut front = None;
    let mut parent = root;
    // Down from the shell a child at a time (bounded: a reused pid could
    // make a cycle). ConPTY's own console host isn't the program.
    for _ in 0..64 {
        let Some(&(child, _, _)) = all.iter().rev().find(|(id, ppid, name)| {
            *ppid == parent && *id != parent && !name.eq_ignore_ascii_case("conhost.exe")
        }) else {
            break;
        };
        parent = child;
        front = Some(child);
    }
    front
}
