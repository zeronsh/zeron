//! Launch a fake engine through the real scope path.
//!
//! The fake engine is a Python script written under this unit's scratch
//! directory. It is not a committed binary. Nothing here opens `~/.zeron`
//! for write or binds port 27654.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use zeron_bench::Trial;

const FAKE_ENGINE: &str = r#"#!/usr/bin/python3
import base64, hashlib, json, os, socket, struct, sys, threading

FORBIDDEN = (
    "ZERON_CHAT_ID",
    "ZERON_IPC_TOKEN",
    "ZERON_IPC_TOKEN_FILE",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "RUST_LOG",
)
GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
data = os.environ["ZERON_DATA_DIR"]
port = int(os.environ["ZERON_IPC_PORT"])
os.makedirs(data, exist_ok=True)
seen = os.path.join(data, "seen-env.txt")
with open(seen, "w", encoding="utf-8") as handle:
    for key in sorted(os.environ):
        handle.write(f"{key}={os.environ[key]}\n")
leaked = [key for key in FORBIDDEN if key in os.environ]
if leaked:
    sys.stderr.write("forbidden env: " + ",".join(leaked) + "\n")
    sys.exit(3)
if port == 27654:
    sys.stderr.write("refusing port 27654\n")
    sys.exit(4)
token_path = os.path.join(data, "ipc-token")
fd = os.open(token_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(fd, b"bench-token\n")
os.close(fd)

def recv_exact(sock, n, buf):
    while len(buf) < n:
        chunk = sock.recv(4096)
        if not chunk:
            return None
        buf.extend(chunk)
    out = bytes(buf[:n])
    del buf[:n]
    return out

def read_headers(sock, buf):
    while b"\r\n\r\n" not in buf:
        chunk = sock.recv(4096)
        if not chunk:
            return None
        buf.extend(chunk)
    raw = bytes(buf)
    head, _, rest = raw.partition(b"\r\n\r\n")
    buf[:] = rest
    return head.decode("latin1")

def send_frame(sock, opcode, payload):
    n = len(payload)
    header = bytearray([0x80 | opcode])
    if n < 126:
        header.append(n)
    elif n < 65536:
        header.append(126)
        header.extend(struct.pack("!H", n))
    else:
        header.append(127)
        header.extend(struct.pack("!Q", n))
    sock.sendall(bytes(header) + payload)

def read_frame(sock, buf):
    hdr = recv_exact(sock, 2, buf)
    if hdr is None:
        return None
    opcode = hdr[0] & 0x0F
    masked = hdr[1] & 0x80
    length = hdr[1] & 0x7F
    if length == 126:
        ext = recv_exact(sock, 2, buf)
        if ext is None:
            return None
        length = struct.unpack("!H", ext)[0]
    elif length == 127:
        ext = recv_exact(sock, 8, buf)
        if ext is None:
            return None
        length = struct.unpack("!Q", ext)[0]
    mask = b""
    if masked:
        mask = recv_exact(sock, 4, buf)
        if mask is None:
            return None
    payload = recv_exact(sock, length, buf) if length else b""
    if payload is None:
        return None
    if masked:
        payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    return opcode, payload

def handle(conn):
    buf = bytearray()
    head = read_headers(conn, buf)
    if not head:
        return False
    key = None
    for line in head.split("\r\n"):
        if line.lower().startswith("sec-websocket-key:"):
            key = line.split(":", 1)[1].strip()
    if not key:
        return False
    accept = base64.b64encode(hashlib.sha1(key.encode() + GUID).digest()).decode()
    conn.sendall(
        (
            "HTTP/1.1 101 Switching Protocols\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Accept: {accept}\r\n"
            "\r\n"
        ).encode()
    )
    while True:
        frame = read_frame(conn, buf)
        if frame is None:
            return False
        opcode, payload = frame
        if opcode == 0x8:
            send_frame(conn, 0x8, payload[:2])
            return False
        if opcode == 0x9:
            send_frame(conn, 0xA, payload)
            continue
        if opcode != 0x1:
            continue
        msg = json.loads(payload.decode())
        reply = json.dumps({"id": msg.get("id", 0), "ok": True}).encode()
        send_frame(conn, 0x1, reply)
        if msg.get("method") == "StopEngine":
            return True

def serve_one(conn, stop):
    try:
        conn.settimeout(30)
        if handle(conn):
            stop.set()
    except Exception as err:
        sys.stderr.write(f"conn error: {err}\n")
        sys.stderr.flush()
    finally:
        try:
            conn.close()
        except Exception:
            pass

stop = threading.Event()
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", port))
srv.listen(16)
srv.settimeout(0.5)
sys.stderr.write(f"listening {port}\n")
sys.stderr.flush()
while not stop.is_set():
    try:
        conn, _ = srv.accept()
    except socket.timeout:
        continue
    threading.Thread(target=serve_one, args=(conn, stop), daemon=True).start()
sys.exit(0)
"#;

fn scratch_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/user"));
    home.join("Documents/dev/tmp/zeron-perf/scratch/b0-bench-scaffold/it")
        .join(std::process::id().to_string())
}

fn write_fake_engine(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("fake-engine.py");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&path)
        .unwrap();
    file.write_all(FAKE_ENGINE.as_bytes()).unwrap();
    path
}

