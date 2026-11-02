//! `zb-shim` resolves the real binary, reports one datagram per spawn, and
//! does not exec itself. The idle-engine check is ignored: it holds a
//! release `zeron-engine` for a minute and is run explicitly.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

fn scratch(label: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user"));
    let dir = home
        .join("Documents/dev/tmp/zeron-perf/scratch/b12-spawn-shim/it")
        .join(format!("{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Short bench root. The idle trial's probe socket is a unix datagram path.
fn short_zb(label: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user"));
    let dir = home
        .join("Documents/dev/tmp/zeron-perf/scratch/b12")
        .join(format!("{label}{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn shim_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_zb-shim"))
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "zeron-bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.invalid")
        .env("GIT_COMMITTER_NAME", "zeron-bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.invalid")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()
        .unwrap()
}

fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    let init = git(dir, &["init", "-b", "main"]);
    assert!(
        init.status.success(),
        "git init: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    fs::write(dir.join("README"), b"bench\n").unwrap();
    let add = git(dir, &["add", "README"]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let commit = git(dir, &["commit", "-m", "init"]);
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );
}

fn link_shim(dir: &Path, name: &str) {
    fs::create_dir_all(dir).unwrap();
    let link = dir.join(name);
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(shim_bin(), &link).unwrap();
}

fn run_through(path: &str, sock: Option<&Path>, cwd: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..])
        .current_dir(cwd)
        .env("PATH", path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match sock {
        Some(sock) => {
            cmd.env("ZB_PROBE_SOCK", sock);
        }
        None => {
            cmd.env_remove("ZB_PROBE_SOCK");
        }
    }
    cmd.output().unwrap()
}

struct Probe {
    sock: UnixDatagram,
    path: PathBuf,
}

impl Probe {
    fn bind(dir: &Path) -> Self {
        let path = dir.join("probe.sock");
        let _ = fs::remove_file(&path);
        let sock = UnixDatagram::bind(&path).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        Self { sock, path }
    }

    fn recv(&self) -> Vec<u8> {
        let mut buf = [0u8; 8192];
        let n = self
            .sock
            .recv(&mut buf)
            .unwrap_or_else(|err| panic!("probe recv: {err}"));
        buf[..n].to_vec()
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn git_status_through_the_shim_reports_one_spawn() {
    let root = scratch("status");
    let repo = root.join("repo");
    init_repo(&repo);
    fs::write(repo.join("new.txt"), b"x\n").unwrap();
    let shims = root.join("shims");
    link_shim(&shims, "git");
    let probe = Probe::bind(&root);
    let path = format!("{}:/usr/bin:/bin", shims.display());
    let out = run_through(
        &path,
        Some(&probe.path),
        &repo,
        &["git", "status", "--porcelain"],
    );
    assert!(
        out.status.success(),
        "status {}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("new.txt"), "{text}");
    let msg = String::from_utf8(probe.recv()).unwrap();
    assert!(msg.contains(r#""k":"spawn""#), "{msg}");
    assert!(msg.contains(r#""prog":"git""#), "{msg}");
    assert!(msg.contains(r#""status""#), "{msg}");
    assert!(
        msg.contains(&format!(r#""ppid":{}"#, std::process::id())),
        "{msg}"
    );
    let again = run_through(&path, None, &repo, &["git", "--version"]);
    assert!(
        again.status.success(),
        "missing socket: {}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(String::from_utf8_lossy(&again.stdout).contains("git version"));
}

#[test]
fn the_shim_does_not_recurse_when_its_directory_is_listed_twice() {
    let root = scratch("recurse");
    let first = root.join("shims");
    let second = root.join("again");
    link_shim(&first, "git");
    link_shim(&second, "git");
    let path = format!("{}:{}:/usr/bin:/bin", first.display(), second.display());
    let mut child = Command::new("git")
        .arg("--version")
        .env("PATH", &path)
        .env("ZB_PROBE_SOCK", root.join("missing.sock"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(2) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("shim still running after 2s; it exec'd itself");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    let _ = std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut stdout);
    assert!(status.success(), "exit {status}");
    assert!(stdout.contains("git version"), "{stdout}");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn shim_only_path_exits_127_instead_of_looping() {
    let root = scratch("only");
    let shims = root.join("shims");
    link_shim(&shims, "git");
    let out = run_through(
        &shims.display().to_string(),
        None,
        &root,
        &["git", "--version"],
    );
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("command not found"), "{err}");
}

fn time_true(path: &str, sock: Option<&Path>, n: u32) -> Duration {
    let start = Instant::now();
    for _ in 0..n {
        let mut cmd = Command::new("true");
        cmd.env("PATH", path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match sock {
            Some(sock) => {
                cmd.env("ZB_PROBE_SOCK", sock);
            }
            None => {
                cmd.env_remove("ZB_PROBE_SOCK");
            }
        }
        let status = cmd.status().unwrap();
        assert!(status.success());
    }
    start.elapsed()
}

fn overhead_ms(shim_dir: &Path, sock: Option<&Path>, samples: u32) -> f64 {
    link_shim(shim_dir, "true");
    let via = format!("{}:/usr/bin:/bin", shim_dir.display());
    let direct = "/usr/bin:/bin";
    // One call so the pages are resident before the timed loop.
    let _ = time_true(&via, sock, 1);
    let _ = time_true(direct, None, 1);
    let shimmed = time_true(&via, sock, samples);
    let plain = time_true(direct, None, samples);
    let extra = shimmed.saturating_sub(plain);
    extra.as_secs_f64() * 1_000.0 / f64::from(samples)
}

#[test]
fn debug_shim_overhead_stays_small() {
    let root = scratch("overhead");
    let ms = overhead_ms(&root.join("shims"), None, 40);
    eprintln!("debug shim overhead {ms:.3} ms/spawn");
    // Dev builds of this package are opt-level 0. The acceptance bound is
    // the release binary (`release_shim_overhead_is_under_a_millisecond`).
    assert!(ms < 25.0, "debug overhead {ms:.3} ms");
}

#[test]
#[ignore = "release binary; set ZB_SHIM_BIN to target/release/zb-shim"]
fn release_shim_overhead_is_under_a_millisecond() {
    let bin = std::env::var_os("ZB_SHIM_BIN").expect("ZB_SHIM_BIN");
    let root = scratch("release-overhead");
    let shims = root.join("shims");
    fs::create_dir_all(&shims).unwrap();
    std::os::unix::fs::symlink(&bin, shims.join("true")).unwrap();
    let probe = Probe::bind(&root);
    let ms = overhead_ms_linked(&shims, Some(&probe.path), 100);
    eprintln!("release shim overhead {ms:.3} ms/spawn");
    assert!(ms < 1.0, "overhead {ms:.3} ms/spawn");
}

fn overhead_ms_linked(shim_dir: &Path, sock: Option<&Path>, samples: u32) -> f64 {
    let via = format!("{}:/usr/bin:/bin", shim_dir.display());
    let direct = "/usr/bin:/bin";
    let _ = time_true(&via, sock, 1);
    let _ = time_true(direct, None, 1);
    let shimmed = time_true(&via, sock, samples);
    let plain = time_true(direct, None, samples);
    shimmed.saturating_sub(plain).as_secs_f64() * 1_000.0 / f64::from(samples)
}

const SPACE_RPC: &str = r#"#!/usr/bin/env python3
import base64, json, os, socket, struct, sys
host, port, token, folder = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
sock = socket.create_connection((host, port), timeout=10)
key = base64.b64encode(os.urandom(16)).decode()
sock.sendall(
    (f"GET / HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\n"
     "Connection: Upgrade\r\nSec-WebSocket-Key: " + key + "\r\n"
     "Sec-WebSocket-Version: 13\r\nAuthorization: Bearer " + token + "\r\n\r\n").encode()
)
buf = b""
while b"\r\n\r\n" not in buf:
    chunk = sock.recv(4096)
    if not chunk:
        sys.exit("eof during handshake")
    buf += chunk
head, _, rest = buf.partition(b"\r\n\r\n")
if b" 101 " not in head.split(b"\r\n", 1)[0]:
    sys.exit("handshake failed: " + head.decode("latin1", "replace"))
pending = bytearray(rest)

def send_bytes(opcode, payload):
    n = len(payload)
    hdr = bytearray([0x80 | opcode])
    if n < 126:
        hdr.append(0x80 | n)
    elif n < 65536:
        hdr.append(0x80 | 126)
        hdr.extend(struct.pack("!H", n))
    else:
        hdr.append(0x80 | 127)
        hdr.extend(struct.pack("!Q", n))
    mask = os.urandom(4)
    hdr.extend(mask)
    sock.sendall(bytes(hdr) + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))

def recv_exact(n):
    while len(pending) < n:
        chunk = sock.recv(4096)
        if not chunk:
            return None
        pending.extend(chunk)
    out = bytes(pending[:n])
    del pending[:n]
    return out

def recv_frame():
    hdr = recv_exact(2)
    if hdr is None:
        return None
    opcode = hdr[0] & 0x0F
    length = hdr[1] & 0x7F
    if length == 126:
        length = struct.unpack("!H", recv_exact(2))[0]
    elif length == 127:
        length = struct.unpack("!Q", recv_exact(8))[0]
    data = recv_exact(length)
    if data is None:
        return None
    return opcode, data

def call(i, method, params):
    send_bytes(1, json.dumps({"id": i, "method": method, "params": params}).encode())
    while True:
        frame = recv_frame()
        if frame is None:
            sys.exit("eof waiting for " + method)
        opcode, data = frame
        if opcode == 9:
            send_bytes(10, data)
            continue
        if opcode == 8:
            sys.exit("closed during " + method + ": " + data.decode("latin1", "replace"))
        if opcode != 1:
            continue
        msg = json.loads(data.decode())
        if msg.get("id") != i:
            continue
        if msg.get("err"):
            sys.exit(method + " error: " + str(msg["err"]))
        return msg.get("ok")

device = call(1, "LocalDevice", {})
call(2, "Mutate", {
    "op": "createSpace",
    "spaceId": "b12-space",
    "deviceId": device["deviceId"],
    "path": folder,
    "name": "b12",
    "gitDetected": False,
})
print(device["deviceId"])
"#;

struct ProbeReader {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Vec<Vec<u8>>>>,
}

impl ProbeReader {
    fn start(sock: UnixDatagram) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let mut messages = Vec::new();
            let mut buf = [0u8; 8192];
            while !flag.load(Ordering::Relaxed) {
                match sock.recv(&mut buf) {
                    Ok(n) => messages.push(buf[..n].to_vec()),
                    Err(err)
                        if err.kind() == std::io::ErrorKind::WouldBlock
                            || err.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => break,
                }
            }
            let _ = sock.set_nonblocking(true);
            loop {
                match sock.recv(&mut buf) {
                    Ok(n) => messages.push(buf[..n].to_vec()),
                    Err(_) => break,
                }
            }
            messages
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> Vec<Vec<u8>> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap()
    }
}

impl Drop for ProbeReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn count_git(messages: &[Vec<u8>]) -> usize {
    messages
        .iter()
        .filter(|msg| {
            let text = String::from_utf8_lossy(msg);
            text.contains(r#""k":"spawn""#) && text.contains(r#""prog":"git""#)
        })
        .count()
}

#[tokio::test]
#[ignore = "one minute with a release zeron-engine; ZB_ENGINE_BIN overrides the path"]
async fn idle_engine_on_a_git_space_counts_git_spawns() {
    let engine = std::env::var_os("ZB_ENGINE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                "/home/user/Documents/dev/zeron-wt/targets/perf-b0-bench-scaffold/release/zeron-engine",
            )
        });
    assert!(engine.is_file(), "engine binary {}", engine.display());
    // `Trial::at` appends `/runs/<id>/trials/0-a/probe.sock`. The datagram
    // path has to fit in `sockaddr_un` (108 bytes including the NUL).
    let zb = short_zb("i");
    let mut trial = zeron_bench::Trial::at(&zb, "m", 0, "a").unwrap();
    trial.bench_bin = PathBuf::from(env!("CARGO_BIN_EXE_zeron-bench"));
    trial.memory_max = "2G".into();
    trial.ready_timeout = Duration::from_secs(60);
    trial.allocate_port().unwrap();
    trial.prepare().unwrap();
    assert!(
        trial.probe_sock.as_os_str().len() < 100,
        "probe path too long for sockaddr_un: {}",
        trial.probe_sock.display()
    );
    let repo = trial.root.join("space");
    init_repo(&repo);
    let _ = fs::remove_file(&trial.probe_sock);
    let sock = UnixDatagram::bind(&trial.probe_sock).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let reader = ProbeReader::start(sock);

    let launched = zeron_bench::launch(&engine, &trial)
        .await
        .unwrap_or_else(|err| {
            let stderr = fs::read_to_string(trial.root.join("engine.stderr")).unwrap_or_default();
            panic!("launch failed: {err:#}\n--- stderr ---\n{stderr}");
        });
    let git_link = fs::read_link(trial.shims.join("git")).unwrap_or_else(|err| {
        panic!("shim git link missing: {err}");
    });
    assert_eq!(git_link, Path::new("zb-shim"));

    let token = zeron_bench::trial_ipc_token(&trial.data)
        .unwrap()
        .expect("ipc-token");
    let script = trial.root.join("space-rpc.py");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&script)
        .unwrap();
    file.write_all(SPACE_RPC.as_bytes()).unwrap();
    drop(file);
    let rpc = Command::new("python3")
        .arg(&script)
        .arg("127.0.0.1")
        .arg(trial.port.to_string())
        .arg(token.trim())
        .arg(&repo)
        .output()
        .unwrap();
    assert!(
        rpc.status.success(),
        "create space failed: {}\n{}",
        String::from_utf8_lossy(&rpc.stdout),
        String::from_utf8_lossy(&rpc.stderr)
    );

    let idle_for = Duration::from_secs(60);
    let deadline = Instant::now() + idle_for;
    while Instant::now() < deadline {
        if !Path::new(&format!("/proc/{}", launched.pid)).exists() {
            let stderr = fs::read_to_string(trial.root.join("engine.stderr")).unwrap_or_default();
            panic!("engine exited during idle\n{stderr}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let report = launched.stop().await.unwrap();
    assert_eq!(report.containment, "scope");
    let messages = reader.finish();
    let git_spawns = count_git(&messages);
    assert!(
        git_spawns > 0,
        "spawns.git was 0 after {}s idle ({} datagrams). sample: {:?}",
        idle_for.as_secs(),
        messages.len(),
        messages
            .iter()
            .take(4)
            .map(|msg| String::from_utf8_lossy(msg).into_owned())
            .collect::<Vec<_>>()
    );
    eprintln!(
        "idle spawns.git={git_spawns} datagrams={} device={}",
        messages.len(),
        String::from_utf8_lossy(&rpc.stdout).trim()
    );
}
