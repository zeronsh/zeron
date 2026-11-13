//! Whether the system asks for no transparency, as the shared value
//! `system.reduced_transparency` every window's scripts read (true: glass
//! turns solid; ui/ds/material.luau `see_through`).
//!
//! zgx reports macOS's Reduce transparency itself
//! (`ui.prefers.reduced_transparency`), which material reads too, but on
//! Windows it always says false (zgx-desktop's system_colors.rs;
//! zgx-asks/windows-transparency). So here the host reads Windows'
//! Transparency effects (Settings → Personalization → Colors): the
//! `EnableTransparency` value under HKCU\Software\Microsoft\Windows\
//! CurrentVersion\Themes\Personalize, 0 when off. It follows the switch as
//! it's flipped: a thread waits on the key's change
//! (`RegNotifyChangeKeyValue`), never polling, and hands what it reads to
//! the window's task. Elsewhere it stays false.

use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

/// `system.reduced_transparency` for `host`'s scripts, kept as the system's;
/// and `system.popups_blur`: whether a popup's glass (a menu, a popover, a
/// tooltip, each a window of its own) can blur what's under it. On Windows
/// it can't: zgx gives popup windows no system backdrop there (DWM draws a
/// never-activated window's flat, popup_window/win32.rs), and a window of its
/// own has nothing of the app's to blur, so what's behind showed through
/// sharp (a wallpaper's dither through the model picker). macOS's popups
/// have AppKit's material; Linux's, zgx's blur.
pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    host.shared("system.popups_blur", !cfg!(windows));
    let shared = host.shared("system.reduced_transparency", false);
    #[cfg(windows)]
    {
        shared.set(Value::Bool(windows::reduced()));
        let (tell, mut heard) = zgx::task::latest::<bool>();
        std::thread::spawn(move || windows::watch(|reduced| tell.send(reduced)));
        cx.tasks.spawn(async move {
            while let Some(reduced) = heard.recv().await {
                shared.set(Value::Bool(reduced));
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (cx, shared, Value::Nil);
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_QUERY_VALUE, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_DWORD, RegCloseKey, RegGetValueW,
        RegNotifyChangeKeyValue, RegOpenKeyExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    const VALUE: &str = "EnableTransparency";

    /// The key's `EnableTransparency`: 0 is off. Missing (an older Windows,
    /// or never set), transparency is on, as Windows draws it.
    fn read(key: HKEY) -> bool {
        let value = wide(VALUE);
        let mut data: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: `key` is open (or HKEY_CURRENT_USER with the subkey's path,
        // below); `data` is a DWORD of `size` bytes, which RRF_RT_REG_DWORD
        // asks for exactly; `value` is NUL-terminated.
        let status = unsafe { RegGetValueW(key, std::ptr::null(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut data as *mut u32).cast(), &mut size) };
        status == ERROR_SUCCESS && data == 0
    }

    /// Whether the system's Transparency effects are off now.
    pub fn reduced() -> bool {
        let path = wide(KEY);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: a NUL-terminated path under HKEY_CURRENT_USER; `key` is
        // written on success and closed here.
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) } != ERROR_SUCCESS {
            return false;
        }
        let off = read(key);
        // SAFETY: opened above.
        unsafe { RegCloseKey(key) };
        off
    }

    /// Tells `tell` each change of the setting (only changes), waiting on
    /// the key between them, until `tell` says no one listens (its window
    /// closed) or the key can't be watched.
    pub fn watch(tell: impl Fn(bool) -> bool) {
        let path = wide(KEY);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: as in `reduced`, with the right to watch it too.
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_QUERY_VALUE | KEY_NOTIFY, &mut key) } != ERROR_SUCCESS {
            return;
        }
        let mut was = read(key);
        loop {
            // SAFETY: an open key; synchronous (no event): returns once a
            // value under it is set.
            if unsafe { RegNotifyChangeKeyValue(key, 0, REG_NOTIFY_CHANGE_LAST_SET, std::ptr::null_mut(), 0) } != ERROR_SUCCESS {
                break;
            }
            let now = read(key);
            if now != was {
                was = now;
                if !tell(now) {
                    break;
                }
            }
        }
        // SAFETY: opened above.
        unsafe { RegCloseKey(key) };
    }
}
