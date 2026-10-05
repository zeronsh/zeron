//! Windows boundary for the browser: WebView2 in visual-hosting mode. Each
//! page is a DirectComposition visual mounted in the zui overlay plane's
//! native layer, so DWM composites it between GPUI's base and overlay planes
//! without copies or child windows. GPUI keeps every pointer event and
//! forwards only those that reach the page; the page owns the keyboard while
//! focused. Callbacks enqueue events and never re-enter GPUI.
use super::model::{PageState, Presentation, allowed_navigation};
use gpui::{Bounds, CursorStyle, Keystroke, Modifiers, MouseButton, Pixels, Point, Window};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CreateCoreWebView2CompositionControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, CursorChangedEventHandler,
    DocumentTitleChangedEventHandler, DownloadStartingEventHandler, ExecuteScriptCompletedHandler,
    FaviconChangedEventHandler, HistoryChangedEventHandler, Microsoft::Web::WebView2::Win32::*,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, ProcessFailedEventHandler, SourceChangedEventHandler,
    take_pwstr,
};
use windows::{
    Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::{Direct2D::Common::D2D_RECT_F, DirectComposition::*},
        UI::Input::KeyboardAndMouse::{
            GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        },
    },
    core::{HSTRING, Interface, PWSTR},
};

type Sender = tokio::sync::mpsc::Sender<NativeEvent>;

pub(super) enum NativeEvent {
    Changed,
    Finished,
    NewTab(String),
    Key(Keystroke),
    Favicon {
        page: String,
        url: String,
    },
    Cursor(CursorStyle),
    /// A script's string result (`None` for any other value or a failure).
    Evaluated(Option<String>),
}

// ---------------------------------------------------------------------------
// Shared environment: one browser process for a window's tabs
// ---------------------------------------------------------------------------

type EnvironmentWaiter = Box<dyn FnOnce(Result<ICoreWebView2Environment, String>)>;

#[derive(Default)]
enum Environment {
    #[default]
    Idle,
    Pending(Vec<EnvironmentWaiter>),
    Ready(ICoreWebView2Environment),
    Failed(String),
}

#[derive(Default)]
struct EnvironmentState {
    environment: Environment,
    /// Set once the shared folder answered ERROR_BUSY (see [`is_busy`]).
    own_folder: bool,
}

/// A window/profile's WebView2 environment, created on first use: one
/// browser process for all of its tabs. Pages are InPrivate, so nothing
/// outlives the session (as macOS's non-persistent store); the user-data
/// folder only holds the runtime's own caches.
#[derive(Clone, Default)]
pub(super) struct BrowserData(Rc<RefCell<EnvironmentState>>);

impl BrowserData {
    fn with_environment(
        &self,
        done: impl FnOnce(Result<ICoreWebView2Environment, String>) + 'static,
    ) {
        let mut state = self.0.borrow_mut();
        match &mut state.environment {
            Environment::Ready(environment) => {
                let environment = environment.clone();
                drop(state);
                done(Ok(environment));
            }
            Environment::Failed(error) => {
                let error = error.clone();
                drop(state);
                done(Err(error));
            }
            Environment::Pending(waiters) => waiters.push(Box::new(done)),
            Environment::Idle => {
                state.environment = Environment::Pending(vec![Box::new(done)]);
                drop(state);
                create_environment(Rc::downgrade(&self.0));
            }
        }
    }

    /// Move this process to its own folder after the shared one was busy.
    /// Returns false when that already happened (no further retries).
    fn fall_back(&self) -> bool {
        let mut state = self.0.borrow_mut();
        if state.own_folder {
            return false;
        }
        state.own_folder = true;
        if !matches!(state.environment, Environment::Pending(_)) {
            state.environment = Environment::Idle;
        }
        true
    }
}

/// A folder still held by another process (a second Zeron, or one shutting
/// down after a quick restart) answers ERROR_BUSY at any creation step.
fn is_busy(error: &windows::core::Error) -> bool {
    error.code() == windows::core::HRESULT(0x8007_00AA_u32 as i32)
}

/// The runtime's caches are shared across launches for a warm start; a
/// busy shared folder sends this process to its own.
fn user_data_folder(own: bool) -> std::path::PathBuf {
    let temp = std::env::temp_dir();
    if !own {
        return temp.join("zeron-webview2");
    }
    // Sweep folders of exited processes. A live runtime locks its files,
    // so removing an in-use folder simply fails.
    let pid = std::process::id();
    let sweep = temp.clone();
    std::thread::spawn(move || {
        for entry in std::fs::read_dir(&sweep).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("zeron-webview2-") && name != format!("zeron-webview2-{pid}") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    });
    temp.join(format!("zeron-webview2-{pid}"))
}

