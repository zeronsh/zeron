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
/// again: 2 s, doubling to five minutes while it keeps ending, and back to
/// 2 s once it's heard (as the shipping app's sidebar retries, #1128).
/// Without it, a watch that failed once (the engine restarting) left the
/// chat's badge missing until its branch changed. A watch that ended because
/// its chat isn't there to watch ([`away`]) isn't retried on a timer at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Retry {
    next: std::time::Duration,
}

impl Retry {
    pub const FIRST: std::time::Duration = std::time::Duration::from_secs(2);
    pub const MOST: std::time::Duration = std::time::Duration::from_secs(5 * 60);

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

/// Why a watch ended when asking again soon can't help: its folder isn't a
/// checkout on the device it names (another device's chat, a removed
/// worktree), or that device is offline. Each kept being asked every few
/// seconds, for as long as its row showed: hundreds of failed watches a
/// minute with a few such chats open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Away {
    NotHere,
    Offline,
}

/// What a watch's end (`why`, the engine's words) says of its chat.
pub fn away(why: &str) -> Option<Away> {
    if why.contains("not a known checkout") {
        Some(Away::NotHere)
    } else if why.contains("device is offline") {
        Some(Away::Offline)
    } else {
        None
    }
}

/// A watch set aside ([`away`]) until something it depends on changes: its
/// chat's folder, branch or device (another watch, by its key), or its
/// device coming back online.
#[derive(Debug, Clone, PartialEq)]
pub struct Parked {
    pub why: Away,
    pub device: String,
    /// The session list has shown its device offline since: online again,
    /// it's worth asking.
    seen_offline: bool,
}

impl Parked {
    pub fn new(why: Away, params: &Json) -> Self {
        Self { why, device: params["targetDeviceId"].as_str().unwrap_or("").to_string(), seen_offline: false }
    }

    /// Its device as the session list has it now (`None`: not listed):
    /// whether to try again. Only coming back online after being seen
    /// offline counts, so a device the list calls online while the engine
    /// can't reach it is asked once, not again and again.
    pub fn wake(&mut self, online: Option<bool>) -> bool {
        match online {
            Some(false) => {
                self.seen_offline = true;
                false
            }
            Some(true) => self.seen_offline,
            None => false,
        }
    }
}

/// What [`Watches::ended`] made of a watch's end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ended {
    /// Not the watch that's wanted now (another key, let go): nothing.
    Stale,
    /// Set aside until its chat or device changes.
    Parked,
    /// Started again after this long, if still wanted then.
    Wait(std::time::Duration),
}

/// One session's pull request watch, as the host keeps it: what it's of
/// (`key`, `params`), the watch itself (`W`; none while it waits, having
/// ended, or is set aside), and why it waits.
pub struct Entry<W> {
    pub key: String,
    pub params: Json,
    pub watch: Option<W>,
    retry: Retry,
    /// The plugin shows its session now. One it stopped showing is kept
    /// while it's set aside or waiting, so its row scrolling back into view
    /// doesn't start it over at once (each start was a failed watch more).
    wanted: bool,
    parked: Option<Parked>,
    again_at: Option<std::time::Instant>,
}

impl<W> Entry<W> {
    fn new(key: String, params: Json) -> Self {
        Self { key, params, watch: None, retry: Retry::new(), wanted: true, parked: None, again_at: None }
    }

    fn ready(&self) -> bool {
        self.wanted && self.watch.is_none() && self.parked.is_none()
    }
}

/// A plugin's pull request watches, by session: which to start, when to
/// start one that ended again, and which to set aside. The host does the
/// starting (`W` is its watch) and tells each end here.
pub struct Watches<W> {
    entries: std::collections::HashMap<String, Entry<W>>,
}

impl<W> Default for Watches<W> {
    fn default() -> Self {
        Self { entries: Default::default() }
    }
}

/// A watch to start: its session, key and params.
pub type Start = (String, String, Json);

impl<W> Watches<W> {
    pub fn get(&self, session: &str) -> Option<&Entry<W>> {
        self.entries.get(session)
    }

