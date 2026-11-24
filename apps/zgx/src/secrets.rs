//! A secret on the clipboard (a deploy token, shown once in
//! ui/screens/plugins/keys): marked so the system and other apps keep no
//! copy, and gone from it a minute later if it's still there.
//!
//!   secret.copy { text }   on the clipboard: on macOS as concealed and
//!                          transient (`org.nspasteboard.ConcealedType`,
//!                          `TransientType`: clipboard managers and history
//!                          skip it); on Windows kept out of clipboard
//!                          history, the cloud clipboard and monitors
//!                          (`ExcludeClipboardContentFromMonitorProcessing`,
//!                          `CanIncludeInClipboardHistory` 0,
//!                          `CanUploadToCloudClipboard` 0); elsewhere as
//!                          plain text (Linux's clipboards have no such
//!                          mark). True once it took it; a minute on, the
//!                          clipboard is cleared if it still holds it.

use std::{cell::RefCell, rc::Rc, time::Duration};

use zgx::platform::PlatformService;
use zgx_script::{ScriptHost, Value};

/// How long a secret stays on the clipboard.
pub const FORGET_AFTER: Duration = Duration::from_secs(60);

/// The longest secret taken (a deploy token is 53 characters).
const MOST: usize = 256;

pub fn install(host: &ScriptHost, platform: Rc<RefCell<Option<PlatformService>>>) {
    host.async_action("secret.copy", move |arg| {
        let platform = platform.clone();
        async move {
            let text = arg.field("text").map(crate::auth::text).unwrap_or_default();
            if text.is_empty() || text.len() > MOST {
                return Err("nothing to copy".into());
            }
            let service = platform.borrow().clone();
            if !write(service.as_ref(), &text) {
                return Err("The clipboard didn't take it".into());
            }
            // (Answered now; the clipboard let go of a minute on, if it
            // still holds this and nothing newer.)
            let later = platform.clone();
            crate::engine::get().spawn(async move {
                zgx::timer::sleep(FORGET_AFTER).await;
                let service = later.borrow().clone();
                if still_holds(service.as_ref(), &text) {
                    clear(service.as_ref());
                }
            });
            Ok(Value::Bool(true))
        }
    });
}

fn still_holds(service: Option<&PlatformService>, text: &str) -> bool {
    service.and_then(|s| s.0.clipboard_text()).is_some_and(|now| now == text)
}

#[cfg(target_os = "macos")]
fn write(_service: Option<&PlatformService>, text: &str) -> bool {
    macos::write(text)
}

#[cfg(target_os = "macos")]
fn clear(_service: Option<&PlatformService>) {
    macos::clear();
}

#[cfg(windows)]
fn write(_service: Option<&PlatformService>, text: &str) -> bool {
    windows::write(text)
}

#[cfg(windows)]
fn clear(_service: Option<&PlatformService>) {
    windows::clear();
}

#[cfg(not(any(target_os = "macos", windows)))]
fn write(service: Option<&PlatformService>, text: &str) -> bool {
    service.is_some_and(|s| s.0.set_clipboard_text(text.to_owned()))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn clear(service: Option<&PlatformService>) {
    if let Some(s) = service {
        s.0.set_clipboard_text(String::new());
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc::rc::autoreleasepool;
    use objc::runtime::{Object, BOOL, NO};
    use objc::{class, msg_send, sel, sel_impl};

    /// The pasteboard's plain-text type (`NSPasteboardTypeString`).
    const STRING: &str = "public.utf8-plain-text";
    /// nspasteboard.org's marks: not to be kept or shown by clipboard
    /// managers, and gone with the next copy.
    const CONCEALED: &str = "org.nspasteboard.ConcealedType";
    const TRANSIENT: &str = "org.nspasteboard.TransientType";

    unsafe fn ns(s: &str) -> *mut Object {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    pub fn write(text: &str) -> bool {
        autoreleasepool(|| unsafe {
            let board: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            if board.is_null() {
                return false;
            }
            let (string, concealed, transient) = (ns(STRING), ns(CONCEALED), ns(TRANSIENT));
            let types = [string, concealed, transient];
            let list: *mut Object = msg_send![class!(NSArray), arrayWithObjects: types.as_ptr() count: types.len()];
            let _: isize = msg_send![board, declareTypes: list owner: std::ptr::null_mut::<Object>()];
            let took: BOOL = msg_send![board, setString: ns(text) forType: string];
            let empty = ns("");
            let _: BOOL = msg_send![board, setString: empty forType: concealed];
            let _: BOOL = msg_send![board, setString: empty forType: transient];
            took != NO
        })
    }

    pub fn clear() {
        autoreleasepool(|| unsafe {
            let board: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            if !board.is_null() {
                let _: isize = msg_send![board, clearContents];
            }
        });
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_UNICODETEXT;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// `bytes` as clipboard data of `format`; whether the clipboard took it
    /// (it owns the memory then).
    unsafe fn put(format: u32, bytes: &[u8]) -> bool {
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
            if memory.is_null() {
                return false;
            }
            let at = GlobalLock(memory) as *mut u8;
            if at.is_null() {
                GlobalFree(memory);
                return false;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len());
            GlobalUnlock(memory);
            if SetClipboardData(format, memory).is_null() {
                GlobalFree(memory);
                return false;
            }
            true
        }
    }

    pub fn write(text: &str) -> bool {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return false;
            }
            EmptyClipboard();
            let words = wide(text);
            let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let took = put(CF_UNICODETEXT as u32, &bytes);
            // Windows' marks: no monitor, history or cloud keeps it.
            for (name, value) in [
                ("ExcludeClipboardContentFromMonitorProcessing", None),
                ("CanIncludeInClipboardHistory", Some(0u32)),
                ("CanUploadToCloudClipboard", Some(0u32)),
            ] {
                let format = RegisterClipboardFormatW(wide(name).as_ptr());
                if format != 0 {
                    let data = value.map(|v| v.to_le_bytes().to_vec()).unwrap_or_default();
                    put(format, &data);
                }
            }
            CloseClipboard();
            took
        }
    }

    pub fn clear() {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                EmptyClipboard();
                CloseClipboard();
            }
        }
    }
}
