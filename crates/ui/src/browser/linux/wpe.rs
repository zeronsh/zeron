//! WPE WebKit engine. Its helper (`wpe.c`) is Zeron's WPEPlatform: pages render
//! into dma-bufs on GPUI's GPU, GPUI copies only their damage into a persistent
//! atlas tile, and GPUI's frame callbacks are the pages' vblank. Shared-memory
//! mirrors stand in when the helper cannot render on GPUI's GPU.
use super::{NativeEvent, Route};
use gpui::{AtlasTileUpdate, Bounds, DevicePixels, RenderImage, Window, point, size};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write as _},
    os::{
        fd::{AsFd as _, AsRawFd as _, FromRawFd as _, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::Instant,
};

pub(super) const HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zeron-wpe"));
/// Largest packet either side sends (page script results, copied text, the
/// clipboard Zeron shares); the helper drops larger ones.
const MAX_PACKET: usize = 64 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
/// gpui_linux scales Wayland axis values and wheel clicks by three; WebKit
/// applies its own scroll distances to the unscaled values.
const GPUI_SCROLL_SCALE: f32 = 3.0;
const FOURCC_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
pub(super) const DRM_FORMAT_MOD_LINEAR: u64 = 0;

/// Commands to the helper, written by a dedicated thread: GPU completion
/// callbacks and the UI thread must never block on a busy helper.
#[derive(Clone)]
pub(super) struct Outbox(mpsc::Sender<Vec<u8>>);

impl Outbox {
    pub(super) fn send(&self, id: u32, mut command: Value) -> Result<(), String> {
        command["id"] = id.into();
        let data = serde_json::to_vec(&command).map_err(|e| e.to_string())?;
        if data.len() > MAX_PACKET {
            return Err("Browser command is too large".into());
        }
        let mut framed = Vec::with_capacity(data.len() + 4);
        framed.extend_from_slice(&(data.len() as u32).to_le_bytes());
        framed.extend_from_slice(&data);
        self.0
            .send(framed)
            .map_err(|_| "Browser helper is shutting down".into())
    }
    /// Closes the helper's input even while completions still hold clones.
    pub(super) fn close(&self) {
        let _ = self.0.send(Vec::new());
    }
}

pub(super) fn writer(socket: UnixStream) -> Outbox {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("browser-commands".into())
        .spawn(move || {
            let mut socket = socket;
            for message in rx {
                if message.is_empty() || socket.write_all(&message).is_err() {
                    break;
                }
            }
            let _ = socket.shutdown(std::net::Shutdown::Write);
        })
        .expect("browser command thread");
    Outbox(tx)
}

/// Reads packets and the descriptors attached to them. A descriptor arrives
/// with the first byte of its packet, so it is queued no later than the packet
/// that claims it completes.
pub(super) struct Reader {
    socket: UnixStream,
    data: Vec<u8>,
    fds: VecDeque<OwnedFd>,
}

impl Reader {
    pub(super) fn new(socket: UnixStream) -> Self {
        Self {
            socket,
            data: Vec::new(),
            fds: VecDeque::new(),
        }
    }

    pub(super) fn next(&mut self) -> io::Result<(u8, u32, Vec<u8>)> {
        loop {
            if self.data.len() >= 9 {
                let length = u32::from_le_bytes(self.data[5..9].try_into().unwrap()) as usize;
                if length > MAX_PACKET {
                    return Err(io::Error::other("Browser packet is too large"));
                }
                if self.data.len() >= 9 + length {
                    let kind = self.data[0];
                    let id = u32::from_le_bytes(self.data[1..5].try_into().unwrap());
                    let payload = self.data[9..9 + length].to_vec();
                    self.data.drain(..9 + length);
                    return Ok((kind, id, payload));
                }
            }
            self.receive()?;
        }
    }

    pub(super) fn take_fd(&mut self) -> io::Result<OwnedFd> {
        self.fds
            .pop_front()
            .ok_or_else(|| io::Error::other("Browser packet is missing its descriptor"))
    }

    fn receive(&mut self) -> io::Result<()> {
        let mut chunk = vec![0u8; 64 * 1024];
        // Room for 16 descriptors, aligned for cmsghdr.
        let mut control = [0u64; 16];
        let mut iov = libc::iovec {
            iov_base: chunk.as_mut_ptr().cast(),
            iov_len: chunk.len(),
        };
        // SAFETY: an all-zero msghdr is valid; the pointers outlive the call.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = std::mem::size_of_val(&control) as _;
        let n = loop {
            // SAFETY: `message` describes buffers owned by this frame.
            let n = unsafe {
                libc::recvmsg(
                    self.socket.as_raw_fd(),
                    &mut message,
                    libc::MSG_CMSG_CLOEXEC,
                )
            };
            if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break n;
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the kernel filled `control`; the CMSG macros walk it in bounds.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let bytes = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                    let data = libc::CMSG_DATA(header) as *const RawFd;
                    for i in 0..bytes / std::mem::size_of::<RawFd>() {
                        self.fds
                            .push_back(OwnedFd::from_raw_fd(data.add(i).read_unaligned()));
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        if message.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::other("Browser helper sent too many descriptors"));
        }
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.data.extend_from_slice(&chunk[..n as usize]);
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Dmabuf,
    Shm,
}

/// A buffer the helper shares once; frames name it by id.
pub(super) struct Buffer {
    pub(super) kind: Kind,
    fd: OwnedFd,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride: u32,
    pub(super) offset: u32,
    fourcc: u32,
    /// How a dma-buf's pixels are laid out; mirrors are linear.
    pub(super) modifier: u64,
    map: OnceLock<Option<Mapping>>,
}

impl Buffer {
    pub(super) fn parse(info: &Value, fd: OwnedFd) -> Option<(u32, Self)> {
        let field = |name: &str| info[name].as_u64().and_then(|v| u32::try_from(v).ok());
        let kind = match info["kind"].as_str()? {
            "dmabuf" => Kind::Dmabuf,
            "shm" => Kind::Shm,
            _ => return None,
        };
        let modifier = match kind {
            // DRM vendors fill the top byte, so modifiers can exceed i64.
            Kind::Dmabuf => {
                let value = &info["modifier"];
                value
                    .as_u64()
                    .or_else(|| value.as_i64().map(|v| v as u64))?
            }
            Kind::Shm => DRM_FORMAT_MOD_LINEAR,
        };
        let buffer = Self {
            kind,
            fd,
            width: field("width")?,
            height: field("height")?,
            stride: field("stride")?,
            offset: field("offset")?,
            fourcc: field("fourcc")?,
            modifier,
            map: OnceLock::new(),
        };
        let valid = (1..=MAX_DIMENSION).contains(&buffer.width)
            && (1..=MAX_DIMENSION).contains(&buffer.height)
            && buffer.stride >= buffer.width * 4
            && buffer.offset.is_multiple_of(4)
            && buffer.fourcc == FOURCC_ARGB8888;
        valid.then_some((field("buffer")?, buffer))
    }

    fn len(&self) -> usize {
        self.offset as usize + self.stride as usize * self.height as usize
    }

    /// Maps a shared-memory mirror. The helper seals it against resizing, so
    /// the mapping can never fault.
    fn pixels(&self) -> Option<&[u8]> {
        self.map
            .get_or_init(|| Mapping::new(&self.fd, self.len()))
            .as_ref()
            .map(|map| map.bytes())
    }

    fn size(&self) -> gpui::Size<DevicePixels> {
        size(
            DevicePixels(self.width as i32),
            DevicePixels(self.height as i32),
        )
    }
}

struct Mapping(*mut libc::c_void, usize);
// SAFETY: a read-only shared mapping can be read from any thread.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
impl Mapping {
    fn new(fd: &OwnedFd, len: usize) -> Option<Self> {
        let fd = fd.as_raw_fd();
        // SAFETY: plain fcntl/fstat/mmap on a descriptor this buffer owns.
        unsafe {
            let seals = libc::fcntl(fd, libc::F_GET_SEALS);
            let mut stat: libc::stat = std::mem::zeroed();
            if seals < 0
                || seals & libc::F_SEAL_SHRINK == 0
                || libc::fstat(fd, &mut stat) < 0
                || (stat.st_size as usize) < len
            {
                return None;
            }
            let map = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            (map != libc::MAP_FAILED).then_some(Self(map, len))
        }
    }
    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping is `self.1` readable bytes until dropped.
        unsafe { std::slice::from_raw_parts(self.0.cast(), self.1) }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what `new` mapped.
        unsafe { libc::munmap(self.0, self.1) };
    }
}

/// A frame waiting for GPUI's next paint: the buffer holds the whole page,
/// `damage` what changed since the frame GPUI painted before it.
pub(super) struct Frame {
    buffer: u32,
    scale: f32,
    damage: Vec<Bounds<DevicePixels>>,
}

impl Frame {
    pub(super) fn parse(value: &Value) -> Option<Self> {
        let scale = value["scale"].as_f64()? as f32;
        if !scale.is_finite() || !(0.5..=4.).contains(&scale) {
            return None;
        }
        let damage = value["damage"]
            .as_array()?
            .iter()
            .filter_map(|rect| {
                let n = |i: usize| rect.get(i)?.as_i64().map(|v| v.clamp(0, 8192) as i32);
                Some(Bounds::new(
                    point(DevicePixels(n(0)?), DevicePixels(n(1)?)),
                    size(DevicePixels(n(2)?), DevicePixels(n(3)?)),
                ))
            })
            .collect();
        Some(Self {
            buffer: u32::try_from(value["buffer"].as_u64()?).ok()?,
            scale,
            damage,
        })
    }
}

/// What the helper shared with a page, read by GPUI's paint.
#[derive(Default)]
pub(super) struct Surface {
    buffers: HashMap<u32, Arc<Buffer>>,
    frame: Option<Frame>,
    forgotten: Vec<u32>,
    pub(super) vsync: bool,
    pub(super) cursor: Option<String>,
}

impl Surface {
    pub(super) fn add_buffer(&mut self, id: u32, buffer: Buffer) {
        self.buffers.insert(id, Arc::new(buffer));
    }
    pub(super) fn forget_buffer(&mut self, id: u32) {
        self.buffers.remove(&id);
        self.forgotten.push(id);
    }
    /// Keeps only the latest frame. One GPUI never painted adds its damage to
    /// the next, whose buffer holds the whole page anyway; its buffer goes
    /// back to WebKit unread, unless it is a mirror the helper never lent.
    pub(super) fn push_frame(&mut self, mut frame: Frame, outbox: &Outbox, page: u32) {
        if let Some(previous) = self.frame.take() {
            if previous.buffer != frame.buffer
                && !self
                    .buffers
                    .get(&previous.buffer)
                    .is_some_and(|b| b.kind == Kind::Shm)
            {
                let _ = outbox.send(page, json!({"cmd":"released","buffer":previous.buffer}));
            }
            frame.damage.extend(previous.damage);
            if frame.damage.len() > 16 {
                let all = frame.damage.iter().copied().reduce(|a, b| a.union(&b));
                frame.damage = all.into_iter().collect();
            }
        }
        self.frame = Some(frame);
    }
}

/// Handles the packets only this engine sends. Returns the event to wake the
/// page with, if any.
pub(super) fn packet(
    route: &Route,
    outbox: &Outbox,
    page: u32,
    kind: u8,
    data: &[u8],
    reader: &mut Reader,
) -> io::Result<Option<NativeEvent>> {
    let json = || serde_json::from_slice::<Value>(data).ok();
    Ok(match kind {
        b'B' => {
            let fd = reader.take_fd()?;
            if let Some((id, buffer)) = json().and_then(|info| Buffer::parse(&info, fd)) {
                route.surface.lock().unwrap().add_buffer(id, buffer);
            }
            None
        }
        b'X' => {
            if let Some(id) = json().and_then(|v| v["buffer"].as_u64()) {
                route.surface.lock().unwrap().forget_buffer(id as u32);
            }
            None
        }
        b'F' => {
            let frame = json().as_ref().and_then(Frame::parse);
            let frame = frame.ok_or_else(|| io::Error::other("Invalid browser frame"))?;
            route
                .surface
                .lock()
                .unwrap()
                .push_frame(frame, outbox, page);
            Some(NativeEvent::Frame)
        }
        b'V' => {
            let on = json().is_some_and(|v| v["on"].as_bool() == Some(true));
            route.surface.lock().unwrap().vsync = on;
            Some(NativeEvent::Vsync)
        }
        b'K' => {
            route.surface.lock().unwrap().cursor = Some(String::from_utf8_lossy(data).into_owned());
            Some(NativeEvent::Cursor)
        }
        _ => None,
    })
}

/// Tells `done` of each display refresh of `window` until it is idle,
/// acknowledging the frame it deferred.
fn refreshes(window: &Window, done: Arc<Mutex<FrameDone>>, outbox: Outbox, page: u32) {
    window.on_next_display_frame(move |window, _| {
        let (ack, idle) = {
            let mut done = done.lock().unwrap();
            (done.refresh(), done.idle())
        };
        if ack {
            let _ = outbox.send(page, json!({"cmd":"presented"}));
        }
        if !idle {
            refreshes(window, done, outbox, page);
        }
    });
}

/// The frame the page's tile shows. Like a compositor's front buffer, it is
/// held until a newer frame replaces it, so the tile can be refilled (another
/// window's atlas, a lost GPU) without WebKit rendering again.
struct Shown {
    id: u32,
    buffer: Arc<Buffer>,
    scale: f32,
}

/// When WebKit hears that a frame is on screen, which is when it renders the
/// next one. Like a compositor's frame callback, that is at most once per
/// display refresh, however often GPUI paints.
#[derive(Default)]
struct FrameDone {
    /// A frame was acknowledged during this refresh.
    acked: bool,
    /// A frame painted after it waits for the next refresh.
    deferred: bool,
}

impl FrameDone {
    /// GPUI painted a frame: whether to acknowledge it now. The first of a
    /// refresh is, so WebKit has the whole refresh to render the next one.
    fn painted(&mut self) -> bool {
        if self.acked {
            self.deferred = true;
            false
        } else {
            self.acked = true;
            true
        }
    }

    /// A display refresh began: whether to acknowledge the deferred frame now.
    fn refresh(&mut self) -> bool {
        self.acked = std::mem::take(&mut self.deferred);
        self.acked
    }

    fn idle(&self) -> bool {
        !self.acked && !self.deferred
    }
}

/// GPUI-side presentation of one page: its imports, and the vblank it drives.
pub(super) struct Presenter {
    outbox: Outbox,
    page: u32,
    imports: HashMap<u32, gpui::DmabufImport>,
    window: Option<gpui::AnyWindowHandle>,
    shown: Option<Shown>,
    done: Arc<Mutex<FrameDone>>,
    /// The helper sends frames through shared memory from now on.
    readback: bool,
    /// The tile does not hold the shown frame: copy the next one whole.
    stale: bool,
    /// The tile missed a frame's damage, which the next frame's leaves out:
    /// copy that one whole too, still showing the tile meanwhile.
    missed: bool,
    pub(super) ticking: bool,
    last_tick: Option<Instant>,
    frame_interval: f64,
    refresh_mhz: u32,
    focused: Option<bool>,
    active: Option<bool>,
    #[cfg(feature = "browser-fixture")]
    pub(super) log: super::FrameLog,
}

impl Presenter {
    pub(super) fn new(outbox: Outbox, page: u32) -> Self {
        Self {
            outbox,
            page,
            imports: HashMap::new(),
            window: None,
            shown: None,
            done: Arc::default(),
            readback: false,
            stale: true,
            missed: false,
            ticking: false,
            last_tick: None,
            frame_interval: 0.,
            refresh_mhz: 60_000,
            focused: None,
            active: None,
            #[cfg(feature = "browser-fixture")]
            log: Default::default(),
        }
    }

    fn send(&self, command: Value) {
        let _ = self.outbox.send(self.page, command);
    }

    pub(super) fn sync_focus(&mut self, focused: bool, active: bool) {
        if self.focused.replace(focused) != Some(focused) {
            self.send(json!({"cmd":"focus","value":u8::from(focused)}));
        }
        if self.active.replace(active) != Some(active) {
            self.send(json!({"cmd":"active","value":u8::from(active)}));
        }
    }

    /// Runs in each GPUI frame callback while WebKit wants vblanks; GPUI's
    /// interval also tells WebKit the display's refresh rate.
    pub(super) fn vblank(&mut self) {
        self.vblank_at(Instant::now());
    }

    fn vblank_at(&mut self, now: Instant) {
        if let Some(last) = self.last_tick.replace(now) {
            let interval = now.duration_since(last).as_secs_f64();
            // Ignore stalls; follow refresh changes within ~30 frames.
            if interval > 0.001 && interval < 0.05 {
                self.frame_interval = if self.frame_interval == 0. {
                    interval
                } else {
                    self.frame_interval * 0.97 + interval * 0.03
                };
                let mhz = (1000. / self.frame_interval).round() as u32;
                if mhz.abs_diff(self.refresh_mhz) * 20 > self.refresh_mhz {
                    self.refresh_mhz = mhz;
                    self.send(json!({"cmd":"refresh","mhz":mhz}));
                }
            }
        }
        self.send(json!({"cmd":"vblank"}));
    }

    /// Acknowledges a painted frame as [`FrameDone`] decides.
    fn frame_done(&self, window: &Window) {
        let (idle, ack) = {
            let mut done = self.done.lock().unwrap();
            (done.idle(), done.painted())
        };
        if ack {
            self.send(json!({"cmd":"presented"}));
        }
        if idle {
            refreshes(window, self.done.clone(), self.outbox.clone(), self.page);
        }
    }

    pub(super) fn stop_ticking(&mut self) {
        self.ticking = false;
        self.last_tick = None;
    }

    /// Applies the page's latest frame to `image`, then paints it.
    pub(super) fn paint(
        &mut self,
        route: &Route,
        image: &mut Option<Arc<RenderImage>>,
        bounds: Bounds<gpui::Pixels>,
        window: &mut Window,
    ) {
        let next = {
            let mut surface = route.surface.lock().unwrap();
            for id in std::mem::take(&mut surface.forgotten) {
                self.imports.remove(&id);
            }
            surface.frame.take().map(|frame| {
                let buffer = surface.buffers.get(&frame.buffer).cloned();
                (frame, buffer)
            })
        };
        let handle = window.window_handle();
        if self.window != Some(handle) {
            // Imports and tiles belong to one window's atlas.
            if self.window.replace(handle).is_some() {
                // The old window may never refresh again: closed, or hidden.
                // WebKit would wait on it forever.
                self.done = Arc::default();
                self.send(json!({"cmd":"presented"}));
            }
            self.imports.clear();
            *image = None;
        }
        match next {
            Some((frame, Some(buffer))) => self.apply(frame, buffer, image, window),
            // A buffer GPUI rejected, or one forgotten before GPUI got to it.
            Some((frame, None)) => {
                self.missed = true;
                self.send(json!({"cmd":"released","buffer":frame.buffer}));
                self.frame_done(window);
            }
            None => {
                // An atlas can drop tiles (a cleared atlas, a lost GPU): one
                // GPUI recreates holds nothing until refilled.
                let lost = image
                    .as_ref()
                    .is_some_and(|image| window.insert_blank_image(image, 0).unwrap_or(false));
                if image.is_none() || self.stale || lost {
                    self.refill(image, window);
                }
            }
        }
        if let (Some(image), Some(shown)) = (image.as_ref(), &self.shown)
            && !self.stale
        {
            let pixels = image.size(0);
            let viewport = Bounds::new(
                bounds.origin,
                size(
                    gpui::px(pixels.width.0 as f32 / shown.scale),
                    gpui::px(pixels.height.0 as f32 / shown.scale),
                ),
            );
            let _ = window.paint_image(viewport, Default::default(), image.clone(), 0, false);
        }
    }

    /// The tile's image, recreated for a new size: a blank image's first
    /// update allocates its tile without uploading anything.
    fn image_for(
        &mut self,
        buffer: &Buffer,
        image: &mut Option<Arc<RenderImage>>,
        window: &mut Window,
    ) -> Arc<RenderImage> {
        let pixels = buffer.size();
        if image.as_ref().map(|image| image.size(0)) != Some(pixels) {
            if let Some(old) = image.take() {
                let _ = window.drop_image(old);
            }
            *image = Some(Arc::new(RenderImage::blank(pixels)));
            self.stale = true;
        }
        image.clone().unwrap()
    }

    /// Copies the shown frame whole into a tile that lost it.
    fn refill(&mut self, image: &mut Option<Arc<RenderImage>>, window: &mut Window) {
        let Some((id, buffer)) = self.shown.as_ref().map(|s| (s.id, s.buffer.clone())) else {
            return;
        };
        let target = self.image_for(&buffer, image, window);
        match self.copy(window, &target, &buffer, id, None, None) {
            Ok(()) => self.stale = false,
            Err(error) => tracing::warn!("browser frame could not be restored: {error:#}"),
        }
    }

    fn apply(
        &mut self,
        frame: Frame,
        buffer: Arc<Buffer>,
        image: &mut Option<Arc<RenderImage>>,
        window: &mut Window,
    ) {
        let target = self.image_for(&buffer, image, window);
        let damage = (!self.stale && !self.missed).then_some(&frame.damage[..]);
        // The previous frame's buffer goes back to WebKit once the GPU has
        // finished these copies, which run after any still reading it.
        let previous = self
            .shown
            .take()
            .filter(|shown| shown.buffer.kind == Kind::Dmabuf && shown.id != frame.buffer);
        let release = previous.map(|previous| {
            let outbox = self.outbox.clone();
            let page = self.page;
            Box::new(move || {
                let _ = outbox.send(page, json!({"cmd":"released","buffer":previous.id}));
            }) as Box<dyn FnOnce() + Send>
        });
        match self.copy(window, &target, &buffer, frame.buffer, damage, release) {
            Ok(()) => {
                self.stale = false;
                self.missed = false;
                self.shown = Some(Shown {
                    id: frame.buffer,
                    buffer,
                    scale: frame.scale,
                });
                self.frame_done(window);
                #[cfg(feature = "browser-fixture")]
                self.log.painted();
            }
            Err(error) if buffer.kind == Kind::Dmabuf && !self.readback => {
                // The helper resends this frame, and every later one, through
                // shared memory.
                tracing::warn!("browser frames fall back to shared memory: {error:#}");
                self.readback = true;
                self.stale = true;
                self.send(json!({"cmd":"readback","buffer":frame.buffer}));
            }
            Err(error) => {
                tracing::warn!("browser frame dropped: {error:#}");
                self.stale = true;
                self.frame_done(window);
                if buffer.kind == Kind::Dmabuf {
                    self.send(json!({"cmd":"released","buffer":frame.buffer}));
                }
            }
        }
    }

    /// Copies `damage` of `buffer` into `image`, or all of it. `release` runs
    /// exactly once, after the GPU stops reading anything queued so far.
    fn copy(
        &mut self,
        window: &mut Window,
        image: &RenderImage,
        buffer: &Buffer,
        id: u32,
        damage: Option<&[Bounds<DevicePixels>]>,
        mut release: Option<Box<dyn FnOnce() + Send>>,
    ) -> anyhow::Result<()> {
        let result = (|| {
            let whole = Bounds::new(point(DevicePixels(0), DevicePixels(0)), buffer.size());
            // A new tile, or one the atlas recreated, needs the whole frame.
            let created = window.insert_blank_image(image, 0)?;
            let mut regions: Vec<_> = damage
                .filter(|_| !created)
                .unwrap_or(std::slice::from_ref(&whole))
                .iter()
                .map(|region| region.intersect(&whole))
                .filter(|region| region.size.width.0 > 0 && region.size.height.0 > 0)
                .collect();
            // Something must carry `release` past the GPU's last read.
            if regions.is_empty() {
                regions.push(whole);
            }
            let result = match buffer.kind {
                Kind::Shm => Self::copy_pixels(window, image, buffer, &regions, &mut release),
                Kind::Dmabuf => {
                    // An import that worked before only fails once the GPU device
                    // was lost: import again once before giving up on the GPU path.
                    let fresh = !self.imports.contains_key(&id);
                    match self.copy_imported(window, image, buffer, id, &regions, &mut release) {
                        Err(_) if !fresh => {
                            self.imports.clear();
                            let whole = [whole];
                            self.copy_imported(window, image, buffer, id, &whole, &mut release)
                        }
                        result => result,
                    }
                }
            };
            #[cfg(feature = "browser-fixture")]
            if result.is_ok() {
                let pixels: i64 = regions
                    .iter()
                    .map(|r| i64::from(r.size.width.0) * i64::from(r.size.height.0))
                    .sum();
                self.log
                    .copied(pixels as u64 * 4, buffer.kind == Kind::Dmabuf);
            }
            result
        })();
        if let Some(release) = release {
            // An error came before any update took it. The atlas still runs a
            // failed update's completion after the GPU's earlier work, which
            // may read the buffer `release` returns.
            let nothing = AtlasTileUpdate::Pixels {
                bytes: &[],
                bytes_per_row: 0,
            };
            let _ = window.update_image(image, 0, Bounds::default(), nothing, Some(release));
        }
        result
    }

    fn copy_pixels(
        window: &mut Window,
        image: &RenderImage,
        buffer: &Buffer,
        regions: &[Bounds<DevicePixels>],
        release: &mut Option<Box<dyn FnOnce() + Send>>,
    ) -> anyhow::Result<()> {
        let pixels = buffer
            .pixels()
            .ok_or_else(|| anyhow::anyhow!("the frame mirror cannot be mapped"))?;
        for region in regions {
            let start = buffer.offset as usize
                + region.origin.y.0 as usize * buffer.stride as usize
                + region.origin.x.0 as usize * 4;
            let update = AtlasTileUpdate::Pixels {
                bytes: &pixels[start..],
                bytes_per_row: buffer.stride,
            };
            anyhow::ensure!(
                window.update_image(image, 0, *region, update, release.take())?,
                "the atlas cannot update browser frames in place"
            );
        }
        Ok(())
    }

    fn copy_imported(
        &mut self,
        window: &mut Window,
        image: &RenderImage,
        buffer: &Buffer,
        id: u32,
        regions: &[Bounds<DevicePixels>],
        release: &mut Option<Box<dyn FnOnce() + Send>>,
    ) -> anyhow::Result<()> {
        let import = match self.imports.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(window.import_dmabuf(&gpui::Dmabuf {
                    fd: buffer.fd.as_fd(),
                    width: buffer.width,
                    height: buffer.height,
                    fourcc: buffer.fourcc,
                    modifier: buffer.modifier,
                    offset: buffer.offset,
                    stride: buffer.stride,
                })?)
            }
        };
        let source = import.id();
        for region in regions {
            // The tile holds the whole page, as the buffer does.
            let update = AtlasTileUpdate::Dmabuf {
                buffer: source,
                origin: region.origin,
            };
            // All copies share one submission; the first carries the release.
            anyhow::ensure!(
                window.update_image(image, 0, *region, update, release.take())?,
                "the atlas cannot copy dma-bufs"
            );
        }
        Ok(())
    }
}

