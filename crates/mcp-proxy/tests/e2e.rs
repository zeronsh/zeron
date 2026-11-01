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
        Self::spawn_command_at(dir, mode, env, command, args, proxy_args, None)
    }
    fn spawn_command_at(
        dir: &Path,
        mode: &str,
        env: &[(&str, &str)],
        command: &str,
        args: &[&str],
        proxy_args: &[&str],
        cwd: Option<&Path>,
    ) -> Self {
        let stderr = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("stderr.log"))
            .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zeron-mcp-proxy"));
        cmd.args(["--server", "fake", "--mode", mode, "--run-dir"])
            .arg(dir.join("run"))
            .arg("--data-dir")
            .arg(dir)
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
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        Self::from_command(cmd)
    }
    fn from_command(mut cmd: Command) -> Self {
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
        let response = self
            .request(json!("init"), "initialize", initialize_params())
            .await;
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
fn initialize_params() -> Value {
    json!({"protocolVersion":"2025-03-26","clientInfo":{"name":"e2e","version":"1.2.3"},"capabilities":{"sampling":{},"roots":{"listChanged":true}}})
}
/// Process-group markers left in a run directory. Run-local evidence the
/// engine reads (capabilities, channel-live) is not a marker and outlives the
/// proxy until the engine removes the directory.
fn markers(run: &Path) -> usize {
    fs::read_dir(run)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .count()
}
async fn seed(dir: &Path) {
    let mut peer = Peer::spawn(dir, "lazy", &[]);
    assert!(peer.init().await.get("result").is_some());
    eventually(|| pids(&dir.join("spawn")).len() == 1 && !alive(pids(&dir.join("spawn"))[0])).await;
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn explicit_data_dir_contains_writes_with_sanitized_or_conflicting_env() {
    let root = temp();
    let data = root.path().join("data");
    let home = root.path().join("home");
    let unused = root.path().join("env-data");
    fs::create_dir(&home).unwrap();
    let legacy = home.join(".comet-native");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("sentinel"), "do not migrate").unwrap();
    for env_dir in [None, Some(&unused)] {
        if data.exists() {
            fs::remove_dir_all(&data).unwrap();
        }
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zeron-mcp-proxy"));
        cmd.arg("--data-dir")
            .arg(&data)
            .args(["--server", "fake", "--run-dir"])
            .arg(data.join("run/mcp/test"))
            .arg("--")
            .arg(env!("CARGO_BIN_EXE_fake-mcp"))
            .env("HOME", &home)
            .env_remove("ZERON_DATA_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(env_dir) = env_dir {
            cmd.env("ZERON_DATA_DIR", env_dir);
        }
        let mut peer = Peer::from_command(cmd);
        assert!(peer.init().await.get("result").is_some());
        peer.close().await;
        assert!(data.join("mcp-cache/v1").is_dir());
        assert!(data.join("mcp-cache/class-v1").is_dir());
        assert!(data.join("run/mcp/test/capabilities").is_dir());
        assert_eq!(
            zeron_mcp_proxy::discovery::run_discovery(&data.join("run/mcp/test"), "fake").unwrap(),
            Some(zeron_mcp_proxy::discovery::Discovery::NonChannel)
        );
        assert!(data.join("logs/mcp-proxy.jsonl").is_file());
        assert_eq!(fs::read_dir(&home).unwrap().count(), 1);
        assert_eq!(
            fs::read_to_string(legacy.join("sentinel")).unwrap(),
            "do not migrate"
        );
        assert!(!home.join(".zeron").exists());
        assert!(!unused.exists());
    }
    let mut names: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["data", "home"]);
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
    assert_eq!(markers(&d.join("run")), 0);
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
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_CALL_DELAY_MS", "2000")]);
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
    // Attach the subscription while the delayed call still holds the runtime.
    // Waiting for the call and then scheduling a subscribe can cross the
    // ordinary idle deadline on a heavily loaded host.
    peer.send(zeron_mcp_proxy::rpc::request(
        json!(4),
        "resources/subscribe",
        json!({"uri":"fake://test"}),
    ))
    .await;
    let mut completed = std::collections::BTreeSet::new();
    while completed.len() < 2 {
        let response = peer.recv().await;
        if response["id"] == 3 || response["id"] == 4 {
            assert!(response.get("result").is_some(), "{response}");
            completed.insert(response["id"].as_i64().unwrap());
        }
    }
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
    assert_eq!(markers(&d.join("run")), 0);
}
#[tokio::test(flavor = "current_thread")]
async fn failed_discovery_next_session_is_transparent() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_INIT_ERROR", "1")]);
    assert_eq!(peer.init().await["error"]["message"], "fake init failure");
    assert_eq!(
        pids(&d.join("spawn")).len(),
        2,
        "failed discovery retries in eager forwarding"
    );
    peer.close().await;
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_INIT_ERROR", "1")]);
    eventually(|| pids(&d.join("spawn")).len() == 3).await;
    assert_eq!(peer.init().await["error"]["message"], "fake init failure");
    peer.close().await;
}
#[tokio::test(flavor = "current_thread")]
async fn expired_failure_retries_discovery_and_returns_to_lazy_cache() {
    let dir = temp();
    let d = dir.path();
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_INIT_ERROR", "1")]);
    assert_eq!(peer.init().await["error"]["message"], "fake init failure");
    peer.close().await;
    // Seed both stores, including the H1 marker used by older proxy writers.
    let command = fs::canonicalize(env!("CARGO_BIN_EXE_fake-mcp")).unwrap();
    let config = zeron_mcp_proxy::Config::parse(
        ["--server", "fake", "--", env!("CARGO_BIN_EXE_fake-mcp")].map(str::to_owned),
        d.into(),
    )
    .unwrap();
    let key = zeron_mcp_proxy::cache::key(
        &config,
        &initialize_params(),
        &command,
        &command,
        Value::Null,
    );
    let mut entry = zeron_mcp_proxy::cache::Entry::new(key.clone(), "fake".into(), Value::Null);
    entry.discovery_failed = true;
    zeron_mcp_proxy::cache::save(d, &entry).unwrap();
    let mut expired = 0;
    for store in ["class-v1", "v1"] {
        for entry in fs::read_dir(d.join("mcp-cache").join(store)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                record["cached_at"] = json!(0);
                fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
                expired += 1;
            }
        }
    }
    assert!(expired >= 2);
    assert!(zeron_mcp_proxy::cache::read(d, &key).is_none());
    let mut peer = Peer::spawn(d, "lazy", &[]);
    assert!(peer.init().await.get("result").is_some());
    eventually(|| pids(&d.join("spawn")).iter().all(|pid| !alive(*pid))).await;
    assert!(
        !zeron_mcp_proxy::cache::read(d, &key)
            .unwrap()
            .discovery_failed
    );
    let starts = pids(&d.join("spawn")).len();
    peer.close().await;
    let mut peer = Peer::spawn(d, "lazy", &[]);
    assert!(peer.init().await.get("result").is_some());
    assert_eq!(
        pids(&d.join("spawn")).len(),
        starts,
        "warm cache must stay lazy after recovery"
    );
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
        .arg("--data-dir")
        .arg(d)
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
    let install = npm
        .join("_npx")
        .join(zeron_mcp_proxy::npx::install_hash("demo@1.2.3"));
    let pkg = install.join("node_modules/demo");
    let bins = install.join("node_modules/.bin");
    fs::create_dir_all(&pkg).unwrap();
    fs::create_dir_all(&bins).unwrap();
    fs::write(
        install.join("package.json"),
        r#"{"dependencies":{"demo":"^1.2.3"},"_npx":{"packages":["demo@1.2.3"]}}"#,
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
async fn discovery_timeout_falls_back_and_first_call_queues_during_fresh_discovery() {
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
    assert!(
        peer.init().await.get("result").is_some(),
        "eager fallback retains the real server's timing"
    );
    assert_eq!(pids(&d.join("spawn")).len(), 2);
    peer.close().await;
    // Change the server identity, not just its client: classification is shared
    // across clients, while their catalogs are partitioned independently.
    let mut peer = Peer::spawn_command(
        d,
        "lazy",
        &[("FAKE_INIT_DELAY_MS", "100")],
        env!("CARGO_BIN_EXE_fake-mcp"),
        &["new-identity"],
        &[],
    );
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

#[tokio::test]
async fn channel_capability_never_idles_and_cached_channel_starts_eagerly() {
    let dir = temp();
    let mut first = Peer::spawn(dir.path(), "lazy", &[("FAKE_CHANNEL", "1")]);
    first.init().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(pids(&dir.path().join("spawn")).len(), 1);
    assert!(pids(&dir.path().join("spawn")).into_iter().all(alive));
    assert_eq!(
        first.call(1, "echo").await["result"]["content"][0]["text"],
        "echo"
    );
    assert!(
        first
            .seen
            .iter()
            .any(|v| v["method"] == "notifications/claude/channel")
    );
    first.close().await;
    let mut second = Peer::spawn(dir.path(), "lazy", &[("FAKE_CHANNEL", "1")]);
    // Shared core must exec the known channel before any client initialize.
    eventually(|| pids(&dir.path().join("spawn")).len() == 2).await;
    #[cfg(target_os = "linux")]
    assert_eq!(
        fs::read_link(format!("/proc/{}/exe", second.child.id().unwrap())).unwrap(),
        Path::new(env!("CARGO_BIN_EXE_fake-mcp"))
            .canonicalize()
            .unwrap()
    );
    second.init().await;
    assert!(alive(*pids(&dir.path().join("spawn")).last().unwrap()));
    assert_eq!(
        second.call(2, "echo").await["result"]["content"][0]["text"],
        "echo"
    );
    assert!(
        second
            .seen
            .iter()
            .any(|v| v["method"] == "notifications/claude/channel")
    );
    second.close().await;
}

#[tokio::test]
async fn inherited_runtime_cwd_partitions_catalogs_without_explicit_cwd() {
    let dir = temp();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    for (cwd, count) in [(&a, 1), (&b, 2), (&a, 2)] {
        let mut peer = Peer::spawn_command_at(
            dir.path(),
            "lazy",
            &[],
            env!("CARGO_BIN_EXE_fake-mcp"),
            &[],
            &[],
            Some(cwd),
        );
        assert!(peer.init().await.get("result").is_some());
        eventually(|| {
            pids(&dir.path().join("spawn")).len() == count
                && pids(&dir.path().join("spawn"))
                    .into_iter()
                    .all(|p| !alive(p))
        })
        .await;
        peer.close().await;
    }
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

#[cfg(target_os = "linux")]
fn children_of(parent: u32) -> Vec<(u32, char)> {
    fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| {
            let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
            (fields.get(1)?.parse::<u32>().ok()? == parent)
                .then(|| (pid, fields[0].chars().next().unwrap()))
        })
        .collect()
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

#[tokio::test]
async fn inherited_runtime_cwd_also_partitions_channel_classification() {
    let dir = temp();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let spawn = |cwd: &Path| {
        Peer::spawn_command_at(
            dir.path(),
            "lazy",
            &[("FAKE_CHANNEL", "1")],
            env!("CARGO_BIN_EXE_fake-mcp"),
            &[],
            &[],
            Some(cwd),
        )
    };
    let mut first = spawn(&a);
    first.init().await;
    first.close().await;
    let mut other = spawn(&b);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        pids(&dir.path().join("spawn")).len(),
        1,
        "another cwd needs its own discovery"
    );
    other.init().await;
    assert_eq!(pids(&dir.path().join("spawn")).len(), 2);
    other.close().await;
    let mut known = spawn(&a);
    eventually(|| pids(&dir.path().join("spawn")).len() == 3).await;
    known.init().await;
    known.close().await;
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
async fn channel_forwarding_skips_noise_and_survives_a_slow_client() {
    let dir = temp();
    let mut peer = Peer::spawn(
        dir.path(),
        "lazy",
        &[
            ("FAKE_CHANNEL", "1"),
            ("FAKE_NOISE", "1"),
            ("FAKE_FLOOD_ON", "flood"),
            ("FAKE_FLOOD", "6000"),
        ],
    );
    assert!(peer.init().await.get("result").is_some());
    peer.send(zeron_mcp_proxy::rpc::request(
        2,
        "tools/call",
        json!({"name":"flood"}),
    ))
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(pids(&dir.path().join("spawn")).into_iter().all(alive));
    assert_eq!(peer.response(json!(2)).await["result"]["isError"], false);
    assert!(
        peer.seen
            .iter()
            .filter(|v| v["method"] == "notifications/message")
            .count()
            >= 6000
    );
    assert!(
        peer.seen
            .iter()
            .any(|v| v["method"] == "notifications/claude/channel")
    );
    assert_eq!(pids(&dir.path().join("spawn")).len(), 1);
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

#[tokio::test(flavor = "current_thread")]
async fn paged_list_with_a_cursor_reaches_the_live_server() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(d, "lazy", &[("FAKE_PAGED", "1")]);
    peer.init().await;
    let page = peer
        .request(json!(2), "tools/list", json!({"cursor":"page2"}))
        .await;
    assert_eq!(page["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(pids(&d.join("spawn")).len(), 2);
    peer.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn first_call_during_discovery_does_not_force_a_restart_for_the_second() {
    let dir = temp();
    let d = dir.path();
    // Only discovery may retire the server here; ordinary idle expiry is independent.
    let mut peer = Peer::spawn(d, "lazy", &[("ZERON_MCP_PROXY_IDLE_MS", "0")]);
    peer.send(zeron_mcp_proxy::rpc::request(
        1,
        "initialize",
        json!({"protocolVersion":"2025-03-26","clientInfo":{"name":"e2e","version":"1"},"capabilities":{}}),
    ))
    .await;
    peer.send(zeron_mcp_proxy::rpc::request(
        2,
        "tools/call",
        json!({"name":"echo"}),
    ))
    .await;
    peer.response(json!(2)).await;
    peer.call(3, "echo").await;
    assert_eq!(pids(&d.join("spawn")).len(), 1);
    peer.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_sessions_leave_one_private_cache_without_secrets() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp();
    let d = dir.path();
    let secret = "super-secret-token-value";
    let mut peers = Vec::new();
    for _ in 0..6 {
        peers.push(Peer::spawn_command(
            d,
            "lazy",
            &[("TOKEN", secret)],
            env!("CARGO_BIN_EXE_fake-mcp"),
            &[],
            &["--identity-env", "TOKEN"],
        ));
    }
    for peer in &mut peers {
        peer.send(zeron_mcp_proxy::rpc::request(
            "init",
            "initialize",
            json!({"protocolVersion":"2025-03-26","clientInfo":{"name":"e2e","version":"1"},"capabilities":{}}),
        ))
        .await;
    }
    for peer in &mut peers {
        assert!(peer.response(json!("init")).await.get("result").is_some());
    }
    for peer in peers {
        peer.close().await;
    }
    let files = cache_files(d, "json");
    assert_eq!(files.len(), 1);
    assert!(cache_files(d, "tmp").is_empty());
    serde_json::from_slice::<Value>(&fs::read(&files[0]).unwrap()).unwrap();
    for entry in ["mcp-cache/v1", "logs"] {
        for file in fs::read_dir(d.join(entry)).unwrap().flatten() {
            let mode = file.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{:?}", file.path());
            assert!(
                !fs::read_to_string(file.path()).unwrap().contains(secret),
                "{:?}",
                file.path()
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn calls_straddling_the_idle_deadline_are_never_lost() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let mut peer = Peer::spawn(
        d,
        "lazy",
        &[
            ("ZERON_MCP_PROXY_IDLE_MS", "40"),
            ("ZERON_MCP_PROXY_CHECK_MS", "5"),
        ],
    );
    peer.init().await;
    let mut seed = 12345u64;
    for id in 1..=80 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        tokio::time::sleep(Duration::from_millis(25 + (seed >> 33) % 40)).await;
        let response = peer.call(id, "echo").await;
        assert_eq!(
            response["result"]["content"][0]["text"], "echo",
            "call {id}"
        );
    }
    assert!(
        pids(&d.join("spawn")).len() > 5,
        "the stop path was exercised"
    );
    peer.close().await;
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn idle_cycles_leave_no_zombies_or_adopted_children() {
    let dir = temp();
    let d = dir.path();
    let env = [("FAKE_GRANDCHILD", "1"), ("FAKE_WATCHDOG", "1")];
    let mut peer = Peer::spawn(d, "lazy", &env);
    peer.init().await;
    let proxy = peer.child.id().unwrap();
    for id in 1..=3 {
        peer.call(id, "echo").await;
        eventually(|| {
            let spawned = pids(&d.join("spawn"));
            spawned.len() == id as usize + 1 && !alive(*spawned.last().unwrap())
        })
        .await;
        // Reaping trails the server's exit by a few milliseconds; a persistent zombie fails.
        eventually(|| children_of(proxy).is_empty()).await;
    }
    assert!(
        pids(&d.join("children")).iter().all(|p| !alive(*p)),
        "grandchildren and watchdogs are gone"
    );
    peer.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn startup_failures_do_not_echo_credentials_to_the_client_or_the_log() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let secret = "hunter2-credential-value";
    let mut peer = Peer::spawn(
        d,
        "lazy",
        &[
            ("FAKE_CRASH_ON", "crash"),
            ("FAKE_INIT_DELAY_MS", "100"),
            ("API_TOKEN", secret),
            ("FAKE_STDERR_TEXT", &format!("auth failed for {secret}")),
        ],
    );
    assert!(peer.init().await.get("result").is_some());
    // A cache-hit start fails after initialization, so this exercises the
    // proxy's diagnostic path rather than the transparent legacy fallback.
    let failure = peer.call(2, "crash").await;
    assert_eq!(failure["result"]["isError"], true);
    let message = failure["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains("auth failed for ***"), "{message}");
    assert!(!message.contains(secret));
    peer.close().await;
    assert!(
        !fs::read_to_string(d.join("logs/mcp-proxy.jsonl"))
            .unwrap()
            .contains(secret)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_old_failed_discovery_marker_is_retried() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    let file = &cache_files(d, "json")[0];
    let mut entry: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    // A catalog failure written by an older proxy must expire too. Current
    // pre-initialize failures use class-v1, covered by the expiry e2e above.
    entry["discovery_failed"] = json!(true);
    entry["cached_at"] = json!(1);
    fs::write(file, entry.to_string()).unwrap();
    let mut peer = Peer::spawn(d, "lazy", &[]);
    assert!(peer.init().await.get("result").is_some());
    eventually(|| {
        serde_json::from_slice::<Value>(&fs::read(file).unwrap())
            .is_ok_and(|e| e["discovery_failed"] == false && e["tools"][0]["name"] == "echo")
    })
    .await;
    peer.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn directories_are_private_and_the_lifecycle_log_is_bounded() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp();
    let d = dir.path();
    fs::create_dir_all(d.join("logs")).unwrap();
    let log = d.join("logs/mcp-proxy.jsonl");
    fs::File::create(&log).unwrap().set_len(5 << 20).unwrap();
    let mut peer = Peer::spawn(d, "lazy-keep-alive", &[]);
    peer.init().await;
    peer.call(2, "echo").await;
    for private in [
        "mcp-cache",
        "mcp-cache/v1",
        "mcp-cache/class-v1",
        "run",
        "run/capabilities",
    ] {
        let mode = fs::metadata(d.join(private)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{private}");
    }
    peer.close().await;
    assert!(d.join("logs/mcp-proxy.jsonl.1").exists());
    assert!(fs::metadata(&log).unwrap().len() < 1 << 20);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn sweep_never_kills_a_group_whose_pid_was_reused() {
    use std::os::unix::process::CommandExt;
    let dir = temp();
    let run = dir.path().join("run");
    fs::create_dir_all(&run).unwrap();
    let mut bystander = std::process::Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = bystander.id();
    let marker = run.join(pgid.to_string());
    // A marker from an earlier life of this pid: same number, different start time.
    fs::write(&marker, json!({"pgid":pgid,"start":"1"}).to_string()).unwrap();
    zeron_mcp_proxy::cleanup_run_dir(&run).await.unwrap();
    assert!(alive(pgid), "an unrelated process group was killed");
    assert!(!marker.exists(), "the stale marker is dropped");
    // The same marker with the true start time is ours and is swept.
    let stat = fs::read_to_string(format!("/proc/{pgid}/stat")).unwrap();
    let start = stat.rsplit_once(')').unwrap().1.split_whitespace().nth(19);
    fs::write(&marker, json!({"pgid":pgid,"start":start}).to_string()).unwrap();
    zeron_mcp_proxy::cleanup_run_dir(&run).await.unwrap();
    assert!(!bystander.wait().unwrap().success());
}

#[tokio::test(flavor = "current_thread")]
async fn first_call_reaches_the_server_before_the_metadata_refresh() {
    let dir = temp();
    let d = dir.path();
    seed(d).await;
    fs::remove_file(d.join("requests")).unwrap();
    let mut peer = Peer::spawn(d, "lazy", &[]);
    peer.init().await;
    peer.call(2, "echo").await;
    eventually(|| {
        fs::read_to_string(d.join("requests")).is_ok_and(|log| log.contains("\"tools/list\""))
    })
    .await;
    let methods: Vec<_> = fs::read_to_string(d.join("requests"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["method"].as_str().map(str::to_owned))
        .collect();
    let at = |name: &str| methods.iter().position(|m| m == name).unwrap();
    assert!(at("tools/call") < at("tools/list"), "{methods:?}");
    peer.close().await;
}
