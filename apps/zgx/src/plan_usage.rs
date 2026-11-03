//! How much of each agent's plan its live account has used, for the
//! composer's plan ring (Zeron's crates/ui/src/account_usage.rs): the
//! engine's `ListAgentAccounts`, as the shipping app's footer and Settings →
//! Accounts read it, for the device a composer works on.
//!
//! Shared state `plan_usage`: `{ device?, agents = { [harness] = Plan } }`,
//! a harness's entry only while its live account reports usage. A plan:
//! `{ email?, plan?, windows = { { label, used, resets? } } }`, `used` 0 to
//! 1 and `resets` when that window starts over (seconds since the epoch).
//! Host actions:
//!   plan_usage.track     follows a device (its id; "" this one): the
//!                        engine's last reading at once, then the providers
//!                        asked again, and every five minutes after
//!   plan_usage.refresh   the providers asked again (the card opening), no
//!                        more than every 30 seconds

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::engine;

/// The providers are asked (`forceUsage`) no more often than this; the
/// engine throttles them too.
const FORCE_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// Usage moves as turns run: asked again this often while followed.
const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
struct PlanUsage {
    shared: RefCell<Vec<Shared>>,
    /// The device followed (`None`: this one), once one is.
    device: RefCell<Option<Option<String>>>,
    /// The engine's last `AgentAccountsSnapshot` for it.
    snapshot: RefCell<Option<Json>>,
    last_forced: Cell<Option<Instant>>,
}

thread_local! {
    static USAGE: Rc<PlanUsage> = Rc::default();
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let usage = USAGE.with(Rc::clone);
    usage.shared.borrow_mut().push(host.shared("plan_usage", usage.value()));
    if usage.shared.borrow().len() == 1 {
        let weak = Rc::downgrade(&usage);
        engine::get().on_connect(move |_| {
            let poll = weak.clone();
            // While a device is followed, asked again every five minutes
            // (the loop goes with the connection's tasks).
            engine::get().spawn(async move {
                loop {
                    zgx::timer::sleep(POLL_INTERVAL).await;
                    let Some(usage) = poll.upgrade() else { return };
                    if usage.device.borrow().is_some() {
                        usage.last_forced.set(None);
                        usage.load(true).await;
                    }
                }
            });
            if let Some(usage) = weak.upgrade()
                && usage.device.borrow().is_some()
            {
                engine::get().spawn(async move { usage.load(true).await });
            }
        });
    }
    let u = usage.clone();
    host.action("plan_usage.track", move |device| {
        let device = text(&device);
        let device = (!device.is_empty()).then_some(device);
        let u = u.clone();
        if u.device.borrow().as_ref() == Some(&device) {
            return;
        }
        *u.device.borrow_mut() = Some(device);
        *u.snapshot.borrow_mut() = None;
        u.last_forced.set(None);
        u.publish();
        // The engine's last reading paints at once; the providers' follows.
        engine::get().spawn(async move {
            u.clone().load(false).await;
            u.load(true).await;
        });
    });
    let u = usage.clone();
    host.action("plan_usage.refresh", move |_| {
        let u = u.clone();
        if u.device.borrow().is_some() {
            engine::get().spawn(async move { u.load(true).await });
        }
    });
}

impl PlanUsage {
    async fn load(self: Rc<Self>, force: bool) {
        if force {
            if self.last_forced.get().is_some_and(|at| at.elapsed() < FORCE_MIN_INTERVAL) {
                return;
            }
            self.last_forced.set(Some(Instant::now()));
        }
        let Some(device) = self.device.borrow().clone() else { return };
        let mut params = json!({ "forceUsage": force });
        if let Some(device) = &device {
            params["targetDeviceId"] = json!(device);
        }
        match engine::get().call("ListAgentAccounts", params).await {
            // (Another device's, asked before the composer moved: not shown.)
            Ok(_) if self.device.borrow().as_ref() != Some(&device) => {}
            Ok(snapshot) => {
                *self.snapshot.borrow_mut() = Some(snapshot);
                self.publish();
            }
            Err(error) => eprintln!("zeron-zgx: plan usage unavailable: {error}"),
        }
    }

    fn value(&self) -> Value {
        let device = match &*self.device.borrow() {
            Some(Some(device)) => Value::from(device.as_str()),
            _ => Value::Nil,
        };
        let agents = self.snapshot.borrow().as_ref().map(plans).unwrap_or_default();
        Value::record([("device", device), ("agents", Value::record(agents))])
    }

    fn publish(&self) {
        let value = self.value();
        for shared in self.shared.borrow().iter() {
            shared.set(value.clone());
        }
    }
}

/// Each harness's live account with usage to show, from an
/// `AgentAccountsSnapshot` (`{accounts, warnings}`): who, on what plan, and
/// its windows as the engine names them.
fn plans(snapshot: &Json) -> Vec<(String, Value)> {
    snapshot["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|account| account["active"].as_bool() == Some(true))
        .filter_map(|account| {
            let harness = account["harness"].as_str()?;
            let windows: Vec<Value> = account["usageWindows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|window| {
                    let mut fields = vec![
                        ("label", Value::from(window["label"].as_str().unwrap_or_default())),
                        (
                            "used",
                            Value::from(window["usedFraction"].as_f64().unwrap_or(0.).clamp(0., 1.)),
                        ),
                    ];
                    if let Some(at) = window["resetsAt"].as_str().and_then(crate::agents::epoch_secs) {
                        fields.push(("resets", Value::from(at as f64)));
                    }
                    Value::record(fields)
                })
                .collect();
            if windows.is_empty() {
                return None;
            }
            let who = account["email"].as_str().or_else(|| account["displayName"].as_str());
            let plan = Value::record([
                ("email", who.map_or(Value::Nil, Value::from)),
                ("plan", account["planLabel"].as_str().map_or(Value::Nil, Value::from)),
                ("windows", Value::List(windows.into())),
            ]);
            Some((harness.to_string(), plan))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_shows_its_live_accounts_windows() {
        let snapshot = json!({
            "accounts": [
                { "id": "a", "harness": "codex", "active": false, "email": "old@x.io",
                  "usageWindows": [{ "label": "5h", "usedFraction": 0.9 }] },
                { "id": "b", "harness": "codex", "active": true, "email": "ada@x.io", "planLabel": "Pro",
                  "usageWindows": [
                      { "label": "5h", "usedFraction": 0.49, "resetsAt": "2026-11-03T15:45:00Z" },
                      { "label": "Weekly", "usedFraction": 1.4 }
                  ] },
                // Live, but reporting nothing: no ring.
                { "id": "c", "harness": "claude-code", "active": true, "usageWindows": [] }
            ],
            "warnings": []
        });
        let plans = plans(&snapshot);
        assert_eq!(plans.len(), 1);
        let (harness, plan) = &plans[0];
        assert_eq!(harness, "codex");
        assert_eq!(plan.field("email"), Some(&Value::from("ada@x.io")));
        assert_eq!(plan.field("plan"), Some(&Value::from("Pro")));
        let Some(Value::List(windows)) = plan.field("windows") else { panic!("its windows") };
        assert_eq!(windows.len(), 2);
        // Clamped to the whole of it; when it starts over, where said.
        assert_eq!(windows[1].field("used"), Some(&Value::from(1.)));
        assert_eq!(windows[1].field("resets"), None);
        assert!(windows[0].field("resets").is_some());
    }
}
