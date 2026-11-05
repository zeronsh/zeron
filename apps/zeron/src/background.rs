//! The desktop app's background engine process (`zeron-engine`, or
//! `zeron engine` where no separate executable ships): the app starts it
//! when nothing serves its port and attaches to it; it keeps running —
//! sessions, desktop banners — with the app closed
//! ([`zeron_engine::managed`]).

use std::path::{Path, PathBuf};

/// Run the background engine until it's stopped.
pub fn run() -> anyhow::Result<()> {
    crate::init_logging(true, "engine", false);
    crate::long_running_setup();
    let config = crate::engine_config_from_env();
    zeron_engine::desktop_notify::enable();
    let app = std::env::var_os(zeron_engine::managed::APP_ENV).map(PathBuf::from);
    // Banners carry the app's name and icon (the development bundle's for
    // the development app), and a click opens the app on the banner's chat.
    if let Some(id) = app.as_deref().and_then(zeron_notify::bundle_id_of) {
        zeron_notify::set_app_bundle_id(&id);
    }
    zeron_notify::on_click(move |target| open_in_app(app.as_deref(), &target));
    run_engine(config)
}

/// A clicked banner: bring the app up on its target. The app may not be
/// running — opening its link launches it.
fn open_in_app(app: Option<&Path>, target: &str) {
    let link = zeron_notify::link(target);
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("/usr/bin/open");
        if let Some(app) = app {
            command.arg("-a").arg(app);
        }
        if let Err(err) = command
            .arg(&link)
            .spawn()
            .and_then(|mut child| child.wait())
        {
            tracing::warn!(error = %err, "couldn't open the app for a clicked banner");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, link);
}

#[cfg(not(target_os = "macos"))]
fn run_engine(config: zeron_engine::EngineConfig) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(zeron_engine::Engine::new(config).run_managed())
}

/// macOS: the engine runs on Tokio threads while the main thread runs a
/// CoreFoundation run loop, which services the main dispatch queue banners
/// are posted on and delivers their clicks.
#[cfg(target_os = "macos")]
fn run_engine(config: zeron_engine::EngineConfig) -> anyhow::Result<()> {
    let worker = std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let result = tokio::runtime::Runtime::new()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| {
                    runtime.block_on(zeron_engine::Engine::new(config).run_managed())
                });
            main_loop::finish();
            result
        })?;
    main_loop::run();
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("the engine thread panicked"))?
}

#[cfg(target_os = "macos")]
mod main_loop {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};

    static DONE: AtomicBool = AtomicBool::new(false);

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        static kCFRunLoopCommonModes: *const c_void;
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
        fn CFRunLoopWakeUp(run_loop: *mut c_void);
        fn CFAbsoluteTimeGetCurrent() -> f64;
        fn CFRunLoopTimerCreate(
            allocator: *const c_void,
            fire_date: f64,
            interval: f64,
            flags: u64,
            order: isize,
            callout: extern "C" fn(*mut c_void, *mut c_void),
            context: *mut c_void,
        ) -> *mut c_void;
        fn CFRunLoopAddTimer(run_loop: *mut c_void, timer: *mut c_void, mode: *const c_void);
    }

    extern "C" fn tick(_timer: *mut c_void, _info: *mut c_void) {}

    /// Serve the main run loop until [`finish`]. A repeating timer keeps the
    /// loop from returning at once for want of sources and bounds how late
    /// the exit check runs.
    pub(super) fn run() {
        unsafe {
            let main = CFRunLoopGetMain();
            let timer = CFRunLoopTimerCreate(
                std::ptr::null(),
                CFAbsoluteTimeGetCurrent() + 1.0,
                1.0,
                0,
                0,
                tick,
                std::ptr::null_mut(),
            );
            CFRunLoopAddTimer(main, timer, kCFRunLoopCommonModes);
            while !DONE.load(Ordering::SeqCst) {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 0);
            }
        }
    }

    /// End [`run`] (any thread).
    pub(super) fn finish() {
        DONE.store(true, Ordering::SeqCst);
        unsafe { CFRunLoopWakeUp(CFRunLoopGetMain()) };
    }
}
