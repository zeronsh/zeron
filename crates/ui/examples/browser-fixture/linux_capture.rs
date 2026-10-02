//! Share one capture target between screenshots, native input and the recorder.
use super::*;
use anyhow::Context as _;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{future::Future, path::Path, sync::OnceLock, time::Instant};
use x11rb::protocol::xproto::{ConnectionExt as _, MapState};

static CAPTURE_WINDOW: OnceLock<u32> = OnceLock::new();
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) fn window_id() -> anyhow::Result<u32> {
    CAPTURE_WINDOW
        .get()
        .copied()
        .context("capture target has not been initialized")
}

fn parse_window_id(value: &str) -> anyhow::Result<u32> {
    let id = match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => value.parse(),
    }
    .context("invalid ZERON_BROWSER_CAPTURE_WINDOW")?;
    anyhow::ensure!(id != 0, "capture window ID must be nonzero");
    Ok(id)
}

pub(super) async fn prepare(
    window: gpui::WindowHandle<shell::Shell>,
    output: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let native = window.update(cx, |_, w, _| {
        HasWindowHandle::window_handle(w).map(|handle| handle.as_raw())
    })??;
    // A native Wayland surface is captured through the enclosing Weston X11
    // window. Never interpret a Wayland handle as an XID.
    let id = match std::env::var("ZERON_BROWSER_CAPTURE_WINDOW") {
        Ok(value) => parse_window_id(&value)?,
        Err(std::env::VarError::NotPresent) => match native {
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
            _ => anyhow::bail!("Wayland capture requires ZERON_BROWSER_CAPTURE_WINDOW"),
        },
        Err(error) => return Err(error.into()),
    };
    CAPTURE_WINDOW
        .set(id)
        .map_err(|_| anyhow::anyhow!("capture target already initialized"))?;
    std::fs::write(output.join("capture-target.txt"), format!("{id}\n"))?;
    eprintln!(
        "capture startup: pid={} target={id:#x} native={native:?} DISPLAY={:?} WAYLAND_DISPLAY={:?}",
        std::process::id(),
        std::env::var_os("DISPLAY"),
        std::env::var_os("WAYLAND_DISPLAY")
    );

    let executor = cx.background_executor().clone();
    let started = Instant::now();
    // X11 round trips run off the UI thread; the GPUI event loop keeps drawing
    // and handling configure/map events while the target becomes viewable.
    let ready = cx.background_executor().spawn(async move {
        wait_for_window(id, READY_TIMEOUT, || executor.timer(POLL_INTERVAL)).await
    });
    // Also bound an unresponsive X server, not just an unmapped window.
    match futures::future::select(ready, cx.background_executor().timer(READY_TIMEOUT)).await {
        futures::future::Either::Left((result, _)) => result?,
        futures::future::Either::Right(((), _)) => anyhow::bail!(
            "capture window {id:#x} readiness deadline exceeded after {} ms",
            started.elapsed().as_millis()
        ),
    }
    eprintln!(
        "capture ready: target={id:#x} elapsed_ms={}",
        started.elapsed().as_millis()
    );
    // Publish atomically. The shell uses this exact XID for ffmpeg instead of
    // independently discovering a possibly different window by process ID.
    let pending = output.join("capture-window.pending");
    std::fs::write(&pending, format!("{id}\n"))?;
    std::fs::rename(pending, output.join("capture-window.txt"))?;
    Ok(())
}

async fn wait_for_window<F: Future<Output = ()>>(
    id: u32,
    timeout: Duration,
    mut pause: impl FnMut() -> F,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let (connection, _) = x11rb::connect(None).context("connect to capture DISPLAY")?;
    let mut last_state = None;
    loop {
        let attributes = connection
            .get_window_attributes(id)?
            .reply()
            .with_context(|| format!("query capture window {id:#x}"))?;
        let geometry = connection.get_geometry(id)?.reply()?;
        let state = (attributes.map_state, geometry.width, geometry.height);
        if last_state != Some(state) {
            eprintln!(
                "capture probe: target={id:#x} elapsed_ms={} map_state={:?} size={}x{}",
                started.elapsed().as_millis(),
                state.0,
                state.1,
                state.2
            );
            last_state = Some(state);
        }
        if attributes.map_state == MapState::VIEWABLE && geometry.width > 0 && geometry.height > 0 {
            return Ok(());
        }
        anyhow::ensure!(
            started.elapsed() < timeout,
            "capture window {id:#x} not ready after {} ms: map_state={:?}, size={}x{}",
            started.elapsed().as_millis(),
            attributes.map_state,
            geometry.width,
            geometry.height
        );
        pause().await;
    }
}

