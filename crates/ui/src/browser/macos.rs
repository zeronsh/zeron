//! AppKit boundary for the browser. Wry owns the native child and its UI
//! delegate; our navigation delegate supplies browser policy and state. All
//! callbacks enqueue events, never re-enter GPUI. No page-to-engine IPC.
use super::model::{PageState, Presentation, allowed_navigation};
use gpui::{Bounds, Pixels, Window};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{
    NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions, NSObject,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationResponse, WKNavigationResponsePolicy, WKWebView,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wry::{WebView, WebViewBuilderExtMacos, WebViewExtMacOS};

#[derive(Default)]
struct BrowserStore {
    store: Option<Retained<objc2_web_kit::WKWebsiteDataStore>>,
    preview_hosts: std::collections::BTreeSet<String>,
}
#[derive(Clone, Default)]
pub(super) struct BrowserData(Rc<RefCell<BrowserStore>>);
impl BrowserData {
    fn configuration(
        &self,
        mtm: MainThreadMarker,
    ) -> Retained<objc2_web_kit::WKWebViewConfiguration> {
        let mut data = self.0.borrow_mut();
        if data.store.is_none() {
            data.store =
                Some(unsafe { objc2_web_kit::WKWebsiteDataStore::nonPersistentDataStore(mtm) });
            if let Err(error) =
                configure_preview_proxy(data.store.as_ref().unwrap(), &data.preview_hosts)
            {
                tracing::warn!(%error, "preview hostname proxy unavailable");
            }
        }
        let configuration = unsafe { objc2_web_kit::WKWebViewConfiguration::new(mtm) };
        unsafe {
            configuration.setWebsiteDataStore(data.store.as_ref().unwrap());
        }
        configuration
    }
    pub(super) fn register_preview(&self, address: &str) {
        let Ok(url) = url::Url::parse(address) else {
            return;
        };
        let Some(host) = url.host_str() else {
            return;
        };
        if url.scheme() != "http"
            || url.port() != Some(zeron_proto::PREVIEW_PROXY_PORT)
            || !host.ends_with(".localhost")
        {
            return;
        }
        let mut data = self.0.borrow_mut();
        if !data.preview_hosts.insert(host.into()) {
            return;
        }
        if let Some(store) = &data.store {
            if let Err(error) = configure_preview_proxy(store, &data.preview_hosts) {
                tracing::warn!(%error, "preview hostname proxy unavailable");
            }
        }
    }
}

/// Network.framework objects are Objective-C OS objects. Resolve the newer API
/// dynamically so older systems can still open ordinary browser tabs. Match
/// only discovered/opened preview hostnames; other websites retain normal routing.
fn configure_preview_proxy(
    store: &objc2_web_kit::WKWebsiteDataStore,
    hosts: &std::collections::BTreeSet<String>,
) -> Result<(), String> {
    if hosts.is_empty() {
        return Ok(());
    }
    use objc2_foundation::{NSArray, NSObject};
    use std::ffi::{CStr, CString, c_char};
    unsafe {
        let supported: bool = msg_send![store, respondsToSelector: sel!(setProxyConfigurations:)];
        if !supported {
            return Err("Automatic preview hostnames require macOS 14 or later".into());
        }
        static NETWORK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let handle = *NETWORK.get_or_init(|| {
            libc::dlopen(
                c"/System/Library/Frameworks/Network.framework/Network".as_ptr(),
                libc::RTLD_LAZY,
            ) as usize
        });
        let symbol = |name: &CStr| -> Result<*mut std::ffi::c_void, String> {
            let value = libc::dlsym(handle as *mut _, name.as_ptr());
            if handle == 0 || value.is_null() {
                Err("Preview proxy API unavailable".into())
            } else {
                Ok(value)
            }
        };
        let endpoint: unsafe extern "C" fn(*const c_char, *const c_char) -> *mut NSObject =
            std::mem::transmute(symbol(c"nw_endpoint_create_host")?);
        let proxy: unsafe extern "C" fn(*mut NSObject, *mut NSObject) -> *mut NSObject =
            std::mem::transmute(symbol(c"nw_proxy_config_create_http_connect")?);
        let match_domain: unsafe extern "C" fn(*mut NSObject, *const c_char) =
            std::mem::transmute(symbol(c"nw_proxy_config_add_match_domain")?);
        let endpoint = Retained::from_raw(endpoint(c"127.0.0.1".as_ptr(), c"7331".as_ptr()))
            .ok_or("Could not create preview endpoint")?;
        let config = Retained::from_raw(proxy(
            Retained::as_ptr(&endpoint) as *mut _,
            std::ptr::null_mut(),
        ))
        .ok_or("Could not create preview proxy")?;
        for host in hosts {
            let host = CString::new(host.as_str()).map_err(|_| "Invalid preview hostname")?;
            match_domain(Retained::as_ptr(&config) as *mut _, host.as_ptr());
        }
        let proxies = NSArray::arrayWithObject(&*config);
        let _: () = msg_send![store, setProxyConfigurations: &*proxies];
    }
    Ok(())
}

type Sender = tokio::sync::mpsc::Sender<NativeEvent>;
const OBSERVED: [&str; 6] = [
    "URL",
    "title",
    "loading",
    "canGoBack",
    "canGoForward",
    "underPageBackgroundColor",
];

pub(super) enum NativeEvent {
    Changed,
    Finished,
    NewTab(String),
    Key(gpui::Keystroke),
    Favicon { page: String, url: String },
    InspectElement(super::model::InspectedElement),
    ConsoleLog(super::model::ConsoleLogEntry),
}

struct ObserverState {
    tx: Sender,
    pending: Cell<bool>,
    error: RefCell<Option<String>>,
    requested_url: RefCell<Option<String>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZeronBrowserObserver"]
    #[ivars = ObserverState]
    struct Observer;
    unsafe impl NSObjectProtocol for Observer {}
    impl Observer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(&self, _key: Option<&NSString>, _object: Option<&AnyObject>, _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, _context: *mut std::ffi::c_void) {
            self.changed();
        }
    }
    unsafe impl WKNavigationDelegate for Observer {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn policy(&self, _view: &WKWebView, action: &WKNavigationAction, decision: &block2::Block<dyn Fn(WKNavigationActionPolicy)>) {
            let url = unsafe { action.request().URL() }.and_then(|u| u.absoluteString()).map(|u| u.to_string()).unwrap_or_default();
            if allowed_navigation(&url) && unsafe { action.targetFrame() }.is_some_and(|frame| unsafe { frame.isMainFrame() }) {
                *self.ivars().requested_url.borrow_mut() = Some(url.clone());
            }
            decision.call((if allowed_navigation(&url) { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Cancel },));
        }
        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn response(&self, _view: &WKWebView, response: &WKNavigationResponse, decision: &block2::Block<dyn Fn(WKNavigationResponsePolicy)>) {
            let displayable = unsafe { response.canShowMIMEType() };
            if !displayable { self.fail("This file can’t be previewed here. Open it in your default browser."); }
            decision.call((if displayable { WKNavigationResponsePolicy::Allow } else { WKNavigationResponsePolicy::Cancel },));
        }
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn start(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().error.borrow_mut().take(); self.changed();
        }
        #[unsafe(method(webView:didCommitNavigation:))]
        fn commit(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().requested_url.borrow_mut().take(); self.changed();
        }
        #[unsafe(method(webView:didFinishNavigation:))]
        fn finish(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.changed(); let _ = self.ivars().tx.try_send(NativeEvent::Finished);
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn provisional_error(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            if error.code() != -999 {
                tracing::warn!(domain = %error.domain(), code = error.code(), "browser provisional navigation failed");
                self.fail("Check the address and make sure your server is running, then try again.");
            }
        }
        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn navigation_error(&self, _view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            if error.code() != -999 {
                tracing::warn!(domain = %error.domain(), code = error.code(), "browser navigation failed");
                self.fail("The connection was interrupted. Try loading this page again.");
            }
        }
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn terminated(&self, _view: &WKWebView) {
            self.fail("The page stopped responding. Reload to continue.");
        }
    }
);

impl Observer {
    fn new(tx: Sender, mtm: MainThreadMarker) -> Retained<Self> {
        let object = mtm.alloc().set_ivars(ObserverState {
            tx,
            pending: Cell::new(false),
            error: RefCell::new(None),
            requested_url: RefCell::new(None),
        });
        unsafe { msg_send![super(object), init] }
    }
    fn changed(&self) {
        if !self.ivars().pending.replace(true) {
            if self.ivars().tx.try_send(NativeEvent::Changed).is_err() {
                self.ivars().pending.set(false);
            }
        }
    }
    fn fail(&self, message: &str) {
        *self.ivars().error.borrow_mut() = Some(message.into());
        self.changed();
    }
}

struct ClipState {
    dragging: Cell<bool>,
    region: Cell<objc2_foundation::NSRect>,
    resize_inset: Cell<f64>,
}

// Clips native content to GPUI's current paint mask. During app drags the
// entire native subtree passes pointer events back to GPUI, without hiding it.
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZeronBrowserClipView"]
    #[ivars = ClipState]
    struct BrowserClipView;
    impl BrowserClipView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: objc2_foundation::NSPoint) -> *mut NSView {
            let region = self.ivars().region.get();
            if self.ivars().dragging.get()
                || point.x < region.origin.x + self.ivars().resize_inset.get() || point.x >= region.origin.x + region.size.width
                || point.y < region.origin.y || point.y >= region.origin.y + region.size.height
            { std::ptr::null_mut() }
            else { unsafe { msg_send![super(self), hitTest: point] } }
        }
    }
);

pub(super) struct NativePage(Rc<RefCell<Host>>);

pub(super) struct Host {
    web: WebView,
    data: BrowserData,
    view: Retained<WKWebView>,
    observer: Retained<Observer>,
    clip: Retained<BrowserClipView>,
    clip_mask: Retained<AnyObject>,
    parent: Retained<NSView>,
    visible_bounds: Option<Bounds<Pixels>>,
    background_color: Option<Retained<NSColor>>,
    monitor: Option<Retained<AnyObject>>,
    shortcuts: Rc<RefCell<Vec<String>>>,
    bounds: Option<Bounds<Pixels>>,
    presentation: Presentation,
    tx: Sender,
    #[cfg(feature = "browser-fixture")]
    visibility_changes: Cell<u64>,
}

