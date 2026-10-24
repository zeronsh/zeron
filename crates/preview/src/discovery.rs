//! Observe only this user's listeners, and attribute each to the Zeron agent
//! run or terminal that started it (see [`zeron_proto::ORIGIN_ENV`]). HTTP probes
//! run only after that origin is established and commands performing
//! authentication have been excluded.
use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_proto::ORIGIN_ENV;

#[derive(Debug, Clone)]
pub struct Listener {
    pub pid: u32,
    pub parent: u32,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub started_at: u64,
    pub address: SocketAddr,
    /// Started, directly or through any ancestor, by a Zeron agent run or
    /// terminal.
    pub zeron_owned: bool,
    /// What started it: the origin marker's value (`chat:…`, `terminal:…`).
    pub origin: Option<String>,
    /// The chat that owns [`Self::origin`], once resolved against the chats
    /// and terminals this device hosts.
    pub chat_id: Option<String>,
}

impl Listener {
    pub fn belongs_to(&self, root: &Path) -> bool {
        self.cwd.starts_with(root)
    }

    /// A login CLI may open an HTTP listener in the project's cwd, but that
    /// listener is a browser callback, not a development server. Some (such as
    /// Infisical) abort the login on an unsolicited HEAD, so exclude explicit
    /// authentication commands before connecting at all. Match whole arguments,
    /// not paths, URLs or source strings that happen to mention authentication.
    pub(crate) fn is_authentication_command(&self) -> bool {
        self.args
            .iter()
            .skip(1)
            .take_while(|arg| arg.as_str() != "--")
            .any(|arg| {
                matches!(
                    arg.as_str(),
                    "login"
                        | "auth"
                        | "authenticate"
                        | "signin"
                        | "sign-in"
                        | "sso"
                        | "oauth"
                        | "oauth2"
                )
            })
    }
}

/// Preserve a useful framework label without advertising process arguments,
/// which may contain credentials. Rank frontends above generic HTTP services.
pub fn framework(args: &[String]) -> (&'static str, u8) {
    for (needle, name) in [
        ("vite", "Vite"),
        ("next", "Next.js"),
        ("astro", "Astro"),
        ("miniflare", "Miniflare"),
    ] {
        if args.iter().any(|arg| {
            let arg = arg.to_lowercase();
            Path::new(&arg).file_stem().and_then(|s| s.to_str()) == Some(needle)
                || arg.contains(&format!("/{needle}/"))
                || arg.starts_with(&format!("{needle}-server"))
        }) {
            return (name, if needle == "miniflare" { 80 } else { 100 });
        }
    }
    if args.first().is_some_and(|s| {
        Path::new(s)
            .file_name()
            .is_some_and(|n| n == "node" || n == "bun" || n == "deno")
    }) {
        ("Node HTTP server", 50)
    } else {
        ("HTTP server", 40)
    }
}

/// Port flags are operational settings, never part of a service identity.
pub fn command_identity(args: &[String]) -> String {
    let mut skip = false;
    let python_http = args.iter().any(|arg| arg == "http.server");
    args.iter()
        .filter_map(|arg| {
            if skip {
                skip = false;
                return None;
            }
            if matches!(arg.as_str(), "--port" | "-p" | "--inspect-port") {
                skip = true;
                return None;
            }
            if arg.starts_with("--port=")
                || arg.starts_with("PORT=")
                || arg.starts_with("--inspect-port=")
            {
                return None;
            }
            if python_http && arg.parse::<u16>().is_ok() {
                return None;
            }
            Some(arg.as_str())
        })
        .collect::<Vec<_>>()
        .join("\0")
}

