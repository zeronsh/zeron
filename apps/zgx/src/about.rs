//! Settings → About & updates (ui/screens/settings/about.luau): which Zeron
//! this is, and its updates. The updater is the engine's (crates/update): it
//! knows its own version and the newest release, checking the public
//! release feed every few hours, and updates an install it manages (a
//! service install on Linux or a Mac), starting itself again on the new
//! version. The Zeron app carries its own engine and updates it from its own
//! window, so there the engine refuses (`unmanaged`). This app isn't
//! packaged yet: nothing here updates it.
//!
//!   shared `about`   { app_version, app_build, engine_version?, engine_build?,
//!                      latest?, available, checked_at?, error? }
//!   about.check      the release feed asked now: { latest, available }
//!   about.apply      the engine updates itself: { version }; rejects with
//!                    unmanaged, current, offline or failed

use std::{cell::RefCell, rc::Rc, time::Duration};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{self, rpc::Watch};

/// The public release feed the engine's updater reads (served without an
/// account). `ZERON_ZGX_RELEASE_FEED` points it elsewhere. Zeron Preview's
/// is a channel of its own (.github/workflows/zgx-preview.yml), as its
/// engine's is (src/engine/manager.rs): never the Zeron app's.
const FEED: &str = if cfg!(feature = "preview") {
    "https://zeron.sh/releases/preview/manifest.json"
} else {
    "https://zeron.sh/releases/manifest.json"
};
const FEED_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Default)]
struct About {
    /// The engine's `UpdateStatus`, as last heard.
    status: RefCell<Json>,
    /// The engine's build (`EngineInfo.build`).
    build: RefCell<Option<String>>,
    /// The feed's newest version and when it was asked here (epoch ms).
    checked: RefCell<Option<(String, i64)>>,
    shared: RefCell<Vec<Shared>>,
    watch: RefCell<Option<Watch>>,
}

thread_local! {
    static ABOUT: Rc<About> = Rc::default();
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let about = ABOUT.with(Rc::clone);
    about.shared.borrow_mut().push(host.shared("about", about.value()));
    if about.shared.borrow().len() == 1 {
        let weak = Rc::downgrade(&about);
        engine::get().on_connect(move |rpc| {
            let Some(about) = weak.upgrade() else { return };
            let heard = Rc::downgrade(&about);
            let watch = rpc.watch("UpdateStatus", json!({}), move |item| {
                if let Some(about) = heard.upgrade() {
                    *about.status.borrow_mut() = item;
                    about.publish();
                }
            });
            *about.watch.borrow_mut() = Some(watch);
            let (me, rpc) = (about.clone(), rpc.clone());
            engine::get().spawn(async move {
                if let Ok(info) = rpc.call("EngineInfo", json!({})).await {
                    *me.build.borrow_mut() = info["build"].as_str().map(Into::into);
                    me.publish();
                }
            });
        });
    }

    let a = about.clone();
    host.async_action("about.check", move |_| {
        let a = a.clone();
        async move {
            // (Zeron Preview reads its own channel only.)
            let feed = std::env::var("ZERON_ZGX_RELEASE_FEED").ok().filter(|v| !cfg!(feature = "preview") && !v.trim().is_empty());
            let url = feed.unwrap_or_else(|| FEED.into());
            let latest = off_thread(move || newest(&url)).await.ok_or("failed")??;
            *a.checked.borrow_mut() = Some((latest.clone(), now_ms()));
            a.publish();
            let available = a.available();
            Ok(Value::record([("latest", Value::from(latest.as_str())), ("available", Value::Bool(available))]))
        }
    });
    host.async_action("about.apply", |_| async {
        match engine::get().call("ApplyUpdate", json!({})).await {
            Ok(answer) => Ok(Value::record([("version", Value::from(answer["version"].as_str().unwrap_or("")))])),
            Err(error) => Err(apply_outcome(&error).to_string()),
        }
    });
}

impl About {
    fn engine_version(&self) -> Option<String> {
        self.status.borrow()["currentVersion"].as_str().map(Into::into)
    }

