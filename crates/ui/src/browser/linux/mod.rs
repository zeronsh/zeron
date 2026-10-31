//! Web pages render in an isolated helper process and GPUI composites their
//! frames, so browser content uses the same clipping and blur as other UI. WPE
//! WebKit renders on GPUI's GPU at the display's refresh rate (`wpe.rs`); the
//! WebKitGTK helper remains for systems without WPE WebKit.
use super::model::{PageState, Presentation};
use gpui::{Bounds, Pixels, RenderImage};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};
use tokio::sync::mpsc::Sender;

mod wpe;

#[derive(Clone, Debug)]
pub enum NativeEvent {
    Changed,
    Frame,
    NewTab(String),
    Clipboard(String),
    Menu(Value),
    /// A script's string result (`None` for any other value or a failure).
    Evaluated(Option<String>),
    /// WebKit started or stopped wanting vblanks; see `Surface::vsync`.
    Vsync,
    /// The page's pointer changed; see `Surface::cursor`.
    Cursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Engine {
    Gtk,
    Wpe,
}

/// Set once WPE WebKit failed to start in this process: later helpers go
/// straight to WebKitGTK instead of failing the same way again.
static WPE_FAILED: AtomicBool = AtomicBool::new(false);

impl Engine {
    /// `ZERON_BROWSER_ENGINE=gtk|wpe` overrides the default, WPE WebKit when
    /// this build embeds its helper.
    fn preferred() -> (Self, bool) {
        match std::env::var("ZERON_BROWSER_ENGINE").as_deref() {
            Ok("gtk") => (Self::Gtk, true),
            Ok("wpe") => (Self::Wpe, true),
            _ if wpe::HELPER.is_empty() || WPE_FAILED.load(Ordering::Relaxed) => {
                (Self::Gtk, false)
            }
            _ => (Self::Wpe, false),
        }
    }
}

#[derive(Clone, Default)]
pub struct BrowserData {
    current: Arc<Mutex<Option<Arc<Worker>>>>,
    profile: Option<super::profile::BrowserProfile>,
    #[cfg(test)]
    helper_override: Option<std::path::PathBuf>,
    #[cfg(test)]
    engine_override: Option<Engine>,
}

struct Worker {
    engine: Engine,
    child: Mutex<Child>,
    link: Link,
    routes: Arc<Mutex<HashMap<u32, Weak<Route>>>>,
    /// Its frames reader stopped: pages can no longer hear from the helper.
    stopped: Arc<AtomicBool>,
    next_id: AtomicU32,
    /// What the helper reported when it started.
    #[cfg(test)]
    ready: Value,
}
enum Link {
    Pipe(Mutex<Option<ChildStdin>>),
    Socket(wpe::Outbox),
}
impl Drop for Worker {
    fn drop(&mut self) {
        // EOF asks the helper to drain pending work. Closing the pipe cannot
        // block on a hung helper, unlike writing another command to a full pipe.
        match &mut self.link {
            Link::Pipe(stdin) => {
                if let Ok(stdin) = stdin.get_mut() {
                    stdin.take();
                }
            }
            Link::Socket(outbox) => outbox.close(),
        }
        if let Ok(child) = self.child.get_mut() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    _ => break,
                }
            }
            tracing::warn!("browser helper did not shut down in time; terminating it");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
struct Route {
    tx: Sender<NativeEvent>,
    state: Mutex<PageState>,
    frame: Mutex<Option<(Arc<RenderImage>, f32)>>,
    surface: Mutex<wpe::Surface>,
    input: Mutex<Value>,
    #[cfg(feature = "browser-fixture")]
    evaluation: Mutex<Option<Value>>,
}

const GTK_HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zeron-webkit"));

fn helper_path(engine: Engine) -> Result<std::path::PathBuf, String> {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".cache")))
        .ok_or("Could not locate the browser cache directory")?
        .join("zeron/browser");
    // Development builds can run a helper built on its own, e.g. instrumented.
    #[cfg(debug_assertions)]
    if let (Engine::Wpe, Some(path)) = (engine, std::env::var_os("ZERON_WPE_HELPER")) {
        return Ok(path.into());
    }
    match engine {
        Engine::Gtk => extract_helper(&root, "webkit", GTK_HELPER),
        Engine::Wpe => extract_helper(&root, "wpe", wpe::HELPER),
    }
}