pub async fn is_http(address: SocketAddr) -> bool {
    tokio::time::timeout(Duration::from_millis(800), async move {
        let mut socket = tokio::net::TcpStream::connect(address).await.ok()?;
        socket
            .write_all(
                format!(
                    "HEAD / HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n",
                    address.port()
                )
                .as_bytes(),
            )
            .await
            .ok()?;
        let mut prefix = [0; 12];
        socket.read_exact(&mut prefix).await.ok()?;
        let version = &prefix[..9];
        let status = std::str::from_utf8(&prefix[9..])
            .ok()?
            .parse::<u16>()
            .ok()?;
        ((version == b"HTTP/1.1 " || version == b"HTTP/1.0 ") && (100..600).contains(&status))
            .then_some(())
    })
    .await
    .ok()
    .flatten()
    .is_some()
}

/// One process in the scan: enough to walk ancestry and to key the origin
/// cache by process identity (a recycled pid is a different process).
#[derive(Clone, Copy)]
struct Process {
    parent: u32,
    started_at: u64,
}

/// Each process's origin marker, read once per process lifetime: a process's
/// environment is fixed when it execs, so neither the verdict nor the cost of
/// reading it repeats every scan, and a later transient read failure (the
/// process exiting mid-scan, a busy sysctl) cannot drop a server already
/// attributed.
#[derive(Default)]
pub struct Origins {
    /// `Some(origin)` when the marker is present.
    markers: HashMap<(u32, u64), Option<String>>,
}

impl Origins {
    /// What started `pid`: its own marker, else the nearest
    /// ancestor's, so a server whose launcher scrubbed its environment still
    /// counts while that launcher lives. The walk stops at this engine: an
    /// engine itself started by an agent (a nested development build) must
    /// not lend that outer chat to everything it spawns.
    fn resolve(
        &mut self,
        pid: u32,
        processes: &HashMap<u32, Process>,
        read: &mut impl FnMut(u32) -> Option<Vec<u8>>,
    ) -> Option<String> {
        let engine = std::process::id();
        let mut pid = pid;
        for _ in 0..64 {
            if pid <= 1 || pid == engine {
                return None;
            }
            let process = processes.get(&pid)?;
            let key = (pid, process.started_at);
            let marker = match self.markers.get(&key) {
                Some(marker) => marker.clone(),
                None => match read(pid) {
                    Some(environment) => {
                        let marker = origin_marker(&environment);
                        self.markers.insert(key, marker.clone());
                        marker
                    }
                    // Unreadable now: retried next scan, never cached.
                    None => None,
                },
            };
            if marker.is_some() {
                return marker;
            }
            if process.parent == pid {
                return None;
            }
            pid = process.parent;
        }
        None
    }

    /// Attribute `listeners`, then forget processes no longer running.
    fn attribute(
        &mut self,
        listeners: &mut [Listener],
        processes: &HashMap<u32, Process>,
        mut read: impl FnMut(u32) -> Option<Vec<u8>>,
    ) {
        for listener in listeners.iter_mut() {
            let origin = self.resolve(listener.pid, processes, &mut read);
            listener.zeron_owned = origin.is_some();
            listener.origin = origin.filter(|origin| !origin.is_empty());
        }
        let alive: HashSet<_> = processes
            .iter()
            .map(|(pid, process)| (*pid, process.started_at))
            .collect();
        self.markers.retain(|key, _| alive.contains(key));
    }
}

/// The origin marker's value in a NUL-separated environment block.
fn origin_marker(environment: &[u8]) -> Option<String> {
    let prefix = format!("{ORIGIN_ENV}=");
    environment
        .split(|b| *b == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .map(|value| String::from_utf8_lossy(value).into_owned())
}

/// Process start in ms since the epoch, derived the same way for the scanner
/// and for [`same_process`] so the two agree exactly.
#[cfg(target_os = "linux")]
fn linux_clock() -> (u64, u64) {
    let boot = std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("btime ")
                    .and_then(|v| v.parse::<u64>().ok())
            })
        })
        .unwrap_or(0);
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    (boot, ticks)
}
#[cfg(target_os = "linux")]
fn linux_started_at(stat_fields: &[&str], boot: u64, ticks: u64) -> u64 {
    boot * 1000
        + stat_fields
            .get(19)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            * 1000
            / ticks
}

