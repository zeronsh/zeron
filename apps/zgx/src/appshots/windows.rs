//! Appshots on Windows, as the shipping app's plan has them
//! (docs/plans/2026-08-22-appshots-windows-linux.md): the shortcut is a hot
//! key (`RegisterHotKey`) held by a thread of its own, whose message loop
//! hears each press; the capture is the window in front as the screen shows
//! it, inside its frame (not the invisible resize borders), named by its
//! program and title. Windows asks no permission for either. No application
//! text (UI Automation isn't walked): the picture only.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, RECT},
    Graphics::{
        Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
            DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
        },
    },
    Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
    System::Threading::{
        GetCurrentProcessId, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
    UI::{
        Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey},
        WindowsAndMessaging::{
            GetForegroundWindow, GetMessageW, GetShellWindow, GetSystemMetrics, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
            GetWindowThreadProcessId, IsIconic, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
            SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WM_APP, WM_HOTKEY, WM_USER,
        },
    },
};

use super::{AccessibilitySnapshot, CaptureError, Captured, Shortcut, validate_capture_dimensions};

/// The thread's message asking for another hot key: `wParam` its
/// modifiers, `lParam` its key and whether to hold it (bit 16).
const SET: u32 = WM_APP + 1;
const HOT_KEY_ID: i32 = 1;
const HOLD: isize = 1 << 16;

/// Whether the shortcut was free the last time it was asked for.
static READY: AtomicBool = AtomicBool::new(false);

struct HotKeyThread {
    id: u32,
    answers: mpsc::Receiver<bool>,
    /// What was last asked for (modifiers, key, held).
    asked: Option<(u32, u32, bool)>,
}

static HOT_KEY: Mutex<Option<HotKeyThread>> = Mutex::new(None);

/// The key's virtual-key code (the keys `Shortcut::parse` takes).
pub(super) fn virtual_key(key: &str) -> Option<u32> {
    let named = match key {
        "space" => 0x20,
        "tab" => 0x09,
        "enter" => 0x0D,
        "backspace" => 0x08,
        "delete" => 0x2E,
        "insert" => 0x2D,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=20).contains(&n).then_some(0x70 + n - 1);
    }
    match key.as_bytes() {
        [c] if c.is_ascii_alphanumeric() => Some(u32::from(c.to_ascii_uppercase())),
        _ => None,
    }
}

fn modifiers(shortcut: &Shortcut) -> u32 {
    (if shortcut.control { MOD_CONTROL } else { 0 })
        | (if shortcut.alt { MOD_ALT } else { 0 })
        | (if shortcut.shift { MOD_SHIFT } else { 0 })
        | (if shortcut.platform { MOD_WIN } else { 0 })
}

/// Starts the hot key's thread (once a process), `press` told of each press.
pub(super) fn start_global_shortcut(press: zgx::task::LatestSender<()>) {
    let mut slot = HOT_KEY.lock().unwrap();
    if slot.is_some() {
        return;
    }
    let (id_tx, id_rx) = mpsc::channel::<u32>();
    let (answer_tx, answers) = mpsc::channel::<bool>();
    let spawned = std::thread::Builder::new().name("appshots-hot-key".into()).spawn(move || {
        // SAFETY: plain Win32 calls on this thread's own message queue; `msg`
        // is a MSG the calls write. A hot key registered with no window is
        // this thread's, its presses posted here as WM_HOTKEY.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            // (Its queue made before anyone posts to it.)
            PeekMessageW(&mut msg, std::ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE);
            let _ = id_tx.send(GetCurrentThreadId());
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                match msg.message {
                    WM_HOTKEY => {
                        press.send(());
                    }
                    SET => {
                        UnregisterHotKey(std::ptr::null_mut(), HOT_KEY_ID);
                        let key = (msg.lParam & 0xFFFF) as u32;
                        let held = msg.lParam & HOLD != 0;
                        let free = key != 0 && RegisterHotKey(std::ptr::null_mut(), HOT_KEY_ID, msg.wParam as u32 | MOD_NOREPEAT, key) != 0;
                        // (Off, it is only asked whether it's free, then let go.)
                        if free && !held {
                            UnregisterHotKey(std::ptr::null_mut(), HOT_KEY_ID);
                        }
                        let _ = answer_tx.send(free);
                    }
                    _ => {}
                }
            }
        }
    });
    if spawned.is_err() {
        return;
    }
    if let Ok(id) = id_rx.recv() {
        *slot = Some(HotKeyThread { id, answers, asked: None });
    }
}

