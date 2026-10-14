//! An inline page on macOS (an agent visual in the transcript): a real
//! WKWebView that GPUI composites, the way Linux composites its offscreen
//! helper's frames.
//!
//! A native child view always draws above GPUI's base plane, so showing the
//! page directly would put it over the glass composer, the scroll fades, and
//! anything else the transcript scrolls under. Instead the WKWebView stays in
//! the window, at the visual's exact position, but fully transparent: WebKit
//! still treats it as on screen, so it lays out, animates, and answers
//! snapshots, and GPUI paints those snapshots as an ordinary image (clipped,
//! faded, and blurred like the text around it).
//!
//! Input goes to the page natively. While the pointer is over the visual, its
//! invisible view takes hits for the visible part of the visual, so hover,
//! clicks, drags, selection, cursors, and form fields are WebKit's own. The
//! view sits beneath GPUI's overlay plane, so popovers and menus above the
//! transcript still win their own clicks; the wheel always scrolls the
//! transcript (an inline page is sized to its content); and leaving the
//! visual hands the pointer straight back to GPUI.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use gpui::{Bounds, Pixels, RenderImage, Window};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, class, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSImage, NSView, NSWindowOrderingMode};
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use wry::{WebView, WebViewExtMacOS};

/// One composited frame of the page and its pixels per point.
pub type PageFrame = (Arc<RenderImage>, f32);

pub struct InputState {
    /// The pointer is over the visual: the view takes hits in `region`.
    armed: Cell<bool>,
    /// The visual's visible part, in the parent's coordinates.
    region: Cell<NSRect>,
}

// Takes pointer events only while armed, and only inside the visible part of
// the visual; everywhere else they fall through to GPUI.
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZeronVisualInputView"]
    #[ivars = InputState]
    struct VisualInputView;
    impl VisualInputView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let state = self.ivars();
            if state.armed.get() && contains(state.region.get(), point) {
                unsafe { msg_send![super(self), hitTest: point] }
            } else {
                std::ptr::null_mut()
            }
        }
    }
);

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y < rect.origin.y + rect.size.height
}

pub struct InlinePage {
    host: Rc<Host>,
}

/// Places a page from a presentation callback, which outlives no page.
#[derive(Clone)]
pub struct PagePlacer(Weak<Host>);

impl PagePlacer {
    pub fn place(&self, bounds: Bounds<Pixels>, visible: Bounds<Pixels>) {
        if let Some(host) = self.0.upgrade() {
            host.place(bounds, visible);
        }
    }
}

struct Host {
    web: WebView,
    view: Retained<WKWebView>,
    input: Retained<VisualInputView>,
    parent: Retained<NSView>,
    monitor: RefCell<Option<Retained<AnyObject>>>,
    /// The visual's full bounds and visible part last placed, in GPUI pixels.
    placed: Cell<Option<(Bounds<Pixels>, Bounds<Pixels>)>>,
    /// A snapshot is in flight; the next one waits for it.
    snapshotting: Cell<bool>,
    frames: UnboundedSender<PageFrame>,
}

impl InlinePage {
    /// Load `url` in a new transparent page. Frames arrive on the receiver;
    /// links the page opens in a new window go to `open`.
    pub fn new(
        window: &Window,
        url: &str,
        open: impl Fn(String) + 'static,
    ) -> Result<(Self, UnboundedReceiver<PageFrame>), String> {
        let mtm = MainThreadMarker::new().ok_or("inline pages live on the main thread")?;
        let page_root = page_root(url);
        let web = wry::WebViewBuilder::new()
            .with_url(url)
            .with_transparent(true)
            // Hidden until it is transparent and placed: no first-frame flash.
            .with_visible(false)
            .with_focused(false)
            // The page itself, nothing else: links leave through Zeron.
            .with_navigation_handler(move |next| next.starts_with(&page_root))
            .with_new_window_req_handler(move |next, _| {
                open(next);
                wry::NewWindowResponse::Deny
            })
            .with_download_started_handler(|_, _| false)
            .build_as_child(window)
            .map_err(|e| e.to_string())?;
        // Popovers and menus paint above native children once this is on.
        let _ = window.enable_scene_overlay();
        let view: Retained<WKWebView> = Retained::into_super(web.webview());
        let parent = unsafe { view.superview() }.ok_or("inline page parent is missing")?;
        let input: Retained<VisualInputView> = unsafe {
            let object = mtm.alloc().set_ivars(InputState {
                armed: Cell::new(false),
                region: Cell::new(NSRect::ZERO),
            });
            msg_send![super(object), initWithFrame: parent.bounds()]
        };
        input.setAutoresizesSubviews(false);
        parent.addSubview(&input);
        // Beneath the overlay plane, like browser tabs, so deferred GPUI
        // content keeps its clicks.
        for sibling in parent.subviews() {
            if sibling.class().name() == c"GPUIOverlayView" {
                parent.addSubview_positioned_relativeTo(
                    &input,
                    NSWindowOrderingMode::Below,
                    Some(&sibling),
                );
                break;
            }
        }
        input.addSubview(&view);
        // Fully transparent but not hidden: WebKit keeps rendering a view
        // that is merely see-through, and GPUI shows its snapshots instead.
        view.setAlphaValue(0.0);
        view.setHidden(false);

        let (frames, receiver) = unbounded_channel();
        let host = Rc::new(Host {
            web,
            view,
            input,
            parent,
            monitor: RefCell::new(None),
            placed: Cell::new(None),
            snapshotting: Cell::new(false),
            frames,
        });
        *host.monitor.borrow_mut() = Some(input_monitor(Rc::downgrade(&host))?);
        Ok((Self { host }, receiver))
    }