const CONSOLE_HOOK_SCRIPT: &str = r#"
(() => {
    if (window.__zeron_console_hooked) return;
    window.__zeron_console_hooked = true;
    window.__zeron_console_logs = window.__zeron_console_logs || [];

    const send = (level, text) => {
        const entry = { level: String(level), text: String(text), timestamp: Date.now() };
        window.__zeron_console_logs.push(entry);
        if (window.__zeron_console_logs.length > 200) {
            window.__zeron_console_logs.shift();
        }
        try {
            if (window.ipc && window.ipc.postMessage) {
                window.ipc.postMessage(JSON.stringify({ action: "console_log", log: entry }));
            }
        } catch (_) {}
    };

    const fmt = (args) => args.map(a => {
        try {
            return typeof a === 'object' ? JSON.stringify(a) : String(a);
        } catch (_) {
            return String(a);
        }
    }).join(' ');

    const origLog = console.log;
    console.log = (...args) => {
        origLog.apply(console, args);
        send("log", fmt(args));
    };
    const origWarn = console.warn;
    console.warn = (...args) => {
        origWarn.apply(console, args);
        send("warn", fmt(args));
    };
    const origError = console.error;
    console.error = (...args) => {
        origError.apply(console, args);
        send("error", fmt(args));
    };
    const origInfo = console.info;
    console.info = (...args) => {
        origInfo.apply(console, args);
        send("info", fmt(args));
    };
    window.addEventListener("error", (e) => {
        send("error", `${e.message} (${e.filename || 'script'}:${e.lineno || 0}:${e.colno || 0})`);
    });
})();
"#;

fn capture_snapshot(
    view: &WKWebView,
    rect: Option<objc2_foundation::NSRect>,
    callback: impl FnOnce(Option<Vec<u8>>) + 'static,
) {
    let callback = std::cell::Cell::new(Some(callback));
    let completion = block2::RcBlock::new(move |snapshot: *mut AnyObject, _err: *mut AnyObject| {
        if let Some(cb) = callback.take() {
            if !snapshot.is_null() {
                unsafe {
                    let tiff: *mut AnyObject = msg_send![snapshot, TIFFRepresentation];
                    if !tiff.is_null() {
                        let len: usize = msg_send![tiff, length];
                        let bytes: *const u8 = msg_send![tiff, bytes];
                        let slice = std::slice::from_raw_parts(bytes, len);
                        if let Ok(img) = image::load_from_memory_with_format(slice, image::ImageFormat::Tiff) {
                            let mut png_bytes = std::io::Cursor::new(Vec::new());
                            if img.write_to(&mut png_bytes, image::ImageFormat::Png).is_ok() {
                                cb(Some(png_bytes.into_inner()));
                                return;
                            }
                        }
                    }
                }
            }
            cb(None);
        }
    });

    unsafe {
        let config_class = objc2::class!(WKSnapshotConfiguration);
        let config: *mut AnyObject = {
            let cfg: *mut AnyObject = msg_send![config_class, new];
            if let Some(r) = rect {
                let _: () = msg_send![cfg, setRect: r];
            }
            cfg
        };
        let _: () = msg_send![view, takeSnapshotWithConfiguration: config, completionHandler: &*completion];
        if !config.is_null() {
            let _: () = msg_send![config, release];
        }
    }
}