/// Asks for `shortcut`: held while `hold` (Appshots on), else only checked
/// for being free; `None` lets it go. Answered before this returns.
pub(super) fn refresh_global_shortcut(shortcut: Option<&Shortcut>, hold: bool) {
    let mut slot = HOT_KEY.lock().unwrap();
    let Some(thread) = slot.as_mut() else {
        return;
    };
    let asked = match shortcut {
        Some(s) => match virtual_key(&s.key) {
            Some(key) => (modifiers(s), key, hold),
            None => {
                READY.store(false, Ordering::Relaxed);
                (0, 0, false)
            }
        },
        None => (0, 0, false),
    };
    if thread.asked == Some(asked) {
        return;
    }
    thread.asked = Some(asked);
    let (mods, key, held) = asked;
    // SAFETY: posts to the hot key's thread, whose queue exists (made before
    // it sent its id).
    let posted = unsafe { PostThreadMessageW(thread.id, SET, mods as usize, key as isize | if held { HOLD } else { 0 }) } != 0;
    let free = posted && thread.answers.recv_timeout(std::time::Duration::from_secs(1)).unwrap_or(false);
    if key != 0 {
        READY.store(free, Ordering::Relaxed);
        if !free {
            eprintln!("zeron-zgx: the Appshot shortcut isn't available (another app holds it)");
        }
    }
}

pub(super) fn shortcut_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

fn foreground_process() -> Option<(HWND, u32)> {
    // SAFETY: plain queries; a window gone meanwhile answers 0.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        Some((window, pid))
    }
}

/// Whether the window in front is one of Zeron's.
pub(super) fn app_is_active() -> bool {
    foreground_process().is_some_and(|(_, pid)| pid == unsafe { GetCurrentProcessId() })
}

fn wide_to_string(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn window_title(window: HWND) -> Option<String> {
    // SAFETY: `buffer` holds `len + 1` UTF-16 units, the most written.
    unsafe {
        let len = GetWindowTextLengthW(window);
        if len <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let got = GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32);
        let title = wide_to_string(&buffer[..got.max(0) as usize]);
        (!title.trim().is_empty()).then_some(title)
    }
}

