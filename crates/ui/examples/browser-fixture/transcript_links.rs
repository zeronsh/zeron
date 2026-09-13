//! Native transcript → Browser coverage, using only the fixture's loopback server.
use super::*;
use gpui::{Entity, MouseButton, PlatformInput, WindowHandle};

fn dispatch(
    window: WindowHandle<shell::Shell>,
    event: PlatformInput,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return super::linux::dispatch(window, event, cx);
    #[cfg(not(target_os = "linux"))]
    {
        window.update(cx, |_, w, cx| {
            w.dispatch_event(event, cx);
        })?;
        Ok(())
    }
}
async fn key(
    window: WindowHandle<shell::Shell>,
    key: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    dispatch(
        window,
        PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse(key)?,
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    )?;
    dispatch(
        window,
        PlatformInput::KeyUp(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key)?,
        }),
        cx,
    )?;
    pause(cx, 100).await;
    window.update(cx, |s, w, cx| {
        eprintln!(
            "link fixture key {key}: focused={:?}, mounted={}",
            w.focused(cx),
            s.fixture_focus_mounted(w, cx)
        )
    })?;
    Ok(())
}
async fn screenshot(
    window: WindowHandle<shell::Shell>,
    output: &std::path::Path,
    name: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    // Synthetic key/focus dispatch updates hit testing synchronously. Request
    // a presented frame too, especially when Wayland's frame loop is idle.
    window.update(cx, |_, w, _| w.on_next_frame(|window, _| window.refresh()))?;
    pause(cx, 250).await;
    capture(output, name)
}

pub(super) async fn exercise(
    window: WindowHandle<shell::Shell>,
    state: Entity<state::AppState>,
    origin: &str,
    output: &std::path::Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let long = format!(
        "{origin}/{}?complete=yes#destination",
        "long-segment/".repeat(35)
    );
    let markdown = include_str!("transcript-links.md")
        .replace("{{origin}}", origin)
        .replace("{{long}}", &long);
    state.update(cx, |s, cx| {
        let entries = serde_json::from_value(serde_json::json!([
            {"id":"fixture-links","role":"assistant","parts":[{"id":"text","kind":"text","text":markdown}],"createdAt":1788900001000_i64,"deviceId":"local","status":"complete"}
        ])).unwrap();
        s.receive_transcript_frame(zeron_doc::TranscriptFrame::Reset { reset: entries }, cx).unwrap();
    });
    pause(cx, 900).await;
    let (position, _) = markdown::render::fixture_link(origin)
        .ok_or_else(|| anyhow::anyhow!("first transcript link was not painted"))?;
    dispatch(
        window,
        PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 900).await;
    screenshot(window, output, "transcript-link-hover", cx).await?;
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 80).await;
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            button: MouseButton::Left,
            position,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 700).await;
    let (id, page) = window
        .update(cx, |s, _, cx| s.fixture_active_browser(cx))?
        .ok_or_else(|| anyhow::anyhow!("click did not open a Browser tab"))?;
    anyhow::ensure!(
        page.read_with(cx, |page, _| page.page.url.as_deref()
            == Some(&format!("{origin}/"))),
        "wrong browser destination"
    );
    let missing_runtime = std::env::var_os("ZERON_LINK_FIXTURE_MISSING_RUNTIME").is_some();
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let state = page.read_with(cx, |p, _| p.page.clone());
        if missing_runtime && state.error.is_some() {
            break;
        }
        if !missing_runtime && state.title == "Fieldnotes" && !state.loading {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "browser did not settle: {state:?}"
        );
        pause(cx, 50).await;
    }
    screenshot(
        window,
        output,
        if missing_runtime {
            "transcript-link-runtime-error"
        } else {
            "transcript-link-browser"
        },
        cx,
    )
    .await?;
    // Focus discloses the original destination even when its label is truncated.
    let (_, focus) = markdown::render::fixture_link(&long)
        .ok_or_else(|| anyhow::anyhow!("long link was not painted"))?;
    window.update(cx, |_, w, cx| w.focus(&focus, cx))?;
    pause(cx, 300).await;
    screenshot(window, output, "transcript-link-focus", cx).await?;
    key(window, "shift-f10", cx).await?;
    screenshot(window, output, "transcript-link-menu", cx).await?;
    key(window, "down", cx).await?;
    key(window, "down", cx).await?;
    key(window, "enter", cx).await?;
    let copied = window.update(cx, |_, _, cx| {
        cx.read_from_clipboard().and_then(|item| item.text())
    })?;
    anyhow::ensure!(
        copied.as_deref() == Some(long.as_str()),
        "copy lost the full destination: {copied:?}"
    );
    // A second activation adds an independent tab and preserves the first page.
    let details = format!("{origin}/two");
    let (_, focus) = markdown::render::fixture_link(&details)
        .ok_or_else(|| anyhow::anyhow!("labeled link not painted"))?;
    window.update(cx, |_, w, cx| w.focus(&focus, cx))?;
    pause(cx, 100).await;
    key(window, "enter", cx).await?;
    let (second, _) = window
        .update(cx, |s, _, cx| s.fixture_active_browser(cx))?
        .ok_or_else(|| anyhow::anyhow!("keyboard did not open a Browser tab"))?;
    anyhow::ensure!(id != second, "second link replaced the first tab");
    let before = page.read_with(cx, |p, _| p.page.url.clone());
    anyhow::ensure!(
        before.as_deref() == Some(&format!("{origin}/")),
        "first page was replaced"
    );
    window.update(cx, |s, w, cx| {
        s.fixture_close_browser(second, w, cx);
        s.fixture_close_browser(id, w, cx);
    })?;
    pause(cx, 250).await;
    anyhow::ensure!(
        !page.read_with(cx, |p, _| p.fixture_native_visible()),
        "closed browser remained visible"
    );
    std::fs::write(
        output.join("result.txt"),
        "PASS: functional original URL copy, pointer/keyboard navigation, independent Browser tabs and close. Destination screenshots captured for manual visual review.\n",
    )?;
    std::fs::write(
        output.join("linux-results.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"fixture":"transcript-links", "passed":true, "missing_runtime":missing_runtime, "platform":std::env::consts::OS}),
        )?,
    )?;
    Ok(())
}