/// Whether `pid` is still the process the scanner observed with `started_at`.
/// Cheap enough to run per proxied connection; a full [`listeners`] pass
/// walks every process (and spawns lsof/ps on macOS), which the proxy used
/// to do for every asset a remote page requested.
#[cfg(target_os = "linux")]
pub fn same_process(pid: u32, started_at: u64) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some((_, tail)) = stat.rsplit_once(") ") else {
        return false;
    };
    let fields: Vec<_> = tail.split_whitespace().collect();
    let (boot, ticks) = linux_clock();
    linux_started_at(&fields, boot, ticks) == started_at
}

/// This user's loopback-reachable TCP listeners, attributed to their origin
/// chat. `None` when the process table itself could not be read: the caller
/// keeps what it last saw rather than reading a failed scan as "nothing runs".
#[cfg(target_os = "linux")]
pub fn listeners(origins: &mut Origins) -> Option<Vec<Listener>> {
    use std::{fs, os::unix::fs::MetadataExt};
    let mut sockets = HashMap::new();
    let mut tables = 0;
    for (file, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Ok(text) = fs::read_to_string(file) {
            tables += 1;
            for line in text.lines().skip(1) {
                let parts: Vec<_> = line.split_whitespace().collect();
                if parts.len() <= 9 || parts[3] != "0A" {
                    continue;
                }
                if let Some(address) = proc_address(parts[1], ipv6) {
                    sockets.insert(parts[9].to_string(), address);
                }
            }
        }
    }
    if tables == 0 {
        return None;
    }
    let (boot, ticks) = linux_clock();
    let uid = unsafe { libc::geteuid() };
    let mut result = Vec::new();
    let mut processes = HashMap::new();
    for process in fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = process.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let path = process.path();
        if !fs::metadata(&path).is_ok_and(|m| m.uid() == uid) {
            continue;
        }
        let Ok(stat) = fs::read_to_string(path.join("stat")) else {
            continue;
        };
        let Some((_, tail)) = stat.rsplit_once(") ") else {
            continue;
        };
        let fields: Vec<_> = tail.split_whitespace().collect();
        let Some(parent) = fields.get(1).and_then(|v| v.parse().ok()) else {
            continue;
        };
        let started_at = linux_started_at(&fields, boot, ticks);
        processes.insert(pid, Process { parent, started_at });
        // Sockets first: most processes listen on nothing, and need neither
        // their cwd nor their arguments read.
        let Ok(fds) = fs::read_dir(path.join("fd")) else {
            continue;
        };
        let addresses: Vec<SocketAddr> = fds
            .flatten()
            .filter_map(|fd| {
                let link = fs::read_link(fd.path()).ok()?;
                let link = link.to_string_lossy();
                let inode = link.strip_prefix("socket:[")?.strip_suffix(']')?;
                sockets.get(inode).copied()
            })
            .collect();
        if addresses.is_empty() {
            continue;
        }
        let Ok(cwd) = fs::read_link(path.join("cwd")) else {
            continue;
        };
        let args = fs::read(path.join("cmdline"))
            .unwrap_or_default()
            .split(|b| *b == 0)
            .filter(|b| !b.is_empty())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<Vec<_>>();
        for address in addresses {
            result.push(Listener {
                pid,
                parent,
                cwd: cwd.clone(),
                args: args.clone(),
                started_at,
                address,
                zeron_owned: false,
                origin: None,
                chat_id: None,
            });
        }
    }
    origins.attribute(&mut result, &processes, |pid| {
        fs::read(format!("/proc/{pid}/environ")).ok()
    });
    result.sort_by_key(|l| (l.address, l.pid));
    result.dedup_by_key(|l| (l.address, l.pid));
    Some(result)
}

