//! The WPE engine's protocol pieces, and the real helper this build embeds:
//! `cargo test -p zeron-ui wpe -- --include-ignored` with WPE WebKit
//! installed. The helper renders offscreen, so no display is needed.
use super::super::{
    BrowserData, Engine, Link, NativePage, Presentation, Route, Startup, Worker, extract_helper,
    spawn_helper, spawn_wpe,
};
use super::*;
use gpui::px;
use std::{
    io::Read as _,
    net::TcpListener,
    path::PathBuf,
    sync::{Mutex, atomic::Ordering},
    time::Duration,
};

fn memfd(bytes: &[u8], seal: bool) -> OwnedFd {
    // SAFETY: plain memfd_create/fcntl on descriptors this test owns.
    unsafe {
        let fd = libc::memfd_create(
            c"zeron-test".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        );
        assert!(fd >= 0, "memfd_create: {}", io::Error::last_os_error());
        let fd = OwnedFd::from_raw_fd(fd);
        std::fs::File::from(fd.try_clone().unwrap())
            .write_all(bytes)
            .unwrap();
        if seal {
            let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
            assert_eq!(libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals), 0);
        }
        fd
    }
}

fn inode(fd: &OwnedFd) -> u64 {
    // SAFETY: fstat into a zeroed struct.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        assert_eq!(libc::fstat(fd.as_raw_fd(), &mut stat), 0);
        stat.st_ino
    }
}

/// Sends `data` with `fds` attached to its first byte, like the helper.
fn send_with_fds(socket: &UnixStream, data: &[u8], fds: &[&OwnedFd]) {
    let mut control = [0u64; 8];
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut _,
        iov_len: data.len(),
    };
    // SAFETY: `message` points at buffers owned by this frame.
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if !fds.is_empty() {
            let bytes = (fds.len() * std::mem::size_of::<RawFd>()) as u32;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = libc::CMSG_SPACE(bytes) as _;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(bytes) as _;
            let slots = libc::CMSG_DATA(header) as *mut RawFd;
            for (i, fd) in fds.iter().enumerate() {
                slots.add(i).write_unaligned(fd.as_raw_fd());
            }
        }
        assert_eq!(
            libc::sendmsg(socket.as_raw_fd(), &message, 0),
            data.len() as isize
        );
    }
}

fn packet(kind: u8, page: u32, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![kind];
    packet.extend_from_slice(&page.to_le_bytes());
    packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    packet.extend_from_slice(payload);
    packet
}

fn outbox() -> (Outbox, mpsc::Receiver<Vec<u8>>) {
    let (tx, rx) = mpsc::channel();
    (Outbox(tx), rx)
}

