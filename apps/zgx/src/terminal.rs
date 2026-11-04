//! A session's terminals, for the panel's terminal tabs: a shell on the
//! session's device, in its folder, through the engine's terminal calls (the
//! shipping app's, crates/ui/src/terminal/panel.rs), drawn by zgx's own
//! terminal (`ui.term` / `ui.terminal`).
//!
//! zgx's terminal runs a process on a pseudoterminal of its own, so the
//! process it runs is this app's bridge (`zeron-zgx terminal-bridge …`):
//! it attaches to the engine's terminal and passes bytes both ways, its own
//! pseudoterminal in raw mode, and tells the engine its size as zgx sizes
//! it. The engine routes the calls to the session's device, as the file
//! calls are. zgx keeps the session (and so the bridge) across tab switches
//! and reloads; the engine's terminal outlives the bridge until it's closed.
//!
//! A new terminal is opened by its bridge, at the size zgx gave the bridge's
//! pseudoterminal: a shell started at another size draws its first prompt
//! for that width (zsh's end-of-line mark, a lone `%`, is left on the first
//! line of a narrower one). The bridge hands the engine's id back through a
//! claim file, which the app reads (`terminal.claimed`).
//!
//! The bridge sleeps until there's something to pass on: what's typed, what
//! the engine sent, or a new size (SIGWINCH). It once looked every 15 ms,
//! each terminal's bridge waking some 70 times a second while nothing
//! happened.
//!
//! On Windows zgx's pseudoterminal is a pseudoconsole (ConPTY), which runs
//! the bridge's command line as it is (no `/bin/sh -c`: its words are quoted
//! for the program's own parser), and the bridge's terminal is a console: put
//! in raw mode with keys as VT sequences, its size read from its buffer. A
//! resize says nothing the bridge can wait on, so it looks at the size each
//! time it wakes, and at least every [`console::LOOK`].
//!
//! The calls (`ui.call`, each a promise):
//!   terminal.start { target }                a new terminal's bridge:
//!                                            { command, claim }
//!   terminal.claimed { claim }               its engine terminal, once the
//!                                            bridge has opened it: the id
//!   terminal.command { id, device? }         the bridge's command line
//!   terminal.status { id, device? }          { busy, exited }: whether
//!                                            closing ends a program
//!   terminal.close { id, device? }           the engine's terminal ended
//!   terminal.write { id, device?, text }     typed into it, as if from the
//!                                            keyboard (an agent's sign-in
//!                                            command, run for you)
//! and `ui.call_sync("terminal.cwd")`: the folder a bridge runs in.
//! (`target` as src/files.rs's: { chat?, space?, device?, root? }).

use std::{
    io::Write,
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};
#[cfg(windows)]
use std::sync::mpsc;

use base64::Engine as _;
use serde_json::{Map, Value as Json, json};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

use crate::engine::{self, rpc};

pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    engine::install(cx);
    host.async_action("terminal.start", |arg| async move { crate::plugins::preview::refuse("terminal.start")?; start(arg).await });
    host.async_action("terminal.claimed", |arg| async move { claimed(arg).await });
    host.async_action("terminal.command", |arg| async move { crate::plugins::preview::refuse("terminal.command")?;
        let device = crate::files::route_to(opt_text(arg.field("device"))).await;
        command(arg, device)
    });
    host.async_action("terminal.status", |arg| async move {
        let reply = call_for("TerminalStatus", &arg, Map::new()).await?;
        Ok(Value::record([
            ("busy", Value::Bool(reply["busy"].as_bool().unwrap_or(false))),
            ("exited", Value::Bool(reply["exited"].as_bool().unwrap_or(false))),
        ]))
    });
    host.async_action("terminal.close", |arg| async move {
        call_for("CloseTerminal", &arg, Map::new()).await?;
        Ok(Value::Nil)
    });
    host.async_action("terminal.write", |arg| async move { crate::plugins::preview::refuse("terminal.write")?;
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(text(arg.field("text")));
        let mut params = Map::new();
        params.insert("data".into(), Json::String(data));
        call_for("WriteTerminal", &arg, params).await?;
        Ok(Value::Nil)
    });
    host.function("terminal.cwd", |_| Value::from(bridge_dir().as_str()));
}

/// Where a bridge runs (its own folder, not the shell's: that's the
/// engine's). The home folder: `~` where zgx expands it, from `HOME`; on
/// Windows, which has no `HOME`, the profile's folder itself (zgx would
/// hand `~` to CreateProcess as it is, and the process wouldn't start:
/// zgx-asks/terminal-home), else the temporary folder.
fn bridge_dir() -> String {
    if cfg!(windows) {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|p| p.is_dir());
        home.unwrap_or_else(std::env::temp_dir).to_string_lossy().into_owned()
    } else {
        "~".into()
    }
}

fn text(value: Option<&Value>) -> String {
    value.map(crate::auth::text).unwrap_or_default()
}

fn opt_text(value: Option<&Value>) -> Option<String> {
    let t = text(value);
    (!t.is_empty()).then_some(t)
}

/// A word for the screen from an engine error: "offline" when it couldn't
/// be reached, what to do when it's from before terminals, else what it
/// said.
fn said(error: String) -> String {
    if engine::outcome(&error) == "offline" {
        "offline".into()
    } else if error.to_ascii_lowercase().contains("unknown method") {
        "Update Zeron to use terminals here.".into()
    } else {
        error
    }
}

