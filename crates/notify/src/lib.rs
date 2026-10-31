//! Desktop banner notifications, posted by whichever process owns the engine
//! — the app's background engine (`zeron-engine`) or, when the app embeds its
//! engine, the app itself. Platform facilities, failures swallowed:
//!
//! - macOS: `NSUserNotification` through the ObjC runtime, so the banner is
//!   attributed to the app with its icon. The API is deprecated (10.14) but
//!   shipping and fits banners that need no actions/attachments; a delegate
//!   answers "present" unconditionally, overriding the center's own
//!   suppress-while-frontmost policy — WHETHER to ping (including the
//!   background-only setting's focus check) is decided by the caller.
//!   Posting hops to the main queue, so any thread may post; the process
//!   must service the main queue (AppKit's loop, or the engine's
//!   `CFRunLoopRun`). A process without a bundle identifier of its own (the
//!   engine binary, `cargo run`) adopts the app's — [`set_app_bundle_id`],
//!   the terminal-notifier technique: override `-[NSBundle
//!   bundleIdentifier]` for the main bundle — so its banners carry the app's
//!   name and icon. Machines without the app installed fall back to
//!   `osascript`, attributed to Script Editor (cosmetics only).
//! - Linux: `notify-send` (libnotify's CLI, present on every mainstream
//!   desktop).
//! - Windows: no-op for now — toasts require a registered AppUserModelID
//!   (an installer concern); the app's chime still covers it.
//! - `ZERON_DISABLE_NOTIFICATIONS` env kill-switch (callers also check the
//!   `notificationsEnabled` setting);
//! - failures are logged and swallowed — a missing notifier must never
//!   bother the session flow.

// objc 0.2's `msg_send!` checks a `cargo-clippy` feature this crate lacks.
#![allow(unexpected_cfgs)]

use std::sync::Mutex;

const DISABLE_ENV: &str = "ZERON_DISABLE_NOTIFICATIONS";

/// Reserved notification target routed to Settings → Agents rather than a
/// conversation. Session ids are generated UUIDs, so this cannot collide
/// with a real chat.
pub const AGENT_UPDATES_TARGET: &str = "__zeron_agent_updates__";

/// The app-side link a clicked banner opens when the engine posted it from
/// its own process: `zeron://notification/<target>`.
pub const LINK_PREFIX: &str = "zeron://notification/";

/// `zeron://notification/<target>` for a banner's target.
pub fn link(target: &str) -> String {
    format!("{LINK_PREFIX}{target}")
}

/// The target a [`link`] carries, or `None` for any other URL.
pub fn link_target(url: &str) -> Option<&str> {
    url.strip_prefix(LINK_PREFIX)
        .filter(|target| !target.is_empty())
}

type ClickHandler = Box<dyn Fn(String) + Send>;

static CLICK: Mutex<Option<ClickHandler>> = Mutex::new(None);

/// Post a desktop banner, optionally linked to `target` (a chat id, or
/// [`AGENT_UPDATES_TARGET`]). Any thread. Silently a no-op when disabled or
/// no notifier is available.
pub fn post(title: &str, body: &str, target: Option<&str>) {
    if std::env::var_os(DISABLE_ENV).is_some() {
        return;
    }
    post_impl(title, body, target);
}

/// Route banner clicks: `handler` receives the clicked banner's target.
/// Replaces any previous handler. Only the native macOS path reports clicks
/// (osascript and notify-send banners can't).
pub fn on_click(handler: impl Fn(String) + Send + 'static) {
    *CLICK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(handler));
    #[cfg(target_os = "macos")]
    on_main(|| unsafe { macos::install_delegate() });
}

/// Only the macOS delegate reports clicks.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn clicked(target: String) {
    if let Some(handler) = CLICK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        handler(target);
    }
}

/// The bundle identifier banners are attributed to when this process has
/// none of its own: the app's (`sh.zeron.app`, `sh.zeron.app.dev` for the
/// development bundle). Call before the first [`post`].
pub fn set_app_bundle_id(id: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = macos::APP_BUNDLE_ID.set(id.replace('\0', ""));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = id;
}

/// The bundle identifier in an app bundle's `Contents/Info.plist`, for the
/// engine binary that ships inside it.
pub fn bundle_id_of(app: &std::path::Path) -> Option<String> {
    let plist = std::fs::read_to_string(app.join("Contents").join("Info.plist")).ok()?;
    let key = plist.find("<key>CFBundleIdentifier</key>")?;
    let rest = &plist[key..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_owned()).filter(|id| !id.is_empty())
}