fn commands(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<Value> {
    rx.try_iter()
        .map(|message| serde_json::from_slice(&message[4..]).unwrap())
        .collect()
}

fn info(id: u32, kind: &str, width: u32, height: u32, stride: u32) -> Value {
    let mut info = json!({
        "buffer": id, "kind": kind, "width": width, "height": height, "stride": stride,
        "offset": 0, "fourcc": FOURCC_ARGB8888,
    });
    if kind == "dmabuf" {
        info["modifier"] = 0.into();
    }
    info
}

fn frame(buffer: u32, damage: &[[i64; 4]]) -> Frame {
    Frame::parse(&json!({"buffer": buffer, "scale": 1.5, "damage": damage})).unwrap()
}

#[test]
fn reader_pairs_descriptors_with_packets_split_across_reads() {
    let (ours, theirs) = UnixStream::pair().unwrap();
    let mut reader = Reader::new(ours);
    let shared = memfd(b"pixels", true);
    let first = packet(b'B', 7, br#"{"buffer":1}"#);
    // The descriptor rides on the packet's first byte; the rest of it and the
    // next packet arrive in a later read.
    send_with_fds(&theirs, &first[..3], &[&shared]);
    let mut rest = first[3..].to_vec();
    rest.extend(packet(b'F', 7, b"{}"));
    send_with_fds(&theirs, &rest, &[]);
    let (kind, page, payload) = reader.next().unwrap();
    assert_eq!(
        (kind, page, &payload[..]),
        (b'B', 7, &br#"{"buffer":1}"#[..])
    );
    assert_eq!(inode(&reader.take_fd().unwrap()), inode(&shared));
    assert_eq!(reader.next().unwrap().0, b'F');
    assert!(reader.take_fd().is_err(), "a packet without a descriptor");
    drop(theirs);
    assert_eq!(
        reader.next().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

#[test]
fn reader_rejects_oversized_packets() {
    let (ours, mut theirs) = UnixStream::pair().unwrap();
    let mut reader = Reader::new(ours);
    let mut header = vec![b'J', 0, 0, 0, 0];
    header.extend_from_slice(&u32::MAX.to_le_bytes());
    theirs.write_all(&header).unwrap();
    assert!(reader.next().is_err());
}

#[test]
fn buffers_are_validated_before_use() {
    let fd = || memfd(&[], true);
    let (id, buffer) = Buffer::parse(&info(3, "dmabuf", 64, 32, 256), fd()).unwrap();
    assert_eq!(id, 3);
    assert_eq!(buffer.kind, Kind::Dmabuf);
    assert_eq!((buffer.width, buffer.height, buffer.stride), (64, 32, 256));
    assert!(
        Buffer::parse(&info(3, "shm", 64, 32, 255), fd()).is_none(),
        "stride below a row"
    );
    assert!(Buffer::parse(&info(3, "shm", 0, 32, 256), fd()).is_none());
    assert!(Buffer::parse(&info(3, "shm", 9000, 32, 36000), fd()).is_none());
    assert!(Buffer::parse(&info(3, "texture", 64, 32, 256), fd()).is_none());
    let mut other = info(3, "shm", 64, 32, 256);
    other["fourcc"] = u32::from_le_bytes(*b"XR24").into();
    assert!(Buffer::parse(&other, fd()).is_none(), "only ARGB8888");
    let mut unaligned = info(3, "shm", 64, 32, 256);
    unaligned["offset"] = 2.into();
    assert!(Buffer::parse(&unaligned, fd()).is_none());
    // A dma-buf says how its pixels are laid out, in all 64 bits.
    let mut tiled = info(3, "dmabuf", 64, 32, 256);
    tiled["modifier"] = 0x0300_0000_0060_6015_u64.into();
    assert_eq!(
        Buffer::parse(&tiled, fd()).unwrap().1.modifier,
        0x0300_0000_0060_6015
    );
    tiled["modifier"] = (-1_i64).into();
    assert_eq!(Buffer::parse(&tiled, fd()).unwrap().1.modifier, u64::MAX);
    tiled.as_object_mut().unwrap().remove("modifier");
    assert!(Buffer::parse(&tiled, fd()).is_none(), "no layout");
    assert_eq!(
        Buffer::parse(&info(3, "shm", 64, 32, 256), fd())
            .unwrap()
            .1
            .modifier,
        0
    );
}

#[test]
fn mirrors_map_only_when_sealed_against_shrinking() {
    let pixels: Vec<u8> = (0..256 * 2).map(|i| i as u8).collect();
    let parse = |fd| Buffer::parse(&info(1, "shm", 64, 2, 256), fd).unwrap().1;
    assert!(
        parse(memfd(&pixels, false)).pixels().is_none(),
        "an unsealed mirror could shrink under the mapping"
    );
    assert_eq!(parse(memfd(&pixels, true)).pixels().unwrap(), &pixels[..]);
    assert!(
        parse(memfd(&pixels[..300], true)).pixels().is_none(),
        "shorter than its rows"
    );
}

#[test]
fn frames_reject_bad_scales_and_clamp_damage() {
    assert!(Frame::parse(&json!({"buffer": 1, "scale": 0.1, "damage": []})).is_none());
    assert!(Frame::parse(&json!({"buffer": -1, "scale": 1, "damage": []})).is_none());
    assert!(Frame::parse(&json!({"buffer": 1, "scale": 1})).is_none());
    let frame =
        Frame::parse(&json!({"buffer": 1, "scale": 2, "damage": [[-5, 3, 99999, 2], [1, 2]]}))
            .unwrap();
    assert_eq!(
        frame.damage,
        [Bounds::new(
            point(DevicePixels(0), DevicePixels(3)),
            size(DevicePixels(8192), DevicePixels(2))
        )],
        "clamped, and the incomplete rect dropped"
    );
}

#[test]
fn unpainted_frames_merge_damage_and_return_their_buffers() {
    let (outbox, rx) = outbox();
    let mut surface = Surface::default();
    for id in [1, 2] {
        let buffer = Buffer::parse(&info(id, "dmabuf", 64, 64, 256), memfd(&[], true));
        surface.add_buffer(id, buffer.unwrap().1);
    }
    surface.add_buffer(
        3,
        Buffer::parse(&info(3, "shm", 64, 64, 256), memfd(&[], true))
            .unwrap()
            .1,
    );
    surface.push_frame(frame(1, &[[0, 0, 10, 10]]), &outbox, 9);
    assert!(commands(&rx).is_empty());
    surface.push_frame(frame(2, &[[20, 20, 5, 5]]), &outbox, 9);
    assert_eq!(
        commands(&rx),
        [json!({"cmd": "released", "buffer": 1, "id": 9})]
    );
    let pending = surface.frame.as_ref().unwrap();
    assert_eq!((pending.buffer, pending.damage.len()), (2, 2));
    for i in 0..20 {
        surface.push_frame(frame(2, &[[30 + i, 30, 1, 1]]), &outbox, 9);
    }
    assert!(
        commands(&rx).is_empty(),
        "the pending buffer is not released"
    );
    let damage = &surface.frame.as_ref().unwrap().damage;
    assert!(damage.len() <= 16, "{} rects", damage.len());
    for (x, y) in [(0, 0), (9, 9), (24, 24), (49, 30)] {
        let pixel = point(DevicePixels(x), DevicePixels(y));
        assert!(
            damage.iter().any(|rect| rect.contains(&pixel)),
            "lost damage at {x},{y}"
        );
    }
    // Shared-memory mirrors go back to WebKit as soon as they are copied.
    surface.push_frame(frame(3, &[]), &outbox, 9);
    assert_eq!(
        commands(&rx),
        [json!({"cmd": "released", "buffer": 2, "id": 9})]
    );
    surface.push_frame(frame(1, &[]), &outbox, 9);
    assert!(commands(&rx).is_empty());
    // A buffer Zeron rejected still goes back.
    surface.push_frame(frame(7, &[]), &outbox, 9);
    surface.push_frame(frame(1, &[]), &outbox, 9);
    assert_eq!(
        commands(&rx),
        [
            json!({"cmd": "released", "buffer": 1, "id": 9}),
            json!({"cmd": "released", "buffer": 7, "id": 9})
        ]
    );
    // A buffer the helper forgot is gone before GPUI paints it.
    surface.forget_buffer(1);
    assert!(!surface.buffers.contains_key(&1));
    assert_eq!(surface.forgotten, [1]);
}

#[test]
fn vblank_intervals_tell_webkit_the_refresh_rate() {
    let (outbox, rx) = outbox();
    let mut presenter = Presenter::new(outbox, 4);
    let start = Instant::now();
    let mut now = start;
    let mut tick = |presenter: &mut Presenter, hz: f64, count: usize| {
        for _ in 0..count {
            now += Duration::from_secs_f64(1. / hz);
            presenter.vblank_at(now);
        }
    };
    tick(&mut presenter, 240., 10);
    let sent = commands(&rx);
    assert_eq!(sent.iter().filter(|c| c["cmd"] == "vblank").count(), 10);
    let refresh = |sent: &[Value]| {
        sent.iter()
            .filter(|c| c["cmd"] == "refresh")
            .map(|c| c["mhz"].as_u64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(refresh(&sent), [240_000]);
    // A stall is not a refresh rate.
    tick(&mut presenter, 2., 1);
    assert!(refresh(&commands(&rx)).is_empty());
    tick(&mut presenter, 60., 150);
    let last = *refresh(&commands(&rx)).last().unwrap();
    // Within the 5% it tolerates before telling WebKit again.
    assert!(
        last.abs_diff(60_000) * 10 <= 60_000,
        "settled at {last} mHz"
    );
    presenter.stop_ticking();
    assert!(!presenter.ticking);
}

#[test]
fn scrolls_undo_gpui_scaling_and_keep_precision() {
    let origin = point(px(100.), px(50.));
    let event = |delta, touch_phase| gpui::ScrollWheelEvent {
        position: point(px(110.), px(60.)),
        delta,
        modifiers: gpui::Modifiers {
            shift: true,
            ..Default::default()
        },
        touch_phase,
    };
    let touchpad = gpui::ScrollDelta::Pixels(point(px(0.), px(-30.)));
    assert_eq!(
        scroll_command(origin, &event(touchpad, gpui::TouchPhase::Moved)),
        json!({"cmd":"scroll","x":10.0,"y":10.0,"dx":0.0,"dy":-10.0,"precise":true,"stop":false,"mods":1})
    );
    let wheel = gpui::ScrollDelta::Lines(point(0., -3.));
    let command = scroll_command(origin, &event(wheel, gpui::TouchPhase::Moved));
    assert_eq!(
        (command["dy"].as_f64(), command["precise"].as_bool()),
        (Some(-1.), Some(false))
    );
    let ended = scroll_command(origin, &event(touchpad, gpui::TouchPhase::Ended));
    assert_eq!(ended["stop"], true);
}

#[test]
fn css_cursors_map_to_gpui_cursors() {
    use gpui::CursorStyle::*;
    for (name, cursor) in [
        ("pointer", PointingHand),
        ("text", IBeam),
        ("grabbing", ClosedHand),
        ("ew-resize", ResizeLeftRight),
        ("nwse-resize", ResizeUpLeftDownRight),
        ("nesw-resize", ResizeUpRightDownLeft),
        ("not-allowed", OperationNotAllowed),
        ("none", Arrow),
        ("wait", Arrow),
    ] {
        assert_eq!(cursor_style(name), cursor, "{name}");
    }
}

/// Serves `pages` on loopback; WebKit only navigates to http(s).
fn serve(pages: &'static [(&'static str, &'static str)]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = [0; 4096];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let body = pages
                .iter()
                .find(|(page, _)| *page == path)
                .map_or("", |(_, body)| body);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    origin
}

const PAGES: &[(&str, &str)] = &[
    (
        "/solid",
        "<!doctype html><title>solid</title><style>body{margin:0;background:#f00}\
         #box{position:absolute;left:10px;top:10px;width:20px;height:20px;background:#0f0}</style>\
         <div id=box></div>",
    ),
    (
        "/animated",
        "<!doctype html><title>animated</title><body style='margin:0'><div id=n></div><script>\
         let n=0,f=()=>{document.getElementById('n').textContent=++n;requestAnimationFrame(f)};\
         requestAnimationFrame(f);window.count=()=>n;</script>",
    ),
    (
        // WebKit's compositor runs this animation itself: it renders whenever
        // its last frame is acknowledged, with no vblank.
        "/spinning",
        "<!doctype html><title>spinning</title><body style='margin:0'>\
         <style>@keyframes s{to{transform:rotate(360deg)}}</style>\
         <div style='width:80px;height:80px;background:#4a8;animation:s 1s linear infinite'></div>",
    ),
    (
        "/input",
        "<!doctype html><title>input</title><body style='margin:0;height:4000px'>\
         <input id=i style='position:absolute;left:0;top:0;width:200px;height:30px'>\
         <button id=b style='position:absolute;left:0;top:50px;width:100px;height:30px'\
          onclick='window.clicks=(window.clicks||0)+1'>b</button>",
    ),
];

/// A page in the real helper, run the way GPUI runs it: vblanks while WebKit
/// asks for them, and each frame shown, acknowledged as [`FrameDone`] decides,
/// and released once the next replaces it.
struct Driver {
    _helper: tempfile::TempDir,
    origin: String,
    worker: Worker,
    pages: Vec<(u32, Arc<Route>)>,
    inboxes: Vec<tokio::sync::mpsc::Receiver<NativeEvent>>,
    /// Whether vblanks reach WebKit at all.
    ticking: bool,
    /// Paint frames as they arrive, as GPUI's test and bench builds do,
    /// rather than at the next vblank.
    eager: bool,
    shown: HashMap<u32, (u32, Arc<Buffer>)>,
    done: HashMap<u32, FrameDone>,
    frames: HashMap<u32, usize>,
    evaluations: u32,
}

impl Driver {
    fn start(gpu: Option<gpui::DmabufTarget>) -> Self {
        let helper = tempfile::tempdir().unwrap();
        let executable = extract_helper(helper.path(), "wpe", HELPER).unwrap();
        let worker = match spawn_wpe(&executable, None, gpu) {
            Ok(worker) => worker,
            Err(error) => panic!("WPE helper did not start: {error:?}"),
        };
        assert_eq!(worker.engine, Engine::Wpe);
        let mut driver = Self {
            _helper: helper,
            origin: serve(PAGES),
            worker,
            pages: Vec::new(),
            inboxes: Vec::new(),
            ticking: true,
            eager: false,
            shown: HashMap::new(),
            done: HashMap::new(),
            frames: HashMap::new(),
            evaluations: 0,
        };
        driver.open();
        driver
    }

    /// Opens a page at 320x200 device pixels, scale 1.5. Returns its id.
    fn open(&mut self) -> u32 {
        let id = self.worker.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let route = Arc::new(Route {
            tx,
            state: Mutex::default(),
            frame: Mutex::default(),
            surface: Mutex::default(),
            input: Mutex::new(json!({})),
            #[cfg(feature = "browser-fixture")]
            evaluation: Mutex::default(),
        });
        self.worker
            .routes
            .lock()
            .unwrap()
            .insert(id, Arc::downgrade(&route));
        self.pages.push((id, route));
        self.inboxes.push(rx);
        for command in [
            json!({"cmd": "create"}),
            json!({"cmd": "resize", "width": 320, "height": 200, "scale": 1.5}),
            json!({"cmd": "visible", "value": 1}),
            json!({"cmd": "focus", "value": 1}),
            json!({"cmd": "active", "value": 1}),
        ] {
            self.send(id, command);
        }
        id
    }

    fn close(&mut self, page: u32) {
        self.send(page, json!({"cmd": "close"}));
        self.pages.retain(|(id, _)| *id != page);
    }

    fn first(&self) -> u32 {
        self.pages[0].0
    }

    fn route(&self, page: u32) -> &Route {
        &self.pages.iter().find(|(id, _)| *id == page).unwrap().1
    }

    fn send(&self, page: u32, command: Value) {
        self.worker.send(page, command).unwrap();
    }

    fn events(&mut self) -> Vec<NativeEvent> {
        self.inboxes
            .iter_mut()
            .flat_map(|rx| std::iter::from_fn(|| rx.try_recv().ok()))
            .collect()
    }

    /// One vblank, as GPUI's frame callbacks then its draw: acknowledges a
    /// deferred frame and ticks pages that want vblanks, then paints.
    fn tick(&mut self) {
        for (page, route) in &self.pages {
            if self.done.entry(*page).or_default().refresh() {
                let _ = self.worker.send(*page, json!({"cmd": "presented"}));
            }
            if self.ticking && route.surface.lock().unwrap().vsync {
                let _ = self.worker.send(*page, json!({"cmd": "vblank"}));
            }
        }
        self.paint();
    }

    /// Shows each page's latest frame.
    fn paint(&mut self) {
        for (page, route) in &self.pages {
            let next = {
                let mut surface = route.surface.lock().unwrap();
                surface.frame.take().map(|frame| {
                    let buffer = surface.buffers.get(&frame.buffer).cloned();
                    (frame, buffer)
                })
            };
            if let Some((frame, buffer)) = next {
                if buffer.is_none() {
                    let _ = self
                        .worker
                        .send(*page, json!({"cmd": "released", "buffer": frame.buffer}));
                }
                if let Some(buffer) = buffer {
                    *self.frames.entry(*page).or_default() += 1;
                    let previous = self.shown.insert(*page, (frame.buffer, buffer));
                    if let Some((previous, buffer)) = previous
                        && previous != frame.buffer
                        && buffer.kind == Kind::Dmabuf
                    {
                        let _ = self
                            .worker
                            .send(*page, json!({"cmd": "released", "buffer": previous}));
                    }
                }
                if self.done.entry(*page).or_default().painted() {
                    let _ = self.worker.send(*page, json!({"cmd": "presented"}));
                }
            }
        }
    }

    fn run(&mut self, hz: f64, duration: Duration) {
        let interval = Duration::from_secs_f64(1. / hz);
        let end = Instant::now() + duration;
        let mut next = Instant::now();
        while Instant::now() < end {
            self.tick();
            next += interval;
            while self.eager && Instant::now() < next {
                std::thread::sleep(Duration::from_micros(250));
                self.paint();
            }
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.run(240., Duration::from_millis(10));
        }
    }

    fn load(&mut self, page: u32, path: &str) {
        let url = format!("{}{path}", self.origin);
        self.send(page, json!({"cmd": "load", "url": url}));
        let title = path.trim_start_matches('/').to_owned();
        self.until(&format!("{path} to load"), |driver| {
            let state = driver.route(page).state.lock().unwrap().clone();
            state.title == title && !state.loading
        });
        // Let the first frame of the loaded page arrive.
        self.run(240., Duration::from_millis(300));
    }

    /// Evaluates `expression` in the page; results come back as its title.
    fn eval(&mut self, page: u32, expression: &str) -> Value {
        self.evaluations += 1;
        let marker = format!("eval:{}:", self.evaluations);
        let script = format!("document.title = '{marker}' + JSON.stringify({expression}); true");
        self.send(page, json!({"cmd": "eval", "script": script}));
        let mut result = None;
        self.until(expression, |driver| {
            let title = driver.route(page).state.lock().unwrap().title.clone();
            result = title
                .strip_prefix(&marker)
                .map(|json| serde_json::from_str(json).unwrap());
            result.is_some()
        });
        result.unwrap()
    }

    /// BGRA bytes at device pixel `x`, `y` of the frame the page shows.
    fn pixel(&self, page: u32, x: u32, y: u32) -> [u8; 4] {
        let (_, buffer) = &self.shown[&page];
        let at = buffer.offset as usize + y as usize * buffer.stride as usize + x as usize * 4;
        let read = |bytes: &[u8]| bytes[at..at + 4].try_into().unwrap();
        match buffer.kind {
            Kind::Shm => read(buffer.pixels().unwrap()),
            Kind::Dmabuf => {
                const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x4008_6200;
                let fd = buffer.fd.as_raw_fd();
                // SAFETY: maps the held, linear dma-buf read-only for one read.
                unsafe {
                    let map = libc::mmap(
                        std::ptr::null_mut(),
                        buffer.len(),
                        libc::PROT_READ,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    );
                    assert_ne!(map, libc::MAP_FAILED, "{}", io::Error::last_os_error());
                    let start: u64 = 1; // DMA_BUF_SYNC_READ | DMA_BUF_SYNC_START
                    libc::ioctl(fd, DMA_BUF_IOCTL_SYNC, &start);
                    let pixel = read(std::slice::from_raw_parts(map.cast(), buffer.len()));
                    let end: u64 = 1 | 4; // DMA_BUF_SYNC_READ | DMA_BUF_SYNC_END
                    libc::ioctl(fd, DMA_BUF_IOCTL_SYNC, &end);
                    libc::munmap(map, buffer.len());
                    pixel
                }
            }
        }
    }

    fn kind(&self, page: u32) -> &Kind {
        &self.shown[&page].1.kind
    }

    /// Has the helper read the shown dma-buf back, as when GPUI cannot import
    /// it, and shows that frame instead.
    fn read_back(&mut self, page: u32) {
        let (id, _) = self.shown[&page];
        self.send(page, json!({"cmd": "readback", "buffer": id}));
        self.until("the frame read back", |d| *d.kind(page) == Kind::Shm);
    }

    fn key(&self, page: u32, key: &str, mods: u32) {
        for cmd in ["key_down", "key_up"] {
            self.send(
                page,
                json!({"cmd": cmd, "key": key, "text": key, "mods": mods}),
            );
        }
    }

    fn click(&self, page: u32, x: f32, y: f32) {
        self.send(page, json!({"cmd": "move", "x": x, "y": y, "mods": 0}));
        for cmd in ["down", "up"] {
            self.send(
                page,
                json!({"cmd": cmd, "x": x, "y": y, "button": 1, "count": 1, "mods": 0}),
            );
        }
    }
}

/// Render nodes to try for the GPU path, the one in `ZERON_TEST_RENDER_NODE`
/// first.
fn render_nodes() -> Vec<PathBuf> {
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    if let Some(node) = std::env::var_os("ZERON_TEST_RENDER_NODE") {
        nodes.insert(0, node.into());
    }
    nodes
}

fn target(node: &std::path::Path, modifiers: &[u64]) -> gpui::DmabufTarget {
    gpui::DmabufTarget {
        render_node: node.to_owned(),
        modifiers: modifiers.to_vec(),
    }
}

/// The layouts WebKit renders into on `node`, as the helper reports them when
/// Zeron imports none: `None` when it cannot render there at all.
fn renderable(node: &std::path::Path) -> Option<Vec<u64>> {
    let helper = tempfile::tempdir().unwrap();
    let executable = extract_helper(helper.path(), "wpe", HELPER).unwrap();
    let worker = spawn_wpe(&executable, None, Some(target(node, &[]))).ok()?;
    let ready = &worker.ready;
    let _ = worker.child.lock().unwrap().kill();
    (ready["frames"] == "readback").then(|| {
        let modifiers = ready["modifiers"].as_array().unwrap();
        modifiers.iter().map(|m| m.as_u64().unwrap()).collect()
    })
}

fn assert_solid_page(driver: &Driver, page: u32) {
    let (_, buffer) = &driver.shown[&page];
    // 320x200 device pixels at 1.5 lay out at 214x134, rounded up so the
    // page covers GPUI's box: WebKit renders 321x201.
    assert_eq!((buffer.width, buffer.height), (321, 201), "device pixels");
    assert_eq!(
        driver.pixel(page, 2, 2),
        [0, 0, 255, 255],
        "red background, BGRA"
    );
    // The box spans logical 10..30, device 15..45 at scale 1.5.
    assert_eq!(driver.pixel(page, 30, 30), [0, 255, 0, 255], "green box");
    assert_eq!(driver.pixel(page, 46, 46), [0, 0, 255, 255], "box edge");
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_renders_pages_into_shared_memory() {
    let mut driver = Driver::start(None);
    let page = driver.first();
    driver.load(page, "/solid");
    assert_eq!(*driver.kind(page), Kind::Shm);
    assert_solid_page(&driver, page);
}

/// Each GPU WebKit renders on shares pages in every layout it renders into,
/// and reads them back, through GL, when Zeron imports none of them: GBM
/// cannot map NVIDIA's tiled layouts, and NVIDIA cannot render linear ones.
#[test]
#[ignore = "requires the WPE WebKit runtime and a GPU"]
fn wpe_renders_pages_on_each_gpu_in_each_layout() {
    let mut rendered = Vec::new();
    for node in render_nodes() {
        let Some(modifiers) = renderable(&node) else {
            eprintln!("{}: shared memory", node.display());
            continue;
        };
        let mut driver = Driver::start(Some(target(&node, &[])));
        let page = driver.first();
        driver.load(page, "/solid");
        assert_eq!(*driver.kind(page), Kind::Shm, "read back");
        assert_solid_page(&driver, page);
        for modifier in modifiers {
            let mut driver = Driver::start(Some(target(&node, &[modifier])));
            let page = driver.first();
            driver.load(page, "/solid");
            let (_, buffer) = &driver.shown[&page];
            assert_eq!((&buffer.kind, buffer.modifier), (&Kind::Dmabuf, modifier));
            // Only linear dma-bufs map as plain rows; the helper reads the
            // others back as it would for a GPU that cannot import them.
            if modifier != super::DRM_FORMAT_MOD_LINEAR {
                driver.read_back(page);
            }
            assert_solid_page(&driver, page);
            rendered.push(format!("{} {modifier:#018x}", node.display()));
        }
    }
    assert!(
        !rendered.is_empty(),
        "no render node produced dma-buf frames"
    );
    eprintln!("dma-buf frames:\n{}", rendered.join("\n"));
}

/// The GPU path, with every layout shared: on `ZERON_TEST_RENDER_NODE`, or
/// else on the first GPU WebKit renders on.
fn test_gpu() -> Option<gpui::DmabufTarget> {
    let nodes = match std::env::var_os("ZERON_TEST_RENDER_NODE") {
        Some(node) => vec![PathBuf::from(node)],
        None => render_nodes(),
    };
    nodes
        .into_iter()
        .find_map(|node| Some(target(&node, &renderable(&node)?)))
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_animates_at_the_vblank_rate_it_is_given() {
    let mut driver = Driver::start(test_gpu());
    let page = driver.first();
    driver.load(page, "/animated");
    for hz in [240., 144., 60.] {
        driver.send(page, json!({"cmd": "refresh", "mhz": (hz * 1000.) as u32}));
        driver.run(hz, Duration::from_millis(300));
        let before = driver.eval(page, "count()").as_f64().unwrap();
        let frames = driver.frames[&page];
        let start = Instant::now();
        driver.run(hz, Duration::from_secs(1));
        let elapsed = start.elapsed().as_secs_f64();
        let after = driver.eval(page, "count()").as_f64().unwrap();
        let raf = (after - before) / elapsed;
        let fps = (driver.frames[&page] - frames) as f64 / elapsed;
        eprintln!(
            "{hz} Hz vblank: {raf:.1} rAF/s, {fps:.1} frames/s ({:?})",
            driver.kind(page)
        );
        assert!((raf - hz).abs() < hz * 0.1, "{raf:.1} rAF/s at {hz} Hz");
        assert!((fps - hz).abs() < hz * 0.15, "{fps:.1} frames/s at {hz} Hz");
    }
    // Without vblanks WebKit stops: GPUI is its only clock.
    driver.ticking = false;
    let before = driver.eval(page, "count()").as_f64().unwrap();
    driver.run(240., Duration::from_millis(300));
    let after = driver.eval(page, "count()").as_f64().unwrap();
    assert!(
        after - before <= 1.,
        "{} rAF callbacks without vblanks",
        after - before
    );
}

/// A stand-in for the helper: reports ready, then runs the shell `then`.
fn stub_helper(dir: &std::path::Path, then: &str) -> PathBuf {
    let helper = dir.join("helper");
    let ready = "printf 'R\\000\\000\\000\\000\\002\\000\\000\\000{}'";
    std::fs::write(&helper, format!("#!/bin/sh\n{ready}\n{then}\n")).unwrap();
    std::fs::set_permissions(&helper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    helper
}

#[test]
fn a_helper_that_dies_at_startup_fails_at_once() {
    // WebKitGTK takes over as soon as the WPE helper exits, as it does when
    // its runtime is missing, not when the startup timeout expires.
    let start = Instant::now();
    let result = spawn_wpe(std::path::Path::new("false"), None, None);
    assert!(matches!(result, Err(Startup::Failed(_))));
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "failed after {:?}",
        start.elapsed()
    );
}

#[test]
fn a_helper_zeron_cannot_read_is_stopped_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    // A packet past the protocol's limit, then writes until the socket fills
    // up and blocks the helper.
    let helper = stub_helper(
        dir.path(),
        "printf 'F\\001\\000\\000\\000\\000\\000\\000\\020'\nexec yes",
    );
    let data = BrowserData {
        engine_override: Some(Engine::Wpe),
        helper_override: Some(helper),
        ..Default::default()
    };
    let first = data.worker(None).unwrap();
    // Zeron hangs up, so the helper exits instead of blocking on its socket.
    let deadline = Instant::now() + Duration::from_secs(5);
    while first.child.lock().unwrap().try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the helper outlived its connection"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(first.stopped.load(Ordering::Relaxed));
    let second = data.worker(None).unwrap();
    assert!(
        !Arc::ptr_eq(&first, &second),
        "the next page reused the stopped helper"
    );
}

#[test]
fn a_helper_busy_being_written_starts_once_released() {
    // A child that another thread forked mid-extraction holds the helper open
    // for writing until that child execs.
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helper");
    std::fs::copy("/bin/true", &helper).unwrap();
    let writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&helper)
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        drop(writer);
    });
    let mut child = spawn_helper(&mut std::process::Command::new(&helper)).unwrap();
    release.join().unwrap();
    assert!(child.wait().unwrap().success());
}

/// A WPE page whose WebKit wants vblanks, on a stub helper that takes
/// commands and never answers.
fn stub_page() -> (
    tempfile::TempDir,
    NativePage,
    tokio::sync::mpsc::Receiver<NativeEvent>,
) {
    let dir = tempfile::tempdir().unwrap();
    let data = BrowserData {
        engine_override: Some(Engine::Wpe),
        helper_override: Some(stub_helper(dir.path(), "exec cat >/dev/null")),
        ..Default::default()
    };
    let (tx, events) = tokio::sync::mpsc::channel(8);
    let page = NativePage::new(&data, tx, None).unwrap();
    page.route.surface.lock().unwrap().vsync = true;
    (dir, page, events)
}

#[test]
fn a_page_showing_its_error_gets_no_vblanks() {
    // WebKit would keep rendering frames no one sees, or failing to deliver
    // them every vblank.
    let (_dir, mut page, _events) = stub_page();
    let fail = |page: &NativePage, error: Option<&str>| {
        page.route.state.lock().unwrap().error = error.map(Into::into);
    };
    fail(&page, Some("This page's frames could not be shown."));
    assert!(!page.wants_vblank(), "a page showing its error got vblanks");
    fail(&page, None);
    assert!(page.wants_vblank(), "a reloading page got no vblanks");
    fail(
        &page,
        Some("The browser process stopped. Reload to try again."),
    );
    assert!(!page.vblank(), "a page showing its error kept its vblanks");
}

#[test]
fn a_page_shown_again_restarts_the_vblanks_hiding_stopped() {
    let (_dir, mut page, mut events) = stub_page();
    assert!(page.wants_vblank());
    page.present(Presentation::Hidden);
    assert!(!page.vblank(), "a hidden page kept its vblanks");
    while events.try_recv().is_ok() {}
    // WebKit still wants vblanks, so it does not ask again: showing the page
    // wakes its surface, which restarts them.
    page.present(Presentation::Live);
    assert!(
        events.try_recv().is_ok(),
        "showing the page did not wake its surface"
    );
    assert!(page.wants_vblank());
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_pages_never_outgrow_the_frames_zeron_takes() {
    let mut driver = Driver::start(test_gpu());
    let page = driver.first();
    driver.load(page, "/animated");
    // 8192 device pixels at 1.5 lay out at 5462 logical pixels rounded up,
    // which WebKit would render 8193 pixels wide.
    driver.send(
        page,
        json!({"cmd": "resize", "width": 8192, "height": 64, "scale": 1.5}),
    );
    driver.run(60., Duration::from_millis(500));
    let frames = driver.frames[&page];
    driver.run(60., Duration::from_millis(500));
    assert!(
        driver.frames[&page] > frames + 10,
        "the page stopped rendering"
    );
    let width = driver.shown[&page].1.width;
    assert!((8190..=8192).contains(&width), "{width} pixels wide");
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_renders_one_frame_per_vblank_however_often_gpui_paints() {
    let mut driver = Driver::start(test_gpu());
    let page = driver.first();
    driver.load(page, "/spinning");
    for (hz, eager) in [(60., false), (60., true), (240., false), (240., true)] {
        driver.eager = eager;
        driver.run(hz, Duration::from_millis(300));
        let frames = driver.frames[&page];
        let start = Instant::now();
        driver.run(hz, Duration::from_secs(1));
        let fps = (driver.frames[&page] - frames) as f64 / start.elapsed().as_secs_f64();
        let buffers = driver.route(page).surface.lock().unwrap().buffers.len();
        let painting = if eager {
            "as frames arrive"
        } else {
            "at vblanks"
        };
        eprintln!(
            "{hz} Hz, painting {painting}: {fps:.1} frames/s, {buffers} buffers ({:?})",
            driver.kind(page)
        );
        assert!(
            (fps - hz).abs() < hz * 0.15,
            "{fps:.1} frames/s at {hz} Hz, painting {painting}"
        );
        // WebKit's swapchain holds four; more, and it renders faster than
        // the display.
        assert!(
            buffers <= 5,
            "{buffers} buffers at {hz} Hz, painting {painting}"
        );
    }
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_takes_keyboard_pointer_scroll_and_clipboard_input() {
    let mut driver = Driver::start(None);
    let page = driver.first();
    driver.load(page, "/input");
    driver.click(page, 20., 15.);
    driver.run(240., Duration::from_millis(100));
    for key in ["h", "i"] {
        driver.key(page, key, 0);
    }
    driver.key(page, "space", 0);
    driver.key(page, "BackSpace", 0);
    driver.until("typing", |d| {
        d.eval(page, "document.getElementById('i').value") == "hi"
    });
    // Ctrl+V pastes what Zeron shared; Ctrl+A, Ctrl+C copy back to Zeron.
    driver.send(page, json!({"cmd": "clipboard", "text": " — café 日本語"}));
    driver.key(page, "v", 4);
    driver.until("paste", |d| {
        d.eval(page, "document.getElementById('i').value") == "hi — café 日本語"
    });
    // A clipboard without text pastes nothing, not the earlier copy. WebKit
    // reads the clipboard after the key, so give it time before checking.
    driver.send(page, json!({"cmd": "clipboard", "text": ""}));
    driver.key(page, "v", 4);
    driver.run(240., Duration::from_millis(500));
    assert_eq!(
        driver.eval(page, "document.getElementById('i').value"),
        "hi — café 日本語"
    );
    driver.events();
    driver.key(page, "a", 4);
    driver.key(page, "c", 4);
    let mut copied = None;
    driver.until("copy", |d| {
        copied = copied.take().or_else(|| {
            d.events().into_iter().find_map(|event| match event {
                NativeEvent::Clipboard(text) => Some(text),
                _ => None,
            })
        });
        copied.is_some()
    });
    assert_eq!(copied.as_deref(), Some("hi — café 日本語"));
    driver.click(page, 50., 65.);
    driver.until("click", |d| d.eval(page, "window.clicks || 0") == 1);
    let scroll = gpui::ScrollWheelEvent {
        position: point(px(100.), px(100.)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-250.))),
        ..Default::default()
    };
    driver.send(page, scroll_command(point(px(0.), px(0.)), &scroll));
    driver.until("scroll", |d| {
        d.eval(page, "scrollY").as_f64().unwrap() > 50.
    });
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_pages_render_independently_and_the_helper_exits_cleanly() {
    let mut driver = Driver::start(None);
    let first = driver.first();
    let second = driver.open();
    let third = driver.open();
    for page in [first, second, third] {
        driver.load(page, "/solid");
        assert_solid_page(&driver, page);
    }
    driver.close(second);
    driver.load(third, "/animated");
    driver.run(120., Duration::from_millis(500));
    assert!(driver.eval(third, "count()").as_f64().unwrap() > 30.);
    assert!(driver.eval(first, "1 + 1") == 2);
    // EOF on its input drains the helper; it exits on its own.
    let Link::Socket(outbox) = &driver.worker.link else {
        unreachable!()
    };
    outbox.close();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = driver.worker.child.lock().unwrap().try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "helper did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_renders_a_page_opened_after_the_last_one_closed() {
    // WebKit keeps the closed page's display link, and the vblank observer
    // it adopted from that page's screen, for the life of the process.
    let mut driver = Driver::start(None);
    let first = driver.first();
    driver.load(first, "/animated");
    driver.close(first);
    driver.run(240., Duration::from_millis(200));
    let second = driver.open();
    driver.load(second, "/animated");
    driver.run(240., Duration::from_millis(500));
    assert!(driver.eval(second, "count()").as_f64().unwrap() > 60.);
    assert_solid_frames_follow(&mut driver, second);
}

/// Resizing reallocates WebKit's buffers; the ones it drops are forgotten
/// rather than kept alive (and pinned on the GPU) by the helper or Zeron.
#[test]
#[ignore = "requires the WPE WebKit runtime and a GPU"]
fn wpe_resizes_release_the_buffers_they_replace() {
    let Some(gpu) = test_gpu() else {
        eprintln!("skipped: WebKit renders on no GPU here");
        return;
    };
    let mut driver = Driver::start(Some(gpu));
    let page = driver.first();
    driver.load(page, "/animated");
    for step in 0..30 {
        let width = 320 + step * 24;
        driver.send(
            page,
            json!({"cmd": "resize", "width": width, "height": 200 + step * 8, "scale": 1.5}),
        );
        driver.run(240., Duration::from_millis(20));
    }
    driver.run(240., Duration::from_millis(500));
    let live = driver.route(page).surface.lock().unwrap().buffers.len();
    // WebKit's swapchain holds at most four, plus a mirror.
    assert!(live <= 5, "{live} buffers still shared after the resizes");
}

/// The page keeps rendering after navigating somewhere static.
fn assert_solid_frames_follow(driver: &mut Driver, page: u32) {
    driver.load(page, "/solid");
    assert_solid_page(driver, page);
}

#[test]
#[ignore = "requires the WPE WebKit runtime"]
fn wpe_helper_crash_reaches_every_page() {
    let mut driver = Driver::start(None);
    let first = driver.first();
    let second = driver.open();
    driver.load(first, "/solid");
    driver.worker.child.lock().unwrap().kill().unwrap();
    driver.until("the crash to be reported", |d| {
        [first, second].iter().all(|page| {
            let state = d.route(*page).state.lock().unwrap();
            state
                .error
                .as_deref()
                .is_some_and(|error| error.contains("WPE WebKit"))
        })
    });
}