fn extract_helper(
    root: &std::path::Path,
    name: &str,
    helper: &[u8],
) -> Result<std::path::PathBuf, String> {
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(helper));
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let path = root.join(format!("{name}-{hash}"));
    if std::fs::read(&path).ok().as_deref() != Some(helper) {
        // Different profiles can now initialize concurrently. Each extraction
        // needs its own staging file, even within the same application process.
        let temp = root.join(format!(
            ".{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let result = (|| -> std::io::Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .open(&temp)?;
            f.write_all(helper)?;
            f.sync_all()?;
            std::fs::rename(&temp, &path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result.map_err(|e| e.to_string())?;
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// The helper's first packet: `R` with its capabilities, or `E` with a
/// storage error to show on the page.
fn read_startup(stdout: &mut impl Read, runtime: &str) -> Result<Vec<u8>, Startup> {
    let mut header = [0u8; 9];
    stdout.read_exact(&mut header).map_err(|error| {
        format!("Browser helper could not start: {error}. Check the {runtime} runtime and display.")
    })?;
    let length = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
    if length > 16384 || header[1..5] != [0; 4] {
        return Err("Invalid browser startup response".to_owned().into());
    }
    let mut message = vec![0; length];
    stdout
        .read_exact(&mut message)
        .map_err(|error| error.to_string())?;
    match header[0] {
        b'R' => Ok(message),
        b'E' => Err(Startup::Storage(String::from_utf8_lossy(&message).into_owned())),
        _ => Err("Invalid browser startup response".to_owned().into()),
    }
}

/// Packets both helpers send. Returns the event to wake the page with.
fn common_packet(route: &Route, kind: u8, data: Vec<u8>) -> Option<NativeEvent> {
    Some(match kind {
        b'I' => {
            if let Ok(Value::Object(update)) = serde_json::from_slice::<Value>(&data) {
                let mut input = route.input.lock().unwrap();
                if !input.is_object() {
                    *input = json!({});
                }
                for (key, value) in update {
                    input[&key] = value;
                }
            }
            NativeEvent::Frame
        }
        b'S' => {
            *route.state.lock().unwrap() = serde_json::from_slice(&data).ok()?;
            NativeEvent::Changed
        }
        b'M' => NativeEvent::Menu(serde_json::from_slice(&data).ok()?),
        b'C' => NativeEvent::Clipboard(String::from_utf8_lossy(&data).into_owned()),
        b'N' => NativeEvent::NewTab(String::from_utf8_lossy(&data).into_owned()),
        b'J' => {
            let value: Option<Value> = serde_json::from_slice(&data).ok();
            #[cfg(feature = "browser-fixture")]
            {
                *route.evaluation.lock().unwrap() = value.clone();
            }
            NativeEvent::Evaluated(value.and_then(|v| v.as_str().map(str::to_owned)))
        }
        _ => return None,
    })
}

fn deliver(route: &Route, event: NativeEvent) {
    // At most one latest frame is retained per page. A busy UI never
    // accumulates video frames or blocks the engine; frame and vsync state is
    // read from the route, so a dropped wakeup loses nothing.
    match event {
        NativeEvent::NewTab(_) | NativeEvent::Clipboard(_) | NativeEvent::Menu(_) => {
            let _ = route.tx.blocking_send(event);
        }
        _ => {
            let _ = route.tx.try_send(event);
        }
    }
}

fn helper_stopped(routes: &Mutex<HashMap<u32, Weak<Route>>>, engine: Engine) {
    let runtime = match engine {
        Engine::Gtk => "WebKitGTK 4.1",
        Engine::Wpe => "WPE WebKit",
    };
    for route in routes.lock().unwrap().values().filter_map(Weak::upgrade) {
        // Ends the page's vblank callbacks too.
        route.surface.lock().unwrap().vsync = false;
        let mut state = route.state.lock().unwrap();
        state.loading = false;
        state.error = Some(format!(
            "The browser helper stopped. Check that {runtime} is installed, then reopen the tab."
        ));
        drop(state);
        let _ = route.tx.try_send(NativeEvent::Changed);
    }
}

/// Kills a helper that failed to start, so it never holds the profile lock.
fn abandon(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads the helper's first packet without blocking startup forever when the
/// display or runtime hangs.
fn await_startup<R: Read + Send + 'static>(
    mut stdout: R,
    runtime: &'static str,
    timeout: std::time::Duration,
) -> Result<(R, Vec<u8>), Startup> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = read_startup(&mut stdout, runtime).map(|ready| (stdout, ready));
        let _ = ready_tx.send(result);
    });
    match ready_rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(error) => Err(format!("Browser startup did not complete: {error}").into()),
    }
}

impl BrowserData {
    pub fn for_profile(profile: super::profile::BrowserProfile) -> Self {
        // One live helper per profile, shared across all windows in this process.
        type SharedWorker = Mutex<Option<Arc<Worker>>>;
        static PROFILES: std::sync::OnceLock<
            Mutex<HashMap<std::path::PathBuf, Weak<SharedWorker>>>,
        > = std::sync::OnceLock::new();
        let mut profiles = PROFILES.get_or_init(Default::default).lock().unwrap();
        profiles.retain(|_, worker| worker.strong_count() > 0);
        let current = profiles
            .get(&profile.root)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let current = Arc::new(Mutex::new(None));
                profiles.insert(profile.root.clone(), Arc::downgrade(&current));
                current
            });
        Self {
            profile: Some(profile),
            current,
            #[cfg(test)]
            helper_override: None,
            #[cfg(test)]
            engine_override: None,
        }
    }
    /// Blocking initialization, called only by the dedicated startup thread.
    /// `gpu` is the GPU GPUI draws with, and the dma-bufs it imports.
    fn worker(&self, gpu: Option<gpui::DmabufTarget>) -> Result<Arc<Worker>, String> {
        let mut current = self.current.lock().unwrap();
        if let Some(worker) = current.as_ref()
            && !worker.stopped.load(Ordering::Relaxed)
            && worker
                .child
                .lock()
                .unwrap()
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_none()
        {
            return Ok(worker.clone());
        }
        let profile = self.profile.as_ref().map(|profile| profile.root.as_path());
        #[cfg(test)]
        let (engine, forced) = match (self.engine_override, &self.helper_override) {
            (Some(engine), _) => (engine, true),
            (None, Some(_)) => (Engine::Gtk, true),
            (None, None) => Engine::preferred(),
        };
        #[cfg(not(test))]
        let (engine, forced) = Engine::preferred();
        let executable = |engine| {
            #[cfg(test)]
            if let Some(helper) = &self.helper_override {
                return Ok(helper.clone());
            }
            helper_path(engine)
        };
        let worker = match engine {
            Engine::Wpe => match executable(Engine::Wpe)
                .map_err(Startup::from)
                .and_then(|path| spawn_wpe(&path, profile, gpu))
            {
                Ok(worker) => worker,
                Err(Startup::Storage(error)) => return Err(error),
                Err(Startup::Failed(error)) if forced => return Err(error),
                Err(Startup::Failed(error)) => {
                    tracing::warn!("WPE WebKit could not start, using WebKitGTK: {error}");
                    WPE_FAILED.store(true, Ordering::Relaxed);
                    spawn_gtk(&executable(Engine::Gtk)?, profile)?
                }
            },
            Engine::Gtk => spawn_gtk(&executable(Engine::Gtk)?, profile)?,
        };
        let worker = Arc::new(worker);
        *current = Some(worker.clone());
        Ok(worker)
    }
}

#[derive(Debug, PartialEq)]
enum Startup {
    /// The helper ran and reported a problem with the profile.
    Storage(String),
    /// The helper did not run: another engine may.
    Failed(String),
}
impl From<String> for Startup {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}
impl From<Startup> for String {
    fn from(error: Startup) -> Self {
        match error {
            Startup::Storage(error) | Startup::Failed(error) => error,
        }
    }
}

