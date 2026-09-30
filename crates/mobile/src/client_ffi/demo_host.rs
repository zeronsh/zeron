//! Demo mode's answers to the untyped host RPCs [`super::CoreClient::host_call`]
//! forwards (Demo has no engine to relay to). Just enough of the engine's
//! harness-install and agent-account surface for the Android Settings →
//! Agents panel to be developed and screenshot offline: a catalog with
//! installed and installable agents, installs that take a few seconds and can
//! be cancelled, an update to apply (alone or with Update all), uninstalls,
//! and browser / paste-code sign-ins that complete.
//!
//! Reply shapes mirror the engine (`HarnessDescriptor`, `AgentAccountsSnapshot`,
//! `AgentLoginStart`, `AgentLoginPoll`, `HarnessUpdateStatus`), camelCase JSON.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use serde_json::{Value, json};

use super::types::{CoreError, CoreResult};

/// How long a demo install takes.
const INSTALL_TIME: Duration = Duration::from_millis(2600);
/// How long a demo `git clone` takes.
const CLONE_TIME: Duration = Duration::from_millis(900);
/// How long a demo update takes.
const UPDATE_TIME: Duration = Duration::from_millis(1800);
/// Browser sign-ins report `done` on this poll.
const POLLS_TO_FINISH: u32 = 3;

/// (id, name, latest release)
const HARNESSES: &[(&str, &str, &str)] = &[
    ("claude-code", "Claude Code", "2.1.14"),
    ("codex", "Codex", "0.42.0"),
    ("opencode", "OpenCode", "0.9.2"),
    ("grok", "Grok", "0.3.1"),
    ("pi", "Pi", "0.12.0"),
];

/// What a demo uninstall removes, per agent.
fn removed_paths(id: &str) -> Vec<&'static str> {
    match id {
        "claude-code" => vec!["~/.local/bin/claude", "~/.local/share/claude"],
        "codex" => vec!["~/.local/bin/codex", "~/.codex/packages/standalone"],
        "opencode" => vec!["npm uninstall -g opencode-ai (/usr/local)"],
        "grok" => vec!["~/.grok/bin/grok", "~/.grok/downloads"],
        _ => vec!["npm uninstall -g @earendil-works/pi-coding-agent (/usr/local)"],
    }
}

#[derive(Default)]
struct State {
    installed: HashSet<String>,
    /// Installed version per agent; the latest release when absent.
    versions: HashMap<String, String>,
    updating: HashSet<String>,
    installing: HashMap<String, Arc<Mutex<bool>>>,
    /// harness → (account id, email, plan)
    accounts: Vec<(String, String, String, String)>,
    /// login id → (harness, polls so far, paste-code mode)
    logins: HashMap<String, (String, u32, bool)>,
    next: u32,
}

/// The simulated engine behind Demo mode's host calls.
pub(crate) struct DemoHost {
    state: Mutex<State>,
}

impl Default for DemoHost {
    fn default() -> Self {
        let state = State {
            installed: ["claude-code", "codex"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            // Codex is one release behind: Update / Update all have work.
            versions: [("codex".to_string(), "0.41.0".to_string())].into(),
            accounts: vec![(
                "claude-code".into(),
                "acct-demo".into(),
                "demo@zeron.sh".into(),
                "Max".into(),
            )],
            ..State::default()
        };
        Self {
            state: Mutex::new(state),
        }
    }
}

fn param<'a>(params: &'a Value, key: &str) -> CoreResult<&'a str> {
    params[key]
        .as_str()
        .ok_or_else(|| CoreError::InvalidArgument {
            message: format!("missing `{key}`"),
        })
}

