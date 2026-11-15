//! Terminal tools: a shell on this device or any other of the person's
//! online devices (the engine's `OpenTerminal` … `CloseTerminal`, sent to
//! another device by `targetDeviceId` as the app's terminal panel does).
//!
//! This server follows each terminal it opens (or is pointed at) with a
//! `SubscribeTerminal` stream, folding its output into an emulator so a tool
//! can return both what was printed since it last looked (escape sequences
//! taken out) and the screen as a person would see it (what a full-screen
//! program like `top` or `vim` shows). The engine lets an agent do this only
//! from a chat with full access and no sandbox (rpc/agent_gate.rs): a shell
//! is neither.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use zeron_proto::TerminalEvent;
use zeron_rpc::methods;
use zeron_term::emulator::Emulator;

use super::{ToolDef, Tools};
use crate::zeron::Zeron;

pub(super) const OPEN_TERMINAL: &str = "open_terminal";
pub(super) const WRITE_TERMINAL: &str = "write_terminal";
pub(super) const READ_TERMINAL: &str = "read_terminal";
pub(super) const CLOSE_TERMINAL: &str = "close_terminal";

pub(super) const NAMES: [&str; 4] = [OPEN_TERMINAL, WRITE_TERMINAL, READ_TERMINAL, CLOSE_TERMINAL];

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 40;
/// How long a write waits for output by default, and the most any wait may.
const DEFAULT_WRITE_WAIT: Duration = Duration::from_secs(2);
const MAX_WAIT: Duration = Duration::from_secs(300);
/// Output quiet this long after it last changed reads as settled.
const QUIET: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_millis(50);
/// The most unread output kept (the newest); older is counted, not kept.
const MAX_UNREAD: usize = 64 * 1024;
/// The most output one reply carries (the newest).
const MAX_REPLY_OUTPUT: usize = 16 * 1024;
/// Resubscribe attempts in a row before a terminal reads as lost.
const MAX_RESUBSCRIBES: u32 = 5;

fn terminal_schema() -> Value {
    json!({ "type": "string", "description": "Terminal id, from open_terminal." })
}

fn device_schema() -> Value {
    json!({ "type": "string", "description": "The terminal's device (id or exact name). Only needed for a terminal this server didn't open (one the person opened, or one from before a restart): one it opened, it remembers." })
}

fn wait_schema(default_ms: u64) -> Value {
    json!({
        "type": "integer", "minimum": 0, "maximum": MAX_WAIT.as_millis() as u64, "default": default_ms,
        "description": "Wait up to this many milliseconds for output: the reply comes once output has been quiet for half a second, `until` appears, or the shell exits."
    })
}

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: OPEN_TERMINAL,
            description: "Open a shell on any of the person's online devices: this one when `device` is omitted, or another (see list_devices; a Cloud machine must be awake). Returns the terminal's id, its device, folder and shell. Type into it with write_terminal, read it with read_terminal, and close it with close_terminal when you're done. It runs as the person, with no sandbox, so only a chat with full access may open one.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string", "description": "Device id or exact name. Omit for this device." },
                    "cwd": { "type": "string", "description": "Folder to start in, on that device (default: your chat's folder on its own device, else home)." },
                    "cols": { "type": "integer", "minimum": 20, "maximum": 500, "default": DEFAULT_COLS },
                    "rows": { "type": "integer", "minimum": 5, "maximum": 200, "default": DEFAULT_ROWS }
                }
            }),
        },
        ToolDef {
            name: WRITE_TERMINAL,
            description: "Type into a terminal, then wait for what it prints. `input` is sent as typed, then Enter unless `enter` is false: send a command as `input`, or keys with `enter: false` (\\u0003 is Ctrl-C, \\u0004 Ctrl-D, \\u001b Esc, \\u001b[A the Up arrow). Returns `output` (what it printed since you last read it, escape sequences removed), `busy` (a program other than the shell is running), `exited` once the shell has ended, and `screen` (the visible screen, as text) when asked for or while a full-screen program shows. For a long command, pass `until` (text its end prints, such as a prompt) and a longer `wait_ms`, or read_terminal again later.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "terminal": terminal_schema(),
                    "input": { "type": "string", "description": "What to type." },
                    "enter": { "type": "boolean", "default": true, "description": "Press Enter after the input." },
                    "wait_ms": wait_schema(DEFAULT_WRITE_WAIT.as_millis() as u64),
                    "until": { "type": "string", "description": "Return as soon as the output since this call contains this text." },
                    "screen": { "type": "boolean", "default": false, "description": "Include the visible screen." },
                    "device": device_schema()
                },
                "required": ["terminal", "input"]
            }),
        },
        ToolDef {
            name: READ_TERMINAL,
            description: "What a terminal printed since you last read it (escape sequences removed), whether a program is still running in it (`busy`) or the shell has exited, and optionally the visible screen. Pass `wait_ms` to wait for more output first. Works on a terminal the person opened too, given its id and device.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "terminal": terminal_schema(),
                    "wait_ms": wait_schema(0),
                    "until": { "type": "string", "description": "With wait_ms: return as soon as the unread output contains this text." },
                    "screen": { "type": "boolean", "default": false, "description": "Include the visible screen." },
                    "device": device_schema()
                },
                "required": ["terminal"]
            }),
        },
        ToolDef {
            name: CLOSE_TERMINAL,
            description: "Close a terminal, ending its shell and anything running in it.",
            input_schema: json!({
                "type": "object",
                "properties": { "terminal": terminal_schema(), "device": device_schema() },
                "required": ["terminal"]
            }),
        },
    ]
}

