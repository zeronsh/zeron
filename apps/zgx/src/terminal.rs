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
//! (`target` as src/files.rs's: { chat?, space?, device?, root? }).

use std::{
    io::Write,
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    time::Duration,
};
#[cfg(not(unix))]
use std::{io::Read, sync::mpsc};

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
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
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
#[cfg(not(unix))]
fn window_size() -> Option<(u16, u16)> {
    None
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
    // thread of its own.
    #[cfg(unix)]
    let Some(resized) = resizes() else { return 1 };
    #[cfg(unix)]
    let _ = socket.get_ref().set_nonblocking(true);
    #[cfg(not(unix))]
    let rx = {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        rx
    };
    let mut stdout = std::io::stdout();
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut typed: Vec<Vec<u8>> = Vec::new();
    let mut look_at_size = true;
    loop {
        // Its size, told as it changes (zgx resizes its pseudoterminal, and
        // says so with SIGWINCH).
        let now = if look_at_size { window_size() } else { size };
        look_at_size = cfg!(not(unix));
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
        #[cfg(not(unix))]
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
        #[cfg(not(unix))]
        let _ = unsent;
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, quote, said};

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

    #[test]
    fn quotes_and_parses_the_bridge_line() {
        assert_eq!(quote("a b'c"), "'a b'\\''c'");
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
