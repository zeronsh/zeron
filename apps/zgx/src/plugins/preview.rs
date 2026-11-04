//! Who a template's power is up to: you, in Zeron's own window.
//!
//! A template is a whole app, so it runs with the app's powers. It gets them
//! only once you've kept it in Zeron's own consent window
//! (`template_consent`, its keep sheet, or allowing its package there).
//! Until then (one an agent made or switched to, one imported and signed by
//! "you", one edited since you kept it) it runs as a **preview**: it draws
//! as it likes, with a toast the host draws over it ("Previewing <name>",
//! Keep and Revert, host_bar.rs), and the host refuses its dangerous
//! actions (`BLOCKED`).
//!
//! What you kept is bound to the template's content: the sha256 of every
//! file it lays over Zeron's own (`digest`). The record lives in the app's
//! own data folder (`approvals_path`). The engine never writes it, and no
//! IPC, MCP tool or plugin reaches it, so an agent with only Zeron's tools
//! can make, edit and switch templates but never give one power. (Code
//! running as you with a shell can write any file of yours; that's outside
//! what this guards.)
//!
//! **Safe mode** (`safe`) is the way back that no template can undo: "Use
//! Default Interface" in the host's menu, Revert in its toast, holding ⌥ as Zeron opens,
//! or a template that crashed the window at launch twice running. It runs
//! Zeron's own whatever `.active` says (a template re-selecting itself from
//! a timer, a terminal or an agent changes nothing), closes the terminals
//! templates started, and stays on across launches until you pick a
//! template on Zeron's Templates page.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use sha2::{Digest, Sha256};

use super::live;

/// Host actions a preview can't call: running programs, your account,
/// plugins and templates, files, and what's sent to agents or answered for
/// them. Each name, or a prefix ending in `.`.
pub const BLOCKED: &[&str] = &[
    "terminal.start",
    "terminal.write",
    "terminal.command",
    "account.",
    "plugins.install",
    // (Deciding on and signing plugins are the consent window's alone, a
    // host of their own: never the main window's to refuse.)
    "plugins.update",
    "plugins.new",
    "plugins.remove",
    "plugins.setting_set",
    "templates.use",
    "templates.create",
    "templates.duplicate",
    "templates.delete",
    "templates.import",
    "templates.export",
    "templates.update",
    "files.write",
    "files.create",
    "files.move",
    "files.delete",
    "chat.send",
    "chat.respond",
    "chat.queue",
    "agents.install",
    "agents.set_key",
    "agents.sign_in",
    "acp_registry.add",
    "acp_registry.add_own",
    "acp_registry.env_set",
    "home.run_action",
    "home.save_action",
    "projects.clone",
    "engine.restart",
    "billing.checkout",
];

pub fn blocked(action: &str) -> bool {
    BLOCKED.iter().any(|b| if b.ends_with('.') { action.starts_with(b) } else { action == *b })
}

/// The template running as a preview now: its id and name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    pub id: String,
    pub name: String,
}

thread_local! {
    static NOW: RefCell<Option<Preview>> = const { RefCell::new(None) };
    /// What's told of the change (the host's toast and menu follow it).
    static HEARD: RefCell<Vec<Box<dyn Fn(Option<&Preview>)>>> = RefCell::default();
    /// Terminals started while a template other than Zeron's own ran:
    /// closed when safe mode starts. `(id, device)`.
    static TEMPLATE_TERMINALS: RefCell<Vec<(String, Option<String>)>> = RefCell::default();
}

static SAFE: AtomicBool = AtomicBool::new(false);
/// A template other than Zeron's own is laid over it now (approved or not).
static TEMPLATE_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn current() -> Option<Preview> {
    NOW.with(|n| n.borrow().clone())
}

pub fn set(preview: Option<Preview>) {
    let changed = NOW.with(|n| {
        let mut n = n.borrow_mut();
        let changed = *n != preview;
        *n = preview.clone();
        changed
    });
    if changed {
        HEARD.with(|h| h.borrow().iter().for_each(|f| f(preview.as_ref())));
    }
}

/// `f` hears each change of the preview (the host's toast, the menu bar).
pub fn on_change(f: impl Fn(Option<&Preview>) + 'static) {
    HEARD.with(|h| h.borrow_mut().push(Box::new(f)));
}