impl NativePage {
    pub fn new(window: &Window, data: &BrowserData, tx: Sender) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("Browser must be created on the main thread")?;
        let new_tab = tx.clone();
        let view_cell: std::rc::Rc<std::cell::RefCell<Option<Weak<WKWebView>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let view_for_ipc = view_cell.clone();
        let inspect_tx = tx.clone();
        let console_tx = tx.clone();
        let web = wry::WebViewBuilder::new()
            .with_webview_configuration(data.configuration(mtm))
            .with_visible(false)
            .with_focused(false)
            .with_incognito(true)
            .with_initialization_script(CONSOLE_HOOK_SCRIPT)
            .with_ipc_handler(move |request| {
                let body = request.body();
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
                    let action = value.get("action").and_then(|v| v.as_str());
                    if action == Some("console_log") {
                        if let Some(log_val) = value.get("log") {
                            if let Ok(entry) = serde_json::from_value::<super::model::ConsoleLogEntry>(log_val.clone()) {
                                let _ = console_tx.try_send(NativeEvent::ConsoleLog(entry));
                            }
                        }
                    } else if action == Some("inspect") || action == Some("inspect_submit") {
                        if let Some(elem_val) = value.get("element") {
                            if let Ok(mut elem) = serde_json::from_value::<super::model::InspectedElement>(elem_val.clone()) {
                                if let Some(prompt) = value.get("user_prompt").and_then(|v| v.as_str()) {
                                    elem.user_prompt = Some(prompt.to_string());
                                }
                                if let Some(elems_val) = value.get("elements").or_else(|| elem_val.get("elements")) {
                                    if let Ok(elems) = serde_json::from_value::<Vec<super::model::InspectedElement>>(elems_val.clone()) {
                                        elem.elements = elems;
                                    }
                                }
                                let x = elem_val.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let y = elem_val.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let w = elem_val.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let h = elem_val.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let rect = if w > 0.0 && h > 0.0 {
                                    Some(objc2_foundation::NSRect::new(
                                        objc2_foundation::NSPoint::new(x, y),
                                        objc2_foundation::NSSize::new(w, h),
                                    ))
                                } else {
                                    None
                                };
                                let inspect_tx = inspect_tx.clone();

                                if let Some(view) = view_for_ipc.borrow().as_ref().and_then(|w| w.load()) {
                                    capture_snapshot(&view, rect, move |bytes| {
                                        elem.screenshot = bytes;
                                        let _ = inspect_tx.try_send(NativeEvent::InspectElement(elem));
                                    });
                                } else {
                                    let _ = inspect_tx.try_send(NativeEvent::InspectElement(elem));
                                }
                            }
                        }
                    }
                }
            })
            .with_new_window_req_handler(move |url, _| {
                if allowed_navigation(&url) {
                    let _ = new_tab.try_send(NativeEvent::NewTab(url));
                }
                wry::NewWindowResponse::Deny
            })
            .with_download_started_handler(|_, _| false)
            .build_as_child(window)
            .map_err(|e| e.to_string())?;
        let view = Retained::into_super(web.webview());
        *view_cell.borrow_mut() = Some(Weak::from_retained(&view));
        window
            .enable_scene_overlay()
            .map_err(|error| error.to_string())?;
        let parent = unsafe { view.superview() }.ok_or("Browser parent is missing")?;
        let clip: Retained<BrowserClipView> = unsafe {
            let object = mtm.alloc().set_ivars(ClipState {
                dragging: Cell::new(false),
                region: Cell::new(objc2_foundation::NSRect::ZERO),
                resize_inset: Cell::new(0.0),
            });
            msg_send![super(object), initWithFrame: objc2_foundation::NSRect::ZERO]
        };
        clip.setWantsLayer(true);
        clip.setAutoresizesSubviews(false);
        let clip_mask: Retained<AnyObject> = unsafe { msg_send![class!(CALayer), new] };
        unsafe {
            let color = NSColor::blackColor().CGColor();
            let _: () = msg_send![&*clip_mask, setBackgroundColor: &*color];
            let layer: *mut AnyObject = msg_send![&*clip, layer];
            let _: () = msg_send![layer, setMasksToBounds: true];
            let _: () = msg_send![layer, setMask: &*clip_mask];
        }
        clip.setHidden(true);
        parent.addSubview(&clip);
        // Keep every native tab beneath the shared GPUI overlay plane.
        for sibling in parent.subviews() {
            if sibling.class().name() == c"GPUIOverlayView" {
                parent.addSubview_positioned_relativeTo(
                    &clip,
                    NSWindowOrderingMode::Below,
                    Some(&sibling),
                );
                break;
            }
        }
        clip.addSubview(&view);

        let observer = Observer::new(tx.clone(), mtm);
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*observer)));
            for key in OBSERVED {
                view.addObserver_forKeyPath_options_context(
                    &observer,
                    &NSString::from_str(key),
                    NSKeyValueObservingOptions::empty(),
                    std::ptr::null_mut(),
                );
            }
        }
        let shortcuts = Rc::new(RefCell::new(Vec::<String>::new()));
        let monitor_view = view.clone();
        let monitor_tx = tx.clone();
        let monitor_shortcuts = shortcuts.clone();
        let callback = block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| {
            let e = unsafe { event.as_ref() };
            if monitor_view.isHidden()
                || !has_focus(&monitor_view)
                || e.window(mtm) != monitor_view.window()
            {
                return event.as_ptr();
            }
            let mods = e.modifierFlags();
            let key = e
                .charactersIgnoringModifiers()
                .map(|s| s.to_string().to_lowercase())
                .unwrap_or_default();
            let mut combo = String::new();
            if mods.contains(NSEventModifierFlags::Command) {
                combo.push_str("cmd-");
            }
            if mods.contains(NSEventModifierFlags::Control) {
                combo.push_str("ctrl-");
            }
            if mods.contains(NSEventModifierFlags::Option) {
                combo.push_str("alt-");
            }
            if mods.contains(NSEventModifierFlags::Shift) {
                combo.push_str("shift-");
            }
            combo.push_str(&key);
            let Ok(keystroke) = gpui::Keystroke::parse(&combo) else {
                return event.as_ptr();
            };
            let browser_key = matches!(
                combo.as_str(),
                "cmd-l" | "cmd-t" | "cmd-w" | "cmd-[" | "cmd-]" | "cmd-shift-r" | "cmd-k" | "cmd-," | "cmd-shift-d"
            );
            let app_key = monitor_shortcuts
                .borrow()
                .iter()
                .any(|s| gpui::Keystroke::parse(s).is_ok_and(|s| s == keystroke));
            if browser_key || app_key {
                if monitor_tx.try_send(NativeEvent::Key(keystroke)).is_ok() {
                    std::ptr::null_mut()
                } else {
                    event.as_ptr()
                }
            } else {
                event.as_ptr()
            }
        });
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &callback)
        };
        Ok(Self(Rc::new(RefCell::new(Host {
            data: data.clone(),
            web,
            view,
            observer,
            clip,
            clip_mask,
            parent,
            visible_bounds: None,
            background_color: None,
            monitor,
            shortcuts,
            bounds: None,
            presentation: Presentation::Hidden,
            #[cfg(feature = "browser-fixture")]
            visibility_changes: Cell::new(0),
            tx,
        }))))
    }
    pub fn handle(&self) -> Rc<RefCell<Host>> {
        self.0.clone()
    }
    pub fn focus_chrome(&self) {
        let _ = self.0.borrow().web.focus_parent();
    }
    pub fn set_shortcuts(&self, shortcuts: Vec<String>) {
        *self.0.borrow().shortcuts.borrow_mut() = shortcuts;
    }
    pub fn present(&mut self, presentation: Presentation) {
        self.0.borrow_mut().present(presentation);
    }
    pub fn load(&self, url: &str) -> Result<(), String> {
        let host = self.0.borrow();
        host.data.register_preview(url);
        host.observer.ivars().error.borrow_mut().take();
        *host.observer.ivars().requested_url.borrow_mut() = Some(url.into());
        host.web.load_url(url).map_err(|e| e.to_string())
    }
    pub fn reload(&self) {
        let host = self.0.borrow();
        host.observer.ivars().error.borrow_mut().take();
        // Retrying the requested URL also works after a failed provisional load.
        unsafe {
            host.view.reload();
        }
        host.observer.changed();
    }
    pub fn history(&self, forward: bool) {
        let host = self.0.borrow();
        unsafe {
            if forward {
                host.view.goForward();
            } else {
                host.view.goBack();
            }
        }
    }
    pub fn state(&self) -> PageState {
        let host = self.0.borrow();
        host.observer.ivars().pending.set(false);
        unsafe {
            PageState {
                url: host
                    .observer
                    .ivars()
                    .requested_url
                    .borrow()
                    .clone()
                    .or_else(|| {
                        host.view
                            .URL()
                            .and_then(|u| u.absoluteString())
                            .map(|u| u.to_string())
                    }),
                title: host.view.title().map(|s| s.to_string()).unwrap_or_default(),
                loading: host.view.isLoading(),
                can_back: host.view.canGoBack(),
                can_forward: host.view.canGoForward(),
                error: host.observer.ivars().error.borrow().clone(),
            }
        }
    }
    pub fn discover_favicon(&self, page: String) {
        let host = self.0.borrow();
        let tx = host.tx.clone();
        // Our navigation delegate owns readiness, so Wry's private pending-
        // script queue is intentionally bypassed. This runs only after finish.
        let completion = block2::RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            if !error.is_null() {
                return;
            }
            if let Some(url) =
                unsafe { value.as_ref() }.and_then(|value| value.downcast_ref::<NSString>())
            {
                let _ = tx.try_send(NativeEvent::Favicon {
                    page: page.clone(),
                    url: url.to_string(),
                });
            }
        });
        unsafe {
            host.view.evaluateJavaScript_completionHandler(
                &NSString::from_str("(() => { const link = document.querySelector('link[rel~=icon]'); return link ? link.href : new URL('/favicon.ico', location.href).href; })()"),
                Some(&completion),
            );
        }
    }
    pub fn sync_design_mode_theme(&self, theme: &crate::theme::Theme) {
        let host = self.0.borrow();
        let css = super::model::DesignPopupTheme::from_theme(theme);
        let theme_json = serde_json::to_string(&css).unwrap_or_default();
        let script = format!(
            r#"(() => {{
                if (window.__zeron_apply_design_theme) {{
                    window.__zeron_apply_design_theme({theme_json});
                }}
            }})()"#
        );
        let ns_script = NSString::from_str(&script);
        unsafe {
            host.view.evaluateJavaScript_completionHandler(&ns_script, None);
        }
    }
    pub fn set_design_mode(&self, enabled: bool, theme: &crate::theme::Theme) {
        let host = self.0.borrow();
        let css = super::model::DesignPopupTheme::from_theme(theme);
        let theme_json = serde_json::to_string(&css).unwrap_or_default();
        let script = format!(
            r#"(() => {{
                if (window.__zeron_apply_design_theme) {{
                    window.__zeron_apply_design_theme({theme_json});
                }}
                if (window.__zeron_toggle_design_mode) {{
                    window.__zeron_toggle_design_mode({enabled});
                    return;
                }}
                if (!{enabled}) return;
                window.__zeron_design_mode_installed = true;
                let active = true;

                let overlay = document.createElement('div');
                overlay.id = '__zeron_design_hover__';
                overlay.style.position = 'fixed';
                overlay.style.pointerEvents = 'none';
                overlay.style.zIndex = '2147483645';
                overlay.style.border = '2px solid #3b82f6';
                overlay.style.backgroundColor = 'rgba(59, 130, 246, 0.12)';
                overlay.style.borderRadius = '2px';
                overlay.style.display = 'none';
                overlay.style.transition = 'all 0.05s ease';

                let badge = document.createElement('div');
                badge.id = '__zeron_design_badge__';
                badge.style.position = 'fixed';
                badge.style.pointerEvents = 'none';
                badge.style.zIndex = '2147483647';
                badge.style.padding = '3px 8px';
                badge.style.borderRadius = '6px';
                badge.style.fontSize = '11px';
                badge.style.fontFamily = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
                badge.style.color = '#f4f4f5';
                badge.style.backgroundColor = 'rgba(24, 24, 27, 0.95)';
                badge.style.border = '1px solid rgba(255, 255, 255, 0.15)';
                badge.style.boxShadow = '0 4px 12px rgba(0,0,0,0.35)';
                badge.style.display = 'none';
                badge.style.whiteSpace = 'nowrap';
                badge.style.maxWidth = '360px';
                badge.style.overflow = 'hidden';
                badge.style.textOverflow = 'ellipsis';

                let currentTheme = null;
                function getColor(idx) {{
                    if (currentTheme && currentTheme.palette && currentTheme.palette.length > 0) {{
                        return currentTheme.palette[idx % currentTheme.palette.length];
                    }}
                    let fallback = [
                        {{ border: '#3b82f6', bg: 'rgba(59, 130, 246, 0.18)', text: '#60a5fa', shadow: '0 0 0 1px rgba(59, 130, 246, 0.4)' }},
                        {{ border: '#818cf8', bg: 'rgba(129, 140, 248, 0.18)', text: '#a5b4fc', shadow: '0 0 0 1px rgba(129, 140, 248, 0.4)' }},
                        {{ border: '#34d399', bg: 'rgba(52, 211, 153, 0.18)', text: '#6ee7b7', shadow: '0 0 0 1px rgba(52, 211, 153, 0.4)' }},
                        {{ border: '#fb923c', bg: 'rgba(251, 146, 60, 0.18)', text: '#fdba74', shadow: '0 0 0 1px rgba(251, 146, 60, 0.4)' }},
                        {{ border: '#f472b6', bg: 'rgba(244, 114, 182, 0.18)', text: '#f9a8d4', shadow: '0 0 0 1px rgba(244, 114, 182, 0.4)' }},
                        {{ border: '#38bdf8', bg: 'rgba(56, 189, 248, 0.18)', text: '#7dd3fc', shadow: '0 0 0 1px rgba(56, 189, 248, 0.4)' }},
                    ];
                    return fallback[idx % fallback.length];
                }}

                let dragOverlay = document.createElement('div');
                dragOverlay.id = '__zeron_design_drag__';
                dragOverlay.style.position = 'fixed';
                dragOverlay.style.pointerEvents = 'none';
                dragOverlay.style.zIndex = '2147483646';
                dragOverlay.style.border = '2px dashed #3b82f6';
                dragOverlay.style.backgroundColor = 'rgba(59, 130, 246, 0.15)';
                dragOverlay.style.borderRadius = '2px';
                dragOverlay.style.display = 'none';

                let popup = document.createElement('div');
                popup.id = '__zeron_design_popup__';
                popup.style.position = 'fixed';
                popup.style.zIndex = '2147483647';
                popup.style.display = 'none';
                popup.style.alignItems = 'center';
                popup.style.gap = '6px';
                popup.style.background = '#18181b';
                popup.style.border = '1px solid rgba(255, 255, 255, 0.16)';
                popup.style.borderRadius = '9999px';
                popup.style.boxShadow = '0 8px 24px rgba(0, 0, 0, 0.5), 0 2px 6px rgba(0, 0, 0, 0.3)';
                popup.style.padding = '4px 6px 4px 8px';
                popup.style.boxSizing = 'border-box';
                popup.style.userSelect = 'none';
                popup.style.width = '380px';
                popup.style.maxWidth = 'calc(100vw - 24px)';
                popup.style.fontFamily = '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
                popup.style.transition = 'border-radius 0.15s ease, padding 0.15s ease';

                popup.addEventListener('pointerdown', (e) => e.stopPropagation());
                popup.addEventListener('mousedown', (e) => e.stopPropagation());
                popup.addEventListener('mouseup', (e) => e.stopPropagation());
                popup.addEventListener('click', (e) => e.stopPropagation());

                let firstPill = document.createElement('span');
                firstPill.className = '__zeron_first_pill__';
                firstPill.style.display = 'none';
                firstPill.style.alignItems = 'center';
                firstPill.style.fontSize = '11px';
                firstPill.style.fontWeight = '600';
                firstPill.style.fontFamily = 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace';
                firstPill.style.padding = '2px 7px';
                firstPill.style.borderRadius = '6px';
                firstPill.style.flexShrink = '0';
                firstPill.style.userSelect = 'none';
                firstPill.style.lineHeight = '14px';

                let input = document.createElement('div');
                input.id = '__zeron_design_input__';
                input.contentEditable = 'true';
                input.setAttribute('role', 'textbox');
                input.setAttribute('aria-multiline', 'true');
                input.setAttribute('spellcheck', 'false');
                input.setAttribute('placeholder', 'Describe the change');
                input.setAttribute('data-empty', 'true');
                input.style.background = 'transparent';
                input.style.border = 'none';
                input.style.outline = 'none';
                input.style.color = '#ffffff';
                input.style.fontSize = '13px';
                input.style.flexGrow = '1';
                input.style.minWidth = '140px';
                input.style.fontFamily = 'inherit';
                input.style.lineHeight = '20px';
                input.style.minHeight = '20px';
                input.style.maxHeight = '160px';
                input.style.overflowY = 'hidden';
                input.style.wordBreak = 'break-word';
                input.style.whiteSpace = 'pre-wrap';
                input.style.padding = '0';
                input.style.margin = '0';
                input.style.boxSizing = 'border-box';
                input.style.userSelect = 'text';

                let submitBtn = document.createElement('button');
                submitBtn.className = '__zeron_submit_btn__';
                submitBtn.style.width = '24px';
                submitBtn.style.height = '24px';
                submitBtn.style.borderRadius = '50%';
                submitBtn.style.background = '#ffffff';
                submitBtn.style.color = '#18181b';
                submitBtn.style.border = 'none';
                submitBtn.style.outline = 'none';
                submitBtn.style.cursor = 'pointer';
                submitBtn.style.display = 'inline-flex';
                submitBtn.style.alignItems = 'center';
                submitBtn.style.justifyContent = 'center';
                submitBtn.style.padding = '0';
                submitBtn.style.flexShrink = '0';
                submitBtn.style.transition = 'opacity 0.15s ease';
                submitBtn.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M5 12l7-7 7 7"/></svg>';
                submitBtn.onmouseenter = () => submitBtn.style.opacity = '0.85';
                submitBtn.onmouseleave = () => submitBtn.style.opacity = '1';

                popup.appendChild(firstPill);
                popup.appendChild(input);
                popup.appendChild(submitBtn);

                let initialStyleTag = document.getElementById('__zeron_design_theme_styles__');
                if (!initialStyleTag) {{
                    initialStyleTag = document.createElement('style');
                    initialStyleTag.id = '__zeron_design_theme_styles__';
                    document.head.appendChild(initialStyleTag);
                }}
                initialStyleTag.textContent = '#__zeron_design_input__:empty::before, #__zeron_design_input__[data-empty="true"]::before {{ content: attr(placeholder); color: rgba(255, 255, 255, 0.4); pointer-events: none; display: inline-block; }} #__zeron_design_input__::-webkit-scrollbar {{ width: 4px; }} #__zeron_design_input__::-webkit-scrollbar-thumb {{ background: rgba(255,255,255,0.2); border-radius: 2px; }}';

                document.documentElement.appendChild(overlay);
                document.documentElement.appendChild(badge);
                document.documentElement.appendChild(dragOverlay);
                document.documentElement.appendChild(popup);

                let hoveredEl = null;
                let selectedElements = [];
                let rafId = null;

                let isMouseDown = false;
                let startX = 0, startY = 0;
                let isDragging = false;

                let lockedScrollX = window.scrollX || window.pageXOffset || 0;
                let lockedScrollY = window.scrollY || window.pageYOffset || 0;
                function lockScroll() {{
                    lockedScrollX = window.scrollX || window.pageXOffset || 0;
                    lockedScrollY = window.scrollY || window.pageYOffset || 0;
                }}

                function getSelector(el) {{
                    if (!(el instanceof Element)) return '';
                    let path = [];
                    while (el && el.nodeType === Node.ELEMENT_NODE) {{
                        let selector = el.nodeName.toLowerCase();
                        if (el.id) {{
                            selector += '#' + el.id;
                            path.unshift(selector);
                            break;
                        }} else {{
                            let sib = el, nth = 1;
                            while (sib = sib.previousElementSibling) {{
                                if (sib.nodeName.toLowerCase() === selector) nth++;
                            }}
                            if (nth !== 1) selector += ':nth-of-type(' + nth + ')';
                        }}
                        path.unshift(selector);
                        el = el.parentNode;
                    }}
                    return path.join(' > ');
                }}

                function getDomPath(el) {{
                    if (!(el instanceof Element)) return '';
                    let path = [];
                    let curr = el;
                    while (curr && curr.nodeType === Node.ELEMENT_NODE && curr !== document.documentElement && curr !== document.body) {{
                        let seg = curr.nodeName.toLowerCase();
                        let classes = (typeof curr.className === 'string' ? curr.className : (curr.className && curr.className.baseVal) || '').trim();
                        if (classes) {{
                            let cls = classes.split(/\s+/).filter(Boolean).join('.');
                            if (cls) seg += '.' + cls;
                        }}
                        let parent = curr.parentElement;
                        if (parent) {{
                            let sameTagSiblings = Array.from(parent.children).filter(c => c.nodeName === curr.nodeName);
                            if (sameTagSiblings.length > 1) {{
                                let idx = sameTagSiblings.indexOf(curr);
                                if (idx >= 0) {{
                                    seg += '[' + idx + ']';
                                }}
                            }}
                        }}
                        path.unshift(seg);
                        curr = curr.parentElement;
                    }}
                    return path.join(' > ');
                }}

                let currentAnchorRect = null;
                let savedRange = null;

                function saveSelection() {{
                    let sel = window.getSelection();
                    if (sel && sel.rangeCount > 0) {{
                        let r = sel.getRangeAt(0);
                        if (input.contains(r.commonAncestorContainer)) {{
                            savedRange = r.cloneRange();
                        }}
                    }}
                }}

                function updatePlaceholder() {{
                    let hasPills = input.querySelector('.__zeron_inline_pill__') !== null;
                    let text = input.innerText.replace(/[\r\n\t\s\u00A0]/g, '');
                    if (!hasPills && text.length === 0) {{
                        input.setAttribute('data-empty', 'true');
                    }} else {{
                        input.removeAttribute('data-empty');
                    }}
                }}

                function adjustInputHeight() {{
                    input.style.height = 'auto';
                    let lineHeight = 20;
                    let minH = 20;
                    let maxH = 8 * lineHeight;
                    let sH = input.scrollHeight;
                    let newH = Math.min(maxH, Math.max(minH, sH));
                    input.style.height = newH + 'px';
                    if (sH > maxH) {{
                        input.style.overflowY = 'auto';
                    }} else {{
                        input.style.overflowY = 'hidden';
                    }}

                    let isMultiLine = sH > 26;
                    if (isMultiLine) {{
                        popup.style.borderRadius = '12px';
                        popup.style.alignItems = 'flex-start';
                        popup.style.padding = '8px 8px 8px 10px';
                        firstPill.style.marginTop = '2px';
                        submitBtn.style.alignSelf = 'flex-end';
                    }} else {{
                        popup.style.borderRadius = '9999px';
                        popup.style.alignItems = 'center';
                        popup.style.padding = '4px 6px 4px 8px';
                        firstPill.style.marginTop = '0';
                        submitBtn.style.alignSelf = 'center';
                    }}

                    updatePopupPosition();
                }}

                function updatePopupPosition() {{
                    if (currentAnchorRect && popup && popup.style.display !== 'none') {{
                        positionPopup(currentAnchorRect);
                    }}
                }}

                function positionBadge(badgeEl, rect) {{
                    let top = rect.top - 22;
                    if (top < 4) top = rect.bottom + 4;
                    let left = Math.min(Math.max(4, rect.left), Math.max(4, window.innerWidth - 120));
                    badgeEl.style.left = left + 'px';
                    badgeEl.style.top = top + 'px';
                }}

                function positionPopup(rect) {{
                    if (rect) currentAnchorRect = rect;
                    if (!currentAnchorRect) return;

                    let popW = popup.offsetWidth || 380;
                    let popH = popup.offsetHeight || 36;

                    let spaceBelow = window.innerHeight - (currentAnchorRect.bottom + 8);
                    let spaceAbove = currentAnchorRect.top - 8;

                    let isAbove = false;
                    if (spaceBelow < popH && spaceAbove > spaceBelow) {{
                        isAbove = true;
                    }}

                    let top = isAbove
                        ? Math.max(8, currentAnchorRect.top - popH - 8)
                        : Math.min(window.innerHeight - popH - 8, currentAnchorRect.bottom + 8);

                    let left = Math.min(
                        Math.max(12, currentAnchorRect.left),
                        Math.max(12, window.innerWidth - popW - 12)
                    );

                    popup.style.top = Math.max(8, top) + 'px';
                    popup.style.left = left + 'px';
                }}

                function clearSelection() {{
                    for (let item of selectedElements) {{
                        if (item.overlayEl && item.overlayEl.parentNode) item.overlayEl.remove();
                        if (item.badgeEl && item.badgeEl.parentNode) item.badgeEl.remove();
                        if (item.pillEl && item.pillEl.parentNode && item.pillEl !== firstPill) item.pillEl.remove();
                    }}
                    selectedElements = [];
                    currentAnchorRect = null;
                    savedRange = null;
                    if (popup) {{
                        popup.style.display = 'none';
                    }}
                    if (firstPill) {{
                        firstPill.style.display = 'none';
                        firstPill.textContent = '';
                    }}
                    if (input) {{
                        input.innerHTML = '';
                        input.style.height = '20px';
                        input.style.overflowY = 'hidden';
                        updatePlaceholder();
                    }}
                }}

                function createInlinePill(item, color) {{
                    let pill = document.createElement('span');
                    pill.className = '__zeron_inline_pill__';
                    pill.contentEditable = 'false';
                    pill.dataset.tag = item.info.tag || 'element';
                    if (item.id) pill.dataset.elemId = item.id;
                    pill.__item = item;
                    pill.style.display = 'inline-block';
                    pill.style.background = color.bg;
                    pill.style.color = color.text;
                    pill.style.border = '1px solid ' + color.border;
                    pill.style.borderRadius = '4px';
                    pill.style.padding = '0 5px';
                    pill.style.margin = '0 2px';
                    pill.style.fontSize = '11px';
                    pill.style.fontWeight = '600';
                    pill.style.fontFamily = 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace';
                    pill.style.lineHeight = '16px';
                    pill.style.userSelect = 'none';
                    pill.style.webkitUserSelect = 'none';
                    pill.style.verticalAlign = 'baseline';
                    pill.style.whiteSpace = 'nowrap';
                    pill.style.cursor = 'default';
                    pill.textContent = item.info.tag || 'element';
                    return pill;
                }}

                function insertPillAtCursor(pill) {{
                    input.focus();
                    let sel = window.getSelection();
                    let range = savedRange;
                    if (!range || !input.contains(range.commonAncestorContainer)) {{
                        range = document.createRange();
                        range.selectNodeContents(input);
                        range.collapse(false);
                    }}

                    range.deleteContents();

                    let spaceAfter = document.createTextNode(' ');

                    range.insertNode(pill);
                    range.setStartAfter(pill);
                    range.collapse(true);

                    range.insertNode(spaceAfter);
                    range.setStartAfter(spaceAfter);
                    range.collapse(true);

                    if (sel) {{
                        sel.removeAllRanges();
                        sel.addRange(range);
                    }}
                    savedRange = range.cloneRange();
                    updatePlaceholder();
                    adjustInputHeight();
                }}

                function syncElementsFromPills() {{
                    if (selectedElements.length <= 1) return;
                    let remainingPills = Array.from(input.querySelectorAll('.__zeron_inline_pill__'));
                    let newSelected = [selectedElements[0]];
                    for (let i = 1; i < selectedElements.length; i++) {{
                        let item = selectedElements[i];
                        let foundIdx = remainingPills.findIndex(p => p.__item === item || p === item.pillEl || (p.dataset.elemId && p.dataset.elemId === item.id));
                        if (foundIdx !== -1) {{
                            newSelected.push(item);
                            remainingPills.splice(foundIdx, 1);
                        }} else {{
                            if (item.overlayEl && item.overlayEl.parentNode) item.overlayEl.remove();
                            if (item.badgeEl && item.badgeEl.parentNode) item.badgeEl.remove();
                        }}
                    }}
                    selectedElements = newSelected;
                }}

                function removeElement(targetItem) {{
                    if (targetItem.overlayEl && targetItem.overlayEl.parentNode) targetItem.overlayEl.remove();
                    if (targetItem.badgeEl && targetItem.badgeEl.parentNode) targetItem.badgeEl.remove();
                    if (targetItem.pillEl && targetItem.pillEl.parentNode && targetItem.pillEl !== firstPill) targetItem.pillEl.remove();

                    selectedElements = selectedElements.filter(it => it !== targetItem);

                    if (selectedElements.length === 0) {{
                        clearSelection();
                        return;
                    }}

                    let primary = selectedElements[0];
                    let primaryCol = getColor(0);
                    primary.overlayEl.style.border = '2px solid ' + primaryCol.border;
                    primary.overlayEl.style.boxShadow = primaryCol.shadow;
                    primary.badgeEl.style.background = primaryCol.bg;
                    primary.badgeEl.style.color = primaryCol.text;
                    primary.badgeEl.style.border = '1px solid ' + primaryCol.border;

                    firstPill.textContent = primary.info.tag || 'element';
                    firstPill.style.background = primaryCol.bg;
                    firstPill.style.color = primaryCol.text;
                    firstPill.style.border = '1px solid ' + primaryCol.border;
                    firstPill.style.display = 'inline-flex';
                    if (primary.pillEl && primary.pillEl.parentNode && primary.pillEl !== firstPill) {{
                        primary.pillEl.remove();
                    }}
                    primary.pillEl = firstPill;

                    for (let i = 1; i < selectedElements.length; i++) {{
                        let it = selectedElements[i];
                        let col = getColor(i);
                        it.overlayEl.style.border = '2px solid ' + col.border;
                        it.overlayEl.style.boxShadow = col.shadow;
                        it.badgeEl.style.background = col.bg;
                        it.badgeEl.style.color = col.text;
                        it.badgeEl.style.border = '1px solid ' + col.border;
                        if (it.pillEl) {{
                            it.pillEl.style.background = col.bg;
                            it.pillEl.style.color = col.text;
                            it.pillEl.style.border = '1px solid ' + col.border;
                        }}
                    }}

                    updatePlaceholder();
                    adjustInputHeight();
                    input.focus();
                }}

                function addElement(el, rect, info) {{
                    let idx = selectedElements.length;
                    let color = getColor(idx);

                    let overlayEl = document.createElement('div');
                    overlayEl.className = '__zeron_design_selected_overlay__';
                    overlayEl.style.position = 'fixed';
                    overlayEl.style.pointerEvents = 'none';
                    overlayEl.style.zIndex = '2147483645';
                    overlayEl.style.border = '2px solid ' + color.border;
                    overlayEl.style.boxShadow = color.shadow;
                    overlayEl.style.borderRadius = '2px';
                    overlayEl.style.left = rect.left + 'px';
                    overlayEl.style.top = rect.top + 'px';
                    overlayEl.style.width = rect.width + 'px';
                    overlayEl.style.height = rect.height + 'px';
                    overlayEl.style.display = 'block';

                    let badgeEl = document.createElement('div');
                    badgeEl.className = '__zeron_design_selected_badge__';
                    badgeEl.style.position = 'fixed';
                    badgeEl.style.pointerEvents = 'none';
                    badgeEl.style.zIndex = '2147483646';
                    badgeEl.style.background = color.bg;
                    badgeEl.style.color = color.text;
                    badgeEl.style.border = '1px solid ' + color.border;
                    badgeEl.style.borderRadius = '4px';
                    badgeEl.style.padding = '1px 6px';
                    badgeEl.style.fontSize = '11px';
                    badgeEl.style.fontWeight = '600';
                    badgeEl.style.fontFamily = 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace';
                    badgeEl.style.whiteSpace = 'nowrap';
                    badgeEl.textContent = info.tag || 'element';
                    if (currentTheme && currentTheme.backdrop_filter && currentTheme.backdrop_filter !== 'none') {{
                        badgeEl.style.webkitBackdropFilter = currentTheme.backdrop_filter;
                        badgeEl.style.backdropFilter = currentTheme.backdrop_filter;
                    }}
                    positionBadge(badgeEl, rect);

                    document.documentElement.appendChild(overlayEl);
                    document.documentElement.appendChild(badgeEl);

                    let item = {{ id: '__zeron_elem_' + (selectedElements.length + 1) + '_' + Date.now(), el, rect, info, overlayEl, badgeEl, pillEl: null }};

                    if (idx === 0) {{
                        firstPill.textContent = info.tag || 'element';
                        firstPill.style.background = color.bg;
                        firstPill.style.color = color.text;
                        firstPill.style.border = '1px solid ' + color.border;
                        firstPill.style.display = 'inline-flex';
                        item.pillEl = firstPill;
                    }} else {{
                        let inlinePill = createInlinePill(item, color);
                        item.pillEl = inlinePill;
                        insertPillAtCursor(inlinePill);
                    }}

                    selectedElements.push(item);

                    overlay.style.display = 'none';
                    badge.style.display = 'none';

                    if (idx === 0) {{
                        input.innerHTML = '';
                        updatePlaceholder();
                        adjustInputHeight();
                        positionPopup(rect);
                        popup.style.display = 'flex';
                    }} else {{
                        adjustInputHeight();
                    }}
                    setTimeout(() => input.focus(), 20);
                }}

                function formatElementPrompt(info) {{
                    let lines = [
                        '@',
                        '```browser_element',
                        'The user selected this node in the browser preview (blue outline in the screenshot).',
                        '',
                        'tag: ' + (info.tag || 'element')
                    ];
                    let domPath = info.domPath || info.selector;
                    if (domPath) {{
                        lines.push('dom_path: ' + domPath);
                    }}
                    if (info.classes) {{
                        lines.push('class: ' + info.classes);
                    }}
                    if (info.text) {{
                        lines.push('visible_text: ' + info.text);
                    }}
                    if (info.bounds) {{
                        lines.push('bounds_css_px: ' + info.bounds);
                    }}
                    if (info.attributes && info.attributes.length > 0) {{
                        lines.push('attributes:');
                        for (let attr of info.attributes) {{
                            lines.push('  ' + attr);
                        }}
                    }}
                    lines.push('```');
                    return lines.join('\n');
                }}

                function getPromptText() {{
                    let result = '';
                    function traverse(node) {{
                        if (node.nodeType === Node.TEXT_NODE) {{
                            result += node.textContent.replace(/\u00A0/g, ' ');
                        }} else if (node.nodeType === Node.ELEMENT_NODE) {{
                            if (node.classList && node.classList.contains('__zeron_inline_pill__')) {{
                                let prompt = '';
                                let item = (node.__item && node.__item.info) ? node.__item : selectedElements.find(it => it.pillEl === node || (node.dataset.elemId && it.id === node.dataset.elemId));
                                if (item && item.info) {{
                                    prompt = formatElementPrompt(item.info);
                                }} else {{
                                    let tag = node.dataset.tag || node.textContent.trim() || 'element';
                                    prompt = formatElementPrompt({{ tag }});
                                }}
                                if (result.length > 0 && !result.endsWith(' ') && !result.endsWith('\n')) {{
                                    result += ' ';
                                }}
                                result += prompt + '\n';
                            }} else if (node.tagName === 'BR') {{
                                result += '\n';
                            }} else {{
                                for (let child of node.childNodes) {{
                                    traverse(child);
                                }}
                            }}
                        }}
                    }}
                    traverse(input);
                    return result.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uFFFC\uFFFD]/g, '').trim();
                }}

                function sanitizeInputText() {{
                    let walker = document.createTreeWalker(input, NodeFilter.SHOW_TEXT, null, false);
                    let node;
                    let modified = false;
                    while ((node = walker.nextNode())) {{
                        if (/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uFFFC\uFFFD]/.test(node.textContent)) {{
                            node.textContent = node.textContent.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uFFFC\uFFFD]/g, '');
                            modified = true;
                        }}
                    }}
                    return modified;
                }}

                function doSubmit() {{
                    if (selectedElements.length === 0) return;
                    syncElementsFromPills();
                    let promptText = getPromptText();
                    let primary = selectedElements[0];
                    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
                    let elementsList = selectedElements.map(s => {{
                        minX = Math.min(minX, s.rect.left);
                        minY = Math.min(minY, s.rect.top);
                        maxX = Math.max(maxX, s.rect.right);
                        maxY = Math.max(maxY, s.rect.bottom);
                        return {{
                            tag: s.info.tag,
                            id: s.info.id || '',
                            classes: s.info.classes || '',
                            selector: s.info.selector || '',
                            text: s.info.text || '',
                            dom_path: s.info.domPath || s.info.selector || '',
                            bounds: s.info.bounds || '',
                            attributes: s.info.attributes || [],
                            x: Math.max(0, s.rect.left),
                            y: Math.max(0, s.rect.top),
                            w: Math.max(0, s.rect.width),
                            h: Math.max(0, s.rect.height),
                        }};
                    }});
                    let unionW = Math.max(10, maxX - minX);
                    let unionH = Math.max(10, maxY - minY);
                    let fullPrompt = formatElementPrompt(primary.info);
                    if (promptText) {{
                        if (promptText.startsWith('@')) {{
                            fullPrompt += '\n' + promptText;
                        }} else {{
                            fullPrompt += '\n ' + promptText;
                        }}
                    }}
                    let payload = {{
                        action: 'inspect_submit',
                        user_prompt: fullPrompt,
                        element: {{
                            tag: primary.info.tag,
                            id: primary.info.id || '',
                            classes: primary.info.classes || '',
                            selector: primary.info.selector || '',
                            text: primary.info.text || '',
                            dom_path: primary.info.domPath || primary.info.selector || '',
                            bounds: primary.info.bounds || '',
                            attributes: primary.info.attributes || [],
                            x: Math.max(0, minX),
                            y: Math.max(0, minY),
                            w: Math.min(window.innerWidth, unionW),
                            h: Math.min(window.innerHeight, unionH),
                            user_prompt: fullPrompt,
                            elements: elementsList
                        }},
                        elements: elementsList
                    }};
                    clearSelection();
                    if (window.ipc && window.ipc.postMessage) {{
                        window.ipc.postMessage(JSON.stringify(payload));
                    }}
                }}

                input.addEventListener('keydown', (e) => {{
                    e.stopPropagation();
                    if (e.key === 'Enter') {{
                        if (e.shiftKey) {{
                            setTimeout(() => {{
                                adjustInputHeight();
                                saveSelection();
                            }}, 0);
                            return;
                        }}
                        e.preventDefault();
                        doSubmit();
                    }} else if (e.key === 'Escape') {{
                        e.preventDefault();
                        clearSelection();
                    }} else if (e.key === 'ArrowLeft') {{
                        let sel = window.getSelection();
                        if (sel && sel.isCollapsed && sel.rangeCount > 0) {{
                            let node = sel.anchorNode;
                            let offset = sel.anchorOffset;
                            let pillBefore = null;
                            if (node === input) {{
                                if (offset > 0 && node.childNodes[offset - 1] && node.childNodes[offset - 1].classList && node.childNodes[offset - 1].classList.contains('__zeron_inline_pill__')) {{
                                    pillBefore = node.childNodes[offset - 1];
                                }}
                            }} else if (node.nodeType === Node.TEXT_NODE && offset === 0) {{
                                let prev = node.previousSibling;
                                if (prev && prev.classList && prev.classList.contains('__zeron_inline_pill__')) {{
                                    pillBefore = prev;
                                }}
                            }}
                            if (pillBefore) {{
                                e.preventDefault();
                                let newRange = document.createRange();
                                if (pillBefore.previousSibling && pillBefore.previousSibling.nodeType === Node.TEXT_NODE) {{
                                    let txt = pillBefore.previousSibling;
                                    newRange.setStart(txt, txt.textContent.length);
                                }} else {{
                                    newRange.setStartBefore(pillBefore);
                                }}
                                newRange.collapse(true);
                                sel.removeAllRanges();
                                sel.addRange(newRange);
                                saveSelection();
                                return;
                            }}
                        }}
                    }} else if (e.key === 'ArrowRight') {{
                        let sel = window.getSelection();
                        if (sel && sel.isCollapsed && sel.rangeCount > 0) {{
                            let node = sel.anchorNode;
                            let offset = sel.anchorOffset;
                            let pillAfter = null;
                            if (node === input) {{
                                if (offset < node.childNodes.length && node.childNodes[offset] && node.childNodes[offset].classList && node.childNodes[offset].classList.contains('__zeron_inline_pill__')) {{
                                    pillAfter = node.childNodes[offset];
                                }}
                            }} else if (node.nodeType === Node.TEXT_NODE && offset === node.textContent.length) {{
                                let next = node.nextSibling;
                                if (next && next.classList && next.classList.contains('__zeron_inline_pill__')) {{
                                    pillAfter = next;
                                }}
                            }}
                            if (pillAfter) {{
                                e.preventDefault();
                                let newRange = document.createRange();
                                if (pillAfter.nextSibling && pillAfter.nextSibling.nodeType === Node.TEXT_NODE) {{
                                    newRange.setStart(pillAfter.nextSibling, 0);
                                }} else {{
                                    newRange.setStartAfter(pillAfter);
                                }}
                                newRange.collapse(true);
                                sel.removeAllRanges();
                                sel.addRange(newRange);
                                saveSelection();
                                return;
                            }}
                        }}
                    }} else if (e.key === 'Backspace') {{
                        let hasPills = input.querySelector('.__zeron_inline_pill__') !== null;
                        let text = input.innerText.replace(/[\r\n\t\s\u00A0]/g, '');
                        if (!hasPills && text.length === 0) {{
                            e.preventDefault();
                            clearSelection();
                        }} else {{
                            setTimeout(() => {{
                                syncElementsFromPills();
                                updatePlaceholder();
                                adjustInputHeight();
                                saveSelection();
                            }}, 0);
                        }}
                    }}
                }});
                input.addEventListener('beforeinput', (e) => {{
                    e.stopPropagation();
                    if (e.data && /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\uFFFC\uFFFD]/.test(e.data)) {{
                        e.preventDefault();
                        return;
                    }}
                    let sel = window.getSelection();
                    if (sel && sel.anchorNode) {{
                        let target = sel.anchorNode;
                        if (target.nodeType === Node.ELEMENT_NODE && target.closest('.__zeron_inline_pill__')) {{
                            e.preventDefault();
                            return;
                        }}
                        if (target.parentElement && target.parentElement.closest('.__zeron_inline_pill__')) {{
                            e.preventDefault();
                            return;
                        }}
                    }}
                }});
                input.addEventListener('keyup', (e) => {{
                    e.stopPropagation();
                    saveSelection();
                    syncElementsFromPills();
                    updatePlaceholder();
                    adjustInputHeight();
                }});
                input.addEventListener('mouseup', (e) => {{
                    e.stopPropagation();
                    let sel = window.getSelection();
                    if (sel && sel.anchorNode) {{
                        let pill = null;
                        if (sel.anchorNode.nodeType === Node.ELEMENT_NODE) {{
                            pill = sel.anchorNode.closest('.__zeron_inline_pill__');
                        }} else if (sel.anchorNode.parentElement) {{
                            pill = sel.anchorNode.parentElement.closest('.__zeron_inline_pill__');
                        }}
                        if (pill) {{
                            let newRange = document.createRange();
                            newRange.setStartAfter(pill);
                            newRange.collapse(true);
                            sel.removeAllRanges();
                            sel.addRange(newRange);
                        }}
                    }}
                    saveSelection();
                }});
                input.addEventListener('keypress', (e) => e.stopPropagation());
                input.addEventListener('input', (e) => {{
                    e.stopPropagation();
                    sanitizeInputText();
                    saveSelection();
                    syncElementsFromPills();
                    updatePlaceholder();
                    adjustInputHeight();
                }});

                submitBtn.addEventListener('click', (e) => {{
                    e.preventDefault();
                    e.stopPropagation();
                    doSubmit();
                }});

                function onPointerMove(e) {{
                    if (!active) return;
                    if (isMouseDown) {{
                        let dx = e.clientX - startX;
                        let dy = e.clientY - startY;
                        if (!isDragging && (Math.abs(dx) > 4 || Math.abs(dy) > 4)) {{
                            isDragging = true;
                            overlay.style.display = 'none';
                            badge.style.display = 'none';
                        }}
                        if (isDragging) {{
                            let l = Math.min(startX, e.clientX);
                            let t = Math.min(startY, e.clientY);
                            let w = Math.abs(e.clientX - startX);
                            let h = Math.abs(e.clientY - startY);
                            dragOverlay.style.left = l + 'px';
                            dragOverlay.style.top = t + 'px';
                            dragOverlay.style.width = w + 'px';
                            dragOverlay.style.height = h + 'px';
                            dragOverlay.style.display = 'block';
                            return;
                        }}
                    }}
                    if (isDragging) return;
                    if (rafId) cancelAnimationFrame(rafId);
                    rafId = requestAnimationFrame(() => {{
                        let target = document.elementFromPoint(e.clientX, e.clientY);
                        if (!target || target === overlay || target === badge || target === dragOverlay || popup.contains(target) || target === document.documentElement || target === document.body || (target.className && typeof target.className === 'string' && target.className.includes('__zeron_'))) {{
                            overlay.style.display = 'none';
                            badge.style.display = 'none';
                            hoveredEl = null;
                            return;
                        }}
                        hoveredEl = target;
                        let rect = target.getBoundingClientRect();
                        overlay.style.left = rect.left + 'px';
                        overlay.style.top = rect.top + 'px';
                        overlay.style.width = rect.width + 'px';
                        overlay.style.height = rect.height + 'px';
                        overlay.style.display = 'block';

                        let tag = target.tagName.toLowerCase();
                        let tagColor = (currentTheme && currentTheme.palette && currentTheme.palette[0]) ? currentTheme.palette[0].text : '#60a5fa';
                        let isMulti = selectedElements.length > 0;
                        let hint = isMulti ? 'Click to add element' : 'Click to select, drag to draw';
                        badge.innerHTML = (tag ? '<span style=\"font-weight:600;font-family:monospace;color:' + tagColor + ';margin-right:6px;\">' + tag + '</span>' : '') + hint;
                        let badgeTop = rect.bottom + 6;
                        if (badgeTop + 30 > window.innerHeight) badgeTop = Math.max(4, rect.top - 28);
                        let maxLeft = Math.max(4, window.innerWidth - 300);
                        badge.style.left = Math.min(Math.max(4, rect.left), maxLeft) + 'px';
                        badge.style.top = badgeTop + 'px';
                        badge.style.display = 'block';
                    }});
                }}

                function onPointerDown(e) {{
                    if (!active || e.button !== 0) return;
                    if (popup && popup.contains(e.target)) return;
                    isMouseDown = true;
                    startX = e.clientX;
                    startY = e.clientY;
                    isDragging = false;
                }}

                function onPointerUp(e) {{
                    if (!active) return;
                    if (popup && popup.contains(e.target)) return;
                    if (isDragging) {{
                        isDragging = false;
                        isMouseDown = false;
                        dragOverlay.style.display = 'none';
                        let l = Math.min(startX, e.clientX);
                        let t = Math.min(startY, e.clientY);
                        let w = Math.abs(e.clientX - startX);
                        let h = Math.abs(e.clientY - startY);
                        if (w >= 10 && h >= 10) {{
                            let bounds = 'top=' + Math.round(t) + ' left=' + Math.round(l) + ' width=' + Math.round(w) + ' height=' + Math.round(h);
                            addElement(
                                null,
                                {{ left: l, top: t, width: w, height: h, bottom: t + h, right: l + w }},
                                {{ tag: 'area', id: '', classes: '', selector: 'area', text: '', domPath: 'area', bounds, attributes: [] }}
                            );
                        }}
                        return;
                    }}
                    isMouseDown = false;
                }}

                function onClick(e) {{
                    if (!active) return;
                    if (popup && popup.contains(e.target)) return;
                    if (isDragging) return;
                    e.preventDefault();
                    e.stopPropagation();

                    if (hoveredEl) {{
                        let el = hoveredEl;
                        let existing = selectedElements.find(s => s.el === el);
                        if (existing) {{
                            removeElement(existing);
                            return;
                        }}
                        let rect = el.getBoundingClientRect();
                        let tag = el.tagName.toLowerCase();
                        let id = el.id || '';
                        let classes = (typeof el.className === 'string' ? el.className : (el.className && el.className.baseVal) || '').trim();
                        let text = (el.innerText || el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 200);
                        let selector = getSelector(el);
                        let domPath = getDomPath(el);
                        let bounds = 'top=' + Math.round(rect.top) + ' left=' + Math.round(rect.left) + ' width=' + Math.round(rect.width) + ' height=' + Math.round(rect.height);
                        let attributes = [];
                        if (el.attributes) {{
                            for (let i = 0; i < el.attributes.length; i++) {{
                                let attr = el.attributes[i];
                                if (attr.name.startsWith('__zeron_')) continue;
                                let val = attr.value;
                                if (val.length > 200) val = val.slice(0, 197) + '...';
                                attributes.push(attr.name + '=' + val);
                            }}
                        }}
                        addElement(
                            el,
                            {{ left: rect.left, top: rect.top, width: rect.width, height: rect.height, bottom: rect.bottom, right: rect.right }},
                            {{ tag, id, classes, selector, text, domPath, bounds, attributes }}
                        );
                    }}
                }}

                function onScroll() {{
                    if (active) {{
                        if (window.scrollX !== lockedScrollX || window.scrollY !== lockedScrollY) {{
                            window.scrollTo(lockedScrollX, lockedScrollY);
                            return;
                        }}
                    }}
                    if (active && hoveredEl && selectedElements.length === 0) {{
                        let rect = hoveredEl.getBoundingClientRect();
                        overlay.style.left = rect.left + 'px';
                        overlay.style.top = rect.top + 'px';
                        overlay.style.width = rect.width + 'px';
                        overlay.style.height = rect.height + 'px';
                        let badgeTop = rect.bottom + 6;
                        if (badgeTop + 30 > window.innerHeight) badgeTop = Math.max(4, rect.top - 28);
                        let maxLeft = Math.max(4, window.innerWidth - 300);
                        badge.style.left = Math.min(Math.max(4, rect.left), maxLeft) + 'px';
                        badge.style.top = badgeTop + 'px';
                    }}
                    for (let item of selectedElements) {{
                        if (item.el) {{
                            let r = item.el.getBoundingClientRect();
                            item.rect = {{ left: r.left, top: r.top, width: r.width, height: r.height, bottom: r.bottom, right: r.right }};
                            item.overlayEl.style.left = r.left + 'px';
                            item.overlayEl.style.top = r.top + 'px';
                            item.overlayEl.style.width = r.width + 'px';
                            item.overlayEl.style.height = r.height + 'px';
                            positionBadge(item.badgeEl, item.rect);
                        }}
                    }}
                    if (selectedElements.length > 0) {{
                        positionPopup(selectedElements[0].rect);
                    }}
                }}

                function onWheel(e) {{
                    if (!active) return;
                    if (input && input.contains(e.target)) return;
                    e.preventDefault();
                }}

                function onTouchMove(e) {{
                    if (!active) return;
                    if (input && input.contains(e.target)) return;
                    e.preventDefault();
                }}

                const SCROLL_KEYS = ['Space', 'PageUp', 'PageDown', 'End', 'Home', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'];
                window.addEventListener('keydown', (e) => {{
                    if (!active) return;
                    if (e.key === 'Escape') {{
                        clearSelection();
                        return;
                    }}
                    if (popup && popup.contains(e.target)) return;
                    if (SCROLL_KEYS.includes(e.code) || SCROLL_KEYS.includes(e.key)) {{
                        e.preventDefault();
                    }}
                }}, {{ capture: true }});

                window.__zeron_toggle_design_mode = function(enable) {{
                    active = enable;
                    if (active) {{
                        lockScroll();
                    }} else {{
                        if (rafId) cancelAnimationFrame(rafId);
                        overlay.style.display = 'none';
                        badge.style.display = 'none';
                        if (dragOverlay) dragOverlay.style.display = 'none';
                        hoveredEl = null;
                        isMouseDown = false;
                        isDragging = false;
                        clearSelection();
                    }}
                }};

                window.__zeron_clear_design_selection = clearSelection;

                window.__zeron_apply_design_theme = function(t) {{
                    if (!t) return;
                    currentTheme = t;
                    let styleTag = document.getElementById('__zeron_design_theme_styles__');
                    if (!styleTag) {{
                        styleTag = document.createElement('style');
                        styleTag.id = '__zeron_design_theme_styles__';
                        document.head.appendChild(styleTag);
                    }}
                    styleTag.textContent = '#__zeron_design_input__:empty::before, #__zeron_design_input__[data-empty="true"]::before {{ content: attr(placeholder); color: ' + t.text_muted + ' !important; pointer-events: none; display: inline-block; opacity: 1 !important; }} #__zeron_design_input__::-webkit-scrollbar {{ width: 4px; }} #__zeron_design_input__::-webkit-scrollbar-thumb {{ background: rgba(255,255,255,0.2); border-radius: 2px; }}';
                    let pop = document.getElementById('__zeron_design_popup__');
                    if (pop) {{
                        pop.style.background = t.bg;
                        pop.style.backdropFilter = t.backdrop_filter;
                        pop.style.webkitBackdropFilter = t.backdrop_filter;
                        pop.style.border = '1px solid ' + t.border;
                        pop.style.boxShadow = t.box_shadow;

                        let inp = document.getElementById('__zeron_design_input__');
                        if (inp) {{
                            inp.style.color = t.text;
                        }}
                        let btn = pop.querySelector('.__zeron_submit_btn__');
                        if (btn) {{
                            btn.style.background = t.submit_bg;
                            btn.style.color = t.submit_color;
                        }}
                    }}
                    for (let i = 0; i < selectedElements.length; i++) {{
                        let it = selectedElements[i];
                        let col = getColor(i);
                        if (it.overlayEl) {{
                            it.overlayEl.style.border = '2px solid ' + col.border;
                            it.overlayEl.style.boxShadow = col.shadow;
                        }}
                        if (it.badgeEl) {{
                            it.badgeEl.style.background = col.bg;
                            it.badgeEl.style.color = col.text;
                            it.badgeEl.style.border = '1px solid ' + col.border;
                            if (t.backdrop_filter && t.backdrop_filter !== 'none') {{
                                it.badgeEl.style.webkitBackdropFilter = t.backdrop_filter;
                                it.badgeEl.style.backdropFilter = t.backdrop_filter;
                            }}
                        }}
                        if (it.pillEl) {{
                            it.pillEl.style.background = col.bg;
                            it.pillEl.style.color = col.text;
                            it.pillEl.style.border = '1px solid ' + col.border;
                        }}
                    }}
                    let bdg = document.getElementById('__zeron_design_badge__');
                    if (bdg) {{
                        bdg.style.background = t.bg;
                        bdg.style.backdropFilter = t.backdrop_filter;
                        bdg.style.webkitBackdropFilter = t.backdrop_filter;
                        bdg.style.border = '1px solid ' + t.border;
                        bdg.style.color = t.text_muted;
                        bdg.style.boxShadow = t.box_shadow;
                    }}
                }};
                window.__zeron_apply_design_theme({theme_json});

                window.addEventListener('wheel', onWheel, {{ capture: true, passive: false }});
                window.addEventListener('touchmove', onTouchMove, {{ capture: true, passive: false }});
                window.addEventListener('pointermove', onPointerMove, {{ capture: true, passive: false }});
                window.addEventListener('pointerdown', onPointerDown, {{ capture: true, passive: false }});
                window.addEventListener('pointerup', onPointerUp, {{ capture: true, passive: false }});
                window.addEventListener('click', onClick, {{ capture: true }});
                window.addEventListener('scroll', onScroll, {{ capture: true, passive: true }});
                window.addEventListener('resize', onScroll, {{ capture: true, passive: true }});
            }})();"#
        );
        let completion = block2::RcBlock::new(|_: *mut AnyObject, _: *mut NSError| {});
        unsafe {
            host.view.evaluateJavaScript_completionHandler(
                &NSString::from_str(&script),
                Some(&completion),
            );
        }
    }

    pub fn is_focused(&self) -> bool {
        has_focus(&self.0.borrow().view)
    }

    pub fn clear_selection(&self) {
        let host = self.0.borrow();
        let script = "(() => { if (window.__zeron_clear_design_selection) { window.__zeron_clear_design_selection(); } })();";
        let completion = block2::RcBlock::new(|_: *mut AnyObject, _: *mut NSError| {});
        unsafe {
            host.view.evaluateJavaScript_completionHandler(
                &NSString::from_str(script),
                Some(&completion),
            );
        }
    }

    pub fn evaluate_with_result(
        &self,
        script: &str,
        callback: impl FnOnce(Result<String, String>) + 'static,
    ) {
        let host = self.0.borrow();
        let callback = std::cell::Cell::new(Some(callback));
        let completion = block2::RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            if let Some(cb) = callback.take() {
                if !error.is_null() {
                    let desc = unsafe { (*error).localizedDescription().to_string() };
                    cb(Err(desc));
                } else if value.is_null() {
                    cb(Ok(String::new()));
                } else {
                    let s: String = unsafe {
                        if let Some(ns_str) = value.as_ref().and_then(|v| v.downcast_ref::<NSString>()) {
                            ns_str.to_string()
                        } else {
                            let desc: Retained<NSString> = msg_send![value, description];
                            desc.to_string()
                        }
                    };
                    cb(Ok(s));
                }
            }
        });
        unsafe {
            host.view.evaluateJavaScript_completionHandler(
                &NSString::from_str(script),
                Some(&completion),
            );
        }
    }

    pub fn snapshot(
        &self,
        rect: Option<objc2_foundation::NSRect>,
        callback: impl FnOnce(Option<Vec<u8>>) + 'static,
    ) {
        let host = self.0.borrow();
        capture_snapshot(&host.view, rect, callback);
    }
}

