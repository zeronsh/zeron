//! Offscreen pictures of the app's screens and the gallery's, for the
//! gallery-vs-app visual audit: each mounted headless (`TestApp`) and drawn
//! by zgx's own GPU renderer into a texture, read back as a PNG. No window
//! opens, so nothing covers the screen and nothing needs Screen Recording.
//!
//! Run on purpose (it's ignored), one shot list at a time:
//!
//! ```sh
//! AUDIT_OUT=/path/to/dir AUDIT_SHOTS=shell,gallery:sidebar \
//!   cargo test --bin zeron-zgx audit_shots -- --ignored --nocapture
//! ```
//!
//! A shot is `<fixture>` (ui/<fixture>_fixture.luau, or a name ending
//! .luau) or `gallery:<page>`, optionally `@<w>x<h>`, `#<target>` (crop to
//! a node's box) and `!<command>` / `=<state>:<json>` / `^<target>` (a click) / `+<ms>` (a wait)
//! steps, joined by `|`:
//! `gallery:composer|#gallery-demo@1240x820`. `AUDIT_SCALE` (2) is the
//! pixels a point; `AUDIT_DARK=1` the dark scheme where the screen has one.
//! What's transparent (the window's glass) is laid on `AUDIT_BACKDROP`
//! (`#e9e9ee` light, `#1d1d22` dark), the same under both. `AUDIT_WAIT_MS`
//! lets real time pass before the picture, for what comes over the network.
//!
//! Plugins too: `ZERON_ZGX_DEV_PLUGINS` loads plugin folders into each shot
//! (lent the design system, as the app lends it), and `ZERON_ZGX_DEV_GITHUB`
//! plays GitHub from a JSON file (src/plugins `dev_plugins`): GitHub's
//! viewer and board in ui/github_viewer_fixture.luau, github_board_fixture.luau.

use std::path::{Path, PathBuf};

use zgx_gpu::GpuRenderer;
use zgx_script::{ScriptHost, testing::TestApp};

struct Shot {
    name: String,
    script: PathBuf,
    size: (f32, f32),
    crop: Option<String>,
    steps: Vec<Step>,
}

enum Step {
    // Real time passing, frames drawn (`+<ms>`): what comes over the network.
    Wait(u64),
    Command(String),
    Click(String),
    // A key pressed (`>Tab`, `>Escape`): keyboard modality, as a person's
    // key would set it (focus rings show after one).
    Key(String),
    // The pointer resting on a target (`*target`): its hover state.
    Hover(String),
    State(String, serde_json::Value),
}

fn ui_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn parse(spec: &str, dark: bool) -> Shot {
    let mut parts = spec.split('|');
    let head = parts.next().unwrap_or_default();
    let (head, size) = match head.split_once('@') {
        Some((h, s)) => {
            let (w, hgt) = s.split_once('x').expect("a size is <w>x<h>");
            (h, (w.parse().expect("width"), hgt.parse().expect("height")))
        }
        None => (head, (1240., 820.)),
    };
    let mut steps = Vec::new();
    let mut crop = None;
    let (name, script) = if let Some(page) = head.strip_prefix("gallery:") {
        steps.push(Step::State("dark".into(), serde_json::json!(dark)));
        steps.push(Step::State("page".into(), serde_json::json!(page)));
        (format!("gallery-{page}"), ui_dir().join("gallery.luau"))
    } else if head.ends_with(".luau") {
        (head.trim_end_matches(".luau").replace('/', "-"), ui_dir().join(head))
    } else {
        (head.to_string(), ui_dir().join(format!("{head}_fixture.luau")))
    };
    let mut name = name;
    for part in parts {
        if let Some(target) = part.strip_prefix('#') {
            crop = Some(format!("#{target}"));
        } else if let Some(target) = part.strip_prefix('^') {
            steps.push(Step::Click(target.to_string()));
        } else if let Some(key) = part.strip_prefix('>') {
            steps.push(Step::Key(key.to_string()));
        } else if let Some(target) = part.strip_prefix('*') {
            steps.push(Step::Hover(target.to_string()));
        } else if let Some(command) = part.strip_prefix('!') {
            steps.push(Step::Command(command.to_string()));
        } else if let Some(rest) = part.strip_prefix('=') {
            let (state, json) = rest.split_once(':').expect("=<state>:<json>");
            steps.push(Step::State(state.to_string(), serde_json::from_str(json).expect("a state's JSON")));
        } else if let Some(ms) = part.strip_prefix('+') {
            steps.push(Step::Wait(ms.parse().expect("+<milliseconds>")));
        } else if let Some(label) = part.strip_prefix('~') {
            name = format!("{name}-{label}");
        }
    }
    Shot { name, script, size, crop, steps }
}

