//! Who a template's power is up to: you, in Zeron's own window.
//!
//! A template is a whole app, so it runs with the app's powers. It gets them
//! only once you've kept it in Zeron's own consent window
//! (`template_consent`, its keep sheet, or allowing its package there).
//! Until then (one an agent made or switched to, one imported and signed by
//! "you", one edited since you kept it) it runs as a **preview**: it draws
//! as it likes, with a toast the host draws over it ("Previewing <name>",
//! Keep and Revert, host_bar.rs), and the host refuses its dangerous
//! actions (`BLOCKED`) and, through zgx's guard (`guard`), what would run a
//! program or carry what it reads out of the window. What it draws stays its
//! own, and what it saves is held until it's kept. It can still read what the
//! app reads, so don't word a preview as safe.
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
    "plugins.set_off",
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
    // (Opens a file as a double click would: a `.command` runs.)
    "files.open_external",
    "chat.send",
    "chat.respond",
    "chat.queue",
    "chat.queue_edit",
    "chat.queue_finish",
    "chat.retry",
    "chat.remove_held",
    // (Only one that says what an agent may do unasked: `chat.configure`.)
    "chat.configure",
    "agents.install",
    "agents.set_key",
    // (An API key, and what an agent's keys are called.)
    "agents.add_key",
    // (The API key every Cloud machine's agent uses, kept by the vault.)
    "agents.rename",
    "agents.sign_in",
    "agents.sign_out",
    "agents.remove",
    "acp_registry.add",
    "acp_registry.add_own",
    "acp_registry.env_set",
    "home.run_action",
    "home.save_action",
    "sessions.delete",
    "sessions.remove_device",
    "projects.clone",
    "projects.delete",
    "github.disconnect",
    "github.forget",
    "engine.restart",
    "billing.checkout",
    // (Each runs a machine, or says which one new chats run on: credits.)
    "cloud.wake",
    "cloud.sleep",
    "cloud.retry",
    "cloud.set_machine",
    // (What a Cloud chat reaches: the person's own choice.)
    "cloud.set_reach",
    "cloud.set_session_reach",
    // Managed MCP servers and their OAuth/credential flows.
    "mcp.upsert",
    "mcp.set_enabled",
    "mcp.delete",
    "mcp.start_auth",
    "mcp.cancel_auth",
    "mcp.disconnect_auth",
    "mcp.resolve_request",
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

/// A switch of what the main window runs (`looked`): begun before the files
/// of what it switches to are laid, finished by the window's reloads. zgx
/// asks one guard for every script the window runs, not by which load made
/// it, so a script runs under whatever preview stands as it asks. What shows
/// until the switch finishes is what ran before, under its own preview.
/// From a preview to anything else (Zeron's own, one kept, another kept
/// before, another preview): a load under the one before takes what ran
/// away (nothing of it, its effects on the states a new load sets among
/// them, runs ungated, nor is what it saves as it goes kept with what comes
/// next); then what comes next's stands (none, or its preview) and the
/// window loads again, so what stays binds what it keeps and asks for what
/// it needs as itself.
pub struct Switch {
    from: Option<Preview>,
    going: Option<Preview>,
    template: bool,
}

thread_local! {
    /// The window owes a load: a switch's load as itself failed, so what
    /// shows loaded under the preview before (what it keeps held). Cleared by
    /// a load that's Ok.
    static OWED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the window owes a load (`OWED`): the next switch reloads.
pub fn reload_owed() -> bool {
    OWED.with(std::cell::Cell::get)
}

fn owe(owed: bool) {
    OWED.with(|o| o.set(owed));
}

/// The host function the window's app asks whether a switch's first load
/// is running (`ui.call_sync`, `switching`): Zeron's own app loads nothing
/// then (app.luau). The host's alone to say: no script sets it.
pub const SWITCHING: &str = "window.switching";

thread_local! {
    /// A switch's first load runs (`Switch::finish`).
    static SWITCHING_NOW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a switch's first load runs: what `SWITCHING` answers.
pub fn switching() -> bool {
    SWITCHING_NOW.with(std::cell::Cell::get)
}

fn switching_now(on: bool) {
    SWITCHING_NOW.with(|s| s.set(on));
}

/// `switching` on while it lives: off however its switch ends (a look's
/// task dropped in a wait, the window closed there, included).
struct Switching;

impl Switching {
    fn on() -> Self {
        switching_now(true);
        Self
    }
}

impl Drop for Switching {
    fn drop(&mut self) {
        switching_now(false);
    }
}

/// What a switch asks laid for a load (`Switch::finish`): Zeron's own, or
/// what it switches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lay {
    ZeronsOwn,
    Next,
}

/// How a switch finished (`Switch::finish`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finished {
    /// What comes next runs, as itself.
    Settled,
    /// Its files weren't laid: what ran before stands.
    NotLaid,
    /// A load failed (the error). Not `lifted`: what ran before stands,
    /// under its preview. `lifted`: what ran before is gone and none stands;
    /// what shows is what loaded under it, to be looked at again.
    Failed { error: String, lifted: bool },
}

impl Switch {
    /// `going`: the preview switched to, if it's one; `template`: whether
    /// it's a template other than Zeron's own.
    pub fn begin(going: Option<Preview>, template: bool) -> Self {
        // (A template's terminals are noted while one runs, to close in safe
        // mode: on at once, off once a switch to Zeron's own has settled.)
        if template {
            set_template_running(true);
        }
        // What the window has is the stricter of what shows and what's laid
        // or being laid: from none, what comes next's from before its files
        // are laid (what a watch of them loads, or any reload, is under it).
        let from = current();
        if from.is_none() {
            set(going.clone());
        }
        Self { from, going, template }
    }

    /// Whether what ran before is a preview that isn't what comes next:
    /// then the window reloads, laid anew or not.
    pub fn holds(&self) -> bool {
        // (By id: a preview renamed is the same one, what it holds kept.)
        match (&self.from, &self.going) {
            (Some(from), Some(going)) => from.id != going.id,
            (from, _) => from.is_some(),
        }
    }

    fn settled(&self) {
        if !self.template {
            set_template_running(false);
        }
    }

    /// The files laid or not (`laid`), a reload asked for what changed
    /// (`asked`), and whether what comes next has an app of its own
    /// (`own_app`: a template's app.luau). `held` keeps or lets go of what
    /// the preview before held, before anything new loads. `lay` lays
    /// Zeron's own, or what comes next again.
    pub async fn finish<F, Fut>(self, laid: bool, asked: bool, own_app: bool, host: &zgx_script::ScriptHost, held: impl FnOnce(&zgx_script::ScriptHost), mut lay: F) -> Finished
    where
        F: FnMut(Lay) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        if !laid {
            return Finished::NotLaid;
        }
        if !self.holds() {
            // None before (or this same preview, by any name): what comes
            // next's stands since `begin`, and stays if its load fails (its
            // files are what a next load loads).
            held(host);
            if self.from.is_some() {
                set(self.going.clone());
            }
            if asked || reload_owed() {
                if let Err(error) = host.reload() {
                    return Finished::Failed { error, lifted: false };
                }
                owe(false);
            }
            self.settled();
            return Finished::Settled;
        }
        // The load under the one before only takes what ran away: it's of
        // Zeron's own app, which loads nothing while `SWITCHING` says so (a
        // template with an app of its own has Zeron's laid for it, then its
        // own again, still under the one before). So it asks for nothing,
        // and fails on no refusal. Every wait is here: from the one before
        // let go to the load as itself, nothing else runs, lands or loads.
        let switching = Switching::on();
        let own_app = self.template && own_app;
        if own_app && !lay(Lay::ZeronsOwn).await {
            return Finished::NotLaid;
        }
        let first = host.reload();
        let relaid = first.is_err() || !own_app || lay(Lay::Next).await;
        drop(switching);
        if let Err(error) = first {
            return Finished::Failed { error, lifted: false };
        }
        // What ran before is gone: what it held kept or let go now (not
        // before: a first load that fails leaves it standing, still held),
        // what the load under it held let go, and loaded again as itself
        // (or under its own preview).
        held(host);
        set(self.going.clone());
        host.drop_held_state();
        self.settled();
        let finished = match host.reload() {
            Ok(()) if relaid => Finished::Settled,
            Ok(()) => Finished::Failed { error: "the template's files weren't laid again: Zeron's own loaded".into(), lifted: true },
            Err(error) => Finished::Failed { error, lifted: true },
        };
        owe(finished != Finished::Settled);
        finished
    }
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

/// `refuse`, for an action with no promise to reject: the same words, as one
/// of the host's toasts. Whether it refused.
pub fn refuse_aloud(action: &str) -> bool {
    let Err(why) = refuse(action) else { return false };
    super::host_bar::notice(super::host_bar::Notice { title: why, body: String::new(), tone: super::host_bar::Tone::Neutral, action: None, stays: false });
    true
}

/// Actions the guard refuses itself, rather than each with a friendly answer
/// of its own: a plugin's own (`plugin.<id>.…`, its programs among them), run
/// as that plugin. Only the app's VM is asked, a plugin's isn't, and Zeron's
/// own screens call none of them.
const REFUSED_BY_GUARD: &[&str] = &["plugin."];

/// The words every refusal shares (`refused` knows one by them).
const IS_A_PREVIEW: &str = " is a preview";

/// Whether `error` is a preview's refusal, not something wrong with the
/// template: its script asked for what it may not have yet.
pub fn refused(error: &str) -> bool {
    current().is_some() && error.contains(IS_A_PREVIEW)
}

/// Whether the app's VM may reach `name` now, as zgx asks before it leaves
/// the interface (a plugin's VM isn't asked). While a template is a preview:
/// no program, no plugin's powers, no page, no request or remote picture (a
/// URL carries what it read), no link, and what it saves is held. Everything
/// else, and everything once it's kept, is as the app's. Most blocked actions
/// answer for themselves (`refuse`), with a promise that rejects.
pub fn guard(kind: zgx_script::GuardKind, name: &str) -> Result<(), String> {
    use zgx_script::GuardKind as Kind;
    let Some(p) = current() else { return Ok(()) };
    let refused = || Err(format!("{}{IS_A_PREVIEW}. Keep it first, or go back to Zeron v3.", p.name));
    match kind {
        Kind::Clipboard | Kind::Dialog | Kind::File | Kind::Folder | Kind::Notification | Kind::Window => Ok(()),
        Kind::Action | Kind::AsyncAction | Kind::Function => match REFUSED_BY_GUARD.iter().any(|b| name.starts_with(b)) {
            true => refused(),
            false => Ok(()),
        },
        // Showing a terminal you already have runs nothing new: only what
        // the host itself started (`terminal.command`, refused here).
        Kind::Term if crate::terminal::issued(name) => Ok(()),
        Kind::OpenLink | Kind::Web => {
            super::host_bar::links_need_keeping();
            refused()
        }
        // (Any kind zgx adds is refused until it's looked at.)
        _ => refused(),
    }
}