pub(super) fn diagnostics(output: &Path) {
    let Ok(mut file) = std::fs::File::create(output.join("window-diagnostics.log")) else {
        return;
    };
    let _ = writeln!(
        file,
        "time={:?} pid={} target={:?}",
        std::time::SystemTime::now(),
        std::process::id(),
        CAPTURE_WINDOW.get()
    );
    for name in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "GDK_BACKEND",
        "ZERON_BROWSER_CAPTURE_WINDOW",
    ] {
        let _ = writeln!(file, "{name}={:?}", std::env::var_os(name));
    }
    let pid = std::process::id().to_string();
    let id = CAPTURE_WINDOW.get().map(|id| id.to_string());
    let mut commands = vec![
        vec!["xdotool", "search", "--onlyvisible", "--pid", &pid],
        vec!["xdotool", "search", "--pid", &pid],
        vec!["xwininfo", "-root", "-tree"],
        vec![
            "xprop",
            "-root",
            "_NET_SUPPORTING_WM_CHECK",
            "_NET_CLIENT_LIST",
        ],
    ];
    if let Some(id) = id.as_deref() {
        commands.push(vec!["xwininfo", "-id", id]);
        commands.push(vec![
            "xprop",
            "-id",
            id,
            "_NET_WM_PID",
            "WM_STATE",
            "_NET_WM_STATE",
        ]);
    }
    for command in commands {
        let _ = writeln!(file, "\n$ {}", command.join(" "));
        match std::process::Command::new("timeout")
            .arg("2s")
            .args(&command)
            .output()
        {
            Ok(result) => {
                let _ = writeln!(
                    file,
                    "status={}\nstdout:\n{}\nstderr:\n{}",
                    result.status,
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
            Err(error) => {
                let _ = writeln!(file, "spawn failed: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::{
        connection::Connection,
        protocol::xproto::{CreateWindowAux, WindowClass},
    };

    fn window() -> (x11rb::rust_connection::RustConnection, u32) {
        let (connection, screen) = x11rb::connect(None).unwrap();
        let id = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                connection.setup().roots[screen].root,
                0,
                0,
                100,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new().override_redirect(1),
            )
            .unwrap()
            .check()
            .unwrap();
        (connection, id)
    }

    #[tokio::test]
    #[ignore = "requires an isolated X11 display (run under xvfb-run)"]
    async fn delayed_mapping_without_pid_metadata_becomes_ready() {
        let (connection, id) = window();
        let start = Instant::now();
        // Both futures run on this thread. A blocking polling loop would stop
        // the mapping future from running and fail this test.
        let (ready, ()) = tokio::join!(
            wait_for_window(id, Duration::from_secs(2), || tokio::time::sleep(
                POLL_INTERVAL
            )),
            async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                connection.map_window(id).unwrap().check().unwrap();
            }
        );
        ready.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(200));
    }

    #[tokio::test]
    #[ignore = "requires an isolated X11 display (run under xvfb-run)"]
    async fn permanently_unmapped_window_times_out() {
        let (_connection, id) = window();
        let start = Instant::now();
        let error = wait_for_window(id, Duration::from_millis(200), || {
            tokio::time::sleep(POLL_INTERVAL)
        })
        .await
        .unwrap_err();
        assert!(start.elapsed() >= Duration::from_millis(200));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            error.to_string().contains("map_state=UNMAPPED"),
            "{error:#}"
        );
    }

    #[tokio::test]
    #[ignore = "requires an isolated X11 display (run under xvfb-run)"]
    async fn destroyed_window_reports_query_failure() {
        let (connection, id) = window();
        connection.destroy_window(id).unwrap().check().unwrap();
        let error = wait_for_window(id, Duration::from_secs(2), || {
            tokio::time::sleep(POLL_INTERVAL)
        })
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("query capture window"),
            "{error:#}"
        );
    }

    #[test]
    fn capture_override_accepts_decimal_and_hex_but_rejects_invalid_ids() {
        assert_eq!(parse_window_id("123").unwrap(), 123);
        assert_eq!(parse_window_id("0x7b").unwrap(), 123);
        for value in ["", "0", "0x0", "invalid", "-1"] {
            assert!(parse_window_id(value).is_err(), "{value}");
        }
    }
}
