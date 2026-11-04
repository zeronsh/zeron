//! The example plugins (plugins-dev/) in Zeron's real shell, headless: the
//! shell fixture (ui/shell_fixture.luau, sessions made up, no engine) with
//! each example loaded from its folder as the bridge loads a snapshot,
//! granted what its manifest asks, and its records made from the shell's
//! own sessions as the bridge makes them.

use std::path::{Path, PathBuf};

use serde_json::json;
use zgx_script::{ScriptHost, testing::TestApp};

use super::{Item, expose_own, expose_surface, json_value, records, value_json};

fn example(id: &str) -> Item {
    from(Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins-dev").join(id), id)
}

/// A plugin of Zeron's own (plugins-builtin/).
pub(super) fn builtin(id: &str) -> Item {
    from(Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins-builtin").join(id), id)
}

fn from(dir: PathBuf, id: &str) -> Item {
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("zeron-plugin.json")).unwrap()).unwrap();
    Item {
        json: json!({
            "id": id, "name": manifest["name"], "status": "on", "entry": manifest["entry"],
            "snapshot": dir, "permissions": manifest["permissions"],
            "ui": manifest.get("ui").cloned().unwrap_or(json!({})),
        }),
    }
}

pub(super) type Heard = std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>>;

/// GitHub as a test plays it, for the GitHub plugin (which asks it through
/// gh, `plugin.github.run`): its pull request (`pr`, in the shape the
/// viewer reads, made GitHub's GraphQL here), `diff`, each branch's pull
/// request (`branches`, GraphQL's shape), the board's pages (`list`), who
/// you are (`viewer`), the repository a folder is (`repository`), and `fail`
/// to say no to everything. Changed as a test goes.
pub(super) type World = std::rc::Rc<std::cell::RefCell<serde_json::Value>>;

pub(super) fn world(value: serde_json::Value) -> World {
    std::rc::Rc::new(std::cell::RefCell::new(value))
}

/// The viewer's shape of a pull request, as GitHub's GraphQL gives one.
fn graphql_pr(d: &serde_json::Value) -> serde_json::Value {
    let nodes = |v: &serde_json::Value| json!({ "nodes": v.as_array().cloned().unwrap_or_default() });
    let checks: Vec<serde_json::Value> = d["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| json!({ "__typename": "CheckRun", "name": c["name"], "status": c["status"], "conclusion": c["conclusion"], "detailsUrl": c["detailsUrl"] }))
        .collect();
    let state = match d["ci"]["state"].as_str() {
        Some("failed") => "FAILURE",
        Some("pending") => "PENDING",
        _ => "SUCCESS",
    };
    let rollup = if checks.is_empty() { serde_json::Value::Null } else { json!({ "state": state, "contexts": { "nodes": checks } }) };
    let threads: Vec<serde_json::Value> = d["reviewThreads"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let mut t = t.clone();
            t["comments"] = nodes(&t["comments"]);
            t
        })
        .collect();
    let mut out = d.clone();
    out["id"] = json!("PR_1");
    out["files"] = nodes(&d["files"]);
    out["comments"] = nodes(&d["comments"]);
    out["reviews"] = nodes(&d["reviews"]);
    out["reviewThreads"] = json!({ "nodes": threads });
    out["commits"] = json!({ "nodes": [{ "commit": { "statusCheckRollup": rollup } }] });
    out
}

