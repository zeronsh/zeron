//! Desktop banner notifications — the sound.rs approach applied to banners
//! (platform facilities, zero new Rust deps, failures swallowed):
//!
//! - macOS: `NSUserNotification` through the already-linked ObjC runtime, so
//!   the banner is attributed to Glitch Flow with its icon. The API is deprecated
//!   (10.14) but shipping and fits an app that needs no actions/attachments;
//!   a delegate answers "present" unconditionally, overriding the center's
//!   own suppress-while-frontmost policy — WHETHER to ping (including the
//!   background-only setting's focus check) is decided at the call site, so
//!   every platform path behaves identically. Unbundled dev runs
//!   (`cargo run`) have no notification
//!   center — those adopt the installed Glitch Flow app's bundle identity (the
//!   terminal-notifier technique: override `-[NSBundle bundleIdentifier]` for
//!   the main bundle), so dev banners still show the Glitch Flow icon; a click may
//!   focus the installed app rather than the dev binary — acceptable for a
//!   dev-only path. Machines without Glitch Flow installed fall back to
//!   `osascript`, attributed to Script Editor (cosmetics only).
//! - Linux: `notify-send` (libnotify's CLI, present on every mainstream
//!   desktop).
//! - Windows: no-op for now — toasts require a registered AppUserModelID
//!   (an installer concern); the chime still covers it.
//! - `ZERON_DISABLE_NOTIFICATIONS` env kill-switch + the
//!   `notificationsEnabled` ui-setting (checked by the caller);
//! - failures are logged and swallowed — a missing notifier must never
//!   bother the session flow.

const DISABLE_ENV: &str = "ZERON_DISABLE_NOTIFICATIONS";

/// Post a desktop banner, optionally linked to `chat_id`'s session. Call from the main thread
/// (the macOS native path talks to AppKit); slow paths (spawning a CLI) hop to
/// a background thread. Silently a no-op when disabled or no notifier is
/// available.
pub fn post(title: &str, body: &str, chat_id: Option<&str>) {
    if std::env::var_os(DISABLE_ENV).is_some() {
        return;
    }
    post_impl(title, body, chat_id);
}

/// Route banner clicks: `handler` receives the clicked banner's chat id.
/// Main thread only; replaces any previous handler. Only the native macOS
/// path reports clicks (osascript and notify-send banners can't).
pub fn on_click(handler: impl Fn(String) + 'static) {
    #[cfg(target_os = "macos")]
    delegate::CLICK.with_borrow_mut(|slot| *slot = Some(Box::new(handler)));
    #[cfg(not(target_os = "macos"))]
    drop(handler);
}

#[cfg(target_os = "macos")]
fn post_impl(title: &str, body: &str, chat_id: Option<&str>) {
    if post_user_notification(title, body, chat_id) {
        return;
    }
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        applescript_escape(title),
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
}

/// The identity banners are attributed to — the packaged app's bundle id
/// (`dist/macos/Info.plist`), which the center resolves to its name + icon.
#[cfg(target_os = "macos")]
const MACOS_BUNDLE_ID: &std::ffi::CStr = c"local.glitchflow.app";

/// `userInfo` key carrying the banner's chat id back to the click handler.
#[cfg(target_os = "macos")]
const CHAT_ID_KEY: &std::ffi::CStr = c"chatId";