fn create_environment(shared: std::rc::Weak<RefCell<EnvironmentState>>) {
    let Some(state) = shared.upgrade() else {
        return;
    };
    let own = state.borrow().own_folder;
    drop(state);
    let finish = |shared: &std::rc::Weak<RefCell<EnvironmentState>>,
                  outcome: Result<ICoreWebView2Environment, String>| {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        let next = match &outcome {
            Ok(environment) => Environment::Ready(environment.clone()),
            Err(error) => Environment::Failed(error.clone()),
        };
        let waiters = match std::mem::replace(&mut shared.borrow_mut().environment, next) {
            Environment::Pending(waiters) => waiters,
            _ => Vec::new(),
        };
        for waiter in waiters {
            waiter(outcome.clone());
        }
    };
    let retry = |shared: std::rc::Weak<RefCell<EnvironmentState>>| {
        if let Some(state) = shared.upgrade() {
            state.borrow_mut().own_folder = true;
        }
        create_environment(shared);
    };
    let callback = shared.clone();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            match result.and_then(|()| environment.ok_or_else(windows::core::Error::empty)) {
                Ok(environment) => finish(&callback, Ok(environment)),
                Err(error) if !own && is_busy(&error) => retry(callback),
                Err(error) => finish(&callback, Err(runtime_error(&error))),
            }
            Ok(())
        },
    ));
    let created = unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            None,
            &HSTRING::from(user_data_folder(own).to_string_lossy().as_ref()),
            None::<&ICoreWebView2EnvironmentOptions>,
            &handler,
        )
    };
    match created {
        Ok(()) => {}
        Err(error) if !own && is_busy(&error) => retry(shared),
        Err(error) => finish(&shared, Err(runtime_error(&error))),
    }
}