/// What gh prints for `asked` (`{ program, args, stdin }`) in `world`, or
/// None: it never answers (a body with "HELD"); a body with "REFUSED" is
/// one GitHub says no to.
fn gh_answer(world: &serde_json::Value, asked: &serde_json::Value) -> Option<serde_json::Value> {
    let args: Vec<String> = asked["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
    let stdin = asked["stdin"].as_str().unwrap_or_default();
    let out = |v: serde_json::Value| Some(json!({ "status": 0, "stdout": v.to_string(), "stderr": "" }));
    let text = |t: String| Some(json!({ "status": 0, "stdout": t, "stderr": "" }));
    let fail = |why: &str| Some(json!({ "status": 1, "stdout": "", "stderr": why }));
    let viewer = world["viewer"].clone();
    if stdin.contains("HELD") || world["hold"] == true {
        return None;
    }
    if stdin.contains("REFUSED") {
        return fail("gh: GitHub said no (HTTP 422)");
    }
    if let Some(why) = world["fail"].as_str() {
        return fail(why);
    }
    // A folder's remotes (`remotes`, by folder): none, no repository.
    if asked["program"] == "git" && args == ["remote", "-v"] {
        let cwd = asked["cwd"].as_str().unwrap_or_default();
        return match world["remotes"][cwd].as_str() {
            Some(listing) => text(listing.to_string()),
            None => fail("fatal: not a git repository (or any of the parent directories): .git"),
        };
    }
    if args.first().map(String::as_str) == Some("repo") {
        // (gh not knowing the remotes' host: a proxy's, say.)
        if world["gh_unknown_host"] == true {
            return fail("none of the git remotes configured for this repository point to a known GitHub host");
        }
        // (At scale, each folder its own: /work/r7 is o/r7.)
        if world["repos_by_cwd"] == true {
            let cwd = asked["cwd"].as_str().unwrap_or_default();
            return match cwd.rsplit('/').next().filter(|n| n.starts_with('r')) {
                Some(name) => text(format!("o/{name}\n")),
                None => fail("no git remotes found"),
            };
        }
        return text(format!("{}\n", world["repository"].as_str().unwrap_or("zeronsh/zgx")));
    }
    if args.iter().any(|a| a.contains("v3.diff")) {
        let diff = world["diff"].as_str().unwrap_or_default().to_string();
        // (`-i`: GitHub's status and headers first; its ETag, and a 304
        // for one asked again with it unchanged.)
        if args.iter().any(|a| a == "-i") {
            let etag = world["diff_etag"].as_str().unwrap_or_default();
            if !etag.is_empty() && args.iter().any(|a| a == &format!("If-None-Match: {etag}")) {
                return text(format!("HTTP/2.0 304 Not Modified\r\nEtag: {etag}\r\n\r\n"));
            }
            let tag = if etag.is_empty() { String::new() } else { format!("Etag: {etag}\r\n") };
            return text(format!("HTTP/2.0 200 OK\r\nContent-Type: text/plain\r\n{tag}\r\n{diff}"));
        }
        return text(diff);
    }
    if args.get(1).map(String::as_str) == Some("graphql") {
        let body: serde_json::Value = serde_json::from_str(stdin).unwrap_or_default();
        let (query, vars) = (body["query"].as_str().unwrap_or_default(), &body["variables"]);
        if query.contains("pullRequest(number") {
            // (The repository's merge settings, and what you may do there.)
            let mut repository = world["repo"].as_object().cloned().unwrap_or_default();
            repository.insert("pullRequest".into(), graphql_pr(&world["pr"]));
            return out(json!({ "data": { "viewer": { "login": viewer }, "repository": repository } }));
        }
        if query.contains("associatedPullRequests") {
            // (The Git plugin's History: each commit's author and pull
            // request, by alias, from `commits`.)
            let mut repository = serde_json::Map::new();
            for part in query.split("object(oid: \"").skip(1) {
                let sha = part.split('"').next().unwrap_or_default();
                let before = query.split(&format!("object(oid: \"{sha}\"")).next().unwrap_or_default();
                let alias = before.rsplit(' ').find(|w| w.ends_with(':')).unwrap_or_default().trim_end_matches(':').to_string();
                let commit = &world["commits"][sha];
                repository.insert(alias, if commit.is_null() { json!(null) } else { commit.clone() });
            }
            return out(json!({ "data": { "repository": repository } }));
        }
        if query.contains("mergePullRequest") {
            return out(json!({ "data": { "mergePullRequest": { "pullRequest": { "merged": true } } } }));
        }
        // Many repositories at once: `r<n>` for repository `o<n>/n<n>`, its
        // branches `b<n>_<k>`; and the quota, as `rateLimit` says it.
        if query.contains("pullRequests(headRefName") && vars.get("o1").is_some() {
            let mut data = serde_json::Map::new();
            let vars = vars.as_object().cloned().unwrap_or_default();
            for (k, branch) in &vars {
                let Some((repo, _)) = k.strip_prefix('b').and_then(|rest| rest.split_once('_')) else { continue };
                let alias = format!("r{repo}");
                let branch = branch.as_str().unwrap_or_default();
                // (`by_repo`: each repository's own branches, `owner/name`
                // to branch to pull request; else `branches`, any's.)
                let pr = if world["by_repo"].is_object() {
                    let name = format!("{}/{}", vars[&format!("o{repo}")].as_str().unwrap_or_default(), vars[&format!("n{repo}")].as_str().unwrap_or_default());
                    &world["by_repo"][name.as_str()][branch]
                } else {
                    &world["branches"][branch]
                };
                let entry = data.entry(alias).or_insert_with(|| json!({}));
                entry[k.as_str()] = json!({ "nodes": if pr.is_null() { json!([]) } else { json!([pr]) } });
            }
            data.insert("viewer".into(), json!({ "login": viewer }));
            if query.contains("rateLimit") {
                let remaining = world["rate_remaining"].as_u64().unwrap_or(4900);
                data.insert("rateLimit".into(), json!({ "cost": 1, "remaining": remaining, "limit": 5000, "resetAt": world["rate_reset"].as_str().unwrap_or("2099-01-01T00:00:00Z") }));
            }
            return out(json!({ "data": data }));
        }
        if query.contains("pullRequests(headRefName") {
            let mut repository = serde_json::Map::new();
            for (k, branch) in vars.as_object().into_iter().flatten() {
                if k.starts_with('b') {
                    let pr = &world["branches"][branch.as_str().unwrap_or_default()];
                    repository.insert(k.clone(), json!({ "nodes": if pr.is_null() { json!([]) } else { json!([pr]) } }));
                }
            }
            return out(json!({ "data": { "viewer": { "login": viewer }, "repository": repository } }));
        }
        if query.contains("search(") {
            let q = vars["q"].as_str().unwrap_or_default();
            if let Some(rest) = q.split("head:").nth(1) {
                let branch = rest.split_whitespace().next().unwrap_or_default();
                let repo = world["repository"].as_str().unwrap_or("zeronsh/zgx");
                let pr = &world["branches"][branch];
                let found = if pr.is_null() {
                    json!([])
                } else {
                    let mut pr = pr.clone();
                    pr["repository"] = json!({ "nameWithOwner": repo, "name": repo.split('/').nth(1).unwrap_or_default() });
                    json!([pr])
                };
                return out(json!({ "data": { "viewer": { "login": viewer }, "search": { "nodes": found } } }));
            }
            let page = if vars["after"].as_str() == Some("c2") {
                &world["list"]["more"]
            } else if q.contains("review-requested:@me") {
                &world["list"]["reviewing"]
            } else {
                &world["list"]["authored"]
            };
            return out(json!({ "data": { "search": page } }));
        }
        if query.contains("addPullRequestReviewThreadReply") {
            return out(json!({ "data": { "addPullRequestReviewThreadReply": { "comment": { "id": "C_reply", "author": { "login": viewer } } } } }));
        }
        return out(json!({ "data": { "done": true } }));
    }
    // A REST write (a comment): GitHub's answer, as you.
    out(json!({ "id": 1, "user": { "login": viewer } }))
}

/// GitHub played by `world` for plugin `id` (its `run`), each run heard as
/// `("run", what it asked)`, and its database a real one of the test's own.
fn fake_github(host: &ScriptHost, id: &str, world: World, heard: Heard) {
    let (w, h) = (world, heard);
    host.async_action(format!("plugin.{id}.run"), move |asked: zgx_script::Value| {
        let json: serde_json::Value = serde_json::from_str(&asked.to_json()).unwrap_or_default();
        h.borrow_mut().push(("run".into(), asked.to_json()));
        let answer = gh_answer(&w.borrow(), &json);
        async move {
            match answer {
                Some(answer) => Ok(json_value(&answer)),
                None => {
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            }
        }
    });
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("zgx-gh-test-db-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = super::db::path_in(&dir, id).unwrap();
    host.async_action(format!("plugin.{id}.db"), move |asked: zgx_script::Value| {
        let json: serde_json::Value = serde_json::from_str(&asked.to_json()).unwrap_or_default();
        let answer = super::db::call(&path, &json);
        async move { answer.map(|v| json_value(&v)) }
    });
}

/// What the plugin ran, as asked (`{ program, args, stdin }`), in order.
fn runs(heard: &Heard) -> Vec<serde_json::Value> {
    heard.borrow().iter().filter(|(k, _)| k == "run").map(|(_, v)| serde_json::from_str(v).unwrap()).collect()
}

/// Whether `run` is gh with these words among its args or body.
fn ran(run: &serde_json::Value, words: &[&str]) -> bool {
    let all = format!("{} {}", run["args"], run["stdin"].as_str().unwrap_or_default());
    run["program"] == "gh" && words.iter().all(|w| all.contains(w))
}

/// The shell with `items` loaded.
fn shell(items: Vec<Item>) -> TestApp {
    shell_hearing(items).0
}

/// The shell with `items` loaded, and what their own actions heard.
fn shell_hearing(items: Vec<Item>) -> (TestApp, Heard) {
    shell_with(items, world(json!({ "viewer": "ada" })))
}

/// `shell_hearing`, GitHub played by `world`.
pub(super) fn shell_with(items: Vec<Item>, github: World) -> (TestApp, Heard) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let heard: Heard = Heard::default();
    let log = heard.clone();
    let app = TestApp::new(move |runtime| {
        let heard = log.clone();
        let host = super::lend_design_system(ScriptHost::new(runtime, path.clone()), &path);
        host.shared("zgx.window.size", zgx_script::window_size_value(1200., 800.));
        offline(&host);
        {
            let heard = heard.clone();
            host.action("zeron.open", move |v: zgx_script::Value| heard.borrow_mut().push(("zeron.open".into(), v.to_json())));
        }
        host.action("close", |_| {});
        expose_surface(&host);
        let mut plugins = Vec::new();
        for item in &items {
            let id = item.id().to_string();
            expose_own(&host, &id);
            // (Its own words, as the bridge publishes them.)
            super::locales::publish(&host, &id, Path::new(item.json["snapshot"].as_str().unwrap()));
            for name in ["result", "ack", "menus", "toast", "problem", "open_tab", "add_page", "github_watch", "open_url", "setting_status", "cost"] {
                let (heard, key) = (heard.clone(), format!("plugin.{id}.{name}"));
                host.action(key.clone(), move |v: zgx_script::Value| heard.borrow_mut().push((key.clone(), v.to_json())));
            }
            {
                // (Storage as the host keeps it: written, then published.)
                let store = host.shared(&format!("plugin.{id}.store"), json_value(&serde_json::json!({})));
                host.action(format!("plugin.{id}.store_set"), move |v: zgx_script::Value| {
                    let json: serde_json::Value = serde_json::from_str(&v.to_json()).unwrap();
                    let mut now: serde_json::Value = serde_json::from_str(&store.signal().with_untracked(zgx_script::Value::clone).to_json()).unwrap_or_default();
                    if !now.is_object() {
                        now = serde_json::json!({});
                    }
                    now[json["key"].as_str().unwrap()] = json["value"].clone();
                    store.set(json_value(&now));
                });
            }
            {
                let (heard, key) = (heard.clone(), format!("plugin.{id}.notify"));
                host.async_action(key.clone(), move |v: zgx_script::Value| {
                    heard.borrow_mut().push((key.clone(), v.to_json()));
                    async move { Ok(zgx_script::Value::from("posted")) }
                });
            }
            fake_github(&host, &id, github.clone(), heard.clone());
            plugins.extend(item.plugin());
        }
        host.set_plugins(plugins);
        host
    });
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    (app, heard)
}

/// Each plugin's records, from the shell's sessions (as the bridge's).
fn publish_records(app: &TestApp, items: &[Item]) {
    let sources = records::Sources {
        sessions: value_json(&app.host.shared("sessions", zgx_script::Value::Nil).signal().with_untracked(zgx_script::Value::clone)),
        ..Default::default()
    };
    for item in items {
        let value = records::records_with(&item.permissions(), &item.change(), &sources);
        app.host.shared(&format!("plugin.{}.records", item.id()), zgx_script::Value::Nil).set(json_value(&value));
    }
    app.frames(10);
}

#[test]
fn session_status_says_what_the_open_session_is_doing() {
    let items = vec![example("session-status")];
    let mut app = shell(items.clone());
    assert_eq!(app.error(), None, "the shell with the example failed");
    publish_records(&app, &items);
    // No session open: nothing in a header that isn't there.
    assert!(app.find("session-status").is_none());
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    assert_eq!(app.text("session-status"), "Working", "{}", app.tree());
    // It finishes: the word follows.
    app.host.run_command("finish");
    app.frames(10);
    publish_records(&app, &items);
    // (A finished session has no status left to say.)
    assert_eq!(app.text("session-status"), "", "it follows its session's record");
    // Another session: its own.
    app.click("#shell-sidebar >> role:button \"Ship the release\"");
    app.frames(30);
    assert_eq!(app.text("session-status"), "Waiting for you");
}

#[test]
fn focus_shows_only_what_s_working_or_needs_you() {
    let items = vec![example("focus")];
    let mut app = shell(items.clone());
    assert_eq!(app.error(), None, "the shell with the example failed");
    assert!(app.find("focus-toggle").is_some(), "its button is in the title bar: {}", app.tree());
    app.click("focus-toggle");
    app.frames(30);
    let filter = app.host.shared("sidebar.filter", zgx_script::Value::Nil).signal().get();
    assert!(filter.to_json().contains("Focus"), "{}", filter.to_json());
    app.click("focus-toggle");
    app.frames(30);
    assert_eq!(app.host.shared("sidebar.filter", zgx_script::Value::Nil).signal().get(), zgx_script::Value::Nil);
}


/// A branch's pull request as GitHub's GraphQL gives it (the badges'
/// fields), `extra` laid over it.
fn branch_pr(number: u64, state: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut pr = json!({
        "number": number, "title": "Plugins: class targets", "url": format!("https://github.com/zeronsh/zgx/pull/{number}"),
        "state": state, "isDraft": false, "reviewDecision": null, "baseRefName": "main", "headRefName": "fix/build",
        "reviewRequests": { "nodes": [] }, "commits": { "nodes": [] },
    });
    for (k, v) in extra.as_object().into_iter().flatten() {
        pr[k] = v.clone();
    }
    pr
}

/// The badges asked again (the plugin's command: GitHub › Refresh pull
/// requests), and the window given until `done`.
fn refresh_until(app: &mut TestApp, done: impl Fn(&mut TestApp) -> bool) -> bool {
    assert!(app.host.run_command("github/refresh"), "no refresh command");
    app.wait_until(done)
}

#[test]
fn github_shows_each_session_s_pull_request_beside_its_title() {
    let items = vec![builtin("github")];
    let github = world(json!({ "viewer": "ada", "repository": "zeronsh/zgx", "branches": {} }));
    let (mut app, heard) = shell_with(items.clone(), github.clone());
    assert_eq!(app.error(), None, "the shell with GitHub failed");
    publish_records(&app, &items);
    app.frames(10);
    // It asks gh which repository the session's project folder is, then
    // GitHub for its branch's pull request: none yet, no badge.
    assert!(app.wait_until(|_| runs(&heard).iter().any(|r| ran(r, &["repo", "view"]) && r["cwd"] == "/work/atlas")), "{:?}", runs(&heard));
    assert!(app.wait_until(|_| runs(&heard).iter().any(|r| ran(r, &["pullRequests(headRefName", "fix/build"]))), "{:?}", runs(&heard));
    assert!(!app.target(".github-pr-badge").is_ok());
    // GitHub has one: its badge, in a1's row, with its number.
    github.borrow_mut()["branches"]["fix/build"] = branch_pr(1264, "OPEN", json!({}));
    assert!(refresh_until(&mut app, |app| app.target("#shell-sidebar >> .session-row >> .github-pr-badge").is_ok()), "{}", app.tree());
    assert!(app.tree().contains("\"1264\""), "{}", app.tree());
    // Its state and title are the session card's to say: resting on the
    // row shows them there, under the branch; the badge has no tooltip.
    app.hover(".github-pr-badge");
    assert!(app.wait_until(|app| app.find(".session-card >> .github-pr-card-row").is_some()), "no pull request in the session card: {}", app.tree());
    // (The sidebar's own badge there too: its number, its state's glyph.)
    assert!(app.find(".session-card >> .github-pr-card-row >> \"1264\"").is_some(), "{}", app.tree());
    // A click: its session, then the pull request in that chat's panel,
    // named by its title.
    app.click(".github-pr-badge");
    app.frames(5);
    {
        let heard = heard.borrow();
        assert!(heard.iter().any(|(k, v)| k == "zeron.open" && v.contains("\"a1\"")), "{heard:?}");
        let (_, tab) = heard.iter().find(|(k, _)| k == "plugin.github.open_tab").expect("a tab asked for");
        let tab: serde_json::Value = serde_json::from_str(tab).unwrap();
        assert_eq!((tab["session"].as_str(), tab["kind"].as_str(), tab["data"]["number"].as_f64()), (Some("a1"), Some("github-pr"), Some(1264.)));
        assert_eq!(tab["title"].as_str(), Some("Plugins: class targets"));
        // (As the host takes it: a kind of letters, digits, - and _.)
        assert!(super::surfaces::tab(&tab).is_ok(), "the host refuses it: {tab}");
    }
    // As the pull request moves, each worth saying once: a review asked of
    // you, changes requested, failing checks.
    let notes = |heard: &Heard| heard.borrow().iter().filter(|(k, _)| k == "plugin.github.notify").map(|(_, v)| v.clone()).collect::<Vec<String>>();
    let rollup = |state: &str| json!({ "nodes": [{ "commit": { "statusCheckRollup": { "state": state } } }] });
    let asked_of_you = json!({ "nodes": [{ "requestedReviewer": { "login": "ada" } }] });
    for (n, step) in [
        json!({ "reviewDecision": "REVIEW_REQUIRED", "reviewRequests": asked_of_you, "commits": rollup("PENDING") }),
        json!({ "reviewDecision": "CHANGES_REQUESTED", "commits": rollup("PENDING") }),
        json!({ "reviewDecision": "CHANGES_REQUESTED", "commits": rollup("FAILURE") }),
    ]
    .into_iter()
    .enumerate()
    {
        github.borrow_mut()["branches"]["fix/build"] = branch_pr(1264, "OPEN", step);
        assert!(refresh_until(&mut app, |_| notes(&heard).len() == n + 1), "{:?}", notes(&heard));
    }
    // Nothing new: nothing said.
    assert!(app.host.run_command("github/refresh"));
    app.frames(30);
    assert_eq!(notes(&heard).len(), 3);
    // Merged: the same badge, and a notification. One first seen already
    // merged (a2's branch) is only noted.
    github.borrow_mut()["branches"]["fix/build"] = branch_pr(1264, "MERGED", json!({}));
    github.borrow_mut()["branches"]["docs"] = branch_pr(7, "MERGED", json!({ "title": "Old", "headRefName": "docs" }));
    assert!(refresh_until(&mut app, |_| notes(&heard).len() == 4), "{:?}", notes(&heard));
    app.frames(10);
    assert!(app.tree().contains("\"1264\""));
    let notes = notes(&heard);
    assert_eq!(notes.len(), 4, "{notes:?}");
    for (note, said) in notes.iter().zip(["is waiting for your review", "has changes requested", "has failing checks", "merged"]) {
        assert!(note.contains(&format!("Pull request #1264 {said}")), "{said}: {note}");
        assert!(note.contains("\"session\":\"a1\"") && note.contains("Fix the build"), "{note}");
    }
}

/// GitHub's watch at scale, measured (ignored: `cargo test --release
/// github_scale -- --ignored --nocapture`; `GITHUB_SCALE_JSON=<path>` writes
/// the numbers): 500 repositories on this Mac, 1,500 sessions in them
/// (ui/shell_fixture's `scale`), gh played by a stub that counts what it's
/// asked. Simulated time (frames at 60 a second): gh runs a minute idle and
/// with one session working, and how long a visible session's new pull
/// request takes to show. CPU: the window's time per simulated minute with
/// the plugin, less the shell's own without it.
#[test]
#[ignore]
fn github_scale() {
    const MINUTE: usize = 60 * 60;
    let kinds = |heard: &Heard, from: usize| -> serde_json::Value {
        let all = runs(heard);
        let new = &all[from.min(all.len())..];
        let arg = |r: &serde_json::Value, i: usize| r["args"][i].as_str().unwrap_or_default().to_string();
        let gh: Vec<&serde_json::Value> = new.iter().filter(|r| r["program"] == "gh").collect();
        let graphql: Vec<&&serde_json::Value> = gh.iter().filter(|r| arg(r, 1) == "graphql").collect();
        json!({
            "gh": gh.len(),
            "git": new.iter().filter(|r| r["program"] == "git").count(),
            "repo_view": gh.iter().filter(|r| arg(r, 0) == "repo").count(),
            "graphql": graphql.len(),
            "search": graphql.iter().filter(|r| r["stdin"].as_str().unwrap_or_default().contains("search(")).count(),
            "graphql_repositories": graphql.iter().map(|r| r["stdin"].as_str().unwrap_or_default().matches("repository(owner").count()).sum::<usize>(),
        })
    };
    // (`GITHUB_SCALE_REAL_GAP`: seconds of real time after each simulated
    // minute, for code that paces itself by the wall clock.)
    let gap = std::env::var("GITHUB_SCALE_REAL_GAP").ok().and_then(|g| g.parse::<f64>().ok()).unwrap_or(0.);
    let span = |app: &mut TestApp, minutes: usize| -> f64 {
        let mut busy = 0.;
        for _ in 0..minutes {
            let t = std::time::Instant::now();
            app.frames(MINUTE);
            busy += t.elapsed().as_secs_f64();
            if gap > 0. {
                std::thread::sleep(std::time::Duration::from_secs_f64(gap));
            }
        }
        busy * 1000. / minutes as f64
    };
    // The shell alone, as a floor.
    let shell_ms = {
        let (mut app, _) = shell_with(vec![], world(json!({ "viewer": "ada" })));
        assert!(app.host.run_command("scale"));
        app.frames(60);
        span(&mut app, 2)
    };
    let items = vec![builtin("github")];
    let github = world(json!({ "viewer": "ada", "repos_by_cwd": true, "branches": {} }));
    let (mut app, heard) = shell_with(items.clone(), github.clone());
    assert!(app.host.run_command("scale"));
    app.frames(10);
    publish_records(&app, &items);
    // The first five minutes: each repository found, then asked.
    let start_ms = span(&mut app, 5);
    let start = kinds(&heard, 0);
    // Ten idle minutes.
    let mark = runs(&heard).len();
    let idle_ms = span(&mut app, 10);
    let idle = kinds(&heard, mark);
    // Ten minutes with an old session working.
    assert!(app.host.run_command("scale_work"));
    app.frames(10);
    publish_records(&app, &items);
    let mark = runs(&heard).len();
    let active_ms = span(&mut app, 10);
    let active = kinds(&heard, mark);
    // A pull request opened for a session on screen: seconds until its
    // badge shows.
    let shown = |app: &mut TestApp| app.texts().iter().any(|t| t == "4242");
    github.borrow_mut()["branches"]["r1-1"] = branch_pr(4242, "OPEN", json!({ "headRefName": "r1-1" }));
    let mut frames = 0;
    while !shown(&mut app) && frames < 10 * MINUTE {
        app.frames(30);
        frames += 30;
        if gap > 0. && frames % MINUTE == 0 {
            std::thread::sleep(std::time::Duration::from_secs_f64(gap));
        }
    }
    let latency = if shown(&mut app) { json!(frames as f64 / 60.) } else { json!(null) };
    let per_min = |v: &serde_json::Value, minutes: f64| v["gh"].as_f64().unwrap_or(0.) / minutes;
    let numbers = json!({
        "start_5min": start, "idle_10min": idle, "active_10min": active,
        "gh_per_min_idle": per_min(&idle, 10.), "gh_per_min_active": per_min(&active, 10.),
        "visible_pr_latency_s": latency,
        "window_ms_per_sim_minute": { "shell_only": shell_ms, "start": start_ms, "idle": idle_ms, "active": active_ms },
        "plugin_ms_per_minute_idle": idle_ms - shell_ms, "plugin_ms_per_minute_active": active_ms - shell_ms,
    });
    println!("{}", serde_json::to_string_pretty(&numbers).unwrap());
    if let Ok(path) = std::env::var("GITHUB_SCALE_JSON") {
        std::fs::write(path, numbers.to_string()).unwrap();
    }
    assert_eq!(app.error(), None);
}

/// The repositories a GraphQL ask covered (`o/r7` …), and its branches.
fn asked_repositories(run: &serde_json::Value) -> (Vec<String>, usize) {
    let body: serde_json::Value = serde_json::from_str(run["stdin"].as_str().unwrap_or_default()).unwrap_or_default();
    let vars = body["variables"].as_object().cloned().unwrap_or_default();
    let mut repos: Vec<String> = vars
        .iter()
        .filter(|(k, _)| k.starts_with('o') && k[1..].chars().all(|c| c.is_ascii_digit()))
        .map(|(k, owner)| format!("{}/{}", owner.as_str().unwrap_or_default(), vars[&format!("n{}", &k[1..])].as_str().unwrap_or_default()))
        .collect();
    repos.sort();
    let branches = vars.keys().filter(|k| k.starts_with('b') && k.contains('_')).count();
    (repos, branches)
}

/// The shell at scale (500 repositories, 1,500 sessions), GitHub played by
/// `world`, once every recent repository is found.
fn github_at_scale(world_json: serde_json::Value) -> (TestApp, Heard, World) {
    let items = vec![builtin("github")];
    let github = world(world_json);
    let (mut app, heard) = shell_with(items.clone(), github.clone());
    assert!(app.host.run_command("scale"));
    app.frames(10);
    publish_records(&app, &items);
    // (Each recent session's folder looked up, a few at a time.)
    app.frames(60 * 60);
    (app, heard, github)
}

/// How many GraphQL asks since `from` covered `repo`.
fn asks_of(heard: &Heard, from: usize, repo: &str) -> usize {
    runs(heard)[from..].iter().filter(|r| r["args"][1] == "graphql" && asked_repositories(r).0.iter().any(|x| x == repo)).count()
}

#[test]
fn github_asks_many_repositories_at_once_and_only_as_often_as_they_matter() {
    let (mut app, heard, _) = github_at_scale(json!({ "viewer": "ada", "repos_by_cwd": true, "branches": {} }));
    assert_eq!(app.error(), None);
    let all = runs(&heard);
    // Old sessions out of sight aren't looked up or asked about: r168 on.
    let old: Vec<String> = all.iter().filter_map(|r| r["cwd"].as_str()).filter(|c| c.trim_start_matches("/work/r").parse::<usize>().is_ok_and(|n| n > 167)).map(str::to_string).collect();
    assert!(old.is_empty(), "old sessions' folders were looked up: {old:?}\n{:?}", app.texts().iter().filter(|t| t.starts_with("Session") || t.starts_with("Repo")).collect::<Vec<_>>());
    let graphql: Vec<&serde_json::Value> = all.iter().filter(|r| r["args"][1] == "graphql").collect();
    // Batches: 50 repositories and 150 branches at most each, and every
    // recent repository asked.
    let mut covered = std::collections::BTreeSet::new();
    for r in &graphql {
        let (repos, branches) = asked_repositories(r);
        assert!(repos.len() <= 50 && branches <= 150, "{} repositories, {branches} branches in one ask", repos.len());
        covered.extend(repos);
    }
    assert!(covered.contains("o/r1") && covered.contains("o/r167"), "{covered:?}");
    assert!(!covered.contains("o/r168") && !covered.contains("o/r300"), "an old repository was asked");
    assert!(graphql.len() < 20, "{} asks for 167 repositories", graphql.len());
    // Ten minutes: what's on screen every half minute; the rest of the
    // recent ones, at most once; the old ones never.
    let mark = runs(&heard).len();
    app.frames(10 * 60 * 60);
    let texts = app.texts();
    let on_screen: Vec<usize> = (1..=500).filter(|i| texts.iter().any(|t| t == &format!("Repo {i}") || t == &format!("Session {}", (i - 1) * 3 + 1))).collect();
    let hot = on_screen.iter().map(|i| asks_of(&heard, mark, &format!("o/r{i}"))).max().unwrap_or(0);
    assert!(hot >= 15, "a repository on screen ({on_screen:?}) was asked {hot} times in ten minutes");
    let quiet = (150..=167).find(|i| !on_screen.contains(i)).unwrap();
    assert!(asks_of(&heard, mark, &format!("o/r{quiet}")) <= 1, "a recent repository out of sight was asked often");
    assert_eq!(asks_of(&heard, mark, "o/r300"), 0);
    // An old one starts working: asked within the half minute, and often.
    assert!(app.host.run_command("scale_work"));
    app.frames(10);
    publish_records(&app, &[builtin("github")]);
    let mark = runs(&heard).len();
    app.frames(31 * 60);
    assert!(app.wait_until(|_| asks_of(&heard, mark, "o/r300") >= 1), "the working session's repository wasn't asked");
    app.frames(3 * 60 * 60);
    assert!(asks_of(&heard, mark, "o/r300") >= 5, "{}", asks_of(&heard, mark, "o/r300"));
    assert_eq!(app.error(), None);
}

/// Wheeling the sidebar down at scale while pull requests arrive: every
/// frame, the rows only move up. (The user saw it jump back a step every
/// few steps.) Returns each frame's backward moves, in points.
fn sidebar_backsteps(steps: usize) -> (Vec<(usize, f32)>, TestApp) {
    let (mut app, _heard, github) = github_at_scale(json!({ "viewer": "ada", "repos_by_cwd": true, "branches": {} }));
    // As rows come into view, GitHub has their pull requests, and the
    // engine (watching what shows) tells them too.
    let engine = app.host.shared("plugin.github.github", json_value(&json!({})));
    // (The host keeps what its watches last said, hundreds of sessions'.)
    let mut told = serde_json::Map::new();
    for n in 1..=500usize {
        let (i, k) = ((n - 1) / 3 + 1, (n - 1) % 3 + 1);
        let number = (i * 10 + k) as u64;
        told.insert(format!("s{n}"), json!({ "number": number, "title": format!("PR {i}-{k}"), "url": format!("https://github.com/o/r{i}/pull/{number}"), "state": "open", "base": "main", "head": format!("r{i}-{k}"), "ci": "pending", "review": "unknown" }));
    }
    engine.set(json_value(&serde_json::Value::Object(told.clone())));
    app.frames(10);
    let mut arrive = |app: &mut TestApp, names: &[(String, f32)]| {
        for (name, _) in names {
            let n: usize = name.trim_start_matches("Session ").parse().unwrap();
            let (i, k) = ((n - 1) / 3 + 1, (n - 1) % 3 + 1);
            let b = format!("r{i}-{k}");
            let number = (i * 10 + k) as u64;
            github.borrow_mut()["branches"][&b] = branch_pr(number, "OPEN", json!({ "headRefName": b, "title": format!("PR {i}-{k}") }));
            told.insert(format!("s{n}"), json!({ "number": number, "title": format!("PR {i}-{k}"), "url": format!("https://github.com/o/r{i}/pull/{number}"), "state": "open", "base": "main", "head": b, "ci": "pending", "review": "unknown" }));
        }
        engine.set(json_value(&serde_json::Value::Object(told.clone())));
        let _ = app;
    };
    let rows = |app: &mut TestApp| -> Vec<(String, f32)> {
        (1..=1500)
            .filter_map(|n| {
                let t = format!("#shell-sidebar >> role:button \"Session {n}\"");
                app.find(&t)?;
                let b = app.bounds(&t);
                (b.y > 0. && b.y < 800.).then(|| (format!("Session {n}"), b.y))
            })
            .collect()
    };
    let mut back = Vec::new();
    let mut frame = 0;
    for _ in 0..steps {
        let before = rows(&mut app);
        arrive(&mut app, &before);
        let before = rows(&mut app);
        let Some((anchor, _)) = before.get(before.len() / 2).cloned() else { break };
        app.scroll(&format!("#shell-sidebar >> role:button \"{anchor}\""), 24.);
        // (Frame to frame: a row lower than it was the frame before moved
        // back; the wheel's own step, too little, is a step not taken.)
        let mut last = before.clone();
        let mut moved = false;
        for _ in 0..12 {
            app.frames(1);
            frame += 1;
            let now = rows(&mut app);
            for (name, y) in &now {
                if let Some((_, was)) = last.iter().find(|(n, _)| n == name) {
                    if *y > *was + 0.5 {
                        back.push((frame, *y - *was));
                    }
                    if *y < *was - 0.5 {
                        moved = true;
                    }
                    break;
                }
            }
            last = now;
        }
        let after = rows(&mut app);
        if let (Some((n, y0)), true) = (before.first(), moved || before.is_empty()) {
            if let Some((_, y1)) = after.iter().find(|(m, _)| m == n) {
                if *y1 > *y0 - 0.5 {
                    back.push((frame, *y1 - *y0));
                }
            }
        }
    }
    (back, app)
}

#[test]
fn the_sidebar_never_scrolls_back_as_pull_requests_arrive() {
    let (back, mut app) = sidebar_backsteps(40);
    // (It went down the list, badges in its rows.)
    assert!(app.texts().iter().any(|t| t == "Session 25"), "{:?}", app.texts());
    assert!(app.texts().iter().filter(|t| t.parse::<u32>().is_ok()).count() >= 10);
    assert!(back.is_empty(), "the sidebar moved back while wheeled down: {back:?}");
    assert_eq!(app.error(), None);
}

/// The GitHub plugin's watch (watch.luau, pulls, gh, the SDK) in a plugin
/// of the test's own, whose text in each sidebar row counts how often it's
/// read: `a1:<number>:<reads>`. (`GITHUB_ISOLATION_FROM`: another
/// watch.luau, and with it the SDK's whole-map `watch_pr`, to see the test
/// fail on the design before.)
fn counting_watch() -> Item {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins-builtin/github");
    let dir = std::env::temp_dir().join(format!("zgx-counting-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    struct At(PathBuf);
    impl At {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    let dir = At(dir);
    for f in ["pulls.luau", "gh.luau", "zeron-plugin.json"] {
        std::fs::copy(src.join(f), dir.path().join(f)).unwrap();
    }
    std::fs::create_dir_all(dir.path().join("locales")).unwrap();
    for f in ["en.json", "zh-Hans.json"] {
        std::fs::copy(src.join("locales").join(f), dir.path().join("locales").join(f)).unwrap();
    }
    let before = std::env::var("GITHUB_ISOLATION_FROM").ok();
    let watch = before.as_ref().map(PathBuf::from).unwrap_or_else(|| src.join("watch.luau"));
    std::fs::copy(watch, dir.path().join("watch.luau")).unwrap();
    let mut sdk = std::fs::read_to_string(src.join("zeron.luau")).unwrap();
    if before.is_some() {
        sdk = sdk.replace("\t\tlocal mine = ui.computed(function(): any\n\t\t\tlocal pr = (github_heard:get() or {})[session]\n\t\t\treturn if pr == false then nil else pr\n\t\tend)\n\t\treturn function(): any\n\t\t\treturn mine:get()\n\t\tend", "\t\treturn function(): any\n\t\t\tlocal pr = (github_heard:get() or {})[session]\n\t\t\treturn if pr == false then nil else pr\n\t\tend");
    }
    std::fs::write(dir.path().join("zeron.luau"), sdk).unwrap();
    std::fs::write(
        dir.path().join("init.luau"),
        r#"local zeron = require("./zeron")("github")
local pulls = require("./pulls")(zeron)
local watch = require("./watch")(zeron, pulls)
local reads: { [string]: number } = {}
zeron.place {
	at = ".session-title",
	after = function(): Node
		local id = zeron.need("session").id
		local pr = watch.pr(id)
		return ui.text {
			class = "pr-reads",
			function(): string
				reads[id] = (reads[id] or 0) + 1
				local now = pr()
				return `{id}:{if now then now.number else "-"}:{reads[id]}`
			end,
		}
	end,
}
"#,
    )
    .unwrap();
    from(dir.path().to_path_buf(), "github")
}

/// Row `id`'s text from the counting watch: its number and its reads.
fn reads_of(app: &mut TestApp, id: &str) -> Option<(String, u32)> {
    let text = app.texts().into_iter().find(|t| t.starts_with(&format!("{id}:")))?;
    let mut parts = text.split(':').skip(1);
    Some((parts.next()?.to_string(), parts.next()?.parse().ok()?))
}

#[test]
fn a_pull_request_moving_tells_only_its_own_row() {
    let item = counting_watch();
    let items = vec![item];
    let github = world(json!({ "viewer": "ada", "repository": "zeronsh/zgx", "branches": {} }));
    let (mut app, _heard) = shell_with(items.clone(), github.clone());
    publish_records(&app, &items);
    assert!(app.wait_until(|app| reads_of(app, "a1").is_some() && reads_of(app, "a2").is_some()), "{:?}", app.texts());
    app.frames(120);
    let (_, a2) = reads_of(&mut app, "a2").unwrap();
    // The engine tells a1's pull request: a1's row reads it; a2's isn't told.
    app.host.shared("plugin.github.github", json_value(&json!({}))).set(json_value(&json!({ "a1": {
        "number": 77, "title": "From the engine", "url": "https://github.com/zeronsh/zgx/pull/77", "state": "open", "base": "main", "head": "fix/build", "ci": "passed", "review": "approved",
    } })));
    assert!(app.wait_until(|app| reads_of(app, "a1").is_some_and(|(n, _)| n == "77")), "{:?}", app.texts());
    app.frames(30);
    assert_eq!(reads_of(&mut app, "a2").unwrap().1, a2, "a2's row was read again for a1's pull request (the engine's)");
    // gh tells a2's: a2's row reads it; a1's isn't told.
    let (_, a1) = reads_of(&mut app, "a1").unwrap();
    github.borrow_mut()["branches"]["docs"] = branch_pr(7, "OPEN", json!({ "headRefName": "docs" }));
    assert!(refresh_until(&mut app, |app| reads_of(app, "a2").is_some_and(|(n, _)| n == "7")), "{:?}", app.texts());
    app.frames(30);
    assert_eq!(reads_of(&mut app, "a1").unwrap().1, a1, "a1's row was read again for a2's pull request (gh's)");
    assert_eq!(app.error(), None);
}

#[test]
fn github_runs_two_gh_at_once_at_most() {
    let (mut app, heard, github) = github_at_scale(json!({ "viewer": "ada", "repos_by_cwd": true, "branches": {} }));
    github.borrow_mut()["hold"] = json!(true);
    let mark = runs(&heard).len();
    assert!(app.host.run_command("github/refresh"));
    app.frames(5 * 60);
    assert_eq!(runs(&heard).len() - mark, 2, "gh runs at once: {}", runs(&heard).len() - mark);
    assert_eq!(app.error(), None);
}

#[test]
fn github_asks_less_as_its_quota_runs_low_and_not_at_all_near_its_end() {
    let (mut app, heard, github) = github_at_scale(json!({ "viewer": "ada", "repos_by_cwd": true, "branches": {} }));
    let count = |from: usize| runs(&heard)[from..].iter().filter(|r| r["args"][1] == "graphql").count();
    let mark = runs(&heard).len();
    app.frames(10 * 60 * 60);
    let full = count(mark);
    // A fifth left: a quarter as often.
    github.borrow_mut()["rate_remaining"] = json!(1000);
    app.frames(60 * 60);
    let mark = runs(&heard).len();
    app.frames(10 * 60 * 60);
    let low = count(mark);
    assert!(low * 2 <= full, "with a fifth of the quota left: {low} asks, {full} with all of it");
    // One percent left, resetting in ten minutes: nothing until it does.
    let reset = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 600;
    let iso = {
        let days = (reset / 86400) as i64;
        let (z, secs) = (days + 719468, reset % 86400);
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
    };
    github.borrow_mut()["rate_remaining"] = json!(40);
    github.borrow_mut()["rate_reset"] = json!(iso);
    app.frames(2 * 60 * 60);
    let mark = runs(&heard).len();
    app.frames(7 * 60 * 60);
    assert_eq!(count(mark), 0, "asked with the quota nearly spent");
    // Reset: asked again.
    github.borrow_mut()["rate_remaining"] = json!(4900);
    app.frames(5 * 60 * 60);
    assert!(count(mark) > 0, "never asked again after the reset");
    assert_eq!(app.error(), None);
}

#[test]
fn github_leaves_what_the_engine_watches_to_the_engine() {
    let items = vec![builtin("github")];
    let github = world(json!({ "viewer": "ada", "repository": "zeronsh/zgx", "branches": {} }));
    let (mut app, heard) = shell_with(items.clone(), github.clone());
    publish_records(&app, &items);
    assert!(app.wait_until(|_| runs(&heard).iter().any(|r| ran(r, &["pullRequests(headRefName", "fix/build"]))), "{:?}", runs(&heard));
    // The sessions on screen are watched by the engine too.
    let watched = |heard: &Heard| heard.borrow().iter().rev().find(|(k, _)| k == "plugin.github.github_watch").map(|(_, v)| v.clone()).unwrap_or_default();
    assert!(watched(&heard).contains("\"a1\""), "{}", watched(&heard));
    // The engine says a1's: shown, from there; and gh asks of it no more
    // at a half minute's pace.
    app.host.shared("plugin.github.github", json_value(&json!({}))).set(json_value(&json!({ "a1": {
        "number": 77, "title": "From the engine", "url": "https://github.com/zeronsh/zgx/pull/77", "state": "open", "base": "main", "head": "fix/build", "ci": "failed", "review": "unknown",
    } })));
    assert!(app.wait_until(|app| app.texts().iter().any(|t| t == "77")), "{}", app.tree());
    let mark = runs(&heard).len();
    app.frames(5 * 60 * 60);
    let fix = runs(&heard)[mark..].iter().filter(|r| ran(r, &["pullRequests(headRefName", "fix/build"])).count();
    assert!(fix <= 1, "a1's branch asked {fix} times in five minutes, the engine watching it");
    assert_eq!(app.error(), None);
}

/// The fixture's sessions made `chats` (each `[id, folder, branch]`), as
/// the engine would say them, and the plugin's records made again.
fn sessions_in(app: &mut TestApp, items: &[Item], chats: &[(&str, &str, &str)]) {
    let shared = app.host.shared("sessions", zgx_script::Value::Nil);
    let mut sessions = value_json(&shared.signal().with_untracked(zgx_script::Value::clone));
    for chat in sessions["chats"].as_array_mut().into_iter().flatten() {
        if let Some((_, cwd, branch)) = chats.iter().find(|(id, _, _)| chat["id"] == *id) {
            chat["cwd"] = json!(cwd);
            chat["branch"] = json!(branch);
        }
    }
    shared.set(json_value(&sessions));
    publish_records(app, items);
}

#[test]
fn github_shows_a_session_only_its_own_repository_s_pull_request() {
    let items = vec![builtin("github")];
    // Your one open pull request from a `main`, in a repository none of the
    // sessions is in (a class's: head `main`, base `feedback`).
    let unrelated = branch_pr(4711, "OPEN", json!({ "headRefName": "main", "baseRefName": "feedback", "url": "https://github.com/class/lab-3/pull/4711" }));
    let github = world(json!({
        "viewer": "ada",
        // (gh can't place the remotes, which go through a local proxy:
        // the folders' own remotes say which repositories they are.)
        "gh_unknown_host": true,
        "remotes": {
            "/work/atlas": "origin\thttp://127.0.0.1:7457/github.com/zeronsh/zgx.git (fetch)\norigin\thttp://127.0.0.1:7457/github.com/zeronsh/zgx.git (push)\n",
            "/work/notes": "origin\tgit@github.com:ada/notes.git (fetch)\norigin\tgit@github.com:ada/notes.git (push)\n",
        },
        "by_repo": {
            "zeronsh/zgx": { "fix/build": branch_pr(1300, "OPEN", json!({ "isCrossRepository": false })) },
            "class/lab-3": { "main": unrelated },
        },
        // (What a search of your pull requests by branch alone would find.)
        "branches": { "main": unrelated },
    }));
    let (mut app, heard) = shell_with(items.clone(), github.clone());
    // Two sessions on `main` in different repositories, one on `main` in a
    // folder that isn't here (no repository to tell), and one on a branch
    // with its own pull request in its own repository.
    sessions_in(&mut app, &items, &[("a1", "/work/atlas", "fix/build"), ("a2", "/work/atlas", "main"), ("a3", "/work/notes", "main"), ("b1", "/srv/borealis", "main")]);
    // (Each found, asked, and asked again.)
    app.frames(5 * 60);
    assert!(app.host.run_command("github/refresh"));
    app.frames(5 * 60);
    // The sessions on `main`: none, never the unrelated repository's.
    assert!(!app.tree().contains("4711"), "another repository's pull request shown: {}", app.tree());
    // a1's, from its own repository: shown.
    assert!(app.wait_until(|app| app.tree().contains("\"1300\"")), "{}\n{:?}", app.tree(), runs(&heard));
    assert!(runs(&heard).iter().any(|r| r["program"] == "git" && r["cwd"] == "/work/notes"), "{:?}", runs(&heard));
    // (Each badge a link named by its pull request.)
    assert_eq!(app.tree().matches("link \"Open pull request ").count(), 1, "{}", app.tree());
    assert!(!runs(&heard).iter().any(|r| ran(r, &["search(", "head:"])), "searched by branch alone: {:?}", runs(&heard));
    assert_eq!(app.error(), None);
}

#[test]
fn github_doesn_t_take_a_fork_s_pull_request_for_a_session_s_own() {
    let items = vec![builtin("github")];
    // Someone's fork opened one from their own `fix/build`: not this session's.
    let github = world(json!({ "viewer": "ada", "repository": "zeronsh/zgx", "branches": { "fix/build": branch_pr(679, "OPEN", json!({ "isCrossRepository": true })) } }));
    let (mut app, heard) = shell_with(items.clone(), github.clone());
    publish_records(&app, &items);
    assert!(app.wait_until(|_| runs(&heard).iter().any(|r| ran(r, &["pullRequests(headRefName", "fix/build"]))), "{:?}", runs(&heard));
    app.frames(30);
    assert!(!app.target(".github-pr-badge").is_ok(), "{}", app.tree());
    // Its own, once there is one: that one.
    github.borrow_mut()["branches"]["fix/build"] = branch_pr(1300, "OPEN", json!({ "isCrossRepository": false }));
    assert!(refresh_until(&mut app, |app| app.tree().contains("\"1300\"")), "{}", app.tree());
}

/// The network as a test has it: none. Every request answered 404 at once
/// (a GitHub picture that isn't there: the face keeps its gradient), so no
/// test waits on the internet.
fn offline(host: &ScriptHost) {
    host.fetcher(|request: zgx_script::Request| {
        // (One picture there is: a badge-shaped PNG, for a test that needs
        // a picture to come.)
        let picture = request.url.to_lowercase().contains("flaky.example");
        async move {
            if picture {
                return Ok(zgx_script::Response { status: 200, headers: vec![("content-type".into(), "image/png".into())], body: BADGE_PNG.to_vec() });
            }
            Ok(zgx_script::Response { status: 404, headers: Vec::new(), body: Vec::new() })
        }
    });
}

/// GitHub's viewer in a plugin tab's placeholder, as the panel draws one
/// (ui/github_viewer_fixture.luau: the design system set up as the app sets
/// it up), GitHub played by `github` (its `pr`, its `diff`).
fn viewer(github: serde_json::Value) -> TestApp {
    viewer_with(github).0
}

/// A badge-shaped picture, 324 × 56, one grey.
const BADGE_PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 1, 68, 0, 0, 0, 56, 8, 6, 0, 0, 0, 133, 125, 129, 200, 0, 0, 0, 168, 73, 68, 65, 84, 120, 218, 237, 212, 33, 1, 0, 0, 12, 195, 176, 201, 159, 235, 93, 198, 73, 64, 12, 20, 52, 109, 7, 64, 23, 17, 0, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 12, 17, 192, 16, 1, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 48, 68, 0, 67, 4, 248, 117, 131, 237, 152, 218, 37, 241, 144, 45, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];

/// `viewer`, and what the plugin asked: each run of gh, each link opened
/// (`open_url`), each picture fetched (`fetch`).
fn viewer_with(github: serde_json::Value) -> (TestApp, Heard) {
    let entry = crate::ui_dir().join("github_viewer_fixture.luau");
    let item = builtin("github");
    let github = world(github);
    let calls: Heard = Heard::default();
    let asked = calls.clone();
    let app = TestApp::new(move |runtime| {
        // (Zeron's design system lent as the app lends it, from ui/.)
        let host = super::lend_design_system(ScriptHost::new(runtime, &entry), &entry);
        host.shared("zgx.window.size", zgx_script::window_size_value(600., 1200.));
        {
            // (Offline, as `offline`, and what was asked of the network heard.)
            let calls = calls.clone();
            host.fetcher(move |request: zgx_script::Request| {
                calls.borrow_mut().push(("fetch".into(), request.url.clone()));
                // (A bot's button, its address an SVG's, answers as a picture
                // drawn at 2x a badge's size, 324 × 56: the test host reads
                // no SVG, and the size it asks for is what's tested.)
                let svg = request.url.ends_with(".svg");
                // (A flaky server: its first answer to each picture fails.)
                let url = request.url.clone();
                let lower = url.to_lowercase();
                let flaky_first = lower.contains("flaky.example") && calls.borrow().iter().filter(|(k, v)| k == "fetch" && v.to_lowercase() == lower).count() == 1;
                let flaky = lower.contains("flaky.example");
                async move {
                    if flaky_first {
                        return Ok(zgx_script::Response { status: 503, headers: Vec::new(), body: Vec::new() });
                    }
                    if svg || flaky {
                        return Ok(zgx_script::Response { status: 200, headers: vec![("content-type".into(), "image/png".into())], body: BADGE_PNG.to_vec() });
                    }
                    Ok(zgx_script::Response { status: 404, headers: Vec::new(), body: Vec::new() })
                }
            });
        }
        expose_surface(&host);
        expose_own(&host, "github");
        // (Its own words, as the bridge publishes them.)
        super::locales::publish(&host, "github", &super::builtin_dir().join("github"));
        for name in ["result", "ack", "menus", "toast", "problem", "open_tab", "add_page", "github_watch", "store_set", "cost"] {
            host.action(format!("plugin.github.{name}"), |_| {});
        }
        {
            let calls = calls.clone();
            host.action("plugin.github.open_url", move |v: zgx_script::Value| calls.borrow_mut().push(("open_url".into(), v.to_json())));
        }
        fake_github(&host, "github", github.clone(), calls.clone());
        host.set_plugins(item.plugin().into_iter().collect());
        host
    });
    // (Room for all of it: a click lands only inside the window.)
    app.ui.scene.borrow_mut().resize(600., 1200.);
    app.frames(10);
    (app, asked)
}

#[test]
fn github_s_viewer_shows_the_pull_request_s_header_and_description() {
    let detail = json!({
        "title": "Plugins: class targets", "body": "## Why\n\nSelectors by class.\n\n- one\n- two",
        "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "author": { "login": "ada" },
        "baseRefName": "main", "headRefName": "plugin/targets", "state": "OPEN", "isDraft": false,
        "additions": 120, "deletions": 8,
        "ci": { "state": "failed", "totalCount": 3 },
        "statusCheckRollup": [
            { "name": "Tests", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": "" },
            { "name": "Lint", "status": "IN_PROGRESS", "conclusion": "", "detailsUrl": "" },
            { "name": "Build", "status": "COMPLETED", "conclusion": "SUCCESS", "detailsUrl": "" },
        ],
    });
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail }));
    assert_eq!(app.error(), None);
    // Asked of GitHub by the plugin itself: gh's GraphQL, this pull request.
    assert!(runs(&asked).iter().any(|r| ran(r, &["graphql", "pullRequest(number", "\"number\":1264", "\"owner\":\"zeronsh\""])), "{:?}", runs(&asked));
    let tree = app.tree();
    assert!(!tree.contains("\"placeholder\""), "the plugin fills its tab: {tree}");
    // The design system's header (its badge says the state and number), its
    // checks folded to a line in the gallery's words, its description.
    for text in ["Pull request 1264, open", "Plugins: class targets", "ada", "plugin/targets", "main", "+120", "−8", "Why", "Selectors by class.", "one", "two", "1 of 3 passed · Tests failed"] {
        assert!(tree.contains(&format!("\"{text}\"")), "{text}: {tree}");
    }
    // A check failed: the checks come first, before the description.
    let at = |text: &str| tree.find(&format!("\"{text}\"")).unwrap_or_else(|| panic!("{text}: {tree}"));
    assert!(at("1 of 3 passed · Tests failed") < at("Selectors by class."), "{tree}");
    // One line, its outcome the glyph's: no words of their own to open it.
    assert!(!tree.contains("\"See what failed\""), "{tree}");
    // Folded to a line; a click shows each, as Zeron's viewer says them.
    assert!(!tree.contains("\"Tests\""));
    app.click("#github-pr-checks >> role:button \"Checks: 1 of 3 passed, Tests failed\"");
    app.frames(5);
    let tree = app.tree();
    for text in ["Tests", "Failure", "Lint", "In progress", "Build", "Passed"] {
        assert!(tree.contains(&format!("\"{text}\"")), "{text}: {tree}");
    }
    // No design lint left over.
    assert_eq!(app.error(), None);
}

#[test]
fn github_s_viewer_opens_at_once_from_what_it_kept() {
    let detail = json!({ "title": "Kept title", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN" });
    let (mut app, _asked) = viewer_with(json!({ "viewer": "ada", "pr": detail }));
    assert!(app.tree().contains("\"Kept title\""), "{}", app.tree());
    // (What's kept is the plugin's own database: the same page read again
    // shows it before GitHub answers. Reloading the scripts keeps it.)
    app.host.reload().unwrap();
    app.frames(2);
    assert!(app.wait_until(|app| app.tree().contains("\"Kept title\"")), "{}", app.tree());
}

#[test]
fn github_s_viewer_says_what_went_wrong_and_tries_again() {
    let mut app = viewer(json!({ "fail": "GitHub isn't connected" }));
    let tree = app.tree();
    assert!(tree.contains("Couldn't load the pull request") && tree.contains("GitHub isn't connected"), "{tree}");
    assert!(app.find("role:button \"Try again\"").is_some());
}

#[test]
fn github_s_viewer_says_how_to_get_gh_when_it_isn_t_signed_in() {
    let mut app = viewer(json!({ "fail": "To get started with GitHub CLI, please run:  gh auth login" }));
    assert!(app.tree().contains("gh auth login"), "{}", app.tree());
}

#[test]
fn github_s_viewer_shows_each_file_s_diff_in_code() {
    let (head, base) = ("a".repeat(40), "b".repeat(40));
    let detail = json!({
        "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264,
        "state": "OPEN", "headRefOid": head, "baseRefOid": base,
        "files": [ { "path": "src/a.rs", "additions": 1, "deletions": 1, "changeType": "MODIFIED" }, { "path": "ui/b.luau", "additions": 2, "deletions": 0, "changeType": "ADDED" } ],
    });
    let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old a\n+new a\ndiff --git a/ui/b.luau b/ui/b.luau\nnew file mode 100644\n--- /dev/null\n+++ b/ui/b.luau\n@@ -0,0 +1,2 @@\n+one b\n+two b\n";
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail, "diff": diff }));
    assert_eq!(app.error(), None);
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    app.frames(10);
    let tree = app.tree();
    // A side pane's width: the files in their card, the first open in place.
    for text in ["src/a.rs", "ui/b.luau", "-old a", "+new a"] {
        assert!(tree.contains(&format!("\"{text}\"")), "{text}: {tree}");
    }
    assert!(!tree.contains("\"+one b\""), "only the open file's");
    // The diff, from GitHub's REST API as a diff.
    assert!(runs(&asked).iter().any(|r| ran(r, &["repos/zeronsh/zgx/pulls/1264", "v3.diff"])), "{:?}", runs(&asked));
    app.click("#github-pr-code >> role:button \"ui/b.luau\"");
    app.frames(10);
    let tree = app.tree();
    assert!(tree.contains("\"+one b\"") && tree.contains("\"+two b\""), "{tree}");
    // A click on an open file folds its diff away.
    app.click("#github-pr-code >> role:button \"src/a.rs\"");
    app.frames(10);
    let tree = app.tree();
    assert!(!tree.contains("\"+new a\"") && tree.contains("\"+one b\""), "{tree}");
}

#[test]
fn github_s_viewer_asks_for_a_diff_again_only_if_it_changed() {
    // (No revisions to key it by: asked of GitHub each time it opens.)
    let detail = json!({
        "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
        "files": [ { "path": "src/a.rs", "additions": 1, "deletions": 1, "changeType": "MODIFIED" } ],
    });
    let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old a\n+new a\n";
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail, "diff": diff, "diff_etag": "\"d1\"" }));
    let diffs = |asked: &Heard| runs(asked).into_iter().filter(|r| ran(r, &["v3.diff"])).collect::<Vec<_>>();
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    assert!(app.wait_until(|app| app.tree().contains("\"+new a\"")), "{}", app.tree());
    assert_eq!(diffs(&asked).len(), 1);
    // Opened again (drawn anew): asked with its ETag, GitHub's 304, the
    // diff kept.
    app.host.set_state("github_fixture.width", zgx_script::Value::from(420.)).unwrap();
    app.frames(10);
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    assert!(app.wait_until(|_| diffs(&asked).len() == 2));
    assert!(diffs(&asked)[1]["args"].to_string().contains("If-None-Match: \\\"d1\\\""), "{}", diffs(&asked)[1]);
    assert!(app.wait_until(|app| app.tree().contains("\"+new a\"")), "{}", app.tree());
    assert_eq!(app.error(), None);
}

