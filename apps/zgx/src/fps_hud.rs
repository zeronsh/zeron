//! A frame stats panel over the main window in the style of Metal's
//! performance HUD, for any build: `ZERON_FPS_HUD=1`. Unset, nothing here
//! runs and frames aren't traced. zgx shows its frames through surfaces of
//! its own, not `CAMetalLayer` drawables, so Metal's own HUD never appears.
//!
//! The host reads the frames zgx keeps for devtools (`zgx::trace`), the
//! window's display and the process's footprint;
//! `ui.call_sync("debug.frame_stats")` answers them and
//! ui/screens/fps_hud.luau draws them.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

/// A gap past this between frames is the window resting, not a frame's.
const IDLE_MS: f64 = 250.;
/// Frames the graph shows.
const BARS: usize = 120;
/// How long the FPS range looks back.
const RANGE: Duration = Duration::from_secs(10);
/// How often the panel is answered at rest (no frames of the app's), so
/// its figures settle.
const REST: Duration = Duration::from_secs(1);

/// Whether the panel is asked for.
pub fn asked() -> bool {
    std::env::var_os("ZERON_FPS_HUD").is_some_and(|v| !v.is_empty() && v != "0")
}

pub fn install(cx: &WindowContext, host: &ScriptHost) {
    zgx::trace::set_enabled(true);
    host.shared("debug.fps_hud", true);
    let displays = cx.window_info.displays.clone();
    let current = cx.window_info.current_display.clone();
    let bounds = cx.window_info.bounds.clone();
    let history = RefCell::new(History::default());
    // Asked at every tick; answered (not nil) when the app drew a frame
    // since, as Metal's HUD updates with each frame presented, and once a
    // second at rest. Its own answers draw frames of the panel alone: those
    // count for nothing, or it would keep the window drawing.
    host.function("debug.frame_stats", move |_| {
        let mut history = history.borrow_mut();
        let newest = zgx::trace::frames(1).first().map_or(0, |f| f.index);
        let drew = newest > history.seen
            && zgx::trace::frames(zgx::trace::MOST_KEPT)
                .iter()
                .filter(|f| f.index > history.seen)
                .any(|f| !hud_only(f));
        history.seen = newest;
        let due = history.answered.is_none_or(|at| at.elapsed() >= REST);
        if !drew && !due {
            return Value::Nil;
        }
        history.answered = Some(Instant::now());
        let display = current.get().and_then(|i| displays.get().into_iter().nth(i));
        let refresh = display.as_ref().and_then(|d| d.refresh_rate_hz).filter(|hz| *hz > 0.).unwrap_or(60.);
        let scale = display.as_ref().map_or(1., |d| d.scale_factor);
        let size = bounds.get().size;
        stats(&mut history, refresh, scale, size)
    });
}

/// What earlier answers saw: the FPS range's samples and the footprint's peak.
#[derive(Default)]
struct History {
    fps: VecDeque<(Instant, f64)>,
    peak_mb: f64,
    /// The newest frame seen, and when the panel was last answered.
    seen: u64,
    answered: Option<Instant>,
}

/// Whether a frame drew only this panel: no input, and nothing ran but the
/// panel's own (its script's effects).
fn hud_only(frame: &zgx::trace::Frame) -> bool {
    frame.events == 0
        && !frame.runs.is_empty()
        && frame
            .runs
            .iter()
            .all(|run| run.name.as_deref().is_some_and(|name| name.contains("fps_hud.luau")))
}

/// A frame the display showed, with the work of the frames it covered.
#[derive(Debug, PartialEq)]
struct Shown {
    /// When the first frame it covered ended: a refresh shows one frame.
    began_ms: f64,
    ended_ms: f64,
    cpu_ms: f64,
    gpu_ms: Option<f64>,
}