    /// The sessions the plugin shows now, each with its watch's params:
    /// those to start now. One no longer shown stops; its wait or its being
    /// set aside is kept (at most `keep_most` entries in all).
    pub fn want(&mut self, wanted: &[(String, Json)], now: std::time::Instant, keep_most: usize) -> Vec<Start> {
        let shown: std::collections::HashSet<&str> = wanted.iter().map(|(id, _)| id.as_str()).collect();
        for (id, w) in self.entries.iter_mut() {
            w.wanted = shown.contains(id.as_str());
            if !w.wanted {
                w.watch = None;
            }
        }
        self.entries.retain(|_, w| w.wanted || w.parked.is_some() || w.again_at.is_some());
        if self.entries.len() > keep_most {
            self.entries.retain(|_, w| w.wanted);
        }
        let mut start = Vec::new();
        for (id, params) in wanted {
            let key = watch_key(params);
            match self.entries.get_mut(id) {
                // Watched, set aside, or waiting with its start still to
                // come: as it is. Its wait ran out while it wasn't shown:
                // started now.
                Some(w) if w.key == key => {
                    if w.ready() && !w.again_at.is_some_and(|at| at > now) {
                        w.again_at = None;
                        start.push((id.clone(), key, params.clone()));
                    }
                }
                _ => {
                    self.entries.insert(id.clone(), Entry::new(key.clone(), params.clone()));
                    start.push((id.clone(), key, params.clone()));
                }
            }
        }
        start
    }

    /// `session`'s watch of `key` started.
    pub fn started(&mut self, session: &str, key: &str, watch: W) {
        if let Some(w) = self.entries.get_mut(session).filter(|w| w.key == key) {
            w.again_at = None;
            w.watch = Some(watch);
        }
    }

    /// `session`'s watch heard: the next end waits the least again.
    pub fn heard(&mut self, session: &str) -> bool {
        self.entries.get_mut(session).map(|w| w.retry.heard()).is_some()
    }

    /// `session`'s watch of `key` ended (`why`, the engine's words).
    pub fn ended(&mut self, session: &str, key: &str, why: &str, now: std::time::Instant) -> Ended {
        let Some(w) = self.entries.get_mut(session).filter(|w| w.key == key) else {
            return Ended::Stale;
        };
        w.watch = None;
        if let Some(away) = away(why) {
            w.parked = Some(Parked::new(away, &w.params));
            w.again_at = None;
            return Ended::Parked;
        }
        let wait = w.retry.ended();
        w.again_at = Some(now + wait);
        Ended::Wait(wait)
    }

    /// `session`'s wait for `key` is over: its params, to start it again,
    /// if it's still shown as it was and neither watched nor set aside.
    pub fn due(&self, session: &str, key: &str) -> Option<Json> {
        self.entries.get(session).filter(|w| w.key == key && w.ready()).map(|w| w.params.clone())
    }

    /// Whether any watch is set aside (nothing to wake otherwise).
    pub fn any_parked(&self) -> bool {
        self.entries.values().any(|w| w.parked.is_some())
    }