#[test]
fn github_s_viewer_shows_what_was_said_and_comments_as_you() {
    let detail = json!({
        "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
        "comments": [
            { "id": "IC_1", "author": { "login": "ada" }, "body": "Looks close.", "createdAt": "2026-11-01T09:00:00Z" },
            { "id": "IC_2", "author": { "login": "ada" }, "body": "Here:\n<img width=\"600\" alt=\"shot\" src=\"https://github.com/user-attachments/assets/abc\" />", "createdAt": "2026-11-01T09:30:00Z" },
        ],
        "reviews": [
            { "author": { "login": "lin" }, "body": "Ship it.", "state": "APPROVED", "submittedAt": "2026-11-01T11:00:00Z" },
            { "author": { "login": "kai" }, "body": "", "state": "CHANGES_REQUESTED", "submittedAt": "2026-11-01T10:00:00Z" },
        ],
    });
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail }));
    app.click("#github-pr-tabs >> role:tab \"Activity\"");
    app.frames(10);
    assert_eq!(app.error(), None);
    let tree = app.tree();
    let at = |text: &str| tree.find(&format!("\"{text}\"")).unwrap_or_else(|| panic!("{text}: {tree}"));
    // In order: ada commented, kai asked for changes, lin approved.
    assert!(at("commented") < at("asked for changes") && at("asked for changes") < at("approved"), "{tree}");
    assert!(tree.contains("\"Looks close.\"") && tree.contains("\"Ship it.\""));
    // A picture GitHub's editor put in as HTML: drawn, from the web.
    assert!(app.wait_until(|_| asked.borrow().iter().any(|(k, v)| k == "fetch" && v == "https://github.com/user-attachments/assets/abc")), "{:?}", asked.borrow());
    app.type_text("#github-pr-activity >> role:textbox \"Comment\"", "Thanks!");
    app.click("#github-pr-activity >> role:button \"Comment\"");
    app.frames(10);
    // GitHub's REST API, as you; then the pull request again.
    let runs = runs(&asked);
    let sent = runs.iter().position(|r| ran(r, &["POST", "repos/zeronsh/zgx/issues/1264/comments", "Thanks!"])).unwrap_or_else(|| panic!("{runs:?}"));
    assert!(runs[sent..].iter().any(|r| ran(r, &["pullRequest(number"])), "{runs:?}");
}

#[test]
fn github_s_viewer_draws_a_bot_s_html_as_github_does() {
    // A bot's footer, as [code]smith writes it: two linked buttons, each a
    // <picture> of a dark and a light SVG, on one line with a space between,
    // then its small print (<sup>, a <code> handle) on the next.
    let body = concat!(
        "Looks fine.\n\n<!-- codesmith:footer -->\n---\n",
        "<a href=\"https://app.example/view?pr=1&amp;ref=footer\"><picture>",
        "<source media=\"(prefers-color-scheme: dark)\" srcset=\"https://assets.example/view-dark.svg\">",
        "<source media=\"(prefers-color-scheme: light)\" srcset=\"https://assets.example/view-light.svg\">",
        "<img alt=\"View with [code]smith\" src=\"https://assets.example/view-dark.svg\"></picture></a> ",
        "<a href=\"https://app.example/autofix\"><picture>",
        "<source media=\"(prefers-color-scheme: dark)\" srcset=\"https://assets.example/fix-dark.svg\">",
        "<source media=\"(prefers-color-scheme: light)\" srcset=\"https://assets.example/fix-light.svg\">",
        "<img alt=\"Autofix with [code]smith\" src=\"https://assets.example/fix-dark.svg\"></picture></a>\n",
        "<sup>Need help on this PR? Tag <code>@codesmith-bot</code> with what you need.</sup>\n\n",
        "<!-- /codesmith:footer -->",
    );
    let detail = json!({
        "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
        "comments": [
            { "id": "IC_1", "author": { "login": "codesmith-bot" }, "body": body, "createdAt": "2026-11-01T09:00:00Z" },
            { "id": "IC_2", "author": { "login": "ada" }, "body": "<details><summary>Build logs</summary>\n\nfirst line<br>second &amp; last &lt;ok&gt;\n</details>\n<p>See <a href=\"https://example.com/x\">the run</a>, <b>twice</b>.</p>", "createdAt": "2026-11-01T09:30:00Z" },
        ],
    });
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail }));
    app.click("#github-pr-tabs >> role:tab \"Activity\"");
    app.frames(10);
    assert_eq!(app.error(), None);
    // The two buttons side by side, one line of pictures, each a link.
    let line = "#github-pr-activity >> .github-pictures";
    for name in ["View with [code]smith", "Autofix with [code]smith"] {
        assert!(app.target(&format!("{line} >> role:link \"{name}\"")).is_ok(), "{name}: {}", app.tree());
    }
    let tree = app.tree();
    // Each its theme's source (the window is dark), never the light ones.
    for src in ["https://assets.example/view-dark.svg", "https://assets.example/fix-dark.svg"] {
        assert!(app.wait_until(|_| asked.borrow().iter().any(|(k, v)| k == "fetch" && v == src)), "{src}: {:?}", asked.borrow());
    }
    assert!(!asked.borrow().iter().any(|(k, v)| k == "fetch" && v.contains("-light.svg")), "{:?}", asked.borrow());
    // The small print as words (its handle as code), none of the HTML.
    assert!(tree.contains("Need help on this PR? Tag") && tree.contains("@codesmith-bot") && tree.contains("with what you need."), "{tree}");
    // A <details>: folded to its summary, as GitHub folds it; opened, what
    // it holds (a break a line's end, entities read); a <p>'s link a link,
    // its <b> bold.
    assert!(tree.contains("Build logs") && !tree.contains("first line"), "{tree}");
    app.click("role:button \"Show Build logs\"");
    app.frames(10);
    let tree = app.tree();
    assert!(tree.contains("first line") && tree.contains("second & last <ok>"), "{tree}");
    assert!(tree.contains("the run") && tree.contains("twice"), "{tree}");
    for raw in ["<sup>", "<code>", "<a ", "<picture", "codesmith:footer", "![", "&amp;", "<details", "<summary", "<br", "<p>", "<b>", "**"] {
        assert!(!tree.contains(raw), "{raw}: {tree}");
    }
    // At a badge's height (the SVG asked for no size), never its own
    // drawn size or the column's width.
    let named = format!("{line} >> role:link \"View with [code]smith\"");
    let fits = |app: &mut TestApp| {
        let b = app.bounds(&named);
        b.height > 20. && b.height < 36. && b.width > 100. && b.width < 300.
    };
    assert!(app.wait_until(fits), "{:?}", app.bounds(&named));
    // A button opens its link where links go (entities read).
    app.click(&format!("{line} >> role:link \"View with [code]smith\""));
    app.frames(5);
    assert!(asked.borrow().iter().any(|(k, v)| k == "open_url" && v == "\"https://app.example/view?pr=1&ref=footer\""), "{:?}", asked.borrow());
}

#[test]
fn a_picture_that_fails_to_load_is_asked_for_again() {
    // A screenshot whose first fetch fails (a busy server, a dropped
    // connection): asked again, it shows, rather than staying blank.
    let src = "https://flaky.example/shot.png";
    let detail = json!({
        "title": "T", "body": format!("Before:\n\n![shot]({src})"), "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
    });
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail }));
    // (Asked again as the same address, its host in capitals: the same
    // picture, a new source to zgx.)
    let fetched = |asked: &Heard| asked.borrow().iter().filter(|(k, v)| k == "fetch" && v.to_lowercase() == src).count();
    assert!(app.wait_until(|_| fetched(&asked) >= 1), "{:?}", asked.borrow());
    // (The design system asks again 6 s on.)
    let mut again = false;
    for _ in 0..4 {
        if app.wait_until(|_| fetched(&asked) >= 2) {
            again = true;
            break;
        }
    }
    assert!(again, "asked once only: {:?}", asked.borrow());
    assert!(app.wait_until(|app| app.target("role:button \"View shot\"").is_ok()), "{}", app.tree());
    // A click looks at it whole, in the design system's lightbox.
    app.click("role:button \"View shot\"");
    assert!(app.wait_until(|app| app.target("\"Image viewer\"").is_ok()), "{}", app.tree());
    assert_eq!(app.error(), None);
}

#[test]
fn github_s_viewer_merges_closes_and_reviews_a_pull_request() {
    let head = "f".repeat(40);
    let detail = json!({
        "title": "Add the primitives", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
        "headRefName": "zeron/primitives", "baseRefName": "main", "headRefOid": head,
        "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "viewerDidAuthor": false, "isCrossRepository": false,
        "allCommits": { "totalCount": 3 },
        "ci": { "state": "passed" }, "statusCheckRollup": [ { "name": "Build", "status": "COMPLETED", "conclusion": "SUCCESS" } ],
        "reviews": [ { "author": { "login": "theo" }, "body": "", "state": "APPROVED", "submittedAt": "2026-11-01T10:00:00Z" } ],
    });
    let repo = json!({ "viewerPermission": "WRITE", "squashMergeAllowed": true, "mergeCommitAllowed": true, "rebaseMergeAllowed": true });
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail, "repo": repo }));
    app.frames(10);
    assert_eq!(app.error(), None);
    let tree = app.tree();
    // Where it stands, how it goes in, what becomes of its branch.
    // (The tree prints long words cut: their start.)
    for text in ["\"Ready to merge\"", "\"Approved by theo, every check passed, no conflicts with", "\"How it goes in\"", "\"The 3 commits become one on main", "\"Merge commit\"", "\"Rebase and merge\"", "\"Delete the branch after\"", "\"zeron/primitives goes from GitHub."] {
        assert!(tree.contains(text), "{text}: {tree}");
    }
    // A verdict: someone else's, still open.
    app.click("#github-pr-review >> role:button \"Approve\"");
    app.frames(10);
    assert!(runs(&asked).iter().any(|r| ran(r, &["addPullRequestReview", "\"event\":\"APPROVE\"", "PR_1"])), "{:?}", runs(&asked));
    // Merged, squashed, against the head it was read at; then its branch
    // deleted on GitHub; then the pull request read again.
    app.click("#github-pr-merge >> role:button \"Squash and merge\"");
    app.frames(10);
    let all = runs(&asked);
    let merged = all.iter().position(|r| ran(r, &["mergePullRequest", "\"how\":\"SQUASH\"", &head])).unwrap_or_else(|| panic!("{all:?}"));
    assert!(all[merged..].iter().any(|r| ran(r, &["DELETE", "repos/zeronsh/zgx/git/refs/heads/zeron/primitives"])), "{all:?}");
    assert!(all[merged..].iter().any(|r| ran(r, &["pullRequest(number"])), "{all:?}");
    // Closing, from the header's menu.
    app.click("role:button \"More\"");
    app.frames(5);
    app.click("role:menuitem \"Close pull request\"");
    app.frames(10);
    assert!(runs(&asked).iter().any(|r| ran(r, &["closePullRequest", "PR_1"])), "{:?}", runs(&asked));
    assert_eq!(app.error(), None);
}