/// The frames the display showed, of `frames` (when each ended, its CPU and
/// GPU ms), as Metal's HUD counts presents: WindowServer composes once a
/// refresh, so a frame handed over less than half a refresh before the next
/// is covered by it, and its work counts toward the next; frames covering
/// each other span a refresh at most.
// ponytail: zgx doesn't tell the app the refresh's phase or when a surface
// was shown, so this guesses from the gaps; frames steadily under a refresh
// but over half of it apart (input past the display's rate) still count each.
fn shown(frames: impl IntoIterator<Item = (f64, f64, Option<f64>)>, budget: f64) -> Vec<Shown> {
    let mut out: Vec<Shown> = Vec::new();
    for (ended_ms, cpu_ms, gpu_ms) in frames {
        match out.last_mut() {
            // The last frame was covered by this one, which takes its work.
            Some(prev) if ended_ms - prev.ended_ms < budget / 2. && ended_ms - prev.began_ms < budget => {
                prev.ended_ms = ended_ms;
                prev.cpu_ms += cpu_ms;
                prev.gpu_ms = match (prev.gpu_ms, gpu_ms) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
            }
            _ => out.push(Shown { began_ms: ended_ms, ended_ms, cpu_ms, gpu_ms }),
        }
    }
    out
}

fn stats(history: &mut History, refresh: f64, scale: f64, size: (u32, u32)) -> Value {
    let budget = 1000. / refresh;
    let now = zgx::trace::now_ms();
    // A frame's CPU: its phases but the waits (pacing's, and in `present`
    // the GPU's, which `gpu` counts).
    let cpu_ms = |f: &zgx::trace::Frame| {
        let micros: f32 = f
            .phases
            .iter()
            .filter(|(name, ..)| !matches!(*name, "wait" | "present"))
            .map(|(.., us)| us)
            .sum();
        f64::from(micros) / 1e3
    };
    let recent = shown(
        zgx::trace::frames(zgx::trace::MOST_KEPT)
            .iter()
            .filter(|f| !hud_only(f))
            .map(|f| (f.ended_ms, cpu_ms(f), f.gpu_micros.map(|us| f64::from(us) / 1e3))),
        budget,
    );
    let second: Vec<&Shown> = recent.iter().filter(|f| now - f.ended_ms <= 1000.).collect();
    let gaps: Vec<f64> = second
        .windows(2)
        .map(|w| w[1].ended_ms - w[0].ended_ms)
        .filter(|gap| *gap <= IDLE_MS)
        .collect();
    let cpu: Vec<f64> = second.iter().map(|f| f.cpu_ms).collect();
    let gpu: Vec<f64> = second.iter().filter_map(|f| f.gpu_ms).collect();

    let fps = second.len() as f64;
    let at = Instant::now();
    history.fps.push_back((at, fps));
    while history.fps.front().is_some_and(|(t, _)| at.duration_since(*t) > RANGE) {
        history.fps.pop_front();
    }
    let fps_range: Vec<f64> = history.fps.iter().map(|(_, f)| *f).collect();
    let mem = footprint_mb();
    history.peak_mb = history.peak_mb.max(mem);

    // The graph: the last frames' gaps and GPU times, oldest first, a rest
    // drawn as the graph's top.
    let tail = &recent[recent.len().saturating_sub(BARS + 1)..];
    let line = |values: Vec<f64>| Value::List(values.into_iter().map(Value::Num).collect::<Vec<_>>().into());
    let pre_line = line(tail.windows(2).map(|w| (w[1].ended_ms - w[0].ended_ms).min(IDLE_MS)).collect());
    let gpu_line = line(tail[1.min(tail.len())..].iter().map(|f| f.gpu_ms.unwrap_or(0.)).collect());

    let num = Value::Num;
    let range = |values: &[f64]| Value::List(vec![num(min(values)), num(max(values))].into());
    Value::record([
        ("device", Value::from(device().as_str())),
        ("width", num(f64::from(size.0))),
        ("height", num(f64::from(size.1))),
        ("scale", num(scale)),
        ("path", Value::from(present_path())),
        ("refresh", num(refresh)),
        ("budget", num(budget)),
        ("fps", num(fps)),
        ("fps_range", range(&fps_range)),
        ("pre", num(mean(&gaps))),
        ("pre_range", range(&gaps)),
        ("gpu", num(mean(&gpu))),
        ("gpu_range", range(&gpu)),
        ("cpu", num(mean(&cpu))),
        ("cpu_range", range(&cpu)),
        ("mem", num(mem)),
        ("mem_peak", num(history.peak_mb)),
        ("pre_line", pre_line),
        ("gpu_line", gpu_line),
    ])
}