pub fn set_template_running(on: bool) {
    TEMPLATE_RUNNING.store(on, Ordering::Relaxed);
}

/// Refused while a template runs as a preview: what the script is told.
pub fn refuse(action: &str) -> Result<(), String> {
    match current() {
        Some(p) if blocked(action) => {
            eprintln!("zeron-zgx: preview: {action} refused while {} isn't kept", p.id);
            Err(format!("{} is a preview. Keep it first, or go back to Zeron v3.", p.name))
        }
        _ => Ok(()),
    }
}

// ── Approvals ───────────────────────────────────────────────────────────────

/// What a template lays over Zeron's own, as one hash: each file it supplies
/// (by its path, its length and its bytes), as `live::compose` takes them.
pub fn digest(template: &Path) -> String {
    let mut h = Sha256::new();
    for (rel, path) in live::files(template, |rel| !live::template_meta(rel) && !live::never_templated(rel)) {
        let bytes = std::fs::read(&path).unwrap_or_default();
        h.update((rel.len() as u64).to_le_bytes());
        h.update(rel.as_bytes());
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Where what you kept is recorded: the app's own data folder.
pub fn approvals_path() -> Option<PathBuf> {
    zgx_script::app_data_dir(crate::APP_ID).map(|d| d.join("template-trust").join("approvals.json"))
}

fn read_approvals(path: &Path) -> BTreeMap<String, String> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Whether template `id`, as it is (`digest`), is one you kept.
pub fn approved_at(path: &Path, id: &str, digest: &str) -> bool {
    !digest.is_empty() && read_approvals(path).get(id).is_some_and(|d| d == digest)
}

/// Template `id` as it is (`digest`) kept: only the keep sheet and an
/// allowed package's install call this.
pub fn approve_at(path: &Path, id: &str, digest: &str) -> std::io::Result<()> {
    let mut all = read_approvals(path);
    all.insert(id.to_string(), digest.to_string());
    live::put(path, &serde_json::to_vec_pretty(&all).unwrap_or_default()).map(|_| ())
}

/// Forgotten (a template deleted, so one made later under its id starts
/// anew).
pub fn forget_at(path: &Path, id: &str) {
    let mut all = read_approvals(path);
    if all.remove(id).is_some() {
        let _ = live::put(path, &serde_json::to_vec_pretty(&all).unwrap_or_default());
    }
}

pub fn approve(id: &str, digest: &str) -> Result<(), String> {
    let path = approvals_path().ok_or("There's nowhere to keep it")?;
    approve_at(&path, id, digest).map_err(|e| format!("Couldn't keep it: {e}"))
}

/// Whether template `id` in `templates` runs with the app's powers: the
/// default, one of Zeron's official templates exactly as this build wrote it
/// (`official::verified`: Zeron's own code, as the default is), or one you
/// kept as it is.
pub fn trusted(templates: &Path, id: &str) -> bool {
    id == live::BUILTIN || approvals_path().is_some_and(|p| trusted_at(templates, id, &p))
}

/// `trusted`, with what you kept recorded at `approvals`.
pub fn trusted_at(templates: &Path, id: &str, approvals: &Path) -> bool {
    if id == live::BUILTIN {
        return true;
    }
    let dir = templates.join(id);
    super::official::verified(&dir).is_some() || approved_at(approvals, id, &digest(&dir))
}

// ── Safe mode ───────────────────────────────────────────────────────────────

fn safe_path() -> Option<PathBuf> {
    zgx_script::app_data_dir(crate::APP_ID).map(|d| d.join("template-trust").join("safe-mode"))
}

/// Safe mode is on: Zeron's own runs, whatever `.active` says. (Read from
/// the look's worker threads too: a process-wide flag.)
pub fn safe() -> bool {
    #[cfg(test)]
    if let Some(on) = TEST_SAFE.with(std::cell::Cell::get) {
        return on;
    }
    SAFE.load(Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    /// A test's own safe mode, on its thread alone (tests run in parallel).
    static TEST_SAFE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub fn enter_safe_for_test(on: bool) {
    TEST_SAFE.with(|s| s.set(Some(on)));
}

/// Read once at launch: on if it was on when Zeron last closed, if ⌥ is
/// held now, or if the last two launches with a template crashed before the
/// window settled (`launch_started`/`launch_settled`).
pub fn read_at_launch() {
    let held = option_held();
    let crashed = crash_looping();
    let on = held || crashed || safe_path().is_some_and(|p| p.is_file());
    if held || crashed {
        eprintln!("zeron-zgx: safe mode ({})", if held { "⌥ held at launch" } else { "the template crashed Zeron twice" });
        let _ = write_safe(true);
        LAUNCH_REASON.store(if held { 1 } else { 2 }, Ordering::Relaxed);
    }
    SAFE.store(on, Ordering::Relaxed);
}

/// Why safe mode started as Zeron opened (the host's toast says so once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafeReason {
    /// ⌥ held as it opened.
    Held,
    /// The template crashed the window at launch twice running.
    Crashed,
}

/// 0: none; 1: `Held`; 2: `Crashed`.
static LAUNCH_REASON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Why safe mode started at this launch, told once.
pub fn take_safe_at_launch() -> Option<SafeReason> {
    match LAUNCH_REASON.swap(0, Ordering::Relaxed) {
        1 => Some(SafeReason::Held),
        2 => Some(SafeReason::Crashed),
        _ => None,
    }
}

fn write_safe(on: bool) -> std::io::Result<()> {
    let Some(path) = safe_path() else { return Ok(()) };
    if on {
        live::put(&path, b"on\n").map(|_| ())
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Into safe mode: Zeron's own from now on (and across launches), and the
/// terminals templates started closed.
pub fn enter_safe() {
    SAFE.store(true, Ordering::Relaxed);
    if let Err(error) = write_safe(true) {
        eprintln!("zeron-zgx: safe mode isn't remembered: {error}");
    }
    close_template_terminals();
}

/// Out of safe mode: only Zeron's own Templates page does this, as you pick
/// a template there.
pub fn leave_safe() {
    if SAFE.swap(false, Ordering::Relaxed) {
        let _ = write_safe(false);
    }
}

/// A terminal opened by the window (`terminal.claimed`): kept to close in
/// safe mode if a template other than Zeron's own was running.
pub fn terminal_opened(id: &str, device: Option<String>) {
    if TEMPLATE_RUNNING.load(Ordering::Relaxed) && !id.is_empty() {
        TEMPLATE_TERMINALS.with(|t| t.borrow_mut().push((id.to_string(), device)));
    }
}

fn close_template_terminals() {
    let all = TEMPLATE_TERMINALS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    for (id, device) in all {
        crate::engine::get().spawn(async move {
            let mut params = serde_json::json!({ "terminalId": id });
            if let Some(device) = device {
                params["targetDeviceId"] = serde_json::json!(device);
            }
            if let Err(error) = crate::engine::get().call("CloseTerminal", params).await {
                eprintln!("zeron-zgx: safe mode: a template's terminal didn't close: {error}");
            }
        });
    }
}

// ── Launch crashes ──────────────────────────────────────────────────────────

fn launches_path() -> Option<PathBuf> {
    zgx_script::app_data_dir(crate::APP_ID).map(|d| d.join("template-trust").join("launches"))
}

/// How many launches in a row with a template laid down never settled.
fn crash_looping() -> bool {
    launches_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|n| n >= 2)
}

/// The main window opens with a template other than Zeron's own: counted
/// until it settles.
pub fn launch_started(template: bool) {
    let Some(path) = launches_path() else { return };
    if !template {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let n = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
    let _ = live::put(&path, format!("{}\n", n + 1).as_bytes());
}

/// The window has run a while: this launch didn't crash.
pub fn launch_settled() {
    if let Some(path) = launches_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// ⌥ (Option, Alt) held as Zeron opens.
#[cfg(target_os = "macos")]
fn option_held() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state: i32) -> u64;
    }
    // kCGEventSourceStateCombinedSessionState, kCGEventFlagMaskAlternate.
    // SAFETY: a plain query of the keyboard's modifier state.
    let flags = unsafe { CGEventSourceFlagsState(0) };
    flags & 0x0008_0000 != 0
}

#[cfg(not(target_os = "macos"))]
fn option_held() -> bool {
    // Linux and Windows: `ZERON_SAFE_MODE=1` (and `--zeron-template`) until
    // the platform's modifier state is read here.
    std::env::var_os("ZERON_SAFE_MODE").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-preview-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn preview_refuses_the_dangerous_actions_only() {
        set(None);
        assert!(refuse("terminal.start").is_ok(), "nothing in preview: all allowed");
        set(Some(Preview { id: "agent".into(), name: "Agent's".into() }));
        for action in ["terminal.start", "terminal.write", "account.sign_out", "account.delete", "plugins.install", "templates.use", "files.write", "chat.send", "chat.respond"] {
            assert!(refuse(action).is_err(), "{action}");
        }
        for action in ["files.read", "files.list", "chat.open", "appearance", "templates.review", "sessions.seen"] {
            assert!(refuse(action).is_ok(), "{action}");
        }
        set(None);
    }

    /// R7: a template an agent made and switched to (MCP create_template +
    /// use_template, which only write files and `.active`) never runs with
    /// Zeron's powers: it's a preview, refused the dangerous actions, until
    /// kept in Zeron's own window; and edited after, it's a preview again.
    #[test]
    fn an_agent_s_template_goes_live_as_a_preview_not_at_full_power() {
        let root = dir("agent");
        let templates = root.join("templates");
        let approvals = root.join("trust/approvals.json");
        // What the engine's create_template and use_template leave.
        write(&templates, "agent-made/zeron-template.json", r#"{"id":"agent-made","name":"Sleek"}"#);
        write(&templates, "agent-made/screens/shell/sidebar.luau", "ui.call('terminal.start', {})");
        live::set_active(&templates, "agent-made").unwrap();
        let id = live::active(&templates);
        assert_eq!(id, "agent-made");
        assert!(!trusted_at(&templates, &id, &approvals), "never approved in Zeron's window");
        // What the window does with it (`looked`): a preview, gated.
        set((!trusted_at(&templates, &id, &approvals)).then(|| Preview { id: id.clone(), name: "Sleek".into() }));
        assert!(refuse("terminal.start").is_err() && refuse("account.sign_out").is_err() && refuse("plugins.install").is_err());
        // Signed by "you" and installed at once by the engine: the same.
        write(&templates, "mine/zeron-template.json", r#"{"id":"mine"}"#);
        write(&templates, "mine/.imported.json", r#"{"key":"k","author":{"kind":"you"}}"#);
        write(&templates, "mine/screens/x.luau", "-- x");
        assert!(!trusted_at(&templates, "mine", &approvals));
        // Kept in Zeron's window: its powers, until its files change.
        approve_at(&approvals, &id, &digest(&templates.join(&id))).unwrap();
        assert!(trusted_at(&templates, &id, &approvals));
        write(&templates, "agent-made/screens/shell/sidebar.luau", "-- changed by the agent after");
        assert!(!trusted_at(&templates, &id, &approvals));
        // Zeron's own needs no keeping.
        assert!(trusted_at(&templates, live::BUILTIN, &approvals));
        set(None);
    }

    #[test]
    fn what_you_kept_is_bound_to_the_template_s_content() {
        let root = dir("digest");
        let t = root.join("t");
        write(&t, "zeron-template.json", r#"{"name":"T"}"#);
        write(&t, "screens/shell/sidebar.luau", "-- t");
        let approvals = root.join("trust/approvals.json");
        let d = digest(&t);
        assert!(!approved_at(&approvals, "t", &d));
        approve_at(&approvals, "t", &d).unwrap();
        assert!(approved_at(&approvals, "t", &d));
        // Its manifest and a protected file don't count; a screen edited does.
        write(&t, "zeron-template.json", r#"{"name":"T renamed"}"#);
        write(&t, "screens/plugins/page.luau", "-- never laid down");
        assert_eq!(digest(&t), d);
        write(&t, "screens/shell/sidebar.luau", "-- t, edited by an agent");
        assert!(!approved_at(&approvals, "t", &digest(&t)), "edited since kept: a preview again");
        // A file added: also new.
        write(&t, "screens/shell/sidebar.luau", "-- t");
        write(&t, "theme.luau", "return nil");
        assert!(!approved_at(&approvals, "t", &digest(&t)));
        forget_at(&approvals, "t");
        assert!(!approved_at(&approvals, "t", &d));
    }
}
