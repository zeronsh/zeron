//! The stream bench (feature `stream-bench`, off in every shipping build): a
//! recorded reply replayed through the host's real chat path, to measure a
//! real window's CPU with no inspector and no driver in Luau.
//!
//!     ZERON_STREAM_BENCH=<frames.jsonl> zeron-zgx
//!
//! opens ui/stream_bench_fixture.luau (the chat page: `chat.Session` and the
//! composer) in a window of its own. The page opens its chat as any session
//! page does (`store.open` → `chat.open` → `View::open_chat`); where the view
//! would watch the engine's transcript (`View::subscribe`, `WatchDocMessages`),
//! this chat's frames come from the recording instead, each handed to
//! `View::heard_frame`, the watch's own callback: `apply`, then
//! `publish_draft` for an append to the reply being written, or
//! `publish_messages` for a new or finished row, exactly as for an engine's.
//! (examples/stream_bench_record.rs writes the standard recording.)
//!
//! The first frame (the opening tail, a reset) is heard as soon as the
//! replay runs, as a watch's first item; the rest on
//! the window's loop at absolute deadlines, `start + k × every`, so a late
//! frame never pushes the next ones later. A line is logged as it starts and
//! one as it ends (frames, chunks and bytes delivered, the rate they came at,
//! how late they were, the CPU the main thread and the process spent).
//!
//! With it set, the engine is refused (src/engine/mod.rs): no connection, no
//! engine started, nothing but the recording reaching the chat state.
//!
//! - `ZERON_STREAM_BENCH_EVERY_MS`: between frames (25: 40 chunks a second)
//! - `ZERON_STREAM_BENCH_LEAD_MS`: from the reset to the first chunk (1500)
//! - `ZERON_STREAM_BENCH_QUIT=1`: quit a second after the last frame

use super::*;

use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    time::{Duration, Instant},
};

#[allow(dead_code)]
#[path = "stream_bench_record.rs"]
pub(crate) mod record;

#[cfg(test)]
#[path = "stream_bench_tests.rs"]
mod tests;

/// The page it opens, under ui/.
const FIXTURE: &str = "stream_bench_fixture.luau";

type Spawn = Rc<dyn Fn(Pin<Box<dyn Future<Output = ()>>>)>;

/// A bench asked for: the recording and how it's played.
pub(super) struct Bench {
    pub path: PathBuf,
    pub every: Duration,
    pub lead: Duration,
    /// Where its replay runs: the window's tasks.
    pub spawn: Spawn,
    /// What quits the app once it has played (`ZERON_STREAM_BENCH_QUIT`).
    pub quit: Option<Rc<dyn Fn()>>,
    started: Cell<bool>,
}

thread_local! {
    static BENCH: RefCell<Option<Rc<Bench>>> = const { RefCell::new(None) };
}

/// The recording `ZERON_STREAM_BENCH` names, if a bench is asked for.
pub(crate) fn asked() -> Option<PathBuf> {
    std::env::var_os("ZERON_STREAM_BENCH")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// The fixture a bench opens in place of any other screen.
pub(crate) fn screen() -> Option<String> {
    asked().map(|_| FIXTURE.to_owned())
}

fn millis(key: &str, default: u64) -> Duration {
    let ms = std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(default);
    Duration::from_millis(ms)
}

/// A window's chat (`chat::install`): the bench is played on its loop.
pub(super) fn install(cx: &mut WindowContext) {
    let Some(path) = asked() else { return };
    if BENCH.with_borrow(Option::is_some) {
        return;
    }
    let tasks = cx.tasks.clone();
    let windows = cx.windows.clone();
    let quit = std::env::var("ZERON_STREAM_BENCH_QUIT").is_ok_and(|v| v == "1");
    set(Bench {
        path,
        every: millis("ZERON_STREAM_BENCH_EVERY_MS", 25).max(Duration::from_millis(1)),
        lead: millis("ZERON_STREAM_BENCH_LEAD_MS", 1500),
        spawn: Rc::new(move |work| tasks.spawn(work)),
        quit: quit.then(|| Rc::new(move || windows.quit()) as Rc<dyn Fn()>),
        started: Cell::new(false),
    });
}

pub(super) fn set(bench: Bench) {
    BENCH.set(Some(Rc::new(bench)));
}

impl Bench {
    /// One played on `spawn` (the headless test's executor).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn new(path: PathBuf, every: Duration, lead: Duration, spawn: Spawn) -> Self {
        Self {
            path,
            every,
            lead,
            spawn,
            quit: None,
            started: Cell::new(false),
        }
    }
}

