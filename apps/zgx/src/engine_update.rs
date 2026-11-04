//! Whether the engine this app talks to is older than the one it would
//! start now, and starting it again when that's this app's to do.
//!
//! Each connection asks the engine which build it is (`EngineInfo.build`,
//! absent on engines from before build ids: unknown, so stale) and the
//! engine binary on disk which build it is (`zeron-engine --build-id`).
//! They differ: the engine is stale. It's this app's own when it reports the
//! build this app last started in that data dir (manager.rs keeps it); only
//! then may this app start it again. An engine anyone else started
//! (scripts/staging-app.sh, the shipping app) is never touched.
//!
//! Shared state `engine.update`: `{ stale, ours }`. The scripts restart an
//! idle one at once and offer a stale one that's busy a quiet "Restart to
//! update" (screens/engine/update). Host action:
//!   engine.restart     stop the engine this app started; the supervisor
//!                      starts the one on disk in its place
//!
//! Zeron.app's beta manages Zeron.app's own engine, which restarts into its
//! bundle's by itself (src/engine/manager.rs): no staleness of this app's to
//! judge there, and `engine.restart` stops it at once. Instead each
//! connection asks whether the engine serves this interface (its
//! `zgx-interface-v1` capability, crates/proto): shared `engine.interface`,
//! `{ supported }`. An older engine (the stable app's from before the beta,
//! still running) can't: the scripts say so and offer to restart it, or to go
//! back to Stable (about.channel).

use std::cell::{Cell, RefCell};
use std::time::Duration;

use serde_json::json;
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::engine::{self, manager};

/// The capability an engine serving this interface advertises
/// (crates/proto `capabilities::ZGX_INTERFACE_V1`).
const INTERFACE: &str = "zgx-interface-v1";

/// Whether an engine advertising `capabilities` serves this interface. Only
/// the beta asks: a development build's staging engine is whatever the
/// checkout built.
fn serves_interface(capabilities: &serde_json::Value) -> bool {
    !cfg!(feature = "beta")
        || capabilities
            .as_array()
            .is_some_and(|all| all.iter().any(|c| c.as_str() == Some(INTERFACE)))
}

/// What's known of the engine's build: stale, and whether it's this app's.
fn record(stale: bool, ours: bool) -> Value {
    Value::record([("stale", Value::Bool(stale)), ("ours", Value::Bool(ours))])
}

/// Whether an engine reporting `running` is stale against the binary's
/// `on_disk` build, and whether it's the one this app `started`.
pub fn judge(running: Option<&str>, on_disk: Option<&str>, started: Option<&str>) -> (bool, bool) {
    let stale = match on_disk {
        // (No binary to start: nothing newer to start.)
        None => false,
        Some(disk) => running != Some(disk),
    };
    let ours = running.is_some() && running == started;
    (stale, ours)
}

/// What tells one build of a binary from another without running it: its
/// path, size, modification time and inode. Unchanged, its build is the one
/// asked last.
type Stamp = (std::path::PathBuf, u64, Option<std::time::SystemTime>, u64);

fn stamp_of(bin: &std::path::Path) -> Option<Stamp> {
    let meta = std::fs::metadata(bin).ok()?;
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let inode = 0;
    Some((bin.to_path_buf(), meta.len(), meta.modified().ok(), inode))
}

thread_local! {
    /// The binary's build as last asked, by its stamp.
    static ASKED: RefCell<Option<(Stamp, Option<String>)>> = const { RefCell::new(None) };
}

/// `bin`'s build: the one asked last while the file is unchanged, else asked
/// of the binary off this thread (it runs it), the task woken by the answer.
async fn build_of(bin: std::path::PathBuf) -> Option<String> {
    let stamp = stamp_of(&bin);
    if let Some(stamp) = &stamp
        && let Some(known) = ASKED.with(|a| a.borrow().as_ref().filter(|(s, _)| s == stamp).map(|(_, id)| id.clone()))
    {
        return known;
    }
    let (tx, mut rx) = zgx::task::latest::<Option<String>>();
    std::thread::spawn(move || {
        tx.send(manager::build_of(&bin));
    });
    let id = rx.recv().await.flatten();
    if let Some(stamp) = stamp {
        ASKED.with(|a| *a.borrow_mut() = Some((stamp, id.clone())));
    }
    id
}

/// How often a connected engine's build is checked against the binary on
/// disk again (a rebuild while the app runs).
const RECHECK: Duration = Duration::from_secs(60);

thread_local! {
    /// Every window's `engine.update`, and what it says now: one checker for
    /// the process, however many windows.
    static WINDOWS: RefCell<Vec<Shared>> = const { RefCell::new(Vec::new()) };
    static SAID: Cell<(bool, bool)> = const { Cell::new((false, false)) };
    static CHECKING: Cell<bool> = const { Cell::new(false) };
    /// Every window's `engine.interface`, and what it says now.
    static INTERFACES: RefCell<Vec<Shared>> = const { RefCell::new(Vec::new()) };
    static SUPPORTED: Cell<bool> = const { Cell::new(true) };
}

fn interface_record(supported: bool) -> Value {
    Value::record([("supported", Value::Bool(supported))])
}

fn tell_interface(supported: bool) {
    if SUPPORTED.with(|s| s.replace(supported)) == supported {
        return;
    }
    if !supported {
        eprintln!("zeron-zgx: engine: doesn't serve this interface ({INTERFACE}): an older engine");
    }
    INTERFACES.with(|w| {
        for shared in w.borrow().iter() {
            shared.set(interface_record(supported));
        }
    });
}