#[cfg(feature = "browser-fixture")]
impl Surface {
    /// Buffers the helper shares and has not forgotten yet.
    pub(super) fn fixture_buffers(&self) -> usize {
        self.buffers.len()
    }
}

#[cfg(feature = "browser-fixture")]
impl Presenter {
    /// How the shown frame reached the tile.
    pub(super) fn fixture_frames(&self) -> Value {
        let shown = self.shown.as_ref().map(|shown| &shown.buffer);
        json!({
            "kind": shown.map(|b| if b.kind == Kind::Dmabuf { "dmabuf" } else { "shm" }),
            "modifier": shown.map(|b| format!("{:#018x}", b.modifier)),
            "size": shown.map(|b| [b.width, b.height]),
            "imports": self.imports.len(),
            "readback": self.readback,
            // How long ago WebKit got its last vblank, while it gets them.
            "vblankMs": self.last_tick.map(|tick| tick.elapsed().as_secs_f64() * 1000.),
        })
    }
}

/// A GPUI scroll over a page whose top-left corner is at `origin`, as WPE's
/// axis event: precise for touchpads, discrete steps for wheels.
pub(super) fn scroll_command(
    origin: gpui::Point<gpui::Pixels>,
    event: &gpui::ScrollWheelEvent,
) -> Value {
    let position = event.position - origin;
    let (delta, precise) = match event.delta {
        gpui::ScrollDelta::Pixels(delta) => (point(f32::from(delta.x), f32::from(delta.y)), true),
        gpui::ScrollDelta::Lines(delta) => (delta, false),
    };
    json!({
        "cmd": "scroll",
        "x": f32::from(position.x),
        "y": f32::from(position.y),
        "dx": delta.x / GPUI_SCROLL_SCALE,
        "dy": delta.y / GPUI_SCROLL_SCALE,
        "precise": precise,
        "stop": event.touch_phase == gpui::TouchPhase::Ended,
        "mods": super::modifiers_mask(event.modifiers),
    })
}