// ── Approvals ───────────────────────────────────────────────────────────────

/// What a template lays over Zeron's own, as one hash: each file it supplies
/// (by its path, its length and its bytes), as `live::compose` takes them.
pub fn digest(template: &Path) -> String {
    let mut h = Sha256::new();
    for (rel, path) in live::files(template, |rel| !live::template_meta(rel) && !live::never_templated(rel)) {
        // (One that can't be read isn't counted: the copy leaves it out too,
        // so what's kept and what's laid are of the same files.)
        let Ok(bytes) = std::fs::read(&path) else { continue };
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

/// Template `id` as its copy `copy` holds it (`live::snapshot`, what's then
/// laid): whether it's Zeron's official one exactly as this build wrote it
/// (only a `plain` copy: a folder with a link or the like never is), and its
/// digest, by which what you kept is asked (`kept`). What's judged is what's
/// laid: no edit of the folder comes between.
pub fn judged(id: &str, copy: &Path, plain: bool) -> (bool, String) {
    // (Each copy a path of its own: asked straight, never cached by path.)
    let official = id != live::BUILTIN && plain && super::official::verified_at(&crate::ui_dir(), copy).is_some();
    (official, digest(copy))
}

/// Where how what's laid was judged is kept (`live::Judged`): beside what
/// you kept, out of the templates' folder (which agents write to, and whose
/// changes wake a look).
pub fn judged_record() -> Option<PathBuf> {
    approvals_path().map(|p| p.with_file_name("laid-judged.json"))
}

/// Whether you kept template `id` as it was when its digest was `digest`.
pub fn kept(id: &str, digest: &str) -> bool {
    approvals_path().is_some_and(|p| approved_at(&p, id, digest))
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
        let key = if cfg!(target_os = "macos") { "⌥ held at launch" } else { "Alt held at launch" };
        eprintln!("zeron-zgx: safe mode ({})", if held { key } else { "the template crashed Zeron twice" });
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

/// Alt held as Zeron opens (either one), or `ZERON_SAFE_MODE` set.
/// (`--safe-mode` is the same, asked on the command line: src/main.rs.)
#[cfg(windows)]
fn option_held() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(key: i32) -> i16;
    }
    const VK_MENU: i32 = 0x12;
    // SAFETY: a plain query of a key's state; the high bit is "down now".
    let down = unsafe { GetAsyncKeyState(VK_MENU) } < 0;
    down || std::env::var_os("ZERON_SAFE_MODE").is_some()
}

/// Alt held as Zeron opens, as the X server has the keyboard (Linux on X11,
/// libX11 opened as it's needed: nothing links it), or `ZERON_SAFE_MODE`
/// set. On Wayland no app reads the keyboard before it has a window, so
/// there it's `--safe-mode`, or Ctrl+Alt+Shift+Home once the window is up
/// (src/menu_bar.rs `DEFAULT_INTERFACE_KEY`).
#[cfg(all(unix, not(target_os = "macos")))]
fn option_held() -> bool {
    std::env::var_os("ZERON_SAFE_MODE").is_some() || x11_alt_held()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn x11_alt_held() -> bool {
    use std::ffi::{c_char, c_int, c_ulong, c_void};
    // The X server's keyboard isn't ours to read in a Wayland session
    // (XWayland's says only what its own windows hear): nothing held.
    if std::env::var_os("DISPLAY").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return false;
    }
    type Open = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
    type Keymap = unsafe extern "C" fn(*mut c_void, *mut c_char) -> c_int;
    type Keycode = unsafe extern "C" fn(*mut c_void, c_ulong) -> u8;
    // SAFETY: libX11's own functions, by their C signatures, on a display
    // opened and closed here; the keymap is the 32 bytes XQueryKeymap fills.
    unsafe {
        let lib = libc::dlopen(c"libX11.so.6".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if lib.is_null() {
            return false;
        }
        let symbol = |name: &std::ffi::CStr| libc::dlsym(lib, name.as_ptr());
        let (open, close, keymap, keycode) =
            (symbol(c"XOpenDisplay"), symbol(c"XCloseDisplay"), symbol(c"XQueryKeymap"), symbol(c"XKeysymToKeycode"));
        let held = if [open, close, keymap, keycode].iter().any(|s| s.is_null()) {
            false
        } else {
            let (open, close, keymap, keycode): (Open, Close, Keymap, Keycode) = (
                std::mem::transmute::<*mut c_void, Open>(open),
                std::mem::transmute::<*mut c_void, Close>(close),
                std::mem::transmute::<*mut c_void, Keymap>(keymap),
                std::mem::transmute::<*mut c_void, Keycode>(keycode),
            );
            let display = open(std::ptr::null());
            if display.is_null() {
                false
            } else {
                let mut keys = [0 as c_char; 32];
                keymap(display, keys.as_mut_ptr());
                // XK_Alt_L, XK_Alt_R, and XK_ISO_Level3_Shift (AltGr) left out.
                let down = [0xffe9_u64, 0xffea].iter().any(|&sym| {
                    let code = keycode(display, sym as c_ulong) as usize;
                    code != 0 && (keys[code / 8] as u8) & (1 << (code % 8)) != 0
                });
                close(display);
                down
            }
        };
        libc::dlclose(lib);
        held
    }
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

    /// The window's app file as a preview has it: a timer that asks for
    /// `plugin.x.run` on each turn, a window of its own whose timer asks
    /// too, a call still out whose answer asks again, its cleanup asking as
    /// it goes, and an effect and a binding that ask as a named state the
    /// next load sets moves.
    const ASKING: &str = r#"ui.every(16, function()
	ui.invoke("turn", 1)
	pcall(ui.invoke, "plugin.x.run", 1)
end)
local function own()
	ui.every(16, function()
		ui.invoke("window_turn", 1)
		pcall(ui.invoke, "plugin.x.run", 1)
	end)
	return ui.text { "its own" }
end
ui.window { id = "own", open = ui.state("own_open", true), title = "Own", own() }
ui.call("slow", 1):after(function()
	ui.invoke("answered_late", 1)
	pcall(ui.invoke, "plugin.x.run", 1)
end)
ui.on_cleanup(function()
	pcall(ui.invoke, "plugin.x.run", 1)
end)
local shared = ui.state("shared_n", 0)
ui.effect(function()
	if shared:get() > 0 then
		ui.invoke("old_followed", 1)
		pcall(ui.invoke, "plugin.x.run", 1)
	end
end)
return ui.column {
	ui.text { "asking" },
	ui.text { function()
		if shared:get() > 0 then
			pcall(ui.invoke, "plugin.x.run", 1)
		end
		return "bound " .. shared:get()
	end },
}"#;
    /// What comes next with an app of its own (a template's): it counts its
    /// loads, moves the named state the preview follows to its load's
    /// number, keeps a key, and asks for `plugin.x.load` as it loads.
    const NEXT_OWN: &str = r#"ui.invoke("loaded", 1)
local loads = ui.state("next_loads", 0)
loads:set(loads:peek() + 1)
ui.state("shared_n", 0):set(loads:peek())
local width = ui.persist("width", 1)
ui.state("width_read", 0):set(width:peek())
pcall(ui.invoke, "plugin.x.load", 1)
ui.command { id = "widen", title = "Widen", run = function() width:set(width:peek() + 1) end }
return ui.column { ui.text { "next" } }"#;
    /// What comes next with Zeron's own app, which loads nothing while the
    /// window changes hands (ui/app.luau's first lines).
    const NEXT: &str = concat!(
        r#"local asked, switching = pcall(ui.call_sync, "window.switching", nil)
if asked and switching == true then return ui.column {} end
"#,
        r#"ui.invoke("loaded", 1)
local loads = ui.state("next_loads", 0)
loads:set(loads:peek() + 1)
ui.state("shared_n", 0):set(loads:peek())
local width = ui.persist("width", 1)
ui.state("width_read", 0):set(width:peek())
pcall(ui.invoke, "plugin.x.load", 1)
ui.command { id = "widen", title = "Widen", run = function() width:set(width:peek() + 1) end }
return ui.column { ui.text { "next" } }"#
    );
    /// A template's own, failing after it moved the state.
    const NEXT_FAILING: &str = r#"ui.state("shared_n", 0):set(7)
error("fails as it loads")"#;
    /// Zeron's own, for these tests: asks nothing.
    const QUIET: &str = r#"return ui.column { ui.text { "quiet" } }"#;
    /// One that doesn't load.
    const BROKEN: &str = "return ui.column {";

    /// A future that's ready: its value.
    fn now<T>(f: impl std::future::Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        match f.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => panic!("not ready"),
        }
    }

    /// A window on `app.luau`, counting what its scripts did.
    struct Window {
        app: zgx_script::testing::TestApp,
        file: PathBuf,
        counts: std::rc::Rc<std::cell::RefCell<std::collections::BTreeMap<&'static str, u32>>>,
        slow: std::rc::Rc<std::cell::RefCell<Vec<zgx::task::LatestSender<()>>>>,
        laid: std::rc::Rc<std::cell::RefCell<Vec<Lay>>>,
    }

    impl Window {
        fn new(name: &str) -> Self {
            let at = dir(name);
            let file = at.join("app.luau");
            std::fs::write(&file, ASKING).unwrap();
            let counts: std::rc::Rc<std::cell::RefCell<std::collections::BTreeMap<&'static str, u32>>> = Default::default();
            let slow: std::rc::Rc<std::cell::RefCell<Vec<zgx::task::LatestSender<()>>>> = Default::default();
            let (c, w) = (counts.clone(), slow.clone());
            let (entry, storage) = (file.clone(), at.join("storage"));
            let app = zgx_script::testing::TestApp::new(move |runtime| {
                let host = zgx_script::ScriptHost::new(runtime, &entry).storage(storage);
                let refused = c.clone();
                host.guard(move |kind, name| {
                    let said = guard(kind, name);
                    if said.is_err() {
                        *refused.borrow_mut().entry("refused").or_default() += 1;
                    }
                    said
                });
                host.function(SWITCHING, |_| zgx_script::Value::Bool(switching()));
                for name in ["turn", "window_turn", "answered_late", "old_followed", "loaded", "plugin.x.run", "plugin.x.load"] {
                    let c = c.clone();
                    host.action(name, move |_| *c.borrow_mut().entry(name).or_default() += 1);
                }
                let w = w.clone();
                host.async_action("slow", move |_| {
                    let (tell, mut heard) = zgx::task::latest::<()>();
                    w.borrow_mut().push(tell);
                    async move {
                        heard.recv().await;
                        Ok(zgx_script::Value::Nil)
                    }
                });
                host
            });
            Self { app, file, counts, slow, laid: Default::default() }
        }

        fn count(&self, name: &str) -> u32 {
            self.counts.borrow().get(name).copied().unwrap_or(0)
        }

        /// A few of its turns: what they asked and had answered.
        fn run(&self) -> (u32, u32) {
            let (turns, answered) = (self.count("turn") + self.count("window_turn"), self.count("plugin.x.run"));
            self.app.frames(6);
            (self.count("turn") + self.count("window_turn") - turns, self.count("plugin.x.run") - answered)
        }

        /// `code` laid as its app file.
        fn lay(&self, code: &str) {
            std::fs::write(&self.file, code).unwrap();
        }

        /// A switch finished: `next` what comes next, `QUIET` Zeron's own.
        fn finish(&self, switch: Switch, laid: bool, asked: bool, next: &'static str, held: impl FnOnce(&zgx_script::ScriptHost)) -> Finished {
            let (file, record, watch) = (self.file.clone(), self.laid.clone(), self.app.host.clone());
            // (What comes next has an app of its own unless it's Zeron's.)
            let own_app = !next.starts_with("local asked, switching = pcall(ui.call_sync") && next != QUIET;
            let finished = now(switch.finish(laid, asked, own_app, &self.app.host, held, move |which| {
                record.borrow_mut().push(which);
                std::fs::write(&file, if which == Lay::ZeronsOwn { QUIET } else { next }).unwrap();
                // (The host's watch of what's laid, as each wait ends.)
                watch.reload_if_changed();
                std::future::ready(true)
            }));
            assert!(!self.switching(), "not left changing hands: {finished:?}");
            finished
        }

        fn state(&self, name: &str) -> String {
            self.app.host.states().into_iter().find(|(n, _)| n.ends_with(name)).map(|(_, v)| v.to_json()).unwrap_or_default()
        }

        /// Whether the host says the window is changing hands.
        fn switching(&self) -> bool {
            switching()
        }

        /// What `NEXT` keeps: widened once, saved, and read back by a load.
        fn keeps_its_width(&self) -> bool {
            let before: u32 = self.state("width_read").parse().unwrap_or(0);
            assert!(self.app.host.run_command("widen"));
            self.app.frames(2);
            self.app.host.save();
            self.app.host.reload().unwrap();
            self.app.frames(2);
            self.state("width_read").parse::<u32>().unwrap_or(0) == before + 1
        }

        /// `preview_is_gone`, but where what comes next is a preview too
        /// (`still`): its asks refused still, whatever runs.
        fn preview_is_gone_unless(&self, still: bool) {
            if still {
                let answered = self.count("plugin.x.run");
                self.app.frames(6);
                assert_eq!(self.count("plugin.x.run"), answered);
            } else {
                self.preview_is_gone();
            }
        }

        /// Nothing of the preview runs or is answered from here on: its
        /// timers, its window's, its call's late answer.
        fn preview_is_gone(&self) {
            for tell in self.slow.borrow_mut().drain(..) {
                tell.send(());
            }
            let turns = self.count("turn") + self.count("window_turn");
            self.app.frames(6);
            assert_eq!(self.count("turn") + self.count("window_turn"), turns, "nothing of the preview runs once it's replaced");
            assert_eq!(self.count("answered_late"), 0, "nor does its call's answer reach it");
        }
    }

    fn named(id: &str) -> Preview {
        Preview { id: id.into(), name: id.into() }
    }

    const DROP: fn(&zgx_script::ScriptHost) = |h| h.drop_held_state();
    const KEEP: fn(&zgx_script::ScriptHost) = |h| drop(h.keep_held_state());

    /// Switches from a preview, as `looked` makes each: Revert (Zeron's own,
    /// chosen in safe mode) and an agent's switch to Zeron's own are the
    /// same one; a look again (a retry, or one asked while the last laid
    /// files) begins again from what stands, and reloads whether or not it
    /// laid anything new.
    #[test]
    fn a_template_switch_settles_in_order() {
        let mut window = Window::new("switch-order");
        let failed = |f: &Finished, was: bool| matches!(f, Finished::Failed { lifted, .. } if *lifted == was);
        set(Some(named("a")));
        let (turns, answered) = window.run();
        assert!(turns > 0 && answered == 0, "a preview's: {turns} turns, {answered} answered");
        assert!(window.count("window_turn") > 0 && window.slow.borrow().len() == 1, "its window's timer runs, and its call is out");

        // Revert, its files not laid.
        let switch = Switch::begin(None, false);
        assert_eq!(window.run().1, 0, "while the files are laid");
        assert!(switch.holds());
        assert_eq!(window.finish(switch, false, false, QUIET, DROP), Finished::NotLaid);
        assert_eq!((window.run().1, current()), (0, Some(named("a"))));

        // An agent's switch to Zeron's own, its load failing: what ran
        // before stays mounted, runs, and is still the preview.
        window.lay(BROKEN);
        let f = window.finish(Switch::begin(None, false), true, true, QUIET, DROP);
        assert!(failed(&f, false), "{f:?}");
        let (turns, answered) = window.run();
        assert!(turns > 0 && answered == 0, "still mounted: {turns} turns, {answered} answered");
        assert_eq!(current(), Some(named("a")));

        // A look again that finds nothing new to lay still reloads.
        let f = window.finish(Switch::begin(None, false), true, false, QUIET, DROP);
        assert!(failed(&f, false), "a switch from a preview always reloads: {f:?}");
        assert_eq!((window.run().1, current()), (0, Some(named("a"))));

        // To another preview, its load failing: the one before stands.
        let f = window.finish(Switch::begin(Some(named("b")), true), true, true, QUIET, DROP);
        assert!(failed(&f, false), "{f:?}");
        assert_eq!((window.run().1, current()), (0, Some(named("a"))));

        // Revert that settles: a load under the preview takes it away
        // (Zeron's own app loads nothing then), then Zeron's own loads as
        // itself: what it asks as it loads answered, nothing of it refused,
        // what it keeps kept. The next frame is Zeron's own.
        window.lay(NEXT);
        window.app.frames(1);
        assert!(window.app.tree().contains("\"asking\"") && !window.app.tree().contains("\"next\""));
        let (loads, refused) = (window.count("loaded"), window.count("refused"));
        assert_eq!(window.finish(Switch::begin(None, false), true, true, QUIET, DROP), Finished::Settled);
        assert_eq!(current(), None);
        assert_eq!(window.count("loaded"), loads + 1, "Zeron's own ran once: its load under the preview loaded nothing");
        assert_eq!(window.count("refused"), refused, "nothing of it refused, so nothing said of it");
        assert_eq!(window.count("plugin.x.load"), 1, "answered as itself");
        assert_eq!((window.count("old_followed"), window.count("plugin.x.run")), (0, 0), "the preview's effects had nothing to follow, nor anything answered");
        window.app.frames(1);
        assert!(window.app.tree().contains("\"next\"") && !window.app.tree().contains("\"asking\""), "the next frame is Zeron's own");
        let refused = window.count("refused");
        window.preview_is_gone();
        assert_eq!(window.count("refused"), refused, "Zeron's own is refused nothing");
        assert!(window.keeps_its_width(), "what Zeron's own keeps is kept: {}", window.state("width_read"));
        assert!(window.laid.borrow().is_empty(), "laid nothing else");

        // From Zeron's own to a preview: one load, under its own.
        window.lay(ASKING);
        let switch = Switch::begin(Some(named("c")), true);
        assert!(!switch.holds());
        assert_eq!(window.finish(switch, true, true, ASKING, DROP), Finished::Settled);
        assert_eq!(current(), Some(named("c")));
        let (turns, answered) = window.run();
        assert!(turns > 0 && answered == 0);

        // To another preview: under the one before, then its own; never none.
        window.lay(NEXT);
        let (loads, answers) = (window.count("loaded"), window.count("plugin.x.load"));
        assert_eq!(window.finish(Switch::begin(Some(named("d")), true), true, true, NEXT, DROP), Finished::Settled);
        assert_eq!((current(), window.count("loaded"), window.count("plugin.x.load")), (Some(named("d")), loads + 1, answers));

        // Kept, nothing new laid: reloaded, as itself, what it held kept.
        let switch = Switch::begin(None, true);
        assert!(switch.holds());
        assert_eq!(window.finish(switch, true, false, NEXT, KEEP), Finished::Settled);
        assert_eq!((current(), window.count("loaded"), window.count("plugin.x.load")), (None, loads + 2, answers + 1));
        assert!(window.keeps_its_width(), "what the kept one keeps is kept: {}", window.state("width_read"));
        assert_eq!(window.count("plugin.x.run"), 0, "no ask of a preview's was ever answered");
        set(None);
    }

    /// A preview's effect and binding following a named state that what
    /// loads next moves (an app of its own): Zeron's own loads under the
    /// preview first and moves nothing, then what comes next loads (the
    /// state moved, to 1), as itself or under its own preview, with nothing
    /// of the one before alive to follow it.
    #[test]
    fn a_preview_s_effects_on_what_the_next_load_sets_stay_gated() {
        for (going, held) in [(Some(named("b")), DROP), (None, KEEP), (None, DROP)] {
            let window = Window::new("follows");
            set(Some(named("a")));
            window.app.frames(4);
            window.lay(NEXT_OWN);
            assert_eq!(window.finish(Switch::begin(going.clone(), true), true, true, NEXT_OWN, held), Finished::Settled);
            assert_eq!(window.count("plugin.x.run"), 0, "{going:?}: nothing it asked was answered");
            assert_eq!(window.count("old_followed"), 0, "{going:?}: Zeron's own, under the preview, moved nothing");
            assert_eq!(window.laid.borrow().clone(), vec![Lay::ZeronsOwn, Lay::Next]);
            // (As itself, its ask answered once; under the next preview, refused.)
            assert_eq!(window.count("plugin.x.load"), if going.is_some() { 0 } else { 1 }, "{going:?}");
            window.preview_is_gone_unless(going.is_some());
            assert_eq!(window.count("plugin.x.run"), 0);
            set(None);
        }
    }

    /// Zeron's own app (ui/app.luau), loaded while the window changes hands
    /// under a preview: nothing built, nothing asked, nothing refused.
    #[test]
    fn zeron_s_own_app_loads_nothing_while_the_window_changes_hands() {
        let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/app.luau");
        let asked: std::rc::Rc<std::cell::Cell<u32>> = Default::default();
        let a = asked.clone();
        set(Some(named("a")));
        switching_now(true);
        let mut app = zgx_script::testing::TestApp::new(move |runtime| {
            let host = zgx_script::ScriptHost::new(runtime, &entry);
            host.guard(move |kind, name| {
                // (Its one ask: whether the window changes hands.)
                if name != SWITCHING {
                    a.set(a.get() + 1);
                }
                guard(kind, name)
            });
            host.shared("window", "main");
            host.function(SWITCHING, |_| zgx_script::Value::Bool(switching()));
            host
        });
        app.frames(4);
        switching_now(false);
        assert_eq!(app.error(), None);
        assert_eq!(asked.get(), 0, "asked for nothing");
        assert!(!app.tree().contains("\"text\""), "built nothing: {}", app.tree());
        set(None);
    }

    /// What a preview saves as it goes, out of a switch to another preview
    /// (its effect following a state the next one's load moves), isn't
    /// kept with that one: kept after, nothing of the first is saved.
    #[test]
    fn what_a_preview_saves_as_it_goes_isn_t_kept_with_the_next() {
        const SAVES_AS_IT_GOES: &str = r#"local leak = ui.persist("leak", "none")
local shared = ui.state("shared_n", 0)
ui.effect(function()
	if shared:get() > 0 then
		leak:set("from the first preview")
	end
end)
return ui.column { ui.text { "first" } }"#;
        let window = Window::new("saves-as-it-goes");
        set(Some(named("a")));
        window.lay(SAVES_AS_IT_GOES);
        window.app.host.reload().unwrap();
        window.app.frames(2);
        window.lay(NEXT_OWN);
        assert_eq!(window.finish(Switch::begin(Some(named("b")), true), true, true, NEXT_OWN, DROP), Finished::Settled);
        assert_eq!(window.finish(Switch::begin(None, true), true, false, NEXT_OWN, KEEP), Finished::Settled);
        window.app.host.save();
        let storage = window.file.parent().unwrap().join("storage");
        let saved = files_under(&storage).iter().any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains("from the first preview")));
        assert!(!saved, "kept with the next");
        set(None);
    }

    /// A script that sets a shared `window.switching` (a template kept, or
    /// a preview) says nothing to the host: Zeron's own, switched to from it
    /// (safe mode, none before), loads whole.
    #[test]
    fn a_script_s_word_on_switching_changes_nothing() {
        const SAYS_SWITCHING: &str = r#"ui.shared("window.switching", true):set(true)
return ui.column { ui.text { "says switching" } }"#;
        let mut window = Window::new("says-switching");
        set(None);
        window.lay(SAYS_SWITCHING);
        window.app.host.reload().unwrap();
        window.app.frames(2);
        window.lay(NEXT);
        assert_eq!(window.finish(Switch::begin(None, false), true, true, QUIET, DROP), Finished::Settled);
        window.app.frames(1);
        assert_eq!(window.count("loaded"), 1);
        assert!(window.app.tree().contains("\"next\""), "{}", window.app.tree());
    }

    /// What a script laid for a switch asks, as the host's watch loads its
    /// files while they land, after a load fails, after a lay-down that
    /// didn't finish, and on a plugin's reload: the window is the stricter
    /// of what shows and what's laid, so what a preview's files ask is
    /// refused throughout, in every pair; trusted to trusted, nothing is.
    #[test]
    fn a_switch_s_files_are_gated_as_they_land() {
        const LANDING: &str = concat!(
            r#"local asked, switching = pcall(ui.call_sync, "window.switching", nil)
if asked and switching == true then return ui.column {} end
"#,
            r#"pcall(ui.invoke, "plugin.x.run", 1)
ui.every(16, function() pcall(ui.invoke, "plugin.x.run", 1) end)
return ui.column { ui.text { "landing" } }"#
        );
        const LANDING_OWN: &str = r#"pcall(ui.invoke, "plugin.x.run", 1)
ui.every(16, function() pcall(ui.invoke, "plugin.x.run", 1) end)
return ui.column { ui.text { "landing, its own app" } }"#;
        let window = Window::new("landing");
        let answered = || window.count("plugin.x.run");
        let watched = |code: &str| {
            window.lay(code);
            window.app.host.reload_if_changed();
            window.app.frames(4);
        };
        let plugin_reload = || {
            let _ = window.app.host.reload();
            window.app.frames(4);
        };
        let start = |code: &str, preview: Option<Preview>| {
            set(preview);
            window.lay(code);
            window.app.host.reload().unwrap();
            window.app.frames(2);
        };

        // From none to a preview: up before its files land, and kept up
        // when its load fails (edited, reloaded by a plugin), or its files
        // were laid only in part.
        start(QUIET, None);
        let n = answered();
        let switch = Switch::begin(Some(named("b")), true);
        watched(LANDING);
        assert_eq!(answered(), n, "none → preview: its files as they land");
        assert_eq!(window.finish(switch, true, true, LANDING, DROP), Finished::Settled);
        window.app.frames(4);
        assert_eq!(answered(), n);
        start(QUIET, None);
        let switch = Switch::begin(Some(named("b")), true);
        window.lay(BROKEN);
        assert!(matches!(window.finish(switch, true, true, BROKEN, DROP), Finished::Failed { lifted: false, .. }));
        assert_eq!(current(), Some(named("b")), "its files are what a next load loads");
        watched(LANDING);
        plugin_reload();
        assert_eq!(answered(), n, "none → preview, failed: edited, and a plugin's reload");
        start(QUIET, None);
        let switch = Switch::begin(Some(named("b")), true);
        watched(LANDING);
        assert_eq!(window.finish(switch, false, true, LANDING, DROP), Finished::NotLaid);
        watched(LANDING);
        plugin_reload();
        assert_eq!((answered(), current()), (n, Some(named("b"))), "none → preview, laid in part");

        // Preview to preview: the one before stands until its load.
        start(QUIET, Some(named("a")));
        let switch = Switch::begin(Some(named("b")), true);
        watched(LANDING);
        assert_eq!(answered(), n, "preview → preview: as its files land");
        assert_eq!(window.finish(switch, true, true, LANDING, DROP), Finished::Settled);
        plugin_reload();
        assert_eq!((answered(), current()), (n, Some(named("b"))));

        // From a preview to Zeron's own, and to one kept with its own app
        // (the waits between watched): refused till the switch settles.
        for (code, template) in [(LANDING, false), (LANDING_OWN, true)] {
            start(QUIET, Some(named("a")));
            let n = answered();
            let switch = Switch::begin(None, template);
            watched(code);
            assert_eq!(answered(), n, "preview → {template}: as its files land");
            assert_eq!(window.finish(switch, true, true, code, KEEP), Finished::Settled);
            assert_eq!(current(), None);
            let before = answered();
            window.app.frames(4);
            assert!(answered() > before, "what comes next, as itself, is answered");
            let _ = answered();
        }

        // Trusted to trusted: nothing refused.
        start(QUIET, None);
        let _ = n;
        let switch = Switch::begin(None, false);
        let before = answered();
        watched(LANDING);
        assert!(answered() > before, "trusted → trusted");
        assert_eq!(window.finish(switch, true, true, LANDING, DROP), Finished::Settled);
        set(None);
    }

    /// A load that moves the states a preview's effects follow, then fails
    /// (an app of its own), kept or to another preview: Zeron's own loads
    /// under the one before first and moves nothing, and the failure after
    /// is what comes next's own, a load owed, its preview (if one) standing.
    #[test]
    fn a_load_that_moves_states_then_fails_leaves_the_preview_gated() {
        for kept in [false, true] {
            let window = Window::new(&format!("moves-then-fails-{kept}"));
            set(Some(named("a")));
            window.app.frames(4);
            window.lay(NEXT_FAILING);
            let going = (!kept).then(|| named("b"));
            let going_was = going.clone();
            let f = window.finish(Switch::begin(going, true), true, true, NEXT_FAILING, if kept { KEEP } else { DROP });
            assert_eq!(window.count("plugin.x.run"), 0, "kept {kept}: nothing asked was answered");
            assert!(matches!(f, Finished::Failed { lifted: true, .. }), "{f:?}");
            assert_eq!((current(), window.count("old_followed"), window.laid.borrow().clone()), (going_was, 0, vec![Lay::ZeronsOwn, Lay::Next]));
            assert!(reload_owed());
            window.preview_is_gone();
            set(None);
        }
    }

    /// A template with an app of its own that asks for what a preview may
    /// not as it loads, its refusal not caught: kept, Zeron's own loads
    /// under the preview first (asking nothing), then the kept one as
    /// itself: it runs, and isn't failed. Switched to as another preview, its
    /// load under that preview fails, a load owed.
    #[test]
    fn a_kept_template_asking_as_it_loads_runs() {
        const ASKS_AT_LOAD: &str = r#"ui.invoke("plugin.x.run", 1)
return ui.column { ui.text { "asks at load" } }"#;
        let window = Window::new("asks-at-load");
        set(Some(named("a")));
        window.app.frames(2);
        window.lay(ASKS_AT_LOAD);
        let f = window.finish(Switch::begin(Some(named("b")), true), true, true, ASKS_AT_LOAD, DROP);
        assert!(matches!(f, Finished::Failed { lifted: true, .. }), "{f:?}");
        assert_eq!(current(), Some(named("b")), "its load, under its own preview, fails");
        window.laid.borrow_mut().clear();
        assert_eq!(window.finish(Switch::begin(None, true), true, true, ASKS_AT_LOAD, KEEP), Finished::Settled);
        assert_eq!((current(), window.count("plugin.x.run"), window.laid.borrow().clone()), (None, 1, vec![Lay::ZeronsOwn, Lay::Next]));
        set(None);
    }

    /// The load as itself failing (here: a script that fails once what it
    /// asks for is answered): none stands, what loaded under the preview
    /// shows, to be looked at again.
    #[test]
    fn a_second_load_that_fails_is_said() {
        const FAILS_AS_ITSELF: &str = r#"if pcall(ui.invoke, "plugin.x.load", 1) then error("fails as itself") end
return ui.column { ui.text { "loaded under the preview" } }"#;
        let mut window = Window::new("second-fails");
        set(Some(named("a")));
        window.app.frames(2);
        window.lay(FAILS_AS_ITSELF);
        let f = window.finish(Switch::begin(None, false), true, true, QUIET, DROP);
        assert!(matches!(&f, Finished::Failed { lifted: true, error } if error.contains("fails as itself")), "{f:?}");
        assert_eq!(current(), None);
        window.app.frames(1);
        assert!(window.app.tree().contains("loaded under the preview"));
        window.preview_is_gone();
        // A load is owed: the next look's switch reloads though it laid
        // nothing new, and once one is Ok nothing more is owed.
        assert!(reload_owed());
        window.lay(NEXT);
        assert_eq!(window.finish(Switch::begin(None, false), true, false, QUIET, DROP), Finished::Settled);
        window.app.frames(1);
        assert!(window.app.tree().contains("\"next\"") && !reload_owed(), "{}", window.app.tree());
        set(None);
    }

    /// A future run to its end on this thread (a lay-down's thread wakes it).
    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        struct Unpark(std::thread::Thread);
        impl std::task::Wake for Unpark {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
        let mut cx = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let std::task::Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::park_timeout(std::time::Duration::from_millis(20));
        }
    }

    /// Zeron's own app, for the lay-down tests: asks nothing.
    const ZERONS_APP: &str = concat!(
        r#"local asked, switching = pcall(ui.call_sync, "window.switching", nil)
if asked and switching == true then return ui.column {} end
"#,
        r#"return ui.column { ui.text { "zeron's own" } }"#
    );
    /// A template's app that asks for what a preview may not, as it loads and
    /// on a timer.
    const ITS_APP: &str = r#"pcall(ui.invoke, "plugin.x.run", 1)