fn runtime_error(error: &windows::core::Error) -> String {
    tracing::warn!(%error, "WebView2 unavailable");
    "The Microsoft Edge WebView2 Runtime is required to show pages here.".into()
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/// COM objects of a created page.
struct Page {
    composition: ICoreWebView2CompositionController,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    /// The page's root visual and the layer it mounts in, from `generation`.
    visual: Option<(IDCompositionVisual, IDCompositionVisual, u64)>,
    mounted: bool,
    /// What was last handed to WebView2 and the visual, so an unchanged
    /// frame makes no calls at all.
    applied: Option<Applied>,
    shown: Option<bool>,
}

#[derive(Clone, Copy, PartialEq)]
struct Applied {
    origin: (f32, f32),
    size: (i32, i32),
    clip: [f32; 4],
    scale: f32,
}

#[derive(Default)]
struct Geometry {
    /// Page bounds and their visible part, in physical pixels.
    origin: (f32, f32),
    size: (i32, i32),
    clip: D2D_RECT_F,
    scale: f32,
}

pub(super) struct Host {
    hwnd: HWND,
    tx: Sender,
    page: RefCell<Option<Page>>,
    pending_url: RefCell<Option<String>>,
    requested_url: RefCell<Option<String>>,
    error: RefCell<Option<String>>,
    loading: Cell<bool>,
    changed_pending: Cell<bool>,
    presentation: Cell<Presentation>,
    geometry: RefCell<Geometry>,
    has_area: Cell<bool>,
    /// Pointer buttons pressed inside the page (forwarded until released).
    pressed: Cell<u32>,
    hovered: Cell<bool>,
    shortcuts: RefCell<Vec<Keystroke>>,
}

pub(super) struct NativePage(Rc<Host>);

impl NativePage {
    pub fn new(window: &Window, data: &BrowserData, tx: Sender) -> Result<Self, String> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let hwnd = match HasWindowHandle::window_handle(window)
            .map_err(|error| error.to_string())?
            .as_raw()
        {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut _),
            _ => return Err("Browser requires a Win32 window".into()),
        };
        window
            .enable_scene_overlay()
            .map_err(|error| error.to_string())?;
        let host = Rc::new(Host {
            hwnd,
            tx,
            page: RefCell::new(None),
            pending_url: RefCell::new(None),
            requested_url: RefCell::new(None),
            error: RefCell::new(None),
            loading: Cell::new(true),
            changed_pending: Cell::new(false),
            presentation: Cell::new(Presentation::Hidden),
            geometry: RefCell::new(Geometry::default()),
            has_area: Cell::new(false),
            pressed: Cell::new(0),
            hovered: Cell::new(false),
            shortcuts: RefCell::new(Vec::new()),
        });
        Host::start(&host, data.clone());
        Ok(Self(host))
    }

    pub fn handle(&self) -> Rc<Host> {
        self.0.clone()
    }

    pub fn set_shortcuts(&self, shortcuts: Vec<String>) {
        *self.0.shortcuts.borrow_mut() = shortcuts
            .iter()
            .filter_map(|s| Keystroke::parse(s).ok())
            .collect();
    }

    pub fn present(&mut self, presentation: Presentation) {
        self.0.presentation.set(presentation);
        if presentation != Presentation::Live {
            self.0.leave();
        }
        self.0.update_visibility();
    }

    pub fn load(&self, url: &str) -> Result<(), String> {
        self.0.error.borrow_mut().take();
        *self.0.requested_url.borrow_mut() = Some(url.into());
        self.0.loading.set(true);
        match &*self.0.page.borrow() {
            Some(page) => unsafe { page.webview.Navigate(&HSTRING::from(url)) }
                .map_err(|error| error.message()),
            None => {
                *self.0.pending_url.borrow_mut() = Some(url.into());
                Ok(())
            }
        }
    }

    pub fn reload(&self) {
        self.0.error.borrow_mut().take();
        if let Some(page) = &*self.0.page.borrow() {
            // Retrying the requested URL also works after a failed load.
            let failed_url = self.0.requested_url.borrow().clone();
            unsafe {
                let _ = match failed_url {
                    Some(url) if self.0.page_url().is_none() => {
                        page.webview.Navigate(&HSTRING::from(url))
                    }
                    _ => page.webview.Reload(),
                };
            }
        }
        self.0.update_visibility();
        self.0.changed();
    }

    pub fn history(&self, forward: bool) {
        if let Some(page) = &*self.0.page.borrow() {
            unsafe {
                let _ = if forward {
                    page.webview.GoForward()
                } else {
                    page.webview.GoBack()
                };
            }
        }
    }

    pub fn state(&self) -> PageState {
        let host = &self.0;
        host.changed_pending.set(false);
        let (title, can_back, can_forward) = match &*host.page.borrow() {
            Some(page) => unsafe {
                let mut title = PWSTR::null();
                let mut back = windows::core::BOOL::default();
                let mut forward = windows::core::BOOL::default();
                let _ = page.webview.DocumentTitle(&mut title);
                let _ = page.webview.CanGoBack(&mut back);
                let _ = page.webview.CanGoForward(&mut forward);
                (take_pwstr(title), back.as_bool(), forward.as_bool())
            },
            None => (String::new(), false, false),
        };
        PageState {
            url: host
                .requested_url
                .borrow()
                .clone()
                .or_else(|| host.page_url()),
            title,
            loading: host.loading.get(),
            can_back,
            can_forward,
            error: host.error.borrow().clone(),
        }
    }

    pub fn discover_favicon(&self, page_url: String) {
        if let Some(page) = &*self.0.page.borrow()
            && let Ok(webview) = page.webview.cast::<ICoreWebView2_15>()
        {
            let mut uri = PWSTR::null();
            if unsafe { webview.FaviconUri(&mut uri) }.is_ok() {
                let url = take_pwstr(uri);
                if !url.is_empty() {
                    let _ = self.0.tx.try_send(NativeEvent::Favicon {
                        page: page_url,
                        url,
                    });
                }
            }
        }
    }

    /// Run `script` in the main frame; a string result arrives as
    /// [`NativeEvent::Evaluated`].
    pub fn evaluate(&self, script: &str) {
        let Some(page) = &*self.0.page.borrow() else {
            let _ = self.0.tx.try_send(NativeEvent::Evaluated(None));
            return;
        };
        let tx = self.0.tx.clone();
        let handler = ExecuteScriptCompletedHandler::create(Box::new(move |result, json| {
            // WebView2 returns the value JSON-encoded.
            let value = result
                .ok()
                .and_then(|()| serde_json::from_str::<serde_json::Value>(&json).ok())
                .and_then(|value| value.as_str().map(str::to_owned));
            let _ = tx.try_send(NativeEvent::Evaluated(value));
            Ok(())
        }));
        unsafe {
            let _ = page.webview.ExecuteScript(&HSTRING::from(script), &handler);
        }
    }

    /// Return the keyboard to GPUI chrome (the address bar).
    pub fn focus_chrome(&self) {
        use windows::Win32::UI::{
            Input::KeyboardAndMouse::{GetFocus, SetFocus},
            WindowsAndMessaging::IsChild,
        };
        unsafe {
            let focus = GetFocus();
            if !focus.is_invalid() && focus != self.0.hwnd && IsChild(self.0.hwnd, focus).as_bool()
            {
                let _ = SetFocus(Some(self.0.hwnd));
            }
        }
    }

    pub fn focus_page(&self) {
        if let Some(page) = &*self.0.page.borrow() {
            unsafe {
                let _ = page
                    .controller
                    .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
    }

    /// A press, release or move at `position` (window coordinates).
    pub fn pointer(
        &self,
        kind: PointerKind,
        position: Point<Pixels>,
        button: Option<MouseButton>,
        modifiers: Modifiers,
        click_count: usize,
    ) {
        self.0
            .pointer(kind, position, button, modifiers, click_count);
    }

    pub fn wheel(&self, position: Point<Pixels>, delta: Point<f32>, modifiers: Modifiers) {
        self.0.wheel(position, delta, modifiers);
    }

    pub fn leave(&self) {
        self.0.leave();
    }

    pub fn pressed(&self) -> bool {
        self.0.pressed.get() != 0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PointerKind {
    Down,
    Up,
    Move,
}

impl Host {
    /// Create the page in `data`'s environment. A busy folder at any step
    /// moves the window to its own folder and retries once.
    fn start(host: &Rc<Self>, data: BrowserData) {
        let weak = Rc::downgrade(host);
        data.clone().with_environment(move |environment| {
            let Some(host) = weak.upgrade() else { return };
            match environment {
                Ok(environment) => {
                    if let Err(error) = Host::create_controller(&host, &environment, data) {
                        host.fail(&runtime_error(&error));
                    }
                }
                Err(error) => host.fail(&error),
            }
        });
    }

    fn create_controller(
        host: &Rc<Self>,
        environment: &ICoreWebView2Environment,
        data: BrowserData,
    ) -> windows::core::Result<()> {
        let weak = Rc::downgrade(host);
        let handler = CreateCoreWebView2CompositionControllerCompletedHandler::create(Box::new(
            move |result, composition| {
                let Some(host) = weak.upgrade() else {
                    return Ok(());
                };
                match result.and_then(|()| composition.ok_or_else(windows::core::Error::empty)) {
                    Ok(composition) => {
                        if let Err(error) = Host::attach(&host, composition) {
                            host.fail(&runtime_error(&error));
                        }
                    }
                    Err(error) if is_busy(&error) && data.fall_back() => Host::start(&host, data),
                    Err(error) => host.fail(&runtime_error(&error)),
                }
                Ok(())
            },
        ));
        unsafe {
            match environment.cast::<ICoreWebView2Environment10>() {
                Ok(environment) => {
                    let options = environment.CreateCoreWebView2ControllerOptions()?;
                    options.SetIsInPrivateModeEnabled(true)?;
                    environment.CreateCoreWebView2CompositionControllerWithOptions(
                        host.hwnd, &options, &handler,
                    )
                }
                Err(_) => environment
                    .cast::<ICoreWebView2Environment3>()?
                    .CreateCoreWebView2CompositionController(host.hwnd, &handler),
            }
        }
    }

    fn attach(
        host: &Rc<Self>,
        composition: ICoreWebView2CompositionController,
    ) -> windows::core::Result<()> {
        let controller: ICoreWebView2Controller = composition.cast()?;
        let webview = unsafe { controller.CoreWebView2()? };
        unsafe {
            let settings = webview.Settings()?;
            settings.SetAreDevToolsEnabled(false)?;
            settings.SetIsStatusBarEnabled(false)?;
            settings.SetIsZoomControlEnabled(true)?;
            if let Ok(controller) = controller.cast::<ICoreWebView2Controller3>() {
                // GPUI owns DPI: it re-syncs the scale with the bounds.
                controller.SetShouldDetectMonitorScaleChanges(false)?;
            }
            controller.SetIsVisible(false)?;
            let mut token = 0i64;
            let weak = Rc::downgrade(host);
            webview.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    if let (Some(host), Some(args)) = (weak.upgrade(), args) {
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        if allowed_navigation(&uri) {
                            *host.requested_url.borrow_mut() = Some(uri);
                            host.error.borrow_mut().take();
                            host.loading.set(true);
                            host.changed();
                        } else {
                            args.SetCancel(true)?;
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let weak = Rc::downgrade(host);
            webview.add_SourceChanged(
                &SourceChangedEventHandler::create(Box::new(move |_, _| {
                    if let Some(host) = weak.upgrade() {
                        host.requested_url.borrow_mut().take();
                        host.changed();
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let weak = Rc::downgrade(host);
            webview.add_HistoryChanged(
                &HistoryChangedEventHandler::create(Box::new(move |_, _| {
                    if let Some(host) = weak.upgrade() {
                        host.changed();
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let weak = Rc::downgrade(host);
            webview.add_DocumentTitleChanged(
                &DocumentTitleChangedEventHandler::create(Box::new(move |_, _| {
                    if let Some(host) = weak.upgrade() {
                        host.changed();
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let weak = Rc::downgrade(host);
            webview.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                    let Some(host) = weak.upgrade() else {
                        return Ok(());
                    };
                    host.loading.set(false);
                    if let Some(args) = args {
                        let mut success = windows::core::BOOL::default();
                        args.IsSuccess(&mut success)?;
                        if !success.as_bool() {
                            let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                            args.WebErrorStatus(&mut status)?;
                            if let Some(message) = navigation_error(status) {
                                tracing::warn!(status = status.0, "browser navigation failed");
                                *host.error.borrow_mut() = Some(message.into());
                                host.update_visibility();
                            }
                        }
                    }
                    host.changed();
                    let _ = host.tx.try_send(NativeEvent::Finished);
                    Ok(())
                })),
                &mut token,
            )?;
            let weak = Rc::downgrade(host);
            webview.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                    if let (Some(host), Some(args)) = (weak.upgrade(), args) {
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        let uri = take_pwstr(uri);
                        if allowed_navigation(&uri) {
                            let _ = host.tx.try_send(NativeEvent::NewTab(uri));
                        }
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let weak = Rc::downgrade(host);
            webview.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                    if let Some(host) = weak.upgrade() {
                        host.fail("The page stopped responding. Reload to continue.");
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            if let Ok(webview4) = webview.cast::<ICoreWebView2_4>() {
                let weak = Rc::downgrade(host);
                webview4.add_DownloadStarting(
                    &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                        if let Some(args) = args {
                            args.SetCancel(true)?;
                        }
                        if let Some(host) = weak.upgrade() {
                            host.fail(
                                "This file can’t be previewed here. Open it in your default browser.",
                            );
                        }
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            if let Ok(webview15) = webview.cast::<ICoreWebView2_15>() {
                let weak = Rc::downgrade(host);
                webview15.add_FaviconChanged(
                    &FaviconChangedEventHandler::create(Box::new(move |sender, _| {
                        let (Some(host), Some(sender)) = (weak.upgrade(), sender) else {
                            return Ok(());
                        };
                        let mut uri = PWSTR::null();
                        let mut source = PWSTR::null();
                        if let Ok(sender) = sender.cast::<ICoreWebView2_15>() {
                            sender.FaviconUri(&mut uri)?;
                        }
                        sender.Source(&mut source)?;
                        let (url, page) = (take_pwstr(uri), take_pwstr(source));
                        if !url.is_empty() {
                            let _ = host.tx.try_send(NativeEvent::Favicon { page, url });
                        }
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            let weak = Rc::downgrade(host);
            composition.add_CursorChanged(
                &CursorChangedEventHandler::create(Box::new(move |sender, _| {
                    if let (Some(host), Some(sender)) = (weak.upgrade(), sender) {
                        let mut id = 0u32;
                        sender.SystemCursorId(&mut id)?;
                        let _ = host.tx.try_send(NativeEvent::Cursor(cursor_style(id)));
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let weak = Rc::downgrade(host);
            controller.add_AcceleratorKeyPressed(
                &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                    let (Some(host), Some(args)) = (weak.upgrade(), args) else {
                        return Ok(());
                    };
                    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                    args.KeyEventKind(&mut kind)?;
                    if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                        && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                    {
                        return Ok(());
                    }
                    let mut key = 0u32;
                    args.VirtualKey(&mut key)?;
                    if let Some(keystroke) = accelerator(key)
                        && host.claims(&keystroke)
                        && host.tx.try_send(NativeEvent::Key(keystroke)).is_ok()
                    {
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        *host.page.borrow_mut() = Some(Page {
            composition,
            controller,
            webview,
            visual: None,
            mounted: false,
            applied: None,
            shown: None,
        });
        host.apply_geometry();
        host.update_visibility();
        if let Some(url) = host.pending_url.borrow_mut().take()
            && let Some(page) = &*host.page.borrow()
        {
            unsafe {
                let _ = page.webview.Navigate(&HSTRING::from(url));
            }
        }
        host.changed();
        Ok(())
    }

    fn page_url(&self) -> Option<String> {
        let page = self.page.borrow();
        let page = page.as_ref()?;
        let mut source = PWSTR::null();
        unsafe { page.webview.Source(&mut source) }.ok()?;
        Some(take_pwstr(source)).filter(|url| !url.is_empty() && url != "about:blank")
    }

    fn changed(&self) {
        if !self.changed_pending.replace(true) && self.tx.try_send(NativeEvent::Changed).is_err() {
            self.changed_pending.set(false);
        }
    }

    fn fail(&self, message: &str) {
        *self.error.borrow_mut() = Some(message.into());
        self.loading.set(false);
        self.update_visibility();
        self.changed();
    }

    /// Browser and app shortcuts go to GPUI even while the page has focus.
    fn claims(&self, keystroke: &Keystroke) -> bool {
        let browser = [
            "ctrl-l", "ctrl-t", "ctrl-w", "ctrl-[", "ctrl-]", "ctrl-k", "ctrl-,",
        ]
        .iter()
        .any(|combo| Keystroke::parse(combo).is_ok_and(|k| &k == keystroke));
        browser || self.shortcuts.borrow().iter().any(|k| k == keystroke)
    }

    /// Mount, position and clip the page for a painted frame. Runs from the
    /// frame's presentation callback; the renderer commits it with the frame.
    pub fn sync(
        &self,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        scale: f32,
        composition: Option<gpui::NativeComposition>,
    ) {
        let visible = bounds.intersect(&mask);
        {
            let mut geometry = self.geometry.borrow_mut();
            let x = f32::from(bounds.origin.x) * scale;
            let y = f32::from(bounds.origin.y) * scale;
            geometry.origin = (x.round(), y.round());
            geometry.size = (
                (f32::from(bounds.size.width) * scale).round().max(0.) as i32,
                (f32::from(bounds.size.height) * scale).round().max(0.) as i32,
            );
            geometry.clip = D2D_RECT_F {
                left: (f32::from(visible.origin.x) * scale - x).max(0.),
                top: (f32::from(visible.origin.y) * scale - y).max(0.),
                right: (f32::from(visible.origin.x + visible.size.width) * scale - x).max(0.),
                bottom: (f32::from(visible.origin.y + visible.size.height) * scale - y).max(0.),
            };
            geometry.scale = scale;
        }
        self.has_area
            .set(f32::from(visible.size.width) > 0. && f32::from(visible.size.height) > 0.);
        if let Some(composition) = composition {
            self.remount(composition);
        }
        self.apply_geometry();
        self.update_visibility();
    }

    /// (Re)create the page visual when the renderer's device generation
    /// changes (first frame, or GPU recovery).
    fn remount(&self, composition: gpui::NativeComposition) {
        let mut page = self.page.borrow_mut();
        let Some(page) = page.as_mut() else { return };
        if page
            .visual
            .as_ref()
            .is_some_and(|(_, _, generation)| *generation == composition.generation)
        {
            return;
        }
        let mounted = unsafe {
            let (Some(device), Some(layer)) = (
                IDCompositionDevice::from_raw_borrowed(&composition.device),
                IDCompositionVisual::from_raw_borrowed(&composition.layer),
            ) else {
                return;
            };
            device.CreateVisual().and_then(|visual| {
                page.composition.SetRootVisualTarget(&visual)?;
                Ok((visual, layer.clone()))
            })
        };
        match mounted {
            Ok((visual, layer)) => {
                page.visual = Some((visual, layer, composition.generation));
                page.mounted = false;
                page.applied = None;
            }
            Err(error) => tracing::warn!(%error, "browser visual unavailable"),
        }
    }

    fn apply_geometry(&self) {
        let mut page = self.page.borrow_mut();
        let Some(page) = page.as_mut() else { return };
        let geometry = self.geometry.borrow();
        let next = Applied {
            origin: geometry.origin,
            size: geometry.size,
            clip: [
                geometry.clip.left,
                geometry.clip.top,
                geometry.clip.right,
                geometry.clip.bottom,
            ],
            scale: geometry.scale,
        };
        let previous = page.applied.replace(next);
        if previous == Some(next) {
            return;
        }
        unsafe {
            if let Some((visual, _, _)) = &page.visual {
                if previous.is_none_or(|p| p.origin != next.origin) {
                    let _ = visual.SetOffsetX2(next.origin.0);
                    let _ = visual.SetOffsetY2(next.origin.1);
                }
                if previous.is_none_or(|p| p.clip != next.clip) {
                    let _ = visual.SetClip2(&geometry.clip);
                }
            }
            if previous.is_none_or(|p| p.size != next.size) {
                let _ = page.controller.SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: next.size.0,
                    bottom: next.size.1,
                });
            }
            if next.scale > 0.
                && previous.is_none_or(|p| p.scale != next.scale)
                && let Ok(controller) = page.controller.cast::<ICoreWebView2Controller3>()
            {
                let _ = controller.SetRasterizationScale(f64::from(next.scale));
            }
        }
    }

    /// Visible pages render and mount; hidden ones unmount and let
    /// Chromium throttle them (timers, painting) like a background tab.
    fn update_visibility(&self) {
        let visible = self.presentation.get() != Presentation::Hidden
            && self.error.borrow().is_none()
            && self.has_area.get();
        let mut page = self.page.borrow_mut();
        let Some(page) = page.as_mut() else { return };
        unsafe {
            if page.shown.replace(visible) != Some(visible) {
                let _ = page.controller.SetIsVisible(visible);
            }
            if let Some((visual, layer, _)) = &page.visual
                && page.mounted != visible
            {
                let _ = if visible {
                    layer.AddVisual(visual, true, None::<&IDCompositionVisual>)
                } else {
                    layer.RemoveVisual(visual)
                };
                page.mounted = visible;
            }
        }
        if !visible {
            self.pressed.set(0);
        }
    }

    fn local_point(&self, position: Point<Pixels>) -> POINT {
        let geometry = self.geometry.borrow();
        POINT {
            x: (f32::from(position.x) * geometry.scale - geometry.origin.0).round() as i32,
            y: (f32::from(position.y) * geometry.scale - geometry.origin.1).round() as i32,
        }
    }

    fn pointer(
        &self,
        kind: PointerKind,
        position: Point<Pixels>,
        button: Option<MouseButton>,
        modifiers: Modifiers,
        click_count: usize,
    ) {
        let page = self.page.borrow();
        let Some(page) = page.as_ref() else { return };
        let bit = match button {
            Some(MouseButton::Left) => 1,
            Some(MouseButton::Right) => 2,
            Some(MouseButton::Middle) => 4,
            _ => 0,
        };
        let pressed = match kind {
            PointerKind::Down => self.pressed.get() | bit,
            PointerKind::Up => self.pressed.get() & !bit,
            PointerKind::Move => self.pressed.get(),
        };
        let event = match (kind, button) {
            (PointerKind::Move, _) => COREWEBVIEW2_MOUSE_EVENT_KIND_MOVE,
            (PointerKind::Down, Some(MouseButton::Left)) if click_count % 2 == 0 => {
                COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOUBLE_CLICK
            }
            (PointerKind::Down, Some(MouseButton::Left)) => {
                COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_DOWN
            }
            (PointerKind::Up, Some(MouseButton::Left)) => {
                COREWEBVIEW2_MOUSE_EVENT_KIND_LEFT_BUTTON_UP
            }
            (PointerKind::Down, Some(MouseButton::Right)) => {
                COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_DOWN
            }
            (PointerKind::Up, Some(MouseButton::Right)) => {
                COREWEBVIEW2_MOUSE_EVENT_KIND_RIGHT_BUTTON_UP
            }
            (PointerKind::Down, Some(MouseButton::Middle)) => {
                COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_DOWN
            }
            (PointerKind::Up, Some(MouseButton::Middle)) => {
                COREWEBVIEW2_MOUSE_EVENT_KIND_MIDDLE_BUTTON_UP
            }
            _ => return,
        };
        self.pressed.set(pressed);
        self.hovered.set(true);
        unsafe {
            let _ = page.composition.SendMouseInput(
                event,
                virtual_keys(pressed, modifiers),
                0,
                self.local_point(position),
            );
            if kind == PointerKind::Down {
                let _ = page
                    .controller
                    .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
    }

    /// GPUI line deltas use WHEEL_DELTA per three lines; precise (pixel)
    /// deltas map 100 px to one notch, matching Chromium's own scaling.
    fn wheel(&self, position: Point<Pixels>, delta: Point<f32>, modifiers: Modifiers) {
        let page = self.page.borrow();
        let Some(page) = page.as_ref() else { return };
        let point = self.local_point(position);
        let keys = virtual_keys(self.pressed.get(), modifiers);
        unsafe {
            if delta.y != 0. {
                let _ = page.composition.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL,
                    keys,
                    (delta.y.round() as i32) as u32,
                    point,
                );
            }
            if delta.x != 0. {
                let _ = page.composition.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_HORIZONTAL_WHEEL,
                    keys,
                    (-(delta.x.round()) as i32) as u32,
                    point,
                );
            }
        }
    }

    fn leave(&self) {
        if !self.hovered.replace(false) {
            return;
        }
        if let Some(page) = &*self.page.borrow() {
            unsafe {
                let _ = page.composition.SendMouseInput(
                    COREWEBVIEW2_MOUSE_EVENT_KIND_LEAVE,
                    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE,
                    0,
                    POINT::default(),
                );
            }
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(page) = self.page.borrow_mut().take() {
            unsafe {
                if let Some((visual, layer, _)) = &page.visual
                    && page.mounted
                {
                    let _ = layer.RemoveVisual(visual);
                }
                let _ = page.controller.Close();
            }
        }
    }
}

/// Wheel units: GPUI lines (3 per notch) or pixels (100 per notch).
pub(super) fn wheel_units(delta: gpui::ScrollDelta) -> Point<f32> {
    const WHEEL_DELTA: f32 = 120.;
    match delta {
        gpui::ScrollDelta::Lines(lines) => {
            gpui::point(lines.x * WHEEL_DELTA / 3., lines.y * WHEEL_DELTA / 3.)
        }
        gpui::ScrollDelta::Pixels(pixels) => gpui::point(
            f32::from(pixels.x) * WHEEL_DELTA / 100.,
            f32::from(pixels.y) * WHEEL_DELTA / 100.,
        ),
    }
}

fn virtual_keys(pressed: u32, modifiers: Modifiers) -> COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS {
    let mut keys = COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_NONE.0;
    if pressed & 1 != 0 {
        keys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON.0;
    }
    if pressed & 2 != 0 {
        keys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_RIGHT_BUTTON.0;
    }
    if pressed & 4 != 0 {
        keys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_MIDDLE_BUTTON.0;
    }
    if modifiers.shift {
        keys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_SHIFT.0;
    }
    if modifiers.control {
        keys |= COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_CONTROL.0;
    }
    COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS(keys)
}

/// User-facing copy for a failed main-frame navigation; cancellations
/// (a newer navigation, a blocked scheme) are silent.
fn navigation_error(status: COREWEBVIEW2_WEB_ERROR_STATUS) -> Option<&'static str> {
    match status {
        COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED => None,
        COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED
        | COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET
        | COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED => {
            Some("The connection was interrupted. Try loading this page again.")
        }
        _ => Some("Check the address and make sure your server is running, then try again."),
    }
}

/// Windows system cursor ids (`IDC_*`) as GPUI cursor styles.
fn cursor_style(id: u32) -> CursorStyle {
    match id {
        32513 => CursorStyle::IBeam,
        32515 => CursorStyle::Crosshair,
        32642 => CursorStyle::ResizeUpLeftDownRight,
        32643 => CursorStyle::ResizeUpRightDownLeft,
        32644 => CursorStyle::ResizeLeftRight,
        32645 => CursorStyle::ResizeUpDown,
        32648 => CursorStyle::OperationNotAllowed,
        32649 => CursorStyle::PointingHand,
        _ => CursorStyle::Arrow,
    }
}

/// The GPUI keystroke for a key-down the page received, when it is one an
/// app shortcut could use (a modifier other than Shift, or a function key).
fn accelerator(key: u32) -> Option<Keystroke> {
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(vk.0)) } < 0;
    let control = down(VK_CONTROL);
    let alt = down(VK_MENU);
    let platform = down(VK_LWIN) || down(VK_RWIN);
    let shift = down(VK_SHIFT);
    let name = match key {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(key)?.to_ascii_lowercase().to_string(),
        0x70..=0x7B => format!("f{}", key - 0x6F),
        0x09 => "tab".into(),
        0xBC => ",".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xDB => "[".into(),
        0xDD => "]".into(),
        0xBA => ";".into(),
        0xDE => "'".into(),
        0xC0 => "`".into(),
        0xBD => "-".into(),
        0xBB => "=".into(),
        0xDC => "\\".into(),
        _ => return None,
    };
    if !(control || alt || platform || matches!(key, 0x70..=0x7B)) {
        return None;
    }
    let mut combo = String::new();
    for (held, prefix) in [
        (control, "ctrl-"),
        (alt, "alt-"),
        (platform, "win-"),
        (shift, "shift-"),
    ] {
        if held {
            combo.push_str(prefix);
        }
    }
    combo.push_str(&name);
    Keystroke::parse(&combo).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_units_match_win32_notches() {
        let lines = wheel_units(gpui::ScrollDelta::Lines(gpui::point(0., 3.)));
        assert_eq!(lines.y, 120.);
        let pixels = wheel_units(gpui::ScrollDelta::Pixels(gpui::point(
            gpui::px(-50.),
            gpui::px(100.),
        )));
        assert_eq!((pixels.x, pixels.y), (-60., 120.));
    }

    #[test]
    fn cursors_and_errors_map_to_gpui_and_copy() {
        assert_eq!(cursor_style(32649), CursorStyle::PointingHand);
        assert_eq!(cursor_style(32513), CursorStyle::IBeam);
        assert_eq!(cursor_style(1), CursorStyle::Arrow);
        assert!(navigation_error(COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED).is_none());
        assert!(
            navigation_error(COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT)
                .unwrap()
                .contains("server")
        );
    }

    #[test]
    fn virtual_keys_carry_buttons_and_modifiers() {
        let keys = virtual_keys(
            1 | 4,
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
        assert_eq!(
            keys.0,
            COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_LEFT_BUTTON.0
                | COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_MIDDLE_BUTTON.0
                | COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS_SHIFT.0
        );
    }
}