/// The terminals this server follows, by id.
#[derive(Default)]
pub(super) struct TerminalState {
    followed: tokio::sync::Mutex<HashMap<String, Arc<Followed>>>,
}

/// One terminal, as this server follows it.
struct Followed {
    id: String,
    /// `None`: this engine's own device.
    target: Option<String>,
    device: Value,
    screen: Mutex<Screen>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for Followed {
    fn drop(&mut self) {
        if let Some(task) = self.task.lock().ok().and_then(|mut t| t.take()) {
            task.abort();
        }
    }
}

struct Screen {
    emulator: Emulator,
    /// Output not yet returned, escape sequences removed.
    unread: String,
    /// Characters dropped from the front of `unread` past [`MAX_UNREAD`].
    dropped: usize,
    /// Bumped by every output chunk (a wait's "since this call" mark).
    total: u64,
    last_seq: u64,
    last_output: Option<Instant>,
    exit: Option<Value>,
    /// Why the stream was lost, when it was.
    lost: Option<String>,
    /// The host leaves terminal queries to its viewers (an older engine).
    answer_queries: bool,
    /// Stripping state carried between chunks.
    strip: Strip,
}

#[derive(Deserialize)]
struct OpenArgs {
    device: Option<String>,
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Deserialize)]
struct WriteArgs {
    terminal: String,
    input: String,
    #[serde(default = "yes")]
    enter: bool,
    wait_ms: Option<u64>,
    until: Option<String>,
    #[serde(default)]
    screen: bool,
    device: Option<String>,
}

#[derive(Deserialize)]
struct ReadArgs {
    terminal: String,
    wait_ms: Option<u64>,
    until: Option<String>,
    #[serde(default)]
    screen: bool,
    device: Option<String>,
}

#[derive(Deserialize)]
struct CloseArgs {
    terminal: String,
    device: Option<String>,
}