/// A call about terminal `arg.id`, routed to `arg.device`.
async fn call_for(method: &str, arg: &Value, mut params: Map<String, Json>) -> Result<Json, String> {
    params.insert("terminalId".into(), Json::String(text(arg.field("id"))));
    if let Some(device) = crate::files::route_to(opt_text(arg.field("device"))).await {
        params.insert("targetDeviceId".into(), Json::String(device));
    }
    engine::get().call(method, Json::Object(params)).await.map_err(said)
}

/// OpenTerminal's parameters for `target`, but its size.
async fn open_params(target: &Value) -> Result<Map<String, Json>, String> {
    let mut params = Map::new();
    // A session's terminal: its chat. A project with none open: its canvas,
    // in its folder (the engine's `space-canvas:{id}`).
    if let Some(chat) = opt_text(target.field("chat")) {
        params.insert("chatId".into(), Json::String(chat));
    } else if let Some(space) = opt_text(target.field("space")) {
        params.insert("chatId".into(), Json::String(format!("space-canvas:{space}")));
        if let Some(root) = opt_text(target.field("root")) {
            params.insert("cwd".into(), Json::String(root));
        }
    } else {
        return Err("no session or project".into());
    }
    if let Some(device) = crate::files::route_to(opt_text(target.field("device"))).await {
        params.insert("targetDeviceId".into(), Json::String(device));
    }
    Ok(params)
}

/// Where new terminals' bridges leave their ids.
fn claims_dir() -> PathBuf {
    std::env::temp_dir().join("zeron-zgx-terminals")
}

/// A new terminal's bridge line: it opens the engine's terminal itself, at
/// its own size, and leaves the id in `claim`.
async fn start(arg: Value) -> Result<Value, String> {
    let target = arg.field("target").ok_or("no target")?;
    let params = open_params(target).await?;
    let (port, data) = engine::get().ipc_endpoint().ok_or("Zeron isn't available here")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = claims_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let claim = dir.join(format!("{}-{}", std::process::id(), operation_nonce()));
    let line = format!(
        "{} terminal-bridge --port {port} --data {} --open {} --claim {}",
        quote(&exe.to_string_lossy()),
        quote(&data.to_string_lossy()),
        quote(&Json::Object(params).to_string()),
        quote(&claim.to_string_lossy()),
    );
    Ok(Value::record([("command", Value::from(line.as_str())), ("claim", Value::from(claim.to_string_lossy().as_ref()))]))
}

/// A number unique to this run.
fn operation_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
    now ^ (NEXT.fetch_add(1, Ordering::Relaxed) << 48)
}

/// The engine terminal a bridge opened, once it has (15 s at most). The
/// claim is the app's own file in its own folder, read once.
async fn claimed(arg: Value) -> Result<Value, String> {
    let claim = PathBuf::from(text(arg.field("claim")));
    if claim.parent() != Some(claims_dir().as_path()) {
        return Err("not a terminal's claim".into());
    }
    // (Watched for off this thread, the task woken by what's found.)
    let (tx, mut rx) = zgx::task::latest::<Result<String, String>>();
    std::thread::spawn(move || {
        tx.send(await_claim(&claim, CLAIM_WAIT));
    });
    let id = rx.recv().await.unwrap_or_else(|| Err("The terminal didn't open.".into()))?;
    // Opened while a template ran: closed if safe mode starts (preview.rs).
    crate::plugins::preview::terminal_opened(&id, None);
    Ok(Value::from(id.as_str()))
}

/// How long a bridge has to open its terminal: it says why itself when it
/// can't, so only a bridge gone without a word takes this long.
const CLAIM_WAIT: Duration = Duration::from_secs(120);

/// What a claim holds: the bridge's terminal id, or why it couldn't open
/// one. A bridge that hasn't claimed within `wait` is told the app has gone
/// (the claim taken first, `GONE`, which its own can't then replace): it
/// closes what it opens. Either side takes the claim with an operation that
/// fails if the other has, so a terminal is never left with no one's.
fn await_claim(claim: &std::path::Path, wait: Duration) -> Result<String, String> {
    let until = std::time::Instant::now() + wait;
    loop {
        if let Ok(text) = std::fs::read_to_string(claim) {
            let _ = std::fs::remove_file(claim);
            let text = text.trim();
            if let Some(why) = text.strip_prefix("error: ") {
                return Err(why.to_owned());
            }
            if !text.is_empty() {
                return Ok(text.to_owned());
            }
        }
        if std::time::Instant::now() >= until {
            use std::io::Write as _;
            match std::fs::OpenOptions::new().write(true).create_new(true).open(claim) {
                // Taken first: a bridge that claims later closes its own.
                Ok(mut file) => {
                    let _ = file.write_all(GONE.as_bytes());
                    return Err("The terminal didn't open.".into());
                }
                // Claimed in the meantime: read on the next turn.
                Err(_) => continue,
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What an app that stopped waiting leaves in a claim.
const GONE: &str = "gone";

/// A word for `/bin/sh -c`, quoted.
#[cfg(unix)]
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// A word for a Windows command line, quoted as the program's own parser
/// (the C runtime's, and Rust's `std::env::args`) reads it back: in double
/// quotes, a quote inside escaped with a backslash, and the backslashes
/// before a quote (or the closing one) doubled. ConPTY hands the line to
/// `CreateProcessW` as it is; nothing else reads it (no `cmd.exe`).
#[cfg(any(windows, test))]
fn quote_windows(word: &str) -> String {
    let mut out = String::with_capacity(word.len() + 2);
    out.push('"');
    let mut backslashes = 0;
    for c in word.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                continue;
            }
            '"' => out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1)),
            _ => out.extend(std::iter::repeat_n('\\', backslashes)),
        }
        backslashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}
