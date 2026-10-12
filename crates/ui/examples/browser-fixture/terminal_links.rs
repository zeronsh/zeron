//! Native terminal link coverage: synthetic PTY-less output, real GPUI input
//! dispatch, and recorded (never launched) link activations.
use super::transcript_links::screenshot;
use super::*;
use gpui::{Entity, Modifiers, MouseButton, Pixels, PlatformInput, Point, WindowHandle};
use zeron_ui::terminal::panel::TerminalPanel;

/// Dispatch through the untyped handle: the typed one keeps `Shell` leased,
/// and Shell's own modifier listener must be able to update it.
fn dispatch(
    window: WindowHandle<shell::Shell>,
    event: PlatformInput,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return super::linux::dispatch(window, event, cx);
    #[cfg(not(target_os = "linux"))]
    {
        gpui::AnyWindowHandle::from(window).update(cx, |_, w, cx| {
            w.dispatch_event(event, cx);
        })?;
        Ok(())
    }
}

const LOCAL_URL: &str = "http://localhost:5173/";
const DOCS_URL: &str = "https://vite.dev/guide/";

/// Shell-and-dev-server output: one plain-text URL, one OSC 8 hyperlink.
fn output() -> Vec<u8> {
    format!(
        "\x1b[32m~/fieldnotes\x1b[0m $ npm run dev\r\n\r\n  \x1b[1;32mVITE\x1b[0m v6.0.0  ready in 312 ms\r\n\r\n  \x1b[32m➜\x1b[0m  Local:   \x1b[36m{LOCAL_URL}\x1b[0m\r\n  \x1b[32m➜\x1b[0m  Docs:    \x1b]8;;{DOCS_URL}\x1b\\\x1b[36mvite.dev/guide\x1b[0m\x1b]8;;\x1b\\\r\n  \x1b[2m➜  press h + enter to show help\x1b[0m\r\n"
    )
    .into_bytes()
}

fn link_modifiers() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            platform: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            control: true,
            ..Default::default()
        }
    }
}

/// Window position of the first cell of `needle` in the active tab.
fn locate(
    panel: &Entity<TerminalPanel>,
    needle: &str,
    cx: &mut AsyncApp,
) -> anyhow::Result<Point<Pixels>> {
    panel
        .read_with(cx, |panel, cx| {
            (0..40).find_map(|row| {
                let text = panel.fixture_row_text(row, cx)?;
                let col = text.find(needle).map(|byte| text[..byte].chars().count())?;
                panel.fixture_cell_position(row, col)
            })
        })
        .ok_or_else(|| anyhow::anyhow!("{needle:?} was not painted in the terminal"))
}

fn mouse_move(
    window: WindowHandle<shell::Shell>,
    position: Point<Pixels>,
    modifiers: Modifiers,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    dispatch(
        window,
        PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        }),
        cx,
    )
}

fn modifiers_changed(
    window: WindowHandle<shell::Shell>,
    modifiers: Modifiers,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    dispatch(
        window,
        PlatformInput::ModifiersChanged(gpui::ModifiersChangedEvent {
            modifiers,
            capslock: Default::default(),
        }),
        cx,
    )
}

async fn click(
    window: WindowHandle<shell::Shell>,
    position: Point<Pixels>,
    modifiers: Modifiers,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    mouse_move(window, position, modifiers, cx)?;
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    )?;
    pause(cx, 60).await;
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
        }),
        cx,
    )?;
    pause(cx, 120).await;
    Ok(())
}

fn highlighted(panel: &Entity<TerminalPanel>, cx: &mut AsyncApp) -> bool {
    panel.read_with(cx, |panel, cx| panel.fixture_link_highlighted(cx))
}

fn opened(panel: &Entity<TerminalPanel>, cx: &mut AsyncApp) -> Vec<String> {
    panel.update(cx, |panel, _| panel.fixture_take_opened_links())
}