/// Deliver through the app's notification center; false when the process has
/// no bundle (dev runs — `defaultUserNotificationCenter` is nil there) and
/// the installed-app identity can't be adopted either.
#[cfg(target_os = "macos")]
fn post_user_notification(title: &str, body: &str, chat_id: Option<&str>) -> bool {
    use objc::runtime::{Class, Object};
    use objc::{class, msg_send, sel, sel_impl};
    // Defensive lookup (not `class!`, which panics if the class is ever
    // dropped from Foundation) — the deprecated API's one real removal risk.
    let Some(center_class) = Class::get("NSUserNotificationCenter") else {
        return false;
    };
    let (Ok(title), Ok(body), Ok(chat_id)) = (
        std::ffi::CString::new(title.replace('\0', "")),
        std::ffi::CString::new(body.replace('\0', "")),
        chat_id.map(std::ffi::CString::new).transpose(),
    ) else {
        return false;
    };
    unsafe {
        let mut center: *mut Object = msg_send![center_class, defaultUserNotificationCenter];
        if center.is_null() && identity::adopt_installed() {
            center = msg_send![center_class, defaultUserNotificationCenter];
        }
        if center.is_null() {
            return false;
        }
        // Idempotent: the delegate is a leaked singleton (the center holds it
        // weakly), re-set on every post in case another center appeared.
        let _: () = msg_send![center, setDelegate: delegate::always_present()];
        let note: *mut Object = msg_send![class!(NSUserNotification), new];
        let ns_title: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: title.as_ptr()];
        let _: () = msg_send![note, setTitle: ns_title];
        let ns_body: *mut Object = msg_send![class!(NSString), stringWithUTF8String: body.as_ptr()];
        let _: () = msg_send![note, setInformativeText: ns_body];
        if let Some(chat_id) = chat_id {
            tag_chat(note, &chat_id);
        }
        let _: () = msg_send![center, deliverNotification: note];
        let _: () = msg_send![note, release];
    }
    true
}

/// Stamp `note` with the chat id [`delegate`] reads back on click.
#[cfg(target_os = "macos")]
unsafe fn tag_chat(note: *mut objc::runtime::Object, chat_id: &std::ffi::CStr) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let ns_chat_id: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: chat_id.as_ptr()];
        let key: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: CHAT_ID_KEY.as_ptr()];
        let info: *mut Object =
            msg_send![class!(NSDictionary), dictionaryWithObject: ns_chat_id forKey: key];
        let _: () = msg_send![note, setUserInfo: info];
    }
}

/// The center delegate: `shouldPresentNotification:` → YES, unconditionally.
/// Without it the center silently swallows banners while the app is
/// frontmost — a policy that lived outside our settings; presenting always
/// moves the whole decision into the caller (`shell::on_state_changed`
/// checks `notifications_background_only` + window focus before posting).
/// `didActivateNotification:` hands a clicked banner's chat id to
/// [`super::on_click`]'s handler.
#[cfg(target_os = "macos")]
mod delegate {
    use std::cell::RefCell;
    use std::ffi::{CStr, c_char};
    use std::sync::OnceLock;

    use objc::declare::ClassDecl;
    use objc::runtime::{BOOL, Object, Sel, YES};
    use objc::{class, msg_send, sel, sel_impl};

    thread_local! {
        pub(super) static CLICK: RefCell<Option<Box<dyn Fn(String)>>> = const { RefCell::new(None) };
    }

    extern "C" fn should_present(
        _this: &Object,
        _sel: Sel,
        _center: *mut Object,
        _notification: *mut Object,
    ) -> BOOL {
        YES
    }

    /// AppKit calls this on the main thread, after activating the app.
    extern "C" fn did_activate(
        _this: &Object,
        _sel: Sel,
        _center: *mut Object,
        notification: *mut Object,
    ) {
        let chat_id = unsafe {
            let info: *mut Object = msg_send![notification, userInfo];
            if info.is_null() {
                return;
            }
            let key: *mut Object =
                msg_send![class!(NSString), stringWithUTF8String: super::CHAT_ID_KEY.as_ptr()];
            let value: *mut Object = msg_send![info, objectForKey: key];
            if value.is_null() {
                return;
            }
            let utf8: *const c_char = msg_send![value, UTF8String];
            if utf8.is_null() {
                return;
            }
            CStr::from_ptr(utf8).to_string_lossy().into_owned()
        };
        CLICK.with_borrow(|handler| {
            if let Some(handler) = handler {
                handler(chat_id);
            }
        });
    }