#[cfg(windows)]
fn quote(word: &str) -> String {
    quote_windows(word)
}

/// The bridge's command line for terminal `arg.id` (on `device`, another
/// device's, routed there).
fn command(arg: Value, device: Option<String>) -> Result<Value, String> {
    let (port, data) = engine::get().ipc_endpoint().ok_or("Zeron isn't available here")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let id = text(arg.field("id"));
    if id.is_empty() {
        return Err("no terminal".into());
    }
    let mut line = format!(
        "{} terminal-bridge --port {port} --data {} --terminal {}",
        quote(&exe.to_string_lossy()),
        quote(&data.to_string_lossy()),
        quote(&id),
    );
    if let Some(device) = device {
        line.push_str(&format!(" --device {}", quote(&device)));
    }
    Ok(Value::from(line.as_str()))
}

// ---------------------------------------------------------------------------
// The bridge: its own process, run by zgx's terminal.

struct Args {
    port: u16,
    data: PathBuf,
    /// The engine terminal to attach to; or, for a new one, what to open
    /// (OpenTerminal's parameters, but its size) and where to leave its id.
    terminal: Option<String>,
    open: Option<Map<String, Json>>,
    claim: Option<PathBuf>,
    device: Option<String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut port = None;
    let mut data = None;
    let mut terminal = None;
    let mut open = None;
    let mut claim = None;
    let mut device = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--port" => port = value.parse().ok(),
            "--data" => data = Some(PathBuf::from(value)),
            "--terminal" => terminal = Some(value.clone()),
            "--open" => match serde_json::from_str::<Json>(value) {
                Ok(Json::Object(params)) => open = Some(params),
                _ => return Err("--open takes OpenTerminal's parameters".into()),
            },
            "--claim" => claim = Some(PathBuf::from(value)),
            "--device" => device = Some(value.clone()),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if terminal.is_none() && (open.is_none() || claim.is_none()) {
        return Err("--terminal, or --open with --claim, is needed".into());
    }
    Ok(Args {
        port: port.ok_or("--port is needed")?,
        data: data.ok_or("--data is needed")?,
        terminal,
        open,
        claim,
        device,
    })
}

type Socket = tungstenite::WebSocket<TcpStream>;

fn dial(port: u16, token: Option<&str>) -> Result<Socket, String> {
    use tungstenite::client::IntoClientRequest;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    let mut request = format!("ws://127.0.0.1:{port}").into_client_request().map_err(|e| e.to_string())?;
    if let Some(token) = token {
        let value = tungstenite::http::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| e.to_string())?;
        request.headers_mut().insert(tungstenite::http::header::AUTHORIZATION, value);
    }
    let (socket, _) = tungstenite::client::client(request, stream).map_err(|e| e.to_string())?;
    // (Opening a terminal waits on its answer with short reads; passing
    // bytes, the socket doesn't block at all: see `bridge`.)
    let _ = socket.get_ref().set_read_timeout(Some(Duration::from_millis(15)));
    Ok(socket)
}

/// The terminal's size as zgx gave this process's pseudoterminal.
#[cfg(unix)]
fn window_size() -> Option<(u16, u16)> {
    // SAFETY: TIOCGWINSZ fills the zeroed `winsize` for fd 0, its own terminal.
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        (libc::ioctl(0, libc::TIOCGWINSZ, &mut size) == 0 && size.ws_col > 0).then_some((size.ws_col, size.ws_row))
    }
}
#[cfg(windows)]
fn window_size() -> Option<(u16, u16)> {
    console::size()
}

/// Its size once zgx has given it one and left it (two reads 40 ms apart
/// agree), half a second at most: a shell started then draws for it.
fn settled_size() -> (u16, u16) {
    let mut last = window_size();
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(40));
        let now = window_size();
        if now.is_some() && now == last {
            break;
        }
        last = now;
    }
    last.unwrap_or((80, 24))
}

/// Writes `text` to `claim` whole (a file made aside, then linked into
/// place, which fails if the claim is there already). False when the app
/// stopped waiting first (its `GONE` there): no one will take what's
/// claimed, and the claim is cleared.
fn leave_claim(claim: &std::path::Path, text: &str) -> bool {
    let aside = claim.with_extension("part");
    if std::fs::write(&aside, text).is_err() {
        return true;
    }
    let linked = std::fs::hard_link(&aside, claim).is_ok();
    let _ = std::fs::remove_file(&aside);
    if !linked {
        let _ = std::fs::remove_file(claim);
    }
    linked
}

