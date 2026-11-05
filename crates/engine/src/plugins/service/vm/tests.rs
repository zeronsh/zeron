use super::*;
use serde_json::json;

/// Ops that answer `run` with its args echoed (or refuse `refused`), and
/// keep what's emitted and logged.
#[derive(Default)]
struct Echo {
    emitted: Mutex<Vec<(String, Json)>>,
    logged: Mutex<Vec<String>>,
}

impl Ops for Echo {
    fn start(&self, kind: &str, args: Json, done: Done) {
        let kind = kind.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            if args["program"] == "refused" {
                done(Err("not allowed".into()));
            } else {
                done(Ok(json!({ "kind": kind, "args": args })));
            }
        });
    }
    fn emit(&self, event: &str, data: Json) {
        self.emitted.lock().unwrap().push((event.to_owned(), data));
    }
    fn log(&self, line: &str) {
        self.logged.lock().unwrap().push(line.to_owned());
    }
}

fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::write(dir.path().join("zeron.luau"), crate::plugins::scaffold::SDK).unwrap();
    dir
}

fn spec(dir: &Path, limits: Limits) -> Spec {
    Spec {
        plugin: "svc".into(),
        dir: dir.to_path_buf(),
        entry: "service.luau".into(),
        device: json!({ "id": "dev-1", "name": "Laptop" }),
        store: Some(dir.join(".store.json")),
        limits,
    }
}

async fn start(dir: &Path, limits: Limits) -> (Vm, Result<Described, String>, Arc<Echo>) {
    let ops = Arc::new(Echo::default());
    let (vm, loaded) = Vm::start(spec(dir, limits), ops.clone());
    let described = loaded.await.unwrap_or_else(|_| Err("crashed".into()));
    (vm, described, ops)
}

async fn ask(vm: &Vm, name: &str, args: Json) -> Result<Reply, &'static str> {
    tokio::time::timeout(Duration::from_secs(10), vm.call(Kind::Method, name, args, json!({})))
        .await
        .map_err(|_| "timed out")?
        .map_err(|_| "ended")
}

const SERVICE: &str = r#"
local zeron = require("./zeron")("svc")
local util = require("./lib/util")

zeron.serve("add", function(args)
	return { sum = args.a + args.b, device = zeron.device.name }
end, { safe = true })

zeron.serve("run", function(args)
	local ran = zeron.run("echo", { args.word }):await()
	zeron.emit("ran", { word = args.word })
	return ran
end)

zeron.serve("refused", function()
	return zeron.run("refused"):await()
end)

zeron.serve("nap", function()
	zeron.sleep(0.05):await()
	return util.twice("nap")
end)

zeron.serve("keep", function(args)
	local box = zeron.store("n", 0)
	box:set(box:get() + args.by)
	return box:get()
end)

zeron.serve("spin", function()
	while true do end
end)

zeron.serve("hog", function()
	local t = {}
	for i = 1, 1e9 do t[i] = string.rep("x", 1024) .. i end
end)

zeron.serve("boom", function() error("it broke", 0) end)

