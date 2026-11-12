//! A session: the pseudoterminal, the thread doing its I/O, and the screen.
use crate::{
    Progress,
    graphics::{Graphics, Images},
    osc::{Osc, Sniffer},
};
use alacritty_terminal::{
    event::{Event, EventListener, OnResize, WindowSize},
    grid::Dimensions,
    sync::FairMutex,
    term::{ClipboardType, Config, Osc52},
    tty::{self, ChildEvent, EventedPty, EventedReadWrite},
    vte::ansi::{Processor, Rgb, StdSyncHandler},
};
use polling::{Event as Interest, Events, PollMode, Poller};
use std::{
    borrow::Cow,
    collections::VecDeque,
    io::{self, ErrorKind, Read, Write},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    time::Instant,
};

/// The screen model a session keeps.
pub type Term = alacritty_terminal::Term<Listener>;

/// How a session starts.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionOptions {
    /// The command line to run (by `/bin/sh -c` on Unix, as is on
    /// Windows); `None` runs the user's login shell (PowerShell on Windows).
    pub command: Option<String>,
    /// Where it starts; `None` is this process's directory.
    pub working_directory: Option<PathBuf>,
    /// Added to the inherited environment (after `TERM`, `COLORTERM` and
    /// `TERM_PROGRAM`, which it may override).
    pub env: Vec<(String, String)>,
    /// Lines of scrollback kept.
    pub scrollback: usize,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            command: None,
            working_directory: None,
            env: Vec::new(),
            scrollback: 10_000,
        }
    }
}

/// A grid's size in cells, and a cell's in pixels (what programs asking for
/// the window's size in pixels are told; 0 when unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    /// Cells across, at least 1.
    pub columns: u16,
    /// Cells down, at least 1.
    pub rows: u16,
    /// A cell's width in pixels.
    pub cell_width: u16,
    /// A cell's height in pixels.
    pub cell_height: u16,
}
impl GridSize {
    /// `columns` by `rows` cells, of unknown pixel size.
    pub fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns: columns.max(1),
            rows: rows.max(1),
            cell_width: 0,
            cell_height: 0,
        }
    }
    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows.max(1),
            num_cols: self.columns.max(1),
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}
impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
    fn screen_lines(&self) -> usize {
        usize::from(self.rows.max(1))
    }
    fn columns(&self) -> usize {
        usize::from(self.columns.max(1))
    }
}

/// What happened in a session, for the app (see [`Session::take_events`]).
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// The running program set its title (OSC 0/2); empty when it reset it.
    Title(String),
    /// The shell reported its working directory (OSC 7), as a path.
    Pwd(String),
    /// The bell rang.
    Bell,
    /// A program's progress report (OSC 9;4).
    Progress(Progress),
    /// A shell-integrated command finished (OSC 133;D): its exit code when
    /// reported, and how long since it started (OSC 133;C), if it said.
    CommandFinished {
        /// Its exit code, when reported.
        exit_code: Option<i32>,
        /// Milliseconds since its start, 0 if no start was marked.
        duration_ms: u64,
    },
    /// A desktop notification the program asked for (OSC 9 or 777).
    Notification {
        /// The notification's title (OSC 9 has none).
        title: String,
        /// Its body.
        body: String,
    },
    /// The program put text on the clipboard (OSC 52).
    ClipboardStore(String),
    /// The program asks to read the clipboard (OSC 52): write
    /// [`ClipboardRequest::reply`] to the session to answer, if allowed.
    ClipboardLoad(ClipboardRequest),
    /// The process ended, with this exit code (128 plus the signal's
    /// number when a signal ended it).
    Exited(i32),
}

/// A program's request to read the clipboard.
#[derive(Clone)]
pub struct ClipboardRequest(Arc<dyn Fn(&str) -> String + Send + Sync>);
impl ClipboardRequest {
    /// The answer to write to the session, giving it `text`.
    pub fn reply(&self, text: &str) -> String {
        (self.0)(text)
    }
}
impl std::fmt::Debug for ClipboardRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClipboardRequest")
    }
}

