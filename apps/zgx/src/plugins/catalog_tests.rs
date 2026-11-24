//! The store's seam, headless: each `plugins.store.*` call a screen makes
//! (ui/screens/plugins/store.luau) reaches the engine as the engine's own
//! method with its params (crates/rpc `STORE_*`), and what the engine
//! answers reaches the script as the screens read it, an `{ ok: false }`
//! answer as its failure word. An engine of the test's own stands in for
//! the IPC (catalog.rs `Engine`), answering as the real one documents.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, testing::TestApp};

use super::{Asker, Engine, install_with, keep_prepared};
use crate::plugins::preview;

/// What the engine was asked: (method, params, device).
type Asked = Rc<RefCell<Vec<(String, Json, Option<String>)>>>;

/// A script that makes `call` (Luau, one `ui.call`) as a button's click and
/// shows its outcome as `#out`: `ok:<json of what it got>` or `err:<word>`.
fn script(call: &str) -> PathBuf {
    // (Its own folder: tests making the same call run at once.)
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("zgx-store-seam-{}-{n}", std::process::id()));
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

/// `call` made against an engine answering `reply` to every method; what
/// the script showed, and what the engine was asked.
fn run(call: &str, reply: Json) -> (String, Vec<(String, Json, Option<String>)>) {
    run_asking(call, reply, Rc::new(|_| Box::pin(async { panic!("asked, with no template but Zeron's run") })))
}

