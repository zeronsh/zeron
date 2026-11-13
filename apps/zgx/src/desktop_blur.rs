//! How much the desktop blurs behind the window's glass, where the app can
//! tell: `ui.shared("window.blur")`, in points (a gaussian's standard
//! deviation), for ds/material to tint the glass to suit it. Nil where it
//! can't tell (macOS, Windows, a Linux desktop other than Hyprland, a window
//! Hyprland doesn't show): the glass is then the theme's own.
//!
//! On macOS the glass is a 60-point blur. On Linux zgx asks the compositor
//! for blur behind the window, and how much is the compositor's: Hyprland's
//! is its `decoration:blur` (none at all with blur off, Omarchy's default,
//! or with `no_blur` on the window). Hyprland is asked over its socket, and
//! again when its config reloads or a window opens or moves.

use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

/// Keep `window.blur` current for the window `host` serves.
pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let shared = host.shared("window.blur", Value::Nil);
    #[cfg(target_os = "linux")]
    {
        let Some(dir) = hyprland::socket_dir() else {
            return;
        };
        let (tell, mut heard) = zgx::task::latest::<Option<f64>>();
        if std::thread::Builder::new()
            .name("zeron-desktop-blur".into())
            .spawn(move || hyprland::follow(&dir, tell))
            .is_err()
        {
            return;
        }
        cx.tasks.spawn(async move {
            while let Some(blur) = heard.recv().await {
                shared.set(blur.map_or(Value::Nil, Value::Num));
            }
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (cx, shared);
}

/// Hyprland's dual-Kawase blur as a gaussian's standard deviation, in the
/// monitor's pixels: `size` the offset of each pass, `passes` halvings (1 to
/// 8, as Hyprland clamps them). Measured on Hyprland 0.57 from a step edge
/// behind a window (its 10 to 90% rise is 2.56 deviations): 4×2 is 8.6 px,
/// 8×3 35, 6×4 54 and 8×4 70, each 0.55 of `size × 2^passes`.
pub fn kawase_deviation(size: i64, passes: i64) -> f64 {
    if size <= 0 {
        return 0.;
    }
    0.55 * size as f64 * f64::from(1u32 << passes.clamp(1, 8))
}

#[cfg(target_os = "linux")]
mod hyprland {
    use serde_json::Value as Json;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// Hyprland's sockets for this session, if it runs one.
    pub fn socket_dir() -> Option<PathBuf> {
        let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        let dir = PathBuf::from(runtime).join("hypr").join(signature);
        dir.join(".socket.sock").exists().then_some(dir)
    }

    /// Tell the blur now, then again as Hyprland's events may change it,
    /// until the window is gone (no one hears) or Hyprland is.
    pub fn follow(dir: &Path, tell: zgx::task::LatestSender<Option<f64>>) {
        if !tell.send(blur(dir)) {
            return;
        }
        let Ok(events) = UnixStream::connect(dir.join(".socket2.sock")) else {
            return;
        };
        let mut lines = BufReader::new(events);
        let mut line = String::new();
        loop {
            line.clear();
            match lines.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if !matters(&line) {
                continue;
            }
            // A burst (a reload, a window opening) is asked about once.
            let _ = lines
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(150)));
            loop {
                line.clear();
                match lines.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            let _ = lines.get_ref().set_read_timeout(None);
            if !tell.send(blur(dir)) {
                return;
            }
        }
    }

    /// Whether Hyprland's event `line` may change the blur behind the window:
    /// its config (re)loaded, a window opened or moved (to a monitor of
    /// another scale), a monitor came or went.
    pub fn matters(line: &str) -> bool {
        let name = line.split(">>").next().unwrap_or("");
        matches!(
            name,
            "configreloaded"
                | "openwindow"
                | "movewindow"
                | "movewindowv2"
                | "moveworkspace"
                | "moveworkspacev2"
                | "monitoradded"
                | "monitoraddedv2"
                | "monitorremoved"
                | "monitorremovedv2"
        )
    }

    fn ask(dir: &Path, request: &str) -> Option<Json> {
        let mut socket = UnixStream::connect(dir.join(".socket.sock")).ok()?;
        socket.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
        socket
            .set_write_timeout(Some(Duration::from_secs(1)))
            .ok()?;
        socket.write_all(request.as_bytes()).ok()?;
        let mut answer = String::new();
        socket.read_to_string(&mut answer).ok()?;
        serde_json::from_str(&answer).ok()
    }

    /// An option's number (a bool's 0 or 1; older Hyprland says `int`).
    fn option(dir: &Path, name: &str) -> Option<i64> {
        let answer = ask(dir, &format!("j/getoption {name}"))?;
        answer["int"]
            .as_i64()
            .or_else(|| answer["bool"].as_bool().map(i64::from))
    }

    /// The blur behind this process's windows, in points; none where
    /// Hyprland shows none of them.
    fn blur(dir: &Path) -> Option<f64> {
        let pid = i64::from(std::process::id());
        let clients = ask(dir, "j/clients")?;
        let ours: Vec<&Json> = clients
            .as_array()?
            .iter()
            .filter(|c| c["pid"].as_i64() == Some(pid))
            .collect();
        let first = ours.first()?;
        let monitors = ask(dir, "j/monitors")?;
        let scale = (monitors.as_array()?.iter())
            .find(|m| m["id"].as_i64() == first["monitor"].as_i64())
            .and_then(|m| m["scale"].as_f64())
            .filter(|s| *s > 0.)
            .unwrap_or(1.);
        let unblurred = ours.iter().any(|c| {
            c["address"].as_str().is_some_and(|address| {
                ask(dir, &format!("j/getprop address:{address} no_blur"))
                    .is_some_and(|p| p["no_blur"].as_bool() == Some(true))
            })
        });
        let enabled = option(dir, "decoration:blur:enabled")? != 0;
        let deviation = super::kawase_deviation(
            option(dir, "decoration:blur:size")?,
            option(dir, "decoration:blur:passes")?,
        );
        Some(if enabled && !unblurred {
            deviation / scale
        } else {
            0.
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kawase_deviation_follows_the_measured_blurs() {
        // Omarchy's Glassy (4×2) and a macOS-like 8×4, as measured.
        assert!((kawase_deviation(4, 2) - 8.8).abs() < 0.5);
        assert!((kawase_deviation(8, 4) - 70.4).abs() < 1.);
        assert_eq!(kawase_deviation(0, 4), 0.);
        // Hyprland clamps passes to 1..=8.
        assert_eq!(kawase_deviation(4, 0), kawase_deviation(4, 1));
        assert_eq!(kawase_deviation(4, 12), kawase_deviation(4, 8));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_events_that_may_change_the_blur_are_asked_about() {
        assert!(hyprland::matters("configreloaded>>\n"));
        assert!(hyprland::matters("movewindowv2>>55aa,3,3\n"));
        assert!(hyprland::matters("openwindow>>55aa,3,zeron-zgx,Zeron\n"));
        assert!(!hyprland::matters("activewindow>>zeron-zgx,Zeron\n"));
        assert!(!hyprland::matters("workspacev2>>3,3\n"));
    }
}