/// Opens the engine's terminal at `size` (what `open` says, and the size);
/// its id, or what went wrong.
fn open_terminal(socket: &mut Socket, open: &Map<String, Json>, size: (u16, u16)) -> Result<String, String> {
    const OPEN_ID: u64 = 1_000_000;
    let mut params = open.clone();
    params.insert("cols".into(), json!(size.0));
    params.insert("rows".into(), json!(size.1));
    // (Routed as `open` says: its targetDeviceId is in it already.)
    send(socket, OPEN_ID, "OpenTerminal", params, &None).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        match socket.read() {
            Ok(tungstenite::Message::Text(frame)) => {
                for line in frame.as_str().lines().filter(|l| !l.trim().is_empty()) {
                    let Ok(frame) = serde_json::from_str::<Json>(line) else {
                        continue;
                    };
                    if frame["id"].as_u64() != Some(OPEN_ID) {
                        continue;
                    }
                    if let Some(error) = frame["err"].as_str() {
                        return Err(said(error.to_owned()));
                    }
                    return frame["ok"]["id"].as_str().map(str::to_owned).ok_or_else(|| "no terminal".into());
                }
            }
            Ok(tungstenite::Message::Close(_)) => return Err("offline".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("The terminal didn't open in time.".into())
}

/// Its pseudoterminal in raw mode (what's typed goes on as it is, echoed by
/// the shell at the other end), put back as the bridge ends.
#[cfg(unix)]
struct Raw(Option<libc::termios>);
#[cfg(unix)]
impl Raw {
    fn new() -> Self {
        // SAFETY: tcgetattr/tcsetattr on fd 0 with a termios read from it.
        unsafe {
            let mut was: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut was) != 0 {
                return Raw(None);
            }
            let mut raw = was;
            libc::cfmakeraw(&mut raw);
            libc::tcsetattr(0, libc::TCSANOW, &raw);
            Raw(Some(was))
        }
    }
}
#[cfg(unix)]
impl Drop for Raw {
    fn drop(&mut self) {
        if let Some(was) = self.0 {
            // SAFETY: restores what tcgetattr gave for fd 0.
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, &was);
            }
        }
    }
}

/// The bridge's terminal on Windows: the console of the pseudoconsole zgx
/// runs it on (ConPTY).
#[cfg(windows)]
mod console {
    use std::{net::TcpStream, os::windows::io::AsRawSocket, sync::mpsc, time::Duration};

    use windows_sys::Win32::{
        Foundation::HANDLE,
        Networking::WinSock::{FD_CLOSE, FD_READ, FD_WRITE, SOCKET, WSACloseEvent, WSACreateEvent, WSAEVENT, WSAEventSelect, WSAResetEvent},
        System::{
            Console::{
                CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
                ENABLE_PROCESSED_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT,
                ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle,
                ReadConsoleW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
            },
            Threading::{CreateEventW, SetEvent, WaitForMultipleObjects},
        },
    };

    /// How often a bridge with nothing to do looks at its size: a resize
    /// reaches the console's buffer with nothing to wait on.
    pub const LOOK: Duration = Duration::from_millis(100);

    fn std_handle(which: u32) -> HANDLE {
        // SAFETY: only reads the process's standard handle.
        unsafe { GetStdHandle(which) }
    }

    /// Its size as zgx gave the pseudoconsole: the console's window.
    pub fn size() -> Option<(u16, u16)> {
        // SAFETY: fills the zeroed struct for the process's own console
        // output; fails (0) on anything that isn't one.
        let window = unsafe {
            let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
            if GetConsoleScreenBufferInfo(std_handle(STD_OUTPUT_HANDLE), &mut info) == 0 {
                return None;
            }
            info.srWindow
        };
        let cols = i32::from(window.Right) - i32::from(window.Left) + 1;
        let rows = i32::from(window.Bottom) - i32::from(window.Top) + 1;
        (cols > 0 && rows > 0).then(|| (cols as u16, rows as u16))
    }

    /// The console in raw mode, put back as the bridge ends: what's typed
    /// comes as the terminal sent it (no echo, no line editing; Ctrl+C a
    /// byte for the shell at the other end, not a signal to the bridge;
    /// keys as VT sequences), and what the shell writes, VT and all, is
    /// drawn as written (a line feed only a line feed, as on a Unix
    /// pseudoterminal in raw mode).
    pub struct Raw {
        input: Option<u32>,
        output: Option<u32>,
    }