/// `run`, what you do as yourself asked by `asker` (act_consent.rs).
fn run_asking(call: &str, reply: Json, asker: Asker) -> (String, Vec<(String, Json, Option<String>)>) {
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
        install_with(&host, engine, asker.clone());
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
    assert_eq!(asked, vec![("StorePlugin".into(), json!({ "id": "focus-timer", "for_zeron": "0.4.2" }), None)]);
    assert!(out.contains("compatible=false") && out.contains("incompatible=Needs Zeron 0.5.0"), "{out}");
    // This device's own: no `for_zeron`.
    let (_, asked) = run(r#"ui.call("plugins.store.detail", "focus-timer")"#, json!({}));
    assert_eq!(asked[0].1, json!({ "id": "focus-timer" }));
}

#[test]
fn a_plugin_s_versions_are_read_a_page_or_one_at_a_time() {
    let (_, asked) = run(r#"ui.call("plugins.store.versions", { id = "focus-timer", cursor = "c2", limit = 500 })"#, json!({ "versions": [] }));
    assert_eq!(asked[0], ("StoreVersions".into(), json!({ "id": "focus-timer", "cursor": "c2", "limit": 50 }), None));
    let (_, asked) = run(r#"ui.call("plugins.store.version", { id = "focus-timer", version = "1.2.0" })"#, json!({}));
    assert_eq!(asked[0], ("StoreVersion".into(), json!({ "id": "focus-timer", "version": "1.2.0" }), None));
    let (out, asked) = run(r#"ui.call("plugins.store.version", { id = "focus-timer", version = "../x" })"#, json!({}));
    assert_eq!(out, "err:not_in_store");
    assert!(asked.is_empty());
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
    // A publisher's plugins, verified ones alone; false and empty are left out.
    let (_, asked) = run(
        r#"ui.call("plugins.store.search", { author = " ada ", verified = true, keyword = "" })"#,
        json!({ "items": [] }),
    );
    assert_eq!(asked[0].1, json!({ "author": "ada", "verified": true }));
    let (_, asked) = run(r#"ui.call("plugins.store.search", { verified = false })"#, json!({ "items": [] }));
    assert_eq!(asked[0].1, json!({}));
}

#[test]
fn an_answer_that_says_no_is_its_word() {
    let (publish, _) = prepared_publish("focus", "h");
    for (call, method) in [
        (publish.as_str(), "PublishPlugin"),
        (r#"ui.call("plugins.store.report", { id = "focus", reason = "broken" })"#, "StoreReport"),
        (r#"ui.call("plugins.store.version_action", { id = "focus", version = "1.0.0", action = "yank" })"#, "StoreVersionAction"),
    ] {
        let (out, asked) = run(call, json!({ "ok": false, "error": "rate_limited", "message": "Too many" }));
        assert_eq!(asked[0].0, method);
        assert_eq!(out, "err:rate_limited: Too many", "{method}");
    }
    let (publish, ticket) = prepared_publish("focus", "h1");
    let (out, asked) = run(&publish, json!({ "ok": true, "version": "1.2.0", "status": "pending" }));
    assert_eq!(asked[0].1, json!({ "id": "focus", "hash": "h1", "ticket": ticket }));
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
fn a_deploy_token_that_isn_t_allowed_says_so() {
    let (out, asked) = run(
        r#"ui.call("plugins.store.create_token", { name = "CI", scope = { plugins = { "focus", "timer" } } })"#,
        json!({ "ok": false, "error": "not_owner", "message": "Not yours: timer" }),
    );
    assert_eq!(asked[0], ("CreateDeployToken".into(), json!({ "name": "CI", "scope": { "plugins": ["focus", "timer"] } }), None));
    assert_eq!(out, "err:not_owner: Not yours: timer");
}

#[test]
fn a_deploy_token_s_scope_is_all_or_one_to_fifty_store_ids_each_once() {
    let (_, asked) = run(r#"ui.call("plugins.store.create_token", { name = "CI", scope = { all = true } })"#, json!({ "ok": true }));
    assert_eq!(asked[0].1, json!({ "name": "CI", "scope": { "all": true } }));
    let (_, asked) = run(r#"ui.call("plugins.store.create_token", { name = "CI", scope = { all = true, all_kinds = { "plugin", "template" } } })"#, json!({ "ok": true }));
    assert_eq!(asked[0].1, json!({ "name": "CI", "scope": { "all": true, "all_kinds": ["plugin", "template"] } }));
    for bad in [
        r#"{ name = "CI" }"#,
        r#"{ name = "CI", scope = { plugins = {} } }"#,
        r#"{ name = "CI", scope = { plugins = { "focus", "focus" } } }"#,
        r#"{ name = "CI", scope = { plugins = { "../etc" } } }"#,
        r#"{ name = "CI", scope = { all = false } }"#,
        r#"{ name = "CI", scope = { all = true, all_kinds = { "template" } } }"#,
        r#"{ name = "CI", scope = { all = true, all_kinds = { "template", "plugin" } } }"#,
        r#"{ name = "CI", scope = { all = true, all_kinds = "template" } }"#,
    ] {
        let (out, asked) = run(&format!(r#"ui.call("plugins.store.create_token", {bad})"#), json!({ "ok": true }));
        assert_eq!(out, "err:invalid_scope", "{bad}");
        assert!(asked.is_empty(), "{bad}");
    }
    // The account's tokens, and one revoked by its id alone.
    let (_, asked) = run(r#"ui.call("plugins.store.tokens", nil)"#, json!({ "tokens": [] }));
    assert_eq!(asked[0], ("ListDeployTokens".into(), json!({}), None));
    let (_, asked) = run(r#"ui.call("plugins.store.revoke_token", { id = "t_gh" })"#, json!({ "ok": true }));
    assert_eq!(asked[0], ("RevokeDeployToken".into(), json!({ "id": "t_gh" }), None));
}

#[test]
fn stopping_the_wait_for_a_token_s_sign_in_cancels_it_at_the_engine() {
    let (out, asked) = run(r#"ui.call("plugins.store.cancel_step_up", nil)"#, json!({ "ok": true }));
    assert_eq!(asked[0], ("CancelStepUp".into(), json!({ "for": "deploy_token" }), None));
    assert_eq!(out, "ok:nil");
    // (An engine without it yet: still stopped here, nothing said.)
    let (out, _) = run(r#"ui.call("plugins.store.cancel_step_up", nil)"#, json!({ "ok": false, "error": "unknown_method" }));
    assert_eq!(out, "ok:nil");
}

#[test]
fn a_deploy_token_s_sign_in_is_begun_for_a_deploy_token() {
    // (Every reply says a sign-in is needed, its page no https one: the
    // browser never opens, the call ends there.)
    let (out, asked) = run(
        r#"ui.call("plugins.store.create_token", { name = "CI", scope = { all = true } })"#,
        json!({ "ok": false, "error": "step_up_required", "url": "about:blank" }),
    );
    let methods: Vec<&str> = asked.iter().map(|(m, _, _)| m.as_str()).collect();
    assert_eq!(methods, ["CreateDeployToken", "StepUp"]);
    assert_eq!(asked[1].1, json!({ "for": "deploy_token" }));
    assert_eq!(out, "err:failed");
}

#[test]
fn a_preview_reads_the_catalog_but_never_your_own_account() {
    crate::plugins::preview::set(Some(crate::plugins::preview::Preview { id: "dusk".into(), name: "Dusk".into() }));
    for call in [r#"ui.call("plugins.store.mine", nil)"#, r#"ui.call("plugins.store.tokens", "focus")"#] {
        let (out, asked) = run(call, json!({ "plugins": [], "tokens": [] }));
        assert!(out.starts_with("err:Dusk is a preview."), "{call}: {out}");
        assert!(asked.is_empty(), "{call} reached the engine: {asked:?}");
    }
    let (out, asked) = run(r#"ui.call("plugins.store.search", { q = "timer" })"#, json!({ "items": [] }));
    assert!(out.starts_with("ok:"), "{out}");
    assert_eq!(asked[0].0, "StoreSearch");
    crate::plugins::preview::set(None);
}

#[test]
fn which_updates_a_plugin_or_the_device_takes_is_newest_or_verified() {
    let (_, asked) = run(r#"ui.call("plugins.store.updates", { id = "focus", prefer = "verified", device = "devbox" })"#, json!({ "prefer": "verified" }));
    assert_eq!(asked[0], ("SetPluginUpdates".into(), json!({ "id": "focus", "prefer": "verified" }), Some("devbox".into())));
    let (_, asked) = run(r#"ui.call("plugins.store.updates", { prefer = "newest", device = "devbox" })"#, json!({}));
    assert_eq!(asked[0], ("SetPluginUpdates".into(), json!({ "prefer": "newest" }), Some("devbox".into())));
    let (_, asked) = run(r#"ui.call("plugins.store.updates", { id = "focus" })"#, json!({}));
    assert_eq!(asked[0].1, json!({ "id": "focus", "prefer": null }));
    let (out, asked) = run(r#"ui.call("plugins.store.updates", { id = "focus", prefer = "anything" })"#, json!({}));
    assert_eq!(out, "err:invalid");
    assert!(asked.is_empty());
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

/// The engine's ticket never reaches the page: it has a handle, good for
/// what was checked and nothing else, once.
#[test]
fn a_publish_s_ticket_stays_in_the_host() {
    let prepared = json!({ "id": "focus", "name": "Focus", "version": "1.2.0", "hash": "h", "ok": true, "ticket": "ticket-real" });
    let (out, _) = run(r#"ui.call("plugins.store.prepare", "focus")"#, prepared);
    assert!(out.contains("ticket=prepared-") && !out.contains("ticket-real"), "{out}");
    // Not all said: no ticket at all.
    let (out, _) = run(r#"ui.call("plugins.store.prepare", "focus")"#, json!({ "id": "focus", "ok": true, "ticket": "ticket-real" }));
    assert!(!out.contains("ticket"), "{out}");
    let (publish, _) = prepared_publish("focus", "h");
    let (_, asked) = run(&publish, json!({ "ok": true }));
    assert_eq!(asked.len(), 1);
    let (out, asked) = run(&publish, json!({ "ok": true }));
    assert_eq!((out.as_str(), asked.len()), ("err:changed", 0), "good once");
    for other in ["prepared-0", "ticket-real", ""] {
        let (out, asked) = run(&format!(r#"ui.call("plugins.store.publish", {{ id = "focus", hash = "h", ticket = "{other}" }})"#), json!({ "ok": true }));
        assert_eq!((out.as_str(), asked.len()), ("err:changed", 0), "{other}");
    }
}

/// A publish of `id` the engine checked: the page's call for it (its handle),
/// and the engine's own ticket.
fn prepared_publish(id: &str, hash: &str) -> (String, &'static str) {
    let shown = keep_prepared(json!({ "id": id, "name": "Focus timer", "version": "1.3.0", "hash": hash, "ticket": "ticket-real" }), "plugin");
    let handle = shown["ticket"].as_str().unwrap();
    assert_ne!(handle, "ticket-real");
    (format!(r#"ui.call("plugins.store.publish", {{ id = "{id}", hash = "{hash}", ticket = "{handle}", name = "Anything", version = "9.9.9" }})"#), "ticket-real")
}

/// An asker that says yes (or no), and what it was asked.
fn asker(allow: bool) -> (Asker, Rc<RefCell<Vec<Json>>>) {
    let items: Rc<RefCell<Vec<Json>>> = Rc::default();
    let seen = items.clone();
    let asker: Asker = Rc::new(move |act| {
        seen.borrow_mut().push(act.item);
        match allow {
            true => (act.run)(),
            false => Box::pin(async { Err("cancelled".to_string()) }),
        }
    });
    (asker, items)
}

/// Once a template that isn't Zeron's has run, publishing, taking new
/// installs again and a new token are asked in Zeron's own window, saying
/// what the engine said of it; not allowed, the engine is asked nothing.
/// Before, and for anything else, nothing is asked.
#[test]
fn acting_as_you_is_asked_in_zeron_s_window_once_another_s_template_ran() {
    let publish = prepared_publish("focus-timer", "h1").0;
    let publish = publish.as_str();
    let unyank = r#"ui.call("plugins.store.version_action", { id = "focus-timer", version = "1.2.0", action = "unyank" })"#;
    let yank = r#"ui.call("plugins.store.version_action", { id = "focus-timer", version = "1.2.0", action = "yank" })"#;
    let token = r#"ui.call("plugins.store.create_token", { name = "CI", scope = { plugins = { "focus-timer" } }, expires = 5 })"#;
    let made = json!({ "ok": true, "token": { "id": "t" }, "secret": "zdt_secret" });

    // Zeron's own alone: as it was (`run`'s asker would panic).
    preview::another_s_ran_for_test(false);
    for call in [publish, unyank, yank, token] {
        assert_eq!(run(call, made.clone()).1.len(), 1, "{call}");
    }
    assert!(run(token, made.clone()).0.contains("secret=zdt_secret"), "the app's own flow shows it");

    preview::another_s_ran_for_test(true);
    let again = prepared_publish("focus-timer", "h1").0;
    let publish = again.as_str();
    for (call, method, item) in [
        (publish, "PublishPlugin", json!({ "act": "publish", "kind": "plugin", "id": "focus-timer", "name": "Focus timer", "version": "1.3.0" })),
        (unyank, "StoreVersionAction", json!({ "act": "unyank", "kind": "plugin", "id": "focus-timer", "name": "focus-timer", "version": "1.2.0" })),
        (token, "CreateDeployToken", json!({ "act": "token", "name": "CI", "scope": { "plugins": ["focus-timer"] }, "expires": 5 })),
    ] {
        let (no, items) = asker(false);
        let (out, asked) = run_asking(call, made.clone(), no);
        assert_eq!((out.as_str(), asked.len()), ("err:cancelled", 0), "{call}");
        assert_eq!(*items.borrow(), vec![item], "{call}");
        let (yes, _) = asker(true);
        let (_, asked) = run_asking(call, made.clone(), yes);
        assert_eq!(asked.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), [method], "{call}");
    }
    // The token itself never reaches the window that asked for it.
    let (yes, _) = asker(true);
    let (out, _) = run_asking(token, made.clone(), yes);
    assert!(out.contains("id=t") && !out.contains("zdt_") && !out.contains("secret"), "{out}");
    // Stopping installs takes nothing from anyone: not asked.
    assert_eq!(run(yank, made.clone()).1.len(), 1);
    // A handle for something else: nothing to say of it.
    let (other, _) = prepared_publish("focus-timer", "h2");
    let (yes, items) = asker(true);
    let (out, asked) = run_asking(&other.replace("hash = \"h2\"", "hash = \"h1\""), made.clone(), yes);
    assert_eq!((out.as_str(), asked.len(), items.borrow().len()), ("err:changed", 0, 0));
}

/// Another's template starting while the app's own flow has a token on its
/// way: the window it draws never gets it.
#[test]
fn a_token_made_as_another_s_template_starts_isn_t_shown_in_the_main_window() {
    preview::another_s_ran_for_test(false);
    let asked: Asked = Rc::default();
    let path = script(r#"ui.call("plugins.store.create_token", { name = "CI", scope = { all = true } })"#);
    let a = asked.clone();
    let mut app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, path.clone());
        let engine: Engine = Rc::new(move |method, params, device| {
            a.borrow_mut().push((method.to_string(), params, device));
            // (It starts as the engine answers.)
            preview::another_s_ran_for_test(true);
            Box::pin(async { Ok(json!({ "ok": true, "token": { "id": "t" }, "secret": "zdt_secret" })) })
        });
        install_with(&host, engine, Rc::new(|_| Box::pin(async { Err("cancelled".to_string()) })));
        host
    });
    app.frames(3);
    app.click("role:button \"go\"");
    assert!(app.wait_until(|app| !app.text("out").is_empty()));
    // (And revoked: it's nobody's.)
    assert_eq!(app.text("out"), "err:cancelled");
    assert_eq!(asked.borrow().iter().map(|a| (a.0.clone(), a.1["id"].clone())).next_back().unwrap(), ("RevokeDeployToken".into(), json!("t")));
}

/// A template's calls are the engine's with its kind, its own publish pair,
/// and neither kind is ever answered with the other's.
#[test]
fn a_template_s_calls_say_what_they_re_for() {
    for (call, method, params) in [
        (r#"ui.call("templates.store.search", { q = "dark", verified = true })"#, "StoreSearch", json!({ "q": "dark", "verified": true, "kind": "template" })),
        (r#"ui.call("templates.store.detail", "midnight")"#, "StorePlugin", json!({ "id": "midnight", "kind": "template" })),
        (r#"ui.call("templates.store.versions", { id = "midnight" })"#, "StoreVersions", json!({ "id": "midnight", "kind": "template" })),
        (r#"ui.call("templates.store.version", { id = "midnight", version = "1.0.0" })"#, "StoreVersion", json!({ "id": "midnight", "version": "1.0.0", "kind": "template" })),
        (r#"ui.call("templates.store.mine", nil)"#, "StoreMyPlugins", json!({ "kind": "template" })),
        (r#"ui.call("templates.store.version_action", { id = "midnight", version = "1.0.0", action = "yank" })"#, "StoreVersionAction", json!({ "id": "midnight", "version": "1.0.0", "action": "yank", "kind": "template" })),
        (r#"ui.call("templates.store.updates", { id = "midnight", prefer = "verified" })"#, "SetPluginUpdates", json!({ "id": "midnight", "prefer": "verified", "kind": "template" })),
        (r#"ui.call("templates.store.report", { id = "midnight", reason = "malware" })"#, "StoreReport", json!({ "id": "midnight", "reason": "malware", "kind": "template" })),
        (r#"ui.call("templates.store.prepare", "midnight")"#, "PrepareTemplatePublish", json!({ "id": "midnight" })),
    ] {
        let (out, asked) = run(call, json!({ "ok": true, "kind": "template" }));
        assert!(out.starts_with("ok:"), "{call}: {out}");
        assert_eq!((asked[0].0.as_str(), &asked[0].1), (method, &params), "{call}");
    }
    for (call, kind) in [("plugins.store.detail", "template"), ("templates.store.detail", "plugin")] {
        let (out, _) = run(&format!(r#"ui.call("{call}", "midnight")"#), json!({ "id": "midnight", "kind": kind }));
        assert_eq!(out, "err:not_in_store", "{call}");
    }
    let ids = |kind: super::Kind| {
        let reply = json!({ "items": [{ "id": "midnight", "kind": "template" }, { "id": "focus", "kind": "plugin" }, { "id": "old" }] });
        let Some(zgx_script::Value::List(items)) = kind.all(reply).unwrap().field("items").cloned() else { panic!("no items") };
        items.iter().map(|item| crate::auth::text(item.field("id").unwrap())).collect::<Vec<_>>()
    };
    assert_eq!(ids(super::TEMPLATES), ["midnight"]);
    assert_eq!(ids(super::PLUGINS), ["focus", "old"], "a store that says no kind has plugins alone");
    // A handle of one kind publishes nothing of the other's, and a template's
    // goes out by its own call.
    let shown = keep_prepared(json!({ "id": "midnight", "version": "1.0.0", "hash": "h", "ticket": "ticket-real" }), "template");
    let call = |calls: &str| format!(r#"ui.call("{calls}.publish", {{ id = "midnight", hash = "h", ticket = "{}" }})"#, shown["ticket"].as_str().unwrap());
    let (out, asked) = run(&call("plugins.store"), json!({ "ok": true }));
    assert_eq!((out.as_str(), asked.len()), ("err:changed", 0));
    let (_, asked) = run(&call("templates.store"), json!({ "ok": true }));
    assert_eq!((asked[0].0.as_str(), &asked[0].1), ("PublishTemplate", &json!({ "id": "midnight", "hash": "h", "ticket": "ticket-real" })));
    // Asked in Zeron's window, as a template's.
    preview::another_s_ran_for_test(true);
    let (no, items) = asker(false);
    run_asking(r#"ui.call("templates.store.version_action", { id = "midnight", version = "1.0.0", action = "unyank" })"#, json!({}), no);
    assert_eq!(items.borrow()[0]["kind"], "template");
    preview::another_s_ran_for_test(false);
}