    /// The session list moved: each set-aside watch that's shown, whose
    /// chat changed (`params`: its watch's params now) or whose device came
    /// back online (`online`), to start again.
    pub fn wake(&mut self, params: impl Fn(&str) -> Option<Json>, online: impl Fn(&str) -> Option<bool>) -> Vec<Start> {
        let mut start = Vec::new();
        for (session, w) in self.entries.iter_mut() {
            if !w.wanted {
                continue;
            }
            let Some(parked) = w.parked.as_mut() else { continue };
            match params(session) {
                Some(now) if watch_key(&now) != w.key => {
                    // Its chat moved: a watch of where it is now.
                    *w = Entry::new(watch_key(&now), now);
                }
                Some(_) if parked.wake(online(&parked.device)) => w.parked = None,
                _ => continue,
            }
            start.push((session.clone(), w.key.clone(), w.params.clone()));
        }
        start
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
        let waits: Vec<u64> = (0..10).map(|_| retry.ended().as_secs()).collect();
        assert_eq!(waits, vec![2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        retry.heard();
        assert_eq!(retry.ended(), Duration::from_secs(2));
        assert_eq!(retry.ended(), Duration::from_secs(4));
    }

    #[test]
    fn a_chat_that_isn_t_here_or_whose_device_is_offline_is_set_aside() {
        // (The words the app log showed, Nov 9.)
        assert_eq!(away("bad params: cwd is not a known checkout on this device"), Some(Away::NotHere));
        assert_eq!(away("transport: peer cloud-3f2a: device is offline (no recent presence)"), Some(Away::Offline));
        for transient in ["closed", "no engine", "transport: peer mac: connect timed out", "transport: peer mac: unreachable (backing off after 3 failed dials; retrying in ~8s)"] {
            assert_eq!(away(transient), None, "{transient}");
        }
    }

    #[test]
    fn a_set_aside_watch_wakes_only_when_its_device_comes_back() {
        let params = json!({ "cwd": "/w", "targetDeviceId": "box" });
        let mut parked = Parked::new(Away::Offline, &params);
        assert_eq!(parked.device, "box");
        // The list still calls it online (its last beat is recent): no
        // asking again on every change of the list.
        for _ in 0..50 {
            assert!(!parked.wake(Some(true)));
        }
        assert!(!parked.wake(None), "not listed: nothing to go by");
        assert!(!parked.wake(Some(false)));
        assert!(!parked.wake(Some(false)));
        assert!(parked.wake(Some(true)), "back online");
        // A chat whose folder isn't on this device: this device is always
        // online, so only a change of its chat wakes it.
        let mut here = Parked::new(Away::NotHere, &json!({ "cwd": "/gone", "targetDeviceId": "mac" }));
        assert!((0..50).all(|_| !here.wake(Some(true))));
    }

    fn shown(ids: &[(&str, &str, &str)]) -> Vec<(String, Json)> {
        ids.iter()
            .map(|(id, cwd, device)| (id.to_string(), watch_params(&json!({ "cwd": cwd, "branch": "fix", "device": device })).unwrap()))
            .collect()
    }

    #[test]
    fn a_chat_that_isn_t_here_is_asked_about_once_until_it_changes() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut w: Watches<u32> = Watches::default();
        let list = shown(&[("a", "/elsewhere", "mac")]);
        let start = w.want(&list, t0, 512);
        assert_eq!(start.len(), 1);
        let (session, key, _) = start[0].clone();
        w.started(&session, &key, 1);
        assert_eq!(w.ended(&session, &key, "bad params: cwd is not a known checkout on this device", t0), Ended::Parked);
        // Its row scrolling away and back, the list moving, its device
        // (this one) online all along: nothing started.
        for i in 0..100 {
            let at = t0 + Duration::from_secs(i);
            assert!(w.want(if i % 2 == 0 { &list } else { &[] }, at, 512).is_empty());
            let same = list[0].1.clone();
            assert!(w.wake(|_| Some(same.clone()), |_| Some(true)).is_empty());
            assert_eq!(w.due(&session, &key), None);
        }
        // Shown, and its chat moved to another branch: a watch of that.
        assert!(w.want(&list, t0, 512).is_empty());
        let moved = watch_params(&json!({ "cwd": "/elsewhere", "branch": "next", "device": "mac" })).unwrap();
        let start = w.wake(|_| Some(moved.clone()), |_| Some(true));
        assert_eq!(start.len(), 1);
        assert_eq!(start[0].1, watch_key(&moved));
    }

    #[test]
    fn a_chat_on_an_offline_device_is_asked_about_again_once_it_s_back() {
        use std::time::Instant;
        let t0 = Instant::now();
        let mut w: Watches<u32> = Watches::default();
        let list = shown(&[("a", "/w", "box")]);
        let (session, key, params) = w.want(&list, t0, 512).remove(0);
        w.started(&session, &key, 1);
        assert_eq!(w.ended(&session, &key, "transport: peer box: device is offline (no recent presence)", t0), Ended::Parked);
        let p = |_: &str| Some(params.clone());
        assert!(w.wake(p, |_| Some(true)).is_empty(), "still called online by the list");
        assert!(w.wake(p, |_| Some(false)).is_empty());
        assert_eq!(w.wake(p, |_| Some(true)).len(), 1, "back");
        w.started(&session, &key, 2);
        assert!(w.get(&session).unwrap().watch.is_some());
    }

    #[test]
    fn a_row_scrolled_away_and_back_keeps_its_wait() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut w: Watches<u32> = Watches::default();
        let list = shown(&[("a", "/w", "mac")]);
        let (session, key, _) = w.want(&list, t0, 512).remove(0);
        for (n, wait) in [2, 4, 8].into_iter().enumerate() {
            w.started(&session, &key, n as u32);
            assert_eq!(w.ended(&session, &key, "closed", t0), Ended::Wait(Duration::from_secs(wait)));
        }
        // Away and back before its 8 s wait is up: not started (its timer
        // will), and its wait didn't start over at 2 s.
        assert!(w.want(&[], t0 + Duration::from_secs(1), 512).is_empty());
        assert_eq!(w.due(&session, &key), None, "not shown: not started");
        assert!(w.want(&list, t0 + Duration::from_secs(2), 512).is_empty());
        assert!(w.due(&session, &key).is_some(), "shown again: its timer starts it");
        // Its wait ran out while it wasn't shown: started as it shows.
        w.want(&[], t0 + Duration::from_secs(3), 512);
        assert_eq!(w.want(&list, t0 + Duration::from_secs(20), 512).len(), 1);
        w.started(&session, &key, 9);
        assert_eq!(w.ended(&session, &key, "closed", t0), Ended::Wait(Duration::from_secs(16)));
    }