zeron.tool { name = "count", run = function(args) return #args.text end }
zeron.every(60, function() end)
"#;

const UTIL: &str = r#"
return { twice = function(s) return s .. s end }
"#;

#[tokio::test]
async fn a_service_loads_serves_and_awaits() {
    let dir = folder(&[("service.luau", SERVICE), ("lib/util.luau", UTIL)]);
    let (vm, described, ops) = start(dir.path(), Limits::default()).await;
    let described = described.unwrap();
    assert_eq!(described.methods.get("add"), Some(&true));
    assert_eq!(described.methods.get("run"), Some(&false));
    assert_eq!(described.tools, vec!["count".to_string()]);
    assert_eq!(described.timers, vec![("#1".to_string(), 60.0)]);

    let added = ask(&vm, "add", json!({ "a": 2, "b": 3 })).await.unwrap().unwrap();
    assert_eq!(added, json!({ "sum": 5, "device": "Laptop" }));

    let ran = ask(&vm, "run", json!({ "word": "hi" })).await.unwrap().unwrap();
    assert_eq!(ran["kind"], "run");
    assert_eq!(ran["args"]["program"], "echo");
    assert_eq!(ran["args"]["args"], json!(["hi"]));
    assert_eq!(ops.emitted.lock().unwrap().as_slice(), &[("ran".to_string(), json!({ "word": "hi" }))]);

    // A refused op fails the handler that awaited it, with the reason.
    match ask(&vm, "refused", json!({})).await.unwrap() {
        Err(Failure::Error(why)) => assert!(why.contains("not allowed"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ask(&vm, "nap", json!({})).await.unwrap().unwrap(), json!("napnap"));
    assert_eq!(ask(&vm, "missing", json!({})).await.unwrap(), Err(Failure::Unknown));
    assert_eq!(
        ask(&vm, "boom", json!({})).await.unwrap(),
        Err(Failure::Error("it broke".into()))
    );
    let counted = tokio::time::timeout(
        Duration::from_secs(5),
        vm.call(Kind::Tool, "count", json!({ "text": "four" }), json!({})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(counted, Ok(json!(4)));
    assert!(!vm.status().busy());
}

#[tokio::test]
async fn calls_overlap_while_one_awaits() {
    let dir = folder(&[("service.luau", SERVICE), ("lib/util.luau", UTIL)]);
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    let slow = vm.call(Kind::Method, "nap", json!({}), json!({}));
    let quick = ask(&vm, "add", json!({ "a": 1, "b": 1 })).await.unwrap().unwrap();
    assert_eq!(quick["sum"], 2);
    assert_eq!(slow.await.unwrap().unwrap(), json!("napnap"));
}

#[tokio::test]
async fn the_store_survives_a_restart() {
    let dir = folder(&[("service.luau", SERVICE), ("lib/util.luau", UTIL)]);
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    assert_eq!(ask(&vm, "keep", json!({ "by": 2 })).await.unwrap().unwrap(), json!(2));
    vm.stop();
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    assert_eq!(ask(&vm, "keep", json!({ "by": 3 })).await.unwrap().unwrap(), json!(5));
}

#[tokio::test]
async fn a_runaway_loop_fails_its_call_and_the_vm_goes_on() {
    let dir = folder(&[("service.luau", SERVICE), ("lib/util.luau", UTIL)]);
    let limits = Limits {
        slice: Duration::from_millis(100),
        ..Limits::default()
    };
    let (vm, described, _) = start(dir.path(), limits).await;
    described.unwrap();
    match ask(&vm, "spin", json!({})).await.unwrap() {
        Err(Failure::Error(why)) => assert!(why.contains("time budget"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ask(&vm, "add", json!({ "a": 1, "b": 2 })).await.unwrap().unwrap()["sum"], 3);
}

#[tokio::test]
async fn running_out_of_memory_ends_the_vm() {
    let dir = folder(&[("service.luau", SERVICE), ("lib/util.luau", UTIL)]);
    let limits = Limits {
        memory: 8 << 20,
        slice: Duration::from_secs(20),
        ..Limits::default()
    };
    let (vm, described, _) = start(dir.path(), limits).await;
    described.unwrap();
    assert_eq!(ask(&vm, "hog", json!({})).await, Err("ended"));
    for _ in 0..100 {
        if vm.status().ended() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(vm.status().ended());
}

#[tokio::test]
async fn the_sandbox_reads_only_the_plugins_own_files() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.luau"), "return 'secret'").unwrap();
    let escape = format!(
        r#"
local zeron = require("./zeron")("svc")
zeron.serve("try", function(args)
	local ok, why = pcall(require, args.path)
	return {{ ok = ok, why = tostring(why) }}
end)
zeron.serve("globals", function()
	return {{ io = io == nil, loadstring = loadstring == nil, debug = debug == nil, getfenv_g = pcall(function() _G.x = 1 end) }}
end)
local _ = "{}"
"#,
        outside.path().display()
    );
    let dir = folder(&[("service.luau", escape.as_str())]);
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    for path in [
        "../secret".to_string(),
        format!("{}/secret", outside.path().display()),
        "@self/x".into(),
        "./.hidden".into(),
    ] {
        let tried = ask(&vm, "try", json!({ "path": path })).await.unwrap().unwrap();
        assert_eq!(tried["ok"], false, "{path}: {tried}");
    }
    let globals = ask(&vm, "globals", json!({})).await.unwrap().unwrap();
    assert_eq!(globals, json!({ "io": true, "loadstring": true, "debug": true, "getfenv_g": false }));
}

#[tokio::test]
async fn a_broken_service_says_why_it_didnt_load() {
    let dir = folder(&[("service.luau", "local x = = 1")]);
    let (_, described, _) = start(dir.path(), Limits::default()).await;
    let why = described.unwrap_err();
    assert!(why.contains("service.luau"), "{why}");
    let dir = folder(&[("service.luau", "error('nope', 0)")]);
    let (_, described, _) = start(dir.path(), Limits::default()).await;
    assert_eq!(described.unwrap_err(), "nope");
    let dir = folder(&[]);
    let (_, described, _) = start(dir.path(), Limits::default()).await;
    assert!(described.unwrap_err().contains("isn't in the plugin"));
}

#[test]
fn paths_resolve_inside_the_plugin() {
    assert_eq!(resolve("service.luau", "./zeron").as_deref(), Some("zeron.luau"));
    assert_eq!(resolve("lib/a.luau", "./b").as_deref(), Some("lib/b.luau"));
    assert_eq!(resolve("lib/a.luau", "../zeron").as_deref(), Some("zeron.luau"));
    assert_eq!(resolve("service.luau", "../x"), None);
    assert_eq!(resolve("service.luau", "x"), None);
    assert_eq!(resolve("service.luau", "/etc/x"), None);
    assert_eq!(resolve("service.luau", "./.git/x"), None);
}

#[test]
fn json_round_trips_through_luau() {
    let lua = Lua::new();
    for value in [
        json!({ "a": [1, 2.5, "x", true], "b": { "c": null }, "d": [] }),
        json!([{ "x": 1 }, { "y": [] }]),
        json!("text"),
        json!(42),
    ] {
        let back = to_json(&to_lua(&lua, &value, 0).unwrap(), 0).unwrap();
        // (null drops out of an object; an empty array reads back as {}.)
        let expected = match value.clone() {
            Json::Object(mut m) => {
                m.insert("b".into(), json!({}));
                m.insert("d".into(), json!({}));
                Json::Object(m)
            }
            Json::Array(_) => json!([{ "x": 1 }, { "y": {} }]),
            other => other,
        };
        assert_eq!(back, expected);
    }
}
