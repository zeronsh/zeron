#![cfg(unix)]
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Peer {
    child: Child,
    input: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    seen: Vec<Value>,
}
fn temp() -> tempfile::TempDir {
    let root =
        PathBuf::from(std::env::var_os("CARGO_TARGET_DIR").expect("isolated target required"))
            .join("mcp-proxy-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn pids(path: &Path) -> Vec<u32> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|v| v.parse().unwrap())
        .collect()
}
fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|s| !s.rsplit_once(')').unwrap().1.trim().starts_with('Z'))
    }
    #[cfg(not(target_os = "linux"))]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}
async fn eventually(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition timed out");
}
impl Peer {
    fn spawn(dir: &Path, mode: &str, env: &[(&str, &str)]) -> Self {
        Self::spawn_command(dir, mode, env, env!("CARGO_BIN_EXE_fake-mcp"), &[], &[])
    }
    fn spawn_command(
        dir: &Path,
        mode: &str,
        env: &[(&str, &str)],
        command: &str,
        args: &[&str],
        proxy_args: &[&str],
    ) -> Self {
        let stderr = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("stderr.log"))
            .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zeron-mcp-proxy"));
        cmd.args(["--server", "fake", "--mode", mode, "--run-dir"])
            .arg(dir.join("run"))
            .args(proxy_args)
            .arg("--")
            .arg(command)
            .args(args)
            .env("ZERON_DATA_DIR", dir)
            .env("FAKE_SPAWN_MARK", dir.join("spawn"))
            .env("FAKE_CHILD_MARK", dir.join("children"))
            .env("FAKE_REQUEST_LOG", dir.join("requests"))
            .env("ZERON_MCP_PROXY_CHECK_MS", "20")
            .env("ZERON_MCP_PROXY_IDLE_MS", "150")
            .env("ZERON_MCP_PROXY_BACKOFF_MS", "250")
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .kill_on_drop(true);
        let mut child = cmd.spawn().unwrap();
        Self {
            input: child.stdin.take(),
            lines: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            seen: Vec::new(),
        }
    }
    async fn send(&mut self, v: Value) {
        self.input
            .as_mut()
            .unwrap()
            .write_all(format!("{v}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(8), self.lines.next_line())
            .await
            .expect("response timeout")
            .unwrap()
            .expect("proxy EOF");
        let v: Value = serde_json::from_str(&line).unwrap();
        self.seen.push(v.clone());
        v
    }
    async fn response(&mut self, id: Value) -> Value {
        loop {
            let v = self.recv().await;
            if v["id"] == id && v.get("method").is_none() {
                return v;
            }
        }
    }
    async fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(zeron_mcp_proxy::rpc::request(id.clone(), method, params))
            .await;
        self.response(id).await
    }
    async fn init(&mut self) -> Value {
        let response = self.request(json!("init"),"initialize",json!({"protocolVersion":"2025-03-26","clientInfo":{"name":"e2e","version":"1.2.3"},"capabilities":{"sampling":{},"roots":{"listChanged":true}}})).await;
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        response
    }
    async fn call(&mut self, id: i32, name: &str) -> Value {
        self.request(json!(id), "tools/call", json!({"name":name}))
            .await
    }
    async fn close(mut self) {
        self.input.take();
        assert!(
            tokio::time::timeout(Duration::from_secs(8), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
async fn seed(dir: &Path) {
    let mut peer = Peer::spawn(dir, "lazy", &[]);
    assert!(peer.init().await.get("result").is_some());
    eventually(|| pids(&dir.join("spawn")).len() == 1 && !alive(pids(&dir.join("spawn"))[0])).await;
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn miss_captures_all_pages_and_kills_the_whole_tree() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(
        d,
        "lazy",
        &[
            ("FAKE_PAGED", "1"),
            ("FAKE_GRANDCHILD", "1"),
            ("FAKE_WATCHDOG", "1"),
        ],
    );
    assert_eq!(
        peer.init().await["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    let tools = peer.request(json!(2), "tools/list", json!({})).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 2);
    assert!(tools["result"].get("nextCursor").is_none());
    eventually(|| {
        pids(&d.join("spawn")).len() == 1
            && pids(&d.join("children")).len() == 2
            && pids(&d.join("spawn"))
                .iter()
                .chain(pids(&d.join("children")).iter())
                .all(|p| !alive(*p))
    })
    .await;
    assert_eq!(
        fs::read_dir(d.join("mcp-cache/v1"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|s| s == "json"))
            .count(),
        1
    );
    assert_eq!(fs::read_dir(d.join("run")).unwrap().count(), 0);
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn cache_hit_has_no_child_then_call_and_idle_restart() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(d, "lazy", &[]);
    peer.init().await;
    assert_eq!(
        peer.request(json!("zp:1"), "tools/list", json!({})).await["result"]["tools"][0]["name"],
        "echo"
    );
    assert_eq!(pids(&d.join("spawn")).len(), 1);
    assert_eq!(
        peer.call(3, "echo").await["result"]["content"][0]["text"],
        "echo"
    );
    assert_eq!(pids(&d.join("spawn")).len(), 2);
    eventually(|| !alive(pids(&d.join("spawn"))[1])).await;
    peer.call(4, "echo").await;
    assert_eq!(pids(&d.join("spawn")).len(), 3);
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn sampling_progress_and_inflight_block_idle() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(
        d,
        "lazy",
        &[("FAKE_SAMPLING_ON", "sample"), ("FAKE_PROGRESS", "2")],
    );
    peer.init().await;
    peer.send(zeron_mcp_proxy::rpc::request(
        "zp:init",
        "tools/call",
        json!({"name":"sample","_meta":{"progressToken":"mine"}}),
    ))
    .await;
    let sampling = loop {
        let v = peer.recv().await;
        if v["method"] == "sampling/createMessage" {
            break v;
        }
    };
    assert!(
        peer.seen
            .iter()
            .any(|v| v["method"] == "notifications/progress"
                && v["params"]["progressToken"] == "mine")
    );
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(alive(*pids(&d.join("spawn")).last().unwrap()));
    peer.send(json!({"jsonrpc":"2.0","id":sampling["id"],"result":{"model":"fake","content":{"type":"text","text":"sampled"}}})).await;
    let response = peer.response(json!("zp:init")).await;
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("sampled")
    );
    // Progress keeps the server warm for 30 seconds after the call ends.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(alive(*pids(&d.join("spawn")).last().unwrap()));
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn cancelled_queued_call_never_reaches_child_or_responds() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_INIT_DELAY_MS", "200")]);
    peer.init().await;
    peer.send(zeron_mcp_proxy::rpc::request(
        10,
        "tools/call",
        json!({"name":"cancel-me"}),
    ))
    .await;
    peer.send(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":10}}),
    )
    .await;
    assert!(peer.call(11, "echo").await.get("result").is_some());
    assert!(!peer.seen.iter().any(|v| v["id"] == 10));
    assert!(
        !fs::read_to_string(d.join("requests"))
            .unwrap()
            .contains("cancel-me")
    );
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn refresh_and_list_changed_replace_cached_catalog() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let changed = r#"[{"name":"new","inputSchema":{"type":"object"}}]"#;
    let mut peer = Peer::spawn(
        d,
        "lazy-keep-alive",
        &[
            ("FAKE_TOOLS", changed),
            ("FAKE_LIST_CHANGED_AFTER", "change"),
            ("FAKE_CHANGED_TOOLS", "[]"),
        ],
    );
    peer.init().await;
    peer.call(3, "echo").await;
    let result = peer.request(json!(4), "tools/list", json!({})).await;
    assert_eq!(result["result"]["tools"][0]["name"], "new");
    eventually(|| {
        fs::read_dir(d.join("mcp-cache/v1"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
            .any(|e| fs::read_to_string(e.path()).unwrap().contains("new"))
    })
    .await;
    if !peer
        .seen
        .iter()
        .any(|v| v["method"] == "notifications/tools/list_changed")
    {
        loop {
            if peer.recv().await["method"] == "notifications/tools/list_changed" {
                break;
            }
        }
    }
    peer.call(5, "change").await;
    assert_eq!(
        peer.request(json!(6), "tools/list", json!({})).await["result"]["tools"],
        json!([])
    );
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn crash_errors_backoff_then_retries() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_CRASH_ON", "crash")]);
    peer.init().await;
    assert_eq!(peer.call(3, "crash").await["result"]["isError"], true);
    let fast = peer.call(4, "echo").await;
    assert!(
        fast["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("failed 0s ago")
    );
    let error = peer
        .request(json!(5), "resources/read", json!({"uri":"fake://test"}))
        .await;
    assert_eq!(error["error"]["code"], -32000);
    assert_eq!(pids(&d.join("spawn")).len(), 2);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(peer.call(6, "echo").await["result"]["isError"], false);
    assert_eq!(pids(&d.join("spawn")).len(), 3);
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn subscriptions_and_pending_calls_hold_idle_and_logging_replays() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_CALL_DELAY_MS", "350")]);
    peer.init().await;
    peer.request(json!(2), "logging/setLevel", json!({"level":"debug"}))
        .await;
    peer.send(json!({"jsonrpc":"2.0","method":"notifications/roots/list_changed"}))
        .await;
    peer.send(zeron_mcp_proxy::rpc::request(
        3,
        "tools/call",
        json!({"name":"echo"}),
    ))
    .await;
    eventually(|| pids(&d.join("spawn")).len() == 2).await;
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert!(alive(pids(&d.join("spawn"))[1]));
    peer.response(json!(3)).await;
    peer.request(
        json!(4),
        "resources/subscribe",
        json!({"uri":"fake://test"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(alive(pids(&d.join("spawn"))[1]));
    let requests: Vec<Value> = fs::read_to_string(d.join("requests"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let last_init = requests
        .iter()
        .rposition(|v| v["method"] == "initialize")
        .unwrap();
    let methods: Vec<_> = requests[last_init..]
        .iter()
        .filter_map(|v| v["method"].as_str())
        .collect();
    assert_eq!(
        &methods[..3],
        &[
            "initialize",
            "notifications/initialized",
            "logging/setLevel"
        ]
    );
    assert!(methods.contains(&"notifications/roots/list_changed"));
    peer.request(
        json!(5),
        "resources/unsubscribe",
        json!({"uri":"fake://test"}),
    )
    .await;
    eventually(|| !alive(pids(&d.join("spawn"))[1])).await;
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn eof_signal_and_sigkill_run_dir_cleanup_leave_no_orphans() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(
        d,
        "lazy-keep-alive",
        &[("FAKE_GRANDCHILD", "1"), ("FAKE_WATCHDOG", "1")],
    );
    peer.init().await;
    let ids: Vec<_> = pids(&d.join("spawn"))
        .into_iter()
        .chain(pids(&d.join("children")))
        .collect();
    peer.close().await;
    eventually(|| ids.iter().all(|p| !alive(*p))).await;
    let mut peer = Peer::spawn(
        d,
        "lazy-keep-alive",
        &[("FAKE_GRANDCHILD", "1"), ("FAKE_WATCHDOG", "1")],
    );
    peer.init().await;
    peer.call(2, "echo").await;
    unsafe {
        libc::kill(peer.child.id().unwrap() as i32, libc::SIGTERM);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(5), peer.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let mut peer = Peer::spawn(
        d,
        "lazy-keep-alive",
        &[("FAKE_GRANDCHILD", "1"), ("FAKE_WATCHDOG", "1")],
    );
    peer.init().await;
    peer.call(3, "echo").await;
    unsafe {
        libc::kill(peer.child.id().unwrap() as i32, libc::SIGKILL);
    }
    peer.child.wait().await.unwrap();
    assert!(fs::read_dir(d.join("run")).unwrap().next().is_some());
    zeron_mcp_proxy::cleanup_run_dir(&d.join("run"))
        .await
        .unwrap();
    eventually(|| {
        pids(&d.join("spawn"))
            .into_iter()
            .chain(pids(&d.join("children")))
            .all(|p| !alive(p))
    })
    .await;
    assert_eq!(fs::read_dir(d.join("run")).unwrap().count(), 0);
}
#[tokio::test(flavor = "current_thread")]
async fn failed_discovery_next_session_is_transparent() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_INIT_ERROR", "1")]);
    assert_eq!(peer.init().await["error"]["code"], -32000);
    peer.close().await;
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_INIT_ERROR", "1")]);
    assert_eq!(peer.init().await["error"]["message"], "fake init failure");
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn keep_alive_starts_before_initialize_and_selftest_is_in_process() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(d, "keep-alive", &[]);
    eventually(|| pids(&d.join("spawn")).len() == 1).await;
    peer.init().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(alive(pids(&d.join("spawn"))[0]));
    peer.close().await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_zeron-mcp-proxy"))
        .arg("--selftest")
        .env("ZERON_DATA_DIR", d)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n")
        .await
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let value: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(value["result"]["tools"].as_array().unwrap().len(), 1);
    drop(input);
    assert!(child.wait().await.unwrap().success());
    assert_eq!(pids(&d.join("spawn")).len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn npx_resolves_without_wrapper_composes_path_and_has_an_escape_hatch() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp();
    let d = dir.path();
    let npm = d.join("npm");
    let install = npm.join("_npx/direct");
    let pkg = install.join("node_modules/demo");
    let bins = install.join("node_modules/.bin");
    fs::create_dir_all(&pkg).unwrap();
    fs::create_dir_all(&bins).unwrap();
    fs::write(
        install.join("package.json"),
        r#"{"dependencies":{"demo":"1.2.3"}}"#,
    )
    .unwrap();
    fs::write(
        pkg.join("package.json"),
        r#"{"version":"1.2.3","bin":"cli.js"}"#,
    )
    .unwrap();
    fs::write(
        pkg.join("cli.js"),
        "// fixture: node is our native fake peer\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_fake-mcp"), bins.join("node")).unwrap();
    std::os::unix::fs::symlink(pkg.join("cli.js"), bins.join("demo")).unwrap();
    let wrapper = d.join("npx");
    fs::write(
        &wrapper,
        "#!/bin/sh\nprintf 'wrapper\\n' >> \"$WRAPPER_MARK\"\nexec \"$FAKE_BINARY\"\n",
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let dump = d.join("dump");
    let mark = d.join("wrapper-mark");
    let env = [
        ("ZERON_NPM_CACHE", npm.to_str().unwrap()),
        ("FAKE_DUMP_ENV", dump.to_str().unwrap()),
        ("FAKE_BINARY", env!("CARGO_BIN_EXE_fake-mcp")),
        ("WRAPPER_MARK", mark.to_str().unwrap()),
    ];
    let mut peer = Peer::spawn_command(
        d,
        "lazy-keep-alive",
        &env,
        wrapper.to_str().unwrap(),
        &["--yes", "demo@1.2.3", "--token=secret-value"],
        &[],
    );
    assert!(peer.init().await.get("result").is_some());
    assert!(!mark.exists());
    let dump_env: Value = serde_json::from_slice(&fs::read(&dump).unwrap()).unwrap();
    assert_eq!(
        std::env::split_paths(std::ffi::OsStr::new(dump_env["PATH"].as_str().unwrap())).next(),
        Some(bins)
    );
    assert!(
        !fs::read_to_string(d.join("mcp-cache/npx.json"))
            .unwrap()
            .contains("secret-value")
    );
    peer.close().await;
    let mut peer = Peer::spawn_command(
        d,
        "keep-alive",
        &env,
        wrapper.to_str().unwrap(),
        &["--yes", "demo@1.2.3"],
        &["--no-npx-resolve"],
    );
    assert!(peer.init().await.get("result").is_some());
    assert!(mark.exists());
    peer.close().await;
    fs::remove_file(&mark).unwrap();
    let mut peer = Peer::spawn_command(
        d,
        "keep-alive",
        &env,
        wrapper.to_str().unwrap(),
        &["--unsupported", "demo"],
        &[],
    );
    assert!(peer.init().await.get("result").is_some());
    assert!(mark.exists());
    peer.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn startup_timeout_and_first_call_queued_during_discovery() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(
        d,
        "lazy",
        &[
            ("FAKE_INIT_DELAY_MS", "100"),
            ("ZERON_MCP_PROXY_START_TIMEOUT_MS", "30"),
        ],
    );
    assert_eq!(peer.init().await["error"]["code"], -32000);
    peer.close().await;
    // A different identity (different client major) bypasses the failure marker.
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_INIT_DELAY_MS", "100")]);
    peer.send(zeron_mcp_proxy::rpc::request(1,"initialize",json!({"protocolVersion":"v","clientInfo":{"name":"queued","version":"2"},"capabilities":{}}))).await;
    peer.send(zeron_mcp_proxy::rpc::request(
        2,
        "tools/call",
        json!({"name":"echo"}),
    ))
    .await;
    assert!(peer.response(json!(1)).await.get("result").is_some());
    assert_eq!(peer.response(json!(2)).await["result"]["isError"], false);
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn lone_server_request_blocks_idle_and_inflight_cancel_is_remapped() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(
        d,
        "lazy",
        &[
            ("FAKE_DETACHED_REQUEST_ON", "detached"),
            ("FAKE_CALL_DELAY_MS", "350"),
        ],
    );
    peer.init().await;
    peer.send(zeron_mcp_proxy::rpc::request(
        10,
        "tools/call",
        json!({"name":"detached"}),
    ))
    .await;
    let sampling = loop {
        let v = peer.recv().await;
        if v["method"] == "sampling/createMessage" {
            break v;
        }
    };
    peer.response(json!(10)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(alive(*pids(&d.join("spawn")).last().unwrap()));
    peer.send(json!({"jsonrpc":"2.0","id":sampling["id"],"result":{"content":{"type":"text","text":"ok"}}})).await;
    peer.send(zeron_mcp_proxy::rpc::request(
        "zp:cancel",
        "tools/call",
        json!({"name":"echo"}),
    ))
    .await;
    eventually(|| {
        fs::read_to_string(d.join("requests"))
            .unwrap()
            .contains("\"name\":\"echo\"\n")
            || fs::read_to_string(d.join("requests"))
                .unwrap()
                .contains("\"name\":\"echo\"}")
    })
    .await;
    peer.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"zp:cancel"}})).await;
    peer.response(json!("zp:cancel")).await;
    let requests: Vec<Value> = fs::read_to_string(d.join("requests"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let cancellation = requests
        .iter()
        .find(|v| v["method"] == "notifications/cancelled")
        .unwrap();
    assert!(
        cancellation["params"]["requestId"]
            .as_str()
            .unwrap()
            .starts_with("zc:")
    );
    eventually(|| !alive(*pids(&d.join("spawn")).last().unwrap())).await;
    peer.close().await;
}

fn cache_files(d: &Path, extension: &str) -> Vec<PathBuf> {
    fs::read_dir(d.join("mcp-cache/v1"))
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == extension))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "current_thread")]
async fn stray_server_stdout_lines_are_skipped_like_a_direct_client_would() {
    for mode in ["lazy", "lazy-keep-alive", "keep-alive"] {
        let dir = temp();
        let mut peer = Peer::spawn(dir.path(), mode, &[("FAKE_NOISE", "1")]);
        assert!(peer.init().await.get("result").is_some(), "{mode}");
        assert_eq!(
            peer.call(2, "echo").await["result"]["content"][0]["text"],
            "echo",
            "{mode}"
        );
        peer.close().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn slow_client_survives_a_notification_flood() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(
        d,
        "lazy",
        &[("FAKE_FLOOD_ON", "flood"), ("FAKE_FLOOD", "6000")],
    );
    peer.init().await;
    peer.send(zeron_mcp_proxy::rpc::request(
        2,
        "tools/call",
        json!({"name":"flood"}),
    ))
    .await;
    // The pipe fills while nobody reads; the proxy must buffer, not give up.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let response = peer.response(json!(2)).await;
    assert_eq!(response["result"]["isError"], false);
    assert!(
        peer.seen
            .iter()
            .filter(|v| v["method"] == "notifications/message")
            .count()
            >= 6000
    );
    peer.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn server_negotiating_an_older_protocol_still_hits_the_cache() {
    let dir = temp();
    let d = dir.path();
    for session in 0..2 {
        let mut peer = Peer::spawn(d, "lazy", &[("FAKE_PROTOCOL", "2024-11-05")]);
        let init = peer.init().await;
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        peer.close().await;
        eventually(|| {
            let spawned = pids(&d.join("spawn"));
            spawned.len() == 1 && !alive(spawned[0])
        })
        .await;
        assert_eq!(pids(&d.join("spawn")).len(), 1, "session {session}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resources_without_template_listing_still_cache() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_NO_TEMPLATES", "1")]);
    assert!(peer.init().await.get("result").is_some());
    peer.close().await;
    eventually(|| cache_files(d, "json").len() == 1).await;
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_NO_TEMPLATES", "1")]);
    peer.init().await;
    peer.close().await;
    assert_eq!(pids(&d.join("spawn")).len(), 1);
}