    /// The leaked delegate singleton (as usize — raw pointers aren't `Sync`).
    /// Leaked deliberately: the center's `delegate` property is weak.
    pub(super) fn always_present() -> *mut Object {
        static DELEGATE: OnceLock<usize> = OnceLock::new();
        *DELEGATE.get_or_init(|| unsafe {
            let mut decl = ClassDecl::new("GlitchFlowNotifyDelegate", class!(NSObject))
                .expect("GlitchFlowNotifyDelegate registered twice");
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
}

/// Bundle-identity adoption for unbundled (dev) processes — the
/// terminal-notifier / mac-notification-sys technique. The notification
/// center refuses processes whose main bundle has no identifier and resolves
/// each banner's name + icon from the identifier at delivery, so overriding
/// `-[NSBundle bundleIdentifier]` to answer the installed Glitch Flow app's id for
/// the MAIN bundle (other bundles keep the original implementation) makes
/// dev-run banners look exactly like the packaged app's.
#[cfg(target_os = "macos")]
mod identity {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use objc::runtime::{Class, Imp, Method, Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe extern "C" {
        fn class_getInstanceMethod(cls: *const Class, name: Sel) -> *mut Method;
        fn method_getImplementation(method: *const Method) -> Imp;
        fn method_setImplementation(method: *mut Method, imp: Imp) -> Imp;
    }

    /// The pre-override `bundleIdentifier` IMP (as usize — fn pointers have
    /// no atomic type), forwarded to for every bundle but the main one.
    static ORIGINAL: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn bundle_identifier_override(this: &Object, sel: Sel) -> *mut Object {
        unsafe {
            let main: *mut Object = msg_send![class!(NSBundle), mainBundle];
            if std::ptr::eq(this, main) {
                return msg_send![
                    class!(NSString),
                    stringWithUTF8String: super::MACOS_BUNDLE_ID.as_ptr()
                ];
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

    /// Install the override once; false when the identity can't (or needn't)
    /// be adopted. Only ever called after `defaultUserNotificationCenter`
    /// returned nil, i.e. never in the packaged app.
    pub(super) fn adopt_installed() -> bool {
        static ADOPTED: OnceLock<bool> = OnceLock::new();
        *ADOPTED.get_or_init(|| unsafe {
            // Adoption only works if the system can resolve the id to an
            // installed app — otherwise stay on the osascript fallback.
            let bundle_id: *mut Object = msg_send![
                class!(NSString),
                stringWithUTF8String: super::MACOS_BUNDLE_ID.as_ptr()
            ];
            let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            let url: *mut Object =
                msg_send![workspace, URLForApplicationWithBundleIdentifier: bundle_id];
            if url.is_null() {
                return false;
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
fn post_impl(title: &str, body: &str, _chat_id: Option<&str>) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        // `--` ends option parsing: session titles are model-generated, so a
        // `-`-leading one must land as the summary, not as a flag.
        let result = std::process::Command::new("notify-send")
            .args(["--app-name=Glitch Flow", "--", &title, &body])
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
fn post_impl(_title: &str, _body: &str, _chat_id: Option<&str>) {}

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

    #[cfg(target_os = "macos")]
    #[test]
    fn clicked_banner_reports_its_chat_id() {
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};
        use std::cell::RefCell;
        use std::rc::Rc;

        let clicked = Rc::new(RefCell::new(Vec::new()));
        let sink = clicked.clone();
        on_click(move |chat_id| sink.borrow_mut().push(chat_id));
        unsafe {
            let delegate = delegate::always_present();
            let note: *mut Object = msg_send![class!(NSUserNotification), new];
            tag_chat(note, c"chat-42");
            let center: *mut Object = std::ptr::null_mut();
            let _: () =
                msg_send![delegate, userNotificationCenter: center didActivateNotification: note];
            // A banner without a chat id (foreign or legacy) is ignored.
            let bare: *mut Object = msg_send![class!(NSUserNotification), new];
            let _: () =
                msg_send![delegate, userNotificationCenter: center didActivateNotification: bare];
            let _: () = msg_send![note, release];
            let _: () = msg_send![bare, release];
        }
        assert_eq!(*clicked.borrow(), vec!["chat-42".to_string()]);
    }
}
