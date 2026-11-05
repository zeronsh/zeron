//! A plugin's GitHub, lent by proxy ("github"): Zeron's own sign-in does the
//! asking, through the engine's GitHub calls (which resolve on the device
//! where a session's checkout lives), and the plugin hears only what comes
//! back. No token ever reaches a plugin's scripts, so none can be kept or
//! sent elsewhere.
//!
//! A plugin names the sessions it shows (`zeron.github.watch_pr`); the host
//! keeps one watch of each one's pull request (`WatchCheckoutChangeRequest`,
//! pushed by the engine as it changes, never polled by the plugin) and
//! writes what it hears to `plugin.<id>.github`: `{ [session] = pr | false }`.

use serde_json::{Value as Json, json};

/// What the engine's watch of a session's checkout takes: its folder and
/// branch, on the device it's on. `None` for a session without a folder (a
/// new chat not yet started, a Cloud session the engine can't see).
pub fn watch_params(session: &Json) -> Option<Json> {
    let cwd = session["cwd"].as_str().filter(|c| !c.is_empty())?;
    let mut params = json!({ "cwd": cwd });
    // The branch its latest run started on (the engine's stamp), else the one
    // it was made on: a branch the agent made in its first run is the chat's
    // from its next, as the shipping app's sidebar has it.
    let stamped = session["source_branch"].as_str().filter(|b| !b.is_empty());
    if let Some(branch) = stamped.or_else(|| session["branch"].as_str().filter(|b| !b.is_empty())) {
        params["branch"] = json!(branch);
    }
    if let Some(device) = session["device"].as_str().filter(|d| !d.is_empty()) {
        params["targetDeviceId"] = json!(device);
    }
    Some(params)
}

/// What a watch is of: a session's checkout as it stands (a new branch or
/// folder is another watch).
pub fn watch_key(params: &Json) -> String {
    format!(
        "{}\u{0}{}\u{0}{}",
        params["cwd"].as_str().unwrap_or(""),
        params["branch"].as_str().unwrap_or(""),
        params["targetDeviceId"].as_str().unwrap_or("")
    )
}

/// How long before a session's pull request watch that ended is started
/// again: 2 s, doubling to a minute while it keeps ending, and back to 2 s
/// once it's heard (as the shipping app's sidebar retries, #1128). Without
/// it, a watch that failed once (the engine restarting, the device away)
/// left the chat's badge missing until its branch changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Retry {
    next: std::time::Duration,
}

impl Retry {
    pub const FIRST: std::time::Duration = std::time::Duration::from_secs(2);
    pub const MOST: std::time::Duration = std::time::Duration::from_secs(60);

    pub fn new() -> Self {
        Self { next: Self::FIRST }
    }

    /// It ended: how long to wait before starting it again.
    pub fn ended(&mut self) -> std::time::Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(Self::MOST);
        wait
    }

    /// It was heard: the next end waits the least again.
    pub fn heard(&mut self) {
        self.next = Self::FIRST;
    }
}

impl Default for Retry {
    fn default() -> Self {
        Self::new()
    }
}