impl DemoHost {
    fn descriptors(&self) -> Value {
        let s = self.state.lock().unwrap();
        Value::Array(
            HARNESSES
                .iter()
                .map(|(id, name, _)| {
                    let installed = s.installed.contains(*id);
                    json!({
                        "id": id,
                        "name": name,
                        "supportsSteering": true,
                        "steeringMode": "step-boundary",
                        "reasoningLevels": ["low", "medium", "high"],
                        "installed": installed,
                        "canInstall": true,
                        "enabled": installed,
                    })
                })
                .collect(),
        )
    }

    fn statuses(&self, only: Option<&str>) -> Value {
        let s = self.state.lock().unwrap();
        Value::Array(
            HARNESSES
                .iter()
                .filter(|h| only.is_none_or(|o| o == h.0))
                .map(|(id, _, latest)| {
                    let installed = s.installed.contains(*id);
                    let version = s.versions.get(*id).map_or(*latest, String::as_str);
                    let phase = match () {
                        _ if !installed => "dormant",
                        _ if s.updating.contains(*id) => "installing",
                        _ if version != *latest => "available",
                        _ => "current",
                    };
                    json!({
                        "harness": id,
                        "installedVersion": installed.then_some(version),
                        "latestVersion": latest,
                        "phase": phase,
                        "canApply": true,
                    })
                })
                .collect(),
        )
    }

    /// One demo update: `installing` for a moment, then the latest release.
    async fn apply(&self, harness: &str) -> CoreResult<String> {
        let latest = HARNESSES
            .iter()
            .find(|h| h.0 == harness)
            .map(|h| h.2)
            .ok_or_else(|| CoreError::HostError {
                message: format!("ApplyHarnessUpdate: unknown harness `{harness}`"),
            })?;
        {
            let mut s = self.state.lock().unwrap();
            let current = s.versions.get(harness).map_or(latest, String::as_str);
            if !s.installed.contains(harness) || current == latest {
                return Err(CoreError::HostError {
                    message: "ApplyHarnessUpdate: no applicable harness update".into(),
                });
            }
            s.updating.insert(harness.to_owned());
        }
        sleep(UPDATE_TIME).await;
        let mut s = self.state.lock().unwrap();
        s.updating.remove(harness);
        s.versions.remove(harness);
        Ok(latest.to_owned())
    }

    fn accounts(&self) -> Value {
        let s = self.state.lock().unwrap();
        json!({
            "accounts": s.accounts.iter().map(|(harness, id, email, plan)| json!({
                "id": id,
                "harness": harness,
                "email": email,
                "planLabel": plan,
                "active": true,
                "usageWindows": [],
                "authKind": "oauth",
                "switchable": true,
            })).collect::<Vec<_>>(),
            "warnings": [],
        })
    }

    fn sign_in(&self, harness: &str) {
        let mut s = self.state.lock().unwrap();
        s.accounts.retain(|a| a.0 != harness);
        let n = s.next;
        s.next += 1;
        s.accounts.push((
            harness.to_owned(),
            format!("acct-{n}"),
            "you@example.com".into(),
            "Pro".into(),
        ));
    }

