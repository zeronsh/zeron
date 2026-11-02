//! `exec-counted` on this machine: stable user-space instruction counts,
//! signal forwarding, and a perf.json report.

#![cfg(unix)]

use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn scratch(name: &str) -> PathBuf {
    let dir =
        PathBuf::from("/home/user/Documents/dev/tmp/zeron-perf/scratch/b11-perf-counted/exec")
            .join(name)
            .join(std::process::id().to_string());
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn bench() -> &'static str {
    env!("CARGO_BIN_EXE_zeron-bench")
}

#[test]
fn exec_counted_busy_child() {
    if std::env::var_os("ZB_PERF_BUSY").is_none() {
        return;
    }
    let mut x = 1u64;
    for i in 1..40_000_000u64 {
        x = std::hint::black_box(x.wrapping_mul(i).wrapping_add(0x9E37));
    }
    std::hint::black_box(x);
    std::process::exit(0);
}

#[test]
fn busy_child_instructions_stay_within_one_percent() {
    let dir = scratch("stable");
    let exe = std::env::current_exe().unwrap();
    let mut counts = Vec::with_capacity(5);
    for n in 0..5 {
        let path = dir.join(format!("perf-{n}.json"));
        let output = Command::new(bench())
            .args([
                "exec-counted",
                "--",
                exe.to_str().unwrap(),
                "--exact",
                "exec_counted_busy_child",
            ])
            .env("ZB_PERF_BUSY", "1")
            .env("ZB_PERF_JSON", &path)
            .env_remove("ZERON_DATA_DIR")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "run {n} status {:?}\nstdout {}\nstderr {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{path:?}: {err}"));
        let parsed: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
        let instructions = parsed["instructions_u"].as_u64().unwrap_or_else(|| {
            panic!(
                "instructions_u null: {text}\nstderr {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert!(instructions > 1_000_000, "{text}");
        assert!(parsed["cycles_u"].as_u64().unwrap_or(0) > 0, "{text}");
        assert!(parsed["task_clock_ns"].as_u64().unwrap_or(0) > 0, "{text}");
        counts.push(instructions);
    }
    let min = counts.iter().copied().min().unwrap();
    let max = counts.iter().copied().max().unwrap();
    assert!(
        (max - min).saturating_mul(100) <= min,
        "instructions varied by more than 1%: min={min} max={max} counts={counts:?}"
    );
}

#[test]
fn exec_counted_forwards_sigterm_to_the_child_group() {
    let dir = scratch("signal");
    let ready = dir.join("ready");
    let grand = dir.join("grand");
    let perf = dir.join("perf.json");
    let script = r#"
import os, signal
open(os.environ["ZB_PERF_READY"], "w").write(str(os.getpid()))
pid = os.fork()
if pid == 0:
    open(os.environ["ZB_PERF_GRAND"], "w").write(str(os.getpid()))
    signal.pause()
else:
    signal.pause()
"#;
    let mut child = Command::new(bench())
        .args(["exec-counted", "--", "/usr/bin/python3", "-c", script])
        .env("ZB_PERF_READY", &ready)
        .env("ZB_PERF_GRAND", &grand)
        .env("ZB_PERF_JSON", &perf)
        .env_remove("ZERON_DATA_DIR")
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.is_file() || !grand.is_file() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("signal child did not become ready");
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("wrapper exited before the signal: {status}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let engine: u32 = fs::read_to_string(&ready).unwrap().trim().parse().unwrap();
    let grandchild: u32 = fs::read_to_string(&grand).unwrap().trim().parse().unwrap();

    let rc = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill wrapper: {}", std::io::Error::last_os_error());

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("wrapper did not exit after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
    assert!(!pid_alive(engine), "engine {engine} still alive");
    assert!(
        !pid_alive(grandchild),
        "grandchild {grandchild} still alive"
    );

    let text = fs::read_to_string(&perf).unwrap_or_else(|err| panic!("perf.json: {err}"));
    let parsed: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    assert!(parsed.get("instructions_u").is_some(), "{text}");
    assert!(parsed.get("cycles_u").is_some(), "{text}");
    assert!(parsed.get("task_clock_ns").is_some(), "{text}");
}

fn pid_alive(pid: u32) -> bool {
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