ui.every(16, function() pcall(ui.invoke, "plugin.x.run", 1) end)
return ui.column { ui.text { "its app" } }"#;

    /// Templates under a folder of their own, Zeron's laid; a window on its
    /// `.live`, counting `plugin.x.run` answered.
    struct Laying {
        root: PathBuf,
        laid: std::cell::RefCell<live::Laid>,
        trust: std::cell::RefCell<Option<live::Judged>>,
        overlay: std::cell::RefCell<live::Overlay>,
        app: zgx_script::testing::TestApp,
        answered: std::rc::Rc<std::cell::Cell<u32>>,
    }

    impl Laying {
        fn new(name: &str) -> Self {
            let root = dir(name).join("templates");
            write(&root.join(live::ZERON), "app.luau", ZERONS_APP);
            write(&root.join(live::ZERON), "screens/shell/sidebar.luau", "return {}");
            let (laid, trust, overlay) = (std::cell::RefCell::default(), std::cell::RefCell::default(), std::cell::RefCell::default());
            block_on(super::super::lay_down(&root, live::BUILTIN.into(), None, std::collections::BTreeSet::new(), false, Self::laid_as(live::BUILTIN), None, &laid, &trust, &overlay)).unwrap();
            let entry = root.join(live::LIVE).join("app.luau");
            let answered = std::rc::Rc::new(std::cell::Cell::new(0));
            let a = answered.clone();
            let app = zgx_script::testing::TestApp::new(move |runtime| {
                let host = zgx_script::ScriptHost::new(runtime, &entry);
                host.guard(guard);
                host.function(SWITCHING, |_| zgx_script::Value::Bool(switching()));
                host.action("plugin.x.run", move |_| a.set(a.get() + 1));
                host.action("plugin.k.run", |_| {});
                host
            });
            Self { root, laid, trust, overlay, app, answered }
        }

        fn laid_as(id: &str) -> live::Laid {
            live::Laid { id: id.into(), ui_stamp: "ui".into(), ..Default::default() }
        }

        /// Template `id` laid, as a look lays it, from a copy taken now: Ok
        /// or not.
        fn lay(&self, id: &str) -> bool {
            let copy = (id != live::BUILTIN).then(|| live::snapshot(&self.root, id).unwrap().0);
            self.lay_from(id, copy)
        }

        /// Template `id` laid from `copy`.
        fn lay_from(&self, id: &str, copy: Option<PathBuf>) -> bool {
            block_on(super::super::lay_down(&self.root, id.into(), copy, std::collections::BTreeSet::new(), false, Self::laid_as(id), None, &self.laid, &self.trust, &self.overlay)).is_ok()
        }

        /// A template whose app is `ITS_APP` and, with `across`, a file
        /// whose path runs through one of Zeron's: its lay-down fails with
        /// its app already written.
        fn template(&self, id: &str, across: bool) {
            self.template_of(id, ITS_APP, across);
        }

        fn template_of(&self, id: &str, app: &str, across: bool) {
            write(&self.root.join(id), "zeron-template.json", &format!(r#"{{"id":"{id}","name":"{id}"}}"#));
            write(&self.root.join(id), "app.luau", app);
            if across {
                write(&self.root.join(id), "screens/shell/sidebar.luau/x.luau", "-- across Zeron's");
            }
        }

        fn live_app(&self) -> String {
            std::fs::read_to_string(self.root.join(live::LIVE).join("app.luau")).unwrap()
        }
    }

    /// A preview's lay-down failing partway (its app written, a file across
    /// one of Zeron's not), then Revert: `.live` was unknown, so the look
    /// lays Zeron's own again, and nothing of the preview's app is answered,
    /// through both loads and after. Through the real lay-down.
    #[test]
    fn a_lay_down_that_fails_partway_is_laid_again() {
        let mut w = Laying::new("partway");
        set(None);
        w.app.frames(2);
        w.template("b", true);
        let switch = Switch::begin(Some(named("b")), true);
        assert!(!w.lay("b"), "fails across Zeron's file");
        assert_eq!(*w.laid.borrow(), live::Laid::default(), "unknown, not Zeron's own");
        assert_eq!(w.live_app(), ITS_APP, "its app was written");
        assert_eq!(now(switch.finish(false, true, false, &w.app.host, DROP, |_| std::future::ready(false))), Finished::NotLaid);
        w.app.host.reload_if_changed();
        w.app.frames(4);
        assert_eq!(w.answered.get(), 0, "under its preview");

        // Revert: the look's want is Zeron's own, which isn't what's laid.
        let want = Laying::laid_as(live::BUILTIN);
        assert_ne!(*w.laid.borrow(), want, "so it lays it again");
        let switch = Switch::begin(None, false);
        assert!(w.lay(live::BUILTIN));
        assert_eq!(w.live_app(), ZERONS_APP);
        assert_eq!(now(switch.finish(true, true, false, &w.app.host, DROP, |_| std::future::ready(false))), Finished::Settled);
        w.app.frames(6);
        assert_eq!((current(), w.answered.get()), (None, 0), "nothing of the preview's answered");
        assert!(w.app.tree().contains("zeron's own"));
        set(None);
    }

    /// A kept template with an app of its own whose lay-down again (after
    /// Zeron's own under the preview) fails partway: `.live` unknown, a load
    /// owed, nothing of the preview before answered.
    #[test]
    fn a_kept_template_s_lay_down_again_failing_partway_is_owed() {
        let w = Laying::new("again-partway");
        w.template("a", false);
        assert!(w.lay("a"));
        set(Some(named("a")));
        w.app.host.reload().unwrap();
        w.app.frames(4);
        assert_eq!(w.answered.get(), 0);
        // (The kept one's own asks are its to make: another action.)
        const KEPTS_APP: &str = r#"pcall(ui.invoke, "plugin.k.run", 1)
return ui.column { ui.text { "kept" } }"#;
        w.template_of("k", KEPTS_APP, false);
        let switch = Switch::begin(None, true);
        assert!(w.lay("k"));
        w.template_of("k", KEPTS_APP, true);
        let lay = |which: Lay| {
            let id = if which == Lay::ZeronsOwn { live::BUILTIN } else { "k" };
            let ok = w.lay(id);
            w.app.host.reload_if_changed();
            std::future::ready(ok)
        };
        let f = now_or_block(switch.finish(true, true, true, &w.app.host, KEEP, lay));
        assert!(matches!(f, Finished::Failed { lifted: true, .. }), "{f:?}");
        assert_eq!(*w.laid.borrow(), live::Laid::default(), "unknown");
        assert!(reload_owed());
        w.app.frames(6);
        assert_eq!(w.answered.get(), 0, "nothing of the preview before answered");
        set(None);
    }

    /// Keep of a preview with an app of its own, nothing new to lay: what's
    /// laid again after Zeron's own is the copy it was judged by, never the
    /// folder as it is then (rewritten as the window turns to Zeron's own):
    /// nothing of the rewrite is laid or answered. A template is never laid
    /// without a copy.
    #[test]
    fn keep_lays_again_from_the_copy_it_judged() {
        const KEPTS_APP: &str = r#"pcall(ui.invoke, "plugin.k.run", 1)
return ui.column { ui.text { "kept" } }"#;
        let w = Laying::new("keep-copy");
        w.template_of("x", KEPTS_APP, false);
        assert!(w.lay("x"));
        set(Some(named("x")));
        w.app.host.reload().unwrap();
        w.app.frames(2);
        // Keep: copied (as `looked` does when a switch lays it again), judged,
        // laid from the copy.
        let (copy, _) = live::snapshot(&w.root, "x").unwrap();
        let switch = Switch::begin(None, true);
        assert!(w.lay_from("x", Some(copy.clone())));
        let lay = |which: Lay| {
            let ok = if which == Lay::ZeronsOwn {
                let ok = w.lay(live::BUILTIN);
                // (The folder rewritten as the window turns to Zeron's own.)
                write(&w.root.join("x"), "app.luau", ITS_APP);
                ok
            } else {
                w.lay_from("x", Some(copy.clone()))
            };
            w.app.host.reload_if_changed();
            std::future::ready(ok)
        };
        assert_eq!(now_or_block(switch.finish(true, true, true, &w.app.host, KEEP, lay)), Finished::Settled);
        w.app.frames(6);
        assert_eq!(w.live_app(), KEPTS_APP, "the copy, not the rewrite");
        assert_eq!((current(), w.answered.get()), (None, 0), "nothing of the rewrite answered");
        // And no copy, no lay-down of a template.
        assert!(!w.lay_from("x", None));
        set(None);
    }

    /// A look with nothing new to lay, a preview with an app of its own
    /// standing: it copies and lays again only when what's laid is approved
    /// (a Keep); else nothing, look after look.
    #[test]
    fn a_preview_left_as_it_is_isn_t_copied_again() {
        let overlay = live::Overlay { template: "x".into(), from_template: ["app.luau".to_string()].into() };
        let unapproved = live::Judged { id: "x".into(), official: false, digest: "not kept".into(), ..Default::default() };
        let official = live::Judged { id: "x".into(), official: true, digest: String::new(), ..Default::default() };
        assert!(!super::super::lays_again(true, &overlay, Some(&unapproved), "x"), "nothing changed: nothing laid");
        assert!(!super::super::lays_again(true, &overlay, None, "x"));
        assert!(super::super::lays_again(true, &overlay, Some(&official), "x"), "approved: laid again");
        assert!(!super::super::lays_again(false, &overlay, Some(&official), "x"), "no preview standing");
        let zerons_app = live::Overlay { template: "x".into(), from_template: ["screens/a.luau".to_string()].into() };
        assert!(!super::super::lays_again(true, &zerons_app, Some(&official), "x"), "Zeron's app: one lay-down");
    }

    /// A launch runs `.live` as it is only while the template's files there
    /// are as they were laid: one written in `.live` since (between launches,
    /// by anyone) differs from the record, so it's all laid again; laid again,
    /// it's as recorded. The record holds it, beside the approvals.
    #[test]
    fn a_file_written_in_live_since_it_was_laid_is_laid_again() {
        let w = Laying::new("as-laid");
        w.template("x", false);
        assert!(w.lay("x"));
        let overlay = w.overlay.borrow().clone();
        let laid = live::laid_digest(&w.root, &overlay);
        let record = dir("as-laid-record").join("laid-judged.json");
        live::Judged { id: "x".into(), official: false, digest: "d".into(), laid: laid.clone() }.write(&record, &w.root).unwrap();
        assert_eq!(live::Judged::read(&record, &w.root).map(|j| j.laid), Some(laid.clone()), "kept with the record");
        assert_eq!(live::Judged::read(&record, &dir("elsewhere")), None, "of these templates only");

        // Written in `.live` since: not as laid.
        write(&w.root.join(live::LIVE), "app.luau", "-- written in .live since");
        assert_ne!(live::laid_digest(&w.root, &overlay), laid, "so the launch lays it again");
        assert!(w.lay("x"));
        assert_eq!(live::laid_digest(&w.root, &overlay), laid, "laid again: as recorded");
    }

    /// `now`, for a finish whose lay closure is already ready.
    fn now_or_block<T>(f: impl std::future::Future<Output = T>) -> T {
        block_on(f)
    }

    /// An async call the preview's script makes as it's leaving (its effect
    /// following a value the host publishes in the switch's turn, or a state
    /// the first load moves) is refused at the call, as every refusing action
    /// decides (`every_refusal_is_decided_at_the_call`), though its future is
    /// polled after the switch: in every pair from a preview to what has its
    /// powers, nothing of it runs.
    #[test]
    fn a_call_made_as_a_preview_leaves_is_refused_at_the_call() {
        const LEAVING: &str = r#"local published = ui.shared("host.published", 0)
local moved = ui.state("moved_at_load", 0)
ui.effect(function()
	if published:get() > 0 or moved:get() > 0 then
		ui.call("probe.leave", 1)
	end
end)
return ui.column { ui.text { "leaving" } }"#;
        const MOVES_AT_LOAD: &str = r#"ui.state("moved_at_load", 0):set(1)
return ui.column { ui.text { "moves at load" } }"#;
        for (template, own_app, first_moves) in [(false, false, false), (true, false, false), (true, true, false), (false, false, true), (true, false, true)] {
            let file = dir("leaving").join("app.luau");
            std::fs::write(&file, LEAVING).unwrap();
            let (called, ran) = (std::rc::Rc::new(std::cell::Cell::new(0)), std::rc::Rc::new(std::cell::Cell::new(0)));
            let (c, r, entry) = (called.clone(), ran.clone(), file.clone());
            set(Some(named("a")));
            let app = zgx_script::testing::TestApp::new(move |runtime| {
                let host = zgx_script::ScriptHost::new(runtime, &entry);
                host.guard(guard);
                host.function(SWITCHING, |_| zgx_script::Value::Bool(switching()));
                // (As every refusing action is made: decided as it's called.)
                host.async_action("probe.leave", move |_| {
                    c.set(c.get() + 1);
                    let allowed = refuse("terminal.start");
                    let r = r.clone();
                    async move {
                        allowed?;
                        r.set(r.get() + 1);
                        Ok(zgx_script::Value::Nil)
                    }
                });
                host
            });
            app.frames(2);
            std::fs::write(&file, if first_moves { MOVES_AT_LOAD } else { QUIET }).unwrap();
            if !first_moves {
                app.host.shared("host.published", 0.0).set(zgx_script::Value::from(1.0));
            }
            let switch = Switch::begin(None, template);
            let f = now(switch.finish(true, true, own_app, &app.host, DROP, |_| std::future::ready(true)));
            assert_eq!(f, Finished::Settled);
            for _ in 0..10 {
                app.frames(2);
            }
            let case = format!("{template} {own_app} {first_moves}");
            assert_eq!(ran.get(), 0, "{case}: nothing of it ran (called {})", called.get());
            set(None);
        }
    }

    /// What's judged is what's laid: a template copied once (`live::snapshot`),
    /// its copy judged (kept by the copy's digest), and `.live` laid from the
    /// copy, so an edit of the folder after it's copied never lands under the
    /// trust given; a link in it is never followed out of it, and a folder
    /// with one is never judged official.
    #[test]
    fn what_s_judged_is_what_s_laid() {
        let root = dir("judged").join("templates");
        write(&root.join(live::ZERON), "app.luau", "return ui.column {}");
        write(&root.join("k"), "zeron-template.json", r#"{"id":"k","name":"k"}"#);
        write(&root.join("k"), "screens/x.luau", "-- as kept");
        let (copy, plain) = live::snapshot(&root, "k").unwrap();
        assert!(plain && copy.ends_with("k") && copy.starts_with(root.join(live::SNAPSHOTS)));
        let (official, digest) = judged("k", &copy, plain);
        assert!(!official);
        assert_eq!(digest, super::digest(&root.join("k")), "the copy's digest is the folder's as copied");

        // Edited after it was copied: what's laid is the copy.
        write(&root.join("k"), "screens/x.luau", "-- edited after");
        write(&root.join("k"), "screens/y.luau", "-- added after");
        live::compose_from(&root, "k", &copy, &std::collections::BTreeSet::new(), false).unwrap();
        let live_dir = root.join(live::LIVE);
        assert_eq!(std::fs::read_to_string(live_dir.join("screens/x.luau")).unwrap(), "-- as kept");
        assert!(!live_dir.join("screens/y.luau").exists());
        assert_ne!(super::digest(&root.join("k")), digest, "and the edit is another digest, not kept");

        // A link in the folder: left out, never followed; never official.
        #[cfg(unix)]
        {
            let outside = root.parent().unwrap().join("outside");
            write(&outside, "secret.luau", "-- outside the folder");
            std::os::unix::fs::symlink(&outside, root.join("k/screens/linked")).unwrap();
            let (copy, plain) = live::snapshot(&root, "k").unwrap();
            assert!(!plain, "a link was met");
            assert!(!copy.join("screens/linked").exists() && !copy.join("screens/linked/secret.luau").exists());
            assert!(!judged("k", &copy, plain).0);
        }
        // One copy at a time: the earlier ones go.
        assert_eq!(std::fs::read_dir(root.join(live::SNAPSHOTS)).unwrap().count(), 1);

        // A dot folder (`.git`) isn't copied, and such a copy isn't plain.
        write(&root.join("g"), "zeron-template.json", r#"{"id":"g","name":"g"}"#);
        write(&root.join("g"), ".git/HEAD", "ref: refs/heads/main");
        let (copy, plain) = live::snapshot(&root, "g").unwrap();
        assert!(!plain && !copy.join(".git").exists() && copy.join("zeron-template.json").is_file());

        // A copy gone before it's laid (another Zeron on this data folder):
        // nothing laid, never Zeron's own taken for the template's.
        std::fs::remove_dir_all(&copy).unwrap();
        assert!(live::compose_from(&root, "g", &copy, &std::collections::BTreeSet::new(), false).is_err());
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir).into_iter().flatten().flatten().flat_map(|e| if e.path().is_dir() { files_under(&e.path()) } else { vec![e.path()] }).collect()
    }

    /// What comes next after a switch from a preview (Zeron's own, or the
    /// one kept) keeps what it changes: shown, and saved.
    #[test]
    fn what_comes_after_a_switch_keeps_what_it_changes() {
        const SAVING: &str = r#"
local accent = ui.persist("accent", "default")
ui.command { id = "set", title = "Set", run = function() accent:set("changed") end }
return ui.column { ui.text { id = "accent", function() return accent:get() end } }
"#;
        for kept in [false, true] {
            let root = dir(&format!("after-switch-saves-{kept}"));
            let storage = root.join("state");
            let file = root.join("app.luau");
            std::fs::write(&file, r#"return ui.column { ui.text { "a preview" } }"#).unwrap();
            let (st, entry) = (storage.clone(), file.clone());
            set(Some(named("a")));
            let mut app = zgx_script::testing::TestApp::new(move |runtime| {
                let host = zgx_script::ScriptHost::new(runtime, &entry).storage(st);
                host.guard(guard);
                host
            });
            app.frames(4);
            let switch = Switch::begin(None, kept);
            std::fs::write(&file, SAVING).unwrap();
            let held = move |h: &zgx_script::ScriptHost| {
                if kept {
                    h.keep_held_state();
                } else {
                    h.drop_held_state();
                }
            };
            assert_eq!(now(switch.finish(true, true, false, &app.host, held, |_| std::future::ready(false))), Finished::Settled);
            app.frames(4);
            assert!(app.host.run_command("set"));
            app.frames(4);
            app.host.save();
            assert_eq!(app.text("accent"), "changed", "kept {kept}");
            let saved = files_under(&storage).iter().any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains("changed")));
            assert!(saved, "kept {kept}: saved to disk");
            set(None);
        }
    }

    /// A switch from a preview: nothing of what goes is answered as it's
    /// torn down (a part still mounted too); what comes
    /// next has what it asks for as it loads, builds, runs its effects and
    /// calls answered.
    #[test]
    fn what_goes_is_torn_down_as_a_preview_and_what_comes_loads_as_itself() {
        const LEAVING: &str = r#"ui.on_cleanup(function()
	ui.invoke("cleanup_ran", 1)
	pcall(ui.invoke, "plugin.x.run", 1)
end)
local Part = ui.component(function()
	ui.on_cleanup(function()
		ui.invoke("part_cleanup_ran", 1)
		pcall(ui.invoke, "plugin.x.run", 1)
	end)
	return ui.text { "part" }
end)
return ui.column { Part {} }"#;
        const LOADS: &str = r#"local ok = pcall(ui.invoke, "plugin.y.run", 1)
ui.invoke(if ok then "load_ok" else "load_refused", 1)
ui.call("later", 1):after(function() ui.invoke("later_ok", 1) end, function() ui.invoke("later_refused", 1) end)
local P = ui.component(function()
	local ok = pcall(ui.invoke, "plugin.y.run", 1)
	ui.invoke(if ok then "build_ok" else "build_refused", 1)
	ui.effect(function()
		local ok = pcall(ui.invoke, "plugin.y.run", 1)
		ui.invoke(if ok then "effect_ok" else "effect_refused", 1)
	end)
	return ui.text { "p" }
end)
return ui.column { P {} }"#;
        let file = dir("goes-and-comes").join("app.luau");
        std::fs::write(&file, LEAVING).unwrap();
        let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> = Default::default();
        let (l, entry) = (log.clone(), file.clone());
        set(Some(named("a")));
        let app = zgx_script::testing::TestApp::new(move |runtime| {
            let host = zgx_script::ScriptHost::new(runtime, &entry);
            host.guard(guard);
            for name in ["cleanup_ran", "part_cleanup_ran", "plugin.x.run", "plugin.y.run", "load_ok", "load_refused", "build_ok", "build_refused", "effect_ok", "effect_refused", "later_ok", "later_refused"] {
                let l = l.clone();
                host.action(name, move |_| l.borrow_mut().push(name.to_string()));
            }
            host.async_action("later", move |_| async move {
                match current().is_some() {
                    true => Err("refused".to_string()),
                    false => Ok(zgx_script::Value::Nil),
                }
            });
            host
        });
        app.frames(6);
        log.borrow_mut().clear();

        for (kept, going) in [(true, None), (false, Some(named("b")))] {
            let switch = Switch::begin(going.clone(), kept);
            std::fs::write(&file, LOADS).unwrap();
            assert_eq!(now(switch.finish(true, true, false, &app.host, DROP, |_| std::future::ready(false))), Finished::Settled);
            app.frames(12);
            let seen = std::mem::take(&mut *log.borrow_mut());
            // (zgx runs no cleanup's call as it reloads: either way none of
            // the preview's is answered.)
            assert!(!seen.contains(&"plugin.x.run".into()), "torn down: {seen:?}");
            let expect = |word: &str| if going.is_none() { format!("{word}_ok") } else { format!("{word}_refused") };
            for word in ["load", "build", "effect", "later"] {
                assert!(seen.contains(&expect(word)), "{going:?}: {word} as what comes next is: {seen:?}");
            }
            assert_eq!(current(), going);
            // (Back to a preview of LEAVING for the next.)
            set(Some(named("a")));
            std::fs::write(&file, LEAVING).unwrap();
            app.host.reload().unwrap();
            app.frames(6);
            log.borrow_mut().clear();
        }
        set(None);
    }

    /// Every action the app's code refuses to a preview (`refuse("…")`,
    /// `refuse_aloud("…")`) is in `BLOCKED`: else its refusal never fires.
    #[test]
    fn every_action_the_code_refuses_is_blocked() {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push((path.clone(), std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let mut files = Vec::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut named = Vec::new();
        for (path, text) in &files {
            for marker in ["refuse(\"", "refuse_aloud(\""] {
                let mut rest = text.as_str();
                while let Some(at) = rest.find(marker) {
                    rest = &rest[at + marker.len()..];
                    let Some(end) = rest.find('"') else { break };
                    let action = &rest[..end];
                    // (Action names only: this test's own doc says `refuse("…")`.)
                    if action.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c == '.') {
                        named.push((path.display().to_string(), action.to_string()));
                    }
                }
            }
        }
        assert!(named.iter().any(|(_, a)| a == "agents.add_key"), "the scan finds the calls");
        let missing: Vec<_> = named.iter().filter(|(_, action)| !blocked(action)).collect();
        assert!(missing.is_empty(), "refused but not in BLOCKED: {missing:?}");
    }

    #[test]
    fn preview_refuses_the_dangerous_actions_only() {
        set(None);
        assert!(refuse("terminal.start").is_ok(), "nothing in preview: all allowed");
        set(Some(Preview { id: "agent".into(), name: "Agent's".into() }));
        for action in ["terminal.start", "terminal.write", "account.sign_out", "account.delete", "plugins.install", "templates.use", "files.write", "chat.send", "chat.respond", "chat.retry", "chat.queue_edit", "chat.queue_finish", "chat.configure", "files.open_external", "github.forget", "github.disconnect"] {
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

    /// A server on this machine: how many requests for `/<token>` reached it.
    /// (By token: a port freed by another process on this machine may have
    /// something still asking it.)
    fn serve() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = format!("preview-{}-{port}-{:?}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (counted, wanted) = (hits.clone(), token.clone());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut asked = [0; 4096];
                let n = stream.read(&mut asked).unwrap_or(0);
                if String::from_utf8_lossy(&asked[..n]).contains(&wanted) {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            }
        });
        (format!("http://127.0.0.1:{port}/{token}"), hits)
    }

    use zgx_script::{Capability, GuardKind, Plugin, ScriptHost, Value, testing::TestApp};

    /// `script` as the app's, under the window's guard.
    fn guarded(script: String, with: impl FnOnce(&ScriptHost) + 'static) -> TestApp {
        TestApp::new(move |runtime| {
            let host = ScriptHost::inline(runtime, "app", script);
            zgx_run::install_fetcher(&host);
            host.guard(guard);
            host.async_action("plugin.git.run", |_| async move { Ok(Value::from("ran")) });
            host.async_action("files.read", |_| async move { Ok(Value::from("read")) });
            with(&host);
            host
        })
    }

    /// `body`'s text, shown once it's run, in `out`.
    fn showing(body: &str) -> String {
        format!(
            r#"
local out = ui.state("out", "")
local function say(v: any) out:set(tostring(v)) end
{body}
return ui.column {{ ui.text {{ id = "out", function() return out:get() end }} }}
"#
        )
    }

    fn answered(app: &mut TestApp) -> String {
        assert!(app.wait_until(|app| !app.text("out").is_empty()), "no answer: {:?}", app.error());
        app.text("out")
    }

    fn sleek() -> Preview {
        Preview { id: "agent".into(), name: "Sleek".into() }
    }

    /// R7: a preview's `ui.fetch` rejects (its script's own `after` hears
    /// it), and nothing reaches the network; kept, the same line goes.
    #[test]
    fn a_preview_s_fetch_rejects_and_goes_nowhere() {
        let (url, hits) = serve();
        let body = format!(r#"ui.fetch("{url}"):after(function(r) say(`ok {{r.status}}`) end, function(e) say(`rejected {{e}}`) end)"#);
        set(Some(sleek()));
        let out = answered(&mut guarded(showing(&body), |_| {}));
        assert!(out.starts_with("rejected") && out.contains("Sleek is a preview"), "{out}");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "it went");
        set(None);
        assert_eq!(answered(&mut guarded(showing(&body), |_| {})), "ok 200");
    }

    /// R7: a preview's remote picture isn't asked for, nothing breaks, and a
    /// reload once it's kept shows it. Its own pictures show all along.
    #[test]
    fn a_preview_s_remote_picture_waits_and_its_own_shows() {
        let (url, hits) = serve();
        let root = dir("pictures");
        let own = root.join("own.png");
        // A 1x1 PNG.
        std::fs::write(&own, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 5, 0, 1, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82]).unwrap();
        let script = format!(
            r#"
local own = ui.state("own", "")
return ui.column {{
    ui.text {{ id = "own", function() return own:get() end }},
    ui.image {{ src = "{}", w = 8, h = 8, on_load = function() own:set("loaded") end }},
    ui.image {{ id = "remote", src = "{url}.png", w = 8, h = 8, fallback = ui.text {{ id = "fell-back", "gradient" }} }},
    ui.text {{ id = "up", "up" }},
}}
"#,
            // (In a Luau string: Windows' `\` escaped.)
            own.display().to_string().replace('\\', "\\\\")
        );
        set(Some(sleek()));
        let mut app = guarded(script, |_| {});
        assert!(app.wait_until(|app| app.text("own") == "loaded"), "its own picture didn't show: {:?}", app.error());
        for _ in 0..20 {
            app.frames(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the remote picture went");
        assert_eq!(app.text("up"), "up", "the window carries on");
        assert!(app.find("#fell-back").is_some(), "its fallback shows");
        // Kept: the guard says yes, and the reload asks again.
        set(None);
        app.host.reload().unwrap();
        assert!(app.wait_until(|_| hits.load(Ordering::SeqCst) > 0), "it didn't come back on Keep, with no relaunch");
    }

    /// R7: a preview can't use a plugin's powers, start a program or load a
    /// page, each with the same words; it reads as the app does, and a plugin
    /// beside it isn't asked.
    #[test]
    fn a_preview_runs_nothing_and_borrows_nothing() {
        let root = dir("runs-nothing");
        let ran = root.join("ran");
        set(Some(sleek()));
        for (what, body) in [
            ("a plugin's programs", r#"local ok, e = pcall(function() return ui.call("plugin.git.run", {}) end); say(if ok then "ok" else `refused {e}`)"#.to_string()),
            ("a program", format!(r#"local ok, e = pcall(function() return ui.term("x", {{ command = "/usr/bin/touch {}" }}) end); say(if ok then "ok" else `refused {{e}}`)"#, ran.display())),
            ("a page", r#"local ok, e = pcall(function() ui.web("p"):load("https://example.com/") end); say(if ok then "ok" else `refused {e}`)"#.to_string()),
        ] {
            let out = answered(&mut guarded(showing(&body), |_| {}));
            assert!(out.starts_with("refused") && out.contains("Sleek is a preview"), "{what}: {out}");
        }
        assert!(!ran.exists(), "the program ran");
        let reads = r#"ui.call("files.read", {}):after(function(v) say(`ok {v}`) end, function(e) say(`rejected {e}`) end)"#;
        assert_eq!(answered(&mut guarded(showing(reads), |_| {})), "ok read", "reading isn't refused");

        let (url, hits) = serve();
        let entry = root.join("init.luau");
        let plugin = format!(
            r#"local out = ui.state("out", "")
            ui.fetch("{url}"):after(function(r) out:set(`ok {{r.status}}`) end, function(e) out:set(`failed {{e}}`) end)
            ui.contribute("bar", ui.text {{ id = "out", function() return out:get() end }})"#
        );
        std::fs::write(&entry, plugin).unwrap();
        let mut app = guarded(r#"return ui.column { ui.slot("bar") }"#.to_string(), move |host| {
            host.set_plugins(vec![Plugin { name: "plug".into(), entry, grants: vec![Capability::Fetch] }]);
        });
        assert_eq!(answered(&mut app), "ok 200", "a plugin's VM isn't under the guard");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        set(None);
    }

    /// Every action in `BLOCKED` answers for itself, as `refuse` does: a name
    /// listed with nothing checking it would be open.
    #[test]
    fn every_blocked_action_checks_for_itself() {
        let mut sources = String::new();
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("preview.rs") {
                    sources.push_str(&std::fs::read_to_string(path).unwrap());
                }
            }
        }
        for action in BLOCKED {
            let (checked, aloud) = (format!("refuse(\"{action}"), format!("refuse_aloud(\"{action}"));
            assert!(sources.contains(&checked) || sources.contains(&aloud), "nothing refuses {action}");
        }
    }

    /// Every refusal is decided as its action is called, never inside the
    /// async block or `async fn` that runs when its future is first polled:
    /// a call made, then a switch in the same turn, would be let through.
    #[test]
    fn every_refusal_is_decided_at_the_call() {
        let mut found = Vec::new();
        let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("preview.rs") && !path.to_string_lossy().ends_with("_tests.rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (line, inside) in refusals_inside_async(&text) {
                        if inside {
                            found.push(format!("{}:{line}", path.display()));
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "decided when polled, not called: {found:#?}");
        // (The scan itself: one decided at the call, one not.)
        let seen = refusals_inside_async(
            "fn i(h: &H) {\n h.async_action(\"a\", |x| { let allowed = preview::refuse(\"a\"); async move { allowed?; Ok(x) } });\n h.async_action(\"b\", |x| async move { preview::refuse(\"b\")?; Ok(x) });\n}\nasync fn c() { preview::refuse(\"c\").ok(); }",
        );
        assert_eq!(seen, vec![(2, false), (3, true), (5, true)]);
    }

    /// Each `refuse(`/`refuse_aloud(` (or a `preview::current().is_some()`) in
    /// `text`: its line, and whether any block
    /// round it is an async block or an `async fn`'s body. (Strings, chars
    /// and comments skipped.)
    fn refusals_inside_async(text: &str) -> Vec<(usize, bool)> {
        let b = text.as_bytes();
        let (mut out, mut blocks, mut pending_fn) = (Vec::new(), Vec::<bool>::new(), false);
        let (mut i, mut line) = (0, 1);
        while i < b.len() {
            if !text.is_char_boundary(i) {
                i += 1;
                continue;
            }
            let rest = &text[i..];
            if rest.starts_with("//") {
                i += rest.find('\n').unwrap_or(rest.len());
                continue;
            }
            if rest.starts_with("/*") {
                i += rest.find("*/").map_or(rest.len(), |e| e + 2);
                continue;
            }
            match b[i] {
                b'\n' => line += 1,
                b'"' => {
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        if b[i] == b'\n' {
                            line += 1;
                        }
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                }
                b'\'' if rest.len() > 2 && (rest.as_bytes()[2] == b'\'' || rest.as_bytes()[1] == b'\\') => {
                    i += rest[1..].find('\'').map_or(1, |e| e + 1);
                }
                b'{' => {
                    let before = text[..i].trim_end();
                    let opener = before.ends_with("async") || before.ends_with("async move") || std::mem::take(&mut pending_fn);
                    blocks.push(opener);
                }
                b'}' => {
                    blocks.pop();
                }
                _ if rest.starts_with("async fn") => pending_fn = true,
                _ if rest.starts_with("refuse(\"") || rest.starts_with("refuse_aloud(\"") || rest.starts_with("preview::current().is_some()") => {
                    out.push((line, blocks.iter().any(|a| *a)));
                }
                _ => {}
            }
            i += 1;
        }
        out
    }

    /// Wing's call: an unkept preview can't delete what you have or undo your
    /// sign-ins. The real actions, as they're installed: each refused at once
    /// with the same words, and without a preview each goes on to the engine
    /// (none here, so it waits for one rather than answering).
    #[test]
    fn a_preview_can_t_remove_what_you_have() {
        let installed = |host: &ScriptHost| {
            crate::projects::install(host);
            crate::agents::install(host);
            crate::sessions::delete_action_for_tests(host);
            super::super::set_off_action_for_tests(host);
        };
        for (action, arg) in [
            ("sessions.delete", r#""s1""#),
            ("projects.delete", r#""p1""#),
            ("agents.remove", r#"{ id = "claude", account = "a1" }"#),
            ("agents.sign_out", r#""acp.tool""#),
            ("plugins.set_off", r#"{ id = "focus", off = true }"#),
        ] {
            let body = format!(r#"ui.call("{action}", {arg}):after(function() say("ok") end, function(e) say(`rejected {{e}}`) end)"#);
            set(Some(sleek()));
            let out = answered(&mut guarded(showing(&body), installed));
            assert!(out.starts_with("rejected") && out.contains("Sleek is a preview"), "{action}: {out}");
            set(None);
            let mut app = guarded(showing(&body), installed);
            for _ in 0..20 {
                app.frames(2);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(!app.text("out").contains("is a preview"), "{action} without a preview: {}", app.text("out"));
            assert_eq!(app.error(), None, "{action}");
        }
        for action in ["github.disconnect", "sessions.delete", "plugins.set_off"] {
            set(Some(sleek()));
            assert!(refuse(action).is_err(), "{action}");
            set(None);
            assert!(refuse(action).is_ok(), "{action}");
        }
    }

    /// What the guard says of each kind, in preview and kept: only what runs
    /// a program or carries data out is refused; the rest is the template's.
    #[test]
    fn the_guard_refuses_only_what_runs_or_leaves() {
        set(Some(sleek()));
        for (kind, name) in [(GuardKind::Fetch, "https://x/"), (GuardKind::Picture, "https://x/a.png"), (GuardKind::Socket, "wss://x/"), (GuardKind::Web, "https://x/"), (GuardKind::OpenLink, "https://x/"), (GuardKind::Term, "curl x | sh"), (GuardKind::TermInput, "main"), (GuardKind::Persist, "accent"), (GuardKind::AsyncAction, "plugin.git.run"), (GuardKind::Action, "plugin.git.toast"), (GuardKind::Function, "plugin.git.file")] {
            let said = guard(kind, name);
            assert!(said.as_ref().is_err_and(|e| refused(e)), "{kind:?} {name}: {said:?}");
        }
        for (kind, name) in [(GuardKind::Clipboard, "write"), (GuardKind::Dialog, "ui.open_files"), (GuardKind::File, "/x"), (GuardKind::Folder, "k"), (GuardKind::Notification, "t"), (GuardKind::Window, "w"), (GuardKind::AsyncAction, "files.read"), (GuardKind::AsyncAction, "chat.send"), (GuardKind::Action, "appearance")] {
            assert!(guard(kind, name).is_ok(), "{kind:?} {name}");
        }
        // A terminal Zeron started shows; a command it didn't never starts.
        crate::terminal::note_issued("zeron-zgx terminal-bridge --terminal t1");
        assert!(guard(GuardKind::Term, "zeron-zgx terminal-bridge --terminal t1").is_ok());
        assert!(guard(GuardKind::Term, "zeron-zgx terminal-bridge --terminal t2").is_err());
        set(None);
        for (kind, name) in [(GuardKind::Fetch, "https://x/"), (GuardKind::Term, "curl x | sh"), (GuardKind::Persist, "accent"), (GuardKind::AsyncAction, "plugin.git.run")] {
            assert!(guard(kind, name).is_ok(), "kept: {kind:?} {name}");
        }
        assert!(!refused("x is a preview"), "no preview, no refusal");
    }

    /// A preview reads what's saved, so settings look as they do; what it
    /// writes isn't saved. Kept, it is; reverted, it's gone.
    #[test]
    fn a_preview_holds_what_it_saves_until_it_s_kept() {
        let root = dir("held");
        let storage = root.join("state");
        let script = r#"
local accent = ui.persist("accent", "default")
ui.command { id = "set", run = function() accent:set("preview") end }
return ui.column { ui.text { id = "accent", function() return accent:get() end } }
"#;
        let mount = |with_guard: bool| {
            let storage = storage.clone();
            TestApp::new(move |runtime| {
                let host = ScriptHost::inline(runtime, "app", script).storage(storage);
                if with_guard {
                    host.guard(guard);
                }
                host
            })
        };
        // What the person has saved, kept.
        set(None);
        {
            let app = mount(false);
            app.host.shared("x", 0.);
            assert!(app.host.run_command("set"));
            app.host.save();
        }
        let saved_as = |word: &str| {
            let found = std::fs::read_dir(&storage).unwrap().flatten().flat_map(|d| if d.path().is_dir() { std::fs::read_dir(d.path()).unwrap().flatten().map(|f| f.path()).collect() } else { vec![d.path()] }).any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains(word)));
            assert!(found, "{word} isn't saved");
        };
        saved_as("preview");
        // Written over as the person's, to tell apart from the preview's.
        for entry in std::fs::read_dir(&storage).unwrap().flatten() {
            let files: Vec<_> = if entry.path().is_dir() { std::fs::read_dir(entry.path()).unwrap().flatten().map(|f| f.path()).collect() } else { vec![entry.path()] };
            for file in files {
                let text = std::fs::read_to_string(&file).unwrap();
                std::fs::write(&file, text.replace("preview", "mine")).unwrap();
            }
        }

        set(Some(sleek()));
        let mut app = mount(true);
        app.frames(2);
        assert_eq!(app.text("accent"), "mine", "a preview reads what's saved");
        assert!(app.host.run_command("set"));
        app.frames(2);
        assert_eq!(app.text("accent"), "preview", "and shows what it wrote");
        app.host.save();
        saved_as("mine");
        // Reverted: nothing of it is saved.
        app.host.drop_held_state();
        set(None);
        app.host.reload().unwrap();
        app.frames(2);
        assert_eq!(app.text("accent"), "mine");
        app.host.save();
        saved_as("mine");

        // Kept: the same keys saved, and what shows after the reload.
        set(Some(sleek()));
        app.host.reload().unwrap();
        app.frames(2);
        assert!(app.host.run_command("set"));
        app.frames(2);
        assert_eq!(app.host.keep_held_state(), 1);
        set(None);
        app.host.reload().unwrap();
        app.frames(2);
        assert_eq!(app.text("accent"), "preview");
        app.host.save();
        saved_as("preview");
    }

    /// A setting first written in a preview, with nothing saved before it, is
    /// on disk once it's kept, as much as one that was (zgx #1468).
    #[test]
    fn a_key_first_written_in_a_preview_is_saved_on_keep() {
        let storage = dir("held-new").join("state");
        let script = r#"
local first = ui.persist("first_written", "default")
ui.command { id = "set", run = function() first:set("written") end }
return ui.column { ui.text { id = "first", function() return first:get() end } }
"#;
        set(Some(sleek()));
        let mut app = TestApp::new({
            let storage = storage.clone();
            move |runtime| {
                let host = ScriptHost::inline(runtime, "app", script).storage(storage);
                host.guard(guard);
                host
            }
        });
        app.frames(2);
        assert!(app.host.run_command("set"));
        app.frames(2);
        app.host.save();
        assert!(!storage.join("app").join("first_written.json").exists(), "saved while it's a preview");
        assert_eq!(app.host.keep_held_state(), 1);
        set(None);
        app.host.reload().unwrap();
        app.frames(2);
        assert_eq!(app.text("first"), "written");
        app.host.save();
        let saved = std::fs::read_to_string(storage.join("app").join("first_written.json")).expect("saved on keep");
        assert!(saved.contains("written"), "{saved}");
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
