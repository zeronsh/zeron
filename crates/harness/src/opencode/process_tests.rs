//! Exercise real launcher/server trees, including the npm/bun spawnSync layout.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    dir: tempfile::TempDir,
    exe: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("opencode");
        let launcher = format!(
            r#"#!/usr/bin/env node
const fs = require('node:fs');
const cp = require('node:child_process');
const path = require('node:path');
if (process.argv.includes('--version')) {{ console.log('1.2.27'); process.exit(0); }}
fs.writeFileSync(path.join(__dirname, 'launcher.pid'), String(process.pid));
const args = [path.join(__dirname, 'server.cjs'), {mode:?}, ...process.argv.slice(2)];
if ({mode:?} === 'launcher-exit') {{
  cp.spawn(process.execPath, args, {{stdio: 'inherit'}});
  setInterval(() => {{
    if (fs.existsSync(path.join(__dirname, 'server.pid'))) process.exit(1);
  }}, 10);
}} else {{
  const result = cp.spawnSync(process.execPath, args, {{stdio: 'inherit'}});
  process.exit(result.status ?? 1);
}}
"#
        );
        std::fs::write(&exe, launcher).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            dir.path().join("server.cjs"),
            r#"const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const mode = process.argv[2];
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
// Outlive the launcher's TERM so the grace deadline must escalate to KILL.
process.on('SIGTERM', () => {});
http.createServer((req, res) => {
  res.setHeader('content-type', 'application/json');
  if (req.url === '/global/health' && mode !== 'unhealthy' && mode !== 'launcher-exit') {
    res.end(JSON.stringify({version: '1.2.27'}));
  } else if (req.url === '/provider') {
    res.statusCode = 500; res.end(JSON.stringify({error: 'broken provider config'}));
  } else if (req.url.startsWith('/command')) {
    res.end(JSON.stringify([{name: 'init', description: 'Create AGENTS.md'}]));
  } else {
    res.statusCode = 404; res.end('{}');
  }
}).listen(port, '127.0.0.1', () => {
  fs.writeFileSync(path.join(__dirname, 'port'), String(port));
  fs.writeFileSync(path.join(__dirname, 'server.pid'), String(process.pid));
});
"#,
        )
        .unwrap();
        Self { dir, exe }
    }

    fn pid(&self, file: &str) -> i32 {
        std::fs::read_to_string(self.dir.path().join(file))
            .unwrap()
            .parse()
            .unwrap()
    }

    async fn wait_for_server(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while std::fs::read_to_string(self.dir.path().join("server.pid"))
                .ok()
                .and_then(|pid| pid.parse::<i32>().ok())
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture server must start");
    }

    async fn assert_stopped(&self) {
        for file in ["launcher.pid", "server.pid"] {
            let pid = self.pid(file);
            tokio::time::timeout(Duration::from_secs(3), async {
                while is_running(pid) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{file} ({pid}) leaked"));
        }
        let port = std::fs::read_to_string(self.dir.path().join("port")).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .is_ok()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned server must stop listening");
    }
}

fn is_running(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence of the recorded fixture process.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        // A killed orphan can await the host's reaper as a zombie.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if stat
            .rsplit_once(") ")
            .is_none_or(|(_, tail)| tail.starts_with('Z'))
        {
            return false;
        }
    }
    true
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Keep a failing regression run from leaking its own fixture processes.
        for file in ["server.pid", "launcher.pid"] {
            if let Ok(raw) = std::fs::read_to_string(self.dir.path().join(file))
                && let Ok(pid) = raw.parse::<i32>()
                && is_running(pid)
            {
                // SAFETY: this PID belongs to this test's private fixture.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

#[tokio::test]
async fn shutdown_kills_spawn_sync_server_even_after_launcher_exits() {
    let fixture = Fixture::new("ready");
    let mut server = Server::spawn(&fixture.exe, None, Duration::from_secs(5), None)
        .await
        .unwrap();
    server.shutdown(Duration::from_millis(100)).await;
    fixture.assert_stopped().await;
}

#[tokio::test]
async fn failed_model_discovery_retries_do_not_leak_servers() {
    let fixture = Fixture::new("ready");
    let mut harness = OpencodeHarness::new().with_executable(&fixture.exe);
    harness.kill_grace = Duration::from_millis(100);
    for _ in 0..3 {
        let error = harness.probe_models().await.unwrap_err();
        assert!(error.to_string().contains("500 Internal Server Error"));
        fixture.assert_stopped().await;
    }
}

#[tokio::test]
async fn command_and_skill_discovery_reap_launcher_and_server() {
    for operation in ["commands", "commands_for", "skills"] {
        let fixture = Fixture::new("ready");
        let mut harness = OpencodeHarness::new().with_executable(&fixture.exe);
        harness.kill_grace = Duration::from_millis(100);
        match operation {
            "commands" => assert_eq!(harness.commands().await.unwrap()[0].name, "init"),
            "commands_for" => assert_eq!(
                harness.commands_for(fixture.dir.path()).await.unwrap()[0].name,
                "init"
            ),
            "skills" => assert!(harness.skills(fixture.dir.path()).await.unwrap().is_some()),
            _ => unreachable!(),
        }
        fixture.assert_stopped().await;
    }
}

#[tokio::test]
async fn startup_timeout_reaps_launcher_and_server() {
    let fixture = Fixture::new("unhealthy");
    let error = Server::spawn(&fixture.exe, None, Duration::from_millis(500), None)
        .await
        .err()
        .expect("unhealthy fixture must fail startup");
    assert!(error.to_string().contains("did not become healthy"));
    fixture.assert_stopped().await;
}

#[tokio::test]
async fn cancelling_startup_reaps_launcher_and_server() {
    let fixture = Fixture::new("unhealthy");
    let mut spawn = Box::pin(Server::spawn(
        &fixture.exe,
        None,
        Duration::from_secs(30),
        None,
    ));
    tokio::select! {
        result = &mut spawn => panic!("startup unexpectedly finished: {:?}", result.err()),
        _ = fixture.wait_for_server() => {}
    }
    drop(spawn);
    fixture.assert_stopped().await;
}

#[tokio::test]
async fn launcher_crash_during_startup_reaps_surviving_server() {
    let fixture = Fixture::new("launcher-exit");
    let error = Server::spawn(&fixture.exe, None, Duration::from_secs(5), None)
        .await
        .err()
        .expect("launcher crash must fail startup");
    assert!(error.to_string().contains("opencode serve"));
    fixture.assert_stopped().await;
}

#[tokio::test]
async fn dropping_ready_server_reaps_launcher_and_server() {
    let fixture = Fixture::new("ready");
    let server = Server::spawn(&fixture.exe, None, Duration::from_secs(5), None)
        .await
        .unwrap();
    drop(server);
    fixture.assert_stopped().await;
}

#[tokio::test]
async fn cancelling_shutdown_preserves_group_cleanup_on_drop() {
    let fixture = Fixture::new("ready");
    let mut server = Server::spawn(&fixture.exe, None, Duration::from_secs(5), None)
        .await
        .unwrap();
    let mut shutdown = Box::pin(server.shutdown(Duration::from_secs(30)));
    tokio::select! {
        _ = &mut shutdown => panic!("TERM-ignoring server must still be in its grace period"),
        _ = async {
            tokio::time::timeout(Duration::from_secs(3), async {
                while is_running(fixture.pid("launcher.pid")) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("launcher must receive TERM");
        } => {}
    }
    assert!(is_running(fixture.pid("server.pid")));
    drop(shutdown);
    drop(server);
    fixture.assert_stopped().await;
}