    pub fn navigate(&self, url: &str) {
        if let Err(error) = self.host.web.load_url(url) {
            tracing::warn!(%error, "inline page reload failed");
        }
    }

    /// Zoom the whole page, so a page laid out at the 16px design base
    /// renders at the user's interface size (the snapshot captures it zoomed).
    pub fn set_page_zoom(&self, zoom: f32) {
        unsafe {
            let _: () = msg_send![&*self.host.view, setPageZoom: f64::from(zoom)];
        }
    }

    pub fn placer(&self) -> PagePlacer {
        PagePlacer(Rc::downgrade(&self.host))
    }

    /// Let the page take the pointer (it is over the visual), or give it back.
    pub fn set_armed(&self, armed: bool) {
        self.host.input.ivars().armed.set(armed);
    }

    pub fn armed(&self) -> bool {
        self.host.input.ivars().armed.get()
    }

    /// Ask for a fresh frame, unless one is already on its way.
    pub fn snapshot(&self) {
        let host = &self.host;
        if host.snapshotting.get() || host.placed.get().is_none() {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        host.snapshotting.set(true);
        let weak = Rc::downgrade(host);
        let done = block2::RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
            let Some(host) = weak.upgrade() else {
                return;
            };
            host.snapshotting.set(false);
            if let Some(image) = unsafe { image.as_ref() } {
                convert_frame(image, host.frames.clone());
            }
        });
        unsafe {
            let config = WKSnapshotConfiguration::new(mtm);
            host.view
                .takeSnapshotWithConfiguration_completionHandler(Some(&config), &done);
        }
    }
}

impl Host {
    /// Place the page over the visual: `bounds` is its full extent and
    /// `visible` the part the transcript shows, both in window pixels.
    fn place(&self, bounds: Bounds<Pixels>, visible: Bounds<Pixels>) {
        if self.placed.get() == Some((bounds, visible)) {
            return;
        }
        self.placed.set(Some((bounds, visible)));
        let actions_disabled: bool = unsafe {
            let previous = msg_send![class!(CATransaction), disableActions];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
            previous
        };
        self.input.setFrame(self.parent.bounds());
        self.input.ivars().region.set(self.rect(visible));
        self.view.setFrame(self.rect(bounds));
        unsafe {
            let _: () = msg_send![class!(CATransaction), setDisableActions: actions_disabled];
        }
        if std::env::var_os("ZERON_VISUAL_DEBUG").is_some() {
            self.debug_hit(visible);
        }
    }

    /// Diagnostics: what AppKit's own hit test finds at the middle of the
    /// visible visual, as if armed.
    fn debug_hit(&self, visible: Bounds<Pixels>) {
        let region = self.input.ivars().region.get();
        let center = NSPoint::new(
            region.origin.x + region.size.width / 2.0,
            region.origin.y + region.size.height / 2.0,
        );
        let armed = self.input.ivars().armed.replace(true);
        let (hit, frame_view) = match self.parent.window() {
            Some(window) => {
                let in_window = self.parent.convertPoint_toView(center, None);
                let content = window.contentView();
                let root = content.as_ref().and_then(|c| unsafe { c.superview() });
                let hit = root.as_ref().and_then(|root| {
                    let point = root.convertPoint_fromView(in_window, None);
                    root.hitTest(point)
                });
                (
                    hit.map(|v| v.class().name().to_string_lossy().into_owned()),
                    content.map(|c| (c.class().name().to_string_lossy().into_owned(), std::ptr::eq(&*c, &*self.parent))),
                )
            }
            None => (None, None),
        };
        self.input.ivars().armed.set(armed);
        tracing::info!(
            ?visible,
            ?region,
            parent_bounds = ?self.parent.bounds(),
            flipped = self.parent.isFlipped(),
            web_frame = ?self.view.frame(),
            ?hit,
            ?frame_view,
            "visual input placement"
        );
    }

    /// GPUI window pixels (top-left origin) to the parent's coordinates.
    fn rect(&self, bounds: Bounds<Pixels>) -> NSRect {
        let x = f64::from(f32::from(bounds.origin.x));
        let y = f64::from(f32::from(bounds.origin.y));
        let width = f64::from(f32::from(bounds.size.width)).max(0.0);
        let height = f64::from(f32::from(bounds.size.height)).max(0.0);
        let y = if self.parent.isFlipped() {
            y
        } else {
            self.parent.bounds().size.height - y - height
        };
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }
}