/// Starts a helper. A child that another thread forks while the helper is
/// being extracted holds the new file open for writing until that child
/// execs, and starting the helper meanwhile fails with ETXTBSY. Wait it out
/// well past any exec: giving up would switch engines for the session.
fn spawn_helper(command: &mut Command) -> std::io::Result<Child> {
    for delay in [5, 20, 50, 100, 200, 400, 800] {
        match command.spawn() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
            result => return result,
        }
    }
    command.spawn()
}

fn spawn_gtk(executable: &std::path::Path, profile: Option<&std::path::Path>) -> Result<Worker, String> {
    let mut command = Command::new(executable);
    if let Some(profile) = profile {
        command.arg("--profile").arg(profile);
    }
    let mut child = spawn_helper(command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()))
        .map_err(|e| format!("Could not start WebKitGTK: {e}. Install the WebKitGTK 4.1 runtime for your distribution."))?;
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    // Report startup/storage failures on the page instead of silently opening
    // an ephemeral session. Bound the wait if the display/runtime hangs.
    let mut stdout = match await_startup(stdout, "WebKitGTK 4.1", std::time::Duration::from_secs(10)) {
        Ok((stdout, _)) => stdout,
        Err(error) => {
            abandon(child);
            return Err(error.into());
        }
    };
    let routes: Arc<Mutex<HashMap<u32, Weak<Route>>>> = Arc::default();
    let reader_routes = routes.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let reader_stopped = stopped.clone();
    std::thread::Builder::new().name("browser-frames".into()).spawn(move || {
        let result = (|| -> std::io::Result<()> {
            loop {
                let mut header = [0u8; 9]; stdout.read_exact(&mut header)?;
                let id = u32::from_le_bytes(header[1..5].try_into().unwrap());
                let length = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
                if length > 8192 * 8192 * 4 + 12 { return Err(std::io::Error::other("Browser packet is too large")); }
                let mut data = vec![0; length]; stdout.read_exact(&mut data)?;
                let route = reader_routes.lock().unwrap().get(&id).and_then(Weak::upgrade);
                let Some(route) = route else { continue; };
                let event = match header[0] {
                    b'F' => {
                        if data.len() < 12 { continue; }
                        let width = u32::from_le_bytes(data[..4].try_into().unwrap());
                        let height = u32::from_le_bytes(data[4..8].try_into().unwrap());
                        let scale=f32::from_bits(u32::from_le_bytes(data[8..12].try_into().unwrap()));
                        if !scale.is_finite() || !(0.5..=4.).contains(&scale) {continue;}
                        data.drain(..12);
                        let Some(pixels) = image::RgbaImage::from_raw(width, height, data) else { continue; };
                        *route.frame.lock().unwrap() = Some((Arc::new(RenderImage::new([image::Frame::new(pixels)])),scale));
                        NativeEvent::Frame
                    }
                    kind => match common_packet(&route, kind, data) { Some(event) => event, None => continue },
                };
                deliver(&route, event);
            }
        })();
        if result.is_err() {
            reader_stopped.store(true, Ordering::Relaxed);
            helper_stopped(&reader_routes, Engine::Gtk);
        }
    }).map_err(|e| e.to_string())?;
    Ok(Worker {
        engine: Engine::Gtk,
        child: Mutex::new(child),
        link: Link::Pipe(Mutex::new(Some(stdin))),
        routes,
        stopped,
        next_id: AtomicU32::new(1),
        #[cfg(test)]
        ready: Value::Null,
    })
}

fn spawn_wpe(
    executable: &std::path::Path,
    profile: Option<&std::path::Path>,
    gpu: Option<gpui::DmabufTarget>,
) -> Result<Worker, Startup> {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    let (socket, theirs) = UnixStream::pair().map_err(|e| e.to_string())?;
    let mut command = Command::new(executable);
    if let Some(profile) = profile {
        command.arg("--profile").arg(profile);
    }
    // `ZERON_BROWSER_DMABUF=0` keeps frames in shared memory, rendered by the
    // CPU; `readback` renders them on the GPU and reads them back.
    let dmabuf = std::env::var("ZERON_BROWSER_DMABUF");
    if let Some(gpu) = gpu.filter(|_| dmabuf.as_deref() != Ok("0")) {
        command.arg("--render-node").arg(&gpu.render_node);
        // WebKit renders in one of these layouts where it can, so GPUI imports
        // frames without copies; the helper reads the others back.
        if !gpu.modifiers.is_empty() && dmabuf.as_deref() != Ok("readback") {
            let modifiers: Vec<_> = gpu.modifiers.iter().map(|m| format!("{m:x}")).collect();
            command.arg("--modifiers").arg(modifiers.join(","));
        }
    }
    // A failed GLib check in WebKit is a bug the tests must not miss.
    #[cfg(test)]
    command.env("G_DEBUG", "fatal-criticals");
    let input = OwnedFd::from(theirs.try_clone().map_err(|e| e.to_string())?);
    let child = spawn_helper(
        command
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(OwnedFd::from(theirs)))
            .stderr(Stdio::inherit()),
    )
    .map_err(|e| format!("Could not start WPE WebKit: {e}"))?;
    // The command still holds the helper's end of the socket: closed, a
    // helper that exits ends the wait for it at once.
    drop(command);
    let startup = socket.try_clone().map_err(|e| e.to_string())?;
    // A missing or broken WPE runtime fails here; WebKitGTK takes over.
    let ready = match await_startup(startup, "WPE WebKit", std::time::Duration::from_secs(10)) {
        Ok((_, ready)) => ready,
        Err(error) => {
            abandon(child);
            return Err(error);
        }
    };
    tracing::info!("browser engine: WPE WebKit {}", String::from_utf8_lossy(&ready));
    let outbox = wpe::writer(socket.try_clone().map_err(|e| e.to_string())?);
    let routes: Arc<Mutex<HashMap<u32, Weak<Route>>>> = Arc::default();
    let reader_routes = routes.clone();
    let reader_outbox = outbox.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let reader_stopped = stopped.clone();
    let connection = socket.try_clone().map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("browser-frames".into())
        .spawn(move || {
            let mut reader = wpe::Reader::new(socket);
            let result = (|| -> std::io::Result<()> {
                loop {
                    let (kind, id, data) = reader.next()?;
                    let route = reader_routes.lock().unwrap().get(&id).and_then(Weak::upgrade);
                    let Some(route) = route else {
                        // A closed page: still consume the descriptor it sent.
                        if kind == b'B' {
                            reader.take_fd()?;
                        }
                        continue;
                    };
                    let event = match wpe::packet(&route, &reader_outbox, id, kind, &data, &mut reader)? {
                        Some(event) => event,
                        None => match common_packet(&route, kind, data) {
                            Some(event) => event,
                            None => continue,
                        },
                    };
                    deliver(&route, event);
                }
            })();
            if let Err(error) = result
                && error.kind() != std::io::ErrorKind::UnexpectedEof
            {
                tracing::warn!("browser helper connection failed: {error}");
            }
            // Nothing reads the helper anymore: end its connection, so it
            // exits rather than block on a full socket, and start a new one
            // for the next page.
            reader_stopped.store(true, Ordering::Relaxed);
            let _ = connection.shutdown(std::net::Shutdown::Both);
            helper_stopped(&reader_routes, Engine::Wpe);
        })
        .map_err(|e| e.to_string())?;
    Ok(Worker {
        engine: Engine::Wpe,
        child: Mutex::new(child),
        link: Link::Socket(outbox),
        routes,
        stopped,
        next_id: AtomicU32::new(1),
        #[cfg(test)]
        ready: serde_json::from_slice(&ready).unwrap_or_default(),
    })
}