/// What a plugin asked of GitHub (`zeron.github.pr(url)`, `.diff(url)`), as
/// the engine's call that answers it, or why it may not: reading needs
/// "github"; and only a pull request's own address on GitHub is asked about.
pub fn call(asked: &Json, may: impl Fn(&str) -> bool) -> Result<(&'static str, Json), String> {
    if !may("github") {
        return Err("This plugin wasn't allowed to see your pull requests on GitHub".into());
    }
    // What's asked of a repository rather than a pull request.
    if asked["op"].as_str() == Some("list") {
        return list_or_repository(asked);
    }
    if asked["op"].as_str() == Some("repository") {
        return Ok(("GetChangeRequestRepository", json!({})));
    }
    let url = asked["url"].as_str().unwrap_or_default();
    let is_pr = url.strip_prefix("https://github.com/").is_some_and(|rest| {
        let parts: Vec<&str> = rest.split('/').collect();
        parts.len() == 4 && parts[2] == "pull" && parts[3].chars().all(|c| c.is_ascii_digit()) && !parts[3].is_empty()
    });
    if !is_pr {
        return Err("That isn't a pull request on GitHub".into());
    }
    let method = match asked["op"].as_str() {
        // A file's place in the pull request's changes on GitHub (its
        // anchor is its path's SHA-256): answered here, nothing asked.
        Some("file_url") => {
            let path = asked["path"].as_str().filter(|p| !p.is_empty()).ok_or("file_url needs a path")?;
            use sha2::Digest;
            let digest = sha2::Sha256::digest(path.as_bytes());
            let anchor: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            return Ok(("", json!(format!("{url}/files#diff-{anchor}"))));
        }
        Some("pr") => "GetChangeRequest",
        Some("diff") => "GetChangeRequestDiff",
        // Writing as you is its own grant.
        Some("comment") => {
            if !may("github.write") {
                return Err("This plugin wasn't allowed to comment as you".into());
            }
            let body = asked["body"].as_str().unwrap_or_default();
            if body.trim().is_empty() || body.len() > 60_000 {
                return Err("A comment says something, in at most 60,000 bytes".into());
            }
            return Ok(("PostChangeRequestComment", json!({ "url": url, "body": body })));
        }
        // A comment on a line (a new review thread), a reply in a thread,
        // a thread resolved or opened again: writing as you too.
        Some(op @ ("line_comment" | "reply" | "resolve")) => {
            if !may("github.write") {
                return Err("This plugin wasn't allowed to comment as you".into());
            }
            let body = asked["body"].as_str().unwrap_or_default();
            let said = !body.trim().is_empty() && body.len() <= 60_000;
            return match op {
                "line_comment" => {
                    if !said {
                        return Err("A comment says something, in at most 60,000 bytes".into());
                    }
                    Ok((
                        "PostChangeRequestReviewComment",
                        json!({
                            "url": url, "body": body,
                            "path": asked["path"], "line": asked["line"], "side": asked["side"], "commitId": asked["commitId"],
                        }),
                    ))
                }
                "reply" => {
                    if !said {
                        return Err("A comment says something, in at most 60,000 bytes".into());
                    }
                    Ok(("ReplyChangeRequestThread", json!({ "url": url, "thread": asked["thread"], "body": body })))
                }
                _ => Ok((
                    "ResolveChangeRequestThread",
                    json!({ "url": url, "thread": asked["thread"], "resolved": asked["resolved"].as_bool().unwrap_or(true) }),
                )),
            };
        }
        _ => return Err("GitHub: pr, diff, comment, line_comment, reply, resolve or file_url".into()),
    };
    let mut params = json!({ "url": url });
    if asked["refresh"].as_bool() == Some(true) {
        params["refresh"] = json!(true);
    }
    // The diff at the revisions the plugin shows (Zeron's viewer asks so),
    // when both are commit ids.
    let oid = |v: &Json| v.as_str().filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())).map(str::to_string);
    if method == "GetChangeRequestDiff"
        && let (Some(head), Some(base)) = (oid(&asked["headRefOid"]), oid(&asked["baseRefOid"]))
    {
        params["headRefOid"] = json!(head);
        params["baseRefOid"] = json!(base);
    }
    Ok((method, params))
}

/// A repository's open pull requests, a page at a time (`after`: the
/// cursor the last page gave).
fn list_or_repository(asked: &Json) -> Result<(&'static str, Json), String> {
    let repository = asked["repository"].as_str().unwrap_or_default();
    if !valid_repository(repository) {
        return Err("Which repository: owner/name".into());
    }
    let filter = asked["filter"].as_str().filter(|f| matches!(*f, "authored" | "reviewing" | "all")).unwrap_or("authored");
    let mut params = json!({ "repository": repository, "filter": filter });
    if let Some(after) = asked["after"].as_str().filter(|a| !a.is_empty() && a.len() <= 400) {
        params["after"] = json!(after);
    }
    for flag in ["refresh", "cached"] {
        if asked[flag].as_bool() == Some(true) {
            params[flag] = json!(true);
        }
    }
    Ok(("ListChangeRequestPage", params))
}