/// How zgx shows the window's frames (zgx-gpu's `ZGX_PRESENT_DRAWABLE`).
fn present_path() -> &'static str {
    match std::env::var("ZGX_PRESENT_DRAWABLE").as_deref() {
        Ok("auto") => "Auto",
        Ok(v) if !v.is_empty() && v != "0" => "Drawable",
        _ => "Surfaces",
    }
}

/// The chip's name ("Apple M1 Pro"), read once.
fn device() -> String {
    thread_local! {
        static NAME: String = sysctl_string(c"machdep.cpu.brand_string").unwrap_or_else(|| "GPU".into());
    }
    NAME.with(Clone::clone)
}

#[cfg(target_os = "macos")]
fn sysctl_string(name: &std::ffi::CStr) -> Option<String> {
    let mut len = 0usize;
    // SAFETY: A size query (null buffer), then a read into a buffer of that size.
    unsafe {
        if libc::sysctlbyname(name.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        buf.truncate(len.saturating_sub(1));
        String::from_utf8(buf).ok()
    }
}

#[cfg(not(target_os = "macos"))]
fn sysctl_string(_: &std::ffi::CStr) -> Option<String> {
    None
}

/// The process's footprint in MB: what Activity Monitor shows as Memory.
#[cfg(target_os = "macos")]
fn footprint_mb() -> f64 {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: The buffer is a `rusage_info_v2`, the flavor asked for.
    let ok = unsafe {
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, (&raw mut info).cast())
    } == 0;
    if ok { info.ri_phys_footprint as f64 / 1e6 } else { 0. }
}

#[cfg(not(target_os = "macos"))]
fn footprint_mb() -> f64 {
    0.
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() { 0. } else { values.iter().sum::<f64>() / values.len() as f64 }
}

fn min(values: &[f64]) -> f64 {
    values.iter().copied().reduce(f64::min).unwrap_or(0.)
}

fn max(values: &[f64]) -> f64 {
    values.iter().copied().reduce(f64::max).unwrap_or(0.)
}

#[cfg(test)]
mod tests {
    #[test]
    fn its_figures_read_the_frames_right() {
        assert_eq!(super::mean(&[2., 4.]), 3.);
        assert_eq!(super::mean(&[]), 0.);
        assert_eq!((super::min(&[3., 1., 2.]), super::max(&[3., 1., 2.])), (1., 3.));
        assert_eq!((super::min(&[]), super::max(&[])), (0., 0.));
        assert!(super::footprint_mb() > 0. || cfg!(not(target_os = "macos")));
    }

    #[test]
    fn a_frame_covered_within_a_refresh_counts_toward_the_next() {
        let at = |shown: Vec<super::Shown>| shown.iter().map(|s| s.ended_ms).collect::<Vec<_>>();
        let budget = 1000. / 60.;
        // A bounce: input frames a refresh apart, each with one 0.1 ms after.
        let bounce = [(0., 1., Some(1.)), (0.1, 0.5, None), (16.7, 1., None), (16.8, 0.5, Some(2.)), (33.4, 1., None)];
        let shown = super::shown(bounce, budget);
        assert_eq!(at(shown), [0.1, 16.8, 33.4]);
        let shown = super::shown(bounce, budget);
        assert_eq!((shown[0].cpu_ms, shown[0].gpu_ms, shown[1].gpu_ms), (1.5, Some(1.), Some(2.)));
        // Frames 5 ms apart don't cover each other past a refresh.
        let steady = (0..8).map(|i| (f64::from(i) * 5., 0., None));
        assert_eq!(at(super::shown(steady, budget)), [15., 35.]);
    }
}
