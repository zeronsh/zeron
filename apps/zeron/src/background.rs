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
    // Before the engine starts: its sockets then never start the polling
    // wake detector (`zeron_sync::wake`).
    main_loop::watch_power();
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
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

    static DONE: AtomicBool = AtomicBool::new(false);
    /// The source [`finish`] signals; null until [`run`] makes it.
    static STOP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    /// `CFRunLoopSourceContext`, version 0: only `perform` is used.
    #[repr(C)]
    struct SourceContext {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
        equal: *const c_void,
        hash: *const c_void,
        schedule: *const c_void,
        cancel: *const c_void,
        perform: extern "C" fn(*mut c_void),
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        static kCFRunLoopCommonModes: *const c_void;
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
        fn CFRunLoopWakeUp(run_loop: *mut c_void);
        fn CFRunLoopSourceCreate(
            allocator: *const c_void,
            order: isize,
            context: *mut SourceContext,
        ) -> *mut c_void;
        fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
        fn CFRunLoopSourceSignal(source: *mut c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegisterForSystemPower(
            refcon: *mut c_void,
            port: *mut *mut c_void,
            callback: extern "C" fn(*mut c_void, u32, u32, *mut c_void),
            notifier: *mut u32,
        ) -> u32;
        fn IONotificationPortGetRunLoopSource(port: *mut c_void) -> *mut c_void;
        fn IOAllowPowerChange(kernel_port: u32, notification_id: isize) -> i32;
    }

    // IOKit's power messages (IOKit/IOMessage.h).
    const CAN_SYSTEM_SLEEP: u32 = 0xe000_0270;
    const SYSTEM_WILL_SLEEP: u32 = 0xe000_0280;
    const SYSTEM_HAS_POWERED_ON: u32 = 0xe000_0300;

    /// The root power domain's connection, for acknowledging sleep.
    static ROOT_PORT: AtomicU32 = AtomicU32::new(0);

    extern "C" fn power(_refcon: *mut c_void, _service: u32, message: u32, argument: *mut c_void) {
        match message {
            // Every registered client must answer these, or the Mac waits
            // up to 30 s before it sleeps.
            CAN_SYSTEM_SLEEP | SYSTEM_WILL_SLEEP => unsafe {
                IOAllowPowerChange(ROOT_PORT.load(Ordering::Acquire), argument as isize);
            },
            SYSTEM_HAS_POWERED_ON => zeron_sync::wake::notify_wake(),
            _ => {}
        }
    }

    /// Hear the Mac's sleep and wake from IOKit on the main run loop, so the
    /// engine's sockets redial on wake without a detector waking the
    /// process every second to compare clocks. Falls back to that detector
    /// if registering fails.
    pub(super) fn watch_power() {
        unsafe {
            let mut port = std::ptr::null_mut();
            let mut notifier = 0u32;
            let root =
                IORegisterForSystemPower(std::ptr::null_mut(), &mut port, power, &mut notifier);
            if root == 0 || port.is_null() {
                tracing::warn!(
                    "couldn't register for power notifications; polling for wake instead"
                );
                return;
            }
            ROOT_PORT.store(root, Ordering::Release);
            let source = IONotificationPortGetRunLoopSource(port);
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
        }
        zeron_sync::wake::use_os_wake_events();
    }

    /// The stop source fired: [`run`]'s wait returns and it reads `DONE`.
    extern "C" fn stop(_info: *mut c_void) {}

    /// Serve the main run loop until [`finish`]. It sleeps until something
    /// arrives (a banner's click, the stop source) rather than on a timer:
    /// an idle engine's main thread never wakes. The stop source both keeps
    /// the loop from returning at once for want of sources and makes a
    /// `finish` that comes before the wait begins end it at once (a signaled
    /// source is served as soon as the loop runs).
    pub(super) fn run() {
        unsafe {
            let main = CFRunLoopGetMain();
            let mut context = SourceContext {
                version: 0,
                info: std::ptr::null_mut(),
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
                equal: std::ptr::null(),
                hash: std::ptr::null(),
                schedule: std::ptr::null(),
                cancel: std::ptr::null(),
                perform: stop,
            };
            let source = CFRunLoopSourceCreate(std::ptr::null(), 0, &mut context);
            CFRunLoopAddSource(main, source, kCFRunLoopCommonModes);
            STOP.store(source, Ordering::SeqCst);
            while !DONE.load(Ordering::SeqCst) {
                // (Returns after each source served, so `DONE` is read then.)
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0e10, 1);
            }
        }
    }

    /// End [`run`] (any thread).
    pub(super) fn finish() {
        DONE.store(true, Ordering::SeqCst);
        let source = STOP.load(Ordering::SeqCst);
        unsafe {
            if !source.is_null() {
                CFRunLoopSourceSignal(source);
            }
            CFRunLoopWakeUp(CFRunLoopGetMain());
        }
    }
}