#[cfg(target_os = "linux")]
fn proc_address(value: &str, ipv6: bool) -> Option<SocketAddr> {
    let (host, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if ipv6 {
        if host.len() != 32 {
            return None;
        }
        let mut bytes = [0; 16];
        for (i, chunk) in host.as_bytes().chunks_exact(8).enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(
                &u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16)
                    .ok()?
                    .to_le_bytes(),
            );
        }
        IpAddr::V6(Ipv6Addr::from(bytes))
    } else {
        IpAddr::V4(Ipv4Addr::from(
            u32::from_str_radix(host, 16).ok()?.to_le_bytes(),
        ))
    };
    if !ip.is_loopback() && !ip.is_unspecified() {
        return None;
    }
    Some(SocketAddr::new(
        if ip.is_unspecified() {
            if ipv6 {
                Ipv6Addr::LOCALHOST.into()
            } else {
                Ipv4Addr::LOCALHOST.into()
            }
        } else {
            ip
        },
        port,
    ))
}

/// `ps lstart=` tokens ("Thu Sep 11 16:14:42 2026") to ms since the epoch.
#[cfg(target_os = "macos")]
fn parse_lstart(parts: &[&str]) -> u64 {
    use chrono::TimeZone;
    chrono::NaiveDateTime::parse_from_str(&parts.join(" "), "%a %b %e %T %Y")
        .ok()
        .and_then(|d| chrono::Local.from_local_datetime(&d).earliest())
        .map(|d| d.timestamp_millis().max(0) as u64)
        .unwrap_or(0)
}

/// See the Linux variant. One `ps -p` for a single pid instead of two lsof
/// passes and a full `ps -ax` per proxied connection.
#[cfg(target_os = "macos")]
pub fn same_process(pid: u32, started_at: u64) -> bool {
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()
    else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<_> = text.split_whitespace().collect();
    parts.len() >= 5 && parse_lstart(&parts[..5]) == started_at
}

/// `program`'s stdout, or `None` when it could not run or outlived
/// `timeout` (lsof can block on an unresponsive network mount; a stuck scan
/// would otherwise freeze discovery for good). The exit status is not
/// checked: lsof exits 1 when it simply found nothing.
#[cfg(target_os = "macos")]
fn command_output(program: &str, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    use std::{io::Read, process::Stdio};
    let mut child = std::process::Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        output
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// `pid`'s environment as a NUL-separated block, read the way `ps -E` does
/// (`KERN_PROCARGS2`: argc, the exec path, padding, argv, then envp). Only
/// this user's processes are readable, which is all discovery looks at.
#[cfg(target_os = "macos")]
fn environment(pid: u32) -> Option<Vec<u8>> {
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut argmax as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || argmax <= 0
    {
        return None;
    }
    let mut buffer = vec![0u8; argmax as usize];
    let mut size = buffer.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buffer.truncate(size);
    procargs_environment(&buffer)
}

/// The envp section of a `KERN_PROCARGS2` buffer.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn procargs_environment(buffer: &[u8]) -> Option<Vec<u8>> {
    let argc = i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?);
    let rest = buffer.get(4..)?;
    // The exec path, then the NUL padding after it.
    let rest = &rest[rest.iter().position(|b| *b == 0)?..];
    let rest = &rest[rest.iter().position(|b| *b != 0).unwrap_or(rest.len())..];
    let mut entries = rest.split(|b| *b == 0);
    for _ in 0..argc.max(0) {
        entries.next()?;
    }
    let mut environment = Vec::new();
    for entry in entries.take_while(|entry| !entry.is_empty()) {
        environment.extend_from_slice(entry);
        environment.push(0);
    }
    Some(environment)
}

