//! The host around the VM: lifecycle, retries, ops' permissions.

use super::*;
use serde_json::json;

#[derive(Default)]
struct Recorder {
    emitted: Mutex<Vec<(String, String, Json)>>,
    notified: Mutex<Vec<String>>,
}

impl Hooks for Recorder {
    fn emit(&self, plugin: &str, event: &str, data: Json) {
        lock(&self.emitted).push((plugin.into(), event.into(), data));
    }
    fn notify(&self, _plugin: &str, title: &str, _body: &str, _session: Option<&str>) -> String {
        lock(&self.notified).push(title.into());
        "posted".into()
    }
    fn device(&self) -> Json {
        json!({ "id": "dev-1", "name": "Laptop" })
    }
}

struct Rig {
    dir: tempfile::TempDir,
    services: Services,
    hooks: Arc<Recorder>,
}

fn rig(config: Config) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let services = Services::new(&dir.path().join(".storage"), config);
    let hooks = Arc::new(Recorder::default());
    services.set_hooks(hooks.clone());
    Rig { dir, services, hooks }
}

fn quick() -> Config {
    Config {
        idle: Duration::from_millis(300),
        tick: Duration::from_millis(50),
        ..Config::default()
    }
}

impl Rig {
    /// A version of plugin `svc` (its own snapshot folder), allowed `allowed`.
    fn plan(&self, version: &str, source: &str, allowed: &[&str]) -> Plan {
        let dir = self.dir.path().join(format!("snap-{version}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("service.luau"), source).unwrap();
        std::fs::write(dir.join("zeron.luau"), crate::plugins::scaffold::SDK).unwrap();
        Plan {
            plugin: "svc".into(),
            name: "Svc".into(),
            version: version.into(),
            hash: format!("hash-{version}"),
            dir,
            entry: "service.luau".into(),
            allowed: allowed.iter().map(|s| s.to_string()).collect(),
            commands: vec!["echo".into()],
            hosts: Vec::new(),
        }
    }

    async fn ask(&self, plan: &Plan, method: &str, args: Json) -> Result<Json, Refusal> {
        self.services
            .call(plan, Kind::Method, method, args, json!({}), Duration::from_secs(10))
            .await
    }

    fn starts(&self) -> u32 {
        lock(&self.services.slot("svc").state).starts
    }

    fn running(&self) -> bool {
        self.services.status("svc")["running"] == true
    }
}

async fn until(what: &str, check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never: {what}");
}

const V1: &str = r#"
local zeron = require("./zeron")("svc")
zeron.serve("version", function() return "v1" end, { safe = true })
zeron.serve("slow", function()
	zeron.sleep(0.4):await()
	return "slow v1"
end)
zeron.serve("echo", function(args)
	return zeron.run("echo", { args.word }):await().stdout
end)
zeron.serve("ls", function()
	return zeron.run("ls", { "/" }):await()
end)
zeron.serve("ping", function(args)
	zeron.emit("pinged", { n = args.n })
	return zeron.notify({ title = "Pinged" }):await()
end)
"#;

const V2: &str = r#"
local zeron = require("./zeron")("svc")
zeron.serve("version", function() return "v2" end, { safe = true })
"#;

#[tokio::test]
async fn it_starts_on_the_first_call_and_stops_when_idle() {
    let r = rig(quick());
    let plan = r.plan("1.0.0", V1, &["service"]);
    assert!(!r.running());
    assert_eq!(r.ask(&plan, "version", json!({})).await.unwrap(), json!("v1"));
    assert!(r.running());
    assert_eq!(r.ask(&plan, "version", json!({})).await.unwrap(), json!("v1"));
    assert_eq!(r.starts(), 1, "one VM serves both calls");
    until("idle stop", || !r.running()).await;
    // And the next call starts it again.
    assert_eq!(r.ask(&plan, "version", json!({})).await.unwrap(), json!("v1"));
    assert_eq!(r.starts(), 2);
}

#[tokio::test]
async fn a_call_in_flight_keeps_it_from_stopping() {
    let r = rig(Config {
        idle: Duration::from_millis(50),
        tick: Duration::from_millis(20),
        ..Config::default()
    });
    let plan = r.plan("1.0.0", V1, &["service"]);
    assert_eq!(r.ask(&plan, "slow", json!({})).await.unwrap(), json!("slow v1"));
    assert_eq!(r.starts(), 1);
}

#[tokio::test]
async fn an_update_swaps_after_the_new_version_loads() {
    let r = rig(Config::default());
    let v1 = r.plan("1.0.0", V1, &["service"]);
    r.services.sync(vec![v1.clone()]);
    // (A first sight loads it once; with no timers, it stops again.)
    until("discovered", || r.starts() == 1 && !r.running()).await;
    assert_eq!(r.ask(&v1, "version", json!({})).await.unwrap(), json!("v1"));
    // A slow call on v1, then the update lands mid-call.
    let slow = {
        let (services, v1) = (r.services.clone(), v1.clone());
        tokio::spawn(async move {
            services
                .call(&v1, Kind::Method, "slow", json!({}), json!({}), Duration::from_secs(10))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let v2 = r.plan("2.0.0", V2, &["service"]);
    r.services.sync(vec![v2.clone()]);
    until("v2 loaded", || r.services.status("svc")["version"] == "2.0.0").await;
    assert_eq!(r.ask(&v2, "version", json!({})).await.unwrap(), json!("v2"));
    // v1 finished the call it had.
    assert_eq!(slow.await.unwrap().unwrap(), json!("slow v1"));
}

#[tokio::test]
async fn a_broken_update_fails_calls_with_why() {
    let r = rig(Config::default());
    let v1 = r.plan("1.0.0", V1, &["service"]);
    assert_eq!(r.ask(&v1, "version", json!({})).await.unwrap(), json!("v1"));
    let v2 = r.plan("2.0.0", "local x = = 1", &["service"]);
    r.services.sync(vec![v2.clone()]);
    let refused = loop {
        match r.ask(&v2, "version", json!({})).await {
            Err(refusal) => break refusal,
            Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    assert_eq!(refused.reason, reason::NOT_LOADED, "{refused:?}");
    assert!(refused.message.contains("service.luau"), "{refused:?}");
    assert!(!r.running(), "the old version doesn't go on serving");
    assert!(r.services.status("svc")["error"].is_string());
}

#[tokio::test]
async fn turned_off_it_stops() {
    let r = rig(Config::default());
    let plan = r.plan("1.0.0", V1, &["service"]);
    r.services.sync(vec![plan.clone()]);
    assert_eq!(r.ask(&plan, "version", json!({})).await.unwrap(), json!("v1"));
    r.services.sync(vec![]);
    assert!(r.services.status("svc").is_null());
}

const FLAKY: &str = r#"
local zeron = require("./zeron")("svc")
local function crash_once(name)
	local seen = zeron.store(name, false)
	if not seen:get() then
		seen:set(true)
		local t = {}
		for i = 1, 1e9 do t[i] = string.rep("x", 1024) .. i end
	end
	return name .. " ran"
end
zeron.serve("safe", function() return crash_once("safe") end, { safe = true })
zeron.serve("unsafe", function() return crash_once("unsafe") end)
zeron.serve("fails", function() error("nope", 0) end, { safe = true })
"#;

fn small() -> Config {
    Config {
        limits: Limits {
            memory: 8 << 20,
            slice: Duration::from_secs(20),
            ..Limits::default()
        },
        ..Config::default()
    }
}

#[tokio::test]
async fn a_safe_method_runs_again_when_the_service_stopped_mid_call() {
    let r = rig(small());
    let plan = r.plan("1.0.0", FLAKY, &["service"]);
    assert_eq!(r.ask(&plan, "safe", json!({})).await.unwrap(), json!("safe ran"));
    assert_eq!(r.starts(), 2, "a fresh VM ran it the second time");
}

#[tokio::test]
async fn an_unsafe_method_is_not_run_again() {
    let r = rig(small());
    let plan = r.plan("1.0.0", FLAKY, &["service"]);
    let refused = r.ask(&plan, "unsafe", json!({})).await.unwrap_err();
    assert_eq!(refused.reason, reason::INTERRUPTED, "{refused:?}");
    // The next call is the plugin's to make; it gets a fresh VM.
    assert_eq!(r.ask(&plan, "unsafe", json!({})).await.unwrap(), json!("unsafe ran"));
}

#[tokio::test]
async fn the_plugins_own_failures_are_never_retried() {
    let r = rig(Config::default());
    let plan = r.plan("1.0.0", FLAKY, &["service"]);
    let refused = r.ask(&plan, "fails", json!({})).await.unwrap_err();
    assert_eq!(refused, Refusal::new(reason::FAILED, "nope"));
    assert_eq!(r.starts(), 1);
    let refused = r.ask(&plan, "missing", json!({})).await.unwrap_err();
    assert_eq!(refused.reason, reason::UNKNOWN_METHOD);
}

#[tokio::test]
#[cfg(unix)]
async fn commands_are_enforced_inside_the_service() {
    let r = rig(Config::default());
    let allowed = r.plan("1.0.0", V1, &["service", "commands"]);
    assert_eq!(r.ask(&allowed, "echo", json!({ "word": "hi" })).await.unwrap(), json!("hi\n"));
    // Not named in its manifest's "commands".
    let refused = r.ask(&allowed, "ls", json!({})).await.unwrap_err();
    assert_eq!(refused.reason, reason::FAILED);
    assert!(refused.message.contains("doesn't run \"ls\""), "{refused:?}");
    // Named, but this device didn't allow "commands".
    let r = rig(Config::default());
    let plan = r.plan("1.0.0", V1, &["service"]);
    let refused = r.ask(&plan, "echo", json!({ "word": "hi" })).await.unwrap_err();
    assert!(refused.message.contains("wasn't allowed to run programs"), "{refused:?}");
}

#[tokio::test]
async fn emits_and_notifications_go_through_the_host() {
    let r = rig(Config::default());
    let plan = r.plan("1.0.0", V1, &["service", "notifications"]);
    assert_eq!(r.ask(&plan, "ping", json!({ "n": 3 })).await.unwrap(), json!("posted"));
    assert_eq!(
        lock(&r.hooks.emitted).as_slice(),
        &[("svc".to_string(), "pinged".to_string(), json!({ "n": 3 }))]
    );
    assert_eq!(lock(&r.hooks.notified).as_slice(), &["Svc: Pinged".to_string()]);
    // Without "notifications", refused.
    let r = rig(Config::default());
    let plan = r.plan("1.0.0", V1, &["service"]);
    let refused = r.ask(&plan, "ping", json!({ "n": 1 })).await.unwrap_err();
    assert!(refused.message.contains("notifications"), "{refused:?}");
}

const TICKS: &str = r#"
local zeron = require("./zeron")("svc")
zeron.every(30, function()
	local n = zeron.store("ticks", 0)
	n:set(n:get() + 1)
end)
zeron.serve("ticks", function() return zeron.store("ticks", 0):get() end)
"#;

#[tokio::test]
async fn a_timer_starts_a_stopped_service() {
    let r = rig(quick());
    let plan = r.plan("1.0.0", TICKS, &["service"]);
    r.services.sync(vec![plan.clone()]);
    until("discovered", || r.starts() == 1).await;
    until("idle", || !r.running()).await;
    // Make the timer due now (its period is 30 s at least).
    {
        let slot = r.services.slot("svc");
        let mut st = lock(&slot.state);
        assert_eq!(st.timers.len(), 1);
        st.timers[0].due = Instant::now();
    }
    until("timer ran", || r.starts() == 2).await;
    until("tick stored", || {
        std::fs::read_to_string(Services::store_path(&r.dir.path().join(".storage"), "svc"))
            .is_ok_and(|s| s.contains("\"ticks\":1"))
    })
    .await;
}

const WATCHER: &str = r#"
local zeron = require("./zeron")("svc")
local stop: (() -> ())? = nil
zeron.serve("watch", function(args)
	stop = zeron.watch({ args.folder }, function(changed)
		local paths = changed[args.folder]
		zeron.emit("changed", { all = paths == true, paths = if paths == true then {} else paths })
	end, { quiet = args.quiet })
	return true
end)
zeron.serve("unwatch", function()
	if stop then stop() end
	return true
end)
"#;

/// What the service told windows of the folder, past the watch going live
/// (its first "anything may have changed").
fn told(r: &Rig) -> Vec<Json> {
    lock(&r.hooks.emitted)
        .iter()
        .filter(|(_, event, _)| event == "changed")
        .map(|(_, _, data)| data.clone())
        .collect()
}

#[tokio::test]
async fn a_watch_tells_a_burst_of_changes_once_and_reads_are_not_changes() {
    let r = rig(Config::default());
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(folder.path().join("src")).unwrap();
    std::fs::write(folder.path().join("src/a.rs"), "a").unwrap();
    let plan = r.plan("1.0.0", WATCHER, &["service", "files.read"]);
    let path = folder.path().display().to_string();
    assert_eq!(r.ask(&plan, "watch", json!({ "folder": path, "quiet": 150 })).await.unwrap(), json!(true));
    // As it goes live: anything may have changed, once.
    until("live", || told(&r).len() == 1).await;
    assert_eq!(told(&r)[0]["all"], json!(true));
    // A burst of writes: told once, after it's quiet, with each path.
    for i in 0..5 {
        std::fs::write(folder.path().join("src/a.rs"), format!("a{i}")).unwrap();
        std::fs::write(folder.path().join(format!("b{i}.txt")), "b").unwrap();
    }
    until("the burst", || told(&r).len() == 2).await;
    let paths = told(&r)[1]["paths"].clone();
    let paths: Vec<&str> = paths.as_array().unwrap().iter().filter_map(|p| p.as_str()).collect();
    assert!(paths.contains(&"src/a.rs") && paths.contains(&"b4.txt"), "{paths:?}");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(told(&r).len(), 2, "one burst, told once: {:?}", told(&r));
    // Reads (as `git status` reads the tree) aren't changes.
    for _ in 0..5 {
        let _ = std::fs::read_to_string(folder.path().join("src/a.rs")).unwrap();
        let _ = std::fs::read_dir(folder.path().join("src")).unwrap().count();
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(told(&r).len(), 2, "reads were told as changes: {:?}", told(&r));
    // Stopped: nothing more.
    r.ask(&plan, "unwatch", json!({})).await.unwrap();
    std::fs::write(folder.path().join("src/a.rs"), "after").unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(told(&r).len(), 2, "heard after it stopped: {:?}", told(&r));
}

#[tokio::test]
async fn a_watch_needs_files_read_and_a_folder() {
    let r = rig(Config::default());
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().display().to_string();
    let plan = r.plan("1.0.0", WATCHER, &["service"]);
    let refused = r.ask(&plan, "watch", json!({ "folder": path })).await.unwrap_err();
    assert!(refused.message.contains("wasn't allowed to read files"), "{refused:?}");
    let r = rig(Config::default());
    let plan = r.plan("1.0.0", WATCHER, &["service", "files.read"]);
    let missing = folder.path().join("nope").display().to_string();
    let refused = r.ask(&plan, "watch", json!({ "folder": missing })).await.unwrap_err();
    assert!(refused.message.contains("no folder"), "{refused:?}");
}

/// R5: a plugin file that is Luau bytecode (unvalidated by Luau, so it could
/// corrupt the engine) is refused, by `require` and as the entry.
#[tokio::test]
async fn a_bytecode_file_is_refused() {
    // A real compiled chunk whose bytes happen to be UTF-8 (as a source's
    // must): before R5 it ran.
    let bytecode = ["return 7", "return nil", "return {}", "return 'x'"]
        .iter()
        .find_map(|source| {
            (0..=2).find_map(|level| {
                let bytes = mlua::chunk::Compiler::new().set_optimization_level(level).compile(source).unwrap();
                String::from_utf8(bytes).ok()
            })
        })
        .expect("a chunk whose bytecode is UTF-8");
    let r = rig(Config::default());
    let plan = r.plan(
        "1.0.0",
        r#"
local zeron = require("./zeron")("svc")
zeron.serve("evil", function() return require("./evil") end)
"#,
        &["service"],
    );
    std::fs::write(plan.dir.join("evil.luau"), &bytecode).unwrap();
    let refused = r.ask(&plan, "evil", json!({})).await.expect_err("bytecode ran in the service VM");
    assert!(refused.message.contains("binary chunk"), "{refused:?}");

    let r = rig(Config::default());
    let plan = r.plan("1.0.0", &bytecode, &["service"]);
    let refused = r.ask(&plan, "anything", json!({})).await.expect_err("a bytecode entry loaded");
    assert!(refused.message.contains("binary chunk"), "{refused:?}");
}

const FETCHER: &str = r#"
local zeron = require("./zeron")("svc")
zeron.serve("get", function(args)
	local r = zeron.fetch(args.url, { headers = args.headers }):await()
	return r.status
end)
"#;

/// A server on this device answering 200 (or a redirect to `to`) to anyone.
async fn local_server(to: Option<String>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut socket, _)) = listener.accept().await {
            let to = to.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let reply = match to {
                    Some(to) => format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                    None => "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_string(),
                };
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    port
}

/// R15: "network" is the internet. A plugin's fetch never reaches this
/// device, your network or instance metadata (by address, by a name that
/// resolves there, or by a redirect), unless its manifest names the host.
#[tokio::test]
async fn fetch_never_reaches_this_device_or_metadata_unless_named() {
    let port = local_server(None).await;
    let r = rig(Config::default());
    let open = r.plan("1.0.0", FETCHER, &["service", "network"]);
    for url in [
        format!("http://127.0.0.1:{port}/"),
        format!("http://localhost:{port}/"),
        format!("http://[::ffff:127.0.0.1]:{port}/"),
        format!("http://0x7f.1:{port}/"),
        "http://169.254.169.254/latest/meta-data/".to_string(),
        "http://0.0.0.0/".to_string(),
    ] {
        let refused = r.ask(&open, "get", json!({ "url": url })).await;
        assert!(refused.is_err(), "{url} was reached: {refused:?}");
    }

    // Named: that host and port, and nothing it redirects to.
    let redirects = local_server(Some(format!("http://127.0.0.1:{port}/"))).await;
    let r = rig(Config::default());
    let mut named = r.plan("1.0.0", FETCHER, &["service", "network"]);
    named.hosts = vec![format!("localhost:{port}"), format!("127.0.0.1:{redirects}")];
    let ok = r.ask(&named, "get", json!({ "url": format!("http://localhost:{port}/") })).await;
    assert_eq!(ok.unwrap(), json!(200));
    let other = r.ask(&named, "get", json!({ "url": format!("http://127.0.0.1:{port}/") })).await;
    assert!(other.is_err(), "a port it didn't name: {other:?}");
    let redirected = r.ask(&named, "get", json!({ "url": format!("http://127.0.0.1:{redirects}/") })).await;
    assert!(redirected.is_err(), "followed a redirect to a host it didn't name: {redirected:?}");
    // Never Host, and credentials only to a named host.
    let host = r.ask(&named, "get", json!({ "url": format!("http://localhost:{port}/"), "headers": { "Host": "x" } })).await;
    assert!(host.is_err(), "{host:?}");
    let r = rig(Config::default());
    let open = r.plan("1.0.0", FETCHER, &["service", "network"]);
    let auth = r.ask(&open, "get", json!({ "url": "https://example.invalid/", "headers": { "Authorization": "Bearer x" } })).await;
    assert!(auth.unwrap_err().message.contains("Authorization"));
}