    /// Answer one host RPC the way an engine would.
    pub(crate) async fn call(&self, method: &str, params: Value) -> CoreResult<Value> {
        match method {
            "ListHarnesses" => Ok(self.descriptors()),
            "InstallHarness" => {
                let harness = param(&params, "harness")?.to_owned();
                if !HARNESSES.iter().any(|h| h.0 == harness) {
                    return Err(CoreError::HostError {
                        message: format!("InstallHarness: unknown harness `{harness}`"),
                    });
                }
                let cancelled = Arc::new(Mutex::new(false));
                {
                    let mut s = self.state.lock().unwrap();
                    if s.installing.contains_key(&harness) {
                        return Err(CoreError::HostError {
                            message: "InstallHarness: already installing".into(),
                        });
                    }
                    s.installing.insert(harness.clone(), cancelled.clone());
                }
                // Poll the cancel flag while "downloading".
                let steps = 13;
                for _ in 0..steps {
                    sleep(INSTALL_TIME / steps).await;
                    if *cancelled.lock().unwrap() {
                        break;
                    }
                }
                let mut s = self.state.lock().unwrap();
                s.installing.remove(&harness);
                if *cancelled.lock().unwrap() {
                    return Err(CoreError::HostError {
                        message: "InstallHarness: cancelled".into(),
                    });
                }
                s.installed.insert(harness);
                drop(s);
                Ok(self.descriptors())
            }
            "CancelInstall" => {
                let harness = param(&params, "harness")?;
                if let Some(flag) = self.state.lock().unwrap().installing.get(harness) {
                    *flag.lock().unwrap() = true;
                }
                Ok(json!({}))
            }
            "CheckHarnessUpdates" => Ok(self.statuses(params["harness"].as_str())),
            "ListHarnessUpdates" => Ok(self.statuses(None)),
            "ApplyHarnessUpdate" => {
                let harness = param(&params, "harness")?;
                let version = self.apply(harness).await?;
                Ok(json!({ "ok": true, "version": version }))
            }
            "ApplyAllHarnessUpdates" => {
                let pending: Vec<String> = self
                    .statuses(None)
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| s["phase"] == "available")
                    .filter_map(|s| s["harness"].as_str().map(str::to_owned))
                    .collect();
                let mut updated = Vec::new();
                for harness in pending {
                    let version = self.apply(&harness).await?;
                    updated.push(json!({ "harness": harness, "version": version }));
                }
                Ok(json!({
                    "updated": updated,
                    "failed": [],
                    "manual": [],
                    "statuses": self.statuses(None),
                }))
            }
            "UninstallHarness" => {
                let harness = param(&params, "harness")?.to_owned();
                let dry_run = params["dryRun"].as_bool().unwrap_or(false);
                if !self.state.lock().unwrap().installed.contains(&harness) {
                    return Err(CoreError::HostError {
                        message: "UninstallHarness: not installed on this device".into(),
                    });
                }
                if !dry_run {
                    sleep(INSTALL_TIME / 3).await;
                    let mut s = self.state.lock().unwrap();
                    s.installed.remove(&harness);
                    s.versions.remove(&harness);
                }
                Ok(json!({
                    "harness": harness,
                    "removed": removed_paths(&harness),
                    "dryRun": dry_run,
                    "harnesses": self.descriptors(),
                }))
            }
            "ListAgentAccounts" => Ok(self.accounts()),
            "StartAgentLogin" => {
                let harness = param(&params, "harness")?.to_owned();
                // Claude demonstrates the paste-code flow; the rest the
                // browser callback.
                let paste = harness == "claude-code";
                let mut s = self.state.lock().unwrap();
                let login_id = format!("login-{}", s.next);
                s.next += 1;
                s.logins
                    .insert(login_id.clone(), (harness.clone(), 0, paste));
                Ok(json!({
                    "loginId": login_id,
                    "url": format!("https://zeron.sh/demo/sign-in/{harness}"),
                    "mode": if paste { "paste-code" } else { "browser" },
                }))
            }
            "PollAgentLogin" => {
                let login_id = param(&params, "loginId")?;
                let mut s = self.state.lock().unwrap();
                let Some(login) = s.logins.get_mut(login_id) else {
                    return Ok(json!({ "status": "error", "message": "Sign-in expired" }));
                };
                login.1 += 1;
                let (harness, polls, paste) = login.clone();
                if paste || polls < POLLS_TO_FINISH {
                    return Ok(json!({ "status": "pending" }));
                }
                s.logins.remove(login_id);
                drop(s);
                self.sign_in(&harness);
                Ok(json!({ "status": "done" }))
            }
            "CompleteAgentLogin" => {
                let login_id = param(&params, "loginId")?;
                let code = param(&params, "code")?;
                let login = self.state.lock().unwrap().logins.remove(login_id);
                let Some((harness, _, _)) = login else {
                    return Err(CoreError::HostError {
                        message: "CompleteAgentLogin: sign-in expired".into(),
                    });
                };
                if code.trim().is_empty() {
                    return Err(CoreError::HostError {
                        message: "CompleteAgentLogin: empty code".into(),
                    });
                }
                self.sign_in(&harness);
                Ok(self.accounts())
            }
            "CloneRepo" => {
                let url = param(&params, "url")?.trim().trim_end_matches('/');
                // The engine's naming: last path segment without `.git`.
                let name = url
                    .trim_end_matches(".git")
                    .rsplit(['/', ':'])
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("repo")
                    .to_owned();
                if !url.contains(['/', ':']) {
                    return Err(CoreError::HostError {
                        message: format!("CloneRepo: not a repository URL `{url}`"),
                    });
                }
                sleep(CLONE_TIME).await;
                Ok(json!({
                    "path": format!("/Users/dev/Projects/{name}"),
                    "name": name,
                    "defaultBranch": "main",
                }))
            }
            "CancelAgentLogin" => {
                let login_id = param(&params, "loginId")?;
                self.state.lock().unwrap().logins.remove(login_id);
                Ok(json!({ "ok": true }))
            }
            "ForgetAgentAccount" => {
                let account_id = param(&params, "accountId")?;
                self.state
                    .lock()
                    .unwrap()
                    .accounts
                    .retain(|a| a.1 != account_id);
                Ok(self.accounts())
            }
            other => Err(CoreError::Unsupported {
                message: format!("{other} (demo)"),
            }),
        }
    }
}