/// See the Linux variant.
#[cfg(target_os = "macos")]
pub fn listeners(origins: &mut Origins) -> Option<Vec<Listener>> {
    const TIMEOUT: Duration = Duration::from_secs(10);
    let uid = unsafe { libc::geteuid() }.to_string();
    let fields = |arguments: &[&str]| -> Option<Vec<(u32, String)>> {
        let output = command_output("/usr/sbin/lsof", arguments, TIMEOUT)?;
        let mut pid = 0;
        let mut values = Vec::new();
        for field in output.split(|b| *b == 0) {
            let field = String::from_utf8_lossy(field);
            let field = field.trim_start_matches('\n');
            if let Some(value) = field.strip_prefix('p') {
                pid = value.parse().unwrap_or(0);
            }
            if let Some(value) = field.strip_prefix('n') {
                values.push((pid, value.to_owned()));
            }
        }
        Some(values)
    };
    let sockets = fields(&["-nP", "-a", "-u", &uid, "-iTCP", "-sTCP:LISTEN", "-F0pn"])?;
    if sockets.is_empty() {
        return Some(Vec::new());
    }
    let cwds: HashMap<_, _> = fields(&["-nP", "-a", "-u", &uid, "-d", "cwd", "-F0pn"])?
        .into_iter()
        .collect();
    let mut processes = HashMap::new();
    let mut commands = HashMap::new();
    let table = command_output("/bin/ps", &["-axo", "pid=,ppid=,lstart=,command="], TIMEOUT)?;
    for line in String::from_utf8_lossy(&table).lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }
        let (Ok(pid), Ok(parent)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
            continue;
        };
        let started_at = parse_lstart(&parts[2..7]);
        processes.insert(pid, Process { parent, started_at });
        commands.insert(
            pid,
            parts[7..].iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
    }
    let mut result = Vec::new();
    for (pid, socket) in sockets {
        let Some(cwd) = cwds.get(&pid).and_then(|s| std::fs::canonicalize(s).ok()) else {
            continue;
        };
        let (Some(process), Some(args)) = (processes.get(&pid), commands.get(&pid)) else {
            continue;
        };
        let addresses = if let Some(port) = socket
            .strip_prefix("*:")
            .and_then(|p| p.parse::<u16>().ok())
        {
            vec![
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
                SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port),
            ]
        } else {
            socket
                .parse::<SocketAddr>()
                .ok()
                .filter(|a| a.ip().is_loopback())
                .into_iter()
                .collect()
        };
        for address in addresses {
            result.push(Listener {
                pid,
                parent: process.parent,
                cwd: cwd.clone(),
                args: args.clone(),
                started_at: process.started_at,
                address,
                zeron_owned: false,
                origin: None,
                chat_id: None,
            });
        }
    }
    origins.attribute(&mut result, &processes, environment);
    result.sort_by_key(|l| (l.address, l.pid));
    result.dedup_by_key(|l| (l.address, l.pid));
    Some(result)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn listeners(_origins: &mut Origins) -> Option<Vec<Listener>> {
    Some(Vec::new())
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn same_process(_pid: u32, _started_at: u64) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_commands_are_not_preview_candidates() {
        let listener = |args: &[&str]| Listener {
            pid: 1,
            parent: 0,
            cwd: "/project".into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            started_at: 1,
            address: "127.0.0.1:3000".parse().unwrap(),
            zeron_owned: true,
            origin: Some("chat:chat".into()),
            chat_id: Some("chat".into()),
        };
        for args in [
            vec!["/usr/bin/infisical", "login"],
            vec![
                "infisical",
                "--silent",
                "login",
                "--domain",
                "https://app.infisical.com",
            ],
            vec!["gh", "auth", "login"],
            vec!["aws", "sso", "login"],
            vec!["gcloud", "auth", "application-default", "login"],
            vec!["codex", "login"],
            vec!["node", "/tools/firebase.js", "login"],
            vec!["cli", "authenticate"],
            vec!["cli", "signin"],
            vec!["cli", "sign-in"],
            vec!["cli", "oauth"],
            vec!["cli", "oauth2"],
        ] {
            assert!(listener(&args).is_authentication_command(), "{args:?}");
        }
        for args in [
            vec![],
            vec!["node", "/project/auth/server.js"],
            vec!["node", "/project/login.js"],
            vec!["node", "/project/node_modules/vite/bin/vite.js"],
            vec!["next", "dev"],
            vec!["python3", "-m", "http.server", "3000"],
            vec!["python3", "-c", "import auth; auth.serve()"],
            vec!["server", "--auth", "--login-url=https://example.com/login"],
            vec!["infisical", "run", "--", "node", "server.js", "login"],
        ] {
            assert!(!listener(&args).is_authentication_command(), "{args:?}");
        }
    }
    fn listener_of(pid: u32) -> Listener {
        Listener {
            pid,
            parent: 0,
            cwd: "/project".into(),
            args: Vec::new(),
            started_at: 0,
            address: "127.0.0.1:3000".parse().unwrap(),
            zeron_owned: false,
            origin: None,
            chat_id: None,
        }
    }
    fn environment(entries: &[&str]) -> Vec<u8> {
        entries
            .iter()
            .flat_map(|e| [e.as_bytes(), b"\0"])
            .flatten()
            .copied()
            .collect()
    }
    #[test]
    fn origin_is_the_nearest_marked_process_and_never_the_engine() {
        let engine = std::process::id();
        // shell (agent's chat) → env-scrubbed server; an unrelated server;
        // and a server whose ancestry reaches this engine before any marker.
        let processes: HashMap<u32, Process> = [
            (
                100,
                Process {
                    parent: engine,
                    started_at: 1,
                },
            ),
            (
                101,
                Process {
                    parent: 100,
                    started_at: 2,
                },
            ),
            (
                102,
                Process {
                    parent: 101,
                    started_at: 3,
                },
            ),
            (
                200,
                Process {
                    parent: 1,
                    started_at: 4,
                },
            ),
            (
                300,
                Process {
                    parent: engine,
                    started_at: 5,
                },
            ),
        ]
        .into();
        let reads = std::cell::RefCell::new(Vec::new());
        let mut read = |pid: u32| {
            reads.borrow_mut().push(pid);
            Some(match pid {
                100 => environment(&["PATH=/bin", "ZERON_ORIGIN=chat:a"]),
                101 => environment(&["PATH=/bin", "ZERON_ORIGIN=terminal:b"]),
                _ => environment(&["PATH=/bin"]),
            })
        };
        let mut origins = Origins::default();
        let mut listeners = vec![listener_of(102), listener_of(200), listener_of(300)];
        origins.attribute(&mut listeners, &processes, &mut read);
        assert!(listeners[0].zeron_owned);
        assert_eq!(listeners[0].origin.as_deref(), Some("terminal:b"));
        assert!(!listeners[1].zeron_owned);
        assert!(!listeners[2].zeron_owned);
        assert!(
            !reads.borrow().contains(&engine),
            "the engine's own environment is never consulted"
        );
        // A process's verdict is read once for its lifetime.
        let first = reads.borrow().len();
        origins.attribute(&mut listeners, &processes, &mut read);
        assert_eq!(reads.borrow().len(), first);
        assert_eq!(listeners[0].origin.as_deref(), Some("terminal:b"));
        // A recycled pid is a different process: read afresh.
        let mut recycled = processes.clone();
        recycled.insert(
            102,
            Process {
                parent: 1,
                started_at: 9,
            },
        );
        let mut listeners = vec![listener_of(102)];
        origins.attribute(&mut listeners, &recycled, &mut read);
        assert!(!listeners[0].zeron_owned);
    }
    #[test]
    fn an_unreadable_environment_is_retried_not_remembered() {
        let processes: HashMap<u32, Process> = [(
            100,
            Process {
                parent: 1,
                started_at: 1,
            },
        )]
        .into();
        let mut origins = Origins::default();
        let mut listeners = vec![listener_of(100)];
        origins.attribute(&mut listeners, &processes, |_| None);
        assert!(!listeners[0].zeron_owned);
        origins.attribute(&mut listeners, &processes, |_| {
            Some(environment(&["ZERON_ORIGIN=chat:c"]))
        });
        assert_eq!(listeners[0].origin.as_deref(), Some("chat:c"));
    }
    #[test]
    fn origin_marker_matches_the_whole_name() {
        assert_eq!(
            origin_marker(&environment(&["XZERON_ORIGIN=no", "ZERON_ORIGIN=yes"])),
            Some("yes".into())
        );
        assert_eq!(origin_marker(&environment(&["ZERON_ORIGIN_X=no"])), None);
        assert_eq!(
            origin_marker(&environment(&["ZERON_ORIGIN="])),
            Some(String::new())
        );
    }
    #[test]
    fn procargs_environment_skips_the_exec_path_and_arguments() {
        let mut buffer = 2i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(
            b"/usr/bin/node\0\0\0\0node\0ZERON_ORIGIN=argv\0A=1\0ZERON_ORIGIN=env\0\0junk",
        );
        let environment = procargs_environment(&buffer).unwrap();
        assert_eq!(origin_marker(&environment), Some("env".into()));
        assert!(procargs_environment(&[1, 0]).is_none());
    }
    #[test]
    fn service_identity_ignores_port_configuration() {
        let args = |port: &str| vec!["node".into(), "api.js".into(), "--port".into(), port.into()];
        assert_eq!(
            command_identity(&args("3000")),
            command_identity(&args("3001"))
        );
        assert_ne!(
            command_identity(&args("3000")),
            command_identity(&["node".into(), "docs.js".into()])
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn accepts_only_loopback_reachable_listeners() {
        assert_eq!(
            proc_address("0100007F:1435", false).unwrap(),
            "127.0.0.1:5173".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            proc_address("00000000000000000000000001000000:1435", true).unwrap(),
            "[::1]:5173".parse::<SocketAddr>().unwrap()
        );
        assert!(proc_address("0101A8C0:1435", false).is_none());
        assert!(proc_address("malformed:00", true).is_none());
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn same_process_matches_the_scanner_start_time() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        struct Kill(std::process::Child);
        impl Drop for Kill {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let _guard = Kill(child);
        // `listeners()` only reports sockets; derive the scanner's start time
        // through the same platform path it uses.
        #[cfg(target_os = "linux")]
        let started_at = {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
            let (_, tail) = stat.rsplit_once(") ").unwrap();
            let fields: Vec<_> = tail.split_whitespace().collect();
            let (boot, ticks) = linux_clock();
            linux_started_at(&fields, boot, ticks)
        };
        #[cfg(target_os = "macos")]
        let started_at = {
            let output = std::process::Command::new("/bin/ps")
                .args(["-axo", "pid=,ppid=,lstart=,command="])
                .env("LC_ALL", "C")
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&output.stdout);
            text.lines()
                .map(|line| line.split_whitespace().collect::<Vec<_>>())
                .find(|parts| parts.first().and_then(|p| p.parse::<u32>().ok()) == Some(pid))
                .map(|parts| parse_lstart(&parts[2..7]))
                .unwrap()
        };
        assert!(started_at > 0);
        assert!(same_process(pid, started_at));
        assert!(!same_process(pid, started_at + 1000));
        assert!(!same_process(pid, 0));
        drop(_guard);
        assert!(
            !same_process(pid, started_at),
            "a reaped pid no longer matches"
        );
    }
    #[tokio::test]
    async fn requires_an_http_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for response in [
                b"SSH-2.0-test\r\n".as_slice(),
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
            ] {
                let (mut client, _) = listener.accept().await.unwrap();
                let mut input = [0; 512];
                let _ = client.read(&mut input).await;
                client.write_all(response).await.unwrap();
            }
        });
        assert!(!is_http(address).await);
        assert!(is_http(address).await);
        task.await.unwrap();
    }
}