    impl Raw {
        pub fn new() -> Self {
            let set = |which: u32, mode: &dyn Fn(u32) -> u32| {
                let handle = std_handle(which);
                let mut was = 0;
                // SAFETY: reads and sets the mode of the process's own
                // console handle; both fail harmlessly on anything else.
                unsafe { (GetConsoleMode(handle, &mut was) != 0 && SetConsoleMode(handle, mode(was)) != 0).then_some(was) }
            };
            Raw {
                input: set(STD_INPUT_HANDLE, &|was| {
                    (was & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT)) | ENABLE_VIRTUAL_TERMINAL_INPUT
                }),
                output: set(STD_OUTPUT_HANDLE, &|was| {
                    was | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN
                }),
            }
        }
    }

    impl Drop for Raw {
        fn drop(&mut self) {
            for (which, was) in [(STD_INPUT_HANDLE, self.input), (STD_OUTPUT_HANDLE, self.output)] {
                if let Some(was) = was {
                    // SAFETY: restores the mode read from the same handle.
                    unsafe { SetConsoleMode(std_handle(which), was) };
                }
            }
        }
    }

    /// An event the bridge waits on, set from another thread.
    pub struct Event(usize);

    impl Event {
        pub fn set(&self) {
            // SAFETY: the event lives as long as the process (never closed:
            // the typing thread may set it until the bridge exits).
            unsafe { SetEvent(self.0 as HANDLE) };
        }
    }

    /// What's typed, read on a thread of its own: each read sent, and the
    /// bridge woken. The channel closes as the console does (its tab).
    /// (`ReadConsoleW` itself: std's console reads end at a Ctrl+Z, which a
    /// shell at the other end may want.)
    pub fn typed(wake: Event) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let input = std_handle(STD_INPUT_HANDLE);
            let mut buf = [0u16; 1024];
            // A character's first half, ending a read, waits for its second.
            let mut high: Option<u16> = None;
            loop {
                let mut read = 0u32;
                // SAFETY: reads at most `buf.len()` UTF-16 units into `buf`.
                let ok = unsafe { ReadConsoleW(input, buf.as_mut_ptr().cast(), buf.len() as u32, &mut read, std::ptr::null()) };
                if ok == 0 || read == 0 {
                    break;
                }
                let mut units: Vec<u16> = high.take().into_iter().chain(buf[..read as usize].iter().copied()).collect();
                if units.last().is_some_and(|u| (0xD800..0xDC00).contains(u)) {
                    high = units.pop();
                }
                if units.is_empty() {
                    continue;
                }
                if tx.send(String::from_utf16_lossy(&units).into_bytes()).is_err() {
                    break;
                }
                wake.set();
            }
            drop(tx);
            wake.set();
        });
        rx
    }

    /// The bridge's sleep: until the engine's socket has something (or
    /// room again, or closed), something's typed, or it's time to look at
    /// the size.
    pub struct Waiter {
        socket: WSAEVENT,
        typed: HANDLE,
    }

    impl Waiter {
        /// Selects the socket's events (which leaves it non-blocking); with
        /// the event that says something was typed.
        pub fn new(socket: &TcpStream) -> Option<(Self, Event)> {
            // SAFETY: a new manual-reset event selected for the socket's
            // reads, room and close; a new auto-reset event, unnamed.
            unsafe {
                let event = WSACreateEvent();
                if event == 0 {
                    return None;
                }
                if WSAEventSelect(socket.as_raw_socket() as SOCKET, event, (FD_READ | FD_WRITE | FD_CLOSE) as i32) != 0 {
                    WSACloseEvent(event);
                    return None;
                }
                let typed = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
                if typed.is_null() {
                    WSACloseEvent(event);
                    return None;
                }
                Some((Waiter { socket: event, typed }, Event(typed as usize)))
            }
        }

        pub fn wait(&self) {
            // (A WSAEVENT is an event's handle.)
            let handles = [self.socket as HANDLE, self.typed];
            // SAFETY: two live event handles; returns on either or the time.
            // The socket's event is reset after: what's come is read next,
            // and anything after that read sets it again.
            unsafe {
                WaitForMultipleObjects(2, handles.as_ptr(), 0, LOOK.as_millis() as u32);
                WSAResetEvent(self.socket);
            }
        }
    }

    impl Drop for Waiter {
        fn drop(&mut self) {
            // (`typed` stays: the typing thread may still set it.)
            // SAFETY: the socket's event, made in `new`, closed once.
            unsafe { WSACloseEvent(self.socket) };
        }
    }
}

/// What the shell wrote, as text for a console, which takes text, not
/// bytes: a character a read ends in the middle of waits for its rest, and
/// bytes that aren't UTF-8 read as U+FFFD (where writing them would fail
/// the whole write).
#[cfg(any(windows, test))]
#[derive(Default)]
struct Utf8Text {
    carry: Vec<u8>,
}

#[cfg(any(windows, test))]
impl Utf8Text {
    fn push(&mut self, bytes: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(bytes);
        let mut out = String::with_capacity(data.len());
        let mut rest = &data[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    return out;
                }
                Err(error) => {
                    let (good, bad) = rest.split_at(error.valid_up_to());
                    out.push_str(std::str::from_utf8(good).unwrap_or_default());
                    match error.error_len() {
                        Some(n) => {
                            out.push('\u{FFFD}');
                            rest = &bad[n..];
                        }
                        None => {
                            self.carry = bad.to_vec();
                            return out;
                        }
                    }
                }
            }
        }
    }
}

/// What the shell wrote, without its pseudoconsole's asks for
/// win32-input-mode (`CSI ? 9001 h`, and `l`). On Windows the engine runs
/// the shell in a pseudoconsole of its own, which asks its terminal for keys
/// as win32 input records; written on to the bridge's console, the ask
/// would switch zgx's pseudoconsole into that mode too, which then takes
/// zgx's VT keys apart (Up's `ESC [ A` reached the shell as Escape, `[`, `A`).
/// Kept from the bridge's console, keys go on as VT, which the engine's
/// pseudoconsole reads as well. An ask split between reads waits for its rest.
#[cfg(any(windows, test))]
#[derive(Default)]
struct NoWin32Input {
    carry: String,
}