/// The program's path, for a process that may be another user's or
/// elevated (then it can't be opened: no name).
fn program_path(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed below; `buffer` holds `len` units.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 32 * 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// The program's own name for itself ("Microsoft Edge"): its version
/// resource's FileDescription, in its first language.
fn program_description(path: &str) -> Option<String> {
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let file = wide(path);
    // SAFETY: `data` is the size GetFileVersionInfoSizeW asked for; the
    // pointers VerQueryValueW hands back point into `data`, read while it
    // lives, within the lengths it says.
    unsafe {
        let size = GetFileVersionInfoSizeW(file.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(file.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut languages: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let query = wide(r"\VarFileInfo\Translation");
        if VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut languages, &mut len) == 0 || len < 4 {
            return None;
        }
        let pair = languages as *const u16;
        let (language, codepage) = (*pair, *pair.add(1));
        let query = wide(&format!(r"\StringFileInfo\{language:04x}{codepage:04x}\FileDescription"));
        let mut value: *mut core::ffi::c_void = std::ptr::null_mut();
        if VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut value, &mut len) == 0 || len == 0 {
            return None;
        }
        let text = wide_to_string(std::slice::from_raw_parts(value as *const u16, len as usize));
        let text = text.trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
}

/// The app's name: its program's description, else its file's name.
fn app_name(pid: u32) -> String {
    let Some(path) = program_path(pid) else {
        return "Application".into();
    };
    program_description(&path)
        .unwrap_or_else(|| std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Application".into()))
}

/// The window's box on the screen: its visible frame (DWM's bounds, without
/// the invisible resize borders), within the screens.
fn window_bounds(window: HWND) -> Option<RECT> {
    // SAFETY: `rect` is a RECT of the size given; plain metrics.
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        let dwm =
            DwmGetWindowAttribute(window, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&mut rect as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32);
        if dwm != 0 && GetWindowRect(window, &mut rect) == 0 {
            return None;
        }
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        rect.left = rect.left.max(x);
        rect.top = rect.top.max(y);
        rect.right = rect.right.min(x + w);
        rect.bottom = rect.bottom.min(y + h);
        (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
    }
}

/// The screen's pixels in `rect`, as RGBA8.
fn screen_pixels(rect: RECT) -> Result<(u32, u32, Vec<u8>), CaptureError> {
    let (w, h) = ((rect.right - rect.left) as u32, (rect.bottom - rect.top) as u32);
    let bytes = validate_capture_dimensions(w, h)?;
    let mut pixels = vec![0u8; bytes];
    // SAFETY: every GDI object made here is let go of below; `pixels` holds
    // the w×h×4 bytes a top-down 32-bit DIB of the bitmap fills.
    let copied = unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("The screen couldn't be read.".into());
        }
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w as i32, h as i32);
        let old = SelectObject(memory, bitmap);
        let blitted = BitBlt(memory, 0, 0, w as i32, h as i32, screen, rect.left, rect.top, SRCCOPY | CAPTUREBLT) != 0;
        SelectObject(memory, old);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            // (Negative: rows top to bottom.)
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let rows = if blitted { GetDIBits(memory, bitmap, 0, h, pixels.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS) } else { 0 };
        DeleteObject(bitmap);
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        rows == h as i32
    };
    if !copied {
        return Err("The window couldn't be captured.".into());
    }
    // BGRX to RGBA: the screen has no transparency.
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    Ok((w, h, pixels))
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, CaptureError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut writer| writer.write_image_data(rgba)).map_err(|e| format!("Could not encode the captured window: {e}"))?;
    Ok(out)
}

/// The window in front, captured: never one of Zeron's, the desktop, or a
/// minimized window.
pub(super) fn capture_foreground_window() -> Result<Captured, CaptureError> {
    let (window, pid) = foreground_process().ok_or(CaptureError::NoEligibleWindow)?;
    if pid == unsafe { GetCurrentProcessId() } {
        return Err(CaptureError::SelfCapture);
    }
    // SAFETY: plain queries of a window handle (0 if it's gone).
    if unsafe { IsIconic(window) } != 0 || window == unsafe { GetShellWindow() } {
        return Err(CaptureError::NoEligibleWindow);
    }
    let rect = window_bounds(window).ok_or(CaptureError::NoEligibleWindow)?;
    let (w, h, rgba) = screen_pixels(rect)?;
    Ok(Captured {
        app_name: app_name(pid),
        bundle_identifier: None,
        window_title: window_title(window),
        accessibility: AccessibilitySnapshot::unavailable(),
        png: encode_png(w, h, &rgba)?,
        icon_png: None,
    })
}

/// The capture sound, played through Windows (no `paplay` here).
pub(super) fn play_sound(path: &std::path::Path) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_NODEFAULT, SND_SYNC};
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated path; SND_SYNC returns once played.
    unsafe {
        PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), SND_FILENAME | SND_NODEFAULT | SND_SYNC);
    }
}