enum Msg {
    Input(Cow<'static, [u8]>),
    Resize(GridSize),
    Shutdown,
}

type ColorRequest = (usize, Arc<dyn Fn(Rgb) -> String + Send + Sync>);
struct Shared {
    /// The app was woken and hasn't read since.
    pending: AtomicBool,
    events: Mutex<Vec<SessionEvent>>,
    wake: Box<dyn Fn() + Send + Sync>,
    size: Mutex<GridSize>,
    palette: Mutex<crate::Palette>,
    colors_asked: Mutex<Vec<ColorRequest>>,
    /// The pictures programs sent (the kitty graphics protocol), by id.
    images: Images,
    /// Whether pictures can arrive: not through Windows' own pseudoconsole,
    /// which drops their strings. Without, the session answers as a
    /// terminal with no pictures does (`shows_pictures`).
    pictures: bool,
    tx: Sender<Msg>,
    poller: Arc<Poller>,
}
impl Shared {
    /// Something changed: wake the app, unless it is already woken.
    fn mark(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
    fn push(&self, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
        self.mark();
    }
    fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_ok() {
            let _ = self.poller.notify();
        }
    }
}

/// How the screen model reaches its session (programs' titles, bells,
/// answers to queries).
pub struct Listener(Arc<Shared>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let shared = &self.0;
        match event {
            Event::Title(title) => shared.push(SessionEvent::Title(title)),
            Event::ResetTitle => shared.push(SessionEvent::Title(String::new())),
            Event::Bell => shared.push(SessionEvent::Bell),
            Event::ClipboardStore(ClipboardType::Clipboard, text) => {
                shared.push(SessionEvent::ClipboardStore(text))
            }
            Event::ClipboardLoad(ClipboardType::Clipboard, format) => {
                shared.push(SessionEvent::ClipboardLoad(ClipboardRequest(format)))
            }
            Event::PtyWrite(text) => shared.send(Msg::Input(text.into_bytes().into())),
            Event::ColorRequest(index, format) => {
                shared.colors_asked.lock().unwrap().push((index, format))
            }
            Event::TextAreaSizeRequest(format) => {
                let size = *shared.size.lock().unwrap();
                shared.send(Msg::Input(format(size.window_size()).into_bytes().into()));
            }
            Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Wakeup => shared.mark(),
            // The selection clipboard (X11's primary) is the app's to keep,
            // from the selection it shows; exits come from the pty.
            Event::ClipboardStore(..)
            | Event::ClipboardLoad(..)
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}

/// A shell on a pseudoterminal and its screen. Dropping it hangs up the
/// shell (SIGHUP; on Windows the pseudoconsole closes) and ends its thread.
pub struct Session {
    shared: Arc<Shared>,
    term: Arc<FairMutex<Term>>,
    pid: u32,
    /// The pty's controller side, for its foreground process group (none
    /// replaying).
    #[cfg(unix)]
    controller: Option<std::os::fd::OwnedFd>,
    exited: Arc<Mutex<Option<i32>>>,
    /// A replay's parsing: output fed (`feed`) rather than read. Boxed:
    /// only a replay has one, and every view holds its session.
    replay: Option<Box<Mutex<Parse>>>,
}

/// Bytes read before the screen is let go of for the app to draw.
const MAX_LOCKED_READ: usize = u16::MAX as usize;
const READ_SIZE: usize = 0x1_0000;
/// The keys alacritty's ptys register their events under (its own
/// constants, private on Unix).
#[cfg(unix)]
const READ_WRITE: usize = 0;
#[cfg(unix)]
const CHILD_EVENT: usize = 1;
#[cfg(windows)]
const READ_WRITE: usize = tty::PTY_READ_WRITE_TOKEN;
#[cfg(windows)]
const CHILD_EVENT: usize = tty::PTY_CHILD_EVENT_TOKEN;

impl Session {
    /// Start `options`' command on a new pseudoterminal of `size`. `wake`
    /// is called, from the session's thread, when there is something to
    /// read and the app hasn't been told since it last read.
    pub fn spawn(
        options: &SessionOptions,
        size: GridSize,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let mut env: std::collections::HashMap<String, String> = [
            ("TERM", "xterm-256color"),
            ("COLORTERM", "truecolor"),
            ("TERM_PROGRAM", "zgx"),
            ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
        env.extend(options.env.iter().cloned());
        let pty_options = tty::Options {
            shell: Some(shell(options.command.as_deref())),
            working_directory: options.working_directory.clone(),
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: false,
        };
        #[cfg(windows)]
        restore_ctrl_c();
        let pty = tty::new(&pty_options, size.window_size(), 0)?;
        #[cfg(unix)]
        let (pid, controller) = (pty.child().id(), pty.file().try_clone()?.into());
        #[cfg(windows)]
        let pid = pty.child_watcher().pid().map_or(0, |pid| pid.get());
        #[cfg(unix)]
        let pictures = true;
        #[cfg(windows)]
        let pictures = pty.hands_on_strings();

        let poller = Arc::new(Poller::new()?);
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            pending: AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
            wake: Box::new(wake),
            size: Mutex::new(size),
            palette: Mutex::new(crate::Palette::default()),
            colors_asked: Mutex::new(Vec::new()),
            images: Images::default(),
            pictures,
            tx,
            poller: poller.clone(),
        });
        let term = Arc::new(FairMutex::new(Term::new(
            config(options, pictures),
            &size,
            Listener(shared.clone()),
        )));
        let exited = Arc::new(Mutex::new(None));
        let io = Io {
            pty,
            shared: shared.clone(),
            term: term.clone(),
            rx,
            poller,
            exited: exited.clone(),
        };
        std::thread::Builder::new()
            .name("zgx terminal".into())
            .spawn(move || io.run())?;
        Ok(Self {
            shared,
            term,
            pid,
            #[cfg(unix)]
            controller: Some(controller),
            exited,
            replay: None,
        })
    }

    /// A session with no program: what `feed` is given is parsed as its
    /// output, on the thread feeding it, as the session's thread parses a
    /// program's. For benches, whose counts would move with how a pty
    /// hands output over and when another thread reads it. What it would
    /// write to a program goes nowhere.
    #[doc(hidden)]
    pub fn replay(
        options: &SessionOptions,
        size: GridSize,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::replay_with(options, size, wake, true).0
    }
    /// As `replay`, showing pictures or not, with what it writes back.
    fn replay_with(
        options: &SessionOptions,
        size: GridSize,
        wake: impl Fn() + Send + Sync + 'static,
        pictures: bool,
    ) -> (Self, Receiver<Msg>) {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            pending: AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
            wake: Box::new(wake),
            size: Mutex::new(size),
            palette: Mutex::new(crate::Palette::default()),
            colors_asked: Mutex::new(Vec::new()),
            images: Images::default(),
            pictures,
            tx,
            poller: Arc::new(Poller::new().expect("a poller")),
        });
        let term = Term::new(config(options, pictures), &size, Listener(shared.clone()));
        let session = Self {
            shared,
            term: Arc::new(FairMutex::new(term)),
            pid: 0,
            #[cfg(unix)]
            controller: None,
            exited: Arc::new(Mutex::new(None)),
            replay: Some(Box::new(Mutex::new(Parse::default()))),
        };
        (session, rx)
    }
    /// Whether pictures a program sends can arrive (the kitty graphics
    /// protocol, sixel). Not through Windows' own pseudoconsole, which drops
    /// them; through Windows Terminal's (`conpty.dll` and `OpenConsole.exe`
    /// beside the app) they do. Without, device attributes don't announce
    /// sixel and picture commands go unanswered, as in a terminal with none.
    #[doc(hidden)]
    pub fn shows_pictures(&self) -> bool {
        self.shared.pictures
    }
    /// Parse `bytes` as a replay's output (see `replay`); nothing for a
    /// session running a program.
    #[doc(hidden)]
    pub fn feed(&self, bytes: &[u8]) {
        let Some(parse) = &self.replay else {
            return;
        };
        let mut parse = parse.lock().unwrap();
        parse.feed(&self.shared, &mut self.term.lock(), bytes);
        if parse.parser.sync_bytes_count() < bytes.len() {
            self.shared.mark();
        }
    }

    /// Send `bytes` to the program, as typed.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            self.shared.send(Msg::Input(bytes));
        }
    }
    /// Give the grid a new size: the screen reflows now and the program is
    /// told (SIGWINCH).
    pub fn resize(&self, size: GridSize) {
        let size = GridSize {
            columns: size.columns.max(1),
            rows: size.rows.max(1),
            ..size
        };
        {
            let mut current = self.shared.size.lock().unwrap();
            if *current == size {
                return;
            }
            *current = size;
        }
        self.term.lock().resize(size);
        self.shared.send(Msg::Resize(size));
    }
    /// The grid's size.
    pub fn size(&self) -> GridSize {
        *self.shared.size.lock().unwrap()
    }
    /// The colours programs asking (OSC 4, 10, 11) are told are the
    /// defaults: the ones the app draws with.
    pub fn set_palette(&self, palette: crate::Palette) {
        *self.shared.palette.lock().unwrap() = palette;
    }
    /// The picture placements (`Term::placements`) show, by its id.
    pub fn image(&self, id: u32) -> Option<Arc<zgx::image::ImageData>> {
        self.shared.images.lock().unwrap().get(&id).cloned()
    }
    /// The screen, locked: what to draw, the selection, the modes. Keep it
    /// briefly: the session's thread waits for it to parse more.
    pub fn term(&self) -> impl std::ops::DerefMut<Target = Term> + '_ {
        self.term.lock()
    }
    /// What happened since the last call, in order. The session wakes the
    /// app again on the next change after this.
    pub fn take_events(&self) -> Vec<SessionEvent> {
        self.shared.pending.store(false, Ordering::Release);
        std::mem::take(&mut *self.shared.events.lock().unwrap())
    }
    /// The process the session started (the shell).
    pub fn pid(&self) -> u32 {
        self.pid
    }
    /// The process in front: the leader of the pty's foreground process
    /// group on Unix, the shell's newest descendant on Windows; the shell
    /// itself when nothing else runs (0 if unknown).
    pub fn foreground_pid(&self) -> u32 {
        if self.exited().is_some() || self.replay.is_some() {
            return 0;
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let Some(controller) = &self.controller else {
                return self.pid;
            };
            // SAFETY: the descriptor is ours (`controller`), open while
            // `self` is; the call only reads the terminal's process group.
            let group = unsafe { libc::tcgetpgrp(controller.as_raw_fd()) };
            if group > 0 { group as u32 } else { self.pid }
        }
        #[cfg(windows)]
        {
            crate::process::newest_descendant(self.pid).unwrap_or(self.pid)
        }
    }
    /// Whether only the shell runs: closing ends nothing else.
    pub fn is_idle(&self) -> bool {
        let front = self.foreground_pid();
        front == 0 || front == self.pid
    }
    /// The process's exit code, once it has ended.
    pub fn exited(&self) -> Option<i32> {
        *self.exited.lock().unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.shared.send(Msg::Shutdown);
    }
}