/// `owner/name`, as GitHub names a repository.
pub fn valid_repository(repository: &str) -> bool {
    let mut parts = repository.split('/');
    let ok = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.len() <= 100 && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && p != "." && p != "..");
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// A checkout's pull request as a plugin reads it: `{ number, title, url,
/// state = "open" | "merged" | "closed", base, head }`, or `false` for none.
pub fn summary(status: &Json) -> Json {
    let pr = &status["changeRequest"];
    let Some(number) = pr["number"].as_u64() else {
        return Json::Bool(false);
    };
    let state = pr["state"].as_str().unwrap_or("open").to_ascii_lowercase();
    let mut out = json!({
        "number": number,
        "title": pr["title"].as_str().unwrap_or("").replace(['\r', '\n'], " "),
        "url": pr["url"].as_str().unwrap_or(""),
        "state": if matches!(state.as_str(), "open" | "merged" | "closed") { state } else { "open".into() },
        "base": pr["baseRef"].as_str().unwrap_or(""),
        "head": pr["headRef"].as_str().unwrap_or(""),
    });
    // An open one's review and checks, when the engine told them: `review`
    // ("approved" | "changesRequested" | "reviewRequired" | "unknown"),
    // `requested_of_you`, `ci` ("passed" | "failed" | "pending" | …).
    let signals = &pr["signals"];
    if signals.is_object() {
        out["review"] = json!(signals["reviewDecision"].as_str().unwrap_or("unknown"));
        out["ci"] = json!(signals["ci"].as_str().unwrap_or("unknown"));
        if let Some(asked) = signals["reviewRequestedOfViewer"].as_bool() {
            out["requested_of_you"] = json!(asked);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_s_checkout_is_watched_where_it_lives() {
        let s = json!({ "id": "a1", "cwd": "/w/atlas", "branch": "fix/build", "device": "mac" });
        let p = watch_params(&s).unwrap();
        assert_eq!(p, json!({ "cwd": "/w/atlas", "branch": "fix/build", "targetDeviceId": "mac" }));
        assert!(watch_params(&json!({ "id": "new" })).is_none(), "no folder, nothing to watch");
        let moved = watch_params(&json!({ "cwd": "/w/atlas", "branch": "feat/x", "device": "mac" })).unwrap();
        assert_ne!(watch_key(&p), watch_key(&moved), "another branch is another watch");
    }

    #[test]
    fn the_branch_its_latest_run_started_on_wins_over_the_one_it_was_made_on() {
        // Made on main; the agent branched off and its next run was stamped
        // on the new branch: that branch's pull request is the chat's.
        let s = json!({ "cwd": "/w/atlas", "branch": "main", "source_branch": "zeron/readme-pr", "device": "mac" });
        assert_eq!(watch_params(&s).unwrap()["branch"], json!("zeron/readme-pr"));
        // Not stamped yet: the one it was made on.
        let s = json!({ "cwd": "/w/atlas", "branch": "main", "device": "mac" });
        assert_eq!(watch_params(&s).unwrap()["branch"], json!("main"));
    }

    #[test]
    fn a_watch_that_keeps_ending_waits_longer_each_time_and_heard_starts_over() {
        use std::time::Duration;
        let mut retry = Retry::new();
        let waits: Vec<u64> = (0..7).map(|_| retry.ended().as_secs()).collect();
        assert_eq!(waits, vec![2, 4, 8, 16, 32, 60, 60]);
        retry.heard();
        assert_eq!(retry.ended(), Duration::from_secs(2));
        assert_eq!(retry.ended(), Duration::from_secs(4));
    }

    #[test]
    fn only_a_pull_request_on_github_is_asked_about_and_only_when_allowed() {
        let ok = |_: &str| true;
        let url = "https://github.com/zeronsh/zeron/pull/1264";
        assert_eq!(call(&json!({ "op": "pr", "url": url }), ok).unwrap(), ("GetChangeRequest", json!({ "url": url })));
        assert_eq!(call(&json!({ "op": "diff", "url": url, "refresh": true }), ok).unwrap().1, json!({ "url": url, "refresh": true }));
        assert!(call(&json!({ "op": "pr", "url": url }), |_| false).unwrap_err().contains("wasn't allowed"));
        for bad in ["https://evil.example/zeronsh/zeron/pull/1", "https://github.com/zeronsh/zeron/issues/1", "https://github.com/o/r/pull/1/../../x", "https://github.com/o/r/pull/"] {
            assert!(call(&json!({ "op": "pr", "url": bad }), ok).is_err(), "{bad}");
        }
        assert!(call(&json!({ "op": "merge", "url": url }), ok).is_err(), "only what's listed");
        let only_read = |p: &str| p == "github";
        assert!(call(&json!({ "op": "comment", "url": url, "body": "hi" }), only_read).unwrap_err().contains("comment as you"));
        assert_eq!(call(&json!({ "op": "comment", "url": url, "body": "hi" }), ok).unwrap(), ("PostChangeRequestComment", json!({ "url": url, "body": "hi" })));
        assert!(call(&json!({ "op": "comment", "url": url, "body": "  " }), ok).is_err());
        for op in ["line_comment", "reply", "resolve"] {
            assert!(call(&json!({ "op": op, "url": url, "body": "x" }), only_read).unwrap_err().contains("comment as you"), "{op}");
        }
        let (m, p) = call(&json!({ "op": "line_comment", "url": url, "body": "Why?", "path": "a.rs", "line": 3, "side": "RIGHT", "commitId": "c" }), ok).unwrap();
        assert_eq!((m, p["path"].as_str(), p["line"].as_u64()), ("PostChangeRequestReviewComment", Some("a.rs"), Some(3)));
        assert_eq!(call(&json!({ "op": "reply", "url": url, "body": "Done", "thread": "T" }), ok).unwrap().0, "ReplyChangeRequestThread");
        assert_eq!(call(&json!({ "op": "resolve", "url": url, "thread": "T", "resolved": false }), ok).unwrap().1["resolved"], false);
        assert!(call(&json!({ "op": "reply", "url": url, "body": " ", "thread": "T" }), ok).is_err());
        // A repository's list, a page at a time.
        let (m, p) = call(&json!({ "op": "list", "repository": "zeronsh/zgx", "filter": "reviewing", "after": "Y3Vy", "cached": true }), ok).unwrap();
        assert_eq!((m, p), ("ListChangeRequestPage", json!({ "repository": "zeronsh/zgx", "filter": "reviewing", "after": "Y3Vy", "cached": true })));
        for bad in ["zeronsh", "a/b/c", "../x", "a/ b", "a/.."] {
            assert!(call(&json!({ "op": "list", "repository": bad }), ok).is_err(), "{bad}");
        }
        assert!(call(&json!({ "op": "list", "repository": "o/r" }), |_| false).is_err(), "github's to ask");
        let (_, file) = call(&json!({ "op": "file_url", "url": url, "path": "README.md" }), ok).unwrap();
        // (GitHub's anchor: the path's SHA-256.)
        assert_eq!(file, json!(format!("{url}/files#diff-b335630551682c19a781afebcf4d07bf978fb1f8ac04c6bf87428ed5106870f5")));
        let (h, b) = ("a".repeat(40), "b".repeat(40));
        let p = call(&json!({ "op": "diff", "url": url, "headRefOid": h, "baseRefOid": b }), ok).unwrap().1;
        assert_eq!((p["headRefOid"].as_str(), p["baseRefOid"].as_str()), (Some(h.as_str()), Some(b.as_str())));
        let p = call(&json!({ "op": "diff", "url": url, "headRefOid": "main", "baseRefOid": b }), ok).unwrap().1;
        assert!(p.get("headRefOid").is_none(), "only commit ids");
    }

    #[test]
    fn what_the_engine_says_is_said_plainly() {
        let status = json!({
            "checkoutId": "c", "deviceId": "mac", "cwd": "/w", "branch": "fix/build", "updatedAt": "2026-11-01T00:00:00Z",
            "changeRequest": { "provider": "github", "number": 12, "title": "Fix the\nbuild", "url": "https://github.com/o/r/pull/12", "state": "merged", "baseRef": "main", "headRef": "fix/build" },
        });
        assert_eq!(
            summary(&status),
            json!({ "number": 12, "title": "Fix the build", "url": "https://github.com/o/r/pull/12", "state": "merged", "base": "main", "head": "fix/build" })
        );
        assert_eq!(summary(&json!({ "changeRequest": null })), json!(false));
        let told = json!({ "changeRequest": {
            "number": 3, "title": "T", "url": "u", "state": "open", "baseRef": "main", "headRef": "x",
            "signals": { "reviewDecision": "changesRequested", "reviewRequestedOfViewer": true, "ci": "failed" },
        } });
        let s = summary(&told);
        assert_eq!((s["review"].as_str(), s["requested_of_you"].as_bool(), s["ci"].as_str()), (Some("changesRequested"), Some(true), Some("failed")));
        assert!(summary(&status).get("review").is_none(), "an older engine tells none: none said");
    }
}