fn has_focus(view: &NSView) -> bool {
    view.window()
        .and_then(|w| w.firstResponder())
        .and_then(|r| r.downcast::<NSView>().ok())
        .is_some_and(|v| v.isDescendantOf(view))
}

impl Host {
    pub fn sync(
        &mut self,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        dragging: bool,
        resize_inset: Pixels,
    ) {
        // WebKit may fill newly exposed tiles a frame after a viewport change.
        // Match its page background beneath those tiles instead of exposing
        // the application's dark window background at the resize edge.
        unsafe {
            let color = self.view.underPageBackgroundColor();
            let unchanged = self
                .background_color
                .as_ref()
                .is_some_and(|previous| msg_send![&**previous, isEqual: &*color]);
            if !unchanged {
                // WebKit's native backing otherwise uses controlBackgroundColor,
                // independently of the CSS page background. Exposed resize tiles
                // must use the page color while the remote content catches up.
                let _: () = msg_send![&*self.view, _setBackgroundColor: &*color];
                let cg_color = color.CGColor();
                let layer: *mut AnyObject = msg_send![&*self.clip, layer];
                let _: () = msg_send![layer, setBackgroundColor: &*cg_color];
                self.background_color = Some(color);
            }
        }
        let visible = bounds.intersect(&mask);
        self.clip.ivars().dragging.set(dragging);
        self.clip
            .ivars()
            .resize_inset
            .set(f64::from(f32::from(resize_inset)));
        if self.bounds != Some(bounds) || self.visible_bounds != Some(visible) {
            self.bounds = Some(bounds);
            self.visible_bounds = Some(visible);
            let x = f64::from(f32::from(visible.origin.x));
            let y = f64::from(f32::from(visible.origin.y));
            let width = f64::from(f32::from(visible.size.width)).max(0.);
            let height = f64::from(f32::from(visible.size.height)).max(0.);
            let y = if self.parent.isFlipped() {
                y
            } else {
                self.parent.bounds().size.height - y - height
            };
            let actions_disabled: bool = unsafe {
                let previous = msg_send![class!(CATransaction), disableActions];
                let _: () = msg_send![class!(CATransaction), setDisableActions: true];
                previous
            };
            // Keep the host stationary. Moving it while WebKit updates its
            // remote layer tree can combine the old origin with the new size.
            self.clip.setFrame(self.parent.bounds());
            let region = objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(x, y),
                objc2_foundation::NSSize::new(width, height),
            );
            self.clip.ivars().region.set(region);
            unsafe {
                let _: () = msg_send![&*self.clip_mask, setFrame: region];
            }
            // Wry's set_bounds rounds logical origins and sizes to whole points. GPUI
            // uses fractional points, so that leaves uncovered background
            // strips between the WKWebView and its clip after a resize.
            let web_height = f64::from(f32::from(bounds.size.height)).max(0.);
            let web_y = f64::from(f32::from(bounds.origin.y));
            let web_y = if self.parent.isFlipped() {
                web_y
            } else {
                self.parent.bounds().size.height - web_y - web_height
            };
            self.view.setFrame(objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(f64::from(f32::from(bounds.origin.x)), web_y),
                objc2_foundation::NSSize::new(
                    f64::from(f32::from(bounds.size.width)).max(0.),
                    web_height,
                ),
            ));
            unsafe {
                // Leave the implicit transaction open for the matching Metal
                // presentation. Committing here would expose native geometry early.
                let _: () = msg_send![class!(CATransaction), setDisableActions: actions_disabled];
            }
            #[cfg(feature = "browser-fixture")]
            unsafe {
                let layer: *mut AnyObject = msg_send![&*self.clip, layer];
                let frame: objc2_foundation::NSRect = msg_send![layer, frame];
                let presentation: *mut AnyObject = msg_send![layer, presentationLayer];
                let presented: objc2_foundation::NSRect = if presentation.is_null() {
                    frame
                } else {
                    msg_send![presentation, frame]
                };
                eprintln!(
                    "Browser geometry: bounds={bounds:?} mask={mask:?} clip={:?} layer={frame:?} presented={presented:?} web={:?} background={:?}",
                    self.clip.frame(),
                    self.view.frame(),
                    self.view.underPageBackgroundColor()
                );
            }
        }
        self.update_visibility();
    }
    fn update_visibility(&self) {
        let visible = self.presentation != Presentation::Hidden
            && self.observer.ivars().error.borrow().is_none()
            && self
                .visible_bounds
                .is_some_and(|b| f32::from(b.size.width) > 0. && f32::from(b.size.height) > 0.);
        if (!visible || self.presentation == Presentation::Passthrough) && has_focus(&self.view) {
            let _ = self.web.focus_parent();
        }
        if self.view.isHidden() == visible {
            #[cfg(feature = "browser-fixture")]
            self.visibility_changes
                .set(self.visibility_changes.get() + 1);
            let _ = self.web.set_visible(visible);
        }
        self.clip.setHidden(!visible);
    }
    fn present(&mut self, presentation: Presentation) {
        self.presentation = presentation;
        self.clip
            .ivars()
            .dragging
            .set(presentation == Presentation::Passthrough);
        self.update_visibility();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if has_focus(&self.view) {
            let _ = self.web.focus_parent();
        }
        unsafe {
            if let Some(monitor) = self.monitor.take() {
                NSEvent::removeMonitor(&monitor);
            }
            for key in OBSERVED {
                self.view
                    .removeObserver_forKeyPath(&self.observer, &NSString::from_str(key));
            }
            self.view.setNavigationDelegate(None);
            self.view.stopLoading();
        }
        self.view.removeFromSuperview();
        self.clip.removeFromSuperview();
    }
}