fn systemctl_value(unit: &str, prop: &str) -> String {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            &format!("{unit}.scope"),
            &format!("-p{prop}"),
            "--value",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl Drop for RestoreEnv {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                // SAFETY: these names are the bench isolation test's own.
                // `set_var` races with `getenv` on other threads; this test
                // is the only one in this binary that writes them.
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

#[tokio::test]
async fn scoped_fake_engine_reaches_ready_and_leaves_nothing() {
    let saved = RestoreEnv(
        ["ZERON_IPC_TOKEN", "ZERON_CHAT_ID", "CLAUDECODE", "RUST_LOG"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    );
    // SAFETY: see `RestoreEnv`. The parent must actually hold these names
    // so a leak through `systemd-run` shows up in the engine's env file.
    unsafe {
        std::env::set_var("ZERON_IPC_TOKEN", "parent-secret");
        std::env::set_var("ZERON_CHAT_ID", "parent-chat");
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("RUST_LOG", "debug");
    }

    let zb = scratch_root().join("launch");
    let _ = fs::remove_dir_all(&zb);
    fs::create_dir_all(&zb).unwrap();
    let engine_bin = write_fake_engine(&zb);
    let lock = home_zeron_lock();
    let lock_before = lock.as_ref().and_then(|path| fs::read(path).ok());

    let mut trial = Trial::at(&zb, &format!("fake-{}", std::process::id()), 0, "a").unwrap();
    trial.bench_bin = PathBuf::from(env!("CARGO_BIN_EXE_zeron-bench"));
    trial.memory_max = "256M".into();
    trial.ready_timeout = Duration::from_secs(20);
    trial.allocate_port().unwrap();
    trial.prepare().unwrap();
    assert_ne!(trial.port, 27654);

    let launched = zeron_bench::launch(&engine_bin, &trial)
        .await
        .unwrap_or_else(|err| {
            let stderr = fs::read_to_string(trial.root.join("engine.stderr")).unwrap_or_default();
            panic!("launch failed: {err:#}\n--- stderr ---\n{stderr}");
        });
    let unit = launched
        .scope
        .clone()
        .expect("this machine has a user systemd bus; scope fallback is a bug here");
    assert_eq!(launched.containment(), "scope");
    assert!(launched.t_listen >= launched.t_spawn);
    assert!(launched.t_ready >= launched.t_listen);
    assert!(pid_alive(launched.pid));
    assert_ne!(trial.port, 27654);

    let active = systemctl_value(&unit, "ActiveState");
    assert_eq!(active, "active", "unit {unit}");
    let memory = systemctl_value(&unit, "MemoryMax");
    assert!(
        memory == "268435456" || memory == "256M",
        "MemoryMax={memory}"
    );

    let seen = fs::read_to_string(trial.data.join("seen-env.txt")).unwrap();
    for key in [
        "ZERON_IPC_TOKEN",
        "ZERON_CHAT_ID",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "RUST_LOG",
        "parent-secret",
        "parent-chat",
    ] {
        assert!(!seen.contains(key), "engine saw {key}:\n{seen}");
    }
    assert!(seen.contains(&format!("ZERON_IPC_PORT={}\n", trial.port)));
    assert!(seen.contains(&format!("HOME={}\n", trial.home.display())));
    assert!(seen.contains("ZERON_NO_LOGIN_SHELL=1\n"));

    let pid = launched.pid;
    let report = launched.stop().await.unwrap();
    assert_eq!(report.containment, "scope");
    assert!(report.stop_engine_answered, "{report:?}");
    assert_eq!(report.how, zeron_bench::StopHow::Rpc);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut load = String::new();
    while std::time::Instant::now() < deadline {
        if !pid_alive(pid) {
            load = systemctl_value(&unit, "LoadState");
            if load == "not-found" || load.is_empty() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!pid_alive(pid), "pid {pid} still alive");
    assert!(
        load == "not-found" || load.is_empty(),
        "scope {unit} still loaded ({load})"
    );
    assert!(!unit_dir_exists(&unit), "cgroup for {unit} still exists");

    if let Some(path) = &lock {
        let lock_after = fs::read(path).ok();
        assert_eq!(lock_before, lock_after, "engine.lock bytes changed");
    }
    drop(saved);
}

fn home_zeron_lock() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".zeron").join("engine.lock");
    path.is_file().then_some(path)
}

fn unit_dir_exists(unit: &str) -> bool {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            &format!("{unit}.scope"),
            "-pControlGroup",
            "--value",
        ])
        .output();
    let Ok(output) = output else {
        return false;
    };
    let rel = String::from_utf8_lossy(&output.stdout);
    let rel = rel.trim();
    if rel.is_empty() {
        return false;
    }
    Path::new("/sys/fs/cgroup")
        .join(rel.trim_start_matches('/'))
        .is_dir()
}

#[test]
fn exec_counted_replaces_itself_with_the_command() {
    let bin = env!("CARGO_BIN_EXE_zeron-bench");
    let status = Command::new(bin)
        .args(["exec-counted", "--", "/usr/bin/true"])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn doctor_cli_reports_each_probe() {
    let bin = env!("CARGO_BIN_EXE_zeron-bench");
    let output = Command::new(bin).arg("doctor").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for needle in ["scope:", "smaps:", "perf:", "rapl:", "real_engine:"] {
        assert!(text.contains(needle), "{text}");
    }
}