fn tell(judged: (bool, bool)) {
    SAID.with(|s| s.set(judged));
    WINDOWS.with(|w| {
        for shared in w.borrow().iter() {
            shared.set(record(judged.0, judged.1));
        }
    });
}

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    engine::install(cx);
    let (stale, ours) = SAID.with(Cell::get);
    WINDOWS.with(|w| w.borrow_mut().push(host.shared("engine.update", record(stale, ours))));
    let supported = SUPPORTED.with(Cell::get);
    INTERFACES.with(|w| w.borrow_mut().push(host.shared("engine.interface", interface_record(supported))));
    install_restart(host);
    if CHECKING.with(|c| c.replace(true)) {
        return;
    }
    // (Each connection's checks stop when a newer connection starts.)
    let generation = std::rc::Rc::new(std::cell::Cell::new(0u64));
    engine::get().on_connect(move |rpc| {
        let (rpc, generation) = (rpc.clone(), generation.clone());
        let mine = generation.get() + 1;
        generation.set(mine);
        engine::get().spawn(async move {
            let engine = engine::get();
            let Some(config) = engine.config() else {
                return;
            };
            let Ok(info) = rpc.call("EngineInfo", json!({})).await else {
                return;
            };
            tell_interface(serves_interface(&info["capabilities"]));
            // Zeron.app's engine follows its bundle by itself.
            if config.follows_bundle() {
                return;
            }
            let running = info["build"].as_str().map(str::to_owned);
            let mut said = None::<(bool, bool)>;
            while generation.get() == mine {
                let bin = config.startable().map(std::path::Path::to_path_buf);
                let on_disk = match bin {
                    Some(bin) => build_of(bin).await,
                    None => None,
                };
                let started = manager::started_build(config);
                let judged = judge(running.as_deref(), on_disk.as_deref(), started.as_deref());
                if said != Some(judged) {
                    let (stale, ours) = judged;
                    if stale {
                        eprintln!(
                            "zeron-zgx: engine: older than the one on disk ({} vs {}){}",
                            running.as_deref().unwrap_or("unknown"),
                            on_disk.as_deref().unwrap_or("?"),
                            if ours { "" } else { "; not this app's to restart" }
                        );
                    }
                    tell(judged);
                    said = Some(judged);
                }
                zgx::timer::sleep(RECHECK).await;
            }
        });
    });
}

fn install_restart(host: &ScriptHost) {
    host.async_action("engine.restart", |_| async move { crate::plugins::preview::refuse("engine.restart")?;
        let engine = engine::get();
        let Some(config) = engine.config() else {
            return Err("Zeron isn't available here".into());
        };
        // (Only one this app started, and only while it can start another;
        // in the beta, Zeron.app's own, which its bundle's replaces.)
        let Some(rpc) = engine.current() else {
            return Err("offline".into());
        };
        if config.follows_bundle() {
            eprintln!("zeron-zgx: engine: restarting Zeron's engine");
            engine.stop().await?;
            return Ok(Value::Nil);
        }
        let info = rpc.call("EngineInfo", json!({})).await?;
        let running = info["build"].as_str().map(str::to_owned);
        if config.startable().is_none() || running.is_none() || running != manager::started_build(config) {
            return Err("This engine wasn't started by this app.".into());
        }
        eprintln!("zeron-zgx: engine: starting the engine on disk in place of an older one");
        engine.stop().await?;
        Ok(Value::Nil)
    });
}

#[cfg(test)]
mod tests {
    use super::judge;

    /// The beta serves only an engine with the interface's capability (the
    /// stable engine from before the beta has none); a development build
    /// takes whatever its checkout built.
    #[test]
    fn the_beta_asks_for_the_interface_s_capability() {
        let with = serde_json::json!(["message-queue-v1", "zgx-interface-v1"]);
        let without = serde_json::json!(["message-queue-v1"]);
        assert!(super::serves_interface(&with));
        assert_eq!(super::serves_interface(&without), !cfg!(feature = "beta"));
        assert_eq!(super::serves_interface(&serde_json::Value::Null), !cfg!(feature = "beta"));
    }

    /// The engine's own name for it (zgx doesn't link crates/proto).
    #[test]
    fn the_capability_is_the_engine_s() {
        let proto = include_str!("../../../crates/proto/src/workspace.rs");
        assert!(proto.contains(&format!("ZGX_INTERFACE_V1: &str = \"{}\"", super::INTERFACE)));
    }

    #[test]
    fn a_binary_is_asked_again_only_once_it_changes() {
        let dir = std::env::temp_dir().join(format!("zgx-engine-stamp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("zeron-engine");
        std::fs::write(&bin, b"one").unwrap();
        let first = super::stamp_of(&bin).expect("a stamp");
        assert_eq!(super::stamp_of(&bin), Some(first.clone()), "unchanged: the same stamp");
        // Rebuilt (written aside and moved in, as a build does): another.
        let next = dir.join("next");
        std::fs::write(&next, b"two, longer").unwrap();
        std::fs::rename(&next, &bin).unwrap();
        assert_ne!(super::stamp_of(&bin), Some(first), "rebuilt: a new stamp");
        assert_eq!(super::stamp_of(&dir.join("gone")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_and_ours_follow_the_builds() {
        // Current, this app's.
        assert_eq!(judge(Some("a.1"), Some("a.1"), Some("a.1")), (false, true));
        // Rebuilt since it started: stale, and this app's to restart.
        assert_eq!(judge(Some("a.1"), Some("a.2"), Some("a.1")), (true, true));
        // From before build ids: unknown, so stale; never this app's.
        assert_eq!(judge(None, Some("a.2"), Some("a.1")), (true, false));
        // Someone else's engine (another build than the one this app started).
        assert_eq!(judge(Some("b.9"), Some("a.2"), Some("a.1")), (true, false));
        // No binary to start: nothing newer.
        assert_eq!(judge(Some("a.1"), None, None), (false, false));
    }
}