fn hex(s: &str) -> [u8; 3] {
    let s = s.trim_start_matches('#');
    let v = u32::from_str_radix(s, 16).expect("a #rrggbb backdrop");
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Draws `shot` and writes `<out>/<name>.png`.
fn take(shot: &Shot, out: &Path, scale: f32, dark: bool) -> Result<PathBuf, String> {
    let script = shot.script.clone();
    let app = TestApp::try_new(|runtime| {
        let host = crate::plugins::lend_design_system(ScriptHost::new(runtime, script.clone()), &script);
        // (What a window host answers and a test host doesn't.)
        host.action("appearance", |_| {});
        host.action("close", |_| {});
        // (The network, as every window has it: remote pictures.)
        zgx_run::install_fetcher(&host);
        crate::plugins::dev_plugins(&host);
        // (Agent visuals, run as a window runs them: ui/visuals_fixture.)
        crate::visuals::register(&host);
        host.shared("zgx.window.size", zgx_script::window_size_value(shot.size.0, shot.size.1));
        host
    })
    .map_err(|e| format!("{}: {e}", shot.name))?;
    let (w, h) = shot.size;
    app.ui.scene.borrow_mut().resize(w, h);
    let mut gpu = GpuRenderer::new((w * scale).round() as u32, (h * scale).round() as u32).map_err(|e| format!("GPU: {e:?}"))?;
    gpu.set_scale_factor(scale);
    for font in crate::fonts() {
        font.map_err(|e| e.to_string())?.install(&mut gpu.text_system().borrow_mut());
    }
    // The text's language, as a window takes it from the locale shown
    // (`AUDIT_TEXT_LANGUAGE=zh-Hans`): which faces draw Han characters.
    if let Ok(language) = std::env::var("AUDIT_TEXT_LANGUAGE") {
        gpu.text_system().borrow_mut().set_language(Some(&language));
    }
    gpu.install_text(&mut app.ui.scene.borrow_mut());
    app.ui.refresh_text_geometry();
    app.frames(10);
    let mut app = app;
    // Real time for what comes over the network, before the steps (a page
    // to click on must have arrived) and again after them.
    let wait = |app: &mut TestApp| {
        if let Some(ms) = std::env::var("AUDIT_WAIT_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(50));
                app.frames(1);
            }
            app.frames(10);
        }
    };
    wait(&mut app);
    for step in &shot.steps {
        match step {
            Step::Wait(ms) => {
                let end = std::time::Instant::now() + std::time::Duration::from_millis(*ms);
                while std::time::Instant::now() < end {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    app.frames(1);
                }
            }
            Step::Click(target) => {
                app.target(target).map_err(|e| format!("{}: click {target}: {e}", shot.name))?;
                app.click(target);
            }
            Step::Key(key) => {
                app.keys(key);
            }
            Step::Hover(target) => {
                app.target(target).map_err(|e| format!("{}: hover {target}: {e}", shot.name))?;
                app.hover(target);
            }
            Step::Command(id) => {
                if !app.host.run_command(id) {
                    return Err(format!("{}: no command `{id}`", shot.name));
                }
            }
            Step::State(name, value) => {
                app.host
                    .set_state(name, crate::plugins::json_value(value))
                    .map_err(|e| format!("{}: {name}: {e}", shot.name))?;
            }
        }
        app.settle();
        app.frames(10);
    }
    // Long enough for entrances and springs to finish.
    app.frames(90);
    wait(&mut app);
    if let Some(error) = app.error() {
        eprintln!("audit_shots: {}: script error: {error}", shot.name);
    }
    app.ui.prepare_frame();
    let report = app.ui.scene.borrow_mut().flush();
    // (Everything, not only what changed: the texture starts empty.)
    let _ = report;
    let all = [zgx::scene::Rect { x: 0., y: 0., width: w, height: h }];
    gpu.render(&app.ui.scene.borrow(), &all).map_err(|e| format!("render: {e:?}"))?;
    let mut rgba = gpu.readback().map_err(|e| format!("readback: {e:?}"))?;
    let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);

    // The glass, laid on one backdrop (premultiplied over it).
    let back = hex(&std::env::var("AUDIT_BACKDROP").unwrap_or_else(|_| if dark { "#1d1d22" } else { "#e9e9ee" }.into()));
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        for c in 0..3 {
            px[c] = (px[c] as u32 + back[c] as u32 * (255 - a) / 255).min(255) as u8;
        }
        px[3] = 255;
    }

    // Cropped to a node's box.
    let (mut x0, mut y0, mut cw, mut ch) = (0u32, 0u32, pw, ph);
    if let Some(target) = &shot.crop {
        let node = app.target(target).map_err(|e| format!("{}: crop {target}: {e}", shot.name))?;
        let b = app.ui.scene.borrow().bounds(node);
        x0 = ((b.x * scale).max(0.)) as u32;
        y0 = ((b.y * scale).max(0.)) as u32;
        cw = ((b.width * scale) as u32).min(pw - x0);
        ch = ((b.height * scale) as u32).min(ph - y0);
    }
    let mut cropped = Vec::with_capacity((cw * ch * 4) as usize);
    for row in y0..y0 + ch {
        let start = ((row * pw + x0) * 4) as usize;
        cropped.extend_from_slice(&rgba[start..start + (cw * 4) as usize]);
    }
    let path = out.join(format!("{}.png", shot.name));
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), cw, ch);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&cropped).map_err(|e| e.to_string())?;
    Ok(path)
}

#[test]
#[ignore = "run on purpose: writes pictures for the visual audit"]
fn audit_shots() {
    let out = PathBuf::from(std::env::var("AUDIT_OUT").expect("AUDIT_OUT: where the pictures go"));
    std::fs::create_dir_all(&out).unwrap();
    let scale: f32 = std::env::var("AUDIT_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(2.);
    // Dark unless asked otherwise: the fixtures follow this Mac's (dark).
    let dark = std::env::var_os("AUDIT_LIGHT").is_none();
    let shots = std::env::var("AUDIT_SHOTS").expect("AUDIT_SHOTS: the shots, comma-separated");
    let mut failed = Vec::new();
    // (Shots apart at commas outside quotes: a target may hold one.)
    let mut specs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in shots.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => specs.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    specs.push(current);
    for spec in specs.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let shot = parse(spec, dark);
        match take(&shot, &out, scale, dark) {
            Ok(path) => println!("audit_shots: {}", path.display()),
            Err(error) => {
                println!("audit_shots: FAILED {error}");
                failed.push(error);
            }
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
