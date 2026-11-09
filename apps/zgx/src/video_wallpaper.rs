//! A new chat's background video (Windows): its first frame, read once with
//! Media Foundation when it's chosen, for its size and a still (Settings'
//! thumbnail, the Adjust dialog). It plays in a web view (`wallpaper.player`),
//! where WebView2 decodes it on the GPU and composites it without a copy:
//! none of its frames pass through the app.

use std::sync::Arc;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;
use zgx::image::ImageData;

/// The files it plays (what WebView2 plays and Media Foundation reads).
pub(crate) const EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm"];

pub(crate) fn is_video(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// COM for this thread, and Media Foundation for the process (started once,
/// never shut down: the app's lifetime).
fn start() -> Result<(), String> {
    static STARTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // (Already initialized, in either model, is fine for a reader.)
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    STARTED
        .get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.is_ok())
        .then_some(())
        .ok_or_else(|| "failed".to_string())
}

const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// `path`'s first frame, as RGBA at its own size. Blocks: call it off the
/// main thread.
pub(crate) fn first_frame(path: &str) -> Result<Arc<ImageData>, String> {
    start()?;
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1).map_err(|_| "failed")?;
        let attrs = attrs.ok_or("failed")?;
        // Color conversion in the reader.
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(|_| "failed")?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), &attrs).map_err(|_| "invalid")?;
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
        reader.SetStreamSelection(STREAM, true).map_err(|_| "invalid")?;
        let ty = MFCreateMediaType().map_err(|_| "failed")?;
        ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|_| "failed")?;
        ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|_| "failed")?;
        reader.SetCurrentMediaType(STREAM, None, &ty).map_err(|_| "invalid")?;
        let current = reader.GetCurrentMediaType(STREAM).map_err(|_| "invalid")?;
        let size = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|_| "invalid")?;
        let (w, h) = ((size >> 32) as u32, size as u32);
        if w == 0 || h == 0 {
            return Err("invalid".into());
        }
        let stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((w * 4) as i32);
        // (A gap in the stream gives no sample: a few are passed over.)
        for _ in 0..64 {
            let (mut flags, mut sample) = (0u32, None);
            reader.ReadSample(STREAM, 0, None, Some(&mut flags), None, Some(&mut sample)).map_err(|_| "invalid")?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            let buffer = sample.ConvertToContiguousBuffer().map_err(|_| "failed")?;
            let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
            buffer.Lock(&mut data, None, Some(&mut len)).map_err(|_| "failed")?;
            let mut rgba = vec![0u8; w as usize * h as usize * 4];
            // (The swap optimized even in a debug build: zeron-zgx-pixels.)
            let ok = !data.is_null()
                && zeron_zgx_pixels::bgrx_rows_to_rgba(
                    std::slice::from_raw_parts(data, len as usize),
                    stride.unsigned_abs() as usize,
                    stride < 0,
                    w as usize,
                    h as usize,
                    &mut rgba,
                );
            let _ = buffer.Unlock();
            if !ok {
                return Err("failed".into());
            }
            return ImageData::new(w, h, rgba).map(Arc::new).map_err(|_| "failed".to_string());
        }
        Err("invalid".into())
    }
}

thread_local! {
    /// `window_minimized`, as the hooks below last heard.
    static MINIMIZED: std::cell::RefCell<Option<zgx_script::Shared>> = const { std::cell::RefCell::new(None) };
}

/// `window_minimized` (a shared state scripts read): whether the app's
/// window is minimized or cloaked (on another virtual desktop), told by the
/// system as it happens (no polling). zgx stops drawing such a window but
/// tells scripts and web views nothing on Windows, so a playing background
/// would go on decoding; the player pauses on it.
pub(crate) fn watch_minimized(host: &zgx_script::ScriptHost) {
    use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART,
        OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
    };
    unsafe extern "system" fn heard(_: HWINEVENTHOOK, event: u32, _: windows_sys::Win32::Foundation::HWND, object: i32, _: i32, _: u32, _: u32) {
        if object != OBJID_WINDOW {
            return;
        }
        let hidden = matches!(event, EVENT_SYSTEM_MINIMIZESTART | EVENT_OBJECT_CLOAKED);
        MINIMIZED.with(|shared| {
            if let Some(shared) = &*shared.borrow() {
                shared.set(zgx_script::Value::Bool(hidden));
            }
        });
    }
    MINIMIZED.with(|shared| *shared.borrow_mut() = Some(host.shared("window_minimized", false)));
    let pid = std::process::id();
    // SAFETY: out of context, so `heard` runs on this (the UI) thread's
    // message loop, for this process's windows alone; the hooks last as the
    // app does.
    unsafe {
        // (Coming to the front, it shows: a window that opened minimized
        // tells no end of it.)
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, std::ptr::null_mut(), Some(heard), pid, 0, WINEVENT_OUTOFCONTEXT);
        SetWinEventHook(EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND, std::ptr::null_mut(), Some(heard), pid, 0, WINEVENT_OUTOFCONTEXT);
        SetWinEventHook(EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED, std::ptr::null_mut(), Some(heard), pid, 0, WINEVENT_OUTOFCONTEXT);
    }
}