    /// Ten chats (five whose watch fails: three not on this device, two on
    /// an offline one), their rows in and out of view every 5 s, for 20
    /// minutes: how many watches are started, under the old rules (a retry
    /// on a 2 s → 60 s timer, started over each time a row comes back) and
    /// these. `cargo test … watch_starts -- --nocapture` prints them.
    #[test]
    fn watch_starts_over_twenty_minutes_with_ten_chats() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let chats = [
            ("a", "/w/a", "mac"), ("b", "/w/b", "mac"), ("c", "/w/c", "mac"), ("d", "/w/d", "mac"), ("e", "/w/e", "mac"),
            ("f", "/gone/f", "mac"), ("g", "/gone/g", "mac"), ("h", "/other/h", "mac"),
            ("i", "/w/i", "box"), ("j", "/w/j", "box"),
        ];
        let fails = |id: &str| match id {
            "f" | "g" | "h" => Some("bad params: cwd is not a known checkout on this device"),
            "i" | "j" => Some("transport: peer box: device is offline (no recent presence)"),
            _ => None,
        };
        // Rows: all shown, then the last three scrolled away, by turns.
        let shown_at = |tick: u64| -> Vec<(String, Json)> {
            let n = if tick % 2 == 0 { 10 } else { 7 };
            shown(&chats[..n])
        };
        let end = Duration::from_secs(20 * 60);

        // These rules.
        let mut w: Watches<()> = Watches::default();
        let mut timers: Vec<(Duration, String, String)> = Vec::new();
        let mut failed_starts = 0;
        let mut start = |w: &mut Watches<()>, timers: &mut Vec<_>, at: Duration, (session, key, _): Start| {
            w.started(&session, &key, ());
            if let Some(why) = fails(&session) {
                failed_starts += 1;
                if let Ended::Wait(wait) = w.ended(&session, &key, why, t0 + at) {
                    timers.push((at + wait, session, key));
                }
            }
        };
        let mut at = Duration::ZERO;
        let mut tick = 0;
        while at < end {
            for s in w.want(&shown_at(tick), t0 + at, 512) {
                start(&mut w, &mut timers, at, s);
            }
            let due: Vec<_> = timers.iter().filter(|(t, ..)| *t <= at).cloned().collect();
            timers.retain(|(t, ..)| *t > at);
            for (_, session, key) in due {
                if let Some(params) = w.due(&session, &key) {
                    start(&mut w, &mut timers, at, (session, key, params));
                }
            }
            // (The list moves; box stays offline.)
            let list = shown_at(tick);
            let params = |id: &str| list.iter().find(|(s, _)| s == id).map(|(_, p)| p.clone());
            for s in w.wake(params, |d| Some(d == "mac")) {
                start(&mut w, &mut timers, at, s);
            }
            at += Duration::from_secs(5);
            tick += 1;
        }

        // The old rules: a failed watch waits 2 s doubling to 60 s; a row
        // that comes back is a new watch, its wait started over.
        let mut old_failed = 0;
        let mut waits: std::collections::HashMap<&str, (Duration, Duration)> = Default::default(); // id → (next start, next wait)
        let mut at = Duration::ZERO;
        let mut tick = 0;
        let mut shown_before: Vec<&str> = Vec::new();
        while at < end {
            let n = if tick % 2 == 0 { 10 } else { 7 };
            let now_shown: Vec<&str> = chats[..n].iter().map(|c| c.0).collect();
            waits.retain(|id, _| now_shown.contains(id));
            for id in &now_shown {
                if fails(id).is_none() {
                    continue;
                }
                let fresh = !shown_before.contains(id);
                let entry = waits.entry(id).or_insert((at, Duration::from_secs(2)));
                if fresh {
                    *entry = (at, Duration::from_secs(2));
                }
                // (A 5 s step: every start due in it.)
                while entry.0 < at + Duration::from_secs(5) {
                    old_failed += 1;
                    entry.0 += entry.1;
                    entry.1 = (entry.1 * 2).min(Duration::from_secs(60));
                }
            }
            shown_before = now_shown;
            at += Duration::from_secs(5);
            tick += 1;
        }
        println!("failed pull request watches started in 20 min, 10 chats (5 failing): before {old_failed} ({:.1}/min), after {failed_starts} ({:.2}/min)", old_failed as f64 / 20.0, failed_starts as f64 / 20.0);
        assert_eq!(failed_starts, 5, "each failing chat asked about once");
        assert!(old_failed > 100);
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