/// CSS cursor names WebKit reports, as GPUI cursors.
pub(super) fn cursor_style(name: &str) -> gpui::CursorStyle {
    use gpui::CursorStyle::*;
    match name {
        "pointer" => PointingHand,
        "text" => IBeam,
        "vertical-text" => IBeamCursorForVerticalLayout,
        "crosshair" | "cell" => Crosshair,
        "grab" => OpenHand,
        "grabbing" | "move" | "all-scroll" => ClosedHand,
        "not-allowed" | "no-drop" => OperationNotAllowed,
        "alias" => DragLink,
        "copy" => DragCopy,
        "context-menu" => ContextualMenu,
        "col-resize" => ResizeColumn,
        "row-resize" => ResizeRow,
        "ew-resize" => ResizeLeftRight,
        "ns-resize" => ResizeUpDown,
        "e-resize" => ResizeRight,
        "w-resize" => ResizeLeft,
        "n-resize" => ResizeUp,
        "s-resize" => ResizeDown,
        "nesw-resize" | "ne-resize" | "sw-resize" => ResizeUpRightDownLeft,
        "nwse-resize" | "nw-resize" | "se-resize" => ResizeUpLeftDownRight,
        // GPUI cannot hide the cursor: `none` keeps the arrow.
        _ => Arrow,
    }
}

#[cfg(test)]
#[path = "wpe_tests.rs"]
mod tests;