fn yes() -> bool {
    true
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

fn wait_of(ms: Option<u64>, default: Duration) -> Duration {
    ms.map(Duration::from_millis).unwrap_or(default).min(MAX_WAIT)
}

impl Tools {
    pub(super) async fn terminals(&self, tool: &str, args: Value) -> Result<Value, String> {
        let result = match tool {
            OPEN_TERMINAL => self.open_terminal(parse(args)?).await,
            WRITE_TERMINAL => self.write_terminal(parse(args)?).await,
            READ_TERMINAL => self.read_terminal(parse(args)?).await,
            _ => self.close_terminal(parse(args)?).await,
        };
        result.map_err(|e| format!("{e:#}"))
    }

    /// The device to send a call to (`None`: this engine's), and how a reply
    /// names it.
    async fn terminal_device(&self, key: Option<&str>) -> anyhow::Result<(Option<String>, Value)> {
        let (devices, local) = tokio::try_join!(self.zeron.devices(), self.zeron.local_device_id())?;
        let id = self.zeron.resolve_device_id(key).await?;
        let name = devices.iter().find(|d| d.id == id).map(|d| d.name.clone());
        let named = json!({ "id": id, "name": name, "local": id == local });
        Ok(((id != local).then_some(id), named))
    }

    async fn open_terminal(&self, args: OpenArgs) -> anyhow::Result<Value> {
        let (target, device) = self.terminal_device(args.device.as_deref()).await?;
        let cols = args.cols.unwrap_or(DEFAULT_COLS).clamp(20, 500);
        let rows = args.rows.unwrap_or(DEFAULT_ROWS).clamp(5, 200);
        // On its own chat's device, the terminal is the chat's (its folder
        // by default, its activity); elsewhere that folder means nothing.
        let origin = self.zeron.origin();
        let own_device = match (&origin.device_id, &target) {
            (_, None) => true,
            (Some(here), Some(there)) => here == there,
            (None, Some(_)) => false,
        };
        let chat_id = match origin.chat_id.as_deref() {
            Some(chat) if own_device => chat.to_owned(),
            Some(chat) => format!("agent-terminal:{chat}"),
            None => "agent-terminal".to_owned(),
        };
        let mut params = json!({ "chatId": chat_id, "cols": cols, "rows": rows });
        if let Some(cwd) = args.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            params["cwd"] = json!(cwd);
        }
        if let Some(target) = &target {
            params["targetDeviceId"] = json!(target);
        }
        let session: zeron_proto::TerminalSession =
            serde_json::from_value(self.zeron.call_rpc(methods::OPEN_TERMINAL, params).await.map_err(said)?)?;
        let followed = self
            .follow(&session.id, target, device.clone(), cols, rows, !session.host_replies)
            .await;
        // The prompt, usually.
        let mut reply = self.settled(&followed, Duration::from_secs(1), None, false).await;
        reply["cwd"] = json!(session.cwd);
        reply["shell"] = json!(session.shell);
        Ok(reply)
    }

    async fn write_terminal(&self, args: WriteArgs) -> anyhow::Result<Value> {
        let followed = self.followed(&args.terminal, args.device.as_deref()).await?;
        let mut input = args.input;
        if args.enter {
            input.push('\r');
        }
        anyhow::ensure!(!input.is_empty(), "nothing to type: input is empty and enter is false");
        let wait = wait_of(args.wait_ms, DEFAULT_WRITE_WAIT);
        // Only output after the keys count toward `until`.
        let mark = followed.screen.lock().map(|s| s.unread.len()).unwrap_or(0);
        let mut params = json!({
            "terminalId": followed.id,
            "data": base64::engine::general_purpose::STANDARD.encode(input.as_bytes()),
        });
        if let Some(target) = &followed.target {
            params["targetDeviceId"] = json!(target);
        }
        self.zeron.call_rpc(methods::WRITE_TERMINAL, params).await.map_err(said)?;
        let until = args.until.as_deref().filter(|u| !u.is_empty()).map(|u| (u, mark));
        Ok(self.settled(&followed, wait, until, args.screen).await)
    }

    async fn read_terminal(&self, args: ReadArgs) -> anyhow::Result<Value> {
        let followed = self.followed(&args.terminal, args.device.as_deref()).await?;
        let wait = wait_of(args.wait_ms, Duration::ZERO);
        let until = args.until.as_deref().filter(|u| !u.is_empty()).map(|u| (u, 0));
        Ok(self.settled(&followed, wait, until, args.screen).await)
    }

    async fn close_terminal(&self, args: CloseArgs) -> anyhow::Result<Value> {
        let known = self.terminal_state.followed.lock().await.remove(&args.terminal);
        let target = match &known {
            Some(followed) => followed.target.clone(),
            None => self.terminal_device(args.device.as_deref()).await?.0,
        };
        let mut params = json!({ "terminalId": args.terminal });
        if let Some(target) = &target {
            params["targetDeviceId"] = json!(target);
        }
        self.zeron.call_rpc(methods::CLOSE_TERMINAL, params).await.map_err(said)?;
        Ok(json!({ "terminal": args.terminal, "closed": true }))
    }

    /// A terminal this server follows, or — named with its device — one it
    /// starts following now (its output so far replays from the start).
    async fn followed(&self, id: &str, device: Option<&str>) -> anyhow::Result<Arc<Followed>> {
        if let Some(followed) = self.terminal_state.followed.lock().await.get(id) {
            return Ok(followed.clone());
        }
        let (target, named) = self.terminal_device(device).await?;
        // Make sure it's there before following it.
        let mut params = json!({ "terminalId": id });
        if let Some(target) = &target {
            params["targetDeviceId"] = json!(target);
        }
        self.zeron.call_rpc(methods::TERMINAL_STATUS, params).await.map_err(|e| {
            anyhow::anyhow!(
                "no terminal {id} on {}: {} (open one with open_terminal; for one this server didn't open, pass its device)",
                named["name"].as_str().unwrap_or("this device"),
                said(e)
            )
        })?;
        Ok(self
            .follow(id, target, named, DEFAULT_COLS, DEFAULT_ROWS, false)
            .await)
    }

    async fn follow(
        &self,
        id: &str,
        target: Option<String>,
        device: Value,
        cols: u16,
        rows: u16,
        answer_queries: bool,
    ) -> Arc<Followed> {
        let followed = Arc::new(Followed {
            id: id.to_owned(),
            target,
            device,
            screen: Mutex::new(Screen {
                emulator: Emulator::new(cols, rows),
                unread: String::new(),
                dropped: 0,
                total: 0,
                last_seq: 0,
                last_output: None,
                exit: None,
                lost: None,
                answer_queries,
                strip: Strip::default(),
            }),
            task: Mutex::new(None),
        });
        let task = tokio::spawn(stream_into(self.zeron.clone(), Arc::downgrade(&followed)));
        if let Ok(mut slot) = followed.task.lock() {
            *slot = Some(task);
        }
        self.terminal_state
            .followed
            .lock()
            .await
            .insert(id.to_owned(), followed.clone());
        followed
    }

    /// Wait as asked, then take the unread output and say where the terminal
    /// stands.
    async fn settled(&self, followed: &Followed, wait: Duration, until: Option<(&str, usize)>, screen: bool) -> Value {
        let start = Instant::now();
        let deadline = start + wait;
        let first_total = followed.screen.lock().map(|s| s.total).unwrap_or(0);
        while Instant::now() < deadline {
            let done = match followed.screen.lock() {
                Ok(s) => {
                    s.exit.is_some()
                        || s.lost.is_some()
                        || match until {
                            Some((text, mark)) => s.unread.get(mark.min(s.unread.len())..).is_some_and(|new| new.contains(text)),
                            None => s.total != first_total && s.last_output.is_some_and(|at| at.elapsed() >= QUIET),
                        }
                }
                Err(_) => true,
            };
            if done {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        let busy = self.busy(followed).await;
        let Ok(mut s) = followed.screen.lock() else {
            return json!({ "terminal": followed.id, "error": "this terminal's state was lost" });
        };
        let unread = std::mem::take(&mut s.unread);
        let dropped = std::mem::take(&mut s.dropped);
        let (output, cut) = newest(&unread, MAX_REPLY_OUTPUT);
        let mut reply = json!({
            "terminal": followed.id,
            "device": followed.device,
            "output": output,
        });
        if dropped + cut > 0 {
            reply["outputDropped"] = json!(format!("{} earlier characters not shown", dropped + cut));
        }
        if let Some(busy) = busy {
            reply["busy"] = json!(busy);
        }
        if let Some(exit) = &s.exit {
            reply["exited"] = exit.clone();
        }
        if let Some(lost) = &s.lost {
            reply["lost"] = json!(lost);
        }
        if let Some((text, _)) = until
            && !output.contains(text)
            && s.exit.is_none()
        {
            reply["untilSeen"] = json!(false);
        }
        if screen || s.emulator.alt_screen() {
            reply["screen"] = json!(screen_text(&s.emulator));
            if let Some(cursor) = s.emulator.cursor() {
                reply["cursor"] = json!({ "row": cursor.row, "col": cursor.col });
            }
        }
        reply
    }

    /// Whether a program other than the shell runs in it; `None` if the
    /// device doesn't say in time.
    async fn busy(&self, followed: &Followed) -> Option<bool> {
        let mut params = json!({ "terminalId": followed.id });
        if let Some(target) = &followed.target {
            params["targetDeviceId"] = json!(target);
        }
        let status = tokio::time::timeout(
            Duration::from_secs(3),
            self.zeron.call_rpc(methods::TERMINAL_STATUS, params),
        )
        .await
        .ok()?
        .ok()?;
        status["busy"].as_bool()
    }
}

/// An engine error in its own words.
fn said(error: zeron_rpc::RpcError) -> anyhow::Error {
    match error {
        zeron_rpc::RpcError::Failed(message) => anyhow::anyhow!(message),
        other => anyhow::anyhow!("{other}"),
    }
}

/// Follow `followed`'s output until it exits, it's dropped, or the stream
/// can't be had back.
async fn stream_into(zeron: Arc<Zeron>, followed: std::sync::Weak<Followed>) {
    let mut failures = 0u32;
    loop {
        let Some(this) = followed.upgrade() else { return };
        let after = this.screen.lock().map(|s| s.last_seq).unwrap_or(0);
        let mut params = json!({ "terminalId": this.id, "afterSeq": after, "hostReplies": true });
        if let Some(target) = &this.target {
            params["targetDeviceId"] = json!(target);
        }
        drop(this);
        match zeron.subscribe(methods::SUBSCRIBE_TERMINAL, params).await {
            Ok(mut subscription) => {
                failures = 0;
                while let Some(item) = subscription.recv().await {
                    let Some(this) = followed.upgrade() else { return };
                    let Ok(event) = serde_json::from_value::<TerminalEvent>(item) else { continue };
                    let replies = this.screen.lock().map(|mut s| s.take(event)).unwrap_or_default();
                    if !replies.is_empty() {
                        let mut params = json!({
                            "terminalId": this.id,
                            "data": base64::engine::general_purpose::STANDARD.encode(&replies),
                        });
                        if let Some(target) = &this.target {
                            params["targetDeviceId"] = json!(target);
                        }
                        let _ = zeron.call_rpc(methods::WRITE_TERMINAL, params).await;
                    }
                    if this.screen.lock().is_ok_and(|s| s.exit.is_some()) {
                        return;
                    }
                }
            }
            Err(error) => {
                failures += 1;
                if failures >= MAX_RESUBSCRIBES {
                    if let Some(this) = followed.upgrade()
                        && let Ok(mut s) = this.screen.lock()
                    {
                        s.lost = Some(format!("can't follow this terminal any more: {error:#}"));
                    }
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(300) * (failures + 1)).await;
    }
}

impl Screen {
    /// Fold one stream event in; returns what the emulator answers back
    /// (only for a host that leaves queries to its viewers).
    fn take(&mut self, event: TerminalEvent) -> Vec<u8> {
        match event {
            TerminalEvent::Data { seq, data } => {
                if seq <= self.last_seq && self.last_seq != 0 {
                    return Vec::new();
                }
                self.last_seq = seq;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .unwrap_or_else(|_| data.into_bytes());
                let replies = self.emulator.feed(&bytes);
                self.strip.push(&bytes, &mut self.unread);
                if self.unread.len() > MAX_UNREAD {
                    let mut cut = self.unread.len() - MAX_UNREAD;
                    while !self.unread.is_char_boundary(cut) {
                        cut += 1;
                    }
                    self.dropped += self.unread[..cut].chars().count();
                    self.unread.drain(..cut);
                }
                self.total += 1;
                self.last_output = Some(Instant::now());
                if self.answer_queries { replies } else { Vec::new() }
            }
            TerminalEvent::Exit { seq, exit_code, signal } => {
                self.last_seq = seq;
                self.exit = Some(json!({ "code": exit_code, "signal": signal }));
                Vec::new()
            }
        }
    }
}

/// The newest `max` bytes of `text` (on a character boundary), and how many
/// characters were left off.
fn newest(text: &str, max: usize) -> (String, usize) {
    if text.len() <= max {
        return (text.to_owned(), 0);
    }
    let mut cut = text.len() - max;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    (text[cut..].to_owned(), text[..cut].chars().count())
}

/// The visible screen as text: trailing blanks off each row, blank rows off
/// the bottom.
fn screen_text(emulator: &Emulator) -> String {
    let mut rows: Vec<String> = emulator
        .lines()
        .into_iter()
        .map(|row| {
            let line: String = row.iter().filter(|cell| !cell.wide_spacer).map(|cell| cell.ch).collect();
            line.trim_end().to_owned()
        })
        .collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows.join("\n")
}

/// Takes escape sequences and control characters out of a terminal's output
/// as it streams (a sequence may span chunks), keeping text, newlines and
/// tabs. A carriage return that doesn't start a newline drops what it would
/// overwrite on that line (progress bars keep their last state).
#[derive(Default)]
struct Strip {
    state: StripState,
    /// A UTF-8 character split across chunks.
    partial: Vec<u8>,
    /// A `\r` not yet known to be part of `\r\n`.
    pending_cr: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum StripState {
    #[default]
    Text,
    Escape,
    /// An escape that takes one more character (`ESC ( B`, a charset).
    EscapeArgument,
    /// CSI: parameters until a final byte.
    Csi,
    /// OSC, DCS, SOS, PM, APC: a string until BEL or ST.
    String,
    StringEscape,
}

impl Strip {
    fn push(&mut self, bytes: &[u8], out: &mut String) {
        let mut bytes_in = std::mem::take(&mut self.partial);
        bytes_in.extend_from_slice(bytes);
        let text = match std::str::from_utf8(&bytes_in) {
            Ok(text) => text.to_owned(),
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                self.partial = bytes_in[valid..].to_vec();
                String::from_utf8_lossy(&bytes_in[..valid]).into_owned()
            }
            Err(_) => String::from_utf8_lossy(&bytes_in).into_owned(),
        };
        for ch in text.chars() {
            self.state = match self.state {
                StripState::Text => match ch {
                    '\u{1b}' => StripState::Escape,
                    _ => {
                        self.text(ch, out);
                        StripState::Text
                    }
                },
                StripState::Escape => match ch {
                    '[' => StripState::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => StripState::String,
                    '(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%' | ' ' => StripState::EscapeArgument,
                    _ => StripState::Text,
                },
                StripState::EscapeArgument => StripState::Text,
                StripState::Csi => match ch {
                    '\u{40}'..='\u{7e}' => StripState::Text,
                    _ => StripState::Csi,
                },
                StripState::String => match ch {
                    '\u{7}' => StripState::Text,
                    '\u{1b}' => StripState::StringEscape,
                    _ => StripState::String,
                },
                StripState::StringEscape => match ch {
                    '\\' => StripState::Text,
                    _ => StripState::String,
                },
            };
        }
    }

    fn text(&mut self, ch: char, out: &mut String) {
        if self.pending_cr {
            self.pending_cr = false;
            if ch != '\n' {
                // Back to the start of the line: what follows overwrites it.
                let start = out.rfind('\n').map_or(0, |i| i + 1);
                out.truncate(start);
            }
        }
        match ch {
            '\r' => self.pending_cr = true,
            '\n' | '\t' => out.push(ch),
            '\u{8}' => {
                if !out.ends_with('\n') {
                    out.pop();
                }
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(chunks: &[&[u8]]) -> String {
        let mut strip = Strip::default();
        let mut out = String::new();
        for chunk in chunks {
            strip.push(chunk, &mut out);
        }
        out
    }

    #[test]
    fn output_loses_its_escapes_and_keeps_its_text() {
        assert_eq!(strip(&[b"\x1b[1;32mok\x1b[0m done\r\n"]), "ok done\n");
        // A title (OSC) and a bracketed-paste toggle, split across chunks.
        assert_eq!(strip(&[b"\x1b]0;tit", b"le\x07$ ls\r\n\x1b[?20", b"04hfile\r\n"]), "$ ls\nfile\n");
        // A progress bar redrawn with \r keeps its last state.
        assert_eq!(strip(&[b"10%\r50%\r", b"100%\r\n"]), "100%\n");
        // Backspace, and a UTF-8 character split across chunks.
        assert_eq!(strip(&[b"ab\x08c \xe2\x9c", b"\x93\r\n"]), "ac \u{2713}\n");
        // A charset designation takes its argument with it.
        assert_eq!(strip(&[b"\x1b(Bplain\x1b="]), "plain");
    }

    #[test]
    fn the_newest_output_is_kept() {
        assert_eq!(newest("abcdef", 10), ("abcdef".into(), 0));
        assert_eq!(newest("abcdef", 3), ("def".into(), 3));
        // Never mid-character.
        assert_eq!(newest("a\u{2713}b", 2), ("b".into(), 2));
    }

    #[test]
    fn the_screen_reads_as_a_person_sees_it() {
        let mut screen = Screen {
            emulator: Emulator::new(20, 5),
            unread: String::new(),
            dropped: 0,
            total: 0,
            last_seq: 0,
            last_output: None,
            exit: None,
            lost: None,
            answer_queries: false,
            strip: Strip::default(),
        };
        let data = |seq, text: &str| TerminalEvent::Data {
            seq,
            data: base64::engine::general_purpose::STANDARD.encode(text),
        };
        screen.take(data(1, "$ echo hi\r\nhi\r\n$ "));
        // A replay of what was already seen changes nothing.
        screen.take(data(1, "$ echo hi\r\nhi\r\n$ "));
        assert_eq!(screen.unread, "$ echo hi\nhi\n$ ");
        assert_eq!(screen_text(&screen.emulator), "$ echo hi\nhi\n$");
        screen.take(TerminalEvent::Exit { seq: 2, exit_code: 0, signal: None });
        assert_eq!(screen.exit, Some(json!({ "code": 0, "signal": null })));
    }

    /// A shell on "Worker" (dev-remote) that echoes what's typed, then
    /// prints `out` and a prompt; every call is recorded.
    struct Shell {
        calls: std::sync::Mutex<Vec<(String, Value)>>,
        output: tokio::sync::broadcast::Sender<Value>,
        seq: std::sync::atomic::AtomicU64,
    }

    impl Shell {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Default::default(),
                output: tokio::sync::broadcast::channel(64).0,
                seq: Default::default(),
            })
        }

        fn print(&self, text: &str) {
            let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let _ = self.output.send(json!({
                "type": "data", "seq": seq,
                "data": base64::engine::general_purpose::STANDARD.encode(text),
            }));
        }
    }

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for Shell {
        async fn handle(&self, method: &str, params: Value) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            use futures::StreamExt;
            use zeron_rpc::RpcReply;
            self.calls.lock().unwrap().push((method.to_owned(), params.clone()));
            Ok(match method {
                methods::LOCAL_DEVICE => RpcReply::Value(json!({ "deviceId": "dev-local" })),
                methods::WATCH_DEVICES => RpcReply::Stream(
                    futures::stream::iter(vec![json!([
                        { "id": "dev-local", "name": "Laptop", "platform": "linux", "lastSeenAt": null },
                        { "id": "dev-remote", "name": "Worker", "platform": "linux", "lastSeenAt": null }
                    ])])
                    .boxed(),
                ),
                methods::OPEN_TERMINAL => {
                    self.print("$ ");
                    RpcReply::Value(json!({ "id": "t1", "cwd": "/home/me", "shell": "bash", "hostReplies": true }))
                }
                methods::SUBSCRIBE_TERMINAL => {
                    let rx = self.output.subscribe();
                    RpcReply::Stream(
                        futures::stream::unfold(rx, |mut rx| async move { Some((rx.recv().await.ok()?, rx)) }).boxed(),
                    )
                }
                methods::WRITE_TERMINAL => {
                    let typed = base64::engine::general_purpose::STANDARD
                        .decode(params["data"].as_str().unwrap_or_default())
                        .unwrap();
                    let typed = String::from_utf8(typed).unwrap();
                    self.print(&format!("{}\r\n\x1b[32mout\x1b[0m\r\n$ ", typed.trim_end_matches('\r')));
                    RpcReply::Value(json!({ "ok": true }))
                }
                methods::TERMINAL_STATUS => RpcReply::Value(json!({ "busy": false, "exited": false })),
                methods::CLOSE_TERMINAL => RpcReply::Value(json!({ "ok": true })),
                other => return Err(zeron_rpc::RpcError::UnknownMethod(other.into())),
            })
        }
    }

    #[tokio::test]
    async fn a_terminal_on_another_device_is_opened_typed_into_read_and_closed() {
        let shell = Shell::new();
        let origin = crate::zeron::Origin {
            chat_id: Some("chat-1".into()),
            device_id: Some("dev-local".into()),
            ..Default::default()
        };
        let tools = Tools::new(Arc::new(Zeron::with_client(zeron_rpc::memory_client(shell.clone()), origin)));

        let opened = tools.call(OPEN_TERMINAL, json!({ "device": "Worker" })).await.unwrap();
        assert_eq!(opened["terminal"], "t1");
        assert_eq!(opened["device"]["id"], "dev-remote");
        assert_eq!(opened["shell"], "bash");
        let calls = shell.calls.lock().unwrap().clone();
        let (_, open) = calls.iter().find(|(m, _)| m == methods::OPEN_TERMINAL).unwrap();
        assert_eq!(open["targetDeviceId"], "dev-remote");
        // Its chat's folder is on another device: not the chat's terminal there.
        assert_eq!(open["chatId"], "agent-terminal:chat-1");

        let wrote = tools
            .call(WRITE_TERMINAL, json!({ "terminal": "t1", "input": "ls", "until": "$ ", "wait_ms": 5000 }))
            .await
            .unwrap();
        assert_eq!(wrote["output"], "ls\nout\n$ ", "{wrote}");
        assert_eq!(wrote["busy"], false);
        assert!(wrote.get("screen").is_none());
        let calls = shell.calls.lock().unwrap().clone();
        let (_, write) = calls.iter().find(|(m, _)| m == methods::WRITE_TERMINAL).unwrap();
        assert_eq!(write["targetDeviceId"], "dev-remote");
        assert_eq!(
            base64::engine::general_purpose::STANDARD.decode(write["data"].as_str().unwrap()).unwrap(),
            b"ls\r"
        );

        // Read: nothing new, and the screen when asked.
        let read = tools.call(READ_TERMINAL, json!({ "terminal": "t1", "screen": true })).await.unwrap();
        assert_eq!(read["output"], "");
        assert!(read["screen"].as_str().unwrap().ends_with("ls\nout\n$"), "{read}");

        let closed = tools.call(CLOSE_TERMINAL, json!({ "terminal": "t1" })).await.unwrap();
        assert_eq!(closed["closed"], true);
        let calls = shell.calls.lock().unwrap().clone();
        let (_, close) = calls.iter().find(|(m, _)| m == methods::CLOSE_TERMINAL).unwrap();
        assert_eq!(close["targetDeviceId"], "dev-remote");
        // Forgotten once closed: naming it again needs its device.
        let again = tools.call(READ_TERMINAL, json!({ "terminal": "t1" })).await.unwrap();
        assert_eq!(again["device"]["id"], "dev-local");
    }
}
