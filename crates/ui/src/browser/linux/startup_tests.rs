//! A gated helper exercises the actual navigation path without a GTK display.
use super::*;
use crate::browser::{BrowserContext, BrowserSurface};
use gpui::TestAppContext;
use std::{
    path::Path,
    time::{Duration, Instant},
};

struct DelayedHelper(tempfile::TempDir);

impl DelayedHelper {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("helper.py");
        std::fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, pathlib, struct, sys, time
root = pathlib.Path(__file__).parent
def packet(kind, page, value):
    data = value.encode()
    sys.stdout.buffer.write(struct.pack('<cII', kind, page, len(data)) + data)
    sys.stdout.buffer.flush()
(root / 'started').touch()
while not (root / 'release').exists():
    if not root.exists():
        sys.exit(1)
    time.sleep(.005)
if (root / 'fail').exists():
    packet(b'E', 0, 'Synthetic storage initialization failure')
    sys.exit(1)
packet(b'R', 0, '')
while True:
    header = sys.stdin.buffer.read(4)
    if not header:
        break
    message = json.loads(sys.stdin.buffer.read(struct.unpack('<I', header)[0]))
    if not root.exists():
        break
    with (root / 'commands').open('a') as log:
        log.write(json.dumps(message) + '\n')
    if message['cmd'] == 'close':
        (root / 'closed').touch()
    if message['cmd'] == 'load':
        packet(b'S', message['id'], json.dumps(dict(url=message['url'], title='Loaded',
            loading=False, can_back=False, can_forward=False, error=None)))
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn context(&self) -> BrowserContext {
        BrowserContext {
            data: BrowserData {
                helper_override: Some(self.path().join("helper.py")),
                ..Default::default()
            },
        }
    }

    fn release(&self) {
        std::fs::write(self.path().join("release"), "").unwrap();
    }

    fn loaded_urls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path().join("commands"))
            .unwrap()
            .lines()
            .filter_map(|line| {
                let command: Value = serde_json::from_str(line).unwrap();
                (command["cmd"] == "load").then(|| command["url"].as_str().unwrap().to_owned())
            })
            .collect()
    }
}

fn init(cx: &mut TestAppContext) {
    // These regressions intentionally use real process I/O and external wakes.
    cx.background_executor.allow_parking();
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::composer::init(cx, Default::default());
        cx.set_global(crate::theme::Theme::default());
    });
}

fn until(cx: &mut TestAppContext, mut condition: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if condition(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "helper/UI completion timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
fn delayed_startup_keeps_both_windows_responsive_and_loads_latest_address(cx: &mut TestAppContext) {
    init(cx);
    let helper = DelayedHelper::new();
    let context = helper.context();
    let first = cx.add_window(|window, cx| BrowserSurface::new(context.clone(), false, window, cx));
    let second = cx.add_window(|window, cx| BrowserSurface::new(context, false, window, cx));
    let before = Instant::now();
    first
        .update(cx, |browser, window, cx| {
            browser.navigate("https://first.test/", window, cx)
        })
        .unwrap();
    assert!(
        before.elapsed() < Duration::from_secs(1),
        "navigation blocked on helper startup"
    );
    until(cx, |_| helper.path().join("started").exists());

    // The helper is still gated and holds the shared profile's worker lock.
    // A second window must not wait for that lock on the UI thread either.
    let before = Instant::now();
    second
        .update(cx, |browser, window, cx| {
            browser.navigate("https://second.test/", window, cx)
        })
        .unwrap();
    first
        .update(cx, |browser, window, cx| {
            assert!(browser.native.is_none());
            assert!(browser.native_startup.is_some());
            assert!(browser.page.loading);
            browser.navigate("https://latest.test/", window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        before.elapsed() < Duration::from_secs(1),
        "another window blocked during startup"
    );

    helper.release();
    until(cx, |cx| {
        first
            .update(cx, |b, _, _| b.page.title == "Loaded")
            .unwrap()
            && second
                .update(cx, |b, _, _| b.page.title == "Loaded")
                .unwrap()
    });
    let mut urls = helper.loaded_urls();
    urls.sort();
    assert_eq!(urls, ["https://latest.test/", "https://second.test/"]);
    first
        .update(cx, |b, _, _| assert!(b.native_startup.is_none()))
        .unwrap();
}

#[gpui::test]
fn delayed_startup_error_is_delivered_to_the_page_and_can_be_retried(cx: &mut TestAppContext) {
    init(cx);
    let helper = DelayedHelper::new();
    std::fs::write(helper.path().join("fail"), "").unwrap();
    let browser =
        cx.add_window(|window, cx| BrowserSurface::new(helper.context(), false, window, cx));
    browser
        .update(cx, |b, w, cx| b.navigate("https://retry.test/", w, cx))
        .unwrap();
    until(cx, |_| helper.path().join("started").exists());
    browser
        .update(cx, |b, _, _| {
            assert!(b.page.loading);
            assert!(b.page.error.is_none());
        })
        .unwrap();
    helper.release();
    until(cx, |cx| {
        browser
            .update(cx, |b, _, _| b.page.error.is_some())
            .unwrap()
    });
    browser
        .update(cx, |b, _, _| {
            assert!(!b.page.loading);
            assert!(b.native.is_none());
            assert!(b.native_startup.is_none());
            assert!(
                b.page
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("Synthetic storage initialization failure")
            );
        })
        .unwrap();
    std::fs::remove_file(helper.path().join("fail")).unwrap();
    browser
        .update(cx, |b, w, cx| b.navigate("https://retry.test/", w, cx))
        .unwrap();
    until(cx, |cx| {
        browser
            .update(cx, |b, _, _| {
                b.page.title == "Loaded" && b.page.error.is_none()
            })
            .unwrap()
    });
}

#[gpui::test]
fn closing_during_startup_cancels_attachment_without_blocking(cx: &mut TestAppContext) {
    init(cx);
    let helper = DelayedHelper::new();
    let browser =
        cx.add_window(|window, cx| BrowserSurface::new(helper.context(), false, window, cx));
    browser
        .update(cx, |b, w, cx| b.navigate("https://closed.test/", w, cx))
        .unwrap();
    until(cx, |_| helper.path().join("started").exists());
    let before = Instant::now();
    browser.update(cx, |b, _, cx| b.close(cx)).unwrap();
    cx.run_until_parked();
    assert!(
        before.elapsed() < Duration::from_secs(1),
        "closing waited for startup"
    );
    helper.release();
    until(cx, |_| helper.path().join("closed").exists());
    browser
        .update(cx, |b, _, _| {
            assert!(b.native.is_none(), "late completion reopened a closed tab");
            assert!(b.native_startup.is_none());
        })
        .unwrap();
    browser
        .update(cx, |b, w, cx| b.navigate("https://reopened.test/", w, cx))
        .unwrap();
    until(cx, |cx| {
        browser
            .update(cx, |b, _, _| b.page.title == "Loaded")
            .unwrap()
    });
    assert_eq!(helper.loaded_urls(), ["https://reopened.test/"]);
}

#[test]
fn concurrent_profiles_can_extract_the_helper_into_a_cold_cache() {
    let root = tempfile::tempdir().unwrap();
    let barrier = std::sync::Barrier::new(8);
    let paths = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    extract_helper(root.path()).unwrap()
                })
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(paths.iter().all(|path| path == &paths[0]));
    assert_eq!(
        std::fs::read(&paths[0]).unwrap(),
        include_bytes!(concat!(env!("OUT_DIR"), "/zeron-webkit"))
    );
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        1,
        "staging files were left behind"
    );
}
