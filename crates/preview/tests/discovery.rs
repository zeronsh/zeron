//! Real OS process/listener association: only servers a Zeron agent or
//! terminal started (carrying its origin marker), each under the right checkout.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use zeron_preview::{PreviewService, Scope};
use zeron_proto::{ORIGIN_ENV, chat_origin, terminal_origin};
/// Every service binds the fixed proxy port, so tests in this binary must
/// not run concurrently: one test freeing 7331 for its own proxy to reclaim
/// would otherwise race another test's service for it.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
const HTTP_SERVER: &str = "import http.server,socketserver; socketserver.TCPServer(('127.0.0.1',0),http.server.SimpleHTTPRequestHandler).serve_forever()";
fn scope(chats: &[(&str, &Path)]) -> Scope {
    Scope {
        chats: chats
            .iter()
            .map(|(chat, cwd)| ((*chat).to_owned(), cwd.to_path_buf()))
            .collect(),
        ..Default::default()
    }
}
/// Ids no other test shares: test runners (nextest) run each test in its own
/// process, concurrently, so a server one test starts is visible to every
/// other test's discovery, and a shared chat id would claim it there.
fn unique(name: &str) -> String {
    format!("{name}-{}", uuid::Uuid::new_v4())
}
/// A server started by `chat`'s agent; `None` is one the user started.
fn launch(cwd: &Path, http: bool, chat: Option<&str>) -> Child {
    launch_from(cwd, http, chat.map(chat_origin))
}
/// A server carrying `origin` (see [`ORIGIN_ENV`]); `None` is the user's.
fn launch_from(cwd: &Path, http: bool, origin: Option<String>) -> Child {
    let mut command = std::process::Command::new("python3");
    command.env_remove(ORIGIN_ENV);
    if let Some(origin) = origin {
        command.env(ORIGIN_ENV, origin);
    }
    if http {
        // HTTPServer.server_bind does a reverse DNS lookup before listen(),
        // which can stall on isolated macOS runners. TCPServer exercises the
        // same real HTTP handler without depending on external DNS readiness.
        command.args(["-c", HTTP_SERVER]);
    } else {
        command.args(["-c", "import socket,time; s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); time.sleep(30)"]);
    }
    Child(
        command
            .current_dir(cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}
async fn wait(service: &PreviewService, expected_pid: Option<u32>) {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let services = service.catalog().snapshot().services;
            if match expected_pid {
                Some(pid) => services.len() == 1 && services[0].pid == pid,
                None => services.is_empty(),
            } {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("discovery did not converge");
}
#[tokio::test]
async fn only_agent_started_http_processes_are_exposed_and_removals_are_live() {
    let _serial = SERIAL.lock().await;
    let (chat_a, chat_b) = (unique("chat-a"), unique("chat-b"));
    let (foreign, stray) = (unique("another-engines-chat"), unique("terminal"));
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("app");
    let b = temp.path().join("unrelated");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let local = launch(&a, true, Some(chat_a.as_str()));
    let other = launch(&b, true, Some(chat_b.as_str()));
    let _non_http = launch(&a, false, Some(chat_a.as_str()));
    // The user's own server in the same project, and one whose marker names
    // a chat this device does not host.
    let _users = launch(&a, true, None);
    let _foreign = launch(&a, true, Some(foreign.as_str()));
    // A terminal this device opened for no chat it hosts.
    let _stray_terminal = launch_from(&a, true, Some(terminal_origin(&stray)));
    let roots = Arc::new(Mutex::new(scope(&[(chat_a.as_str(), &a)])));
    let projects = roots.clone();
    let service = PreviewService::new(
        temp.path().join("names.json"),
        "local".into(),
        "Laptop".into(),
    )
    .unwrap();
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:7331").await.ok();
    service
        .start(Arc::new(move || projects.lock().unwrap().clone()), None)
        .await;
    wait(&service, Some(local.0.id())).await;
    if let Some(listener) = occupied {
        assert!(service.catalog().snapshot().error.is_some());
        drop(listener);
        tokio::time::timeout(Duration::from_secs(6), async {
            while service.catalog().snapshot().error.is_some() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("proxy did not retry the freed port");
    }
    // Several cycles pass and the user's server never joins it.
    tokio::time::sleep(Duration::from_secs(5)).await;
    wait(&service, Some(local.0.id())).await;
    let found = &service.catalog().snapshot().services[0];
    assert!(found.zeron_owned);
    assert_eq!(found.chat_id.as_deref(), Some(chat_a.as_str()));
    assert_eq!(std::path::Path::new(&found.cwd), a.canonicalize().unwrap());
    assert!(found.started_at > 0);
    *roots.lock().unwrap() = scope(&[(chat_b.as_str(), &b)]);
    wait(&service, Some(other.0.id())).await;
    drop(other);
    wait(&service, None).await;
    service.shutdown().await;
    // Shutdown fully releases the fixed proxy port before a profile boots.
    if service.catalog().snapshot().error.is_none() {
        assert!(
            tokio::net::TcpListener::bind("127.0.0.1:7331")
                .await
                .is_ok()
        );
    }
}

/// A discovered dev server receives exactly one probe for its lifetime. The
/// scanner used to send `HEAD /` every cycle to every project listener, which
/// showed up as request spam (and growing memory) in dev servers like Expo.
#[tokio::test]
async fn a_discovered_server_is_probed_once_not_every_cycle() {
    let _serial = SERIAL.lock().await;
    let chat = unique("chat");
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    let log = temp.path().join("requests.log");
    let script = format!(
        "import http.server,socketserver\n\
         class H(http.server.SimpleHTTPRequestHandler):\n\
         \x20   def do_HEAD(self):\n\
         \x20       open({log:?},'a').write(self.command+' '+self.path+'\\n')\n\
         \x20       super().do_HEAD()\n\
         \x20   def log_message(self,*a): pass\n\
         socketserver.TCPServer(('127.0.0.1',0),H).serve_forever()\n",
        log = log.display().to_string()
    );
    let server = Child(
        std::process::Command::new("python3")
            .args(["-c", &script])
            .env(ORIGIN_ENV, chat_origin(&chat))
            .current_dir(&app)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let service = PreviewService::new(
        temp.path().join("names.json"),
        "local".into(),
        "Laptop".into(),
    )
    .unwrap();
    let roots = scope(&[(chat.as_str(), &app)]);
    service.start(Arc::new(move || roots.clone()), None).await;
    wait(&service, Some(server.0.id())).await;
    // Several scan cycles (2s cadence) pass while the server stays discovered.
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert_eq!(
        service.catalog().snapshot().services.len(),
        1,
        "the server stays listed without being re-probed"
    );
    let requests = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        requests.lines().count(),
        1,
        "expected a single discovery probe, got:\n{requests}"
    );
    service.shutdown().await;
}

/// Infisical's browser login decodes the first callback request as JSON. A
/// discovery HEAD has no body and aborts that login with EOF. Even opening a
/// connection to these one-shot listeners is outside preview discovery's remit.
#[tokio::test]
async fn authentication_callbacks_receive_no_discovery_connections() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let _serial = SERIAL.lock().await;
    let chat = unique("chat");
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    let ready = temp.path().join("callback-port");
    let connections = temp.path().join("callback-connections");
    let script = format!(
        r#"import json,socket
s=socket.socket()
s.bind(('127.0.0.1',0))
s.listen()
open({ready:?},'w').write(str(s.getsockname()[1]))
c,_=s.accept()
open({connections:?},'a').write('connected\n')
r=c.makefile('rb')
request=r.readline()
headers={{}}
while True:
    line=r.readline()
    if line in (b'\r\n',b''): break
    name,value=line.decode().split(':',1)
    headers[name.lower()]=value.strip()
body=r.read(int(headers.get('content-length','0')))
if not body: raise RuntimeError('EOF')
assert request.startswith(b'POST ')
assert json.loads(body)=={{'code':'test-code'}}
c.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK')
"#,
        ready = ready.display().to_string(),
        connections = connections.display().to_string(),
    );
    let mut callback = Child(
        std::process::Command::new("python3")
            .args([
                "-c",
                &script,
                "login",
                "--domain",
                "https://app.infisical.com",
            ])
            .env(ORIGIN_ENV, chat_origin(&chat))
            .current_dir(&app)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let port: u16 = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(port) = std::fs::read_to_string(&ready)
                .ok()
                .and_then(|s| s.parse::<u16>().ok())
            {
                break port;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("callback did not start");
    let server = launch(&app, true, Some(chat.as_str()));
    let service = PreviewService::new(
        temp.path().join("names.json"),
        "local".into(),
        "Laptop".into(),
    )
    .unwrap();
    let roots = scope(&[(chat.as_str(), &app)]);
    service.start(Arc::new(move || roots.clone()), None).await;
    wait(&service, Some(server.0.id())).await;
    // Discovery actually runs, including multiple cycles after the ordinary
    // development server is found. The callback must remain entirely untouched.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        !connections.exists(),
        "preview discovery connected to the authentication callback"
    );
    assert!(callback.0.try_wait().unwrap().is_none());

    // The real browser request can still finish the waiting sign-in.
    let body = r#"{"code":"test-code"}"#;
    let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    browser
        .write_all(
            format!(
                "POST / HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        browser.read_to_string(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(std::fs::read_to_string(connections).unwrap(), "connected\n");
    assert!(callback.0.wait().unwrap().success());
    service.shutdown().await;
}

/// Agents background their servers (`nohup … &`, `setsid`, double forks) and
/// launchers scrub environments (`env -i`). Neither loses the server: the
/// marker survives reparenting, and a scrubbed server is attributed through
/// the marked process that launched it. A server started outside the
/// checkout is listed under the chat that started it.
#[cfg(unix)]
#[tokio::test]
async fn backgrounded_scrubbed_terminal_and_out_of_tree_servers_stay_attributed() {
    let _serial = SERIAL.lock().await;
    let chat = unique("chat");
    let terminal = unique("terminal");
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir(&app).unwrap();
    std::fs::create_dir(&elsewhere).unwrap();
    let python = String::from_utf8(
        std::process::Command::new("sh")
            .args(["-c", "command -v python3"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let pidfile = temp.path().join("daemon.pid");
    // A double fork: the server's parent exits at once, so it is reparented.
    let status = std::process::Command::new("sh")
        .args([
            "-c",
            &format!("( {python} -c \"{HTTP_SERVER}\" >/dev/null 2>&1 & echo $! > {pidfile:?} ) &"),
        ])
        .env(ORIGIN_ENV, chat_origin(&chat))
        .current_dir(&app)
        .status()
        .unwrap();
    assert!(status.success());
    let daemon: u32 = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("daemon did not start");
    struct Kill(u32);
    impl Drop for Kill {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0 as i32, libc::SIGKILL) };
        }
    }
    let _daemon = Kill(daemon);
    // A marked shell that stays alive around a server with no environment.
    let _scrubbed = Child(
        std::process::Command::new("sh")
            .args(["-c", &format!("env -i {python} -c \"{HTTP_SERVER}\"; true")])
            .env(ORIGIN_ENV, chat_origin(&chat))
            .current_dir(&app)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let outside = launch(&elsewhere, true, Some(chat.as_str()));
    // From a terminal (or the project action running in one), which may
    // have closed since: its chat is remembered.
    let from_terminal = launch_from(&app, true, Some(terminal_origin(&terminal)));
    let service = PreviewService::new(
        temp.path().join("names.json"),
        "local".into(),
        "Laptop".into(),
    )
    .unwrap();
    let mut roots = scope(&[(chat.as_str(), &app)]);
    roots.terminals.insert(terminal.clone(), chat.clone());
    service.start(Arc::new(move || roots.clone()), None).await;
    let app = app.canonicalize().unwrap();
    let services = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let services = service.catalog().snapshot().services;
            if services.len() == 4 {
                break services;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("discovery did not find every agent server");
    assert!(services.iter().any(|s| s.pid == daemon));
    assert!(services.iter().any(|s| s.pid == outside.0.id()));
    assert!(services.iter().any(|s| s.pid == from_terminal.0.id()));
    for service in &services {
        assert_eq!(service.chat_id.as_deref(), Some(chat.as_str()));
        assert_eq!(PathBuf::from(&service.project_cwd), app);
    }
    service.shutdown().await;
}