#[test]
fn github_s_viewer_says_what_stands_in_a_merge_s_way_and_what_became_of_one() {
    let base = json!({ "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "baseRefName": "main", "headRefName": "zeron/x" });
    let with = |extra: serde_json::Value| {
        let mut d = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            d[k] = v.clone();
        }
        d
    };
    let writer = json!({ "viewerPermission": "WRITE" });
    for (extra, said) in [
        (json!({ "state": "OPEN", "mergeable": "CONFLICTING", "mergeStateStatus": "DIRTY" }), "Conflicts with main"),
        (json!({ "state": "OPEN", "mergeStateStatus": "BLOCKED", "reviewDecision": "CHANGES_REQUESTED" }), "Not yet"),
        (json!({ "state": "OPEN", "mergeStateStatus": "BEHIND" }), "Update branch"),
        (json!({ "state": "OPEN", "isDraft": true, "mergeStateStatus": "DRAFT" }), "Ready for review"),
        (json!({ "state": "MERGED", "merged": true, "mergedBy": { "login": "priya" } }), "Merged #1264 into main"),
        (json!({ "state": "CLOSED", "closed": true }), "Reopen pull request"),
    ] {
        let (mut app, _) = viewer_with(json!({ "viewer": "ada", "pr": with(extra), "repo": writer }));
        app.frames(10);
        assert_eq!(app.error(), None);
        let tree = app.tree();
        assert!(tree.contains(&format!("\"{said}\"")), "{said}: {tree}");
    }
    // Not yours to merge: where it stands, without the button.
    let (mut app, _) = viewer_with(json!({ "viewer": "ada", "pr": with(json!({ "state": "OPEN", "mergeStateStatus": "CLEAN" })), "repo": { "viewerPermission": "READ" } }));
    app.frames(10);
    let tree = app.tree();
    assert!(tree.contains("\"Ready to merge\"") && !tree.contains("\"How it goes in\"") && !tree.contains("\"Squash and merge\""), "{tree}");
}

#[test]
fn a_picture_in_a_pull_request_tab_opens_in_the_lightbox_over_the_window() {
    // The viewer in a chat's panel tab, as the app places it (not the
    // fixture's placeholder): a click on a picture opens the design
    // system's lightbox over the whole window.
    let items = vec![builtin("github")];
    let detail = json!({
        "title": "T", "body": "Before:\n\n![shot](https://flaky.example/shot.png)", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
    });
    let (mut app, _) = shell_with(items.clone(), world(json!({ "viewer": "ada", "pr": detail })));
    assert_eq!(app.error(), None);
    publish_records(&app, &items);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(10);
    // (The host's part: the plugin's tab asked for, in the session's panel.)
    app.host.set_state("shell_fixture.plugins", json_value(&json!(true))).unwrap();
    app.frames(5);
    let request = json!([{ "seq": 1, "kind": "panel_tab", "title": "GitHub", "value": { "plugin": "github", "session": "a1", "kind": "github-pr", "title": "T", "icon": "pull-request", "data": { "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264 } } }]);
    app.host.shared("plugins.request", zgx_script::Value::Nil).set(json_value(&request));
    app.frames(10);
    assert!(app.wait_until(|app| app.target("#panel >> .github-pr").is_ok()), "{}", app.tree());
    assert!(app.wait_until(|app| app.target("role:button \"View shot\"").is_ok()), "{}", app.tree());
    app.click("role:button \"View shot\"");
    assert!(app.wait_until(|app| app.target("\"Image viewer\"").is_ok()), "{}", app.tree());
    assert_eq!(app.error(), None);
    let viewer = app.bounds("\"Image viewer\"");
    let window = app.bounds("#shell");
    assert!(viewer.width >= window.width - 1. && viewer.height >= window.height - 1., "over the window, not the pane: {viewer:?} in {window:?}");
    // And the picture in it, at a size (not a blank viewer).
    assert!(app.wait_until(|app| app.bounds("\"Image viewer\" >> \"Image\"").width > 100.), "{:?}", app.bounds("\"Image viewer\" >> \"Image\""));
}

#[test]
fn github_s_viewer_opens_its_links_where_zeron_opens_links() {
    let head = "e".repeat(40);
    let detail = json!({
        "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN", "headRefOid": head,
        "files": [ { "path": "src/a.rs", "additions": 1, "deletions": 0 } ],
    });
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail, "diff": "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n+x\n" }));
    app.click("role:link \"Open on GitHub\"");
    app.frames(5);
    assert!(asked.borrow().iter().any(|(k, v)| k == "open_url" && v == "\"https://github.com/zeronsh/zgx/pull/1264\""), "{:?}", asked.borrow());
    // A file's way to GitHub is at the end of its diff's head, at the
    // window's width: the file as it is at the pull request's head.
    app.ui.scene.borrow_mut().resize(1100., 1200.);
    app.frames(10);
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    // (The width is taken a beat after it changes: the split layout, with
    // the file's link in its diff's head, comes then.)
    assert!(app.wait_until(|app| app.target("role:link \"Open src/a.rs on GitHub\"").is_ok()), "{}", app.tree());
    app.click("role:link \"Open src/a.rs on GitHub\"");
    app.frames(10);
    let link = format!("\"https://github.com/zeronsh/zgx/blob/{head}/src/a.rs\"");
    assert!(asked.borrow().iter().any(|(k, v)| k == "open_url" && *v == link), "{:?}", asked.borrow());
}

#[test]
fn github_s_viewer_shows_review_threads_on_their_lines_and_in_activity() {
    let detail = json!({
        "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
        "files": [ { "path": "src/a.rs", "additions": 1, "deletions": 1 } ],
        "reviewThreads": [ {
            "id": "t1", "isResolved": false, "isOutdated": false, "path": "src/a.rs", "line": 11, "diffSide": "RIGHT",
            "comments": [ { "id": "RC_1", "author": { "login": "lin" }, "body": "Why the clone?", "createdAt": "2026-11-01T12:00:00Z", "url": "https://github.com/zeronsh/zgx/pull/1264#discussion_r1" } ],
        } ],
    });
    let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,2 +10,2 @@\n keep\n-old\n+let x = y.clone();\n";
    let (mut app, asked) = viewer_with(json!({ "viewer": "ada", "pr": detail, "diff": diff }));
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    app.frames(10);
    assert_eq!(app.error(), None);
    let tree = app.tree();
    // Under line 11 of the new side: the added line.
    let line = tree.find("\"+let x = y.clone();\"").unwrap_or_else(|| panic!("{tree}"));
    let said = tree.find("\"Why the clone?\"").unwrap_or_else(|| panic!("{tree}"));
    assert!(said > line, "the thread sits under its line: {tree}");
    // Its people's GitHub pictures, asked of GitHub by the plugin itself
    // ("network"): their gradient while none has come.
    assert!(asked.borrow().iter().any(|(k, v)| k == "fetch" && v == "https://github.com/lin.png?size=96"), "{:?}", asked.borrow());
    assert!(app.find(".github-thread >> role:button \"Resolve\"").is_some(), "unresolved, so it can be: {tree}");
    // Not yours: no way to edit or delete it.
    assert!(app.find(".github-thread >> role:button \"Delete\"").is_none(), "{tree}");
    app.click("#github-pr-tabs >> role:tab \"Activity\"");
    app.frames(10);
    let tree = app.tree();
    assert!(tree.contains("\"src/a.rs:11 · Unresolved\""), "{tree}");
    app.click("role:link \"Open src/a.rs:11 · Unresolved on GitHub\"");
    app.frames(5);
    assert!(asked.borrow().iter().any(|(k, v)| k == "open_url" && v.contains("discussion_r1")), "{:?}", asked.borrow());
}

/// A pull request with a thread on line 10 of src/a.rs (lin's comment, and
/// yours when `mine`), its diff, for the comment tests.
fn threaded(head: &str, mine: bool) -> serde_json::Value {
    let mut comments = vec![json!({ "id": "RC_1", "author": { "login": "lin" }, "body": "Keep?", "createdAt": "2026-11-01T12:00:00Z" })];
    if mine {
        comments.push(json!({ "id": "RC_2", "author": { "login": "ada" }, "body": "Yes, keep it.", "createdAt": "2026-11-01T12:05:00Z", "viewerDidAuthor": true }));
    }
    json!({
        "viewer": "ada",
        "diff": "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -10,2 +10,2 @@\n keep\n-old\n+new\n",
        "pr": {
            "title": "T", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN",
            "headRefOid": head, "baseRefOid": "d".repeat(40),
            "files": [ { "path": "src/a.rs", "additions": 1, "deletions": 1 } ],
            "reviewThreads": [ { "id": "PRRT_1", "isResolved": false, "isOutdated": false, "path": "src/a.rs", "line": 10, "diffSide": "RIGHT", "comments": comments } ],
        },
    })
}

#[test]
fn a_review_comment_s_actions_sit_in_its_own_corner_one_comment_at_a_time() {
    // A resolved thread: lin's comment (a way to it on GitHub), then yours
    // (edit, delete, the way to it).
    let mut world = threaded(&"c".repeat(40), true);
    let thread = &mut world["pr"]["reviewThreads"][0];
    thread["isResolved"] = json!(true);
    thread["comments"][0]["url"] = json!("https://github.com/zeronsh/zgx/pull/1264#discussion_r1");
    thread["comments"][1]["url"] = json!("https://github.com/zeronsh/zgx/pull/1264#discussion_r2");
    let (mut app, _) = viewer_with(world);
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    assert!(app.wait_until(|app| app.find("role:article \"ada: Yes, keep it.\"").is_some()), "{}", app.tree());
    let (lin, mine) = ("role:article \"lin: Keep?\"", "role:article \"ada: Yes, keep it.\"");
    let bar = |c: &str| format!("{c} >> role:toolbar \"Comment actions\"");
    macro_rules! inside {
        ($outer:expr, $inner:expr) => {{
            let (outer, inner) = ($outer, $inner);
            inner.x >= outer.x - 0.5 && inner.y >= outer.y - 0.5 && inner.x + inner.width <= outer.x + outer.width + 0.5 && inner.y + inner.height <= outer.y + outer.height + 0.5
        }};
    }
    // Under the pointer, yours: its actions, in its own box, at its top
    // right; lin's stay hidden.
    app.hover(&format!("{mine} >> \"Yes, keep it.\""));
    app.frames(20);
    let (card, actions) = (app.bounds(mine), app.bounds(&bar(mine)));
    assert!(inside!(card, actions), "{card:?} {actions:?}");
    assert!((card.x + card.width - (actions.x + actions.width)).abs() < 0.5 && (actions.y - card.y).abs() < 0.5, "its top right: {card:?} {actions:?}");
    let (om, ol) = (app.opacity(&bar(mine)), app.opacity(&bar(lin)));
    assert!(om > 0.99 && ol < 0.01, "{om} {ol}");
    // Never over the thread's head (Resolved).
    let resolved = app.bounds(".github-thread >> role:button \"Resolved\"");
    assert!(actions.y >= resolved.y + resolved.height, "{resolved:?} {actions:?}");
    for name in ["Edit", "Delete", "Open on GitHub"] {
        assert!(app.find(&format!("{} >> \"{name}\"", bar(mine))).is_some(), "{name}: {}", app.tree());
    }
    // lin's under the pointer: lin's alone.
    app.hover(&format!("{lin} >> \"Keep?\""));
    app.frames(20);
    let (a, b) = (app.bounds(lin), app.bounds(&bar(lin)));
    assert!(inside!(a, b), "{a:?} {b:?}");
    assert!(app.opacity(&bar(lin)) > 0.99 && app.opacity(&bar(mine)) < 0.01);
    assert!(app.find(&format!("{} >> \"Delete\"", bar(lin))).is_none(), "not yours: {}", app.tree());
    // By the keys: Tab reaches them, and the one with focus shows its own.
    app.hover("#github-pr-tabs");
    app.frames(20);
    let mut reached = false;
    for _ in 0..40 {
        app.keys("tab");
        app.frames(2);
        if app.tree().lines().any(|l| l.contains("focused") && l.contains("\"Edit\"")) {
            reached = true;
            break;
        }
    }
    assert!(reached, "Tab reaches Edit: {}", app.tree());
    app.frames(20);
    assert!(app.opacity(&bar(mine)) > 0.99 && app.opacity(&bar(lin)) < 0.01, "shown by focus alone");
}

#[test]
fn github_s_viewer_comments_on_a_line_replies_and_resolves() {
    let head = "c".repeat(40);
    let (mut app, asked) = viewer_with(threaded(&head, false));
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    app.frames(10);
    assert_eq!(app.error(), None);
    // + on the added line (new line 11), under the pointer: a box under it;
    // sent, a line comment, by GitHub's REST API, at the head commit.
    app.hover("\"+new\"");
    app.frames(5);
    app.click("role:button \"Comment on line 11\"");
    app.frames(5);
    app.type_text(".github-writer >> role:textbox \"Comment\"", "Why new?");
    app.click(".github-writer >> role:button \"Comment\"");
    app.frames(10);
    {
        let runs = runs(&asked);
        let at = runs.iter().position(|r| ran(r, &["POST", "repos/zeronsh/zgx/pulls/1264/comments"])).unwrap_or_else(|| panic!("{runs:?}"));
        let body: serde_json::Value = serde_json::from_str(runs[at]["stdin"].as_str().unwrap()).unwrap();
        assert_eq!(
            (body["path"].as_str(), body["line"].as_f64(), body["side"].as_str(), body["commit_id"].as_str(), body["body"].as_str()),
            (Some("src/a.rs"), Some(11.), Some("RIGHT"), Some(head.as_str()), Some("Why new?")),
        );
        assert!(runs[at..].iter().any(|r| ran(r, &["pullRequest(number"])), "then the pull request again");
    }
    // One GitHub refuses: it goes, its words back in the box, and why said.
    app.hover("\"+new\"");
    app.frames(5);
    app.click("role:button \"Comment on line 11\"");
    app.frames(5);
    app.type_text(".github-writer >> role:textbox \"Comment\"", "REFUSED words");
    app.click(".github-writer >> role:button \"Comment\"");
    app.frames(10);
    {
        let tree = app.tree();
        assert!(tree.contains("GitHub said no"), "why: {tree}");
        assert!(app.find(".github-writer >> role:textbox \"Comment\"").is_some(), "the box again: {tree}");
        assert!(tree.contains("REFUSED words"), "its words kept: {tree}");
    }
    app.click(".github-writer >> role:button \"Cancel\"");
    app.frames(5);
    // The thread on line 10: Reply, then Resolve, by its id (GraphQL).
    app.click(".github-thread >> role:button \"Reply\"");
    app.frames(5);
    app.type_text(".github-thread >> role:textbox \"Reply\"", "Yes.");
    app.click(".github-thread >> role:button \"Comment\"");
    app.frames(10);
    app.click(".github-thread >> role:button \"Resolve\"");
    app.frames(10);
    {
        let runs = runs(&asked);
        assert!(runs.iter().any(|r| ran(r, &["addPullRequestReviewThreadReply", "PRRT_1", "Yes."])), "{runs:?}");
        assert!(runs.iter().any(|r| ran(r, &["resolveReviewThread", "PRRT_1"])), "{runs:?}");
    }
    // Sent and not yet answered: on its line at once, as you, by your name
    // on GitHub (the pull request's `viewer`).
    app.hover("\"+new\"");
    app.frames(5);
    app.click("role:button \"Comment on line 11\"");
    app.frames(5);
    app.type_text(".github-writer >> role:textbox \"Comment\"", "HELD words");
    app.click(".github-writer >> role:button \"Comment\"");
    app.frames(10);
    let tree = app.tree();
    assert!(tree.contains("HELD words") && app.find(".github-writer").is_none(), "it stands before GitHub answers: {tree}");
    assert!(tree.contains("article \"ada: HELD words\""), "{tree}");
}

#[test]
fn github_s_viewer_edits_and_deletes_your_own_comments() {
    let head = "f".repeat(40);
    let (mut app, asked) = viewer_with(threaded(&head, true));
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    app.frames(10);
    assert_eq!(app.error(), None);
    // Yours (GitHub says you wrote it): Edit, in place; saved, GitHub's
    // GraphQL as you, and the new words at once.
    // (Its actions show under the pointer.)
    app.hover("\"ada: Yes, keep it.\"");
    app.frames(10);
    app.click(".github-thread >> role:button \"Edit\"");
    app.frames(5);
    assert!(app.find(".github-thread >> role:textbox \"Edit comment\"").is_some(), "{}", app.tree());
    app.fill(".github-thread >> role:textbox \"Edit comment\"", "Yes, keep it for now.");
    app.click(".github-thread >> role:button \"Save\"");
    app.frames(10);
    assert!(runs(&asked).iter().any(|r| ran(r, &["updatePullRequestReviewComment", "RC_2", "Yes, keep it for now."])), "{:?}", runs(&asked));
    // Delete: said once more; then gone at once, GitHub asked.
    // (The pull request read back is GitHub's copy here, which the fake
    // doesn't change: the comment has its old words again.)
    app.hover("\"ada: Yes, keep it.\"");
    app.frames(10);
    app.click(".github-thread >> role:button \"Delete\"");
    app.frames(5);
    assert!(app.tree().contains("\"Delete this comment?\""), "{}", app.tree());
    app.click(".github-thread >> role:button \"Delete comment\"");
    app.frames(10);
    assert!(runs(&asked).iter().any(|r| ran(r, &["deletePullRequestReviewComment", "RC_2"])), "{:?}", runs(&asked));
    // (GitHub's copy here still has it until the pull request is read back
    // without it; the comment others wrote stays either way.)
    assert!(app.tree().contains("\"Keep?\""), "{}", app.tree());
}

/// GitHub's board in its page's placeholder, as the page area draws one
/// (ui/github_board_fixture.luau), GitHub, the records and storage played
/// here.
fn board() -> (TestApp, Heard) {
    let entry = crate::ui_dir().join("github_board_fixture.luau");
    let item = builtin("github");
    let calls: Heard = Heard::default();
    let asked = calls.clone();
    let row = |n: u64, title: &str, mine: bool| json!({
        "number": n, "title": title, "url": format!("https://github.com/zeronsh/zgx/pull/{n}"), "repository": { "nameWithOwner": "zeronsh/zgx" },
        "author": { "login": if mine { "ada" } else { "lin" } }, "isDraft": n == 2, "reviewDecision": if n == 1 { json!("APPROVED") } else { serde_json::Value::Null },
        "mergeable": "MERGEABLE", "additions": 3, "deletions": 1,
        "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] },
    });
    let page = |rows: Vec<serde_json::Value>, next: Option<&str>, total: u64| json!({
        "issueCount": total, "pageInfo": { "hasNextPage": next.is_some(), "endCursor": next }, "nodes": rows,
    });
    let github = world(json!({
        "viewer": "ada", "repository": "zeronsh/zgx",
        "pr": { "title": "First fix", "body": "", "url": "https://github.com/zeronsh/zgx/pull/1", "number": 1, "state": "OPEN" },
        "list": {
            "authored": page(vec![row(1, "First fix", true), row(2, "Second draft", true)], Some("c2"), 3),
            "more": page(vec![row(3, "Third", true)], None, 3),
            "reviewing": page(vec![row(9, "Review me", false)], None, 1),
        },
    }));
    let app = TestApp::new(move |runtime| {
        let host = super::lend_design_system(ScriptHost::new(runtime, &entry), &entry);
        offline(&host);
        expose_surface(&host);
        expose_own(&host, "github");
        super::locales::publish(&host, "github", &super::builtin_dir().join("github"));
        // (Its projects' folders: "files.read".)
        host.shared("plugin.github.records", zgx_script::Value::Nil).set(json_value(&json!({ "profile": {}, "projects": { "p1": { "name": "Atlas", "path": "/work/atlas" }, "p2": { "name": "Borealis", "path": "/work/borealis" } } })));
        for name in ["result", "ack", "menus", "toast", "add_page", "github_watch", "open_url", "cost"] {
            host.action(format!("plugin.github.{name}"), |_| {});
        }
        {
            let calls = calls.clone();
            host.action("plugin.github.open_tab", move |v: zgx_script::Value| calls.borrow_mut().push(("open_tab".into(), v.to_json())));
        }
        {
            let calls = calls.clone();
            host.action("plugin.github.problem", move |v: zgx_script::Value| calls.borrow_mut().push(("problem".into(), v.to_json())));
        }
        {
            // (Storage as the host keeps it: written, then published.)
            let (calls, store) = (calls.clone(), host.shared("plugin.github.store", json_value(&json!({}))));
            host.action("plugin.github.store_set", move |v: zgx_script::Value| {
                let json: serde_json::Value = serde_json::from_str(&v.to_json()).unwrap();
                calls.borrow_mut().push(("store_set".into(), v.to_json()));
                let mut now: serde_json::Value = serde_json::from_str(&store.signal().with_untracked(zgx_script::Value::clone).to_json()).unwrap_or_default();
                now[json["key"].as_str().unwrap()] = json["value"].clone();
                store.set(json_value(&now));
            });
        }
        fake_github(&host, "github", github.clone(), calls.clone());
        host.set_plugins(item.plugin().into_iter().collect());
        host
    });
    app.ui.scene.borrow_mut().resize(1100., 900.);
    app.frames(10);
    (app, asked)
}

#[test]
fn github_s_board_lists_a_project_s_pull_requests_by_filter_and_opens_one() {
    let (mut app, asked) = board();
    assert_eq!(app.error(), None);
    assert!(!asked.borrow().iter().any(|(k, _)| k == "problem"), "{:?}", asked.borrow());
    let tree = app.tree();
    assert!(!tree.contains("\"placeholder\""), "the plugin fills its page: {tree}");
    assert!(tree.contains("\"Choose a project to see its pull requests.\""), "{tree}");
    // A project, from the picker: gh says which repository its folder is,
    // then its pull requests (yours first), searched on GitHub.
    app.click("role:button \"Project\"");
    app.frames(10);
    app.click("\"Atlas\"");
    app.frames(10);
    assert!(runs(&asked).iter().any(|r| ran(r, &["repo", "view"]) && r["cwd"] == "/work/atlas"), "{:?}", runs(&asked));
    assert!(runs(&asked).iter().any(|r| ran(r, &["search(", "repo:zeronsh/zgx", "author:@me"])), "{:?}", runs(&asked));
    let tree = app.tree();
    for text in ["zeronsh/zgx", "Pull request 1, open", "Pull request 2, draft", "First fix", "Second draft", "ada · Approved · CI passed", "ada · Draft · CI passed", "2 of 3"] {
        assert!(tree.contains(&format!("\"{text}\"")), "{text}: {tree}");
    }
    // Kept for the next run.
    assert!(asked.borrow().iter().any(|(k, v)| k == "store_set" && v.contains("\"p1\"")), "{:?}", asked.borrow());
    // The next page, after the first.
    app.click("role:button \"Load more\"");
    app.frames(10);
    let tree = app.tree();
    assert!(tree.contains("\"Third\"") && tree.contains("\"3 of 3\"") && !tree.contains("\"Load more\""), "{tree}");
    // Searched as you type.
    app.type_text("role:textbox \"Search pull requests\"", "draft");
    app.frames(10);
    let tree = app.tree();
    assert!(tree.contains("\"Second draft\"") && !tree.contains("\"First fix\""), "{tree}");
    app.fill("role:textbox \"Search pull requests\"", "");
    app.frames(5);
    // Reviewing: what waits for your review.
    app.click("#github-board-filters >> role:tab \"Reviewing\"");
    app.frames(10);
    assert!(app.tree().contains("\"Review me\""), "{}", app.tree());
    assert!(runs(&asked).iter().any(|r| ran(r, &["search(", "review-requested:@me"])), "{:?}", runs(&asked));
    // A row: the pull request in the page's own panel (Zeron's right pane),
    // as a tab.
    app.click("#github-board-filters >> role:tab \"Authored\"");
    app.frames(10);
    app.click("role:button \"#1 First fix\"");
    app.frames(10);
    assert_eq!(app.error(), None);
    assert!(!asked.borrow().iter().any(|(k, _)| k == "problem"), "{:?}", asked.borrow().iter().filter(|(k, _)| k == "problem").collect::<Vec<_>>());
    let (_, tab) = asked.borrow().iter().find(|(k, _)| k == "open_tab").cloned().unwrap_or_else(|| panic!("{:?}", asked.borrow()));
    let tab: serde_json::Value = serde_json::from_str(&tab).unwrap();
    assert_eq!(
        (tab["page"]["plugin"].as_str(), tab["page"]["id"].as_str(), tab["kind"].as_str(), tab["title"].as_str(), tab["data"]["url"].as_str()),
        (Some("github"), Some("pulls"), Some("github-pr"), Some("First fix"), Some("https://github.com/zeronsh/zgx/pull/1"))
    );
    assert!(super::surfaces::tab(&tab).is_ok(), "the host takes it: {tab}");
}

#[test]
fn github_s_page_is_titled_in_the_language_shown() {
    // Its sidebar entry follows Settings → Language as it changes: listed
    // again under the same id, in the plugin's own words.
    let items = vec![builtin("github")];
    let (mut app, heard) = shell_hearing(items.clone());
    publish_records(&app, &items);
    app.frames(10);
    let titles = |heard: &Heard| -> Vec<String> {
        heard.borrow().iter().filter(|(k, _)| k == "plugin.github.add_page").filter_map(|(_, v)| serde_json::from_str::<serde_json::Value>(v).ok()).filter_map(|v| v["title"].as_str().map(str::to_string)).collect()
    };
    assert_eq!(titles(&heard).last().map(String::as_str), Some("Pull requests"), "{:?}", titles(&heard));
    // (As Settings → Language sets it: the app's script, screens/language.)
    app.host.set_state("settings_language", json_value(&json!("zh-Hans"))).unwrap();
    app.frames(10);
    assert_eq!(titles(&heard).last().map(String::as_str), Some("拉取请求"), "{:?}", titles(&heard));
    app.host.set_state("settings_language", json_value(&json!("en"))).unwrap();
    app.frames(10);
    assert_eq!(titles(&heard).last().map(String::as_str), Some("Pull requests"), "{:?}", titles(&heard));
    assert_eq!(app.error(), None);
}

#[test]
fn a_plugin_s_page_survives_a_reload_and_takes_its_place_in_the_history() {
    let items = vec![builtin("github")];
    let (mut app, _) = shell_hearing(items.clone());
    assert_eq!(app.error(), None);
    publish_records(&app, &items);
    // (The bridge's part: the page the plugin listed, for the sidebar.)
    let pages = || json_value(&serde_json::json!([{ "plugin": "github", "plugin_name": "GitHub", "id": "pulls", "title": "Pull requests", "icon": "pull-request" }]));
    app.host.shared("plugins.pages", zgx_script::Value::Nil).set(pages());
    app.frames(10);
    // A chat first, then the board: a route of its own, so that chat can be
    // gone back to from it.
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click(".plugin-page-row");
    app.frames(20);
    let on_board = |app: &mut TestApp| app.target(".github-board").is_ok();
    assert!(on_board(&mut app), "the board in its page: {}", app.tree());
    // A page of its own: the new chat's page under it isn't seen, its
    // composer (and what plugins put round it) gone with it.
    assert!(app.find("#chat-composer >> role:textbox").is_none(), "{}", app.tree());
    assert!(app.target("#plugin-page-area >> .github-board").is_ok(), "{}", app.tree());
    // Its own right pane: the title bar's toggle opens it beside the board.
    assert!(app.bounds("#panel").width < 1., "closed: {}", app.tree());
    assert!(app.host.run_command("shell.toggle_files"));
    assert!(app.wait_until(|app| app.bounds("#panel").width > 200.), "{}", app.tree());
    assert!(on_board(&mut app));
    app.host.run_command("shell.toggle_files");
    app.frames(30);
    // The scripts reloaded (a save, a template switched): still there.
    app.host.reload().unwrap();
    app.frames(20);
    app.host.shared("plugins.pages", zgx_script::Value::Nil).set(pages());
    app.frames(10);
    assert!(on_board(&mut app), "kept across a reload: {}", app.tree());
    // A session, then back to the board, then forward to the session.
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    assert!(!on_board(&mut app));
    // (The chat's page back as it was, its composer with it.)
    assert!(app.wait_until(|app| app.find("#chat-composer >> role:textbox").is_some()), "{}", app.tree());
    app.host.run_command("router.back");
    app.frames(20);
    assert!(on_board(&mut app), "back to the board: {}", app.tree());
    app.host.run_command("router.forward");
    app.frames(20);
    assert!(!on_board(&mut app), "forward to the session");
}

/// What a new chat's wallpaper holds on the GPU, state by state: the shell
/// drawn offscreen by zgx's own renderer (no window), Parallax scenes
/// loaded, and the renderer's memory report read after each (its layers,
/// each by node, and its image textures). Run on purpose, as audit_shots:
/// `cargo test --bin zeron-zgx wallpaper_gpu_memory -- --ignored --nocapture`.
#[test]
#[ignore]
fn wallpaper_gpu_memory() {
    use zgx_gpu::GpuRenderer;
    let items = vec![builtin("parallax-scenes")];
    let mut app = shell(items.clone());
    assert_eq!(app.error(), None);
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(Path::new(items[0].json["snapshot"].as_str().unwrap()).join("zeron-plugin.json")).unwrap()).unwrap();
    app.host.shared("plugins", zgx_script::Value::Nil).set(json_value(&json!({
        "state": "ready",
        "list": [{ "id": "parallax-scenes", "name": manifest["name"], "status": "on", "ui": manifest["ui"], "permissions": manifest["permissions"], "allowed": manifest["permissions"], "ask": [] }],
    })));
    let parallax = std::env::var("WALLPAPER_PARALLAX").unwrap_or_else(|_| "normal".into());
    app.host.shared("plugin.parallax-scenes.settings", zgx_script::Value::Nil).set(json_value(&json!({ "scene": "violet-dusk", "parallax": parallax })));
    let (w, h, scale) = (1200f32, 800f32, 2f32);
    let mut gpu = GpuRenderer::new((w * scale) as u32, (h * scale) as u32).expect("an offscreen GPU");
    gpu.set_scale_factor(scale);
    for font in crate::fonts() {
        font.unwrap().install(&mut gpu.text_system().borrow_mut());
    }
    gpu.install_text(&mut app.ui.scene.borrow_mut());
    app.ui.refresh_text_geometry();
    let set = |app: &TestApp, name: &str, value: serde_json::Value| app.host.set_state(name, json_value(&value)).unwrap();
    // Drawn for a while (pictures decode off the thread; crossfades end),
    // then the renderer's lines.
    let measure = |app: &mut TestApp, gpu: &mut GpuRenderer, label: &str| -> (usize, usize, usize, usize) {
        let all = [zgx::scene::Rect { x: 0., y: 0., width: w, height: h }];
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(60));
            app.frames(15);
            app.ui.prepare_frame();
            let _ = app.ui.scene.borrow_mut().flush();
            gpu.render(&app.ui.scene.borrow(), &all).unwrap();
        }
        // (At rest: the window idle trims what a fade held for drawing, zgx
        // #1416.)
        gpu.trim();
        let mut report = zgx::memory::Report::default();
        gpu.memory(&mut report);
        app.host.memory(&mut report);
        let get = |area: &str, name: &str| report.lines.iter().find(|l| l.area == area && l.name == name).map(|l| (l.entries.unwrap_or(0), l.bytes.unwrap_or(0))).unwrap_or((0, 0));
        let (layers, live) = get("gpu", "layers live");
        let (images, image_bytes) = get("gpu", "image textures");
        let (_, decoded) = get("images", "decoded cached");
        let (_, held) = get("images", "decoded pixels held");
        let mb = |b: usize| b as f32 / 1_048_576.;
        eprintln!(
            "{label:<34} layers {layers} ({:.1} MiB live), image textures {images} ({:.1} MiB), decoded {:.1} MiB cached + {:.1} MiB held",
            mb(live),
            mb(image_bytes),
            mb(decoded),
            mb(held)
        );
        let tree = app.tree();
        for l in report.lines.iter().filter(|l| l.area == "gpu layer") {
            // (Which node: its line in the tree, by its number.)
            let id = l.name.split_whitespace().next().unwrap_or("");
            let node = tree.lines().find(|t| t.trim_start().starts_with(&format!("{id} "))).map(str::trim).unwrap_or("(not in the tree)");
            eprintln!("    {}  <- {node}", l.name);
        }
        (layers, live, images, image_bytes)
    };
    set(&app, "shell_wallpaper", json!("none"));
    set(&app, "shell_wallpaper_plugin", json!(""));
    measure(&mut app, &mut gpu, "none");
    set(&app, "shell_wallpaper_plugin_effect", json!("none"));
    set(&app, "shell_wallpaper_plugin", json!("parallax-scenes"));
    measure(&mut app, &mut gpu, "scene, no effect");
    set(&app, "plugin.parallax-scenes.settings", json!({ "scene": "violet-dusk", "parallax": "off" }));
    measure(&mut app, &mut gpu, "scene, no effect, parallax off");
    set(&app, "plugin.parallax-scenes.settings", json!({ "scene": "violet-dusk", "parallax": "normal" }));
    // The page's width in pixels: what the effect's layer may take.
    let page_px = (app.bounds("#shell-page").width * scale).round() as usize + 4;
    for effect in ["scanlines", "dither", "none"] {
        set(&app, "shell_wallpaper_plugin_effect", json!(effect));
        let (layers, live, ..) = measure(&mut app, &mut gpu, &format!("scene, {effect} (after a change)"));
        // One layer at most (the effect draws in the wallpaper's own box),
        // no wider than the page: changing effects leaves none behind.
        assert!(layers <= 1, "{effect}: {layers} layers");
        assert!(live <= page_px * (h * scale) as usize * 4 + page_px * 16, "{effect}: {live} bytes");
    }
    // The pointer over the page: the scene's layers move.
    set(&app, "shell_wallpaper_plugin_effect", json!("scanlines"));
    set(&app, "shell_wallpaper_px", json!(0.2));
    set(&app, "shell_wallpaper_py", json!(0.3));
    measure(&mut app, &mut gpu, "scene, scanlines, pointer moved");
    set(&app, "shell_wallpaper_plugin_effect", json!("none"));
    measure(&mut app, &mut gpu, "scene, none, pointer moved");
    set(&app, "shell_wallpaper_plugin", json!(""));
    set(&app, "shell_wallpaper", json!("image"));
    let sky = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/assets/cards/violet-dusk/sky.webp");
    set(&app, "shell_wallpaper_image", json!({ "path": sky, "name": "sky.webp", "w": 576, "h": 768, "x": 0.5, "y": 0.5, "zoom": 1 }));
    for effect in ["none", "scanlines"] {
        set(&app, "shell_wallpaper_effect", json!(effect));
        let (layers, live, ..) = measure(&mut app, &mut gpu, &format!("image, {effect}"));
        // A picture alone draws straight; under an effect, one page-wide layer.
        assert_eq!(layers, if effect == "none" { 0 } else { 1 }, "image, {effect}");
        assert!(live <= page_px * (h * scale) as usize * 4 + page_px * 16, "image, {effect}: {live} bytes");
    }
    // A camera's photo, 4032 × 3024: as picked, then as it's kept now (no
    // larger than covering the screen it's shown on, here the render's).
    let dir = std::env::temp_dir().join(format!("zgx-wallpaper-gpu-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("photo.png");
    let (pw, ph) = (4032u32, 3024u32);
    let mut raw = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph {
        for x in 0..pw {
            raw.extend_from_slice(&[(x / 16) as u8, (y / 12) as u8, 160, 255]);
        }
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, pw, ph);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&raw).unwrap();
    }
    std::fs::write(&photo, png_bytes).unwrap();
    set(&app, "shell_wallpaper_effect", json!("none"));
    for (label, screen) in [("photo as picked, none", None), ("photo as kept now, none", Some(((w * scale) as u32, (h * scale) as u32)))] {
        let kept = crate::settings::keep_wallpaper_for_tests(photo.to_str().unwrap(), &dir.join(label.replace(' ', "-").replace(',', "")), screen);
        set(&app, "shell_wallpaper_image", json!({ "path": kept.0, "name": "photo.png", "w": kept.1, "h": kept.2, "x": 0.5, "y": 0.5, "zoom": 1 }));
        measure(&mut app, &mut gpu, &format!("{label} ({}x{})", kept.1, kept.2));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Parallax scenes (plugins-builtin/parallax-scenes): used as the wallpaper
/// (its page's Zeron row), its scene stands in a new chat's background
/// place, under the effect, as its settings say; let go, it's gone.
#[test]
fn parallax_scenes_draws_a_new_chat_s_background_as_its_settings_say() {
    let items = vec![builtin("parallax-scenes")];
    let mut app = shell(items.clone());
    assert_eq!(app.error(), None, "the shell with Parallax scenes failed");
    // On, and replacing the place (as the host lists it).
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(Path::new(items[0].json["snapshot"].as_str().unwrap()).join("zeron-plugin.json")).unwrap()).unwrap();
    // Its settings page: a scene and how far it moves, as Zeron checks them.
    let declared = super::settings::declared("parallax-scenes", &manifest, true);
    assert_eq!(declared.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), ["wallpaper", "scene", "parallax"]);
    app.host.shared("plugins", zgx_script::Value::Nil).set(json_value(&json!({
        "state": "ready",
        "list": [{ "id": "parallax-scenes", "name": manifest["name"], "status": "on", "ui": manifest["ui"], "permissions": manifest["permissions"], "allowed": manifest["permissions"], "ask": [] }],
    })));
    app.host.shared("plugin.parallax-scenes.settings", zgx_script::Value::Nil).set(json_value(&json!({ "scene": "aurora", "parallax": "normal" })));
    app.host.set_state("shell_wallpaper_plugin", zgx_script::Value::from("parallax-scenes")).unwrap();
    app.frames(30);
    assert!(app.find("parallax-scene").is_some(), "its scene in the place: {}", app.tree());
    assert!(app.find(".scene-aurora").is_some(), "the scene its settings name");
    assert!(app.bounds("parallax-scene").width > 400., "it fills the background: {:?}", app.bounds("parallax-scene"));
    // Another scene set: it follows.
    app.host.shared("plugin.parallax-scenes.settings", zgx_script::Value::Nil).set(json_value(&json!({ "scene": "wheat", "parallax": "off" })));
    app.frames(30);
    assert!(app.find(".scene-wheat").is_some(), "{}", app.tree());
    // Let go (Appearance's None back): the place, and the scene, go.
    app.host.set_state("shell_wallpaper_plugin", zgx_script::Value::from("")).unwrap();
    app.frames(30);
    assert!(app.find("parallax-scene").is_none());
    assert!(app.find("#new-chat-background").is_none());
    assert_eq!(app.error(), None, "nothing it drew failed");
}