#[cfg(feature = "browser-fixture")]
impl NativePage {
    pub fn fixture_move_cursor(&self, x: f64, y: f64) {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGWarpMouseCursorPosition(point: objc2_foundation::NSPoint) -> i32;
        }
        let host = self.0.borrow();
        let window = host.view.window().unwrap();
        let height = host.parent.clone().bounds().size.height;
        let p = window.convertPointToScreen(objc2_foundation::NSPoint::new(x, height - y));
        let screen = objc2_app_kit::NSScreen::mainScreen(host.view.mtm()).unwrap();
        unsafe {
            CGWarpMouseCursorPosition(objc2_foundation::NSPoint::new(
                p.x,
                screen.frame().size.height - p.y,
            ));
        }
    }
    pub fn fixture_backdrop_layers(&self) -> String {
        unsafe fn describe(layer: *mut AnyObject, depth: usize, out: &mut String) {
            if layer.is_null() || depth > 8 {
                return;
            }
            unsafe {
                let desc: Retained<NSString> = msg_send![layer, description];
                out.push_str(&format!("{}{}\n", " ".repeat(depth), desc));
                let layers: *mut AnyObject = msg_send![layer, sublayers];
                if !layers.is_null() {
                    let count: usize = msg_send![layers, count];
                    for i in 0..count.min(30) {
                        let child: *mut AnyObject = msg_send![layers, objectAtIndex:i];
                        describe(child, depth + 1, out);
                    }
                }
            }
        }
        let host = self.0.borrow();
        let mut out = String::new();
        for view in host.parent.subviews() {
            if view.class().name() == c"GPUIBackdropView" {
                unsafe {
                    let layer: *mut AnyObject = msg_send![&*view, layer];
                    describe(layer, 0, &mut out);
                }
            }
        }
        out
    }
    pub fn fixture_page_hit(&self, x: f64, y: f64) -> bool {
        let host = self.0.borrow();
        let point = objc2_foundation::NSPoint::new(x, host.parent.bounds().size.height - y);
        host.parent
            .hitTest(point)
            .is_some_and(|hit| hit.isDescendantOf(&host.clip))
    }
    pub fn fixture_backdrops(&self) -> Vec<(f64, f64, f64, f64)> {
        let host = self.0.borrow();
        host.parent
            .subviews()
            .iter()
            .filter(|view| view.class().name() == c"GPUIBackdropView" && !view.isHidden())
            .map(|view| {
                let r = view.frame();
                (
                    r.origin.x,
                    host.parent.bounds().size.height - r.origin.y - r.size.height,
                    r.size.width,
                    r.size.height,
                )
            })
            .collect()
    }
    pub fn fixture_geometry(&self) -> (f32, f32, f32) {
        let host = self.0.borrow();
        (
            host.bounds.unwrap().size.width.into(),
            host.view.frame().size.width as f32,
            host.visible_bounds.unwrap().size.width.into(),
        )
    }
    pub fn fixture_origin(&self) -> (f32, f32) {
        let b = self.0.borrow().bounds.unwrap();
        (b.origin.x.into(), b.origin.y.into())
    }
    pub fn fixture_overlay_visible(&self) -> bool {
        let host = self.0.borrow();
        host.parent
            .clone()
            .subviews()
            .iter()
            .any(|v| v.class().name() == c"GPUIOverlayView" && !v.isHidden())
    }
    pub fn fixture_visibility_changes(&self) -> u64 {
        let host = self.0.borrow();
        host.visibility_changes.get()
    }
    pub fn fixture_focus(&self) {
        let _ = self.0.borrow().web.focus();
    }
    pub fn fixture_focused(&self) -> bool {
        has_focus(&self.0.borrow().view)
    }
    pub fn fixture_overlay_at(&self, x: f64, y: f64) -> bool {
        let host = self.0.borrow();
        let parent = host.parent.clone();
        let point = objc2_foundation::NSPoint::new(
            x,
            if parent.isFlipped() {
                y
            } else {
                parent.bounds().size.height - y
            },
        );
        parent
            .hitTest(point)
            .is_some_and(|hit| hit.class().name() == c"GPUIOverlayView")
    }
    pub fn fixture_click(&self, x: f64, y: f64) {
        self.fixture_move_cursor(x, y);
        let host = self.0.borrow();
        let parent = host.parent.clone();
        let window = host.view.window().unwrap();
        let point = objc2_foundation::NSPoint::new(x, parent.bounds().size.height - y);
        let app = objc2_app_kit::NSApplication::sharedApplication(host.view.mtm());
        for kind in [
            objc2_app_kit::NSEventType::LeftMouseDown,
            objc2_app_kit::NSEventType::LeftMouseUp,
        ] {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(kind, point, NSEventModifierFlags::empty(), 0., window.windowNumber(), None, 0, 1, 1.).unwrap();
            app.postEvent_atStart(&event, false);
        }
    }
    pub fn fixture_visible(&self) -> bool {
        !self.0.borrow().view.isHidden()
    }
    pub fn fixture_eval(&self, script: &str) {
        unsafe {
            self.0
                .borrow()
                .view
                .evaluateJavaScript_completionHandler(&NSString::from_str(script), None);
        }
    }
}
