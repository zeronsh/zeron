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
    "engine.restart",
    "billing.checkout",
    // (Each runs a machine, or says which one new chats run on: credits.)
    "cloud.wake",
    "cloud.sleep",
    "cloud.retry",
    "cloud.set_machine",
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

    #[test]
    fn preview_refuses_the_dangerous_actions_only() {
        set(None);
        assert!(refuse("terminal.start").is_ok(), "nothing in preview: all allowed");
        set(Some(Preview { id: "agent".into(), name: "Agent's".into() }));
        for action in ["terminal.start", "terminal.write", "account.sign_out", "account.delete", "plugins.install", "templates.use", "files.write", "chat.send", "chat.respond", "chat.retry", "chat.queue_edit", "chat.queue_finish", "chat.configure", "files.open_external"] {
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