#[test]
fn greeter_reads_its_settings_and_hears_each_change() {
    use super::settings;
    let items = vec![example("settings-demo")];
    let (mut app, heard) = shell_hearing(items.clone());
    assert_eq!(app.error(), None, "the shell with Greeter failed");
    // Its values as the bridge publishes them: the manifest's defaults,
    // then what's set on its page (kept by the book, as the bridge keeps it).
    let dir = std::env::temp_dir().join(format!("zgx-greeter-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut book = settings::Book::new(Some(dir.clone()));
    let snapshot = PathBuf::from(items[0].json["snapshot"].as_str().unwrap());
    book.sync(&[settings::On { id: "settings-demo".into(), name: "Greeter".into(), hash: "h1".into(), builtin: false }], |_| settings::manifest_in(&snapshot));
    // Not one of Zeron's own: its permission row is left out.
    assert!(!book.list().to_string().contains("screen-recording"), "{}", book.list());
    let own = app.host.shared("plugin.settings-demo.settings", zgx_script::Value::Nil);
    let publish = |book: &settings::Book| own.set(json_value(&serde_json::Value::Object(book.values("settings-demo"))));
    publish(&book);
    app.frames(10);
    assert_eq!(app.text("greeter"), "Hello", "its defaults: {}", app.tree());
    let toasts = |heard: &Heard| heard.borrow().iter().filter(|(k, _)| k == "plugin.settings-demo.toast").count();
    assert_eq!(toasts(&heard), 0, "the values it starts with aren't changes");
    // Set on its page: it follows, and hears each change.
    book.set("settings-demo", "words", &json!("Hi")).unwrap();
    book.set("settings-demo", "tone", &json!("excited")).unwrap();
    publish(&book);
    app.frames(10);
    assert_eq!(app.text("greeter"), "Hi!");
    let said: Vec<String> = heard.borrow().iter().filter(|(k, _)| k == "plugin.settings-demo.toast").map(|(_, v)| v.clone()).collect();
    assert!(said.iter().any(|s| s.contains("words is now Hi")) && said.iter().any(|s| s.contains("tone is now excited")), "{said:?}");
    // It reports a status for a setting, and clears it again.
    let statuses = |heard: &Heard| -> Vec<String> { heard.borrow().iter().filter(|(k, _)| k == "plugin.settings-demo.setting_status").map(|(_, v)| v.clone()).collect() };
    assert!(statuses(&heard).last().is_some_and(|s| s.contains("\"status\":null")), "starting with words, nothing to say: {:?}", statuses(&heard));
    book.set("settings-demo", "words", &json!("")).unwrap();
    publish(&book);
    app.frames(10);
    let last = statuses(&heard).last().cloned().unwrap_or_default();
    let asked: serde_json::Value = serde_json::from_str(&last).unwrap();
    assert_eq!((asked["id"].as_str(), asked["status"]["badge"].as_str(), asked["status"]["tone"].as_str()), (Some("words"), Some("Empty"), Some("warning")), "{last}");
    // As the bridge takes it: kept for the page while it's on.
    assert!(book.set_status("settings-demo", "words", &asked["status"]).unwrap());
    assert_eq!(book.list()[0]["status"]["words"]["badge"], "Empty");
    book.set("settings-demo", "words", &json!("Hey")).unwrap();
    publish(&book);
    app.frames(10);
    assert!(statuses(&heard).last().is_some_and(|s| s.contains("\"status\":null")), "cleared once it says something");
    book.set("settings-demo", "show", &json!(false)).unwrap();
    publish(&book);
    app.frames(10);
    assert_eq!(app.text("greeter"), "");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_plugin_s_icon_is_the_svg_in_its_folder_and_nothing_outside_it() {
    // GitHub ships the GitHub mark.
    let github = builtin("github");
    let svg = super::icon_svg(&github.json).expect("GitHub's icon");
    assert!(svg.trim_start().starts_with("<svg"), "{svg}");
    // One naming nothing, or a file outside its folder, has none.
    assert!(super::icon_svg(&example("focus").json).is_none());
    let dir = std::env::temp_dir().join(format!("zgx-icon-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("icon.png"), "png").unwrap();
    for icon in ["../secret.svg", "/etc/x.svg", "icon.png", ""] {
        std::fs::write(dir.join("zeron-plugin.json"), json!({ "id": "p", "icon": icon }).to_string()).unwrap();
        assert!(super::icon_svg(&json!({ "snapshot": dir })).is_none(), "{icon} isn't an icon");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_design_system_s_icons_are_its_assets_inlined() {
    // A plugin lent the design system loads no file outside its own folder,
    // so ds/icon draws each glyph from ui/ds/icon_svgs.luau as code: every
    // icon in ui/assets/icons/ui must be there, as it is on the disk
    // (tools/icon_svgs.py writes it).
    let ui = crate::ui_dir();
    let table = std::fs::read_to_string(ui.join("ds/icon_svgs.luau")).unwrap();
    let mut n = 0;
    for entry in std::fs::read_dir(ui.join("assets/icons/ui")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "svg") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let svg = std::fs::read_to_string(&path).unwrap();
            let line = format!("[\"{name}\"] = [==[{}]==],", svg.trim());
            assert!(table.contains(&line), "{name}.svg isn't inlined as it is: run tools/icon_svgs.py");
            n += 1;
        }
    }
    assert_eq!(table.matches("]==],").count(), n, "an icon inlined that's no longer in assets/icons/ui: run tools/icon_svgs.py");
}

/// How long the viewer takes with a long description (`PERF_BODY`: a JSON
/// file with `body`): building, scrolling, and a version bump. A look, not
/// a test: `PERF_BODY=… cargo test --bin zeron-zgx perf_long_description -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_long_description() {
    let path = std::env::var("PERF_BODY").expect("PERF_BODY");
    let body: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (name, body) in [("short", json!("Just a line.")), ("long", body["body"].clone())] {
        let detail = json!({ "title": "T", "body": body, "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN", "mergeStateStatus": "CLEAN" });
        let t0 = std::time::Instant::now();
        let (mut app, _) = viewer_with(json!({ "viewer": "ada", "pr": detail, "repo": { "viewerPermission": "WRITE" } }));
        assert!(app.wait_until(|app| app.target("#github-pr-merge").is_ok()));
        let built = t0.elapsed();
        // Idle frames: nothing changes.
        let t1 = std::time::Instant::now();
        app.frames(30);
        let idle = t1.elapsed() / 30;
        // Scrolling the page.
        let mut worst = std::time::Duration::ZERO;
        let t2 = std::time::Instant::now();
        for _ in 0..30 {
            let t = std::time::Instant::now();
            app.scroll(".github-pr", 120.);
            app.frame();
            worst = worst.max(t.elapsed());
        }
        let scroll = t2.elapsed() / 30;
        // Typing in the review box (a state change beside the description).
        let t3 = std::time::Instant::now();
        app.type_text("#github-pr-review >> role:textbox \"Comment\"", "hello world");
        app.frames(5);
        let typed = t3.elapsed();
        // The pull request read again (Refresh): the page built again.
        let t4 = std::time::Instant::now();
        app.click("role:button \"Refresh\"");
        app.frames(10);
        let refreshed = t4.elapsed();
        eprintln!("PERF {name}: built {built:?}, idle frame {idle:?}, scroll frame avg {scroll:?} worst {worst:?}, typing {typed:?}, refresh {refreshed:?}, nodes {}", app.tree().lines().count());
    }
}

/// The cost of each frame while the panel toggles with a pull request open
/// in it (`PERF_BODY`: a JSON file with `body`). A look, not a test:
/// `PERF_BODY=… cargo test --bin zeron-zgx perf_panel_toggle -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_panel_toggle() {
    let path = std::env::var("PERF_BODY").expect("PERF_BODY");
    let long: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (name, body) in [("short", json!("Just a line.")), ("long", long["body"].clone())] {
        let items = vec![builtin("github")];
        let detail = json!({ "title": "T", "body": body, "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN", "mergeStateStatus": "CLEAN" });
        let (mut app, _) = shell_with(items.clone(), world(json!({ "viewer": "ada", "pr": detail, "repo": { "viewerPermission": "WRITE" } })));
        publish_records(&app, &items);
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
        app.frames(10);
        app.host.set_state("shell_fixture.plugins", json_value(&json!(true))).unwrap();
        app.frames(5);
        let request = json!([{ "seq": 1, "kind": "panel_tab", "title": "GitHub", "value": { "plugin": "github", "session": "a1", "kind": "github-pr", "title": "T", "icon": "pull-request", "data": { "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264 } } }]);
        app.host.shared("plugins.request", zgx_script::Value::Nil).set(json_value(&request));
        app.frames(10);
        assert!(app.wait_until(|app| app.target("#panel >> #github-pr-merge").is_ok()), "{}", app.tree());
        // (Real text: shaped and measured as the window shapes it, and
        // painted by the GPU, so a frame costs what it costs.)
        let (w, h) = (1200., 800.);
        app.ui.scene.borrow_mut().resize(w, h);
        let mut gpu = zgx_gpu::GpuRenderer::new((w * 2.) as u32, (h * 2.) as u32).unwrap();
        gpu.set_scale_factor(2.);
        for font in crate::fonts() {
            font.unwrap().install(&mut gpu.text_system().borrow_mut());
        }
        gpu.install_text(&mut app.ui.scene.borrow_mut());
        app.ui.refresh_text_geometry();
        app.frames(30);
        let _ = app.ui.scene.borrow_mut().flush();
        let all = [zgx::scene::Rect { x: 0., y: 0., width: w, height: h }];
        gpu.render(&app.ui.scene.borrow(), &all).unwrap();
        let mut lines = Vec::new();
        let root_before = app.target(".github-pr").ok();
        let holder_before = app.target("#panel >> .plugin-tab").ok();
        let panel_before = app.target("#panel").ok();
        for pass in ["close", "open"] {
            assert!(app.host.run_command("shell.toggle_files"));
            let mut total = std::time::Duration::ZERO;
            let mut worst = std::time::Duration::ZERO;
            let mut layouts = 0;
            let mut paints = 0;
            for _ in 0..20 {
                let t = std::time::Instant::now();
                app.frame();
                let report = app.ui.scene.borrow_mut().flush();
                let cpu = t.elapsed();
                gpu.render(&app.ui.scene.borrow(), &report.damage).unwrap();
                let took = t.elapsed();
                let _ = cpu;
                total += took;
                worst = worst.max(took);
                layouts += report.layout_nodes;
                paints += report.paint_nodes;
            }
            lines.push(format!("{pass}: avg {:?} worst {worst:?} layout_nodes {layouts} paint_nodes {paints}", total / 20));
            // What comes after the motion (a rebuild deferred past it).
            let mut after = std::time::Duration::ZERO;
            let mut worst_after = std::time::Duration::ZERO;
            let mut layouts_after = 0;
            for _ in 0..60 {
                let t = std::time::Instant::now();
                app.frame();
                let report = app.ui.scene.borrow_mut().flush();
                gpu.render(&app.ui.scene.borrow(), &report.damage).unwrap();
                let took = t.elapsed();
                after += took;
                worst_after = worst_after.max(took);
                layouts_after += report.layout_nodes;
            }
            lines.push(format!("  then 60 frames: total {after:?} worst {worst_after:?} layout_nodes {layouts_after}"));
        }
        let root_after = app.target(".github-pr").ok();
        let holder_after = app.target("#panel >> .plugin-tab").ok();
        let panel_after = app.target("#panel").ok();
        eprintln!(
            "PERF {name} ({} nodes): {} | rebuilt across the toggle: viewer {}, its placeholder {}, the panel {}",
            app.tree().lines().count(),
            lines.join(" | "),
            root_before != root_after,
            holder_before != holder_after,
            panel_before != panel_after
        );
    }
}

/// The cost of each frame while a pull request scrolls in the panel
/// (`PERF_BODY`: a JSON file with `body`; `PERF_STATES`: states to set
/// first, `name=json;name=json`, a wallpaper and a material). A look, not a
/// test: `PERF_BODY=… cargo test --bin zeron-zgx perf_scroll_pane -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_scroll_pane() {
    let path = std::env::var("PERF_BODY").expect("PERF_BODY");
    let long: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let states: Vec<(String, serde_json::Value)> = std::env::var("PERF_STATES")
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (k, v) = s.split_once('=').expect("name=json");
            (k.to_string(), serde_json::from_str(v).expect("json"))
        })
        .collect();
    let items = vec![builtin("github")];
    let detail = json!({ "title": "T", "body": long["body"], "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN", "mergeStateStatus": "CLEAN" });
    let (mut app, _) = shell_with(items.clone(), world(json!({ "viewer": "ada", "pr": detail, "repo": { "viewerPermission": "WRITE" } })));
    publish_records(&app, &items);
    for (name, value) in &states {
        app.host.set_state(name, json_value(value)).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(10);
    app.host.set_state("shell_fixture.plugins", json_value(&json!(true))).unwrap();
    app.frames(5);
    let request = json!([{ "seq": 1, "kind": "panel_tab", "title": "GitHub", "value": { "plugin": "github", "session": "a1", "kind": "github-pr", "title": "T", "icon": "pull-request", "data": { "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264 } } }]);
    app.host.shared("plugins.request", zgx_script::Value::Nil).set(json_value(&request));
    let t_open = std::time::Instant::now();
    let mut shown = false;
    for _ in 0..12 {
        if app.wait_until(|app| app.target("#panel >> #github-pr-merge").is_ok()) {
            shown = true;
            break;
        }
    }
    eprintln!("PERF open: the pull request showed after {:?} (shown {shown}, error {:?})", t_open.elapsed(), app.error());
    assert!(shown, "{}", app.tree().lines().filter(|l| l.contains("github") || l.contains("didn't")).collect::<Vec<_>>().join("\n"));
    let (w, h) = (1400., 900.);
    app.ui.scene.borrow_mut().resize(w, h);
    let mut gpu = zgx_gpu::GpuRenderer::new((w * 2.) as u32, (h * 2.) as u32).unwrap();
    gpu.set_scale_factor(2.);
    for font in crate::fonts() {
        font.unwrap().install(&mut gpu.text_system().borrow_mut());
    }
    gpu.install_text(&mut app.ui.scene.borrow_mut());
    app.ui.refresh_text_geometry();
    app.frames(60);
    let _ = app.ui.scene.borrow_mut().flush();
    let all = [zgx::scene::Rect { x: 0., y: 0., width: w, height: h }];
    gpu.render(&app.ui.scene.borrow(), &all).unwrap();
    // Idle: nothing changes.
    let mut idle = std::time::Duration::ZERO;
    for _ in 0..20 {
        let t = std::time::Instant::now();
        app.frame();
        let report = app.ui.scene.borrow_mut().flush();
        gpu.render(&app.ui.scene.borrow(), &report.damage).unwrap();
        idle += t.elapsed();
    }
    // Scrolling the pull request, a wheel tick a frame.
    let mut total = std::time::Duration::ZERO;
    let mut worst = std::time::Duration::ZERO;
    let (mut layouts, mut paints, mut composites, mut damage_px) = (0, 0, 0, 0.);
    for _ in 0..40 {
        let t = std::time::Instant::now();
        app.scroll("#panel >> .github-pr", 60.);
        app.frame();
        let report = app.ui.scene.borrow_mut().flush();
        gpu.render(&app.ui.scene.borrow(), &report.damage).unwrap();
        let took = t.elapsed();
        total += took;
        worst = worst.max(took);
        layouts += report.layout_nodes;
        paints += report.paint_nodes;
        composites += report.composite_nodes;
        damage_px += report.damage.iter().map(|r| r.width * r.height).sum::<f32>();
    }
    eprintln!(
        "PERF scroll ({} nodes, states {:?}): idle frame {:?}, scroll frame avg {:?} worst {worst:?}, per frame: layout {} paint {} composite {} damage {:.0} pt²",
        app.tree().lines().count(),
        states.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        idle / 20,
        total / 40,
        layouts / 40,
        paints / 40,
        composites / 40,
        damage_px / 40.
    );
}

/// The app's language, as Settings → General → Language sets it.
fn choose_language(app: &mut TestApp, language: &str) {
    app.host.set_state("settings_language", zgx_script::Value::from(language)).expect("the language state");
    app.frames(20);
}

#[test]
fn a_plugin_s_own_words_follow_the_language_chosen_and_fall_back_to_its_english() {
    let items = vec![example("session-words")];
    let mut app = shell(items.clone());
    assert_eq!(app.error(), None, "the shell with the example failed");
    publish_records(&app, &items);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    assert_eq!(app.text("session-words"), "Working", "{}", app.tree());
    // Chinese, chosen: its own words in Chinese, at once.
    choose_language(&mut app, "zh-Hans");
    assert_eq!(app.text("session-words"), "正在处理");
    // A language it lacks (Zeron has it too: English's parent rules): its
    // English.
    choose_language(&mut app, "en");
    assert_eq!(app.text("session-words"), "Working");
}

#[test]
fn a_plugin_without_locales_says_what_it_did_in_any_language() {
    let items = vec![example("session-status")];
    let mut app = shell(items.clone());
    assert_eq!(app.error(), None);
    publish_records(&app, &items);
    choose_language(&mut app, "zh-Hans");
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    assert_eq!(app.text("session-status"), "Working", "its own plain strings, untouched");
}

#[test]
fn the_design_system_lent_to_a_plugin_has_zeron_s_words_from_the_first_load() {
    // A plugin loads before the app's script: the lent design system finds
    // Zeron's catalog loaded (lend_design_system's script), and follows
    // the language chosen.
    let dir = std::env::temp_dir().join(format!("zgx-lent-ds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("zeron-plugin.json"),
        r#"{ "id": "lent-words", "name": "Lent words", "entry": "init.luau", "permissions": ["ui.add"] }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("init.luau"),
        r##"assert(pcall(ui.t, "ds.status.working"), "the app's catalog isn't loaded before plugins")
local locale = require("@ds/locale")
local status = require("@ds/status")
assert(type(locale.t("ds.status.working")) ~= "string", "the design system made English strings, not messages")
assert(status.KINDS.working.label == "Working", "the design system's own strings, in English first")
ui.contribute { target = "#page-title", after = ui.text { id = "lent-words", locale.t("ds.status.working") } }
"##,
    )
    .unwrap();
    let items = vec![from(dir.clone(), "lent-words")];
    let mut app = shell(items.clone());
    assert_eq!(app.error(), None, "the plugin's design system failed to load");
    publish_records(&app, &items);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    assert_eq!(app.text("lent-words"), "Working", "{}", app.tree());
    choose_language(&mut app, "zh-Hans");
    assert_ne!(app.text("lent-words"), "Working", "Zeron's words follow the language in a plugin too");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The cost of each frame as a pull request with a big diff opens in the
/// panel (72 files, ~21k lines). A look, not a test:
/// `cargo test --bin zeron-zgx perf_open_pane -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_open_pane() {
    let mut diff = String::new();
    let mut files = Vec::new();
    for i in 0..72 {
        let path = format!("crates/thing{}/src/mod{}.rs", i % 25, i);
        files.push(json!({ "path": path, "additions": 290, "deletions": 18 }));
        diff.push_str(&format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,20 +1,300 @@\n"));
        for l in 0..300 {
            diff.push_str(&format!("{}fn line_{l}() -> u32 {{ {l} * 2 }}\n", if l % 16 == 0 { "-" } else { "+" }));
        }
    }
    let items = vec![builtin("github")];
    let head = "e".repeat(40);
    let detail = json!({ "title": "T", "body": "A change.", "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN", "mergeStateStatus": "CLEAN", "headRefOid": head, "baseRefOid": "f".repeat(40), "files": files, "additions": 21000, "deletions": 1300 });
    let (mut app, _) = shell_with(items.clone(), world(json!({ "viewer": "ada", "pr": detail, "diff": diff, "repo": { "viewerPermission": "WRITE" } })));
    publish_records(&app, &items);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(10);
    app.host.set_state("shell_fixture.plugins", json_value(&json!(true))).unwrap();
    app.frames(5);
    let (w, h) = (1400., 900.);
    app.ui.scene.borrow_mut().resize(w, h);
    let mut gpu = zgx_gpu::GpuRenderer::new((w * 2.) as u32, (h * 2.) as u32).unwrap();
    gpu.set_scale_factor(2.);
    for font in crate::fonts() {
        font.unwrap().install(&mut gpu.text_system().borrow_mut());
    }
    gpu.install_text(&mut app.ui.scene.borrow_mut());
    app.ui.refresh_text_geometry();
    app.frames(30);
    let _ = app.ui.scene.borrow_mut().flush();
    let all = [zgx::scene::Rect { x: 0., y: 0., width: w, height: h }];
    gpu.render(&app.ui.scene.borrow(), &all).unwrap();
    // The tab asked for: the panel opens with the pull request in it.
    let request = json!([{ "seq": 1, "kind": "panel_tab", "title": "GitHub", "value": { "plugin": "github", "session": "a1", "kind": "github-pr", "title": "T", "icon": "pull-request", "data": { "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264 } } }]);
    app.host.shared("plugins.request", zgx_script::Value::Nil).set(json_value(&request));
    let mut frames = Vec::new();
    let mut builds = 0;
    let mut last_root = None;
    for _ in 0..120 {
        let t = std::time::Instant::now();
        app.frame();
        let report = app.ui.scene.borrow_mut().flush();
        gpu.render(&app.ui.scene.borrow(), &report.damage).unwrap();
        frames.push((t.elapsed(), report.layout_nodes));
        let root = app.target(".github-pr").ok();
        if root.is_some() && root != last_root {
            builds += 1;
        }
        last_root = root;
    }
    let worst: Vec<String> = {
        let mut v: Vec<(usize, std::time::Duration, usize)> = frames.iter().enumerate().map(|(i, (d, l))| (i, *d, *l)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.iter().take(6).map(|(i, d, l)| format!("#{i} {d:?} ({l} laid out)")).collect()
    };
    let total: std::time::Duration = frames.iter().map(|(d, _)| *d).sum();
    eprintln!("PERF open: 120 frames took {total:?}; the viewer built {builds} time(s); worst frames: {}", worst.join(", "));
    // Then the Code tab: the diff parsed and its list built.
    let t = std::time::Instant::now();
    app.click("#github-pr-tabs >> role:tab \"Code\"");
    let mut frames2 = Vec::new();
    for _ in 0..60 {
        let f = std::time::Instant::now();
        app.frame();
        let report = app.ui.scene.borrow_mut().flush();
        gpu.render(&app.ui.scene.borrow(), &report.damage).unwrap();
        frames2.push(f.elapsed());
    }
    let worst2 = frames2.iter().max().copied().unwrap_or_default();
    eprintln!("PERF code tab: 60 frames took {:?}; worst {worst2:?}; error {:?}", t.elapsed(), app.error());
}

/// Each frame's cost while the panel's edge is dragged with a pull request
/// in it (`PERF_BODY`: a JSON file with `body`), against the same drag with
/// the panel empty, real fonts and the GPU drawing. A look, not a test:
/// `PERF_BODY=… cargo test --release --bin zeron-zgx perf_resize_pane -- --ignored --nocapture`.
#[test]
#[ignore]
fn perf_resize_pane() {
    let path = std::env::var("PERF_BODY").expect("PERF_BODY");
    let long: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for case in ["pr", "empty"] {
        let items = vec![builtin("github")];
        let detail = json!({ "title": "Bump the github-actions group across 2 directories with 9 updates", "body": long["body"], "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264, "state": "OPEN", "mergeStateStatus": "CLEAN", "headRefName": "dependabot/x", "baseRefName": "main", "allCommits": { "totalCount": 1 } });
        let (mut app, _) = shell_with(items.clone(), world(json!({ "viewer": "ada", "pr": detail, "repo": { "viewerPermission": "WRITE" } })));
        publish_records(&app, &items);
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
        app.frames(10);
        app.host.set_state("shell_fixture.plugins", json_value(&json!(true))).unwrap();
        app.frames(5);
        let (w, h) = (1265., 821.);
        app.ui.scene.borrow_mut().resize(w, h);
        if case == "pr" {
            let request = json!([{ "seq": 1, "kind": "panel_tab", "title": "GitHub", "value": { "plugin": "github", "session": "a1", "kind": "github-pr", "title": "T", "icon": "pull-request", "data": { "url": "https://github.com/zeronsh/zgx/pull/1264", "number": 1264 } } }]);
            app.host.shared("plugins.request", zgx_script::Value::Nil).set(json_value(&request));
            for _ in 0..12 {
                if app.wait_until(|app| app.target("#panel >> #github-pr-merge").is_ok()) {
                    break;
                }
            }
        } else {
            assert!(app.host.run_command("shell.toggle_files"));
        }
        let mut gpu = zgx_gpu::GpuRenderer::new((w * 2.) as u32, (h * 2.) as u32).unwrap();
        gpu.set_scale_factor(2.);
        for font in crate::fonts() {
            font.unwrap().install(&mut gpu.text_system().borrow_mut());
        }
        gpu.install_text(&mut app.ui.scene.borrow_mut());
        app.ui.refresh_text_geometry();
        app.frames(60);
        let _ = app.ui.scene.borrow_mut().flush();
        let all = [zgx::scene::Rect { x: 0., y: 0., width: w, height: h }];
        gpu.render(&app.ui.scene.borrow(), &all).unwrap();
        // The edge dragged to and fro, a few points a frame, around 640.
        let start: f64 = app.host.states().iter().find(|(n, _)| n.ends_with("shell_files_width")).and_then(|(_, v)| v.to_json().parse().ok()).unwrap_or(760.);
        let (mut cpu_t, mut gpu_t, mut worst) = (std::time::Duration::ZERO, std::time::Duration::ZERO, std::time::Duration::ZERO);
        let (mut layouts, mut paints) = (0usize, 0usize);
        let (mut passes, mut blurs, mut draws, mut instances, mut shaped, mut svgs, mut canvases) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let n = 120;
        for i in 0..n {
            let off = ((i as f64) * 0.25).sin() * 60.;
            app.host.set_state("shell_files_width", zgx_script::Value::from(start + off)).unwrap();
            let t = std::time::Instant::now();
            app.frame();
            let report = app.ui.scene.borrow_mut().flush();
            let c = t.elapsed();
            let st = gpu.render(&app.ui.scene.borrow(), &report.damage).unwrap();
            passes += st.render_passes;
            blurs += st.blur_passes;
            draws += st.draw_calls;
            instances += st.instances;
            shaped += st.shaped_nodes;
            svgs += st.svg_rasterizations;
            canvases += st.canvas_rasterizations;
            let g = t.elapsed() - c;
            cpu_t += c;
            gpu_t += g;
            worst = worst.max(c + g);
            layouts += report.layout_nodes;
            paints += report.paint_nodes;
        }
        eprintln!(
            "PERF resize {case}: per frame cpu {:?} (layout+paint+script), gpu-encode {:?}, worst {worst:?}; nodes laid out {} painted {} a frame; tree {} lines; error {:?}",
            cpu_t / n,
            gpu_t / n,
            layouts / n as usize,
            paints / n as usize,
            app.tree().lines().count(),
            app.error()
        );
        let k = n as usize;
        eprintln!(
            "PERF gpu {case}: per frame render passes {} (blur {}), draw calls {}, instances {}, text nodes shaped {}, svgs rasterized {}, canvases rasterized {}",
            passes / k, blurs / k, draws / k, instances / k, shaped / k, svgs / k, canvases / k
        );
    }
}

// -- Zeron's Git plugin (plugins-builtin/git) ---------------------------------

/// A checkout as the device's Git service answers for it: its status, what
/// changed in each scope, a file's diff, its history and branches. `fail`
/// (method → "reason: words") refuses a method as the engine would.
fn git_world() -> serde_json::Value {
    let file = |path: &str, status: &str, add: u64, del: u64, stage: &str, rev: &str| {
        json!({ "path": path, "status": status, "add": add, "del": del, "stage": stage, "rev": rev })
    };
    json!({
        "status": {
            "branch": "feature/graph", "oid": "a".repeat(40), "upstream": "origin/feature/graph",
            "ahead": 2, "behind": 0, "pushed": false, "base": "origin/main", "base_ahead": 3, "base_behind": 0,
            "conflicts": 0, "changes": 4, "tree": "t1", "index": "i1", "device": "devbox", "remote": "origin",
            "remote_url": "git@github.com:zeronsh/zeron.git", "who": "Ada Lovelace",
        },
        "changes": {
            "working": { "scope": "working", "a": "a".repeat(40), "b": "t1", "files": [
                file("src/graph.rs", "modified", 12, 3, "all", "r1"),
                file("src/lib.rs", "modified", 1, 1, "some", "r2"),
                file("docs/history.md", "added", 40, 0, "none", "r3"),
                file("src/old.rs", "deleted", 0, 20, "none", "r4"),
            ] },
            "branch": { "scope": "branch", "a": "b".repeat(40), "b": "t1", "base": "origin/main", "files": [
                file("src/graph.rs", "modified", 30, 3, "", "r1"),
            ] },
            "turn": { "scope": "turn", "a": "c".repeat(40), "b": "t1", "files": [] },
            "commit": { "scope": "commit", "a": "d".repeat(40), "b": "e".repeat(40), "files": [
                file("src/graph.rs", "added", 80, 0, "", "r9"),
            ] },
        },
        "diff": { "text": "diff --git a/src/graph.rs b/src/graph.rs\n--- a/src/graph.rs\n+++ b/src/graph.rs\n@@ -1,3 +1,4 @@\n fn lanes() {}\n-fn rows() {}\n+fn rows(n: usize) {}\n+fn lines() {}\n" },
        "history": { "more": false, "commits": [
            { "sha": "1".repeat(40), "parents": ["2".repeat(40), "4".repeat(40)], "author": "Ada Lovelace", "email": "ada@example.com", "at": 1_790_000_000, "refs": ["HEAD -> feature/graph", "origin/feature/graph"], "subject": "Merge main into feature/graph" },
            { "sha": "2".repeat(40), "parents": ["3".repeat(40)], "author": "Ada Lovelace", "email": "ada@example.com", "at": 1_789_990_000, "refs": [], "subject": "Draw the lanes" },
            { "sha": "4".repeat(40), "parents": ["3".repeat(40)], "author": "Grace Hopper", "email": "grace@example.com", "at": 1_789_980_000, "refs": ["origin/main"], "subject": "Fix the build" },
            { "sha": "3".repeat(40), "parents": [], "author": "Grace Hopper", "email": "grace@example.com", "at": 1_789_970_000, "refs": ["tag: v1.0"], "subject": "First" },
        ] },
        "branches": { "branches": [
            { "name": "feature/graph", "remote": false, "current": true, "ahead": 2, "behind": 0, "at": 1_790_000_000 },
            { "name": "main", "remote": false, "current": false, "ahead": 0, "behind": 1, "at": 1_789_980_000 },
            { "name": "origin/release", "remote": true, "local_name": "release", "current": false, "at": 1_789_000_000 },
        ] },
        "fail": {},
    })
}

/// What the plugin asked of its service, in order: `{ method, args, device }`.
fn service_calls(heard: &Heard) -> Vec<serde_json::Value> {
    heard.borrow().iter().filter(|(k, _)| k == "service").map(|(_, v)| serde_json::from_str(v).unwrap()).collect()
}

fn asked_service(heard: &Heard, method: &str) -> Vec<serde_json::Value> {
    service_calls(heard).into_iter().filter(|c| c["method"] == method).collect()
}

/// The Git tab in a plugin tab's place (ui/git_fixture.luau), session s1 on
/// device "box" in /work/atlas, its service played by `service` (and
/// GitHub, when `with_github`, played by `github`).
fn git_tab(service: serde_json::Value, github: Option<serde_json::Value>) -> (TestApp, Heard, World) {
    // The session as each plugin may read it: s1, its folder on "box".
    let records = json!({ "sessions": { "s1": {
        "title": "Fix the build", "status": "done", "device": "box", "cwd": "/work/atlas",
        "branch": "feature/graph", "project": "atlas",
    } }, "files_read": true });
    git_in("git_fixture.luau", records, service, github)
}

/// The Git plugin (and GitHub's, with `github`) in `fixture` (ui/), the
/// session records `records`, the device's service played by `service`.
fn git_in(fixture: &str, records: serde_json::Value, service: serde_json::Value, github: Option<serde_json::Value>) -> (TestApp, Heard, World) {
    let entry = crate::ui_dir().join(fixture);
    let mut items = vec![builtin("git")];
    if github.is_some() {
        items.push(builtin("github"));
    }
    let service = world(service);
    let gh = world(github.unwrap_or(json!({ "viewer": "ada" })));
    let calls: Heard = Heard::default();
    let heard = calls.clone();
    let played = service.clone();
    let app = TestApp::new(move |runtime| {
        let host = super::lend_design_system(ScriptHost::new(runtime, &entry), &entry);
        host.shared("zgx.window.size", zgx_script::window_size_value(1200., 800.));
        offline(&host);
        expose_surface(&host);
        let mut plugins = Vec::new();
        for item in &items {
            let id = item.id().to_string();
            expose_own(&host, &id);
            super::locales::publish(&host, &id, Path::new(item.json["snapshot"].as_str().unwrap()));
            for name in ["result", "ack", "menus", "toast", "problem", "open_tab", "add_page", "github_watch", "store_set", "cost", "open_url", "notify", "compose_comment"] {
                let (calls, key) = (calls.clone(), format!("plugin.{id}.{name}"));
                host.action(key.clone(), move |v: zgx_script::Value| calls.borrow_mut().push((key.clone(), v.to_json())));
            }
            host.shared(&format!("plugin.{id}.records"), zgx_script::Value::Nil).set(json_value(&records));
            fake_github(&host, &id, gh.clone(), calls.clone());
            plugins.extend(item.plugin());
        }
        // Menus a plugin offers others (plugin_menus), routed as the bridge
        // routes them: each plugin's items kept, each plugin told those for
        // its own menus, one chosen run in the plugin that added it.
        let all_menus: std::rc::Rc<std::cell::RefCell<std::collections::BTreeMap<String, Vec<serde_json::Value>>>> = Default::default();
        let ids: Vec<String> = items.iter().map(|i| i.id().to_string()).collect();
        let publish = {
            let host = host.clone();
            let (all_menus, ids) = (all_menus.clone(), ids.clone());
            move || {
                let all: Vec<zgx_script::Value> = all_menus.borrow().values().flatten().map(json_value).collect();
                for id in &ids {
                    let mine: Vec<serde_json::Value> = super::plugin_menus::offered_to(id, &all).iter().map(value_json).collect();
                    host.shared(&format!("plugin.{id}.menu_items"), json_value(&json!([]))).set(json_value(&json!(mine)));
                }
            }
        };
        let publish = std::rc::Rc::new(publish);
        for item in &items {
            let id = item.id().to_string();
            let name = item.json["name"].clone();
            let (all_menus, publish, calls) = (all_menus.clone(), publish.clone(), calls.clone());
            host.action(format!("plugin.{id}.menus"), move |list: zgx_script::Value| {
                calls.borrow_mut().push((format!("plugin.{id}.menus"), list.to_json()));
                let list: serde_json::Value = serde_json::from_str(&list.to_json()).unwrap_or_default();
                let mine = list.as_array().into_iter().flatten().map(|m| json!({ "plugin": id, "plugin_name": name, "kind": m["kind"], "id": m["id"], "label": m["label"] })).collect();
                all_menus.borrow_mut().insert(id.clone(), mine);
                publish();
            });
        }
        for id in &ids {
            let (all_menus, host2, offering) = (all_menus.clone(), host.clone(), id.clone());
            let seq = std::rc::Rc::new(std::cell::Cell::new(0));
            host.action(format!("plugin.{id}.menu_run"), move |asked: zgx_script::Value| {
                let all: Vec<zgx_script::Value> = all_menus.borrow().values().flatten().map(json_value).collect();
                let Some((target, item)) = super::plugin_menus::route(&offering, &asked, &all) else { return };
                seq.set(seq.get() + 1);
                let context: serde_json::Value = asked.field("context").map(|c| serde_json::from_str(&c.to_json()).unwrap_or_default()).unwrap_or_default();
                let calls_state = host2.shared(&format!("plugin.{target}.calls"), json_value(&json!([])));
                let mut now: serde_json::Value = serde_json::from_str(&calls_state.signal().with_untracked(zgx_script::Value::clone).to_json()).unwrap_or(json!([]));
                if !now.is_array() {
                    now = json!([]);
                }
                now.as_array_mut().unwrap().push(json!({ "id": format!("menu-{}", seq.get()), "kind": "menu", "item": item, "args": context }));
                calls_state.set(json_value(&now));
            });
        }
        // The device's Git service: each call heard, answered as `service` says.
        let (w, h) = (played.clone(), calls.clone());
        host.async_action("plugin.git.service_call", move |asked: zgx_script::Value| {
            let asked: serde_json::Value = serde_json::from_str(&asked.to_json()).unwrap_or_default();
            h.borrow_mut().push(("service".into(), asked.to_string()));
            let method = asked["method"].as_str().unwrap_or_default().to_string();
            let world = w.borrow();
            let answer = if let Some(why) = world["fail"][&method].as_str() {
                Err(why.to_string())
            } else if method == "changes" {
                let scope = asked["args"]["scope"].as_str().unwrap_or("working");
                Ok(world["changes"][scope].clone())
            } else if world.get(&method).is_some() {
                Ok(world[&method].clone())
            } else {
                Ok(json!(true))
            };
            async move { answer.map(|v| json_value(&v)) }
        });
        host.set_plugins(plugins);
        host
    });
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(10);
    (app, heard, service)
}

/// The Git service on "box" says `data` (`zeron.emit("changed", data)`), as
/// the host's inbox brings another device's events.
fn tell_git(app: &mut TestApp, data: serde_json::Value) {
    tell_git_n(app, data, 1);
}

/// `data` said `n` times at once (before the window draws again).
fn tell_git_n(app: &mut TestApp, data: serde_json::Value, n: usize) {
    let inbox = app.host.shared("plugin.git.inbox", json_value(&json!([])));
    let mut now: serde_json::Value = serde_json::from_str(&inbox.signal().with_untracked(zgx_script::Value::clone).to_json()).unwrap_or(json!([]));
    if !now.is_array() {
        now = json!([]);
    }
    let list = now.as_array_mut().unwrap();
    for _ in 0..n {
        let seq = list.len() + 1;
        list.push(json!({ "seq": seq, "from": { "id": "box", "name": "box" }, "event": "changed", "here": false, "data": data, "at": 0 }));
    }
    inbox.set(json_value(&now));
    app.frames(2);
}

fn git_fixture(app: &mut TestApp, name: &str, value: zgx_script::Value) {
    app.host.set_state(&format!("git_fixture.{name}"), value).unwrap();
    app.frames(10);
}

#[test]
fn git_s_changes_show_a_session_s_work_tree_on_its_device() {
    let (mut app, heard, _) = git_tab(git_world(), None);
    assert_eq!(app.error(), None);
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()), "{}", app.tree());
    // Asked of the service on the session's device, in its folder.
    let status = asked_service(&heard, "status");
    assert!(!status.is_empty() && status[0]["device"] == "box" && status[0]["args"]["cwd"] == "/work/atlas", "{status:?}");
    let changes = asked_service(&heard, "changes");
    assert_eq!(changes[0]["args"]["scope"], "working", "{changes:?}");
    let tree = app.tree();
    // The branch, how far ahead of its base, push, the files, the commit.
    // (How far it is from its base: History's pill, the branch switcher's.)
    let missing: Vec<&str> = ["Push 2", "graph.rs", "history.md", "old.rs", "Commit 2 files", "Ada Lovelace", "What changed, and why"]
        .into_iter()
        .filter(|t| !tree.contains(t))
        .collect();
    assert!(missing.is_empty(), "{missing:?}: {tree}");
    // (The branch's chip rolls its name in a letter at a time.)
    assert!(app.find("#git-branch >> role:button \"Branch\"").is_some());
    assert!(!tree.contains("\"placeholder\""), "the plugin fills its tab: {tree}");
    // A file's diff, asked between the trees the list came from, beside
    // the list at the window's width.
    assert!(app.wait_until(|_| !asked_service(&heard, "diff").is_empty()));
    let diff = &asked_service(&heard, "diff")[0];
    assert_eq!((diff["args"]["b"].as_str(), diff["args"]["path"].as_str()), (Some("t1"), Some("src/graph.rs")), "{diff}");
    assert!(app.wait_until(|app| app.tree().contains("fn rows(n: usize) {}")), "{}", app.tree());
    // Its device says it changed, and nothing moved: its status is asked
    // again, the files aren't.
    let (before, asked) = (asked_service(&heard, "changes").len(), asked_service(&heard, "status").len());
    tell_git(&mut app, json!({ "root": "/work/atlas", "cwds": ["/work/atlas"] }));
    assert!(app.wait_until(|_| asked_service(&heard, "status").len() > asked));
    app.frames(10);
    assert_eq!(asked_service(&heard, "changes").len(), before, "the files are asked again only as they move");
    assert_eq!(app.error(), None);
}

#[test]
fn git_asks_again_only_as_its_device_says_and_while_it_shows() {
    let (mut app, heard, _) = git_tab(git_world(), None);
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()), "{}", app.tree());
    // Asked with its device keeping watch, and listening for what it says.
    assert_eq!(asked_service(&heard, "status")[0]["args"]["watch"], json!(true));
    assert!(!asked_service(&heard, "@watch").is_empty(), "{:?}", service_calls(&heard));
    let status = || asked_service(&heard, "status").len();
    app.frames(30);
    let quiet = status();
    // Nothing heard: nothing asked (no loop on its own answers), for half
    // a minute.
    app.frames(30 * 60);
    assert_eq!(status(), quiet, "asked with nothing heard");
    // A change said: asked once.
    tell_git(&mut app, json!({ "root": "/work/atlas", "cwds": ["/work/atlas"] }));
    assert!(app.wait_until(|_| status() == quiet + 1));
    app.frames(120);
    assert_eq!(status(), quiet + 1, "one change, one ask");
    // Another checkout's change: nothing.
    tell_git(&mut app, json!({ "root": "/work/other", "cwds": ["/work/other"] }));
    app.frames(120);
    assert_eq!(status(), quiet + 1, "asked for another checkout's change");
    // Changes said while it's asking: once more after, not once each.
    tell_git_n(&mut app, json!({ "root": "/work/atlas", "cwds": ["/work/atlas"] }), 5);
    assert!(app.wait_until(|_| status() >= quiet + 2));
    app.frames(120);
    assert_eq!(status(), quiet + 3, "five said at once: asked, then once more");
    let shown = status();
    // The window hidden: a change is kept, not asked.
    let window = app.host.shared("zgx.window.shown", zgx_script::Value::Bool(true));
    window.set(zgx_script::Value::Bool(false));
    app.frames(5);
    tell_git(&mut app, json!({ "root": "/work/atlas", "cwds": ["/work/atlas"] }));
    app.frames(120);
    assert_eq!(status(), shown, "asked while the window is hidden");
    // Shown again: asked once, for what changed meanwhile.
    window.set(zgx_script::Value::Bool(true));
    assert!(app.wait_until(|_| status() == shown + 1));
    app.frames(120);
    assert_eq!(status(), shown + 1);
    // A minute with nothing heard: asked once, the net under a missed one.
    app.frames(61 * 60);
    assert_eq!(status(), shown + 2, "the minute's safety ask");
    // Another app in front, then this one again: asked once as it comes back.
    let front = app.host.shared("zgx.window.active", zgx_script::Value::Bool(true));
    front.set(zgx_script::Value::Bool(false));
    app.frames(30);
    assert_eq!(status(), shown + 2, "asked as it went behind");
    front.set(zgx_script::Value::Bool(true));
    assert!(app.wait_until(|_| status() == shown + 3));
    app.frames(120);
    assert_eq!(status(), shown + 3);
    // The tab closed: nothing more, a minute on.
    git_fixture(&mut app, "mode", zgx_script::Value::from("empty"));
    let closed = status();
    tell_git(&mut app, json!({ "root": "/work/atlas", "cwds": ["/work/atlas"] }));
    app.frames(61 * 60);
    assert_eq!(status(), closed, "asked with no tab showing it");
    assert_eq!(app.error(), None);
}

#[test]
fn git_stages_and_discards_after_asking() {
    let (mut app, heard, world) = git_tab(git_world(), None);
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()), "{}", app.tree());
    // A box: staged at once, asked of the service (and so read back).
    world.borrow_mut()["changes"]["working"]["files"][2]["stage"] = json!("all");
    app.click("role:checkbox \"docs/history.md\"");
    assert!(app.wait_until(|_| !asked_service(&heard, "stage").is_empty()));
    assert_eq!(asked_service(&heard, "stage")[0]["args"]["paths"], json!(["docs/history.md"]));
    assert!(app.tree().contains("Commit 3 files"), "{}", app.tree());
    // Discarding asks first, and only the confirm does it.
    // (Its Discard shows under the pointer, as a person's does.)
    app.hover("role:button \"src/graph.rs\"");
    app.frames(5);
    app.click("role:button \"src/graph.rs\" >> role:button \"Discard changes\"");
    assert!(app.wait_until(|app| app.find("role:dialog \"Discard the changes to graph.rs?\"").is_some()), "{}", app.tree());
    assert!(asked_service(&heard, "discard").is_empty());
    // (Once it has arrived: it moves as it opens.)
    app.frames(40);
    app.click("role:dialog \"Discard the changes to graph.rs?\" >> role:button \"Discard\"");
    assert!(app.wait_until(|_| !asked_service(&heard, "discard").is_empty()));
    assert_eq!(asked_service(&heard, "discard")[0]["args"]["paths"], json!(["src/graph.rs"]));
    assert_eq!(app.error(), None);
}

#[test]
fn git_commits_and_pushes_and_a_refused_push_pulls_first() {
    let (mut app, heard, world) = git_tab(git_world(), None);
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()), "{}", app.tree());
    // A commit, with its message; then its push.
    app.type_text("role:textbox \"Commit message\"", "Draw the lanes");
    app.frames(5);
    app.click("#git-commit >> role:button \"Commit 2 files\"");
    assert!(app.wait_until(|_| !asked_service(&heard, "commit").is_empty()), "{}", app.tree());
    assert_eq!(asked_service(&heard, "commit")[0]["args"]["message"], "Draw the lanes");
    // A push the remote refuses: why, and Pull, then push.
    world.borrow_mut()["fail"]["push"] = json!("failed: rejected: ! [rejected] feature/graph (fetch first)");
    app.click("role:button \"Push 2\"");
    assert!(app.wait_until(|app| app.tree().contains("The branch moved on origin")), "{:?}\n{}", service_calls(&heard).iter().map(|c| c["method"].to_string()).collect::<Vec<_>>(), app.tree());
    assert!(app.tree().contains("Pull, then push"), "{}", app.tree());
    world.borrow_mut()["fail"] = json!({});
    app.click("role:button \"Pull, then push\"");
    assert!(app.wait_until(|_| !asked_service(&heard, "pull").is_empty() && asked_service(&heard, "push").len() >= 2), "{:?}", service_calls(&heard));
    assert_eq!(app.error(), None);
}

#[test]
fn git_says_why_a_device_can_t_answer_and_tries_again() {
    for (fail, title) in [
        ("offline: box can't be reached right now", "Can't reach this chat's device"),
        ("not_approved: Git is waiting to be allowed on box", "Git isn't allowed on this chat's device yet"),
        ("not_installed: git isn't installed on box", "Git isn't on this chat's device"),
        ("version_mismatch: box has Git 0.9.0", "Another version of Git is on this chat's device"),
        ("failed: not_a_repo: fatal: not a git repository", "Not a git repository"),
    ] {
        let mut w = git_world();
        w["fail"]["status"] = json!(fail);
        let (mut app, heard, world) = git_tab(w, None);
        assert!(app.wait_until(|app| app.tree().contains(title)), "{fail}: {}", app.tree());
        let retry = app.find("role:button \"Try again\"").is_some();
        assert_eq!(retry, fail.starts_with("offline"), "{fail}: only what trying again can help offers it");
        if retry {
            world.borrow_mut()["fail"] = json!({});
            let asked = asked_service(&heard, "status").len();
            app.frames(30);
            app.click("role:button \"Try again\"");
            assert!(app.wait_until(|app| asked_service(&heard, "status").len() > asked && app.find("role:checkbox \"src/graph.rs\"").is_some()), "{:?} {}", service_calls(&heard), app.tree());
        }
        assert_eq!(app.error(), None, "{fail}");
    }
}

#[test]
fn git_s_scopes_branch_and_last_turn() {
    let (mut app, heard, _) = git_tab(git_world(), None);
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()));
    app.click("#git-scope >> role:button \"What changed\"");
    app.frames(10);
    app.click("role:listbox \"What changed\" >> \"Branch\"");
    assert!(app.wait_until(|_| asked_service(&heard, "changes").iter().any(|c| c["args"]["scope"] == "branch")), "{:?}", service_calls(&heard));
    assert!(app.wait_until(|app| !app.tree().contains("history.md")), "only the branch's files: {}", app.tree());
    // The branch has no commit box: commits are the work tree's.
    assert!(app.find("#git-commit").is_none());
    app.click("#git-scope >> role:button \"What changed\"");
    app.frames(10);
    app.click("role:listbox \"What changed\" >> \"Last turn\"");
    assert!(app.wait_until(|app| app.tree().contains("Nothing changed this turn")), "{:?} {}", service_calls(&heard), app.tree());
    let turn = asked_service(&heard, "changes").into_iter().find(|c| c["args"]["scope"] == "turn").unwrap();
    assert_eq!(turn["args"]["chat"], "s1", "the turn is this chat's: {turn}");
}

#[test]
fn git_s_history_draws_the_graph_and_opens_a_commit() {
    let (mut app, heard, _) = git_tab(git_world(), None);
    history_of(&mut app);
    assert!(app.wait_until(|app| app.find("role:button \"Merge main into feature/graph\"").is_some()), "{}", app.tree());
    let tree = app.tree();
    // A line each: subject, refs (HEAD's branch, a remote's, a tag), when.
    // (The list stands beside the commit here: no room for ids.)
    let missing: Vec<&str> = ["Merge main into feature/graph", "Draw the lanes", "Fix the build", "First", "v1.0", "origin/main", "feature/graph"]
        .into_iter()
        .filter(|t| !tree.contains(t))
        .collect();
    assert!(missing.is_empty(), "{missing:?}: {tree}");
    // Git alone: no pull request, no picture.
    assert!(!app.tree().contains("Open pull request") && !app.tree().contains("on GitHub"), "{}", app.tree());
    // A commit clicked: a tab of its own (Zeron's commit tab), titled by
    // its subject, known by its id; the list stays as it was.
    app.click("role:button \"Merge main into feature/graph\"");
    app.frames(5);
    let tabs = || heard.borrow().iter().filter(|(k, _)| k == "plugin.git.open_tab").map(|(_, v)| serde_json::from_str::<serde_json::Value>(v).unwrap()).collect::<Vec<_>>();
    let tab = tabs().pop().unwrap_or_else(|| panic!("a tab asked for: {:?}", heard.borrow()));
    assert_eq!((tab["kind"].as_str(), tab["title"].as_str(), tab["icon"].as_str(), tab["data"]["key"].as_str()), (Some("git-commit"), Some("Merge main into feature/graph"), Some("git-commit"), Some(format!("commit:{}", "1".repeat(40)).as_str())), "{tab}");
    assert_eq!(tab["data"]["commit"]["sha"], "1".repeat(40));
    assert!(app.find("#git-commit-head").is_none() && app.find("#git-back").is_none(), "no commit in the list's place: {}", app.tree());
    // Clicked again: asked again, the same key (the panel brings it forward).
    app.click("role:button \"Merge main into feature/graph\"");
    app.frames(5);
    assert_eq!(tabs().len(), 2);
    assert_eq!(tabs()[1]["data"]["key"], tab["data"]["key"]);
    // The commit's tab: its own files, asked by its sha; no band of the
    // checkout's over it.
    app.host.set_state("git_fixture.commit", json_value(&tab["data"]["commit"])).unwrap();
    git_fixture(&mut app, "view", zgx_script::Value::from("commit"));
    assert!(app.wait_until(|_| asked_service(&heard, "changes").iter().any(|c| c["args"]["scope"] == "commit")), "{:?}", service_calls(&heard));
    let asked = asked_service(&heard, "changes").into_iter().find(|c| c["args"]["scope"] == "commit").unwrap();
    assert_eq!(asked["args"]["sha"], "1".repeat(40));
    // (Its files fade in: there, if not yet seen.)
    assert!(app.wait_until(|app| app.tree().contains("#git-commit-files")), "{}", app.tree());
    assert!(app.find("#git-head").is_none(), "{}", app.tree());
    // Its head's words stand on the files card's words, the card's box
    // hanging past them; the id chip's word too.
    // (The card's first word: its first file's status letter.)
    assert!(app.wait_until(|app| app.find("#git-commit-files >> \"A\"").is_some()), "{}", app.tree());
    let (subject, card_word) = (app.bounds("#git-commit-head >> \"Merge main into feature/graph\""), app.bounds("#git-commit-files >> \"A\""));
    assert!((subject.x - card_word.x).abs() < 0.5, "text on text: {subject:?} {card_word:?}");
    let sha = app.bounds(&format!("#git-commit-facts >> \"{}\"", "1".repeat(7)));
    assert!((sha.x - card_word.x).abs() < 0.5, "the id's word on it too: {sha:?} {card_word:?}");
    assert_eq!(app.error(), None);
}

#[test]
fn git_s_graph_lanes_follow_branches_and_merges() {
    // (The layout alone, run in a VM: graph.luau is plain Luau.)
    let mut app = TestApp::inline(&format!(
        r#"
local graph = (function() {} end)()
local rows, widest = graph.layout({{
  {{ sha = "m", parents = {{ "a", "b" }} }},
  {{ sha = "a", parents = {{ "o" }} }},
  {{ sha = "b", parents = {{ "o" }} }},
  {{ sha = "o", parents = {{}} }},
}})
local out = {{}}
for i, r in rows do
  local lines = {{}}
  for _, l in r.lines do table.insert(lines, `{{l.a}}>{{l.b}}@{{l.from}}-{{l.to}}`) end
  table.sort(lines)
  table.insert(out, `{{r.col}}:{{table.concat(lines, ",")}}`)
end
return ui.text {{ id = "out", `{{widest}}|` .. table.concat(out, " ") }}
"#,
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins-builtin/git/graph.luau")).unwrap()
    ));
    // The merge in lane 1, its second parent opening lane 2; each parent
    // down its lane; both meeting again at their parent, in lane 1.
    assert_eq!(app.text("out"), "2|1:1>1@0.5-1,1>2@0.5-1 1:1>1@0-0.5,1>1@0.5-1,2>2@0-1 2:1>1@0-1,2>2@0-0.5,2>2@0.5-1 1:1>1@0-0.5,2>1@0-0.5");
}

#[test]
fn git_s_empty_page_rows_open_its_tab() {
    let (mut app, heard, _) = git_tab(git_world(), None);
    git_fixture(&mut app, "mode", zgx_script::Value::from("empty"));
    assert!(app.wait_until(|app| app.find("#git-open-changes").is_some()), "{}", app.tree());
    // After the panel's own rows, as they are.
    let tree = app.tree();
    assert!(tree.find("Browser").unwrap() < tree.find("\"Changes\"").unwrap(), "{tree}");
    app.click("#git-open-history");
    app.frames(5);
    let (_, tab) = heard.borrow().iter().find(|(k, _)| k == "plugin.git.open_tab").cloned().expect("a tab asked for");
    let tab: serde_json::Value = serde_json::from_str(&tab).unwrap();
    assert_eq!((tab["kind"].as_str(), tab["title"].as_str(), tab["icon"].as_str(), tab["data"]["session"].as_str()), (Some("git-history"), Some("History"), Some("git-commit"), Some("s1")), "{tab}");
    assert!(super::surfaces::tab(&tab).is_ok(), "the host refuses it: {tab}");
    // Their letters, C and H, as the home hears them (`panel_key`): a press
    // is a new `n`; one already there as the rows appear isn't.
    assert!(app.tree().contains("\"C\"") && app.tree().contains("\"H\""), "{}", app.tree());
    let opened = |heard: &Heard| heard.borrow().iter().filter(|(k, _)| k == "plugin.git.open_tab").map(|(_, v)| serde_json::from_str::<serde_json::Value>(v).unwrap()).collect::<Vec<_>>();
    app.host.set_state("git_fixture.panel_key", json_value(&json!({ "key": "C", "n": 1 }))).unwrap();
    assert!(app.wait_until(|_| opened(&heard).len() == 2), "{:?}", opened(&heard));
    assert_eq!((opened(&heard)[1]["kind"].as_str(), opened(&heard)[1]["icon"].as_str()), (Some("git-changes"), Some("file-diff")));
    app.host.set_state("git_fixture.panel_key", json_value(&json!({ "key": "B", "n": 2 }))).unwrap();
    app.frames(10);
    assert_eq!(opened(&heard).len(), 2, "not Git's letter");
    app.host.set_state("git_fixture.panel_key", json_value(&json!({ "key": "h", "n": 3 }))).unwrap();
    assert!(app.wait_until(|_| opened(&heard).len() == 3));
    assert_eq!(opened(&heard)[2]["kind"], "git-history");
}

#[test]
fn github_puts_create_pull_request_and_its_pull_request_in_git_s_slot() {
    // A pushed branch ahead of main, no pull request yet: Create pull
    // request, opening GitHub's own page for it.
    let github = json!({ "viewer": "ada", "repository": "zeronsh/zeron", "branches": {} });
    let (mut app, heard, _) = git_tab(git_world(), Some(github.clone()));
    assert_eq!(app.error(), None);
    assert!(app.wait_until(|app| app.find(".git-branch-actions >> #github-git-create-pr").is_some()), "{}", app.tree());
    app.click("#github-git-create-pr");
    app.frames(5);
    let opened: Vec<String> = heard.borrow().iter().filter(|(k, _)| k == "plugin.github.open_url").map(|(_, v)| v.clone()).collect();
    assert!(opened.iter().any(|u| u.contains("https://github.com/zeronsh/zeron/compare/main...feature/graph?expand=1")), "{opened:?}");
    // GitHub has one now: its badge in Create's place, a click opening it in
    // the chat's panel.
    let (mut app, heard, _) = git_tab(
        git_world(),
        Some(json!({ "viewer": "ada", "repository": "zeronsh/zeron", "branches": { "feature/graph": branch_pr(77, "OPEN", json!({ "headRefName": "feature/graph", "title": "Graph lanes" })) } })),
    );
    assert!(refresh_until(&mut app, |app| app.find(".git-branch-actions >> .github-pr-badge").is_some()), "{}", app.tree());
    assert!(app.find("#github-git-create-pr").is_none());
    app.click(".github-pr-badge");
    app.frames(5);
    let (_, tab) = heard.borrow().iter().find(|(k, _)| k == "plugin.github.open_tab").cloned().expect("a tab asked for");
    let tab: serde_json::Value = serde_json::from_str(&tab).unwrap();
    assert_eq!((tab["kind"].as_str(), tab["data"]["number"].as_f64(), tab["session"].as_str()), (Some("github-pr"), Some(77.), Some("s1")), "{tab}");
}

#[test]
fn git_s_tab_opens_to_the_scope_it_was_asked_for() {
    // (The panel home's branch row: its +N −M opens the branch's changes.)
    let (mut app, heard, _) = git_tab(git_world(), None);
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()), "{}", app.tree());
    git_fixture(&mut app, "scope", zgx_script::Value::from("branch"));
    assert!(app.wait_until(|_| asked_service(&heard, "changes").iter().any(|c| c["args"]["scope"] == "branch")), "{:?}", service_calls(&heard));
    assert!(app.wait_until(|app| !app.tree().contains("history.md")), "{}", app.tree());
}

#[test]
fn git_puts_the_branch_s_totals_in_the_panel_home_and_opens_its_changes() {
    // The real panel home (ui/panel_home_fixture.luau), chat-1 on device
    // "d" in /proj on zeron/panel-home; Git's service says what it adds.
    let records = json!({ "sessions": { "chat-1": {
        "title": "Parent", "status": "idle", "device": "d", "cwd": "/proj", "branch": "zeron/panel-home", "project": "p1",
    } }, "files_read": true });
    let mut service = git_world();
    service["branch_totals"] = json!({ "base": "main", "adds": 2034, "dels": 51 });
    let (mut app, heard, _) = git_in("panel_home_fixture.luau", records, service, None);
    assert_eq!(app.error(), None);
    assert!(app.wait_until(|app| app.find("#git-branch-totals").is_some()), "{}", app.tree());
    let tree = app.tree();
    assert!(tree.contains("+2034") && tree.contains("−51"), "{tree}");
    // Asked of the session's device, in its folder: the plugin's service, not core.
    let asked = asked_service(&heard, "branch_totals");
    assert_eq!((asked[0]["device"].as_str(), asked[0]["args"]["cwd"].as_str()), (Some("d"), Some("/proj")), "{asked:?}");
    // In the branch's row, after its name.
    assert!(app.find("#panel-home-branch >> #git-branch-totals").is_some(), "{tree}");
    // A click: Changes, at the branch's scope.
    app.click("#git-branch-totals");
    app.frames(5);
    let (_, tab) = heard.borrow().iter().find(|(k, _)| k == "plugin.git.open_tab").cloned().expect("a tab asked for");
    let tab: serde_json::Value = serde_json::from_str(&tab).unwrap();
    assert_eq!((tab["kind"].as_str(), tab["session"].as_str()), (Some("git-changes"), Some("chat-1")), "{tab}");
    // The scope is the session's, set as it opens: a Changes tab already
    // there, brought forward, shows the branch too.
    assert!(app.wait_until(|_| asked_service(&heard, "changes").iter().any(|c| c["args"]["scope"] == "branch")), "{:?}", service_calls(&heard));
}

/// What Git asked the composer to carry (`zeron.compose_comment`), in order.
fn composed_comments(heard: &Heard) -> Vec<serde_json::Value> {
    heard.borrow().iter().filter(|(k, _)| k == "plugin.git.compose_comment").map(|(_, v)| serde_json::from_str(v).unwrap()).collect()
}

#[test]
fn git_s_diff_comments_go_to_the_chat_s_draft_and_mark_their_lines() {
    // The window's width: src/graph.rs chosen, its diff beside the list:
    // " fn lanes() {}" (1), "-fn rows() {}" (old 2), "+fn rows(n: usize) {}"
    // (2), "+fn lines() {}" (3).
    let (mut app, heard, _) = git_tab(git_world(), None);
    assert!(app.wait_until(|app| app.find("\"+fn lines() {}\"").is_some()), "{}", app.tree());
    assert_eq!(app.error(), None);

    // One line: its +, a box under it; said, a chip's worth to the composer.
    app.hover("\"+fn lines() {}\"");
    app.frames(5);
    app.click("role:button \"Comment on line 3\"");
    assert!(app.wait_until(|app| app.find(".git-comment-writer").is_some()), "{}", app.tree());
    let (line, writer) = (app.bounds("\"+fn lines() {}\""), app.bounds(".git-comment-writer"));
    assert!(writer.y >= line.y + line.height - 0.5, "under its line: {line:?} {writer:?}");
    app.type_text(".git-comment-writer >> role:textbox \"Comment\"", "Name it for what it draws.");
    app.click(".git-comment-writer >> role:button \"Add to chat\"");
    app.frames(5);
    let asked = composed_comments(&heard);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let one = &asked[0];
    assert_eq!(
        (one["file"].as_str(), one["line"].as_i64(), one["to"].is_null(), one["side"].as_str(), one["snippet"].as_str(), one["note"].as_str(), one["session"].as_str()),
        (Some("src/graph.rs"), Some(3), true, Some("R"), Some("+fn lines() {}"), Some("Name it for what it draws."), Some("s1")),
        "{one}"
    );
    assert!(app.find(".git-comment-writer").is_none(), "the box goes: {}", app.tree());

    // A range: dragged from line 1's + down to line 2; the box under its
    // last line; its first and last lines, all three lines' code.
    app.hover("\" fn lanes() {}\"");
    app.frames(5);
    app.drag("role:button \"Comment on line 1\"", "\"+fn rows(n: usize) {}\"");
    assert!(app.wait_until(|app| app.find(".git-comment-writer").is_some()), "{}", app.tree());
    let (last, writer) = (app.bounds("\"+fn rows(n: usize) {}\""), app.bounds(".git-comment-writer"));
    assert!(writer.y >= last.y + last.height - 0.5, "under the range's last line: {last:?} {writer:?}");
    app.type_text(".git-comment-writer >> role:textbox \"Comment\"", "Keep the old name.");
    app.click(".git-comment-writer >> role:button \"Add to chat\"");
    app.frames(5);
    let asked = composed_comments(&heard);
    assert_eq!(asked.len(), 2, "{asked:?}");
    let range = &asked[1];
    assert_eq!(
        (range["line"].as_i64(), range["to"].as_i64(), range["side"].as_str(), range["snippet"].as_str()),
        (Some(1), Some(2), Some("R"), Some(" fn lanes() {}\n-fn rows() {}\n+fn rows(n: usize) {}")),
        "{range}"
    );

    // While the composer carries them: a quiet mark on their lines; one's
    // chip pointed at, its lines lit; taken off, no mark.
    let (k1, k2) = (one["key"].as_str().unwrap().to_owned(), range["key"].as_str().unwrap().to_owned());
    let mark = |line: &str| format!("\"{line}\" >> .diff-mark");
    let bar = |line: &str| format!("\"{line}\" >> .diff-mark-bar");
    app.host.set_state("git_fixture.composed", json_value(&json!({ k1.clone(): { "session": "s1", "hovered": false }, k2.clone(): { "session": "s1", "hovered": false } }))).unwrap();
    app.frames(5);
    for line in ["+fn lines() {}", " fn lanes() {}", "-fn rows() {}", "+fn rows(n: usize) {}"] {
        assert!(app.opacity(&mark(line)) > 0.99, "{line} marked");
        assert!(app.opacity(&bar(line)) < 0.9, "{line} quiet");
    }
    app.host.set_state("git_fixture.composed", json_value(&json!({ k1.clone(): { "session": "s1", "hovered": true }, k2.clone(): { "session": "s1", "hovered": false } }))).unwrap();
    app.frames(5);
    assert!(app.opacity(&bar("+fn lines() {}")) > 0.99, "its chip pointed at: lit");
    assert!(app.opacity(&bar(" fn lanes() {}")) < 0.9, "the other stays quiet");
    app.host.set_state("git_fixture.composed", json_value(&json!({ k2: { "session": "s1", "hovered": false } }))).unwrap();
    app.frames(5);
    assert!(app.opacity(&mark("+fn lines() {}")) < 0.01, "its chip taken off: no mark");
    assert!(app.opacity(&mark(" fn lanes() {}")) > 0.99);
    // Another chat's comment marks nothing here.
    app.host.set_state("git_fixture.composed", json_value(&json!({ k1: { "session": "s2", "hovered": false } }))).unwrap();
    app.frames(5);
    assert!(app.opacity(&mark("+fn lines() {}")) < 0.01);
    assert_eq!(app.error(), None);
}

#[test]
fn github_puts_the_branch_s_pull_request_under_it_in_the_panel_home() {
    // The real panel home, chat-1 on zeron/panel-home in /proj; GitHub
    // knows that branch's pull request, open, its checks passed.
    let records = json!({ "sessions": { "chat-1": {
        "title": "Parent", "status": "idle", "device": "d", "cwd": "/proj", "branch": "zeron/panel-home", "project": "p1",
    } }, "files_read": true });
    let pr = json!({
        "number": 1200, "title": "Panel home details", "url": "https://github.com/zeronsh/zeron/pull/1200",
        "state": "OPEN", "isDraft": false, "isCrossRepository": false, "headRefName": "zeron/panel-home", "baseRefName": "main",
        "reviewRequests": { "nodes": [] },
        "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] },
    });
    let github = json!({ "viewer": "ada", "repository": "zeronsh/zeron", "branches": { "zeron/panel-home": pr } });
    let (mut app, heard, _) = git_in("panel_home_fixture.luau", records.clone(), git_world(), Some(github));
    assert_eq!(app.error(), None);
    assert!(app.wait_until(|app| app.find("#github-panel-pr").is_some()), "{}", app.tree());
    // Under the branch's row, in the home's details, by number and title.
    assert!(app.find("#panel-home-details >> #github-panel-pr").is_some(), "{}", app.tree());
    let (branch, row) = (app.bounds("#panel-home-branch"), app.bounds("#github-panel-pr"));
    assert!(row.y >= branch.y + branch.height - 0.5, "{branch:?} {row:?}");
    assert!((row.height - branch.height).abs() < 0.5, "the home's own row metric: {branch:?} {row:?}");
    let tree = app.tree();
    assert!(tree.contains("Panel home details"), "{tree}");
    // Its trail: the sidebar's pull request badge (its number) and a checks
    // badge, never a bare glyph.
    assert!(app.find("#github-panel-pr >> .github-panel-pr-badges >> \"1200\"").is_some(), "{tree}");
    assert!(app.find("#github-panel-pr >> .github-panel-pr-badges >> \"Passing\"").is_some(), "{tree}");
    // A click: the pull request, in the chat's panel.
    app.click("#github-panel-pr");
    app.frames(5);
    let (_, tab) = heard.borrow().iter().find(|(k, _)| k == "plugin.github.open_tab").cloned().expect("a tab asked for");
    let tab: serde_json::Value = serde_json::from_str(&tab).unwrap();
    assert_eq!((tab["kind"].as_str(), tab["data"]["number"].as_i64()), (Some("github-pr"), Some(1200)), "{tab}");

    // No pull request for the branch: nothing, and no room taken.
    let github = json!({ "viewer": "ada", "repository": "zeronsh/zeron", "branches": {} });
    let (mut app, heard, _) = git_in("panel_home_fixture.luau", records, git_world(), Some(github));
    assert!(app.wait_until(|_| runs(&heard).iter().any(|r| ran(r, &["graphql", "pullRequests"]))), "{:?}", runs(&heard));
    app.frames(20);
    assert!(app.find("#github-panel-pr").is_none(), "{}", app.tree());
    assert!(app.bounds("#panel-home-details").height < 0.5, "{:?}", app.bounds("#panel-home-details"));
}


/// The History tab (its own kind of tab, "git-history").
fn history_of(app: &mut TestApp) {
    git_fixture(app, "view", zgx_script::Value::from("history"));
    assert!(app.wait_until(|app| app.find("role:button \"Merge main into feature/graph\"").is_some()), "{}", app.tree());
}

#[test]
fn history_s_refs_keep_to_their_room_and_fold_the_rest() {
    // The merge carries five refs (Zeron's rule: 45% of the commit's room,
    // the subject never under 80, each badge at most 112, the rest "+N").
    let refs = ["HEAD -> zeron/create-readme-update-pr", "origin/zeron/create-readme-update-pr", "tag: v1.0.0-release-candidate", "origin/main", "release/9"];
    // (The branch and its own remote are one badge; release/9 is a local
    // branch with a slash, told from a remote's by the service's list.)
    let names = ["zeron/create-readme-update-pr", "v1.0.0-release-candidate", "origin/main", "release/9"];
    let mut w = git_world();
    w["history"]["commits"][0]["refs"] = json!(refs);
    w["history"]["commits"][0]["remotes"] = json!(["origin/zeron/create-readme-update-pr", "origin/main"]);
    let (mut app, _heard, _) = git_tab(w, None);
    history_of(&mut app);
    let row = "role:button \"Merge main into feature/graph\"";
    for width in [420.0, 0.0] {
        git_fixture(&mut app, "width", zgx_script::Value::from(width));
        history_of(&mut app);
        assert!(app.wait_until(|app| app.find(&format!("{row} >> .git-commit-refs")).is_some()), "{}", app.tree());
        app.frames(20);
        let main = app.bounds(&format!("{row} >> .git-commit-main"));
        let area = app.bounds(&format!("{row} >> .git-commit-refs"));
        let subject = app.bounds(&format!("{row} >> \"Merge main into feature/graph\""));
        // The subject keeps its 80 (fading past it), the refs their 45%.
        assert!(subject.width >= 79.5, "{width}: subject {subject:?}");
        assert!(area.width <= main.width * 0.45 + 0.5, "{width}: refs {area:?} in {main:?}");
        // What shows and what's folded add up to them all.
        let shown = names.iter().filter(|n| {
            let at = format!("{row} >> .git-commit-refs >> \"{n}\"");
            app.find(&at).is_some() && app.shown(&at)
        }).count();
        let more = (1..=names.len()).find(|n| app.find(&format!("{row} >> .git-ref-more >> \"+{n}\"")).is_some()).unwrap_or(0);
        assert_eq!(shown + more, names.len(), "{width}: {shown} shown, +{more}: {}", app.tree());
        if width > 0.0 {
            assert!(more >= 1, "a pane folds some: {}", app.tree());
        } else {
            assert!(shown >= 1, "the window shows some: {}", app.tree());
        }
        // Each at most 112 (its label fading past it): the area holds no
        // more than that a badge, "+N" and the gaps between.
        let most = shown as f32 * 112.0 + if more > 0 { 40.0 } else { 0.0 } + (shown as f32) * 5.0 + 0.5;
        assert!(area.width <= most, "{width}: {area:?}, {shown} shown, +{more}");
    }
    assert_eq!(app.error(), None);
}

#[test]
fn history_s_graph_goes_compact_when_narrow_and_back_with_room() {
    // Two lanes (the merge's): 30 wide at their 12; compact, one rail, 18
    // (Zeron's rule: compact once the subject would get under 160 beside
    // the columns, back only once it has 184).
    let (mut app, _heard, _) = git_tab(git_world(), None);
    history_of(&mut app);
    let row = "role:button \"Merge main into feature/graph\"";
    let graph = |app: &mut TestApp| app.bounds(&format!("{row} >> .git-graph")).width;
    let at = |app: &mut TestApp, w: f64| {
        git_fixture(app, "width", zgx_script::Value::from(w));
        app.frames(20);
    };
    at(&mut app, 420.0);
    let node = app.find(row);
    assert!((graph(&mut app) - 30.0).abs() < 0.5, "room enough: its lanes {}", graph(&mut app));
    at(&mut app, 240.0);
    assert!((graph(&mut app) - 18.0).abs() < 0.5, "narrow: compact {}", graph(&mut app));
    // Wider, but not by enough: still compact (no flicker at the edge).
    at(&mut app, 290.0);
    assert!((graph(&mut app) - 18.0).abs() < 0.5, "still compact {}", graph(&mut app));
    at(&mut app, 340.0);
    assert!((graph(&mut app) - 30.0).abs() < 0.5, "room again: its lanes {}", graph(&mut app));
    // The rows kept through it all: only their graphs redrew.
    assert_eq!(app.find(row), node, "the row was built again");
    // Compact, the pointer still lights its path and quiets the rest.
    at(&mut app, 240.0);
    app.hover(&format!("{row} >> \"Merge main into feature/graph\""));
    app.frames(30);
    assert!(app.opacity(&format!("{row} >> .git-graph-lit")) > 0.99);
    assert!((app.opacity("role:button \"Fix the build\" >> .git-commit-words") - 0.6).abs() < 0.05);
    assert_eq!(app.error(), None);
}

#[test]
fn history_lights_the_path_under_the_pointer_and_quiets_the_rest() {
    // The merge (1) and "Draw the lanes" (2) are one path, the branch's
    // first parents; "Fix the build" (4) came in by the merge, a path of its
    // own (Zeron's graph focus).
    let (mut app, _heard, _) = git_tab(git_world(), None);
    history_of(&mut app);
    app.frames(10);
    let row = |subject: &str| format!("role:button \"{subject}\"");
    let words = |app: &mut TestApp, subject: &str| app.opacity(&format!("{} >> .git-commit-words", row(subject)));
    let lit = |app: &mut TestApp, subject: &str| app.opacity(&format!("{} >> .git-graph-lit", row(subject)));
    // At rest: all as they are, nothing lit.
    assert!(words(&mut app, "Fix the build") > 0.99 && lit(&mut app, "Fix the build") < 0.01);
    // A row under the pointer: its own path lit, the other rows' words quieter.
    app.hover(&format!("{} >> \"Draw the lanes\"", row("Draw the lanes")));
    app.frames(30);
    assert!(lit(&mut app, "Draw the lanes") > 0.99, "lit: {}", lit(&mut app, "Draw the lanes"));
    assert!(words(&mut app, "Draw the lanes") > 0.99 && words(&mut app, "Merge main into feature/graph") > 0.99, "its path stays as it is");
    let quiet = words(&mut app, "Fix the build");
    assert!((quiet - 0.6).abs() < 0.05, "off the path: {quiet}");
    // Another path: the merged branch's.
    app.hover(&format!("{} >> \"Fix the build\"", row("Fix the build")));
    app.frames(30);
    assert!(words(&mut app, "Fix the build") > 0.99);
    assert!((words(&mut app, "Draw the lanes") - 0.6).abs() < 0.05);
    // Away: it fades back, nothing lit.
    app.hover("#git-head");
    app.frames(40);
    assert!(app.wait_until(|app| words(app, "Draw the lanes") > 0.99 && lit(app, "Fix the build") < 0.01));
    assert_eq!(app.error(), None);
}

#[test]
fn github_puts_faces_pull_requests_and_its_menu_items_in_git_s_history() {
    let github = json!({
        "viewer": "ada", "repository": "zeronsh/zeron", "branches": {},
        "commits": {
            "1111111111111111111111111111111111111111": {
                "author": { "user": { "login": "ada", "avatarUrl": "https://avatars.githubusercontent.com/u/1" } },
                "associatedPullRequests": { "nodes": [{ "number": 1142, "title": "Graph lanes", "url": "https://github.com/zeronsh/zeron/pull/1142", "state": "MERGED", "merged": true }] },
            },
            "2222222222222222222222222222222222222222": { "author": { "user": { "login": "ada", "avatarUrl": "https://avatars.githubusercontent.com/u/1" } }, "associatedPullRequests": { "nodes": [] } },
        },
    });
    let (mut app, heard, _) = git_tab(git_world(), Some(github));
    history_of(&mut app);
    // One ask of GitHub for the rows in view, through gh.
    assert!(app.wait_until(|_| runs(&heard).iter().any(|r| ran(r, &["graphql", "associatedPullRequests", "zeronsh"]))), "{:?}", runs(&heard));
    // The merge's pull request, by GitHub's word, in its row; a picture
    // over the faces GitHub knows.
    assert!(app.wait_until(|app| app.tree().contains("1142")), "{}", app.tree());
    assert!(app.find("role:button \"Merge main into feature/graph\" >> role:link \"Open pull request #1142: Graph lanes\"").is_some(), "{}", app.tree());
    assert!(app.tree().contains("img \"ada on GitHub\""), "{}", app.tree());
    // Asked once: kept in its database, the next look asks nothing.
    let asked = runs(&heard).iter().filter(|r| ran(r, &["associatedPullRequests"])).count();
    assert_eq!(asked, 1, "one batch for the rows: {:?}", runs(&heard));
    // Its menu items, offered to Git's commit menu alone (plugin_menus)…
    let offered = app.host.shared("plugin.git.menu_items", zgx_script::Value::Nil).signal().get().to_json();
    assert!(offered.contains("View on GitHub") && offered.contains("Copy GitHub link") && offered.contains("git.commit"), "{offered}");
    // …and, chosen in Git's menu, run in GitHub's plugin with the commit.
    app.host
        .set_state(
            "git_fixture.menu_ask",
            json_value(&json!({ "plugin": "github", "id": "github-view-commit", "kind": "git.commit", "context": { "sha": "1".repeat(40), "remote_url": "git@github.com:zeronsh/zeron.git" } })),
        )
        .unwrap();
    assert!(app.host.run_command("fixture.menu_run"));
    assert!(
        app.wait_until(|_| heard.borrow().iter().any(|(k, v)| k == "plugin.github.open_url" && v.contains("https://github.com/zeronsh/zeron/commit/1111111111"))),
        "{:?}",
        heard.borrow().iter().filter(|(k, _)| k.contains("open_url")).collect::<Vec<_>>()
    );
    // Opened: the commit's head has its pull request and its page.
    // The commit's own tab (as a click opens it): GitHub's word in its head.
    app.host.set_state("git_fixture.commit", json_value(&json!({ "sha": "1".repeat(40), "subject": "Merge main into feature/graph", "parents": ["2".repeat(40), "4".repeat(40)], "author": "Ada Lovelace", "email": "ada@example.com", "at": 1_790_000_000, "refs": [] }))).unwrap();
    git_fixture(&mut app, "view", zgx_script::Value::from("commit"));
    assert!(app.wait_until(|app| app.find("#github-commit-head").is_some()), "{}", app.tree());
    app.click("#github-view-commit");
    app.frames(5);
    assert!(heard.borrow().iter().filter(|(k, v)| k == "plugin.github.open_url" && v.contains("/commit/1111111111")).count() >= 2);
}

#[test]
fn git_s_history_searches_shows_every_branch_and_says_what_it_found() {
    let mut w = git_world();
    w["status"]["base_behind"] = json!(1);
    let (mut app, heard, world) = git_tab(w, None);
    history_of(&mut app);
    // How far from its base, as a pill.
    let tree = app.tree();
    assert!(tree.contains("↑3") && tree.contains("↓1"), "{tree}");
    // Every branch: asked of git with `all`.
    app.click("role:checkbox \"Every branch\"");
    assert!(app.wait_until(|_| asked_service(&heard, "history").iter().any(|c| c["args"]["all"] == true)), "{:?}", service_calls(&heard));
    // A search: asked once typing rests; none found, said.
    world.borrow_mut()["history"] = json!({ "commits": [], "more": false });
    app.type_text("role:textbox \"Search\"", "lanes");
    assert!(app.wait_until(|_| asked_service(&heard, "history").iter().any(|c| c["args"]["query"] == "lanes")), "{:?}", service_calls(&heard));
    assert!(app.wait_until(|app| app.tree().contains("No commits found")), "{}", app.tree());
    assert_eq!(app.error(), None);
}

#[test]
fn git_s_branch_scope_compares_with_the_base_chosen_and_says_renames() {
    let mut w = git_world();
    w["changes"]["branch"]["files"] = json!([{ "path": "src/lanes.rs", "old": "src/walk.rs", "status": "renamed", "add": 3, "del": 1, "rev": "r7" }]);
    w["changes"]["branch"]["total"] = json!(5200);
    let (mut app, heard, _) = git_tab(w, None);
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()));
    app.click("#git-scope >> role:button \"What changed\"");
    app.frames(10);
    app.click("role:listbox \"What changed\" >> \"Branch\"");
    // A rename says what it was; a list cut short says how much shows.
    assert!(app.wait_until(|app| app.tree().contains("src/walk.rs → src/lanes.rs")), "{}", app.tree());
    assert!(app.tree().contains("showing 1 of 5200"), "{}", app.tree());
    // Another base: asked against it.
    assert!(app.find("#git-base >> role:button \"Compare with\"").is_some(), "{}", app.tree());
    app.click("#git-base >> role:button \"Compare with\"");
    app.frames(10);
    assert!(app.wait_until(|_| !asked_service(&heard, "branches").is_empty()));
    app.frames(10);
    app.click("role:option \"main\"");
    assert!(app.wait_until(|_| asked_service(&heard, "changes").iter().any(|c| c["args"]["scope"] == "branch" && c["args"]["base"] == "main")), "{:?}", service_calls(&heard));
}

#[test]
fn git_s_base_stands_beside_the_scope_and_failures_say_what_git_said() {
    // No base known (the status says none) and git refusing Changes.
    let mut w = git_world();
    w["status"]["base"] = json!(null);
    w["fail"]["changes"] = json!("failed: git: fatal: bad object HEAD");
    let (mut app, _heard, world) = git_tab(w, None);
    assert!(app.wait_until(|app| app.find("#git-head >> #git-scope").is_some()), "{}", app.tree());
    app.click("#git-scope >> role:button \"What changed\"");
    app.frames(10);
    app.click("role:listbox \"What changed\" >> \"Branch\"");
    assert!(app.wait_until(|app| app.find("#git-head >> #git-base").is_some()), "{}", app.tree());
    // At the band's start: what Changes is of, then "against" its base, on
    // one line; the branch and ⋯ at the end.
    let (band, scope, base, controls) = (app.bounds("#git-head"), app.bounds("#git-scope"), app.bounds("#git-base"), app.bounds("#git-head-controls"));
    assert!(scope.x - band.x < 1.0, "the scope first: {band:?} {scope:?}");
    assert!(base.x >= scope.x + scope.width && (base.y - scope.y).abs() < 2.0, "beside it: {scope:?} {base:?}");
    assert!(controls.x > base.x + base.width, "{base:?} {controls:?}");
    // No base yet: said plainly, no "…".
    assert!(app.find("#git-base >> \"Choose a branch\"").is_some(), "{}", app.tree());
    // One control twice: the scope, the base and the branch chips one
    // height; "against" between the two with the same gap each side,
    // chip edge to word; every word on one line.
    let (scope_chip, base_chip, branch_chip) = (app.bounds("#git-scope >> role:button"), app.bounds("#git-base >> role:button"), app.bounds("#git-branch >> role:button"));
    assert!((scope_chip.height - base_chip.height).abs() < 0.5 && (base_chip.height - branch_chip.height).abs() < 0.5, "{scope_chip:?} {base_chip:?} {branch_chip:?}");
    let against = app.bounds("#git-base >> \"against\"");
    let (left, right) = (against.x - (scope_chip.x + scope_chip.width), base_chip.x - (against.x + against.width));
    assert!((left - right).abs() < 0.5, "gaps {left} | {right}");
    // (The base is the scope's own control, so its word stands for both:
    // the scope's is still rolling to its new name here, a letter at a
    // time.)
    let base_word = app.bounds("#git-base >> \"Choose a branch\"");
    let (f_against, f_base) = (against.y + against.height, base_word.y + base_word.height);
    assert!((f_base - f_against).abs() < 0.5, "one line: {against:?} {base_word:?}");
    // What git said, cleaned, as the title; what to do under it. Never the
    // bare "git couldn't do it".
    assert!(app.wait_until(|app| app.tree().contains("Bad object HEAD")), "{}", app.tree());
    assert!(app.tree().contains("Git stopped in this chat's folder"), "{}", app.tree());
    assert!(!app.tree().contains("git couldn't do it"), "{}", app.tree());
    // A base that isn't there: said so.
    world.borrow_mut()["fail"]["changes"] = json!("failed: no_ref: release/9");
    app.click("#git-scope >> role:button \"What changed\"");
    app.frames(10);
    app.click("role:listbox \"What changed\" >> \"Working tree\"");
    app.frames(10);
    app.click("#git-scope >> role:button \"What changed\"");
    app.frames(10);
    app.click("role:listbox \"What changed\" >> \"Branch\"");
    assert!(app.wait_until(|app| app.tree().contains("There's no branch named release/9 here")), "{}", app.tree());
    // A narrow pane: the base goes under the scope only when it must.
    git_fixture(&mut app, "width", zgx_script::Value::from(300.0));
    app.frames(20);
    let (scope, base, band) = (app.bounds("#git-scope"), app.bounds("#git-base"), app.bounds("#git-head"));
    assert!(base.y >= scope.y + scope.height - 0.5 && base.y + base.height <= band.y + band.height + 0.5, "under it, in the band: {scope:?} {base:?} {band:?}");
    // Wrapped: its first word starts where the scope chip's word does.
    // (The scope's word, by its first letter: still rolling, it rolls up
    // and down, never sideways.)
    let first = if app.find("#git-scope >> role:button >> \"Branch\"").is_some() { "#git-scope >> role:button >> \"Branch\"" } else { "#git-scope >> role:button >> \"B\"" };
    let (scope_word, against) = (app.bounds(first), app.bounds("#git-base >> \"against\""));
    assert!((scope_word.x - against.x).abs() < 0.5, "text on text: {scope_word:?} {against:?}");
    git_fixture(&mut app, "width", zgx_script::Value::from(640.0));
    app.frames(20);
    let (scope, base) = (app.bounds("#git-scope"), app.bounds("#git-base"));
    assert!((base.y - scope.y).abs() < 2.0, "room enough: one line again: {scope:?} {base:?}");
    assert_eq!(app.error(), None);
}

/// A diff of `n` lines put in, the first long, for the diff view's tests.
fn long_diff(n: usize) -> String {
    let mut text = String::from("diff --git a/src/graph.rs b/src/graph.rs\n--- a/src/graph.rs\n+++ b/src/graph.rs\n");
    text.push_str(&format!("@@ -1,1 +1,{} @@ fn lanes()\n-short\n", n));
    text.push_str("+a very long line that goes on and on and on and on and on and on and on and on and on and on and on and on and past the edge of any narrow pane at all\n");
    for k in 2..=n {
        text.push_str(&format!("+let row_{k} = {k};\n"));
    }
    text
}

/// The Git tab at a pane's width, src/graph.rs open in place.
fn open_diff(n: usize) -> TestApp {
    let mut w = git_world();
    w["diff"]["text"] = json!(long_diff(n));
    let (mut app, _heard, _) = git_tab(w, None);
    git_fixture(&mut app, "width", zgx_script::Value::from(420.0));
    assert!(app.wait_until(|app| app.find("role:checkbox \"src/graph.rs\"").is_some()), "{}", app.tree());
    app.click("role:button \"src/graph.rs\"");
    assert!(app.wait_until(|app| app.tree().contains("a very long")), "{}", app.tree());
    app
}

#[test]
fn git_s_tabs_are_changes_or_history_and_a_commit_opens_its_own() {
    // Changes: no switch to History in its band (History is a tab of its
    // own); at its end, what it's of, the branch, ⋯.
    let (mut app, _heard, _) = git_tab(git_world(), None);
    git_fixture(&mut app, "width", zgx_script::Value::from(420.0));
    assert!(app.wait_until(|app| app.find("#git-head >> #git-scope").is_some()), "{}", app.tree());
    assert!(app.find("#git-head >> role:tablist").is_none() && app.find("role:tab \"History\"").is_none(), "{}", app.tree());
    // Changes too: the band's ⋯ ends where the card head's last toggle does.
    let (more, wrap) = (app.bounds("#git-more"), app.bounds("#diff-view >> role:checkbox \"Wrap long lines\""));
    assert!((more.x + more.width - (wrap.x + wrap.width)).abs() < 0.5, "one right edge: {more:?} {wrap:?}");
    assert!((more.width - wrap.width).abs() < 0.5, "one size: {more:?} {wrap:?}");
    assert!(app.find("#git-back").is_none());
    // History at a pane's width: no way back over the list; a commit opened
    // in its place, the way back at the band's start, its title starting
    // the page under the band with no back row of its own.
    history_of(&mut app);
    assert!(app.find("#git-head >> #git-scope").is_none(), "no scope in History: {}", app.tree());
    assert!(app.find("#git-back").is_none(), "{}", app.tree());
    // The band's last control and the card head's end on one edge, the
    // card's inner inset; trailing controls one gap apart, one size.
    let edge = |r: (f32, f32)| r.0 + r.1;
    let more = app.bounds("#git-more");
    let refresh = app.bounds("#git-history-tools >> role:button \"Fetch\"");
    let toggle = app.bounds("#git-history-tools >> role:checkbox \"Every branch\"");
    let branch = app.bounds("#git-branch >> role:button");
    assert!((edge((more.x, more.width)) - edge((refresh.x, refresh.width))).abs() < 0.5, "one right edge: {more:?} {refresh:?}");
    assert!((more.width - refresh.width).abs() < 0.5 && (more.width - toggle.width).abs() < 0.5, "one size: {more:?} {refresh:?} {toggle:?}");
    let band_gap = more.x - (branch.x + branch.width);
    let card_gap = refresh.x - (toggle.x + toggle.width);
    assert!((band_gap - card_gap).abs() < 0.5, "one gap: {band_gap} {card_gap}");
    // A commit opens in a tab of its own: the list stays, with no way back
    // to need.
    app.click("role:button \"Merge main into feature/graph\"");
    app.frames(10);
    assert!(app.find("#git-commit-head").is_none() && app.find("#git-back").is_none(), "{}", app.tree());
    assert!(app.find("role:button \"Merge main into feature/graph\"").is_some());
    assert_eq!(app.error(), None);
}

/// What scrolling one of Git's lists costs, frame by frame, on a large
/// repository (`GIT_PERF_JSON`: a service answer as `git_world`'s, with
/// thousands of commits, hundreds of files and a long diff): each wheel step
/// one frame, its time and its phases (zgx's trace), and the rows built
/// (`count`, a tree line pattern) at rest and as it goes. Run on purpose:
/// `GIT_PERF_JSON=… GIT_PERF=history,files,diff,split,wrap cargo test
/// --release --bin zeron-zgx git_scroll_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn git_scroll_cost() {
    let Ok(path) = std::env::var("GIT_PERF_JSON") else { return };
    let world: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let only = std::env::var("GIT_PERF").unwrap_or_else(|_| "history,files,diff,split,wrap".into());
    let width: f64 = std::env::var("GIT_PERF_WIDTH").ok().and_then(|w| w.parse().ok()).unwrap_or(0.0);
    let pct = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() as f64 - 1.0) * p).round() as usize]
    };
    for surface in only.split(',') {
        let (mut app, _heard, _) = git_tab(world.clone(), None);
        if width > 0.0 {
            git_fixture(&mut app, "width", zgx_script::Value::from(width));
        }
        let target: &str = match surface {
            "history" | "hover" => {
                git_fixture(&mut app, "view", zgx_script::Value::from("history"));
                "#git-history"
            }
            "files" => {
                // (At a pane's width: the list alone, no diff beside it.)
                git_fixture(&mut app, "width", zgx_script::Value::from(420.0));
                "#git-changes-list"
            }
            // (A diff's line: a node with no role, its label the line.)
            _ => "#git-diff",
        };
        assert!(app.wait_until(|app| app.find(target).is_some()), "{surface}: {}", app.tree());
        if surface == "split" || surface == "wrap" {
            let toggle = if surface == "split" { "Side by side" } else { "Wrap long lines" };
            app.click(&format!("role:checkbox \"{toggle}\""));
        }
        if surface != "history" && surface != "files" && surface != "hover" {
            // (Its lines come once the device answers and they're read: a
            // hunk's header shows then.)
            assert!(app.wait_until(|app| app.texts().iter().any(|t| t.starts_with("@@")) || app.error().is_some()), "{surface} lines: {}", app.tree());
            assert_eq!(app.error(), None, "{surface}");
        }
        app.frames(30);
        if let Ok(out) = std::env::var("GIT_PERF_TREE") {
            std::fs::write(out, app.tree()).unwrap();
        }
        // Rows mounted in the list (zgx's own count: a row past them panics
        // saying how many), and the texts on screen, untruncated.
        // (What's mounted, as texts: a list that builds every row holds
        // them all, one that builds what's in view about the same at rest
        // and as it scrolls.)
        let count = |app: &mut TestApp| -> usize { app.texts().len() };
        // (The texts seen on the way: many more than at rest, or it never moved.)
        let mut seen: std::collections::HashSet<String> = app.texts().into_iter().collect();
        let first_seen = seen.len();
        let at_rest = count(&mut app);
        zgx::trace::set_enabled(true);
        let mut times = Vec::new();
        let mut phases: std::collections::BTreeMap<&'static str, Vec<f64>> = Default::default();
        let mut most = 0;
        // (Hovering: the rows in view, one a frame, each lighting its path
        // as the last fades.)
        let subjects: Vec<String> = world["history"]["commits"].as_array().map(|c| c.iter().take(18).filter_map(|c| c["subject"].as_str().map(str::to_string)).collect()).unwrap_or_default();
        for step in 0..240 {
            let start = std::time::Instant::now();
            if surface == "hover" {
                let s = &subjects[step % subjects.len().max(1)];
                let quoted = format!("role:button \"{}\"", s.replace('"', "\\\""));
                if app.find(&quoted).is_some() {
                    app.hover(&quoted);
                }
            } else {
                app.scroll(target, 48.0);
            }
            app.frame();
            times.push(start.elapsed().as_secs_f64() * 1e3);
            if let Some(f) = zgx::trace::frames(1).pop() {
                for (name, _, us) in f.phases {
                    phases.entry(name).or_default().push(us as f64 / 1e3);
                }
            }
            if step % 40 == 0 {
                most = most.max(count(&mut app));
                seen.extend(app.texts());
            }
        }
        zgx::trace::set_enabled(false);
        let (p50, p95, max) = (pct(&mut times, 0.5), pct(&mut times, 0.95), pct(&mut times, 1.0));
        let split: Vec<String> = phases.iter_mut().map(|(k, v)| format!("{k} {:.2}/{:.2}", pct(v, 0.5), pct(v, 0.95))).collect();
        eprintln!("PERF {surface}: frame p50 {p50:.2} ms, p95 {p95:.2}, max {max:.2}; texts mounted at rest {at_rest}, scrolling ≤{most}, distinct texts passed {} (at rest {first_seen}); phases p50/p95 ms: {}", seen.len(), split.join(", "));
        assert_eq!(app.error(), None, "{surface}");
    }
}

/// A diff of `n` lines in one hunk, code-like, for the large-diff tests.
fn big_diff(n: usize) -> String {
    let mut text = String::from("diff --git a/src/graph.rs b/src/graph.rs\n--- a/src/graph.rs\n+++ b/src/graph.rs\n");
    text.push_str(&format!("@@ -1,{n} +1,{n} @@ impl Lanes {{\n"));
    for k in 0..n {
        let sign = match k % 3 {
            0 => ' ',
            1 => '+',
            _ => '-',
        };
        text.push_str(&format!("{sign}    pub fn lane_{k}(&mut self, oid: Oid) -> Option<usize> {{ self.find(oid).map(|l| l + {k}) }}\n"));
    }
    text
}

#[test]
fn a_large_diff_loads_within_a_plugin_s_memory() {
    // Eight thousand lines, the window's width (the diff beside the list).
    let mut w = git_world();
    w["diff"]["text"] = json!(big_diff(8000));
    let (mut app, _heard, _) = git_tab(w, None);
    assert!(app.wait_until(|app| app.find("#git-diff").is_some()), "{}", app.tree());
    for _ in 0..40 {
        app.frames(5);
        if app.error().is_some() || app.tree().contains("lane_10(") {
            break;
        }
    }
    assert_eq!(app.error(), None);
    assert!(app.tree().contains("lane_10("), "its lines show: {}", app.tree());
}

#[test]
fn a_diff_wraps_long_lines_and_reads_side_by_side() {
    let mut app = open_diff(4);
    let line = |app: &mut TestApp| {
        let id = "\"+a very long line that goes on and on and on and on and on and on and on and on and on and on and on and on and past the edge of any narrow pane at all\"";
        app.find(id).map(|_| app.bounds(id).height)
    };
    assert!(line(&mut app).is_some_and(|h| h <= 24.0), "one line, scrolled: {:?}", line(&mut app));
    // Wrapped: as tall as its text, measured; the rest as they were.
    app.click("role:checkbox \"Wrap long lines\"");
    assert!(app.wait_until(|app| line(app).is_some_and(|h| h > 40.0)), "{:?}\n{}", line(&mut app), app.tree());
    app.click("role:checkbox \"Wrap long lines\"");
    // Side by side: what was taken out beside what was put in.
    app.click("role:checkbox \"Side by side\"");
    assert!(app.wait_until(|app| app.tree().contains("-short │ +a very long")), "{}", app.tree());
    assert!(app.tree().contains("@@ -1,1 +1,4 @@") && app.tree().contains("fn lanes()"), "{}", app.tree());
    app.click("role:checkbox \"Side by side\"");
    assert!(app.wait_until(|app| !app.tree().contains(" │ ")), "{}", app.tree());
}

#[test]
fn an_open_file_s_row_sticks_while_its_lines_scroll() {
    let mut app = open_diff(80);
    let top_of = |app: &mut TestApp, what: &str| app.find(what).map(|_| app.bounds(what).y);
    let list_top = top_of(&mut app, "#git-changes-list").unwrap();
    let row = "role:button \"src/graph.rs\"";
    let before = top_of(&mut app, row).unwrap();
    app.scroll("#git-changes-list", 900.0);
    app.frames(10);
    // Scrolled well past it, its row stands at the list's top, over its lines.
    let after = top_of(&mut app, row);
    assert!(after.is_some_and(|y| (y - list_top).abs() < 6.0 && y < before + 1.0), "list {list_top}, row {before} → {after:?}\n{}", app.tree());
}

/// Every list's rows hover alike: a Changes file row (ds/git, ds/git_changes),
/// a History row (the Git plugin), a GitHub board row and a pull request's
/// file rows (the GitHub plugin, through ds/git_changes) all draw through
/// the design system's list row (ds/list_row), and the sidebar's rows
/// through the same flat Surface it is: the theme's `$hover` wash, the flat
/// selection, no edge, no lift, no shadow, no corner of its own in a card.
/// (Read from their source: a test sees no painted colours.)
#[test]
fn every_list_row_hovers_by_the_design_system_s_one_recipe() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let read = |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    // The recipe: a flat Surface, edge to edge, no edge or shadow of its own.
    let recipe = read("ui/ds/list_row.luau");
    for must in ["require(\"@self/surface\")", "depth = \"flat\"", "rounded = 0", "border = 0", "M.HOVER, M.PRESSED, M.SELECTED = \"$hover\", \"$pressed\", \"$zeron.selected.flat.bg\""] {
        assert!(recipe.contains(must), "ds/list_row lacks {must}");
    }
    for never in ["shadow", "inset_shadow", "border_color"] {
        assert!(!recipe.contains(&format!("{never} =")), "ds/list_row draws a {never}");
    }
    // The Surface's flat depth: the wash under the pointer, the flat
    // selection, no ring or lift.
    let surface = read("ui/ds/surface.luau");
    assert!(surface.contains("hover = { bg = \"$hover\" }, active = { bg = \"$pressed\" }"), "Surface's hover isn't the theme's wash");
    let flat = surface.split("depth = \"flat\",").nth(1).unwrap_or_default().split("},").next().unwrap_or_default();
    assert!(flat.contains("$zeron.selected.flat.bg") && !flat.contains("shadow") && !flat.contains("ring"), "flat selection: {flat}");
    // Every list on it, none washing itself.
    for (file, uses) in [
        ("ui/ds/git.luau", "require(\"@self/list_row\")"),
        ("plugins-builtin/git/history.luau", "require(\"@ds/list_row\")"),
        ("plugins-builtin/github/board.luau", "require(\"@ds/list_row\")"),
    ] {
        let src = read(file);
        assert!(src.contains(uses), "{file} doesn't use the list row");
        assert!(!src.contains("then \"$hover\" else"), "{file} washes a row itself");
        assert!(!src.contains("\"$selection.item\" elseif"), "{file} selects a row itself");
    }
    // The sidebar's rows: the same flat Surface.
    for file in ["ui/ds/session_row.luau", "ui/ds/sidebar.luau"] {
        let src = read(file);
        assert!(src.contains("Surface {") && src.contains("depth = \"flat\""), "{file}'s rows aren't the flat Surface");
    }
}

/// Settings, sealed from plugins (`#shell-settings`, `hook = false`), lays
/// its pages out at their full width with plugins loaded, its rows
/// clickable, whether plugins load before Settings opens or while it's
/// open (the window reloads with them, as the bridge's do).
#[test]
fn settings_pages_lay_out_at_full_width_with_plugins_loaded() {
    let items = vec![builtin("parallax-scenes"), builtin("git"), builtin("github")];
    for late in [false, true] {
        let mut app = shell(if late { vec![] } else { items.clone() });
        assert_eq!(app.error(), None);
        app.host.run_command("shell.settings");
        app.frames(30);
        if late {
            // (One of them, its own actions answered: the others' services
            // aren't played here.)
            let items = &items[..1];
            for item in items {
                expose_own(&app.host, item.id());
                for name in ["result", "ack", "menus", "toast", "problem", "open_tab", "add_page", "github_watch", "open_url", "setting_status", "cost"] {
                    app.host.action(format!("plugin.{}.{name}", item.id()), |_| {});
                }
            }
            app.host.set_plugins(items.iter().filter_map(Item::plugin).collect());
            app.host.reload().unwrap();
            app.frames(30);
        }
        let page = app.bounds("settings-page");
        assert!(page.width > 400., "late={late}: the page's width: {page:?}\n{}", app.tree());
        assert!(app.bounds("settings-nav").width > 100., "late={late}: {:?}", app.bounds("settings-nav"));
        app.click("#settings-nav >> role:tab \"Appearance\"");
        app.frames(20);
        let row = app.bounds("setting-scheme");
        assert!(row.width > 400., "late={late}: a page's row: {row:?}\n{}", app.tree());
        app.click("#setting-scheme >> role:radio \"Dark\"");
        app.frames(10);
        assert_eq!(app.error(), None);
    }
}