#[cfg(any(windows, test))]
impl NoWin32Input {
    const ASKS: [&'static str; 2] = ["\x1b[?9001h", "\x1b[?9001l"];

    fn pass(&mut self, text: &str) -> String {
        let mut out = std::mem::take(&mut self.carry);
        out.push_str(text);
        for ask in Self::ASKS {
            if out.contains(ask) {
                out = out.replace(ask, "");
            }
        }
        if let Some(at) = out.rfind('\x1b') {
            let tail = &out[at..];
            if Self::ASKS.iter().any(|ask| ask.len() > tail.len() && ask.starts_with(tail)) {
                self.carry = tail.to_owned();
                out.truncate(at);
            }
        }
        out
    }
}

/// Sends a call. On a socket that doesn't block, what doesn't go at once
/// waits in tungstenite's buffer (`rpc::would_block`) for the socket to
/// have room.
fn send(socket: &mut Socket, id: u64, method: &str, mut params: Map<String, Json>, device: &Option<String>) -> tungstenite::Result<()> {
    if let Some(device) = device {
        params.insert("targetDeviceId".into(), Json::String(device.clone()));
    }
    let frame = json!({ "id": id, "method": method, "params": Json::Object(params) });
    socket.send(tungstenite::Message::text(frame.to_string()))
}

/// Whether a send failed for good (not merely waiting for room).
fn failed(sent: tungstenite::Result<()>) -> bool {
    sent.is_err_and(|e| !rpc::would_block(&e))
}

/// The write end of the pipe SIGWINCH is told down (-1: none).
#[cfg(unix)]
static WINCH: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

#[cfg(unix)]
extern "C" fn on_winch(_: libc::c_int) {
    let fd = WINCH.load(std::sync::atomic::Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write is async-signal-safe; one byte from a static buffer
        // to the pipe's non-blocking write end, which lives as long as the
        // process.
        unsafe {
            libc::write(fd, [1u8].as_ptr().cast(), 1);
        }
    }
}

/// A pipe a byte goes down each time zgx resizes this pseudoterminal
/// (SIGWINCH), for the bridge to wait on: its read end.
#[cfg(unix)]
fn resizes() -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::IntoRawFd;
    let (heard, told) = rpc::pipe().ok()?;
    // Kept for the process's life: the handler may write to it any time.
    WINCH.store(told.into_raw_fd(), std::sync::atomic::Ordering::Relaxed);
    // SAFETY: installs a handler that only writes a byte to a pipe.
    unsafe {
        libc::signal(libc::SIGWINCH, on_winch as *const () as libc::sighandler_t);
    }
    Some(heard)
}