/// A timer future that needs no reactor: one short-lived thread per sleep
/// (demo installs only), so the client runtime's two workers never block.
fn sleep(duration: Duration) -> impl Future<Output = ()> {
    struct Timer {
        shared: Arc<Mutex<(bool, Option<Waker>)>>,
    }
    impl Future for Timer {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let mut s = self.shared.lock().unwrap();
            if s.0 {
                Poll::Ready(())
            } else {
                s.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
    let shared = Arc::new(Mutex::new((false, None::<Waker>)));
    let fire = shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let mut s = fire.lock().unwrap();
        s.0 = true;
        if let Some(w) = s.1.take() {
            w.wake();
        }
    });
    Timer { shared }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block<T>(fut: impl Future<Output = T>) -> T {
        // A minimal executor: park the thread until woken.
        struct ThreadWaker(std::thread::Thread);
        impl std::task::Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut fut = std::pin::pin!(fut);
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::park_timeout(Duration::from_millis(50));
        }
    }

    fn installed(host: &DemoHost, id: &str) -> bool {
        block(host.call("ListHarnesses", json!({})))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["id"] == id)
            .unwrap()["installed"]
            .as_bool()
            .unwrap()
    }

    #[test]
    fn install_flips_installed_and_reports_version() {
        let host = DemoHost::default();
        assert!(!installed(&host, "grok"));
        let catalog = block(host.call("InstallHarness", json!({ "harness": "grok" }))).unwrap();
        assert!(
            catalog
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["id"] == "grok" && h["installed"] == true)
        );
        let updates =
            block(host.call("CheckHarnessUpdates", json!({ "harness": "grok" }))).unwrap();
        assert_eq!(updates[0]["installedVersion"], "0.3.1");
    }

    #[test]
    fn update_all_and_uninstall() {
        let host = DemoHost::default();
        let statuses = block(host.call("ListHarnessUpdates", json!({}))).unwrap();
        let codex = |v: &Value| {
            v.as_array()
                .unwrap()
                .iter()
                .find(|s| s["harness"] == "codex")
                .cloned()
                .unwrap()
        };
        assert_eq!(codex(&statuses)["phase"], "available");
        let all = block(host.call("ApplyAllHarnessUpdates", json!({}))).unwrap();
        assert_eq!(all["updated"][0]["harness"], "codex");
        assert_eq!(codex(&all["statuses"])["phase"], "current");
        assert_eq!(codex(&all["statuses"])["installedVersion"], "0.42.0");
        let again = block(host.call("ApplyAllHarnessUpdates", json!({}))).unwrap();
        assert!(again["updated"].as_array().unwrap().is_empty());

        let preview = block(host.call(
            "UninstallHarness",
            json!({ "harness": "claude-code", "dryRun": true }),
        ))
        .unwrap();
        assert_eq!(preview["removed"][0], "~/.local/bin/claude");
        assert!(installed(&host, "claude-code"));
        block(host.call("UninstallHarness", json!({ "harness": "claude-code" }))).unwrap();
        assert!(!installed(&host, "claude-code"));
        // Accounts stay unless signed out.
        let snap = block(host.call("ListAgentAccounts", json!({}))).unwrap();
        assert!(!snap["accounts"].as_array().unwrap().is_empty());
        assert!(block(host.call("UninstallHarness", json!({ "harness": "claude-code" }))).is_err());
    }

    #[test]
    fn cancelled_install_stays_uninstalled() {
        let host = Arc::new(DemoHost::default());
        let canceller = host.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            block(canceller.call("CancelInstall", json!({ "harness": "pi" }))).unwrap();
        });
        let err = block(host.call("InstallHarness", json!({ "harness": "pi" }))).unwrap_err();
        assert!(matches!(err, CoreError::HostError { .. }));
        assert!(!installed(&host, "pi"));
    }

    #[test]
    fn browser_login_completes_after_polls() {
        let host = DemoHost::default();
        let start = block(host.call("StartAgentLogin", json!({ "harness": "codex" }))).unwrap();
        assert_eq!(start["mode"], "browser");
        let id = start["loginId"].as_str().unwrap().to_owned();
        let mut status = String::new();
        for _ in 0..POLLS_TO_FINISH {
            let poll = block(host.call("PollAgentLogin", json!({ "loginId": id }))).unwrap();
            status = poll["status"].as_str().unwrap().to_owned();
        }
        assert_eq!(status, "done");
        let snap = block(host.call("ListAgentAccounts", json!({}))).unwrap();
        assert!(
            snap["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["harness"] == "codex")
        );
    }

    #[test]
    fn paste_code_login_and_sign_out() {
        let host = DemoHost::default();
        let start =
            block(host.call("StartAgentLogin", json!({ "harness": "claude-code" }))).unwrap();
        assert_eq!(start["mode"], "paste-code");
        let id = start["loginId"].as_str().unwrap();
        let snap = block(host.call(
            "CompleteAgentLogin",
            json!({ "loginId": id, "code": "abc#123" }),
        ))
        .unwrap();
        let account = snap["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["harness"] == "claude-code")
            .unwrap()
            .clone();
        assert_eq!(account["email"], "you@example.com");
        let snap = block(host.call(
            "ForgetAgentAccount",
            json!({ "harness": "claude-code", "accountId": account["id"] }),
        ))
        .unwrap();
        assert!(snap["accounts"].as_array().unwrap().is_empty());
    }

    #[test]
    fn clone_names_the_checkout_like_the_engine() {
        let host = DemoHost::default();
        let repo = block(host.call(
            "CloneRepo",
            json!({ "url": "git@github.com:acme/widgets.git" }),
        ))
        .unwrap();
        assert_eq!(repo["path"], "/Users/dev/Projects/widgets");
        let err = block(host.call("CloneRepo", json!({ "url": "widgets" }))).unwrap_err();
        assert!(matches!(err, CoreError::HostError { .. }));
    }

    #[test]
    fn unknown_methods_are_unsupported() {
        let host = DemoHost::default();
        let err = block(host.call("OpenTerminal", json!({}))).unwrap_err();
        assert!(matches!(err, CoreError::Unsupported { .. }));
    }
}