/// Run `work` on the main thread: inline when already there, otherwise
/// queued on the main dispatch queue.
#[cfg(target_os = "macos")]
fn on_main(work: impl FnOnce() + Send + 'static) {
    use std::ffi::c_void;
    unsafe extern "C" {
        static _dispatch_main_q: c_void;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
        fn pthread_main_np() -> i32;
    }
    extern "C" fn trampoline(context: *mut c_void) {
        let work = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce() + Send>) };
        work();
    }
    if unsafe { pthread_main_np() } != 0 {
        work();
        return;
    }
    let boxed: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(work));
    unsafe {
        dispatch_async_f(
            &raw const _dispatch_main_q,
            Box::into_raw(boxed) as *mut c_void,
            trampoline,
        );
    }
}

#[cfg(target_os = "macos")]
fn post_impl(title: &str, body: &str, target: Option<&str>) {
    let (title, body, target) = (title.to_owned(), body.to_owned(), target.map(str::to_owned));
    on_main(move || {
        if unsafe { macos::post_user_notification(&title, &body, target.as_deref()) } {
            return;
        }
        tracing::warn!("no notification center for this process; posting through osascript");
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            applescript_escape(&body),
            applescript_escape(&title),
        );
        std::thread::spawn(move || {
            let result = std::process::Command::new("osascript")
                .args(["-e", &script])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            match result {
                Ok(status) if status.success() => {}
                Ok(status) => tracing::debug!(?status, "osascript notification failed"),
                Err(err) => tracing::debug!(error = %err, "osascript unavailable"),
            }
        });
    });
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CStr, CString, c_char};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use objc::declare::ClassDecl;
    use objc::runtime::{BOOL, Class, Imp, Method, Object, Sel, YES};
    use objc::{class, msg_send, sel, sel_impl};

    /// NSUserNotification lives in Foundation. It's loaded on first use,
    /// not linked: a process that loads Foundation (let alone AppKit)
    /// without running a main run loop gets slow, fragile system networking
    /// — everything linking the engine (its tests, servers) would pay for
    /// banners only the desktop engine posts.
    pub(super) fn load_foundation() -> bool {
        static LOADED: OnceLock<bool> = OnceLock::new();
        *LOADED.get_or_init(|| {
            unsafe extern "C" {
                fn dlopen(path: *const c_char, mode: i32) -> *mut std::ffi::c_void;
            }
            const RTLD_LAZY: i32 = 0x1;
            const RTLD_GLOBAL: i32 = 0x8;
            let path = c"/System/Library/Frameworks/Foundation.framework/Foundation";
            !unsafe { dlopen(path.as_ptr(), RTLD_LAZY | RTLD_GLOBAL) }.is_null()
        })
    }

    /// The production app's id: the default identity to adopt.
    const DEFAULT_BUNDLE_ID: &str = "sh.zeron.app";

    pub(super) static APP_BUNDLE_ID: OnceLock<String> = OnceLock::new();

    /// `userInfo` key carrying the banner's target back to the click handler.
    const TARGET_KEY: &CStr = c"chatId";

    fn app_bundle_id() -> CString {
        let id = APP_BUNDLE_ID
            .get()
            .map(String::as_str)
            .unwrap_or(DEFAULT_BUNDLE_ID);
        CString::new(id).unwrap_or_else(|_| CString::new(DEFAULT_BUNDLE_ID).unwrap())
    }

    unsafe fn ns_string(text: &CStr) -> *mut Object {
        unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
    }

    /// The notification center, adopting the app's identity when this
    /// process has none (or a different one than the app it belongs to).
    unsafe fn center() -> *mut Object {
        if !load_foundation() {
            return std::ptr::null_mut();
        }
        // Defensive lookup (not `class!`, which panics if the class is ever
        // dropped from Foundation) — the deprecated API's one real removal risk.
        let Some(center_class) = Class::get("NSUserNotificationCenter") else {
            return std::ptr::null_mut();
        };
        unsafe {
            if APP_BUNDLE_ID.get().is_some() {
                identity::adopt();
            }
            let mut center: *mut Object = msg_send![center_class, defaultUserNotificationCenter];
            if center.is_null() && identity::adopt() {
                center = msg_send![center_class, defaultUserNotificationCenter];
            }
            center
        }
    }

    /// Main thread only. Idempotent: the delegate is a leaked singleton (the
    /// center holds it weakly).
    pub(super) unsafe fn install_delegate() {
        unsafe {
            let center = center();
            if !center.is_null() {
                let _: () = msg_send![center, setDelegate: delegate()];
            }
        }
    }

    /// Deliver through the notification center; false when there is none
    /// (no bundle and the app's identity can't be adopted). Main thread.
    pub(super) unsafe fn post_user_notification(
        title: &str,
        body: &str,
        target: Option<&str>,
    ) -> bool {
        let (Ok(title), Ok(body), Ok(target)) = (
            CString::new(title.replace('\0', "")),
            CString::new(body.replace('\0', "")),
            target.map(CString::new).transpose(),
        ) else {
            return false;
        };
        unsafe {
            let center = center();
            if center.is_null() {
                return false;
            }
            let _: () = msg_send![center, setDelegate: delegate()];
            let note: *mut Object = msg_send![class!(NSUserNotification), new];
            let _: () = msg_send![note, setTitle: ns_string(&title)];
            let _: () = msg_send![note, setInformativeText: ns_string(&body)];
            if let Some(target) = target {
                tag_target(note, &target);
            }
            let _: () = msg_send![center, deliverNotification: note];
            let _: () = msg_send![note, release];
        }
        true
    }

    /// Stamp `note` with the target [`did_activate`] reads back on click.
    pub(super) unsafe fn tag_target(note: *mut Object, target: &CStr) {
        unsafe {
            let info: *mut Object = msg_send![
                class!(NSDictionary),
                dictionaryWithObject: ns_string(target)
                forKey: ns_string(TARGET_KEY)
            ];
            let _: () = msg_send![note, setUserInfo: info];
        }
    }

    extern "C" fn should_present(
        _this: &Object,
        _sel: Sel,
        _center: *mut Object,
        _notification: *mut Object,
    ) -> BOOL {
        YES
    }

    /// AppKit calls this on the main thread when a banner is clicked.
    extern "C" fn did_activate(
        _this: &Object,
        _sel: Sel,
        _center: *mut Object,
        notification: *mut Object,
    ) {
        let target = unsafe {
            let info: *mut Object = msg_send![notification, userInfo];
            if info.is_null() {
                return;
            }
            let value: *mut Object = msg_send![info, objectForKey: ns_string(TARGET_KEY)];
            if value.is_null() {
                return;
            }
            let utf8: *const c_char = msg_send![value, UTF8String];
            if utf8.is_null() {
                return;
            }
            CStr::from_ptr(utf8).to_string_lossy().into_owned()
        };
        super::clicked(target);
    }

    /// The leaked delegate singleton (as usize — raw pointers aren't `Sync`).
    /// Leaked deliberately: the center's `delegate` property is weak.
    pub(super) fn delegate() -> *mut Object {
        static DELEGATE: OnceLock<usize> = OnceLock::new();
        *DELEGATE.get_or_init(|| unsafe {
            let mut decl = ClassDecl::new("ZeronNotifyDelegate", class!(NSObject))
                .expect("ZeronNotifyDelegate registered twice");
            decl.add_method(
                sel!(userNotificationCenter:shouldPresentNotification:),
                should_present as extern "C" fn(&Object, Sel, *mut Object, *mut Object) -> BOOL,
            );
            decl.add_method(
                sel!(userNotificationCenter:didActivateNotification:),
                did_activate as extern "C" fn(&Object, Sel, *mut Object, *mut Object),
            );
            let class = decl.register();
            let instance: *mut Object = msg_send![class, new];
            instance as usize
        }) as *mut Object
    }

    /// Bundle-identity adoption — the terminal-notifier /
    /// mac-notification-sys technique. The notification center refuses
    /// processes whose main bundle has no identifier and resolves each
    /// banner's name + icon from the identifier at delivery, so overriding
    /// `-[NSBundle bundleIdentifier]` to answer the app's id for the MAIN
    /// bundle (other bundles keep the original implementation) makes the
    /// engine's banners look exactly like the app's.
    mod identity {
        use super::*;

        unsafe extern "C" {
            fn class_getInstanceMethod(cls: *const Class, name: Sel) -> *mut Method;
            fn method_getImplementation(method: *const Method) -> Imp;
            fn method_setImplementation(method: *mut Method, imp: Imp) -> Imp;
        }

        /// The pre-override `bundleIdentifier` IMP (as usize — fn pointers
        /// have no atomic type), forwarded to for every bundle but the main one.
        static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

        extern "C" fn bundle_identifier_override(this: &Object, sel: Sel) -> *mut Object {
            unsafe {
                let main: *mut Object = msg_send![class!(NSBundle), mainBundle];
                if std::ptr::eq(this, main) {
                    return ns_string(&app_bundle_id());
                }
                match ORIGINAL.load(Ordering::Relaxed) {
                    0 => std::ptr::null_mut(),
                    imp => {
                        let original: extern "C" fn(&Object, Sel) -> *mut Object =
                            std::mem::transmute(imp);
                        original(this, sel)
                    }
                }
            }
        }

        /// Install the override once; false when the identity can't be
        /// adopted (no app with that id is installed).
        pub(super) fn adopt() -> bool {
            static ADOPTED: OnceLock<bool> = OnceLock::new();
            *ADOPTED.get_or_init(|| unsafe {
                // Adoption only works if the system can resolve the id to an
                // installed app. Where AppKit is loaded (the app), ask; the
                // engine adopts only an id it was given — its own app's.
                match Class::get("NSWorkspace") {
                    Some(workspace_class) => {
                        let workspace: *mut Object = msg_send![workspace_class, sharedWorkspace];
                        let url: *mut Object = msg_send![
                            workspace,
                            URLForApplicationWithBundleIdentifier: ns_string(&app_bundle_id())
                        ];
                        if url.is_null() {
                            return false;
                        }
                    }
                    None if APP_BUNDLE_ID.get().is_some() => {}
                    None => return false,
                }
                let method = class_getInstanceMethod(class!(NSBundle), sel!(bundleIdentifier));
                if method.is_null() {
                    return false;
                }
                ORIGINAL.store(method_getImplementation(method) as usize, Ordering::Relaxed);
                let replacement: extern "C" fn(&Object, Sel) -> *mut Object =
                    bundle_identifier_override;
                method_setImplementation(method, std::mem::transmute::<_, Imp>(replacement));
                true
            })
        }
    }
}