impl Worker {
    fn send(&self, id: u32, mut command: Value) -> Result<(), String> {
        match &self.link {
            Link::Socket(outbox) => outbox.send(id, command),
            Link::Pipe(stdin) => {
                command["id"] = id.into();
                let data = serde_json::to_vec(&command).map_err(|e| e.to_string())?;
                let mut stdin = stdin.lock().unwrap();
                let pipe = stdin.as_mut().ok_or("Browser helper is shutting down")?;
                pipe.write_all(&(data.len() as u32).to_le_bytes())
                    .and_then(|_| pipe.write_all(&data))
                    .map_err(|e| e.to_string())
            }
        }
    }
}

/// The browser fixture's account of the frames GPUI painted for a page, and of
/// the bytes copied into its atlas tile to show them.
#[cfg(feature = "browser-fixture")]
#[derive(Default)]
struct FrameLog {
    painted: u64,
    /// When the latest frame was painted.
    last: Option<std::time::Instant>,
    /// When each frame was painted, while the fixture records them.
    times: Option<Vec<std::time::Instant>>,
    /// Bytes copied from CPU memory, and between GPU buffers.
    uploaded: u64,
    copied: u64,
    /// The WebKitGTK image painted last: GPUI uploads each new one whole.
    image: Option<gpui::ImageId>,
}

#[cfg(feature = "browser-fixture")]
impl FrameLog {
    fn painted(&mut self) {
        let now = std::time::Instant::now();
        self.painted += 1;
        self.last = Some(now);
        if let Some(times) = &mut self.times {
            times.push(now);
        }
    }

    fn copied(&mut self, bytes: u64, on_gpu: bool) {
        if on_gpu {
            self.copied += bytes;
        } else {
            self.uploaded += bytes;
        }
    }
}