    /// The newest version known: the engine's or this window's check,
    /// whichever is newer.
    fn latest(&self) -> Option<String> {
        let theirs = self.status.borrow()["latestVersion"].as_str().map(String::from);
        let ours = self.checked.borrow().as_ref().map(|(v, _)| v.clone());
        match (theirs, ours) {
            (Some(a), Some(b)) => Some(if newer(&b, &a) { b } else { a }),
            (a, b) => a.or(b),
        }
    }

    fn available(&self) -> bool {
        match (self.latest(), self.engine_version()) {
            (Some(latest), Some(current)) => newer(&latest, &current),
            _ => self.status.borrow()["updateAvailable"].as_bool().unwrap_or(false),
        }
    }

    fn value(&self) -> Value {
        let status = self.status.borrow();
        let theirs = status["checkedAt"].as_i64();
        let ours = self.checked.borrow().as_ref().map(|(_, at)| *at);
        let checked_at = theirs.max(ours);
        // The engine's last error, unless this window has checked since.
        let error = status["error"].as_str().filter(|_| ours.is_none_or(|o| theirs.is_some_and(|t| t > o)));
        let mut fields = vec![
            ("app_version", Value::from(env!("ZERON_VERSION"))),
            ("app_build", Value::from(app_build().as_str())),
            ("available", Value::Bool(self.available())),
        ];
        if let Some(v) = self.engine_version() {
            fields.push(("engine_version", Value::from(v.as_str())));
        }
        if let Some(b) = self.build.borrow().as_deref() {
            fields.push(("engine_build", Value::from(b)));
        }
        if let Some(l) = self.latest() {
            fields.push(("latest", Value::from(l.as_str())));
        }
        if let Some(at) = checked_at {
            fields.push(("checked_at", Value::from(at as f64)));
        }
        if let Some(e) = error {
            fields.push(("error", Value::from(e)));
        }
        Value::record(fields)
    }

    fn publish(&self) {
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }
}

/// The engine's refusal, as what happened.
fn apply_outcome(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("not update-managed") {
        "unmanaged"
    } else if lower.contains("already up to date") {
        "current"
    } else {
        engine::outcome(error)
    }
}

/// The feed's newest version (blocking: for a worker thread).
fn newest(url: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(FEED_TIMEOUT).user_agent("zeron-zgx").build();
    let manifest: Json = match agent.get(url).call() {
        Ok(res) => res.into_json().map_err(|_| "failed".to_string())?,
        Err(ureq::Error::Transport(_)) => return Err("offline".into()),
        Err(ureq::Error::Status(..)) => return Err("failed".into()),
    };
    manifest["version"].as_str().filter(|v| !v.is_empty()).map(Into::into).ok_or_else(|| "failed".into())
}

/// Whether `a` is a later version than `b` (dotted numbers; a leading `v`
/// and anything after a `-` or `+` ignored).
fn newer(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        let v = v.trim().trim_start_matches('v');
        let core = v.split(['-', '+']).next().unwrap_or("");
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// This app's build: when its executable was built (seconds since the
/// epoch), as the engine says its own.
fn app_build() -> String {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or_else(String::new, |d| d.as_secs().to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked), as images.rs's.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(newer("0.2.131", "0.2.99"));
        assert!(newer("v0.3.0", "0.2.131"));
        assert!(!newer("0.2.131", "0.2.131"));
        assert!(!newer("0.2.130", "0.2.131-dev"));
        assert!(newer("1.0", "0.9.9"));
    }

    #[test]
    fn the_engine_s_refusals_say_what_happened() {
        assert_eq!(
            apply_outcome("this install is not update-managed — the desktop app updates from its UI"),
            "unmanaged"
        );
        assert_eq!(apply_outcome("already up to date (0.2.131)"), "current");
    }

    #[test]
    fn the_newest_known_version_is_the_engine_s_or_a_later_check() {
        let about = About::default();
        *about.status.borrow_mut() = json!({ "currentVersion": "0.2.130", "latestVersion": "0.2.130", "updateAvailable": false, "checkedAt": 5 });
        assert!(!about.available());
        *about.checked.borrow_mut() = Some(("0.2.131".into(), 10));
        assert_eq!(about.latest().as_deref(), Some("0.2.131"));
        assert!(about.available());
    }
}