/// Whether a process handles Ctrl+C is inherited on Windows: a parent that
/// ignores it (`SetConsoleCtrlHandler(NULL, TRUE)`: an agent's shell, a
/// service, some launchers) passes that on to the app and from the app to
/// the shell it starts, where Ctrl+C then never interrupts a program. So the
/// app handles Ctrl+C again before it starts one, as Windows' default is:
/// with no console of its own (a windowed app), nothing else changes.
#[cfg(windows)]
fn restore_ctrl_c() {
    // SAFETY: no handler, FALSE: restores the default handling of Ctrl+C
    // for this process and the children it starts.
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0) };
}

/// The program and arguments for `command`, or for the login shell.
fn shell(command: Option<&str>) -> tty::Shell {
    #[cfg(unix)]
    {
        match command {
            Some(command) => tty::Shell::new("/bin/sh".into(), vec!["-c".into(), command.into()]),
            None => tty::Shell::new(crate::process::login_shell(), vec!["-l".into()]),
        }
    }
    #[cfg(windows)]
    {
        match command {
            Some(command) => tty::Shell::new(command.into(), Vec::new()),
            None => tty::Shell::new("powershell.exe".into(), vec!["-NoLogo".into()]),
        }
    }
}

/// The session's thread: reads the program's output into the screen,
/// writes what the app sends, and passes on resizes, until the program
/// exits or the session is dropped.
struct Io {
    pty: tty::Pty,
    shared: Arc<Shared>,
    term: Arc<FairMutex<Term>>,
    rx: Receiver<Msg>,
    poller: Arc<Poller>,
    exited: Arc<Mutex<Option<i32>>>,
}
/// The screen model's configuration: device attributes announce sixel
/// only where pictures can arrive.
fn config(options: &SessionOptions, pictures: bool) -> Config {
    Config {
        scrolling_history: options.scrollback,
        // Programs may set the clipboard, and ask to read it: the app asks
        // its user (`SessionEvent::ClipboardLoad`).
        osc52: Osc52::CopyPaste,
        // Programs may turn on the kitty keyboard protocol (`CSI > u`): the
        // view encodes keys as it asks.
        kitty_keyboard: true,
        sixel: pictures,
        ..Config::default()
    }
}
#[derive(Default)]
struct Parse {
    parser: Processor<StdSyncHandler>,
    sniffer: Sniffer,
    graphics: Graphics,
    /// When the running command started (OSC 133;C).
    command_started: Option<Instant>,
}
impl Parse {
    /// Parse `bytes` of output into the screen, reporting the commands the
    /// screen model leaves to its embedder.
    fn feed(&mut self, shared: &Shared, term: &mut Term, bytes: &[u8]) {
        // The screen model's events and the sniffed commands' in the
        // order the program wrote them: parse up to each command's end
        // before reporting it.
        let mut parsed = 0;
        let (started, parser, graphics) = (
            &mut self.command_started,
            &mut self.parser,
            &mut self.graphics,
        );
        self.sniffer.feed(bytes, |end, osc| {
            // A picture's body: the screen model reads around it.
            if let Osc::Body(from) = osc {
                parser.advance(term, &bytes[parsed..from]);
                parsed = end;
                return;
            }
            parser.advance(term, &bytes[parsed..end]);
            parsed = end;
            let event = match osc {
                Osc::Body(_) => return,
                Osc::Pwd(path) => SessionEvent::Pwd(path),
                Osc::CommandStarted => {
                    *started = Some(Instant::now());
                    return;
                }
                Osc::Sixel(_) | Osc::Graphics(_) if !shared.pictures => return,
                Osc::Sixel(body) => {
                    let size = *shared.size.lock().unwrap();
                    let cell = (size.cell_width, size.cell_height);
                    let Rgb { r, g, b } = shared.palette.lock().unwrap().background;
                    graphics.sixel(&body, term, cell, [r, g, b], &shared.images);
                    return;
                }
                Osc::Graphics(command) => {
                    let size = *shared.size.lock().unwrap();
                    let cell = (size.cell_width, size.cell_height);
                    if let Some(answer) = graphics.command(&command, term, cell, &shared.images) {
                        shared.send(Msg::Input(answer.into()));
                    }
                    return;
                }
                Osc::CommandFinished(exit_code) => SessionEvent::CommandFinished {
                    exit_code,
                    duration_ms: started
                        .take()
                        .map_or(0, |at| at.elapsed().as_millis() as u64),
                },
                Osc::Progress(progress) => SessionEvent::Progress(progress),
                Osc::Notification { title, body } => SessionEvent::Notification { title, body },
            };
            shared.push(event);
        });
        parser.advance(term, &bytes[parsed..]);
    }
}
impl Io {
    fn run(mut self) {
        let mut parse = Parse::default();
        let mut buffer = vec![0u8; READ_SIZE];
        let mut writes: VecDeque<Cow<'static, [u8]>> = VecDeque::new();
        let mut written = 0;
        let mut interest = Interest::readable(0);
        // SAFETY: the pty outlives its registration: it is deregistered
        // below, before it drops.
        if unsafe { self.pty.register(&self.poller, interest, PollMode::Level) }.is_err() {
            return;
        }
        let mut events = Events::with_capacity(NonZeroUsize::new(16).unwrap());
        'run: loop {
            let sync_until = parse.parser.sync_timeout().sync_timeout();
            let timeout = sync_until.map(|until| until.saturating_duration_since(Instant::now()));
            events.clear();
            if let Err(error) = self.poller.wait(&mut events, timeout) {
                if error.kind() == ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            // A synchronized update (DEC 2026) that never ended: show it.
            if events.is_empty() && sync_until.is_some_and(|until| until <= Instant::now()) {
                parse.parser.stop_sync(&mut *self.term.lock());
                self.shared.mark();
            }
            loop {
                match self.rx.try_recv() {
                    Ok(Msg::Input(bytes)) => writes.push_back(bytes),
                    Ok(Msg::Resize(size)) => self.pty.on_resize(size.window_size()),
                    Ok(Msg::Shutdown) | Err(TryRecvError::Disconnected) => break 'run,
                    Err(TryRecvError::Empty) => break,
                }
            }
            for event in events.iter() {
                match event.key {
                    CHILD_EVENT => {
                        if let Some(ChildEvent::Exited(status)) = self.pty.next_child_event() {
                            self.read_last(&mut parse, &mut buffer);
                            let code = status.map_or(-1, exit_code);
                            *self.exited.lock().unwrap() = Some(code);
                            self.shared.push(SessionEvent::Exited(code));
                            break 'run;
                        }
                    }
                    READ_WRITE => {
                        if event.is_interrupt() {
                            continue;
                        }
                        if event.readable
                            && let Err(error) = self.read(&mut parse, &mut buffer)
                        {
                            // Linux: reading a pty whose other side hung up
                            // fails with EIO; the child's exit follows.
                            #[cfg(target_os = "linux")]
                            if error.raw_os_error() == Some(libc::EIO) {
                                continue;
                            }
                            let _ = error;
                            break 'run;
                        }
                        if event.writable && self.write(&mut writes, &mut written).is_err() {
                            break 'run;
                        }
                    }
                    _ => {}
                }
            }
            // Bytes queued since: try now, then wait to be writable.
            if !writes.is_empty() && self.write(&mut writes, &mut written).is_err() {
                break;
            }
            let wants_write = !writes.is_empty();
            if wants_write != interest.writable {
                interest.writable = wants_write;
                if self
                    .pty
                    .reregister(&self.poller, interest, PollMode::Level)
                    .is_err()
                {
                    break;
                }
            }
        }
        let _ = self.pty.deregister(&self.poller);
        // Dropping the pty hangs up the program and reaps it.
    }

