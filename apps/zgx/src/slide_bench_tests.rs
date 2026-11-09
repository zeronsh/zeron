//! What a chat's message costs the main window, measured headless: the
//! shell's fixture (ui/shell_fixture.luau) with four hundred sessions, a
//! message every two seconds bringing the third most recent chat to the top
//! (its `message` command), the rows sliding as a window shows them, drawn
//! by zgx's renderer offscreen at a Retina scale, frame by frame on the
//! test's 60 Hz clock (so every run draws the same frames at the same times).
//!
//!   cargo test --release --bin zeron-zgx slide_cost -- --ignored --nocapture
//!       each message's CPU: script and layout, damage, drawing (this thread),
//!       and the whole process's (the GPU driver's threads with it)
//!   SLIDE_PIXELS=<dir> cargo test --release --bin zeron-zgx slide_pixels -- --ignored --nocapture
//!       every frame drawn after each of the first messages read back, its
//!       pixels hashed into <dir>/frames.txt (and kept, raw RGBA, in <dir>),
//!       to compare two builds frame for frame (`cmp`, or a script)
//!
//! Against another zgx: `--config 'patch."https://github.com/zeronsh/zgx".<crate>.path="<copy>/crates/<crate>"'`
//! for zgx, zgx-gpu, zgx-script, zgx-desktop and zgx-run.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zgx_gpu::GpuRenderer;
use zgx_script::{ScriptHost, testing::TestApp};

const W: f32 = 1240.;
const H: f32 = 820.;
const SCALE: f32 = 2.;
/// Frames a message's two seconds hold on the test's 60 Hz clock.
const FRAMES_PER_MESSAGE: usize = 120;
/// The frames after a message its rows slide in (about 180 ms of them).
const SLIDE_FRAMES: usize = 12;

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-slide-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// This thread's CPU time so far.
fn thread_cpu() -> Duration {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: fills a timespec for this thread's CPU clock.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

/// The process's CPU time so far, every thread.
fn process_cpu() -> Duration {
    // SAFETY: fills a rusage for this process.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let t = |v: libc::timeval| Duration::new(v.tv_sec as u64, v.tv_usec as u32 * 1000);
    t(usage.ru_utime) + t(usage.ru_stime)
}

/// The shell with four hundred sessions, drawn offscreen by `gpu`.
fn mounted(storage: &Path) -> (TestApp, GpuRenderer) {
    let fixture = ui().join("shell_fixture.luau");
    let storage = storage.to_path_buf();
    let app = TestApp::new(move |runtime| ScriptHost::new(runtime, fixture).storage(storage));
    let mut gpu = GpuRenderer::new((W * SCALE) as u32, (H * SCALE) as u32).expect("a GPU");
    gpu.set_scale_factor(SCALE);
    for font in crate::fonts() {
        font.expect("the app's fonts").install(&mut gpu.text_system().borrow_mut());
    }
    gpu.install_text(&mut app.ui.scene.borrow_mut());
    app.ui.refresh_text_geometry();
    app.ui.scene.borrow_mut().resize(W, H);
    assert!(app.host.run_command("many"), "the fixture's four hundred sessions");
    // Settled: everything drawn once, the launch's motion over.
    for _ in 0..240 {
        frame(&app, &mut gpu);
    }
    (app, gpu)
}

/// One frame as a window's: the test clock's next, laid out, its damage
/// drawn (nothing drawn where nothing changed). Whether it drew.
fn frame(app: &TestApp, gpu: &mut GpuRenderer) -> bool {
    app.frame();
    let report = app.ui.scene.borrow_mut().flush();
    if report.damage.is_empty() {
        return false;
    }
    gpu.render(&app.ui.scene.borrow(), &report.damage).expect("a frame drawn");
    true
}

#[test]
#[ignore = "a benchmark: run it --release, as the doc above says"]
fn slide_cost() {
    let messages: usize = std::env::var("SLIDE_MESSAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let (app, mut gpu) = mounted(&scratch("cost"));
    let (mut script, mut damage, mut draw, mut drawn) = (Duration::ZERO, Duration::ZERO, Duration::ZERO, 0usize);
    // The slide's frames (the first after a message) apart from the rest
    // (a working session's glyph turning, at its own pace).
    let (mut slide_draw, mut slide_drawn) = (Duration::ZERO, 0usize);
    let process = process_cpu();
    for _ in 0..messages {
        assert!(app.host.run_command("message"));
        for f in 0..FRAMES_PER_MESSAGE {
            let a = thread_cpu();
            app.frame();
            let b = thread_cpu();
            let report = app.ui.scene.borrow_mut().flush();
            let c = thread_cpu();
            if !report.damage.is_empty() {
                gpu.render(&app.ui.scene.borrow(), &report.damage).expect("a frame drawn");
                drawn += 1;
            }
            let d = thread_cpu();
            if f < SLIDE_FRAMES && !report.damage.is_empty() {
                slide_draw += d - c;
                slide_drawn += 1;
            }
            script += b - a;
            damage += c - b;
            draw += d - c;
        }
    }
    let process = process_cpu() - process;
    let per = |d: Duration| d.as_secs_f64() * 1e3 / messages as f64;
    println!(
        "slide_cost: {messages} messages; per message: script+layout {:.2} ms, damage {:.2} ms, draw {:.2} ms (this thread); process {:.2} ms; {:.1} frames drawn; the slide's {:.1} frames drawn in {:.2} ms",
        per(script),
        per(damage),
        per(draw),
        per(process),
        drawn as f64 / messages as f64,
        slide_drawn as f64 / messages as f64,
        per(slide_draw),
    );
}

#[test]
#[ignore = "a comparison: run it --release, as the doc above says"]
fn slide_pixels() {
    let out = PathBuf::from(std::env::var("SLIDE_PIXELS").expect("SLIDE_PIXELS=<dir>"));
    std::fs::create_dir_all(&out).unwrap();
    let messages: usize = std::env::var("SLIDE_MESSAGES").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let (app, mut gpu) = mounted(&scratch("pixels"));
    let mut lines = Vec::new();
    for m in 1..=messages {
        assert!(app.host.run_command("message"));
        for f in 0..FRAMES_PER_MESSAGE {
            if !frame(&app, &mut gpu) {
                continue;
            }
            let pixels = gpu.readback().expect("the frame's pixels");
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            pixels.hash(&mut h);
            lines.push(format!("message {m} frame {f} {:016x}", h.finish()));
            std::fs::write(out.join(format!("m{m}-f{f:03}.rgba")), &pixels).unwrap();
        }
    }
    std::fs::write(out.join("frames.txt"), lines.join("\n") + "\n").unwrap();
    println!("slide_pixels: {} frames drawn after {messages} messages, in {}", lines.len(), out.display());
}