/// Escape a string for interpolation inside a double-quoted AppleScript
/// literal: backslash and double-quote are the metacharacters, and a raw
/// newline ends the statement (which would silently drop the banner) — those
/// flatten to spaces.
#[cfg(any(target_os = "macos", test))]
fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ")
}

#[cfg(target_os = "linux")]
fn post_impl(title: &str, body: &str, _target: Option<&str>) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        // `--` ends option parsing: session titles are model-generated, so a
        // `-`-leading one must land as the summary, not as a flag.
        let result = std::process::Command::new("notify-send")
            .args(["--app-name=Zeron", "--", &title, &body])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match result {
            Ok(status) if status.success() => {}
            Ok(status) => tracing::debug!(?status, "notify-send failed"),
            Err(err) => tracing::debug!(error = %err, "notify-send unavailable"),
        }
    });
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn post_impl(_title: &str, _body: &str, _target: Option<&str>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_escaping() {
        assert_eq!(applescript_escape("plain"), "plain");
        assert_eq!(
            applescript_escape(r#"say "hi" \ bye"#),
            r#"say \"hi\" \\ bye"#
        );
        // Raw newlines would end the AppleScript statement mid-literal.
        assert_eq!(applescript_escape("two\nlines\r\n"), "two lines  ");
    }

    #[test]
    fn links_round_trip_their_target() {
        assert_eq!(link("chat-1"), "zeron://notification/chat-1");
        assert_eq!(link_target(&link("chat-1")), Some("chat-1"));
        assert_eq!(link_target(LINK_PREFIX), None);
        assert_eq!(link_target("zeron://chat/x"), None);
    }

    #[test]
    fn reads_the_bundle_id_from_an_app_plist() {
        let dir = std::env::temp_dir().join(format!("zeron-notify-{}", std::process::id()));
        let contents = dir.join("Zeron Dev.app").join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            "<dict>\n<key>CFBundleName</key>\n<string>Zeron Dev</string>\n\
             <key>CFBundleIdentifier</key>\n    <string>sh.zeron.app.dev</string>\n</dict>",
        )
        .unwrap();
        assert_eq!(
            bundle_id_of(&dir.join("Zeron Dev.app")).as_deref(),
            Some("sh.zeron.app.dev")
        );
        assert_eq!(bundle_id_of(&dir.join("Missing.app")), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clicked_banner_reports_its_target() {
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};
        use std::sync::{Arc, Mutex};

        let clicked = Arc::new(Mutex::new(Vec::new()));
        let sink = clicked.clone();
        *CLICK.lock().unwrap() = Some(Box::new(move |target| sink.lock().unwrap().push(target)));
        assert!(macos::load_foundation());
        unsafe {
            let delegate = macos::delegate();
            let note: *mut Object = msg_send![class!(NSUserNotification), new];
            macos::tag_target(note, c"chat-42");
            let center: *mut Object = std::ptr::null_mut();
            let _: () =
                msg_send![delegate, userNotificationCenter: center didActivateNotification: note];
            // A banner without a target (foreign or legacy) is ignored.
            let bare: *mut Object = msg_send![class!(NSUserNotification), new];
            let _: () =
                msg_send![delegate, userNotificationCenter: center didActivateNotification: bare];
            let _: () = msg_send![note, release];
            let _: () = msg_send![bare, release];
        }
        assert_eq!(*clicked.lock().unwrap(), vec!["chat-42".to_string()]);
    }
}