pub struct NativePage {
    worker: Arc<Worker>,
    route: Arc<Route>,
    id: u32,
    pub bounds: Bounds<Pixels>,
    pub scale: f32,
    geometry: Option<(u32, u32, u32)>,
    presentation: Presentation,
    pub image: Option<Arc<RenderImage>>,
    pub image_scale: f32,
    pub menu: Option<Value>,
    pub menu_active: usize,
    preedit: String,
    pub pressed: std::cell::Cell<Option<gpui::MouseButton>>,
    /// WPE WebKit pages: their frames, imports and vblank.
    presenter: Option<wpe::Presenter>,
    #[cfg(feature = "browser-fixture")]
    log: FrameLog,
}
impl NativePage {
    /// Extract, spawn and handshake entirely off the UI thread. Concurrent
    /// tabs wait for their shared profile's worker here, never in navigation.
    pub fn start(
        data: BrowserData,
        tx: Sender<NativeEvent>,
        gpu: Option<gpui::DmabufTarget>,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<Self, String>>, String> {
        // A unit test has no display for WebKitGTK, and this thread would wake
        // the test scheduler from outside it. Startup tests bring a helper.
        #[cfg(test)]
        if data.helper_override.is_none() {
            return Err("Unit tests start no browser helper".into());
        }
        let (ready, result) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("browser-startup".into())
            .spawn(move || {
                let page = Self::new(&data, tx, gpu);
                // If the tab was closed, send returns the page and drops it on
                // this thread. The UI never waits for an abandoned startup.
                let _ = ready.send(page);
            })
            .map_err(|error| format!("Could not start browser initialization: {error}"))?;
        Ok(result)
    }

    fn new(
        data: &BrowserData,
        tx: Sender<NativeEvent>,
        gpu: Option<gpui::DmabufTarget>,
    ) -> Result<Self, String> {
        let worker = data.worker(gpu)?;
        let id = worker.next_id.fetch_add(1, Ordering::Relaxed);
        let route = Arc::new(Route {
            tx,
            state: Mutex::default(),
            frame: Mutex::default(),
            surface: Mutex::default(),
            input: Mutex::new(json!({})),
            #[cfg(feature = "browser-fixture")]
            evaluation: Mutex::default(),
        });
        {
            let mut routes = worker.routes.lock().unwrap();
            // A helper that stopped before the route was in place would never
            // tell this page.
            if worker.stopped.load(Ordering::Relaxed) {
                return Err("The browser helper stopped".into());
            }
            routes.insert(id, Arc::downgrade(&route));
        }
        worker.send(id, json!({"cmd":"create"}))?;
        let presenter = match &worker.link {
            Link::Socket(outbox) => Some(wpe::Presenter::new(outbox.clone(), id)),
            Link::Pipe(_) => None,
        };
        Ok(Self {
            worker,
            route,
            id,
            bounds: Bounds::default(),
            scale: 1.,
            geometry: None,
            presentation: Presentation::Live,
            image: None,
            image_scale: 1.,
            menu: None,
            menu_active: 0,
            preedit: String::new(),
            pressed: std::cell::Cell::new(None),
            presenter,
            #[cfg(feature = "browser-fixture")]
            log: FrameLog::default(),
        })
    }
    pub(super) fn engine(&self) -> Engine {
        self.worker.engine
    }
    pub fn load(&self, url: &str) -> Result<(), String> {
        self.worker.send(self.id, json!({"cmd":"load","url":url}))
    }
    pub fn reload(&self) {
        self.command(json!({"cmd":"reload"}));
    }
    pub fn history(&self, forward: bool) {
        self.command(json!({"cmd":if forward {"forward"} else {"back"}}));
    }
    pub fn state(&self) -> PageState {
        self.route.state.lock().unwrap().clone()
    }
    pub fn present(&mut self, presentation: Presentation) {
        if (self.presentation == Presentation::Hidden) != (presentation == Presentation::Hidden) {
            let visible = presentation != Presentation::Hidden;
            self.command(json!({"cmd":"visible","value":u8::from(visible)}));
            if visible {
                // WebKit may still want the vblanks hiding stopped, and then
                // never asks again: wake the surface to restart them.
                let _ = self.route.tx.try_send(NativeEvent::Vsync);
            }
        }
        if presentation == Presentation::Hidden && self.menu.take().is_some() {
            self.command(json!({"cmd":"dismiss-menu"}));
        }
        self.presentation = presentation;
    }
    pub fn update_frame(&mut self, window: &mut gpui::Window) {
        if let Some((frame, scale)) = self.route.frame.lock().unwrap().take() {
            self.image_scale = scale;
            if let Some(old) = self.image.replace(frame) {
                let _ = window.drop_image(old);
            }
        }
    }
    /// Paints the page's latest frame at `bounds`, the page's layout box.
    pub fn paint(&mut self, bounds: Bounds<Pixels>, focused: bool, window: &mut gpui::Window) {
        let Some(presenter) = &mut self.presenter else {
            let Some(image) = &self.image else {
                return;
            };
            #[cfg(feature = "browser-fixture")]
            if self.log.image.replace(image.id) != Some(image.id) {
                self.log.painted();
                self.log.copied(image.as_bytes(0).map_or(0, |b| b.len() as u64), false);
            }
            if let Some(bytes) = image.as_bytes(0).filter(|b| b.len() >= 4) {
                let color = gpui::rgb(
                    ((bytes[2] as u32) << 16) | ((bytes[1] as u32) << 8) | bytes[0] as u32,
                );
                window.paint_quad(gpui::fill(bounds, color));
            }
            let dimensions = image.size(0);
            // Keep the previous frame at its original scale while WebKit
            // reflows; clipping never stretches the page.
            let viewport = Bounds::new(
                bounds.origin,
                gpui::size(
                    gpui::px(dimensions.width.0 as f32 / self.image_scale),
                    gpui::px(dimensions.height.0 as f32 / self.image_scale),
                ),
            );
            let _ = window.paint_image(viewport, gpui::Corners::default(), image.clone(), 0, false);
            return;
        };
        presenter.sync_focus(focused, window.is_window_active());
        presenter.paint(&self.route, &mut self.image, bounds, window);
    }
    /// Whether WebKit gets vblanks: it wants them, and Zeron shows the page
    /// rather than hiding it or showing its error instead.
    fn needs_vblank(&self) -> bool {
        self.route.surface.lock().unwrap().vsync
            && self.presentation != Presentation::Hidden
            && self.route.state.lock().unwrap().error.is_none()
    }
    /// Whether GPUI should start feeding WebKit vblanks: it needs them and no
    /// frame callback chain is running yet.
    fn wants_vblank(&mut self) -> bool {
        let needed = self.needs_vblank();
        let Some(presenter) = self.presenter.as_mut() else {
            return false;
        };
        let start = needed && !presenter.ticking;
        presenter.ticking |= start;
        start
    }
    /// One GPUI frame callback. Returns whether to keep ticking.
    fn vblank(&mut self) -> bool {
        let needed = self.needs_vblank();
        let Some(presenter) = self.presenter.as_mut() else {
            return false;
        };
        if !needed {
            presenter.stop_ticking();
            return false;
        }
        presenter.vblank();
        true
    }
    fn cursor(&self) -> gpui::CursorStyle {
        self.route
            .surface
            .lock()
            .unwrap()
            .cursor
            .as_deref()
            .map_or(gpui::CursorStyle::Arrow, wpe::cursor_style)
    }
    pub fn sync(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        self.bounds = bounds;
        self.scale = scale;
        let geometry = (
            (f32::from(bounds.size.width) * scale)
                .round()
                .clamp(1., 8192.) as u32,
            (f32::from(bounds.size.height) * scale)
                .round()
                .clamp(1., 8192.) as u32,
            scale.to_bits(),
        );
        if self.geometry != Some(geometry) {
            self.command(
                json!({"cmd":"resize","width":geometry.0,"height":geometry.1,"scale":scale}),
            );
            self.geometry = Some(geometry);
        }
    }
    pub fn command(&self, value: Value) {
        let _ = self.worker.send(self.id, value);
    }
    /// Run `script` in the page; a string result arrives as
    /// [`NativeEvent::Evaluated`].
    pub fn run_script(&self, script: &str) {
        self.command(json!({"cmd":"eval","script":script}));
    }
    #[cfg(feature = "browser-fixture")]
    pub fn evaluate(&self, script: &str) {
        *self.route.evaluation.lock().unwrap() = None;
        self.command(json!({"cmd":"eval","script":script}));
    }
    #[cfg(feature = "browser-fixture")]
    pub fn evaluation(&self) -> Option<Value> {
        self.route.evaluation.lock().unwrap().take()
    }
    #[cfg(feature = "browser-fixture")]
    fn log(&mut self) -> &mut FrameLog {
        match &mut self.presenter {
            Some(presenter) => &mut presenter.log,
            None => &mut self.log,
        }
    }
}
impl Drop for NativePage {
    fn drop(&mut self) {
        self.command(json!({"cmd":"close"}));
        self.worker.routes.lock().unwrap().remove(&self.id);
    }
}

impl super::BrowserSurface {
    pub(super) fn start_linux_page(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Result<(), String> {
        if self.native_startup.is_some() {
            // navigate() has already updated page.url. Use that latest address
            // when startup completes instead of starting a second helper.
            return Ok(());
        }
        let ready = NativePage::start(
            self.context.data.clone(),
            self.native_tx.clone(),
            window.dmabuf_target(),
        )?;
        self.native_startup = Some(cx.spawn_in(window, async move |this, cx| {
            let result = ready
                .await
                .unwrap_or_else(|_| Err("Browser initialization stopped unexpectedly".into()));
            let _ = this.update_in(cx, |this, _, cx| {
                this.native_startup = None;
                let result = result.and_then(|mut native| {
                    native.present(this.presentation);
                    let result = this
                        .page
                        .url
                        .as_deref()
                        .map_or(Ok(()), |url| native.load(url))
                        // A helper that stopped before the page attached told
                        // no one.
                        .and(native.state().error.map_or(Ok(()), Err));
                    this.native = Some(native);
                    result
                });
                this.page.loading = result.is_ok();
                if let Err(error) = result {
                    this.page.error = Some(format!("Could not open this page: {error}"));
                }
                cx.emit(super::BrowserEvent::Changed);
                cx.notify();
            });
        }));
        Ok(())
    }

    pub(super) fn on_native_event(
        &mut self,
        event: NativeEvent,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        use gpui::Focusable;
        let Some(native) = &mut self.native else {
            return;
        };
        native.update_frame(window);
        if native.wants_vblank() {
            let entity = cx.entity().downgrade();
            window.on_next_display_frame(move |window, cx| vblank(entity, window, cx));
        }
        match event {
            NativeEvent::NewTab(url) => {
                if self.presentation == Presentation::Live {
                    cx.emit(super::BrowserEvent::NewTab(Some(url)));
                }
            }
            NativeEvent::Menu(menu) => {
                if self.presentation == Presentation::Live {
                    native.menu_active = menu["items"]
                        .as_array()
                        .and_then(|items| {
                            items
                                .iter()
                                .position(|item| item["selected"].as_bool().unwrap_or(false))
                        })
                        .unwrap_or(0);
                    native.menu = Some(menu);
                } else {
                    native.command(json!({"cmd":"dismiss-menu"}));
                }
            }
            NativeEvent::Clipboard(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            NativeEvent::Evaluated(value) => self.on_evaluated(value, cx),
            NativeEvent::Vsync => return,
            NativeEvent::Cursor => {
                let cursor = native.cursor();
                if self.page_cursor != cursor {
                    self.page_cursor = cursor;
                    cx.notify();
                }
                return;
            }
            NativeEvent::Frame | NativeEvent::Changed => {
                let mut page = native.state();
                if page.url.is_none() {
                    page.url = self.page.url.clone();
                }
                if page.error.is_some() {
                    page.loading = false;
                }
                if !self.address.focus_handle(cx).is_focused(window) {
                    if let Some(url) = &page.url {
                        if self.address.read(cx).text() != url {
                            self.address
                                .update(cx, |input, cx| input.set_text(url.clone(), cx));
                        }
                    }
                    self.address_edited = false;
                }
                if page != self.page {
                    self.page = page;
                    cx.emit(super::BrowserEvent::Changed);
                }
            }
        }
        cx.notify();
    }
    pub(super) fn linux_pointer(
        &self,
        kind: &str,
        position: gpui::Point<Pixels>,
        button: Option<gpui::MouseButton>,
        modifiers: gpui::Modifiers,
        clicks: usize,
    ) {
        let Some(native) = &self.native else {
            return;
        };
        if self.presentation != Presentation::Live {
            return;
        }
        if kind == "down" {
            native.pressed.set(button);
        }
        if kind == "up" && native.pressed.replace(None) != button {
            return;
        }
        let position = position - native.bounds.origin;
        native.command(json!({"cmd":kind,"x":f32::from(position.x),"y":f32::from(position.y),"button":button.map(mouse_button).unwrap_or(0),"count":clicks,"mods":modifiers_mask(modifiers) | if kind == "move" { button.map(|b| 1 << (mouse_button(b)+7)).unwrap_or(0) } else { 0 }}));
    }
    pub(super) fn linux_scroll(&self, event: &gpui::ScrollWheelEvent) -> bool {
        let Some(native) = &self.native else {
            return false;
        };
        native.command(match native.engine() {
            Engine::Wpe => wpe::scroll_command(native.bounds.origin, event),
            Engine::Gtk => {
                let delta = event.delta.pixel_delta(gpui::px(16.));
                let p = event.position - native.bounds.origin;
                json!({"cmd":"scroll","x":f32::from(p.x),"y":f32::from(p.y),"dx":-f32::from(delta.x)/40.,"dy":-f32::from(delta.y)/40.,"mods":modifiers_mask(event.modifiers)})
            }
        });
        true
    }
    /// WebKit reads the system clipboard only through Zeron: refresh its copy
    /// before anything that pastes. No text clears it, so a paste never
    /// inserts what an earlier one shared.
    pub(super) fn linux_share_clipboard(&self, cx: &gpui::App) {
        if let Some(native) = self.native.as_ref().filter(|n| n.engine() == Engine::Wpe) {
            let text = cx.read_from_clipboard().and_then(|item| item.text());
            native.command(json!({"cmd":"clipboard","text":text.unwrap_or_default()}));
        }
    }
    pub(super) fn linux_key(&self, stroke: &gpui::Keystroke, down: bool) {
        let Some(native) = &self.native else {
            return;
        };
        let printable = stroke.key_char.as_deref().filter(|s| {
            s.chars().count() == 1
                && !s.chars().any(char::is_control)
                && !stroke.modifiers.control
                && !stroke.modifiers.platform
        });
        let key = match printable.unwrap_or(stroke.key.as_str()) {
            "enter" => "Return",
            "backspace" => "BackSpace",
            "delete" => "Delete",
            "escape" => "Escape",
            "tab" => "Tab",
            "left" => "Left",
            "right" => "Right",
            "up" => "Up",
            "down" => "Down",
            "home" => "Home",
            "end" => "End",
            "pageup" => "Page_Up",
            "pagedown" => "Page_Down",
            "space" => "space",
            key => key,
        };
        native.command(json!({"cmd":if down {"key_down"} else {"key_up"},"key":key,"text":stroke.key_char.as_deref().unwrap_or(""),"mods":modifiers_mask(stroke.modifiers)}));
    }
}
/// Feeds WebKit one vblank per GPUI frame callback, which follow the
/// compositor's refresh, for as long as WebKit wants them. A page keeps the
/// display's rate while its window is in the background, like any browser.
fn vblank(entity: gpui::WeakEntity<super::BrowserSurface>, window: &mut gpui::Window, cx: &mut gpui::App) {
    let again = entity
        .update(cx, |this, _| this.native.as_mut().is_some_and(NativePage::vblank))
        .unwrap_or(false);
    if again {
        window.on_next_display_frame(move |window, cx| vblank(entity, window, cx));
    }
}
pub(super) fn modifiers_mask(m: gpui::Modifiers) -> u32 {
    u32::from(m.shift)
        | (u32::from(m.control) << 2)
        | (u32::from(m.alt) << 3)
        | (u32::from(m.platform) << 26)
}
fn mouse_button(button: gpui::MouseButton) -> u32 {
    match button {
        gpui::MouseButton::Left => 1,
        gpui::MouseButton::Middle => 2,
        gpui::MouseButton::Right => 3,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Back) => 8,
        gpui::MouseButton::Navigate(gpui::NavigationDirection::Forward) => 9,
    }
}

#[cfg(feature = "browser-fixture")]
impl super::BrowserSurface {
    pub fn fixture_linux_menu_open(&self) -> bool {
        self.native.as_ref().is_some_and(|n| n.menu.is_some())
    }
    pub fn fixture_linux_bounds(&self) -> Bounds<Pixels> {
        self.native.as_ref().unwrap().bounds
    }
    pub fn fixture_linux_evaluation(&self) -> Option<Value> {
        self.native.as_ref().unwrap().evaluation()
    }
    /// The frames GPUI painted for the page so far, the bytes copied into its
    /// tile from CPU memory and between GPU buffers, and when the latest was
    /// painted.
    pub fn fixture_linux_painted(&mut self) -> (u64, u64, u64, Option<std::time::Instant>) {
        let log = self.native.as_mut().unwrap().log();
        (log.painted, log.uploaded, log.copied, log.last)
    }
    /// Starts listing when each frame is painted; `false` stops and returns
    /// the list.
    pub fn fixture_linux_record(&mut self, on: bool) -> Vec<std::time::Instant> {
        let log = self.native.as_mut().unwrap().log();
        std::mem::replace(&mut log.times, on.then(Vec::new)).unwrap_or_default()
    }
    /// How WPE WebKit frames reach GPUI; `null` with WebKitGTK.
    pub fn fixture_linux_frames(&self) -> Value {
        let Some(native) = self.native.as_ref() else {
            return Value::Null;
        };
        let Some(presenter) = native.presenter.as_ref() else {
            return Value::Null;
        };
        let mut frames = presenter.fixture_frames();
        frames["shared"] = native.route.surface.lock().unwrap().fixture_buffers().into();
        frames
    }
}

impl gpui::EntityInputHandler for super::BrowserSurface {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        actual: &mut Option<std::ops::Range<usize>>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<String> {
        let native = self.native.as_ref()?;
        let input = native.route.input.lock().unwrap();
        let text = input["text"].as_str().unwrap_or("");
        let units: Vec<u16> = text.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(units.get(range)?))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        let native = self.native.as_ref()?;
        let input = native.route.input.lock().unwrap();
        let text = input["text"].as_str().unwrap_or("");
        let offset = |key: &str| {
            let n = input[key].as_u64().unwrap_or(0) as usize;
            text.get(..n).unwrap_or(text).encode_utf16().count()
        };
        let cursor = offset("cursor");
        let selection = offset("selection");
        Some(gpui::UTF16Selection {
            range: cursor.min(selection)..cursor.max(selection),
            reversed: cursor < selection,
        })
    }
    fn marked_text_range(
        &self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let native = self.native.as_ref()?;
        (!native.preedit.is_empty()).then(|| 0..native.preedit.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) {
        if let Some(native) = &mut self.native {
            native.command(json!({"cmd":"unmark"}));
            native.preedit.clear();
        }
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) {
        if let Some(native) = &mut self.native {
            native.command(json!({"cmd":"commit","text":text}));
            native.preedit.clear();
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) {
        if let Some(native) = &mut self.native {
            native.command(json!({"cmd":"preedit","text":text}));
            native.preedit = text.to_owned();
        }
    }
    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let native = self.native.as_ref()?;
        let input = native.route.input.lock().unwrap();
        let c = &input["caret"];
        Some(Bounds::new(
            native.bounds.origin
                + gpui::point(
                    gpui::px(c[0].as_f64().unwrap_or(0.) as f32 / native.scale),
                    gpui::px(c[1].as_f64().unwrap_or(0.) as f32 / native.scale),
                ),
            gpui::size(
                gpui::px(c[2].as_f64().unwrap_or(1.) as f32 / native.scale),
                gpui::px(c[3].as_f64().unwrap_or(18.) as f32 / native.scale),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> Option<usize> {
        None
    }
    fn accepts_text_input(&self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> bool {
        self.native.as_ref().is_some_and(|n| {
            n.route.input.lock().unwrap()["focused"]
                .as_bool()
                .unwrap_or(false)
        })
    }
}

impl super::BrowserSurface {
    pub(super) fn linux_dismiss_menu(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(native) = &mut self.native {
            native.menu = None;
            native.command(json!({"cmd":"dismiss-menu"}));
        }
        cx.notify();
    }
    pub(super) fn linux_choose_menu(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        let action = self.native.as_ref().and_then(|native| {
            let item = native.menu.as_ref()?["items"].get(index)?;
            item["enabled"].as_bool().unwrap_or(false).then_some(())?;
            item["action"].as_str().map(str::to_owned)
        });
        if let (Some(action), Some(native)) = (action, &self.native) {
            if action == "text" {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    native.command(json!({"cmd":"text","text":text}));
                }
            } else {
                if action == "paste" {
                    self.linux_share_clipboard(cx);
                }
                native.command(json!({"cmd":action}));
            }
        }
        self.linux_dismiss_menu(cx);
    }
    pub(super) fn linux_menu_key(&mut self, key: &str, cx: &mut gpui::Context<Self>) -> bool {
        let Some(native) = self.native.as_mut().filter(|n| n.menu.is_some()) else {
            return false;
        };
        match key {
            "escape" => self.linux_dismiss_menu(cx),
            "enter" => {
                let index = native.menu_active;
                self.linux_choose_menu(index, cx);
            }
            "up" | "down" => {
                if let Some(items) = native.menu.as_ref().unwrap()["items"]
                    .as_array()
                    .filter(|items| !items.is_empty())
                {
                    for _ in 0..items.len() {
                        native.menu_active = if key == "down" {
                            (native.menu_active + 1) % items.len()
                        } else {
                            (native.menu_active + items.len() - 1) % items.len()
                        };
                        if items[native.menu_active]["enabled"]
                            .as_bool()
                            .unwrap_or(false)
                        {
                            break;
                        }
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
        true
    }
    pub(super) fn linux_menu(
        &self,
        theme: &crate::theme::Theme,
        cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = &theme.for_popup();
        use gpui::{IntoElement, div, prelude::*, px};
        let native = self.native.as_ref()?;
        let menu = native.menu.as_ref()?;
        let items = menu["items"].as_array()?;
        let pos = native.bounds.origin
            + gpui::point(
                px(menu["x"].as_f64().unwrap_or(0.) as f32),
                px(menu["y"].as_f64().unwrap_or(0.) as f32),
            );
        let mut content = crate::popover::popover_card(theme)
            .w(px(240.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.linux_dismiss_menu(cx)));
        let mut rows = div()
            .id("browser-menu-options")
            .flex()
            .flex_col()
            .max_h(px(320.))
            .overflow_y_scroll();
        for (index, item) in items.iter().enumerate() {
            let enabled = item["enabled"].as_bool().unwrap_or(false);
            let row = crate::popover::menu_row(
                theme,
                index == native.menu_active,
                format!("browser-option-{index}"),
            )
            .id(("browser-option", index))
            .child(gpui::SharedString::from(
                item["label"].as_str().unwrap_or("").to_owned(),
            ))
            .when(!enabled, |el| el.opacity(0.4))
            .when(enabled, |el| {
                el.on_click(cx.listener(move |this, _, _, cx| this.linux_choose_menu(index, cx)))
            });
            rows = rows.child(row);
        }
        content = content.child(rows);
        Some(crate::popover::menu_at(
            "browser-page-menu",
            pos,
            content.into_any_element(),
            None,
        ))
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;

    #[test]
    fn startup_surfaces_storage_errors_and_rejects_invalid_packets() {
        assert!(read_startup(&mut &b"R\0\0\0\0\0\0\0\0"[..], "WebKitGTK 4.1").is_ok());
        let mut packet = b"E\0\0\0\0".to_vec();
        packet.extend_from_slice(&6u32.to_le_bytes());
        packet.extend_from_slice(b"locked");
        assert_eq!(
            read_startup(&mut packet.as_slice(), "WebKitGTK 4.1"),
            Err(Startup::Storage("locked".into()))
        );
        assert!(read_startup(&mut &b"R\0"[..], "WebKitGTK 4.1").is_err());
        assert!(read_startup(&mut &b"R\0\0\0\0\xff\xff\xff\xff"[..], "WebKitGTK 4.1").is_err());
    }

    fn profile_contexts_share_a_live_helper_and_release_its_lock_on_shutdown(engine: Engine) {
        let root = tempfile::tempdir().unwrap();
        let profile = super::super::profile::BrowserProfile::for_workspace(
            root.path(),
            Some(zeron_proto::WorkspaceScope::Local),
            None,
            Some("device"),
        )
        .unwrap();
        let data = |profile| BrowserData {
            engine_override: Some(engine),
            ..BrowserData::for_profile(profile)
        };
        let first = data(profile.clone());
        let second = data(profile.clone());
        let worker = first.worker(None).unwrap();
        assert_eq!(worker.engine, engine);
        let pid = worker.child.lock().unwrap().id();
        drop(worker);
        // No page owns the worker. The profile keeps it alive between tabs.
        assert_eq!(pid, first.worker(None).unwrap().child.lock().unwrap().id());
        assert_eq!(pid, second.worker(None).unwrap().child.lock().unwrap().id());
        drop(first);
        assert_eq!(pid, second.worker(None).unwrap().child.lock().unwrap().id());
        drop(second);
        // Immediate reopening must acquire the released process lock.
        let reopened = data(profile);
        let worker = reopened.worker(None).unwrap();
        assert_ne!(pid, worker.child.lock().unwrap().id());
        drop(worker);
        drop(reopened);
    }

    #[test]
    #[ignore = "requires a real WebKitGTK runtime and display (xvfb-run)"]
    fn gtk_profiles_share_a_live_helper_and_release_its_lock_on_shutdown() {
        profile_contexts_share_a_live_helper_and_release_its_lock_on_shutdown(Engine::Gtk);
    }

    #[test]
    #[ignore = "requires the WPE WebKit runtime"]
    fn wpe_profiles_share_a_live_helper_and_release_its_lock_on_shutdown() {
        profile_contexts_share_a_live_helper_and_release_its_lock_on_shutdown(Engine::Wpe);
    }
}

#[cfg(test)]
mod startup_tests;