/// Whether the bench has `view`'s chat (`View::subscribe`, in place of the
/// engine's watch): its replay starts the first time.
pub(super) fn claims(view: &Rc<View>) -> bool {
    let Some(bench) = BENCH.with_borrow(Clone::clone) else {
        return false;
    };
    if view.open_id().as_deref() != Some(record::CHAT) {
        return false;
    }
    if !bench.started.replace(true) {
        start(view, &bench);
    }
    true
}

/// The recording's frames, one a line (blank lines skipped).
fn load(path: &std::path::Path) -> Result<Vec<Json>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(n, line)| {
            serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", path.display(), n + 1))
        })
        .collect()
}

/// The chunks of a reply's words a frame brings, and their bytes: each
/// append's, and a reply's row coming with its first words (a streaming
/// entry upserted with text; its end, settled, brings none).
fn appended(frame: &Json) -> (usize, usize) {
    let (mut chunks, mut bytes) = (0, 0);
    for add in frame["append"].as_array().into_iter().flatten() {
        chunks += 1;
        bytes += add["text"].as_str().map_or(0, str::len);
    }
    for up in frame["upsert"].as_array().into_iter().flatten() {
        let entry = &up["entry"];
        if entry["status"].as_str() != Some("streaming") {
            continue;
        }
        let words: usize = entry["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["kind"].as_str() == Some("text"))
            .map(|p| p["text"].as_str().map_or(0, str::len))
            .sum();
        if words > 0 {
            chunks += 1;
            bytes += words;
        }
    }
    (chunks, bytes)
}

fn start(view: &Rc<View>, bench: &Bench) {
    let mut frames = match load(&bench.path) {
        Ok(frames) if !frames.is_empty() => frames,
        Ok(_) => {
            return eprintln!(
                "zeron-zgx: stream-bench: {}: no frames",
                bench.path.display()
            );
        }
        Err(error) => return eprintln!("zeron-zgx: stream-bench: {error}"),
    };
    // The chat, as the engine's list would have it: the page shows it ready
    // once its transcript has come.
    CHATS.with(|chats| {
        chats.made(
            record::CHAT,
            json!({ "id": record::CHAT, "title": "Stream bench", "config": { "harness": "claude-code", "model": "opus" } }),
        );
        // Its agent's catalog, listed as an engine's would list it: with
        // none, the composer's model chip stays a loading bar pulsing for
        // as long as the bench runs, which a session's page never shows.
        *chats.harnesses.borrow_mut() = vec![json!({ "id": "claude-code", "name": "Claude Code" })];
        chats
            .models
            .borrow_mut()
            .insert("claude-code".into(), vec![json!({ "id": "opus", "label": "Opus" })]);
        chats.catalog_state.set(CatalogState::Ready);
        chats.publish_catalog();
    });
    // The opening tail first, as a watch's first item.
    let opening = frames[0].get("reset").is_some().then(|| frames.remove(0));
    let (chunks, bytes) = frames
        .iter()
        .map(appended)
        .fold((0, 0), |(c, b), (c2, b2)| (c + c2, b + b2));
    let every = bench.every;
    eprintln!(
        "zeron-zgx: stream-bench: start: {}: {}{} frames ({chunks} chunks, {bytes} bytes) every {} ms ({:.1}/s), from {} ms after the reset",
        bench.path.display(),
        if opening.is_some() {
            "a reset, then "
        } else {
            ""
        },
        frames.len(),
        every.as_millis(),
        1. / every.as_secs_f64(),
        bench.lead.as_millis(),
    );
    let generation = view.generation.get();
    let weak = Rc::downgrade(view);
    let (lead, quit) = (bench.lead, bench.quit.clone());
    (bench.spawn)(Box::pin(async move {
        // (After `subscribe` returns, as a watch's first item comes.)
        if let Some(reset) = opening
            && let Some(view) = weak.upgrade().filter(|v| v.generation.get() == generation)
        {
            view.heard_frame(reset);
        }
        let start = Instant::now() + lead;
        let mut late = Lateness::default();
        let mut cpu = None;
        let (mut delivered, mut chunks, mut bytes) = (0usize, 0usize, 0usize);
        for (k, frame) in frames.into_iter().enumerate() {
            let due = start + every * k as u32;
            zgx::timer::sleep_until(due).await;
            late.add(Instant::now().saturating_duration_since(due));
            let Some(view) = weak.upgrade().filter(|v| v.generation.get() == generation) else {
                eprintln!(
                    "zeron-zgx: stream-bench: stopped after {delivered} frames: the chat was left"
                );
                return;
            };
            cpu.get_or_insert_with(Cpu::now);
            let (c, b) = appended(&frame);
            view.heard_frame(frame);
            delivered += 1;
            chunks += c;
            bytes += b;
        }
        let took = start.elapsed();
        let spent = cpu.map(|c| c.since()).unwrap_or_default();
        eprintln!(
            "zeron-zgx: stream-bench: end: {delivered} frames ({chunks} chunks, {bytes} bytes) in {:.2} s = {:.2} chunks/s \
             (planned {:.2} s); late by {:.2} ms at most, {:.2} ms on average; CPU {}",
            took.as_secs_f64(),
            chunks as f64 / took.as_secs_f64().max(1e-9),
            (every * delivered.saturating_sub(1) as u32).as_secs_f64(),
            late.most.as_secs_f64() * 1e3,
            late.mean().as_secs_f64() * 1e3,
            spent.describe(took),
        );
        if let Some(quit) = quit {
            zgx::timer::sleep(Duration::from_secs(1)).await;
            quit();
        }
    }));
}

#[derive(Default)]
struct Lateness {
    most: Duration,
    sum: Duration,
    n: u32,
}

impl Lateness {
    fn add(&mut self, late: Duration) {
        self.most = self.most.max(late);
        self.sum += late;
        self.n += 1;
    }

    fn mean(&self) -> Duration {
        self.sum.checked_div(self.n).unwrap_or_default()
    }
}

/// CPU time so far: this thread's (the window's loop) and the process's.
#[derive(Clone, Copy, Default)]
struct Cpu {
    thread: Duration,
    process: Duration,
}

impl Cpu {
    #[cfg(unix)]
    fn now() -> Self {
        let clock = |id| {
            let mut t = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: `t` is a valid timespec for the call to fill.
            let ok = unsafe { libc::clock_gettime(id, &mut t) } == 0;
            if ok {
                Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
            } else {
                Duration::ZERO
            }
        };
        Self {
            thread: clock(libc::CLOCK_THREAD_CPUTIME_ID),
            process: clock(libc::CLOCK_PROCESS_CPUTIME_ID),
        }
    }

    #[cfg(not(unix))]
    fn now() -> Self {
        Self::default()
    }

    fn since(self) -> Self {
        let now = Self::now();
        Self {
            thread: now.thread.saturating_sub(self.thread),
            process: now.process.saturating_sub(self.process),
        }
    }

    fn describe(self, over: Duration) -> String {
        if cfg!(not(unix)) {
            return "not measured here".into();
        }
        let share = |d: Duration| 100. * d.as_secs_f64() / over.as_secs_f64().max(1e-9);
        format!(
            "main thread {:.2} s ({:.1}% of a core), process {:.2} s ({:.1}%)",
            self.thread.as_secs_f64(),
            share(self.thread),
            self.process.as_secs_f64(),
            share(self.process),
        )
    }
}
