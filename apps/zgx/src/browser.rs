//! Whether the panel's browser tabs can show pages here
//! (ui/screens/panel/browser.luau). On Windows a page is WebView2's, in the
//! window's DirectComposition tree, so it takes the WebView2 runtime and
//! Direct3D 12: without either, zgx draws a placeholder where the page
//! would be ("Web view · <url>"), and the tab says why instead. zgx decides
//! the same way (zgx-desktop's native_host: `webview2::available` and
//! `WGPU_BACKEND`); scripts can't ask it yet (zgx-asks/web-view-supported).
//!
//!   ui.call_sync("browser.unavailable")   nil where pages show; else why:
//!                                         "runtime" (no WebView2) or
//!                                         "renderer" (no Direct3D 12)
//!
//! Elsewhere it's always nil: macOS's pages are WKWebView's, Linux's
//! WebKitGTK's (or zgx's placeholder), as they were.

use zgx_script::{ScriptHost, Value};

pub fn install(host: &ScriptHost) {
    host.function("browser.unavailable", |_| unavailable().map_or(Value::Nil, Value::from));
}

#[cfg(windows)]
fn unavailable() -> Option<&'static str> {
    static WHY: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *WHY.get_or_init(|| {
        if !windows::runtime_installed() {
            Some("runtime")
        } else if !direct3d_12(std::env::var("WGPU_BACKEND").ok().as_deref()) {
            Some("renderer")
        } else {
            None
        }
    })
}

#[cfg(not(windows))]
fn unavailable() -> Option<&'static str> {
    None
}

/// Whether wgpu may draw with Direct3D 12, given `WGPU_BACKEND` (any, unset).
#[cfg(any(windows, test))]
fn direct3d_12(backends: Option<&str>) -> bool {
    backends.is_none_or(|backends| {
        let backends = backends.to_ascii_lowercase();
        backends.contains("dx12") || backends.contains("d3d12")
    })
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;

    // The loader zgx-desktop links in (webview2-com-sys ships it, and says
    // where): the same call zgx asks.
    // SAFETY: WebView2Loader.h's declaration (`STDAPI`: "system"): the folder
    // a NUL-terminated string or null, `version` a string the loader
    // allocates with CoTaskMemAlloc.
    #[link(name = "WebView2LoaderStatic", kind = "static")]
    unsafe extern "system" {
        #[link_name = "GetAvailableCoreWebView2BrowserVersionString"]
        fn browser_version(browser_folder: *const u16, version: *mut *mut u16) -> i32;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(memory: *const c_void);
    }

    /// Whether a WebView2 runtime is there to make pages with (the
    /// evergreen one Windows 11 ships, a fixed version, or Edge's).
    pub(super) fn runtime_installed() -> bool {
        let mut version: *mut u16 = std::ptr::null_mut();
        // SAFETY: the loader writes a string it allocated, freed here.
        let result = unsafe { browser_version(std::ptr::null(), &mut version) };
        let found = result >= 0 && !version.is_null();
        if !version.is_null() {
            // SAFETY: allocated by the loader with CoTaskMemAlloc.
            unsafe { CoTaskMemFree(version.cast()) };
        }
        found
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pages_need_direct3d_12_among_the_backends() {
        assert!(super::direct3d_12(None));
        assert!(super::direct3d_12(Some("dx12")));
        assert!(super::direct3d_12(Some("vulkan,DX12")));
        assert!(!super::direct3d_12(Some("vulkan")));
        assert!(!super::direct3d_12(Some("gl")));
    }
}