pub(super) async fn exercise(
    window: WindowHandle<shell::Shell>,
    output_dir: &std::path::Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let panel = window.update(cx, |shell, w, cx| shell.fixture_open_terminal(w, cx))?;
    panel.update(cx, |panel, cx| panel.fixture_seed_tab("zsh", &output(), cx));
    pause(cx, 800).await;
    let local = locate(&panel, "http://", cx)?;
    let docs = locate(&panel, "vite.dev/guide", cx)?;
    let plain_text = locate(&panel, "ready in", cx)?;
    let none = Modifiers::default();

    for (mode, mode_name) in [
        (appearance::AppearanceMode::Dark, "dark"),
        (appearance::AppearanceMode::Light, "light"),
    ] {
        for (surface, surface_name) in [
            (zeron_theme::SurfacePreference::Frosted, "frosted"),
            (zeron_theme::SurfacePreference::Opaque, "opaque"),
        ] {
            cx.update(|cx| {
                appearance::set_mode(mode, cx);
                appearance::set_surface(surface, cx);
            });
            pause(cx, 700).await;
            // Before: hovering without the modifier leaves the URL as plain output.
            mouse_move(window, local, none, cx)?;
            pause(cx, 150).await;
            anyhow::ensure!(
                !highlighted(&panel, cx),
                "link highlighted without the modifier"
            );
            screenshot(
                window,
                output_dir,
                &format!("terminal-link-{mode_name}-{surface_name}-idle"),
                cx,
            )
            .await?;
            // After: pressing the modifier over the URL underlines it in place.
            modifiers_changed(window, link_modifiers(), cx)?;
            pause(cx, 150).await;
            anyhow::ensure!(
                highlighted(&panel, cx),
                "modifier press did not highlight the hovered link"
            );
            screenshot(
                window,
                output_dir,
                &format!("terminal-link-{mode_name}-{surface_name}-hover"),
                cx,
            )
            .await?;
            modifiers_changed(window, none, cx)?;
            pause(cx, 100).await;
            anyhow::ensure!(
                !highlighted(&panel, cx),
                "modifier release left the link highlighted"
            );
        }
    }

    // Modifier-click opens the plain-text URL, exactly as printed.
    click(window, local, link_modifiers(), cx).await?;
    anyhow::ensure!(
        opened(&panel, cx) == [LOCAL_URL],
        "modifier-click did not open the plain-text URL"
    );
    // Modifier-click on an OSC 8 label opens its target, not the label text.
    click(window, docs, link_modifiers(), cx).await?;
    anyhow::ensure!(
        opened(&panel, cx) == [DOCS_URL],
        "modifier-click did not open the OSC 8 target"
    );
    // A plain click on a URL stays a selection gesture.
    click(window, local, none, cx).await?;
    anyhow::ensure!(opened(&panel, cx).is_empty(), "plain click opened a link");
    // Modifier-click on ordinary output opens nothing.
    click(window, plain_text, link_modifiers(), cx).await?;
    anyhow::ensure!(
        opened(&panel, cx).is_empty(),
        "modifier-click on plain text opened something"
    );
    mouse_move(window, plain_text, link_modifiers(), cx)?;
    pause(cx, 100).await;
    anyhow::ensure!(!highlighted(&panel, cx), "plain text highlighted as a link");
    // Drag-select across a URL still selects and opens nothing.
    dispatch(
        window,
        PlatformInput::MouseDown(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position: local,
            modifiers: none,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    )?;
    for step in 1..=6 {
        let position = gpui::point(local.x + px(12. * step as f32), local.y);
        dispatch(
            window,
            PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                modifiers: none,
            }),
            cx,
        )?;
        pause(cx, 30).await;
    }
    dispatch(
        window,
        PlatformInput::MouseUp(gpui::MouseUpEvent {
            button: MouseButton::Left,
            position: local,
            modifiers: none,
            click_count: 1,
        }),
        cx,
    )?;
    pause(cx, 120).await;
    anyhow::ensure!(
        panel.read_with(cx, |panel, cx| panel.fixture_has_selection(cx)),
        "drag across a URL did not select"
    );
    anyhow::ensure!(opened(&panel, cx).is_empty(), "drag-select opened a link");
    // OSC 8 targets outside the allowed schemes are inert.
    panel.update(cx, |panel, cx| {
        panel.fixture_seed_tab(
            "zsh",
            b"\x1b]8;;javascript:alert(1)\x1b\\run this\x1b]8;;\x1b\\\r\n",
            cx,
        )
    });
    pause(cx, 400).await;
    let unsafe_link = locate(&panel, "run this", cx)?;
    mouse_move(window, unsafe_link, link_modifiers(), cx)?;
    pause(cx, 100).await;
    anyhow::ensure!(!highlighted(&panel, cx), "javascript: link highlighted");
    click(window, unsafe_link, link_modifiers(), cx).await?;
    anyhow::ensure!(opened(&panel, cx).is_empty(), "javascript: link was opened");

    std::fs::write(
        output_dir.join("result.txt"),
        "PASS: modifier hover highlight/clear, modifier-click opens plain-text URL and OSC 8 target, plain click and drag-select open nothing, plain text and javascript: targets inert. Captures: light/dark x frosted/opaque, idle and modifier-hover.\n",
    )?;
    Ok(())
}