/// Runs the bridge; returns the process's exit code.
pub fn bridge(args: &[String]) -> i32 {
    let args = match parse(args) {
        Ok(args) => args,
        Err(why) => {
            eprintln!("terminal-bridge: {why}");
            return 2;
        }
    };
    let token = rpc::read_token(&args.data);
    let mut socket = match dial(args.port, token.as_deref()) {
        Ok(socket) => socket,
        Err(why) => {
            print!("\r\nZeron can't be reached right now ({why}).\r\n");
            let _ = std::io::stdout().flush();
            return 1;
        }
    };
    // (A device given is another's: the app only passes it then.)
    let mut device = args.device.clone();
    #[cfg(unix)]
    let _raw = Raw::new();
    #[cfg(windows)]
    let _raw = console::Raw::new();
    let mut size = None;
    // A new one: opened here, at the size zgx gave this pseudoterminal, so
    // the shell's first prompt is drawn for it; its id left for the app.
    let id = match (&args.terminal, &args.open, &args.claim) {
        (Some(id), _, _) => id.clone(),
        (None, Some(open), Some(claim)) => {
            let now = settled_size();
            match open_terminal(&mut socket, open, now) {
                Ok(id) => {
                    device = open.get("targetDeviceId").and_then(Json::as_str).map(str::to_owned);
                    if !leave_claim(claim, &id) {
                        // (The app stopped waiting: no tab knows this one, so
                        // it ends here rather than run on with no one's.)
                        let mut params = Map::new();
                        params.insert("terminalId".into(), Json::String(id));
                        let _ = send(&mut socket, 1, "CloseTerminal", params, &device);
                        print!("\r\nThe terminal took too long to open. Open another.\r\n");
                        let _ = std::io::stdout().flush();
                        return 1;
                    }
                    size = Some(now);
                    id
                }
                Err(why) => {
                    leave_claim(claim, &format!("error: {why}"));
                    let shown = if why == "offline" { "Zeron can't reach this session's device right now.".to_owned() } else { why };
                    print!("\r\n{shown}\r\n");
                    let _ = std::io::stdout().flush();
                    return 1;
                }
            }
        }
        _ => return 2,
    };
    let terminal = Json::String(id);
    let mut params = Map::new();
    params.insert("terminalId".into(), terminal.clone());
    // (zgx's terminal answers queries itself: the host mustn't too.)
    params.insert("hostReplies".into(), Json::Bool(false));
    if failed(send(&mut socket, 1, "SubscribeTerminal", params, &device)) {
        return 1;
    }
    let mut next_id = 2u64;
    // What's typed: read as the pseudoterminal has it (unix), else on a
    // thread of its own (Windows), which wakes the bridge.
    #[cfg(unix)]
    let Some(resized) = resizes() else { return 1 };
    let _ = socket.get_ref().set_nonblocking(true);
    #[cfg(windows)]
    let Some((waiter, wake)) = console::Waiter::new(socket.get_ref()) else { return 1 };
    #[cfg(windows)]
    let rx: mpsc::Receiver<Vec<u8>> = console::typed(wake);
    #[cfg(windows)]
    let (mut shown, mut keys_as_vt) = (Utf8Text::default(), NoWin32Input::default());
    let mut stdout = std::io::stdout();
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut typed: Vec<Vec<u8>> = Vec::new();
    let mut look_at_size = true;
    loop {
        // Its size, told as it changes (zgx resizes its pseudoterminal, and
        // says so with SIGWINCH; on Windows, looked at on every wake).
        let now = if look_at_size { window_size() } else { size };
        look_at_size = cfg!(windows);
        if now.is_some() && now != size {
            size = now;
            let (cols, rows) = now.unwrap_or((80, 24));
            let mut p = Map::new();
            p.insert("terminalId".into(), terminal.clone());
            p.insert("cols".into(), json!(cols));
            p.insert("rows".into(), json!(rows));
            if failed(send(&mut socket, next_id, "ResizeTerminal", p, &device)) {
                return 1;
            }
            next_id += 1;
        }
        // What's typed, on.
        #[cfg(windows)]
        loop {
            match rx.try_recv() {
                Ok(bytes) => typed.push(bytes),
                Err(mpsc::TryRecvError::Empty) => break,
                // Its terminal closed (the tab): done.
                Err(mpsc::TryRecvError::Disconnected) => return 0,
            }
        }
        for bytes in typed.drain(..) {
            let mut p = Map::new();
            p.insert("terminalId".into(), terminal.clone());
            p.insert("data".into(), Json::String(b64.encode(bytes)));
            if failed(send(&mut socket, next_id, "WriteTerminal", p, &device)) {
                return 1;
            }
            next_id += 1;
        }
        // What didn't go yet waits for the socket to have room.
        let unsent = match socket.flush() {
            Ok(()) => false,
            Err(e) if rpc::would_block(&e) => true,
            Err(_) => return 1,
        };
        // What the shell wrote, out: all there is.
        loop {
            match socket.read() {
                Ok(tungstenite::Message::Text(frame)) => {
                    for line in frame.as_str().lines().filter(|l| !l.trim().is_empty()) {
                        let Ok(frame) = serde_json::from_str::<Json>(line) else {
                            continue;
                        };
                        if frame["id"].as_u64() != Some(1) {
                            continue;
                        }
                        if let Some(error) = frame["err"].as_str() {
                            print!("\r\nThis terminal isn't there any more ({error}).\r\n");
                            let _ = stdout.flush();
                            return 1;
                        }
                        if frame.get("done").is_some() {
                            return 0;
                        }
                        let item = &frame["item"];
                        match item["type"].as_str() {
                            Some("data") => {
                                if let Some(bytes) = item["data"].as_str().and_then(|d| b64.decode(d).ok()) {
                                    #[cfg(windows)]
                                    let bytes = keys_as_vt.pass(&shown.push(&bytes)).into_bytes();
                                    let _ = stdout.write_all(&bytes);
                                    let _ = stdout.flush();
                                }
                            }
                            Some("exit") => {
                                return item["exitCode"].as_i64().unwrap_or(0) as i32;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(tungstenite::Message::Close(_)) => return 1,
                Ok(_) => {}
                Err(e) if rpc::would_block(&e) => break,
                Err(_) => {
                    print!("\r\nZeron's connection ended.\r\n");
                    let _ = stdout.flush();
                    return 1;
                }
            }
        }
        // Asleep until one of them has something.
        #[cfg(unix)]
        {
            let out = if unsent { libc::POLLOUT } else { 0 };
            let stdin = std::io::stdin();
            let mut fds = [
                rpc::poll_fd(socket.get_ref(), libc::POLLIN | out),
                rpc::poll_fd(&stdin, libc::POLLIN),
                rpc::poll_fd(&resized, libc::POLLIN),
            ];
            rpc::wait(&mut fds);
            if fds[2].revents != 0 {
                rpc::drain(&resized);
                look_at_size = true;
            }
            if fds[1].revents != 0 {
                let mut buf = [0u8; 4096];
                // SAFETY: reads into a buffer of its own length from fd 0,
                // which poll says has something (or has closed).
                let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
                if n > 0 {
                    typed.push(buf[..n as usize].to_vec());
                } else if n == 0 || !matches!(std::io::Error::last_os_error().kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) {
                    // Its terminal closed (the tab): done.
                    return 0;
                }
            }
        }
        // (The socket's room is among what it waits for: `unsent` or not.)
        #[cfg(windows)]
        {
            let _ = unsent;
            waiter.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::quote;
    use super::{parse, said};

    #[test]
    fn a_claim_changes_hands_once_and_a_late_one_is_never_left_with_no_one() {
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("zgx-claims-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The bridge claims in time: the app reads its id, and the claim goes.
        let claim = dir.join("one");
        assert!(super::leave_claim(&claim, "term-7"));
        assert_eq!(super::await_claim(&claim, Duration::from_millis(200)), Ok("term-7".into()));
        assert!(!claim.exists());
        // It couldn't open one: why, passed on.
        let claim = dir.join("two");
        assert!(super::leave_claim(&claim, "error: offline"));
        assert_eq!(super::await_claim(&claim, Duration::from_millis(200)), Err("offline".into()));
        // No claim in time: the app stops waiting, and a bridge claiming
        // after is told so (and closes its terminal); nothing is left.
        let claim = dir.join("three");
        assert!(super::await_claim(&claim, Duration::from_millis(100)).is_err());
        assert!(!super::leave_claim(&claim, "term-9"), "a late claim must fail");
        assert!(!claim.exists(), "the claim is cleared");
        assert!(!claim.with_extension("part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_engine_says_to_update() {
        assert_eq!(said("unknown method: TerminalStatus".into()), "Update Zeron to use terminals here.");
        assert_eq!(said("no such terminal".into()), "no such terminal");
    }

    /// A Windows command line's words as the C runtime (and Rust's
    /// `std::env::args`) reads them back, for the arguments after the
    /// program: the rules `quote_windows` writes for.
    fn split_windows(line: &str) -> Vec<String> {
        let (mut words, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    let mut n = 1;
                    while chars.peek() == Some(&'\\') {
                        chars.next();
                        n += 1;
                    }
                    if chars.peek() == Some(&'"') {
                        word.extend(std::iter::repeat_n('\\', n / 2));
                        if n % 2 == 1 {
                            chars.next();
                            word.push('"');
                        }
                    } else {
                        word.extend(std::iter::repeat_n('\\', n));
                    }
                    any = true;
                }
                '"' => {
                    quoted = !quoted;
                    any = true;
                }
                ' ' | '\t' if !quoted => {
                    if any {
                        words.push(std::mem::take(&mut word));
                        any = false;
                    }
                }
                c => {
                    word.push(c);
                    any = true;
                }
            }
        }
        if any {
            words.push(word);
        }
        words
    }

    #[test]
    fn windows_words_read_back_as_they_were() {
        use super::quote_windows;
        let words = [
            r"C:\Users\Wing Lee\AppData\Local\Temp\zeron-zgx-terminals\12-34",
            r#"{"chatId":"c1","cwd":"C:\\Users\\me\\my project\\"}"#,
            r"ends in a backslash\",
            r#"a "quoted" \"word\""#,
            r"\\server\share\x",
            "",
            "plain",
            "it's",
        ];
        let line = words.iter().map(|w| quote_windows(w)).collect::<Vec<_>>().join(" ");
        assert_eq!(split_windows(&line), words);
        assert_eq!(quote_windows(r"C:\Program Files\zeron-zgx.exe"), r#""C:\Program Files\zeron-zgx.exe""#);
        assert_eq!(quote_windows(r"a\"), r#""a\\""#);
        assert_eq!(quote_windows(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn a_consoles_text_is_whole_characters() {
        let mut text = super::Utf8Text::default();
        // "é" (C3 A9) split across reads: the first half waits.
        assert_eq!(text.push(b"caf\xC3"), "caf");
        assert_eq!(text.push(b"\xA9 ok"), "\u{e9} ok");
        // Bytes that aren't UTF-8 read as U+FFFD, and the rest goes on.
        assert_eq!(text.push(b"a\xFFb"), "a\u{FFFD}b");
        // A four-byte character in three reads.
        assert_eq!(text.push(b"\xF0\x9F"), "");
        assert_eq!(text.push(b"\x98"), "");
        assert_eq!(text.push(b"\x80!"), "\u{1F600}!");
    }

    #[test]
    fn the_shells_ask_for_win32_input_never_reaches_the_bridges_console() {
        let mut out = super::NoWin32Input::default();
        // A pseudoconsole's start, as the engine's sends it.
        assert_eq!(out.pass("\x1b[?9001h\x1b[?1004hC:\\> "), "\x1b[?1004hC:\\> ");
        // Split between reads: held, then dropped whole.
        assert_eq!(out.pass("ok\x1b[?90"), "ok");
        assert_eq!(out.pass("01l done"), " done");
        // Another sequence that starts the same way goes on, once it's told apart.
        assert_eq!(out.pass("\x1b[?9"), "");
        assert_eq!(out.pass("h\x1b[2J"), "\x1b[?9h\x1b[2J");
        // A lone escape at the end of a read isn't held for long: the next read
        // brings it out.
        assert_eq!(out.pass("a\x1b"), "a");
        assert_eq!(out.pass("[0m"), "\x1b[0m");
    }

    #[cfg(unix)]
    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(quote("a b'c"), "'a b'\\''c'");
    }

    #[test]
    fn quotes_and_parses_the_bridge_line() {
        let args: Vec<String> = ["--port", "27897", "--data", "/tmp/d", "--terminal", "t1", "--device", "dev"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed = parse(&args).unwrap();
        assert_eq!(parsed.port, 27897);
        assert_eq!(parsed.terminal.as_deref(), Some("t1"));
        assert_eq!(parsed.device.as_deref(), Some("dev"));
        assert!(parse(&["--port".to_string()]).is_err());
        // A new one: what to open and where to leave its id.
        let args: Vec<String> = ["--port", "1", "--data", "/d", "--open", r#"{"chatId":"c1"}"#, "--claim", "/tmp/x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed = parse(&args).unwrap();
        assert!(parsed.terminal.is_none());
        assert_eq!(parsed.open.unwrap()["chatId"], "c1");
        let args: Vec<String> = ["--port", "1", "--data", "/d", "--open", "{}"].iter().map(|s| s.to_string()).collect();
        assert!(parse(&args).is_err(), "a new one needs somewhere to leave its id");
    }
}
