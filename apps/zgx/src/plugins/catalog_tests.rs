//! The store's seam, headless: each `plugins.store.*` call a screen makes
//! (ui/screens/plugins/store.luau) reaches the engine as the engine's own
//! method with its params (crates/rpc `STORE_*`), and what the engine
//! answers reaches the script as the screens read it, an `{ ok: false }`
//! answer as its failure word. An engine of the test's own stands in for
//! the IPC (catalog.rs `Engine`), answering as the real one documents.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, testing::TestApp};

use super::{Engine, install_with};

/// What the engine was asked: (method, params, device).
type Asked = Rc<RefCell<Vec<(String, Json, Option<String>)>>>;

/// A script that makes `call` (Luau, one `ui.call`) as a button's click and
/// shows its outcome as `#out`: `ok:<json of what it got>` or `err:<word>`.
fn script(call: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-store-seam-{}-{}", std::process::id(), rand_name(call)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let code = format!(
        r#"
local out = ui.state("out", "")
local function shown(v: any): string
	if type(v) ~= "table" then
		return tostring(v)
	end
	local keys = {{}}
	for k in v do
		table.insert(keys, tostring(k))
	end
	table.sort(keys)
	local parts = {{}}
	for _, k in keys do
		table.insert(parts, `{{k}}={{shown(v[k])}}`)
	end
	return "{{" .. table.concat(parts, ",") .. "}}"
end
return ui.column {{
	ui.button {{ "go", on_click = function()
		({call}):after(function(r: any)
			out:set("ok:" .. shown(r))
		end, function(err: any)
			out:set("err:" .. tostring(err.message))
		end)
	end }},
	ui.text {{ id = "out", function(): string return out:get() end }},
}}
"#
    );
    let path = dir.join("app.luau");
    std::fs::write(&path, code).unwrap();
    path
}

fn rand_name(s: &str) -> String {
    let mut n: u64 = 1469598103934665603;
    for b in s.bytes() {
        n = (n ^ b as u64).wrapping_mul(1099511628211);
    }
    format!("{n:x}")
}

/// `call` made against an engine answering `reply` to every method; what
/// the script showed, and what the engine was asked.
fn run(call: &str, reply: Json) -> (String, Vec<(String, Json, Option<String>)>) {
    let asked: Asked = Rc::default();
    let path = script(call);
    let (a, r) = (asked.clone(), reply.clone());
    let mut app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, path.clone());
        let engine: Engine = Rc::new(move |method, params, device| {
            a.borrow_mut().push((method.to_string(), params, device));
            let r = r.clone();
            Box::pin(async move { Ok(r) })
        });
        install_with(&host, engine);
        host
    });
    app.frames(3);
    app.click("role:button \"go\"");
    let done = app.wait_until(|app| !app.text("out").is_empty());
    assert!(done, "no answer came");
    let out = app.text("out");
    let asked = asked.borrow().clone();
    (out, asked)
}