    /// What the program wrote last, once it has exited. Windows: ConPTY
    /// renders it after the program has gone, as the pseudoconsole closes,
    /// and then ends the output pipe, so it is read until the pipe ends (a
    /// few seconds at most, should it never). Read once at once, it was
    /// often still to come: a program's last lines (ping's statistics after
    /// Ctrl+C) never showed.
    fn read_last(&mut self, parse: &mut Parse, buffer: &mut [u8]) {
        #[cfg(windows)]
        {
            self.pty.close_console();
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            let mut events = Events::with_capacity(NonZeroUsize::new(4).unwrap());
            loop {
                if self.read(parse, buffer).is_err() || self.pty.output_finished() {
                    return;
                }
                let now = Instant::now();
                if now >= deadline {
                    return;
                }
                events.clear();
                if self.poller.wait(&mut events, Some(deadline - now)).is_err() {
                    return;
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = self.read(parse, buffer);
        }
    }

    fn read(&mut self, parse: &mut Parse, buffer: &mut [u8]) -> io::Result<()> {
        let _lease = self.term.lease();
        let mut term = None;
        let mut processed = 0;
        loop {
            let read = match self.pty.reader().read(buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error)
                    if matches!(error.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            };
            let bytes = &buffer[..read];
            let term = term.get_or_insert_with(|| self.term.lock_unfair());
            parse.feed(&self.shared, term, bytes);
            processed += read;
            if processed >= MAX_LOCKED_READ {
                break;
            }
        }
        if let Some(term) = &term {
            self.answer_colors(term);
        }
        drop(term);
        // Unless all of it waits in a synchronized update.
        if processed > 0 && parse.parser.sync_bytes_count() < processed {
            self.shared.mark();
        }
        Ok(())
    }

    /// Answer programs' colour queries (OSC 4, 10, 11, 12): a colour they
    /// set, else the app's.
    fn answer_colors(&self, term: &Term) {
        let asked = std::mem::take(&mut *self.shared.colors_asked.lock().unwrap());
        if asked.is_empty() {
            return;
        }
        let palette = self.shared.palette.lock().unwrap().clone();
        for (index, format) in asked {
            let color = term.colors()[index].unwrap_or_else(|| palette.color(index));
            let _ = self
                .shared
                .tx
                .send(Msg::Input(format(color).into_bytes().into()));
        }
        let _ = self.poller.notify();
    }

    fn write(
        &mut self,
        writes: &mut VecDeque<Cow<'static, [u8]>>,
        written: &mut usize,
    ) -> io::Result<()> {
        while let Some(front) = writes.front() {
            match self.pty.writer().write(&front[*written..]) {
                Ok(0) => break,
                Ok(count) => {
                    *written += count;
                    if *written == front.len() {
                        writes.pop_front();
                        *written = 0;
                    }
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) =>
                {
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(-1)
    }
    #[cfg(windows)]
    {
        status.code().unwrap_or(-1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a session writes back to its program for `output`.
    fn answers(pictures: bool, output: &[u8]) -> (Vec<String>, Session) {
        let size = GridSize::new(40, 10);
        let (session, rx) = Session::replay_with(&SessionOptions::default(), size, || {}, pictures);
        session.feed(output);
        let written = rx
            .try_iter()
            .filter_map(|msg| match msg {
                Msg::Input(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                _ => None,
            })
            .collect();
        (written, session)
    }

    /// A screen's cells, row by row, and its cursor.
    fn screen<T: EventListener>(term: &alacritty_terminal::Term<T>) -> (Vec<String>, (i32, usize)) {
        use alacritty_terminal::index::{Column, Line};
        let grid = term.grid();
        let rows = (0..grid.screen_lines() as i32)
            .map(|line| {
                let row = &grid[Line(line)];
                (0..grid.columns()).map(|c| row[Column(c)].c).collect()
            })
            .collect();
        let cursor = grid.cursor.point;
        (rows, (cursor.line.0, cursor.column.0))
    }

    /// Spared pictures' bodies, the screen model ends as it would reading
    /// them, however reads cut the output: the same cells and cursor, the
    /// pictures placed by the same commands.
    #[test]
    fn the_screen_is_the_same_without_reading_picture_bodies() {
        let output: &[u8] = b"ab\x1b_Ga=T,f=24,s=1,v=1,c=2,r=1,i=3,q=2;AAAA\x1b\\cd\r\n\
            \x1b[31mred\x1bP0;0q#1;2;100;0;0!8~-!8~\x1b\\ef\x1b_Xapc\x1b\\gh\r\n\
            \x1bP+q544e\x1b\\\x1b]0;t\x07ij\x1b_Gq=2,a=t,f=24,s=1,v=1,i=4;AAAA\x1b\\kl";
        let size = GridSize::new(40, 10);
        // Read whole by the screen model, as before.
        let reference = {
            let mut term = alacritty_terminal::Term::new(
                config(&SessionOptions::default(), true),
                &size,
                alacritty_terminal::event::VoidListener,
            );
            let (mut parser, mut sniffer) =
                (Processor::<StdSyncHandler>::new(), Sniffer::default());
            let (mut graphics, images) = (Graphics::default(), Images::default());
            let mut parsed = 0;
            sniffer.feed(output, |end, osc| {
                parser.advance(&mut term, &output[parsed..end]);
                parsed = end;
                match osc {
                    Osc::Graphics(command) => {
                        graphics.command(&command, &mut term, (0, 0), &images);
                    }
                    Osc::Sixel(body) => {
                        graphics.sixel(&body, &mut term, (0, 0), [0, 0, 0], &images)
                    }
                    _ => {}
                }
            });
            parser.advance(&mut term, &output[parsed..]);
            (screen(&term), term.placements().len())
        };
        assert_eq!(reference.1, 2);
        for cut in 0..output.len() {
            let (session, _) = Session::replay_with(&SessionOptions::default(), size, || {}, true);
            session.feed(&output[..cut]);
            session.feed(&output[cut..]);
            let term = session.term();
            assert_eq!(
                (screen(&term), term.placements().len()),
                reference,
                "cut at {cut}"
            );
        }
    }

    /// Where pictures can't arrive (Windows' own pseudoconsole), nothing
    /// says they can: device attributes announce no sixel, a kitty query
    /// goes unanswered and a picture placed anyway shows nothing, as in a
    /// terminal with no pictures, so programs send text instead.
    #[test]
    fn without_pictures_nothing_announces_them() {
        let query = b"\x1b_Ga=q,i=1,s=1,v=1,f=24;AAAA\x1b\\";
        let place = b"\x1b_Ga=T,i=2,s=1,v=1,f=24;AAAA\x1b\\\x1bPq#1;2;100;0;0~\x1b\\";
        let ask = [&b"\x1b[c"[..], query, place].concat();
        let (written, session) = answers(false, &ask);
        assert_eq!(written, ["\x1b[?62;22c"]);
        assert!(!session.shows_pictures());
        assert!(session.term().placements().is_empty());
        assert!(session.image(2).is_none());
        // Where they can, all of it is as before.
        let (written, session) = answers(true, &ask);
        assert_eq!(
            written,
            ["\x1b[?62;4;22c", "\x1b_Gi=1;OK\x1b\\", "\x1b_Gi=2;OK\x1b\\"]
        );
        assert!(session.shows_pictures());
        assert_eq!(session.term().placements().len(), 2);
    }
}
