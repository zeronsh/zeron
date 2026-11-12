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
        eval: true,
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

const EVAL: &str = r#"
local zeron = require("./zeron")("svc")
local made = {}
local api = { math = math, pcall = pcall, error = error, make = function(kind, w) table.insert(made, kind) return { kind = kind, w = w } end }

zeron.serve("eval", function(args)
	local started = os.clock()
	local ok, value = zeron.eval(args.source, { name = "agent", env = api, budget_ms = args.budget })
	return { ok = ok, value = value, ms = (os.clock() - started) * 1000, made = #made, leaked = api.x ~= nil }
end)

zeron.serve("add", function(args) return args.a + args.b end)
"#;

async fn eval(vm: &Vm, source: &str, budget: Option<u64>) -> Json {
    ask(vm, "eval", json!({ "source": source, "budget": budget })).await.unwrap().unwrap()
}

#[tokio::test]
async fn a_script_sees_only_what_its_given() {
    let dir = folder(&[("service.luau", EVAL)]);
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    let seen = eval(
        &vm,
        r#"
local here = {}
here.zeron, here.ui, here.require = zeron ~= nil, ui ~= nil, require ~= nil
here.getfenv, here.setfenv, here.debug = getfenv ~= nil, setfenv ~= nil, debug ~= nil
here.loadstring, here._G, here.print, here.os = loadstring ~= nil, _G ~= nil, print ~= nil, os ~= nil
-- (Strings keep their methods; the `string` library isn't given.)
here.strings = ("ab"):len() == 2 and string == nil
x = 1
here.w = make("rect", math.floor(2.5)).w
return here
"#,
        None,
    )
    .await;
    assert_eq!(seen["ok"], true, "{seen}");
    assert_eq!(
        seen["value"],
        json!({ "zeron": false, "ui": false, "require": false, "getfenv": false, "setfenv": false, "debug": false,
                "loadstring": false, "_G": false, "print": false, "os": false, "strings": true, "w": 2 })
    );
    assert_eq!(seen["made"], 1);
    // (What it set stayed its own.)
    assert_eq!(seen["leaked"], false);
}

#[tokio::test]
async fn a_scripts_errors_say_where() {
    let dir = folder(&[("service.luau", EVAL)]);
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    let syntax = eval(&vm, "local a = 1\nlocal = 2", None).await;
    assert_eq!(syntax["ok"], false);
    assert!(syntax["value"].as_str().unwrap().starts_with("agent:2:"), "{syntax}");
    let runtime = eval(&vm, "local a = 1\nlocal b = nil\nreturn b.field", None).await;
    assert_eq!(runtime["ok"], false);
    assert!(runtime["value"].as_str().unwrap().starts_with("agent:3:"), "{runtime}");
    let raised = eval(&vm, "\nerror('no such screen')", None).await;
    assert_eq!(raised["value"], "agent:2: no such screen");
    // Bytecode is refused, as `require` refuses it.
    let binary = eval(&vm, "\x1bLua", None).await;
    assert_eq!(binary["ok"], false, "{binary}");
}

#[tokio::test]
async fn a_runaway_script_stops_at_its_budget_and_the_service_goes_on() {
    let dir = folder(&[("service.luau", EVAL)]);
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    for source in [
        "while true do end",
        // (A `pcall` it was given doesn't keep it going.)
        "while true do pcall(function() while true do end end) end",
    ] {
        let spun = eval(&vm, source, Some(50)).await;
        assert_eq!(spun["ok"], false, "{spun}");
        assert_eq!(spun["value"], "agent: the script ran past its time budget (50 ms)");
        let ms = spun["ms"].as_f64().unwrap();
        assert!((50.0..500.0).contains(&ms), "{ms}");
    }
    assert_eq!(eval(&vm, "return 1 + 1", None).await["value"], 2);
    assert_eq!(ask(&vm, "add", json!({ "a": 1, "b": 2 })).await.unwrap().unwrap(), json!(3));
}

#[tokio::test]
async fn eval_needs_its_permission() {
    let dir = folder(&[("service.luau", EVAL)]);
    let ops = Arc::new(Echo::default());
    let (vm, loaded) = Vm::start(Spec { eval: false, ..spec(dir.path(), Limits::default()) }, ops);
    loaded.await.unwrap().unwrap();
    match ask(&vm, "eval", json!({ "source": "return 1" })).await.unwrap() {
        Err(Failure::Error(why)) => assert!(why.contains("needs the \"eval\" permission"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_hundred_line_script_runs_in_a_few_milliseconds() {
    let dir = folder(&[("service.luau", EVAL)]);
    let (vm, described, _) = start(dir.path(), Limits::default()).await;
    described.unwrap();
    let mut source = String::from("local total = 0\n");
    for i in 0..98 {
        source.push_str(&format!("total = total + make(\"rect\", math.max({i}, 1)).w\n"));
    }
    source.push_str("return total");
    let ran = eval(&vm, &source, None).await;
    assert_eq!(ran["ok"], true, "{ran}");
    assert_eq!(ran["made"], 98);
    let ms = ran["ms"].as_f64().unwrap();
    eprintln!("a 100-line script compiled and ran in {ms:.2} ms");
    assert!(ms < 50.0, "{ms}");
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