#[test]
fn a_plugin_s_page_asks_for_every_version_and_this_device_s_zeron() {
    let (out, asked) = run(
        r#"ui.call("plugins.store.detail", { id = "focus-timer", for_zeron = "0.4.2" })"#,
        json!({ "id": "focus-timer", "compatible": false, "incompatible": "Needs Zeron 0.5.0 or later" }),
    );
    assert_eq!(asked, vec![("StorePlugin".into(), json!({ "id": "focus-timer", "versions": true, "for_zeron": "0.4.2" }), None)]);
    assert!(out.contains("compatible=false") && out.contains("incompatible=Needs Zeron 0.5.0"), "{out}");
    // This device's own: no `for_zeron`.
    let (_, asked) = run(r#"ui.call("plugins.store.detail", "focus-timer")"#, json!({}));
    assert_eq!(asked[0].1, json!({ "id": "focus-timer", "versions": true }));
}

#[test]
fn what_isn_t_a_store_id_never_reaches_the_engine() {
    let (out, asked) = run(r#"ui.call("plugins.store.detail", "../../etc")"#, json!({}));
    assert_eq!(out, "err:not_in_store");
    assert!(asked.is_empty());
}

#[test]
fn a_search_sends_what_was_asked_and_nothing_empty() {
    let (_, asked) = run(
        r#"ui.call("plugins.store.search", { q = " timer ", category = "", sort = "popular", limit = 500, featured = true })"#,
        json!({ "items": [] }),
    );
    assert_eq!(asked[0].0, "StoreSearch");
    assert_eq!(asked[0].1, json!({ "q": "timer", "sort": "popular", "limit": 50, "featured": true }));
}

#[test]
fn an_answer_that_says_no_is_its_word() {
    for (call, method) in [
        (r#"ui.call("plugins.store.publish", { id = "focus", hash = "h", ticket = "t" })"#, "PublishPlugin"),
        (r#"ui.call("plugins.store.report", { id = "focus", reason = "broken" })"#, "StoreReport"),
        (r#"ui.call("plugins.store.version_action", { id = "focus", version = "1.0.0", action = "yank" })"#, "StoreVersionAction"),
    ] {
        let (out, asked) = run(call, json!({ "ok": false, "error": "rate_limited", "message": "Too many" }));
        assert_eq!(asked[0].0, method);
        assert_eq!(out, "err:rate_limited: Too many", "{method}");
    }
    let (out, asked) = run(r#"ui.call("plugins.store.publish", { id = "focus", hash = "h1", ticket = "t1" })"#, json!({ "ok": true, "version": "1.2.0", "status": "pending" }));
    assert_eq!(asked[0].1, json!({ "id": "focus", "hash": "h1", "ticket": "t1" }));
    assert!(out.contains("status=pending"), "{out}");
}

#[test]
fn a_report_carries_its_reason_and_words_and_only_the_store_s_reasons() {
    let (_, asked) = run(r#"ui.call("plugins.store.report", { id = "focus", reason = "misleading", text = "It says it's Zeron's" })"#, json!({ "ok": true }));
    assert_eq!(asked[0], ("StoreReport".into(), json!({ "id": "focus", "reason": "misleading", "text": "It says it's Zeron's" }), None));
    let (out, asked) = run(r#"ui.call("plugins.store.report", { id = "focus", reason = "spam" })"#, json!({ "ok": true }));
    assert_eq!(out, "err:invalid");
    assert!(asked.is_empty());
}

#[test]
fn a_deploy_key_that_isn_t_allowed_yet_says_so() {
    let (out, asked) = run(
        r#"ui.call("plugins.store.create_key", { id = "focus", name = "CI" })"#,
        json!({ "ok": false, "error": "not_published", "message": "Publish a version first" }),
    );
    assert_eq!(asked[0], ("CreateDeployKey".into(), json!({ "id": "focus", "name": "CI" }), None));
    assert_eq!(out, "err:not_published: Publish a version first");
}

#[test]
fn a_deploy_key_s_sign_in_is_begun_for_a_deploy_key() {
    // (Every reply says a sign-in is needed, its page no https one: the
    // browser never opens, the call ends there.)
    let (out, asked) = run(
        r#"ui.call("plugins.store.create_key", { id = "focus", name = "CI" })"#,
        json!({ "ok": false, "error": "step_up_required", "url": "about:blank" }),
    );
    let methods: Vec<&str> = asked.iter().map(|(m, _, _)| m.as_str()).collect();
    assert_eq!(methods, ["CreateDeployKey", "StepUp"]);
    assert_eq!(asked[1].1, json!({ "for": "deploy_key" }));
    assert_eq!(out, "err:failed");
}

#[test]
fn pre_releases_go_to_the_device_chosen() {
    let (_, asked) = run(r#"ui.call("plugins.store.prereleases", { id = "focus", on = true, device = "devbox" })"#, json!({}));
    assert_eq!(asked[0], ("SetPluginPrereleases".into(), json!({ "id": "focus", "on": true }), Some("devbox".into())));
}

#[test]
fn a_picture_is_the_engine_s_kept_file_and_never_anything_else() {
    let (out, asked) = run(
        r#"ui.call("plugins.store.picture", { id = "focus", version = "1.0.0", path = "./shots//a.png" })"#,
        json!({ "file": "/data/store-files/focus/1.0.0/shots/a.png", "type": "image/png", "bytes": 10 }),
    );
    assert_eq!(asked[0], ("StoreFile".into(), json!({ "id": "focus", "version": "1.0.0", "path": "shots/a.png" }), None));
    assert_eq!(out, "ok:{src=/data/store-files/focus/1.0.0/shots/a.png}");
    // An SVG, a file that isn't one here, or a path out of the package: no.
    let (out, _) = run(r#"ui.call("plugins.store.picture", { id = "focus", version = "1.0.0", path = "a.svg" })"#, json!({ "file": "/x/a.svg", "type": "image/svg+xml" }));
    assert_eq!(out, "err:not_a_picture");
    let (out, _) = run(r#"ui.call("plugins.store.picture", { id = "focus", version = "1.0.0", path = "a.png" })"#, json!({ "file": "relative/a.png", "type": "image/png" }));
    assert_eq!(out, "err:not_in_store");
    let (out, asked) = run(r#"ui.call("plugins.store.picture", { id = "focus", version = "1.0.0", path = "%2e%2e/a.png" })"#, json!({}));
    assert_eq!(out, "err:not_in_store");
    assert!(asked.is_empty());
}