impl Drop for InlinePage {
    fn drop(&mut self) {
        if let Some(monitor) = self.host.monitor.borrow_mut().take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        self.host.input.ivars().armed.set(false);
        self.host.input.removeFromSuperview();
    }
}

/// The served page's own path: navigation may stay on it, nowhere else.
fn page_root(url: &str) -> String {
    match url.rfind('/') {
        Some(end) => url[..=end].to_owned(),
        None => url.to_owned(),
    }
}

/// Hand the pointer back to GPUI when it leaves the visual, and send the
/// wheel to the transcript, before AppKit routes either event.
fn input_monitor(host: Weak<Host>) -> Result<Retained<AnyObject>, String> {
    let callback = block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| {
        let Some(host) = host.upgrade() else {
            return event.as_ptr();
        };
        let state = host.input.ivars();
        if !state.armed.get() {
            return event.as_ptr();
        }
        let e = unsafe { event.as_ref() };
        let Some(window) = host.view.window() else {
            return event.as_ptr();
        };
        // Where the pointer is now, from the screen: mouse-moved events often
        // carry no window, so their own location can't be trusted here.
        let in_window = window.convertPointFromScreen(NSEvent::mouseLocation());
        let point = host.parent.convertPoint_fromView(in_window, None);
        let inside = contains(state.region.get(), point);
        if e.r#type() == NSEventType::ScrollWheel || !inside {
            if std::env::var_os("ZERON_VISUAL_DEBUG").is_some() {
                tracing::info!(kind = ?e.r#type(), ?point, region = ?state.region.get(), "visual input disarmed");
            }
            state.armed.set(false);
            let same_window = e.windowNumber() == window.windowNumber();
            // A press outside the visual returns the keyboard to GPUI, which
            // the page took when it was clicked into.
            if same_window
                && matches!(
                    e.r#type(),
                    NSEventType::LeftMouseDown
                        | NSEventType::RightMouseDown
                        | NSEventType::OtherMouseDown
                )
                && let Some(responder) = window.firstResponder()
                && responder_is_within(&responder, &host.view)
            {
                window.makeFirstResponder(Some(&host.parent));
            }
        }
        event.as_ptr()
    });
    unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::MouseMoved
                | NSEventMask::ScrollWheel
                | NSEventMask::LeftMouseDown
                | NSEventMask::RightMouseDown
                | NSEventMask::OtherMouseDown,
            &callback,
        )
    }
    .ok_or_else(|| "inline page input monitor unavailable".to_owned())
}

/// Whether `responder` is `view` or one of its subviews (WebKit's content
/// view takes first responder, not the WKWebView itself).
fn responder_is_within(responder: &objc2_app_kit::NSResponder, view: &WKWebView) -> bool {
    let Some(mut current) = responder.downcast_ref::<NSView>().map(|view| view.retain()) else {
        return false;
    };
    let target: &NSView = view;
    loop {
        if std::ptr::eq(&*current, target) {
            return true;
        }
        match unsafe { current.superview() } {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// The snapshot as straight-alpha BGRA, which is what GPUI blends. The copy
/// runs off the main thread.
fn convert_frame(image: &NSImage, frames: UnboundedSender<PageFrame>) {
    use core_graphics::image::CGImage;
    use foreign_types::ForeignType;

    let points = image.size();
    let cg: *mut std::ffi::c_void = unsafe {
        msg_send![
            image,
            CGImageForProposedRect: std::ptr::null_mut::<NSRect>(),
            context: std::ptr::null_mut::<AnyObject>(),
            hints: std::ptr::null_mut::<AnyObject>()
        ]
    };
    if cg.is_null() || points.width <= 0.0 {
        return;
    }
    // The NSImage owns its CGImage; keep our own reference for the copy.
    let cg = unsafe {
        core_foundation::base::CFRetain(cg as *const _);
        CGImage::from_ptr(cg.cast())
    };
    struct SendImage(CGImage);
    // SAFETY: a CGImage is immutable and safe to read from any thread.
    unsafe impl Send for SendImage {}
    let image = SendImage(cg);
    let points_width = points.width as f32;
    std::thread::Builder::new()
        .name("visual-frame".into())
        .spawn(move || {
            let image = image;
            if let Some(frame) = bgra_frame(&image.0, points_width) {
                let _ = frames.send(frame);
            }
        })
        .ok();
}

fn bgra_frame(image: &core_graphics::image::CGImage, points_width: f32) -> Option<PageFrame> {
    use core_graphics::base::{kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst};
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};

    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return None;
    }
    let mut context = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width * 4,
        &CGColorSpace::create_device_rgb(),
        kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little,
    );
    context.draw_image(
        CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width as f64, height as f64),
        ),
        image,
    );
    let stride = context.bytes_per_row();
    let data = context.data();
    let mut pixels = Vec::with_capacity(width * height * 4);
    for row in data.chunks_exact(stride).take(height) {
        pixels.extend_from_slice(&row[..width * 4]);
    }
    // CoreGraphics only draws premultiplied; GPUI expects straight alpha.
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha != 0 && alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
    let scale = width as f32 / points_width;
    Some((
        Arc::new(RenderImage::new([image::Frame::new(buffer)])),
        scale,
    ))
}
