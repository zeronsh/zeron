use super::*;
use gpui::{
    AnyWindowHandle, Entity, MouseButton, Pixels, PlatformInput, Point, WindowHandle, point,
};
use zeron_ui::browser::BrowserSurface;

async fn eval(
    page: &Entity<BrowserSurface>,
    script: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<serde_json::Value> {
    page.read_with(cx, |b, _| b.fixture_eval(script));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(value) = page.read_with(cx, |b, _| b.fixture_linux_evaluation()) {
            return Ok(value);
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "JavaScript evaluation timed out: {script}"
        );
        pause(cx, 20).await;
    }
}
pub(super) fn dispatch(
    window: WindowHandle<shell::Shell>,
    event: PlatformInput,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    if std::env::var_os("ZERON_BROWSER_NATIVE_POINTER").is_some() {
        let pointer = match &event {
            PlatformInput::MouseDown(e) => Some((e.position, Some(("mousedown", e.button)))),
            PlatformInput::MouseUp(e) => Some((e.position, Some(("mouseup", e.button)))),
            PlatformInput::MouseMove(e) => Some((e.position, None)),
            _ => None,
        };
        if let Some((position, action)) = pointer {
            let id = if let Ok(id) = std::env::var("ZERON_BROWSER_CAPTURE_WINDOW") {
                id
            } else {
                let ids = std::process::Command::new("xdotool")
                    .args([
                        "search",
                        "--onlyvisible",
                        "--pid",
                        &std::process::id().to_string(),
                    ])
                    .output()?;
                String::from_utf8(ids.stdout)?
                    .lines()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("fixture window not found"))?
                    .to_owned()
            };
            let scale = AnyWindowHandle::from(window).update(cx, |_, w, _| w.scale_factor())?;
            let mut cmd = std::process::Command::new("xdotool");
            cmd.args([
                "mousemove",
                "--window",
                &id,
                &((f32::from(position.x) * scale) as i32).to_string(),
                &((f32::from(position.y) * scale) as i32).to_string(),
            ]);
            if let Some((kind, button)) = action {
                let button = match button {
                    MouseButton::Left => "1",
                    MouseButton::Middle => "2",
                    MouseButton::Right => "3",
                    _ => "1",
                };
                cmd.args([kind, button]);
            }
            anyhow::ensure!(cmd.status()?.success(), "native pointer injection failed");
            return Ok(());
        }
    }
    AnyWindowHandle::from(window).update(cx, |_, w, cx| {
        w.dispatch_event(event, cx);
    })?;
    Ok(())
}
fn click(
    window: WindowHandle<shell::Shell>,
    position: Point<Pixels>,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            position,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )
}
pub async fn exercise(
    window: WindowHandle<shell::Shell>,
    page: Entity<BrowserSurface>,
    output: &std::path::Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    eval(&page,"document.body.insertAdjacentHTML('afterbegin', `<div id='browser-blur-grid' style='height:140px;background:repeating-conic-gradient(#172f25 0% 25%,#f5f0df 0% 50%) 0 0/16px 16px'></div><input id='browser-input' style='height:32px;width:200px' placeholder='Browser input'><button id='browser-button' onclick='window.browserClicks=(window.browserClicks||0)+1'>Click</button>`); window.browserClicks=0; document.body.addEventListener('pointerdown',()=>window.pagePresses=(window.pagePresses||0)+1); let live=document.createElement('div');live.style='position:fixed;bottom:12px;right:12px;background:#29483b;color:white;padding:8px;border-radius:6px;font:12px monospace';document.body.append(live);window.browserFrames=0;function frame(){live.textContent='LIVE '+(++window.browserFrames);requestAnimationFrame(frame)}frame();true",cx).await?;
    pause(cx, 400).await;
    let bounds = page.read_with(cx, |b, _| b.fixture_linux_bounds());
    let input=eval(&page,"(()=>{let r=document.getElementById('browser-input').getBoundingClientRect();return [r.x+20,r.y+15]})()",cx).await?;
    click(
        window,
        bounds.origin
            + point(
                px(input[0].as_f64().unwrap() as f32),
                px(input[1].as_f64().unwrap() as f32),
            ),
        cx,
    )?;
    pause(cx, 100).await;
    for c in "hello linux".chars() {
        let key = if c == ' ' {
            "space".into()
        } else {
            c.to_string()
        };
        let stroke = gpui::Keystroke {
            key,
            key_char: Some(c.to_string()),
            modifiers: Default::default(),
        };
        dispatch(
            window,
            PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: stroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )?;
        dispatch(
            window,
            PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke: stroke }),
            cx,
        )?;
        pause(cx, 35).await;
    }
    anyhow::ensure!(
        eval(&page, "document.getElementById('browser-input').value", cx).await? == "hello linux",
        "GPUI keyboard input did not reach WebKit"
    );
    if let Ok(id) = std::env::var("ZERON_BROWSER_CAPTURE_WINDOW") {
        let scale = AnyWindowHandle::from(window).update(cx, |_, w, _| w.scale_factor())?;
        let x = ((f32::from(bounds.origin.x) + input[0].as_f64().unwrap() as f32) * scale) as i32;
        let y = ((f32::from(bounds.origin.y) + input[1].as_f64().unwrap() as f32) * scale) as i32;
        std::process::Command::new("xdotool")
            .args([
                "mousemove",
                "--window",
                &id,
                &x.to_string(),
                &y.to_string(),
                "click",
                "1",
            ])
            .status()?;
        pause(cx, 200).await;
        // Restore caret to end after the real seat-focus click.
        eval(&page,"(()=>{let e=document.getElementById('browser-input');e.setSelectionRange(e.value.length,e.value.length);return true})()",cx).await?;
    }
    cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(" — café 日本語".into())));
    pause(cx, 200).await;
    let paste = gpui::Keystroke::parse("ctrl-v")?;
    dispatch(
        window,
        PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke: paste,
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    )?;
    pause(cx, 150).await;
    anyhow::ensure!(
        eval(&page, "document.getElementById('browser-input').value", cx).await?
            == "hello linux — café 日本語",
        "Unicode clipboard paste failed: {:?}",
        eval(&page, "document.getElementById('browser-input').value", cx).await?
    );
    eval(
        &page,
        "document.getElementById('browser-input').select();true",
        cx,
    )
    .await?;
    dispatch(
        window,
        PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("ctrl-c")?,
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    )?;
    pause(cx, 150).await;
    anyhow::ensure!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|v| v.text()))
            .as_deref()
            == Some("hello linux — café 日本語"),
        "browser copy did not reach app clipboard"
    );
    eval(&page,"(()=>{let e=document.getElementById('browser-input');e.setSelectionRange(e.value.length,e.value.length);return true})()",cx).await?;
    window.update(cx, |_, w, cx| {
        page.update(cx, |b, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(b, None, "にほ", None, w, cx)
        })
    })?;
    pause(cx, 100).await;
    window.update(cx, |_, w, cx| {
        page.update(cx, |b, cx| {
            gpui::EntityInputHandler::replace_and_mark_text_in_range(b, None, "日本語", None, w, cx)
        })
    })?;
    pause(cx, 100).await;
    capture(output, "linux-ime-dark")?;
    window.update(cx, |_, w, cx| {
        page.update(cx, |b, cx| {
            gpui::EntityInputHandler::replace_text_in_range(b, None, "日本語", w, cx)
        })
    })?;
    pause(cx, 150).await;
    anyhow::ensure!(
        eval(&page, "document.getElementById('browser-input').value", cx).await?
            == "hello linux — café 日本語日本語",
        "IME composition did not commit correctly"
    );
    eval(&page,"document.getElementById('browser-button').insertAdjacentHTML('afterend','<select id=browser-select><option value=a>Alpha</option><option value=b>Beta</option></select>');true",cx).await?;
    let select=eval(&page,"(()=>{let r=document.getElementById('browser-select').getBoundingClientRect();return [r.x+10,r.y+10]})()",cx).await?;
    click(
        window,
        bounds.origin
            + point(
                px(select[0].as_f64().unwrap() as f32),
                px(select[1].as_f64().unwrap() as f32),
            ),
        cx,
    )?;
    pause(cx, 250).await;
    anyhow::ensure!(
        page.read_with(cx, |b, _| b.fixture_linux_menu_open()),
        "HTML select did not open a GPUI menu"
    );
    capture(output, "linux-select-dark")?;
    for key in ["down", "enter"] {
        dispatch(
            window,
            PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse(key)?,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        )?;
        pause(cx, 100).await;
    }
    anyhow::ensure!(
        eval(&page, "document.getElementById('browser-select').value", cx).await? == "b",
        "select keyboard navigation failed"
    );
    let context = bounds.origin + point(px(300.), px(330.));
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            position: context,
            button: MouseButton::Right,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            position: context,
            button: MouseButton::Right,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 250).await;
    anyhow::ensure!(
        page.read_with(cx, |b, _| b.fixture_linux_menu_open()),
        "context menu did not open in GPUI"
    );
    capture(output, "linux-context-dark")?;
    dispatch(
        window,
        PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("escape")?,
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    )?;
    pause(cx, 150).await;
    anyhow::ensure!(
        !page.read_with(cx, |b, _| b.fixture_linux_menu_open()),
        "context menu did not dismiss"
    );
    for _ in 0..5 {
        for pos in [
            point(bounds.origin.x + px(25.), px(20.)),
            point(bounds.origin.x + px(75.), px(56.)),
            point(bounds.origin.x + px(180.), px(20.)),
        ] {
            dispatch(
                window,
                PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: pos,
                    ..Default::default()
                }),
                cx,
            )?;
            pause(cx, 120).await;
            anyhow::ensure!(
                page.read_with(cx, |b, _| b.fixture_native_visible()),
                "hover hid the browser"
            );
        }
    }
    dispatch(
        window,
        PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position: point(bounds.origin.x + px(75.), px(56.)),
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 800).await;
    capture(output, "linux-tooltip-dark")?;
    let button=eval(&page,"(()=>{let r=document.getElementById('browser-button').getBoundingClientRect();return [r.x+15,r.y+10]})()",cx).await?;
    click(
        window,
        bounds.origin
            + point(
                px(button[0].as_f64().unwrap() as f32),
                px(button[1].as_f64().unwrap() as f32),
            ),
        cx,
    )?;
    pause(cx, 100).await;
    anyhow::ensure!(
        eval(&page, "window.browserClicks", cx).await? == 1,
        "page button did not activate"
    );
    dispatch(
        window,
        PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
            position: bounds.origin + point(px(300.), px(300.)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-250.))),
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 250).await;
    anyhow::ensure!(
        eval(&page, "scrollY", cx).await?.as_f64().unwrap() > 50.,
        "scroll did not reach page"
    );
    eval(&page, "scrollTo(0,0);true", cx).await?;
    pause(cx, 200).await;
    let start = point(bounds.origin.x - px(2.), bounds.origin.y + px(100.));
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    let mut widths = Vec::new();
    for delta in [
        20., 60., 100., 140., 100., 60., 20., -20., -60., -100., -140., -100., -60., -20., 0.,
    ] {
        dispatch(
            window,
            PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: start + point(px(delta), px(0.)),
                pressed_button: Some(MouseButton::Left),
                ..Default::default()
            }),
            cx,
        )?;
        pause(cx, 100).await;
        anyhow::ensure!(
            cx.update(|cx| cx.has_active_drag()),
            "resize failed to start"
        );
        widths.push(f32::from(
            page.read_with(cx, |b, _| b.fixture_linux_bounds().size.width),
        ));
        anyhow::ensure!(
            page.read_with(cx, |b, _| b.fixture_native_visible()),
            "resizing hid browser"
        );
    }
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            position: start,
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
        cx,
    )?;
    pause(cx, 400).await;
    anyhow::ensure!(
        widths.iter().copied().fold(f32::MIN, f32::max)
            - widths.iter().copied().fold(f32::MAX, f32::min)
            > 150.,
        "resize did not change viewport"
    );
    let width = eval(&page, "innerWidth", cx).await?.as_f64().unwrap();
    let layout = f32::from(page.read_with(cx, |b, _| b.fixture_linux_bounds().size.width));
    anyhow::ensure!(
        (width - layout as f64).abs() < 2.,
        "CSS viewport {width} differs from GPUI {layout}"
    );
    capture(output, "linux-resize-dark")?;
    for _ in 0..4 {
        window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(false, cx))?;
        pause(cx, 400).await;
        anyhow::ensure!(
            page.read_with(cx, |b, _| b.fixture_native_visible()),
            "left sidebar lost browser"
        );
    }
    for _ in 0..3 {
        window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
        pause(cx, 60).await;
        window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
        pause(cx, 400).await;
        anyhow::ensure!(
            page.read_with(cx, |b, _| b.fixture_native_visible()),
            "interrupted sidebar lost browser"
        );
    }
    window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
    pause(cx, 400).await;
    anyhow::ensure!(
        !page.read_with(cx, |b, _| b.fixture_native_visible()),
        "closed sidebar kept browser active"
    );
    window.update(cx, |s, _, cx| s.fixture_toggle_sidebar(true, cx))?;
    pause(cx, 400).await;
    anyhow::ensure!(
        page.read_with(cx, |b, _| b.fixture_native_visible()),
        "reopened sidebar lost browser"
    );
    cx.update(|cx| appearance::set_surface(zeron_theme::SurfacePreference::Frosted, cx));
    capture(output, "browser-blur-baseline-dark")?;
    window.update(cx, |s, _, cx| s.fixture_browser_menu(true, cx))?;
    pause(cx, 700).await;
    let presses = eval(&page, "window.pagePresses||0", cx).await?;
    let outside =
        page.read_with(cx, |b, _| b.fixture_linux_bounds().origin) + point(px(350.), px(300.));
    click(window, outside, cx)?;
    // Dismissal includes an exit animation and a deferred repaint. Wait for
    // the real popup lifecycle instead of racing its 120ms removal timer.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while window.update(cx, |s, _, _| s.fixture_browser_menu_mounted())? {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "outside click did not dismiss browser menu"
        );
        pause(cx, 20).await;
    }
    pause(cx, 100).await;
    anyhow::ensure!(
        eval(&page, "window.pagePresses||0", cx).await? == presses,
        "outside menu click leaked into browser"
    );
    click(window, outside, cx)?;
    pause(cx, 150).await;
    anyhow::ensure!(
        eval(&page, "window.pagePresses||0", cx).await?.as_u64()
            == Some(presses.as_u64().unwrap() + 1),
        "browser input did not return after menu dismissal"
    );
    window.update(cx, |s, _, cx| s.fixture_browser_menu(true, cx))?;
    pause(cx, 500).await;
    capture(output, "browser-blur-dark")?;
    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
    pause(cx, 500).await;
    capture(output, "browser-blur-light")?;
    eval(
        &page,
        "document.getElementById('browser-blur-grid').style.background='#263d35'",
        cx,
    )
    .await?;
    pause(cx, 300).await;
    capture(output, "browser-blur-solid-light")?;
    cx.update(|cx| appearance::set_surface(zeron_theme::SurfacePreference::Opaque, cx));
    pause(cx, 300).await;
    capture(output, "browser-menu-opaque")?;
    let bounds = page.read_with(cx, |b, _| b.fixture_linux_bounds());
    let viewport =
        AnyWindowHandle::from(window).update(cx, |_, w, _| f32::from(w.viewport_size().width))?;
    super::validate_blur(
        output,
        (f32::from(bounds.origin.x) as f64 + 124., 42., 168., 112.),
        viewport,
    )?;
    window.update(cx, |s, _, cx| s.fixture_browser_menu(false, cx))?;
    cx.update(|cx| {
        appearance::set_mode(appearance::AppearanceMode::Dark, cx);
        appearance::set_surface(zeron_theme::SurfacePreference::Frosted, cx);
    });
    pause(cx, 300).await;
    let frames = eval(&page, "window.browserFrames", cx)
        .await?
        .as_u64()
        .unwrap();
    anyhow::ensure!(frames > 60, "page stopped rendering during interaction");
    std::fs::write(
        output.join("linux-results.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"keyboard":true,"unicodeClipboard":true,"imeComposition":true,"buttonAndScroll":true,"menuClickIsolation":true,"hoverVisibility":true,"resizeWidths":widths,"cssViewport":width,"liveFrames":frames,"sidebarTransitions":true}),
        )?,
    )?;
    Ok(())
}
